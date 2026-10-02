//! The design document: pages of artboards whose contents are HTML elements
//! with inline CSS.
//!
//! Nodes live in an arena keyed by [`NodeId`]. An artboard is a top-level
//! element placed on the infinite canvas; everything inside it is ordinary
//! HTML. The same tree is rendered natively, edited by the design panel,
//! written back to disk as standalone HTML files, and exposed to MCP agents.

pub mod color;
pub mod components;
pub mod connection;
pub mod css;
pub mod grid;
pub mod html;
pub mod style;
pub mod variables;

use std::collections::BTreeMap;
use std::fmt;

pub use color::Color;
pub use connection::{ArrowHeads, Connection, ConnectorStyle, Endpoint};
pub use style::{Computed, Length, Style};
pub use variables::Variables;

/// Stable identity of one node, persisted as `data-id="n42"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "n{}", self.0)
    }
}

impl serde::Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid node id {text:?}")))
    }
}

impl NodeId {
    /// Parse `n42` (or a bare `42`).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        value
            .strip_prefix('n')
            .unwrap_or(value)
            .parse()
            .ok()
            .map(Self)
    }
}

/// What a node holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// An HTML element with children.
    Element {
        /// Lowercase tag name.
        tag: String,
    },
    /// A run of text.
    Text(String),
    /// An opaque `<svg>` subtree kept verbatim.
    Svg(String),
}

/// One node of the design tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Identity.
    pub id: NodeId,
    /// Parent element; `None` for artboard roots and detached nodes.
    pub parent: Option<NodeId>,
    /// Payload.
    pub kind: NodeKind,
    /// Layer name shown in the layers panel (`data-name`).
    pub name: Option<String>,
    /// Other HTML attributes in source order (excluding `style` and editor data).
    pub attrs: Vec<(String, String)>,
    /// Inline CSS.
    pub style: Style,
    /// Child nodes in order.
    pub children: Vec<NodeId>,
    /// Hidden in the editor and in exports (`data-hidden`).
    pub hidden: bool,
    /// Locked against canvas selection (`data-locked`).
    pub locked: bool,
}

impl Node {
    /// New detached element.
    #[must_use]
    pub fn element(id: NodeId, tag: &str) -> Self {
        Self {
            id,
            parent: None,
            kind: NodeKind::Element {
                tag: tag.to_ascii_lowercase(),
            },
            name: None,
            attrs: Vec::new(),
            style: Style::default(),
            children: Vec::new(),
            hidden: false,
            locked: false,
        }
    }

    /// New detached text run.
    #[must_use]
    pub fn text(id: NodeId, text: &str) -> Self {
        Self {
            kind: NodeKind::Text(text.to_owned()),
            ..Self::element(id, "#text")
        }
    }

    /// Element tag, `#text`, or `svg`.
    #[must_use]
    pub fn tag(&self) -> &str {
        match &self.kind {
            NodeKind::Element { tag } => tag,
            NodeKind::Text(_) => "#text",
            NodeKind::Svg(_) => "svg",
        }
    }

    /// Whether this is a text run.
    #[must_use]
    pub fn is_text(&self) -> bool {
        matches!(self.kind, NodeKind::Text(_))
    }

    /// Attribute value.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Set or remove (empty value removes) an attribute.
    pub fn set_attr(&mut self, name: &str, value: &str) {
        let name = name.trim().to_ascii_lowercase();
        if value.is_empty() {
            self.attrs.retain(|(n, _)| *n != name);
        } else if let Some(slot) = self.attrs.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = value.to_owned();
        } else {
            self.attrs.push((name, value.to_owned()));
        }
    }
}

/// Tags whose content flows inline inside a text block.
#[must_use]
pub fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "#text"
            | "span"
            | "a"
            | "b"
            | "strong"
            | "i"
            | "em"
            | "u"
            | "s"
            | "del"
            | "ins"
            | "mark"
            | "small"
            | "code"
            | "kbd"
            | "sub"
            | "sup"
            | "abbr"
            | "time"
            | "br"
            | "label"
            | "q"
            | "cite"
    )
}

/// One artboard placed on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Artboard {
    /// Root element.
    pub root: NodeId,
    /// File name inside the project's `artboards/` directory.
    pub file: String,
    /// Canvas X in document pixels.
    pub x: f32,
    /// Canvas Y in document pixels.
    pub y: f32,
}

