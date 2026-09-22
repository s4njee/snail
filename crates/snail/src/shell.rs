//! The shell: titlebar, the unified sidebar, and the mail three-pane frame over the real store
//! (plan.md E1.8/E5). First cut: the store is read on the main thread at construction and on
//! mailbox changes (cheap local queries); the background-executor move and the paint budget are
//! E5.12's job.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{
    Escape as InputEscape, Input, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::pgp::VerificationState;
use snail_ui::calendar::CalendarView;
use snail_ui::commands::{CommandId, EnableGuard, ShortcutPlatform, command, display_binding};
use snail_ui::empty::EmptyState;
use snail_ui::palette::{NamedTarget, PaletteCandidate, PaletteTarget, candidates};
use snail_ui::selection::Selection;
use snail_ui::text::TextRole;

use crate::calendar_model::{CalendarModel, RsvpDelivery};
use crate::calendar_view::CalendarWorkspace;
use crate::commands::RunCommand;
use crate::dev_overlay::DevOverlay;
use crate::icons::{Icon, icon};
use crate::mail_model::{MailModel, MailboxRow, MessageRow, SearchHit};
use crate::onboarding::Onboarding;
use crate::settings;
use crate::settings_model::SettingsModel;
use crate::settings_view::{SettingsEvent, SettingsWorkspace};
use crate::style::{self, ThemePref};
use crate::sync_model::{OpenMessage, SyncChanged, SyncHub, SyncStatus};

const PAGE: u32 = 200;

struct Reading {
    subject: String,
    from: String,
    sender_addr: Option<String>,
    meta: String,
    crypto: VerificationState,
    invite: Option<snail_core::calendar::CalendarEvent>,
    body: BodyKind,
}

enum BodyKind {
    Plain {
        visible: String,
        quoted: Option<String>,
        expanded: bool,
    },
    Html {
        document: snail_core::html::SanitizedDocument,
        view: crate::html_view::HtmlView,
        images: crate::html_view::Images,
        quotes_collapsed: bool,
        has_quotes: bool,
    },
}

/// A remote image fetched on the background executor, waiting to be turned into a gpui image.
struct RemoteImage {
    generation: u64,
    token: String,
    width: f32,
    height: f32,
    bgra: Vec<u8>,
}

/// A transient undo affordance after a triage action (E8.5), covering a whole batch (E8.7).
struct UndoBar {
    ops: Vec<i64>,
    label: String,
    at: Instant,
}

/// The "move to mailbox" picker (E8.4): the candidate mailboxes and the highlighted one.
struct MovePicker {
    mailboxes: Vec<(i64, String)>,
    selected: usize,
}

/// Search state (E9.3): the results and whether the background query is still running.
struct SearchResults {
    hits: Vec<SearchHit>,
    running: bool,
    /// The parsed query, for the header line (e.g. `unread · from:alice`).
    describe: String,
}

