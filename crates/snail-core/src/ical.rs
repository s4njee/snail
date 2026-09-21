//! iCalendar parsing through exactly pinned `calcard` (plan.md E11.20).
//!
//! `calcard` is not used merely as a syntax checker: its `TzResolver` consumes VTIMEZONE and its
//! Windows/Exchange aliases, and its date engine resolves DTSTART/DTEND before normalized rows are
//! stored. Recurrence masters and their raw rules remain intact for local expansion in E13.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use calcard::common::timezone::Tz;
use calcard::icalendar::dates::TimeOrDelta;
use calcard::icalendar::{ICalendar, ICalendarComponentType, ICalendarProperty, ICalendarValue};
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz as ChronoTz;

use crate::calendar::{CalendarAttendee, CalendarEvent, CalendarReminder};

pub const MAX_ICAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EXPANSION_PROBE: usize = 2;
pub const MAX_RANGE_OCCURRENCES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedOccurrence {
    pub start_utc: i64,
    pub end_utc: i64,
    pub original_start: String,
}

/// Expand one stored master for a buffered UI range. The cap is deliberately high enough for old
/// daily series while still making malformed minutely rules fail closed off the UI thread.
pub fn expand_event_between(
    event: &CalendarEvent,
    range_start_utc: i64,
    range_end_utc: i64,
) -> Result<Vec<ExpandedOccurrence>> {
    if range_end_utc <= range_start_utc {
        bail!("calendar occurrence range is empty");
    }
    let (Some(base_start), Some(base_end)) = (event.start_utc, event.end_utc) else {
        return Ok(Vec::new());
    };
    if event.rrule.is_none() && event.rdate.is_none() {
        return Ok((base_start < range_end_utc && base_end > range_start_utc)
            .then(|| ExpandedOccurrence {
                start_utc: base_start,
                end_utc: base_end,
                original_start: event
                    .original_start
                    .clone()
                    .unwrap_or_else(|| base_start.to_string()),
            })
            .into_iter()
            .collect());
    }

    let source = match event.raw_ical.as_deref() {
        Some(raw) if raw.contains("RRULE") || raw.contains("RDATE") => raw.to_string(),
        _ => synthetic_calendar(event)?,
    };
    let calendar = ICalendar::parse(&source)
        .map_err(|error| anyhow!("invalid recurring iCalendar: {error:?}"))?;
    let utc = Tz::from_str("Etc/UTC").expect("calcard knows UTC");
    let expanded = calendar.expand_dates(utc, MAX_RANGE_OCCURRENCES);
    if !expanded.errors.is_empty() {
        bail!(
            "recurrence expansion failed: {:?}",
            expanded.errors[0].error
        );
    }
    let expanded_count = expanded.events.len();
    let last_expanded_start = expanded.events.last().map(|event| event.start.timestamp());
    let mut occurrences = Vec::new();
    for occurrence in expanded.events {
        let start = occurrence.start.timestamp();
        let end = match occurrence.end {
            TimeOrDelta::Time(end) => end.timestamp(),
            TimeOrDelta::Delta(delta) => start + delta.num_seconds(),
        };
        if start < range_end_utc && end > range_start_utc {
            occurrences.push(ExpandedOccurrence {
                start_utc: start,
                end_utc: end,
                original_start: start.to_string(),
            });
        }
    }
    occurrences.sort_by_key(|occurrence| occurrence.start_utc);
    if occurrences.is_empty()
        && expanded_count == MAX_RANGE_OCCURRENCES
        && last_expanded_start.is_some_and(|start| start < range_start_utc)
    {
        bail!("recurrence expansion exceeded {MAX_RANGE_OCCURRENCES} occurrences before range");
    }
    Ok(occurrences)
}

fn synthetic_calendar(event: &CalendarEvent) -> Result<String> {
    serialize_event(event)
}

