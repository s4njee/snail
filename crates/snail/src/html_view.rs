//! The HTML message renderer (plan.md E6): sanitize → html5ever → the `snail-ui` DOM → layout →
//! GPUI elements, including inline `cid:` images (E6.8).
//!
//! The parsing lives here, not in `snail-core`, because the E6 crate-boundary decision is still
//! open: `snail-ui` owns the DOM and layout but may not depend on `snail-core`. Remote images are
//! **not** fetched — they stay blocked placeholders (E6.2); only `cid:` parts, whose bytes are
//! already in the cached raw MIME, are decoded and painted.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use snail_ui::html::{apply_inline_style, resolve_style, Element, FontSpec, Node, Style};
use snail_ui::layout::{self, Fragment, TextMeasure};

/// A decoded inline image, with the size it should occupy.
struct Placed {
    width: f32,
    height: f32,
    render: Arc<RenderImage>,
}

/// The images a document may reference, keyed by content id.
#[derive(Default)]
pub struct Images {
    map: HashMap<String, Placed>,
}

impl Images {
    /// Decode every inline (`cid:`) image from the raw MIME (E6.8). Remote `http(s)` images are
    /// deliberately absent, so they render as reserved-size placeholders (E6.2).
    pub fn from_message(raw: &[u8]) -> Self {
        let mut map = HashMap::new();
        for inline in snail_core::mime::inline_images(raw) {
            if let Some(placed) = decode(&inline.bytes) {
                map.insert(inline.content_id, placed);
            }
        }
        Self { map }
    }

    fn resolve(&self, src: &str) -> Option<&Placed> {
        let key = src
            .strip_prefix("cid:")
            .unwrap_or(src)
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        self.map.get(key)
    }

    fn size(&self, src: &str) -> Option<(f32, f32)> {
        self.resolve(src).map(|placed| (placed.width, placed.height))
    }
}

fn decode(bytes: &[u8]) -> Option<Placed> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let mut rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    // gpui's RenderImage buffer is BGRA; the image crate decodes to RGBA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let frame = image::Frame::new(rgba);
    let render = Arc::new(RenderImage::new(vec![frame]));
    Some(Placed {
        width: width as f32,
        height: height as f32,
        render,
    })
}

/// A laid-out HTML document, in the display-list form the painter consumes.
pub struct HtmlView {
    fragments: Vec<Fragment>,
    width: f32,
    height: f32,
}

/// Sanitize, parse and lay out `html` at `width`, measuring text with the window's text system.
pub fn layout_html(html: &str, width: f32, window: &Window, images: &Images) -> HtmlView {
    let dom = parse_to_dom(html, images);
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
pub fn paint(view: &HtmlView, images: &Images) -> AnyElement {
    div()
        .relative()
        .w(px(view.width))
        .h(px(view.height))
        .children(view.fragments.iter().map(|fragment| paint_fragment(fragment, images)))
        .into_any_element()
}

const DEFAULT_IMAGE: (f32, f32) = (240.0, 140.0);

fn paint_fragment(fragment: &Fragment, images: &Images) -> AnyElement {
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
        Fragment::Image { rect, src } => match images.resolve(src) {
            // A real inline image (E6.8).
            Some(placed) => img(ImageSource::Render(placed.render.clone()))
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.w))
                .h(px(rect.h))
                .into_any_element(),
            // A blocked remote image or a missing part: reserved size, no fetch (E6.2).
            None => div()
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
                .child(if src.is_empty() {
                    "[image]".to_string()
                } else {
                    format!("[image blocked] {}", short(src))
                })
                .into_any_element(),
        },
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

