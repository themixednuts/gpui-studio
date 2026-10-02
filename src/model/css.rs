//! Minimal stylesheet parsing used to inline `<style>` rules on import.
//!
//! Designs store every style inline, like Paper. HTML pasted from elsewhere or
//! written by an agent often carries a `<style>` block instead; its plain style
//! rules are resolved onto the matching elements once, at import time.

use super::style::parse_declarations;

/// One qualified style rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Selector list text, e.g. `.card, #hero h1`.
    pub selector: String,
    /// Declarations in order.
    pub declarations: Vec<(String, String)>,
}

/// Parse top-level style rules, skipping at-rules (and their blocks) and comments.
#[must_use]
pub fn parse_stylesheet(source: &str) -> Vec<Rule> {
    let source = strip_comments(source);
    let mut rules = Vec::new();
    let mut rest = source.as_str();
    while let Some(open) = rest.find('{') {
        let prelude = rest[..open].trim();
        let Some(close) = matching_brace(rest, open) else {
            break;
        };
        let body = &rest[open + 1..close];
        if prelude.starts_with('@') {
            // `@media` and similar wrap nested rules that depend on the viewport;
            // only `@media screen` style blocks are applied, unconditionally.
            if prelude.starts_with("@media") && !prelude.contains("max-width") {
                rules.extend(parse_stylesheet(body));
            }
        } else if !prelude.is_empty() {
            rules.push(Rule {
                selector: prelude.to_owned(),
                declarations: parse_declarations(body),
            });
        }
        rest = &rest[close + 1..];
    }
    rules
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Approximate CSS specificity `(ids, classes/attributes/pseudo-classes, types)`.
#[must_use]
pub fn specificity(selector: &str) -> (u16, u16, u16) {
    let mut ids = 0;
    let mut classes = 0;
    let mut types = 0;
    for compound in selector.split([' ', '>', '+', '~']) {
        let compound = compound.trim();
        if compound.is_empty() || compound == "*" {
            continue;
        }
        let mut chars = compound.chars().peekable();
        let mut starts_with_type = compound
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        while let Some(c) = chars.next() {
            match c {
                '#' => ids += 1,
                '.' | '[' => classes += 1,
                ':' => {
                    if chars.peek() == Some(&':') {
                        chars.next();
                        types += 1;
                    } else {
                        classes += 1;
                    }
                }
                _ => {}
            }
        }
        if std::mem::take(&mut starts_with_type) {
            types += 1;
        }
    }
    (ids, classes, types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_and_skips_at_rules() {
        let rules = parse_stylesheet(
            "/* x */ .card { padding: 8px; color: red }\n@font-face { font-family: X }\n@media (max-width: 600px) { .card { padding: 2px } }\n#a, p { margin: 0 }",
        );
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].selector, ".card");
        assert_eq!(
            rules[0].declarations[1],
            ("color".to_owned(), "red".to_owned())
        );
        assert_eq!(rules[1].selector, "#a, p");
    }

    #[test]
    fn specificity_orders_selectors() {
        assert!(specificity("#a") > specificity(".a.b"));
        assert!(specificity(".a") > specificity("div p"));
        assert_eq!(specificity("div.card:hover"), (0, 2, 1));
    }
}