/// One row of the grouped search list (E9.4): a mailbox header or a hit. Both render at the same
/// height so a single `uniform_list` can hold them.
enum SearchListRow {
    Header { name: String, count: usize },
    Hit(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceMode {
    Mail,
    Calendar,
    Settings,
}

pub struct Shell {
    focus: FocusHandle,
    list_focus: FocusHandle,
    reading_focus: FocusHandle,
    _activation: Subscription,
    _focus_lost: Subscription,
    overlay: DevOverlay,
    _appearance: Subscription,
    _search_sub: Subscription,
    mail: MailModel,
    calendar_model: CalendarModel,
    calendar: Entity<CalendarWorkspace>,
    settings_workspace: Entity<SettingsWorkspace>,
    _settings_sub: Subscription,
    workspace: WorkspaceMode,
    mailboxes: Vec<MailboxRow>,
    selected_mailbox: usize,
    rows: Arc<Vec<MessageRow>>,
    selection: Selection,
    reading: Option<Reading>,
    /// Remote images are blocked by default (E6.2); F4 or `SNAIL_REMOTE_IMAGES=1` unblocks.
    remote_enabled: bool,
    pending_remote: Vec<RemoteImage>,
    generation: u64,
    reading_scroll: ScrollHandle,
    /// The inline reply box (E7.5).
    inline_reply: Entity<TextareaState>,
    inline_open: bool,
    /// The transient undo affordance after a triage action (E8.5).
    undo: Option<UndoBar>,
    /// "Group messages by thread" (E8.3).
    group_threads: bool,
    /// The open "move to mailbox" picker, if any (E8.4).
    move_picker: Option<MovePicker>,
    /// The search box and its state (E9.3/E9.6). `search` is `Some` exactly when the box is
    /// non-empty, and the list then shows results regardless of the selected mailbox (E9.6).
    search_input: Entity<InputState>,
    search_query: String,
    search: Option<SearchResults>,
    search_generation: u64,
    palette_input: Entity<InputState>,
    palette_query: String,
    palette_results: Vec<PaletteCandidate>,
    palette_selected: usize,
    palette_open: bool,
    _palette_sub: Subscription,
    app_menu_open: bool,
    /// The message whose reading is loaded, so render can reload it when the cursor moves without a
    /// handler (a search result arriving on the background executor sets the cursor).
    reading_id: Option<i64>,
    /// Set when the user clicks "Load images" for the current message (E6.2), independent of the
    /// global switch and the per-sender allowance.
    forced_message: Option<i64>,
    /// The width HTML is laid out at: the reading pane's content width as last measured, via
    /// `snail_ui::layout::document_width`. `None` until the pane has been laid out once.
    html_width: Option<f32>,
    /// The pending debounced relayout. Replacing it drops, and so cancels, the previous one, which
    /// is what keeps a window-edge drag from re-running layout on every frame.
    html_relayout: Option<Task<()>>,
    /// Set when the debounce fires; the relayout itself runs in render, where the window's text
    /// system is available (the same pattern as `pending_remote`).
    html_relayout_due: bool,
    /// Accounts and background sync (E4, E16). The shell re-reads the list when it reports.
    sync: Entity<SyncHub>,
    _sync_sub: Subscription,
    _open_sub: Subscription,
    /// The connect-an-account flow, alive only while it is on screen (E14.3).
    onboarding: Option<Entity<Onboarding>>,
    /// Marks the open message read after a short dwell (the handoff's ~1s); replacing it cancels.
    mark_read: Option<Task<()>>,
}

/// How long a message must stay open before it counts as read.
const READ_DWELL: Duration = Duration::from_millis(1000);

/// How long the reading pane's width must hold still before HTML is laid out again. Long enough
/// to span the frames of a window-edge drag, short enough to feel like a response to letting go.
const HTML_RELAYOUT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

impl Shell {
    pub fn new(
        mail: MailModel,
        calendar_model: CalendarModel,
        settings_model: SettingsModel,
        sync: Entity<SyncHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let list_focus = cx.focus_handle();
        let reading_focus = cx.focus_handle();
        window.focus(&focus, cx);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && window.focused(cx).is_none() {
                window.focus(&this.focus, cx);
            }
        });
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            // A dialog or input with focus wins. Restore the shell only when the focus tree is
            // genuinely empty, such as after closing a transient view or reactivating the window.
            if window.is_window_active() && window.focused(cx).is_none() {
                window.focus(&this.focus, cx);
            }
        });
        let appearance = cx.observe_window_appearance(window, |_this, _window, cx| {
            if settings::pref(cx) == ThemePref::System {
                style::apply(ThemePref::System, cx);
                cx.notify();
            }
        });
        let inline_reply =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Write a reply…"));
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search mail")
                .clean_on_escape()
        });
        let search_sub = cx.subscribe(&search_input, |this, _state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_search_changed(cx);
            }
        });
        let palette_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type a command, mailbox, calendar or date…")
                .clean_on_escape()
        });
        let palette_sub = cx.subscribe(&palette_input, |this, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.palette_query = state.read(cx).value().to_string();
                this.rebuild_palette();
                this.palette_selected = 0;
                cx.notify();
            }
        });
        let open_sub = cx.subscribe_in(
            &sync,
            window,
            |this, _hub, open: &OpenMessage, window, cx| {
                this.open_message(open.0, window, cx);
            },
        );
        let sync_sub = cx.subscribe_in(&sync, window, |this, _hub, _: &SyncChanged, window, cx| {
            this.sync_refresh(window, cx);
            cx.notify();
        });
        let calendar = cx.new(|cx| CalendarWorkspace::new(calendar_model.clone(), cx));
        let settings_workspace =
            cx.new(|cx| SettingsWorkspace::new(settings_model, sync.clone(), window, cx));
        let settings_sub = cx.subscribe_in(
            &settings_workspace,
            window,
            |this, _settings, event: &SettingsEvent, _window, cx| match event {
                SettingsEvent::AddGoogle => {
                    this.workspace = WorkspaceMode::Mail;
                    this.sync.update(cx, |hub, cx| hub.begin_reconnect(cx));
                    cx.notify();
                }
                SettingsEvent::OpenPgpManager => crate::pgp_keys::open(this.mail.clone(), cx),
            },
        );
        let general = settings::general(cx);
        let mut shell = Self {
            focus,
            list_focus,
            reading_focus,
            _activation: activation,
            _focus_lost: focus_lost,
            overlay: DevOverlay::new(),
            _appearance: appearance,
            _search_sub: search_sub,
            mail,
            calendar_model,
            calendar,
            settings_workspace,
            _settings_sub: settings_sub,
            workspace: WorkspaceMode::Mail,
            mailboxes: Vec::new(),
            selected_mailbox: 0,
            rows: Arc::new(Vec::new()),
            selection: Selection::new(),
            reading: None,
            remote_enabled: general.load_remote_images
                || std::env::var_os("SNAIL_REMOTE_IMAGES").is_some(),
            pending_remote: Vec::new(),
            generation: 0,
            reading_scroll: ScrollHandle::new(),
            inline_reply,
            inline_open: false,
            undo: None,
            group_threads: general.group_by_thread,
            move_picker: None,
            search_input,
            search_query: String::new(),
            search: None,
            search_generation: 0,
            palette_input,
            palette_query: String::new(),
            palette_results: Vec::new(),
            palette_selected: 0,
            palette_open: false,
            _palette_sub: palette_sub,
            app_menu_open: false,
            reading_id: None,
            forced_message: None,
            html_width: None,
            html_relayout: None,
            html_relayout_due: false,
            sync,
            _sync_sub: sync_sub,
            _open_sub: open_sub,
            onboarding: None,
            mark_read: None,
        };
        shell.reload(window, cx);
        shell.rebuild_palette();
        // Harvest contacts once on the background executor so autocomplete has data (E7.3).
        {
            let mail = shell.mail.clone();
            cx.background_executor()
                .spawn(async move {
                    mail.harvest_contacts();
                })
                .detach();
        }
        // Previews stored with CSS in them are re-derived once; show the fixed ones straight away.
        {
            let mail = shell.mail.clone();
            let repair = cx
                .background_executor()
                .spawn(async move { mail.repair_previews() });
            cx.spawn(async move |this, cx| {
                let repaired = repair.await;
                if repaired > 0 {
                    log::info!("repaired {repaired} message preview(s)");
                    this.update(cx, |this, cx| {
                        this.refresh_rows(cx);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        shell
    }

    /// Read mailboxes and the selected mailbox's page. Local and fast; E5.12 moves it off-thread.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh_rows(cx) {
            self.load_reading(window, cx);
        }
    }

    /// Sync changed the store: refresh the list, but leave the open message alone — re-laying it
    /// out would jump its scroll back to the top every time mail arrived. It is reloaded only if
    /// it is gone, or the cursor moved because it was.
    fn sync_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.reading_id;
        if !self.refresh_rows(cx) {
            return;
        }
        let still_there = open.is_some_and(|id| self.mail.message(id).is_some());
        if self.selection.cursor() != open || !still_there || self.reading.is_none() {
            self.load_reading(window, cx);
        }
    }

    /// Re-read mailboxes and rows and reconcile the selection. False when a search owns the list
    /// (the search is re-run instead), so there is no reading to reload.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) -> bool {
        // If the provider rejected a triage op, put the row back and say so (E8.4).
        let rolled_back = self.mail.rollback_failed_triage();
        if rolled_back > 0 {
            self.undo = Some(UndoBar {
                ops: Vec::new(),
                label: format!("{rolled_back} change(s) couldn't be applied and were rolled back"),
                at: Instant::now(),
            });
        }
        self.mailboxes = self.mail.mailboxes();
        // The Dock badge is the Inbox's unread count, across accounts (E16.8).
        crate::dock::set_unread(
            self.mailboxes
                .iter()
                .filter(|mailbox| mailbox.kind == "inbox")
                .map(|mailbox| mailbox.unread)
                .sum(),
        );
        if self.selected_mailbox >= self.mailboxes.len() {
            self.selected_mailbox = 0;
        }
        let rows = match self.mailboxes.get(self.selected_mailbox) {
            // Group by thread: one row per thread, its newest message (E8.3).
            Some(mailbox) if self.group_threads => self
                .mail
                .thread_page(mailbox.id, PAGE)
                .into_iter()
                .filter_map(|thread| self.mail.message(thread.newest_id))
                .collect(),
            Some(mailbox) => self.mail.page(mailbox.id, PAGE),
            None => Vec::new(),
        };
        self.rows = Arc::new(rows);
        log::info!(
            "shell: {} mailboxes; {} messages in mailbox #{}",
            self.mailboxes.len(),
            self.rows.len(),
            self.selected_mailbox
        );
        if self.search.is_some() {
            // The list is showing results, so refresh them instead of the mailbox rows (E9.6): a
            // triage that emptied a result must disappear from the results.
            self.run_search(cx);
            return false;
        }
        let ids: Vec<i64> = self.rows.iter().map(|row| row.id).collect();
        self.selection.reconcile(&ids);
        if self.selection.cursor().is_none() {
            if let Some(first) = ids.first() {
                self.selection.select_in(&ids, *first);
            }
        }
        true
    }

    fn select_mailbox(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_mailbox = index;
        self.selection = Selection::new();
        self.reload(window, cx);
    }

    /// Select a message that may not be a row of the list — a collapsed row of the thread view
    /// (E8.2) — and open it.
    fn select_message(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.select(id);
        self.load_reading(window, cx);
        cx.notify();
    }

    fn rebuild_palette(&mut self) {
        let mailboxes = self
            .mailboxes
            .iter()
            .map(|mailbox| NamedTarget {
                id: mailbox.id,
                title: mailbox.name.clone(),
            })
            .collect::<Vec<_>>();
        let calendars = self
            .calendar_model
            .calendars()
            .into_iter()
            .map(|calendar| NamedTarget {
                id: calendar.id,
                title: calendar.name,
            })
            .collect::<Vec<_>>();
        let today = snail_ui::calendar::CivilDate::from_naive(chrono::Local::now().date_naive());
        self.palette_results = candidates(&self.palette_query, &mailboxes, &calendars, today);
        if self.palette_selected >= self.palette_results.len() {
            self.palette_selected = self.palette_results.len().saturating_sub(1);
        }
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.app_menu_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.rebuild_palette();
        let input = self.palette_input.clone();
        input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn step_palette(&mut self, direction: i32, cx: &mut Context<Self>) {
        if self.palette_results.is_empty() {
            return;
        }
        let last = self.palette_results.len() - 1;
        self.palette_selected = if direction < 0 {
            self.palette_selected.saturating_sub(1)
        } else {
            (self.palette_selected + 1).min(last)
        };
        cx.notify();
    }

    fn activate_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.palette_results.get(self.palette_selected).cloned() else {
            return;
        };
        self.palette_open = false;
        match candidate.target {
            PaletteTarget::Command(CommandId::OpenPalette) => {
                window.focus(&self.focus, cx);
            }
            PaletteTarget::Command(id) => {
                window.focus(&self.focus, cx);
                self.execute_command(id, window, cx);
            }
            PaletteTarget::Mailbox(id) => {
                self.workspace = WorkspaceMode::Mail;
                if let Some(index) = self.mailboxes.iter().position(|mailbox| mailbox.id == id) {
                    self.select_mailbox(index, window, cx);
                }
                window.focus(&self.list_focus, cx);
            }
            PaletteTarget::Calendar(id) => {
                self.workspace = WorkspaceMode::Calendar;
                self.calendar
                    .update(cx, |calendar, cx| calendar.show_calendar(id, cx));
                window.focus(&self.focus, cx);
            }
            PaletteTarget::Date(date) => {
                self.workspace = WorkspaceMode::Calendar;
                self.calendar
                    .update(cx, |calendar, cx| calendar.go_to_date(date, cx));
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    fn command_enabled(&self, id: CommandId, cx: &App) -> bool {
        match command(id).enable {
            EnableGuard::Always => true,
            EnableGuard::HasMessages => !self.rows.is_empty(),
            EnableGuard::HasSelection => self.selection.cursor().is_some(),
            EnableGuard::HasReading => self.reading.is_some(),
            EnableGuard::ComposeOpen => false,
            EnableGuard::EventEditorOpen => self.calendar.read(cx).has_editor(),
        }
    }

    fn execute_command(
        &mut self,
        id: CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.palette_open {
            match id {
                CommandId::PalettePrevious => self.step_palette(-1, cx),
                CommandId::PaletteNext => self.step_palette(1, cx),
                CommandId::PaletteConfirm => self.activate_palette(window, cx),
                CommandId::Cancel => self.close_palette(window, cx),
                CommandId::OpenPalette => self.close_palette(window, cx),
                _ => return false,
            }
            return true;
        }
        if self.move_picker.is_some() {
            match id {
                CommandId::PreviousMessage => self.move_picker_step(-1),
                CommandId::NextMessage => self.move_picker_step(1),
                CommandId::ConfirmSelection => self.commit_move(window, cx),
                CommandId::Cancel => self.move_picker = None,
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if !self.command_enabled(id, cx) {
            return false;
        }

        let ids: Vec<i64> = match &self.search {
            Some(search) => search.hits.iter().map(|hit| hit.id).collect(),
            None => self.rows.iter().map(|row| row.id).collect(),
        };
        match id {
            CommandId::PreviousMessage => {
                self.selection.move_by(-1, &ids);
                self.load_reading(window, cx);
            }
            CommandId::NextMessage => {
                self.selection.move_by(1, &ids);
                self.load_reading(window, cx);
            }
            CommandId::ConfirmSelection => return false,
            CommandId::Compose => crate::compose::open(self.mail.clone(), Default::default(), cx),
            CommandId::Reply => self.compose_reply(false, cx),
            CommandId::ReplyAll => self.compose_reply(true, cx),
            CommandId::Forward => self.compose_forward(cx),
            CommandId::Trash => self.triage_selection(
                snail_core::triage::TriageAction::Trash,
                "Trashed",
                window,
                cx,
            ),
            CommandId::Archive => self.triage_selection(
                snail_core::triage::TriageAction::Archive,
                "Archived",
                window,
                cx,
            ),
            CommandId::ArchiveConversation => self.archive_thread(window, cx),
            CommandId::ToggleRead => self.toggle_read(window, cx),
            CommandId::MoveToMailbox => self.open_move_picker(cx),
            CommandId::ToggleThreadGrouping => {
                self.group_threads = !self.group_threads;
                self.reload(window, cx);
            }
            CommandId::ToggleRemoteImages => {
                self.remote_enabled = !self.remote_enabled;
                self.load_reading(window, cx);
            }
            CommandId::ToggleBodyPreference => {
                settings::toggle_body_preference(cx);
                self.load_reading(window, cx);
            }
            CommandId::FocusSearch => {
                self.workspace = WorkspaceMode::Mail;
                self.focus_search(window, cx);
            }
            CommandId::OpenPalette => self.open_palette(window, cx),
            CommandId::SaveEvent => self
                .calendar
                .update(cx, |calendar, cx| calendar.save_open_editor(cx)),
            CommandId::Cancel => {
                if self.calendar.read(cx).has_editor() {
                    self.calendar
                        .update(cx, |calendar, cx| calendar.cancel_open_editor(cx));
                } else if self.search.is_some() {
                    self.clear_search(window, cx);
                } else {
                    return false;
                }
            }
            CommandId::CycleAppearance => {
                let next = match settings::pref(cx) {
                    ThemePref::System => ThemePref::Light,
                    ThemePref::Light => ThemePref::Dark,
                    ThemePref::Dark => ThemePref::System,
                };
                settings::set(next, cx);
            }
            CommandId::ToggleDiagnostics => self.overlay.toggle(),
            CommandId::ShowMail => self.workspace = WorkspaceMode::Mail,
            CommandId::ShowCalendar => self.workspace = WorkspaceMode::Calendar,
            CommandId::ShowSettings => self.workspace = WorkspaceMode::Settings,
            CommandId::Quit => cx.quit(),
            CommandId::Send
            | CommandId::PalettePrevious
            | CommandId::PaletteNext
            | CommandId::PaletteConfirm => return false,
        }
        cx.notify();
        true
    }

    fn load_reading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.pending_remote.clear();
        self.reading_scroll.set_offset(point(px(0.0), px(0.0)));
        self.reading = self
            .selection
            .cursor()
            .and_then(|id| self.read_message(id, window, cx));
        self.reading_id = self.selection.cursor();
        self.schedule_mark_read(cx);
        self.schedule_pgp_analysis(cx);

        // If a message has remote images and the user has unblocked them, fetch on the background
        // executor; the result lands in `pending_remote` and is applied on the next render (E6.8).
        let sender_allowed = self
            .reading
            .as_ref()
            .and_then(|reading| reading.sender_addr.as_deref())
            .is_some_and(|sender| settings::always_load_images_from(sender, cx));
        if !self.remote_enabled && !sender_allowed && self.forced_message != self.selection.cursor()
        {
            return;
        }
        let urls = match &self.reading {
            Some(Reading {
                body: BodyKind::Html {
                    document, images, ..
                },
                ..
            }) => document
                .remote_resources
                .iter()
                .map(|resource| (resource.token.clone(), resource.url.clone()))
                .into_iter()
                .filter(|(token, _)| !images.contains(token))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        if !urls.is_empty() {
            log::info!("html: fetching {} remote image(s)", urls.len());
            self.fetch_remote(urls, cx);
        }
    }

    /// PGP parsing, certificate policy and decryption are always off the UI thread. The task
    /// returns display-only memory; it never writes decrypted bytes through `MailModel`'s store.
    fn schedule_pgp_analysis(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.reading_id else { return };
        let Some(raw) = self.mail.raw(id) else { return };
        let mail = self.mail.clone();
        let task = cx.background_executor().spawn(async move {
            let result = mail.analyze_pgp(&raw);
            let display_plain = result.plaintext.as_deref().and_then(|plaintext| {
                snail_core::mime::parse_raw(plaintext)
                    .ok()
                    .and_then(|parsed| parsed.plain)
                    .or_else(|| String::from_utf8(plaintext.to_vec()).ok())
            });
            (result.state, display_plain)
        });
        cx.spawn(async move |this, cx| {
            let (state, display_plain) = task.await;
            this.update(cx, |this, cx| {
                if this.reading_id != Some(id) {
                    return;
                }
                if let Some(reading) = &mut this.reading {
                    reading.crypto = state;
                    if let Some(plain) = display_plain {
                        reading.body = plain_body(plain);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show one message, wherever it is filed: a clicked notification (E16.8). Leaves any search,
    /// switches to the message's mailbox, selects it, and brings the window forward.
    fn open_message(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            self.clear_search(window, cx);
        }
        if let Some(mailbox) = self.mail.mailbox_of(id) {
            if let Some(index) = self.mailboxes.iter().position(|row| row.id == mailbox) {
                self.selected_mailbox = index;
                self.selection = Selection::new();
                self.refresh_rows(cx);
            }
        }
        self.select_message(id, window, cx);
        window.activate_window();
    }

    /// Mark the open message read once it has been open for [`READ_DWELL`] — through the triage
    /// queue, so it reaches Gmail like any other change, but without an undo bar.
    fn schedule_mark_read(&mut self, cx: &mut Context<Self>) {
        self.mark_read = None;
        let Some(id) = self.reading_id else {
            return;
        };
        if !self.mail.message(id).is_some_and(|row| row.unread) {
            return;
        }
        self.mark_read = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(READ_DWELL).await;
            this.update(cx, |this, cx| {
                if this.reading_id != Some(id) {
                    return;
                }
                this.mail
                    .triage(id, &snail_core::triage::TriageAction::MarkRead(true));
                // Read here, so its notification is no longer news.
                cx.dismiss_system_notification(&crate::notify::message_tag(id));
                this.mark_read = None;
                // Refresh counts and the row's dot without touching the open message.
                this.refresh_rows(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn fetch_remote(&mut self, urls: Vec<(String, String)>, cx: &mut Context<Self>) {
        let mail = self.mail.clone();
        let generation = self.generation;
        let task = cx.background_executor().spawn(async move {
            let mut out = Vec::new();
            for (token, url) in urls {
                if let Some(bytes) = mail.fetch_remote_image(&url) {
                    if let Some((width, height, bgra)) = crate::html_view::decode_to_bgra(&bytes) {
                        out.push(RemoteImage {
                            generation,
                            token,
                            width,
                            height,
                            bgra,
                        });
                    }
                }
            }
            out
        });
        cx.spawn(async move |this, cx| {
            let fetched = task.await;
            log::info!("remote: {} image(s) decoded", fetched.len());
            this.update(cx, |this, cx| {
                this.pending_remote.extend(fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The width to lay HTML out at now: the measured pane width, or the default before the pane
    /// has been measured. Every `layout_document` call uses this, so they cannot disagree.
    fn html_layout_width(&self) -> f32 {
        self.html_width
            .unwrap_or(snail_ui::layout::DEFAULT_DOCUMENT_WIDTH)
    }

    /// Record the reading pane's content width, measured during prepaint, and schedule a relayout
    /// if the open HTML message was laid out at a different width.
    ///
    /// The first measurement relays out on the next frame, so the first HTML message after launch
    /// is not shown at the default width for the length of the debounce. Every later change —
    /// a window resize, the list collapsing — waits for the width to hold still.
    fn note_html_pane_width(&mut self, content_width: f32, cx: &mut Context<Self>) {
        use snail_ui::layout::{document_width, width_changed};
        let width = document_width(content_width);
        let previous = self.html_width.replace(width);
        if previous.is_some_and(|previous| !width_changed(previous, width)) {
            // Unchanged since the last frame: leave any pending relayout to fire. Re-arming it
            // here would let a steady stream of frames postpone it forever.
            return;
        }
        let stale = matches!(
            &self.reading,
            Some(Reading { body: BodyKind::Html { view, .. }, .. })
                if width_changed(view.width(), width)
        );
        if !stale {
            // Resized back to the width already laid out: nothing to do.
            self.html_relayout = None;
            return;
        }
        let delay = if previous.is_some() {
            HTML_RELAYOUT_DEBOUNCE
        } else {
            std::time::Duration::ZERO
        };
        self.html_relayout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.html_relayout_due = true;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Lay the open HTML message out again at the current pane width, if it is not already. Runs in
    /// render, once the debounce has fired.
    fn apply_pending_relayout(&mut self, window: &mut Window, cx: &mut App) {
        if !std::mem::take(&mut self.html_relayout_due) {
            return;
        }
        self.html_relayout = None;
        let width = self.html_layout_width();
        let Some(Reading {
            body:
                BodyKind::Html {
                    document,
                    view,
                    images,
                    quotes_collapsed,
                    ..
                },
            ..
        }) = &mut self.reading
        else {
            return;
        };
        if !snail_ui::layout::width_changed(view.width(), width) {
            return;
        }
        let selection = view.selection();
        match crate::html_view::layout_document(
            document,
            width,
            window,
            images,
            Some(selection),
            !*quotes_collapsed,
            cx,
        ) {
            Ok(updated) => *view = updated,
            Err(error) => log::warn!("could not re-lay out the message at {width}px: {error:?}"),
        }
    }

    /// Turn fetched remote images into gpui images and re-lay out the reading pane. Runs in render,
    /// where the window is available for text metrics.
    fn apply_pending_images(&mut self, window: &mut Window, cx: &mut App) {
        if self.pending_remote.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_remote);
        let width = self.html_layout_width();
        let generation = self.generation;
        let mut applied = 0;
        if let Some(Reading {
            body:
                BodyKind::Html {
                    document,
                    view,
                    images,
                    quotes_collapsed,
                    ..
                },
            ..
        }) = &mut self.reading
        {
            for remote in pending {
                if remote.generation != generation {
                    continue;
                }
                let render =
                    crate::html_view::render_image(remote.width, remote.height, remote.bgra);
                images.insert_remote(&remote.token, remote.width, remote.height, render);
                applied += 1;
            }
            let selection = view.selection();
            if let Ok(updated) = crate::html_view::layout_document(
                document,
                width,
                window,
                images,
                Some(selection),
                !*quotes_collapsed,
                cx,
            ) {
                *view = updated;
            }
        }
        if applied > 0 {
            log::info!("remote: applied {applied} image(s)");
        }
    }

    fn read_message(&self, id: i64, window: &mut Window, cx: &mut App) -> Option<Reading> {
        let row = self.mail.message(id)?;
        let raw = self.mail.raw(id);
        let invite = raw.as_deref().and_then(snail_core::mime::calendar_invite);
        let parsed = self
            .mail
            .parsed_with_preference(id, settings::body_preference(cx))?;
        let fallback_plain = parsed.plain.clone();
        let body = match parsed.body {
            snail_core::mime::Body::Html(html) => {
                let images = match self.mail.raw(id) {
                    Some(raw) => crate::html_view::Images::from_message(&raw),
                    None => crate::html_view::Images::default(),
                };
                let document = crate::html_view::prepare_document(&html);
                let has_quotes = crate::html_view::has_quoted_content(&document);
                match crate::html_view::layout_document(
                    &document,
                    self.html_layout_width(),
                    window,
                    &images,
                    None,
                    !has_quotes,
                    cx,
                ) {
                    Ok(view) => BodyKind::Html {
                        document,
                        view,
                        images,
                        quotes_collapsed: has_quotes,
                        has_quotes,
                    },
                    Err(error) => {
                        log::warn!("html layout fallback: {error:?}");
                        plain_body(
                            fallback_plain
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or_else(|| document.plain_text.clone()),
                        )
                    }
                }
            }
            _ => plain_body(
                parsed
                    .plain
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "(This message has no readable text part.)".to_string()),
            ),
        };
        let sender_addr = row.from_addr.clone();
        Some(Reading {
            subject: row.subject.unwrap_or_else(|| "(no subject)".into()),
            from: row
                .from_name
                .or(row.from_addr)
                .unwrap_or_else(|| "(unknown sender)".into()),
            sender_addr,
            meta: row
                .date
                .map(|date| format!("{date}"))
                .unwrap_or_else(|| "(no date)".into()),
            crypto: VerificationState::NotSigned,
            invite,
            body,
        })
    }

    fn expand_quotes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.html_layout_width();
        let Some(reading) = &mut self.reading else {
            return;
        };
        match &mut reading.body {
            BodyKind::Plain { expanded, .. } => *expanded = true,
            BodyKind::Html {
                document,
                view,
                images,
                quotes_collapsed,
                ..
            } => {
                let selection = view.selection();
                match crate::html_view::layout_document(
                    document,
                    width,
                    window,
                    images,
                    Some(selection),
                    true,
                    cx,
                ) {
                    Ok(updated) => {
                        *view = updated;
                        *quotes_collapsed = false;
                    }
                    Err(error) => log::warn!("could not expand quoted text: {error:?}"),
                }
            }
        }
        cx.notify();
    }

    fn allow_sender_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sender) = self
            .reading
            .as_ref()
            .and_then(|reading| reading.sender_addr.clone())
        else {
            return;
        };
        settings::allow_images_from(&sender, cx);
        self.load_reading(window, cx);
        cx.notify();
    }

    fn open_current_in_browser(&self, cx: &mut App) {
        let Some(Reading {
            body: BodyKind::Html { document, .. },
            ..
        }) = &self.reading
        else {
            return;
        };
        match crate::html_view::browser_file(document) {
            Ok(path) => cx.open_with_system(&path),
            Err(error) => log::warn!("could not write browser message: {error}"),
        }
    }

    fn palette_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.palette_open {
            return None;
        }
        let palette = style::palette(cx);
        let mut results = div()
            .flex()
            .flex_col()
            .max_h(px(430.0))
            .overflow_y_scrollbar();
        for (index, candidate) in self.palette_results.iter().cloned().enumerate() {
            let selected = index == self.palette_selected;
            let enabled = match candidate.target {
                PaletteTarget::Command(id) => self.command_enabled(id, cx),
                _ => true,
            };
            let detail = match candidate.target {
                PaletteTarget::Command(id) => format!(
                    "{}  ·  {}",
                    candidate.detail,
                    display_binding(
                        command(id).default_binding,
                        if cfg!(target_os = "macos") {
                            ShortcutPlatform::Mac
                        } else {
                            ShortcutPlatform::Other
                        }
                    )
                ),
                _ => candidate.detail.to_string(),
            };
            results = results.child(
                div()
                    .id(("palette-result", index))
                    .h(px(44.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(palette.radii.button))
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(selected, |this| {
                        this.bg(style::color(palette.colors.accent_tint))
                    })
                    .when(enabled, |this| {
                        this.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.palette_selected = index;
                                this.activate_palette(window, cx);
                            }),
                        )
                    })
                    .child(style::text(candidate.title, TextRole::EventTitle, cx))
                    .child(div().flex_1())
                    .child(style::text(detail, TextRole::ReadingMeta, cx)),
            );
        }
        if self.palette_results.is_empty() {
            results = results.child(
                div()
                    .h(px(70.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(style::text("No matches", TextRole::ReadingMeta, cx)),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(rgba(0x11182742))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(90.0))
                .child(
                    div()
                        .id("command-palette")
                        .key_context("Search")
                        .w(px(620.0))
                        .max_w(px(620.0))
                        .p_2()
                        .rounded(px(13.0))
                        .border_1()
                        .border_color(style::color(palette.colors.border_strong))
                        .bg(style::color(palette.colors.card))
                        .shadow_lg()
                        .child(div().px_2().pb_2().child(Input::new(&self.palette_input)))
                        .child(results),
                )
                .into_any_element(),
        )
    }

    #[cfg(target_os = "macos")]
    fn fallback_menu_button(&self, _cx: &mut Context<Self>) -> AnyElement {
        div().into_any_element()
    }

    #[cfg(not(target_os = "macos"))]
    fn fallback_menu_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .id("fallback-menu-button")
            .h(px(30.0))
            .px_3()
            .flex()
            .items_center()
            .rounded(px(palette.radii.button))
            .cursor_pointer()
            .hover(|this| this.bg(style::color(palette.colors.sunken)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.app_menu_open = !this.app_menu_open;
                cx.notify();
            }))
            .child(style::text("Menu", TextRole::ButtonLabel, cx))
            .into_any_element()
    }

    #[cfg(target_os = "macos")]
    fn fallback_menu_overlay(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    #[cfg(not(target_os = "macos"))]
    fn fallback_menu_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.app_menu_open {
            return None;
        }
        let palette = style::palette(cx);
        let mut card = div()
            .absolute()
            .top(px(44.0))
            .right(px(12.0))
            .w(px(300.0))
            .max_h(px(620.0))
            .overflow_y_scrollbar()
            .p_2()
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border_strong))
            .bg(style::color(palette.colors.card))
            .shadow_lg();
        for (menu_index, item) in snail_ui::commands::commands()
            .iter()
            .filter(|command| command.menu.is_some())
            .enumerate()
        {
            let id = item.id;
            let enabled = self.command_enabled(id, cx);
            card = card.child(
                div()
                    .id(("fallback-menu-item", menu_index))
                    .h(px(34.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(px(palette.radii.button))
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.app_menu_open = false;
                                this.execute_command(id, window, cx);
                            }))
                    })
                    .child(style::text(item.title, TextRole::ButtonLabel, cx))
                    .child(div().flex_1())
                    .child(style::text(
                        display_binding(item.default_binding, ShortcutPlatform::Other),
                        TextRole::EventTime,
                        cx,
                    )),
            );
        }
        Some(card.into_any_element())
    }

    fn titlebar(
        &self,
        palette: &snail_ui::theme::Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let calendar_view = self.calendar.read(cx).view();
        div()
            .h(px(palette.metrics.titlebar_h))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .bg(style::color(palette.colors.chrome))
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(cfg!(target_os = "macos"), |this| this.pl(px(78.0)))
                    .child(style::text("Snail", TextRole::ListHeaderTitle, cx))
                    .child(self.workspace_button("workspace-mail", "Mail", WorkspaceMode::Mail, cx))
                    .child(self.workspace_button(
                        "workspace-calendar",
                        "Calendar",
                        WorkspaceMode::Calendar,
                        cx,
                    ))
                    .child(self.workspace_button(
                        "workspace-settings",
                        "Settings",
                        WorkspaceMode::Settings,
                        cx,
                    ))
                    .child(
                        div()
                            .id("command-palette-button")
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(palette.radii.button))
                            .cursor_pointer()
                            .hover(|this| this.bg(style::color(palette.colors.sunken)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.palette_open {
                                    this.close_palette(window, cx);
                                } else {
                                    this.open_palette(window, cx);
                                }
                            }))
                            .child(style::text(
                                if cfg!(target_os = "macos") {
                                    "Search…  ⌘K"
                                } else {
                                    "Search…  Ctrl+K"
                                },
                                TextRole::ButtonLabel,
                                cx,
                            )),
                    ),
            )
            .when(self.workspace == WorkspaceMode::Calendar, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .children(CalendarView::ALL.map(|view| {
                            let selected = view == calendar_view;
                            let calendar = self.calendar.clone();
                            div()
                                .id(("calendar-view", view as usize))
                                .h(px(30.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(palette.radii.button))
                                .cursor_pointer()
                                .when(selected, |this| {
                                    this.bg(style::color(palette.colors.card))
                                        .text_color(style::color(palette.colors.accent))
                                })
                                .hover(|this| this.bg(style::color(palette.colors.sunken)))
                                .on_click(move |_, _, cx| {
                                    calendar.update(cx, |calendar, cx| calendar.set_view(view, cx));
                                })
                                .child(style::text(view.label(), TextRole::ButtonLabel, cx))
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.fallback_menu_button(cx))
                    .child(Self::window_controls(palette, cx)),
            )
            .into_any_element()
    }

    fn workspace_button(
        &self,
        id: &'static str,
        label: &'static str,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = style::palette(cx);
        let selected = self.workspace == mode;
        div()
            .id(id)
            .h(px(30.0))
            .px_3()
            .flex()
            .items_center()
            .rounded(px(palette.radii.button))
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(style::color(palette.colors.card))
                    .text_color(style::color(palette.colors.accent))
            })
            .hover(|this| this.bg(style::color(palette.colors.sunken)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.workspace = mode;
                if mode == WorkspaceMode::Mail {
                    let general = settings::general(cx);
                    this.remote_enabled = general.load_remote_images
                        || std::env::var_os("SNAIL_REMOTE_IMAGES").is_some();
                    this.group_threads = general.group_by_thread;
                }
                cx.notify();
            }))
            .child(style::text(label, TextRole::ButtonLabel, cx))
            .into_any_element()
    }

    fn window_controls(palette: &snail_ui::theme::Theme, cx: &App) -> AnyElement {
        #[cfg(target_os = "macos")]
        {
            let _ = (palette, cx);
            div().into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let height = palette.metrics.titlebar_h;
            let button = move |glyph: &'static str, area: WindowControlArea| {
                div()
                    .window_control_area(area)
                    .w(px(34.0))
                    .h(px(height))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(style::color(palette.colors.secondary))
                    .hover(|this| this.bg(style::color(palette.colors.sunken)))
                    .child(glyph)
            };
            let _ = cx;
            div()
                .flex()
                .child(
                    button("–", WindowControlArea::Min)
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.minimize_window()),
                )
                .child(
                    button("□", WindowControlArea::Max)
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.zoom_window()),
                )
                .child(
                    button("×", WindowControlArea::Close)
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.remove_window()),
                )
                .into_any_element()
        }
    }

    fn sidebar(&self, palette: &snail_ui::theme::Theme, cx: &mut Context<Self>) -> AnyElement {
        let mailboxes = self.mailboxes.clone();
        div()
            .flex_none()
            .w(px(palette.metrics.sidebar_w))
            .h_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_3()
            .bg(style::color(palette.colors.chrome))
            .border_r_1()
            .border_color(style::color(palette.colors.border_soft))
            .child(
                div()
                    .id("open-calendar")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .mb_2()
                    .rounded(px(palette.radii.button))
                    .cursor_pointer()
                    .hover(|this| this.bg(style::color(palette.colors.accent_tint)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace = WorkspaceMode::Calendar;
                        cx.notify();
                    }))
                    .child(icon(Icon::Calendar).text_color(style::color(palette.colors.accent)))
                    .child(style::text("Calendar", TextRole::SidebarItem, cx)),
            )
            .child(
                div()
                    .id("pgp-key-manager")
                    .px_2()
                    .pb_2()
                    .hover(|this| this.bg(style::color(palette.colors.sunken)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            crate::pgp_keys::open(this.mail.clone(), cx);
                        }),
                    )
                    .child(style::text(
                        "Mailboxes · PGP keys…",
                        TextRole::SectionLabel,
                        cx,
                    )),
            )
            .children(mailboxes.into_iter().enumerate().map(|(index, mailbox)| {
                let selected = index == self.selected_mailbox;
                Self::sidebar_item(palette, cx, index, mailbox, selected)
            }))
            .child(div().flex_1())
            .child(self.sync_footer(palette, cx))
            .when(self.mail.pending_sends() > 0, |this| {
                let (failed, pending) = self.mail.send_queue();
                this.child(
                    div()
                        .px_2()
                        .pt_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .border_t_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .when(pending > 0, |this| {
                            this.child(style::text(
                                format!("{pending} pending"),
                                TextRole::SidebarCount,
                                cx,
                            ))
                        })
                        .when(failed > 0, |this| {
                            this.child(
                                div()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.mail.retry_failed_sends();
                                            cx.notify();
                                        }),
                                    )
                                    .child(style::text(
                                        format!("{failed} failed · retry"),
                                        TextRole::SidebarCount,
                                        cx,
                                    )),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    /// One line of sync state at the foot of the sidebar; the sign-in case is a button.
    fn sync_footer(&self, palette: &snail_ui::theme::Theme, cx: &mut Context<Self>) -> AnyElement {
        let status = self.sync.read(cx).status().clone();
        let (line, action) = match &status {
            SyncStatus::Starting => (String::new(), false),
            SyncStatus::Syncing {
                first: true,
                done,
                total,
            } if *total > 0 => (format!("Bringing in mail · {done} of {total}"), false),
            SyncStatus::Syncing { first: true, .. } => ("Bringing in mail…".into(), false),
            SyncStatus::Syncing { .. } => ("Checking for mail…".into(), false),
            SyncStatus::UpToDate { at } => (
                if at.elapsed() < Duration::from_secs(60) {
                    "Up to date".into()
                } else {
                    format!("Updated {} min ago", at.elapsed().as_secs() / 60)
                },
                false,
            ),
            // Throttling is not being offline, and says why the count stopped moving.
            SyncStatus::Offline(error) if error.contains("Quota exceeded") => {
                ("Gmail rate limit · resuming shortly".into(), false)
            }
            SyncStatus::Offline(_) => ("Offline · will retry".into(), false),
            SyncStatus::NeedsSignIn { .. } => ("Sign in to Gmail again ›".into(), true),
            SyncStatus::NoClient => ("Set up Gmail ›".into(), true),
        };
        if line.is_empty() {
            return div().into_any_element();
        }
        let tooltip = match &status {
            SyncStatus::Offline(error) => Some(error.clone()),
            SyncStatus::NeedsSignIn { address, message } => Some(format!("{address}: {message}")),
            _ => None,
        };
        let role = if action {
            TextRole::SidebarItemSelected
        } else {
            TextRole::SidebarCount
        };
        div()
            .id("sync-status")
            .px_2()
            .py_1()
            .when(action, |this| {
                this.cursor_pointer()
                    .rounded(px(palette.radii.button))
                    .hover(|this| this.bg(style::color(palette.colors.accent_tint)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sync.update(cx, |hub, cx| hub.begin_reconnect(cx));
                    }))
            })
            .when(!action, |this| {
                this.cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sync.update(cx, |hub, cx| hub.sync_now(cx));
                    }))
            })
            .when_some(tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .child(style::text(line, role, cx).truncate())
            .into_any_element()
    }

    fn sidebar_item(
        palette: &snail_ui::theme::Theme,
        cx: &mut Context<Self>,
        index: usize,
        mailbox: MailboxRow,
        selected: bool,
    ) -> AnyElement {
        let glyph = match mailbox.kind.as_str() {
            "inbox" => Icon::Inbox,
            "sent" => Icon::Send,
            "drafts" => Icon::Document,
            "archive" => Icon::Archive,
            "trash" => Icon::Trash,
            _ => Icon::List,
        };
        let role = if selected {
            TextRole::SidebarItemSelected
        } else {
            TextRole::SidebarItem
        };
        let icon_color = if selected {
            palette.colors.accent
        } else {
            palette.colors.muted
        };
        let count_color = if selected {
            palette.colors.accent
        } else {
            palette.colors.soft
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(px(palette.radii.button))
            .when(selected, |this| {
                this.bg(style::color(palette.colors.accent_tint_deep))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, window, cx| {
                    this.select_mailbox(index, window, cx);
                    cx.notify();
                }),
            )
            .child(icon(glyph).text_color(style::color(icon_color)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(style::text(mailbox.name, role, cx)),
            )
            .when(mailbox.unread > 0, |this| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(style::color(count_color))
                        .child(mailbox.unread.to_string()),
                )
            })
            .into_any_element()
    }

    fn list(&self, palette: &snail_ui::theme::Theme, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let selection = self.selection.clone();

        // Header: the mailbox, or the active query and its result count (E9.3).
        let (title, meta) = match &self.search {
            Some(search) => (
                if search.describe.is_empty() {
                    "Search".to_string()
                } else {
                    search.describe.clone()
                },
                if search.running {
                    "searching…".to_string()
                } else {
                    format!("{} results", search.hits.len())
                },
            ),
            None => {
                let title = self
                    .mailboxes
                    .get(self.selected_mailbox)
                    .map(|mailbox| mailbox.name.clone())
                    .unwrap_or_else(|| "Mail".into());
                let unread = self
                    .mailboxes
                    .get(self.selected_mailbox)
                    .map(|mailbox| mailbox.unread)
                    .unwrap_or(0);
                (title, format!("{unread} unread"))
            }
        };

        let header = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(style::text(title, TextRole::ListHeaderTitle, cx))
                    .child(style::text(meta, TextRole::ListHeaderMeta, cx)),
            )
            // Search always sits in the header, so an active query is visible and Escape-able (E9.6).
            .child(
                div()
                    .key_context("Search")
                    .child(Input::new(&self.search_input)),
            );

        let body = if let Some(search) = &self.search {
            Self::search_results(palette, search, selection, weak, cx)
        } else if self.rows.is_empty() {
            let state = if self.sync.read(cx).is_first_sync() {
                EmptyState::FirstSync
            } else {
                EmptyState::EmptyMailbox
            };
            div()
                .flex_1()
                .child(Self::empty_state(palette, cx, state))
                .into_any_element()
        } else {
            let rows = self.rows.clone();
            let row_ids = Arc::new(self.rows.iter().map(|row| row.id).collect::<Vec<i64>>());
            let count = self.rows.len();
            div()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list("message-list", count, move |range, _window, cx| {
                        let palette = style::palette(cx);
                        range
                            .map(|index| {
                                Self::row(
                                    palette,
                                    &rows[index],
                                    selection.is_selected(rows[index].id),
                                    weak.clone(),
                                    row_ids.clone(),
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .size_full(),
                )
                .into_any_element()
        };

        let list_focus = self.list_focus.clone();
        div()
            .flex_none()
            .w(px(palette.metrics.list_w))
            .h_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .border_r_1()
            .border_color(style::color(palette.colors.border_soft))
            .key_context("List")
            .track_focus(&self.list_focus)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&list_focus, cx);
            })
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The grouped search results: a mailbox header, then its hits, all in one `uniform_list`
    /// because headers and hits share a height (E9.4).
    fn search_results(
        palette: &snail_ui::theme::Theme,
        search: &SearchResults,
        selection: Selection,
        weak: WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        if search.hits.is_empty() {
            let message = if search.running {
                "Searching…"
            } else {
                "No results"
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(style::text(
                    message.to_string(),
                    TextRole::ListHeaderMeta,
                    cx,
                ))
                .into_any_element();
        }

        let mut list_rows: Vec<SearchListRow> = Vec::new();
        let mut index = 0;
        while index < search.hits.len() {
            let name = search.hits[index]
                .mailbox_name
                .clone()
                .unwrap_or_else(|| "Other".into());
            let start = index;
            while index < search.hits.len()
                && search.hits[index].mailbox_name == search.hits[start].mailbox_name
            {
                index += 1;
            }
            list_rows.push(SearchListRow::Header {
                name,
                count: index - start,
            });
            for hit in start..index {
                list_rows.push(SearchListRow::Hit(hit));
            }
        }
        let hits = Arc::new(search.hits.clone());
        let list_rows = Arc::new(list_rows);
        let count = list_rows.len();
        let ids = Arc::new(search.hits.iter().map(|hit| hit.id).collect::<Vec<i64>>());

        div()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("search-results", count, move |range, _window, cx| {
                    let palette = style::palette(cx);
                    range
                        .map(|index| match &list_rows[index] {
                            SearchListRow::Header { name, count } => {
                                Self::search_header(palette, name, *count, cx)
                            }
                            SearchListRow::Hit(hit) => {
                                let row = Self::hit_row(&hits[*hit]);
                                Self::row(
                                    palette,
                                    &row,
                                    selection.is_selected(row.id),
                                    weak.clone(),
                                    ids.clone(),
                                    cx,
                                )
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .size_full(),
            )
            .into_any_element()
    }

    /// A group header, drawn at exactly a result row's height so one `uniform_list` holds both (9.4).
    fn search_header(
        palette: &snail_ui::theme::Theme,
        name: &str,
        count: usize,
        cx: &App,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .h(px(snail_ui::text::message_row_height()))
            .overflow_hidden()
            .bg(style::color(palette.colors.sunken))
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .child(style::text(name.to_string(), TextRole::SectionLabel, cx))
            .child(style::text(count.to_string(), TextRole::SectionLabel, cx))
            .into_any_element()
    }

    /// A search hit as a list row, so it reuses the message row exactly (E9.4).
    fn hit_row(hit: &SearchHit) -> MessageRow {
        MessageRow {
            id: hit.id,
            subject: hit.subject.clone(),
            from_name: hit.from_name.clone(),
            from_addr: hit.from_addr.clone(),
            date: hit.date,
            preview: hit.preview.clone(),
            unread: hit.unread,
            body_hash: None,
        }
    }

    fn row(
        palette: &snail_ui::theme::Theme,
        row: &MessageRow,
        selected: bool,
        weak: WeakEntity<Self>,
        ids: Arc<Vec<i64>>,
        cx: &App,
    ) -> AnyElement {
        let id = row.id;
        // Each field is one flowing paragraph; GPUI wraps and clamps it at the row's real width.
        let single_line = snail_ui::preview::single_line;
        let sender = single_line(
            row.from_name
                .as_deref()
                .or(row.from_addr.as_deref())
                .unwrap_or("(unknown)"),
        );
        let subject = single_line(row.subject.as_deref().unwrap_or("(no subject)"));
        let preview = single_line(row.preview.as_deref().unwrap_or_default());
        let unread = row.unread;
        let timestamp = row.date.map(row_timestamp).unwrap_or_default();

        let base = div()
            .relative()
            .w_full()
            // Tall enough for its own text (derived from the text roles), so clipping it below can
            // only ever trim what the clamps have already cut.
            .h(px(snail_ui::text::message_row_height()))
            // Nothing a row draws may reach the next one.
            .overflow_hidden()
            .flex()
            .gap_2()
            .px_4()
            .py(px(snail_ui::text::MESSAGE_ROW_PADDING_Y))
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .when(selected, |this| {
                this.bg(style::color(palette.colors.accent_tint))
            })
            .when(!selected, |this| this.hover(|s| s.bg(rgba(0x00000008))))
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                weak.update(cx, |this, cx| {
                    this.selection.select_in(&ids, id);
                    this.load_reading(window, cx);
                    cx.notify();
                })
                .ok();
            });
        base.when(selected, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(3.0))
                    .h_full()
                    .bg(style::color(palette.colors.accent)),
            )
        })
        .child(
            div()
                .flex_none()
                .mt(px(4.0))
                .w(px(7.0))
                .h(px(7.0))
                .rounded_full()
                .when(unread, |this| this.bg(style::color(palette.colors.accent))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(snail_ui::text::MESSAGE_ROW_GAP))
                // The sender, with the time pushed to the top-right corner, baseline-aligned
                // (mail handoff, list row line 1). The sender truncates; the time never does.
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(
                                style::text(
                                    sender,
                                    if unread {
                                        TextRole::RowSenderUnread
                                    } else {
                                        TextRole::RowSenderRead
                                    },
                                    cx,
                                )
                                .truncate(),
                            ),
                        )
                        .child(div().flex_none().child(style::text(
                            timestamp,
                            TextRole::RowTimestamp,
                            cx,
                        ))),
                )
                .child(
                    style::text(
                        subject,
                        if unread {
                            TextRole::RowSubjectUnread
                        } else {
                            TextRole::RowSubjectRead
                        },
                        cx,
                    )
                    .truncate(),
                )
                // Two lines with an ellipsis, measured on the shaped text — including a long URL,
                // which wraps at character boundaries rather than overflowing (E5.3).
                .child(
                    style::text(preview, TextRole::RowPreview, cx)
                        .line_clamp(2)
                        .text_ellipsis(),
                ),
        )
        .into_any_element()
    }

    /// Apply a triage action to the whole selection (E8.4/E8.7) and arm the undo bar (E8.5).
    fn triage_selection(
        &mut self,
        action: snail_core::triage::TriageAction,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selection.selected_ids();
        let ops: Vec<i64> = ids
            .iter()
            .filter_map(|id| self.mail.triage(*id, &action))
            .collect();
        if !ops.is_empty() {
            self.undo = Some(UndoBar {
                ops,
                label: label.to_string(),
                at: Instant::now(),
            });
        }
        self.reload(window, cx);
        cx.notify();
    }

    /// Toggle read/unread on the selection (E8.4).
    fn toggle_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selection.cursor() else {
            return;
        };
        let unread = self.mail.message(id).map(|row| row.unread).unwrap_or(false);
        self.triage_selection(
            snail_core::triage::TriageAction::MarkRead(unread),
            if unread {
                "Marked read"
            } else {
                "Marked unread"
            },
            window,
            cx,
        );
    }

    /// Conversation-level archive: every message in the cursor's thread (E8.8).
    fn archive_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message_id) = self.selection.cursor() else {
            return;
        };
        let Some(thread_id) = self.mail.thread_id_of(message_id) else {
            return;
        };
        let ids: Vec<i64> = self
            .mail
            .thread_messages(thread_id)
            .iter()
            .map(|row| row.id)
            .collect();
        let ops: Vec<i64> = ids
            .iter()
            .filter_map(|id| {
                self.mail
                    .triage(*id, &snail_core::triage::TriageAction::Archive)
            })
            .collect();
        if !ops.is_empty() {
            self.undo = Some(UndoBar {
                ops,
                label: "Conversation archived".into(),
                at: Instant::now(),
            });
        }
        self.reload(window, cx);
        cx.notify();
    }

    /// How many search results to ask for (E9.3): enough to feel complete, bounded for speed.
    const SEARCH_LIMIT: u32 = 300;

    /// ⌘F puts the caret in the search box (E9.6).
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search_input.clone();
        input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn on_search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_query = self.search_input.read(cx).value().to_string();
        self.run_search(cx);
    }

    /// Clear the box and restore the mailbox list (E9.6). `set_value` does not emit `Change`, so the
    /// rerun is explicit.
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search_input.clone();
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.search_query.clear();
        self.run_search(cx);
    }

    /// Run the query on the background executor, guarded by a generation so a slow earlier query
    /// cannot overwrite a newer one (E9.3).
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        if self.search_query.trim().is_empty() {
            self.search = None;
            self.selection = Selection::new();
            self.reading = None;
            self.reading_id = None;
            cx.notify();
            return;
        }
        let parsed = snail_ui::search::parse(&self.search_query, current_epoch());
        let criteria = MailModel::search_criteria(&parsed);
        self.search = Some(SearchResults {
            hits: Vec::new(),
            running: true,
            describe: parsed.describe(),
        });
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.search(&criteria, Self::SEARCH_LIMIT) });
        cx.spawn(async move |this, cx| {
            let hits = task.await;
            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                let Some(mut search) = this.search.take() else {
                    return;
                };
                search.hits = hits;
                search.running = false;
                this.search = Some(search);
                let ids: Vec<i64> = this
                    .search
                    .as_ref()
                    .map(|search| search.hits.iter().map(|hit| hit.id).collect())
                    .unwrap_or_default();
                this.selection.reconcile(&ids);
                if this.selection.cursor().is_none() && !ids.is_empty() {
                    this.selection.select(ids[0]);
                }
                // Render reloads the reading for whatever the cursor now is.
                this.reading_id = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the move picker over the mailboxes other than the one being read (E8.4).
    fn open_move_picker(&mut self, cx: &mut Context<Self>) {
        let current = self
            .mailboxes
            .get(self.selected_mailbox)
            .map(|mailbox| mailbox.id);
        let mailboxes: Vec<(i64, String)> = self
            .mailboxes
            .iter()
            .filter(|mailbox| Some(mailbox.id) != current)
            .map(|mailbox| (mailbox.id, mailbox.name.clone()))
            .collect();
        if mailboxes.is_empty() {
            return;
        }
        self.move_picker = Some(MovePicker {
            mailboxes,
            selected: 0,
        });
        cx.notify();
    }

    fn move_picker_step(&mut self, delta: isize) {
        if let Some(picker) = self.move_picker.as_mut() {
            let count = picker.mailboxes.len() as isize;
            if count > 0 {
                picker.selected = (picker.selected as isize + delta).rem_euclid(count) as usize;
            }
        }
    }

    /// Move the whole selection to the chosen mailbox, with one undo (E8.4/E8.7).
    fn commit_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.move_picker.take() else {
            return;
        };
        let Some((_, name)) = picker.mailboxes.get(picker.selected).cloned() else {
            return;
        };
        let label = format!("Moved to {name}");
        self.triage_selection(
            snail_core::triage::TriageAction::Move { mailbox: name },
            &label,
            window,
            cx,
        );
    }

    /// Highlight a clicked picker row and commit it (E8.4).
    fn select_move_target(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.move_picker.as_mut() {
            if let Some(index) = picker
                .mailboxes
                .iter()
                .position(|(mailbox, _)| *mailbox == id)
            {
                picker.selected = index;
            }
        }
        self.commit_move(window, cx);
    }

    fn undo_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.undo.take() {
            for op in bar.ops {
                self.mail.undo_triage(op);
            }
        }
        self.reload(window, cx);
        cx.notify();
    }

    fn compose_reply(&mut self, all: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selection.cursor() else {
            return;
        };
        let Some(parsed) = self.mail.parsed(id) else {
            return;
        };
        let self_addr = self.mail.first_account_address();
        let mut draft = snail_core::compose::reply(&parsed, self_addr.as_deref(), all);
        if let Some(signature) = self
            .mail
            .signature_for(self_addr.as_deref())
            .or_else(|| crate::settings::signature_for(self_addr.as_deref(), cx))
        {
            draft.body_text = snail_core::compose::with_signature(&draft.body_text, &signature);
        }
        crate::compose::open(self.mail.clone(), draft, cx);
    }

    fn compose_forward(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selection.cursor() else {
            return;
        };
        let Some(parsed) = self.mail.parsed(id) else {
            return;
        };
        let draft = snail_core::compose::forward(&parsed);
        crate::compose::open(self.mail.clone(), draft, cx);
    }

    /// The conversation strip above an open message when its thread has siblings (E8.2): the
    /// thread subject, its mailbox, the participant list and the count, then the *other* messages
    /// collapsed to name, snippet and date. The open message stays expanded below; a collapsed row
    /// opens that message in place. Roles come from `snail-ui::text`, so no size or hex is here.
    fn thread_strip(
        &self,
        palette: &snail_ui::theme::Theme,
        thread: &[MessageRow],
        mailbox: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cursor = self.selection.cursor();
        let mut seen = std::collections::HashSet::new();
        let mut participants: Vec<String> = Vec::new();
        for message in thread {
            let name = message
                .from_name
                .clone()
                .or_else(|| message.from_addr.clone())
                .unwrap_or_default();
            if !name.is_empty() && seen.insert(name.clone()) {
                participants.push(name);
            }
        }
        let subject = thread
            .iter()
            .rev()
            .find_map(|message| message.subject.clone())
            .unwrap_or_else(|| "(no subject)".into());

        let mut strip = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_6()
            .py_4()
            .bg(style::color(palette.colors.sunken))
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .child(style::text(subject, TextRole::ThreadSubject, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .when_some(mailbox, |this, mailbox| {
                        this.child(
                            div()
                                .px_2()
                                .py(px(1.0))
                                .rounded(px(palette.radii.chip))
                                .bg(style::color(palette.colors.accent_tint_deep))
                                .child(style::text(mailbox, TextRole::MailboxPill, cx)),
                        )
                    })
                    .child(style::text(
                        format!("{} messages", thread.len()),
                        TextRole::ReadingMeta,
                        cx,
                    ))
                    .child(style::text(
                        participants.join(", "),
                        TextRole::ReadingMeta,
                        cx,
                    )),
            );

        for message in thread {
            if Some(message.id) == cursor {
                continue;
            }
            let id = message.id;
            let sender = snail_ui::preview::single_line(
                message
                    .from_name
                    .as_deref()
                    .or(message.from_addr.as_deref())
                    .unwrap_or("(unknown)"),
            );
            let snippet =
                snail_ui::preview::single_line(message.preview.as_deref().unwrap_or_default());
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(px(palette.radii.button))
                    .hover(|this| this.bg(style::color(palette.colors.card)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            this.select_message(id, window, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(style::color(palette.colors.accent_tint_deep))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(style::text(initials(&sender), TextRole::MailboxPill, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(style::text(sender, TextRole::CollapsedSender, cx))
                            .child(style::text(snippet, TextRole::CollapsedSnippet, cx)),
                    )
                    .child(style::text(
                        short_date(message.date),
                        TextRole::CollapsedDate,
                        cx,
                    )),
            );
        }
        strip.into_any_element()
    }

    fn rsvp_from_mail(&mut self, status: &'static str, cx: &mut Context<Self>) {
        let Some(invite) = self
            .reading
            .as_ref()
            .and_then(|reading| reading.invite.clone())
        else {
            return;
        };
        let model = self.calendar_model.clone();
        let mail = self.mail.clone();
        let subject = invite
            .summary
            .clone()
            .unwrap_or_else(|| "Calendar invitation".into());
        let task = cx.background_executor().spawn(async move {
            let delivery = model.rsvp_invite(&invite, status, chrono::Utc::now().timestamp())?;
            if let RsvpDelivery::Imip {
                recipient,
                calendar_reply,
            } = delivery
            {
                let from = mail
                    .first_account_address()
                    .ok_or_else(|| anyhow::anyhow!("no sending account for RSVP"))?;
                let answer = match status {
                    "accepted" => "Accepted",
                    "tentative" => "Tentatively accepted",
                    "declined" => "Declined",
                    _ => "Responded",
                };
                let draft = snail_core::compose::Draft {
                    from_addr: Some(from),
                    to: vec![snail_core::mime::Recipient {
                        name: None,
                        address: recipient,
                    }],
                    subject: format!("Re: {subject}"),
                    body_text: format!("{answer}: {subject}"),
                    attachments: vec![snail_core::compose::Attachment {
                        filename: "invite.ics".into(),
                        content_type: "text/calendar; method=REPLY; charset=utf-8".into(),
                        bytes: calendar_reply.into_bytes(),
                    }],
                    ..Default::default()
                };
                let raw = snail_core::compose::build_raw(&draft)?;
                mail.enqueue_send(&raw)
                    .ok_or_else(|| anyhow::anyhow!("could not queue iMIP response"))?;
            }
            anyhow::Ok(())
        });
        cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                this.update(cx, |this, cx| {
                    this.sync.update(cx, |hub, cx| hub.sync_now(cx));
                })
                .ok();
            }
            Err(error) => log::warn!("could not respond to calendar invitation: {error:#}"),
        })
        .detach();
    }

    fn reading_pane(&self, palette: &snail_ui::theme::Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(reading) = &self.reading else {
            return Self::empty_state(palette, cx, EmptyState::EmptyMailbox);
        };
        let is_html = matches!(&reading.body, BodyKind::Html { .. });
        let unloaded_remote = matches!(
            &reading.body,
            BodyKind::Html { document, images, .. }
                if document.remote_resources.iter().any(|resource| !images.contains(&resource.token))
        );
        let sender_allowed = reading
            .sender_addr
            .as_deref()
            .is_some_and(|sender| settings::always_load_images_from(sender, cx));
        let has_blocked_remote = unloaded_remote
            && !self.remote_enabled
            && !sender_allowed
            && self.forced_message != self.selection.cursor();
        let quotes_collapsed = match &reading.body {
            BodyKind::Plain {
                quoted, expanded, ..
            } => quoted.is_some() && !expanded,
            BodyKind::Html {
                quotes_collapsed,
                has_quotes,
                ..
            } => *has_quotes && *quotes_collapsed,
        };
        let preference_label = match settings::body_preference(cx) {
            snail_core::mime::BodyPreference::Html => "Prefer plain text",
            snail_core::mime::BodyPreference::Plain => "Prefer HTML",
        };
        let crypto_label = match &reading.crypto {
            VerificationState::NotSigned => None,
            VerificationState::Valid { signer, .. } => Some(format!("✓ Verified · {signer}")),
            VerificationState::Unverified { detail } => Some(format!("? Unverified · {detail}")),
            VerificationState::Failed { detail } => Some(format!("! Signature failed · {detail}")),
            VerificationState::PolicyRejected { reason, .. } => {
                Some(format!("! Signature rejected · {reason:?}"))
            }
            VerificationState::Encrypted { detail, .. } => Some(format!("🔒 {detail}")),
            VerificationState::DecryptionFailed { detail } => Some(format!("! {detail}")),
        };
        let crypto_retry = matches!(
            &reading.crypto,
            VerificationState::Encrypted {
                decrypted: false,
                ..
            } | VerificationState::DecryptionFailed { .. }
        );
        let action = |label: &str| {
            div()
                .px_2()
                .py_1()
                .rounded(px(palette.radii.button))
                .border_1()
                .border_color(style::color(palette.colors.border_soft))
                .text_size(px(11.0))
                .text_color(style::color(palette.colors.secondary))
                .hover(|this| this.bg(style::color(palette.colors.sunken)))
                .child(label.to_string())
        };
        // The conversation, if this message has siblings (E8.2). Computed before the body so the
        // strip is the first child and the open message is the expanded one below it.
        let thread = self
            .selection
            .cursor()
            .and_then(|id| self.mail.thread_id_of(id))
            .map(|thread_id| self.mail.thread_messages(thread_id))
            .unwrap_or_default();
        let strip = (thread.len() > 1).then(|| {
            self.thread_strip(
                palette,
                &thread,
                self.mailboxes
                    .get(self.selected_mailbox)
                    .map(|mailbox| mailbox.name.clone()),
                cx,
            )
        });
        let reading_focus = self.reading_focus.clone();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .key_context("Reading")
            .track_focus(&self.reading_focus)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&reading_focus, cx);
            })
            .when_some(strip, |this, strip| this.child(strip))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_hairline))
                    .child(style::text(
                        reading.subject.clone(),
                        TextRole::ReadingSubject,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded_full()
                                    .bg(style::color(palette.colors.accent_tint_deep))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(style::text(
                                        initials(&reading.from),
                                        TextRole::MailboxPill,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(style::text(
                                        reading.from.clone(),
                                        TextRole::ReadingSenderName,
                                        cx,
                                    ))
                                    .child(style::text(
                                        reading.meta.clone(),
                                        TextRole::ReadingMeta,
                                        cx,
                                    )),
                            )
                            .child(div().flex_1()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .when_some(crypto_label, |this, label| this.child(action(&label)))
                            .when(crypto_retry, |this| {
                                this.child(action("Retry PGP").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.schedule_pgp_analysis(cx);
                                    }),
                                ))
                            })
                            .child(action(preference_label).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    settings::toggle_body_preference(cx);
                                    this.load_reading(window, cx);
                                    cx.notify();
                                }),
                            ))
                            .child(action("Reply").on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| {
                                    this.compose_reply(false, cx);
                                }),
                            ))
                            .child(action("Forward").on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| {
                                    this.compose_forward(cx);
                                }),
                            ))
                            .when(is_html, |this| {
                                this.child(action("Open in browser").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.open_current_in_browser(cx);
                                    }),
                                ))
                            })
                            .when(has_blocked_remote, |this| {
                                this.child(action("Load images").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.forced_message = this.selection.cursor();
                                        this.load_reading(window, cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    action("Always load images from this sender").on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.allow_sender_images(window, cx);
                                        }),
                                    ),
                                )
                            }),
                    ),
            )
            .when_some(reading.invite.clone(), |this, invite| {
                let starts = invite
                    .start_utc
                    .and_then(|value| chrono::DateTime::from_timestamp(value, 0))
                    .map(|value| value.format("%a, %b %-d · %H:%M UTC").to_string())
                    .unwrap_or_else(|| "Time not supplied".into());
                this.child(
                    div()
                        .mx_6()
                        .mt_4()
                        .p_4()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded(px(palette.radii.card))
                        .border_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .bg(style::color(palette.colors.accent_tint))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(style::text(
                                    invite
                                        .summary
                                        .unwrap_or_else(|| "Calendar invitation".into()),
                                    TextRole::EventTitle,
                                    cx,
                                ))
                                .child(style::text(starts, TextRole::EventTime, cx)),
                        )
                        .children(
                            [
                                ("accepted", "Accept"),
                                ("tentative", "Maybe"),
                                ("declined", "Decline"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, (status, label))| {
                                div()
                                    .id(("mail-rsvp", index))
                                    .h(px(30.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded(px(palette.radii.button))
                                    .border_1()
                                    .border_color(style::color(palette.colors.border_soft))
                                    .bg(style::color(palette.colors.card))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.rsvp_from_mail(status, cx)
                                    }))
                                    .child(style::text(label, TextRole::ButtonLabel, cx))
                            }),
                        ),
                )
            })
            .child(
                div()
                    .id("reading-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.reading_scroll)
                    .px_6()
                    .py_5()
                    // Measures the content box — the width inside the padding — as it is laid
                    // out this frame, so HTML uses the pane's real width. Zero height, so it takes
                    // no space; full width in this block container, so it spans exactly that box.
                    .child({
                        let shell = cx.entity().downgrade();
                        canvas(
                            move |bounds, _window, cx| {
                                shell
                                    .update(cx, |this, cx| {
                                        this.note_html_pane_width(bounds.size.width.into(), cx)
                                    })
                                    .ok();
                            },
                            |_, _, _, _| {},
                        )
                        .w_full()
                        .h(px(0.0))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(match &reading.body {
                                BodyKind::Html { view, images, .. } => {
                                    let visible_top: f32 = (-self.reading_scroll.offset().y).into();
                                    let visible_height: f32 =
                                        self.reading_scroll.bounds().size.height.into();
                                    crate::html_view::paint(
                                        view,
                                        images,
                                        visible_top,
                                        visible_height,
                                    )
                                    .into_any_element()
                                }
                                BodyKind::Plain {
                                    visible,
                                    quoted,
                                    expanded,
                                } => {
                                    let text = if *expanded {
                                        quoted
                                            .as_ref()
                                            .map(|quoted| format!("{visible}\n{quoted}"))
                                            .unwrap_or_else(|| visible.clone())
                                    } else {
                                        visible.clone()
                                    };
                                    gpui_kit::component::text::TextView::markdown(
                                        "plain-message-body",
                                        text,
                                    )
                                    .selectable(true)
                                    .scrollable(false)
                                    .font_family("Newsreader")
                                    .text_size(px(15.0))
                                    .into_any_element()
                                }
                            })
                            .when(quotes_collapsed, |this| {
                                this.child(action("•••  Show quoted text").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.expand_quotes(window, cx);
                                    }),
                                ))
                            }),
                    ),
            )
            .child({
                // Inline reply (E7.5): collapsed placeholder, expands in place, promotable.
                let base = div()
                    .flex_none()
                    .px_6()
                    .py_3()
                    .border_t_1()
                    .border_color(style::color(palette.colors.border_hairline));
                if self.inline_open {
                    base.child(
                        div()
                            .rounded(px(palette.radii.card))
                            .border_1()
                            .border_color(style::color(palette.colors.border))
                            .bg(style::color(palette.colors.card))
                            .p_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(Textarea::new(&self.inline_reply).h(px(110.0)))
                                    .child(
                                        div()
                                            .flex()
                                            .justify_end()
                                            .gap_2()
                                            .child(action("Pop out").on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, window, cx| {
                                                    this.inline_pop_out(window, cx)
                                                }),
                                            ))
                                            .child(action("Send").on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, window, cx| {
                                                    this.inline_send(window, cx)
                                                }),
                                            )),
                                    ),
                            ),
                    )
                    .into_any_element()
                } else {
                    base.child(
                        div()
                            .rounded(px(palette.radii.card))
                            .border_1()
                            .border_color(style::color(palette.colors.border))
                            .bg(style::color(palette.colors.card))
                            .px_3()
                            .py_2()
                            .text_size(px(13.0))
                            .text_color(style::color(palette.colors.faint))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.inline_open = true;
                                    let state = this.inline_reply.clone();
                                    state.update(cx, |state, cx| state.focus(window, cx));
                                    cx.notify();
                                }),
                            )
                            .child("Write a reply…"),
                    )
                    .into_any_element()
                }
            })
            .into_any_element()
    }

    fn inline_reply_draft(&self, cx: &App) -> Option<snail_core::compose::Draft> {
        let id = self.selection.cursor()?;
        let parsed = self.mail.parsed(id)?;
        let self_addr = self.mail.first_account_address();
        let mut draft = snail_core::compose::reply(&parsed, self_addr.as_deref(), false);
        if let Some(signature) = self
            .mail
            .signature_for(self_addr.as_deref())
            .or_else(|| crate::settings::signature_for(self_addr.as_deref(), cx))
        {
            draft.body_text = snail_core::compose::with_signature(&draft.body_text, &signature);
        }
        let typed = self.inline_reply.read(cx).value().to_string();
        draft.body_text = format!("{typed}{}", draft.body_text);
        Some(draft)
    }

    /// Promote the inline reply to the full compose window without losing the text (E7.5).
    fn inline_pop_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.inline_reply_draft(cx) else {
            return;
        };
        self.inline_open = false;
        let state = self.inline_reply.clone();
        state.update(cx, |state, cx| state.set_value("", window, cx));
        crate::compose::open(self.mail.clone(), draft, cx);
        cx.notify();
    }

    fn inline_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.inline_reply_draft(cx) {
            if let Ok(raw) = snail_core::compose::build_raw(&draft) {
                if self.mail.enqueue_send(&raw).is_some() {
                    self.sync.update(cx, |hub, cx| hub.sync_now(cx));
                }
            }
        }
        self.inline_open = false;
        let state = self.inline_reply.clone();
        state.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn empty_state(palette: &snail_ui::theme::Theme, cx: &App, state: EmptyState) -> AnyElement {
        let copy = snail_ui::empty::copy(state);
        let glyph = match copy.icon {
            snail_ui::empty::IconHint::Envelope => Icon::Envelope,
            snail_ui::empty::IconHint::Search => Icon::Search,
            snail_ui::empty::IconHint::CloudOff => Icon::Overflow,
            snail_ui::empty::IconHint::Alert => Icon::Bell,
            snail_ui::empty::IconHint::Calendar => Icon::Calendar,
            snail_ui::empty::IconHint::Shield => Icon::Shield,
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                icon(glyph)
                    .w(px(28.0))
                    .h(px(28.0))
                    .text_color(style::color(palette.colors.faint)),
            )
            .child(style::text(copy.title, TextRole::ListHeaderTitle, cx))
            .child(style::text(copy.body, TextRole::ReadingMeta, cx))
            .into_any_element()
    }
}

