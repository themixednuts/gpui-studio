//! HTML import and export for the design tree.
//!
//! Import uses an HTML5 parser, so anything a browser accepts (pasted markup,
//! agent output, hand-edited files) becomes nodes. `<style>` rules are inlined
//! onto matching elements; scripts and other active content are dropped.
//! Export writes clean, indented HTML with inline styles.

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};

use super::css::{parse_stylesheet, specificity};
use super::style::Style;
use super::{Document, Node, NodeId, NodeKind};

/// Elements never imported.
const DROPPED: &[&str] = &[
    "script", "style", "head", "meta", "link", "title", "template", "noscript", "iframe", "object",
    "embed", "base",
];

/// Void elements, serialized without a closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Editor-only attributes stored on disk but stripped from exported code.
const EDITOR_ATTRS: &[&str] = &["data-id", "data-name", "data-hidden", "data-locked"];

/// Import options.
#[derive(Clone, Copy, Debug)]
pub struct ImportOptions {
    /// Reuse persisted `data-id` values when they are free.
    pub keep_ids: bool,
}

/// Parse a standalone artboard file. Returns the detached root element.
///
/// When `<body>` holds exactly one element that becomes the root; otherwise
/// the body's content is wrapped in a new `div`.
pub fn parse_document(doc: &mut Document, source: &str, options: ImportOptions) -> NodeId {
    let html = Html::parse_document(source);
    let styles = collect_rules(&html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|selector| html.select(&selector).next())
        .map(|title| title.text().collect::<String>().trim().to_owned())
        .filter(|title| !title.is_empty());
    let body = Selector::parse("body")
        .ok()
        .and_then(|selector| html.select(&selector).next())
        .unwrap_or_else(|| html.root_element());
    let mut importer = Importer {
        doc,
        styles,
        options,
    };
    let roots = importer.import_children(body);
    let root = match roots.as_slice() {
        [single] if !importer.doc.get(*single).is_some_and(Node::is_text) => *single,
        _ => {
            let wrapper = importer.doc.new_element("div");
            for root in roots {
                let _ = importer.doc.attach(root, wrapper, None);
            }
            wrapper
        }
    };
    if let (Some(title), Some(node)) = (title, doc_node(importer.doc, root))
        && node.name.is_none()
    {
        node.name = Some(title);
    }
    root
}

fn doc_node(doc: &mut Document, id: NodeId) -> Option<&mut Node> {
    doc.get_mut(id)
}

/// Parse an HTML fragment into detached top-level nodes (always fresh ids
/// unless `keep_ids`).
pub fn parse_fragment(doc: &mut Document, source: &str, options: ImportOptions) -> Vec<NodeId> {
    let html = Html::parse_fragment(source);
    let styles = collect_rules(&html);
    let mut importer = Importer {
        doc,
        styles,
        options,
    };
    // A fragment is parsed inside a synthetic <html> element.
    let root = html.root_element();
    let container = if root.value().name() == "html" {
        root.children()
            .filter_map(ElementRef::wrap)
            .find(|child| child.value().name() == "body")
            .unwrap_or(root)
    } else {
        root
    };
    importer.import_children(container)
}

type RuleMap = HashMap<ego_tree::NodeId, Vec<((u16, u16, u16), usize, Vec<(String, String)>)>>;

fn collect_rules(html: &Html) -> RuleMap {
    let mut map: RuleMap = HashMap::new();
    let Ok(style_selector) = Selector::parse("style") else {
        return map;
    };
    let mut order = 0;
    for style in html.select(&style_selector) {
        let sheet = style.text().collect::<String>();
        for rule in parse_stylesheet(&sheet) {
            for selector_text in rule.selector.split(',') {
                let selector_text = selector_text.trim();
                // State rules (`:hover`) and pseudo-elements cannot be inlined.
                if selector_text.contains(":hover")
                    || selector_text.contains(":focus")
                    || selector_text.contains(":active")
                    || selector_text.contains("::")
                    || selector_text.contains(":before")
                    || selector_text.contains(":after")
                {
                    continue;
                }
                let Ok(selector) = Selector::parse(selector_text) else {
                    continue;
                };
                let rank = specificity(selector_text);
                for element in html.select(&selector) {
                    map.entry(element.id()).or_default().push((
                        rank,
                        order,
                        rule.declarations.clone(),
                    ));
                }
                order += 1;
            }
        }
    }
    map
}

struct Importer<'a> {
    doc: &'a mut Document,
    styles: RuleMap,
    options: ImportOptions,
}

