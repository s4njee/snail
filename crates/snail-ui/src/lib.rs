//! Snail's pure UI model, with no UI framework.
//!
//! Theme/palette maths, text roles, the command + shortcut table, list selection and keyboard-nav
//! rules, calendar grid geometry, date/timezone formatting, and HTML layout (plan.md §2).

pub mod calendar;
pub mod commands;
pub mod dates;
pub mod empty;
pub mod html;
pub mod layout;
pub mod motion;
pub mod palette;
pub mod preview;
pub mod search;
pub mod selection;
pub mod text;
pub mod theme;
pub mod threads;
