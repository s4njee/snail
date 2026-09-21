//! A minimal HTML subset for the E0.3/E6 prototype: a DOM, a style model, and the CSS subset
//! resolver. Pure data and string parsing — no GPUI, no serde even.
//!
//! `snail-core` owns html5ever parsing and sanitization; this module owns the resolved model the
//! layout engine consumes.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    InlineBlock,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
    #[default]
    Baseline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListStyle {
    #[default]
    None,
    Disc,
    Decimal,
}

/// RGBA, 0..=255.
pub type Color = [u8; 4];

/// The supported style subset (plan.md E6.4). Anything not represented here is dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub display: Display,
    pub color: Color,
    pub background: Option<Color>,
    /// A CSS `background-image: url(...)` (E6.8). Remote, so it obeys the same block/unblock rule.
    pub background_image: Option<String>,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// `text-decoration: line-through`, and `<s>` / `<strike>` / `<del>`.
    pub strike: bool,
    pub align: Align,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_auto_horizontal: bool,
    pub padding_top: f32,
    pub padding_right: f32,
    pub padding_bottom: f32,
    pub padding_left: f32,
    pub border: f32,
    pub border_top: Option<f32>,
    pub border_right: Option<f32>,
    pub border_bottom: Option<f32>,
    pub border_left: Option<f32>,
    /// The colour of every side not given its own. `None` is CSS's `currentColor`: the text
    /// colour.
    pub border_color: Option<Color>,
    /// Per-side colours, top/right/bottom/left, from `border-top: … #hex` and friends.
    pub border_colors: [Option<Color>; 4],
    /// Corner radius in px. A negative value is a percentage of the box's shorter side, resolved
    /// in layout (`border-radius: 50%` is a circle or a pill).
    pub border_radius: f32,
    /// Absolute dimensions and percentages are kept separately so layout can resolve percentages
    /// against the containing block.
    pub width: Option<f32>,
    pub width_percent: Option<f32>,
    pub max_width: Option<f32>,
    pub max_width_percent: Option<f32>,
    pub height: Option<f32>,
    pub height_percent: Option<f32>,
    pub line_height: Option<f32>,
    pub vertical_align: VerticalAlign,
    pub list_style: ListStyle,
    pub colspan: usize,
    pub rowspan: usize,
    /// Legacy table attributes, retained because production email still relies on them.
    pub cell_padding: f32,
    pub cell_spacing: f32,
    /// A replaced element's natural (decoded) pixel size, when known. Distinct from
    /// `width`/`height`, which are what the *sender specified*: sizing an `<img>` needs both,
    /// because a single specified dimension is completed from the natural aspect ratio.
    /// Not inherited.
    pub natural_size: Option<(f32, f32)>,
    /// The parent's computed font size, which `em` and `%` font sizes resolve against (where
    /// every other `em` resolves against the element's own size). Set by inheritance.
    pub parent_font_size: f32,
}

/// WebKit's `medium`, and so the size of any email text that never sets one.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;

/// The family used when a message names none, or none of the families it names is installed.
/// Measurement and painting must both fall back to this, or text overflows its measured box.
pub const DEFAULT_FONT_FAMILY: &str = "Helvetica";

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            color: [0x24, 0x1f, 0x1b, 0xff],
            background: None,
            background_image: None,
            font_family: None,
            font_size: DEFAULT_FONT_SIZE,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            align: Align::Left,
            margin_top: 0.0,
            margin_bottom: 0.0,
            margin_auto_horizontal: false,
            padding_top: 0.0,
            padding_right: 0.0,
            padding_bottom: 0.0,
            padding_left: 0.0,
            border: 0.0,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            border_color: None,
            border_colors: [None; 4],
            border_radius: 0.0,
            width: None,
            width_percent: None,
            max_width: None,
            max_width_percent: None,
            height: None,
            height_percent: None,
            line_height: None,
            vertical_align: VerticalAlign::Baseline,
            list_style: ListStyle::None,
            colspan: 1,
            rowspan: 1,
            cell_padding: 0.0,
            cell_spacing: 0.0,
            natural_size: None,
            parent_font_size: DEFAULT_FONT_SIZE,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Element {
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub style: Style,
    pub children: Vec<Node>,
}

/// Font identity for measurement. The layout engine never parses a font name.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: Option<String>,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub line_height: Option<f32>,
}

/// Inherited, non-geometric style carried into children.
fn inherit(parent: &Style) -> Style {
    Style {
        color: parent.color,
        font_family: parent.font_family.clone(),
        font_size: parent.font_size,
        parent_font_size: parent.font_size,
        bold: parent.bold,
        italic: parent.italic,
        underline: parent.underline,
        strike: parent.strike,
        align: parent.align,
        line_height: parent.line_height,
        list_style: parent.list_style,
        display: Display::Block,
        ..Style::default()
    }
}

