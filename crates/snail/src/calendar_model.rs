//! Calendar reads and range expansion for the shell (plan.md E12.9/E12.11/E12.13).
//!
//! Like `mail_model`, this is a model boundary: views never touch SQLite. A load is synchronous so
//! the shell can move the whole operation to GPUI's background executor and generation-guard it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use snail_core::calendar::{
    CalendarAttendee, CalendarEvent, CalendarReminder, CalendarWrite, EventEditScope,
    scoped_event_delete, scoped_event_edit,
};
use snail_core::ical::{
    expand_event_between, organizer_address, serialize_attendee_reply, serialize_event,
};
use snail_core::store::{AttendeeRow, ReminderKind, Store};
use snail_ui::calendar::{CivilDate, RangePlan, RenderedInterval, render_in_zone};

#[derive(Clone)]
pub struct CalendarModel {
    store: Arc<Store>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarListItem {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub visible: bool,
    pub event_count: i64,
    pub supports_scheduling: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarOccurrence {
    pub event_id: i64,
    pub occurrence_start_utc: i64,
    pub calendar_id: i64,
    pub calendar_name: String,
    pub color: String,
    pub title: String,
    pub location: Option<String>,
    pub timezone: Option<String>,
    pub rendered: RenderedInterval,
}

#[derive(Clone, Debug)]
pub struct CalendarLoad {
    pub plan: RangePlan,
    pub local_timezone: String,
    pub calendars: Vec<CalendarListItem>,
    pub occurrences: Vec<CalendarOccurrence>,
    pub tasks: Vec<CalendarTask>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarTask {
    pub id: i64,
    pub title: String,
    pub notes: Option<String>,
    pub due_date: Option<CivilDate>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct EventDraft {
    pub event_id: Option<i64>,
    pub occurrence_start_utc: Option<i64>,
    pub calendar_id: i64,
    pub title: String,
    pub location: String,
    pub notes: String,
    pub start_utc: i64,
    pub end_utc: i64,
    pub all_day: bool,
    pub timezone: String,
    pub rrule: Option<String>,
    pub reminders: Vec<ReminderKind>,
    pub attendees: Vec<AttendeeRow>,
    pub scope: EventEditScope,
}

#[derive(Clone, Debug)]
pub struct EventDetails {
    pub draft: EventDraft,
    pub is_recurring: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RsvpDelivery {
    ServerScheduled,
    Imip {
        recipient: String,
        calendar_reply: String,
    },
}

impl CalendarModel {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn local_timezone() -> String {
        iana_time_zone::get_timezone().unwrap_or_else(|_| "Etc/UTC".into())
    }

    pub fn set_visible(&self, calendar_id: i64, visible: bool) -> Result<()> {
        self.store.set_calendar_visible(calendar_id, visible)
    }

    pub fn calendars(&self) -> Vec<CalendarListItem> {
        let mut rows = Vec::new();
        for account in self.store.accounts().unwrap_or_default() {
            rows.extend(
                self.store
                    .calendars_for_account(account.id)
                    .unwrap_or_default(),
            );
        }
        rows.into_iter()
            .map(|calendar| CalendarListItem {
                id: calendar.id,
                name: calendar.info.name.unwrap_or_else(|| "Calendar".into()),
                color: calendar.info.color.unwrap_or_else(|| "#2c5fb8".into()),
                visible: calendar.visible,
                event_count: calendar.event_count,
                supports_scheduling: calendar.info.supports_scheduling,
            })
            .collect()
    }

    pub fn event_details(
        &self,
        event_id: i64,
        occurrence_start_utc: i64,
    ) -> Result<Option<EventDetails>> {
        let Some(row) = self.store.event_by_id(event_id)? else {
            return Ok(None);
        };
        let duration = row
            .event
            .end_utc
            .unwrap_or(occurrence_start_utc + 3600)
            .saturating_sub(row.event.start_utc.unwrap_or(occurrence_start_utc));
        let is_recurring = row.event.rrule.is_some() || row.event.rdate.is_some();
        Ok(Some(EventDetails {
            draft: EventDraft {
                event_id: Some(row.id),
                occurrence_start_utc: Some(occurrence_start_utc),
                calendar_id: row.calendar_id,
                title: row.event.summary.unwrap_or_default(),
                location: row.event.location.unwrap_or_default(),
                notes: row.event.description.unwrap_or_default(),
                start_utc: occurrence_start_utc,
                end_utc: occurrence_start_utc.saturating_add(duration.max(60)),
                all_day: row.event.all_day,
                timezone: row.event.tz.unwrap_or_else(Self::local_timezone),
                rrule: row.event.rrule,
                reminders: self
                    .store
                    .reminders_for_event(row.id)?
                    .into_iter()
                    .map(|r| r.kind)
                    .collect(),
                attendees: self.store.attendees_for_event(row.id)?,
                scope: EventEditScope::All,
            },
            is_recurring,
        }))
    }

    pub fn save_event(&self, draft: &EventDraft, now: i64) -> Result<Vec<i64>> {
        let calendar = self
            .store
            .calendar_by_id(draft.calendar_id)?
            .context("calendar no longer exists")?;
        if calendar.info.read_only {
            anyhow::bail!("calendar is read-only");
        }
        let mut replacement = CalendarEvent {
            remote_id: format!("local-{}-{}-{}", draft.calendar_id, now, std::process::id()),
            ical_uid: Some(format!("{}@snail.local", now)),
            summary: Some(draft.title.trim().to_string()),
            location: (!draft.location.trim().is_empty()).then(|| draft.location.trim().into()),
            description: (!draft.notes.trim().is_empty()).then(|| draft.notes.trim().into()),
            start_utc: Some(draft.start_utc),
            end_utc: Some(draft.end_utc.max(draft.start_utc + 60)),
            all_day: draft.all_day,
            tz: Some(draft.timezone.clone()),
            rrule: draft.rrule.clone(),
            sequence: Some(0),
            attendees: draft
                .attendees
                .iter()
                .map(|attendee| CalendarAttendee {
                    email: attendee.email.clone(),
                    display_name: attendee.display_name.clone(),
                    role: attendee.role.clone(),
                    status: attendee.status.clone(),
                    is_self: attendee.is_self,
                })
                .collect(),
            reminders: draft
                .reminders
                .iter()
                .map(|reminder| match reminder {
                    ReminderKind::MinutesBefore(value) => CalendarReminder::MinutesBefore(*value),
                    ReminderKind::SameDayMinute(value) => CalendarReminder::SameDayMinute(*value),
                    ReminderKind::Absolute(value) => CalendarReminder::Absolute(*value),
                })
                .collect(),
            ..Default::default()
        };
        let mutation = if let Some(event_id) = draft.event_id {
            let original = self
                .store
                .event_by_id(event_id)?
                .context("event no longer exists")?;
            replacement.remote_id.clone_from(&original.event.remote_id);
            replacement.href.clone_from(&original.event.href);
            replacement.etag.clone_from(&original.event.etag);
            replacement.ical_uid.clone_from(&original.event.ical_uid);
            replacement.sequence = Some(original.event.sequence.unwrap_or(0) + 1);
            let occurrence = draft.occurrence_start_utc.unwrap_or(draft.start_utc);
            scoped_event_edit(&original.event, occurrence, &replacement, draft.scope)
        } else {
            snail_core::calendar::ScopedEventMutation {
                master: Some(replacement),
                ..Default::default()
            }
        };
        let mut ids = Vec::new();
        for (suffix, event) in [
            ("master", mutation.master),
            ("exception", mutation.exception),
            ("successor", mutation.successor),
        ] {
            let Some(mut event) = event else { continue };
            event.raw_ical = Some(serialize_event(&event)?);
            let write = CalendarWrite::Put {
                calendar: calendar.info.clone(),
                base_etag: event.etag.clone(),
                event: event.clone(),
            };
            let id = self.store.queue_calendar_write(
                calendar.account_id,
                draft.calendar_id,
                &event,
                &write,
                &format!(
                    "event:{}:{suffix}:{}",
                    event.remote_id,
                    event.sequence.unwrap_or(0)
                ),
                now,
            )?;
            self.store.replace_event_reminders(id, &draft.reminders)?;
            self.store.replace_event_attendees(id, &draft.attendees)?;
            ids.push(id);
        }
        Ok(ids)
    }

    pub fn delete_event(
        &self,
        event_id: i64,
        occurrence_start_utc: i64,
        scope: EventEditScope,
        now: i64,
    ) -> Result<()> {
        let row = self
            .store
            .event_by_id(event_id)?
            .context("event no longer exists")?;
        let calendar = self
            .store
            .calendar_by_id(row.calendar_id)?
            .context("calendar no longer exists")?;
        let recurring = row.event.rrule.is_some() || row.event.rdate.is_some();
        if recurring && scope != EventEditScope::All {
            let mutation = scoped_event_delete(&row.event, occurrence_start_utc, scope);
            if let Some(mut event) = mutation.master {
                event.raw_ical = Some(serialize_event(&event)?);
                let write = CalendarWrite::Put {
                    calendar: calendar.info,
                    base_etag: row.event.etag.clone(),
                    event: event.clone(),
                };
                self.store.queue_calendar_write(
                    calendar.account_id,
                    row.calendar_id,
                    &event,
                    &write,
                    &format!(
                        "event:{}:delete-scope:{occurrence_start_utc}",
                        event.remote_id
                    ),
                    now,
                )?;
            }
        } else {
            let mut hidden = row.event.clone();
            hidden.status = Some("cancelled".into());
            let write = CalendarWrite::Delete {
                calendar: calendar.info,
                event: row.event.clone(),
                base_etag: row.event.etag.clone(),
            };
            self.store.queue_calendar_write(
                calendar.account_id,
                row.calendar_id,
                &hidden,
                &write,
                &format!("event:{}:delete", row.event.remote_id),
                now,
            )?;
        }
        Ok(())
    }

    pub fn create_task(&self, title: &str, due_utc: Option<i64>, now: i64) -> Result<i64> {
        let account = self
            .store
            .accounts()?
            .into_iter()
            .next()
            .context("no account")?;
        self.store
            .insert_task(account.id, title, None, due_utc, now)
    }

    pub fn set_task_done(&self, task_id: i64, done: bool) -> Result<()> {
        self.store.set_task_done(task_id, done)
    }

    pub fn rsvp_invite(
        &self,
        invite: &CalendarEvent,
        status: &str,
        now: i64,
    ) -> Result<RsvpDelivery> {
        let organizer = invite.raw_ical.as_deref().and_then(organizer_address);
        let accounts = self.store.accounts()?;
        let self_addresses: HashSet<_> = accounts
            .iter()
            .map(|account| account.address.to_ascii_lowercase())
            .collect();
        let existing = invite
            .ical_uid
            .as_deref()
            .map(|uid| self.store.event_by_ical_uid(uid))
            .transpose()?
            .flatten();
        let (calendar, mut event) = if let Some(row) = existing.as_ref() {
            (
                self.store
                    .calendar_by_id(row.calendar_id)?
                    .context("calendar no longer exists")?,
                row.event.clone(),
            )
        } else {
            let calendar = accounts
                .iter()
                .flat_map(|account| {
                    self.store
                        .calendars_for_account(account.id)
                        .unwrap_or_default()
                })
                .find(|calendar| !calendar.info.read_only)
                .context("no writable calendar for invitation")?;
            let mut event = invite.clone();
            event.remote_id = format!("local-invite-{}-{}", now, std::process::id());
            event.href = None;
            event.etag = None;
            (calendar, event)
        };
        let mut attendees = existing
            .as_ref()
            .map(|row| self.store.attendees_for_event(row.id))
            .transpose()?
            .unwrap_or_else(|| {
                invite
                    .attendees
                    .iter()
                    .map(|attendee| AttendeeRow {
                        id: 0,
                        event_id: 0,
                        email: attendee.email.clone(),
                        display_name: attendee.display_name.clone(),
                        role: attendee.role.clone(),
                        status: attendee.status.clone(),
                        is_self: self_addresses.contains(&attendee.email.to_ascii_lowercase()),
                    })
                    .collect()
            });
        let target_index = attendees
            .iter()
            .position(|attendee| attendee.is_self)
            .or_else(|| {
                attendees.iter().position(|attendee| {
                    self_addresses.contains(&attendee.email.to_ascii_lowercase())
                })
            })
            .context("invitation does not include this account")?;
        let target = &mut attendees[target_index];
        target.is_self = true;
        target.status = status.into();
        let attendee_email = target.email.clone();
        event.attendees = attendees
            .iter()
            .map(|attendee| CalendarAttendee {
                email: attendee.email.clone(),
                display_name: attendee.display_name.clone(),
                role: attendee.role.clone(),
                status: attendee.status.clone(),
                is_self: attendee.is_self,
            })
            .collect();
        event.sequence = Some(event.sequence.unwrap_or(0) + 1);
        event.raw_ical = Some(serialize_event(&event)?);
        if !calendar.info.supports_scheduling {
            let recipient = organizer.context(
                "this server does not schedule invitations and the invite has no organizer",
            )?;
            let calendar_reply =
                serialize_attendee_reply(invite, &attendee_email, status, &recipient)?;
            return Ok(RsvpDelivery::Imip {
                recipient,
                calendar_reply,
            });
        }
        let write = CalendarWrite::Put {
            calendar: calendar.info.clone(),
            base_etag: event.etag.clone(),
            event: event.clone(),
        };
        let event_id = self.store.queue_calendar_write(
            calendar.account_id,
            calendar.id,
            &event,
            &write,
            &format!(
                "event:{}:rsvp:{status}:{}",
                event.remote_id,
                event.sequence.unwrap_or(0)
            ),
            now,
        )?;
        self.store.replace_event_attendees(event_id, &attendees)?;
        Ok(RsvpDelivery::ServerScheduled)
    }

    pub fn load(&self, plan: RangePlan, local_timezone: &str) -> Result<CalendarLoad> {
        let zone: Tz = local_timezone
            .parse()
            .with_context(|| format!("invalid local timezone {local_timezone}"))?;
        let start_utc = local_midnight(plan.load_start, zone)?;
        let end_utc = local_midnight(plan.load_end, zone)?;

        let mut calendar_rows = Vec::new();
        for account in self.store.accounts()? {
            calendar_rows.extend(self.store.calendars_for_account(account.id)?);
        }
        let calendars: Vec<_> = calendar_rows
            .iter()
            .map(|calendar| CalendarListItem {
                id: calendar.id,
                name: calendar
                    .info
                    .name
                    .clone()
                    .unwrap_or_else(|| "Calendar".into()),
                color: calendar
                    .info
                    .color
                    .clone()
                    .unwrap_or_else(|| "#2c5fb8".into()),
                visible: calendar.visible,
                event_count: calendar.event_count,
                supports_scheduling: calendar.info.supports_scheduling,
            })
            .collect();
        let calendar_meta: HashMap<_, _> = calendars
            .iter()
            .map(|calendar| (calendar.id, calendar.clone()))
            .collect();

        let rows = self
            .store
            .calendar_events_for_range(start_utc, end_utc, true)?;
        let mut overridden: HashMap<String, HashSet<i64>> = HashMap::new();
        for row in &rows {
            if let (Some(series), Some(original)) = (
                row.event.recurring_event_id.as_ref(),
                row.event.original_start.as_deref(),
            ) && let Some(timestamp) = original_timestamp(original)
            {
                overridden
                    .entry(series.clone())
                    .or_default()
                    .insert(timestamp);
            }
        }

        let mut occurrences = Vec::new();
        for row in rows {
            if row
                .event
                .status
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("cancelled"))
            {
                continue;
            }
            let Some(calendar) = calendar_meta.get(&row.calendar_id) else {
                continue;
            };
            let mut excluded = HashSet::new();
            if row.event.recurring_event_id.is_none() {
                excluded.extend(
                    overridden
                        .get(&row.event.remote_id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
                for original in self.store.cancelled_occurrences(row.id)? {
                    if let Some(timestamp) = original_timestamp(&original) {
                        excluded.insert(timestamp);
                    }
                }
            }
            let expanded = match expand_event_between(&row.event, start_utc, end_utc) {
                Ok(expanded) => expanded,
                Err(error) => {
                    log::warn!(
                        "could not expand calendar event {}: {error:#}",
                        row.event.remote_id
                    );
                    continue;
                }
            };
            for occurrence in expanded {
                if excluded.contains(&occurrence.start_utc) {
                    continue;
                }
                let Some(rendered) = render_in_zone(
                    occurrence.start_utc,
                    occurrence.end_utc,
                    row.event.tz.as_deref(),
                    local_timezone,
                    row.event.all_day,
                ) else {
                    continue;
                };
                occurrences.push(CalendarOccurrence {
                    event_id: row.id,
                    occurrence_start_utc: occurrence.start_utc,
                    calendar_id: row.calendar_id,
                    calendar_name: calendar.name.clone(),
                    color: calendar.color.clone(),
                    title: row
                        .event
                        .summary
                        .clone()
                        .unwrap_or_else(|| "Untitled event".into()),
                    location: row.event.location.clone(),
                    timezone: row.event.tz.clone(),
                    rendered,
                });
            }
        }
        occurrences.sort_by_key(|event| {
            (
                event.rendered.local_date,
                event.rendered.local_start_minute,
                event.event_id,
                event.occurrence_start_utc,
            )
        });
        let tasks = self
            .store
            .tasks_between(start_utc, end_utc)?
            .into_iter()
            .map(|task| CalendarTask {
                id: task.id,
                title: task.title,
                notes: task.notes,
                due_date: task.due_utc.and_then(|due| {
                    DateTime::<Utc>::from_timestamp(due, 0).map(|due| {
                        let local = due.with_timezone(&zone);
                        CivilDate::new(local.year(), local.month(), local.day()).unwrap()
                    })
                }),
                done: task.done,
            })
            .collect();
        Ok(CalendarLoad {
            plan,
            local_timezone: local_timezone.into(),
            calendars,
            occurrences,
            tasks,
        })
    }
}

fn local_midnight(date: CivilDate, zone: Tz) -> Result<i64> {
    let naive = date
        .to_naive()
        .and_hms_opt(0, 0, 0)
        .context("invalid local midnight")?;
    zone.from_local_datetime(&naive)
        .earliest()
        .context("local midnight does not exist")
        .map(|date| date.timestamp())
}

fn original_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp())
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|date| date.and_utc().timestamp())
        })
        .or_else(|| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_instance_identity_accepts_google_date_shapes() {
        assert_eq!(
            original_timestamp("2026-08-13T09:00:00-05:00"),
            DateTime::parse_from_rfc3339("2026-08-13T09:00:00-05:00")
                .ok()
                .map(|date| date.timestamp())
        );
        assert_eq!(
            original_timestamp("2026-08-13"),
            NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc().timestamp())
        );
    }

    #[test]
    fn buffered_date_bounds_respect_dst_midnights() {
        let zone: Tz = "America/Chicago".parse().unwrap();
        let first = local_midnight(CivilDate::new(2026, 3, 8).unwrap(), zone).unwrap();
        let next = local_midnight(CivilDate::new(2026, 3, 9).unwrap(), zone).unwrap();
        assert_eq!(next - first, 23 * 60 * 60);
    }
}
