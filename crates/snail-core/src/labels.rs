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
//! so the rest of the app never asks which provider a message came from. This module is the one
//! place that mapping lives: [`MailboxKind`] and the folder-name rules used to be in
//! `providers::imap`, which `providers::imap` now re-exports. The interesting part is that Gmail's
//! archive is a *removal* while iCloud's is a *move*; [`archive_operation`] names that.

/// The five mailboxes the handoff fixes, plus everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MailboxKind {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
    Other,
}

impl MailboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MailboxKind::Inbox => "inbox",
            MailboxKind::Sent => "sent",
            MailboxKind::Drafts => "drafts",
            MailboxKind::Archive => "archive",
            MailboxKind::Trash => "trash",
            MailboxKind::Other => "other",
        }
    }

    /// The name the five fixed mailboxes carry in the UI and the store.
    pub fn display_name(self) -> &'static str {
        match self {
            MailboxKind::Inbox => "Inbox",
            MailboxKind::Sent => "Sent",
            MailboxKind::Drafts => "Drafts",
            MailboxKind::Archive => "Archive",
            MailboxKind::Trash => "Trash",
            MailboxKind::Other => "Other",
        }
    }
}

/// Map a folder **name** (E4.18). iCloud advertises SPECIAL-USE for only `\Sent` and `\Trash`
/// (E0.4), so the rest is name matching, and the names are non-standard: `Sent Messages` and
/// `Deleted Messages`, not `Sent` and `Trash`.
pub fn map_folder(name: &str) -> MailboxKind {
    let trimmed = name.trim();
    // Strip any hierarchy prefix: `INBOX.Sent`, `Folders/Sent`.
    let leaf = trimmed
        .rsplit(['.', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed);
    match leaf.to_ascii_lowercase().as_str() {
        "inbox" => MailboxKind::Inbox,
        "sent" | "sent messages" | "sent items" => MailboxKind::Sent,
        "drafts" | "draft" => MailboxKind::Drafts,
        "archive" | "all mail" => MailboxKind::Archive,
        "trash" | "deleted messages" | "bin" => MailboxKind::Trash,
        _ => MailboxKind::Other,
    }
}

/// The SPECIAL-USE attributes, when the server provides them.
pub fn special_use(attributes: &[String]) -> Option<MailboxKind> {
    for attribute in attributes {
        match attribute
            .trim()
            .trim_start_matches('\\')
            .to_ascii_lowercase()
            .as_str()
        {
            "sent" => return Some(MailboxKind::Sent),
            "trash" => return Some(MailboxKind::Trash),
            "drafts" => return Some(MailboxKind::Drafts),
            "archive" => return Some(MailboxKind::Archive),
            "junk" | "flagged" | "all" | "nospecialuse" => {}
            _ => {}
        }
    }
    None
}

/// Prefer the flag, fall back to the name (E4.18).
pub fn resolve_kind(name: &str, attributes: &[String]) -> MailboxKind {
    special_use(attributes).unwrap_or_else(|| map_folder(name))
}

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

/// The one mailbox a Gmail message is filed under, from its `labelIds` (the precedence is
/// Gmail's: trash beats spam beats draft beats sent beats inbox, and no mailbox label means
/// archived).
pub fn gmail_primary_kind(label_ids: &[String]) -> MailboxKind {
    let has = |name: &str| label_ids.iter().any(|label| label.eq_ignore_ascii_case(name));
    if has("TRASH") {
        MailboxKind::Trash
    } else if has("SPAM") {
        MailboxKind::Other
    } else if has("DRAFT") {
        MailboxKind::Drafts
    } else if has("SENT") {
        MailboxKind::Sent
    } else if has("INBOX") {
        MailboxKind::Inbox
    } else {
        MailboxKind::Archive
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
    match map_folder(path) {
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

    fn attrs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn icloud_folder_names_map_to_the_five_mailboxes() {
        assert_eq!(map_folder("INBOX"), MailboxKind::Inbox);
        assert_eq!(map_folder("Sent Messages"), MailboxKind::Sent);
        assert_eq!(map_folder("Deleted Messages"), MailboxKind::Trash);
        assert_eq!(map_folder("Drafts"), MailboxKind::Drafts);
        assert_eq!(map_folder("Archive"), MailboxKind::Archive);
        assert_eq!(map_folder("Junk"), MailboxKind::Other);
        assert_eq!(map_folder("Notes"), MailboxKind::Other);
    }

    #[test]
    fn hierarchy_prefixes_are_stripped() {
        assert_eq!(map_folder("INBOX.Sent"), MailboxKind::Sent);
        assert_eq!(map_folder("Folders/Trash"), MailboxKind::Trash);
    }

    #[test]
    fn special_use_wins_over_the_name() {
        // E0.4 found iCloud advertises \Sent and \Trash; a differently-named folder still maps.
        assert_eq!(
            resolve_kind("Whatever", &attrs(&["\\Sent"])),
            MailboxKind::Sent
        );
        assert_eq!(
            resolve_kind("Deleted Messages", &attrs(&["\\Trash"])),
            MailboxKind::Trash
        );
        assert_eq!(
            resolve_kind("Sent Messages", &attrs(&[])),
            MailboxKind::Sent
        );
    }

    #[test]
    fn gmail_precedence_falls_out_of_the_labels() {
        // Trash wins over a still-present INBOX.
        assert_eq!(
            gmail_primary_kind(&labels(&["INBOX", "TRASH"])),
            MailboxKind::Trash
        );
        assert_eq!(gmail_primary_kind(&labels(&["INBOX", "UNREAD"])), MailboxKind::Inbox);
        assert_eq!(gmail_primary_kind(&labels(&["SENT"])), MailboxKind::Sent);
        assert_eq!(gmail_primary_kind(&labels(&["DRAFT"])), MailboxKind::Drafts);
        assert_eq!(gmail_primary_kind(&labels(&["SPAM"])), MailboxKind::Other);
        assert_eq!(gmail_primary_kind(&labels(&[])), MailboxKind::Archive);
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
            let from_gmail = gmail_primary_kind(gmail);
            let from_imap = resolve_kind(imap_path, &[]);
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
        assert_eq!(
            gmail_labels(&labels(&["INBOX", "Label_7"])),
            vec![Label::Inbox, Label::Named("Label_7".into())]
        );
    }

    #[test]
    fn gmail_flags_are_not_mailboxes() {
        assert_eq!(
            gmail_labels(&labels(&[
                "INBOX",
                "UNREAD",
                "STARRED",
                "IMPORTANT",
                "CATEGORY_PERSONAL"
            ])),
            vec![Label::Inbox]
        );
    }

    #[test]
    fn imap_unknown_folders_keep_their_path() {
        assert_eq!(
            imap_label("Receipts/2026"),
            Label::Named("Receipts/2026".into())
        );
        assert_eq!(imap_label("INBOX.Sent"), Label::Sent);
    }

    #[test]
    fn archiving_is_a_removal_on_gmail_and_a_move_on_imap() {
        assert_eq!(
            archive_operation(Provider::Gmail),
            ArchiveOperation::RemoveInbox
        );
        assert_eq!(
            archive_operation(Provider::Imap),
            ArchiveOperation::MoveToFolder("Archive")
        );
    }

    #[test]
    fn label_names_are_the_store_names() {
        assert_eq!(Label::Inbox.name(), "Inbox");
        assert_eq!(Label::Named("Work".into()).name(), "Work");
        assert_eq!(
            Label::Named("Work".into()).mailbox_kind(),
            MailboxKind::Other
        );
    }
}
