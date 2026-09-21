//! The HTML message renderer (plan.md E6): sanitize → html5ever → the `snail-ui` DOM → layout →
//! GPUI elements, including inline `cid:` images (E6.8).
//!
//! Untrusted parsing and sanitization live in `snail-core`; this adapter computes the restricted
//! style subset, asks `snail-ui` for framework-free layout, then paints GPUI elements.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::html::{HtmlNode as CoreNode, SanitizedDocument};
use snail_ui::html::{
    DEFAULT_FONT_FAMILY, Element, FontSpec, Node, Style, apply_inline_style, font_stack,
    resolve_style,
};
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
        self.resolve(src)
            .map(|placed| (placed.width, placed.height))
    }

    /// Add a remote image once it has been fetched and decoded (E6.8).
    pub fn insert_remote(&mut self, url: &str, width: f32, height: f32, render: Arc<RenderImage>) {
        self.map.insert(
            url.to_string(),
            Placed {
                width,
                height,
                render,
            },
        );
    }

    pub fn contains(&self, url: &str) -> bool {
        self.map.contains_key(url)
    }
}

pub fn prepare_document(html: &str) -> SanitizedDocument {
    snail_core::html::sanitize_document(html)
}

/// Decode an image to BGRA bytes plus its size, ready for gpui's `RenderImage`.
pub fn decode_to_bgra(bytes: &[u8]) -> Option<(f32, f32, Vec<u8>)> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let mut rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    // gpui's RenderImage buffer is BGRA; the image crate decodes to RGBA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some((width as f32, height as f32, rgba.into_raw()))
}

/// Build a gpui image from BGRA bytes, with a one-pixel [`EDGE_PAD`] around it.
///
/// GPUI samples images from a shared atlas with linear filtering and no gutter between tiles, so
/// an image drawn larger than its pixels blends its outermost row and column with whatever tile
/// sits next to it — thin grey or black lines along image edges that Apple Mail does not have.
/// Padding each image with a copy of its own edge moves that blend into the pad, which the
/// painter clips away.
pub fn render_image(width: f32, height: f32, bgra: Vec<u8>) -> Arc<RenderImage> {
    let (padded_width, padded_height) = (width as u32 + 2 * EDGE_PAD, height as u32 + 2 * EDGE_PAD);
    let padded = pad_edges(width as u32, height as u32, &bgra);
    let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        padded_width,
        padded_height,
        padded,
    )
    .expect("padded buffer matches its dimensions");
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// Texels of replicated edge around every image texture; see [`render_image`].
pub const EDGE_PAD: u32 = 1;

