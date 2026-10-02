//! Code export: HTML with inline styles, HTML with a class stylesheet, and a
//! GPUI builder chain for native Rust apps.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::model::html::{ExportOptions, escape_attr, escape_text, to_html};
use crate::model::style::{
    Align, Computed, Direction, Display, Length, LineHeight, Overflow, Position, TextAlign, fmt_num,
};
use crate::model::{Color, Document, NodeId, NodeKind};

/// Export targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeFormat {
    /// HTML with inline `style` attributes.
    Html,
    /// HTML with classes plus a stylesheet.
    HtmlCss,
    /// GPUI Rust builder code.
    Gpui,
}

impl CodeFormat {
    /// All formats in UI order.
    pub const ALL: [Self; 3] = [Self::Html, Self::HtmlCss, Self::Gpui];

    /// Tab label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::HtmlCss => "HTML + CSS",
            Self::Gpui => "GPUI",
        }
    }

    /// Parse an MCP name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "html" => Some(Self::Html),
            "html_css" | "css" => Some(Self::HtmlCss),
            "gpui" | "rust" => Some(Self::Gpui),
            _ => None,
        }
    }
}

/// Export a subtree.
#[must_use]
pub fn export(doc: &Document, id: NodeId, format: CodeFormat) -> String {
    match format {
        CodeFormat::Html => {
            let html = to_html(doc, id, ExportOptions::CODE);
            match crate::model::variables::root_rule(&used_variables(doc, id)) {
                Some(rule) => format!("<style>\n{rule}\n</style>\n\n{html}"),
                None => html,
            }
        }
        CodeFormat::HtmlCss => html_with_classes(doc, id),
        CodeFormat::Gpui => gpui(doc, id),
    }
}

/// Variables a subtree references, including the variables those refer to.
fn used_variables(doc: &Document, id: NodeId) -> crate::model::Variables {
    let mut pending: Vec<String> = Vec::new();
    let collect = |value: &str, pending: &mut Vec<String>| {
        let mut rest = value;
        while let Some(start) = rest.find("var(--") {
            let after = &rest[start + 6..];
            let end = after
                .find(|c: char| c == ')' || c == ',' || c.is_whitespace())
                .unwrap_or(after.len());
            pending.push(after[..end].to_owned());
            rest = &after[end..];
        }
    };
    for node in std::iter::once(id).chain(doc.descendants(id)) {
        if let Some(node) = doc.get(node) {
            for (_, value) in node.style.iter() {
                collect(value, &mut pending);
            }
        }
    }
    let mut used = crate::model::Variables::new();
    while let Some(name) = pending.pop() {
        if used.contains_key(&name) {
            continue;
        }
        if let Some(value) = doc.variables.get(&name) {
            collect(value, &mut pending);
            used.insert(name, value.clone());
        }
    }
    used
}

fn class_slug(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() || slug.starts_with(|c: char| c.is_ascii_digit()) {
        format!("layer-{slug}")
    } else {
        slug
    }
}

fn html_with_classes(doc: &Document, id: NodeId) -> String {
    let mut used = BTreeSet::new();
    let mut css = String::new();
    let mut html = String::new();
    let mut classes = std::collections::HashMap::new();
    write_classed(doc, id, 0, &mut used, &mut classes, &mut css, &mut html);
    css.push_str(&media_queries(doc, id, &classes));
    let root = crate::model::variables::root_rule(&used_variables(doc, id))
        .map(|rule| format!("{rule}\n"))
        .unwrap_or_default();
    format!(
        "<style>\n*, *::before, *::after {{ box-sizing: border-box; }}\n{root}{css}</style>\n\n{html}"
    )
}

