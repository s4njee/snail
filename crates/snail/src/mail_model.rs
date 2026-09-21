//! Mail reads for the shell (plan.md E5). A **model**, not a view: it is the only module in the app
//! crate that touches the store, which is what keeps `store::tests::no_view_module_reaches_the_store`
//! honest.
//!
//! First cut: these reads are local and fast, so they run inline. E5.12 moves them onto the
//! background executor with a generation guard (§2).

use std::sync::Arc;

use snail_core::mime::ParsedMessage;
use snail_core::store::Store;

// The shell sees these through the model, so it never names the store.
pub use snail_core::store::{MailboxRow, MessageRow};

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
    pub fn parsed(&self, id: i64) -> Option<ParsedMessage> {
        let raw = self.store.raw_bytes(id).ok().flatten()?;
        snail_core::mime::parse_raw(&raw).ok()
    }

    /// The cached raw MIME bytes, for resolving inline `cid:` images (E6.8).
    pub fn raw(&self, id: i64) -> Option<Vec<u8>> {
        self.store.raw_bytes(id).ok().flatten()
    }
}
