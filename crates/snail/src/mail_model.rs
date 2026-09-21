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
