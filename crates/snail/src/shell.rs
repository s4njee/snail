//! The shell root. Nothing but the warm mail ground for E0.1; E1 builds the real shells.

use gpui_kit::*;

pub struct Shell;

impl Shell {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Mail ground, the design language the calendar handoff is restated into (plan.md §1.3).
        div().size_full().bg(rgb(0xfaf8f5))
    }
}
