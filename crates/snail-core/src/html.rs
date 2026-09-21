//! Safe, UI-agnostic HTML ingestion for the message renderer (plan.md E6.2/E6.3).
//!
//! This module owns the untrusted-document boundary. It extracts the supported stylesheet before
//! sanitizing, rewrites every remote image URL to an inert local token, and parses the result into
//! a deliberately small DOM that `snail-ui` can style and lay out without depending on html5ever.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub const REMOTE_TOKEN_PREFIX: &str = "snail-remote:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteResource {
    pub token: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtmlNode {
    Element(HtmlElement),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlElement {
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub children: Vec<HtmlNode>,
}

#[derive(Clone, Debug)]
pub struct SanitizedDocument {
    /// Safe HTML suitable for the browser escape hatch and TextView fallback. Remote image URLs
    /// have already become inert `snail-remote:` tokens.
    pub html: String,
    /// CSS extracted before ammonia removes `<style>` elements, with remote URLs rewritten too.
    pub stylesheet: String,
    pub roots: Vec<HtmlNode>,
    pub remote_resources: Vec<RemoteResource>,
    pub node_count: usize,
    pub plain_text: String,
}

/// Sanitize and parse one HTML message. This function never permits a renderer to see a network
/// image URL; callers must explicitly fetch `remote_resources` and associate decoded bytes with
/// the corresponding inert token.
pub fn sanitize_document(source: &str) -> SanitizedDocument {
    let without_head = strip_document_head(source);
    let resources = Arc::new(Mutex::new(Vec::<RemoteResource>::new()));
    let filter_resources = resources.clone();
    let properties = supported_style_properties();

    let mut builder = ammonia::Builder::default();
    builder
        .tags(HashSet::from([
            "a",
            "article",
            "b",
            "big",
            "blockquote",
            "br",
            "caption",
            "center",
            "code",
            "col",
            "colgroup",
            "div",
            "em",
            "font",
            "footer",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "header",
            "hr",
            "i",
            "img",
            "label",
            "li",
            "ol",
            "p",
            "pre",
            "section",
            "small",
            "span",
            "strong",
            "sub",
            "sup",
            "s",
            "strike",
            "del",
            "table",
            "tbody",
            "td",
            "tfoot",
            "th",
            "thead",
            "tr",
            "u",
            "ul",
        ]))
        // Dropped with their content. `title` is here because email templates paste whole
        // documents into the body — `<!doctype html><html><head><title>Untitled Document…` — and
        // a `<title>` there still parses, as text a browser never shows.
        .clean_content_tags(HashSet::from([
            "head", "style", "script", "iframe", "object", "form", "title",
        ]))
        .generic_attributes(HashSet::from([
            "style",
            "class",
            "id",
            "align",
            "valign",
            "bgcolor",
            "background",
            "width",
            "height",
            "colspan",
            "rowspan",
            "border",
            "cellpadding",
            "cellspacing",
            "color",
            "face",
            "size",
            "src",
            "alt",
            "href",
            "title",
        ]))
        .url_schemes(HashSet::from([
            "http",
            "https",
            "mailto",
            "cid",
            REMOTE_TOKEN_PREFIX.trim_end_matches(':'),
        ]))
        .filter_style_properties(properties)
        .attribute_filter(move |tag, attribute, value| {
            // Event attributes are not on the allowlist, but reject them here as defense in depth.
            if attribute.to_ascii_lowercase().starts_with("on") {
                return None;
            }
            if is_remote_resource_attribute(tag, attribute) && is_remote_url(value) {
                let token = register_remote(&filter_resources, value);
                return Some(Cow::Owned(token));
            }
            if attribute == "style" {
                return Some(Cow::Owned(rewrite_css_remote_urls(
                    value,
                    &filter_resources,
                )));
            }
            if is_dangerous_url(value) && matches!(attribute, "src" | "href" | "background") {
                return None;
            }
            Some(Cow::Borrowed(value))
        });

    let clean = builder.clean(&without_head).to_string();
    let html = rewrite_css_remote_urls(&clean, &resources);
    let stylesheet = rewrite_css_remote_urls(&extract_styles(source), &resources);
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html.clone());
    let mut roots = Vec::new();
    let mut node_count = 0;
    build_children(&dom.document, &mut roots, &mut node_count);
    let plain_text = flatten_text(&roots);
    let remote_resources = resources.lock().expect("remote resource lock").clone();

    SanitizedDocument {
        html,
        stylesheet,
        roots,
        remote_resources,
        node_count,
        plain_text,
    }
}