pub fn serialize_event(event: &CalendarEvent) -> Result<String> {
    let start = event.start_utc.context("recurring event has no DTSTART")?;
    let end = event.end_utc.context("recurring event has no DTEND")?;
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Snail//Calendar range expansion//EN".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!(
            "UID:{}",
            ical_text(event.ical_uid.as_deref().unwrap_or(&event.remote_id))
        ),
        calendar_date_line("DTSTART", start, event.tz.as_deref(), event.all_day)?,
        calendar_date_line("DTEND", end, event.tz.as_deref(), event.all_day)?,
    ];
    for (name, value) in [
        ("SUMMARY", event.summary.as_deref()),
        ("LOCATION", event.location.as_deref()),
        ("DESCRIPTION", event.description.as_deref()),
        ("STATUS", event.status.as_deref()),
    ] {
        if let Some(value) = value {
            lines.push(format!("{name}:{}", ical_text(value)));
        }
    }
    if let Some(sequence) = event.sequence {
        lines.push(format!("SEQUENCE:{sequence}"));
    }
    if let Some(original) = event.original_start.as_deref() {
        let value = original
            .parse::<i64>()
            .ok()
            .map(ical_timestamp)
            .unwrap_or_else(|| original.to_string());
        lines.push(format!("RECURRENCE-ID:{value}"));
    }
    if let Some(rrule) = event.rrule.as_deref() {
        let rule = one_ical_value(rrule)?;
        lines.push(format!("RRULE:{rule}"));
    }
    for (name, values) in [("RDATE", &event.rdate), ("EXDATE", &event.exdate)] {
        for value in values.as_deref().into_iter().flat_map(str::lines) {
            lines.push(format!("{name}:{}", one_ical_value(value)?));
        }
    }
    for attendee in &event.attendees {
        let mut params = vec![format!("ROLE={}", attendee.role.to_ascii_uppercase())];
        params.push(format!("PARTSTAT={}", attendee.status.to_ascii_uppercase()));
        if let Some(name) = attendee.display_name.as_deref() {
            params.push(format!("CN={}", ical_param(name)));
        }
        if attendee.is_self {
            params.push("X-SNAIL-SELF=TRUE".into());
        }
        lines.push(format!(
            "ATTENDEE;{}:mailto:{}",
            params.join(";"),
            attendee.email
        ));
    }
    for reminder in &event.reminders {
        lines.push("BEGIN:VALARM".into());
        lines.push("ACTION:DISPLAY".into());
        lines.push("DESCRIPTION:Event reminder".into());
        match reminder {
            CalendarReminder::MinutesBefore(minutes) => {
                lines.push(format!("TRIGGER:-PT{minutes}M"));
            }
            CalendarReminder::SameDayMinute(minute) => {
                lines.push(format!("X-SNAIL-SAME-DAY-MINUTE:{minute}"));
            }
            CalendarReminder::Absolute(timestamp) => {
                lines.push(format!(
                    "TRIGGER;VALUE=DATE-TIME:{}",
                    ical_timestamp(*timestamp)
                ));
            }
        }
        lines.push("END:VALARM".into());
    }
    lines.extend(["END:VEVENT".into(), "END:VCALENDAR".into()]);
    Ok(lines.join("\r\n") + "\r\n")
}

/// Build an iTIP REPLY without the other attendees or local alarms.  CalDAV collections which do
/// not advertise RFC 6638 use this payload through regular mail instead of risking both the server
/// and Snail sending a response.
pub fn serialize_attendee_reply(
    invite: &CalendarEvent,
    attendee_email: &str,
    status: &str,
    organizer_email: &str,
) -> Result<String> {
    let status = match status.to_ascii_lowercase().as_str() {
        "accepted" => "accepted",
        "tentative" => "tentative",
        "declined" => "declined",
        _ => bail!("invalid RSVP status"),
    };
    let mut reply = invite.clone();
    reply.remote_id = invite
        .ical_uid
        .clone()
        .unwrap_or_else(|| invite.remote_id.clone());
    reply.href = None;
    reply.etag = None;
    reply.recurring_event_id = None;
    reply.original_start = None;
    reply.rrule = None;
    reply.rdate = None;
    reply.exdate = None;
    reply.reminders.clear();
    let display_name = invite
        .attendees
        .iter()
        .find(|attendee| attendee.email.eq_ignore_ascii_case(attendee_email))
        .and_then(|attendee| attendee.display_name.clone());
    reply.attendees = vec![CalendarAttendee {
        email: attendee_email.into(),
        display_name,
        role: "required".into(),
        status: status.into(),
        is_self: true,
    }];
    let mut raw = serialize_event(&reply)?;
    raw = raw.replacen(
        "PRODID:-//Snail//Calendar range expansion//EN\r\n",
        "PRODID:-//Snail//Calendar range expansion//EN\r\nMETHOD:REPLY\r\n",
        1,
    );
    raw = raw.replacen(
        "BEGIN:VEVENT\r\n",
        &format!(
            "BEGIN:VEVENT\r\nORGANIZER:mailto:{}\r\n",
            organizer_email.trim()
        ),
        1,
    );
    Ok(raw)
}