/// Resolve one element's style from its tag, presentational attributes and inline `style`
/// (plan.md E6.3), inheriting the properties that inherit.
pub fn resolve_style(tag: &str, attrs: &HashMap<String, String>, parent: &Style) -> Style {
    let tag = tag.to_ascii_lowercase();
    let mut style = inherit(parent);
    match tag.as_str() {
        "html" | "body" => {}
        "div" | "p" | "section" | "article" | "header" | "footer" => {
            style.display = Display::Block;
        }
        // WebKit's user-agent sizes, relative to the inherited size, and bold.
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let factor = match tag.as_str() {
                "h1" => 2.0,
                "h2" => 1.5,
                "h3" => 1.17,
                "h4" => 1.0,
                "h5" => 0.83,
                _ => 0.67,
            };
            style.font_size = parent.font_size * factor;
            style.bold = true;
        }
        "b" | "strong" => style.bold = true,
        "i" | "em" => style.italic = true,
        "u" => style.underline = true,
        "s" | "strike" | "del" => style.strike = true,
        "a" => {
            style.display = Display::Inline;
            style.color = [0x2c, 0x5f, 0xb8, 0xff];
            style.underline = true;
        }
        "center" => style.align = Align::Center,
        "span" | "font" | "sub" | "sup" | "label" => {
            style.display = Display::Inline;
        }
        // `font-size: smaller` / `larger` in WebKit's user-agent stylesheet.
        "small" | "big" => {
            style.display = Display::Inline;
            style.font_size = if tag == "small" {
                parent.font_size / 1.2
            } else {
                parent.font_size * 1.2
            };
        }
        "ul" => style.list_style = ListStyle::Disc,
        "ol" => style.list_style = ListStyle::Decimal,
        "li" => style.display = Display::Block,
        "br" | "hr" | "img" => style.display = Display::Inline,
        "table" => style.display = Display::Table,
        "thead" | "tbody" | "tfoot" => style.display = Display::TableRowGroup,
        "tr" => style.display = Display::TableRow,
        "td" | "th" => {
            style.display = Display::TableCell;
            if tag == "th" {
                style.bold = true;
            }
        }
        "blockquote" => {
            style.margin_top = 10.0;
            style.margin_bottom = 10.0;
            set_padding(&mut style, [10.0; 4]);
            style.background = Some([0xf6, 0xf3, 0xee, 0xff]);
        }
        // `font-family: monospace`, which WebKit sets at its 13px default fixed size.
        "pre" | "code" | "kbd" | "samp" | "tt" => {
            style.font_family = parse_font_stack("monospace");
            style.font_size = 13.0;
        }
        _ => {}
    }

    // Presentational attributes email actually uses (E6.3).
    if let Some(value) = attrs.get("bgcolor").and_then(|v| parse_color(v)) {
        style.background = Some(value);
    }
    if let Some(value) = attrs.get("color").and_then(|v| parse_color(v)) {
        style.color = value;
    }
    if let Some(value) = attrs.get("align") {
        style.align = match value.to_ascii_lowercase().as_str() {
            "center" => Align::Center,
            "right" => Align::Right,
            _ => Align::Left,
        };
    }
    if let Some(value) = attrs.get("width") {
        set_width(&mut style, value);
    }
    if let Some(value) = attrs.get("height") {
        set_height(&mut style, value);
    }
    // A real `border` attribute still draws a border; the default is none (email tables are layout
    // tables, and drawing a grid by default was wrong).
    if let Some(border) = attrs
        .get("border")
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        style.border = border;
        // The legacy attribute's grid is grey, not the text colour.
        style.border_color = Some([0x80, 0x80, 0x80, 0xff]);
    }
    // `<font size>` only: other elements' `size` attributes (`<hr size>`, `<input size>`) are not
    // font sizes.
    if matches!(tag.as_str(), "font" | "basefont")
        && let Some(size) = attrs.get("size").and_then(|value| legacy_font_size(value))
    {
        style.font_size = size;
    }
    if let Some(face) = attrs.get("face") {
        style.font_family = parse_font_stack(face);
    }
    if let Some(span) = attrs
        .get("colspan")
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        style.colspan = span.max(1);
    }
    if let Some(span) = attrs
        .get("rowspan")
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        style.rowspan = span.max(1);
    }
    if let Some(value) = attrs.get("valign") {
        style.vertical_align = parse_vertical_align(value);
    }
    if let Some(value) = attrs.get("cellpadding").and_then(|v| parse_length(v)) {
        style.cell_padding = value.max(0.0);
    }
    if let Some(value) = attrs.get("cellspacing").and_then(|v| parse_length(v)) {
        style.cell_spacing = value.max(0.0);
    }

    if let Some(css) = attrs.get("style") {
        apply_inline_style(&mut style, css);
    }

    style
}

/// The `style=""` subset. Unknown properties are dropped, never approximated (E6.4).
///
/// Font size is applied first whatever order the declarations are written in, because every
/// other `em` length resolves against the element's own computed font size (CSS computes
/// `font-size` before the properties that depend on it).
pub fn apply_inline_style(style: &mut Style, css: &str) {
    let declarations: Vec<(String, &str)> = css
        .split(';')
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let value = value.trim();
            // Email generators append `!important` liberally; class/inline precedence has
            // already been decided before this function is called.
            let lower = value.to_ascii_lowercase();
            let value = lower
                .strip_suffix("!important")
                .map(|prefix| &value[..prefix.len()])
                .unwrap_or(value)
                .trim();
            Some((name.trim().to_ascii_lowercase(), value))
        })
        .collect();
    let sets_font_size = |name: &str| matches!(name, "font-size" | "font");
    for (name, value) in declarations.iter().filter(|(name, _)| sets_font_size(name)) {
        apply_declaration(style, name, value);
    }
    for (name, value) in declarations
        .iter()
        .filter(|(name, _)| !sets_font_size(name))
    {
        apply_declaration(style, name, value);
    }
}