fn supported_style_properties() -> HashSet<&'static str> {
    [
        "color",
        "background",
        "background-color",
        "font",
        "font-family",
        "font-size",
        "font-weight",
        "font-style",
        "line-height",
        "text-decoration",
        "text-decoration-line",
        "text-align",
        "vertical-align",
        "list-style",
        "list-style-type",
        "margin",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "padding",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "border",
        "border-top",
        "border-right",
        "border-bottom",
        "border-left",
        "border-width",
        "border-color",
        "border-style",
        "border-radius",
        "border-top-width",
        "border-right-width",
        "border-bottom-width",
        "border-left-width",
        "border-top-style",
        "border-right-style",
        "border-bottom-style",
        "border-left-style",
        "border-top-color",
        "border-right-color",
        "border-bottom-color",
        "border-left-color",
        "width",
        "height",
        "max-width",
        "display",
        "visibility",
        "background-image",
    ]
    .into_iter()
    .collect()
}

fn is_remote_resource_attribute(tag: &str, attribute: &str) -> bool {
    (tag == "img" && attribute == "src") || attribute == "background"
}

fn is_remote_url(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//")
}

fn is_dangerous_url(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && !character.is_control())
        .collect();
    let lower = compact.to_ascii_lowercase();
    lower.starts_with("javascript:") || lower.starts_with("data:") || lower.starts_with("vbscript:")
}

fn register_remote(resources: &Arc<Mutex<Vec<RemoteResource>>>, url: &str) -> String {
    let mut resources = resources.lock().expect("remote resource lock");
    if let Some(existing) = resources.iter().find(|entry| entry.url == url) {
        return existing.token.clone();
    }
    let token = format!("{REMOTE_TOKEN_PREFIX}{}", resources.len());
    resources.push(RemoteResource {
        token: token.clone(),
        url: url.to_string(),
    });
    token
}

/// Replace `url(http...)` in supported CSS. The scanner is intentionally narrow: quoted and
/// unquoted url() forms are handled; arbitrary CSS syntax remains outside the supported subset.
fn rewrite_css_remote_urls(input: &str, resources: &Arc<Mutex<Vec<RemoteResource>>>) -> String {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find("url(") {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        let value_start = start + 4;
        let Some(close_relative) = lower[value_start..].find(')') else {
            output.push_str(&input[start..]);
            return output;
        };
        let close = value_start + close_relative;
        let raw = input[value_start..close].trim();
        let quote = raw
            .chars()
            .next()
            .filter(|character| matches!(character, '\'' | '"'));
        let value = quote
            .and_then(|quote| raw.strip_prefix(quote)?.strip_suffix(quote))
            .unwrap_or(raw)
            .trim();
        if is_remote_url(value) {
            let token = register_remote(resources, value);
            output.push_str("url('");
            output.push_str(&token);
            output.push_str("')");
        } else if is_dangerous_url(value) {
            output.push_str("none");
        } else {
            output.push_str(&input[start..=close]);
        }
        cursor = close + 1;
    }
    output.push_str(&input[cursor..]);
    output
}

