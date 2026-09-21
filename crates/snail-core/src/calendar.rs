//! Provider-neutral calendar synchronization primitives (plan.md E11).
//!
//! Google Calendar and CalDAV have very different wire models.  Everything above the provider
//! boundary sees these types instead: subscribed collections, recurrence masters, stable instance
//! identities, and optimistic writes with explicit conflict outcomes.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One subscribed calendar. Removing a subscription does not imply deleting its events (E11.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub remote_id: String,
    /// CalDAV's server-provided collection URL. Never reconstructed from another identifier.
    pub href: Option<String>,
    pub name: Option<String>,
    pub color: Option<String>,
    pub selected: bool,
    pub access_role: Option<String>,
    pub read_only: bool,
    pub subscribed: bool,
    pub ctag: Option<String>,
    pub supports_sync_collection: bool,
    /// CalDAV RFC 6638 schedule inbox/outbox are both present. iCloud RSVP stays disabled unless
    /// this capability was observed during discovery.
    pub supports_scheduling: bool,
}

/// The stable identity of an event or recurrence override (E11.8).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventIdentity {
    Event(String),
    Instance {
        recurring_event_id: String,
        original_start: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAttendee {
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
    pub status: String,
    pub is_self: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarReminder {
    MinutesBefore(i64),
    SameDayMinute(u16),
    Absolute(i64),
}

/// Provider-neutral recurrence master or exception.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub remote_id: String,
    /// The exact server-provided resource URL for CalDAV (E11.14).
    pub href: Option<String>,
    pub etag: Option<String>,
    pub ical_uid: Option<String>,
    pub recurring_event_id: Option<String>,
    pub original_start: Option<String>,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub start_utc: Option<i64>,
    pub end_utc: Option<i64>,
    pub all_day: bool,
    pub tz: Option<String>,
    pub rrule: Option<String>,
    pub rdate: Option<String>,
    pub exdate: Option<String>,
    pub status: Option<String>,
    pub sequence: Option<i64>,
    #[serde(default)]
    pub attendees: Vec<CalendarAttendee>,
    #[serde(default)]
    pub reminders: Vec<CalendarReminder>,
    /// CalDAV canonical server bytes. Kept separately from optimistic local serialization.
    pub raw_ical: Option<String>,
}

impl CalendarEvent {
    pub fn identity(&self) -> EventIdentity {
        match (&self.recurring_event_id, &self.original_start) {
            (Some(series), Some(original)) => EventIdentity::Instance {
                recurring_event_id: series.clone(),
                original_start: original.clone(),
            },
            _ => EventIdentity::Event(self.remote_id.clone()),
        }
    }
}

/// A pull result. Providers page internally and expose a token only from the terminal page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarBatch {
    pub calendars: Vec<CalendarInfo>,
    pub changes: Vec<EventChange>,
    pub next_sync_token: Option<String>,
    pub next_ctag: Option<String>,
    /// A provider invalidated just this collection's cursor (Google 410).
    pub full_resync_required: bool,
    /// A calendar disappeared from the subscription list; its event rows remain on disk.
    pub unsubscribed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventChange {
    Upsert(Box<CalendarEvent>),
    Delete(EventIdentity),
    /// A cancelled exception hides exactly one occurrence, not its live recurrence master.
    CancelInstance {
        recurring_event_id: String,
        original_start: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarCursor {
    pub sync_token: Option<String>,
    pub ctag: Option<String>,
    /// Exact href -> etag map used by CalDAV's ctag fallback.
    pub etags: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarWrite {
    Put {
        calendar: CalendarInfo,
        event: CalendarEvent,
        /// The last server ETag. CalDAV must send it as If-Match.
        base_etag: Option<String>,
    },
    Delete {
        calendar: CalendarInfo,
        event: CalendarEvent,
        base_etag: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Applied(Box<CalendarEvent>),
    Deleted(EventIdentity),
    /// HTTP 412 or an equivalent provider precondition failure.
    Conflict {
        local: Box<CalendarEvent>,
        server: Option<Box<CalendarEvent>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventEditScope {
    This,
    ThisAndFuture,
    All,
}

/// Provider-neutral local mutations for an edit/delete scope. Providers receive the resulting
/// ordinary event writes, so Google and CalDAV share the same recurrence semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopedEventMutation {
    pub master: Option<CalendarEvent>,
    pub exception: Option<CalendarEvent>,
    pub successor: Option<CalendarEvent>,
    pub delete_master: bool,
}

pub fn scoped_event_edit(
    master: &CalendarEvent,
    occurrence_start_utc: i64,
    replacement: &CalendarEvent,
    scope: EventEditScope,
) -> ScopedEventMutation {
    match scope {
        EventEditScope::All => {
            let mut edited = replacement.clone();
            edited.remote_id.clone_from(&master.remote_id);
            edited.href.clone_from(&master.href);
            edited.etag.clone_from(&master.etag);
            edited.ical_uid.clone_from(&master.ical_uid);
            edited.rrule = replacement.rrule.clone().or_else(|| master.rrule.clone());
            edited.rdate = replacement.rdate.clone().or_else(|| master.rdate.clone());
            edited.exdate = replacement.exdate.clone().or_else(|| master.exdate.clone());
            ScopedEventMutation {
                master: Some(edited),
                ..Default::default()
            }
        }
        EventEditScope::This => {
            let mut exception = replacement.clone();
            exception.remote_id = format!("{}#{occurrence_start_utc}", master.remote_id);
            exception.href = None;
            exception.etag = None;
            exception.ical_uid.clone_from(&master.ical_uid);
            exception.recurring_event_id = Some(master.remote_id.clone());
            exception.original_start = Some(occurrence_start_utc.to_string());
            exception.rrule = None;
            exception.rdate = None;
            exception.exdate = None;
            exception.raw_ical = None;
            ScopedEventMutation {
                exception: Some(exception),
                ..Default::default()
            }
        }
        EventEditScope::ThisAndFuture => {
            let mut old = master.clone();
            old.rrule = old
                .rrule
                .as_deref()
                .map(|rule| rrule_ending_before(rule, occurrence_start_utc));
            let mut successor = replacement.clone();
            successor.remote_id = format!("{}#future-{occurrence_start_utc}", master.remote_id);
            successor.href = None;
            successor.etag = None;
            successor.ical_uid = Some(format!(
                "{}-future-{occurrence_start_utc}",
                master.ical_uid.as_deref().unwrap_or(&master.remote_id)
            ));
            successor.recurring_event_id = None;
            successor.original_start = None;
            successor.rrule = master.rrule.as_deref().map(rrule_without_end);
            successor.rdate = None;
            successor.exdate = None;
            successor.raw_ical = None;
            ScopedEventMutation {
                master: Some(old),
                successor: Some(successor),
                ..Default::default()
            }
        }
    }
}

pub fn scoped_event_delete(
    master: &CalendarEvent,
    occurrence_start_utc: i64,
    scope: EventEditScope,
) -> ScopedEventMutation {
    match scope {
        EventEditScope::All => ScopedEventMutation {
            delete_master: true,
            ..Default::default()
        },
        EventEditScope::This => {
            let mut edited = master.clone();
            let date = ical_utc(occurrence_start_utc);
            let mut values: Vec<_> = edited
                .exdate
                .as_deref()
                .into_iter()
                .flat_map(str::lines)
                .map(str::to_string)
                .collect();
            if !values.contains(&date) {
                values.push(date);
            }
            edited.exdate = Some(values.join("\n"));
            ScopedEventMutation {
                master: Some(edited),
                ..Default::default()
            }
        }
        EventEditScope::ThisAndFuture => {
            let mut edited = master.clone();
            edited.rrule = edited
                .rrule
                .as_deref()
                .map(|rule| rrule_ending_before(rule, occurrence_start_utc));
            ScopedEventMutation {
                master: Some(edited),
                ..Default::default()
            }
        }
    }
}

fn rrule_without_end(rule: &str) -> String {
    rule.split(';')
        .filter(|part| {
            let key = part.split_once('=').map(|(key, _)| key).unwrap_or(part);
            !key.eq_ignore_ascii_case("COUNT") && !key.eq_ignore_ascii_case("UNTIL")
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn rrule_ending_before(rule: &str, occurrence_start_utc: i64) -> String {
    let mut value = rrule_without_end(rule);
    if !value.is_empty() {
        value.push(';');
    }
    value.push_str("UNTIL=");
    value.push_str(&ical_utc(occurrence_start_utc.saturating_sub(1)));
    value
}

fn ical_utc(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|date| date.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| "19700101T000000Z".into())
}

/// One boundary shared by Google Calendar and iCloud CalDAV (E11 introduction).
pub trait CalendarProvider: Send + Sync {
    fn discover_calendars(&self, cursor: Option<&str>) -> Result<CalendarBatch>;
    fn sync_calendar(
        &self,
        calendar: &CalendarInfo,
        cursor: &CalendarCursor,
    ) -> Result<CalendarBatch>;
    fn apply(&self, writes: &[CalendarWrite]) -> Result<Vec<WriteOutcome>>;
}

/// The foreground polling interval with Google's recommended ±25% jitter (E11.22).
pub fn poll_interval_secs(base_secs: u64, jitter_unit: f64, on_battery: bool) -> u64 {
    let base = if on_battery {
        base_secs.saturating_mul(4)
    } else {
        base_secs
    };
    let bounded = jitter_unit.clamp(0.0, 1.0);
    let multiplier = 0.75 + bounded * 0.5;
    ((base as f64) * multiplier).round().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_instances_keep_their_original_identity() {
        let event = CalendarEvent {
            remote_id: "opaque-instance-id".into(),
            recurring_event_id: Some("series".into()),
            original_start: Some("2026-03-08T09:00:00-05:00".into()),
            start_utc: Some(1_773_064_800),
            ..Default::default()
        };
        assert_eq!(
            event.identity(),
            EventIdentity::Instance {
                recurring_event_id: "series".into(),
                original_start: "2026-03-08T09:00:00-05:00".into(),
            }
        );
    }

    #[test]
    fn poll_jitter_is_bounded_and_battery_is_slower() {
        assert_eq!(poll_interval_secs(600, 0.0, false), 450);
        assert_eq!(poll_interval_secs(600, 1.0, false), 750);
        assert_eq!(poll_interval_secs(600, 0.5, true), 2400);
    }

    #[test]
    fn recurrence_scope_this_creates_a_stable_exception() {
        let master = CalendarEvent {
            remote_id: "series".into(),
            ical_uid: Some("series@example.com".into()),
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            ..Default::default()
        };
        let replacement = CalendarEvent {
            summary: Some("Moved".into()),
            start_utc: Some(1_800_000_000),
            end_utc: Some(1_800_003_600),
            ..Default::default()
        };
        let mutation =
            scoped_event_edit(&master, 1_799_000_000, &replacement, EventEditScope::This);
        let exception = mutation.exception.unwrap();
        assert_eq!(exception.recurring_event_id.as_deref(), Some("series"));
        assert_eq!(exception.original_start.as_deref(), Some("1799000000"));
        assert!(exception.rrule.is_none());
    }

    #[test]
    fn recurrence_scope_future_splits_without_count_or_old_until() {
        let master = CalendarEvent {
            remote_id: "series".into(),
            rrule: Some("FREQ=MONTHLY;COUNT=20;BYDAY=MO;BYSETPOS=1".into()),
            ..Default::default()
        };
        let mutation = scoped_event_edit(
            &master,
            1_800_000_000,
            &CalendarEvent::default(),
            EventEditScope::ThisAndFuture,
        );
        assert!(mutation.master.unwrap().rrule.unwrap().contains("UNTIL="));
        let successor = mutation.successor.unwrap().rrule.unwrap();
        assert!(successor.contains("BYSETPOS=1"));
        assert!(!successor.contains("COUNT="));
        assert!(!successor.contains("UNTIL="));
    }

    #[test]
    fn recurrence_delete_this_adds_exdate_once() {
        let master = CalendarEvent {
            remote_id: "series".into(),
            rrule: Some("FREQ=DAILY".into()),
            ..Default::default()
        };
        let first = scoped_event_delete(&master, 1_800_000_000, EventEditScope::This)
            .master
            .unwrap();
        let second = scoped_event_delete(&first, 1_800_000_000, EventEditScope::This)
            .master
            .unwrap();
        assert_eq!(second.exdate.unwrap().lines().count(), 1);
    }
}
