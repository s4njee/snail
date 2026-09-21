//! Pure command-palette matching and natural-language date parsing.

use chrono::Weekday;

use crate::calendar::CivilDate;
use crate::commands::{CommandId, commands};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedTarget {
    pub id: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteTarget {
    Command(CommandId),
    Mailbox(i64),
    Calendar(i64),
    Date(CivilDate),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteCandidate {
    pub target: PaletteTarget,
    pub title: String,
    pub detail: &'static str,
    score: u8,
}

pub fn candidates(
    query: &str,
    mailboxes: &[NamedTarget],
    calendars: &[NamedTarget],
    today: CivilDate,
) -> Vec<PaletteCandidate> {
    let query = query.trim().to_ascii_lowercase();
    let mut matches = Vec::new();

    for command in commands() {
        if let Some(score) = match_score(&query, command.title, command.name) {
            matches.push(PaletteCandidate {
                target: PaletteTarget::Command(command.id),
                title: command.title.into(),
                detail: "Command",
                score,
            });
        }
    }
    for mailbox in mailboxes {
        if let Some(score) = match_score(&query, &mailbox.title, "mailbox") {
            matches.push(PaletteCandidate {
                target: PaletteTarget::Mailbox(mailbox.id),
                title: mailbox.title.clone(),
                detail: "Mailbox",
                score: score.saturating_add(1),
            });
        }
    }
    for calendar in calendars {
        if let Some(score) = match_score(&query, &calendar.title, "calendar") {
            matches.push(PaletteCandidate {
                target: PaletteTarget::Calendar(calendar.id),
                title: calendar.title.clone(),
                detail: "Calendar",
                score: score.saturating_add(1),
            });
        }
    }
    if !query.is_empty()
        && let Some(date) = parse_natural_date(&query, today)
    {
        matches.push(PaletteCandidate {
            target: PaletteTarget::Date(date),
            title: format!(
                "Go to {}, {} {}",
                weekday_name(date.weekday()),
                month_name(date.month),
                date.day
            ),
            detail: "Date",
            score: 0,
        });
    }

    matches.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| left.title.cmp(&right.title))
    });
    matches.truncate(12);
    matches
}

fn match_score(query: &str, title: &str, alternate: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(4);
    }
    let title = title.to_ascii_lowercase();
    let alternate = alternate.to_ascii_lowercase();
    if title == query || alternate == query {
        Some(0)
    } else if title.starts_with(query) || alternate.starts_with(query) {
        Some(1)
    } else if title.split_whitespace().any(|word| word.starts_with(query)) {
        Some(2)
    } else if title.contains(query) || alternate.contains(query) {
        Some(3)
    } else if query
        .split_whitespace()
        .all(|term| title.contains(term) || alternate.contains(term))
    {
        Some(4)
    } else {
        None
    }
}

pub fn parse_natural_date(input: &str, today: CivilDate) -> Option<CivilDate> {
    let input = input.trim().to_ascii_lowercase();
    match input.as_str() {
        "today" => return Some(today),
        "tomorrow" => return Some(today.add_days(1)),
        "yesterday" => return Some(today.add_days(-1)),
        _ => {}
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(&input, "%Y-%m-%d") {
        return Some(CivilDate::from_naive(date));
    }

    let words: Vec<_> = input.split_whitespace().collect();
    if words.len() == 2 && words[0] == "next" {
        return weekday(words[1]).map(|target| next_weekday(today, target));
    }
    if words.len() == 1
        && let Some(target) = weekday(words[0])
    {
        return Some(next_weekday(today.add_days(-1), target));
    }

    if (2..=3).contains(&words.len()) {
        let month = month(words[0])?;
        let day: u32 = words[1].trim_end_matches(',').parse().ok()?;
        let explicit_year = words.get(2).and_then(|year| year.parse::<i32>().ok());
        let mut year = explicit_year.unwrap_or(today.year);
        let mut date = CivilDate::new(year, month, day)?;
        if explicit_year.is_none() && date < today {
            year += 1;
            date = CivilDate::new(year, month, day)?;
        }
        return Some(date);
    }
    None
}

fn next_weekday(today: CivilDate, target: Weekday) -> CivilDate {
    let current = today.weekday().num_days_from_monday() as i64;
    let target = target.num_days_from_monday() as i64;
    let distance = (target - current).rem_euclid(7);
    today.add_days(if distance == 0 { 7 } else { distance })
}

fn weekday(input: &str) -> Option<Weekday> {
    match input.trim_end_matches(',') {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn month(input: &str) -> Option<u32> {
    match input.trim_end_matches(',') {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

fn month_name(month: u32) -> &'static str {
    [
        "",
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ][month as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> CivilDate {
        CivilDate::new(2026, 9, 21).unwrap()
    }

    #[test]
    fn parses_relative_weekday_month_and_iso_dates() {
        assert_eq!(
            parse_natural_date("next tuesday", today()),
            CivilDate::new(2026, 9, 22)
        );
        assert_eq!(
            parse_natural_date("aug 17", today()),
            CivilDate::new(2027, 8, 17)
        );
        assert_eq!(
            parse_natural_date("Aug 17 2026", today()),
            CivilDate::new(2026, 8, 17)
        );
        assert_eq!(
            parse_natural_date("2026-12-05", today()),
            CivilDate::new(2026, 12, 5)
        );
        assert_eq!(
            parse_natural_date("tomorrow", today()),
            CivilDate::new(2026, 9, 22)
        );
    }

    #[test]
    fn palette_combines_all_target_kinds() {
        let matches = candidates(
            "work",
            &[NamedTarget {
                id: 1,
                title: "Work mail".into(),
            }],
            &[NamedTarget {
                id: 2,
                title: "Work calendar".into(),
            }],
            today(),
        );
        assert!(
            matches
                .iter()
                .any(|item| matches!(item.target, PaletteTarget::Mailbox(1)))
        );
        assert!(
            matches
                .iter()
                .any(|item| matches!(item.target, PaletteTarget::Calendar(2)))
        );
        assert!(
            candidates("next tuesday", &[], &[], today())
                .iter()
                .any(|item| matches!(item.target, PaletteTarget::Date(_)))
        );
    }
}