fn strip_document_head(html: &str) -> String {
    let lowercase = html.to_ascii_lowercase();
    let mut cursor = 0;
    let start = loop {
        let Some(relative) = lowercase[cursor..].find("<head") else {
            return html.to_string();
        };
        let candidate = cursor + relative;
        let terminator = lowercase.as_bytes().get(candidate + 5).copied();
        if terminator.is_some_and(|byte| byte == b'>' || byte.is_ascii_whitespace()) {
            break candidate;
        }
        cursor = candidate + 5;
    };
    let Some(relative_end) = lowercase[start..].find("</head>") else {
        return html.to_string();
    };
    let end = start + relative_end + "</head>".len();
    let mut body = String::with_capacity(html.len() - (end - start));
    body.push_str(&html[..start]);
    body.push_str(&html[end..]);
    body
}

fn extract_styles(html: &str) -> String {
    // A `<style>` inside a comment is not a stylesheet. Outlook's `<!--[if mso]><style>…` blocks
    // are exactly that to every other client, and they carry rules like
    // `p { margin: 0 !important }` that would flatten the real layout.
    let html = &strip_comments(html);
    let lower = html.to_ascii_lowercase();
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(open) = lower[cursor..].find("<style") {
        let open = cursor + open;
        let Some(gt) = lower[open..].find('>') else {
            break;
        };
        let start = open + gt + 1;
        let Some(close) = lower[start..].find("</style") else {
            break;
        };
        let end = start + close;
        output.push_str(&html[start..end]);
        output.push('\n');
        cursor = end + 8;
    }
    output
}

/// Remove `<!-- … -->` comments as an HTML parser would. The "downlevel-revealed" form
/// `<!--[if !mso]><!-->…<!--<![endif]-->` is two short comments around live content, so its
/// content survives, as it does in a browser.
///
/// Inside `<style>` nothing is a comment: `<style><!-- .x { … } --></style>` is the old way of
/// hiding CSS from ancient browsers, and CSS itself ignores those markers, so it is copied as is.
fn strip_comments(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut output = String::with_capacity(html.len());
    let mut at = 0;
    loop {
        let comment = lower[at..].find("<!--").map(|found| at + found);
        let style = lower[at..].find("<style").map(|found| at + found);
        match (comment, style) {
            (Some(comment), style) if style.is_none_or(|style| comment < style) => {
                output.push_str(&html[at..comment]);
                match lower[comment + 4..].find("-->") {
                    Some(end) => at = comment + 4 + end + 3,
                    None => return output,
                }
            }
            (_, Some(style)) => {
                let end = lower[style..]
                    .find("</style")
                    .map(|found| style + found)
                    .unwrap_or(html.len());
                output.push_str(&html[at..end]);
                at = end;
                if at == html.len() {
                    return output;
                }
                // Step past `</style` so the next search starts after it.
                output.push_str(&html[at..at + 7]);
                at += 7;
            }
            _ => {
                output.push_str(&html[at..]);
                return output;
            }
        }
    }
}

fn build_children(handle: &Handle, output: &mut Vec<HtmlNode>, count: &mut usize) {
    for child in handle.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = contents.borrow().to_string();
                if !text.is_empty() {
                    output.push(HtmlNode::Text(text));
                    *count += 1;
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.to_string();
                if matches!(
                    tag.as_str(),
                    "script" | "style" | "head" | "meta" | "title" | "link"
                ) {
                    continue;
                }
                let attrs = attrs
                    .borrow()
                    .iter()
                    .map(|attribute| {
                        (
                            attribute.name.local.to_string(),
                            attribute.value.to_string(),
                        )
                    })
                    .collect();
                let mut children = Vec::new();
                build_children(child, &mut children, count);
                output.push(HtmlNode::Element(HtmlElement {
                    tag,
                    attrs,
                    children,
                }));
                *count += 1;
            }
            _ => build_children(child, output, count),
        }
    }
}

