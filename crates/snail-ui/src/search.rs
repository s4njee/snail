//! The search query language (plan.md E9.2), framework-free and unit-tested.
//!
//! Grammar, deliberately small:
//!
//! ```text
//! query   := token*
//! token   := bare | phrase | key ':' value
//! key     := from | to | subject | in | is | has | before | after
//! value   := bare | '"' … '"'
//! phrase  := '"' … '"'
//! ```
//!
//! Bare tokens and phrases are free text (AND-ed). `from:`, `to:` and `subject:` filter by field;
//! `in:mailbox`, `is:unread`/`is:read` and `has:attachment` are flags; `before:`/`after:` take a
//! date (`2026-09-01`) or a word (`today`, `yesterday`, `week`, `month`), and the two-token forms
//! `last week` / `last month` are understood too.
//!
//! **A parse error is never an error.** An unclosed quote or an unparseable date falls back to the
//! whole input as one literal term, so typing `foo: bar "` mid-thought still searches for it.

use std::fmt;

/// One parsed query. Empty fields mean "no constraint".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<String>,
    pub phrases: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub mailbox: Option<String>,
    /// `Some(true)` for `is:unread`, `Some(false)` for `is:read`.
    pub unread: Option<bool>,
    pub has_attachment: bool,
    /// `date >= after` (epoch seconds).
    pub after: Option<i64>,
    /// `date < before` (epoch seconds).
    pub before: Option<i64>,
    /// The input could not be parsed and was taken whole as one literal term.
    pub fallback: bool,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.phrases.is_empty() && !self.has_filters()
    }

    /// Whether any structured filter is set (so a search is meaningful even with no free text).
    pub fn has_filters(&self) -> bool {
        !self.from.is_empty()
            || !self.to.is_empty()
            || !self.subject.is_empty()
            || self.mailbox.is_some()
            || self.unread.is_some()
            || self.has_attachment
            || self.after.is_some()
            || self.before.is_some()
    }

    /// A short human line for the results header, e.g. `unread · from:alice · "quarterly"`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for term in &self.terms {
            parts.push(term.clone());
        }
        for phrase in &self.phrases {
            parts.push(format!("\"{phrase}\""));
        }
        for value in &self.from {
            parts.push(format!("from:{value}"));
        }
        for value in &self.to {
            parts.push(format!("to:{value}"));
        }
        for value in &self.subject {
            parts.push(format!("subject:{value}"));
        }
        if let Some(mailbox) = &self.mailbox {
            parts.push(format!("in:{mailbox}"));
        }
        match self.unread {
            Some(true) => parts.push("is:unread".into()),
            Some(false) => parts.push("is:read".into()),
            None => {}
        }
        if self.has_attachment {
            parts.push("has:attachment".into());
        }
        if self.after.is_some() {
            parts.push("after:…".into());
        }
        if self.before.is_some() {
            parts.push("before:…".into());
        }
        parts.join(" · ")
    }
}

/// Parse `input` against `now` (epoch seconds, for the relative date words). Never fails.
pub fn parse(input: &str, now: i64) -> Query {
    match parse_tokens(input, now) {
        Ok(query) => query,
        Err(()) => {
            let literal = input.split_whitespace().collect::<Vec<_>>().join(" ");
            Query {
                terms: if literal.is_empty() {
                    Vec::new()
                } else {
                    vec![literal]
                },
                fallback: true,
                ..Default::default()
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Token {
    prefix: Option<String>,
    value: String,
    quoted: bool,
}

fn parse_tokens(input: &str, now: i64) -> Result<Query, ()> {
    let tokens = tokenize(input)?;
    let mut query = Query::default();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        // The two-token relative words.
        if token.prefix.is_none()
            && !token.quoted
            && token.value.eq_ignore_ascii_case("last")
            && index + 1 < tokens.len()
        {
            let next = &tokens[index + 1];
            if next.prefix.is_none() {
                if next.value.eq_ignore_ascii_case("week") {
                    query.after = Some(now - 7 * 86_400);
                    index += 2;
                    continue;
                }
                if next.value.eq_ignore_ascii_case("month") {
                    query.after = Some(now - 30 * 86_400);
                    index += 2;
                    continue;
                }
            }
        }
        // A lone relative word.
        if token.prefix.is_none() && !token.quoted {
            if let Some(start) = relative_day(&token.value, now) {
                query.after = Some(start);
                index += 1;
                continue;
            }
        }

        match token.prefix.as_deref().map(str::to_ascii_lowercase) {
            None => {
                if token.value.is_empty() {
                    // An empty phrase is not a constraint.
                } else if token.quoted {
                    query.phrases.push(token.value.clone());
                } else {
                    query.terms.push(token.value.clone());
                }
            }
            Some(key) => {
                let value = token.value.clone();
                match key.as_str() {
                    "from" if !value.is_empty() => query.from.push(value),
                    "to" if !value.is_empty() => query.to.push(value),
                    "subject" if !value.is_empty() => query.subject.push(value),
                    "in" if !value.is_empty() => query.mailbox = Some(value),
                    "is" => match value.to_ascii_lowercase().as_str() {
                        "unread" => query.unread = Some(true),
                        "read" => query.unread = Some(false),
                        _ => return Err(()),
                    },
                    "has" => {
                        if value.eq_ignore_ascii_case("attachment") {
                            query.has_attachment = true;
                        } else {
                            return Err(());
                        }
                    }
                    "before" => match parse_date(&value, now) {
                        Some(date) => query.before = Some(date),
                        None => return Err(()),
                    },
                    "after" => match parse_date(&value, now) {
                        Some(date) => query.after = Some(date),
                        None => return Err(()),
                    },
                    // Unknown keys are not errors: `http://x` is just a term.
                    _ => {
                        let literal = format!("{key}:{value}");
                        if token.quoted {
                            query.phrases.push(literal);
                        } else {
                            query.terms.push(literal);
                        }
                    }
                }
            }
        }
        index += 1;
    }
    Ok(query)
}

/// Split the input into bare/quoted tokens. An unclosed quote is the one hard error.
fn tokenize(input: &str) -> Result<Vec<Token>, ()> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        // A token that begins with a quote is a whole phrase.
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut value = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '"' {
                    closed = true;
                    break;
                }
                value.push(c);
            }
            if !closed {
                return Err(());
            }
            tokens.push(Token {
                prefix: None,
                value,
                quoted: true,
            });
            continue;
        }

        // Otherwise read the bare run up to whitespace or a quote.
        let mut raw = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '"' {
                break;
            }
            raw.push(c);
            chars.next();
        }
        let (prefix, mut value) = match raw.split_once(':') {
            Some((key, rest)) => (Some(key.to_string()), rest.to_string()),
            None => (None, raw),
        };
        let mut quoted = false;
        if chars.peek() == Some(&'"') {
            chars.next();
            quoted = true;
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '"' {
                    closed = true;
                    break;
                }
                value.push(c);
            }
            if !closed {
                return Err(());
            }
        }
        tokens.push(Token {
            prefix,
            value,
            quoted,
        });
    }
    Ok(tokens)
}