/// For an artboard with breakpoints, the CSS that turns its breakpoint
/// overrides into `@media (max-width: …)` rules, widest first. Text and
/// structure differences can't be expressed in CSS and are skipped.
fn media_queries(
    doc: &Document,
    main: NodeId,
    classes: &std::collections::HashMap<NodeId, String>,
) -> String {
    use crate::editor::BREAKPOINT_ATTR;
    use crate::model::components::REF_ATTR;
    let mut breakpoints: Vec<(NodeId, f32)> = doc
        .instances_of(main)
        .into_iter()
        .filter_map(|id| Some((id, doc.get(id)?.attr(BREAKPOINT_ATTR)?.parse().ok()?)))
        .collect();
    if breakpoints.is_empty() {
        return String::new();
    }
    breakpoints.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut css = String::new();
    // The page fills the viewport instead of the desktop artboard's width.
    if let Some(class) = classes.get(&main) {
        let _ = writeln!(css, ".{class} {{ width: auto; max-width: 100%; }}");
    }
    for (instance, width) in breakpoints {
        let mut rules = String::new();
        for id in std::iter::once(instance).chain(doc.descendants(instance)) {
            let Some(node) = doc.get(id) else {
                continue;
            };
            let source = if id == instance {
                Some(main)
            } else {
                node.attr(REF_ATTR).and_then(NodeId::parse)
            };
            let (Some(source), true) = (source, !node.is_text()) else {
                continue;
            };
            let (Some(class), Some(original)) = (classes.get(&source), doc.get(source)) else {
                continue;
            };
            let mut decls = Vec::new();
            if node.hidden && !original.hidden {
                decls.push(("display".to_owned(), "none".to_owned()));
            }
            for (property, value) in node.style.iter() {
                if id == instance && matches!(property, "width" | "height") {
                    continue;
                }
                if original.style.get(property) != Some(value) {
                    decls.push((property.to_owned(), value.to_owned()));
                }
            }
            for (property, _) in original.style.iter() {
                if node.style.get(property).is_none()
                    && !(id == instance && matches!(property, "width" | "height"))
                {
                    decls.push((property.to_owned(), "initial".to_owned()));
                }
            }
            if decls.is_empty() {
                continue;
            }
            let _ = writeln!(rules, "  .{class} {{");
            for (property, value) in decls {
                let _ = writeln!(rules, "    {property}: {value};");
            }
            rules.push_str("  }\n");
        }
        if !rules.is_empty() {
            let _ = writeln!(
                css,
                "@media (max-width: {}px) {{\n{rules}}}",
                crate::model::style::fmt_num(width)
            );
        }
    }
    css
}

fn write_classed(
    doc: &Document,
    id: NodeId,
    depth: usize,
    used: &mut BTreeSet<String>,
    classes: &mut std::collections::HashMap<NodeId, String>,
    css: &mut String,
    out: &mut String,
) {
    let Some(node) = doc.get(id) else {
        return;
    };
    if node.hidden {
        return;
    }
    let indent = "  ".repeat(depth);
    match &node.kind {
        NodeKind::Text(text) => {
            let _ = writeln!(out, "{indent}{}", escape_text(text));
        }
        NodeKind::Svg(source) => {
            let _ = writeln!(out, "{indent}{source}");
        }
        NodeKind::Element { tag } => {
            let mut open = format!("<{tag}");
            if !node.style.is_empty() {
                let base = class_slug(&doc.display_name(id));
                let mut class = base.clone();
                let mut n = 2;
                while !used.insert(class.clone()) {
                    class = format!("{base}-{n}");
                    n += 1;
                }
                classes.insert(id, class.clone());
                let _ = writeln!(css, ".{class} {{");
                for (property, value) in node.style.iter() {
                    let _ = writeln!(css, "  {property}: {value};");
                }
                css.push_str("}\n");
                let existing = node
                    .attr("class")
                    .map(|c| format!("{c} "))
                    .unwrap_or_default();
                let _ = write!(open, " class=\"{}{class}\"", escape_attr(&existing));
            }
            for (name, value) in &node.attrs {
                let editor_only = crate::model::components::COMPONENT_ATTRS
                    .contains(&name.as_str())
                    || crate::model::prototype::PROTOTYPE_ATTRS.contains(&name.as_str())
                    || name == crate::editor::BREAKPOINT_ATTR;
                if name != "class" && !editor_only {
                    let _ = write!(open, " {name}=\"{}\"", escape_attr(value));
                }
            }
            open.push('>');
            if matches!(tag.as_str(), "img" | "br" | "hr" | "input") {
                let _ = writeln!(out, "{indent}{open}");
                return;
            }
            if doc.is_text_layer(id) {
                let inner = to_html(doc, id, ExportOptions::CODE);
                // Reuse the inline serializer for mixed text, swapping the opening tag.
                let inner = inner
                    .split_once('>')
                    .map_or(String::new(), |(_, rest)| rest.trim_end().to_owned());
                let _ = writeln!(out, "{indent}{open}{inner}");
                return;
            }
            let _ = writeln!(out, "{indent}{open}");
            for child in &node.children {
                write_classed(doc, *child, depth + 1, used, classes, css, out);
            }
            let _ = writeln!(out, "{indent}</{tag}>");
        }
    }
}