/// Surround a `width` × `height` 4-byte-per-pixel image with [`EDGE_PAD`] copies of its edge
/// pixels (clamp-to-edge, done by hand because the atlas cannot).
pub fn pad_edges(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let (width, height, pad) = (width as usize, height as usize, EDGE_PAD as usize);
    let padded_width = width + 2 * pad;
    let mut out = Vec::with_capacity(padded_width * (height + 2 * pad) * 4);
    for row in 0..height + 2 * pad {
        let source_row = row.saturating_sub(pad).min(height.saturating_sub(1));
        for column in 0..padded_width {
            let source_column = column.saturating_sub(pad).min(width.saturating_sub(1));
            let at = (source_row * width + source_column) * 4;
            out.extend_from_slice(&pixels[at..at + 4]);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<Placed> {
    let (width, height, bgra) = decode_to_bgra(bytes)?;
    Some(Placed {
        width,
        height,
        render: render_image(width, height, bgra),
    })
}

/// A laid-out HTML document, in the display-list form the painter consumes.
pub struct HtmlView {
    fragments: Vec<Fragment>,
    width: f32,
    pub height: f32,
    selection: gpui_kit::base::TextSelectionHandle,
}

impl HtmlView {
    pub fn selection(&self) -> gpui_kit::base::TextSelectionHandle {
        self.selection.clone()
    }

    /// The width this view was laid out at, so a relayout can be skipped when it is unchanged.
    pub fn width(&self) -> f32 {
        self.width
    }
}

/// Sanitize, parse and lay out `html` at `width`, measuring text with the window's text system.
pub fn layout_document(
    document: &SanitizedDocument,
    width: f32,
    window: &Window,
    images: &Images,
    selection: Option<gpui_kit::base::TextSelectionHandle>,
    show_quotes: bool,
    cx: &mut App,
) -> Result<HtmlView, layout::LayoutError> {
    let mut dom = parse_to_dom_at(document, images, show_quotes, width);
    resolve_font_families(&mut dom, window.text_system());
    let measure = GpuiMeasure {
        text_system: window.text_system(),
    };
    let result =
        layout::layout_with_limits(&dom, width, &measure, layout::LayoutLimits::default())?;
    let selection = selection.unwrap_or_else(|| {
        gpui_kit::base::TextSelectionHandle::new(document.plain_text.clone(), cx)
    });
    selection.set_fallback_copy_text(document.plain_text.clone(), cx);
    Ok(HtmlView {
        fragments: result.fragments,
        width,
        height: result.height,
        selection,
    })
}

/// Replace every element's font stack with the first family this machine actually has.
///
/// Measurement and painting both read the resolved name, so they cannot disagree about which
/// face a word is set in. Resolving here, rather than handing GPUI the stack, matters because
/// GPUI only uses a `Font`'s `fallbacks` for missing *glyphs*: a missing *family* drops straight
/// to GPUI's own fallback stack, which starts with a monospace face. A stack with nothing
/// installed becomes `None`, which both sides treat as [`DEFAULT_FONT_FAMILY`].
fn resolve_font_families(nodes: &mut [Node], text_system: &WindowTextSystem) {
    for node in nodes {
        if let Node::Element(element) = node {
            if let Some(stack) = element.style.font_family.take() {
                element.style.font_family = first_installed(&stack, text_system);
            }
            resolve_font_families(&mut element.children, text_system);
        }
    }
}

/// The first installed family in a normalized stack, cached per stack: a newsletter repeats
/// the same few stacks on hundreds of elements.
fn first_installed(stack: &str, text_system: &WindowTextSystem) -> Option<String> {
    static RESOLVED: LazyLock<Mutex<HashMap<String, Option<String>>>> =
        LazyLock::new(Mutex::default);
    let lock = || {
        RESOLVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    if let Some(resolved) = lock().get(stack) {
        return resolved.clone();
    }
    let resolved = font_stack(stack)
        .find(|family| family_installed(family, text_system))
        .map(str::to_string);
    lock().insert(stack.to_string(), resolved.clone());
    resolved
}

/// Whether the platform has `family`. GPUI's `font_id`, which would report a missing family,
/// is private; `resolve_font` instead substitutes GPUI's fallback face. So a family is installed
/// when it resolves to something other than that fallback — or when it *is* the fallback.
fn family_installed(family: &str, text_system: &WindowTextSystem) -> bool {
    let fallback = text_system.resolve_font(&font("\u{1}snail: no such family"));
    let id = text_system.resolve_font(&font(family.to_string()));
    id != fallback
        || text_system
            .get_font_for_id(fallback)
            .is_some_and(|face| face.family.eq_ignore_ascii_case(family))
}

pub fn has_quoted_content(document: &SanitizedDocument) -> bool {
    fn visit(nodes: &[CoreNode]) -> bool {
        nodes.iter().any(|node| match node {
            CoreNode::Element(element) => {
                element.tag.eq_ignore_ascii_case("blockquote") || visit(&element.children)
            }
            CoreNode::Text(_) => false,
        })
    }
    visit(&document.roots)
}

/// Write only the already-sanitized, network-inert document for the browser escape hatch.
pub fn browser_file(document: &SanitizedDocument) -> std::io::Result<PathBuf> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><style>{}</style>{}",
        document.stylesheet, document.html
    );
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    let path = std::env::temp_dir().join(format!("snail-message-{:016x}.html", hasher.finish()));
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Paint only fragments intersecting the viewport (plus overscan) while retaining a full-height
/// document box so the native scroll container keeps correct geometry (E6.7).
pub fn paint(
    view: &HtmlView,
    images: &Images,
    visible_top: f32,
    visible_height: f32,
) -> AnyElement {
    let overscan = 320.0;
    let min_y = (visible_top - overscan).max(0.0);
    let max_y = visible_top + visible_height.max(600.0) + overscan;
    div()
        .relative()
        .w(px(view.width))
        .h(px(view.height))
        // Email is authored for white paper; give the document its own ground so a dark app theme
        // does not turn every message black.
        .bg(rgb(0xffffff))
        .text_color(rgb(0x202020))
        .children(
            view.fragments
                .iter()
                .enumerate()
                .filter_map(|(index, fragment)| {
                    fragment_intersects(fragment, min_y, max_y)
                        .then(|| paint_fragment(index, fragment, images, &view.selection))
                }),
        )
        .into_any_element()
}

fn fragment_intersects(fragment: &Fragment, min_y: f32, max_y: f32) -> bool {
    let (top, bottom) = match fragment {
        Fragment::Rect { rect, .. }
        | Fragment::Border { rect, .. }
        | Fragment::Image { rect, .. } => (rect.y, rect.y + rect.h),
        Fragment::Word { y, font, .. } => (*y, *y + layout::line_height(font)),
    };
    bottom >= min_y && top <= max_y
}

fn paint_fragment(
    index: usize,
    fragment: &Fragment,
    images: &Images,
    selection: &gpui_kit::base::TextSelectionHandle,
) -> AnyElement {
    match fragment {
        Fragment::Rect {
            rect,
            color,
            radius,
        } => div()
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
            radius,
        } => div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.w))
            .h(px(rect.h))
            .border(px((*width).max(0.75)))
            .border_color(rgba(to_u32(*color)))
            .rounded(px(*radius))
            .into_any_element(),
        Fragment::Word {
            x,
            y,
            text,
            font,
            color,
            underline,
            strike,
            href,
        } => {
            // Paint each word in a box exactly its CSS line height tall, so GPUI centres the
            // glyphs where layout put the line. Without it GPUI used its own default line height
            // (about 1.6×), which drew every line of email text lower than laid out and left a
            // `line-height: 50px` button's label at the top instead of in the middle.
            let line = layout::line_height(font);
            let selectable = gpui_kit::base::SelectableText::with_handle(
                format!("html-word-{index}"),
                selection.clone(),
                format!("{text} "),
            )
            .document_order(index as u64);
            let weight = if font.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            };
            let family = font
                .family
                .clone()
                .unwrap_or_else(|| DEFAULT_FONT_FAMILY.into());
            if let Some(href) = href {
                let target = href.clone();
                let tooltip = target.clone();
                let visible = text.clone();
                gpui_kit::base::Link::new(format!("html-link-{index}"))
                    .absolute()
                    .left(px(*x))
                    .top(px(*y))
                    .font_family(family)
                    .text_size(px(font.size))
                    .text_color(rgba(to_u32(*color)))
                    .font_weight(weight)
                    .line_height(px(line))
                    .when(*underline, |this| this.underline())
                    .when(*strike, |this| this.line_through())
                    .when(font.italic, |this| this.italic())
                    .href(target.clone())
                    .tooltip(move |window, cx| link_tooltip(&tooltip, window, cx))
                    .open_with(move |href, _, window, cx| {
                        open_link(visible.clone(), href.to_string(), window, cx)
                    })
                    .child(selectable)
                    .into_any_element()
            } else {
                div()
                    .absolute()
                    .left(px(*x))
                    .top(px(*y))
                    .font_family(family)
                    .text_size(px(font.size))
                    .text_color(rgba(to_u32(*color)))
                    .font_weight(weight)
                    .line_height(px(line))
                    .when(*underline, |this| this.underline())
                    .when(*strike, |this| this.line_through())
                    .when(font.italic, |this| this.italic())
                    .child(selectable)
                    .into_any_element()
            }
        }
        Fragment::Image {
            rect,
            src,
            href,
            background,
        } => {
            // A CSS background that cannot be shown leaves its box as it is, as in a browser. A
            // placeholder here drew only its 1px frame, because the box's own content covers the
            // rest — thin grey lines along table cells that Apple Mail does not have.
            if *background && images.resolve(src).is_none() {
                return div().into_any_element();
            }
            // The box is placed in document space; the picture fills it.
            let picture = match images.resolve(src) {
                // A real inline image (E6.8).
                // `Fill` is HTML's default `object-fit`. GPUI's own default is `Contain`, which
                // shrank the picture to fit inside any box whose aspect ratio differed.
                // The texture carries an EDGE_PAD border; draw it that much larger and let the
                // frame clip it, so the filtered edge lands outside the box.
                Some(placed) => {
                    let pad_x = rect.w / placed.width.max(1.0) * EDGE_PAD as f32;
                    let pad_y = rect.h / placed.height.max(1.0) * EDGE_PAD as f32;
                    img(ImageSource::Render(placed.render.clone()))
                        .object_fit(ObjectFit::Fill)
                        .absolute()
                        .left(px(-pad_x))
                        .top(px(-pad_y))
                        .w(px(rect.w + 2.0 * pad_x))
                        .h(px(rect.h + 2.0 * pad_y))
                        .into_any_element()
                }
                // A blocked remote image or a missing part: reserved size, no fetch (E6.2).
                None => div()
                    .size_full()
                    .bg(rgb(0xefefef))
                    .border_1()
                    .border_color(rgb(0xd5d5d5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(rgb(0x848484))
                    .when(rect.w >= 120.0 && rect.h >= 48.0, |this| {
                        this.child(if src.is_empty() {
                            "[image]"
                        } else {
                            "[remote image blocked]"
                        })
                    })
                    .into_any_element(),
            };
            let frame = div()
                .id(format!("html-image-{index}"))
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.w))
                .h(px(rect.h))
                .overflow_hidden()
                .child(picture);
            match href {
                // A linked image (an email's buttons, logos and banners): the whole box is the
                // link, with its target on hover, like Apple Mail.
                Some(target) => {
                    let tooltip = target.clone();
                    let target = target.clone();
                    frame
                        .cursor_pointer()
                        .tooltip(move |window, cx| link_tooltip(&tooltip, window, cx))
                        .on_click(move |_, window, cx| {
                            // No visible text to disagree with the target, so nothing to confirm.
                            open_link(String::new(), target.clone(), window, cx)
                        })
                        .into_any_element()
                }
                None => frame.into_any_element(),
            }
        }
    }
}

