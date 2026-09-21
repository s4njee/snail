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
use crate::icons::{Icon, icon};
use crate::mail_model::{MailModel, MailboxRow, MessageRow};
use crate::settings;
use crate::style::{self, ThemePref};

const ROW_HEIGHT: f32 = 84.0;
const PAGE: u32 = 200;

struct Reading {
    subject: String,
    from: String,
    sender_addr: Option<String>,
    meta: String,
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
    /// Remote images are blocked by default (E6.2); F4 or `SNAIL_REMOTE_IMAGES=1` unblocks.
    remote_enabled: bool,
    pending_remote: Vec<RemoteImage>,
    generation: u64,
    reading_scroll: ScrollHandle,
    /// Set when the user clicks "Load images" for the current message (E6.2), independent of the
    /// global switch and the per-sender allowance.
    forced_message: Option<i64>,
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
            remote_enabled: std::env::var_os("SNAIL_REMOTE_IMAGES").is_some(),
            pending_remote: Vec::new(),
            generation: 0,
            reading_scroll: ScrollHandle::new(),
            forced_message: None,
        };
        shell.reload(window, cx);
        shell
    }

    /// Read mailboxes and the selected mailbox's page. Local and fast; E5.12 moves it off-thread.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.load_reading(window, cx);
    }

    fn select_mailbox(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_mailbox = index;
        self.selection = Selection::new();
        self.reload(window, cx);
    }

    fn load_reading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.pending_remote.clear();
        self.reading_scroll.set_offset(point(px(0.0), px(0.0)));
        self.reading = self
            .selection
            .cursor()
            .and_then(|id| self.read_message(id, window, cx));

        // If a message has remote images and the user has unblocked them, fetch on the background
        // executor; the result lands in `pending_remote` and is applied on the next render (E6.8).
        let sender_allowed = self
            .reading
            .as_ref()
            .and_then(|reading| reading.sender_addr.as_deref())
            .is_some_and(|sender| settings::always_load_images_from(sender, cx));
        if !self.remote_enabled && !sender_allowed && self.forced_message != self.selection.cursor() {
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

    /// Turn fetched remote images into gpui images and re-lay out the reading pane. Runs in render,
    /// where the window is available for text metrics.
    fn apply_pending_images(&mut self, window: &mut Window, cx: &mut App) {
        if self.pending_remote.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_remote);
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
                640.0,
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
                // 640px is roughly the reading pane's content width at the default window size.
                let document = crate::html_view::prepare_document(&html);
                let has_quotes = crate::html_view::has_quoted_content(&document);
                match crate::html_view::layout_document(
                    &document,
                    640.0,
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
            body,
        })
    }

    fn expand_quotes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    640.0,
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
        let subject = snail_ui::preview::clamp(&subject, 1, 300.0, &|line| {
            line.chars().count() as f32 * 7.0
        });
        let preview = snail_ui::preview::clamp(&preview, 2, 300.0, &|line| {
            line.chars().count() as f32 * 6.5
        });

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

    fn compose_reply(&mut self, all: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selection.cursor() else {
            return;
        };
        let Some(parsed) = self.mail.parsed(id) else {
            return;
        };
        let self_addr = self.mail.first_account_address();
        let draft = snail_core::compose::reply(&parsed, self_addr.as_deref(), all);
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
                    .child(style::text(
                        reading.subject.clone(),
                        TextRole::ReadingSubject,
                        cx,
                    ))
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
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
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
            .child(
                div()
                    .id("reading-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.reading_scroll)
                    .px_6()
                    .py_5()
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
            .into_any_element()
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
                        this.load_reading(window, cx);
                    }
                    "down" => {
                        this.selection.move_by(1, &ids);
                        this.load_reading(window, cx);
                    }
                    // F4 toggles remote images (E6.2's unblock), for now a global switch.
                    "f4" => {
                        this.remote_enabled = !this.remote_enabled;
                        this.load_reading(window, cx);
                    }
                    "f5" => {
                        settings::toggle_body_preference(cx);
                        this.load_reading(window, cx);
                    }
                    // Compose / reply / forward (E7).
                    "c" => crate::compose::open(this.mail.clone(), Default::default(), cx),
                    "r" => this.compose_reply(event.keystroke.modifiers.shift, cx),
                    "f" => this.compose_forward(cx),
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