/// A page groups artboards on one canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// Display name.
    pub name: String,
    /// Artboards in z-order (last is front-most).
    pub artboards: Vec<Artboard>,
    /// Connectors drawn on this page's canvas.
    pub connections: Vec<Connection>,
}

impl Page {
    /// An empty page.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            artboards: Vec::new(),
            connections: Vec::new(),
        }
    }
}

/// Errors from structural edits.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    /// The node does not exist.
    #[error("node {0} does not exist")]
    Missing(NodeId),
    /// The target cannot hold children.
    #[error("node {0} cannot contain children")]
    NotContainer(NodeId),
    /// The move would place a node inside itself.
    #[error("cannot move {0} inside itself")]
    Cycle(NodeId),
    /// Artboard roots are managed by page operations.
    #[error("{0} is an artboard; use artboard operations")]
    Artboard(NodeId),
}

/// The complete multi-page design document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    nodes: BTreeMap<NodeId, Node>,
    /// Pages in order.
    pub pages: Vec<Page>,
    /// Design variables (CSS custom properties on `:root`).
    pub variables: Variables,
    next_id: u64,
    next_connection: u64,
}

impl Document {
    /// Empty document with one page.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            pages: vec![Page::new("Page 1")],
            variables: Variables::new(),
            next_id: 1,
            next_connection: 1,
        }
    }

    /// Rewrite every inline style value. Returns how many nodes changed.
    pub fn map_style_values(&mut self, mut f: impl FnMut(&str) -> String) -> usize {
        let mut changed = 0;
        for node in self.nodes.values_mut() {
            if node.style.map_values(&mut f) {
                changed += 1;
            }
        }
        changed
    }

    /// How many layers reference a variable.
    #[must_use]
    pub fn variable_usage(&self, name: &str) -> usize {
        let needle = format!("var(--{name}");
        self.nodes
            .values()
            .filter(|n| {
                n.style.iter().any(|(_, v)| {
                    v.match_indices(&needle).any(|(i, _)| {
                        v[i + needle.len()..]
                            .chars()
                            .next()
                            .is_none_or(|c| c == ')' || c == ',' || c.is_whitespace())
                    })
                })
            })
            .count()
    }

    /// A node's computed style with design variables resolved.
    #[must_use]
    pub fn computed(&self, node: &Node) -> Computed {
        node.style.computed_with(&self.variables)
    }

    /// Allocate a fresh identity.
    pub fn alloc_id(&mut self) -> NodeId {
        while self.nodes.contains_key(&NodeId(self.next_id)) {
            self.next_id += 1;
        }
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Reserve an identity read from disk so allocation never reuses it.
    pub(crate) fn reserve_id(&mut self, id: NodeId) {
        self.next_id = self.next_id.max(id.0 + 1);
    }

    /// Insert a detached node, returning its id.
    pub fn add(&mut self, node: Node) -> NodeId {
        let id = node.id;
        self.reserve_id(id);
        self.nodes.insert(id, node);
        id
    }

    /// Create a detached element.
    pub fn new_element(&mut self, tag: &str) -> NodeId {
        let id = self.alloc_id();
        self.add(Node::element(id, tag))
    }

    /// Create a detached text run.
    pub fn new_text(&mut self, text: &str) -> NodeId {
        let id = self.alloc_id();
        self.add(Node::text(id, text))
    }

    /// Lookup.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Mutable lookup.
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Whether the node exists.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the document has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Children of a node (empty when missing).
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.nodes.get(&id).map_or(&[], |node| &node.children)
    }

    /// Parent of a node.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(&id).and_then(|node| node.parent)
    }

    /// Ancestors from the parent up to the root.
    #[must_use]
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut current = self.parent(id);
        while let Some(node) = current {
            out.push(node);
            current = self.parent(node);
        }
        out
    }

    /// The root of the tree containing a node.
    #[must_use]
    pub fn root_of(&self, id: NodeId) -> NodeId {
        self.ancestors(id).last().copied().unwrap_or(id)
    }

    /// Whether `ancestor` is `id` or one of its ancestors.
    #[must_use]
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, id: NodeId) -> bool {
        id == ancestor || self.ancestors(id).contains(&ancestor)
    }

    /// Depth-first pre-order ids of a subtree.
    #[must_use]
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            out.push(current);
            for child in self.children(current).iter().rev() {
                stack.push(*child);
            }
        }
        out
    }

    /// Locate an artboard by its root id: `(page, index)`.
    #[must_use]
    pub fn artboard_index(&self, root: NodeId) -> Option<(usize, usize)> {
        self.pages.iter().enumerate().find_map(|(page, p)| {
            p.artboards
                .iter()
                .position(|artboard| artboard.root == root)
                .map(|index| (page, index))
        })
    }

    /// The artboard containing a node.
    #[must_use]
    pub fn artboard_of(&self, id: NodeId) -> Option<&Artboard> {
        let root = self.root_of(id);
        let (page, index) = self.artboard_index(root)?;
        self.pages.get(page)?.artboards.get(index)
    }

    /// Mutable artboard by root.
    pub fn artboard_mut(&mut self, root: NodeId) -> Option<&mut Artboard> {
        let (page, index) = self.artboard_index(root)?;
        self.pages.get_mut(page)?.artboards.get_mut(index)
    }

    /// Whether a node is an artboard root.
    #[must_use]
    pub fn is_artboard(&self, id: NodeId) -> bool {
        self.artboard_index(id).is_some()
    }

    /// Display name: explicit layer name, else a readable default.
    #[must_use]
    pub fn display_name(&self, id: NodeId) -> String {
        let Some(node) = self.get(id) else {
            return id.to_string();
        };
        if let Some(name) = node.name.as_ref().filter(|name| !name.is_empty()) {
            return name.clone();
        }
        match &node.kind {
            NodeKind::Text(text) => truncate(text.trim(), 32),
            NodeKind::Svg(_) => "Vector".to_owned(),
            NodeKind::Element { tag } => {
                if let Some(text) = self.text_content(id).filter(|t| !t.trim().is_empty())
                    && self.is_text_layer(id)
                {
                    return truncate(text.trim(), 32);
                }
                match tag.as_str() {
                    "div" | "section" | "main" | "header" | "footer" | "nav" | "aside"
                    | "article" => {
                        if self.children(id).is_empty() {
                            "Rectangle".to_owned()
                        } else {
                            "Frame".to_owned()
                        }
                    }
                    "img" => "Image".to_owned(),
                    "button" => "Button".to_owned(),
                    "input" => "Input".to_owned(),
                    other => other.to_owned(),
                }
            }
        }
    }

    /// Whether an element holds only inline/text content (a "text layer").
    #[must_use]
    pub fn is_text_layer(&self, id: NodeId) -> bool {
        let Some(node) = self.get(id) else {
            return false;
        };
        if node.is_text() {
            return true;
        }
        // Flex and grid containers blockify their children into separate items.
        if matches!(
            node.style.computed().display,
            style::Display::Flex | style::Display::Grid
        ) && node
            .children
            .iter()
            .any(|c| self.get(*c).is_some_and(|n| !n.is_text()))
        {
            return false;
        }
        let children = &node.children;
        !children.is_empty()
            && children.iter().any(|c| self.subtree_has_text(*c))
            && children.iter().all(|c| self.is_inline_subtree(*c))
    }

    fn subtree_has_text(&self, id: NodeId) -> bool {
        self.descendants(id)
            .into_iter()
            .any(|n| self.get(n).is_some_and(Node::is_text))
    }

    /// Whether a subtree is entirely inline content.
    #[must_use]
    pub fn is_inline_subtree(&self, id: NodeId) -> bool {
        self.descendants(id).into_iter().all(|n| {
            self.get(n).is_some_and(|node| {
                is_inline_tag(node.tag()) && node.style.get("display").is_none_or(|d| d == "inline")
            })
        })
    }

    /// Concatenated text of a subtree, if any text exists.
    #[must_use]
    pub fn text_content(&self, id: NodeId) -> Option<String> {
        let mut out = String::new();
        let mut any = false;
        for node in self.descendants(id) {
            match self.get(node).map(|n| &n.kind) {
                Some(NodeKind::Text(text)) => {
                    any = true;
                    out.push_str(text);
                }
                Some(NodeKind::Element { tag }) if tag == "br" => out.push('\n'),
                _ => {}
            }
        }
        any.then_some(out)
    }

    /// Replace an element's content with a single text run.
    pub fn set_text(&mut self, id: NodeId, text: &str) -> Result<(), EditError> {
        let node = self.get(id).ok_or(EditError::Missing(id))?;
        if let NodeKind::Text(_) = node.kind {
            if let Some(node) = self.get_mut(id) {
                node.kind = NodeKind::Text(text.to_owned());
            }
            return Ok(());
        }
        for child in node.children.clone() {
            self.remove(child)?;
        }
        let run = self.new_text(text);
        self.attach(run, id, None)
    }

    /// Attach a detached node under `parent` at `index` (append when `None`).
    pub fn attach(
        &mut self,
        child: NodeId,
        parent: NodeId,
        index: Option<usize>,
    ) -> Result<(), EditError> {
        if !self.contains(child) {
            return Err(EditError::Missing(child));
        }
        let parent_node = self.get(parent).ok_or(EditError::Missing(parent))?;
        if !matches!(parent_node.kind, NodeKind::Element { .. }) {
            return Err(EditError::NotContainer(parent));
        }
        if self.is_ancestor_or_self(child, parent) {
            return Err(EditError::Cycle(child));
        }
        self.detach(child);
        if let Some(parent_node) = self.get_mut(parent) {
            let index = index
                .unwrap_or(parent_node.children.len())
                .min(parent_node.children.len());
            parent_node.children.insert(index, child);
        }
        if let Some(node) = self.get_mut(child) {
            node.parent = Some(parent);
        }
        Ok(())
    }

    /// Detach a node from its parent (keeping it in the arena).
    pub fn detach(&mut self, id: NodeId) {
        let Some(parent) = self.parent(id) else {
            return;
        };
        if let Some(parent_node) = self.get_mut(parent) {
            parent_node.children.retain(|child| *child != id);
        }
        if let Some(node) = self.get_mut(id) {
            node.parent = None;
        }
    }

    /// Remove a node and its subtree. Removing an artboard root removes the artboard.
    pub fn remove(&mut self, id: NodeId) -> Result<(), EditError> {
        if !self.contains(id) {
            return Err(EditError::Missing(id));
        }
        self.detach(id);
        if let Some((page, index)) = self.artboard_index(id) {
            self.pages[page].artboards.remove(index);
        }
        let removed = self.descendants(id);
        for page in &mut self.pages {
            page.connections
                .retain(|c| !removed.iter().any(|node| c.touches(*node)));
        }
        for node in removed {
            self.nodes.remove(&node);
        }
        Ok(())
    }

    /// Add a connection on a page. Returns its id.
    pub fn add_connection(
        &mut self,
        page: usize,
        from: Endpoint,
        to: Endpoint,
    ) -> Result<u64, EditError> {
        for end in [from, to] {
            if let Some(node) = end.node()
                && !self.contains(node)
            {
                return Err(EditError::Missing(node));
            }
        }
        let id = self.next_connection_id();
        let page = page.min(self.pages.len().saturating_sub(1));
        if let Some(page) = self.pages.get_mut(page) {
            page.connections.push(Connection::new(id, from, to));
        }
        Ok(id)
    }

    fn next_connection_id(&mut self) -> u64 {
        let used = self
            .pages
            .iter()
            .flat_map(|p| &p.connections)
            .map(|c| c.id + 1)
            .max()
            .unwrap_or(1);
        self.next_connection = self.next_connection.max(used);
        let id = self.next_connection;
        self.next_connection += 1;
        id
    }

    /// Find a connection: `(page, index)`.
    #[must_use]
    pub fn connection_index(&self, id: u64) -> Option<(usize, usize)> {
        self.pages.iter().enumerate().find_map(|(page, p)| {
            p.connections
                .iter()
                .position(|c| c.id == id)
                .map(|index| (page, index))
        })
    }

    /// Lookup a connection.
    #[must_use]
    pub fn connection(&self, id: u64) -> Option<&Connection> {
        let (page, index) = self.connection_index(id)?;
        self.pages.get(page)?.connections.get(index)
    }

    /// Mutable lookup.
    pub fn connection_mut(&mut self, id: u64) -> Option<&mut Connection> {
        let (page, index) = self.connection_index(id)?;
        self.pages.get_mut(page)?.connections.get_mut(index)
    }

    /// Remove a connection; returns whether it existed.
    pub fn remove_connection(&mut self, id: u64) -> bool {
        match self.connection_index(id) {
            Some((page, index)) => {
                self.pages[page].connections.remove(index);
                true
            }
            None => false,
        }
    }

    /// Move a node to a new parent position.
    pub fn move_node(
        &mut self,
        id: NodeId,
        parent: NodeId,
        index: Option<usize>,
    ) -> Result<(), EditError> {
        if self.is_artboard(id) {
            return Err(EditError::Artboard(id));
        }
        // Adjust the index when moving later within the same parent.
        let index = match (self.parent(id), index) {
            (Some(current), Some(index)) if current == parent => {
                let old = self.children(parent).iter().position(|c| *c == id);
                Some(match old {
                    Some(old) if old < index => index - 1,
                    _ => index,
                })
            }
            _ => index,
        };
        self.attach(id, parent, index)
    }

    /// Deep-copy a subtree as a new detached subtree.
    pub fn clone_subtree(&mut self, id: NodeId) -> Result<NodeId, EditError> {
        let source = self.get(id).cloned().ok_or(EditError::Missing(id))?;
        let new_id = self.alloc_id();
        let mut copy = source.clone();
        copy.id = new_id;
        copy.parent = None;
        copy.children = Vec::new();
        self.add(copy);
        for child in source.children {
            let child_copy = self.clone_subtree(child)?;
            self.attach(child_copy, new_id, None)?;
        }
        Ok(new_id)
    }

    /// Duplicate a node next to the original (artboards are offset on the canvas).
    pub fn duplicate(&mut self, id: NodeId) -> Result<NodeId, EditError> {
        let copy = self.clone_subtree(id)?;
        if let Some((page, index)) = self.artboard_index(id) {
            let original = self.pages[page].artboards[index].clone();
            let width = self
                .get(id)
                .and_then(|n| n.style.computed().width.px())
                .unwrap_or(400.0);
            let file = self.unique_artboard_file(&self.display_name(id));
            self.pages[page].artboards.insert(
                index + 1,
                Artboard {
                    root: copy,
                    file,
                    x: original.x + width + 80.0,
                    y: original.y,
                },
            );
            return Ok(copy);
        }
        if let Some(parent) = self.parent(id) {
            let index = self.children(parent).iter().position(|c| *c == id);
            self.attach(copy, parent, index.map(|i| i + 1))?;
            if self
                .get(copy)
                .is_some_and(|n| n.style.computed().position == style::Position::Absolute)
                && let Some(node) = self.get_mut(copy)
            {
                let computed = node.style.computed();
                let left = computed.inset[3].px().unwrap_or(0.0) + 16.0;
                let top = computed.inset[0].px().unwrap_or(0.0) + 16.0;
                node.style.set("left", &Length::Px(left).to_css());
                node.style.set("top", &Length::Px(top).to_css());
            }
        }
        Ok(copy)
    }

    /// Add a detached element as a new artboard on a page.
    pub fn add_artboard(&mut self, page: usize, root: NodeId, x: f32, y: f32) -> NodeId {
        let file = self.unique_artboard_file(&self.display_name(root));
        let page = page.min(self.pages.len().saturating_sub(1));
        if self.pages.is_empty() {
            self.pages.push(Page::new("Page 1"));
        }
        self.detach(root);
        self.pages[page]
            .artboards
            .push(Artboard { root, file, x, y });
        root
    }

    /// Create a new empty artboard element with a size.
    pub fn create_artboard(
        &mut self,
        page: usize,
        name: &str,
        (x, y): (f32, f32),
        (width, height): (f32, f32),
    ) -> NodeId {
        let root = self.new_element("div");
        if let Some(node) = self.get_mut(root) {
            node.name = Some(name.to_owned());
            node.style = Style::parse(&format!(
                "position: relative; width: {}; height: {}; background-color: #ffffff; overflow: hidden",
                Length::Px(width).to_css(),
                Length::Px(height).to_css()
            ));
        }
        self.add_artboard(page, root, x, y)
    }

    /// A file name derived from a display name that no artboard uses yet.
    #[must_use]
    pub fn unique_artboard_file(&self, name: &str) -> String {
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
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "artboard" } else { slug };
        let taken = |file: &str| {
            self.pages
                .iter()
                .flat_map(|p| &p.artboards)
                .any(|a| a.file.eq_ignore_ascii_case(file))
        };
        let mut candidate = format!("{slug}.html");
        let mut n = 2;
        while taken(&candidate) {
            candidate = format!("{slug}-{n}.html");
            n += 1;
        }
        candidate
    }

    /// Every artboard on every page.
    pub fn artboards(&self) -> impl Iterator<Item = &Artboard> {
        self.pages.iter().flat_map(|page| &page.artboards)
    }

    /// Collect garbage nodes no longer reachable from any artboard.
    pub fn prune_detached(&mut self) {
        let mut live = std::collections::BTreeSet::new();
        for artboard in self.artboards() {
            live.extend(self.descendants(artboard.root));
        }
        self.nodes.retain(|id, _| live.contains(id));
    }
}