fn parse_to_dom(html: &str, images: &Images) -> Vec<Node> {
    let sheet = parse_sheet(&extract_styles(html));
    let sanitized = sanitize(html);
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(sanitized);
    let mut roots = Vec::new();
    let parent = Style::default();
    let body = find_body(&dom.document);
    build_children(
        body.as_ref().unwrap_or(&dom.document),
        &parent,
        images,
        &sheet,
        &mut roots,
    );
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

fn build_children(
    handle: &Handle,
    parent: &Style,
    images: &Images,
    sheet: &[Rule],
    out: &mut Vec<Node>,
) {
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

                // Precedence (E6.3/E6.4): tag + presentational attrs, then <style>/class rules in
                // specificity order, then the inline style attribute. resolve_style applies the
                // inline style itself, so it is withheld here and applied last.
                let mut presentational = map.clone();
                presentational.remove("style");
                let mut style = resolve_style(&tag, &presentational, parent);
                apply_sheet(&mut style, &tag, &map, sheet);
                if let Some(inline) = map.get("style") {
                    apply_inline_style(&mut style, inline);
                }

                // Reserve an inline image's real size so unblocking never reflows (E6.8) — but only
                // when the message did not specify one.
                if tag == "img" && (style.width.is_none() || style.height.is_none()) {
                    let (width, height) = map
                        .get("src")
                        .and_then(|src| images.size(src))
                        .unwrap_or(DEFAULT_IMAGE);
                    style.width = style.width.or(Some(width));
                    style.height = style.height.or(Some(height));
                }

                let mut children = Vec::new();
                build_children(child, &style, images, sheet, &mut children);
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

// --- The `<style>` subset (E6.3/E6.4) --------------------------------------------------------

/// One parsed rule. Selectors are approximated: descendant combinators are ignored and only the
/// last compound is matched, which is the common case in mail.
struct Rule {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    declarations: String,
    specificity: u32,
}

/// Concatenate every `<style>` block's contents. Ammonia strips the tags anyway, so this runs first.
fn extract_styles(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut cursor = 0;
    while let Some(open) = lower[cursor..].find("<style") {
        let open = cursor + open;
        let Some(gt) = lower[open..].find('>') else { break };
        let start = open + gt + 1;
        let Some(close) = lower[start..].find("</style") else { break };
        let end = start + close;
        out.push_str(&html[start..end]);
        out.push('\n');
        cursor = end;
    }
    out
}

fn parse_sheet(css: &str) -> Vec<Rule> {
    // Drop comments.
    let mut cleaned = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        cleaned.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    cleaned.push_str(rest);

    let mut rules = Vec::new();
    for block in cleaned.split('}') {
        let Some((selectors, declarations)) = block.split_once('{') else {
            continue;
        };
        let declarations = declarations.trim();
        if declarations.is_empty() {
            continue;
        }
        for selector in selectors.split(',') {
            let selector = selector.trim();
            if selector.is_empty() || selector.contains('@') {
                continue;
            }
            // Only the last compound of a descendant selector is matched.
            let compound = selector.split_whitespace().last().unwrap_or(selector);
            let mut tag = None;
            let mut id = None;
            let mut classes = Vec::new();
            for part in compound.split_inclusive(['.', '#']) {
                let token = part.trim().to_string();
                if token.is_empty() {
                    continue;
                }
                if let Some(rest) = token.strip_prefix('.') {
                    classes.push(rest.trim_end_matches('.').to_string());
                } else if let Some(rest) = token.strip_prefix('#') {
                    id = Some(rest.trim_end_matches('#').to_string());
                } else {
                    let name = token.trim_end_matches(['.', '#']).to_string();
                    if !name.is_empty() {
                        tag = Some(name);
                    }
                }
            }
            let specificity = (id.is_some() as u32) * 100
                + classes.len() as u32 * 10
                + tag.is_some() as u32;
            rules.push(Rule {
                tag,
                id,
                classes,
                declarations: declarations.to_string(),
                specificity,
            });
        }
    }
    rules
}

fn apply_sheet(style: &mut Style, tag: &str, attrs: &HashMap<String, String>, sheet: &[Rule]) {
    let id = attrs.get("id").map(String::as_str).unwrap_or("");
    let class_attr = attrs.get("class").map(String::as_str).unwrap_or("");
    let classes: Vec<&str> = class_attr.split_whitespace().collect();

    let mut matching: Vec<&Rule> = sheet
        .iter()
        .filter(|rule| {
            rule.tag
                .as_deref()
                .is_none_or(|expected| expected.eq_ignore_ascii_case(tag))
                && rule.id.as_deref().is_none_or(|expected| expected == id)
                && rule
                    .classes
                    .iter()
                    .all(|class| classes.iter().any(|candidate| candidate == class))
        })
        .collect();
    matching.sort_by_key(|rule| rule.specificity);
    for rule in matching {
        apply_inline_style(style, &rule.declarations);
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
