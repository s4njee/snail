//! Opt-in ten-minute iCloud capability probe required by plan.md E11.13.
//!
//! Run with:
//! `SNAIL_ICLOUD_USER=... SNAIL_ICLOUD_APP_PASSWORD=... cargo test -p snail-core --test caldav_live -- --ignored --nocapture`

use std::thread;
use std::time::Duration;

use snail_core::calendar::{CalendarCursor, CalendarProvider};
use snail_core::providers::caldav::CalDavClient;

#[test]
#[ignore = "requires an iCloud account, app-specific password, network, and ten minutes"]
fn probe_icloud_incremental_sync_for_ten_minutes() {
    let user = std::env::var("SNAIL_ICLOUD_USER").expect("SNAIL_ICLOUD_USER");
    let password = std::env::var("SNAIL_ICLOUD_APP_PASSWORD").expect("SNAIL_ICLOUD_APP_PASSWORD");
    let client = CalDavClient::new(&user, &password).unwrap();
    let discovered = client.discover_calendars(None).unwrap();
    assert!(
        !discovered.calendars.is_empty(),
        "iCloud returned no VEVENT calendars"
    );
    for calendar in &discovered.calendars {
        eprintln!(
            "{}: sync-collection={}, ctag={:?}",
            calendar.name.as_deref().unwrap_or(&calendar.remote_id),
            calendar.supports_sync_collection,
            calendar.ctag
        );
    }
    let calendar = &discovered.calendars[0];
    let mut cursor = CalendarCursor::default();
    for minute in 0..=10 {
        let batch = client.sync_calendar(calendar, &cursor).unwrap();
        eprintln!(
            "minute {minute}: changes={}, sync_token={}, ctag={:?}",
            batch.changes.len(),
            batch.next_sync_token.is_some(),
            batch.next_ctag
        );
        cursor.sync_token = batch.next_sync_token.or(cursor.sync_token);
        cursor.ctag = batch.next_ctag.or(cursor.ctag);
        if minute != 10 {
            thread::sleep(Duration::from_secs(60));
        }
    }
}