/// `before:`/`after:` values: a civil date or a relative word.
fn parse_date(value: &str, now: i64) -> Option<i64> {
    if let Some(start) = relative_day(value, now) {
        return Some(start);
    }
    let normalized = value.replace('/', "-");
    let mut parts = normalized.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

/// `today`/`yesterday`/`week`/`month` → the start of the relevant day, or `None`.
fn relative_day(value: &str, now: i64) -> Option<i64> {
    let today = now.div_euclid(86_400) * 86_400;
    match value.to_ascii_lowercase().as_str() {
        "today" => Some(today),
        "yesterday" => Some(today - 86_400),
        "week" => Some(now - 7 * 86_400),
        "month" => Some(now - 30 * 86_400),
        _ => None,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = ((month + 9) % 12) as i64;
    let day_of_year = (153 * month_index + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000; // 2027-01-15T08:00:00Z

    #[test]
    fn bare_terms_and_phrases_are_free_text() {
        let query = parse("quarterly report \"almanac geometry\"", NOW);
        assert_eq!(query.terms, ["quarterly", "report"]);
        assert_eq!(query.phrases, ["almanac geometry"]);
        assert!(!query.fallback);
    }

    #[test]
    fn field_prefixes_are_typed() {
        let query = parse("from:alice@example.com to:bob subject:\"Q3 numbers\"", NOW);
        assert_eq!(query.from, ["alice@example.com"]);
        assert_eq!(query.to, ["bob"]);
        assert_eq!(query.subject, ["Q3 numbers"]);
    }

    #[test]
    fn flags_parse() {
        assert_eq!(parse("is:unread", NOW).unread, Some(true));
        assert_eq!(parse("is:read", NOW).unread, Some(false));
        assert!(parse("has:attachment", NOW).has_attachment);
        assert_eq!(parse("in:Archive", NOW).mailbox.as_deref(), Some("Archive"));
    }

    #[test]
    fn absolute_dates_parse_to_utc_midnight() {
        let query = parse("after:2026-09-01 before:2026/10/01", NOW);
        assert_eq!(query.after, Some(days_from_civil(2026, 9, 1) * 86_400));
        assert_eq!(query.before, Some(days_from_civil(2026, 10, 1) * 86_400));
    }

    #[test]
    fn relative_dates_and_two_token_words_parse() {
        let today = NOW.div_euclid(86_400) * 86_400;
        assert_eq!(parse("after:yesterday", NOW).after, Some(today - 86_400));
        assert_eq!(parse("before:today", NOW).before, Some(today));
        assert_eq!(parse("last week", NOW).after, Some(NOW - 7 * 86_400));
        assert_eq!(parse("last month", NOW).after, Some(NOW - 30 * 86_400));
        // A lone `yesterday` is a bound too.
        assert_eq!(parse("yesterday", NOW).after, Some(today - 86_400));
    }

    #[test]
    fn an_unclosed_quote_falls_back_to_one_literal_term() {
        let query = parse("almanac \"geometry", NOW);
        assert!(query.fallback);
        assert_eq!(query.terms, ["almanac \"geometry"]);
        assert_eq!(query.describe(), "almanac \"geometry");
    }

    #[test]
    fn a_bad_date_falls_back_rather_than_erroring() {
        let query = parse("before:not-a-date", NOW);
        assert!(query.fallback);
    }

    #[test]
    fn an_unknown_key_is_just_a_term() {
        let query = parse("https://example.com/x", NOW);
        assert!(!query.fallback);
        assert_eq!(query.terms, ["https://example.com/x"]);
    }

    #[test]
    fn a_query_of_only_filters_is_not_empty() {
        assert!(parse("is:unread", NOW).has_filters());
        assert!(!parse("is:unread", NOW).is_empty());
        assert!(parse("", NOW).is_empty());
    }
}
