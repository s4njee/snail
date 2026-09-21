//! The shell: titlebar, the unified sidebar, and the mail three-pane frame over the real store
//! (plan.md E1.8/E5). First cut: the store is read on the main thread at construction and on
//! mailbox changes (cheap local queries); the background-executor move and the paint budget are
//! E5.12's job.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_ui::empty::EmptyState;
use snail_ui::selection::Selection;
use snail_ui::text::TextRole;

use crate::dev_overlay::DevOverlay;
use crate::icons::{icon, Icon};
use crate::mail_model::{MailModel, MailboxRow, MessageRow};
use crate::settings;
use crate::style::{self, ThemePref};

const ROW_HEIGHT: f32 = 84.0;
const PAGE: u32 = 200;

struct Reading {
    subject: String,
    from: String,
    meta: String,
    body: BodyKind,
}

enum BodyKind {
    Plain(String),
    Html(crate::html_view::HtmlView, crate::html_view::Images),
}

pub struct Shell {
    focus: FocusHandle,
    overlay: DevOverlay,
    _appearance: Subscription,
    mail: MailModel,
    mailboxes: Vec<MailboxRow>,
    selected_mailbox: usize,
    rows: Arc<Vec<MessageRow>>,
    selection: Selection,
    reading: Option<Reading>,
}

impl Shell {
    pub fn new(mail: MailModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let appearance = cx.observe_window_appearance(window, |_this, _window, cx| {
            if settings::pref(cx) == ThemePref::System {
                style::apply(ThemePref::System, cx);
                cx.notify();
            }
        });
        let mut shell = Self {
            focus,
            overlay: DevOverlay::new(),
            _appearance: appearance,
            mail,
            mailboxes: Vec::new(),
            selected_mailbox: 0,
            rows: Arc::new(Vec::new()),
            selection: Selection::new(),
            reading: None,
        };
        shell.reload(window);
        shell
    }

    /// Read mailboxes and the selected mailbox's page. Local and fast; E5.12 moves it off-thread.
    fn reload(&mut self, window: &mut Window) {
        self.mailboxes = self.mail.mailboxes();
        if self.selected_mailbox >= self.mailboxes.len() {
            self.selected_mailbox = 0;
        }
        let rows = self
            .mailboxes
            .get(self.selected_mailbox)
            .map(|mailbox| self.mail.page(mailbox.id, PAGE))
            .unwrap_or_default();
        self.rows = Arc::new(rows);
        log::info!(
            "shell: {} mailboxes; {} messages in mailbox #{}",
            self.mailboxes.len(),
            self.rows.len(),
            self.selected_mailbox
        );
        let ids: Vec<i64> = self.rows.iter().map(|row| row.id).collect();
        self.selection.reconcile(&ids);
        if self.selection.cursor().is_none() {
            if let Some(first) = ids.first() {
                self.selection.select_in(&ids, *first);
            }
        }
        self.load_reading(window);
    }

    fn select_mailbox(&mut self, index: usize, window: &mut Window) {
        self.selected_mailbox = index;
        self.selection = Selection::new();
        self.reload(window);
    }

    fn load_reading(&mut self, window: &mut Window) {
        self.reading = self.selection.cursor().and_then(|id| self.read_message(id, window));
    }

