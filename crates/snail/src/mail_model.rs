//! Mail reads for the shell (plan.md E5). A **model**, not a view: it is the only module in the app
//! crate that touches the store, which is what keeps `store::tests::no_view_module_reaches_the_store`
//! honest.
//!
//! First cut: these reads are local and fast, so they run inline. E5.12 moves them onto the
//! background executor with a generation guard (§2).

use std::sync::Arc;

use snail_core::mime::{BodyPreference, ParsedMessage};
use snail_core::store::Store;

// The shell sees these through the model, so it never names the store.
pub use snail_core::store::{MailboxRow, MessageRow};

#[derive(Clone)]
pub struct MailModel {
    store: Arc<Store>,
}

impl MailModel {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
    pub fn mailboxes(&self) -> Vec<MailboxRow> {
        self.store.mailboxes().unwrap_or_default()
    }

    pub fn page(&self, mailbox_id: i64, limit: u32) -> Vec<MessageRow> {
        self.store.page(mailbox_id, limit, 0).unwrap_or_default()
    }

    pub fn message(&self, id: i64) -> Option<MessageRow> {
        self.store.message(id).ok().flatten()
    }

    /// Parse a message's cached raw MIME. `None` means the cache file is gone (re-fetch is E2.3).
    pub fn parsed_with_preference(
        &self,
        id: i64,
        preference: BodyPreference,
    ) -> Option<ParsedMessage> {
        let raw = self.store.raw_bytes(id).ok().flatten()?;
        snail_core::mime::parse_raw_with_preference(&raw, preference).ok()
    }

    /// The cached raw MIME bytes, for resolving inline `cid:` images (E6.8).
    pub fn raw(&self, id: i64) -> Option<Vec<u8>> {
        self.store.raw_bytes(id).ok().flatten()
    }

    /// Parse a cached message for reply/forward derivation (E7.4).
    pub fn parsed(&self, id: i64) -> Option<ParsedMessage> {
        snail_core::mime::parse_raw(&self.raw(id)?).ok()
    }

    /// The first account's address, to exclude the user from a reply-all.
    pub fn first_account_address(&self) -> Option<String> {
        self.store
            .accounts()
            .ok()?
            .first()
            .map(|account| account.address.clone())
    }

    /// Address autocomplete from the harvested contacts (E7.3).
    pub fn suggest_contacts(&self, prefix: &str, limit: u32) -> Vec<snail_core::store::ContactRow> {
        let Some(account_id) = self.first_account_id() else {
            return Vec::new();
        };
        self.store
            .suggest_contacts(account_id, prefix, limit)
            .unwrap_or_default()
    }

    /// Rebuild the contacts table from the messages already on disk (E7.3).
    pub fn harvest_contacts(&self) -> Option<usize> {
        self.store.harvest_contacts(self.first_account_id()?).ok()
    }

    /// Pending send count, for the sync footer's "N changes pending" (E16.6/E7.12).
    pub fn pending_sends(&self) -> i64 {
        self.store
            .op_counts()
            .map(|counts| counts.outstanding() + counts.dead)
            .unwrap_or(0)
    }

    /// `(failed_or_dead, pending)` send counts for the sidebar footer (E7.12).
    pub fn send_queue(&self) -> (i64, i64) {
        let counts = self.store.op_counts().unwrap_or_default();
        (counts.failed + counts.dead, counts.pending)
    }

    /// Explicit retry: move failed and dead-lettered sends back to pending (E7.12).
    pub fn retry_failed_sends(&self) -> usize {
        self.store.requeue_failed_ops().unwrap_or(0)
    }

    /// The first account, for a first-cut send (E7.11 will pick by identity).
    pub fn first_account_id(&self) -> Option<i64> {
        self.store.accounts().ok()?.first().map(|account| account.id)
    }

    /// Autosave a draft (E7.6): the raw message goes to the cache, the row to the Drafts mailbox.
    /// Returns the message id to reuse on the next autosave.
    pub fn save_draft(
        &self,
        draft_id: Option<i64>,
        subject: &str,
        body_text: &str,
        to_json: &str,
        raw: &[u8],
    ) -> Option<i64> {
        let account_id = self.first_account_id()?;
        let raw_hash = self.store.cache().put(raw).ok();
        let now = now_epoch();
        self.store
            .save_draft(draft_id, account_id, subject, body_text, to_json, raw_hash.as_deref(), now)
            .ok()
    }

    /// Discard a draft (a sent message, or an explicit delete).
    pub fn delete_message(&self, id: i64) -> bool {
        self.store.delete_message(id).is_ok()
    }

    /// Queue an outbound message (E7.10): the raw bytes go to the cache, and the `pending_op` holds
    /// the hash so nothing large sits in SQLite. Blocked by default (after the undo window).
    pub fn enqueue_send(&self, raw: &[u8]) -> Option<String> {
        let account_id = self.first_account_id()?;
        let hash = self.store.cache().put(raw).ok()?;
        self.store
            .enqueue_op(&snail_core::store::NewOp {
                account_id,
                target_kind: "message".into(),
                target_id: None,
                operation: "send".into(),
                payload_json: Some(format!("{{\"raw_hash\":\"{hash}\"}}")),
                idempotency_key: format!("send:{hash}"),
                now: now_epoch(),
            })
            .ok()?;
        Some(hash)
    }

    /// Fetch a remote image, served from the content-addressed cache when it has been seen (E6.8).
    /// Runs on whatever thread calls it — the shell calls it from the background executor.
    pub fn fetch_remote_image(&self, url: &str) -> Option<Vec<u8>> {
        if let Ok(Some(cached)) = self.store.cache().get_alias(url) {
            return Some(cached);
        }
        let response = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .ok()?
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .header(reqwest::header::USER_AGENT, "snail/0.1 (gzip)")
            .send()
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = response.bytes().ok()?.to_vec();
        if bytes.len() > 10 * 1024 * 1024 {
            return None;
        }
        let _ = self.store.cache().put_alias(url, &bytes);
        Some(bytes)
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
