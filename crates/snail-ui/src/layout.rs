//! A minimal block/inline/table layout for the E0.3/E6 prototype: it turns the resolved DOM into a
//! flat display list of positioned fragments, using a `TextMeasure` for word advance widths.
//!
//! This is the seed of plan.md E6.5 (and a first cut at 6.6). It is deliberately small: block
//! stacking, greedy inline wrapping with alignment, and table columns with padding and borders.
//! Geometry is unit-tested against a fake metrics implementation, which is the whole reason layout
//! lives here rather than in the GPUI crate.

use std::time::{Duration, Instant};

use crate::html::{
    Align, Color, Display, Element, FontSpec, ListStyle, Node, Style, VerticalAlign, primary_family,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fragment {
    /// A filled rectangle (background).
    Rect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    /// A stroked rectangle border.
    Border {
        rect: Rect,
        color: Color,
        width: f32,
    },
    /// A single laid-out word, positioned by its top-left corner.
    Word {
        x: f32,
        y: f32,
        text: String,
        font: FontSpec,
        color: Color,
        underline: bool,
        href: Option<String>,
    },
    /// A reserved image box.
    Image { rect: Rect, src: String },
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub fragments: Vec<Fragment>,
}

impl Layout {
    pub fn words(&self) -> impl Iterator<Item = (&str, f32, f32, f32)> {
        self.fragments.iter().filter_map(|fragment| match fragment {
            Fragment::Word {
                x, y, text, font, ..
            } => Some((text.as_str(), *x, *y, font.size)),
            _ => None,
        })
    }
}

/// Measures the natural advance width of a string in a font, with no wrapping.
pub trait TextMeasure {
    fn width(&self, text: &str, font: &FontSpec) -> f32;
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutLimits {
    pub max_nodes: usize,
    pub max_duration: Duration,
}

impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_nodes: 50_000,
            max_duration: Duration::from_millis(40),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    NodeBudget { nodes: usize, limit: usize },
    TimeBudget,
}

/// The width a document is laid out at before the reading pane has been measured — only the
/// first frame of the first HTML message after launch; every later layout uses the pane's real
/// width.
pub const DEFAULT_DOCUMENT_WIDTH: f32 = 640.0;

/// The narrowest width a document is laid out at. 320px is the narrowest layout responsive email
/// templates target; below it, table layouts degenerate into one word per line, which is worse
/// than letting a narrow pane clip the right edge.
pub const MIN_DOCUMENT_WIDTH: f32 = 320.0;

/// The width to lay a document out at, given the reading pane's measured content width (its
/// width inside its padding). Floored to whole pixels: at fractional scale factors the measured
/// width can differ by a fraction of a pixel between frames, and the document must never be a
/// fraction wider than the box it sits in.
pub fn document_width(content_width: f32) -> f32 {
    if content_width.is_finite() {
        content_width.floor().max(MIN_DOCUMENT_WIDTH)
    } else {
        DEFAULT_DOCUMENT_WIDTH
    }
}

/// Whether a document laid out at `laid_out` needs laying out again for `target`. Both come from
/// [`document_width`], so any real change is at least a whole pixel.
pub fn width_changed(laid_out: f32, target: f32) -> bool {
    (laid_out - target).abs() >= 0.5
}

/// A line's height: the specified `line-height`, else the font's `normal` line height.
pub fn line_height(font: &FontSpec) -> f32 {
    font.line_height
        .unwrap_or_else(|| font.size * normal_line_factor(font.family.as_deref()))
}

/// CSS `line-height: normal` as a multiple of the font size, for the families email uses.
///
/// Measured from the installed fonts with WebKit's macOS rule (`FontCocoa`): ascent + descent +
/// line gap, with Times, Helvetica and Courier given 15% extra ascent to match their Microsoft
/// metrics. The rule reproduces the published browser values (Arial 1.149, Georgia 1.136,
/// Verdana 1.215). Unlisted families use 1.2, the usual approximation. This replaces a flat
/// 1.45, which made every unstyled line 25–30% looser than Apple Mail.
pub fn normal_line_factor(family: Option<&str>) -> f32 {
    match primary_family(family).to_ascii_lowercase().as_str() {
        "helvetica" | "arial" | "times" | "times new roman" | "courier" => 1.15,
        "georgia" => 1.136,
        "verdana" => 1.215,
        "tahoma" => 1.207,
        "trebuchet ms" => 1.161,
        "courier new" => 1.133,
        "menlo" => 1.164,
        "helvetica neue" => 1.193,
        ".applesystemuifont" | "lucida grande" => 1.178,
        "palatino" => 1.1,
        "gill sans" => 1.148,
        "futura" => 1.328,
        "avenir" => 1.366,
        _ => 1.2,
    }
}

#[derive(Clone, Debug)]
enum Token {
    Word {
        text: String,
        font: FontSpec,
        color: Color,
        underline: bool,
        href: Option<String>,
        vertical_align: VerticalAlign,
    },
    Image {
        src: String,
        width: f32,
        height: f32,
        vertical_align: VerticalAlign,
    },
    Break,
}

#[derive(Clone, Debug)]
enum LineItem {
    Word {
        x: f32,
        text: String,
        font: FontSpec,
        color: Color,
        underline: bool,
        href: Option<String>,
        vertical_align: VerticalAlign,
    },
    Image {
        x: f32,
        src: String,
        width: f32,
        height: f32,
        vertical_align: VerticalAlign,
    },
}

#[derive(Clone, Debug, Default)]
struct Line {
    items: Vec<LineItem>,
    width: f32,
    height: f32,
}

/// Lay a list of top-level nodes into a display list of the given width.
pub fn layout(roots: &[Node], width: f32, measure: &dyn TextMeasure) -> Layout {
    let mut fragments = Vec::new();
    let base = Style::default();
    let height = layout_children(roots, 0.0, 0.0, width, &base, &mut fragments, measure);
    Layout {
        width,
        height,
        fragments,
    }
}

pub fn layout_with_limits(
    roots: &[Node],
    width: f32,
    measure: &dyn TextMeasure,
    limits: LayoutLimits,
) -> Result<Layout, LayoutError> {
    fn count(nodes: &[Node]) -> usize {
        nodes
            .iter()
            .map(|node| match node {
                Node::Text(_) => 1,
                Node::Element(element) => 1 + count(&element.children),
            })
            .sum()
    }
    let nodes = count(roots);
    if nodes > limits.max_nodes {
        return Err(LayoutError::NodeBudget {
            nodes,
            limit: limits.max_nodes,
        });
    }
    let started = Instant::now();
    let result = layout(roots, width, measure);
    if started.elapsed() > limits.max_duration {
        Err(LayoutError::TimeBudget)
    } else {
        Ok(result)
    }
}

fn layout_children(
    nodes: &[Node],
    x: f32,
    y: f32,
    width: f32,
    parent: &Style,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    let mut cursor = y;
    let mut tokens: Vec<Token> = Vec::new();
    let mut list_index = 0usize;

    for node in nodes {
        match node {
            Node::Text(text) => push_words(text, parent, None, &mut tokens),
            Node::Element(element) => {
                if element.style.display == Display::None {
                    continue;
                }
                if element.tag == "img" {
                    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                    cursor += layout_standalone_image(element, x, cursor, width, out);
                    continue;
                }
                if element.tag == "li" {
                    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                    list_index += 1;
                    cursor += layout_list_item(element, list_index, x, cursor, width, out, measure);
                    continue;
                }
                if element.style.display == Display::InlineBlock && has_box_decoration(element) {
                    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                    cursor += layout_inline_box(element, x, cursor, width, out, measure);
                    continue;
                }
                match element.style.display {
                    Display::Inline if element.tag == "br" => tokens.push(Token::Break),
                    Display::Inline | Display::InlineBlock if has_block_content(element) => {
                        cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                        cursor += layout_children(
                            &element.children,
                            x,
                            cursor,
                            width,
                            &element.style,
                            out,
                            measure,
                        );
                    }
                    Display::Inline | Display::InlineBlock => {
                        collect_inline(std::slice::from_ref(node), parent, None, width, &mut tokens)
                    }
                    Display::Table => {
                        cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                        cursor += layout_table(element, x, cursor, width, out, measure);
                    }
                    _ if element.tag == "hr" => {
                        cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                        out.push(Fragment::Border {
                            rect: Rect {
                                x,
                                y: cursor,
                                w: width,
                                h: 1.0,
                            },
                            color: [0xd9, 0xd4, 0xcc, 0xff],
                            width: 0.0,
                        });
                        cursor += 12.0;
                    }
                    _ => {
                        cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                        cursor += layout_block(element, x, cursor, width, out, measure);
                    }
                }
            }
        }
    }
    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
    cursor - y
}

fn has_box_decoration(element: &Element) -> bool {
    let style = &element.style;
    style.background.is_some()
        || style.background_image.is_some()
        || style.padding_top > 0.0
        || style.padding_right > 0.0
        || style.padding_bottom > 0.0
        || style.padding_left > 0.0
        || border_widths(style).iter().any(|width| *width > 0.0)
        || style.width.is_some()
        || style.width_percent.is_some()
        || style.height.is_some()
}

fn layout_inline_box(
    element: &Element,
    x: f32,
    y: f32,
    available: f32,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    let [border_top, border_right, border_bottom, border_left] = border_widths(&element.style);
    let horizontal_inset =
        border_left + border_right + element.style.padding_left + element.style.padding_right;
    let natural = natural_width(&element.children, measure) + horizontal_inset;
    let width = if element.style.width.is_some() || element.style.width_percent.is_some() {
        resolved_width(&element.style, available)
    } else {
        natural.min(available)
    }
    .max(horizontal_inset + 1.0);
    let box_x = match element.style.align {
        Align::Left => x,
        Align::Center => x + ((available - width) / 2.0).max(0.0),
        Align::Right => x + (available - width).max(0.0),
    };
    let mut tokens = Vec::new();
    collect_inline(
        &element.children,
        &element.style,
        element.attrs.get("href").map(String::as_str),
        (width - horizontal_inset).max(1.0),
        &mut tokens,
    );
    let mut inner = Vec::new();
    let content_height = flush(
        &mut tokens,
        box_x + border_left + element.style.padding_left,
        y + border_top + element.style.padding_top,
        (width - horizontal_inset).max(1.0),
        &element.style,
        &mut inner,
        measure,
    );
    let height = element
        .style
        .height
        .unwrap_or(
            content_height
                + border_top
                + border_bottom
                + element.style.padding_top
                + element.style.padding_bottom,
        )
        .max(1.0);
    let rect = Rect {
        x: box_x,
        y,
        w: width,
        h: height,
    };
    if let Some(color) = element.style.background {
        out.push(Fragment::Rect {
            rect,
            color,
            radius: 0.0,
        });
    }
    if let Some(src) = &element.style.background_image {
        out.push(Fragment::Image {
            rect,
            src: src.clone(),
        });
    }
    emit_border(out, rect, &element.style);
    out.extend(inner);
    element.style.margin_top + height + element.style.margin_bottom
}

fn layout_list_item(
    element: &Element,
    index: usize,
    x: f32,
    y: f32,
    width: f32,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    let marker = match element.style.list_style {
        ListStyle::None => None,
        ListStyle::Disc => Some("•".to_string()),
        ListStyle::Decimal => Some(format!("{index}.")),
    };
    let indent = if marker.is_some() { 22.0 } else { 0.0 };
    if let Some(marker) = marker {
        let font = FontSpec {
            family: element.style.font_family.clone(),
            size: element.style.font_size,
            bold: element.style.bold,
            italic: element.style.italic,
            line_height: element.style.line_height,
        };
        out.push(Fragment::Word {
            x,
            y,
            text: marker,
            font,
            color: element.style.color,
            underline: false,
            href: None,
        });
    }
    let height = layout_children(
        &element.children,
        x + indent,
        y,
        (width - indent).max(0.0),
        &element.style,
        out,
        measure,
    );
    height.max(line_height(&FontSpec {
        family: element.style.font_family.clone(),
        size: element.style.font_size,
        bold: element.style.bold,
        italic: element.style.italic,
        line_height: element.style.line_height,
    }))
}

/// The box reserved for an image whose pixels are not known yet (a blocked remote image, or one
/// still loading). Only its *aspect ratio* matters once the sender specifies a dimension.
pub const PLACEHOLDER_IMAGE: (f32, f32) = (240.0, 140.0);

/// CSS sizing for a replaced element (`<img>`), per CSS 2.1 §10.3.2 / §10.6.2 and the
/// `max-width` rules of §10.4 — the same algorithm WebKit, and therefore Apple Mail, runs:
///
/// - both dimensions specified: use them as given (the image is then stretched to fill);
/// - one specified: complete the other from the image's **natural aspect ratio**;
/// - neither: use the natural size;
/// - then `max-width` clamps the width, and the height follows the ratio unless it was specified.
///
/// Percentage heights are treated as `auto`: email containers have no definite height, which is
/// exactly the case CSS resolves a percentage height to `auto`.
fn replaced_size(element: &Element, available: f32) -> (f32, f32) {
    let style = &element.style;
    let specified_w = style
        .width
        .or_else(|| style.width_percent.map(|percent| available * percent));
    let specified_h = style.height;
    let (mut width, mut height) = match (style.natural_size, specified_w, specified_h) {
        (_, Some(width), Some(height)) => (width, height),
        // The pixels are known: complete a missing dimension from the real aspect ratio.
        (Some((natural_w, natural_h)), Some(width), None) => {
            (width, width * ratio(natural_w, natural_h))
        }
        (Some((natural_w, natural_h)), None, Some(height)) => {
            (height / ratio(natural_w, natural_h), height)
        }
        (Some(natural), None, None) => natural,
        // Not known yet (blocked or still loading): reserve a guessed box. This is the only
        // place a guess is allowed, and it is replaced by the real size as soon as the image
        // decodes.
        (None, Some(width), None) => (width, placeholder_height(element, width)),
        (None, None, Some(height)) => (height, height),
        (None, None, None) => PLACEHOLDER_IMAGE,
    };
    let max_width = style
        .max_width
        .or_else(|| style.max_width_percent.map(|percent| available * percent));
    if let Some(max_width) = max_width
        && width > max_width
        && width > 0.0
    {
        // `max-width` clamps the width; an auto height follows proportionally, while a
        // specified height stays as specified (so the image stretches, as it does in a browser).
        if specified_h.is_none() {
            height *= max_width / width;
        }
        width = max_width;
    }
    (width.max(0.0), height.max(0.0))
}

fn ratio(width: f32, height: f32) -> f32 {
    if width > 0.0 { height / width } else { 1.0 }
}

/// A guessed height for an image of known width but unknown pixels. Wide images named like
/// logos or headers are usually banners, so a banner-shaped box reserves far less empty space.
/// Otherwise the placeholder's fixed height: a blocked image must never reserve *more* space
/// than a fixed placeholder does, or a newsletter full of blocked heroes becomes mostly gaps.
fn placeholder_height(element: &Element, width: f32) -> f32 {
    let hint = format!(
        "{} {}",
        element.attrs.get("src").map(String::as_str).unwrap_or(""),
        element.attrs.get("alt").map(String::as_str).unwrap_or("")
    )
    .to_ascii_lowercase();
    if width >= 200.0 && (hint.contains("logo") || hint.contains("header")) {
        (width * 0.12).clamp(32.0, 120.0)
    } else {
        PLACEHOLDER_IMAGE
            .1
            .min(width * ratio(PLACEHOLDER_IMAGE.0, PLACEHOLDER_IMAGE.1))
    }
}

fn layout_standalone_image(
    element: &Element,
    x: f32,
    y: f32,
    available: f32,
    out: &mut Vec<Fragment>,
) -> f32 {
    let (mut width, mut height) = replaced_size(element, available);
    if width > available {
        let scale = available / width;
        width = available;
        height *= scale;
    }
    let image_x = match element.style.align {
        Align::Left => x,
        Align::Center => x + ((available - width) / 2.0).max(0.0),
        Align::Right => x + (available - width).max(0.0),
    };
    out.push(Fragment::Image {
        rect: Rect {
            x: image_x,
            y,
            w: width,
            h: height,
        },
        src: element.attrs.get("src").cloned().unwrap_or_default(),
    });
    height
}

fn layout_block(
    element: &Element,
    x: f32,
    y: f32,
    available: f32,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    let style = &element.style;
    let box_w = resolved_width(style, available);
    let box_x = if style.margin_auto_horizontal {
        x + ((available - box_w) / 2.0).max(0.0)
    } else {
        x
    };
    let [border_top, border_right, border_bottom, border_left] = border_widths(style);
    let horizontal_inset = border_left + border_right + style.padding_left + style.padding_right;
    let vertical_inset = border_top + border_bottom + style.padding_top + style.padding_bottom;
    let content_w = (box_w - horizontal_inset).max(0.0);
    let content_x = box_x + border_left + style.padding_left;
    let content_y = y + style.margin_top + border_top + style.padding_top;

    // Lay content out first so the background can be emitted behind it.
    let mut inner = Vec::new();
    let inner_h = layout_children(
        &element.children,
        content_x,
        content_y,
        content_w,
        style,
        &mut inner,
        measure,
    );
    let requested_height = style
        .height
        .or_else(|| style.height_percent.map(|percent| available * percent))
        .unwrap_or(0.0);
    let total_h = (inner_h + vertical_inset).max(requested_height);

    if let Some(background) = style.background {
        out.push(Fragment::Rect {
            rect: Rect {
                x: box_x,
                y: y + style.margin_top,
                w: box_w,
                h: total_h,
            },
            color: background,
            radius: 0.0,
        });
    }
    // A CSS background image sits above the colour and below the content (E6.8).
    if let Some(src) = &style.background_image {
        out.push(Fragment::Image {
            rect: Rect {
                x: box_x,
                y: y + style.margin_top,
                w: box_w,
                h: total_h,
            },
            src: src.clone(),
        });
    }
    emit_border(
        out,
        Rect {
            x: box_x,
            y: y + style.margin_top,
            w: box_w,
            h: total_h,
        },
        style,
    );
    out.append(&mut inner);
    style.margin_top + total_h + style.margin_bottom
}

fn resolved_width(style: &Style, available: f32) -> f32 {
    let width = style
        .width
        .or_else(|| style.width_percent.map(|percent| available * percent))
        .unwrap_or(available);
    let max = style
        .max_width
        .or_else(|| style.max_width_percent.map(|percent| available * percent))
        .unwrap_or(available);
    width.min(max).min(available).max(0.0)
}

fn border_widths(style: &Style) -> [f32; 4] {
    [
        style.border_top.unwrap_or(style.border),
        style.border_right.unwrap_or(style.border),
        style.border_bottom.unwrap_or(style.border),
        style.border_left.unwrap_or(style.border),
    ]
}

fn emit_border(out: &mut Vec<Fragment>, rect: Rect, style: &Style) {
    let [top, right, bottom, left] = border_widths(style);
    if top == right && right == bottom && bottom == left {
        if top > 0.0 {
            out.push(Fragment::Border {
                rect,
                color: style.border_color,
                width: top,
            });
        }
        return;
    }
    for edge in [
        Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: top,
        },
        Rect {
            x: rect.x + rect.w - right,
            y: rect.y,
            w: right,
            h: rect.h,
        },
        Rect {
            x: rect.x,
            y: rect.y + rect.h - bottom,
            w: rect.w,
            h: bottom,
        },
        Rect {
            x: rect.x,
            y: rect.y,
            w: left,
            h: rect.h,
        },
    ] {
        if edge.w > 0.0 && edge.h > 0.0 {
            out.push(Fragment::Rect {
                rect: edge,
                color: style.border_color,
                radius: 0.0,
            });
        }
    }
}

