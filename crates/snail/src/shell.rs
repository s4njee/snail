//! The shell: titlebar, the unified sidebar, and the mail three-pane frame (plan.md E1.8). Real
//! data arrives in E4/E5; the rows here are representative so the geometry and roles are visible.
//! Everything reads tokens through `style`; no raw hex or size appears (E1.3).

use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_ui::empty::{self, EmptyState, IconHint};
use snail_ui::text::TextRole;

use crate::dev_overlay::DevOverlay;
use crate::icons::{icon, Icon};
use crate::settings;
use crate::style::{self, ThemePref};

/// sender, subject, preview, unread. Replace with the store in E5.
const MESSAGES: &[(&str, &str, &str, bool)] = &[
    (
        "Priya Raman",
        "Design review Thursday",
        "I moved the review to Thursday so the tokens are merged first. Anything you want on the agenda?",
        true,
    ),
    (
        "GitHub",
        "[snail] CI failed on main",
        "Run 35553496010: spikes (ubuntu-latest) — Build E0.2. Open the log for details.",
        true,
    ),
    (
        "Maya Okonkwo",
        "Re: Almanac calendar geometry",
        "The 6×7 grid maths checks out at 27px. I left two notes on the overlap case.",
        false,
    ),
    (
        "Apple",
        "An app-specific password was generated",
        "If you did not do this, change your password immediately.",
        false,
    ),
    (
        "Stripe",
        "Your payout is on the way",
        "A payout of $1,240.00 should arrive in two business days.",
        false,
    ),
];

