//! Snail's UI-agnostic services.
//!
//! Sync scheduling, account/secret storage, settings, notifications, the `Hub<T>` event bus and
//! attachment/temp-file handling (plan.md §2).

pub mod auth;
pub mod outbox;
pub mod secrets;
