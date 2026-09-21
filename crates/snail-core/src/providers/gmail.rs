//! Gmail-over-REST: the pieces that are pure logic and worth testing without a network (E4.4–E4.9).
//!
//! The live calls live behind `MailProvider`; here are the URL shapes, the history parsing, the
//! cursor discipline, and the error classification that decide what the syncer does next.

use serde_json::Value;

/// Google silently disables compression without the literal string `gzip` in the User-Agent (E4.9).
pub const USER_AGENT: &str = "snail/0.1 (gzip) (gmail)";

const BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

pub fn profile_url() -> String {
    format!("{BASE}/profile")
}

/// One **unfiltered** history stream per account: never pass `labelId` (E4.5).
pub fn history_url(start_history_id: u64, page_token: Option<&str>) -> String {
    let mut url = format!("{BASE}/history?startHistoryId={start_history_id}&maxResults=500");
    if let Some(token) = page_token {
        url.push_str("&pageToken=");
        url.push_str(token);
    }
    url
}

/// `threads.get?format=raw`: 40 units per thread beats 20 units per message at 3+ messages (E4.3).
pub fn thread_raw_url(thread_id: &str) -> String {
    format!("{BASE}/threads/{thread_id}?format=raw")
}

pub fn message_raw_url(message_id: &str) -> String {
    format!("{BASE}/messages/{message_id}?format=raw")
}

pub fn modify_url(message_id: &str) -> String {
    format!("{BASE}/messages/{message_id}/modify")
}