fn layout_table(
    table: &Element,
    x: f32,
    y: f32,
    available: f32,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    let rows: Vec<&Element> = collect_rows(table);
    if rows.is_empty() {
        return 0.0;
    }
    let row_cells: Vec<Vec<&Element>> = rows
        .iter()
        .map(|row| {
            row.children
                .iter()
                .filter_map(as_element)
                .filter(|cell| cell.style.display == Display::TableCell)
                .collect()
        })
        .collect();
    let (grid, columns) = build_table_grid(&row_cells);
    let spacing = table.style.cell_spacing.max(0.0);
    let horizontal_spacing = spacing * (columns as f32 + 1.0);

    let mut widths = vec![0f32; columns];
    let mut fixed = vec![false; columns];
    let [
        table_border_top,
        table_border_right,
        table_border_bottom,
        table_border_left,
    ] = border_widths(&table.style);
    let table_inset = table_border_left
        + table_border_right
        + table.style.padding_left
        + table.style.padding_right;
    let provisional_target = (available - table_inset - horizontal_spacing).max(1.0);
    for placement in &grid {
        let cell = placement.element;
        let [_, border_right, _, border_left] = border_widths(&cell.style);
        let inset = border_left + border_right + cell.style.padding_left + cell.style.padding_right;
        let natural = natural_width(&cell.children, measure) + inset;
        let preferred = cell
            .style
            .width
            .or_else(|| {
                cell.style
                    .width_percent
                    .map(|percent| provisional_target * percent)
            })
            .map(|width| width + inset);
        let desired = preferred.unwrap_or(natural);
        let end = (placement.column + placement.colspan).min(columns);
        let current: f32 = widths[placement.column..end].iter().sum();
        if desired > current {
            let extra = (desired - current) / (end - placement.column).max(1) as f32;
            for width in &mut widths[placement.column..end] {
                *width += extra;
            }
        }
        if preferred.is_some() {
            for column in placement.column..end {
                fixed[column] = true;
            }
        }
    }
    let total: f32 = widths.iter().sum();
    let auto_width = table
        .attrs
        .get("width")
        .is_some_and(|width| width.eq_ignore_ascii_case("auto"))
        || table.attrs.get("style").is_some_and(|style| {
            style
                .split(';')
                .any(|part| part.trim().eq_ignore_ascii_case("width:auto"))
        });
    let table_width = if auto_width {
        (total + table_inset + horizontal_spacing).min(available)
    } else {
        resolved_width(&table.style, available)
    }
    .max(table_inset + 1.0);
    let target = (table_width - table_inset - horizontal_spacing).max(1.0);
    if total <= f32::EPSILON {
        widths.fill(target / columns as f32);
    } else if total > target {
        let scale = target / total;
        for width in &mut widths {
            *width *= scale;
        }
    } else if total < target && !auto_width {
        let flexible = fixed.iter().filter(|fixed| !**fixed).count();
        if flexible > 0 {
            let extra = (target - total) / flexible as f32;
            for (width, fixed) in widths.iter_mut().zip(&fixed) {
                if !fixed {
                    *width += extra;
                }
            }
        } else {
            let extra = (target - total) / columns as f32;
            for width in &mut widths {
                *width += extra;
            }
        }
    }

    let table_x = match if table.style.margin_auto_horizontal {
        Align::Center
    } else {
        table.style.align
    } {
        Align::Left => x,
        Align::Center => x + ((available - table_width) / 2.0).max(0.0),
        Align::Right => x + (available - table_width).max(0.0),
    };
    let top = y + table.style.margin_top;
    let content_top = top + table_border_top + table.style.padding_top;
    let content_left = table_x + table_border_left + table.style.padding_left;
    let mut column_offsets = Vec::with_capacity(columns + 1);
    column_offsets.push(spacing);
    for width in &widths {
        column_offsets.push(column_offsets.last().copied().unwrap_or(0.0) + width + spacing);
    }

    struct LaidCell<'a> {
        placement: &'a GridCell<'a>,
        x: f32,
        width: f32,
        natural_height: f32,
        inner: Vec<Fragment>,
    }
    let mut laid = Vec::with_capacity(grid.len());
    let mut row_heights = vec![0.0f32; rows.len()];
    for placement in &grid {
        let cell = placement.element;
        let end_column = (placement.column + placement.colspan).min(columns);
        let cell_x = content_left + column_offsets[placement.column];
        let width = column_offsets[end_column] - column_offsets[placement.column] - spacing;
        let [border_top, border_right, border_bottom, border_left] = border_widths(&cell.style);
        let horizontal_inset =
            border_left + border_right + cell.style.padding_left + cell.style.padding_right;
        let vertical_inset =
            border_top + border_bottom + cell.style.padding_top + cell.style.padding_bottom;
        let mut inner = Vec::new();
        let inner_h = layout_children(
            &cell.children,
            cell_x + border_left + cell.style.padding_left,
            border_top + cell.style.padding_top,
            (width - horizontal_inset).max(0.0),
            &cell.style,
            &mut inner,
            measure,
        );
        let requested = cell
            .style
            .height
            .or_else(|| cell.style.height_percent.map(|percent| available * percent))
            .unwrap_or(0.0);
        let natural_height = (inner_h + vertical_inset).max(requested);
        if placement.rowspan == 1 {
            row_heights[placement.row] = row_heights[placement.row].max(natural_height);
        }
        laid.push(LaidCell {
            placement,
            x: cell_x,
            width,
            natural_height,
            inner,
        });
    }
    for cell in laid.iter().filter(|cell| cell.placement.rowspan > 1) {
        let end = (cell.placement.row + cell.placement.rowspan).min(row_heights.len());
        let current: f32 = row_heights[cell.placement.row..end].iter().sum();
        if cell.natural_height > current {
            let extra = (cell.natural_height - current) / (end - cell.placement.row).max(1) as f32;
            for height in &mut row_heights[cell.placement.row..end] {
                *height += extra;
            }
        }
    }
    for height in &mut row_heights {
        *height = height.max(1.0);
    }
    let mut row_offsets = Vec::with_capacity(row_heights.len() + 1);
    row_offsets.push(spacing);
    for height in &row_heights {
        row_offsets.push(row_offsets.last().copied().unwrap_or(0.0) + height + spacing);
    }

    for mut cell in laid {
        let placement = cell.placement;
        let element = placement.element;
        let end_row = (placement.row + placement.rowspan).min(row_heights.len());
        let cell_y = content_top + row_offsets[placement.row];
        let height = row_offsets[end_row] - row_offsets[placement.row] - spacing;
        let spare = (height - cell.natural_height).max(0.0);
        let vertical_shift = match element.style.vertical_align {
            VerticalAlign::Top | VerticalAlign::Baseline => 0.0,
            VerticalAlign::Middle => spare / 2.0,
            VerticalAlign::Bottom => spare,
        };
        translate_fragments(&mut cell.inner, 0.0, cell_y + vertical_shift);
        let rect = Rect {
            x: cell.x,
            y: cell_y,
            w: cell.width,
            h: height,
        };
        if let Some(background) = element.style.background {
            out.push(Fragment::Rect {
                rect,
                color: background,
                radius: 0.0,
            });
        }
        if let Some(src) = &element.style.background_image {
            out.push(Fragment::Image {
                rect,
                src: src.clone(),
            });
        }
        emit_border(out, rect, &element.style);
        out.extend(cell.inner);
    }

    let content_height = row_offsets.last().copied().unwrap_or(0.0);
    let table_height = table_border_top
        + table.style.padding_top
        + content_height
        + table.style.padding_bottom
        + table_border_bottom;
    emit_border(
        out,
        Rect {
            x: table_x,
            y: top,
            w: table_width,
            h: table_height,
        },
        &table.style,
    );
    table.style.margin_top + table_height + table.style.margin_bottom
}

