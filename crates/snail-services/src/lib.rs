//! Snail's UI-agnostic services.
//!
//! Sync scheduling, account/secret storage, settings, notifications, the `Hub<T>` event bus and
//! attachment/temp-file handling (plan.md §2).

pub mod auth;
pub mod calendar_sync;
pub mod icloud_imap;
pub mod outbox;
pub mod pgp;
pub mod reminders;
pub mod secrets;
pub mod sync;