/// Extract the organizer mailbox from an iCalendar entity. Parameters such as CN are ignored;
/// callers only need the routing address for an iMIP response.
pub fn organizer_address(input: &str) -> Option<String> {
    unfold(input).into_iter().find_map(|line| {
        let upper = line.to_ascii_uppercase();
        if !upper.starts_with("ORGANIZER") {
            return None;
        }
        let (_, value) = line.split_once(':')?;
        Some(
            value
                .strip_prefix("mailto:")
                .or_else(|| value.strip_prefix("MAILTO:"))
                .unwrap_or(value)
                .trim()
                .to_string(),
        )
    })
}

fn ical_timestamp(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|date| date.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| "19700101T000000Z".into())
}

fn ical_param(value: &str) -> String {
    if value
        .bytes()
        .any(|value| matches!(value, b':' | b';' | b',' | b' '))
    {
        format!("\"{}\"", value.replace(['\r', '\n', '"'], ""))
    } else {
        value.to_string()
    }
}

fn calendar_date_line(
    name: &str,
    timestamp: i64,
    timezone: Option<&str>,
    all_day: bool,
) -> Result<String> {
    let utc = DateTime::<Utc>::from_timestamp(timestamp, 0).context("invalid event timestamp")?;
    if all_day {
        return Ok(format!("{name};VALUE=DATE:{}", utc.format("%Y%m%d")));
    }
    if let Some(timezone) = timezone {
        let zone: ChronoTz = timezone
            .parse()
            .with_context(|| format!("invalid IANA timezone {timezone}"))?;
        let local = utc.with_timezone(&zone);
        Ok(format!(
            "{name};TZID={timezone}:{:04}{:02}{:02}T{:02}{:02}{:02}",
            local.year(),
            local.month(),
            local.day(),
            local.hour(),
            local.minute(),
            local.second()
        ))
    } else {
        Ok(format!("{name}:{}", utc.format("%Y%m%dT%H%M%SZ")))
    }
}

fn one_ical_value(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.contains(['\r', '\n']) {
        bail!("invalid multiline iCalendar recurrence value");
    }
    Ok(value)
}

fn ical_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(['\r', '\n'], "")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

