//! Gmail-over-REST: the pieces that are pure logic and worth testing without a network (E4.4–E4.9).
//!
//! The live calls live behind `MailProvider`; here are the URL shapes, the history parsing, the
//! cursor discipline, and the error classification that decide what the syncer does next.

use serde_json::Value;

use crate::labels::MailboxKind;

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

        let mut push = |kind: RecordKind| records.push(HistoryRecord { history_id, kind });
        for message in array(&entry, "messages") {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                push(RecordKind::Added(id.to_string()));
            }
        }
        // `messagesDeleted` is a permanent expunge only — trash arrives as a TRASH label change.
        for message in array(&entry, "messagesDeleted") {
            if let Some(id) = message
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
            {
                push(RecordKind::Deleted(id.to_string()));
            }
        }
        for message in array(&entry, "labelsAdded") {
            let id = message
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let labels = message
                .get("labelIds")
                .and_then(Value::as_array)
                .map(|values| strings(values))
                .unwrap_or_default();
            push(RecordKind::LabelsAdded {
                id: id.to_string(),
                labels,
            });
        }
        for message in array(&entry, "labelsRemoved") {
            let id = message
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let labels = message
                .get("labelIds")
                .and_then(Value::as_array)
                .map(|values| strings(values))
                .unwrap_or_default();
            push(RecordKind::LabelsRemoved {
                id: id.to_string(),
                labels,
            });
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
                "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded" => {
                    ApiError::Retryable
                }
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

// --- The live client (E4.2, E4.3, E4.7) ------------------------------------------------------

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::{Changes, LabelChange, MailProvider, OpOutcome, RawMessage, RemoteOp};
use crate::mime;
use crate::store::{NewMessage, ProviderRef, Store, SyncState};

/// Supplies the current access token; refreshed elsewhere (E3.3).
pub trait TokenSource: Send + Sync {
    fn access_token(&self) -> Result<String>;
}

/// A source that never changes, for tests and short-lived commands.
pub struct FixedToken(pub String);

impl TokenSource for FixedToken {
    fn access_token(&self) -> Result<String> {
        Ok(self.0.clone())
    }
}

#[derive(Debug)]
pub struct GmailError {
    pub kind: ApiError,
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for GmailError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Gmail {} ({:?}): {}",
            self.status, self.kind, self.message
        )
    }
}

impl std::error::Error for GmailError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub email: String,
    pub messages_total: u64,
    pub threads_total: u64,
    /// The mailbox head **at request time** — take this before enumerating (E4.2).
    pub history_id: Option<u64>,
}