/// A link's target on hover, wrapped at a readable width. Tracking links run to hundreds of
/// characters with no spaces; on one line they ran off the window.
fn link_tooltip(target: &str, window: &mut Window, cx: &mut App) -> AnyView {
    let target = SharedString::from(target.to_string());
    gpui_kit::component::tooltip::Tooltip::element(move |_, _| {
        div().max_w(px(LINK_TOOLTIP_WIDTH)).child(target.clone())
    })
    .build(window, cx)
}

/// About Apple Mail's link-tooltip width.
const LINK_TOOLTIP_WIDTH: f32 = 420.0;

fn open_link(visible: String, target: String, window: &mut Window, cx: &mut App) {
    if !matches!(
        target.to_ascii_lowercase().split(':').next(),
        Some("http" | "https" | "mailto")
    ) {
        return;
    }
    if !link_needs_confirmation(&visible, &target) {
        cx.open_url(&target);
        return;
    }
    let detail = format!("The link text says “{visible}”, but it opens:\n{target}");
    let answer = window.prompt(
        PromptLevel::Warning,
        "Open a different link target?",
        Some(&detail),
        &[
            PromptButton::ok("Open link"),
            PromptButton::cancel("Cancel"),
        ],
        cx,
    );
    cx.spawn(async move |cx| {
        if answer.await.ok() == Some(0) {
            cx.update(|cx| cx.open_url(&target));
        }
    })
    .detach();
}