pub fn parse_event_resource(href: &str, etag: Option<&str>, input: &str) -> Result<CalendarEvent> {
    if input.len() > MAX_ICAL_BYTES {
        bail!("iCalendar resource exceeds {MAX_ICAL_BYTES} bytes");
    }
    let calendar =
        ICalendar::parse(input).map_err(|error| anyhow!("invalid iCalendar: {error:?}"))?;
    let component_id = calendar
        .components
        .iter()
        .position(|component| component.component_type == ICalendarComponentType::VEvent)
        .context("iCalendar resource contains no VEVENT")? as u32;
    let component = &calendar.components[component_id as usize];

    // Building the resolver is load-bearing: custom VTIMEZONE and Exchange TZIDs are resolved by
    // calcard rather than treated as untrusted strings.
    let resolver = calendar.build_tz_resolver();
    let tz = component
        .entries
        .iter()
        .find(|entry| entry.name == ICalendarProperty::Dtstart)
        .and_then(|entry| entry.tz_id())
        .and_then(|name| resolver.resolve(name))
        .and_then(|tz| tz.name().map(|name| name.into_owned()));
    let utc = Tz::from_str("Etc/UTC").expect("calcard knows UTC");
    let expanded = calendar.expand_dates(utc, MAX_EXPANSION_PROBE);
    let dates = expanded
        .events
        .iter()
        .find(|event| event.comp_id == component_id);
    let start_utc = dates.map(|event| event.start.timestamp());
    let end_utc = dates.and_then(|event| match &event.end {
        TimeOrDelta::Time(end) => Some(end.timestamp()),
        TimeOrDelta::Delta(delta) => start_utc.map(|start| start + delta.num_seconds()),
    });

    let uid = text_property(component, ICalendarProperty::Uid);
    let recurrence_id = property_string(component, ICalendarProperty::RecurrenceId);
    Ok(CalendarEvent {
        // The href is the stable CalDAV resource identity. It is never synthesized from UID.
        remote_id: href.to_string(),
        href: Some(href.to_string()),
        etag: etag.map(str::to_string),
        ical_uid: uid,
        recurring_event_id: recurrence_id.as_ref().map(|_| {
            text_property(component, ICalendarProperty::Uid).unwrap_or_else(|| href.to_string())
        }),
        original_start: recurrence_id,
        summary: text_property(component, ICalendarProperty::Summary),
        location: text_property(component, ICalendarProperty::Location),
        description: text_property(component, ICalendarProperty::Description),
        start_utc,
        end_utc,
        all_day: dtstart_is_date(input),
        tz,
        rrule: property_string(component, ICalendarProperty::Rrule),
        rdate: property_string(component, ICalendarProperty::Rdate),
        exdate: property_string(component, ICalendarProperty::Exdate),
        status: text_property(component, ICalendarProperty::Status),
        sequence: component
            .entries
            .iter()
            .find(|entry| entry.name == ICalendarProperty::Sequence)
            .and_then(|entry| entry.values.first())
            .and_then(|value| match value {
                ICalendarValue::Integer(value) => Some(*value),
                _ => None,
            }),
        attendees: parse_attendees(input),
        reminders: parse_reminders(input),
        // Keep the exact canonical body returned by the server. Re-serializing through the parser
        // would make a later server-state comparison partly a comparison with our own formatter.
        raw_ical: Some(input.to_string()),
    })
}

fn parse_attendees(input: &str) -> Vec<CalendarAttendee> {
    unfold(input)
        .into_iter()
        .filter(|line| line.to_ascii_uppercase().starts_with("ATTENDEE"))
        .filter_map(|line| {
            let (head, value) = line.split_once(':')?;
            let mut attendee = CalendarAttendee {
                email: value
                    .strip_prefix("mailto:")
                    .or_else(|| value.strip_prefix("MAILTO:"))
                    .unwrap_or(value)
                    .to_string(),
                display_name: None,
                role: "required".into(),
                status: "needs-action".into(),
                is_self: false,
            };
            for parameter in head.split(';').skip(1) {
                let Some((key, value)) = parameter.split_once('=') else {
                    continue;
                };
                match key.to_ascii_uppercase().as_str() {
                    "CN" => attendee.display_name = Some(value.trim_matches('"').into()),
                    "ROLE" => attendee.role = value.to_ascii_lowercase(),
                    "PARTSTAT" => attendee.status = value.to_ascii_lowercase(),
                    "X-SNAIL-SELF" => attendee.is_self = value.eq_ignore_ascii_case("TRUE"),
                    _ => {}
                }
            }
            Some(attendee)
        })
        .collect()
}