impl Importer<'_> {
    fn import_children(&mut self, parent: ElementRef<'_>) -> Vec<NodeId> {
        let preformatted = matches!(parent.value().name(), "pre" | "textarea");
        let mut out = Vec::new();
        let children: Vec<_> = parent.children().collect();
        let count = children.len();
        for (index, child) in children.into_iter().enumerate() {
            match child.value() {
                scraper::Node::Text(text) => {
                    let text: &str = text;
                    let value = if preformatted {
                        text.to_owned()
                    } else {
                        let mut collapsed = collapse_whitespace(text);
                        if index == 0 {
                            collapsed = collapsed.trim_start().to_owned();
                        }
                        if index + 1 == count {
                            collapsed = collapsed.trim_end().to_owned();
                        }
                        collapsed
                    };
                    if value.trim().is_empty() && !self.between_inline(parent, index) {
                        continue;
                    }
                    if value.is_empty() {
                        continue;
                    }
                    out.push(self.doc.new_text(&value));
                }
                scraper::Node::Element(_) => {
                    if let Some(element) = ElementRef::wrap(child)
                        && let Some(id) = self.import_element(element)
                    {
                        out.push(id);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Whether whitespace at `index` separates two inline siblings.
    fn between_inline(&self, parent: ElementRef<'_>, index: usize) -> bool {
        let siblings: Vec<_> = parent.children().collect();
        let inline_at = |i: usize| {
            siblings.get(i).is_some_and(|node| match node.value() {
                scraper::Node::Element(element) => super::is_inline_tag(element.name()),
                scraper::Node::Text(text) => !text.trim().is_empty(),
                _ => false,
            })
        };
        index > 0 && inline_at(index - 1) && inline_at(index + 1)
    }

    fn import_element(&mut self, element: ElementRef<'_>) -> Option<NodeId> {
        let tag = element.value().name().to_ascii_lowercase();
        if DROPPED.contains(&tag.as_str()) {
            return None;
        }
        if tag == "body" || tag == "html" {
            // Nested wrappers from fragments: flatten into a div.
            let wrapper = self.doc.new_element("div");
            for child in self.import_children(element) {
                let _ = self.doc.attach(child, wrapper, None);
            }
            return Some(wrapper);
        }
        let persisted = element
            .value()
            .attr("data-id")
            .and_then(NodeId::parse)
            .filter(|id| self.options.keep_ids && !self.doc.contains(*id));
        let id = persisted.unwrap_or_else(|| self.doc.alloc_id());

        if tag == "svg" {
            let mut node = Node::element(id, "svg");
            node.kind = NodeKind::Svg(strip_editor_attrs_from_svg(&element.html()));
            self.apply_common(&mut node, element);
            return Some(self.doc.add(node));
        }

        let mut node = Node::element(id, &tag);
        self.apply_common(&mut node, element);
        for (name, value) in element.value().attrs() {
            let name = name.to_ascii_lowercase();
            if name == "style" || EDITOR_ATTRS.contains(&name.as_str()) || name.starts_with("on") {
                continue;
            }
            if (name == "href" || name == "src") && value.trim_start().starts_with("javascript:") {
                continue;
            }
            node.attrs.push((name, value.to_owned()));
        }
        self.doc.add(node);
        let children = if tag == "textarea" {
            let text = element.text().collect::<String>();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![self.doc.new_text(&text)]
            }
        } else {
            self.import_children(element)
        };
        for child in children {
            let _ = self.doc.attach(child, id, None);
        }
        Some(id)
    }

    fn apply_common(&self, node: &mut Node, element: ElementRef<'_>) {
        let value = element.value();
        node.name = value.attr("data-name").map(ToOwned::to_owned);
        node.hidden = value.attr("data-hidden").is_some_and(|v| v != "false");
        node.locked = value.attr("data-locked").is_some_and(|v| v != "false");
        let mut style = Style::default();
        if let Some(rules) = self.styles.get(&element.id()) {
            let mut rules = rules.clone();
            rules.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            for (_, _, declarations) in rules {
                for (property, value) in declarations {
                    // Studio always lays out border-box and every saved file
                    // declares it, so the reset is never inlined.
                    if property != "box-sizing" {
                        style.set(&property, &value);
                    }
                }
            }
        }
        if let Some(inline) = value.attr("style") {
            for (property, value) in super::style::parse_declarations(inline) {
                style.set(&property, &value);
            }
        }
        node.style = style;
    }
}

fn strip_editor_attrs_from_svg(source: &str) -> String {
    // Editor attributes and the inline style live on the node and are
    // re-emitted on the outer tag at save time.
    let Some(end) = source.find('>') else {
        return source.to_owned();
    };
    let (open, rest) = source.split_at(end);
    let mut cleaned = open.to_owned();
    for name in EDITOR_ATTRS.iter().chain(&["style"]) {
        while let Some(start) = cleaned.find(&format!(" {name}=\"")) {
            let value_start = start + name.len() + 3;
            let Some(value_len) = cleaned[value_start..].find('"') else {
                break;
            };
            cleaned.replace_range(start..value_start + value_len + 1, "");
        }
    }
    cleaned + rest
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out
}

/// Serialization options.
#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    /// Emit `data-id`/`data-name`/... so the file round-trips in Studio.
    pub editor_attrs: bool,
    /// Include hidden layers.
    pub include_hidden: bool,
}

impl ExportOptions {
    /// For files on disk.
    pub const STORAGE: Self = Self {
        editor_attrs: true,
        include_hidden: true,
    };
    /// For code handed to developers or agents.
    pub const CODE: Self = Self {
        editor_attrs: false,
        include_hidden: false,
    };
}

/// Escape text content.
#[must_use]
pub fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\u{a0}', "&nbsp;")
}

/// Escape an attribute value.
#[must_use]
pub fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Serialize a subtree as indented HTML.
#[must_use]
pub fn to_html(doc: &Document, id: NodeId, options: ExportOptions) -> String {
    let mut out = String::new();
    write_node(doc, id, options, 0, &mut out);
    out
}

/// Serialize an artboard as a complete standalone HTML document.
#[must_use]
pub fn artboard_document(doc: &Document, root: NodeId) -> String {
    let title = escape_text(&doc.display_name(root));
    let body = to_html(doc, root, ExportOptions::STORAGE);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{title}</title>\n  <style>*, *::before, *::after {{ box-sizing: border-box; }} body {{ margin: 0; }}</style>\n</head>\n<body>\n{body}</body>\n</html>\n"
    )
}

