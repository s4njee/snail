//! Google Calendar v3 synchronization (plan.md E11.1–E11.11).
//!
//! The client deliberately syncs recurrence masters (`singleEvents=false`) and keeps an independent
//! token per collection. Paging is completed inside a call, so a caller can only persist the
//! terminal page's `nextSyncToken`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use url::form_urlencoded::byte_serialize;

use super::gmail::TokenSource;
use crate::calendar::{
    CalendarAttendee, CalendarBatch, CalendarCursor, CalendarEvent, CalendarInfo, CalendarProvider,
    CalendarReminder, CalendarWrite, EventChange, EventIdentity, WriteOutcome,
};

const BASE: &str = "https://www.googleapis.com/calendar/v3";
pub const USER_REQUESTS_PER_MINUTE: usize = 600;
pub const PROJECT_REQUESTS_PER_MINUTE: usize = 10_000;
pub const PROJECT_REQUESTS_PER_DAY: usize = 1_000_000;

#[derive(Debug)]
pub struct GoogleCalendarError {
    pub status: u16,
    pub reason: String,
    pub retryable: bool,
    pub auth: bool,
}

impl std::fmt::Display for GoogleCalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Google Calendar {}: {}", self.status, self.reason)
    }
}

impl std::error::Error for GoogleCalendarError {}

/// A per-account request budget. Calendar quota counts requests, not Gmail's weighted units.
#[derive(Default)]
struct RequestBudget {
    recent: Mutex<VecDeque<Instant>>,
}

impl RequestBudget {
    fn charge(&self) -> Result<()> {
        let now = Instant::now();
        let mut recent = self.recent.lock().expect("calendar quota mutex poisoned");
        while recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            recent.pop_front();
        }
        if recent.len() >= USER_REQUESTS_PER_MINUTE {
            bail!(
                "Google Calendar per-user request budget exhausted; retry after the minute window"
            );
        }
        recent.push_back(now);
        Ok(())
    }
}

pub struct GoogleCalendarClient {
    http: Client,
    tokens: Arc<dyn TokenSource>,
    budget: RequestBudget,
}

impl GoogleCalendarClient {
    pub fn new(tokens: Arc<dyn TokenSource>) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("snail/0.1 (calendar)")
                .build()?,
            tokens,
            budget: RequestBudget::default(),
        })
    }

    fn get(&self, url: &str) -> Result<Response> {
        self.budget.charge()?;
        let token = self.tokens.access_token()?;
        Ok(self.http.get(url).bearer_auth(token).send()?)
    }

    fn checked_json(response: Response) -> Result<Value> {
        let status = response.status();
        let value: Value = response.json().context("decode Google Calendar response")?;
        if status.is_success() {
            return Ok(value);
        }
        let reason = google_error_reason(&value);
        Err(anyhow!(GoogleCalendarError {
            status: status.as_u16(),
            retryable: status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
                || (status == StatusCode::FORBIDDEN
                    && matches!(
                        reason.as_str(),
                        "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded"
                    )),
            auth: status == StatusCode::UNAUTHORIZED,
            reason,
        }))
    }

    fn patch_event(
        &self,
        calendar: &CalendarInfo,
        event: &CalendarEvent,
        base_etag: Option<&str>,
    ) -> Result<Response> {
        self.budget.charge()?;
        let token = self.tokens.access_token()?;
        let mut request = if event.remote_id.is_empty() {
            self.http
                .post(format!(
                    "{BASE}/calendars/{}/events",
                    encode(&calendar.remote_id)
                ))
                .bearer_auth(token)
                .json(&event_patch(event))
        } else {
            let url = format!(
                "{BASE}/calendars/{}/events/{}",
                encode(&calendar.remote_id),
                encode(&event.remote_id)
            );
            self.http
                .patch(url)
                .bearer_auth(token)
                .json(&event_patch(event))
        };
        if !event.remote_id.is_empty()
            && let Some(etag) = base_etag
        {
            request = request.header(reqwest::header::IF_MATCH, etag);
        }
        Ok(request.send()?)
    }

    fn delete_event(
        &self,
        calendar: &CalendarInfo,
        event: &CalendarEvent,
        base_etag: Option<&str>,
    ) -> Result<Response> {
        self.budget.charge()?;
        let token = self.tokens.access_token()?;
        let url = format!(
            "{BASE}/calendars/{}/events/{}",
            encode(&calendar.remote_id),
            encode(&event.remote_id)
        );
        let mut request = self.http.delete(url).bearer_auth(token);
        if let Some(etag) = base_etag {
            request = request.header(reqwest::header::IF_MATCH, etag);
        }
        Ok(request.send()?)
    }

    fn fetch_event(
        &self,
        calendar: &CalendarInfo,
        event: &CalendarEvent,
    ) -> Result<Option<CalendarEvent>> {
        if event.remote_id.is_empty() {
            return Ok(None);
        }
        let response = self.get(&event_url(&calendar.remote_id, &event.remote_id))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(parse_google_event(&Self::checked_json(response)?)))
    }
}

