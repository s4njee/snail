//! Labels and folders reconciled into one concept (plan.md E8.6).
//!
//! Gmail and IMAP disagree about what a "mailbox" is, and the difference shows up most clearly in
//! archive:
//!
//! | concept        | Gmail                              | iCloud IMAP                    |
//! |----------------|------------------------------------|--------------------------------|
//! | inbox          | the `INBOX` label                  | the `INBOX` folder             |
//! | archive        | **the absence of `INBOX`**         | move to the `Archive` folder   |
//! | trash          | add the `TRASH` label              | move to `Deleted Messages`     |
//! | other labels   | many per message (user labels)     | one folder per message         |
//!
//! Both providers project onto the same five fixed mailboxes (plus an account-specific remainder),
//! so the rest of the app never asks which provider a message came from. The interesting part is
//! that Gmail's archive is a *removal* while iCloud's is a *move*; [`archive_operation`] names that
//! so the sync loop does not have to.

use crate::providers::imap::{self, MailboxKind};

/// Which provider a message came from, for the places where the mapping genuinely differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Gmail,
    Imap,
}

/// One provider-neutral label: one of the five fixed mailboxes, or the account-specific remainder
/// (a Gmail user label name / an IMAP folder path).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Label {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
    /// The remainder: `Spam`, `Work`, `Receipts`, `Notes`, …
    Named(String),
}

impl Label {
    pub fn mailbox_kind(&self) -> MailboxKind {
        match self {
            Label::Inbox => MailboxKind::Inbox,
            Label::Sent => MailboxKind::Sent,
            Label::Drafts => MailboxKind::Drafts,
            Label::Archive => MailboxKind::Archive,
            Label::Trash => MailboxKind::Trash,
            Label::Named(_) => MailboxKind::Other,
        }
    }

    /// The name the store files it under: the five fixed names, or the provider's own name.
    pub fn name(&self) -> String {
        match self {
            Label::Named(name) => name.clone(),
            fixed => fixed.mailbox_kind().display_name().to_string(),
        }
    }
}

/// Gmail's `labelIds` → labels. Flag-only labels (`UNREAD`, `STARRED`, `IMPORTANT`, `CATEGORY_*`)
/// are not mailboxes and are dropped; the system mailbox labels map onto the fixed five; everything
/// else is the remainder. A message with none of the mailbox labels is archived — Gmail's "All
/// Mail" — which is why the absence of `INBOX` is itself a label.
pub fn gmail_labels(label_ids: &[String]) -> Vec<Label> {
    let mut labels = Vec::new();
    let mut has_mailbox = false;
    for id in label_ids {
        let (label, is_mailbox) = match id.as_str() {
            "INBOX" => (Label::Inbox, true),
            "SENT" => (Label::Sent, true),
            "DRAFT" => (Label::Drafts, true),
            "TRASH" => (Label::Trash, true),
            "SPAM" => (Label::Named("Spam".into()), true),
            // Flags, not mailboxes.
            "UNREAD" | "STARRED" | "IMPORTANT" | "CHAT" | "SNOOZED" | "SCHEDULED" => continue,
            other if other.starts_with("CATEGORY_") => continue,
            other => (Label::Named(other.to_string()), false),
        };
        has_mailbox |= is_mailbox;
        labels.push(label);
    }
    if !has_mailbox {
        labels.push(Label::Archive);
    }
    labels
}

/// An IMAP folder path → a label. `map_folder` matches iCloud's non-standard leaf names
/// (`Sent Messages`, `Deleted Messages`) after stripping any hierarchy prefix; anything else is
/// the remainder, named by its full path.
pub fn imap_label(path: &str) -> Label {
    match imap::map_folder(path) {
        MailboxKind::Inbox => Label::Inbox,
        MailboxKind::Sent => Label::Sent,
        MailboxKind::Drafts => Label::Drafts,
        MailboxKind::Archive => Label::Archive,
        MailboxKind::Trash => Label::Trash,
        MailboxKind::Other => Label::Named(path.to_string()),
    }
}

/// How archiving is expressed at the provider — the one place Gmail and IMAP truly differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveOperation {
    /// Gmail: remove `INBOX`. The message stays in All Mail.
    RemoveInbox,
    /// IMAP: move the message to the archive folder.
    MoveToFolder(&'static str),
}

pub fn archive_operation(provider: Provider) -> ArchiveOperation {
    match provider {
        Provider::Gmail => ArchiveOperation::RemoveInbox,
        Provider::Imap => ArchiveOperation::MoveToFolder(MailboxKind::Archive.display_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn both_providers_reach_the_same_five_mailboxes() {
        // The inbox, sent, drafts, archive and trash of each provider project onto one kind.
        let cases: &[(&str, Vec<String>, &str)] = &[
            ("INBOX", labels(&["INBOX"]), "INBOX"),
            ("SENT", labels(&["SENT"]), "Sent Messages"),
            ("DRAFT", labels(&["DRAFT"]), "Drafts"),
            ("archive", labels(&[]), "Archive"),
            ("TRASH", labels(&["TRASH"]), "Deleted Messages"),
        ];
        for (gmail_id, gmail, imap_path) in cases {
            let from_gmail = gmail_labels(gmail)
                .first()
                .map(Label::mailbox_kind)
                .expect(gmail_id);
            let from_imap = imap_label(imap_path).mailbox_kind();
            assert_eq!(from_gmail, from_imap, "{gmail_id} vs {imap_path}");
        }
    }

    #[test]
    fn gmail_archive_is_the_absence_of_a_mailbox_label() {
        assert_eq!(gmail_labels(&labels(&[])), vec![Label::Archive]);
        // A user label alone is still archived (no INBOX), but keeps the label.
        let with_user = gmail_labels(&labels(&["Label_7"]));
        assert!(with_user.contains(&Label::Archive));
        assert!(with_user.contains(&Label::Named("Label_7".into())));
        // INBOX present means it is not archived.
        assert_eq!(gmail_labels(&labels(&["INBOX", "Label_7"])), vec![
            Label::Inbox,
            Label::Named("Label_7".into())
        ]);
    }

    #[test]
    fn gmail_flags_are_not_mailboxes() {
        assert_eq!(
            gmail_labels(&labels(&["INBOX", "UNREAD", "STARRED", "IMPORTANT", "CATEGORY_PERSONAL"])),
            vec![Label::Inbox]
        );
    }

    #[test]
    fn imap_unknown_folders_keep_their_path() {
        assert_eq!(imap_label("Receipts/2026"), Label::Named("Receipts/2026".into()));
        assert_eq!(imap_label("INBOX.Sent"), Label::Sent);
    }

    #[test]
    fn archiving_is_a_removal_on_gmail_and_a_move_on_imap() {
        assert_eq!(archive_operation(Provider::Gmail), ArchiveOperation::RemoveInbox);
        assert_eq!(
            archive_operation(Provider::Imap),
            ArchiveOperation::MoveToFolder("Archive")
        );
    }

    #[test]
    fn label_names_are_the_store_names() {
        assert_eq!(Label::Inbox.name(), "Inbox");
        assert_eq!(Label::Named("Work".into()).name(), "Work");
        assert_eq!(Label::Named("Work".into()).mailbox_kind(), MailboxKind::Other);
    }
}
