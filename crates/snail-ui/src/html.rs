//! A minimal HTML subset for the E0.3/E6 prototype: a DOM, a style model, and the CSS subset
//! resolver. Pure data and string parsing — no GPUI, no serde even.
//!
//! This is the seed of plan.md E6.3/E6.4. The parsing (html5ever) lives in the caller for now; this
//! module owns the resolved model the layout engine consumes.

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
    Table,
    TableRow,
    TableCell,
    None,
}

/// RGBA, 0..=255.
pub type Color = [u8; 4];

/// The supported style subset (plan.md E6.4). Anything not represented here is dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub display: Display,
    pub color: Color,
    pub background: Option<Color>,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: Align,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub padding: f32,
    pub border: f32,
    pub border_color: Color,
    /// Resolved to px when the source was a length; `None` for percentages we do not resolve.
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub colspan: usize,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            color: [0x24, 0x1f, 0x1b, 0xff],
            background: None,
            font_size: 15.0,
            bold: false,
            italic: false,
            underline: false,
            align: Align::Left,
            margin_top: 0.0,
            margin_bottom: 0.0,
            padding: 0.0,
            border: 0.0,
            border_color: [0xd9, 0xd4, 0xcc, 0xff],
            width: None,
            height: None,
            colspan: 1,
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSpec {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

/// Inherited, non-geometric style carried into children.
fn inherit(parent: &Style) -> Style {
    Style {
        color: parent.color,
        font_size: parent.font_size,
        bold: parent.bold,
        italic: parent.italic,
        underline: parent.underline,
        align: parent.align,
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
            style.color = [0x2c, 0x5f, 0xb8, 0xff];
            style.underline = true;
        }
        "center" => style.align = Align::Center,
        "span" | "font" | "small" | "big" | "sub" | "sup" | "label" => {
            style.display = Display::Inline;
        }
        "br" | "hr" | "img" => style.display = Display::Inline,
        "table" => {
            style.display = Display::Table;
            style.border = 1.0;
        }
        // Sections are pass-through: only rows are rows (collect_rows recurses into sections).
        "thead" | "tbody" | "tfoot" => {}
        "tr" => style.display = Display::TableRow,
        "td" | "th" => {
            style.display = Display::TableCell;
            style.padding = 6.0;
            style.border = 1.0;
            if tag == "th" {
                style.bold = true;
            }
        }
        "blockquote" => {
            style.margin_top = 10.0;
            style.margin_bottom = 10.0;
            style.padding = 10.0;
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
    if let Some(px) = attrs.get("width").and_then(|v| parse_length(v)) {
        style.width = Some(px);
    }
    if let Some(px) = attrs.get("height").and_then(|v| parse_length(v)) {
        style.height = Some(px);
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
        // We do not map faces to bundled families yet; keep the default UI face.
        let _ = face;
    }
    if let Some(span) = attrs.get("colspan").and_then(|v| v.trim().parse::<usize>().ok()) {
        style.colspan = span.max(1);
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
        match name.as_str() {
            "color" => {
                if let Some(color) = parse_color(value) {
                    style.color = color;
                }
            }
            "background" | "background-color" => {
                if let Some(color) = parse_color(value) {
                    style.background = Some(color);
                }
            }
            "font-size" => {
                if let Some(px) = parse_length(value) {
                    style.font_size = px;
                }
            }
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
            "margin" | "margin-top" => style.margin_top = parse_length(value).unwrap_or(0.0),
            "margin-bottom" => style.margin_bottom = parse_length(value).unwrap_or(0.0),
            "padding" => style.padding = parse_length(value).unwrap_or(0.0),
            "border" | "border-width" => {
                style.border = parse_length(value).unwrap_or(0.0);
            }
            "border-color" => {
                if let Some(color) = parse_color(value) {
                    style.border_color = color;
                }
            }
            "width" => style.width = parse_length(value),
            "height" => style.height = parse_length(value),
            "display" => {
                style.display = match lower.as_str() {
                    "none" => Display::None,
                    "inline" | "inline-block" => Display::Inline,
                    "table" => Display::Table,
                    "table-row" => Display::TableRow,
                    "table-cell" => Display::TableCell,
                    _ => Display::Block,
                };
            }
            // Deliberately unsupported: float, position, flex, grid, transform, line-height,
            // vertical-align, media queries, pseudo-elements, web fonts (E6.4).
            _ => {}
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

/// Parse `12px`, `12`, `1.5em`, `9pt`. Percentages and unknown units return `None`.
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
        _ => None, // including %, which the layout does not resolve
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
        assert_eq!(style.padding, 3.0);
    }

    #[test]
    fn unsupported_properties_are_dropped() {
        let mut style = Style::default();
        let before = style.font_size;
        apply_inline_style(&mut style, "float: left; position: absolute; font-size: 20px");
        assert_eq!(style.font_size, 20.0);
        // No field exists for float/position, so nothing to assert but non-mutation of others.
        assert!(!style.bold);
        let _ = before;
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
    fn font_tag_size_maps_to_the_legacy_scale() {
        let style = resolve_style("font", &attrs(&[("size", "5")]), &Style::default());
        assert_eq!(style.font_size, 22.0);
    }
}
