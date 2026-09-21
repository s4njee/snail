//! E0.3 layout prototype — the salvageable half of the "quick prototype": a minimal block / inline
//! / table layout in `snail-ui`, painted here so the difference against `TextView::html` is visible.
//!
//! Run: `cargo run --features gui --bin e0-3-layout -- [--raw] [dir]`
//!
//! The pipeline mirrors plan.md E6: parse MIME → sanitize (ammonia, **keeping the supported style
//! subset**) → html5ever → `snail-ui` DOM → `snail-ui` layout → GPUI fragments.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use html5ever::tendril::TendrilSink;
use mail_parser::MessageParser;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use snail_ui::html::{resolve_style, FontSpec, Node, Style};
use snail_ui::layout::{self, Fragment};

fn main() {
    let _ = env_logger::try_init();
    let raw = std::env::args().any(|arg| arg == "--raw");
    let dir = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if raw {
                PathBuf::from("../corpus")
            } else {
                PathBuf::from("../corpus/sanitized")
            }
        });

    let docs = load_docs(&dir, raw);
    println!("loaded {} documents from {}", docs.len(), dir.display());

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(900.0), px(820.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(620.0), px(520.0))),
                    window_decorations: Some(WindowDecorations::Client),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let page = cx.new(|cx| Page::new(docs, window, cx));
                    let focus = page.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    cx.new(|cx| Root::new(page, window, cx))
                },
            )
            .expect("the layout window opens");
            cx.activate(true);
        });
}

struct Page {
    docs: Vec<(String, String)>,
    index: usize,
    title: String,
    fragments: Vec<Fragment>,
    page_width: f32,
    page_height: f32,
    focus: FocusHandle,
}

impl Page {
    fn new(docs: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            docs,
            index: 0,
            title: String::new(),
            fragments: Vec::new(),
            page_width: 820.0,
            page_height: 0.0,
            focus: cx.focus_handle(),
        };
        page.relayout(window);
        page
    }

    fn relayout(&mut self, window: &mut Window) {
        let Some((name, html)) = self.docs.get(self.index) else {
            self.title = "(empty)".to_string();
            self.fragments = Vec::new();
            return;
        };
        self.title = format!("[{}/{}]  {}    —  ← → to switch", self.index + 1, self.docs.len(), name);

        let dom = parse_to_dom(html);
        let measure = GpuiMeasure {
            text_system: window.text_system(),
        };
        let result = layout::layout(&dom, self.page_width, &measure);
        self.page_height = result.height.max(1.0);
        self.fragments = result.fragments;
    }

    fn step(&mut self, delta: isize, window: &mut Window) {
        if self.docs.is_empty() {
            return;
        }
        let len = self.docs.len() as isize;
        self.index = (((self.index as isize + delta) % len + len) % len) as usize;
        self.relayout(window);
    }
}

impl Render for Page {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .relative()
            .w(px(self.page_width))
            .h(px(self.page_height + 48.0))
            .bg(rgb(0xffffff))
            .children(self.fragments.iter().map(paint_fragment));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xe8e4dc))
            .text_color(rgb(0x1b1917))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "right" | "space" | "down" => this.step(1, window),
                    "left" | "up" => this.step(-1, window),
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
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id("page-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_6()
                    .child(page),
            )
    }
}

fn paint_fragment(fragment: &Fragment) -> AnyElement {
    match fragment {
        Fragment::Rect { rect, color, radius } => div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.w))
            .h(px(rect.h))
            .bg(rgba(to_u32(*color)))
            .rounded(px(*radius))
            .into_any_element(),
        Fragment::Border {
            rect,
            color,
            width,
        } => div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.w))
            .h(px(rect.h))
            .border(px((*width).max(0.75)))
            .border_color(rgba(to_u32(*color)))
            .into_any_element(),
        Fragment::Word {
            x,
            y,
            text,
            font,
            color,
            underline,
        } => div()
            .absolute()
            .left(px(*x))
            .top(px(*y))
            .text_size(px(font.size))
            .text_color(rgba(to_u32(*color)))
            .font_weight(if font.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            })
            .when(*underline, |this| this.underline())
            .when(font.italic, |this| this.italic())
            .child(text.clone())
            .into_any_element(),
        Fragment::Image { rect, src } => div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.w))
            .h(px(rect.h))
            .bg(rgb(0xf2efe9))
            .border_1()
            .border_color(rgb(0xd9d4cc))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.0))
            .text_color(rgb(0x8a837b))
            .child(format!("[image] {}", short(src)))
            .into_any_element(),
    }
}

fn to_u32(color: [u8; 4]) -> u32 {
    (color[0] as u32) << 24 | (color[1] as u32) << 16 | (color[2] as u32) << 8 | color[3] as u32
}