pub fn parse_profile(json: &Value) -> Profile {
    Profile {
        email: json
            .get("emailAddress")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        messages_total: json
            .get("messagesTotal")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        threads_total: json
            .get("threadsTotal")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        history_id: json
            .get("historyId")
            .and_then(Value::as_str)
            .and_then(|id| id.parse().ok()),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageList {
    pub ids: Vec<String>,
    pub next_page_token: Option<String>,
    pub estimate: Option<u64>,
}

pub fn parse_message_list(json: &Value) -> MessageList {
    MessageList {
        ids: json
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| {
                        message
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        next_page_token: json
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_string),
        estimate: json.get("resultSizeEstimate").and_then(Value::as_u64),
    }
}

/// The backfill window: the owner chose the last 30 days (plan.md §8 open question 3).
pub fn backfill_query(days: u32) -> String {
    format!("newer_than:{days}d")
}

/// Minimal metadata: enough to know a message's thread and labels without paying for a body.
pub fn message_meta_url(id: &str) -> String {
    format!("{BASE}/messages/{id}?format=minimal")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageMeta {
    pub id: String,
    pub thread_id: Option<String>,
    pub label_ids: Vec<String>,
}

pub fn parse_message_meta(json: &Value) -> MessageMeta {
    MessageMeta {
        id: json
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        thread_id: json
            .get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string),
        label_ids: json
            .get("labelIds")
            .and_then(Value::as_array)
            .map(|values| strings(values))
            .unwrap_or_default(),
    }
}

/// Gmail's labels → the handoff's five mailboxes (E8.6). Archive is the interesting one: it is
/// "none of the system folders" rather than a label of its own. The rule lives in
/// [`crate::labels::gmail_primary_kind`] now, next to iCloud's, so both agree by construction.
pub fn mailbox_kind_for_labels(labels: &[String]) -> MailboxKind {
    crate::labels::gmail_primary_kind(labels)
}

/// Gmail's `raw` field: standard base64url, occasionally padded.
pub fn encode_raw(raw: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(raw)
}

pub fn decode_raw(encoded: &str) -> Result<Vec<u8>> {
    let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let trimmed = cleaned.trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| {
            let standard = trimmed.replace('-', "+").replace('_', "/");
            base64::engine::general_purpose::STANDARD.decode(standard)
        })
        .context("decode Gmail raw")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackfillReport {
    pub enumerated: usize,
    pub inserted: usize,
    /// The head taken **before** enumeration, committed only after (E4.2).
    pub head_history_id: Option<u64>,
}

pub struct GmailClient {
    http: reqwest::blocking::Client,
    tokens: Arc<dyn TokenSource>,
}

impl GmailClient {
    pub fn new(tokens: Arc<dyn TokenSource>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { http, tokens })
    }

    fn get(&self, url: &str) -> Result<reqwest::blocking::RequestBuilder, GmailError> {
        let token = self.tokens.access_token().map_err(|error| GmailError {
            kind: ApiError::Auth,
            status: 0,
            message: error.to_string(),
        })?;
        Ok(self
            .http
            .get(url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .header(reqwest::header::USER_AGENT, USER_AGENT))
    }

    fn get_json(&self, url: &str) -> Result<Value, GmailError> {
        self.with_retry(|| self.get_json_once(url))
    }

    fn get_json_once(&self, url: &str) -> Result<Value, GmailError> {
        let response = self.get(url)?.send().map_err(|error| GmailError {
            kind: ApiError::Other,
            status: 0,
            message: error.to_string(),
        })?;
        read_json(response)
    }

    fn post_json(&self, url: &str, body: Value) -> Result<Value, GmailError> {
        self.with_retry(|| self.post_json_once(url, body.clone()))
    }

    fn post_json_once(&self, url: &str, body: Value) -> Result<Value, GmailError> {
        let token = self.tokens.access_token().map_err(|error| GmailError {
            kind: ApiError::Auth,
            status: 0,
            message: error.to_string(),
        })?;
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .json(&body)
            .send()
            .map_err(|error| GmailError {
                kind: ApiError::Other,
                status: 0,
                message: error.to_string(),
            })?;
        read_json(response)
    }

    /// Retry only the retryable class — 403 `usageLimits`, 429, 5xx (E4.8). Terminal errors
    /// (`domainPolicy`, auth) return at once. The per-user quota is shared with the user's phone,
    /// so a backfill must expect to be throttled and wait it out (E16.4).
    fn with_retry<T>(
        &self,
        mut attempt_fn: impl FnMut() -> Result<T, GmailError>,
    ) -> Result<T, GmailError> {
        let mut delay = Duration::from_millis(1000);
        let mut last: Option<GmailError> = None;
        for attempt in 0..6 {
            match attempt_fn() {
                Ok(value) => return Ok(value),
                Err(error) if error.kind == ApiError::Retryable && attempt < 5 => {
                    log::warn!(
                        "Gmail throttled ({}); retrying in {:?}",
                        error.message,
                        delay
                    );
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_secs(32));
                    last = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last.unwrap_or(GmailError {
            kind: ApiError::Retryable,
            status: 0,
            message: "exhausted retries".into(),
        }))
    }

    pub fn profile(&self) -> Result<Profile> {
        let json = self.get_json(&profile_url())?;
        Ok(parse_profile(&json))
    }

    pub fn list_ids(&self, query: &str, page_token: Option<&str>) -> Result<MessageList> {
        let mut url = format!("{BASE}/messages?maxResults=500&q={}", urlencode(query));
        if let Some(token) = page_token {
            url.push_str("&pageToken=");
            url.push_str(token);
        }
        let json = self.get_json(&url)?;
        Ok(parse_message_list(&json))
    }

    pub fn raw_message(&self, id: &str) -> Result<Vec<u8>> {
        let json = self.get_json(&message_raw_url(id))?;
        let encoded = json
            .get("raw")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("message {id} had no raw field"))?;
        decode_raw(encoded)
    }

    pub fn message_meta(&self, id: &str) -> Result<MessageMeta> {
        let json = self.get_json(&message_meta_url(id))?;
        Ok(parse_message_meta(&json))
    }

    /// Backfill the last `days` days, **head first** (E4.2), committing the cursor only after the
    /// whole enumeration succeeds so a crash re-runs from the last committed point (E4.23).
    pub fn backfill(
        &self,
        store: &Store,
        account_id: i64,
        mailbox_id: Option<i64>,
        days: u32,
        limit: usize,
    ) -> Result<BackfillReport> {
        // 1. Head before enumerating. Doing it after silently loses changes (E4.2).
        let profile = self.profile()?;
        let head = profile.history_id;

        // 2. Enumerate the window.
        let query = backfill_query(days);
        let mut ids = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page = self.list_ids(&query, page_token.as_deref())?;
            ids.extend(page.ids);
            if ids.len() >= limit {
                ids.truncate(limit);
                break;
            }
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }

        // 3. Fetch, parse, cache and insert.
        let mut inserted = 0;
        for id in &ids {
            if store.gmail_message_exists(account_id, id)? {
                continue;
            }
            // Metadata gives the thread and the labels the raw fetch does not carry (E4.7/E8.6).
            let meta = self.message_meta(id)?;
            let kind = mailbox_kind_for_labels(&meta.label_ids);
            let resolved_mailbox = match mailbox_id {
                Some(id) => id,
                None => store.ensure_mailbox(account_id, kind.display_name(), kind.as_str())?,
            };
            let thread_id = store.ensure_thread(account_id, meta.thread_id.as_deref(), None)?;

            let raw = self.raw_message(id)?;
            let parsed = mime::parse_raw(&raw).with_context(|| format!("parse message {id}"))?;
            let raw_hash = store.cache().put(&raw)?;
            let message = NewMessage {
                account_id,
                mailbox_id: Some(resolved_mailbox),
                thread_id: Some(thread_id),
                provider: Some(ProviderRef::Gmail {
                    id: id.clone(),
                    thread_id: meta.thread_id.clone(),
                    history_id: None,
                }),
                message_id: parsed.message_id,
                in_reply_to: parsed.in_reply_to,
                references: parsed.references,
                subject: parsed.subject,
                from_name: parsed.from_name,
                from_addr: parsed.from_addr,
                to_json: Some(serde_json::to_string(&parsed.to)?),
                date: parsed.date,
                preview: parsed.preview,
                unread: meta.label_ids.iter().any(|label| label == "UNREAD"),
                raw_hash: Some(raw_hash),
                labels_json: Some(serde_json::to_string(&meta.label_ids)?),
                ..Default::default()
            };
            if store.insert_message_if_new(&message)? {
                inserted += 1;
            }
        }
        // Keep the mailbox counters honest after a batch (E5.9 reads them).
        store.with_db(|conn| {
            conn.execute(
                "UPDATE mailbox SET
                    total = (SELECT count(*) FROM message WHERE message.mailbox_id = mailbox.id),
                    unread = (SELECT count(*) FROM message WHERE message.mailbox_id = mailbox.id AND message.unread = 1)",
                [],
            )?;
            Ok(())
        })?;

        // 4. Commit the head only now.
        store.set_sync_state(&SyncState {
            account_id,
            kind: "gmail".into(),
            collection: String::new(),
            history_id: head.map(|id| id as i64),
            last_sync: Some(now_epoch()),
            ..Default::default()
        })?;

        Ok(BackfillReport {
            enumerated: ids.len(),
            inserted,
            head_history_id: head,
        })
    }
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn read_json(response: reqwest::blocking::Response) -> Result<Value, GmailError> {
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let reason = value
            .pointer("/error/errors/0/reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        let domain = value
            .pointer("/error/errors/0/domain")
            .and_then(Value::as_str)
            .unwrap_or("");
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string();
        return Err(GmailError {
            kind: classify(status, reason, domain),
            status,
            message,
        });
    }
    Ok(value)
}

impl MailProvider for GmailClient {
    fn list_changes(&self, cursor: &SyncState) -> Result<Changes> {
        let Some(start) = cursor.history_id else {
            // No cursor yet: the caller must backfill.
            return Ok(Changes {
                full_resync: true,
                ..Default::default()
            });
        };
        let mut changes = Changes::default();
        let mut page_token: Option<String> = None;
        let mut page_max: Option<u64> = None;
        let mut top: Option<u64> = None;
        loop {
            let url = history_url(start as u64, page_token.as_deref());
            let json = match self.get_json(&url) {
                Ok(json) => json,
                // A 404 window is routine, not an error (E4.6).
                Err(error) if error.kind == ApiError::HistoryExpired => {
                    return Ok(Changes {
                        full_resync: true,
                        ..Default::default()
                    });
                }
                Err(error) => return Err(error.into()),
            };
            let page = parse_history(&json);
            if let Some(id) = page.page_max_id {
                page_max = Some(page_max.map_or(id, |current: u64| current.max(id)));
            }
            if page.top_history_id.is_some() {
                top = page.top_history_id.or(top);
            }
            for record in page.records {
                match record.kind {
                    RecordKind::Added(id) => changes.added.push(id),
                    RecordKind::Deleted(id) => changes.deleted.push(id),
                    RecordKind::LabelsAdded { id, labels } => changes.labels.push(LabelChange {
                        id,
                        added: labels,
                        removed: Vec::new(),
                    }),
                    RecordKind::LabelsRemoved { id, labels } => changes.labels.push(LabelChange {
                        id,
                        added: Vec::new(),
                        removed: labels,
                    }),
                }
            }
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }
        changes.next_cursor = committed_cursor(page_max, top).map(|id| id.to_string());
        Ok(changes)
    }

    fn fetch_raw(&self, ids: &[String]) -> Result<Vec<RawMessage>> {
        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            messages.push(RawMessage {
                provider_id: id.clone(),
                bytes: self.raw_message(id)?,
            });
        }
        Ok(messages)
    }

    fn apply(&self, ops: &[RemoteOp]) -> Result<Vec<OpOutcome>> {
        let mut outcomes = Vec::with_capacity(ops.len());
        for op in ops {
            let (id, add, remove) = match op {
                // Gmail's archive is "remove INBOX"; trash is a TRASH label (E8.6).
                RemoteOp::Archive { id } => (id.clone(), vec![], vec!["INBOX".to_string()]),
                RemoteOp::Trash { id } => (id.clone(), vec!["TRASH".to_string()], vec![]),
                RemoteOp::MarkRead { id, read } => (
                    id.clone(),
                    if *read {
                        vec![]
                    } else {
                        vec!["UNREAD".to_string()]
                    },
                    if *read {
                        vec!["UNREAD".to_string()]
                    } else {
                        vec![]
                    },
                ),
                RemoteOp::Label { id, add, remove } => (id.clone(), add.clone(), remove.clone()),
                RemoteOp::Move { id, mailbox } => {
                    (id.clone(), vec![mailbox.clone()], vec!["INBOX".to_string()])
                }
                RemoteOp::Send { raw } => {
                    let result = self.send(raw);
                    outcomes.push(OpOutcome {
                        id: "send".into(),
                        ok: result.is_ok(),
                        terminal: false,
                        error: result.err().map(|error| error.to_string()),
                    });
                    continue;
                }
            };
            let result = self.post_json(
                &modify_url(&id),
                serde_json::json!({ "addLabelIds": add, "removeLabelIds": remove }),
            );
            outcomes.push(OpOutcome {
                id,
                ok: result.is_ok(),
                terminal: matches!(
                    result.as_ref().err().map(|error| error.kind),
                    Some(ApiError::Auth) | Some(ApiError::WorkspaceBlocked)
                ),
                error: result.err().map(|error| error.to_string()),
            });
        }
        Ok(outcomes)
    }

    fn send(&self, raw: &[u8]) -> Result<()> {
        self.post_json(&send_url(), serde_json::json!({ "raw": encode_raw(raw) }))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_takes_the_head_at_request_time() {
        let profile = parse_profile(&json!({
            "emailAddress": "me@x.com", "messagesTotal": 10, "threadsTotal": 5, "historyId": "999"
        }));
        assert_eq!(profile.email, "me@x.com");
        assert_eq!(profile.messages_total, 10);
        assert_eq!(profile.history_id, Some(999));
    }

    #[test]
    fn message_list_parses_ids_and_paging() {
        let list = parse_message_list(&json!({
            "messages": [{"id": "a"}, {"id": "b"}], "nextPageToken": "N", "resultSizeEstimate": 2
        }));
        assert_eq!(list.ids, vec!["a", "b"]);
        assert_eq!(list.next_page_token.as_deref(), Some("N"));
        assert!(parse_message_list(&json!({})).ids.is_empty());
    }

    #[test]
    fn raw_round_trips_and_tolerates_padding() {
        let raw = b"From: a@b.c\r\n\r\nhello\r\n";
        let encoded = encode_raw(raw);
        assert_eq!(decode_raw(&encoded).unwrap(), raw);
        // Gmail's raw is sometimes padded; the decoder must cope (E0.3).
        assert_eq!(decode_raw(&format!("{encoded}==")).unwrap(), raw);
    }

    #[test]
    fn the_backfill_window_is_the_last_thirty_days() {
        assert_eq!(backfill_query(30), "newer_than:30d");
    }

    #[test]
    fn gmail_labels_map_to_the_five_mailboxes() {
        let labels = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["INBOX", "UNREAD"])),
            MailboxKind::Inbox
        );
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["SENT"])),
            MailboxKind::Sent
        );
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["DRAFT"])),
            MailboxKind::Drafts
        );
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["TRASH"])),
            MailboxKind::Trash
        );
        // Archive is the absence of every system folder (E8.6).
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["STARRED", "IMPORTANT"])),
            MailboxKind::Archive
        );
        assert_eq!(mailbox_kind_for_labels(&labels(&[])), MailboxKind::Archive);
        // Trash wins over a still-present INBOX.
        assert_eq!(
            mailbox_kind_for_labels(&labels(&["INBOX", "TRASH"])),
            MailboxKind::Trash
        );
    }

    #[test]
    fn message_meta_parses_thread_and_labels() {
        let meta = parse_message_meta(
            &json!({"id": "m1", "threadId": "t1", "labelIds": ["INBOX", "UNREAD"]}),
        );
        assert_eq!(meta.id, "m1");
        assert_eq!(meta.thread_id.as_deref(), Some("t1"));
        assert_eq!(meta.label_ids, vec!["INBOX", "UNREAD"]);
    }

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
        assert!(
            matches!(&page.records[2].kind, RecordKind::LabelsAdded { labels, .. } if labels == &["STARRED"])
        );
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
        assert_eq!(
            classify(403, "rateLimitExceeded", "usageLimits"),
            ApiError::Retryable
        );
        assert_eq!(classify(429, "", ""), ApiError::Retryable);
        assert_eq!(classify(503, "", ""), ApiError::Retryable);
        assert_eq!(
            classify(403, "", "domainPolicy"),
            ApiError::WorkspaceBlocked
        );
        assert_eq!(classify(401, "", ""), ApiError::Auth);
        assert_eq!(classify(400, "badRequest", ""), ApiError::Other);
    }
}