fn apply_declaration(style: &mut Style, name: &str, value: &str) {
    let lower = value.to_ascii_lowercase();
    let em = style.font_size;
    match name {
        "color" => {
            if let Some(color) = parse_color(value) {
                style.color = color;
            }
        }
        "background" | "background-color" => {
            if let Some(url) = extract_url(value) {
                style.background_image = Some(url);
            }
            // The shorthand carries the colour among other tokens.
            let color =
                parse_color(value).or_else(|| value.split_whitespace().find_map(parse_color));
            if let Some(color) = color {
                style.background = Some(color);
            }
        }
        "background-image" => {
            if let Some(url) = extract_url(value) {
                style.background_image = Some(url);
            }
        }
        "font-size" => {
            if let Some(px) = parse_font_size(value, style.parent_font_size) {
                style.font_size = px;
            }
        }
        "font-family" => style.font_family = parse_font_stack(value),
        "font" => apply_font_shorthand(style, value),
        "font-weight" => {
            style.bold = matches!(lower.as_str(), "bold" | "600" | "700" | "800" | "900");
        }
        "font-style" => style.italic = lower == "italic",
        "text-decoration" | "text-decoration-line" => {
            style.underline = lower.contains("underline");
            style.strike = lower.contains("line-through");
        }
        "text-align" => {
            style.align = match lower.as_str() {
                "center" => Align::Center,
                "right" => Align::Right,
                _ => Align::Left,
            };
        }
        "line-height" => style.line_height = parse_line_height(value, style.font_size),
        "margin" => {
            if value
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("auto"))
            {
                style.margin_auto_horizontal = true;
            }
            let numeric = value
                .split_whitespace()
                .map(|part| {
                    if part.eq_ignore_ascii_case("auto") {
                        "0"
                    } else {
                        part
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            if let Some([top, _, bottom, _]) = parse_box_lengths(&numeric, em) {
                style.margin_top = top;
                style.margin_bottom = bottom;
            }
        }
        "margin-top" => style.margin_top = parse_length_in(value, em).unwrap_or(0.0),
        "margin-bottom" => style.margin_bottom = parse_length_in(value, em).unwrap_or(0.0),
        "margin-left" | "margin-right" if lower == "auto" => {
            style.margin_auto_horizontal = true;
        }
        "padding" => {
            if let Some(values) = parse_box_lengths(value, em) {
                set_padding(style, values);
            }
        }
        "padding-top" => style.padding_top = parse_length_in(value, em).unwrap_or(0.0),
        "padding-right" => style.padding_right = parse_length_in(value, em).unwrap_or(0.0),
        "padding-bottom" => style.padding_bottom = parse_length_in(value, em).unwrap_or(0.0),
        "padding-left" => style.padding_left = parse_length_in(value, em).unwrap_or(0.0),
        "border" => {
            let side = parse_border_side(value, em);
            set_border(style, side.width);
            style.border_color = side.color;
            style.border_colors = [None; 4];
        }
        "border-width" => {
            if let Some([top, right, bottom, left]) = parse_box_lengths(value, em) {
                style.border = top;
                style.border_top = Some(top);
                style.border_right = Some(right);
                style.border_bottom = Some(bottom);
                style.border_left = Some(left);
            }
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let side = parse_border_side(value, em);
            let index = match name {
                "border-top" => 0,
                "border-right" => 1,
                "border-bottom" => 2,
                _ => 3,
            };
            *[
                &mut style.border_top,
                &mut style.border_right,
                &mut style.border_bottom,
                &mut style.border_left,
            ][index] = Some(side.width);
            style.border_colors[index] = side.color;
        }
        "border-color" => {
            let colors: Vec<Color> = split_css_values(value)
                .iter()
                .filter_map(|part| parse_color(part))
                .collect();
            let [top, right, bottom, left] = match colors.as_slice() {
                [all] => [*all; 4],
                [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
                [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
                [top, right, bottom, left, ..] => [*top, *right, *bottom, *left],
                [] => return,
            };
            style.border_color = Some(top);
            style.border_colors = [Some(top), Some(right), Some(bottom), Some(left)];
        }
        "border-style" if matches!(lower.as_str(), "none" | "hidden") => set_border(style, 0.0),
        // The per-side longhands: `border-bottom-width: 1px; border-bottom-style: solid;
        // border-bottom-color: #e1e4e8` is how GitHub draws its dividers.
        _ if name.starts_with("border-") && side_longhand(name).is_some() => {
            let (index, part) = side_longhand(name).expect("checked");
            let width = [
                &mut style.border_top,
                &mut style.border_right,
                &mut style.border_bottom,
                &mut style.border_left,
            ];
            match part {
                "width" => {
                    let parsed = match lower.as_str() {
                        "thin" => Some(1.0),
                        "medium" => Some(3.0),
                        "thick" => Some(5.0),
                        _ => parse_length_in(value, em),
                    };
                    if let Some(parsed) = parsed {
                        *width[index] = Some(parsed);
                    }
                }
                "style" if matches!(lower.as_str(), "none" | "hidden") => *width[index] = Some(0.0),
                "color" => {
                    if let Some(color) = parse_color(value) {
                        style.border_colors[index] = Some(color);
                    }
                }
                _ => {}
            }
        }
        "border-radius" => {
            // One radius for all four corners; the first value of a longer form.
            if let Some(first) = split_css_values(value).first() {
                if let Some(percent) = parse_percentage(first) {
                    style.border_radius = -percent * 100.0;
                } else if let Some(px) = parse_length_in(first, em) {
                    style.border_radius = px.max(0.0);
                }
            }
        }
        "width" => set_width(style, value),
        "max-width" => set_max_width(style, value),
        "height" => set_height(style, value),
        "display" => {
            let supported = match lower.as_str() {
                "block" => Some(Display::Block),
                "none" => Some(Display::None),
                "inline" => Some(Display::Inline),
                "inline-block" => Some(Display::InlineBlock),
                "table" => Some(Display::Table),
                "table-row-group" | "table-header-group" | "table-footer-group" => {
                    Some(Display::TableRowGroup)
                }
                "table-row" => Some(Display::TableRow),
                "table-cell" => Some(Display::TableCell),
                _ => None,
            };
            if let Some(display) = supported {
                style.display = display;
            }
        }
        // Hidden preheaders often use visibility instead of display. Preserving that intent is
        // important: their zero-width filler text can otherwise push the real mail thousands
        // of lines below the viewport.
        "visibility" if lower == "hidden" || lower == "collapse" => {
            style.display = Display::None;
        }
        "vertical-align" => style.vertical_align = parse_vertical_align(value),
        "list-style" | "list-style-type" => {
            let supported = if lower.contains("decimal") {
                Some(ListStyle::Decimal)
            } else if lower.contains("none") {
                Some(ListStyle::None)
            } else if lower.contains("disc") {
                Some(ListStyle::Disc)
            } else {
                None
            };
            if let Some(list_style) = supported {
                style.list_style = list_style;
            }
        }
        // Deliberately unsupported: float, position, flex, grid, transform,
        // media queries, pseudo-elements, web fonts (E6.4).
        _ => {}
    }
}

fn set_border(style: &mut Style, width: f32) {
    style.border = width;
    style.border_top = Some(width);
    style.border_right = Some(width);
    style.border_bottom = Some(width);
    style.border_left = Some(width);
}

/// `border-bottom-width` → (2, "width"); `None` for anything else.
fn side_longhand(name: &str) -> Option<(usize, &str)> {
    let rest = name.strip_prefix("border-")?;
    let (side, part) = rest.split_once('-')?;
    let index = match side {
        "top" => 0,
        "right" => 1,
        "bottom" => 2,
        "left" => 3,
        _ => return None,
    };
    matches!(part, "width" | "style" | "color").then_some((index, part))
}

/// One side of a border shorthand: `1px solid #ccc`, `12px solid rgb(20, 111, 245)`, `none`.
struct BorderSide {
    width: f32,
    color: Option<Color>,
}

/// Parse a `border` / `border-top` … shorthand. As in CSS, a border with no style is no border
/// (`border: 1px #ccc` draws nothing); a style with no width is `medium`, 3px; and a missing colour
/// is the text colour, left to layout.
fn parse_border_side(value: &str, em: f32) -> BorderSide {
    let mut width = None;
    let mut color = None;
    let mut styled = false;
    for part in split_css_values(value) {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "none" | "hidden" => {
                return BorderSide {
                    width: 0.0,
                    color: None,
                };
            }
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                styled = true;
            }
            "thin" => width = Some(1.0),
            "medium" => width = Some(3.0),
            "thick" => width = Some(5.0),
            _ => {
                if let Some(length) = parse_length_in(&part, em) {
                    width = Some(length);
                } else if let Some(parsed) = parse_color(&part) {
                    color = Some(parsed);
                }
            }
        }
    }
    let width = match (styled, width) {
        (true, width) => width.unwrap_or(3.0),
        // Unstyled: only an explicit zero is meaningful (and harmless) — `border: 0`.
        (false, _) => 0.0,
    };
    BorderSide { width, color }
}

/// Split a CSS value on whitespace, keeping `rgb(20, 111, 245)` and friends whole.
fn split_css_values(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn set_width(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.width_percent = Some(percent);
        style.width = None;
    } else if let Some(width) = parse_length_in(value, style.font_size) {
        style.width = Some(width);
        style.width_percent = None;
    }
}

fn set_max_width(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.max_width_percent = Some(percent);
        style.max_width = None;
    } else if let Some(width) = parse_length_in(value, style.font_size) {
        style.max_width = Some(width);
        style.max_width_percent = None;
    }
}

fn set_height(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.height_percent = Some(percent);
        style.height = None;
    } else if let Some(height) = parse_length_in(value, style.font_size) {
        style.height = Some(height);
        style.height_percent = None;
    }
}

