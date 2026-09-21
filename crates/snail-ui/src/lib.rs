//! Snail's pure UI model, with no UI framework.
//!
//! Theme/palette maths, text roles, the command + shortcut table, list selection and keyboard-nav
//! rules, calendar grid geometry, date formatting, and HTML layout (plan.md §2). Only deps are
//! `serde`/`serde_json`.

pub mod html;
pub mod layout;
pub mod text;
pub mod theme;
