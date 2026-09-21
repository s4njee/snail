//! Triage actions and their inverses (plan.md E8.4/E8.5/E8.7/E8.8). The action is applied locally
//! immediately (optimistic) and queued remotely; the payload carries enough previous state that an
//! undo can restore the row and either cancel the queued op or issue its inverse.

use serde::{Deserialize, Serialize};

/// The five fixed mailboxes (E8.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mailbox {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
}

impl Mailbox {
    pub fn name(self) -> &'static str {
        match self {
            Mailbox::Inbox => "Inbox",
            Mailbox::Sent => "Sent",
            Mailbox::Drafts => "Drafts",
            Mailbox::Archive => "Archive",
            Mailbox::Trash => "Trash",
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Mailbox::Inbox => "inbox",
            Mailbox::Sent => "sent",
            Mailbox::Drafts => "drafts",
            Mailbox::Archive => "archive",
            Mailbox::Trash => "trash",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriageAction {
    Archive,
    Trash,
    MarkRead(bool),
    MarkFlagged(bool),
    /// A move to an arbitrary named mailbox (IMAP folders; Gmail labels, E8.6).
    Move {
        mailbox: String,
    },
}

impl TriageAction {
    pub fn operation(&self) -> &'static str {
        match self {
            TriageAction::Archive => "archive",
            TriageAction::Trash => "trash",
            TriageAction::MarkRead(_) => "mark_read",
            TriageAction::MarkFlagged(_) => "mark_flagged",
            TriageAction::Move { .. } => "move",
        }
    }

    /// The state-independent inverse (a flag toggle). Archive/Trash/Move need the previous mailbox,
    /// which lives in the payload, not the action.
    pub fn toggle_inverse(&self) -> Option<TriageAction> {
        match self {
            TriageAction::MarkRead(read) => Some(TriageAction::MarkRead(!read)),
            TriageAction::MarkFlagged(flagged) => Some(TriageAction::MarkFlagged(!flagged)),
            _ => None,
        }
    }
}

/// Everything an undo needs to put the row back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriagePayload {
    pub action: TriageAction,
    pub previous_mailbox: Option<String>,
    pub previous_unread: bool,
    pub previous_flagged: bool,
}

impl TriagePayload {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_toggles_invert_themselves() {
        assert_eq!(
            TriageAction::MarkRead(true).toggle_inverse(),
            Some(TriageAction::MarkRead(false))
        );
        assert_eq!(
            TriageAction::MarkFlagged(false).toggle_inverse(),
            Some(TriageAction::MarkFlagged(true))
        );
        assert_eq!(TriageAction::Archive.toggle_inverse(), None);
    }

    #[test]
    fn payload_round_trips() {
        let payload = TriagePayload {
            action: TriageAction::Move {
                mailbox: "Work".into(),
            },
            previous_mailbox: Some("Inbox".into()),
            previous_unread: false,
            previous_flagged: true,
        };
        assert_eq!(TriagePayload::from_json(&payload.to_json()), Some(payload));
    }
}