fn parse_percentage(value: &str) -> Option<f32> {
    let number = value.trim().strip_suffix('%')?.trim().parse::<f32>().ok()?;
    Some((number / 100.0).max(0.0))
}

fn parse_vertical_align(value: &str) -> VerticalAlign {
    match value.trim().to_ascii_lowercase().as_str() {
        "top" | "text-top" => VerticalAlign::Top,
        "middle" | "center" => VerticalAlign::Middle,
        "bottom" | "text-bottom" => VerticalAlign::Bottom,
        _ => VerticalAlign::Baseline,
    }
}

/// Parse a CSS `font-family` list into a normalized stack: every family kept in order, CSS
/// generics and system aliases expanded to concrete families, duplicates dropped. The result is
/// comma-separated (see [`font_stack`]); the platform picks the first family that is installed.
///
/// Keeping the whole stack matters: the ubiquitous `-apple-system, BlinkMacSystemFont, "Segoe
/// UI", Roboto, Helvetica, Arial, sans-serif` names nothing a font system can find in its first
/// two entries — CoreText matches none of `-apple-system`, `system-ui`, `BlinkMacSystemFont`,
/// `serif` or `sans-serif` — so taking only the first family meant no family at all.
pub fn parse_font_stack(value: &str) -> Option<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut push = |family: &str| {
        if !stack.iter().any(|known| known.eq_ignore_ascii_case(family)) {
            stack.push(family.to_string());
        }
    };
    for raw in value.split(',') {
        let name = raw.trim().trim_matches(['\'', '"']).trim();
        if name.is_empty() {
            continue;
        }
        match expand_family(name) {
            Some(families) => families.iter().for_each(|family| push(family)),
            None => push(name),
        }
    }
    (!stack.is_empty()).then(|| stack.join(", "))
}

