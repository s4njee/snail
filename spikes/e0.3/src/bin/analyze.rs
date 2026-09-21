//! E0.3 analyze — parse, body-select and sanitize the corpus, and emit the failure taxonomy.
//!
//! Run from `spikes/e0.3`:
//!   cargo run --bin e0-3-analyze            # ../corpus by default
//!   cargo run --bin e0-3-analyze -- <dir> <outdir>
//!
//! The plan's go/no-go is "all 20 readable and a screenful laid out in <16 ms". This binary
//! produces the *taxonomy* (what the corpus actually contains, and what falls outside the E6.4
//! subset) and the parse+sanitize timings; readability and layout belong to `e0-3-render`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mail_parser::MessageParser;

#[derive(Default)]
struct Features {
    tables: usize,
    max_table_depth: usize,
    divs: usize,
    images: usize,
    cid_images: usize,
    remote_images: usize,
    style_attr: usize,
    style_blocks: usize,
    bgcolor: usize,
    colspan: usize,
    rowspan: usize,
    font_tags: usize,
    center_tags: usize,
    class_attr: usize,
    unsupported: usize,
    format_flowed: bool,
}

impl Features {
    /// The E6.4 "not supported and deliberately so" list plus the legacy presentational attributes
    /// E6.3 has to resolve.
    const UNSUPPORTED_TOKENS: &'static [&'static str] =
        &["float", "position:", "display:flex", "display: flex", "display:grid", "display: grid", "transform", "animation", "@media"];

    fn scan(html: &str) -> Self {
        let lower = html.to_ascii_lowercase();
        let count = |needle: &str| lower.matches(needle).count();
        let unsupported = Self::UNSUPPORTED_TOKENS
            .iter()
            .map(|token| count(token))
            .sum();
        Self {
            tables: count("<table"),
            max_table_depth: max_table_depth(&lower),
            divs: count("<div"),
            images: count("<img"),
            cid_images: count("cid:"),
            remote_images: count("http://") + count("https://"),
            style_attr: count("style="),
            style_blocks: count("<style"),
            bgcolor: count("bgcolor"),
            colspan: count("colspan"),
            rowspan: count("rowspan"),
            font_tags: count("<font"),
            center_tags: count("<center"),
            class_attr: count("class="),
            unsupported,
            format_flowed: false,
        }
    }
}

fn max_table_depth(lower: &str) -> usize {
    let mut depth: usize = 0;
    let mut max = 0usize;
    let mut cursor = 0;
    while cursor < lower.len() {
        let open = lower[cursor..].find("<table").map(|i| cursor + i);
        let close = lower[cursor..].find("</table").map(|i| cursor + i);
        match (open, close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                max = max.max(depth);
                cursor = o + 6;
            }
            (_, Some(c)) => {
                depth = depth.saturating_sub(1);
                cursor = c + 7;
            }
            (Some(o), None) => {
                depth += 1;
                max = max.max(depth);
                cursor = o + 6;
            }
            (None, None) => break,
        }
    }
    max
}

struct Row {
    name: String,
    kind: &'static str,
    raw_bytes: usize,
    sanitized_bytes: usize,
    features: Features,
    parse_ms: f64,
    sanitize_ms: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("../corpus"));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("sanitized"));
    fs::create_dir_all(&out)?;

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "eml"))
        .collect();
    files.sort();

    println!(
        "{} corpus messages from {}\n",
        files.len(),
        dir.display()
    );
    println!(
        "{:<34} {:>5} {:>8} {:>8} {:>3} {:>4} {:>4} {:>4} {:>5} {:>7} {:>7}",
        "message", "kind", "raw", "clean", "tbl", "dp", "cid", "rem", "unsup", "parse", "sanit"
    );

    let mut rows = Vec::new();
    for path in &files {
        let bytes = fs::read(path)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let parse_start = Instant::now();
        let parsed = MessageParser::default().parse(&bytes);
        let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;

        let (kind, body): (&'static str, String) = match &parsed {
            Some(message) => {
                if let Some(html) = message.body_html(0) {
                    ("html", html.to_string())
                } else if let Some(text) = message.body_text(0) {
                    ("text", text.to_string())
                } else {
                    ("none", String::new())
                }
            }
            None => ("none", String::new()),
        };

        let mut features = Features::scan(&body);
        features.format_flowed = body.to_ascii_lowercase().contains("format=flowed");

        let sanitize_start = Instant::now();
        let sanitized = if kind == "html" {
            clean_html(&body)
        } else if kind == "text" {
            format!("<pre>{}</pre>", escape(&body))
        } else {
            String::new()
        };
        let sanitize_ms = sanitize_start.elapsed().as_secs_f64() * 1000.0;

        if !sanitized.is_empty() {
            let document = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"></head><body>{sanitized}</body></html>"
            );
            fs::write(out.join(format!("{name}.html")), document)?;
        }

        println!(
            "{:<34} {:>5} {:>8} {:>8} {:>3} {:>4} {:>4} {:>4} {:>5} {:>6.2}ms {:>6.2}ms",
            truncate(&name, 34),
            kind,
            bytes.len(),
            sanitized.len(),
            features.tables,
            features.max_table_depth,
            features.cid_images,
            features.remote_images,
            features.unsupported,
            parse_ms,
            sanitize_ms,
        );

        rows.push(Row {
            name,
            kind,
            raw_bytes: bytes.len(),
            sanitized_bytes: sanitized.len(),
            features,
            parse_ms,
            sanitize_ms,
        });
    }

    summarize(&rows, &out);
    Ok(())
}

