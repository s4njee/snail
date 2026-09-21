//! Durable calendar-reminder scheduling (plan.md E13.7).
//!
//! Definitions live on events; this service expands only a bounded horizon into a delivery table.
//! The `(reminder, occurrence)` key is the exactly-once guard across process restarts and sleep.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use snail_core::ical::expand_event_between;
use snail_core::store::{DueReminder, ReminderKind, Store};

pub const MATERIALIZE_AHEAD_SECS: i64 = 45 * 24 * 60 * 60;
pub const LATE_GRACE_SECS: i64 = 24 * 60 * 60;
pub const DUE_BATCH: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReminderPass {
    pub materialized: usize,
    pub due: Vec<DueReminder>,
}

/// Rebuild the near-future queue and claim reminders due now. A machine waking after a short sleep
/// gets one late notification per reminder; older stale deliveries stay silent.
pub fn tick(store: &Store, now: i64) -> Result<ReminderPass> {
    let start = now.saturating_sub(LATE_GRACE_SECS);
    let end = now.saturating_add(MATERIALIZE_AHEAD_SECS);
    let mut pass = ReminderPass::default();
    for row in store.calendar_events_for_range(start, end, false)? {
        let reminders = store.reminders_for_event(row.id)?;
        if reminders.is_empty() {
            continue;
        }
        for occurrence in expand_event_between(&row.event, start, end)? {
            for reminder in &reminders {
                let due = reminder_due(
                    &reminder.kind,
                    occurrence.start_utc,
                    row.event.tz.as_deref(),
                )?;
                store.schedule_reminder_delivery(reminder.id, occurrence.start_utc, due)?;
                pass.materialized += 1;
            }
        }
    }
    pass.due = store.due_reminders(now, start, DUE_BATCH)?;
    Ok(pass)
}

pub fn acknowledge(store: &Store, reminders: &[DueReminder], now: i64) -> Result<()> {
    for reminder in reminders {
        store.mark_reminder_fired(reminder.reminder_id, reminder.occurrence_start, now)?;
    }
    Ok(())
}

fn reminder_due(kind: &ReminderKind, occurrence_start: i64, timezone: Option<&str>) -> Result<i64> {
    match kind {
        ReminderKind::MinutesBefore(minutes) => {
            Ok(occurrence_start.saturating_sub(minutes.saturating_mul(60)))
        }
        ReminderKind::Absolute(timestamp) => Ok(*timestamp),
        ReminderKind::SameDayMinute(minute) => {
            let zone: Tz = timezone.unwrap_or("Etc/UTC").parse().with_context(|| {
                format!("invalid event timezone {}", timezone.unwrap_or("Etc/UTC"))
            })?;
            let local = DateTime::<Utc>::from_timestamp(occurrence_start, 0)
                .context("invalid occurrence timestamp")?
                .with_timezone(&zone);
            let date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
                .context("invalid occurrence date")?;
            let time = date
                .and_hms_opt((*minute / 60) as u32, (*minute % 60) as u32, 0)
                .context("invalid same-day reminder time")?;
            zone.from_local_datetime(&time)
                .earliest()
                .context("same-day reminder falls in a missing local time")
                .map(|value| value.timestamp())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_same_day_reminders_obey_the_event_zone() {
        let occurrence = DateTime::parse_from_rfc3339("2026-03-08T09:00:00-04:00")
            .unwrap()
            .timestamp();
        assert_eq!(
            reminder_due(&ReminderKind::MinutesBefore(10), occurrence, None).unwrap(),
            occurrence - 600
        );
        let due = reminder_due(
            &ReminderKind::SameDayMinute(8 * 60),
            occurrence,
            Some("America/New_York"),
        )
        .unwrap();
        assert_eq!(
            DateTime::<Utc>::from_timestamp(due, 0)
                .unwrap()
                .to_rfc3339(),
            "2026-03-08T12:00:00+00:00"
        );
    }
}