fn link_needs_confirmation(visible: &str, target: &str) -> bool {
    fn host(value: &str) -> Option<String> {
        let lower = value.trim().to_ascii_lowercase();
        let rest = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .or_else(|| lower.strip_prefix("www."))?;
        Some(
            rest.split(['/', ':', '?', '#'])
                .next()
                .unwrap_or("")
                .trim_start_matches("www.")
                .to_string(),
        )
    }
    match (host(visible), host(target)) {
        (Some(visible), Some(target)) => !visible.is_empty() && visible != target,
        _ => false,
    }
}

fn to_u32(color: [u8; 4]) -> u32 {
    (color[0] as u32) << 24 | (color[1] as u32) << 16 | (color[2] as u32) << 8 | color[3] as u32
}

#[cfg(test)]
fn parse_to_dom(document: &SanitizedDocument, images: &Images, show_quotes: bool) -> Vec<Node> {
    parse_to_dom_at(
        document,
        images,
        show_quotes,
        layout::DEFAULT_DOCUMENT_WIDTH,
    )
}

/// Build the styled tree for a document laid out `width` wide. The width decides which `@media`
/// blocks apply, as the viewport does in a browser: mobile-first mail keeps its desktop layout
/// (side-by-side columns, wider containers) behind `@media (min-width: …)`.
fn parse_to_dom_at(
    document: &SanitizedDocument,
    images: &Images,
    show_quotes: bool,
    width: f32,
) -> Vec<Node> {
    let sheet = parse_sheet(&document.stylesheet, width);
    let mut roots = Vec::new();
    let parent = Style::default();
    let ancestors = Vec::new();
    build_children(
        &document.roots,
        &parent,
        images,
        &sheet,
        show_quotes,
        None,
        &ancestors,
        &mut roots,
    );
    roots
}

fn build_children(
    source: &[CoreNode],
    parent: &Style,
    images: &Images,
    sheet: &[Rule],
    show_quotes: bool,
    inherited_cell_padding: Option<f32>,
    ancestors: &[(String, HashMap<String, String>)],
    out: &mut Vec<Node>,
) -> bool {
    for child in source {
        match child {
            CoreNode::Text(text) => {
                if !text.trim().is_empty() {
                    out.push(Node::Text(text.clone()));
                }
            }
            CoreNode::Element(element) => {
                let tag = element.tag.clone();
                if !show_quotes && tag.eq_ignore_ascii_case("blockquote") {
                    return true;
                }
                let map = element.attrs.clone();

                // Precedence (E6.3/E6.4): tag + presentational attrs, then <style>/class rules in
                // specificity order, then the inline style attribute. resolve_style applies the
                // inline style itself, so it is withheld here and applied last.
                let mut presentational = map.clone();
                presentational.remove("style");
                let mut style = resolve_style(&tag, &presentational, parent);
                // CSS's order: sheet, inline, then `!important` sheet, then `!important` inline —
                // which is how a `@media` rule overrides an inline width.
                let (inline_normal, inline_important) = map
                    .get("style")
                    .map(|inline| split_important(inline))
                    .unwrap_or_default();
                apply_sheet(&mut style, &tag, &map, ancestors, sheet, false);
                apply_inline_style(&mut style, &inline_normal);
                apply_sheet(&mut style, &tag, &map, ancestors, sheet, true);
                apply_inline_style(&mut style, &inline_important);
                if matches!(tag.as_str(), "td" | "th")
                    && let Some(padding) = inherited_cell_padding
                    && style.padding_top == 0.0
                    && style.padding_right == 0.0
                    && style.padding_bottom == 0.0
                    && style.padding_left == 0.0
                {
                    style.padding_top = padding;
                    style.padding_right = padding;
                    style.padding_bottom = padding;
                    style.padding_left = padding;
                }

                // Hand the layout engine the image's *natural* size, kept separate from the
                // sender's specified width/height (E6.8). Layout completes a single specified
                // dimension from the natural aspect ratio, as a browser does. Remote images are
                // keyed by their `snail-remote:` token and resolve once fetched; a blocked one
                // stays unknown and layout reserves its placeholder box.
                if tag == "img" {
                    style.natural_size = map.get("src").and_then(|src| images.size(src));
                }

                let mut children = Vec::new();
                let mut child_ancestors = ancestors.to_vec();
                child_ancestors.push((tag.clone(), map.clone()));
                let child_cell_padding = if tag == "table" {
                    Some(style.cell_padding)
                } else {
                    inherited_cell_padding
                };
                let quote_boundary = build_children(
                    &element.children,
                    &style,
                    images,
                    sheet,
                    show_quotes,
                    child_cell_padding,
                    &child_ancestors,
                    &mut children,
                );
                out.push(Node::Element(Element {
                    tag,
                    attrs: map,
                    style,
                    children,
                }));
                if quote_boundary {
                    return true;
                }
            }
        }
    }
    false
}