pub struct Shell {
    focus: FocusHandle,
    overlay: DevOverlay,
    /// Kept, not dropped: a dropped subscription cancels the OS appearance feed (E1.12).
    _appearance: Subscription,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // The shell takes focus at launch, or single-letter shortcuts dispatch above it (E15.5).
        window.focus(&focus, cx);
        // While set to System, follow the OS flipping light/dark under us (E1.12).
        let appearance = cx.observe_window_appearance(window, |_this, _window, cx| {
            if settings::pref(cx) == ThemePref::System {
                style::apply(ThemePref::System, cx);
                cx.notify();
            }
        });
        Self {
            focus,
            overlay: DevOverlay::new(),
            _appearance: appearance,
        }
    }

    fn titlebar(palette: &snail_ui::theme::Theme, _window: &mut Window, cx: &App) -> impl IntoElement {
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
                    .child(style::text("E1 · design system", TextRole::SectionLabel, cx)),
            )
            .child(Self::window_controls(palette, cx))
    }

    /// macOS uses the system traffic lights; Windows and Linux get drawn controls (E1.2/E18.5).
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
                .child(button("–", WindowControlArea::Min).on_click(|_, window, _| window.minimize_window()))
                .child(button("□", WindowControlArea::Max).on_click(|_, window, _| window.zoom_window()))
                .child(button("×", WindowControlArea::Close).on_click(|_, window, _| window.remove_window()))
                .into_any_element()
        }
    }

    fn sidebar(palette: &snail_ui::theme::Theme, cx: &App) -> impl IntoElement {
        let items = [
            ("Inbox", Icon::Inbox),
            ("Sent", Icon::Send),
            ("Drafts", Icon::Document),
            ("Archive", Icon::Archive),
            ("Trash", Icon::Trash),
        ];
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
            .children(items.into_iter().enumerate().map(|(index, (name, glyph))| {
                let selected = index == 0;
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
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(palette.radii.button))
                    .when(selected, |this| this.bg(style::color(palette.colors.accent_tint_deep)))
                    .child(icon(glyph).text_color(style::color(icon_color)))
                    .child(style::text(name, role, cx))
            }))
    }

    fn mail_list(palette: &snail_ui::theme::Theme, cx: &App) -> impl IntoElement {
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
                    .child(style::text("Inbox", TextRole::ListHeaderTitle, cx))
                    .child(style::text("12 unread", TextRole::ListHeaderMeta, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .children(MESSAGES.iter().enumerate().map(|(index, message)| {
                        Self::message_row(palette, cx, index, *message)
                    })),
            )
    }

    fn message_row(
        palette: &snail_ui::theme::Theme,
        cx: &App,
        index: usize,
        (sender, subject, preview, unread): (&str, &str, &str, bool),
    ) -> impl IntoElement {
        let selected = index == 0;
        let row = div()
            .relative()
            .flex()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(style::color(palette.colors.border_hairline))
            .when(selected, |this| this.bg(style::color(palette.colors.accent_tint)))
            .when(!selected, |this| {
                this.hover(|s| s.bg(rgba(0x00000008)))
            });
        row.when(selected, |this| {
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
        // Unread dot, or the same 7px of empty space so the text stays aligned (E5.2).
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(style::text(
                            sender,
                            if unread {
                                TextRole::RowSenderUnread
                            } else {
                                TextRole::RowSenderRead
                            },
                            cx,
                        ))
                        .child(style::text("9:14 AM", TextRole::RowTimestamp, cx)),
                )
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
    }

    fn reading_pane(palette: &snail_ui::theme::Theme, cx: &App) -> impl IntoElement {
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
                        "Design review Thursday",
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
                                    .child(style::text("PR", TextRole::MailboxPill, cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(style::text(
                                        "Priya Raman",
                                        TextRole::ReadingSenderName,
                                        cx,
                                    ))
                                    .child(style::text(
                                        "to me · 9:14 AM",
                                        TextRole::ReadingMeta,
                                        cx,
                                    )),
                            )
                            .child(div().flex_1())
                            .child(Self::outlined_button(palette, cx, "Reply"))
                            .child(Self::outlined_button(palette, cx, "Forward")),
                    ),
            )
            .child(
                div().flex_1().min_h_0().px_6().py_5().child(style::text(
                    "I moved the review to Thursday so the tokens are merged first. \
                     Anything you want on the agenda?",
                    TextRole::BodySerif,
                    cx,
                )),
            )
    }

    fn outlined_button(
        palette: &snail_ui::theme::Theme,
        cx: &App,
        label: &'static str,
    ) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(px(palette.radii.button))
            .border_1()
            .border_color(style::color(palette.colors.border_strong))
            .hover(|s| s.border_color(style::color([0x00, 0x00, 0x00, 51])))
            .child(style::text(label, TextRole::ButtonLabel, cx))
    }

    /// E1.10's states, drawn from `snail_ui::empty`.
    fn empty_state(
        palette: &snail_ui::theme::Theme,
        cx: &App,
        state: EmptyState,
    ) -> impl IntoElement {
        let copy = empty::copy(state);
        let glyph = match copy.icon {
            IconHint::Envelope => Icon::Envelope,
            IconHint::Search => Icon::Search,
            IconHint::CloudOff => Icon::Overflow,
            IconHint::Alert => Icon::Bell,
            IconHint::Calendar => Icon::Calendar,
            IconHint::Shield => Icon::Shield,
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(style::color(palette.colors.canvas))
            .child(
                icon(glyph)
                    .w(px(28.0))
                    .h(px(28.0))
                    .text_color(style::color(palette.colors.faint)),
            )
            .child(style::text(copy.title, TextRole::ListHeaderTitle, cx))
            .child(style::text(copy.body, TextRole::ReadingMeta, cx))
    }
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

        // `SNAIL_EMPTY=mailbox|offline|…` shows one of the undesigned states for review (E1.10).
        let demo_state = std::env::var("SNAIL_EMPTY").ok().and_then(|name| match name.as_str() {
            "no-accounts" => Some(EmptyState::NoAccounts),
            "first-sync" => Some(EmptyState::FirstSync),
            "mailbox" => Some(EmptyState::EmptyMailbox),
            "no-results" => Some(EmptyState::NoSearchResults),
            "body-failed" => Some(EmptyState::BodyFailed),
            "offline" => Some(EmptyState::Offline),
            "sync-error" => Some(EmptyState::SyncError),
            "calendar" => Some(EmptyState::EmptyCalendarRange),
            _ => None,
        });

        let content = if let Some(state) = demo_state {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(Self::empty_state(palette, cx, state))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(Self::mail_list(palette, cx))
                .child(Self::reading_pane(palette, cx))
                .into_any_element()
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    // F2 toggles the dev overlay; F3 cycles System → Light → Dark (E1.12).
                    "f2" => this.overlay.toggle(),
                    "f3" => {
                        let next = match settings::pref(cx) {
                            ThemePref::System => ThemePref::Light,
                            ThemePref::Light => ThemePref::Dark,
                            ThemePref::Dark => ThemePref::System,
                        };
                        settings::set(next, cx);
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
                    .child(Self::sidebar(palette, cx))
                    .child(content),
            )
            .when_some(overlay, |this, overlay| this.child(overlay))
    }
}