fn gpui_color(color: Color) -> String {
    if color.a == 255 {
        format!("rgb(0x{:02x}{:02x}{:02x})", color.r, color.g, color.b)
    } else {
        format!("rgba(0x{:08x})", color.to_u32())
    }
}

fn gpui_len(length: Length) -> Option<String> {
    match length {
        Length::Px(value) => Some(px_literal(value)),
        Length::Percent(value) => Some(format!("relative({})", fmt_num(value / 100.0))),
        Length::Auto | Length::Fit => None,
    }
}

fn px_literal(value: f32) -> String {
    let text = fmt_num(value);
    if text.contains('.') {
        format!("px({text})")
    } else {
        format!("px({text}.)")
    }
}

fn gpui_style_calls(c: &Computed) -> Vec<String> {
    let mut calls = Vec::new();
    match c.display {
        Display::Flex => {
            calls.push("flex()".to_owned());
            calls.push(
                match c.direction {
                    Direction::Row => "flex_row()",
                    Direction::Column => "flex_col()",
                    Direction::RowReverse => "flex_row_reverse()",
                    Direction::ColumnReverse => "flex_col_reverse()",
                }
                .to_owned(),
            );
            if c.wrap {
                calls.push("flex_wrap()".to_owned());
            }
        }
        Display::Grid => {
            calls.push("grid()".to_owned());
            if let Some(columns) = c.grid_columns {
                calls.push(format!("grid_cols({columns})"));
            }
            if let Some(rows) = c.grid_rows {
                calls.push(format!("grid_rows({rows})"));
            }
        }
        Display::Block | Display::Inline | Display::None => {}
    }
    if c.display == Display::Block {
        // Block flow stacks children; express it as a column flexbox.
        calls.push("flex()".to_owned());
        calls.push("flex_col()".to_owned());
    }
    let align = |a: Align, prefix: &str| -> Option<String> {
        let suffix = match a {
            Align::Start => "start",
            Align::Center => "center",
            Align::End => "end",
            Align::Stretch => "stretch",
            Align::Baseline => "baseline",
            Align::SpaceBetween => "between",
            Align::SpaceAround => "around",
            Align::SpaceEvenly => "evenly",
            Align::Normal => return None,
        };
        Some(format!("{prefix}_{suffix}()"))
    };
    calls.extend(align(c.align_items, "items"));
    calls.extend(align(c.justify, "justify"));
    if c.row_gap > 0.0 && c.row_gap == c.column_gap {
        calls.push(format!("gap({})", px_literal(c.row_gap)));
    } else {
        if c.row_gap > 0.0 {
            calls.push(format!("gap_y({})", px_literal(c.row_gap)));
        }
        if c.column_gap > 0.0 {
            calls.push(format!("gap_x({})", px_literal(c.column_gap)));
        }
    }
    let [top, right, bottom, left] = c.padding;
    if let Some(all) = c.uniform_padding().filter(|p| *p > 0.0) {
        calls.push(format!("p({})", px_literal(all)));
    } else {
        for (value, name) in [(top, "pt"), (right, "pr"), (bottom, "pb"), (left, "pl")] {
            if value > 0.0 {
                calls.push(format!("{name}({})", px_literal(value)));
            }
        }
    }
    for (length, name) in [
        (c.margin[0], "mt"),
        (c.margin[1], "mr"),
        (c.margin[2], "mb"),
        (c.margin[3], "ml"),
    ] {
        if let Some(value) = length.px().filter(|v| *v != 0.0) {
            calls.push(format!("{name}({})", px_literal(value)));
        }
    }
    for (length, name) in [
        (c.width, "w"),
        (c.height, "h"),
        (c.min_width, "min_w"),
        (c.min_height, "min_h"),
        (c.max_width, "max_w"),
        (c.max_height, "max_h"),
    ] {
        if let Some(value) = gpui_len(length) {
            calls.push(format!("{name}({value})"));
        }
    }
    if c.grow > 0.0 {
        calls.push("flex_grow()".to_owned());
    }
    if c.shrink == 0.0 {
        calls.push("flex_shrink_0()".to_owned());
    }
    if let Some(basis) = gpui_len(c.basis) {
        calls.push(format!("flex_basis({basis})"));
    }
    if c.position == Position::Absolute {
        calls.push("absolute()".to_owned());
    } else if c.position == Position::Relative {
        calls.push("relative()".to_owned());
    }
    for (length, name) in [
        (c.inset[0], "top"),
        (c.inset[1], "right"),
        (c.inset[2], "bottom"),
        (c.inset[3], "left"),
    ] {
        if let Some(value) = gpui_len(length) {
            calls.push(format!("{name}({value})"));
        }
    }
    if let Some(background) = c.background.filter(|c| c.a > 0) {
        calls.push(format!("bg({})", gpui_color(background)));
    }
    let [bt, br, bb, bl] = c.border_width;
    if bt > 0.0 || br > 0.0 || bb > 0.0 || bl > 0.0 {
        if bt == br && br == bb && bb == bl {
            calls.push(format!("border({})", px_literal(bt)));
        } else {
            for (value, name) in [
                (bt, "border_t"),
                (br, "border_r"),
                (bb, "border_b"),
                (bl, "border_l"),
            ] {
                if value > 0.0 {
                    calls.push(format!("{name}({})", px_literal(value)));
                }
            }
        }
        if let Some(color) = c.border_color {
            calls.push(format!("border_color({})", gpui_color(color)));
        }
        if c.border_style.as_deref() == Some("dashed") {
            calls.push("border_dashed()".to_owned());
        }
    }
    let [tl, tr, brr, bll] = c.resolved_radii(c.width.px(), c.height.px());
    let full = [tl, tr, brr, bll].iter().all(|r| *r >= 10_000.0);
    if full {
        calls.push("rounded_full()".to_owned());
    }
    if !full && (tl > 0.0 || tr > 0.0 || brr > 0.0 || bll > 0.0) {
        if tl == tr && tr == brr && brr == bll {
            calls.push(format!("rounded({})", px_literal(tl)));
        } else {
            for (value, name) in [
                (tl, "rounded_tl"),
                (tr, "rounded_tr"),
                (brr, "rounded_br"),
                (bll, "rounded_bl"),
            ] {
                if value > 0.0 {
                    calls.push(format!("{name}({})", px_literal(value)));
                }
            }
        }
    }
    if c.opacity < 1.0 {
        calls.push(format!("opacity({})", fmt_num(c.opacity)));
    }
    match c.overflow {
        Overflow::Hidden => calls.push("overflow_hidden()".to_owned()),
        Overflow::Scroll => calls.push("overflow_y_scroll()".to_owned()),
        Overflow::Visible => {}
    }
    for shadow in c.shadows.iter().filter(|s| !s.inset) {
        calls.push(format!(
            "shadow(vec![BoxShadow {{ color: {}.into(), offset: point({}, {}), blur_radius: {}, spread_radius: {}, inset: false }}])",
            gpui_color(shadow.color),
            px_literal(shadow.x),
            px_literal(shadow.y),
            px_literal(shadow.blur / 2.0),
            px_literal(shadow.spread),
        ));
    }
    if let Some(color) = c.color {
        calls.push(format!("text_color({})", gpui_color(color)));
    }
    if let Some(family) = &c.font_family {
        let first = family
            .split(',')
            .next()
            .unwrap_or(family)
            .trim()
            .trim_matches(['"', '\'']);
        calls.push(format!("font_family({first:?})"));
    }
    if let Some(size) = c.font_size {
        calls.push(format!("text_size({})", px_literal(size)));
    }
    if let Some(weight) = c.font_weight {
        calls.push(format!("font_weight(FontWeight({weight}.))"));
    }
    if c.italic == Some(true) {
        calls.push("italic()".to_owned());
    }
    match c.line_height {
        Some(LineHeight::Px(value)) => calls.push(format!("line_height({})", px_literal(value))),
        Some(LineHeight::Relative(value)) => {
            calls.push(format!("line_height(relative({}))", fmt_num(value)));
        }
        None => {}
    }
    match c.text_align {
        TextAlign::Center => calls.push("text_center()".to_owned()),
        TextAlign::Right => calls.push("text_right()".to_owned()),
        TextAlign::Left | TextAlign::Inherit => {}
    }
    if c.underline == Some(true) {
        calls.push("underline()".to_owned());
    }
    if c.strikethrough == Some(true) {
        calls.push("line_through()".to_owned());
    }
    if c.nowrap == Some(true) {
        calls.push("whitespace_nowrap()".to_owned());
    }
    calls
}

