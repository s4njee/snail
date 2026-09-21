//! The text roles, deduplicated from both handoffs (plan.md E1.4). A view produces text only
//! through a role, so a raw size or a raw hex never appears in a view module.
//!
//! This is the framework-free half: it says what a role *is*. `snail::style::text` turns a
//! `TextSpec` into a GPUI element.

/// The three families (plan.md §1.3): Instrument Sans, Newsreader, DM Mono.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Ui,
    Serif,
    Mono,
}

/// The token a role paints with; resolved against the active palette by the view layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextColor {
    Ink,
    BodyInk,
    Secondary,
    Muted,
    Soft,
    Faint,
    Accent,
    AccentText,
    Inverted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSpec {
    pub family: Family,
    pub size: f32,
    pub weight: u16,
    /// Multiplier, not pixels: the view multiplies by `size`.
    pub line_height: f32,
    /// Letter-spacing in em (plan.md §1.2; GPUI has no tracking, E1.5 renders it).
    pub tracking_em: f32,
    pub uppercase: bool,
    pub color: TextColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextRole {
    // Section labels and micro copy (DM Mono, wide tracking).
    SectionLabel,
    SidebarCount,
    RowTimestamp,
    HourGutter,
    WeekdayLabel,
    CalendarYear,
    EventTime,
    Kbd,

    // Sidebar and list chrome.
    SidebarItem,
    SidebarItemSelected,
    ListHeaderTitle,
    ListHeaderMeta,

    // Message list rows.
    RowSenderUnread,
    RowSenderRead,
    RowSubjectUnread,
    RowSubjectRead,
    RowPreview,

    // Reading pane and thread.
    ReadingSubject,
    ReadingSenderName,
    ReadingMeta,
    ThreadSubject,
    MailboxPill,
    CollapsedSender,
    CollapsedSnippet,
    CollapsedDate,

    // Buttons and controls.
    ButtonLabel,
    ButtonLabelFilled,

    // Compose.
    ComposeSubject,
    ComposeLabel,
    Signature,

    // Settings.
    SettingsRowTitle,
    SettingsRowHelper,

    // Calendar.
    CalendarMonthTitle,
    CalendarTitle,
    CalendarButtonLabelFilled,
    MonthDayNumeral,
    MonthDayOutside,
    MonthDayToday,
    AgendaDate,
    EventTitle,
    EventNotes,

    // Message and event bodies (Newsreader).
    BodySerif,
    ThreadBodySerif,
}

/// Every role, for the completeness test. Keep in step with `spec`.
pub const ALL_ROLES: &[TextRole] = &[
    TextRole::SectionLabel,
    TextRole::SidebarCount,
    TextRole::RowTimestamp,
    TextRole::HourGutter,
    TextRole::WeekdayLabel,
    TextRole::CalendarYear,
    TextRole::EventTime,
    TextRole::Kbd,
    TextRole::SidebarItem,
    TextRole::SidebarItemSelected,
    TextRole::ListHeaderTitle,
    TextRole::ListHeaderMeta,
    TextRole::RowSenderUnread,
    TextRole::RowSenderRead,
    TextRole::RowSubjectUnread,
    TextRole::RowSubjectRead,
    TextRole::RowPreview,
    TextRole::ReadingSubject,
    TextRole::ReadingSenderName,
    TextRole::ReadingMeta,
    TextRole::ThreadSubject,
    TextRole::MailboxPill,
    TextRole::CollapsedSender,
    TextRole::CollapsedSnippet,
    TextRole::CollapsedDate,
    TextRole::ButtonLabel,
    TextRole::ButtonLabelFilled,
    TextRole::ComposeSubject,
    TextRole::ComposeLabel,
    TextRole::Signature,
    TextRole::SettingsRowTitle,
    TextRole::SettingsRowHelper,
    TextRole::CalendarMonthTitle,
    TextRole::CalendarTitle,
    TextRole::CalendarButtonLabelFilled,
    TextRole::MonthDayNumeral,
    TextRole::MonthDayOutside,
    TextRole::MonthDayToday,
    TextRole::AgendaDate,
    TextRole::EventTitle,
    TextRole::EventNotes,
    TextRole::BodySerif,
    TextRole::ThreadBodySerif,
];

const fn ui(size: f32, weight: u16, color: TextColor) -> TextSpec {
    TextSpec {
        family: Family::Ui,
        size,
        weight,
        line_height: 1.35,
        tracking_em: 0.0,
        uppercase: false,
        color,
    }
}

const fn mono(size: f32, weight: u16, tracking_em: f32, color: TextColor) -> TextSpec {
    TextSpec {
        family: Family::Mono,
        size,
        weight,
        line_height: 1.3,
        tracking_em,
        uppercase: false,
        color,
    }
}

const fn serif(size: f32, line_height: f32, color: TextColor) -> TextSpec {
    TextSpec {
        family: Family::Serif,
        size,
        weight: 400,
        line_height,
        tracking_em: 0.0,
        uppercase: false,
        color,
    }
}

/// Resolve a role. Exhaustive, so a new variant cannot compile without a spec.
/// A message-list row's vertical padding, and the gap between its lines, in pixels.
pub const MESSAGE_ROW_PADDING_Y: f32 = 12.0;
pub const MESSAGE_ROW_GAP: f32 = 4.0;

/// The height of a message-list row: its padding, then sender, subject and a two-line preview at
/// their roles' line heights. Derived rather than written down: a hand-picked 84px was about 15px
/// shorter than the row's own text, which went unnoticed only because the preview spilled into
/// the bottom padding — until rows started clipping their overflow, and the preview's second
/// line lost its descenders.
pub fn message_row_height() -> f32 {
    let line = |role: TextRole| {
        let spec = spec(role);
        spec.size * spec.line_height
    };
    let sender = line(TextRole::RowSenderUnread).max(line(TextRole::RowSenderRead));
    let subject = line(TextRole::RowSubjectUnread).max(line(TextRole::RowSubjectRead));
    let preview = 2.0 * line(TextRole::RowPreview);
    (2.0 * MESSAGE_ROW_PADDING_Y + sender + MESSAGE_ROW_GAP + subject + MESSAGE_ROW_GAP + preview)
        .ceil()
}

pub fn spec(role: TextRole) -> TextSpec {
    use TextColor::*;
    match role {
        TextRole::SectionLabel => TextSpec {
            uppercase: true,
            ..mono(10.0, 600, 0.09, Faint)
        },
        TextRole::SidebarCount => mono(11.0, 500, 0.0, Soft),
        TextRole::RowTimestamp => mono(11.0, 400, 0.0, Soft),
        TextRole::HourGutter => mono(10.0, 500, 0.04, Faint),
        TextRole::WeekdayLabel => TextSpec {
            uppercase: true,
            ..mono(10.0, 400, 0.12, Faint)
        },
        TextRole::CalendarYear => mono(13.0, 400, 0.04, Muted),
        TextRole::EventTime => mono(11.0, 400, 0.0, Muted),
        TextRole::Kbd => mono(11.0, 500, 0.02, Muted),

        TextRole::SidebarItem => ui(13.0, 500, Secondary),
        TextRole::SidebarItemSelected => ui(13.0, 600, Ink),
        TextRole::ListHeaderTitle => ui(14.0, 600, Ink),
        TextRole::ListHeaderMeta => ui(11.0, 400, Soft),

        TextRole::RowSenderUnread => ui(13.0, 600, Ink),
        TextRole::RowSenderRead => ui(13.0, 500, Secondary),
        TextRole::RowSubjectUnread => ui(12.5, 500, Ink),
        TextRole::RowSubjectRead => ui(12.5, 400, Secondary),
        TextRole::RowPreview => ui(12.0, 400, Muted),

        TextRole::ReadingSubject => TextSpec {
            tracking_em: -0.01,
            ..ui(19.0, 600, Ink)
        },
        TextRole::ReadingSenderName => ui(13.0, 600, Ink),
        TextRole::ReadingMeta => ui(11.5, 400, Soft),
        TextRole::ThreadSubject => TextSpec {
            tracking_em: -0.015,
            ..ui(24.0, 600, Ink)
        },
        TextRole::MailboxPill => TextSpec {
            uppercase: true,
            ..ui(10.5, 600, AccentText)
        },
        TextRole::CollapsedSender => ui(12.5, 500, Secondary),
        TextRole::CollapsedSnippet => ui(12.5, 400, Soft),
        TextRole::CollapsedDate => mono(11.5, 400, 0.0, Faint),

        TextRole::ButtonLabel => ui(12.0, 500, Secondary),
        TextRole::ButtonLabelFilled => ui(12.0, 600, Inverted),

        TextRole::ComposeSubject => ui(13.5, 500, Ink),
        TextRole::ComposeLabel => ui(12.0, 500, Faint),
        TextRole::Signature => serif(13.0, 1.6, Secondary),

        TextRole::SettingsRowTitle => ui(13.0, 500, Ink),
        TextRole::SettingsRowHelper => ui(11.5, 400, Soft),

        TextRole::CalendarMonthTitle => TextSpec {
            tracking_em: -0.03,
            ..ui(42.0, 600, Ink)
        },
        TextRole::CalendarTitle => TextSpec {
            tracking_em: -0.025,
            ..ui(34.0, 400, Ink)
        },
        TextRole::CalendarButtonLabelFilled => ui(12.0, 500, Inverted),
        TextRole::MonthDayNumeral => TextSpec {
            tracking_em: -0.02,
            ..ui(14.0, 400, Secondary)
        },
        TextRole::MonthDayOutside => TextSpec {
            tracking_em: -0.02,
            ..ui(14.0, 400, Muted)
        },
        TextRole::MonthDayToday => TextSpec {
            tracking_em: -0.02,
            ..ui(14.0, 500, Inverted)
        },
        TextRole::AgendaDate => TextSpec {
            tracking_em: -0.045,
            ..ui(46.0, 400, Ink)
        },
        TextRole::EventTitle => ui(13.0, 400, Ink),
        TextRole::EventNotes => serif(15.0, 1.7, BodyInk),

        TextRole::BodySerif => serif(15.0, 1.7, BodyInk),
        TextRole::ThreadBodySerif => serif(16.0, 1.75, BodyInk),
    }
}

#[cfg(test)]
mod tests {

    // Regression: the row was a fixed 84px while its text needed about 99px, so a clipped row cut
    // the preview's second line through its descenders.
    #[test]
    fn a_message_row_fits_sender_subject_and_a_two_line_preview() {
        let line = |role: TextRole| spec(role).size * spec(role).line_height;
        let content = line(TextRole::RowSenderUnread)
            + MESSAGE_ROW_GAP
            + line(TextRole::RowSubjectUnread)
            + MESSAGE_ROW_GAP
            + 2.0 * line(TextRole::RowPreview);
        let height = message_row_height();
        assert!(height >= 2.0 * MESSAGE_ROW_PADDING_Y + content);
        assert!(height - (2.0 * MESSAGE_ROW_PADDING_Y + content) < 1.0);
        assert_eq!(height, 99.0);
    }
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_role_resolves_to_a_sane_spec() {
        for role in ALL_ROLES {
            let spec = spec(*role);
            assert!(spec.size >= 10.0 && spec.size <= 80.0, "{role:?} size");
            assert!(spec.line_height >= 1.0, "{role:?} line height");
            assert!(spec.tracking_em.abs() < 0.2, "{role:?} tracking");
            assert!(
                matches!(spec.weight, 400 | 500 | 600 | 700),
                "{role:?} weight"
            );
        }
    }

    #[test]
    fn all_roles_are_listed_once() {
        let unique: HashSet<_> = ALL_ROLES.iter().collect();
        assert_eq!(unique.len(), ALL_ROLES.len(), "duplicate in ALL_ROLES");
        // Every variant that `spec` handles must be in the list: this count is the guard that a
        // newly added role is also added to ALL_ROLES.
        assert_eq!(ALL_ROLES.len(), 43);
    }

    #[test]
    fn serif_roles_are_the_body_and_notes() {
        for role in [
            TextRole::BodySerif,
            TextRole::ThreadBodySerif,
            TextRole::EventNotes,
            TextRole::Signature,
        ] {
            assert_eq!(spec(role).family, Family::Serif, "{role:?}");
        }
    }

    #[test]
    fn mono_roles_carry_their_tracking() {
        assert!(spec(TextRole::WeekdayLabel).tracking_em >= 0.1);
        assert!(spec(TextRole::SectionLabel).tracking_em >= 0.09);
        assert!(spec(TextRole::CalendarTitle).tracking_em < 0.0);
        assert!(spec(TextRole::MonthDayNumeral).tracking_em < 0.0);
    }
}