fn parse_reminders(input: &str) -> Vec<CalendarReminder> {
    let mut inside = false;
    let mut reminders = Vec::new();
    for line in unfold(input) {
        match line.to_ascii_uppercase().as_str() {
            "BEGIN:VALARM" => inside = true,
            "END:VALARM" => inside = false,
            _ if inside => {
                if let Some(value) = line
                    .strip_prefix("TRIGGER:-PT")
                    .and_then(|value| value.strip_suffix('M'))
                    && let Ok(minutes) = value.parse()
                {
                    reminders.push(CalendarReminder::MinutesBefore(minutes));
                } else if let Some(value) = line.strip_prefix("X-SNAIL-SAME-DAY-MINUTE:")
                    && let Ok(minute) = value.parse()
                {
                    reminders.push(CalendarReminder::SameDayMinute(minute));
                } else if let Some((head, value)) = line.split_once(':')
                    && head.eq_ignore_ascii_case("TRIGGER;VALUE=DATE-TIME")
                    && let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
                {
                    reminders.push(CalendarReminder::Absolute(date.and_utc().timestamp()));
                }
            }
            _ => {}
        }
    }
    reminders
}

fn text_property(
    component: &calcard::icalendar::ICalendarComponent,
    property: ICalendarProperty,
) -> Option<String> {
    component
        .entries
        .iter()
        .find(|entry| entry.name == property)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_text)
        .map(str::to_string)
}

fn property_string(
    component: &calcard::icalendar::ICalendarComponent,
    property: ICalendarProperty,
) -> Option<String> {
    component
        .entries
        .iter()
        .find(|entry| entry.name == property)
        .and_then(|entry| entry.values.first())
        .and_then(|value| value.clone().into_text())
        .map(|value| value.into_owned())
}

fn dtstart_is_date(input: &str) -> bool {
    unfold(input).into_iter().any(|line| {
        let upper = line.to_ascii_uppercase();
        upper.starts_with("DTSTART;VALUE=DATE:")
            || (upper.starts_with("DTSTART:")
                && line.split_once(':').is_some_and(|(_, value)| {
                    value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit())
                }))
    })
}

