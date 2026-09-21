//! Motion (plan.md E1.11). The calendar README's prescription: 120–180 ms ease-out for view changes
//! and sheet entry, **and nothing else anywhere**. No row transitions, no hover fades — hover is the
//! one thing that must be instant.
//!
//! Encoding it as an enum with exactly two variants is the guard: there is no `Hover` or `Row`
//! variant to reach for, so the rule cannot drift.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Month ↔ week, mailbox ↔ mailbox, list collapse.
    ViewChange,
    /// The event editor sheet (and, later, other sheets).
    SheetEntry,
}

/// The prescribed window.
pub const MIN_MS: u64 = 120;
pub const MAX_MS: u64 = 180;

impl Motion {
    pub fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Motion::ViewChange => 160,
            Motion::SheetEntry => 180,
        })
    }
}

/// Ease-out, the only curve in the app.
pub const EASE_OUT: [f32; 4] = [0.0, 0.0, 0.2, 1.0];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_motion_is_inside_the_prescribed_window() {
        for motion in [Motion::ViewChange, Motion::SheetEntry] {
            let ms = motion.duration().as_millis() as u64;
            assert!(
                (MIN_MS..=MAX_MS).contains(&ms),
                "{motion:?} is {ms}ms, outside {MIN_MS}-{MAX_MS}"
            );
        }
    }
}
