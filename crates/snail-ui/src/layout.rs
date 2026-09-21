//! A minimal block/inline/table layout for the E0.3/E6 prototype: it turns the resolved DOM into a
//! flat display list of positioned fragments, using a `TextMeasure` for word advance widths.
//!
//! This is the seed of plan.md E6.5 (and a first cut at 6.6). It is deliberately small: block
//! stacking, greedy inline wrapping with alignment, and table columns with padding and borders.
//! Geometry is unit-tested against a fake metrics implementation, which is the whole reason layout
//! lives here rather than in the GPUI crate.

use crate::html::{Align, Color, Display, Element, FontSpec, Node, Style};

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
    Rect { rect: Rect, color: Color, radius: f32 },
    /// A stroked rectangle border.
    Border { rect: Rect, color: Color, width: f32 },
    /// A single laid-out word, positioned by its top-left corner.
    Word {
        x: f32,
        y: f32,
        text: String,
        font: FontSpec,
        color: Color,
        underline: bool,
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
            Fragment::Word { x, y, text, font, .. } => Some((text.as_str(), *x, *y, font.size)),
            _ => None,
        })
    }
}

/// Measures the natural advance width of a string in a font, with no wrapping.
pub trait TextMeasure {
    fn width(&self, text: &str, font: &FontSpec) -> f32;
}

const LINE_FACTOR: f32 = 1.45;

pub fn line_height(font: &FontSpec) -> f32 {
    font.size * LINE_FACTOR
}

#[derive(Clone, Debug)]
enum Token {
    Word {
        text: String,
        font: FontSpec,
        color: Color,
        underline: bool,
    },
    Break,
}

#[derive(Clone, Debug)]
struct LineItem {
    x: f32,
    text: String,
    font: FontSpec,
    color: Color,
    underline: bool,
}

#[derive(Clone, Debug, Default)]
struct Line {
    items: Vec<LineItem>,
    width: f32,
    max_size: f32,
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

    for node in nodes {
        match node {
            Node::Text(text) => push_words(text, parent, &mut tokens),
            Node::Element(element) => match element.style.display {
                Display::None => {}
                Display::Inline if element.tag == "br" => tokens.push(Token::Break),
                Display::Inline if element.tag == "img" => {
                    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
                    cursor += layout_image(element, x, cursor, out);
                }
                Display::Inline => collect_inline(std::slice::from_ref(node), parent, &mut tokens),
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
            },
        }
    }
    cursor += flush(&mut tokens, x, cursor, width, parent, out, measure);
    cursor - y
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
    let box_w = style.width.map(|w| w.min(available)).unwrap_or(available);
    let inset = style.border + style.padding;
    let content_w = (box_w - 2.0 * inset).max(0.0);
    let content_x = x + inset;
    let content_y = y + style.margin_top + inset;

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
    let total_h = inner_h + 2.0 * inset;

    if let Some(background) = style.background {
        out.push(Fragment::Rect {
            rect: Rect {
                x,
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
                x,
                y: y + style.margin_top,
                w: box_w,
                h: total_h,
            },
            src: src.clone(),
        });
    }
    if style.border > 0.0 && style.background.is_some() {
        out.push(Fragment::Border {
            rect: Rect {
                x,
                y: y + style.margin_top,
                w: box_w,
                h: total_h,
            },
            color: style.border_color,
            width: style.border,
        });
    }
    out.append(&mut inner);
    style.margin_top + total_h + style.margin_bottom
}

