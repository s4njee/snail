//! Date labels for the message list (plan.md E5, the mail handoff's row timestamp).
//!
//! Pure calendar arithmetic: the caller supplies UTC offsets, so this module needs no time-zone
//! database and is testable for any zone. "Today" and "yesterday" are **calendar days** in the
//! local zone, not rolling 24-hour windows: a message from 11 pm reads "Yesterday" at 8 am.

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A local calendar date and wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// Days since 1970-01-01 in the local zone, for comparing dates.
    pub days: i64,
}

/// `epoch` seconds at a UTC offset, as a local date and time.
pub fn civil(epoch: i64, offset_secs: i32) -> Civil {
    let local = epoch + offset_secs as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    Civil {
        year,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: ((secs % 3600) / 60) as u32,
        days,
    }
}

/// "9:14 AM".
pub fn clock(at: &Civil) -> String {
    let hour = match at.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let half = if at.hour < 12 { "AM" } else { "PM" };
    format!("{hour}:{:02} {half}", at.minute)
}

/// The list row's timestamp: the time today, "Yesterday", then the date — with the year only
/// when it is not this year. Each offset is the local zone's offset *at that instant*, so a
/// daylight-saving change between the two does not shift the day.
pub fn row_timestamp(epoch: i64, offset_secs: i32, now: i64, now_offset_secs: i32) -> String {
    let at = civil(epoch, offset_secs);
    let today = civil(now, now_offset_secs);
    if at.days == today.days {
        clock(&at)
    } else if at.days == today.days - 1 {
        "Yesterday".into()
    } else {
        let month = MONTHS[(at.month - 1) as usize];
        if at.year == today.year {
            format!("{month} {}", at.day)
        } else {
            format!("{month} {}, {}", at.day, at.year)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-21 14:30:00 UTC, a Monday.
    const NOW: i64 = 1_790_001_000;
    const HOUR: i64 = 3600;

    #[test]
    fn civil_dates_match_known_instants() {
        let at = civil(NOW, 0);
        assert_eq!(
            (at.year, at.month, at.day, at.hour, at.minute),
            (2026, 9, 21, 14, 30)
        );
        let epoch = civil(0, 0);
        assert_eq!((epoch.year, epoch.month, epoch.day), (1970, 1, 1));
        // Leap day, and a negative offset crossing back over midnight.
        let leap = civil(951_782_400, 0); // 2000-02-29 00:00 UTC
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
        let behind = civil(951_782_400, -5 * 3600);
        assert_eq!((behind.month, behind.day, behind.hour), (2, 28, 19));
    }

    #[test]
    fn today_shows_the_time_in_twelve_hour_form() {
        assert_eq!(row_timestamp(NOW - 5 * HOUR, 0, NOW, 0), "9:30 AM");
        assert_eq!(
            row_timestamp(NOW - 14 * HOUR - 25 * 60, 0, NOW, 0),
            "12:05 AM"
        );
        assert_eq!(
            row_timestamp(NOW - 2 * HOUR - 30 * 60, 0, NOW, 0),
            "12:00 PM"
        );
        assert_eq!(row_timestamp(NOW, 0, NOW, 0), "2:30 PM");
    }

    #[test]
    fn yesterday_is_the_previous_calendar_day_not_the_last_24_hours() {
        // 11 pm the night before is only 15.5 hours ago, and still "Yesterday".
        assert_eq!(
            row_timestamp(NOW - 15 * HOUR - 30 * 60, 0, NOW, 0),
            "Yesterday"
        );
        // 30 hours ago is two calendar days back.
        assert_eq!(row_timestamp(NOW - 39 * HOUR, 0, NOW, 0), "Sep 19");
    }

    #[test]
    fn older_mail_shows_the_date_and_the_year_only_when_it_differs() {
        assert_eq!(row_timestamp(NOW - 10 * 86_400, 0, NOW, 0), "Sep 11");
        assert_eq!(row_timestamp(NOW - 300 * 86_400, 0, NOW, 0), "Nov 25, 2025");
    }

    #[test]
    fn the_day_is_decided_in_the_local_zone() {
        // 2:30 pm UTC on the 21st is 12:30 am on the 22nd at UTC+10, so an hour earlier is
        // already the previous local day there.
        let plus10 = 10 * 3600;
        assert_eq!(row_timestamp(NOW, plus10, NOW, plus10), "12:30 AM");
        assert_eq!(row_timestamp(NOW - HOUR, plus10, NOW, plus10), "Yesterday");
        // At UTC-7 the same pair of instants is the same afternoon's morning.
        assert_eq!(
            row_timestamp(NOW - HOUR, -7 * 3600, NOW, -7 * 3600),
            "6:30 AM"
        );
    }
}