fn short(input: &str) -> String {
    let cleaned = input.trim_start_matches("cid:").trim();
    if cleaned.chars().count() <= 40 {
        cleaned.to_string()
    } else {
        let tail: String = cleaned.chars().rev().take(36).collect::<String>().chars().rev().collect();
        format!("…{tail}")
    }
}

// --- HTML -> snail-ui DOM -------------------------------------------------------------------

/// Ammonia, but configured for a reader: keep the style subset 6.4 supports and the legacy
/// presentational attributes, and still strip scripts, iframes and event handlers.
fn sanitize(html: &str) -> String {
    let properties: HashSet<&str> = [
        "color",
        "background",
        "background-color",
        "font-size",
        "font-weight",
        "font-style",
        "text-decoration",
        "text-decoration-line",
        "text-align",
        "margin",
        "margin-top",
        "margin-bottom",
        "padding",
        "border",
        "border-width",
        "border-color",
        "width",
        "height",
        "display",
    ]
    .into_iter()
    .collect();
    ammonia::Builder::default()
        .add_tags([
            "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup", "col",
            "font", "center", "span", "blockquote", "pre", "h1", "h2", "h3", "h4", "h5", "h6",
        ])
        .add_generic_attributes([
            "style", "class", "align", "valign", "bgcolor", "width", "height", "colspan", "rowspan",
            "border", "cellpadding", "cellspacing", "color", "face", "size", "src", "alt",
        ])
        .filter_style_properties(properties)
        .clean(html)
        .to_string()
}

fn parse_to_dom(html: &str) -> Vec<Node> {
    let sanitized = sanitize(html);
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(sanitized);
    let mut roots = Vec::new();
    let body = find_body(&dom.document);
    let parent = Style::default();
    build_children(body.as_ref().unwrap_or(&dom.document), &parent, &mut roots);
    roots
}

fn find_body(handle: &Handle) -> Option<Handle> {
    for child in handle.children.borrow().iter() {
        if let NodeData::Element { name, .. } = &child.data {
            if name.local.as_ref() == "body" {
                return Some(child.clone());
            }
        }
        if let Some(found) = find_body(child) {
            return Some(found);
        }
    }
    None
}

fn build_children(handle: &Handle, parent: &Style, out: &mut Vec<Node>) {
    for child in handle.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = contents.borrow().to_string();
                if !text.trim().is_empty() {
                    out.push(Node::Text(text));
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.to_string();
                if matches!(tag.as_str(), "script" | "style" | "head" | "meta" | "title" | "link") {
                    continue;
                }
                let map: HashMap<String, String> = attrs
                    .borrow()
                    .iter()
                    .map(|a| (a.name.local.to_string(), a.value.to_string()))
                    .collect();
                let style = resolve_style(&tag, &map, parent);
                let mut children = Vec::new();
                build_children(child, &style, &mut children);
                out.push(Node::Element(snail_ui::html::Element {
                    tag,
                    attrs: map,
                    style,
                    children,
                }));
            }
            _ => {}
        }
    }
}

// --- documents ------------------------------------------------------------------------------

fn load_docs(dir: &PathBuf, raw: bool) -> Vec<(String, String)> {
    let mut docs = Vec::new();
    let extension = if raw { "eml" } else { "html" };
    let Ok(entries) = fs::read_dir(dir) else {
        return docs;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if raw {
            let Ok(bytes) = fs::read(&path) else { continue };
            let Some(message) = MessageParser::default().parse(&bytes) else {
                continue;
            };
            let html = if message.html_bodies().any(|part| part.is_text_html()) {
                message.body_html(0).map(|c| c.into_owned()).unwrap_or_default()
            } else if let Some(text) = message.body_text(0) {
                format!("<pre>{text}</pre>")
            } else {
                continue;
            };
            docs.push((name, html));
        } else if let Ok(html) = fs::read_to_string(&path) {
            docs.push((name, html));
        }
    }
    docs
}

// --- GPUI text metrics ----------------------------------------------------------------------

struct GpuiMeasure<'a> {
    text_system: &'a WindowTextSystem,
}

impl snail_ui::layout::TextMeasure for GpuiMeasure<'_> {
    fn width(&self, text: &str, font: &FontSpec) -> f32 {
        let gpu_font = Font {
            family: "Helvetica".into(),
            weight: if font.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if font.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..Default::default()
        };
        let run = TextRun {
            len: text.len(),
            font: gpu_font,
            color: rgb(0x000000).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = self
            .text_system
            .shape_line(text.to_string().into(), px(font.size), &[run], None);
        line.width().into()
    }
}