struct GridCell<'a> {
    element: &'a Element,
    row: usize,
    column: usize,
    colspan: usize,
    rowspan: usize,
}

fn build_table_grid<'a>(rows: &'a [Vec<&'a Element>]) -> (Vec<GridCell<'a>>, usize) {
    let mut grid = Vec::new();
    let mut occupied: Vec<usize> = Vec::new();
    let mut columns = 0usize;
    for (row_index, row) in rows.iter().enumerate() {
        let mut column = 0usize;
        for element in row {
            let colspan = element.style.colspan.max(1);
            let rowspan = element.style.rowspan.max(1);
            loop {
                if occupied.len() < column + colspan {
                    occupied.resize(column + colspan, 0);
                }
                if occupied[column..column + colspan]
                    .iter()
                    .all(|remaining| *remaining == 0)
                {
                    break;
                }
                column += 1;
            }
            for remaining in &mut occupied[column..column + colspan] {
                *remaining = (*remaining).max(rowspan);
            }
            grid.push(GridCell {
                element,
                row: row_index,
                column,
                colspan,
                rowspan,
            });
            column += colspan;
            columns = columns.max(column);
        }
        for remaining in &mut occupied {
            *remaining = remaining.saturating_sub(1);
        }
        columns = columns.max(occupied.len());
    }
    (grid, columns.max(1))
}

