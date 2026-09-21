//! The five calendar surfaces from plan.md E12.
//!
//! Date math and packing live in `snail-ui`; this module only coordinates background loads and
//! paints the shared focused date into month, time-grid and agenda presentations.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::calendar::{CalendarEvent, EventEditScope};
use snail_core::ical::expand_event_between;
use snail_core::store::{AttendeeRow, ReminderKind};
use snail_ui::calendar::{
    CalendarView, CivilDate, EventDrag, GridPitch, Interval, MINI_MONTH_CELL_PX, apply_drag,
    month_grid, now_line, pack_overlaps, range_plan, time_rect,
};
use snail_ui::text::TextRole;

use crate::calendar_model::{
    CalendarLoad, CalendarModel, CalendarOccurrence, CalendarTask, EventDetails, EventDraft,
};
use crate::style;

const SIDEBAR_WIDTH: f32 = 232.0;
const AGENDA_DATE_WIDTH: f32 = 132.0;
const AGENDA_ROW_HEIGHT: f32 = 62.0;

pub struct CalendarWorkspace {
    focus: FocusHandle,
    model: CalendarModel,
    view: CalendarView,
    focused: CivilDate,
    today: CivilDate,
    local_timezone: String,
    load: Option<Arc<CalendarLoad>>,
    loading: bool,
    generation: u64,
    time_scroll: ScrollHandle,
    agenda_scroll: UniformListScrollHandle,
    initial_scroll_pending: bool,
    minute_scheduled: bool,
    now_minute: u32,
    show_tasks: bool,
    drag_preview: Option<EventDrag>,
    drag_preview_day: Option<usize>,
    editor: Option<EventEditor>,
    task_input: Option<Entity<InputState>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepeatFrequency {
    Never,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl RepeatFrequency {
    const ALL: [Self; 5] = [
        Self::Never,
        Self::Daily,
        Self::Weekly,
        Self::Monthly,
        Self::Yearly,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
        }
    }

    fn next(self) -> Self {
        Self::ALL
            [(Self::ALL.iter().position(|item| *item == self).unwrap_or(0) + 1) % Self::ALL.len()]
    }
}

struct EventEditor {
    event_id: Option<i64>,
    occurrence_start_utc: Option<i64>,
    calendar_id: i64,
    title: Entity<InputState>,
    date: Entity<InputState>,
    start: Entity<InputState>,
    end: Entity<InputState>,
    location: Entity<InputState>,
    notes: Entity<TextareaState>,
    repeat_end: Entity<InputState>,
    attendee: Entity<InputState>,
    all_day: bool,
    frequency: RepeatFrequency,
    repeat_days: [bool; 7],
    reminders: Vec<ReminderKind>,
    attendees: Vec<AttendeeRow>,
    scope: EventEditScope,
    recurring: bool,
    saving: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct CalendarDrag(EventDrag, usize);

struct CalendarDragPreview(CalendarDrag);

#[derive(Clone)]
enum AgendaRowItem {
    Event(CalendarOccurrence),
    Task(CalendarTask),
}

impl Render for CalendarDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);
        div()
            .w(px(150.0))
            .px_3()
            .py_2()
            .rounded(px(palette.radii.button))
            .border_1()
            .border_color(style::color(palette.colors.accent))
            .bg(style::color(palette.colors.accent_tint))
            .child(style::text(
                format!(
                    "{:02}:{:02}–{:02}:{:02}",
                    self.0.0.start_minute / 60,
                    self.0.0.start_minute % 60,
                    self.0.0.end_minute / 60,
                    self.0.0.end_minute % 60
                ),
                TextRole::EventTime,
                cx,
            ))
    }
}

impl CalendarWorkspace {
    pub fn new(model: CalendarModel, cx: &mut Context<Self>) -> Self {
        let local_timezone = CalendarModel::local_timezone();
        let (today, now_minute) = local_now(&local_timezone);
        let mut this = Self {
            focus: cx.focus_handle(),
            model,
            view: CalendarView::Month,
            focused: today,
            today,
            local_timezone,
            load: None,
            loading: false,
            generation: 0,
            time_scroll: ScrollHandle::new(),
            agenda_scroll: UniformListScrollHandle::new(),
            initial_scroll_pending: true,
            minute_scheduled: false,
            now_minute,
            show_tasks: false,
            drag_preview: None,
            drag_preview_day: None,
            editor: None,
            task_input: None,
        };
        this.request_load(cx);
        this
    }

    pub fn view(&self) -> CalendarView {
        self.view
    }

    pub fn set_view(&mut self, view: CalendarView, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        self.view = view;
        self.initial_scroll_pending = true;
        self.request_load(cx);
        cx.notify();
    }

    pub fn step(&mut self, direction: i32, cx: &mut Context<Self>) {
        self.focused = self.view.step(self.focused, direction);
        self.request_load(cx);
        cx.notify();
    }

    pub fn go_today(&mut self, cx: &mut Context<Self>) {
        self.focused = self.today;
        self.initial_scroll_pending = true;
        self.request_load(cx);
        cx.notify();
    }

    pub fn go_to_date(&mut self, date: CivilDate, cx: &mut Context<Self>) {
        self.focused = date;
        self.initial_scroll_pending = true;
        self.request_load(cx);
        cx.notify();
    }

