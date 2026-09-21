//! Pure calendar geometry and interaction model (plan.md E12).
//!
//! The GPUI layer only paints these decisions. Date grids, overlap packing, range buffering,
//! timezone projection and drag snapping stay deterministic and fast to unit-test here.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

pub const MONTH_ROWS: usize = 6;
pub const WEEK_DAYS: usize = 7;
pub const MONTH_CELLS: usize = MONTH_ROWS * WEEK_DAYS;
pub const MINI_MONTH_CELL_PX: f32 = 27.0;
pub const SNAP_MINUTES: i32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl CivilDate {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        NaiveDate::from_ymd_opt(year, month, day).map(Self::from_naive)
    }

    pub fn from_naive(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
            day: date.day(),
        }
    }

    pub fn to_naive(self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
            .expect("CivilDate is constructed from a valid date")
    }

    pub fn add_days(self, days: i64) -> Self {
        Self::from_naive(self.to_naive() + Duration::days(days))
    }

    pub fn weekday(self) -> Weekday {
        self.to_naive().weekday()
    }

    pub fn iso_week(self) -> u32 {
        self.to_naive().iso_week().week()
    }

    pub fn start_of_week_sunday(self) -> Self {
        self.add_days(-(self.weekday().num_days_from_sunday() as i64))
    }

    pub fn first_of_month(self) -> Self {
        Self { day: 1, ..self }
    }

    pub fn add_months(self, months: i32) -> Self {
        let absolute = self.year * 12 + self.month as i32 - 1 + months;
        let year = absolute.div_euclid(12);
        let month = absolute.rem_euclid(12) as u32 + 1;
        let day = self.day.min(days_in_month(year, month));
        Self { year, month, day }
    }
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap()
    };
    (next - first).num_days() as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonthCell {
    pub date: CivilDate,
    pub row: usize,
    pub column: usize,
    pub in_month: bool,
    pub iso_week: u32,
}