// --- The `<style>` subset (E6.3/E6.4) --------------------------------------------------------

/// One parsed rule. Selectors are approximated: descendant combinators are ignored and only the
/// last compound is matched, which is the common case in mail.
struct Rule {
    compounds: Vec<Compound>,
    /// The rule's declarations, split by `!important`, which cascades after inline style.
    normal: String,
    important: String,
    specificity: u32,
}

/// Split a declaration block into its normal and `!important` halves.
fn split_important(declarations: &str) -> (String, String) {
    let mut normal = String::new();
    let mut important = String::new();
    for declaration in declarations.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        let target = if declaration
            .to_ascii_lowercase()
            .trim_end()
            .ends_with("!important")
        {
            &mut important
        } else {
            &mut normal
        };
        target.push_str(declaration);
        target.push(';');
    }
    (normal, important)
}

#[derive(Clone)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

fn parse_sheet(css: &str, viewport_width: f32) -> Vec<Rule> {
    let css = resolve_at_rules(css, viewport_width);
    // Drop comments.
    let mut cleaned = String::with_capacity(css.len());
    let mut rest = css.as_str();
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
            if selector.is_empty()
                || selector
                    .chars()
                    .any(|character| matches!(character, '@' | ':' | '[' | '+' | '~'))
            {
                continue;
            }
            let compounds = selector
                .replace('>', " ")
                .split_whitespace()
                .map(|compound| {
                    let (tag, id, classes) = parse_compound_selector(compound);
                    Compound { tag, id, classes }
                })
                .filter(|compound| {
                    compound.tag.is_some() || compound.id.is_some() || !compound.classes.is_empty()
                })
                .collect::<Vec<_>>();
            if compounds.is_empty() {
                continue;
            }
            let specificity = compounds.iter().fold(0, |specificity, compound| {
                specificity
                    + (compound.id.is_some() as u32) * 100
                    + compound.classes.len() as u32 * 10
                    + compound.tag.is_some() as u32
            });
            let (normal, important) = split_important(declarations);
            rules.push(Rule {
                compounds,
                normal,
                important,
                specificity,
            });
        }
    }
    rules
}

/// Unwrap the `@media` blocks that apply at `viewport_width` and drop every other at-rule
/// (`@font-face`, `@import`, `@supports`, non-matching media).
fn resolve_at_rules(css: &str, viewport_width: f32) -> String {
    let bytes = css.as_bytes();
    let mut output = String::with_capacity(css.len());
    let mut index = 0;
    let mut copy_start = 0;
    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }
        output.push_str(&css[copy_start..index]);
        let mut cursor = index + 1;
        while cursor < bytes.len() && !matches!(bytes[cursor], b'{' | b';') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            copy_start = bytes.len();
            index = bytes.len();
            continue;
        }
        if bytes[cursor] == b';' {
            index = cursor + 1;
            copy_start = index;
            continue;
        }
        let prelude = &css[index + 1..cursor];
        let body_start = cursor + 1;
        let mut depth = 1usize;
        cursor += 1;
        while cursor < bytes.len() && depth > 0 {
            match bytes[cursor] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            cursor += 1;
        }
        let body_end = if depth == 0 { cursor - 1 } else { cursor };
        if let Some(query) = prelude
            .trim()
            .strip_prefix("media")
            .filter(|_| prelude.trim().len() > "media".len())
        {
            if media_matches(query, viewport_width) {
                output.push_str(&resolve_at_rules(
                    &css[body_start..body_end],
                    viewport_width,
                ));
                output.push(' ');
            }
        }
        index = cursor;
        copy_start = index;
    }
    output.push_str(&css[copy_start..]);
    output
}

/// Whether a media query list applies to a light-themed screen `viewport_width` px wide. Any part
/// it does not understand fails the query, so an unknown feature never switches layouts.
fn media_matches(query: &str, viewport_width: f32) -> bool {
    query.split(',').any(|single| {
        let single = single.trim().to_ascii_lowercase();
        let (negated, single) = match single.strip_prefix("not ") {
            Some(rest) => (true, rest.trim().to_string()),
            None => (
                false,
                single
                    .strip_prefix("only ")
                    .unwrap_or(&single)
                    .trim()
                    .to_string(),
            ),
        };
        let mut matched = true;
        for part in single.split(" and ") {
            let part = part.trim();
            let ok = match part {
                "" | "screen" | "all" => true,
                "print" | "speech" | "tv" | "handheld" => false,
                feature if feature.starts_with('(') && feature.ends_with(')') => {
                    feature_matches(&feature[1..feature.len() - 1], viewport_width)
                }
                _ => false,
            };
            matched &= ok;
        }
        matched != negated
    })
}