fn layout_image(element: &Element, x: f32, y: f32, out: &mut Vec<Fragment>) -> f32 {
    let w = element.style.width.unwrap_or(260.0).min(600.0);
    let h = element.style.height.unwrap_or(w * 0.4);
    out.push(Fragment::Image {
        rect: Rect { x, y, w, h },
        src: element
            .attrs
            .get("src")
            .cloned()
            .unwrap_or_default(),
    });
    h + 8.0
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
    let cells: Vec<Vec<&Element>> = rows
        .iter()
        .map(|row| {
            row.children
                .iter()
                .filter_map(as_element)
                .filter(|cell| cell.style.display == Display::TableCell)
                .collect()
        })
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);

    // Natural column widths: the longest word, but willing to grow toward a single short line.
    let mut widths = vec![0f32; columns];
    for row in &cells {
        for (index, cell) in row.iter().enumerate() {
            let inset = 2.0 * (cell.style.border + cell.style.padding);
            let natural = natural_width(&cell.children, measure);
            widths[index] = widths[index].max(natural + inset);
        }
    }
    let table_inset = 2.0 * (table.style.border + table.style.padding);
    let total: f32 = widths.iter().sum();
    let target = (available - table_inset).max(1.0);
    if total > target {
        let scale = target / total;
        for width in &mut widths {
            *width *= scale;
        }
    } else if total < target {
        let extra = (target - total) / columns as f32;
        for width in &mut widths {
            *width += extra;
        }
    }

    let mut cursor_y = y + table.style.margin_top;
    let top = cursor_y;
    let mut spans: Vec<(f32, f32)> = Vec::new(); // (x offset, width) per column
    {
        let mut offset = x + table.style.border + table.style.padding;
        for width in &widths {
            spans.push((offset, *width));
            offset += width;
        }
    }
    let table_width: f32 = widths.iter().sum::<f32>() + table_inset;

    for row in &cells {
        let mut cursor_x = x + table.style.border + table.style.padding;
        let mut row_height: f32 = 0.0;
        let mut laid: Vec<(f32, f32, f32, &Element, Vec<Fragment>)> = Vec::new();
        for (index, cell) in row.iter().enumerate() {
            let width = widths[index.min(columns - 1)];
            let inset = cell.style.border + cell.style.padding;
            let content_w = (width - 2.0 * inset).max(0.0);
            let mut inner = Vec::new();
            let inner_h = layout_children(
                &cell.children,
                cursor_x + inset,
                cursor_y + inset,
                content_w,
                &cell.style,
                &mut inner,
                measure,
            );
            let cell_h = inner_h + 2.0 * inset;
            row_height = row_height.max(cell_h);
            laid.push((cursor_x, width, cell_h, cell, inner));
            cursor_x += width;
        }
        for (cell_x, width, cell_h, cell, inner) in laid {
            let height = cell_h.max(row_height);
            if let Some(background) = cell.style.background {
                out.push(Fragment::Rect {
                    rect: Rect {
                        x: cell_x,
                        y: cursor_y,
                        w: width,
                        h: height,
                    },
                    color: background,
                    radius: 0.0,
                });
            }
            if let Some(src) = &cell.style.background_image {
                out.push(Fragment::Image {
                    rect: Rect {
                        x: cell_x,
                        y: cursor_y,
                        w: width,
                        h: height,
                    },
                    src: src.clone(),
                });
            }
            if cell.style.border > 0.0 {
                out.push(Fragment::Border {
                    rect: Rect {
                        x: cell_x,
                        y: cursor_y,
                        w: width,
                        h: height,
                    },
                    color: cell.style.border_color,
                    width: cell.style.border,
                });
            }
            out.extend(inner);
        }
        cursor_y += row_height;
    }

    if table.style.border > 0.0 {
        out.push(Fragment::Border {
            rect: Rect {
                x,
                y: top,
                w: table_width,
                h: cursor_y - top,
            },
            color: table.style.border_color,
            width: table.style.border,
        });
    }
    let _ = spans;
    cursor_y - y + table.style.margin_bottom
}

fn collect_rows(table: &Element) -> Vec<&Element> {
    let mut rows = Vec::new();
    for child in table.children.iter().filter_map(as_element) {
        match child.style.display {
            Display::TableRow => rows.push(child),
            // thead/tbody/tfoot
            _ => rows.extend(collect_rows(child)),
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

/// Longest word width, capped by a single-line width so short cells do not become a whole column.
fn natural_width(nodes: &[Node], measure: &dyn TextMeasure) -> f32 {
    let mut tokens = Vec::new();
    collect_inline(nodes, &Style::default(), &mut tokens);
    let mut longest = 0f32;
    let mut total = 0f32;
    for token in &tokens {
        if let Token::Word { text, font, .. } = token {
            longest = longest.max(measure.width(text, font));
            total += measure.width(text, font) + measure.width(" ", font);
        }
    }
    longest.max(total.min(220.0))
}

fn collect_inline(nodes: &[Node], parent: &Style, out: &mut Vec<Token>) {
    for node in nodes {
        match node {
            Node::Text(text) => push_words(text, parent, out),
            Node::Element(element) => match element.style.display {
                Display::None => {}
                Display::Inline if element.tag == "br" => out.push(Token::Break),
                Display::Inline => collect_inline(&element.children, &element.style, out),
                _ => {}
            },
        }
    }
}

fn push_words(text: &str, style: &Style, out: &mut Vec<Token>) {
    let font = FontSpec {
        size: style.font_size,
        bold: style.bold,
        italic: style.italic,
    };
    for word in text.split_whitespace() {
        out.push(Token::Word {
            text: word.to_string(),
            font,
            color: style.color,
            underline: style.underline,
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
                line.max_size = line.max_size.max(style.font_size);
                lines.push(line);
                line = Line::default();
            }
            Token::Word {
                text,
                font,
                color,
                underline,
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
                line.max_size = line.max_size.max(font.size);
                line.items.push(LineItem {
                    x: item_x,
                    text,
                    font,
                    color,
                    underline,
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
            out.push(Fragment::Word {
                x: x + offset + item.x,
                y: cursor,
                text: item.text,
                font: item.font,
                color: item.color,
                underline: item.underline,
            });
        }
        cursor += line_height(&FontSpec {
            size: line.max_size.max(1.0),
            bold: false,
            italic: false,
        });
    }
    cursor - y
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
                tr.children
                    .push(Node::Element(element("td", vec![Node::Text(cell.to_string())])));
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
}