fn truncate(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let mut out: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Document, NodeId, NodeId, NodeId) {
        let mut doc = Document::new();
        let board = doc.create_artboard(0, "Desktop", (0.0, 0.0), (800.0, 600.0));
        let a = doc.new_element("div");
        let b = doc.new_element("p");
        doc.attach(a, board, None).unwrap();
        doc.attach(b, board, None).unwrap();
        doc.set_text(b, "Hello").unwrap();
        (doc, board, a, b)
    }

    #[test]
    fn structure_edits_keep_parent_links() {
        let (mut doc, board, a, b) = sample();
        assert_eq!(doc.children(board), &[a, b]);
        doc.move_node(b, board, Some(0)).unwrap();
        assert_eq!(doc.children(board), &[b, a]);
        doc.move_node(b, board, Some(2)).unwrap();
        assert_eq!(doc.children(board), &[a, b]);
        doc.move_node(b, a, None).unwrap();
        assert_eq!(doc.parent(b), Some(a));
        assert_eq!(doc.move_node(a, b, None), Err(EditError::Cycle(a)));
        assert!(doc.is_text_layer(b));
        assert_eq!(doc.display_name(b), "Hello");
    }

    #[test]
    fn flex_containers_are_not_text_layers() {
        let mut doc = Document::new();
        let roots = html::parse_fragment(
            &mut doc,
            r#"<nav style="display: flex; gap: 8px"><span>A</span><span>B</span></nav><p>x <b>y</b></p><button style="display: flex">Go</button>"#,
            html::ImportOptions { keep_ids: false },
        );
        assert!(!doc.is_text_layer(roots[0]));
        assert!(doc.is_text_layer(roots[1]));
        assert!(
            doc.is_text_layer(roots[2]),
            "a flex box with only text is still a text layer"
        );
    }

    #[test]
    fn connections_follow_node_lifetimes() {
        let (mut doc, board, a, b) = sample();
        let c1 = doc
            .add_connection(0, Endpoint::Node(a), Endpoint::Node(b))
            .unwrap();
        let c2 = doc
            .add_connection(0, Endpoint::Point { x: 0.0, y: 0.0 }, Endpoint::Node(board))
            .unwrap();
        assert_ne!(c1, c2);
        assert_eq!(
            doc.add_connection(0, Endpoint::Node(NodeId(999)), Endpoint::Node(a)),
            Err(EditError::Missing(NodeId(999)))
        );
        doc.remove(b).unwrap();
        assert!(
            doc.connection(c1).is_none(),
            "removing a layer drops its connectors"
        );
        assert!(doc.connection(c2).is_some());
        assert!(doc.remove_connection(c2));
        assert!(!doc.remove_connection(c2));
    }

    #[test]
    fn duplicate_and_remove() {
        let (mut doc, board, _a, b) = sample();
        let copy = doc.duplicate(b).unwrap();
        assert_eq!(doc.text_content(copy).as_deref(), Some("Hello"));
        assert_eq!(doc.children(board).len(), 3);
        let board_copy = doc.duplicate(board).unwrap();
        assert_eq!(doc.pages[0].artboards.len(), 2);
        assert_eq!(doc.pages[0].artboards[1].file, "desktop-2.html");
        doc.remove(board_copy).unwrap();
        assert_eq!(doc.pages[0].artboards.len(), 1);
        let before = doc.len();
        doc.remove(copy).unwrap();
        assert_eq!(doc.len(), before - 2);
    }
}
