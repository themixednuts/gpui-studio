//! Design variables: project-wide CSS custom properties.
//!
//! Variables are written into every artboard file as a `:root { --name: … }`
//! rule, so layers that use `var(--name)` render the same in a browser as on
//! the canvas. The canvas resolves `var()` (with fallbacks and nesting) before
//! parsing a declaration.

use std::collections::BTreeMap;

/// Name (without the leading `--`) → CSS value.
pub type Variables = BTreeMap<String, String>;

const MAX_DEPTH: usize = 8;

/// Normalize a variable name: lowercase letters, digits, `-` and `_`, with
/// any leading `--` removed. Returns `None` when nothing valid remains.
#[must_use]
pub fn normalize_name(name: &str) -> Option<String> {
    let name = name.trim().trim_start_matches("--");
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch.is_whitespace() || ch == '/' || ch == '.')
            && !out.is_empty()
            && !out.ends_with('-')
        {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    (!out.is_empty()).then_some(out)
}

/// What kind of value a variable holds, for grouping and pickers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableKind {
    /// A color.
    Color,
    /// A length or number.
    Number,
    /// Anything else (font stacks, shadows, ...).
    Other,
}

/// Classify a variable value.
#[must_use]
pub fn kind_of(value: &str, vars: &Variables) -> VariableKind {
    let resolved = resolve(value, vars).unwrap_or_else(|| value.to_owned());
    let resolved = resolved.trim();
    if super::Color::parse_loose(resolved).is_some() {
        VariableKind::Color
    } else if resolved
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%')
        .parse::<f32>()
        .is_ok()
    {
        VariableKind::Number
    } else {
        VariableKind::Other
    }
}

/// Substitute every `var(--name[, fallback])` in a value. Returns `None` when
/// a reference cannot be resolved (CSS treats the declaration as invalid).
#[must_use]
pub fn resolve(value: &str, vars: &Variables) -> Option<String> {
    resolve_depth(value, vars, 0)
}

fn resolve_depth(value: &str, vars: &Variables, depth: usize) -> Option<String> {
    if !value.contains("var(") {
        return Some(value.to_owned());
    }
    if depth >= MAX_DEPTH {
        return None;
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let inner_start = start + 4;
        // Find the matching close paren.
        let mut depth_paren = 1;
        let mut end = None;
        for (i, ch) in rest[inner_start..].char_indices() {
            match ch {
                '(' => depth_paren += 1,
                ')' => {
                    depth_paren -= 1;
                    if depth_paren == 0 {
                        end = Some(inner_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        let inner = &rest[inner_start..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (inner.trim(), None),
        };
        let name = name.strip_prefix("--")?;
        let replacement = match vars.get(name) {
            Some(value) => resolve_depth(value, vars, depth + 1),
            None => None,
        }
        .or_else(|| fallback.and_then(|f| resolve_depth(f, vars, depth + 1)))?;
        out.push_str(&replacement);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The variable a value refers to when it is exactly `var(--name)`.
#[must_use]
pub fn reference(value: &str) -> Option<&str> {
    let inner = value.trim().strip_prefix("var(")?.strip_suffix(')')?;
    let name = inner.split(',').next()?.trim().strip_prefix("--")?;
    (!name.is_empty()).then_some(name)
}

/// Rewrite `var(--from…)` references to `var(--to…)`.
#[must_use]
pub fn rename_references(value: &str, from: &str, to: &str) -> String {
    let needle = format!("var(--{from}");
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(&needle) {
        let after = &rest[start + needle.len()..];
        // Only whole names: the next character must end the name.
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| c == ')' || c == ',' || c.is_whitespace());
        out.push_str(&rest[..start]);
        if boundary {
            out.push_str(&format!("var(--{to}"));
        } else {
            out.push_str(&needle);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Replace every `var(--name…)` reference with `replacement` (detaching it).
#[must_use]
pub fn inline_references(value: &str, name: &str, replacement: &str) -> String {
    let needle = format!("var(--{name}");
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(&needle) {
        let after = &rest[start + needle.len()..];
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| c == ')' || c == ',' || c.is_whitespace());
        out.push_str(&rest[..start]);
        if !boundary {
            out.push_str(&needle);
            rest = after;
            continue;
        }
        // Skip to the matching close paren of this var().
        let mut depth = 1;
        let mut end = None;
        for (i, ch) in after.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(end) => {
                out.push_str(replacement);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&needle);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `:root { … }` rule declaring every variable.
#[must_use]
pub fn root_rule(vars: &Variables) -> Option<String> {
    if vars.is_empty() {
        return None;
    }
    let body: Vec<String> = vars
        .iter()
        .map(|(name, value)| format!("  --{name}: {value};"))
        .collect();
    Some(format!(":root {{\n{}\n}}", body.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Variables {
        let mut v = Variables::new();
        v.insert("brand".into(), "#ff5a36".into());
        v.insert("space-2".into(), "8px".into());
        v.insert("accent".into(), "var(--brand)".into());
        v.insert("loop".into(), "var(--loop)".into());
        v
    }

    #[test]
    fn resolves_nested_fallbacks_and_cycles() {
        let v = vars();
        assert_eq!(resolve("var(--brand)", &v).as_deref(), Some("#ff5a36"));
        assert_eq!(resolve("var(--accent)", &v).as_deref(), Some("#ff5a36"));
        assert_eq!(
            resolve("var(--space-2) calc(var(--space-2) * 2)", &v).as_deref(),
            Some("8px calc(8px * 2)")
        );
        assert_eq!(resolve("var(--missing, #000)", &v).as_deref(), Some("#000"));
        assert_eq!(
            resolve("var(--missing, var(--brand))", &v).as_deref(),
            Some("#ff5a36")
        );
        assert_eq!(resolve("var(--missing)", &v), None);
        assert_eq!(resolve("var(--loop)", &v), None);
        assert_eq!(resolve("12px", &v).as_deref(), Some("12px"));
    }

    #[test]
    fn names_kinds_references_and_renames() {
        assert_eq!(
            normalize_name("--Brand Primary").as_deref(),
            Some("brand-primary")
        );
        assert_eq!(
            normalize_name("color/text.muted").as_deref(),
            Some("color-text-muted")
        );
        assert_eq!(normalize_name("--"), None);
        let v = vars();
        assert_eq!(kind_of("#fff", &v), VariableKind::Color);
        assert_eq!(kind_of("var(--accent)", &v), VariableKind::Color);
        assert_eq!(kind_of("12px", &v), VariableKind::Number);
        assert_eq!(kind_of("Geist, sans-serif", &v), VariableKind::Other);
        assert_eq!(reference(" var(--brand) "), Some("brand"));
        assert_eq!(reference("var(--brand, red)"), Some("brand"));
        assert_eq!(reference("1px solid var(--brand)"), None);
        assert_eq!(
            rename_references(
                "var(--brand) var(--brand-2) var(--brand, red)",
                "brand",
                "primary"
            ),
            "var(--primary) var(--brand-2) var(--primary, red)"
        );
        assert_eq!(
            inline_references(
                "1px solid var(--brand, red) var(--brand-2)",
                "brand",
                "#000"
            ),
            "1px solid #000 var(--brand-2)"
        );
        assert!(root_rule(&v).unwrap().contains("  --brand: #ff5a36;"));
        assert_eq!(root_rule(&Variables::new()), None);
    }
}
