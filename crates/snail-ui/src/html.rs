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
    pub border_color: Color,
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
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            color: [0x24, 0x1f, 0x1b, 0xff],
            background: None,
            background_image: None,
            font_family: None,
            font_size: 15.0,
            bold: false,
            italic: false,
            underline: false,
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
            border_color: [0xd9, 0xd4, 0xcc, 0xff],
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
        bold: parent.bold,
        italic: parent.italic,
        underline: parent.underline,
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
        "h1" => style.font_size = 32.0,
        "h2" => style.font_size = 24.0,
        "h3" => style.font_size = 19.0,
        "h4" => style.font_size = 17.0,
        "h5" => style.font_size = 15.0,
        "h6" => style.font_size = 14.0,
        "b" | "strong" => style.bold = true,
        "i" | "em" => style.italic = true,
        "u" => style.underline = true,
        "a" => {
            style.display = Display::Inline;
            style.color = [0x2c, 0x5f, 0xb8, 0xff];
            style.underline = true;
        }
        "center" => style.align = Align::Center,
        "span" | "font" | "small" | "big" | "sub" | "sup" | "label" => {
            style.display = Display::Inline;
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
        "pre" | "code" => style.font_size = 13.0,
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
    }
    if let Some(size) = attrs.get("size").and_then(|v| v.trim().parse::<f32>().ok()) {
        // <font size="1..7">, the legacy scale.
        style.font_size = match size as i32 {
            1 => 10.0,
            2 => 12.0,
            3 => 15.0,
            4 => 18.0,
            5 => 22.0,
            6 => 26.0,
            7 => 32.0,
            _ => style.font_size,
        };
    }
    if let Some(face) = attrs.get("face") {
        style.font_family = parse_font_family(face);
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
pub fn apply_inline_style(style: &mut Style, css: &str) {
    for declaration in css.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        // Email generators append `!important` liberally, including to the properties that make
        // class/inline precedence has already been decided before this function is called.
        let value = lower
            .strip_suffix("!important")
            .map(|prefix| &value[..prefix.len()])
            .unwrap_or(value)
            .trim();
        let lower = value.to_ascii_lowercase();
        match name.as_str() {
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
                if let Some(px) = parse_length(value) {
                    style.font_size = px;
                }
            }
            "font-family" => style.font_family = parse_font_family(value),
            "font" => apply_font_shorthand(style, value),
            "font-weight" => {
                style.bold = matches!(lower.as_str(), "bold" | "600" | "700" | "800" | "900");
            }
            "font-style" => style.italic = lower == "italic",
            "text-decoration" | "text-decoration-line" => {
                style.underline = lower.contains("underline");
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
                if let Some([top, _, bottom, _]) = parse_box_lengths(&numeric) {
                    style.margin_top = top;
                    style.margin_bottom = bottom;
                }
            }
            "margin-top" => style.margin_top = parse_length(value).unwrap_or(0.0),
            "margin-bottom" => style.margin_bottom = parse_length(value).unwrap_or(0.0),
            "margin-left" | "margin-right" if lower == "auto" => {
                style.margin_auto_horizontal = true;
            }
            "padding" => {
                if let Some(values) = parse_box_lengths(value) {
                    set_padding(style, values);
                }
            }
            "padding-top" => style.padding_top = parse_length(value).unwrap_or(0.0),
            "padding-right" => style.padding_right = parse_length(value).unwrap_or(0.0),
            "padding-bottom" => style.padding_bottom = parse_length(value).unwrap_or(0.0),
            "padding-left" => style.padding_left = parse_length(value).unwrap_or(0.0),
            "border" | "border-width" => {
                let border = value
                    .split_whitespace()
                    .find_map(parse_length)
                    .unwrap_or(0.0);
                set_border(style, border);
            }
            "border-top" => style.border_top = parse_border_width(value),
            "border-right" => style.border_right = parse_border_width(value),
            "border-bottom" => style.border_bottom = parse_border_width(value),
            "border-left" => style.border_left = parse_border_width(value),
            "border-color" => {
                if let Some(color) = parse_color(value) {
                    style.border_color = color;
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
}

fn set_border(style: &mut Style, width: f32) {
    style.border = width;
    style.border_top = Some(width);
    style.border_right = Some(width);
    style.border_bottom = Some(width);
    style.border_left = Some(width);
}

fn parse_border_width(value: &str) -> Option<f32> {
    value.split_whitespace().find_map(parse_length)
}

fn set_width(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.width_percent = Some(percent);
        style.width = None;
    } else if let Some(width) = parse_length(value) {
        style.width = Some(width);
        style.width_percent = None;
    }
}

fn set_max_width(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.max_width_percent = Some(percent);
        style.max_width = None;
    } else if let Some(width) = parse_length(value) {
        style.max_width = Some(width);
        style.max_width_percent = None;
    }
}

fn set_height(style: &mut Style, value: &str) {
    if let Some(percent) = parse_percentage(value) {
        style.height_percent = Some(percent);
        style.height = None;
    } else if let Some(height) = parse_length(value) {
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

fn parse_font_family(value: &str) -> Option<String> {
    value
        .split(',')
        .map(|family| family.trim().trim_matches(['\'', '"']))
        .find(|family| !family.is_empty())
        .map(str::to_string)
}

fn set_padding(style: &mut Style, [top, right, bottom, left]: [f32; 4]) {
    style.padding_top = top;
    style.padding_right = right;
    style.padding_bottom = bottom;
    style.padding_left = left;
}

fn parse_box_lengths(value: &str) -> Option<[f32; 4]> {
    let values: Vec<f32> = value
        .split_whitespace()
        .map(parse_length)
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
        .or_else(|| parse_length(value))
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
        if let Some(px) = parse_length(size) {
            style.font_size = px;
            if let Some((_, height)) = size_and_height {
                style.line_height = parse_line_height(height, px);
            }
            style.font_family = parse_font_family(&parts[index + 1..].join(" "));
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
pub fn parse_length(input: &str) -> Option<f32> {
    let value = input.trim().to_ascii_lowercase();
    let (number, unit) = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .map(|i| (value[..i].trim().to_string(), value[i..].to_string()))
        .unwrap_or((value.clone(), String::new()));
    let number: f32 = number.parse().ok()?;
    match unit.as_str() {
        "" | "px" => Some(number),
        "pt" => Some(number * 4.0 / 3.0),
        "em" | "rem" => Some(number * 15.0),
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

    #[test]
    fn font_tag_size_maps_to_the_legacy_scale() {
        let style = resolve_style("font", &attrs(&[("size", "5")]), &Style::default());
        assert_eq!(style.font_size, 22.0);
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
        assert_eq!(font.font_family.as_deref(), Some("Arial"));
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
        assert_eq!(style.font_family.as_deref(), Some("Georgia"));
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
