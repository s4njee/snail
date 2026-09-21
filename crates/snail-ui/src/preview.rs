//! Preview clamping (plan.md E5.3). GPUI has no `-webkit-line-clamp`, so the truncation is computed
//! here from the measured width and the two-line box, against a `measure` the caller supplies
//! (GPUI in the app, a fake in tests).

/// Wrap `text` to `width` and clamp to `max_lines`, ending the last line with `…` when it is cut.
///
/// `measure` returns the rendered width of a candidate line.
pub fn clamp(
    text: &str,
    max_lines: usize,
    width: f32,
    measure: &dyn Fn(&str) -> f32,
) -> String {
    if max_lines == 0 {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;

    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if current.is_empty() || measure(&candidate) <= width {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                truncated = true;
                break;
            }
            current = word.to_string();
        }
    }

    if !truncated {
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        lines.truncate(max_lines);
    }

    if truncated {
        if let Some(last) = lines.last_mut() {
            *last = ellipsize(last, width, measure);
        }
    }

    lines.join("\n")
}

fn ellipsize(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    let mut text = line.to_string();
    loop {
        let candidate = format!("{text} …");
        if measure(&candidate) <= width {
            return candidate;
        }
        if text.is_empty() {
            return "…".to_string();
        }
        text.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10px per character, so a 100px line holds ten.
    fn measure(line: &str) -> f32 {
        line.chars().count() as f32 * 10.0
    }

    #[test]
    fn short_text_is_unchanged_on_one_line() {
        assert_eq!(clamp("hello world", 2, 200.0, &measure), "hello world");
    }

    #[test]
    fn text_that_fits_in_two_lines_keeps_both() {
        let clamped = clamp("aaaa bbbb cccc dddd", 2, 100.0, &measure);
        assert_eq!(clamped, "aaaa bbbb\ncccc dddd");
    }

    #[test]
    fn text_past_the_clamp_is_truncated_with_an_ellipsis() {
        let clamped = clamp("aaaa bbbb cccc dddd eeee ffff", 2, 100.0, &measure);
        let lines: Vec<&str> = clamped.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{clamped}");
        for line in lines {
            assert!(measure(line) <= 100.0, "{line} is too wide");
        }
    }

    #[test]
    fn a_single_word_wider_than_the_box_still_renders() {
        let clamped = clamp("supercalifragilistic", 1, 50.0, &measure);
        assert!(!clamped.is_empty());
    }

    #[test]
    fn zero_lines_is_empty() {
        assert_eq!(clamp("anything", 0, 100.0, &measure), "");
    }
}
