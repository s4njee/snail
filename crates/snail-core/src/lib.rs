//! Snail's UI-agnostic core.
//!
//! Store (SQLite + FTS5), MIME parse/build, threading, recurrence, PGP, the four provider clients,
//! HTML sanitize + DOM. No UI, no GPUI (plan.md §2).

pub mod atomic_file;
pub mod cache;
pub mod calendar;
pub mod compose;
pub mod fixture;
pub mod html;
pub mod ical;
pub mod labels;
pub mod mime;
pub mod paths;
pub mod pgp;
pub mod pgp_mime;
pub mod providers;
pub mod settings;
pub mod store;
pub mod triage;
pub mod wkd;
