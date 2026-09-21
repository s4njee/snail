//! E0.3 render — the E6.13b shortcut test: does gpui-kit's `TextView::html` already read the corpus?
//!
//! Build/run (requires the `gui` feature):
//!   cargo run --features gui --bin e0-3-render -- ../corpus/sanitized
//!
//! Left/Right (or Space) switch messages. If this reads the corpus, E6 gets a large shortcut
//! and/or 6.13's fallback renderer for free.

use std::fs;
use std::path::PathBuf;

use gpui_kit::component::text::TextView;
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

struct Renderer {
    docs: Vec<(String, String)>,
    index: usize,
    focus: FocusHandle,
}

impl Renderer {
    fn new(docs: Vec<(String, String)>, cx: &mut Context<Self>) -> Self {
        Self {
            docs,
            index: 0,
            focus: cx.focus_handle(),
        }
    }

    fn step(&mut self, delta: isize) {
        if self.docs.is_empty() {
            return;
        }
        let len = self.docs.len() as isize;
        self.index = (((self.index as isize + delta) % len + len) % len) as usize;
    }
}

impl Render for Renderer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, html) = self
            .docs
            .get(self.index)
            .cloned()
            .unwrap_or_else(|| ("(empty)".to_string(), "<p>No corpus documents found.</p>".to_string()));
        let title = format!(
            "[{}/{}]  {}    —  ← → to switch",
            self.index + 1,
            self.docs.len(),
            name
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xfaf8f5))
            .text_color(rgb(0x1b1917))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "right" | "space" | "down" => this.step(1),
                    "left" | "up" => this.step(-1),
                    _ => return,
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .bg(rgb(0xf2efe9))
                    .text_size(px(12.0))
                    .child(title),
            )
            .child(
                div().flex_1().overflow_hidden().p_4().child(
                    TextView::html(("corpus-doc", self.index), html)
                        .selectable(true)
                        .scrollable(true),
                ),
            )
    }
}

fn main() {
    let _ = env_logger::try_init();
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../corpus/sanitized"));

    let mut docs: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Ok(html) = fs::read_to_string(&path) {
                docs.push((name, html));
            }
        }
    }
    println!("loaded {} documents from {}", docs.len(), dir.display());

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(900.0), px(760.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(620.0), px(520.0))),
                    window_decorations: Some(WindowDecorations::Client),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let view = cx.new(|cx| Renderer::new(docs, cx));
                    let focus = view.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("the render window opens");
            cx.activate(true);
        });
}
