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

/// Gmail's per-user budget under the quota model in force since 2026-05-01 (plan.md §6.5).
pub const USER_UNITS_PER_MINUTE: f64 = 6_000.0;

/// Back-off after a throttle. The window is a minute, so the later steps wait most of one out.
const RETRY_DELAYS: [Duration; 7] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(40),
    Duration::from_secs(60),
    Duration::ZERO,
];

/// What a call costs against the per-user quota (plan.md §6.5). Unknown calls are charged like a
/// message fetch rather than for free.
pub fn quota_cost(method: &str, url: &str) -> f64 {
    let path = url.split('?').next().unwrap_or(url);
    if path.ends_with("/profile") {
        1.0
    } else if path.ends_with("/history") {
        2.0
    } else if path.ends_with("/messages/send") {
        100.0
    } else if path.ends_with("/modify") || (path.ends_with("/messages") && method == "GET") {
        5.0
    } else if path.contains("/threads/") {
        40.0
    } else {
        20.0
    }
}

/// A token bucket over quota units, shared by every request (and every fetch thread) of one
/// client. The rate is a fraction of Google's limit, leaving headroom for the user's other Gmail
/// clients; the burst is a tenth of a minute's budget, so a minute can never exceed it.
pub struct QuotaLimiter {
    per_second: f64,
    burst: f64,
    state: std::sync::Mutex<LimiterState>,
}

struct LimiterState {
    tokens: f64,
    at: std::time::Instant,
    paused_until: Option<std::time::Instant>,
}

impl QuotaLimiter {
    pub fn new(units_per_minute: f64) -> Self {
        let burst = units_per_minute / 10.0;
        Self {
            per_second: units_per_minute / 60.0,
            burst,
            state: std::sync::Mutex::new(LimiterState {
                tokens: burst,
                at: std::time::Instant::now(),
                paused_until: None,
            }),
        }
    }

    /// How long a request of `cost` must wait at `now`; zero means it was admitted and charged.
    fn reserve(&self, cost: f64, now: std::time::Instant) -> Duration {
        let mut state = self.state.lock().unwrap();
        if let Some(until) = state.paused_until {
            if now < until {
                return until - now;
            }
            state.paused_until = None;
            // After a throttle, start from empty rather than a full burst.
            state.tokens = 0.0;
            state.at = now;
        }
        let elapsed = now.saturating_duration_since(state.at).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.per_second).min(self.burst);
        state.at = now;
        let cost = cost.min(self.burst);
        // A hair of tolerance: refilling exactly one wait's worth can land a rounding error short.
        if state.tokens + 1e-6 >= cost {
            state.tokens = (state.tokens - cost).max(0.0);
            Duration::ZERO
        } else {
            // Never zero here, or a caller would take "wait nothing" for "admitted".
            Duration::from_secs_f64((cost - state.tokens) / self.per_second)
                .max(Duration::from_millis(1))
        }
    }

    /// Block until `cost` units are available, then spend them.
    pub fn acquire(&self, cost: f64) {
        loop {
            let wait = self.reserve(cost, std::time::Instant::now());
            if wait.is_zero() {
                return;
            }
            std::thread::sleep(wait);
        }
    }

    /// Google said stop: hold every request for `delay`.
    pub fn pause(&self, delay: Duration) {
        let mut state = self.state.lock().unwrap();
        let until = std::time::Instant::now() + delay;
        state.paused_until = Some(
            state
                .paused_until
                .map_or(until, |current| current.max(until)),
        );
    }
}

pub struct GmailClient {
    http: reqwest::blocking::Client,
    tokens: Arc<dyn TokenSource>,
    quota: QuotaLimiter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GmailLabel {
    pub id: String,
    pub name: String,
    pub user: bool,
}

pub fn parse_labels(json: &Value) -> Vec<GmailLabel> {
    json.get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|label| {
            Some(GmailLabel {
                id: label.get("id")?.as_str()?.to_string(),
                name: label.get("name")?.as_str()?.to_string(),
                user: label.get("type").and_then(Value::as_str) == Some("user"),
            })
        })
        .collect()
}

