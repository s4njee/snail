//! iCloud IMAP folder mapping (E4.18).
//!
//! The mapping itself moved to [`crate::labels`] in E8.6 so Gmail's and iCloud's rules live in one
//! place; this module re-exports it under the provider name for the call sites (and tests) that
//! think in terms of the IMAP side. The live IMAP connection lives behind `MailProvider`.

pub use crate::labels::{MailboxKind, map_folder, resolve_kind, special_use};