fn plain_body(text: String) -> BodyKind {
    let folded = snail_core::mime::fold_plain_quotes(&text);
    BodyKind::Plain {
        visible: folded.visible,
        quoted: folded.quoted,
        expanded: false,
    }
}

fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn short_date(epoch: Option<i64>) -> String {
    let Some(epoch) = epoch else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let delta = now - epoch;
    if delta < 60 {
        "now".into()
    } else if delta < 3600 {
        format!("{}m", delta / 60)
    } else if delta < 86_400 {
        format!("{}h", delta / 3600)
    } else if delta < 7 * 86_400 {
        format!("{}d", delta / 86_400)
    } else {
        format!("{}w", delta / (7 * 86_400))
    }
}

/// The list row's timestamp in the local zone: the time today, "Yesterday", then the date.
fn row_timestamp(epoch: i64) -> String {
    use chrono::{Local, Offset, TimeZone};
    let offset = |at: i64| {
        Local
            .timestamp_opt(at, 0)
            .single()
            .map(|local| local.offset().fix().local_minus_utc())
            .unwrap_or(0)
    };
    let now = current_epoch();
    snail_ui::dates::row_timestamp(epoch, offset(epoch), now, offset(now))
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Apply any remote images that arrived since the last frame, then re-lay out (E6.8).
        self.apply_pending_images(window, cx);
        self.apply_pending_relayout(window, cx);
        // A background search can move the cursor without a key or mouse handler; make the reading
        // follow it (E9.3).
        if self.selection.cursor().is_some() && self.reading_id != self.selection.cursor() {
            self.load_reading(window, cx);
        }
        // The undo bar auto-expires (E8.5): keep ticking while it is up, then clear it.
        if let Some(bar) = &self.undo {
            if bar.at.elapsed() >= Duration::from_secs(6) {
                self.undo = None;
            } else {
                cx.on_next_frame(window, |this, _window, cx| {
                    if this
                        .undo
                        .as_ref()
                        .is_some_and(|bar| bar.at.elapsed() >= Duration::from_secs(6))
                    {
                        this.undo = None;
                    }
                    cx.notify();
                });
            }
        }
        let palette = style::palette(cx);

        // No account yet, or a reconnect in progress: the connect flow replaces the mailbox.
        let onboarding =
            if self.workspace == WorkspaceMode::Mail && self.sync.read(cx).needs_onboarding() {
                let hub = self.sync.clone();
                Some(
                    self.onboarding
                        .get_or_insert_with(|| cx.new(|cx| Onboarding::new(hub, window, cx)))
                        .clone(),
                )
            } else {
                self.onboarding = None;
                None
            };
        let onboarding_open = onboarding.is_some();

        let overlay = if self.overlay.visible {
            self.overlay.tick();
            cx.on_next_frame(window, |this, _window, cx| {
                this.overlay.tick();
                cx.notify();
            });
            Some(
                div()
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .p_2()
                    .rounded(px(6.0))
                    .bg(rgba(0x191919e6))
                    .text_color(rgb(0xededed))
                    .text_size(px(11.0))
                    .children(
                        self.overlay
                            .lines(&crate::startup::summary())
                            .into_iter()
                            .map(|line| div().child(line)),
                    ),
            )
        } else {
            None
        };

        let undo = self
            .undo
            .as_ref()
            .filter(|bar| bar.at.elapsed() < Duration::from_secs(6))
            .map(|bar| (bar.label.clone(), !bar.ops.is_empty()));
        // The "move to mailbox" picker, floating above the undo bar (E8.4).
        let move_picker = self.move_picker.as_ref().map(|picker| {
            let mut card = div()
                .id("move-picker")
                .w(px(260.0))
                .max_h(px(380.0))
                .overflow_y_scroll()
                .rounded(px(palette.radii.card))
                .border_1()
                .border_color(style::color(palette.colors.border))
                .bg(style::color(palette.colors.card))
                .p_2()
                .flex()
                .flex_col()
                .gap_1();
            for (index, (id, name)) in picker.mailboxes.iter().enumerate() {
                let id = *id;
                let name = name.clone();
                let highlighted = index == picker.selected;
                card = card.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(palette.radii.button))
                        .when(highlighted, |this| {
                            this.bg(style::color(palette.colors.accent_tint))
                        })
                        .child(style::text(name, TextRole::SidebarItem, cx))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                this.select_move_target(id, window, cx);
                            }),
                        ),
                );
            }
            div()
                .absolute()
                .bottom(px(56.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(card)
        });
        let command_palette = self.palette_overlay(cx);
        let fallback_menu = self.fallback_menu_overlay(cx);
        let root_context = match self.workspace {
            WorkspaceMode::Mail => "Global List Reading",
            WorkspaceMode::Calendar if self.calendar.read(cx).has_editor() => {
                "Global Calendar Editor"
            }
            WorkspaceMode::Calendar => "Global Calendar",
            WorkspaceMode::Settings => "Global",
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .key_context(root_context)
            .track_focus(&self.focus)
            // gpui-base deliberately propagates its own Escape action when the input has
            // nothing internal to dismiss. Bridge that third-party action back into Snail's
            // single generic command so palettes, searches and editors close while typing.
            .on_action(cx.listener(move |this, _: &InputEscape, window, cx| {
                if this.palette_open
                    || this.move_picker.is_some()
                    || this.calendar.read(cx).has_editor()
                    || this.search.is_some()
                {
                    window.dispatch_action(Box::new(RunCommand(CommandId::Cancel)), cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(move |this, action: &RunCommand, window, cx| {
                if onboarding_open
                    && !matches!(
                        action.0,
                        CommandId::OpenPalette
                            | CommandId::PalettePrevious
                            | CommandId::PaletteNext
                            | CommandId::PaletteConfirm
                            | CommandId::Cancel
                            | CommandId::CycleAppearance
                            | CommandId::ToggleDiagnostics
                            | CommandId::ShowMail
                            | CommandId::ShowCalendar
                            | CommandId::ShowSettings
                            | CommandId::Quit
                    )
                {
                    return;
                }
                if this.execute_command(action.0, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.titlebar(palette, window, cx))
            .child(match self.workspace {
                WorkspaceMode::Calendar => {
                    div().flex().flex_1().min_h_0().child(self.calendar.clone())
                }
                WorkspaceMode::Settings => div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.settings_workspace.clone()),
                WorkspaceMode::Mail => match onboarding {
                    Some(onboarding) => div().flex().flex_1().min_h_0().child(onboarding),
                    None => div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(self.sidebar(palette, cx))
                        .child(self.list(palette, cx))
                        .child(self.reading_pane(palette, cx)),
                },
            })
            .when_some(overlay, |this, overlay| this.child(overlay))
            .when_some(fallback_menu, |this, menu| this.child(menu))
            .when_some(undo, |this, (label, has_ops)| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(16.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2()
                                .rounded(px(palette.radii.card))
                                .bg(style::color(palette.colors.ink))
                                .text_color(style::color(palette.colors.canvas))
                                .text_size(px(12.0))
                                .child(label)
                                .when(has_ops, |this| {
                                    this.child(
                                        div()
                                            .text_color(style::color(palette.colors.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, window, cx| {
                                                    this.undo_last(window, cx)
                                                }),
                                            )
                                            .child("Undo"),
                                    )
                                }),
                        ),
                )
            })
            .when_some(move_picker, |this, picker| this.child(picker))
            .when_some(command_palette, |this, palette| this.child(palette))
    }
}