fn feature_matches(feature: &str, viewport_width: f32) -> bool {
    let Some((name, value)) = feature.split_once(':') else {
        return false;
    };
    let (name, value) = (name.trim(), value.trim());
    let px = || -> Option<f32> {
        let number = value
            .strip_suffix("px")
            .map(|n| n.trim().parse::<f32>().ok())
            .unwrap_or_else(|| {
                value
                    .strip_suffix("em")
                    .and_then(|n| n.trim().parse::<f32>().ok())
                    .map(|em| em * 16.0)
            });
        number
    };
    match name {
        "min-width" | "min-device-width" => px().is_some_and(|min| viewport_width >= min),
        "max-width" | "max-device-width" => px().is_some_and(|max| viewport_width <= max),
        // Email is drawn on white paper whatever the app theme, so its dark-mode rules never apply.
        "prefers-color-scheme" => value == "light",
        _ => false,
    }
}

fn parse_compound_selector(compound: &str) -> (Option<String>, Option<String>, Vec<String>) {
    fn commit(
        mode: char,
        token: &mut String,
        tag: &mut Option<String>,
        id: &mut Option<String>,
        classes: &mut Vec<String>,
    ) {
        if token.is_empty() {
            return;
        }
        let value = std::mem::take(token);
        match mode {
            '.' => classes.push(value),
            '#' => *id = Some(value),
            _ if value != "*" => *tag = Some(value),
            _ => {}
        }
    }

    let compound = compound.split(':').next().unwrap_or(compound);
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut token = String::new();
    let mut mode = 't';
    for character in compound.chars() {
        match character {
            '.' | '#' => {
                commit(mode, &mut token, &mut tag, &mut id, &mut classes);
                mode = character;
            }
            '[' => break,
            character if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') => {
                token.push(character)
            }
            _ => {}
        }
    }
    commit(mode, &mut token, &mut tag, &mut id, &mut classes);
    (tag, id, classes)
}

fn apply_sheet(
    style: &mut Style,
    tag: &str,
    attrs: &HashMap<String, String>,
    ancestors: &[(String, HashMap<String, String>)],
    sheet: &[Rule],
    important: bool,
) {
    fn matches(compound: &Compound, tag: &str, attrs: &HashMap<String, String>) -> bool {
        let id = attrs.get("id").map(String::as_str).unwrap_or("");
        let class_attr = attrs.get("class").map(String::as_str).unwrap_or("");
        let classes: Vec<&str> = class_attr.split_whitespace().collect();
        compound
            .tag
            .as_deref()
            .is_none_or(|expected| expected.eq_ignore_ascii_case(tag))
            && compound.id.as_deref().is_none_or(|expected| expected == id)
            && compound
                .classes
                .iter()
                .all(|class| classes.iter().any(|candidate| candidate == class))
    }

    let mut matching: Vec<&Rule> = sheet
        .iter()
        .filter(|rule| {
            let Some(current) = rule.compounds.last() else {
                return false;
            };
            if !matches(current, tag, attrs) {
                return false;
            }
            let mut ancestor_index = ancestors.len();
            for required in rule.compounds[..rule.compounds.len() - 1].iter().rev() {
                let Some(found) = ancestors[..ancestor_index]
                    .iter()
                    .rposition(|(tag, attrs)| matches(required, tag, attrs))
                else {
                    return false;
                };
                ancestor_index = found;
            }
            true
        })
        .collect();
    matching.sort_by_key(|rule| rule.specificity);
    for rule in matching {
        apply_inline_style(
            style,
            if important {
                &rule.important
            } else {
                &rule.normal
            },
        );
    }
}

struct GpuiMeasure<'a> {
    text_system: &'a WindowTextSystem,
}

#[cfg(test)]
mod tests {
    use super::{
        Fragment, Images, Node, TextMeasure, browser_file, link_needs_confirmation, media_matches,
        pad_edges, parse_to_dom, parse_to_dom_at, prepare_document, resolve_at_rules,
    };
    use serde_json::{Value, json};
    use snail_ui::html::FontSpec;
    use snail_ui::layout;

    struct FakeMeasure;
    impl TextMeasure for FakeMeasure {
        fn width(&self, text: &str, font: &FontSpec) -> f32 {
            text.chars().count() as f32 * font.size * 0.55
        }
    }

    fn words(nodes: &[Node]) -> Vec<String> {
        layout::layout(nodes, 640.0, &FakeMeasure)
            .words()
            .map(|(word, _, _, _)| word.to_string())
            .collect()
    }

    #[test]
    fn html_quote_folding_stops_at_the_first_boundary() {
        let document = prepare_document(
            "<p>New answer</p><blockquote><p>Old answer</p></blockquote><p>signature</p>",
        );
        assert_eq!(
            words(&parse_to_dom(&document, &Images::default(), false)),
            ["New", "answer"]
        );
        assert_eq!(
            words(&parse_to_dom(&document, &Images::default(), true)),
            ["New", "answer", "Old", "answer", "signature"]
        );
    }

    #[test]
    fn phishing_check_only_warns_for_disagreeing_visible_hosts() {
        assert!(!link_needs_confirmation(
            "https://example.com/account",
            "https://example.com/login"
        ));
        assert!(link_needs_confirmation(
            "https://example.com",
            "https://evil.test/login"
        ));
        assert!(!link_needs_confirmation(
            "Read the story",
            "https://example.com"
        ));
    }