/// The concrete families a CSS generic or system alias stands for, in preference order across
/// macOS, Windows and Linux. `Some(&[])` drops a generic with no sensible email mapping.
fn expand_family(name: &str) -> Option<&'static [&'static str]> {
    match name.to_ascii_lowercase().as_str() {
        "-apple-system" | "system-ui" | "blinkmacsystemfont" | "ui-sans-serif" => {
            Some(&[".AppleSystemUIFont", "Segoe UI"])
        }
        "sans-serif" => Some(&["Helvetica", "Arial", "Liberation Sans", "DejaVu Sans"]),
        "serif" | "ui-serif" => Some(&[
            "Times",
            "Times New Roman",
            "Liberation Serif",
            "DejaVu Serif",
        ]),
        "monospace" | "ui-monospace" => {
            Some(&["Menlo", "Consolas", "Liberation Mono", "DejaVu Sans Mono"])
        }
        "cursive" | "fantasy" | "emoji" | "math" | "fangsong" | "ui-rounded" => Some(&[]),
        _ => None,
    }
}

/// The families of a normalized stack from [`parse_font_stack`], in preference order.
pub fn font_stack(family: &str) -> impl Iterator<Item = &str> {
    family
        .split(',')
        .map(str::trim)
        .filter(|family| !family.is_empty())
}

/// The first family of a stack, or [`DEFAULT_FONT_FAMILY`] when there is none.
pub fn primary_family(family: Option<&str>) -> &str {
    family
        .and_then(|stack| font_stack(stack).next())
        .unwrap_or(DEFAULT_FONT_FAMILY)
}

fn set_padding(style: &mut Style, [top, right, bottom, left]: [f32; 4]) {
    style.padding_top = top;
    style.padding_right = right;
    style.padding_bottom = bottom;
    style.padding_left = left;
}

fn parse_box_lengths(value: &str, em: f32) -> Option<[f32; 4]> {
    let values: Vec<f32> = value
        .split_whitespace()
        .map(|part| parse_length_in(part, em))
        .collect::<Option<_>>()?;
    match values.as_slice() {
        [all] => Some([*all; 4]),
        [vertical, horizontal] => Some([*vertical, *horizontal, *vertical, *horizontal]),
        [top, horizontal, bottom] => Some([*top, *horizontal, *bottom, *horizontal]),
        [top, right, bottom, left] => Some([*top, *right, *bottom, *left]),
        _ => None,
    }
}

fn parse_line_height(value: &str, font_size: f32) -> Option<f32> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .map(|factor| factor * font_size)
        .or_else(|| {
            parse_percentage(value)
                .map(|percent| percent * font_size)
                .or_else(|| parse_length_in(value, font_size))
        })
}

fn apply_font_shorthand(style: &mut Style, value: &str) {
    let parts: Vec<&str> = value.split_whitespace().collect();
    for (index, part) in parts.iter().enumerate() {
        let lower = part.to_ascii_lowercase();
        if matches!(lower.as_str(), "bold" | "600" | "700" | "800" | "900") {
            style.bold = true;
            continue;
        } else if lower == "italic" {
            style.italic = true;
            continue;
        } else if matches!(lower.as_str(), "normal" | "400" | "500") {
            continue;
        }
        let size_and_height = part.split_once('/');
        let size = size_and_height.map(|(size, _)| size).unwrap_or(part);
        if let Some(px) = parse_font_size(size, style.parent_font_size) {
            style.font_size = px;
            if let Some((_, height)) = size_and_height {
                style.line_height = parse_line_height(height, px);
            }
            style.font_family = parse_font_stack(&parts[index + 1..].join(" "));
            break;
        }
    }
}

/// Parse `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(...)` or a handful of named colours.
pub fn parse_color(input: &str) -> Option<Color> {
    let value = input.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some([r, g, b, 0xff])
            }
            6 => Some([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                0xff,
            ]),
            8 => Some([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ]),
            _ => None,
        };
    }
    if let Some(rest) = value.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<u8> = rest
            .split(',')
            .filter_map(|p| p.trim().parse::<u8>().ok())
            .collect();
        if parts.len() == 3 {
            return Some([parts[0], parts[1], parts[2], 0xff]);
        }
    }
    let named = match value.as_str() {
        "black" => [0, 0, 0, 0xff],
        "white" => [0xff, 0xff, 0xff, 0xff],
        "red" => [0xd0, 0x2b, 0x2b, 0xff],
        "green" => [0x0f, 0x76, 0x6e, 0xff],
        "blue" => [0x1f, 0x6f, 0xeb, 0xff],
        "gray" | "grey" => [0x8a, 0x83, 0x7b, 0xff],
        "silver" => [0xdd, 0xd8, 0xd0, 0xff],
        "navy" => [0x1d, 0x44, 0x89, 0xff],
        "transparent" => [0, 0, 0, 0],
        _ => return None,
    };
    Some(named)
}

