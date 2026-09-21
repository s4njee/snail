//! The unread badge on the app icon (plan.md E16.8). macOS draws it on the Dock tile; elsewhere
//! there is no equivalent GPUI can reach, so it is a no-op.

use std::sync::atomic::{AtomicI64, Ordering};

/// The last count shown, so a refresh that changes nothing does not touch AppKit.
static SHOWN: AtomicI64 = AtomicI64::new(-1);

/// The Dock label for an unread count: nothing at zero, and capped so the badge stays small.
pub fn badge_label(unread: i64) -> Option<String> {
    match unread {
        count if count <= 0 => None,
        count if count > 999 => Some("999+".into()),
        count => Some(count.to_string()),
    }
}

/// Show `unread` on the app icon. Must be called on the main thread (every caller is a view or
/// model update); off it, this does nothing.
pub fn set_unread(unread: i64) {
    if SHOWN.swap(unread, Ordering::Relaxed) == unread {
        return;
    }
    platform::set_label(badge_label(unread).as_deref());
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub fn set_label(label: Option<&str>) {
        let Some(main_thread) = MainThreadMarker::new() else {
            log::warn!("dock badge set off the main thread; ignoring");
            return;
        };
        let tile = NSApplication::sharedApplication(main_thread).dockTile();
        let label = label.map(NSString::from_str);
        tile.setBadgeLabel(label.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn set_label(_label: Option<&str>) {}
}

#[cfg(test)]
mod tests {
    use super::badge_label;

    #[test]
    fn the_badge_hides_at_zero_and_caps_large_counts() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(-3), None);
        assert_eq!(badge_label(7).as_deref(), Some("7"));
        assert_eq!(badge_label(999).as_deref(), Some("999"));
        assert_eq!(badge_label(12_345).as_deref(), Some("999+"));
    }
}