fn summarize(rows: &[Row], out: &Path) {
    let html = rows.iter().filter(|r| r.kind == "html").count();
    let text = rows.iter().filter(|r| r.kind == "text").count();
    let none = rows.iter().filter(|r| r.kind == "none").count();
    let tables = rows.iter().filter(|r| r.features.tables > 0).count();
    let nested2 = rows.iter().filter(|r| r.features.max_table_depth >= 2).count();
    let nested3 = rows.iter().filter(|r| r.features.max_table_depth >= 3).count();
    let cid = rows.iter().filter(|r| r.features.cid_images > 0).count();
    let unsupported = rows.iter().filter(|r| r.features.unsupported > 0).count();
    let flowed = rows.iter().filter(|r| r.features.format_flowed).count();

    let parse_max = rows.iter().map(|r| r.parse_ms).fold(0.0f64, f64::max);
    let sanitize_max = rows.iter().map(|r| r.sanitize_ms).fold(0.0f64, f64::max);
    let parse_avg = rows.iter().map(|r| r.parse_ms).sum::<f64>() / rows.len().max(1) as f64;
    let sanitize_avg = rows.iter().map(|r| r.sanitize_ms).sum::<f64>() / rows.len().max(1) as f64;

    println!("\n--- taxonomy ---");
    println!("  HTML bodies: {html}   plain-text bodies: {text}   neither: {none}");
    println!("  contain tables: {tables}   nested >=2: {nested2}   nested >=3: {nested3}");
    println!("  inline (cid:) images: {cid}   format=flowed: {flowed}");
    println!("  use a deliberately-unsupported CSS/property: {unsupported}");
    println!("  parse  avg {parse_avg:.2}ms max {parse_max:.2}ms");
    println!("  sanitize avg {sanitize_avg:.2}ms max {sanitize_max:.2}ms");
    println!("\n  sanitized documents written to {}", out.display());

    let worst: Vec<&Row> = {
        let mut sorted: Vec<&Row> = rows.iter().collect();
        sorted.sort_by(|a, b| b.features.unsupported.cmp(&a.features.unsupported));
        sorted.into_iter().filter(|r| r.features.unsupported > 0).take(5).collect()
    };
    if !worst.is_empty() {
        println!("\n  worst offenders (unsupported constructs):");
        for row in worst {
            println!(
                "    {:<40} unsupported={} tables={} depth={} remote={}",
                truncate(&row.name, 40),
                row.features.unsupported,
                row.features.tables,
                row.features.max_table_depth,
                row.features.remote_images,
            );
        }
    }
    println!("\n  E6.13b: run `cargo run --features gui --bin e0-3-render -- {}", out.display());
}

/// The E6.2 allowlist. Remote content is a placeholder, not a setting, so images are rewritten.
fn clean_html(html: &str) -> String {
    ammonia::Builder::default()
        .add_tags(["table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup", "col"])
        .add_tag_attributes("table", ["width", "height", "border", "cellpadding", "cellspacing", "bgcolor", "align"])
        .add_tag_attributes("td", ["width", "height", "bgcolor", "align", "valign", "colspan", "rowspan"])
        .add_tag_attributes("th", ["width", "height", "bgcolor", "align", "valign", "colspan", "rowspan"])
        .add_tag_attributes("tr", ["bgcolor", "align", "valign"])
        .add_tags(["font"])
        .add_tag_attributes("font", ["size", "color", "face"])
        .add_tag_attributes("img", ["src", "alt", "width", "height"])
        .clean(html)
        .to_string()
}

fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        input.to_string()
    } else {
        let head: String = input.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