fn open_tag(doc: &Document, node: &Node, tag: &str, options: ExportOptions) -> String {
    let mut out = format!("<{tag}");
    if options.editor_attrs {
        out.push_str(&format!(" data-id=\"{}\"", node.id));
        if let Some(name) = &node.name {
            out.push_str(&format!(" data-name=\"{}\"", escape_attr(name)));
        }
        if node.hidden {
            out.push_str(" data-hidden=\"true\"");
        }
        if node.locked {
            out.push_str(" data-locked=\"true\"");
        }
    }
    for (name, value) in &node.attrs {
        if value.is_empty() {
            out.push_str(&format!(" {name}"));
        } else {
            out.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
        }
    }
    if !node.style.is_empty() {
        out.push_str(&format!(" style=\"{}\"", escape_attr(&node.style.to_css())));
    }
    let _ = doc;
    out.push('>');
    out
}

fn write_node(doc: &Document, id: NodeId, options: ExportOptions, depth: usize, out: &mut String) {
    let Some(node) = doc.get(id) else {
        return;
    };
    if node.hidden && !options.include_hidden {
        return;
    }
    let indent = "  ".repeat(depth);
    match &node.kind {
        NodeKind::Text(text) => {
            out.push_str(&indent);
            out.push_str(&escape_text(text));
            out.push('\n');
        }
        NodeKind::Svg(source) => {
            out.push_str(&indent);
            if options.editor_attrs || !node.style.is_empty() {
                // Re-insert identity and inline style on the outer tag.
                let open = open_tag(doc, node, "svg", options);
                let attrs = &open[4..open.len() - 1];
                out.push_str(&source.replacen("<svg", &format!("<svg{attrs}"), 1));
            } else {
                out.push_str(source);
            }
            out.push('\n');
        }
        NodeKind::Element { tag } => {
            out.push_str(&indent);
            out.push_str(&open_tag(doc, node, tag, options));
            if VOID.contains(&tag.as_str()) {
                out.push('\n');
                return;
            }
            if doc.is_text_layer(id) || node.children.is_empty() {
                for child in &node.children {
                    write_inline(doc, *child, options, out);
                }
            } else {
                out.push('\n');
                for child in &node.children {
                    write_node(doc, *child, options, depth + 1, out);
                }
                out.push_str(&indent);
            }
            out.push_str(&format!("</{tag}>\n"));
        }
    }
}