/// A fixed Sunday-first 6×7 month grid, including both adjacent months.
pub fn month_grid(month: CivilDate) -> [MonthCell; MONTH_CELLS] {
    let first = month.first_of_month();
    let start = first.start_of_week_sunday();
    std::array::from_fn(|index| {
        let date = start.add_days(index as i64);
        MonthCell {
            date,
            row: index / WEEK_DAYS,
            column: index % WEEK_DAYS,
            in_month: date.year == first.year && date.month == first.month,
            iso_week: date.iso_week(),
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridPitch {
    Week,
    ThreeDay,
    Day,
}

impl GridPitch {
    pub fn pixels(self) -> f32 {
        match self {
            Self::Week => 45.0,
            Self::ThreeDay => 47.0,
            Self::Day => 49.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeRect {
    pub top: f32,
    pub height: f32,
}

/// Geometry relative to `grid_start_minute`; the handoff uses 07:00 while the scroll surface can
/// pass 00:00 to expose all 24 hours.
pub fn time_rect(
    start_minute: i32,
    end_minute: i32,
    grid_start_minute: i32,
    pitch: GridPitch,
    gap: f32,
) -> TimeRect {
    let pixels_per_minute = pitch.pixels() / 60.0;
    TimeRect {
        top: (start_minute - grid_start_minute) as f32 * pixels_per_minute,
        height: ((end_minute - start_minute).max(0) as f32 * pixels_per_minute - gap).max(1.0),
    }
}

pub fn now_line(minute_of_day: u32, grid_start_minute: i32, pitch: GridPitch) -> f32 {
    (minute_of_day as i32 - grid_start_minute) as f32 * pitch.pixels() / 60.0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interval<T> {
    pub id: T,
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackedInterval<T> {
    pub id: T,
    pub start: i64,
    pub end: i64,
    pub column: usize,
    pub columns: usize,
    pub left_fraction: f32,
    pub width_fraction: f32,
}

/// Pack half-open intervals into the minimum columns for each connected overlap group. Events that
/// only touch at one endpoint do not overlap.
pub fn pack_overlaps<T: Clone>(intervals: &[Interval<T>]) -> Vec<PackedInterval<T>> {
    let mut sorted: Vec<_> = intervals
        .iter()
        .filter(|interval| interval.end > interval.start)
        .cloned()
        .collect();
    sorted.sort_by_key(|interval| (interval.start, interval.end));

    let mut output = Vec::with_capacity(sorted.len());
    let mut cursor = 0;
    while cursor < sorted.len() {
        let group_start = cursor;
        let mut group_end = sorted[cursor].end;
        cursor += 1;
        while cursor < sorted.len() && sorted[cursor].start < group_end {
            group_end = group_end.max(sorted[cursor].end);
            cursor += 1;
        }

        let group = &sorted[group_start..cursor];
        let mut column_ends: Vec<i64> = Vec::new();
        let mut assigned = Vec::with_capacity(group.len());
        for interval in group {
            let column = column_ends
                .iter()
                .position(|end| *end <= interval.start)
                .unwrap_or(column_ends.len());
            if column == column_ends.len() {
                column_ends.push(interval.end);
            } else {
                column_ends[column] = interval.end;
            }
            assigned.push((interval, column));
        }
        let columns = column_ends.len().max(1);
        output.extend(
            assigned
                .into_iter()
                .map(|(interval, column)| PackedInterval {
                    id: interval.id.clone(),
                    start: interval.start,
                    end: interval.end,
                    column,
                    columns,
                    left_fraction: column as f32 / columns as f32,
                    width_fraction: 1.0 / columns as f32,
                }),
        );
    }
    output
}

/// All-day events use the same half-open interval packing, with civil-day ordinals as coordinates.
pub fn assign_all_day_bands<T: Clone>(events: &[Interval<T>]) -> Vec<PackedInterval<T>> {
    pack_overlaps(events)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaItem<T> {
    pub id: T,
    pub date: CivilDate,
    pub start_minute: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaGroup<T> {
    pub date: CivilDate,
    pub rows: Vec<AgendaItem<T>>,
}

pub fn group_agenda<T: Clone + Ord>(items: &[AgendaItem<T>]) -> Vec<AgendaGroup<T>> {
    let mut items = items.to_vec();
    items.sort_by_key(|item| (item.date, item.start_minute, item.id.clone()));
    let mut groups: Vec<AgendaGroup<T>> = Vec::new();
    for item in items {
        if groups.last().is_none_or(|group| group.date != item.date) {
            groups.push(AgendaGroup {
                date: item.date,
                rows: Vec::new(),
            });
        }
        groups.last_mut().unwrap().rows.push(item);
    }
    groups
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarView {
    #[default]
    Month,
    Week,
    ThreeDay,
    Day,
    Agenda,
}

impl CalendarView {
    pub const ALL: [Self; 5] = [
        Self::Month,
        Self::Week,
        Self::ThreeDay,
        Self::Day,
        Self::Agenda,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Month => "Month",
            Self::Week => "Week",
            Self::ThreeDay => "3-day",
            Self::Day => "Day",
            Self::Agenda => "Agenda",
        }
    }

    pub fn step(self, focused: CivilDate, direction: i32) -> CivilDate {
        match self {
            Self::Month => focused.add_months(direction),
            Self::Week => focused.add_days(7 * direction as i64),
            Self::ThreeDay => focused.add_days(3 * direction as i64),
            Self::Day => focused.add_days(direction as i64),
            Self::Agenda => focused.add_days(14 * direction as i64),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangePlan {
    pub visible_start: CivilDate,
    pub visible_end: CivilDate,
    pub load_start: CivilDate,
    pub load_end: CivilDate,
}

/// Visible interval plus one equally sized interval of prefetch on either side.
pub fn range_plan(view: CalendarView, focused: CivilDate) -> RangePlan {
    let (start, days) = match view {
        CalendarView::Month => (month_grid(focused)[0].date, 42),
        CalendarView::Week => (focused.start_of_week_sunday(), 7),
        CalendarView::ThreeDay => (focused, 3),
        CalendarView::Day => (focused, 1),
        // Agenda rows are virtualized; each generation loads a broad window around its anchor.
        CalendarView::Agenda => (focused.add_days(-90), 180),
    };
    RangePlan {
        visible_start: start,
        visible_end: start.add_days(days),
        load_start: start.add_days(-days),
        load_end: start.add_days(days * 2),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedInterval {
    pub local_date: CivilDate,
    pub local_start_minute: u16,
    pub local_end_date: CivilDate,
    pub local_end_minute: u16,
    pub local_label: String,
    pub original_label: Option<String>,
    pub all_day: bool,
}

/// Project a UTC event into the viewer's IANA zone. Floating all-day dates deliberately bypass
/// timezone conversion so crossing UTC midnight cannot move them to another day.
pub fn render_in_zone(
    start_utc: i64,
    end_utc: i64,
    event_tz: Option<&str>,
    local_tz: &str,
    all_day: bool,
) -> Option<RenderedInterval> {
    let start = DateTime::<Utc>::from_timestamp(start_utc, 0)?;
    let end = DateTime::<Utc>::from_timestamp(end_utc, 0)?;
    if all_day {
        let start = start.naive_utc();
        let end = end.naive_utc();
        return Some(RenderedInterval {
            local_date: CivilDate::new(start.year(), start.month(), start.day())?,
            local_start_minute: 0,
            local_end_date: CivilDate::new(end.year(), end.month(), end.day())?,
            local_end_minute: 0,
            local_label: "all day".into(),
            original_label: None,
            all_day: true,
        });
    }

    let local: Tz = local_tz.parse().ok()?;
    let local_start = start.with_timezone(&local);
    let local_end = end.with_timezone(&local);
    let event_zone: Tz = event_tz.unwrap_or(local_tz).parse().ok()?;
    let original_start = start.with_timezone(&event_zone);
    let original_end = end.with_timezone(&event_zone);
    let different_zone = event_zone != local;
    Some(RenderedInterval {
        local_date: CivilDate::from_naive(local_start.date_naive()),
        local_start_minute: (local_start.hour() * 60 + local_start.minute()) as u16,
        local_end_date: CivilDate::from_naive(local_end.date_naive()),
        local_end_minute: (local_end.hour() * 60 + local_end.minute()) as u16,
        local_label: format!(
            "{}–{}",
            local_start.format("%H:%M"),
            local_end.format("%H:%M")
        ),
        original_label: different_zone.then(|| {
            format!(
                "{}–{} {}",
                original_start.format("%H:%M"),
                original_end.format("%H:%M"),
                event_tz.unwrap_or(local_tz)
            )
        }),
        all_day: false,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragKind {
    Create,
    Move,
    ResizeStart,
    ResizeEnd,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDrag {
    pub event_id: Option<i64>,
    pub kind: DragKind,
    pub start_minute: i32,
    pub end_minute: i32,
}

pub fn snap_to_quarter(minutes: i32) -> i32 {
    (minutes as f32 / SNAP_MINUTES as f32).round() as i32 * SNAP_MINUTES
}

pub fn apply_drag(payload: &EventDrag, delta_minutes: i32) -> EventDrag {
    let delta = snap_to_quarter(delta_minutes);
    let mut result = payload.clone();
    match payload.kind {
        DragKind::Create | DragKind::ResizeEnd => {
            result.end_minute = (payload.end_minute + delta)
                .max(payload.start_minute + SNAP_MINUTES)
                .min(24 * 60);
        }
        DragKind::ResizeStart => {
            result.start_minute = (payload.start_minute + delta)
                .min(payload.end_minute - SNAP_MINUTES)
                .max(0);
        }
        DragKind::Move => {
            let duration = payload.end_minute - payload.start_minute;
            result.start_minute = (payload.start_minute + delta).clamp(0, 24 * 60 - duration);
            result.end_minute = result.start_minute + duration;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn date(year: i32, month: u32, day: u32) -> CivilDate {
        CivilDate::new(year, month, day).unwrap()
    }

    #[test]
    fn month_grid_crosses_year_and_is_always_six_by_seven() {
        let grid = month_grid(date(2026, 1, 1));
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0].date, date(2025, 12, 28));
        assert_eq!(grid[41].date, date(2026, 2, 7));
        assert_eq!(grid[4].date, date(2026, 1, 1));
        assert!(grid[4].in_month);
        assert!(!grid[0].in_month);
    }

    #[test]
    fn time_grid_uses_the_three_handoff_pitches() {
        assert_eq!(
            time_rect(9 * 60, 10 * 60, 7 * 60, GridPitch::Week, 3.0),
            TimeRect {
                top: 90.0,
                height: 42.0
            }
        );
        assert_eq!(
            time_rect(9 * 60, 10 * 60, 7 * 60, GridPitch::ThreeDay, 3.0),
            TimeRect {
                top: 94.0,
                height: 44.0
            }
        );
        assert_eq!(
            time_rect(9 * 60, 10 * 60, 7 * 60, GridPitch::Day, 3.0),
            TimeRect {
                top: 98.0,
                height: 46.0
            }
        );
    }

    #[test]
    fn three_way_overlap_gets_three_equal_columns() {
        let packed = pack_overlaps(&[
            Interval {
                id: 'a',
                start: 60,
                end: 180,
            },
            Interval {
                id: 'b',
                start: 90,
                end: 150,
            },
            Interval {
                id: 'c',
                start: 120,
                end: 210,
            },
        ]);
        assert!(packed.iter().all(|event| event.columns == 3));
        assert_eq!(
            packed.iter().map(|event| event.column).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn long_event_and_disjoint_short_events_need_only_two_columns() {
        let packed = pack_overlaps(&[
            Interval {
                id: 'l',
                start: 0,
                end: 600,
            },
            Interval {
                id: 'a',
                start: 30,
                end: 60,
            },
            Interval {
                id: 'b',
                start: 60,
                end: 90,
            },
        ]);
        assert!(packed.iter().all(|event| event.columns == 2));
        assert_eq!(packed[1].column, 1);
        assert_eq!(packed[2].column, 1);
    }

    #[test]
    fn touching_endpoints_do_not_overlap() {
        let packed = pack_overlaps(&[
            Interval {
                id: 1,
                start: 60,
                end: 120,
            },
            Interval {
                id: 2,
                start: 120,
                end: 180,
            },
        ]);
        assert!(packed.iter().all(|event| event.columns == 1));
    }

    #[test]
    fn all_day_bands_and_agenda_groups_are_stable() {
        let bands = assign_all_day_bands(&[
            Interval {
                id: 1,
                start: 10,
                end: 13,
            },
            Interval {
                id: 2,
                start: 11,
                end: 12,
            },
        ]);
        assert_eq!(bands[1].column, 1);
        let groups = group_agenda(&[
            AgendaItem {
                id: 2,
                date: date(2026, 8, 14),
                start_minute: Some(600),
            },
            AgendaItem {
                id: 1,
                date: date(2026, 8, 13),
                start_minute: None,
            },
        ]);
        assert_eq!(
            groups.iter().map(|group| group.date).collect::<Vec<_>>(),
            vec![date(2026, 8, 13), date(2026, 8, 14)]
        );
    }

    #[test]
    fn range_loading_adds_one_visible_range_on_each_side() {
        let plan = range_plan(CalendarView::Week, date(2026, 12, 31));
        assert_eq!(plan.visible_start, date(2026, 12, 27));
        assert_eq!(plan.visible_end, date(2027, 1, 3));
        assert_eq!(plan.load_start, date(2026, 12, 20));
        assert_eq!(plan.load_end, date(2027, 1, 10));
    }

    #[test]
    fn spring_forward_and_fall_back_use_real_iana_rules() {
        let new_york: Tz = "America/New_York".parse().unwrap();
        let spring_start = new_york
            .with_ymd_and_hms(2026, 3, 8, 3, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let spring = render_in_zone(
            spring_start,
            spring_start + 60 * 60,
            Some("America/New_York"),
            "America/New_York",
            false,
        )
        .unwrap();
        assert_eq!(spring.local_label, "03:00–04:00");
        let fall_start = new_york
            .with_ymd_and_hms(2026, 11, 1, 1, 30, 0)
            .earliest()
            .unwrap()
            .timestamp();
        let fall_end = new_york
            .with_ymd_and_hms(2026, 11, 1, 2, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        let fall = render_in_zone(
            fall_start,
            fall_end,
            Some("America/New_York"),
            "America/New_York",
            false,
        )
        .unwrap();
        assert_eq!(fall.local_label, "01:30–02:30");
    }

    #[test]
    fn foreign_zone_gets_secondary_text_and_all_day_never_shifts() {
        let foreign = render_in_zone(
            1_768_496_400,
            1_768_500_000,
            Some("America/Los_Angeles"),
            "America/Chicago",
            false,
        )
        .unwrap();
        assert_eq!(foreign.local_label, "11:00–12:00");
        assert_eq!(
            foreign.original_label.as_deref(),
            Some("09:00–10:00 America/Los_Angeles")
        );
        let all_day = render_in_zone(
            1_767_225_600,
            1_767_312_000,
            Some("Pacific/Auckland"),
            "America/Chicago",
            true,
        )
        .unwrap();
        assert_eq!(all_day.local_date, date(2026, 1, 1));
        assert_eq!(all_day.original_label, None);
    }

    #[test]
    fn drag_edits_snap_to_fifteen_minutes_and_preserve_duration() {
        let moving = EventDrag {
            event_id: Some(7),
            kind: DragKind::Move,
            start_minute: 600,
            end_minute: 660,
        };
        assert_eq!(
            apply_drag(&moving, 22),
            EventDrag {
                start_minute: 615,
                end_minute: 675,
                ..moving.clone()
            }
        );
        let resizing = EventDrag {
            kind: DragKind::ResizeEnd,
            ..moving
        };
        assert_eq!(apply_drag(&resizing, -58).end_minute, 615);
    }
}