fn translate_fragments(fragments: &mut [Fragment], dx: f32, dy: f32) {
    for fragment in fragments {
        match fragment {
            Fragment::Rect { rect, .. }
            | Fragment::Border { rect, .. }
            | Fragment::Image { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
            Fragment::Word { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
}

fn collect_rows(table: &Element) -> Vec<&Element> {
    let mut rows = Vec::new();
    for child in table.children.iter().filter_map(as_element) {
        match child.style.display {
            Display::TableRow => rows.push(child),
            // Row groups may sit between a table and its rows. Never recurse through cells: doing
            // so flattens rows from a nested table into the parent table and lays them out twice.
            Display::TableRowGroup => rows.extend(collect_rows(child)),
            _ => {}
        }
    }
    rows
}

fn as_element(node: &Node) -> Option<&Element> {
    match node {
        Node::Element(element) => Some(element),
        Node::Text(_) => None,
    }
}

fn has_block_content(element: &Element) -> bool {
    element.children.iter().any(|child| match child {
        Node::Element(child) => !matches!(
            child.style.display,
            Display::Inline | Display::InlineBlock | Display::None
        ),
        Node::Text(_) => false,
    })
}

/// Longest word width, capped by a single-line width so short cells do not become a whole column.
fn natural_width(nodes: &[Node], measure: &dyn TextMeasure) -> f32 {
    fn visit(nodes: &[Node], style: &Style, measure: &dyn TextMeasure) -> (f32, f32) {
        let mut longest = 0f32;
        let mut total = 0f32;
        for node in nodes {
            match node {
                Node::Text(text) => {
                    let font = FontSpec {
                        family: style.font_family.clone(),
                        size: style.font_size,
                        bold: style.bold,
                        italic: style.italic,
                        line_height: style.line_height,
                    };
                    for word in text.split_whitespace() {
                        let width = measure.width(word, &font);
                        longest = longest.max(width);
                        total += width + measure.width(" ", &font);
                    }
                }
                Node::Element(element) if element.style.display == Display::None => {}
                Node::Element(element) if element.tag == "img" => {
                    // Measure with percentages set aside: they resolve against the container,
                    // which is exactly what is being measured here.
                    let mut fixed = element.clone();
                    fixed.style.width_percent = None;
                    fixed.style.max_width_percent = None;
                    let (width, _) = replaced_size(&fixed, 0.0);
                    // Per CSS Sizing, an image whose width or max-width is a percentage is
                    // "compressible": it contributes nothing to min-content and its size to
                    // max-content. (This used to resolve percentages against a guessed 220px,
                    // which under-sized any auto-layout column holding a full-width image.)
                    let compressible = element.style.width_percent.is_some()
                        || element.style.max_width_percent.is_some();
                    if !compressible {
                        longest = longest.max(width);
                    }
                    total += width;
                }
                Node::Element(element) => {
                    let (child_longest, child_total) =
                        visit(&element.children, &element.style, measure);
                    let [_, border_right, _, border_left] = border_widths(&element.style);
                    let inset = border_left
                        + border_right
                        + element.style.padding_left
                        + element.style.padding_right;
                    longest = longest.max(child_longest + inset);
                    total += child_total + inset;
                }
            }
        }
        (longest, total)
    }
    let (longest, total) = visit(nodes, &Style::default(), measure);
    longest.max(total.min(220.0))
}

fn collect_inline(
    nodes: &[Node],
    parent: &Style,
    href: Option<&str>,
    available: f32,
    out: &mut Vec<Token>,
) {
    for node in nodes {
        match node {
            Node::Text(text) => push_words(text, parent, href, out),
            Node::Element(element) => match element.style.display {
                Display::None => {}
                Display::Inline if element.tag == "br" => out.push(Token::Break),
                Display::Inline if element.tag == "img" => {
                    let (width, height) = replaced_size(element, available);
                    out.push(Token::Image {
                        src: element.attrs.get("src").cloned().unwrap_or_default(),
                        width,
                        height,
                        vertical_align: element.style.vertical_align,
                    });
                }
                Display::Inline | Display::InlineBlock => {
                    let child_href = element.attrs.get("href").map(String::as_str).or(href);
                    collect_inline(
                        &element.children,
                        &element.style,
                        child_href,
                        available,
                        out,
                    )
                }
                Display::Block => {
                    if !out.is_empty() {
                        out.push(Token::Break);
                    }
                    collect_inline(&element.children, &element.style, href, available, out);
                    out.push(Token::Break);
                }
                _ => {}
            },
        }
    }
}

fn push_words(text: &str, style: &Style, href: Option<&str>, out: &mut Vec<Token>) {
    let font = FontSpec {
        family: style.font_family.clone(),
        size: style.font_size,
        bold: style.bold,
        italic: style.italic,
        line_height: style.line_height,
    };
    for word in text.split_whitespace() {
        out.push(Token::Word {
            text: word.to_string(),
            font: font.clone(),
            color: style.color,
            underline: style.underline,
            href: href.map(str::to_string),
            vertical_align: style.vertical_align,
        });
    }
}

/// Lay out the pending inline tokens as one paragraph; returns its height.
fn flush(
    tokens: &mut Vec<Token>,
    x: f32,
    y: f32,
    width: f32,
    style: &Style,
    out: &mut Vec<Fragment>,
    measure: &dyn TextMeasure,
) -> f32 {
    if tokens.is_empty() {
        return 0.0;
    }
    let tokens = std::mem::take(tokens);
    let mut lines: Vec<Line> = Vec::new();
    let mut line = Line::default();
    for token in tokens {
        match token {
            Token::Break => {
                line.height = line.height.max(line_height(&FontSpec {
                    family: style.font_family.clone(),
                    size: style.font_size,
                    bold: style.bold,
                    italic: style.italic,
                    line_height: style.line_height,
                }));
                lines.push(line);
                line = Line::default();
            }
            Token::Word {
                text,
                font,
                color,
                underline,
                href,
                vertical_align,
            } => {
                let word_width = measure.width(&text, &font);
                let space = if line.items.is_empty() {
                    0.0
                } else {
                    measure.width(" ", &font)
                };
                if !line.items.is_empty() && line.width + space + word_width > width {
                    lines.push(line);
                    line = Line::default();
                }
                let space = if line.items.is_empty() {
                    0.0
                } else {
                    measure.width(" ", &font)
                };
                let item_x = line.width + space;
                line.width = item_x + word_width;
                line.height = line.height.max(line_height(&font));
                line.items.push(LineItem::Word {
                    x: item_x,
                    text,
                    font,
                    color,
                    underline,
                    href,
                    vertical_align,
                });
            }
            Token::Image {
                src,
                width: mut image_width,
                height: mut image_height,
                vertical_align,
            } => {
                if image_width > width {
                    let scale = width / image_width;
                    image_width = width;
                    image_height *= scale;
                }
                if !line.items.is_empty() && line.width + image_width > width {
                    lines.push(line);
                    line = Line::default();
                }
                let item_x = line.width;
                line.width += image_width;
                line.height = line.height.max(image_height);
                line.items.push(LineItem::Image {
                    x: item_x,
                    src,
                    width: image_width,
                    height: image_height,
                    vertical_align,
                });
            }
        }
    }
    if !line.items.is_empty() {
        lines.push(line);
    }

    let mut cursor = y;
    for line in lines {
        let offset = match style.align {
            Align::Left => 0.0,
            Align::Center => ((width - line.width) / 2.0).max(0.0),
            Align::Right => (width - line.width).max(0.0),
        };
        for item in line.items {
            match item {
                LineItem::Word {
                    x: item_x,
                    text,
                    font,
                    color,
                    underline,
                    href,
                    vertical_align,
                } => out.push(Fragment::Word {
                    x: x + offset + item_x,
                    y: cursor + vertical_offset(vertical_align, line.height, line_height(&font)),
                    text,
                    font,
                    color,
                    underline,
                    href,
                }),
                LineItem::Image {
                    x: item_x,
                    src,
                    width,
                    height,
                    vertical_align,
                } => out.push(Fragment::Image {
                    rect: Rect {
                        x: x + offset + item_x,
                        y: cursor + vertical_offset(vertical_align, line.height, height),
                        w: width,
                        h: height,
                    },
                    src,
                }),
            }
        }
        cursor += line.height.max(line_height(&FontSpec {
            family: style.font_family.clone(),
            size: style.font_size,
            bold: style.bold,
            italic: style.italic,
            line_height: style.line_height,
        }));
    }
    cursor - y
}

fn vertical_offset(align: VerticalAlign, line_height: f32, item_height: f32) -> f32 {
    let spare = (line_height - item_height).max(0.0);
    match align {
        VerticalAlign::Top => 0.0,
        VerticalAlign::Middle => spare / 2.0,
        VerticalAlign::Bottom | VerticalAlign::Baseline => spare,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::resolve_style;
    use std::collections::HashMap;

    /// Every glyph is 10px wide and every space 5px; deterministic and GPUI-free.
    struct Fake;
    impl TextMeasure for Fake {
        fn width(&self, text: &str, _font: &FontSpec) -> f32 {
            text.chars().count() as f32 * 10.0
        }
    }

    fn element(tag: &str, children: Vec<Node>) -> Element {
        let style = resolve_style(tag, &HashMap::new(), &Style::default());
        Element {
            tag: tag.to_string(),
            attrs: HashMap::new(),
            style,
            children,
        }
    }

    #[test]
    fn paragraph_wraps_at_the_available_width() {
        // five 5-char words at 50px each = 250px + spaces; needs >= 2 lines at 200px.
        let text = "aaaaa bbbbb ccccc ddddd".to_string();
        let layout = layout(&[Node::Text(text)], 200.0, &Fake);
        let ys: std::collections::BTreeSet<i32> =
            layout.words().map(|(_, _, y, _)| y as i32).collect();
        assert!(ys.len() >= 2, "expected wrapping, got {ys:?}");
        for (_, x, _, _) in layout.words() {
            assert!(x <= 200.0, "word at x={x} escaped the column");
        }
    }

    #[test]
    fn center_alignment_offsets_the_line() {
        let mut p = element("p", vec![Node::Text("aa".to_string())]);
        p.style.align = Align::Center;
        let layout = layout(&[Node::Element(p)], 100.0, &Fake);
        let x = layout.words().next().unwrap().1;
        // "aa" is 20px, so centered in 100px means x = 40.
        assert!((x - 40.0).abs() < 0.5, "x={x}");
    }

    #[test]
    fn table_columns_fill_the_available_width() {
        let row = |cells: &[&str]| {
            let mut tr = element("tr", vec![]);
            for cell in cells {
                tr.children.push(Node::Element(element(
                    "td",
                    vec![Node::Text(cell.to_string())],
                )));
            }
            Node::Element(tr)
        };
        let mut table = element("table", vec![row(&["a", "b", "c"])]);
        table.style.border = 1.0;
        let layout = layout(&[Node::Element(table)], 300.0, &Fake);
        let words = layout.words().count();
        assert_eq!(words, 3);
        // All three columns are on one row (same y).
        let ys: std::collections::BTreeSet<i32> =
            layout.words().map(|(_, _, y, _)| y as i32).collect();
        assert_eq!(ys.len(), 1);
        // The widest cell's x is within the table width.
        for (_, x, _, _) in layout.words() {
            assert!(x < 300.0, "cell word at x={x}");
        }
    }

    #[test]
    fn background_and_border_are_emitted_for_a_cell() {
        let mut td = element("td", vec![Node::Text("x".to_string())]);
        td.style.background = Some([1, 2, 3, 255]);
        let mut tr = element("tr", vec![]);
        tr.children.push(Node::Element(td));
        let table = element("table", vec![Node::Element(tr)]);
        let layout = layout(&[Node::Element(table)], 200.0, &Fake);
        assert!(
            layout
                .fragments
                .iter()
                .any(|f| matches!(f, Fragment::Rect { color, .. } if *color == [1, 2, 3, 255])),
            "cell background missing"
        );
    }

    #[test]
    fn nested_table_rows_are_not_flattened_or_painted_twice() {
        let row = |cells: &[&str]| {
            let mut tr = element("tr", vec![]);
            for cell in cells {
                tr.children.push(Node::Element(element(
                    "td",
                    vec![Node::Text(cell.to_string())],
                )));
            }
            Node::Element(tr)
        };
        let nested = element("table", vec![row(&["left", "right"])]);
        let outer = element(
            "table",
            vec![Node::Element(element(
                "tr",
                vec![Node::Element(element("td", vec![Node::Element(nested)]))],
            ))],
        );

        let layout = layout(&[Node::Element(outer)], 200.0, &Fake);
        let words: Vec<_> = layout.words().map(|(word, _, _, _)| word).collect();
        assert_eq!(words, ["left", "right"]);
    }

    #[test]
    fn images_wrapped_in_links_participate_in_inline_layout() {
        let mut image = element("img", vec![]);
        image.style.width = Some(80.0);
        image.style.height = Some(40.0);
        image.attrs.insert("src".into(), "cid:hero".into());
        let link = element("a", vec![Node::Element(image)]);

        let layout = layout(&[Node::Element(link)], 200.0, &Fake);
        assert!(matches!(
            layout.fragments.as_slice(),
            [Fragment::Image { rect, src }] if *rect == Rect { x: 0.0, y: 0.0, w: 80.0, h: 40.0 } && src == "cid:hero"
        ));
        assert_eq!(layout.height, 40.0);
    }

    #[test]
    fn document_width_is_the_panes_content_width_floored_with_a_minimum() {
        assert_eq!(document_width(812.6), 812.0);
        assert_eq!(document_width(1400.0), 1400.0);
        // Narrow panes and the zero width of a pane that is not laid out yet hit the floor.
        assert_eq!(document_width(200.0), MIN_DOCUMENT_WIDTH);
        assert_eq!(document_width(0.0), MIN_DOCUMENT_WIDTH);
        assert_eq!(document_width(f32::NAN), DEFAULT_DOCUMENT_WIDTH);
        assert_eq!(document_width(f32::INFINITY), DEFAULT_DOCUMENT_WIDTH);
    }

    #[test]
    fn only_whole_pixel_width_changes_trigger_a_relayout() {
        assert!(!width_changed(640.0, 640.0));
        assert!(!width_changed(640.0, 640.4));
        assert!(width_changed(640.0, 641.0));
        assert!(width_changed(900.0, 640.0));
        // Sub-pixel jitter in the measured width never survives `document_width`.
        assert!(!width_changed(document_width(812.2), document_width(812.9)));
    }

    // A document takes the width it is given: text wraps to it and a full-width image fills it,
    // which is what lets the reading pane use its real width instead of a fixed 640px.
    #[test]
    fn layout_uses_the_width_it_is_given() {
        let paragraph = || Node::Element(element("p", vec![Node::Text("word ".repeat(80))]));
        let narrow = layout(&[paragraph()], 400.0, &Fake);
        let wide = layout(&[paragraph()], 1000.0, &Fake);
        assert!(narrow.height > wide.height);
        // `words()` yields each word's origin; `Fake` sets every glyph 10px wide.
        let right_edge = |layout: &Layout| {
            layout
                .words()
                .map(|(word, x, _, _)| x + word.chars().count() as f32 * 10.0)
                .fold(0.0, f32::max)
        };
        assert!(right_edge(&narrow) <= 400.0);
        assert!(right_edge(&wide) <= 1000.0 && right_edge(&wide) > 400.0);

        let mut image = element("img", vec![]);
        image.style.display = Display::Block;
        image.style.width_percent = Some(1.0);
        image.style.natural_size = Some((1200.0, 600.0));
        let rect = image_box(image, 900.0);
        assert_eq!((rect.w, rect.h), (900.0, 450.0));
    }

    // `line-height: normal` is per font, as WebKit measures it; it was a flat 1.45.
    #[test]
    fn normal_line_height_follows_the_font() {
        let spec = |family: Option<&str>| FontSpec {
            family: family.map(str::to_string),
            size: 20.0,
            bold: false,
            italic: false,
            line_height: None,
        };
        assert!((line_height(&spec(Some("Helvetica"))) - 23.0).abs() < 0.01);
        assert!((line_height(&spec(Some("georgia"))) - 22.72).abs() < 0.01);
        assert!((line_height(&spec(Some("Verdana, Arial"))) - 24.3).abs() < 0.01);
        // The default family is Helvetica, so no family means Helvetica's metrics.
        assert!((line_height(&spec(None)) - 23.0).abs() < 0.01);
        assert!((line_height(&spec(Some("Some Unknown Face"))) - 24.0).abs() < 0.01);
        // A specified line height always wins.
        let explicit = FontSpec {
            line_height: Some(31.0),
            ..spec(Some("Helvetica"))
        };
        assert_eq!(line_height(&explicit), 31.0);
    }

    /// The single image box a lone `<img>` lays out to.
    fn image_box(image: Element, available: f32) -> Rect {
        let layout = layout(&[Node::Element(image)], available, &Fake);
        match layout.fragments.as_slice() {
            [Fragment::Image { rect, .. }] => *rect,
            other => panic!("expected one image fragment, got {other:?}"),
        }
    }

    fn block_image(natural: Option<(f32, f32)>) -> Element {
        let mut image = element("img", vec![]);
        image.style.display = Display::Block;
        image.style.natural_size = natural;
        image
    }

    // Regression (NYT breaking-news hero): `width="600" style="width:100%;height:auto"` on a
    // 1050x700 photo. This rendered at 240x140 — the placeholder — even after the image loaded,
    // and GPUI's `Contain` fit then shrank the photo inside that box to about 210x140.
    #[test]
    fn width_only_image_takes_its_height_from_the_natural_aspect_ratio() {
        let mut image = block_image(Some((1050.0, 700.0)));
        image.style.width_percent = Some(1.0);
        let rect = image_box(image, 600.0);
        assert_eq!((rect.w, rect.h), (600.0, 400.0));
    }

    // Regression (LinkedIn header icons): only `height="25"`. The missing width used to fall to
    // the 240px placeholder, putting three 25px icons in 720px of a 512px email.
    #[test]
    fn height_only_image_takes_its_width_from_the_natural_aspect_ratio() {
        let mut image = block_image(Some((50.0, 50.0)));
        image.style.height = Some(25.0);
        let rect = image_box(image, 512.0);
        assert_eq!((rect.w, rect.h), (25.0, 25.0));
    }

    // Regression (NYT header): `max-width:600px` on a 1200x110 retina asset, no height. The
    // width clamps and the auto height must follow it down rather than stay at the natural 110.
    #[test]
    fn max_width_clamp_carries_an_auto_height_down_proportionally() {
        let mut image = block_image(Some((1200.0, 110.0)));
        image.style.max_width = Some(600.0);
        let rect = image_box(image, 640.0);
        assert_eq!((rect.w, rect.h), (600.0, 55.0));
    }

    // Regression (NYT footer "Zeta" / AdChoices logos): `width="150"` on a 300x30 image. These
    // were padded out to 140px tall, which is most of the "huge spacing between items".
    #[test]
    fn small_width_only_logo_does_not_reserve_a_placeholder_height_once_loaded() {
        let mut image = block_image(Some((300.0, 30.0)));
        image.style.width = Some(150.0);
        let rect = image_box(image, 600.0);
        assert_eq!((rect.w, rect.h), (150.0, 15.0));
    }

    #[test]
    fn unsized_image_uses_its_natural_size() {
        let rect = image_box(block_image(Some((320.0, 180.0))), 600.0);
        assert_eq!((rect.w, rect.h), (320.0, 180.0));
    }

    // Both dimensions specified is the one case a browser distorts rather than preserving the
    // ratio; matching it is what makes `object-fit: fill` the correct paint mode.
    #[test]
    fn fully_specified_image_keeps_the_senders_box() {
        let mut image = block_image(Some((1050.0, 700.0)));
        image.style.width = Some(200.0);
        image.style.height = Some(50.0);
        let rect = image_box(image, 600.0);
        assert_eq!((rect.w, rect.h), (200.0, 50.0));
    }

    #[test]
    fn wide_logo_without_height_uses_a_banner_aspect_ratio() {
        let mut image = element("img", vec![]);
        image.style.display = Display::Block;
        image.style.width = Some(600.0);
        image
            .attrs
            .insert("src".into(), "https://example.test/header-logo.png".into());

        let layout = layout(&[Node::Element(image)], 600.0, &Fake);
        assert!(matches!(
            layout.fragments.as_slice(),
            [Fragment::Image { rect, .. }] if *rect == Rect { x: 0.0, y: 0.0, w: 600.0, h: 72.0 }
        ));
        assert_eq!(layout.height, 72.0);
    }

    #[test]
    fn auto_table_honors_a_fixed_content_column() {
        let mut middle = element("td", vec![Node::Text("content".into())]);
        middle.style.width = Some(200.0);
        let mut row = element("tr", vec![]);
        row.children.push(Node::Element(element("td", vec![])));
        row.children.push(Node::Element(middle));
        row.children.push(Node::Element(element("td", vec![])));
        let mut table = element("table", vec![Node::Element(row)]);
        table.attrs.insert("width".into(), "auto".into());
        table.style.align = Align::Center;

        let layout = layout(&[Node::Element(table)], 400.0, &Fake);
        let (_, x, _, _) = layout.words().next().unwrap();
        assert_eq!(x, 100.0, "the fixed 200px column should be centered");
    }

    #[test]
    fn colspan_and_rowspan_place_each_cell_once() {
        let mut spanning = element("td", vec![Node::Text("wide".into())]);
        spanning.style.colspan = 2;
        let first = element(
            "tr",
            vec![
                Node::Element(spanning),
                Node::Element(element("td", vec![Node::Text("right".into())])),
            ],
        );
        let mut tall = element("td", vec![Node::Text("tall".into())]);
        tall.style.rowspan = 2;
        let second = element(
            "tr",
            vec![
                Node::Element(tall),
                Node::Element(element("td", vec![Node::Text("middle".into())])),
                Node::Element(element("td", vec![Node::Text("end".into())])),
            ],
        );
        let third = element(
            "tr",
            vec![
                Node::Element(element("td", vec![Node::Text("below".into())])),
                Node::Element(element("td", vec![Node::Text("last".into())])),
            ],
        );
        let table = element(
            "table",
            vec![
                Node::Element(first),
                Node::Element(second),
                Node::Element(third),
            ],
        );
        let result = layout(&[Node::Element(table)], 360.0, &Fake);
        let words: Vec<_> = result.words().map(|(word, _, _, _)| word).collect();
        assert_eq!(
            words,
            ["wide", "right", "tall", "middle", "end", "below", "last"]
        );
    }

    #[test]
    fn four_deep_nested_tables_do_not_duplicate_content() {
        let mut node = Node::Text("deep".into());
        for _ in 0..4 {
            node = Node::Element(element(
                "table",
                vec![Node::Element(element(
                    "tr",
                    vec![Node::Element(element("td", vec![node]))],
                ))],
            ));
        }
        let result = layout(&[node], 320.0, &Fake);
        assert_eq!(
            result
                .words()
                .map(|(word, _, _, _)| word)
                .collect::<Vec<_>>(),
            ["deep"]
        );
    }

    #[test]
    fn percentages_lists_and_link_targets_reach_the_display_list() {
        let mut block = element("div", vec![Node::Text("half".into())]);
        block.style.width_percent = Some(0.5);
        let mut link = element("a", vec![Node::Text("site".into())]);
        link.attrs
            .insert("href".into(), "https://example.com".into());
        let mut item = element("li", vec![Node::Text("one".into())]);
        item.style.list_style = ListStyle::Decimal;
        let mut list = element("ol", vec![Node::Element(item)]);
        list.style.list_style = ListStyle::Decimal;
        let result = layout(
            &[
                Node::Element(block),
                Node::Element(link),
                Node::Element(list),
            ],
            200.0,
            &Fake,
        );
        assert!(result.fragments.iter().any(|fragment| matches!(
            fragment,
            Fragment::Word { text, href: Some(target), .. }
                if text == "site" && target == "https://example.com"
        )));
        assert!(result.words().any(|(word, _, _, _)| word == "1."));
        let half_x = result
            .words()
            .find(|(word, _, _, _)| *word == "half")
            .unwrap()
            .1;
        assert!(half_x <= 100.0);
    }

    #[test]
    fn node_and_time_budgets_fail_closed() {
        let nodes = vec![Node::Text("a".into()), Node::Text("b".into())];
        assert!(matches!(
            layout_with_limits(
                &nodes,
                100.0,
                &Fake,
                LayoutLimits {
                    max_nodes: 1,
                    max_duration: Duration::from_secs(1)
                },
            ),
            Err(LayoutError::NodeBudget { nodes: 2, limit: 1 })
        ));
        assert!(matches!(
            layout_with_limits(
                &nodes,
                100.0,
                &Fake,
                LayoutLimits {
                    max_nodes: 10,
                    max_duration: Duration::ZERO
                },
            ),
            Err(LayoutError::TimeBudget)
        ));
    }

    #[test]
    fn decorated_inline_block_paints_a_content_sized_link_box() {
        let mut button = element("a", vec![Node::Text("Go now".into())]);
        button.style.display = Display::InlineBlock;
        button.style.background = Some([1, 2, 3, 255]);
        button.style.padding_left = 10.0;
        button.style.padding_right = 10.0;
        button
            .attrs
            .insert("href".into(), "https://example.com".into());
        let result = layout(&[Node::Element(button)], 300.0, &Fake);
        assert!(result.fragments.iter().any(|fragment| matches!(
            fragment,
            Fragment::Rect { rect, color, .. }
                if *color == [1, 2, 3, 255] && rect.w < 300.0
        )));
        assert!(result.fragments.iter().any(|fragment| matches!(
            fragment,
            Fragment::Word { href: Some(target), .. } if target == "https://example.com"
        )));
    }
}