/// Extract the first `url(...)` target from a CSS value.
pub fn extract_url(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let start = lower.find("url(")? + 4;
    let rest = &value[start..];
    let end = rest.find(')')?;
    let url = rest[..end].trim().trim_matches(['"', '\'']).trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

/// Parse `12px`, `12`, `1.5em`, `9pt`. Percentages are resolved by layout instead.
/// `<font size>`: an absolute level 1–7, or one relative to the default level 3 (`+1`, `-2`),
/// on WebKit's legacy scale.
fn legacy_font_size(value: &str) -> Option<f32> {
    const SCALE: [f32; 7] = [10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0];
    let value = value.trim();
    let level = if value.starts_with(['+', '-']) {
        3 + value.parse::<i32>().ok()?
    } else {
        value.parse::<f32>().ok()? as i32
    };
    Some(SCALE[(level.clamp(1, 7) - 1) as usize])
}

/// A `font-size` value against the parent's computed size: WebKit's absolute keywords, the
/// relative `smaller`/`larger`, percentages and `em` of the parent, `rem`, and plain lengths.
pub fn parse_font_size(value: &str, parent: f32) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    let keyword = match value.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "smaller" => Some(parent / 1.2),
        "larger" => Some(parent * 1.2),
        _ => None,
    };
    keyword
        .or_else(|| parse_percentage(&value).map(|percent| parent * percent))
        .or_else(|| parse_length_in(&value, parent))
}

/// A length with no element context: `em` and `rem` both resolve against [`DEFAULT_FONT_SIZE`].
/// Inside a style, prefer [`parse_length_in`], which knows the element's font size.
pub fn parse_length(input: &str) -> Option<f32> {
    parse_length_in(input, DEFAULT_FONT_SIZE)
}

