//! The provider boundary (plan.md E4.1). Gmail-over-REST and iCloud-over-IMAP produce the same
//! rows; **no view ever branches on the account kind** except to draw its name and colour dot.

pub mod gmail;
pub mod imap;

use anyhow::Result;

use crate::store::SyncState;

/// One change page, provider-shaped then normalized.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub labels: Vec<LabelChange>,
    /// The cursor to commit **only after the last page** (E4.4).
    pub next_cursor: Option<String>,
    /// The provider invalidated its cursor; a full resync is required (E4.6, E11.7).
    pub full_resync: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelChange {
    pub id: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// The original RFC822 bytes, fetched whole so PGP verification and round-tripping are exact (E4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMessage {
    pub provider_id: String,
    pub bytes: Vec<u8>,
}

/// A queued local change, drained by the sync loop (E2.5 → E4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    Archive {
        id: String,
    },
    Trash {
        id: String,
    },
    MarkRead {
        id: String,
        read: bool,
    },
    Label {
        id: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    Move {
        id: String,
        mailbox: String,
    },
    Send {
        raw: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpOutcome {
    pub id: String,
    pub ok: bool,
    /// Terminal failures go to the dead-letter state instead of retrying forever (E16.5).
    pub terminal: bool,
    pub error: Option<String>,
}

pub trait MailProvider: Send + Sync {
    /// Changes since the cursor, in provider terms.
    fn list_changes(&self, cursor: &SyncState) -> Result<Changes>;
    /// The original bytes for a set of provider ids (E4.3).
    fn fetch_raw(&self, ids: &[String]) -> Result<Vec<RawMessage>>;
    fn apply(&self, ops: &[RemoteOp]) -> Result<Vec<OpOutcome>>;
    fn send(&self, raw: &[u8]) -> Result<()>;
}