    fn read_message(&self, id: i64, window: &mut Window) -> Option<Reading> {
        let row = self.mail.message(id)?;
        let parsed = self.mail.parsed(id)?;
        let body = match parsed.body {
            snail_core::mime::Body::Html(html) => {
                let images = match self.mail.raw(id) {
                    Some(raw) => crate::html_view::Images::from_message(&raw),
                    None => crate::html_view::Images::default(),
                };
                // 640px is roughly the reading pane's content width at the default window size.
                let view = crate::html_view::layout_html(&html, 640.0, window, &images);
                BodyKind::Html(view, images)
            }
            _ => BodyKind::Plain(
                parsed
                    .plain
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "(This message has no readable text part.)".to_string()),
            ),
        };
        Some(Reading {
            subject: row.subject.unwrap_or_else(|| "(no subject)".into()),
            from: row
                .from_name
                .or(row.from_addr)
                .unwrap_or_else(|| "(unknown sender)".into()),
            meta: row
                .date
                .map(|date| format!("{date}"))
                .unwrap_or_else(|| "(no date)".into()),
            body,
        })
    }

    fn titlebar(palette: &snail_ui::theme::Theme, _window: &mut Window, cx: &App) -> AnyElement {
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
                    .child(style::text("Inbox", TextRole::SectionLabel, cx)),
            )
            .child(Self::window_controls(palette, cx))
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
                    .px_2()
                    .pb_2()
                    .child(style::text("Mailboxes", TextRole::SectionLabel, cx)),
            )
            .children(mailboxes.into_iter().enumerate().map(|(index, mailbox)| {
                let selected = index == self.selected_mailbox;
                Self::sidebar_item(palette, cx, index, mailbox, selected)
            }))
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
            .when(selected, |this| this.bg(style::color(palette.colors.accent_tint_deep)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, window, cx| {
                    this.select_mailbox(index, window);
                    cx.notify();
                }),
            )
            .child(icon(glyph).text_color(style::color(icon_color)))
            .child(div().flex_1().min_w_0().child(style::text(mailbox.name, role, cx)))
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
        let rows = self.rows.clone();
        let selection = self.selection.clone();
        let row_ids = Arc::new(self.rows.iter().map(|row| row.id).collect::<Vec<i64>>());
        let weak = cx.entity().downgrade();
        let count = self.rows.len();
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

        div()
            .flex_none()
            .w(px(palette.metrics.list_w))
            .h_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .border_r_1()
            .border_color(style::color(palette.colors.border_soft))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_hairline))
                    .child(style::text(title, TextRole::ListHeaderTitle, cx))
                    .child(style::text(
                        format!("{unread} unread"),
                        TextRole::ListHeaderMeta,
                        cx,
                    )),
            )
            .child(if count == 0 {
                div()
                    .flex_1()
                    .child(Self::empty_state(palette, cx, EmptyState::EmptyMailbox))
                    .into_any_element()
            } else {
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
            })
            .into_any_element()
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
        let sender = row
            .from_name
            .clone()
            .or_else(|| row.from_addr.clone())
            .unwrap_or_else(|| "(unknown)".into());
        let subject = row.subject.clone().unwrap_or_else(|| "(no subject)".into());
        let preview = row.preview.clone().unwrap_or_default();
        let unread = row.unread;
        let subject = snail_ui::preview::clamp(&subject, 1, 300.0, &|line| line.chars().count() as f32 * 7.0);
        let preview = snail_ui::preview::clamp(&preview, 2, 300.0, &|line| line.chars().count() as f32 * 6.5);

        let base = div()
            .relative()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .when(selected, |this| this.bg(style::color(palette.colors.accent_tint)))
            .when(!selected, |this| this.hover(|s| s.bg(rgba(0x00000008))))
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                weak.update(cx, |this, cx| {
                    this.selection.select_in(&ids, id);
                    this.load_reading(window);
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
                .gap_1()
                .child(style::text(
                    sender,
                    if unread {
                        TextRole::RowSenderUnread
                    } else {
                        TextRole::RowSenderRead
                    },
                    cx,
                ))
                .child(style::text(
                    subject,
                    if unread {
                        TextRole::RowSubjectUnread
                    } else {
                        TextRole::RowSubjectRead
                    },
                    cx,
                ))
                .child(style::text(preview, TextRole::RowPreview, cx)),
        )
        .into_any_element()
    }

    fn reading_pane(&self, palette: &snail_ui::theme::Theme, cx: &App) -> AnyElement {
        let Some(reading) = &self.reading else {
            return Self::empty_state(palette, cx, EmptyState::EmptyMailbox);
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_hairline))
                    .child(style::text(reading.subject.clone(), TextRole::ReadingSubject, cx))
                    .child(
                        div()
                            .flex()
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
                                    .child(style::text(initials(&reading.from), TextRole::MailboxPill, cx)),
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
                            ),
                    ),
            )
            .child(
                div()
                    .id("reading-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .py_5()
                    .child(match &reading.body {
                        BodyKind::Html(view, images) => {
                            crate::html_view::paint(view, images).into_any_element()
                        }
                        BodyKind::Plain(text) => {
                            style::text(text.clone(), TextRole::BodySerif, cx).into_any_element()
                        }
                    }),
            )
            .into_any_element()
    }

    fn empty_state(
        palette: &snail_ui::theme::Theme,
        cx: &App,
        state: EmptyState,
    ) -> AnyElement {
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

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);

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
                    .bg(rgba(0x1b1917e6))
                    .text_color(rgb(0xf0ece6))
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

        let ids: Vec<i64> = self.rows.iter().map(|row| row.id).collect();
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "f2" => this.overlay.toggle(),
                    "f3" => {
                        let next = match settings::pref(cx) {
                            ThemePref::System => ThemePref::Light,
                            ThemePref::Light => ThemePref::Dark,
                            ThemePref::Dark => ThemePref::System,
                        };
                        settings::set(next, cx);
                    }
                    "up" => {
                        this.selection.move_by(-1, &ids);
                        this.load_reading(window);
                    }
                    "down" => {
                        this.selection.move_by(1, &ids);
                        this.load_reading(window);
                    }
                    _ => return,
                }
                cx.notify();
            }))
            .child(Self::titlebar(palette, window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(palette, cx))
                    .child(self.list(palette, cx))
                    .child(self.reading_pane(palette, cx)),
            )
            .when_some(overlay, |this, overlay| this.child(overlay))
    }
}
