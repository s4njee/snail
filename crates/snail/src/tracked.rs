//! Tracked text (plan.md E1.5). GPUI's `TextStyle` has no tracking field (zed#16686, closed "not
//! planned"), and both handoffs letter-space their labels: wide on DM Mono micro-labels, negative
//! on display type.
//!
//! This shapes the line once and places each character at its shaped x plus the extra advance of
//! the characters before it — what CSS `letter-spacing` does. Laying the characters out as a row of
//! one-character text elements does not work: GPUI rounds each element's width up to a whole pixel
//! and the rounding accumulates.
//!
//! **Hard-scoped to short, single-line, non-editable labels.** One element per character is right
//! for "MAILBOXES" and catastrophic for a message list (plan.md §1.3).

use gpui_kit::prelude::*;
use gpui_kit::*;

/// `text` set in `font` at `size`, with `spacing` added after every character. Zero spacing is a
/// single text run, so callers can pass a role's tracking without special-casing it.
pub fn tracked(
    text: impl Into<SharedString>,
    font: Font,
    size: Pixels,
    line_height: Pixels,
    spacing: Pixels,
) -> Div {
    let text: SharedString = text.into();
    if spacing == px(0.0) {
        return div().flex().flex_none().child(text);
    }
    div().flex().flex_none().child(TrackedLine {
        text,
        font,
        size,
        line_height,
        spacing,
    })
}

#[derive(IntoElement)]
struct TrackedLine {
    text: SharedString,
    font: Font,
    size: Pixels,
    line_height: Pixels,
    spacing: Pixels,
}

impl RenderOnce for TrackedLine {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let run = TextRun {
            len: self.text.len(),
            font: self.font,
            // Shaping ignores colour; the characters inherit the caller's text style.
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(self.text.clone(), self.size, &[run], None);
        let count = self.text.chars().count() as f32;
        let spacing = self.spacing;
        // The width includes the spacing after the last character, as CSS does; negative tracking
        // can take that below zero, so clamp.
        let width = (line.width() + spacing * count).max(px(0.0));
        div()
            .relative()
            .flex_none()
            .w(width)
            .h(self.line_height)
            .children(
                self.text
                    .char_indices()
                    .enumerate()
                    .map(|(position, (byte, character))| {
                        div()
                            .absolute()
                            .top_0()
                            .left(line.x_for_index(byte) + spacing * position as f32)
                            .child(SharedString::from(character.to_string()))
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
