//! Message-list preview text (plan.md E5.3).
//!
//! Wrapping and the two-line clamp are GPUI's own (`line_clamp` + `text_ellipsis`), which shapes
//! the real text at the row's real width. An earlier hand-rolled clamp estimated width at a fixed
//! 6.5px per character in a fixed 300px box, and never split a word wider than a line — so a long
//! URL in a preview counted as one line while GPUI wrapped it onto four, and it overflowed its row.
//!
//! What is left here is the part GPUI cannot know: a row field is one flowing paragraph.

/// Collapse every run of whitespace — including the newlines and tabs that previews and folded
/// headers carry — into a single space, and trim the ends. Without this a preview spends its two
/// lines on hard line breaks instead of text.
pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines_and_runs_of_whitespace_become_single_spaces() {
        assert_eq!(
            single_line("View as a Web page:\r\n  https://trk.example/m/1\tthen"),
            "View as a Web page: https://trk.example/m/1 then"
        );
    }

    #[test]
    fn ends_are_trimmed_and_blank_text_is_empty() {
        assert_eq!(single_line("  \n hello \n "), "hello");
        assert_eq!(single_line(" \r\n\t "), "");
    }

    // Regression (ZipRecruiter): a preview that is mostly one long URL. Normalizing must not break
    // or drop it; splitting it across lines is the renderer's job, at the real width.
    #[test]
    fn long_unbroken_tokens_are_kept_whole() {
        let url = "<https://www.ziprecruiter.com/km/AAF1Fs6XUlfR86Qf91lmmQqvDFuyaei4jKQDW6jhp\
                   bVnBGkpyN7Mt7S2TUSejuxsQXILI33K2VIZZwMzR6B>";
        assert_eq!(single_line(&format!("\n{url}\n")), url);
    }
}
