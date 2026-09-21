//! The shell root. Nothing but the warm mail ground and the dev overlay for now; E1 builds the
//! real shells (plan.md E1).

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dev_overlay::DevOverlay;

pub struct Shell {
    focus: FocusHandle,
    overlay: DevOverlay,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // The shell takes focus at launch, or single-letter shortcuts dispatch above it (E15.5).
        window.focus(&focus, cx);
        Self {
            focus,
            overlay: DevOverlay::new(),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        // Mail ground, the design language the calendar handoff is restated into (plan.md §1.3).
        div()
            .relative()
            .size_full()
            .bg(rgb(0xfaf8f5))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "f2" {
                    this.overlay.toggle();
                    cx.notify();
                }
            }))
            .when_some(overlay, |this, overlay| this.child(overlay))
    }
}