impl GmailClient {
    pub fn new(tokens: Arc<dyn TokenSource>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            tokens,
            quota: QuotaLimiter::new(USER_UNITS_PER_MINUTE * 0.9),
        })
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
        let cost = quota_cost("GET", url);
        self.with_retry(|| {
            self.quota.acquire(cost);
            self.get_json_once(url)
        })
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
        let cost = quota_cost("POST", url);
        self.with_retry(|| {
            self.quota.acquire(cost);
            self.post_json_once(url, body.clone())
        })
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
    /// (`domainPolicy`, auth) return at once. The quota is per user **per minute** and shared with
    /// the user's phone, so a throttle pauses *every* request on this client, not just the one
    /// that hit it — otherwise parallel fetches retry in lockstep and keep the window full (E16.4).
    fn with_retry<T>(
        &self,
        mut attempt_fn: impl FnMut() -> Result<T, GmailError>,
    ) -> Result<T, GmailError> {
        let mut last: Option<GmailError> = None;
        for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
            match attempt_fn() {
                Ok(value) => return Ok(value),
                Err(error)
                    if error.kind == ApiError::Retryable && attempt + 1 < RETRY_DELAYS.len() =>
                {
                    log::warn!("Gmail throttled ({}); pausing {:?}", error.message, delay);
                    self.quota.pause(*delay);
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

    pub fn labels(&self) -> Result<Vec<GmailLabel>> {
        let json = self.get_json(&format!("{BASE}/labels"))?;
        Ok(parse_labels(&json))
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

    /// A message's raw bytes **and** its thread and labels in one call: `format=raw` carries
    /// `threadId` and `labelIds` too, so a separate `format=minimal` fetch is wasted quota (E4.3).
    pub fn raw_message_with_meta(&self, id: &str) -> Result<(MessageMeta, Vec<u8>), GmailError> {
        let json = self.get_json(&message_raw_url(id))?;
        parse_raw_message(&json).map_err(|error| GmailError {
            kind: ApiError::Other,
            status: 0,
            message: format!("message {id}: {error:#}"),
        })
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
        self.backfill_with_progress(store, account_id, mailbox_id, days, limit, &mut |_, _| {})
    }

    /// [`Self::backfill`], reporting `(done, total)` after each batch so the UI can show mail as it
    /// lands rather than after the whole window.
    pub fn backfill_with_progress(
        &self,
        store: &Store,
        account_id: i64,
        mailbox_id: Option<i64>,
        days: u32,
        limit: usize,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<BackfillReport> {
        self.backfill_query_with_progress(
            store,
            account_id,
            mailbox_id,
            &backfill_query(days),
            limit,
            progress,
        )
    }

    /// Backfill a caller-selected Gmail search. Settings uses this for the first pull so disabled
    /// mailbox classes are not downloaded merely because an account was just connected.
    pub fn backfill_query_with_progress(
        &self,
        store: &Store,
        account_id: i64,
        mailbox_id: Option<i64>,
        query: &str,
        limit: usize,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<BackfillReport> {
        // 1. Head before enumerating. Doing it after silently loses changes (E4.2).
        let profile = self.profile()?;
        let head = profile.history_id;

        // 2. Enumerate the window. The list is newest first, so the inbox fills from the top.
        let mut ids = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page = self.list_ids(query, page_token.as_deref())?;
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
        let inserted = self.import_ids(store, account_id, mailbox_id, &ids, progress)?;

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

    /// Fetch and insert every id not already stored, [`FETCH_BATCH`] at a time in parallel, in
    /// the order given. A message that is gone by the time it is fetched (404) is skipped; any
    /// other failure aborts, so the caller does not commit a cursor past a message it never got.
    pub fn import_ids(
        &self,
        store: &Store,
        account_id: i64,
        mailbox_id: Option<i64>,
        ids: &[String],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<usize> {
        let mut wanted = Vec::with_capacity(ids.len());
        for id in ids {
            if !wanted.contains(id) && !store.gmail_message_exists(account_id, id)? {
                wanted.push(id.clone());
            }
        }
        let mut inserted = 0;
        let mut done = 0;
        for batch in wanted.chunks(FETCH_BATCH) {
            let fetched: Vec<Result<(MessageMeta, Vec<u8>), GmailError>> =
                std::thread::scope(|scope| {
                    let handles: Vec<_> = batch
                        .iter()
                        .map(|id| scope.spawn(move || self.raw_message_with_meta(id)))
                        .collect();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle.join().unwrap_or_else(|_| {
                                Err(GmailError {
                                    kind: ApiError::Other,
                                    status: 0,
                                    message: "fetch thread panicked".into(),
                                })
                            })
                        })
                        .collect()
                });
            for (id, result) in batch.iter().zip(fetched) {
                match result {
                    Ok((meta, raw)) => {
                        if import_message(store, account_id, mailbox_id, id, &meta, &raw)? {
                            inserted += 1;
                        }
                    }
                    Err(error) if error.status == 404 => {
                        log::info!("Gmail message {id} vanished before it was fetched; skipping");
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            done += batch.len();
            store.recount_account(account_id)?;
            progress(done, wanted.len());
        }
        Ok(inserted)
    }

    /// Apply one history batch to the store (E4.7): fetch what was added, delete what was
    /// deleted, and re-file what was relabelled. The caller commits the cursor afterwards.
    pub fn apply_changes(
        &self,
        store: &Store,
        account_id: i64,
        changes: &Changes,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        let deleted: std::collections::HashSet<&String> = changes.deleted.iter().collect();
        let added: Vec<String> = changes
            .added
            .iter()
            .filter(|id| !deleted.contains(id))
            .cloned()
            .collect();
        report.inserted = self.import_ids(store, account_id, None, &added, progress)?;

        for id in &changes.deleted {
            if store.delete_gmail_message(account_id, id)? {
                report.deleted += 1;
            }
        }

        for change in &changes.labels {
            let Some((message_id, mut labels)) =
                store.gmail_message_labels(account_id, &change.id)?
            else {
                // Not stored (outside the backfill window, or just deleted): nothing to re-file.
                continue;
            };
            // A local change still on its way up wins over whatever the server said before it.
            if store.has_outstanding_ops(message_id)? {
                continue;
            }
            let before = labels.clone();
            merge_labels(&mut labels, &change.added, &change.removed);
            if labels == before {
                continue;
            }
            let kind = mailbox_kind_for_labels(&labels);
            let mailbox = store.ensure_mailbox(account_id, kind.display_name(), kind.as_str())?;
            store.set_gmail_labels(message_id, &labels, mailbox)?;
            report.relabelled += 1;
        }
        store.recount_account(account_id)?;
        Ok(report)
    }
}

/// How many messages are fetched at once. The [`QuotaLimiter`] sets the pace (a raw
/// `messages.get` is 20 units against 6,000 a minute, so about 4.5 a second); four in flight is
/// just enough to hide each request's latency, and stays clear of Gmail's undocumented
/// concurrent-request 429.
pub const FETCH_BATCH: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub inserted: usize,
    pub deleted: usize,
    pub relabelled: usize,
}

/// Split a `format=raw` response into its metadata and decoded bytes.
pub fn parse_raw_message(json: &Value) -> Result<(MessageMeta, Vec<u8>)> {
    let encoded = json
        .get("raw")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("no raw field"))?;
    Ok((parse_message_meta(json), decode_raw(encoded)?))
}

/// Apply a `labelsAdded` / `labelsRemoved` record to a stored label set, keeping its order.
pub fn merge_labels(labels: &mut Vec<String>, added: &[String], removed: &[String]) {
    labels.retain(|label| !removed.contains(label));
    for label in added {
        if !labels.contains(label) {
            labels.push(label.clone());
        }
    }
}

/// File one fetched message: thread, mailbox from its labels, raw bytes into the cache, row into
/// the store. True when it was new. The local stand-in for a message we sent is replaced by the
/// real one (E7.11), so Sent does not list it twice.
pub fn import_message(
    store: &Store,
    account_id: i64,
    mailbox_id: Option<i64>,
    id: &str,
    meta: &MessageMeta,
    raw: &[u8],
) -> Result<bool> {
    // Metadata gives the thread and the labels the raw bytes do not carry (E4.7/E8.6).
    let kind = mailbox_kind_for_labels(&meta.label_ids);
    let resolved_mailbox = match mailbox_id {
        Some(id) => id,
        None => store.ensure_mailbox(account_id, kind.display_name(), kind.as_str())?,
    };
    let thread_id = store.ensure_thread(account_id, meta.thread_id.as_deref(), None)?;
    let parsed = mime::parse_raw(raw).with_context(|| format!("parse message {id}"))?;
    if let Some(message_id) = parsed.message_id.as_deref() {
        store.delete_sent_placeholder(account_id, message_id)?;
    }
    let raw_hash = store.cache().put(raw)?;
    let message = NewMessage {
        account_id,
        mailbox_id: Some(resolved_mailbox),
        thread_id: Some(thread_id),
        provider: Some(ProviderRef::Gmail {
            id: id.to_string(),
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
        has_attachments: !parsed.attachments.is_empty(),
        body_text: parsed.plain.map(|plain| crate::mime::search_text(&plain)),
        raw_hash: Some(raw_hash),
        labels_json: Some(serde_json::to_string(&meta.label_ids)?),
        ..Default::default()
    };
    store.insert_message_if_new(&message)
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

    fn raw(subject: &str, message_id: &str) -> Vec<u8> {
        format!(
            "From: Ada <ada@example.com>\r\nTo: me@example.com\r\nSubject: {subject}\r\n\
             Message-ID: <{message_id}>\r\nDate: Mon, 21 Sep 2026 09:00:00 +0000\r\n\r\nhello\r\n"
        )
        .into_bytes()
    }

    fn meta(id: &str, labels: &[&str]) -> MessageMeta {
        MessageMeta {
            id: id.into(),
            thread_id: Some(format!("t-{id}")),
            label_ids: labels.iter().map(|label| label.to_string()).collect(),
        }
    }

    fn mailbox_of(store: &Store, message_id: i64) -> String {
        store
            .with_db(|conn| {
                Ok(conn.query_row(
                    "SELECT b.name FROM message m JOIN mailbox b ON b.id = m.mailbox_id WHERE m.id = ?1",
                    [message_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    }

    #[test]
    fn calls_are_charged_what_gmail_charges() {
        assert_eq!(quota_cost("GET", &message_raw_url("m1")), 20.0);
        assert_eq!(quota_cost("GET", &history_url(5, None)), 2.0);
        assert_eq!(
            quota_cost("GET", &format!("{BASE}/messages?maxResults=500&q=x")),
            5.0
        );
        assert_eq!(quota_cost("POST", &modify_url("m1")), 5.0);
        assert_eq!(quota_cost("POST", &send_url()), 100.0);
        assert_eq!(
            quota_cost("GET", &format!("{BASE}/threads/t1?format=full")),
            40.0
        );
        assert_eq!(quota_cost("GET", &profile_url()), 1.0);
    }

    #[test]
    fn the_limiter_never_lets_a_minute_exceed_its_budget() {
        let limiter = QuotaLimiter::new(5_400.0);
        let start = std::time::Instant::now();
        // Simulate fetches as fast as the limiter admits them for one minute.
        let mut now = start;
        let mut spent = 0.0;
        while now < start + Duration::from_secs(60) {
            let wait = limiter.reserve(20.0, now);
            if wait.is_zero() {
                spent += 20.0;
            } else {
                now += wait;
            }
        }
        assert!(spent <= 6_000.0, "spent {spent} units in a minute");
        // And it does not starve: close to the configured rate.
        assert!(spent >= 5_400.0, "spent only {spent}");
    }

    #[test]
    fn a_throttle_pauses_everyone_then_restarts_from_empty() {
        let limiter = QuotaLimiter::new(6_000.0);
        let now = std::time::Instant::now();
        assert!(limiter.reserve(20.0, now).is_zero());
        limiter.pause(Duration::from_secs(5));
        let wait = limiter.reserve(20.0, now + Duration::from_secs(1));
        assert!(wait >= Duration::from_secs(3), "{wait:?}");
        // Just after the pause there is no burst left to spend.
        assert!(
            !limiter
                .reserve(20.0, now + Duration::from_secs(6))
                .is_zero()
        );
    }

    #[test]
    fn a_raw_response_carries_its_labels_and_thread() {
        let encoded = encode_raw(b"Subject: hi\r\n\r\nbody");
        let (meta, bytes) = parse_raw_message(&json!({
            "id": "m1", "threadId": "t1", "labelIds": ["INBOX", "UNREAD"], "raw": encoded
        }))
        .unwrap();
        assert_eq!(meta.thread_id.as_deref(), Some("t1"));
        assert_eq!(meta.label_ids, vec!["INBOX", "UNREAD"]);
        assert_eq!(bytes, b"Subject: hi\r\n\r\nbody");
        assert!(parse_raw_message(&json!({"id": "m1"})).is_err());
    }

    #[test]
    fn label_records_merge_without_duplicates() {
        let mut labels = vec!["INBOX".to_string(), "UNREAD".to_string()];
        merge_labels(
            &mut labels,
            &["STARRED".into(), "INBOX".into()],
            &["UNREAD".into()],
        );
        assert_eq!(labels, vec!["INBOX", "STARRED"]);
    }

    #[test]
    fn history_records_refile_delete_and_respect_pending_local_changes() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        assert!(
            import_message(
                &store,
                account,
                None,
                "m1",
                &meta("m1", &["INBOX", "UNREAD"]),
                &raw("One", "one@x")
            )
            .unwrap()
        );
        assert!(
            import_message(
                &store,
                account,
                None,
                "m2",
                &meta("m2", &["INBOX"]),
                &raw("Two", "two@x")
            )
            .unwrap()
        );
        assert!(
            import_message(
                &store,
                account,
                None,
                "m3",
                &meta("m3", &["INBOX"]),
                &raw("Three", "three@x")
            )
            .unwrap()
        );
        let (m1, _) = store.gmail_message_labels(account, "m1").unwrap().unwrap();
        let (m3, _) = store.gmail_message_labels(account, "m3").unwrap().unwrap();
        assert_eq!(mailbox_of(&store, m1), "Inbox");

        // The user archived m3 locally; that op has not reached Gmail yet.
        store
            .apply_triage(m3, &crate::triage::TriageAction::Archive, "archive:m3", 0)
            .unwrap();

        let client = GmailClient::new(Arc::new(FixedToken("unused".into()))).unwrap();
        let changes = Changes {
            deleted: vec!["m2".into()],
            labels: vec![
                // Archived and read elsewhere (the phone).
                LabelChange {
                    id: "m1".into(),
                    added: vec![],
                    removed: vec!["INBOX".into(), "UNREAD".into()],
                },
                // A stale "still in the inbox" view of m3 must not undo the local archive.
                LabelChange {
                    id: "m3".into(),
                    added: vec!["STARRED".into()],
                    removed: vec![],
                },
                // Outside the window: ignored, not an error.
                LabelChange {
                    id: "unknown".into(),
                    added: vec!["INBOX".into()],
                    removed: vec![],
                },
            ],
            ..Default::default()
        };
        let report = client
            .apply_changes(&store, account, &changes, &mut |_, _| {})
            .unwrap();
        assert_eq!(
            report,
            ApplyReport {
                inserted: 0,
                deleted: 1,
                relabelled: 1
            }
        );

        assert_eq!(mailbox_of(&store, m1), "Archive");
        assert!(!store.message(m1).unwrap().unwrap().unread);
        assert!(!store.gmail_message_exists(account, "m2").unwrap());
        assert_eq!(mailbox_of(&store, m3), "Archive");
        let (_, m3_labels) = store.gmail_message_labels(account, "m3").unwrap().unwrap();
        assert!(
            !m3_labels.contains(&"STARRED".to_string()),
            "skipped while the op is pending"
        );

        // Counters follow the moves: nothing left in the inbox.
        let inbox = store
            .mailboxes()
            .unwrap()
            .into_iter()
            .find(|b| b.name == "Inbox")
            .unwrap();
        assert_eq!((inbox.total, inbox.unread), (0, 0));
    }

    #[test]
    fn the_real_sent_copy_replaces_the_local_stand_in() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let sent = store.ensure_mailbox(account, "Sent", "sent").unwrap();
        store
            .insert_message(&NewMessage {
                account_id: account,
                mailbox_id: Some(sent),
                provider: Some(ProviderRef::Gmail {
                    id: "sent-abc".into(),
                    thread_id: None,
                    history_id: None,
                }),
                message_id: Some("mine@x".into()),
                ..Default::default()
            })
            .unwrap();
        import_message(
            &store,
            account,
            None,
            "real",
            &meta("real", &["SENT"]),
            &raw("Mine", "mine@x"),
        )
        .unwrap();
        let count: i64 = store
            .with_db(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*) FROM message WHERE mailbox_id = ?1",
                    [sent],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert!(store.gmail_message_exists(account, "real").unwrap());
        assert!(!store.gmail_message_exists(account, "sent-abc").unwrap());
    }

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
    fn labels_distinguish_user_collections_from_system_folders() {
        let labels = parse_labels(&json!({
            "labels": [
                {"id": "INBOX", "name": "INBOX", "type": "system"},
                {"id": "Label_7", "name": "Receipts/2026", "type": "user"},
                {"id": "missing-name", "type": "user"}
            ]
        }));
        assert_eq!(
            labels,
            vec![
                GmailLabel {
                    id: "INBOX".into(),
                    name: "INBOX".into(),
                    user: false,
                },
                GmailLabel {
                    id: "Label_7".into(),
                    name: "Receipts/2026".into(),
                    user: true,
                },
            ]
        );
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

    // E4.3: bodies come from `messages.get?format=raw`. `threads.get` has no raw format, so a
    // thread URL here would silently lose the original bytes.
    #[test]
    fn raw_fetches_are_per_message() {
        assert!(message_raw_url("m1").contains("/messages/m1?format=raw"));
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