impl CalendarProvider for GoogleCalendarClient {
    fn discover_calendars(&self, cursor: Option<&str>) -> Result<CalendarBatch> {
        let mut page_token = None;
        let mut batch = CalendarBatch::default();
        loop {
            let url = calendar_list_url(cursor, page_token.as_deref());
            let response = self.get(&url)?;
            if response.status() == StatusCode::GONE {
                batch.full_resync_required = true;
                return Ok(batch);
            }
            let value = Self::checked_json(response)?;
            for item in array(&value, "items") {
                let calendar = parse_calendar(&item);
                if calendar.subscribed {
                    batch.calendars.push(calendar);
                } else {
                    batch.unsubscribed.push(calendar.remote_id);
                }
            }
            page_token = text(&value, "nextPageToken");
            if page_token.is_none() {
                batch.next_sync_token = terminal_sync_token(&value);
                break;
            }
            debug_assert!(value.get("nextSyncToken").is_none());
        }
        Ok(batch)
    }

    fn sync_calendar(
        &self,
        calendar: &CalendarInfo,
        cursor: &CalendarCursor,
    ) -> Result<CalendarBatch> {
        let mut page_token = None;
        let mut batch = CalendarBatch::default();
        loop {
            let url = events_url(
                &calendar.remote_id,
                cursor.sync_token.as_deref(),
                page_token.as_deref(),
            );
            let response = self.get(&url)?;
            if response.status() == StatusCode::GONE {
                batch.full_resync_required = true;
                return Ok(batch);
            }
            let value = Self::checked_json(response)?;
            batch
                .changes
                .extend(array(&value, "items").iter().filter_map(parse_event_change));
            page_token = text(&value, "nextPageToken");
            if page_token.is_none() {
                // Google only returns this on the final page. Never surface an intermediate token.
                batch.next_sync_token = terminal_sync_token(&value);
                break;
            }
            debug_assert!(value.get("nextSyncToken").is_none());
        }
        Ok(batch)
    }

    fn apply(&self, writes: &[CalendarWrite]) -> Result<Vec<WriteOutcome>> {
        let mut outcomes = Vec::with_capacity(writes.len());
        for write in writes {
            match write {
                CalendarWrite::Put {
                    calendar,
                    event,
                    base_etag,
                } => {
                    let response = self.patch_event(calendar, event, base_etag.as_deref())?;
                    if response.status() == StatusCode::PRECONDITION_FAILED {
                        outcomes.push(WriteOutcome::Conflict {
                            local: Box::new(event.clone()),
                            server: self.fetch_event(calendar, event)?.map(Box::new),
                        });
                    } else {
                        let value = Self::checked_json(response)?;
                        outcomes.push(WriteOutcome::Applied(Box::new(parse_google_event(&value))));
                    }
                }
                CalendarWrite::Delete {
                    calendar,
                    event,
                    base_etag,
                } => {
                    let response = self.delete_event(calendar, event, base_etag.as_deref())?;
                    if response.status() == StatusCode::PRECONDITION_FAILED {
                        outcomes.push(WriteOutcome::Conflict {
                            local: Box::new(event.clone()),
                            server: self.fetch_event(calendar, event)?.map(Box::new),
                        });
                    } else if response.status().is_success()
                        || response.status() == StatusCode::NOT_FOUND
                    {
                        outcomes.push(WriteOutcome::Deleted(event.identity()));
                    } else {
                        let _ = Self::checked_json(response)?;
                    }
                }
            }
        }
        Ok(outcomes)
    }
}

pub fn calendar_list_url(sync_token: Option<&str>, page_token: Option<&str>) -> String {
    let mut url = format!("{BASE}/users/me/calendarList?maxResults=250&showDeleted=true");
    if let Some(token) = sync_token {
        url.push_str("&syncToken=");
        url.push_str(&encode(token));
    }
    if let Some(token) = page_token {
        url.push_str("&pageToken=");
        url.push_str(&encode(token));
    }
    url
}

