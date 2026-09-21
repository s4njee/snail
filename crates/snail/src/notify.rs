//! New-mail notifications (plan.md E16.8), posted through GPUI's system notifications.
//!
//! macOS delivers these only to a bundled app — `scripts/bundle.sh` — so under `cargo run` GPUI
//! logs "not running from an app bundle" and drops them. The first one asks for permission;
//! Do Not Disturb and Focus are the system's to apply.

use gpui_kit::{SharedString, SystemNotification};

use crate::mail_model::MessageRow;

/// Ask for permission to notify **and badge the app icon**, once, at launch. GPUI requests only
/// alerts and sounds when it posts its first notification; macOS then treats badges as off for
/// Snail and the Dock silently hides the unread label. Asking first, with all three, gets the one
/// prompt right. macOS prompts only the first time: if permission was already granted without
/// badges, it stays that way until the user turns on "Badge application icon" in System Settings →
/// Notifications → Snail.
pub fn request_permission() {
    platform::request_permission();
}

#[cfg(target_os = "macos")]
mod platform {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNNotificationSetting, UNNotificationSettings,
        UNUserNotificationCenter,
    };

    pub fn request_permission() {
        // Outside an app bundle `currentNotificationCenter` aborts the process (GPUI guards the
        // same way), so a `cargo run` build skips this.
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return;
        }
        let completion = RcBlock::new(|granted: Bool, error: *mut NSError| {
            // SAFETY: when non-null, `error` is a valid `NSError` for the callback's duration.
            if let Some(error) = unsafe { error.as_ref() } {
                log::warn!(
                    "notification permission failed: {}",
                    error.localizedDescription()
                );
            } else {
                log::info!("notification permission granted: {}", granted.as_bool());
            }
        });
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge,
            &completion,
        );
        // Say so when badges are off, because the Dock gives no sign: the label is set and simply
        // not drawn.
        let report = RcBlock::new(|settings: std::ptr::NonNull<UNNotificationSettings>| {
            // SAFETY: the settings object is valid for the callback's duration.
            let settings = unsafe { settings.as_ref() };
            if settings.badgeSetting() == UNNotificationSetting::Enabled {
                log::info!("dock badge allowed");
            } else {
                log::warn!(
                    "dock badge is turned off for Snail (setting {:?}); turn on \"Badge \
                     application icon\" in System Settings → Notifications → Snail",
                    settings.badgeSetting()
                );
            }
        });
        center.getNotificationSettingsWithCompletionHandler(&report);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn request_permission() {}
}

/// More than this many at once become one summary instead of a burst.
const MAX_INDIVIDUAL: usize = 3;

/// The tag a message's notification carries, so a click can open it and reading it can
/// withdraw it.
pub fn message_tag(id: i64) -> String {
    format!("message:{id}")
}

/// The message a clicked notification refers to; `None` for the summary.
pub fn message_from_tag(tag: &str) -> Option<i64> {
    tag.strip_prefix("message:")?.parse().ok()
}

/// The tag of the "N new messages" summary. Reusing it replaces an older summary.
pub const SUMMARY_TAG: &str = "new-mail";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub tag: String,
    pub title: String,
    pub body: String,
}

fn sender(row: &MessageRow) -> String {
    snail_ui::preview::single_line(
        row.from_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or(row.from_addr.as_deref())
            .unwrap_or("Unknown sender"),
    )
}

/// What to post for the mail that just arrived: one note per message, or a single summary.
pub fn notes(fresh: &[MessageRow]) -> Vec<Note> {
    if fresh.len() > MAX_INDIVIDUAL {
        let mut senders: Vec<String> = Vec::new();
        for name in fresh.iter().rev().map(sender) {
            if !senders.contains(&name) {
                senders.push(name);
            }
        }
        let body = match senders.as_slice() {
            [one] => format!("From {one}"),
            [one, two] => format!("From {one} and {two}"),
            [one, two, rest @ ..] => format!(
                "From {one}, {two} and {} other{}",
                rest.len(),
                if rest.len() == 1 { "" } else { "s" }
            ),
            [] => String::new(),
        };
        return vec![Note {
            tag: SUMMARY_TAG.into(),
            title: format!("{} new messages", fresh.len()),
            body,
        }];
    }
    fresh
        .iter()
        .map(|row| {
            let subject =
                snail_ui::preview::single_line(row.subject.as_deref().unwrap_or("(no subject)"));
            let preview = snail_ui::preview::single_line(row.preview.as_deref().unwrap_or(""));
            let body = if preview.is_empty() {
                subject
            } else {
                format!("{subject}\n{preview}")
            };
            Note {
                tag: message_tag(row.id),
                title: sender(row),
                body,
            }
        })
        .collect()
}

pub fn new_mail(fresh: &[MessageRow]) -> Vec<SystemNotification> {
    notes(fresh)
        .into_iter()
        .map(|note| SystemNotification {
            tag: SharedString::from(note.tag),
            title: SharedString::from(note.title),
            body: SharedString::from(note.body),
            actions: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, name: &str, subject: &str) -> MessageRow {
        MessageRow {
            id,
            subject: Some(subject.into()),
            from_name: Some(name.into()),
            from_addr: Some("x@example.com".into()),
            date: Some(0),
            preview: Some("Hello\nthere".into()),
            unread: true,
            body_hash: None,
        }
    }

    #[test]
    fn a_few_messages_are_announced_one_by_one() {
        let notes = notes(&[row(7, "Ada", "Lunch?"), row(8, "Grace", "Re: compilers")]);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "Ada");
        assert_eq!(notes[0].body, "Lunch?\nHello there");
        assert_eq!(message_from_tag(&notes[1].tag), Some(8));
    }

    #[test]
    fn a_burst_becomes_one_summary_naming_the_newest_senders() {
        let fresh: Vec<_> = (1..=6)
            .map(|id| {
                row(
                    id,
                    ["Ada", "Grace", "Ada", "Linus", "Ken", "Barbara"][id as usize - 1],
                    "s",
                )
            })
            .collect();
        let notes = notes(&fresh);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "6 new messages");
        assert_eq!(notes[0].body, "From Barbara, Ken and 3 others");
        assert_eq!(message_from_tag(&notes[0].tag), None);
    }

    #[test]
    fn a_missing_name_falls_back_to_the_address() {
        let mut anonymous = row(1, "", "Hi");
        anonymous.from_name = None;
        assert_eq!(notes(&[anonymous])[0].title, "x@example.com");
    }
}
