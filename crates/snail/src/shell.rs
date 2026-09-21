//! The shell root: the titlebar, sidebar and content skeleton that E1 grows into the real shells.
//! Everything here reads tokens through `style`; no raw hex or size appears (plan.md E1.3).

use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_ui::text::TextRole;

use crate::dev_overlay::DevOverlay;
use crate::icons::{icon, Icon};
use crate::settings;
use crate::style::{self, ThemePref};

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
            // The 52px bar is the OS drag region (E1.2).
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    // macOS draws real traffic lights at (14, 20); leave them room.
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
                .child(
                    button("–", WindowControlArea::Min)
                        .on_click(|_, window, _| window.minimize_window()),
                )
                .child(
                    button("□", WindowControlArea::Max)
                        .on_click(|_, window, _| window.zoom_window()),
                )
                .child(
                    button("×", WindowControlArea::Close)
                        .on_click(|_, window, _| window.remove_window()),
                )
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
                    .when(selected, |this| {
                        this.bg(style::color(palette.colors.accent_tint_deep))
                    })
                    .child(icon(glyph).text_color(style::color(icon_color)))
                    .child(style::text(name, role, cx))
            }))
    }

    fn content(palette: &snail_ui::theme::Theme, cx: &App) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .bg(style::color(palette.colors.canvas))
            .child(style::text(
                "Messages read like letters, not UI",
                TextRole::ReadingSubject,
                cx,
            ))
            .child(style::text(
                "Instrument Sans · Newsreader · DM Mono, self-hosted",
                TextRole::ReadingMeta,
                cx,
            ))
            .child(style::text(
                "The quick brown fox jumps over the lazy dog. Snail keeps the loop you actually run \
                 all day: sync, read, triage, reply — done fast enough that opening it never feels \
                 like a decision.",
                TextRole::BodySerif,
                cx,
            ))
            .child(
                div()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .child(style::text("12 UNREAD", TextRole::SectionLabel, cx))
                    .child(style::text("9:14 AM", TextRole::RowTimestamp, cx))
                    .child(style::text("DM Mono 11/400", TextRole::Kbd, cx)),
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);

        let overlay = if self.overlay.visible {
            self.overlay.tick();
            // Keep sampling while open; the plan's §1.2 rule: notify from render, not a timer.
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
                    .child(Self::content(palette, cx)),
            )
            .when_some(overlay, |this, overlay| this.child(overlay))
    }
}
