//! The empty, loading and error states (plan.md E1.10). Neither handoff designs these, so the copy
//! and the icon live here — framework-free and testable — and the view layer draws them.

/// A hint for which icon to draw. Kept as an enum so `snail-ui` stays GPUI-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconHint {
    Envelope,
    Search,
    CloudOff,
    Alert,
    Calendar,
    Shield,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmptyState {
    /// No account has been added yet.
    NoAccounts,
    /// First sync is running and there is nothing to show yet.
    FirstSync,
    /// A mailbox with no messages.
    EmptyMailbox,
    /// A search that matched nothing.
    NoSearchResults,
    /// The message body could not be parsed (E6.13's fallback also failed).
    BodyFailed,
    /// Offline is the normal case, not an error (plan.md §2).
    Offline,
    /// A sync failed and needs attention.
    SyncError,
    /// A calendar range with no events.
    EmptyCalendarRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmptyCopy {
    pub icon: IconHint,
    pub title: &'static str,
    pub body: &'static str,
}

pub const ALL: &[EmptyState] = &[
    EmptyState::NoAccounts,
    EmptyState::FirstSync,
    EmptyState::EmptyMailbox,
    EmptyState::NoSearchResults,
    EmptyState::BodyFailed,
    EmptyState::Offline,
    EmptyState::SyncError,
    EmptyState::EmptyCalendarRange,
];

pub fn copy(state: EmptyState) -> EmptyCopy {
    match state {
        EmptyState::NoAccounts => EmptyCopy {
            icon: IconHint::Envelope,
            title: "No accounts yet",
            body: "Add a Google or iCloud account to start reading mail.",
        },
        EmptyState::FirstSync => EmptyCopy {
            icon: IconHint::Envelope,
            title: "Bringing in your mail",
            body: "The first sync runs in the background — you can keep working.",
        },
        EmptyState::EmptyMailbox => EmptyCopy {
            icon: IconHint::Envelope,
            title: "Nothing here",
            body: "This mailbox is empty.",
        },
        EmptyState::NoSearchResults => EmptyCopy {
            icon: IconHint::Search,
            title: "No matches",
            body: "Nothing matches in the last 30 days. Older mail is fetched on demand.",
        },
        EmptyState::BodyFailed => EmptyCopy {
            icon: IconHint::Alert,
            title: "This message couldn't be displayed",
            body: "The message body could not be parsed. Try opening it in your browser.",
        },
        EmptyState::Offline => EmptyCopy {
            icon: IconHint::CloudOff,
            title: "You're offline",
            body: "Showing the mail already on this machine. New mail arrives when you reconnect.",
        },
        EmptyState::SyncError => EmptyCopy {
            icon: IconHint::Alert,
            title: "Couldn't sync",
            body: "Your mail is still here. Snail will try again on its own.",
        },
        EmptyState::EmptyCalendarRange => EmptyCopy {
            icon: IconHint::Calendar,
            title: "Nothing scheduled",
            body: "No events in this range.",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_state_has_copy() {
        for state in ALL {
            let copy = copy(*state);
            assert!(!copy.title.trim().is_empty(), "{state:?} title");
            assert!(!copy.body.trim().is_empty(), "{state:?} body");
        }
    }

    #[test]
    fn all_states_are_listed_once() {
        let unique: HashSet<_> = ALL.iter().collect();
        assert_eq!(unique.len(), ALL.len());
        assert_eq!(ALL.len(), 8);
    }

    #[test]
    fn offline_is_not_framed_as_an_error() {
        // plan.md §2: "Offline is the normal case, not an error state."
        let copy = copy(EmptyState::Offline);
        assert!(!copy.title.to_lowercase().contains("error"));
        assert!(copy.title.to_lowercase().contains("offline"));
    }
}