    #[test]
    fn css_compound_selectors_keep_class_id_and_tag_roles() {
        assert_eq!(
            super::parse_compound_selector(".cta"),
            (None, None, vec!["cta".into()])
        );
        assert_eq!(
            super::parse_compound_selector("td.card#primary:hover"),
            (
                Some("td".into()),
                Some("primary".into()),
                vec!["card".into()]
            )
        );
    }

    #[test]
    fn descendant_rules_do_not_leak_and_media_queries_are_dropped() {
        let document = prepare_document(
            r#"<head><style>
                .hidden table { display:none }
                @media (max-width: 600px) { .mobile { display:none } }
            </style></head>
            <table><tr><td>shown</td></tr></table>
            <div class="hidden"><table><tr><td>hidden</td></tr></table></div>
            <p class="mobile">desktop</p>"#,
        );
        assert_eq!(
            words(&parse_to_dom(&document, &Images::default(), true)),
            ["shown", "desktop"]
        );
    }

    #[test]
    fn media_queries_follow_the_width_mail_is_laid_out_at() {
        assert!(media_matches(" screen and (min-width: 600px)", 640.0));
        assert!(!media_matches(" screen and (min-width: 600px)", 480.0));
        assert!(media_matches(" (max-width:600px)", 480.0));
        assert!(!media_matches(" only screen and (max-width: 600px)", 640.0));
        assert!(!media_matches(" print", 640.0));
        assert!(!media_matches(" (prefers-color-scheme: dark)", 640.0));
        assert!(media_matches(" print, screen and (min-width: 30em)", 640.0));
        assert!(!media_matches(
            " screen and (-webkit-min-device-pixel-ratio: 2)",
            640.0
        ));
        assert!(media_matches(" not print", 640.0));
    }

    // Regression (Slickdeals): a mobile-first card's desktop layout is `@media (min-width)` rules
    // marked `!important`, which must beat the inline mobile widths, as they do in Apple Mail.
    #[test]
    fn important_media_rules_override_inline_style_at_desktop_width() {
        let document = prepare_document(
            r#"<head><style>
                @media screen and (min-width: 600px) { .dtop-w-580 { width: 580px !important; } }
                .plain { width: 100px; }
            </style></head>
            <table class="dtop-w-580" style="width:290px"><tr><td>card</td></tr></table>
            <table class="plain" style="width:200px"><tr><td>inline wins</td></tr></table>"#,
        );
        fn tables(nodes: &[Node], out: &mut Vec<Option<f32>>) {
            for node in nodes {
                if let Node::Element(element) = node {
                    if element.tag == "table" {
                        out.push(element.style.width);
                    }
                    tables(&element.children, out);
                }
            }
        }
        let widths_at = |width: f32| {
            let mut out = Vec::new();
            tables(
                &parse_to_dom_at(&document, &Images::default(), true, width),
                &mut out,
            );
            out.into_iter().flatten().collect::<Vec<_>>()
        };
        assert_eq!(widths_at(640.0), vec![580.0, 200.0]);
        assert_eq!(widths_at(400.0), vec![290.0, 200.0]);
    }

    #[test]
    fn nested_and_unknown_at_rules_are_resolved_or_dropped() {
        let css = "@font-face { font-family: x; src: url(y) } \
                   @media screen { @media (min-width: 100px) { .a { color: red } } } \
                   @supports (display: grid) { .b { color: blue } } .c { color: green }";
        let resolved = resolve_at_rules(css, 640.0);
        assert!(resolved.contains(".a { color: red }"), "{resolved}");
        assert!(
            !resolved.contains(".b") && !resolved.contains("font-face"),
            "{resolved}"
        );
        assert!(resolved.contains(".c { color: green }"));
    }

    #[test]
    fn browser_escape_hatch_writes_only_sanitized_inert_html() {
        let document = prepare_document(
            r#"<script>alert(1)</script><img src="https://tracker.test/p.gif"><p>safe</p>"#,
        );
        let path = browser_file(&document).unwrap();
        let output = std::fs::read_to_string(&path).unwrap();
        assert!(!output.contains("<script"));
        assert!(!output.contains("https://tracker.test"));
        assert!(output.contains("snail-remote:"));
        assert!(output.contains("safe"));
        let _ = std::fs::remove_file(path);
    }