pub fn send_url() -> String {
    format!("{BASE}/messages/send")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Added(String),
    Deleted(String),
    LabelsAdded { id: String, labels: Vec<String> },
    LabelsRemoved { id: String, labels: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRecord {
    pub history_id: u64,
    pub kind: RecordKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub records: Vec<HistoryRecord>,
    /// `max(History.id)` across the records in **this** page, when there are any.
    pub page_max_id: Option<u64>,
    /// The mailbox head at request time — only a cursor when `history[]` came back empty (E4.4).
    pub top_history_id: Option<u64>,
    pub next_page_token: Option<String>,
}

/// Parse a `history.list` response, keeping the record ids needed for the cursor discipline.
pub fn parse_history(json: &Value) -> HistoryPage {
    let mut records = Vec::new();
    let mut page_max_id: Option<u64> = None;

    for entry in json
        .get("history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let history_id = entry
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<u64>().ok())
            .unwrap_or(0);
        page_max_id = Some(page_max_id.map_or(history_id, |current: u64| current.max(history_id)));

        let mut push = |kind: RecordKind| {
            records.push(HistoryRecord {
                history_id,
                kind,
            })
        };
        for message in array(&entry, "messages") {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                push(RecordKind::Added(id.to_string()));
            }
        }
        // `messagesDeleted` is a permanent expunge only — trash arrives as a TRASH label change.
        for message in array(&entry, "messagesDeleted") {
            if let Some(id) = message.get("message").and_then(|m| m.get("id")).and_then(Value::as_str) {
                push(RecordKind::Deleted(id.to_string()));
            }
        }
        for message in array(&entry, "labelsAdded") {
            let id = message.get("message").and_then(|m| m.get("id")).and_then(Value::as_str).unwrap_or("");
            let labels = message.get("labelIds").and_then(Value::as_array).map(|values| strings(values)).unwrap_or_default();
            push(RecordKind::LabelsAdded { id: id.to_string(), labels });
        }
        for message in array(&entry, "labelsRemoved") {
            let id = message.get("message").and_then(|m| m.get("id")).and_then(Value::as_str).unwrap_or("");
            let labels = message.get("labelIds").and_then(Value::as_array).map(|values| strings(values)).unwrap_or_default();
            push(RecordKind::LabelsRemoved { id: id.to_string(), labels });
        }
    }

    HistoryPage {
        records,
        page_max_id,
        top_history_id: json
            .get("historyId")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<u64>().ok()),
        next_page_token: json
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The cursor to commit, and only once the whole chain has been paged (E4.4): prefer the highest
/// `History.id` actually processed; fall back to the top-level `historyId` only when there were no
/// records at all. Committing the top-level id mid-pagination loses changes.
pub fn committed_cursor(page_max_id: Option<u64>, top_history_id: Option<u64>) -> Option<u64> {
    match page_max_id {
        Some(0) | None => top_history_id,
        Some(id) => Some(id),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// 404 from `history.list`, or a 404 mid-pagination: the window slid. Abandon and resync (E4.6).
    HistoryExpired,
    /// Retry with backoff (403 usageLimits, 429, 5xx).
    Retryable,
    /// 403 `domainPolicy`: a Workspace admin blocked the app — terminal, needs its own message (E4.8).
    WorkspaceBlocked,
    /// 401: the token is no longer accepted.
    Auth,
    Other,
}

pub fn classify(status: u16, reason: &str, domain: &str) -> ApiError {
    match status {
        404 => ApiError::HistoryExpired,
        401 => ApiError::Auth,
        429 => ApiError::Retryable,
        403 => match domain {
            "domainPolicy" => ApiError::WorkspaceBlocked,
            "usageLimits" => ApiError::Retryable,
            _ => match reason {
                "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded" => ApiError::Retryable,
                _ => ApiError::Other,
            },
        },
        500..=599 => ApiError::Retryable,
        _ => ApiError::Other,
    }
}

fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn strings(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_history_stream_is_never_filtered_by_label() {
        // E4.5: a filtered cursor goes stale, then expires.
        let url = history_url(1234, None);
        assert!(!url.contains("labelId"), "{url}");
        assert!(url.contains("startHistoryId=1234"));
        assert!(history_url(1, Some("TOKEN")).contains("pageToken=TOKEN"));
    }

    #[test]
    fn raw_fetches_prefer_threads() {
        assert!(thread_raw_url("t1").contains("format=raw"));
        assert!(message_raw_url("m1").contains("format=raw"));
    }

    #[test]
    fn the_user_agent_carries_the_literal_gzip() {
        assert!(USER_AGENT.contains("gzip"));
    }

    #[test]
    fn history_parses_every_record_type() {
        let page = parse_history(&json!({
            "history": [
                {"id": "100", "messages": [{"id": "m1"}]},
                {"id": "101", "messagesDeleted": [{"message": {"id": "m2"}}]},
                {"id": "105", "labelsAdded": [{"message": {"id": "m3"}, "labelIds": ["STARRED"]}]},
                {"id": "106", "labelsRemoved": [{"message": {"id": "m3"}, "labelIds": ["UNREAD"]}]}
            ],
            "historyId": "107",
            "nextPageToken": "NEXT"
        }));
        assert_eq!(page.records.len(), 4);
        assert_eq!(page.page_max_id, Some(106));
        assert_eq!(page.top_history_id, Some(107));
        assert_eq!(page.next_page_token.as_deref(), Some("NEXT"));
        assert!(matches!(&page.records[0].kind, RecordKind::Added(id) if id == "m1"));
        assert!(matches!(&page.records[1].kind, RecordKind::Deleted(id) if id == "m2"));
        assert!(matches!(&page.records[2].kind, RecordKind::LabelsAdded { labels, .. } if labels == &["STARRED"]));
    }

    #[test]
    fn the_cursor_prefers_the_last_record_processed() {
        // Processed pages up to history id 106; commit 106, not the top-level head 107.
        assert_eq!(committed_cursor(Some(106), Some(107)), Some(106));
        // No records at all: the top-level id is the only cursor.
        assert_eq!(committed_cursor(None, Some(107)), Some(107));
        assert_eq!(committed_cursor(Some(0), Some(107)), Some(107));
        assert_eq!(committed_cursor(None, None), None);
    }

    #[test]
    fn errors_classify_into_the_right_recovery() {
        assert_eq!(classify(404, "notFound", ""), ApiError::HistoryExpired);
        assert_eq!(classify(403, "rateLimitExceeded", "usageLimits"), ApiError::Retryable);
        assert_eq!(classify(429, "", ""), ApiError::Retryable);
        assert_eq!(classify(503, "", ""), ApiError::Retryable);
        assert_eq!(classify(403, "", "domainPolicy"), ApiError::WorkspaceBlocked);
        assert_eq!(classify(401, "", ""), ApiError::Auth);
        assert_eq!(classify(400, "badRequest", ""), ApiError::Other);
    }
}