fn flatten_text(nodes: &[HtmlNode]) -> String {
    fn visit(nodes: &[HtmlNode], output: &mut String) {
        for node in nodes {
            match node {
                HtmlNode::Text(text) => {
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        if !output.is_empty() && !output.ends_with([' ', '\n']) {
                            output.push(' ');
                        }
                        output.push_str(&text);
                    }
                }
                HtmlNode::Element(element) => {
                    let block = matches!(
                        element.tag.as_str(),
                        "p" | "div"
                            | "table"
                            | "tr"
                            | "li"
                            | "blockquote"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                            | "br"
                    );
                    if block && !output.is_empty() && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    visit(&element.children, output);
                    if block && !output.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        }
    }
    let mut output = String::new();
    visit(nodes, &mut output);
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_active_content_and_dangerous_urls() {
        let document = sanitize_document(
            r#"<head><title>Secret</title><style>.x{color:red}</style></head>
            <body onload="steal()"><script>bad()</script><iframe>bad</iframe>
            <a href="javascript:bad()">bad link</a><img src="data:image/png;base64,xx">
            <p class="x">Safe</p></body>"#,
        );
        assert!(!document.html.contains("Secret"));
        assert!(!document.html.contains("script"));
        assert!(!document.html.contains("iframe"));
        assert!(!document.html.contains("onload"));
        assert!(!document.html.contains("javascript:"));
        assert!(!document.html.contains("data:image"));
        assert!(document.stylesheet.contains(".x{color:red}"));
        assert!(document.plain_text.contains("Safe"));
    }

    #[test]
    fn remote_resources_are_inert_and_deduplicated() {
        let document = sanitize_document(
            r#"<img src="https://tracker.test/pixel.png"><div style="background-image:url('https://tracker.test/pixel.png')"></div>"#,
        );
        assert_eq!(document.remote_resources.len(), 1);
        assert_eq!(
            document.remote_resources[0].url,
            "https://tracker.test/pixel.png"
        );
        assert!(!document.html.contains("https://tracker.test"));
        assert!(document.html.matches(REMOTE_TOKEN_PREFIX).count() >= 2);
    }

    // Regression (Samsung): a template pasted a whole second document into the body, and its
    // `<title>Untitled Document</title>` rendered as a line of text between two banners.
    #[test]
    fn a_title_inside_the_body_is_not_text() {
        let document = sanitize_document(
            "<body><p>Before</p><!doctype html><html><head><meta charset=\"utf-8\">\
             <title>Untitled Document</title></head><body></body></html><p>After</p></body>",
        );
        assert!(!document.html.contains("Untitled"), "{}", document.html);
        assert!(!document.plain_text.contains("Untitled"));
        assert!(document.plain_text.contains("Before") && document.plain_text.contains("After"));
    }

    // Regression (Slickdeals): an Outlook-only `<!--[if mso]><style>` block zeroed every
    // paragraph margin once `!important` rules were honoured.
    #[test]
    fn styles_inside_comments_are_not_stylesheets() {
        let document = sanitize_document(
            "<head><!--[if mso]><style>p { margin: 0 !important }</style><![endif]-->\
             <!--[if !mso]><!--><style>.shown { color: red }</style><!--<![endif]-->\
             <style>.real { color: blue }</style></head><body><p>x</p></body>",
        );
        assert!(
            !document.stylesheet.contains("margin: 0"),
            "{}",
            document.stylesheet
        );
        assert!(document.stylesheet.contains(".shown"));
        assert!(document.stylesheet.contains(".real"));
        // The old CSS-hiding convention keeps its rules.
        let hidden = sanitize_document("<style><!--\n.kept { color: red }\n--></style><p>x</p>");
        assert!(hidden.stylesheet.contains(".kept"), "{}", hidden.stylesheet);
    }

    #[test]
    fn header_elements_are_not_mistaken_for_the_document_head() {
        let document = sanitize_document("<header>Brand</header><main>Body</main>");
        assert!(document.plain_text.contains("Brand"));
        assert!(document.plain_text.contains("Body"));
    }
}