    fn fingerprint(layout: &layout::Layout) -> String {
        let mut hash = 0xcbf29ce484222325u64;
        let mut feed = |value: &str| {
            for byte in value.as_bytes() {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        };
        for fragment in &layout.fragments {
            let value = match fragment {
                Fragment::Rect {
                    rect,
                    color,
                    radius,
                } => format!(
                    "r:{:.1}:{:.1}:{:.1}:{:.1}:{color:?}:{radius:.1}",
                    rect.x, rect.y, rect.w, rect.h
                ),
                Fragment::Border {
                    rect, color, width, ..
                } => format!(
                    "b:{:.1}:{:.1}:{:.1}:{:.1}:{color:?}:{width:.1}",
                    rect.x, rect.y, rect.w, rect.h
                ),
                Fragment::Word {
                    x,
                    y,
                    text,
                    font,
                    color,
                    underline,
                    href,
                    ..
                } => format!(
                    "w:{x:.1}:{y:.1}:{text}:{:?}:{:.1}:{}:{}:{:?}:{underline}:{href:?}",
                    font.family, font.size, font.bold, font.italic, color
                ),
                Fragment::Image { rect, src, .. } => format!(
                    "i:{:.1}:{:.1}:{:.1}:{:.1}:{src}",
                    rect.x, rect.y, rect.w, rect.h
                ),
            };
            feed(&value);
        }
        format!("{hash:016x}")
    }

    fn snapshot(nodes: &[Node], node_count: usize) -> Value {
        let output = layout::layout_with_limits(
            nodes,
            640.0,
            &FakeMeasure,
            layout::LayoutLimits {
                max_nodes: 100_000,
                max_duration: std::time::Duration::from_secs(2),
            },
        )
        .expect("corpus item must stay inside the renderer budget");
        json!({
            "nodes": node_count,
            "fragments": output.fragments.len(),
            "height_tenths": (output.height * 10.0).round() as i64,
            "fingerprint": fingerprint(&output),
        })
    }

    // Regression (Samsung): 1px grey lines along image edges, from GPUI's unpadded atlas.
    #[test]
    fn image_textures_are_padded_with_their_own_edges() {
        // A 2x1 image: red then blue.
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let padded = pad_edges(2, 1, &pixels);
        let pixel = |x: usize, y: usize| &padded[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(padded.len(), 4 * 3 * 4);
        for y in 0..3 {
            assert_eq!(pixel(0, y), &pixels[0..4], "left pad copies the left edge");
            assert_eq!(pixel(1, y), &pixels[0..4]);
            assert_eq!(pixel(2, y), &pixels[4..8]);
            assert_eq!(
                pixel(3, y),
                &pixels[4..8],
                "right pad copies the right edge"
            );
        }
    }

    #[test]
    fn e0_3_corpus_box_trees_match_the_checked_in_snapshot() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut actual = serde_json::Map::new();

        // The corpus is the owner's real mail and is deliberately **not** in the repo, so it is
        // present on the owner's machine and absent in CI. When it is absent we still verify every
        // fixture that ships — the checked-in regressions below — against the same snapshot.
        let corpus = root.join("spikes/corpus");
        let corpus_present = corpus.is_dir();
        if corpus_present {
            let mut paths = std::fs::read_dir(&corpus)
                .expect("corpus directory")
                .map(|entry| entry.expect("corpus entry").path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "eml"))
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                let raw = std::fs::read(&path).unwrap();
                let parsed = snail_core::mime::parse_raw(&raw).unwrap();
                let value = match parsed.body {
                    snail_core::mime::Body::Html(html) => {
                        let document = prepare_document(&html);
                        let nodes = parse_to_dom(&document, &Images::default(), true);
                        let value = snapshot(&nodes, document.node_count);
                        assert_ne!(
                            value["fragments"],
                            0,
                            "{} rendered empty; stylesheet:\n{}",
                            path.display(),
                            document.stylesheet
                        );
                        value
                    }
                    snail_core::mime::Body::Text(text) => {
                        let nodes = vec![Node::Text(text)];
                        snapshot(&nodes, 1)
                    }
                    snail_core::mime::Body::None => panic!("{} has no body", path.display()),
                };
                actual.insert(
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    value,
                );
            }
        }

        let regressions = root.join("crates/snail/tests/fixtures/e6-regressions");
        let mut paths = std::fs::read_dir(regressions)
            .expect("e6-regressions directory (checked in)")
            .map(|entry| entry.expect("regression entry").path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let document = prepare_document(&std::fs::read_to_string(&path).unwrap());
            let nodes = parse_to_dom(&document, &Images::default(), true);
            actual.insert(
                path.file_name().unwrap().to_string_lossy().into_owned(),
                snapshot(&nodes, document.node_count),
            );
        }

        let expected: Value =
            serde_json::from_str(include_str!("../tests/fixtures/e6-corpus-snapshot.json"))
                .unwrap();
        let expected = expected.as_object().expect("snapshot is an object");

        if !corpus_present {
            // Compare the subset both sides have. The corpus entries cannot be reproduced here,
            // but every shipped fixture must still match its recorded box tree.
            assert!(!actual.is_empty(), "no shipped fixtures to verify");
            for (name, expected_value) in expected {
                if let Some(actual_value) = actual.get(name) {
                    assert_eq!(
                        actual_value, expected_value,
                        "snapshot mismatch for {name}: update e6-corpus-snapshot.json only after \
                         reviewing the box-tree diff"
                    );
                }
            }
            for name in actual.keys() {
                assert!(
                    expected.contains_key(name),
                    "{name} is not in e6-corpus-snapshot.json"
                );
            }
            return;
        }

        let actual = Value::Object(actual);
        assert_eq!(
            actual,
            Value::Object(expected.clone()),
            "update e6-corpus-snapshot.json only after reviewing this box-tree diff:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }
}

impl TextMeasure for GpuiMeasure<'_> {
    fn width(&self, text: &str, font: &FontSpec) -> f32 {
        let gpu_font = Font {
            family: font
                .family
                .as_deref()
                .unwrap_or(DEFAULT_FONT_FAMILY)
                .to_string()
                .into(),
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
        let line =
            self.text_system
                .shape_line(text.to_string().into(), px(font.size), &[run], None);
        line.width().into()
    }
}