fn unfold(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in input.replace("\r\n", "\n").split('\n') {
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&line[1..]);
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_timezone_resolves_through_vtimezone() {
        let input = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Pacific Standard Time\r\nX-MICROSOFT-CDO-TZID:4\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:exchange@example.com\r\nDTSTART;TZID=Pacific Standard Time:20260115T090000\r\nDTEND;TZID=Pacific Standard Time:20260115T100000\r\nSUMMARY:West coast call\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = parse_event_resource("/cal/a.ics", Some("\"1\""), input).unwrap();
        assert_eq!(event.ical_uid.as_deref(), Some("exchange@example.com"));
        assert_eq!(event.tz.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(event.start_utc, Some(1_768_496_400));
        assert_eq!(event.href.as_deref(), Some("/cal/a.ics"));
    }

    #[test]
    fn recurrence_master_is_not_replaced_by_unbounded_instances() {
        let input = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:weekly@example.com\r\nDTSTART:20260101T150000Z\r\nDTEND:20260101T160000Z\r\nRRULE:FREQ=WEEKLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = parse_event_resource("/opaque-name.ics", None, input).unwrap();
        assert_eq!(event.remote_id, "/opaque-name.ics");
        assert_eq!(event.rrule.as_deref(), Some("FREQ=WEEKLY"));
    }

    #[test]
    fn range_expansion_keeps_a_wall_clock_time_across_dst() {
        let zone: ChronoTz = "America/New_York".parse().unwrap();
        let local = |year, month, day, hour| {
            use chrono::TimeZone as _;
            zone.with_ymd_and_hms(year, month, day, hour, 0, 0)
                .single()
                .unwrap()
                .timestamp()
        };
        let event = CalendarEvent {
            remote_id: "weekly".into(),
            ical_uid: Some("weekly@example.com".into()),
            start_utc: Some(local(2026, 3, 1, 9)),
            end_utc: Some(local(2026, 3, 1, 10)),
            tz: Some("America/New_York".into()),
            rrule: Some("FREQ=WEEKLY;COUNT=4".into()),
            ..Default::default()
        };
        let occurrences =
            expand_event_between(&event, local(2026, 3, 1, 0), local(2026, 3, 31, 0)).unwrap();
        assert_eq!(occurrences.len(), 4);
        let utc_hours: Vec<_> = occurrences
            .iter()
            .map(|event| {
                DateTime::<Utc>::from_timestamp(event.start_utc, 0)
                    .unwrap()
                    .hour()
            })
            .collect();
        assert_eq!(utc_hours, vec![14, 13, 13, 13]);
    }

    #[test]
    fn one_off_events_are_filtered_to_the_requested_range() {
        let event = CalendarEvent {
            remote_id: "one".into(),
            start_utc: Some(100),
            end_utc: Some(200),
            ..Default::default()
        };
        assert_eq!(expand_event_between(&event, 150, 300).unwrap().len(), 1);
        assert!(expand_event_between(&event, 200, 300).unwrap().is_empty());
    }

    #[test]
    fn rrule_count_until_byday_bymonthday_and_bysetpos_expand() {
        let event = CalendarEvent {
            remote_id: "monthly".into(),
            start_utc: DateTime::parse_from_rfc3339("2026-01-05T15:00:00Z")
                .ok()
                .map(|date| date.timestamp()),
            end_utc: DateTime::parse_from_rfc3339("2026-01-05T16:00:00Z")
                .ok()
                .map(|date| date.timestamp()),
            rrule: Some(
                "FREQ=MONTHLY;COUNT=3;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1;BYMONTHDAY=1,2,3,4,5,6,7"
                    .into(),
            ),
            ..Default::default()
        };
        let start = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let end = DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let occurrences = expand_event_between(&event, start, end).unwrap();
        assert_eq!(occurrences.len(), 3);
        assert_eq!(
            DateTime::<Utc>::from_timestamp(occurrences[1].start_utc, 0)
                .unwrap()
                .day(),
            2
        );
    }

    #[test]
    fn rdate_adds_and_exdate_removes_occurrences() {
        let event = CalendarEvent {
            remote_id: "dates".into(),
            start_utc: Some(1_767_268_800), // 2026-01-01 12:00Z
            end_utc: Some(1_767_272_400),
            rrule: Some("FREQ=DAILY;COUNT=3".into()),
            rdate: Some("20260110T120000Z".into()),
            exdate: Some("20260102T120000Z".into()),
            ..Default::default()
        };
        let occurrences = expand_event_between(&event, 1_767_225_600, 1_768_176_000).unwrap();
        let starts: Vec<_> = occurrences.iter().map(|item| item.start_utc).collect();
        assert_eq!(starts.len(), 3);
        assert!(!starts.contains(&1_767_355_200));
        assert!(starts.contains(&1_768_046_400));
    }

    #[test]
    fn attendee_reply_is_a_single_recipient_imip_payload_without_alarms() {
        let invite = CalendarEvent {
            remote_id: "invite".into(),
            ical_uid: Some("invite@example.com".into()),
            summary: Some("Planning".into()),
            start_utc: Some(1_767_268_800),
            end_utc: Some(1_767_272_400),
            attendees: vec![
                CalendarAttendee {
                    email: "me@example.com".into(),
                    display_name: Some("Me".into()),
                    role: "required".into(),
                    status: "needs-action".into(),
                    is_self: true,
                },
                CalendarAttendee {
                    email: "other@example.com".into(),
                    display_name: None,
                    role: "required".into(),
                    status: "accepted".into(),
                    is_self: false,
                },
            ],
            reminders: vec![CalendarReminder::MinutesBefore(10)],
            ..Default::default()
        };
        let raw =
            serialize_attendee_reply(&invite, "me@example.com", "accepted", "host@example.com")
                .unwrap();
        assert!(raw.contains("METHOD:REPLY"));
        assert!(raw.contains("ORGANIZER:mailto:host@example.com"));
        assert!(raw.contains("ATTENDEE;ROLE=REQUIRED;PARTSTAT=ACCEPTED"));
        assert!(!raw.contains("other@example.com"));
        assert!(!raw.contains("VALARM"));
        assert_eq!(
            organizer_address("ORGANIZER;CN=Host:MAILTO:host@example.com\r\n"),
            Some("host@example.com".into())
        );
    }
}