    pub fn show_calendar(&mut self, id: i64, cx: &mut Context<Self>) {
        if self
            .load
            .as_ref()
            .and_then(|load| load.calendars.iter().find(|calendar| calendar.id == id))
            .is_some_and(|calendar| !calendar.visible)
            && let Err(error) = self.model.set_visible(id, true)
        {
            log::warn!("could not reveal calendar from palette: {error:#}");
        }
        self.request_load(cx);
        cx.notify();
    }

    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    pub fn save_open_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            self.save_editor(cx);
        }
    }

    pub fn cancel_open_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.take().is_some() {
            cx.notify();
        }
    }

    fn select_date(&mut self, date: CivilDate, cx: &mut Context<Self>) {
        self.focused = date;
        self.request_load(cx);
        cx.notify();
    }

    fn open_new_editor(
        &mut self,
        date: CivilDate,
        start_minute: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(calendar_id) = self.load.as_ref().and_then(|load| {
            load.calendars
                .iter()
                .find(|calendar| calendar.visible)
                .or_else(|| load.calendars.first())
                .map(|calendar| calendar.id)
        }) else {
            return;
        };
        let zone: Tz = self.local_timezone.parse().unwrap_or(chrono_tz::UTC);
        let local = date
            .to_naive()
            .and_hms_opt((start_minute / 60) as u32, (start_minute % 60) as u32, 0)
            .and_then(|value| zone.from_local_datetime(&value).earliest());
        let Some(start) = local.map(|value| value.timestamp()) else {
            return;
        };
        let draft = EventDraft {
            event_id: None,
            occurrence_start_utc: None,
            calendar_id,
            title: String::new(),
            location: String::new(),
            notes: String::new(),
            start_utc: start,
            end_utc: start + 3600,
            all_day: false,
            timezone: self.local_timezone.clone(),
            rrule: None,
            reminders: vec![ReminderKind::MinutesBefore(10)],
            attendees: Vec::new(),
            scope: EventEditScope::All,
        };
        self.install_editor(
            EventDetails {
                draft,
                is_recurring: false,
            },
            window,
            cx,
        );
    }

    fn open_existing_editor(
        &mut self,
        event_id: i64,
        occurrence_start_utc: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let task = cx
            .background_executor()
            .spawn(async move { model.event_details(event_id, occurrence_start_utc) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(Some(details)) => this.install_editor(details, window, cx),
                Ok(None) => {}
                Err(error) => log::warn!("could not load event editor: {error:#}"),
            })
            .ok();
        })
        .detach();
    }

    fn install_editor(
        &mut self,
        details: EventDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone: Tz = details.draft.timezone.parse().unwrap_or(chrono_tz::UTC);
        let start = Utc
            .timestamp_opt(details.draft.start_utc, 0)
            .single()
            .unwrap_or_else(Utc::now)
            .with_timezone(&zone);
        let end = Utc
            .timestamp_opt(details.draft.end_utc, 0)
            .single()
            .unwrap_or_else(Utc::now)
            .with_timezone(&zone);
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Event title")
                .default_value(details.draft.title.clone())
        });
        let date = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("YYYY-MM-DD")
                .default_value(start.format("%Y-%m-%d").to_string())
        });
        let start_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("09:00")
                .default_value(start.format("%H:%M").to_string())
        });
        let end_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("10:00")
                .default_value(end.format("%H:%M").to_string())
        });
        let location = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Location")
                .default_value(details.draft.location.clone())
        });
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Notes")
                .default_value(details.draft.notes.clone())
        });
        let repeat_end = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("YYYY-MM-DD")
                .default_value(
                    rrule_until(details.draft.rrule.as_deref()).unwrap_or_else(|| {
                        start
                            .date_naive()
                            .checked_add_months(chrono::Months::new(6))
                            .unwrap_or(start.date_naive())
                            .format("%Y-%m-%d")
                            .to_string()
                    }),
                )
        });
        let attendee = cx.new(|cx| InputState::new(window, cx).placeholder("person@example.com"));
        let mut subscriptions = Vec::new();
        for input in [
            &title,
            &date,
            &start_input,
            &end_input,
            &location,
            &repeat_end,
            &attendee,
        ] {
            subscriptions.push(cx.subscribe(input, |_this, _state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.subscribe(&notes, |_this, _state, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        }));
        let (frequency, repeat_days) =
            parse_repeat(details.draft.rrule.as_deref(), start.weekday());
        self.editor = Some(EventEditor {
            event_id: details.draft.event_id,
            occurrence_start_utc: details.draft.occurrence_start_utc,
            calendar_id: details.draft.calendar_id,
            title,
            date,
            start: start_input,
            end: end_input,
            location,
            notes,
            repeat_end,
            attendee,
            all_day: details.draft.all_day,
            frequency,
            repeat_days,
            reminders: details.draft.reminders,
            attendees: details.draft.attendees,
            scope: EventEditScope::All,
            recurring: details.is_recurring,
            saving: false,
            error: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn request_load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let model = self.model.clone();
        let timezone = self.local_timezone.clone();
        let plan = range_plan(self.view, self.focused);
        let task = cx
            .background_executor()
            .spawn(async move { model.load(plan, &timezone) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(load) => this.load = Some(Arc::new(load)),
                    Err(error) => log::warn!("calendar range load failed: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_calendar(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(load) = &self.load else { return };
        let Some(calendar) = load.calendars.iter().find(|calendar| calendar.id == id) else {
            return;
        };
        let visible = !calendar.visible;

        // The current frame drops hidden events immediately; persistence and the authoritative
        // reload happen off the UI thread.
        let mut next = (**load).clone();
        if let Some(calendar) = next.calendars.iter_mut().find(|calendar| calendar.id == id) {
            calendar.visible = visible;
        }
        if !visible {
            next.occurrences.retain(|event| event.calendar_id != id);
        }
        self.load = Some(Arc::new(next));
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let model = self.model.clone();
        let timezone = self.local_timezone.clone();
        let plan = range_plan(self.view, self.focused);
        let task = cx.background_executor().spawn(async move {
            model.set_visible(id, visible)?;
            model.load(plan, &timezone)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(load) => this.load = Some(Arc::new(load)),
                    Err(error) => log::warn!("calendar visibility update failed: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn editor_draft(&self, cx: &App) -> Result<EventDraft, String> {
        self.editor_draft_with_validation(cx, true)
    }

    fn editor_preview_draft(&self, cx: &App) -> Result<EventDraft, String> {
        self.editor_draft_with_validation(cx, false)
    }

    fn editor_draft_with_validation(
        &self,
        cx: &App,
        require_title: bool,
    ) -> Result<EventDraft, String> {
        let editor = self.editor.as_ref().ok_or("The editor is closed")?;
        let title = editor.title.read(cx).value().trim().to_string();
        if require_title && title.is_empty() {
            return Err("Add a title before saving.".into());
        }
        let date = NaiveDate::parse_from_str(editor.date.read(cx).value().trim(), "%Y-%m-%d")
            .map_err(|_| "Use YYYY-MM-DD for the date.".to_string())?;
        let parse_time = |value: &str| {
            chrono::NaiveTime::parse_from_str(value.trim(), "%H:%M")
                .map_err(|_| "Use HH:MM for event times.".to_string())
        };
        let start_time = if editor.all_day {
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
        } else {
            parse_time(&editor.start.read(cx).value())?
        };
        let end_time = if editor.all_day {
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
        } else {
            parse_time(&editor.end.read(cx).value())?
        };
        let zone: Tz = self
            .local_timezone
            .parse()
            .map_err(|_| "The local timezone is invalid.".to_string())?;
        let start = zone
            .from_local_datetime(&date.and_time(start_time))
            .earliest()
            .ok_or_else(|| "That start time does not exist in this timezone.".to_string())?;
        let mut end_date = date;
        if editor.all_day {
            end_date = date.succ_opt().ok_or("The date is out of range")?;
        } else if end_time <= start_time {
            end_date = date.succ_opt().ok_or("The date is out of range")?;
        }
        let end = zone
            .from_local_datetime(&end_date.and_time(end_time))
            .earliest()
            .ok_or_else(|| "That end time does not exist in this timezone.".to_string())?;
        let rrule = build_rrule(editor, date, cx)?;
        Ok(EventDraft {
            event_id: editor.event_id,
            occurrence_start_utc: editor.occurrence_start_utc,
            calendar_id: editor.calendar_id,
            title,
            location: editor.location.read(cx).value().trim().to_string(),
            notes: editor.notes.read(cx).value().trim().to_string(),
            start_utc: start.timestamp(),
            end_utc: end.timestamp(),
            all_day: editor.all_day,
            timezone: self.local_timezone.clone(),
            rrule,
            reminders: editor.reminders.clone(),
            attendees: editor.attendees.clone(),
            scope: editor.scope,
        })
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let draft = match self.editor_draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(error);
                }
                cx.notify();
                return;
            }
        };
        if let Some(editor) = &mut self.editor {
            editor.saving = true;
            editor.error = None;
        }
        let model = self.model.clone();
        let now = Utc::now().timestamp();
        let task = cx
            .background_executor()
            .spawn(async move { model.save_event(&draft, now) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.editor = None;
                    this.request_load(cx);
                }
                Err(error) => {
                    if let Some(editor) = &mut this.editor {
                        editor.saving = false;
                        editor.error = Some(format!("Could not save: {error:#}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn delete_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (Some(event_id), Some(occurrence)) = (editor.event_id, editor.occurrence_start_utc)
        else {
            self.editor = None;
            cx.notify();
            return;
        };
        editor.saving = true;
        let scope = editor.scope;
        let model = self.model.clone();
        let task = cx.background_executor().spawn(async move {
            model.delete_event(event_id, occurrence, scope, Utc::now().timestamp())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.editor = None;
                    this.request_load(cx);
                }
                Err(error) => {
                    if let Some(editor) = &mut this.editor {
                        editor.saving = false;
                        editor.error = Some(format!("Could not delete: {error:#}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn add_attendee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let email = editor.attendee.read(cx).value().trim().to_ascii_lowercase();
        if !email.contains('@') || editor.attendees.iter().any(|item| item.email == email) {
            return;
        }
        editor.attendees.push(AttendeeRow {
            id: 0,
            event_id: editor.event_id.unwrap_or_default(),
            email,
            display_name: None,
            role: "required".into(),
            status: "needs-action".into(),
            is_self: false,
        });
        editor
            .attendee
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn begin_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task_input.is_none() {
            self.task_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("New task")));
        }
        cx.notify();
    }

    fn commit_task(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.task_input else {
            return;
        };
        let title = input.read(cx).value().trim().to_string();
        if title.is_empty() {
            self.task_input = None;
            cx.notify();
            return;
        }
        let zone: Tz = self.local_timezone.parse().unwrap_or(chrono_tz::UTC);
        let due = self
            .focused
            .to_naive()
            .and_hms_opt(17, 0, 0)
            .and_then(|value| zone.from_local_datetime(&value).earliest())
            .map(|value| value.timestamp());
        let model = self.model.clone();
        let task = cx
            .background_executor()
            .spawn(async move { model.create_task(&title, due, Utc::now().timestamp()) });
        self.task_input = None;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log::warn!("could not create task: {error:#}");
                }
                this.request_load(cx);
            })
            .ok();
        })
        .detach();
    }

    fn toggle_task_done(&mut self, task_id: i64, done: bool, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let task = cx
            .background_executor()
            .spawn(async move { model.set_task_done(task_id, done) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log::warn!("could not update task: {error:#}");
                }
                this.request_load(cx);
            })
            .ok();
        })
        .detach();
    }

    fn schedule_minute_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.minute_scheduled {
            return;
        }
        self.minute_scheduled = true;
        // GPUI requires this handoff to start in render. The background timer wakes exactly at the
        // next minute boundary and the next render schedules the following tick.
        cx.on_next_frame(window, |_this, _window, cx| {
            let wait = Duration::from_secs(60 - (Utc::now().timestamp() as u64 % 60));
            let timer = cx.background_executor().timer(wait);
            cx.spawn(async move |this, cx| {
                timer.await;
                this.update(cx, |this, cx| {
                    let (today, minute) = local_now(&this.local_timezone);
                    this.today = today;
                    this.now_minute = minute;
                    this.minute_scheduled = false;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        });
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let grid = month_grid(self.focused);
        let month_name = self.focused.to_naive().format("%B %Y").to_string();
        let mut mini = div().grid().grid_cols(7).grid_rows(6).w_full();
        for cell in grid {
            let selected = cell.date == self.focused;
            let today = cell.date == self.today;
            let date = cell.date;
            mini = mini.child(
                div()
                    .id(("mini-day", date_key(cell.date)))
                    .h(px(MINI_MONTH_CELL_PX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(MINI_MONTH_CELL_PX / 2.0))
                    .cursor_pointer()
                    .when(selected && !today, |this| {
                        this.bg(style::color(palette.colors.accent_tint))
                    })
                    .when(today, |this| this.bg(style::color(palette.colors.accent)))
                    .when(!cell.in_month && !today, |this| {
                        this.text_color(style::color(palette.colors.faint))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_date(date, cx)))
                    .child(style::text(
                        cell.date.day.to_string(),
                        if today {
                            TextRole::MonthDayToday
                        } else if cell.in_month {
                            TextRole::MonthDayNumeral
                        } else {
                            TextRole::MonthDayOutside
                        },
                        cx,
                    )),
            );
        }

        let calendars = self
            .load
            .as_ref()
            .map(|load| load.calendars.clone())
            .unwrap_or_default();
        let tasks = self
            .load
            .as_ref()
            .map(|load| load.tasks.clone())
            .unwrap_or_default();
        let task_input = self.task_input.clone();
        div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.chrome))
            .border_r_1()
            .border_color(style::color(palette.colors.border_soft))
            .px_3()
            .py_3()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(style::text(month_name, TextRole::ListHeaderTitle, cx))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.sidebar_arrow("mini-prev", "‹", -1, cx))
                            .child(self.sidebar_arrow("mini-next", "›", 1, cx)),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(7)
                    .children(["S", "M", "T", "W", "T", "F", "S"].map(|day| {
                        div()
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(style::text(day, TextRole::WeekdayLabel, cx))
                    })),
            )
            .child(mini)
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .child(style::text("Calendars", TextRole::WeekdayLabel, cx)),
            )
            .children(calendars.into_iter().map(|calendar| {
                let id = calendar.id;
                let color = calendar_color(id, palette);
                div()
                    .id(("calendar-toggle", id as u64))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .rounded(px(palette.radii.button))
                    .cursor_pointer()
                    .hover(|this| this.bg(style::color(palette.colors.sunken)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_calendar(id, cx)))
                    .child(
                        div()
                            .size(px(10.0))
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(color)
                            .when(calendar.visible, |this| this.bg(color)),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            style::text(calendar.name, TextRole::SidebarItem, cx).truncate(),
                        ),
                    )
                    .child(style::text(
                        calendar.event_count.to_string(),
                        TextRole::SidebarCount,
                        cx,
                    ))
            }))
            .child(
                div()
                    .mt_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(style::text(
                        "Tasks · local only",
                        TextRole::WeekdayLabel,
                        cx,
                    ))
                    .child(
                        div()
                            .id("task-add")
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.begin_task(window, cx)),
                            )
                            .child("+"),
                    ),
            )
            .children(tasks.iter().take(6).map(|task| {
                let id = task.id;
                let done = task.done;
                let overdue = task.due_date.is_some_and(|date| date < self.today) && !done;
                div()
                    .id(("task", id as u64))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_task_done(id, !done, cx)),
                    )
                    .child(if done { "✓" } else { "○" })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(overdue, |this| {
                                this.text_color(style::color(palette.colors.danger))
                            })
                            .when(done, |this| this.opacity(0.55).line_through())
                            .child(style::text(task.title.clone(), TextRole::EventNotes, cx)),
                    )
            }))
            .when_some(task_input, |this, input| {
                this.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(Input::new(&input)))
                        .child(
                            div()
                                .id("task-save")
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded(px(palette.radii.button))
                                .bg(style::color(palette.colors.accent))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.commit_task(cx)))
                                .child(style::text("Add", TextRole::CalendarButtonLabelFilled, cx)),
                        ),
                )
            })
            .child(div().flex_1())
            .when(self.loading, |this| {
                this.child(style::text("Loading…", TextRole::SidebarCount, cx))
            })
            .child(style::text(
                self.local_timezone.clone(),
                TextRole::SidebarCount,
                cx,
            ))
            .into_any_element()
    }

    fn sidebar_arrow(
        &self,
        id: &'static str,
        label: &'static str,
        direction: i32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id(id)
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(palette.radii.button))
            .cursor_pointer()
            .hover(|this| this.bg(style::color(palette.colors.accent_tint)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focused = this.focused.add_months(direction);
                this.request_load(cx);
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    fn header(&self, height: f32, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let title = match self.view {
            CalendarView::Month => self.focused.to_naive().format("%B").to_string(),
            CalendarView::Week => format!("Week {}", self.focused.iso_week()),
            CalendarView::ThreeDay => self.focused.to_naive().format("%b %-d").to_string(),
            CalendarView::Day => self.focused.to_naive().format("%A, %B %-d").to_string(),
            CalendarView::Agenda => "Agenda".into(),
        };
        div()
            .h(px(height))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_5()
            .border_b_1()
            .border_color(style::color(palette.colors.border_soft))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .child(style::text(
                        title,
                        if self.view == CalendarView::Month {
                            TextRole::CalendarMonthTitle
                        } else {
                            TextRole::CalendarTitle
                        },
                        cx,
                    ))
                    .child(style::text(
                        self.focused.year.to_string(),
                        TextRole::CalendarYear,
                        cx,
                    ))
                    .when(self.view == CalendarView::Month, |this| {
                        this.child(style::text(
                            format!("W{}", self.focused.iso_week()),
                            TextRole::CalendarYear,
                            cx,
                        ))
                        .child(style::text(
                            format!(
                                "{} calendars",
                                self.load
                                    .as_ref()
                                    .map(|load| load
                                        .calendars
                                        .iter()
                                        .filter(|calendar| calendar.visible)
                                        .count())
                                    .unwrap_or(0)
                            ),
                            TextRole::CalendarYear,
                            cx,
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("calendar-new-event")
                            .h(px(30.0))
                            .px_3()
                            .mr_2()
                            .flex()
                            .items_center()
                            .rounded(px(palette.radii.button))
                            .cursor_pointer()
                            .bg(style::color(palette.colors.accent))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_editor(this.focused, 9 * 60, window, cx)
                            }))
                            .child(style::text(
                                "+ New event",
                                TextRole::CalendarButtonLabelFilled,
                                cx,
                            )),
                    )
                    .child(self.step_button("calendar-prev", "‹", -1, cx))
                    .child(self.today_button(cx))
                    .child(self.step_button("calendar-next", "›", 1, cx)),
            )
            .into_any_element()
    }

    fn step_button(
        &self,
        id: &'static str,
        label: &'static str,
        direction: i32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id(id)
            .size(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(palette.radii.button))
            .border_1()
            .border_color(style::color(palette.colors.border_soft))
            .cursor_pointer()
            .hover(|this| this.bg(style::color(palette.colors.sunken)))
            .on_click(cx.listener(move |this, _, _, cx| this.step(direction, cx)))
            .child(label)
            .into_any_element()
    }

    fn today_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id("calendar-today")
            .h(px(30.0))
            .px_3()
            .flex()
            .items_center()
            .rounded(px(palette.radii.button))
            .border_1()
            .border_color(style::color(palette.colors.border_soft))
            .cursor_pointer()
            .hover(|this| this.bg(style::color(palette.colors.sunken)))
            .on_click(cx.listener(|this, _, _, cx| this.go_today(cx)))
            .child(style::text("Today", TextRole::ButtonLabel, cx))
            .into_any_element()
    }

    fn month_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let occurrences = self.occurrences();
        let mut cells = div().grid().grid_cols(7).grid_rows(6).flex_1().min_h_0();
        for cell in month_grid(self.focused) {
            let mut events: Vec<_> = occurrences
                .iter()
                .filter(|event| event.rendered.local_date == cell.date)
                .collect();
            events.sort_by_key(|event| (event.rendered.local_start_minute, event.event_id));
            let overflow = events.len().saturating_sub(3);
            let date = cell.date;
            cells = cells.child(
                div()
                    .id(("month-cell", date_key(cell.date)))
                    .min_h_0()
                    .p_2()
                    .overflow_hidden()
                    .border_r_1()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .cursor_pointer()
                    .when(cell.date == self.today, |this| {
                        this.bg(style::color(palette.colors.accent_tint))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_date(date, cx)))
                    .child(
                        div()
                            .size(px(25.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(13.0))
                            .when(cell.date == self.today, |this| {
                                this.bg(style::color(palette.colors.accent))
                            })
                            .when(!cell.in_month && cell.date != self.today, |this| {
                                this.text_color(style::color(palette.colors.faint))
                            })
                            .child(style::text(
                                cell.date.day.to_string(),
                                if cell.date == self.today {
                                    TextRole::MonthDayToday
                                } else if cell.in_month {
                                    TextRole::MonthDayNumeral
                                } else {
                                    TextRole::MonthDayOutside
                                },
                                cx,
                            )),
                    )
                    .children(
                        events
                            .into_iter()
                            .take(3)
                            .map(|event| self.month_chip(event, cx)),
                    )
                    .when(overflow > 0, |this| {
                        this.child(style::text(
                            format!("+{overflow} more"),
                            TextRole::EventNotes,
                            cx,
                        ))
                    }),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(88.0, cx))
            .child(
                div()
                    .h(px(30.0))
                    .flex_none()
                    .grid()
                    .grid_cols(7)
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .children(
                        [
                            "Sunday",
                            "Monday",
                            "Tuesday",
                            "Wednesday",
                            "Thursday",
                            "Friday",
                            "Saturday",
                        ]
                        .map(|day| {
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(style::text(day, TextRole::WeekdayLabel, cx))
                        }),
                    ),
            )
            .child(cells)
            .into_any_element()
    }

    fn month_chip(&self, event: &CalendarOccurrence, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let event_id = event.event_id;
        let occurrence_start = event.occurrence_start_utc;
        div()
            .id(("month-event", occurrence_start as u64))
            .mt_1()
            .h(px(20.0))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(palette.radii.label))
            .bg(calendar_tint(event.calendar_id, palette))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_existing_editor(event_id, occurrence_start, window, cx)
            }))
            .child(
                div()
                    .size(px(6.0))
                    .rounded(px(3.0))
                    .bg(calendar_color(event.calendar_id, palette)),
            )
            .child(style::text(event.title.clone(), TextRole::EventNotes, cx).truncate())
            .into_any_element()
    }

    fn time_grid_view(
        &mut self,
        days: usize,
        pitch: GridPitch,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        let header_height = if days == 7 {
            78.0
        } else if days == 3 {
            78.0
        } else {
            106.0
        };
        let start = if days == 7 {
            self.focused.start_of_week_sunday()
        } else {
            self.focused
        };
        let dates: Vec<_> = (0..days).map(|day| start.add_days(day as i64)).collect();
        let all_day: Vec<_> = self
            .occurrences()
            .iter()
            .filter(|event| event.rendered.all_day && dates.contains(&event.rendered.local_date))
            .cloned()
            .collect();
        let grid_height = pitch.pixels() * 24.0;
        if self.initial_scroll_pending {
            let target = (self.now_minute.saturating_sub(60) as f32 / 60.0) * pitch.pixels();
            self.time_scroll.set_offset(point(px(0.0), px(-target)));
            self.initial_scroll_pending = false;
        }

        let mut columns = div()
            .relative()
            .ml(px(58.0))
            .h(px(grid_height))
            .grid()
            .grid_cols(days as u16);
        for (index, date) in dates.iter().copied().enumerate() {
            let day_events: Vec<_> = self
                .occurrences()
                .iter()
                .filter(|event| !event.rendered.all_day && event.rendered.local_date == date)
                .cloned()
                .collect();
            let intervals: Vec<_> = day_events
                .iter()
                .map(|event| Interval {
                    id: (event.event_id, event.occurrence_start_utc),
                    start: event.rendered.local_start_minute as i64,
                    end: end_minute(event) as i64,
                })
                .collect();
            let packed = pack_overlaps(&intervals);
            let mut column = div()
                .id(("time-day", index))
                .relative()
                .h(px(grid_height))
                .border_r_1()
                .border_color(style::color(palette.colors.border_soft))
                .when(date == self.today, |this| {
                    this.bg(style::color(palette.colors.accent_tint))
                });
            for hour in 0..24 {
                column = column.child(
                    div()
                        .absolute()
                        .top(px(hour as f32 * pitch.pixels()))
                        .left_0()
                        .right_0()
                        .border_t_1()
                        .border_color(style::color(palette.colors.border_hairline)),
                );
            }
            for quarter in 0..96 {
                let start_minute = quarter * 15;
                let payload = CalendarDrag(
                    EventDrag {
                        event_id: None,
                        kind: snail_ui::calendar::DragKind::Create,
                        start_minute,
                        end_minute: start_minute + 15,
                    },
                    index,
                );
                column = column.child(
                    div()
                        .id(("create-slot", index * 96 + quarter as usize))
                        .absolute()
                        .top(px(start_minute as f32 / 60.0 * pitch.pixels()))
                        .left_0()
                        .right_0()
                        .h(px(pitch.pixels() / 4.0))
                        .cursor_crosshair()
                        .on_drag(payload, |payload, _, _, cx| {
                            cx.new(|_| CalendarDragPreview(payload.clone()))
                        })
                        .on_drag_move(cx.listener(
                            move |this, event: &DragMoveEvent<CalendarDrag>, _, cx| {
                                let payload = event.drag(cx).0.clone();
                                let relative_y =
                                    f32::from(event.event.position.y - event.bounds.top());
                                let target = payload.start_minute
                                    + (relative_y / pitch.pixels() * 60.0) as i32;
                                this.drag_preview =
                                    Some(apply_drag(&payload, target - payload.end_minute));
                                this.drag_preview_day = Some(index);
                                cx.notify();
                            },
                        )),
                );
            }
            for event in day_events {
                let Some(slot) = packed
                    .iter()
                    .find(|slot| slot.id == (event.event_id, event.occurrence_start_utc))
                else {
                    continue;
                };
                let rect = time_rect(
                    event.rendered.local_start_minute as i32,
                    end_minute(&event) as i32,
                    0,
                    pitch,
                    3.0,
                );
                column = column.child(self.timed_event(
                    event,
                    slot.left_fraction,
                    slot.width_fraction,
                    rect.top,
                    rect.height,
                    index,
                    pitch,
                    cx,
                ));
            }
            if date == self.today {
                let top = now_line(self.now_minute, 0, pitch);
                column = column.child(
                    div()
                        .absolute()
                        .top(px(top))
                        .left_0()
                        .right_0()
                        .h(px(1.5))
                        .bg(style::color(palette.colors.accent))
                        .child(
                            div()
                                .absolute()
                                .left(px(-4.0))
                                .top(px(-3.25))
                                .size(px(8.0))
                                .rounded(px(4.0))
                                .bg(style::color(palette.colors.accent)),
                        ),
                );
            }
            if self.drag_preview_day == Some(index)
                && let Some(preview) = self.drag_preview.clone()
            {
                let rect = time_rect(preview.start_minute, preview.end_minute, 0, pitch, 3.0);
                column = column.child(
                    div()
                        .absolute()
                        .top(px(rect.top))
                        .left(px(3.0))
                        .right(px(3.0))
                        .h(px(rect.height))
                        .rounded(px(palette.radii.label))
                        .border_1()
                        .border_color(style::color(palette.colors.accent))
                        .bg(style::color(palette.colors.accent_tint))
                        .child(style::text(
                            "Preview · 15 min snap",
                            TextRole::EventNotes,
                            cx,
                        )),
                );
            }
            columns = columns.child(column);
        }

        let drag_pitch = pitch;
        let drag_days = days;
        let time_scroll = self.time_scroll.clone();
        let body = div()
            .id("calendar-time-scroll")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.time_scroll)
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<CalendarDrag>, _, cx| {
                    let payload = event.drag(cx).0.clone();
                    let scroll_y = -f32::from(time_scroll.offset().y);
                    let relative_y =
                        f32::from(event.event.position.y - event.bounds.top()) + scroll_y;
                    let target = (relative_y / drag_pitch.pixels() * 60.0) as i32;
                    let available_width = (f32::from(event.bounds.size.width) - 58.0).max(1.0);
                    let relative_x = (f32::from(event.event.position.x - event.bounds.left())
                        - 58.0)
                        .clamp(0.0, available_width - 1.0);
                    this.drag_preview_day = Some(
                        (relative_x / (available_width / drag_days as f32))
                            .floor()
                            .clamp(0.0, drag_days.saturating_sub(1) as f32)
                            as usize,
                    );
                    let anchor = match payload.kind {
                        snail_ui::calendar::DragKind::Create
                        | snail_ui::calendar::DragKind::ResizeEnd => payload.end_minute,
                        snail_ui::calendar::DragKind::Move
                        | snail_ui::calendar::DragKind::ResizeStart => payload.start_minute,
                    };
                    this.drag_preview = Some(apply_drag(&payload, target - anchor));
                    cx.notify();
                }),
            )
            .on_drop(cx.listener(|this, payload: &CalendarDrag, _, cx| {
                if this.drag_preview.is_none() {
                    this.drag_preview = Some(payload.0.clone());
                    this.drag_preview_day = Some(payload.1);
                }
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .h(px(grid_height))
                    .children((0..24).map(|hour| {
                        div()
                            .absolute()
                            .top(px(hour as f32 * pitch.pixels() - 7.0))
                            .left_0()
                            .w(px(52.0))
                            .flex()
                            .justify_end()
                            .pr_2()
                            .child(style::text(
                                format!("{hour:02}:00"),
                                TextRole::HourGutter,
                                cx,
                            ))
                    }))
                    .child(columns)
                    .when(
                        pitch == GridPitch::ThreeDay && dates.contains(&self.today),
                        |this| {
                            let top = now_line(self.now_minute, 0, pitch);
                            this.child(
                                div()
                                    .absolute()
                                    .top(px(top - 10.0))
                                    .left(px(5.0))
                                    .w(px(48.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(palette.radii.label))
                                    .bg(style::color(palette.colors.accent))
                                    .child(style::text(
                                        format!(
                                            "{:02}:{:02}",
                                            self.now_minute / 60,
                                            self.now_minute % 60
                                        ),
                                        TextRole::CalendarButtonLabelFilled,
                                        cx,
                                    )),
                            )
                        },
                    ),
            );

        let calendar = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.header(header_height, cx))
            .child(self.day_headers(&dates, cx))
            .child(self.all_day_band(&dates, &all_day, cx))
            .child(body);

        if days == 1 {
            div()
                .size_full()
                .flex()
                .child(calendar)
                .child(self.day_rail(cx))
                .into_any_element()
        } else {
            calendar.into_any_element()
        }
    }

    fn day_headers(&self, dates: &[CivilDate], cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .h(px(52.0))
            .flex_none()
            .pl(px(58.0))
            .grid()
            .grid_cols(dates.len() as u16)
            .border_b_1()
            .border_color(style::color(palette.colors.border_soft))
            .children(dates.iter().copied().map(|date| {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .border_r_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .when(date == self.today, |this| {
                        this.text_color(style::color(palette.colors.accent))
                    })
                    .child(style::text(
                        date.to_naive().format("%a").to_string(),
                        TextRole::WeekdayLabel,
                        cx,
                    ))
                    .child(style::text(
                        date.day.to_string(),
                        TextRole::MonthDayNumeral,
                        cx,
                    ))
            }))
            .into_any_element()
    }

    fn all_day_band(
        &self,
        dates: &[CivilDate],
        events: &[CalendarOccurrence],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .h(px(40.0))
            .flex_none()
            .pl(px(58.0))
            .grid()
            .grid_cols(dates.len() as u16)
            .border_b_1()
            .border_color(style::color(palette.colors.border_soft))
            .children(dates.iter().copied().map(|date| {
                div()
                    .min_w_0()
                    .border_r_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .children(
                        events
                            .iter()
                            .filter(move |event| event.rendered.local_date == date)
                            .take(1)
                            .map(|event| self.month_chip(event, cx)),
                    )
            }))
            .into_any_element()
    }

    fn timed_event(
        &self,
        event: CalendarOccurrence,
        left: f32,
        width: f32,
        top: f32,
        height: f32,
        day_index: usize,
        pitch: GridPitch,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        let event_id = event.event_id;
        let occurrence_start = event.occurrence_start_utc;
        let payload = CalendarDrag(
            EventDrag {
                event_id: Some(event.event_id),
                kind: snail_ui::calendar::DragKind::Move,
                start_minute: event.rendered.local_start_minute as i32,
                end_minute: end_minute(&event) as i32,
            },
            day_index,
        );
        let resize_start = CalendarDrag(
            EventDrag {
                kind: snail_ui::calendar::DragKind::ResizeStart,
                ..payload.0.clone()
            },
            day_index,
        );
        let resize_end = CalendarDrag(
            EventDrag {
                kind: snail_ui::calendar::DragKind::ResizeEnd,
                ..payload.0.clone()
            },
            day_index,
        );
        div()
            .id(("timed-event", event.event_id as u64))
            .absolute()
            .top(px(top))
            .left(relative(left))
            .w(relative(width))
            .h(px(height))
            .px_2()
            .py_1()
            .overflow_hidden()
            .cursor_grab()
            .rounded(px(palette.radii.label))
            .border_l_2()
            .border_color(calendar_color(event.calendar_id, palette))
            .bg(calendar_tint(event.calendar_id, palette))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_existing_editor(event_id, occurrence_start, window, cx)
            }))
            .on_drag(payload, |payload, _, _, cx| {
                cx.new(|_| CalendarDragPreview(payload.clone()))
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<CalendarDrag>, _, cx| {
                    let payload = event.drag(cx).0.clone();
                    let relative_y = f32::from(event.event.position.y - event.bounds.top());
                    let delta = (relative_y / pitch.pixels() * 60.0) as i32;
                    this.drag_preview = Some(apply_drag(&payload, delta));
                    this.drag_preview_day = Some(day_index);
                    cx.notify();
                }),
            )
            .child(style::text(event.title, TextRole::EventTitle, cx).truncate())
            .child(style::text(
                event.rendered.local_label,
                TextRole::EventTime,
                cx,
            ))
            .when_some(event.rendered.original_label, |this, original| {
                this.child(style::text(original, TextRole::EventNotes, cx))
            })
            .child(
                div()
                    .id(("resize-start", event.event_id as u64))
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(5.0))
                    .cursor_row_resize()
                    .on_drag(resize_start, |payload, _, _, cx| {
                        cx.new(|_| CalendarDragPreview(payload.clone()))
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<CalendarDrag>, _, cx| {
                            let payload = event.drag(cx).0.clone();
                            let relative_y = f32::from(event.event.position.y - event.bounds.top());
                            let delta = (relative_y / pitch.pixels() * 60.0) as i32;
                            this.drag_preview = Some(apply_drag(&payload, delta));
                            this.drag_preview_day = Some(day_index);
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .id(("resize-end", event.event_id as u64))
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(5.0))
                    .cursor_row_resize()
                    .on_drag(resize_end, |payload, _, _, cx| {
                        cx.new(|_| CalendarDragPreview(payload.clone()))
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<CalendarDrag>, _, cx| {
                            let payload = event.drag(cx).0.clone();
                            let relative_y = f32::from(event.event.position.y - event.bounds.top());
                            let delta = (relative_y / pitch.pixels() * 60.0) as i32;
                            this.drag_preview = Some(apply_drag(&payload, delta));
                            this.drag_preview_day = Some(day_index);
                            cx.notify();
                        },
                    )),
            )
            .into_any_element()
    }

    fn day_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let reminders: Vec<_> = self
            .occurrences()
            .iter()
            .filter(|event| event.rendered.local_date == self.focused)
            .take(4)
            .cloned()
            .collect();
        let tasks = self
            .load
            .as_ref()
            .map(|load| {
                load.tasks
                    .iter()
                    .filter(|task| task.due_date == Some(self.focused))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .w(px(palette.metrics.rail_w))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(style::color(palette.colors.border_soft))
            .bg(style::color(palette.colors.card))
            .p_4()
            .gap_4()
            .child(self.task_rail_section(&tasks, cx))
            .child(self.rail_section("Reminders", &reminders, cx))
            .child(self.rail_section("Repeats", &[], cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("calendar-add-to-day")
                    .h(px(46.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_dashed()
                    .border_color(style::color(palette.colors.border_strong))
                    .rounded(px(palette.radii.button))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_editor(this.focused, 9 * 60, window, cx)
                    }))
                    .child(style::text("+ Add to this day", TextRole::ButtonLabel, cx)),
            )
            .into_any_element()
    }

    fn rail_section(
        &self,
        title: &'static str,
        events: &[CalendarOccurrence],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(style::text(title, TextRole::WeekdayLabel, cx))
            .when(events.is_empty(), |this| {
                this.child(style::text("Nothing here", TextRole::EventNotes, cx))
            })
            .children(events.iter().map(|event| {
                div()
                    .flex()
                    .gap_2()
                    .child(style::text(
                        event.rendered.local_label.clone(),
                        TextRole::EventTime,
                        cx,
                    ))
                    .child(style::text(event.title.clone(), TextRole::EventNotes, cx))
            }))
            .into_any_element()
    }

    fn task_rail_section(
        &self,
        tasks: &[crate::calendar_model::CalendarTask],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(style::text("Due today", TextRole::WeekdayLabel, cx))
            .when(tasks.is_empty(), |this| {
                this.child(style::text("Nothing here", TextRole::EventNotes, cx))
            })
            .children(tasks.iter().map(|task| {
                let id = task.id;
                let done = task.done;
                div()
                    .id(("rail-task", id as u64))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_task_done(id, !done, cx)),
                    )
                    .child(if done { "✓" } else { "○" })
                    .child(
                        div()
                            .when(done, |this| this.opacity(0.55).line_through())
                            .child(style::text(task.title.clone(), TextRole::EventNotes, cx)),
                    )
            }))
            .into_any_element()
    }

    fn agenda_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let mut entries: Vec<_> = self
            .occurrences()
            .iter()
            .cloned()
            .map(|event| {
                (
                    event.rendered.local_date,
                    event.rendered.local_start_minute,
                    AgendaRowItem::Event(event),
                )
            })
            .collect();
        if self.show_tasks {
            entries.extend(
                self.load
                    .as_ref()
                    .into_iter()
                    .flat_map(|load| load.tasks.iter())
                    .filter_map(|task| {
                        task.due_date
                            .map(|date| (date, 24 * 60 - 1, AgendaRowItem::Task(task.clone())))
                    }),
            );
        }
        entries.sort_by_key(|(date, minute, item)| {
            let kind = matches!(item, AgendaRowItem::Task(_)) as u8;
            (*date, *minute, kind)
        });
        let mut previous = None;
        let rows: Vec<_> = entries
            .into_iter()
            .map(|(date, _, item)| {
                let first = previous != Some(date);
                previous = Some(date);
                (date, first, item)
            })
            .collect();
        let rows = Arc::new(rows);
        let row_count = rows.len();
        let list_rows = rows.clone();
        let list = uniform_list("calendar-agenda", row_count, move |range, _, cx| {
            let palette = style::palette(cx);
            range
                .map(|index| {
                    let (date, first, item) = &list_rows[index];
                    let (time, color, title, location, done) = match item {
                        AgendaRowItem::Event(event) => (
                            if event.rendered.all_day {
                                "All day".into()
                            } else {
                                event.rendered.local_label.clone()
                            },
                            calendar_color(event.calendar_id, palette),
                            event.title.clone(),
                            event.location.clone().unwrap_or_default(),
                            false,
                        ),
                        AgendaRowItem::Task(task) => (
                            "Task".into(),
                            style::color(palette.colors.personal),
                            task.title.clone(),
                            task.notes.clone().unwrap_or_else(|| "Local only".into()),
                            task.done,
                        ),
                    };
                    div()
                        .h(px(AGENDA_ROW_HEIGHT))
                        .w_full()
                        .flex()
                        .items_center()
                        .when(index % 2 == 1, |this| {
                            this.bg(style::color(palette.colors.sunken))
                        })
                        .border_b_1()
                        .border_color(style::color(palette.colors.border_hairline))
                        .child(
                            div()
                                .w(px(AGENDA_DATE_WIDTH))
                                .h_full()
                                .flex_none()
                                .px_4()
                                .flex()
                                .items_center()
                                .child(if *first {
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap_2()
                                        .child(style::text(
                                            date.day.to_string(),
                                            TextRole::AgendaDate,
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .pt_1()
                                                .flex()
                                                .flex_col()
                                                .child(style::text(
                                                    date.to_naive().format("%A").to_string(),
                                                    TextRole::EventTitle,
                                                    cx,
                                                ))
                                                .child(style::text(
                                                    date.to_naive().format("%b").to_string(),
                                                    TextRole::WeekdayLabel,
                                                    cx,
                                                )),
                                        )
                                } else {
                                    div()
                                }),
                        )
                        .child(
                            div()
                                .w(px(108.0))
                                .child(style::text(time, TextRole::EventTime, cx)),
                        )
                        .child(div().size(px(8.0)).rounded(px(4.0)).bg(color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .when(done, |this| this.opacity(0.55).line_through())
                                .child(style::text(title, TextRole::EventTitle, cx)),
                        )
                        .child(div().w(px(180.0)).child(style::text(
                            location,
                            TextRole::EventNotes,
                            cx,
                        )))
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.agenda_scroll)
        .w_full()
        .h_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(78.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .child(style::text("Agenda", TextRole::CalendarTitle, cx))
                    .child(self.agenda_segment(cx))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.step_button("agenda-prev", "‹", -1, cx))
                            .child(self.today_button(cx))
                            .child(self.step_button("agenda-next", "›", 1, cx)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    fn agenda_segment(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .flex()
            .p(px(2.0))
            .rounded(px(palette.radii.control))
            .bg(style::color(palette.colors.sunken))
            .child(self.segment_button("agenda-events", "Events", !self.show_tasks, false, cx))
            .child(self.segment_button(
                "agenda-events-tasks",
                "Events + tasks",
                self.show_tasks,
                true,
                cx,
            ))
            .into_any_element()
    }

    fn segment_button(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        show_tasks: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id(id)
            .h(px(28.0))
            .px_3()
            .flex()
            .items_center()
            .rounded(px(palette.radii.button))
            .cursor_pointer()
            .when(selected, |this| this.bg(style::color(palette.colors.card)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_tasks = show_tasks;
                cx.notify();
            }))
            .child(style::text(label, TextRole::ButtonLabel, cx))
            .into_any_element()
    }

    fn editor_sheet(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.editor.as_ref()?;
        let palette = style::palette(cx);
        let calendar = self.load.as_ref().and_then(|load| {
            load.calendars
                .iter()
                .find(|calendar| calendar.id == editor.calendar_id)
        });
        let calendar_name = calendar
            .map(|calendar| calendar.name.clone())
            .unwrap_or_else(|| "Calendar".into());
        let supports_scheduling = calendar.is_some_and(|calendar| calendar.supports_scheduling);
        let count = self
            .editor_preview_draft(cx)
            .ok()
            .and_then(|draft| occurrence_count(&draft).ok())
            .unwrap_or(0);
        let saving = editor.saving;
        let title = editor.title.clone();
        let date = editor.date.clone();
        let start = editor.start.clone();
        let end = editor.end.clone();
        let location = editor.location.clone();
        let notes = editor.notes.clone();
        let repeat_end = editor.repeat_end.clone();
        let attendee = editor.attendee.clone();
        let frequency = editor.frequency;
        let all_day = editor.all_day;
        let scope = editor.scope;
        let recurring =
            editor.event_id.is_some() && (editor.recurring || frequency != RepeatFrequency::Never);
        let labels = ["S", "M", "T", "W", "T", "F", "S"];
        let repeat_days = editor.repeat_days;
        let reminders = editor.reminders.clone();
        let attendees = editor.attendees.clone();
        let error = editor.error.clone();

        let control = |child: AnyElement| {
            div()
                .h(px(34.0))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .px_2()
                .rounded(px(palette.radii.button))
                .border_1()
                .border_color(style::color(palette.colors.border_soft))
                .bg(style::color(palette.colors.card))
                .child(child)
        };
        let label = |value: &'static str, cx: &mut Context<Self>| {
            div()
                .w(px(92.0))
                .flex_none()
                .child(style::text(value, TextRole::WeekdayLabel, cx))
        };
        let row = |label_el: AnyElement, content: AnyElement| {
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_4()
                .child(label_el)
                .child(content)
        };

        let calendar_row = row(
            label("Calendar", cx).into_any_element(),
            div()
                .id("event-calendar-picker")
                .h(px(34.0))
                .flex_1()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(palette.radii.button))
                .border_1()
                .border_color(style::color(palette.colors.border_soft))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    let Some(editor) = &mut this.editor else {
                        return;
                    };
                    let Some(load) = &this.load else { return };
                    let writable: Vec<_> =
                        load.calendars.iter().filter(|item| item.visible).collect();
                    if let Some(index) = writable
                        .iter()
                        .position(|item| item.id == editor.calendar_id)
                    {
                        editor.calendar_id = writable[(index + 1) % writable.len()].id;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(11.0))
                        .rounded(px(3.0))
                        .bg(calendar_color(editor.calendar_id, palette)),
                )
                .child(style::text(calendar_name, TextRole::ButtonLabel, cx))
                .child(div().flex_1())
                .child(style::text("▾", TextRole::EventTime, cx))
                .into_any_element(),
        );

        let when_row = row(
            label("When", cx).into_any_element(),
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    control(Input::new(&date).into_any_element())
                        .w(px(150.0))
                        .flex_none(),
                )
                .when(!all_day, |this| {
                    this.child(
                        control(Input::new(&start).into_any_element())
                            .w(px(72.0))
                            .flex_none(),
                    )
                    .child(style::text("→", TextRole::EventNotes, cx))
                    .child(
                        control(Input::new(&end).into_any_element())
                            .w(px(72.0))
                            .flex_none(),
                    )
                })
                .child(div().flex_1())
                .child(style::text("All day", TextRole::EventNotes, cx))
                .child(self.editor_toggle(
                    "event-all-day",
                    all_day,
                    |editor| {
                        editor.all_day = !editor.all_day;
                    },
                    cx,
                ))
                .into_any_element(),
        );

        let repeats =
            row(
                label("Repeats", cx).into_any_element(),
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id("event-frequency")
                            .h(px(34.0))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(palette.radii.button))
                            .border_1()
                            .border_color(style::color(palette.colors.border_soft))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(editor) = &mut this.editor {
                                    editor.frequency = editor.frequency.next();
                                    cx.notify();
                                }
                            }))
                            .child(style::text(frequency.label(), TextRole::ButtonLabel, cx))
                            .child(div().flex_1())
                            .child(style::text("▾", TextRole::EventTime, cx)),
                    )
                    .when(frequency == RepeatFrequency::Weekly, |this| {
                        this.child(div().flex().gap_1().children(
                            labels.into_iter().enumerate().map(|(index, value)| {
                                let selected = repeat_days[index];
                                div()
                                    .id(("repeat-day", index))
                                    .w(px(34.0))
                                    .h(px(30.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(palette.radii.button))
                                    .border_1()
                                    .border_color(style::color(if selected {
                                        palette.colors.accent
                                    } else {
                                        palette.colors.border_soft
                                    }))
                                    .when(selected, |this| {
                                        this.bg(style::color(palette.colors.accent))
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(editor) = &mut this.editor {
                                            editor.repeat_days[index] = !editor.repeat_days[index];
                                            cx.notify();
                                        }
                                    }))
                                    .child(style::text(
                                        value,
                                        if selected {
                                            TextRole::CalendarButtonLabelFilled
                                        } else {
                                            TextRole::EventTime
                                        },
                                        cx,
                                    ))
                            }),
                        ))
                    })
                    .when(frequency != RepeatFrequency::Never, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(style::text("Ends", TextRole::EventNotes, cx))
                                .child(
                                    control(Input::new(&repeat_end).into_any_element())
                                        .w(px(130.0))
                                        .flex_none(),
                                )
                                .child(style::text(
                                    format!("{count} occurrences"),
                                    TextRole::EventTime,
                                    cx,
                                )),
                        )
                    })
                    .into_any_element(),
            );

        let reminder_row = row(
            label("Remind me", cx).into_any_element(),
            div()
                .flex_1()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(
                    reminders
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(index, reminder)| {
                            div()
                                .id(("remove-reminder", index))
                                .h(px(30.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded(px(15.0))
                                .bg(style::color(palette.colors.accent_tint))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor
                                        && index < editor.reminders.len()
                                    {
                                        editor.reminders.remove(index);
                                        cx.notify();
                                    }
                                }))
                                .child(style::text(
                                    reminder_label(&reminder),
                                    TextRole::EventTime,
                                    cx,
                                ))
                                .child("×")
                        }),
                )
                .child(
                    div()
                        .id("add-reminder")
                        .size(px(30.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(15.0))
                        .border_1()
                        .border_dashed()
                        .border_color(style::color(palette.colors.border_strong))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(editor) = &mut this.editor {
                                let presets = [
                                    ReminderKind::MinutesBefore(10),
                                    ReminderKind::MinutesBefore(30),
                                    ReminderKind::SameDayMinute(8 * 60),
                                ];
                                if let Some(next) = presets
                                    .into_iter()
                                    .find(|item| !editor.reminders.contains(item))
                                {
                                    editor.reminders.push(next);
                                }
                                cx.notify();
                            }
                        }))
                        .child("+"),
                )
                .into_any_element(),
        );

        let attendee_row =
            row(
                label("Attendees", cx).into_any_element(),
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(!attendees.is_empty(), |this| {
                        this.children(attendees.iter().enumerate().map(|(index, item)| {
                            let mut attendee_row = div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(style::text(
                                    item.display_name
                                        .clone()
                                        .unwrap_or_else(|| item.email.clone()),
                                    TextRole::EventNotes,
                                    cx,
                                ))
                                .child(style::text(item.status.clone(), TextRole::EventTime, cx))
                                .child(div().flex_1());
                            if item.is_self && supports_scheduling {
                                for (choice, (status, text)) in [
                                    ("accepted", "Accept"),
                                    ("tentative", "Maybe"),
                                    ("declined", "Decline"),
                                ]
                                .into_iter()
                                .enumerate()
                                {
                                    attendee_row = attendee_row.child(
                                        div()
                                            .id(("rsvp", index * 10 + choice))
                                            .h(px(26.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .rounded(px(palette.radii.button))
                                            .border_1()
                                            .border_color(style::color(palette.colors.border_soft))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(editor) = &mut this.editor
                                                    && let Some(attendee) =
                                                        editor.attendees.get_mut(index)
                                                {
                                                    attendee.status = status.into();
                                                    cx.notify();
                                                }
                                            }))
                                            .child(style::text(text, TextRole::ButtonLabel, cx)),
                                    );
                                }
                            }
                            attendee_row.child(
                                div()
                                    .id(("remove-attendee", index))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(editor) = &mut this.editor
                                            && index < editor.attendees.len()
                                        {
                                            editor.attendees.remove(index);
                                            cx.notify();
                                        }
                                    }))
                                    .child("×"),
                            )
                        }))
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(control(Input::new(&attendee).into_any_element()))
                            .child(
                                div()
                                    .id("add-attendee")
                                    .h(px(34.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded(px(palette.radii.button))
                                    .border_1()
                                    .border_color(style::color(palette.colors.border_soft))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_attendee(window, cx)
                                    }))
                                    .child(style::text("Add", TextRole::ButtonLabel, cx)),
                            ),
                    )
                    .into_any_element(),
            );

        let location_row = row(
            label("Where", cx).into_any_element(),
            control(Input::new(&location).into_any_element()).into_any_element(),
        );
        let notes_row = row(
            label("Notes", cx).into_any_element(),
            div()
                .h(px(72.0))
                .flex_1()
                .min_w_0()
                .rounded(px(palette.radii.button))
                .border_1()
                .border_color(style::color(palette.colors.border_soft))
                .bg(style::color(palette.colors.card))
                .child(
                    Textarea::new(&notes)
                        .size_full()
                        .font_family("Newsreader")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::NORMAL)
                        .line_height(px(25.5)),
                )
                .into_any_element(),
        );

        let scope_row = recurring.then(|| {
            row(
                label("Apply to", cx).into_any_element(),
                div()
                    .flex_1()
                    .flex()
                    .gap_1()
                    .children(
                        [
                            (EventEditScope::This, "This"),
                            (EventEditScope::ThisAndFuture, "This + future"),
                            (EventEditScope::All, "All"),
                        ]
                        .into_iter()
                        .map(|(value, text)| {
                            let selected = scope == value;
                            div()
                                .id(("event-scope", value as usize))
                                .h(px(30.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(palette.radii.button))
                                .cursor_pointer()
                                .when(selected, |this| {
                                    this.bg(style::color(palette.colors.accent_tint))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor {
                                        editor.scope = value;
                                        cx.notify();
                                    }
                                }))
                                .child(style::text(text, TextRole::ButtonLabel, cx))
                        }),
                    )
                    .into_any_element(),
            )
        });

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(hsla(0.0, 0.0, 0.0, 0.34))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(44.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("event-editor-sheet")
                        .w(px(576.0))
                        .max_h(relative(0.94))
                        .overflow_y_scroll()
                        .rounded(px(14.0))
                        .shadow_2xl()
                        .bg(style::color(palette.colors.card))
                        .child(
                            div()
                                .px(px(26.0))
                                .pt(px(22.0))
                                .pb(px(18.0))
                                .border_b_1()
                                .border_color(style::color(palette.colors.border_soft))
                                .child(style::text(
                                    if editor.event_id.is_some() {
                                        "Edit event"
                                    } else {
                                        "New event"
                                    },
                                    TextRole::WeekdayLabel,
                                    cx,
                                ))
                                .child(
                                    div()
                                        .mt_3()
                                        .pb_2()
                                        .border_b_2()
                                        .border_color(style::color(palette.colors.accent))
                                        .child(
                                            Input::new(&title)
                                                .font_family("Instrument Sans")
                                                .text_size(px(26.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .line_height(px(31.0)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .px(px(26.0))
                                .py_4()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(calendar_row)
                                .child(when_row)
                                .child(repeats)
                                .child(reminder_row)
                                .child(attendee_row)
                                .child(location_row)
                                .child(notes_row)
                                .when_some(scope_row, |this, row| this.child(row))
                                .when_some(error, |this, error| {
                                    this.child(
                                        div()
                                            .text_color(style::color(palette.colors.danger))
                                            .child(style::text(error, TextRole::EventNotes, cx)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .px(px(26.0))
                                .py_4()
                                .flex()
                                .items_center()
                                .gap_2()
                                .border_t_1()
                                .border_color(style::color(palette.colors.border_soft))
                                .bg(style::color(palette.colors.chrome))
                                .when(editor.event_id.is_some(), |this| {
                                    this.child(
                                        div()
                                            .id("event-delete")
                                            .cursor_pointer()
                                            .text_color(style::color(palette.colors.danger))
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.delete_editor(cx)
                                                }),
                                            )
                                            .child(style::text(
                                                "Delete",
                                                TextRole::ButtonLabel,
                                                cx,
                                            )),
                                    )
                                })
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id("event-cancel")
                                        .h(px(34.0))
                                        .px_4()
                                        .flex()
                                        .items_center()
                                        .rounded(px(palette.radii.button))
                                        .border_1()
                                        .border_color(style::color(palette.colors.border_soft))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.editor = None;
                                            cx.notify();
                                        }))
                                        .child(style::text("Cancel", TextRole::ButtonLabel, cx)),
                                )
                                .child(
                                    div()
                                        .id("event-save")
                                        .h(px(34.0))
                                        .px_4()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(px(palette.radii.button))
                                        .bg(style::color(palette.colors.accent))
                                        .cursor_pointer()
                                        .when(!saving, |this| {
                                            this.on_click(
                                                cx.listener(|this, _, _, cx| this.save_editor(cx)),
                                            )
                                        })
                                        .child(style::text(
                                            if saving { "Saving…" } else { "Save event" },
                                            TextRole::CalendarButtonLabelFilled,
                                            cx,
                                        ))
                                        .child(style::text(
                                            "⌘↵",
                                            TextRole::CalendarButtonLabelFilled,
                                            cx,
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn editor_toggle(
        &self,
        id: &'static str,
        on: bool,
        change: impl Fn(&mut EventEditor) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id(id)
            .w(px(32.0))
            .h(px(19.0))
            .p(px(2.0))
            .flex()
            .justify_end()
            .when(!on, |this| this.justify_start())
            .rounded(px(10.0))
            .bg(style::color(if on {
                palette.colors.accent
            } else {
                palette.colors.border_soft
            }))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(editor) = &mut this.editor {
                    change(editor);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size(px(15.0))
                    .rounded(px(8.0))
                    .bg(style::color(palette.colors.card)),
            )
            .into_any_element()
    }

    fn occurrences(&self) -> &[CalendarOccurrence] {
        self.load
            .as_ref()
            .map(|load| load.occurrences.as_slice())
            .unwrap_or(&[])
    }
}

impl Render for CalendarWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_minute_tick(window, cx);
        let palette = style::palette(cx);
        let main = match self.view {
            CalendarView::Month => self.month_view(cx),
            CalendarView::Week => self.time_grid_view(7, GridPitch::Week, cx),
            CalendarView::ThreeDay => self.time_grid_view(3, GridPitch::ThreeDay, cx),
            CalendarView::Day => self.time_grid_view(1, GridPitch::Day, cx),
            CalendarView::Agenda => self.agenda_view(cx),
        };
        let editor = self.editor_sheet(cx);
        div()
            .relative()
            .size_full()
            .flex()
            .bg(style::color(palette.colors.canvas))
            .key_context(if self.editor.is_some() {
                "Calendar Editor"
            } else {
                "Calendar"
            })
            .track_focus(&self.focus)
            .child(self.sidebar(cx))
            .child(div().flex_1().min_w_0().h_full().child(main))
            .when_some(editor, |this, editor| this.child(editor))
    }
}

fn local_now(timezone: &str) -> (CivilDate, u32) {
    let zone: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now = Utc::now().with_timezone(&zone);
    (
        CivilDate::new(now.year(), now.month(), now.day()).unwrap(),
        now.hour() * 60 + now.minute(),
    )
}

fn end_minute(event: &CalendarOccurrence) -> u16 {
    if event.rendered.local_end_date == event.rendered.local_date {
        event
            .rendered
            .local_end_minute
            .max(event.rendered.local_start_minute + 1)
    } else {
        24 * 60
    }
}

fn build_rrule(
    editor: &EventEditor,
    start_date: NaiveDate,
    cx: &App,
) -> Result<Option<String>, String> {
    if editor.frequency == RepeatFrequency::Never {
        return Ok(None);
    }
    let frequency = match editor.frequency {
        RepeatFrequency::Never => unreachable!(),
        RepeatFrequency::Daily => "DAILY",
        RepeatFrequency::Weekly => "WEEKLY",
        RepeatFrequency::Monthly => "MONTHLY",
        RepeatFrequency::Yearly => "YEARLY",
    };
    let mut parts = vec![format!("FREQ={frequency}")];
    if editor.frequency == RepeatFrequency::Weekly {
        let names = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
        let selected: Vec<_> = names
            .into_iter()
            .zip(editor.repeat_days)
            .filter_map(|(name, selected)| selected.then_some(name))
            .collect();
        if selected.is_empty() {
            return Err("Choose at least one repeat day.".into());
        }
        parts.push(format!("BYDAY={}", selected.join(",")));
    }
    let end = NaiveDate::parse_from_str(editor.repeat_end.read(cx).value().trim(), "%Y-%m-%d")
        .map_err(|_| "Use YYYY-MM-DD for the repeat end date.".to_string())?;
    if end < start_date {
        return Err("The repeat end must be on or after the event date.".into());
    }
    parts.push(format!("UNTIL={}T235959Z", end.format("%Y%m%d")));
    Ok(Some(parts.join(";")))
}

fn parse_repeat(rule: Option<&str>, start_day: chrono::Weekday) -> (RepeatFrequency, [bool; 7]) {
    let mut days = [false; 7];
    days[start_day.num_days_from_sunday() as usize] = true;
    let Some(rule) = rule else {
        return (RepeatFrequency::Never, days);
    };
    let mut frequency = RepeatFrequency::Never;
    for part in rule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("FREQ") {
            frequency = match value.to_ascii_uppercase().as_str() {
                "DAILY" => RepeatFrequency::Daily,
                "WEEKLY" => RepeatFrequency::Weekly,
                "MONTHLY" => RepeatFrequency::Monthly,
                "YEARLY" => RepeatFrequency::Yearly,
                _ => RepeatFrequency::Never,
            };
        } else if key.eq_ignore_ascii_case("BYDAY") {
            days = [false; 7];
            for value in value.split(',') {
                if let Some(index) = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"]
                    .iter()
                    .position(|item| value.ends_with(item))
                {
                    days[index] = true;
                }
            }
        }
    }
    (frequency, days)
}

fn rrule_until(rule: Option<&str>) -> Option<String> {
    let value = rule?
        .split(';')
        .find_map(|part| {
            part.split_once('=')
                .filter(|(key, _)| key.eq_ignore_ascii_case("UNTIL"))
        })?
        .1;
    let value = value.trim_end_matches('Z');
    let prefix = value.get(..8)?;
    NaiveDate::parse_from_str(prefix, "%Y%m%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn occurrence_count(draft: &EventDraft) -> anyhow::Result<usize> {
    let end = draft
        .rrule
        .as_deref()
        .and_then(|rule| rrule_until(Some(rule)))
        .and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok())
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| date.and_utc().timestamp())
        .unwrap_or_else(|| draft.start_utc.saturating_add(366 * 24 * 60 * 60));
    let event = CalendarEvent {
        remote_id: "editor-preview".into(),
        start_utc: Some(draft.start_utc),
        end_utc: Some(draft.end_utc),
        all_day: draft.all_day,
        tz: Some(draft.timezone.clone()),
        rrule: draft.rrule.clone(),
        ..Default::default()
    };
    Ok(expand_event_between(&event, draft.start_utc, end)?.len())
}

fn reminder_label(reminder: &ReminderKind) -> String {
    match reminder {
        ReminderKind::MinutesBefore(minutes) => format!("{minutes} min before"),
        ReminderKind::SameDayMinute(minute) => {
            format!("at {:02}:{:02} same day", minute / 60, minute % 60)
        }
        ReminderKind::Absolute(timestamp) => DateTime::<Utc>::from_timestamp(*timestamp, 0)
            .map(|date| format!("at {}", date.format("%Y-%m-%d %H:%MZ")))
            .unwrap_or_else(|| "at a fixed time".into()),
    }
}

fn date_key(date: CivilDate) -> u64 {
    ((date.year as i64 + 10_000) as u64) * 10_000 + date.month as u64 * 100 + date.day as u64
}

fn calendar_color(id: i64, palette: &snail_ui::theme::Theme) -> Hsla {
    style::color(match id.rem_euclid(5) {
        0 => palette.colors.classes,
        1 => palette.colors.personal,
        2 => palette.colors.work,
        3 => palette.colors.birthdays,
        _ => palette.colors.home,
    })
}

fn calendar_tint(id: i64, palette: &snail_ui::theme::Theme) -> Hsla {
    style::color(match id.rem_euclid(5) {
        0 => palette.colors.classes_tint,
        1 => palette.colors.personal_tint,
        2 => palette.colors.work_tint,
        3 => palette.colors.birthdays_tint,
        _ => palette.colors.home_tint,
    })
}