fn gpui(doc: &Document, id: NodeId) -> String {
    let mut out = String::from(
        "use gpui::{BoxShadow, FontWeight, div, point, prelude::*, px, relative, rgb, rgba};\n\n",
    );
    out.push_str("fn render() -> impl IntoElement {\n    ");
    write_gpui(doc, id, 1, &mut out);
    out.push_str("\n}\n");
    out
}

fn write_gpui(doc: &Document, id: NodeId, depth: usize, out: &mut String) {
    let Some(node) = doc.get(id) else {
        return;
    };
    let indent = "    ".repeat(depth + 1);
    match &node.kind {
        NodeKind::Text(text) => {
            let _ = write!(out, "{:?}", text);
        }
        NodeKind::Svg(_) => {
            out.push_str("svg().path(\"icons/vector.svg\") /* inline SVG: export the asset */");
        }
        NodeKind::Element { tag } => {
            if tag == "img" {
                let source = node.attr("src").unwrap_or("image.png");
                let _ = write!(out, "img({source:?})");
            } else {
                out.push_str("div()");
            }
            if let Some(name) = &node.name {
                let _ = write!(out, "\n{indent}.id({:?})", class_slug(name));
            }
            for call in gpui_style_calls(&doc.computed(node)) {
                let _ = write!(out, "\n{indent}.{call}");
            }
            if doc.is_text_layer(id) {
                if let Some(text) = doc.text_content(id) {
                    let _ = write!(out, "\n{indent}.child({text:?})");
                }
                return;
            }
            for child in &node.children {
                if doc.get(*child).is_some_and(|c| c.hidden) {
                    continue;
                }
                let _ = write!(out, "\n{indent}.child(\n{indent}    ");
                write_gpui(doc, *child, depth + 1, out);
                let _ = write!(out, ",\n{indent})");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::html::{ImportOptions, parse_fragment};

    fn sample() -> (Document, NodeId) {
        let mut doc = Document::new();
        let roots = parse_fragment(
            &mut doc,
            r#"<div data-name="Hero Card" style="display: flex; flex-direction: column; gap: 8px; padding: 16px; background-color: #fff; border-radius: 12px"><h1 style="font-size: 24px; font-weight: 700">Hello</h1></div>"#,
            ImportOptions { keep_ids: false },
        );
        (doc, roots[0])
    }

    #[test]
    fn gpui_export_maps_layout_and_paint() {
        let (doc, root) = sample();
        let code = export(&doc, root, CodeFormat::Gpui);
        for expected in [
            ".flex()",
            ".flex_col()",
            ".gap(px(8.))",
            ".p(px(16.))",
            ".bg(rgb(0xffffff))",
            ".rounded(px(12.))",
            ".text_size(px(24.))",
            ".child(\"Hello\")",
            ".id(\"hero-card\")",
        ] {
            assert!(code.contains(expected), "missing {expected} in\n{code}");
        }
    }

    #[test]
    fn class_export_moves_inline_styles_to_rules() {
        let (doc, root) = sample();
        let code = export(&doc, root, CodeFormat::HtmlCss);
        assert!(code.contains(".hero-card {\n  display: flex;"));
        assert!(code.contains("<div class=\"hero-card\">"));
        assert!(!code.contains("style=\""));
        assert!(code.contains(">Hello</h1>"));
    }

    #[test]
    fn breakpoint_overrides_become_media_queries() {
        use crate::editor::{Editor, InsertTarget};
        let mut doc = Document::new();
        let board = doc.create_artboard(0, "Landing", (0.0, 0.0), (1280.0, 800.0));
        let mut editor = Editor::new(doc);
        let ids = editor
            .insert_html(
                "<h1 data-name=\"Title\" style=\"font-size: 64px; color: #111111\">Hi</h1>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let mobile = editor.add_breakpoint(board, 390.0).unwrap();
        let title = editor.doc.children(mobile)[0];
        editor
            .set_styles(&[title], &[("font-size".into(), Some("32px".into()))], None)
            .unwrap();
        let css = export(&editor.doc, board, CodeFormat::HtmlCss);
        assert!(css.contains("@media (max-width: 390px) {"), "{css}");
        assert!(css.contains("    font-size: 32px;"), "{css}");
        assert!(
            !css.contains("    color:"),
            "unchanged properties are not repeated: {css}"
        );
        assert!(!css.contains("data-component") && !css.contains("data-breakpoint"));
        let _ = ids;
    }
}