/// Initial and incremental calls keep every non-cursor parameter identical (E11.5).
pub fn events_url(calendar_id: &str, sync_token: Option<&str>, page_token: Option<&str>) -> String {
    let mut url = format!(
        "{BASE}/calendars/{}/events?maxResults=2500&showDeleted=true&singleEvents=false",
        encode(calendar_id)
    );
    if let Some(token) = sync_token {
        url.push_str("&syncToken=");
        url.push_str(&encode(token));
    }
    if let Some(token) = page_token {
        url.push_str("&pageToken=");
        url.push_str(&encode(token));
    }
    url
}

fn event_url(calendar_id: &str, event_id: &str) -> String {
    format!(
        "{BASE}/calendars/{}/events/{}",
        encode(calendar_id),
        encode(event_id)
    )
}

fn parse_calendar(value: &Value) -> CalendarInfo {
    let access_role = text(value, "accessRole");
    CalendarInfo {
        remote_id: text(value, "id").unwrap_or_default(),
        name: text(value, "summaryOverride").or_else(|| text(value, "summary")),
        color: text(value, "backgroundColor"),
        selected: value
            .get("selected")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        read_only: !matches!(access_role.as_deref(), Some("writer" | "owner")),
        access_role,
        supports_scheduling: true,
        subscribed: !value
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..Default::default()
    }
}

pub fn parse_event_change(value: &Value) -> Option<EventChange> {
    let status = text(value, "status");
    if status.as_deref() == Some("cancelled") {
        return match (
            text(value, "recurringEventId"),
            value.get("originalStartTime").and_then(event_time_identity),
        ) {
            (Some(recurring_event_id), Some(original_start)) => Some(EventChange::CancelInstance {
                recurring_event_id,
                original_start,
            }),
            _ => text(value, "id").map(|id| EventChange::Delete(EventIdentity::Event(id))),
        };
    }
    Some(EventChange::Upsert(Box::new(parse_google_event(value))))
}

pub fn parse_google_event(value: &Value) -> CalendarEvent {
    let (start_utc, all_day, tz) = value.get("start").map(parse_event_time).unwrap_or_default();
    let (end_utc, _, _) = value.get("end").map(parse_event_time).unwrap_or_default();
    let recurrence = value
        .get("recurrence")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    CalendarEvent {
        remote_id: text(value, "id").unwrap_or_default(),
        etag: text(value, "etag"),
        ical_uid: text(value, "iCalUID"),
        recurring_event_id: text(value, "recurringEventId"),
        original_start: value.get("originalStartTime").and_then(event_time_identity),
        summary: text(value, "summary"),
        location: text(value, "location"),
        description: text(value, "description"),
        start_utc,
        end_utc,
        all_day,
        tz,
        rrule: recurrence_value(&recurrence, "RRULE:"),
        rdate: recurrence_value(&recurrence, "RDATE:"),
        exdate: recurrence_value(&recurrence, "EXDATE:"),
        status: text(value, "status"),
        sequence: value.get("sequence").and_then(Value::as_i64),
        attendees: value
            .get("attendees")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|attendee| {
                Some(CalendarAttendee {
                    email: text(attendee, "email")?,
                    display_name: text(attendee, "displayName"),
                    role: if attendee
                        .get("optional")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "optional".into()
                    } else {
                        "required".into()
                    },
                    status: text(attendee, "responseStatus")
                        .unwrap_or_else(|| "needsAction".into()),
                    is_self: attendee
                        .get("self")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect(),
        reminders: google_reminders(value),
        ..Default::default()
    }
}

fn google_reminders(value: &Value) -> Vec<CalendarReminder> {
    let mut reminders: Vec<_> = value
        .pointer("/reminders/overrides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| text(item, "method").as_deref() == Some("popup"))
        .filter_map(|item| item.get("minutes").and_then(Value::as_i64))
        .map(CalendarReminder::MinutesBefore)
        .collect();
    if let Some(encoded) = value
        .pointer("/extendedProperties/private/snailReminders")
        .and_then(Value::as_str)
        && let Ok(extra) = serde_json::from_str::<Vec<CalendarReminder>>(encoded)
    {
        reminders.extend(
            extra
                .into_iter()
                .filter(|item| !matches!(item, CalendarReminder::MinutesBefore(_))),
        );
    }
    reminders
}

fn parse_event_time(value: &Value) -> (Option<i64>, bool, Option<String>) {
    if let Some(date_time) = text(value, "dateTime") {
        return (
            DateTime::parse_from_rfc3339(&date_time)
                .ok()
                .map(|date| date.timestamp()),
            false,
            text(value, "timeZone"),
        );
    }
    let date = text(value, "date");
    (
        date.as_deref()
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|date| date.and_utc().timestamp()),
        date.is_some(),
        text(value, "timeZone"),
    )
}

fn event_time_identity(value: &Value) -> Option<String> {
    text(value, "dateTime").or_else(|| text(value, "date"))
}

fn recurrence_value(values: &[Value], prefix: &str) -> Option<String> {
    let values = values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|value| value.strip_prefix(prefix))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join("\n"))
}

