//! iCloud IMAP specifics that are pure logic (E4.18). The live connection lives behind
//! `MailProvider`; the interesting part is mapping iCloud's non-standard folder names onto the
//! handoff's five fixed mailboxes.

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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(resolve_kind("Sent Messages", &attrs(&[])), MailboxKind::Sent);
    }
}
