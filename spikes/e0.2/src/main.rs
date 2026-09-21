//! E0.2 — gpui-kit `Input`/`Textarea` under real load (plan.md §3).
//!
//! A throwaway compose window: To / Cc / Subject single-line inputs, a multi-line body
//! `Textarea`, tab order, IME, select-all/copy/paste/undo, placeholder, blur/focus events, and a
//! 40-line body with soft wrap.
//!
//! **Go/no-go:** if the `Textarea` cannot carry a compose body, the fallback is writing a
//! text-input element against GPUI's `text_system` directly (~3 weeks), and that must be known now.
//!
//! Run: `cargo run` then work the checklist in `../E0.2-CHECKLIST.md`.

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

/// A body long enough to exercise soft wrap well past one screenful (the plan asks for 40 lines).
fn forty_line_body() -> String {
    (1..=40)
        .map(|i| {
            format!(
                "Line {i:02}: the quick brown fox jumps over the lazy dog, and then keeps going for \
                 a while so this line is comfortably wider than the compose pane."
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn event_name(event: &InputEvent) -> String {
    match event {
        InputEvent::Change => "Change".to_string(),
        InputEvent::PressEnter { secondary, shift } => {
            format!("PressEnter(secondary={secondary}, shift={shift})")
        }
        InputEvent::Focus => "Focus".to_string(),
        InputEvent::Blur => "Blur".to_string(),
    }
}

struct Compose {
    to: Entity<InputState>,
    cc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    events: Vec<String>,
    /// Held, not dropped: a dropped `Subscription` cancels the event feed.
    _subscriptions: Vec<Subscription>,
}

impl Compose {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let to = cx.new(|cx| InputState::new(window, cx).placeholder("To"));
        let cc = cx.new(|cx| InputState::new(window, cx).placeholder("Cc"));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Subject"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write your message…")
                .default_value(forty_line_body())
        });

        // Record the InputEvent surface actually delivered at this pin. The 0.6.4 surface is
        // `Change | PressEnter { secondary, shift } | Focus | Blur` (gpui-base) — much thinner than
        // earlier snapshots; E7 depends on knowing exactly this.
        let mut subscriptions = Vec::new();
        for (field, state) in [
            ("To", to.clone()),
            ("Cc", cc.clone()),
            ("Subject", subject.clone()),
        ] {
            subscriptions.push(cx.subscribe(&state, move |this, _state, event, _cx| {
                this.events.push(format!("{field}: {}", event_name(event)));
            }));
        }

        Self {
            to,
            cc,
            subject,
            body,
            events: Vec::new(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Compose {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut pasted = self.events.join("  |  ");
        if pasted.len() > 900 {
            pasted = pasted[pasted.len() - 900..].to_string();
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xfaf8f5))
            .text_color(rgb(0x1b1917))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    // Tab order across the four fields, exactly as a compose window needs.
                    .child(Input::new(&self.to).tab_index(0))
                    .child(Input::new(&self.cc).tab_index(1))
                    .child(Input::new(&self.subject).tab_index(2))
                    .child(
                        // A body area taller than the window, so wrap and scrolling both matter.
                        Textarea::new(&self.body).tab_index(3).h(px(420.)),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .text_size(px(11.))
                    .text_color(rgb(0x6f6963))
                    .child(pasted),
            )
    }
}

fn main() {
    let _ = env_logger::try_init();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(hack::run);
}

/// Split out only so the asset closure stays readable.
mod hack {
    use super::*;
    pub fn run(cx: &mut App) {
        gpui_kit::init(cx);

        let bounds = Bounds::centered(None, size(px(620.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(620.), px(520.))),
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            },
            |window, cx| {
                let compose = cx.new(|cx| Compose::new(window, cx));
                cx.new(|cx| Root::new(compose, window, cx))
            },
        )
        .expect("the compose window opens");

        cx.activate(true);
    }
}
