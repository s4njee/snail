//! The HTML message renderer's integration into the reading pane (plan.md E6).
//!
//! This is the E0.3 prototype promoted into the app: sanitize (ammonia, keeping the supported
//! `style` subset) → html5ever → the `snail-ui` DOM → the `snail-ui` layout → GPUI elements. The
//! parsing lives here, not in `snail-core`, because the crate-boundary decision E6 needs is still
//! open (see the plan): `snail-ui` owns the DOM and layout but may not depend on `snail-core`.
//!
//! Still to come from E6: `<style>`/class resolution (6.3/6.4), `colspan`/nested tables (6.6),
//! `cid:` images (6.8), quoted-text folding (6.9), and selection (6.10).

use std::collections::{HashMap, HashSet};

use gpui_kit::prelude::*;
use gpui_kit::*;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use snail_ui::html::{resolve_style, Element, FontSpec, Node, Style};
use snail_ui::layout::{self, Fragment, TextMeasure};

/// A laid-out HTML document, in the display-list form the painter consumes.
pub struct HtmlView {
    fragments: Vec<Fragment>,
    width: f32,
    height: f32,
}

/// Sanitize, parse and lay out `html` at `width`, measuring text with the window's text system.
pub fn layout_html(html: &str, width: f32, window: &Window) -> HtmlView {
    let dom = parse_to_dom(html);
    let measure = GpuiMeasure {
        text_system: window.text_system(),
    };
    let result = layout::layout(&dom, width, &measure);
    HtmlView {
        fragments: result.fragments,
        width,
        height: result.height,
    }
}

/// Paint the laid-out document as absolutely-positioned elements inside a relative box.
pub fn paint(view: &HtmlView) -> AnyElement {
    div()
        .relative()
        .w(px(view.width))
        .h(px(view.height))
        .children(view.fragments.iter().map(paint_fragment))
        .into_any_element()
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
        Fragment::Border { rect, color, width } => div()
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
        let tail: String = cleaned.chars().rev().take(36).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    }
}

/// Ammonia, configured for a reader: keep the style subset E6.4 supports and the legacy
/// presentational attributes, still stripping scripts, iframes and event handlers (E6.2).
fn sanitize(html: &str) -> String {
    let properties: HashSet<&str> = [
        "color", "background", "background-color", "font-size", "font-weight", "font-style",
        "text-decoration", "text-decoration-line", "text-align", "margin", "margin-top",
        "margin-bottom", "padding", "border", "border-width", "border-color", "width", "height",
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
    let parent = Style::default();
    let body = find_body(&dom.document);
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
                out.push(Node::Element(Element {
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

struct GpuiMeasure<'a> {
    text_system: &'a WindowTextSystem,
}

impl TextMeasure for GpuiMeasure<'_> {
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