fn write_inline(doc: &Document, id: NodeId, options: ExportOptions, out: &mut String) {
    let Some(node) = doc.get(id) else {
        return;
    };
    if node.hidden && !options.include_hidden {
        return;
    }
    match &node.kind {
        NodeKind::Text(text) => out.push_str(&escape_text(text)),
        NodeKind::Svg(source) => out.push_str(source),
        NodeKind::Element { tag } => {
            out.push_str(&open_tag(doc, node, tag, options));
            if VOID.contains(&tag.as_str()) {
                return;
            }
            for child in &node.children {
                write_inline(doc, *child, options, out);
            }
            out.push_str(&format!("</{tag}>"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEEP: ImportOptions = ImportOptions { keep_ids: true };

    #[test]
    fn round_trips_a_document_with_ids() {
        let mut doc = Document::new();
        let source = r#"<!doctype html><html><head><title>Home</title>
<style>.card { padding: 12px; color: #333 } #hero { padding: 4px }</style></head>
<body><main data-id="n7" style="display: flex; gap: 8px">
  <h1 id="hero" class="card">Hello <b>world</b></h1>
  <img src="a.png" alt="A">
  <script>alert(1)</script>
  <button onclick="x()">Go</button>
</main></body></html>"#;
        let root = parse_document(&mut doc, source, KEEP);
        assert_eq!(root, NodeId(7));
        let node = doc.get(root).unwrap();
        assert_eq!(node.name.as_deref(), Some("Home"));
        assert_eq!(node.children.len(), 3);
        let h1 = node.children[0];
        let h1_node = doc.get(h1).unwrap();
        // The #id rule beats the class rule; inline rules beat both.
        assert_eq!(h1_node.style.computed().padding, [4.0; 4]);
        assert_eq!(h1_node.style.get("color"), Some("#333"));
        assert!(doc.is_text_layer(h1));
        assert_eq!(doc.text_content(h1).as_deref(), Some("Hello world"));
        let button = doc.get(node.children[2]).unwrap();
        assert!(button.attr("onclick").is_none());

        doc.add_artboard(0, root, 0.0, 0.0);
        let file = artboard_document(&doc, root);
        let mut reloaded = Document::new();
        let again = parse_document(&mut reloaded, &file, KEEP);
        assert_eq!(again, root);
        assert_eq!(
            to_html(&reloaded, again, ExportOptions::STORAGE),
            to_html(&doc, root, ExportOptions::STORAGE)
        );
        assert!(file.contains("<h1 data-id"));
        assert!(file.contains(">Hello <b data-id"));
    }

    #[test]
    fn fragments_get_fresh_ids_and_keep_svg() {
        let mut doc = Document::new();
        let existing = doc.new_element("div");
        let html = format!(
            r#"<div data-id="{existing}"><svg viewBox="0 0 10 10"><path d="M0 0L10 10"/></svg></div><p>Hi</p>"#
        );
        let roots = parse_fragment(&mut doc, &html, ImportOptions { keep_ids: false });
        assert_eq!(roots.len(), 2);
        assert_ne!(roots[0], existing);
        let svg = doc.children(roots[0])[0];
        assert!(matches!(&doc.get(svg).unwrap().kind, NodeKind::Svg(s) if s.contains("<path")));
        let code = to_html(&doc, roots[0], ExportOptions::CODE);
        assert!(!code.contains("data-id"));

        // A styled SVG round-trips with exactly one style attribute.
        let styled = parse_fragment(
            &mut doc,
            r#"<svg width="10" height="10" style="position: absolute; left: 4px"><path d="M0 0"/></svg>"#,
            ImportOptions { keep_ids: false },
        );
        let saved = to_html(&doc, styled[0], ExportOptions::STORAGE);
        assert_eq!(saved.matches("style=").count(), 1, "{saved}");
        let mut again = Document::new();
        let reparsed = parse_fragment(&mut again, &saved, ImportOptions { keep_ids: true });
        assert_eq!(to_html(&again, reparsed[0], ExportOptions::STORAGE), saved);
    }

    #[test]
    fn code_export_skips_hidden_layers() {
        let mut doc = Document::new();
        let roots = parse_fragment(
            &mut doc,
            r#"<div><span data-hidden="true">secret</span><span>shown</span></div>"#,
            KEEP,
        );
        let code = to_html(&doc, roots[0], ExportOptions::CODE);
        assert!(!code.contains("secret"));
        assert!(code.contains("shown"));
    }
}
