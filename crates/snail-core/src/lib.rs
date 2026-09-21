//! Snail's UI-agnostic core.
//!
//! Store (SQLite + FTS5), MIME parse/build, threading, recurrence, PGP, the four provider clients,
//! HTML sanitize + DOM. No UI, no GPUI (plan.md §2).

pub mod cache;
pub mod fixture;
pub mod paths;
pub mod settings;
pub mod store;