/// A length where `em` is the given size — the element's own computed font size for most
/// properties, the parent's for `font-size` — and `rem` is the root size.
pub fn parse_length_in(input: &str, em: f32) -> Option<f32> {
    let value = input.trim().to_ascii_lowercase();
    let (number, unit) = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .map(|i| (value[..i].trim().to_string(), value[i..].to_string()))
        .unwrap_or((value.clone(), String::new()));
    let number: f32 = number.parse().ok()?;
    match unit.as_str() {
        "" | "px" => Some(number),
        "pt" => Some(number * 4.0 / 3.0),
        "em" => Some(number * em),
        "rem" => Some(number * DEFAULT_FONT_SIZE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn inline_style_overrides_tag_defaults() {
        let parent = Style::default();
        let style = resolve_style(
            "td",
            &attrs(&[("style", "color: #ff0000; font-weight: bold; padding: 3px")]),
            &parent,
        );
        assert_eq!(style.color, [0xff, 0, 0, 0xff]);
        assert!(style.bold);
        assert_eq!(style.padding_top, 3.0);
        assert_eq!(style.padding_right, 3.0);
    }

    #[test]
    fn unsupported_properties_are_dropped() {
        let mut style = Style::default();
        let before = style.font_size;
        apply_inline_style(
            &mut style,
            "float: left; position: absolute; font-size: 20px",
        );
        assert_eq!(style.font_size, 20.0);
        // No field exists for float/position, so nothing to assert but non-mutation of others.
        assert!(!style.bold);
        let _ = before;
    }

    // Regression (Slickdeals): a button whose padding is a same-coloured border drew as a grey
    // box, because the shorthand's colour was dropped for a hardcoded default.
    #[test]
    fn border_shorthands_keep_their_colour_and_need_a_style() {
        let mut style = Style::default();
        apply_inline_style(
            &mut style,
            "border-top:12px solid #146ff5;border-left:23px solid rgb(20, 111, 245)",
        );
        assert_eq!(style.border_top, Some(12.0));
        assert_eq!(style.border_colors[0], Some([0x14, 0x6f, 0xf5, 0xff]));
        assert_eq!(style.border_left, Some(23.0));
        assert_eq!(style.border_colors[3], Some([20, 111, 245, 0xff]));

        let mut unstyled = Style::default();
        apply_inline_style(&mut unstyled, "border: 1px #cccccc");
        assert_eq!(
            unstyled.border_top,
            Some(0.0),
            "no style means no border, as in CSS"
        );

        let mut plain = Style::default();
        apply_inline_style(&mut plain, "border: solid");
        assert_eq!(plain.border_top, Some(3.0), "a style alone is `medium`");
        assert_eq!(plain.border_color, None, "and takes the text colour");

        let mut longhand = Style::default();
        apply_inline_style(
            &mut longhand,
            "border-bottom-width:thin;border-bottom-style:solid;border-bottom-color:#e1e4e8",
        );
        assert_eq!(longhand.border_bottom, Some(1.0));
        assert_eq!(longhand.border_colors[2], Some([0xe1, 0xe4, 0xe8, 0xff]));

        let mut none = Style::default();
        apply_inline_style(&mut none, "border:1px solid red;border-style:none");
        assert_eq!(none.border_top, Some(0.0));
    }

    #[test]
    fn border_radius_takes_pixels_or_a_percentage() {
        let mut pill = Style::default();
        apply_inline_style(&mut pill, "border-radius:50px");
        assert_eq!(pill.border_radius, 50.0);
        let mut circle = Style::default();
        apply_inline_style(&mut circle, "border-radius:50% 50%");
        assert_eq!(circle.border_radius, -50.0);
    }

    #[test]
    fn important_and_visibility_hide_email_preheaders() {
        let mut display = Style::default();
        apply_inline_style(&mut display, "display:none!important");
        assert_eq!(display.display, Display::None);

        let mut visibility = Style::default();
        apply_inline_style(&mut visibility, "visibility: hidden");
        assert_eq!(visibility.display, Display::None);
    }

    #[test]
    fn colors_parse_in_all_forms() {
        assert_eq!(parse_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("#336699"), Some([0x33, 0x66, 0x99, 255]));
        assert_eq!(parse_color("rgb(1,2,3)"), Some([1, 2, 3, 255]));
        assert_eq!(parse_color("not-a-color"), None);
    }

    #[test]
    fn lengths_parse_and_reject_percentages() {
        assert_eq!(parse_length("12px"), Some(12.0));
        assert_eq!(parse_length("9pt"), Some(12.0));
        assert_eq!(parse_length("50%"), None);
    }

    #[test]
    fn box_and_font_shorthands_match_email_css() {
        let mut style = Style::default();
        apply_inline_style(
            &mut style,
            "padding: 4px 8px 12px 16px; font: 700 20px/27.5px georgia, serif",
        );
        assert_eq!(
            [
                style.padding_top,
                style.padding_right,
                style.padding_bottom,
                style.padding_left,
            ],
            [4.0, 8.0, 12.0, 16.0]
        );
        assert!(style.bold);
        assert_eq!(style.font_size, 20.0);
        assert_eq!(style.line_height, Some(27.5));

        apply_inline_style(&mut style, "font-size: 16px; line-height: 1.25");
        assert_eq!(style.line_height, Some(20.0));
    }

    #[test]
    fn background_image_urls_are_parsed() {
        let mut style = Style::default();
        apply_inline_style(&mut style, "background-image: url('https://x/y.png')");
        assert_eq!(style.background_image.as_deref(), Some("https://x/y.png"));
        apply_inline_style(
            &mut style,
            "background: #ffffff url(https://x/z.jpg) no-repeat",
        );
        assert_eq!(style.background_image.as_deref(), Some("https://x/z.jpg"));
        assert_eq!(style.background, Some([255, 255, 255, 255]));
    }

    // Regression (LinkedIn): the stack leads with aliases no font system matches, so taking only
    // the first family meant none at all. Every family is kept, aliases and generics expanded.
    #[test]
    fn font_stacks_keep_every_family_and_expand_aliases() {
        let stack = parse_font_stack(
            "-apple-system, system-ui, BlinkMacSystemFont, 'Segoe UI', Roboto, \"Helvetica Neue\", \
             Arial, sans-serif",
        );
        assert_eq!(
            stack.as_deref(),
            Some(
                ".AppleSystemUIFont, Segoe UI, Roboto, Helvetica Neue, Arial, Helvetica, \
                 Liberation Sans, DejaVu Sans"
            )
        );
    }

    #[test]
    fn font_stacks_drop_repeats_case_insensitively_and_unmappable_generics() {
        assert_eq!(
            parse_font_stack("'Times New Roman', times, serif").as_deref(),
            Some("Times New Roman, times, Liberation Serif, DejaVu Serif")
        );
        assert_eq!(parse_font_stack("cursive"), None);
        assert_eq!(primary_family(None), DEFAULT_FONT_FAMILY);
    }

    #[test]
    fn unstyled_text_uses_webkits_medium_size() {
        assert_eq!(Style::default().font_size, 16.0);
        let paragraph = resolve_style("p", &HashMap::new(), &Style::default());
        assert_eq!(paragraph.font_size, 16.0);
    }

    // `em` in `font-size` is the parent's size; in every other property it is the element's own
    // computed size — even when `font-size` is written after the property that uses it.
    #[test]
    fn em_resolves_against_the_right_font_size_in_any_declaration_order() {
        let parent = Style {
            font_size: 20.0,
            ..Style::default()
        };
        let mut style = resolve_style("div", &HashMap::new(), &parent);
        apply_inline_style(
            &mut style,
            "padding: 1em; font-size: 1.5em; margin-top: 0.5em",
        );
        assert_eq!(style.font_size, 30.0);
        assert_eq!(style.padding_top, 30.0);
        assert_eq!(style.margin_top, 15.0);
    }

    #[test]
    fn rem_percent_and_keyword_font_sizes() {
        let parent = Style {
            font_size: 20.0,
            ..Style::default()
        };
        let size = |css: &str| {
            let mut style = resolve_style("span", &HashMap::new(), &parent);
            apply_inline_style(&mut style, css);
            style.font_size
        };
        assert_eq!(size("font-size: 2rem"), 32.0);
        assert_eq!(size("font-size: 90%"), 18.0);
        assert_eq!(size("font-size: small"), 13.0);
        assert_eq!(size("font-size: x-large"), 24.0);
        assert_eq!(size("font-size: larger"), 24.0);
        assert_eq!(size("font: bold 1.5em/1.2 georgia"), 30.0);
    }

    #[test]
    fn line_height_accepts_percentages_and_ems_of_the_elements_own_size() {
        let mut style = Style::default();
        apply_inline_style(&mut style, "font-size: 20px; line-height: 150%");
        assert_eq!(style.line_height, Some(30.0));
        apply_inline_style(&mut style, "line-height: 1.2em");
        assert_eq!(style.line_height, Some(24.0));
    }

    #[test]
    fn font_tag_relative_sizes_and_clamping() {
        let size = |value: &str| {
            resolve_style("font", &attrs(&[("size", value)]), &Style::default()).font_size
        };
        assert_eq!(size("+1"), 18.0);
        assert_eq!(size("-1"), 13.0);
        assert_eq!(size("2"), 13.0);
        assert_eq!(size("3"), 16.0);
        assert_eq!(size("7"), 48.0);
        assert_eq!(size("9"), 48.0);
        assert_eq!(size("+9"), 48.0);
    }

    // `size` on other elements is not a font size (`<hr size>` is a thickness).
    #[test]
    fn size_attribute_only_sets_font_size_on_font_elements() {
        let rule = resolve_style("hr", &attrs(&[("size", "1")]), &Style::default());
        assert_eq!(rule.font_size, 16.0);
    }

    #[test]
    fn headings_are_bold_and_sized_relative_to_their_parent() {
        let h1 = resolve_style("h1", &HashMap::new(), &Style::default());
        assert_eq!((h1.font_size, h1.bold), (32.0, true));
        let parent = Style {
            font_size: 20.0,
            ..Style::default()
        };
        let h2 = resolve_style("h2", &HashMap::new(), &parent);
        assert_eq!((h2.font_size, h2.bold), (30.0, true));
        let h6 = resolve_style("h6", &HashMap::new(), &Style::default());
        assert!((h6.font_size - 10.72).abs() < 0.01);
    }

    #[test]
    fn small_big_and_monospace_elements_follow_the_user_agent_stylesheet() {
        let small = resolve_style("small", &HashMap::new(), &Style::default());
        assert!((small.font_size - 16.0 / 1.2).abs() < 0.01);
        let big = resolve_style("big", &HashMap::new(), &Style::default());
        assert!((big.font_size - 19.2).abs() < 0.01);
        let code = resolve_style("code", &HashMap::new(), &Style::default());
        assert_eq!(code.font_size, 13.0);
        assert_eq!(primary_family(code.font_family.as_deref()), "Menlo");
    }

    #[test]
    fn font_tag_size_maps_to_the_legacy_scale() {
        let style = resolve_style("font", &attrs(&[("size", "5")]), &Style::default());
        assert_eq!(style.font_size, 24.0);
    }

    #[test]
    fn legacy_table_and_font_attributes_are_resolved() {
        let style = resolve_style(
            "table",
            &attrs(&[
                ("cellpadding", "8"),
                ("cellspacing", "3"),
                ("width", "75%"),
                ("valign", "middle"),
            ]),
            &Style::default(),
        );
        assert_eq!(style.cell_padding, 8.0);
        assert_eq!(style.cell_spacing, 3.0);
        assert_eq!(style.width_percent, Some(0.75));
        assert_eq!(style.vertical_align, VerticalAlign::Middle);

        let font = resolve_style(
            "font",
            &attrs(&[("face", "Arial, sans-serif"), ("color", "navy")]),
            &Style::default(),
        );
        // The whole stack is kept, with the `sans-serif` generic expanded.
        assert_eq!(primary_family(font.font_family.as_deref()), "Arial");
        assert_eq!(
            font.font_family.as_deref(),
            Some("Arial, Helvetica, Liberation Sans, DejaVu Sans")
        );
        assert_eq!(font.color, [0x1d, 0x44, 0x89, 0xff]);
    }

    #[test]
    fn documented_css_fixture_covers_the_supported_subset() {
        let mut style = Style::default();
        apply_inline_style(
            &mut style,
            include_str!("../tests/fixtures/e6-css-subset.css"),
        );
        assert_eq!(style.color, [0x11, 0x22, 0x33, 0xff]);
        assert_eq!(style.background, Some([0xee, 0xdd, 0xcc, 0xff]));
        assert_eq!(primary_family(style.font_family.as_deref()), "Georgia");
        assert_eq!(style.font_size, 18.0);
        assert!(style.bold && style.italic && style.underline);
        assert_eq!(style.align, Align::Center);
        assert_eq!(style.line_height, Some(27.0));
        assert_eq!(style.margin_top, 4.0);
        assert_eq!(style.margin_bottom, 8.0);
        assert_eq!(style.padding_left, 12.0);
        assert_eq!(style.border_top, Some(2.0));
        assert_eq!(style.border_right, Some(3.0));
        assert_eq!(style.width_percent, Some(0.75));
        assert_eq!(style.max_width, Some(640.0));
        assert_eq!(style.height_percent, Some(0.5));
        assert_eq!(style.display, Display::InlineBlock);
        assert_eq!(style.vertical_align, VerticalAlign::Middle);
        assert_eq!(style.list_style, ListStyle::Decimal);
        // Unsupported declarations in the fixture never acquire model fields or perturb display.
        assert_eq!(style.display, Display::InlineBlock);
    }
}