fn event_patch(event: &CalendarEvent) -> Value {
    let mut object = serde_json::Map::new();
    for (key, value) in [
        ("summary", event.summary.as_ref()),
        ("location", event.location.as_ref()),
        ("description", event.description.as_ref()),
        ("status", event.status.as_ref()),
    ] {
        if let Some(value) = value {
            object.insert(key.into(), json!(value));
        }
    }
    if let Some(sequence) = event.sequence {
        object.insert("sequence".into(), json!(sequence));
    }
    if let Some(start) = event
        .start_utc
        .and_then(|value| DateTime::from_timestamp(value, 0))
    {
        object.insert(
            "start".into(),
            if event.all_day {
                json!({"date": start.format("%Y-%m-%d").to_string()})
            } else {
                json!({
                    "dateTime": start.to_rfc3339(),
                    "timeZone": event.tz.as_deref().unwrap_or("Etc/UTC")
                })
            },
        );
    }
    if let Some(end) = event
        .end_utc
        .and_then(|value| DateTime::from_timestamp(value, 0))
    {
        object.insert(
            "end".into(),
            if event.all_day {
                json!({"date": end.format("%Y-%m-%d").to_string()})
            } else {
                json!({
                    "dateTime": end.to_rfc3339(),
                    "timeZone": event.tz.as_deref().unwrap_or("Etc/UTC")
                })
            },
        );
    }
    let recurrence: Vec<String> = [
        ("RRULE:", event.rrule.as_deref()),
        ("RDATE:", event.rdate.as_deref()),
        ("EXDATE:", event.exdate.as_deref()),
    ]
    .into_iter()
    .flat_map(|(prefix, values)| {
        values
            .into_iter()
            .flat_map(str::lines)
            .map(move |value| format!("{prefix}{value}"))
    })
    .collect();
    if !recurrence.is_empty() {
        object.insert("recurrence".into(), json!(recurrence));
    }
    if !event.attendees.is_empty() {
        object.insert(
            "attendees".into(),
            json!(
                event
                    .attendees
                    .iter()
                    .map(|attendee| json!({
                        "email": attendee.email,
                        "displayName": attendee.display_name,
                        "optional": attendee.role.eq_ignore_ascii_case("optional"),
                        "responseStatus": attendee.status,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    if !event.reminders.is_empty() {
        let overrides: Vec<_> = event
            .reminders
            .iter()
            .filter_map(|item| match item {
                CalendarReminder::MinutesBefore(minutes) => {
                    Some(json!({"method": "popup", "minutes": minutes}))
                }
                _ => None,
            })
            .collect();
        object.insert(
            "reminders".into(),
            json!({"useDefault": false, "overrides": overrides}),
        );
        object.insert(
            "extendedProperties".into(),
            json!({"private": {
                "snailReminders": serde_json::to_string(&event.reminders).unwrap_or_default()
            }}),
        );
    }
    Value::Object(object)
}

fn google_error_reason(value: &Value) -> String {
    value
        .pointer("/error/errors/0/reason")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or("unknown Google Calendar error")
        .to_string()
}

fn encode(value: &str) -> String {
    byte_serialize(value.as_bytes()).collect()
}

fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn terminal_sync_token(value: &Value) -> Option<String> {
    value
        .get("nextPageToken")
        .is_none()
        .then(|| text(value, "nextSyncToken"))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_queries_are_master_only_and_cursor_compatible() {
        let initial = events_url("team@example.com", None, None);
        let incremental = events_url("team@example.com", Some("sync token"), None);
        for url in [&initial, &incremental] {
            assert!(url.contains("singleEvents=false"));
            assert!(url.contains("showDeleted=true"));
            for forbidden in [
                "timeMin=",
                "timeMax=",
                "updatedMin=",
                "orderBy=",
                "iCalUID=",
                "q=",
            ] {
                assert!(!url.contains(forbidden));
            }
        }
        assert!(incremental.contains("syncToken=sync+token"));
    }

    #[test]
    fn only_the_terminal_page_can_advance_a_sync_token() {
        assert_eq!(
            terminal_sync_token(&json!({
                "nextPageToken": "page-2",
                "nextSyncToken": "must-not-commit"
            })),
            None
        );
        assert_eq!(
            terminal_sync_token(&json!({"nextSyncToken": "commit-me"})).as_deref(),
            Some("commit-me")
        );
    }

    #[test]
    fn calendar_list_keeps_subscription_metadata() {
        let calendar = parse_calendar(&json!({
            "id": "team",
            "summary": "Team",
            "backgroundColor": "#336699",
            "selected": true,
            "accessRole": "reader"
        }));
        assert_eq!(calendar.color.as_deref(), Some("#336699"));
        assert!(calendar.selected);
        assert!(calendar.read_only);
        assert_eq!(calendar.access_role.as_deref(), Some("reader"));
    }

    #[test]
    fn request_quota_constants_match_google_calendar_not_gmail_units() {
        assert_eq!(USER_REQUESTS_PER_MINUTE, 600);
        assert_eq!(PROJECT_REQUESTS_PER_MINUTE, 10_000);
        assert_eq!(PROJECT_REQUESTS_PER_DAY, 1_000_000);
    }

    #[test]
    fn a_cancelled_exception_never_deletes_its_series() {
        let value = json!({
            "id": "instance",
            "status": "cancelled",
            "recurringEventId": "series",
            "originalStartTime": {"dateTime": "2026-05-01T09:00:00-05:00"}
        });
        assert_eq!(
            parse_event_change(&value),
            Some(EventChange::CancelInstance {
                recurring_event_id: "series".into(),
                original_start: "2026-05-01T09:00:00-05:00".into(),
            })
        );
    }

    #[test]
    fn a_nearly_empty_cancelled_event_is_a_delete() {
        let value = json!({"id": "gone", "status": "cancelled"});
        assert_eq!(
            parse_event_change(&value),
            Some(EventChange::Delete(EventIdentity::Event("gone".into())))
        );
    }

    #[test]
    fn masters_keep_rrule_and_cross_system_uid() {
        let value = json!({
            "id": "google-id",
            "iCalUID": "same-everywhere@example.com",
            "start": {"dateTime": "2026-01-01T09:00:00-06:00", "timeZone": "America/Chicago"},
            "end": {"dateTime": "2026-01-01T10:00:00-06:00", "timeZone": "America/Chicago"},
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=TH"]
        });
        let event = parse_google_event(&value);
        assert_eq!(
            event.ical_uid.as_deref(),
            Some("same-everywhere@example.com")
        );
        assert_eq!(event.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=TH"));
        assert_eq!(event.tz.as_deref(), Some("America/Chicago"));
    }

    #[test]
    fn patches_include_times_and_recurrence_without_updated_field() {
        let patch = event_patch(&CalendarEvent {
            start_utc: Some(1_767_283_200),
            end_utc: Some(1_767_286_800),
            tz: Some("America/Chicago".into()),
            rrule: Some("FREQ=WEEKLY".into()),
            ..Default::default()
        });
        assert!(patch.get("start").is_some());
        assert!(patch.get("end").is_some());
        assert_eq!(patch["recurrence"][0], "RRULE:FREQ=WEEKLY");
        assert!(patch.get("updated").is_none());
    }

    #[test]
    fn all_recurrence_exclusions_survive_parse_and_patch() {
        let value = json!({
            "id": "series",
            "recurrence": [
                "RRULE:FREQ=WEEKLY",
                "EXDATE:20260108T150000Z",
                "EXDATE:20260115T150000Z"
            ]
        });
        let event = parse_google_event(&value);
        assert_eq!(
            event.exdate.as_deref(),
            Some("20260108T150000Z\n20260115T150000Z")
        );
        assert_eq!(event_patch(&event)["recurrence"], value["recurrence"]);
    }

    #[test]
    fn deleted_calendar_is_only_unsubscribed() {
        let calendar = parse_calendar(&json!({"id": "old", "deleted": true}));
        assert!(!calendar.subscribed);
    }
}
