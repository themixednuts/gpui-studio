//! Editor state and every document operation.
//!
//! The canvas, panels, keyboard shortcuts, and MCP agents all call these
//! methods, so a change made by a person and the same change made by an agent
//! take one path: snapshot for undo, apply, bump the revision, autosave.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::comments::Comments;
use crate::geometry::{Align, Axis, Rect, align_deltas, distribute_deltas};
use crate::history::History;
use crate::model::html::{ExportOptions, ImportOptions, parse_fragment, to_html};
use crate::model::style::{Position, fmt_num};
use crate::model::{ArrowHeads, ConnectorStyle, Document, Endpoint, Length, NodeId, NodeKind};
use crate::presets;
use crate::project::Project;

const AUTOSAVE_QUIET: Duration = Duration::from_millis(400);

/// Canvas tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Select, move, resize.
    Select,
    /// Pan the canvas.
    Hand,
    /// Draw frames (artboards on empty canvas).
    Frame,
    /// Draw rectangles.
    Rectangle,
    /// Draw ellipses.
    Ellipse,
    /// Place text.
    Text,
    /// Freehand vector strokes.
    Pencil,
    /// Straight vector lines.
    Line,
    /// Vector arrows.
    Arrow,
    /// Connectors between layers that follow them.
    Connector,
    /// Drop comment pins.
    Comment,
}

impl Tool {
    /// Tooltip label with shortcut.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select (V)",
            Self::Hand => "Hand (H)",
            Self::Frame => "Frame (F)",
            Self::Rectangle => "Rectangle (R)",
            Self::Ellipse => "Ellipse (O)",
            Self::Text => "Text (T)",
            Self::Pencil => "Pencil (P)",
            Self::Line => "Line (L)",
            Self::Arrow => "Arrow (Shift+L)",
            Self::Connector => "Connector (X)",
            Self::Comment => "Comment (C)",
        }
    }

    /// Whether dragging draws a box-shaped layer.
    #[must_use]
    pub fn draws_box(self) -> bool {
        matches!(
            self,
            Self::Frame | Self::Rectangle | Self::Ellipse | Self::Text
        )
    }
}

/// Where inserted content goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsertTarget {
    /// Parent element; `None` places each root as a new artboard.
    pub parent: Option<NodeId>,
    /// Index among the parent's children; `None` appends.
    pub index: Option<usize>,
}

/// Where an imported image goes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImagePlacement {
    /// Parent element; `None` creates an artboard holding the image.
    pub parent: Option<NodeId>,
    /// Top-left corner: canvas position for a new artboard, or the offset in
    /// the parent for absolute placement. `None` picks a free spot.
    pub at: Option<(f32, f32)>,
    /// Index among the parent's children for flex/grid parents.
    pub index: Option<usize>,
}

/// Where a layer dropped in the layer list lands relative to the target row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPosition {
    /// Just before the target, as its sibling.
    Before,
    /// As the target's last child.
    Inside,
    /// Just after the target, as its sibling.
    After,
}

/// The open design and its editing state.
pub struct Editor {
    /// The document.
    pub doc: Document,
    /// Backing project directory (absent in tests that run in memory).
    pub project: Option<Project>,
    /// Review comments.
    pub comments: Option<Comments>,
    /// Undo stacks.
    pub history: History,
    /// Selected nodes, primary first.
    pub selection: Vec<NodeId>,
    /// Current page.
    pub page: usize,
    /// Monotonic revision, bumped by every edit.
    pub revision: u64,
    /// Last status message.
    pub status: String,
    /// Laid-out bounds in document pixels from the canvas, for operations that
    /// depend on rendered geometry (alignment, export, agent queries). Empty
    /// when nothing has been rendered.
    pub measured: HashMap<NodeId, Rect>,
    dirty: bool,
    last_edit: Instant,
}

impl Editor {
    /// In-memory editor.
    #[must_use]
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            project: None,
            comments: None,
            history: History::default(),
            selection: Vec::new(),
            page: 0,
            revision: 1,
            status: "Ready".to_owned(),
            measured: HashMap::new(),
            dirty: false,
            last_edit: Instant::now(),
        }
    }

    /// Open a project directory (scaffolding a starter design if empty).
    pub fn open(root: &std::path::Path) -> Result<Self> {
        let (mut project, doc) = Project::open(root)?;
        if let Err(error) = project.watch() {
            eprintln!("watching {} failed: {error:#}", root.display());
        }
        let comments = Comments::load(&project.studio_dir())?;
        let mut editor = Self::new(doc);
        editor.status = format!("Opened {}", project.root().display());
        editor.project = Some(project);
        editor.comments = Some(comments);
        Ok(editor)
    }

    /// Whether unsaved edits exist.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Primary selected node.
    #[must_use]
    pub fn primary(&self) -> Option<NodeId> {
        self.selection.first().copied()
    }

    /// Replace the selection, dropping missing nodes and duplicates.
    pub fn select(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let mut out: Vec<NodeId> = Vec::new();
        for id in ids {
            if self.doc.contains(id) && !out.contains(&id) {
                out.push(id);
            }
        }
        if let Some(first) = out.first()
            && let Some(artboard) = self.doc.artboard_of(*first)
        {
            let root = artboard.root;
            if let Some((page, _)) = self.doc.artboard_index(root) {
                self.page = page;
            }
        }
        self.selection = out;
    }

    /// Toggle one node in the selection.
    pub fn toggle_selected(&mut self, id: NodeId) {
        if let Some(index) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(index);
        } else if self.doc.contains(id) {
            self.selection.push(id);
        }
    }

    /// Run an edit as one undoable, revisioned step. On error the document is
    /// left untouched.
    pub fn edit<R>(
        &mut self,
        key: Option<&str>,
        apply: impl FnOnce(&mut Document) -> Result<R>,
    ) -> Result<R> {
        let before = self.doc.clone();
        match apply(&mut self.doc) {
            Ok(result) => {
                if self.doc != before {
                    // Edits to main components flow into their instances as
                    // part of the same undoable step.
                    self.doc.sync_components(&before);
                    self.history.record(&before, key);
                    self.revision += 1;
                    self.dirty = true;
                    self.last_edit = Instant::now();
                    self.doc.prune_detached();
                }
                self.selection.retain(|id| self.doc.contains(*id));
                Ok(result)
            }
            Err(error) => {
                self.doc = before;
                Err(error)
            }
        }
    }

    /// Undo one step.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.history.undo(&self.doc) else {
            return false;
        };
        self.doc = previous;
        self.after_history_jump();
        true
    }

    /// Redo one step.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.history.redo(&self.doc) else {
            return false;
        };
        self.doc = next;
        self.after_history_jump();
        true
    }

    fn after_history_jump(&mut self) {
        self.revision += 1;
        self.dirty = true;
        self.last_edit = Instant::now();
        self.selection.retain(|id| self.doc.contains(*id));
        self.page = self.page.min(self.doc.pages.len().saturating_sub(1));
    }

    /// Save when edits have been quiet long enough. Returns whether it saved.
    pub fn autosave(&mut self) -> bool {
        if self.dirty && self.last_edit.elapsed() >= AUTOSAVE_QUIET {
            if let Err(error) = self.save() {
                self.status = format!("Save failed: {error:#}");
                // Retry after the next quiet period instead of every tick.
                self.last_edit = Instant::now();
            }
            return true;
        }
        false
    }

    /// Write all changes now.
    pub fn save(&mut self) -> Result<()> {
        if let Some(project) = &mut self.project {
            let written = project.save(&self.doc)?;
            if written > 0 {
                self.status = format!("Saved {written} file(s)");
            }
        }
        self.dirty = false;
        Ok(())
    }

    /// Merge changes other tools made on disk. Returns whether anything changed.
    pub fn sync_external(&mut self) -> bool {
        let Some(project) = &mut self.project else {
            return false;
        };
        if !project.poll_events() || self.dirty {
            // While Studio has unsaved edits its own save wins; the next
            // external write after that is merged.
            return false;
        }
        let before = self.doc.clone();
        match project.sync_external(&mut self.doc) {
            Ok(changed) if !changed.is_empty() => {
                self.doc.sync_components(&before);
                self.revision += 1;
                self.history.seal();
                self.selection.retain(|id| self.doc.contains(*id));
                self.page = self.page.min(self.doc.pages.len().saturating_sub(1));
                self.status = format!("Reloaded {} from disk", changed.join(", "));
                true
            }
            Ok(_) => false,
            Err(error) => {
                self.status = format!("Reload failed: {error:#}");
                true
            }
        }
    }

    // ---- structural operations -------------------------------------------------

    /// Set (Some) or remove (None) CSS properties on nodes.
    pub fn set_styles(
        &mut self,
        ids: &[NodeId],
        changes: &[(String, Option<String>)],
        key: Option<&str>,
    ) -> Result<()> {
        self.edit(key, |doc| {
            for id in ids {
                let node = doc
                    .get_mut(*id)
                    .ok_or_else(|| anyhow!("node {id} does not exist"))?;
                for (property, value) in changes {
                    match value {
                        Some(value) => node.style.set(property, value),
                        None => node.style.clear_family(property),
                    }
                }
            }
            Ok(())
        })
    }

    /// Replace a node's whole inline style.
    pub fn set_style_text(&mut self, id: NodeId, css: &str, key: Option<&str>) -> Result<()> {
        self.edit(key, |doc| {
            let node = doc
                .get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?;
            node.style = crate::model::Style::parse(css);
            Ok(())
        })
    }

    /// Replace text content.
    pub fn set_text(&mut self, id: NodeId, text: &str, key: Option<&str>) -> Result<()> {
        self.edit(key, |doc| Ok(doc.set_text(id, text)?))
    }

    /// Rename a layer (empty clears the name).
    pub fn rename(&mut self, id: NodeId, name: &str) -> Result<()> {
        self.edit(Some(&format!("rename-{id}")), |doc| {
            let node = doc
                .get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?;
            node.name = (!name.trim().is_empty()).then(|| name.trim().to_owned());
            // A main component's name is its layer name.
            if let Some(name) = node.name.clone()
                && node
                    .attr(crate::model::components::COMPONENT_ATTR)
                    .is_some()
            {
                node.set_attr(crate::model::components::COMPONENT_ATTR, &name);
            }
            Ok(())
        })
    }

    /// Set or remove HTML attributes.
    pub fn set_attributes(&mut self, id: NodeId, attrs: &[(String, Option<String>)]) -> Result<()> {
        self.edit(None, |doc| {
            let node = doc
                .get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?;
            for (name, value) in attrs {
                let lowered = name.to_ascii_lowercase();
                if lowered == "style" || lowered.starts_with("on") || lowered.starts_with("data-id")
                {
                    bail!("attribute {name} cannot be set this way");
                }
                node.set_attr(&lowered, value.as_deref().unwrap_or(""));
            }
            Ok(())
        })
    }

    /// Change an element's tag, keeping content and style.
    pub fn set_tag(&mut self, id: NodeId, tag: &str) -> Result<()> {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("invalid tag {tag:?}");
        }
        self.edit(None, |doc| {
            let node = doc
                .get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?;
            match &mut node.kind {
                NodeKind::Element { tag: current } => *current = tag,
                _ => bail!("node {id} is not an element"),
            }
            Ok(())
        })
    }

    /// Toggle visibility.
    pub fn set_hidden(&mut self, id: NodeId, hidden: bool) -> Result<()> {
        self.edit(None, |doc| {
            doc.get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?
                .hidden = hidden;
            Ok(())
        })
    }

    /// Toggle lock.
    pub fn set_locked(&mut self, id: NodeId, locked: bool) -> Result<()> {
        self.edit(None, |doc| {
            doc.get_mut(id)
                .ok_or_else(|| anyhow!("node {id} does not exist"))?
                .locked = locked;
            Ok(())
        })
    }

    /// Drop ids whose ancestor is also listed.
    fn topmost(&self, ids: &[NodeId]) -> Vec<NodeId> {
        ids.iter()
            .copied()
            .filter(|id| {
                self.doc.contains(*id)
                    && !self
                        .doc
                        .ancestors(*id)
                        .iter()
                        .any(|ancestor| ids.contains(ancestor))
            })
            .collect()
    }

    /// Delete nodes (and artboards).
    pub fn delete(&mut self, ids: &[NodeId]) -> Result<usize> {
        let ids = self.topmost(ids);
        let count = ids.len();
        self.edit(None, |doc| {
            for id in &ids {
                doc.remove(*id)?;
            }
            Ok(())
        })?;
        self.selection.retain(|id| self.doc.contains(*id));
        Ok(count)
    }

    /// Duplicate nodes; the copies become the selection.
    pub fn duplicate(&mut self, ids: &[NodeId]) -> Result<Vec<NodeId>> {
        let ids = self.topmost(ids);
        let copies = self.edit(None, |doc| {
            ids.iter()
                .map(|id| Ok(doc.duplicate(*id)?))
                .collect::<Result<Vec<_>>>()
        })?;
        self.selection = copies.clone();
        Ok(copies)
    }

    /// Move a node to a parent position.
    pub fn move_node(&mut self, id: NodeId, parent: NodeId, index: Option<usize>) -> Result<()> {
        self.edit(None, |doc| Ok(doc.move_node(id, parent, index)?))
    }

    /// Copy an image into the project's `artboards/assets/` and place it as an
    /// `<img>` layer. One undoable step.
    pub fn import_image(
        &mut self,
        bytes: &[u8],
        file_name: &str,
        placement: ImagePlacement,
    ) -> Result<NodeId> {
        let project = self
            .project
            .as_ref()
            .context("images can only be imported into a saved project")?;
        let src = crate::assets::store_image(&project.root().join("artboards"), file_name, bytes)?;
        let ext = crate::assets::extension(file_name).unwrap_or_default();
        let natural = crate::assets::image_size(bytes, &ext).unwrap_or((400.0, 300.0));
        let stem = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Image")
            .to_owned();
        let name = crate::model::html::escape_attr(&stem);
        let src = crate::model::html::escape_attr(&src);
        let page = self.page;
        let next = self.next_artboard_position();
        let id = match placement.parent {
            None => {
                let (w, h) = crate::assets::fit_within(natural, 1600.0);
                let (x, y) = placement.at.unwrap_or(next);
                let html = format!(
                    "<div data-name=\"{name}\" style=\"position: relative; width: {w}px; height: {h}px; background-color: transparent; overflow: hidden\"><img data-name=\"{name}\" src=\"{src}\" alt=\"{name}\" style=\"display: block; width: 100%; height: 100%; object-fit: cover\"></div>",
                    w = fmt_num(w),
                    h = fmt_num(h),
                );
                self.edit(None, |doc| {
                    let roots = parse_fragment(doc, &html, ImportOptions { keep_ids: false });
                    let root = *roots.first().context("image markup did not parse")?;
                    doc.add_artboard(page, root, x.round(), y.round());
                    Ok(root)
                })?
            }
            Some(parent) => {
                let computed = self
                    .doc
                    .get(parent)
                    .context("the parent no longer exists")?
                    .style
                    .computed();
                let flow = matches!(
                    computed.display,
                    crate::model::style::Display::Flex | crate::model::style::Display::Grid
                );
                let limit = self
                    .measured
                    .get(&parent)
                    .map_or(800.0, |r| r.w.clamp(48.0, 800.0));
                let (w, h) = crate::assets::fit_within(natural, limit);
                let mut style = format!(
                    "display: block; width: {}px; height: {}px; object-fit: cover",
                    fmt_num(w),
                    fmt_num(h)
                );
                if !flow {
                    let (x, y) = placement.at.unwrap_or((0.0, 0.0));
                    style = format!(
                        "position: absolute; left: {}px; top: {}px; {style}",
                        fmt_num(x.round()),
                        fmt_num(y.round())
                    );
                }
                let html = format!(
                    "<img data-name=\"{name}\" src=\"{src}\" alt=\"{name}\" style=\"{style}\">"
                );
                let needs_relative =
                    !flow && computed.position == Position::Static && !self.doc.is_artboard(parent);
                let index = if flow { placement.index } else { None };
                self.edit(None, |doc| {
                    if needs_relative && let Some(node) = doc.get_mut(parent) {
                        node.style.set("position", "relative");
                    }
                    let roots = parse_fragment(doc, &html, ImportOptions { keep_ids: false });
                    let root = *roots.first().context("image markup did not parse")?;
                    doc.attach(root, parent, index)?;
                    Ok(root)
                })?
            }
        };
        self.select([id]);
        Ok(id)
    }

    /// Whether `dragged` may be dropped at `target`/`position` in the layer list.
    #[must_use]
    pub fn can_drop_layer(&self, dragged: NodeId, target: NodeId, position: DropPosition) -> bool {
        let doc = &self.doc;
        if dragged == target || doc.is_ancestor_or_self(dragged, target) {
            return false;
        }
        let dragged_artboard = doc.is_artboard(dragged);
        match position {
            DropPosition::Inside => {
                !dragged_artboard
                    && doc.get(target).is_some_and(|n| {
                        matches!(&n.kind, NodeKind::Element { tag } if !matches!(tag.as_str(), "img" | "input" | "br" | "hr"))
                    })
                    && !doc.is_text_layer(target)
            }
            DropPosition::Before | DropPosition::After => dragged_artboard == doc.is_artboard(target),
        }
    }

    /// Move a layer (or reorder an artboard) by dropping it on another row of
    /// the layer list.
    pub fn drop_layer(
        &mut self,
        dragged: NodeId,
        target: NodeId,
        position: DropPosition,
    ) -> Result<()> {
        if !self.can_drop_layer(dragged, target, position) {
            bail!("can't move {} there", self.doc.display_name(dragged));
        }
        if let (Some((page, from)), Some((target_page, to))) = (
            self.doc.artboard_index(dragged),
            self.doc.artboard_index(target),
        ) {
            if page != target_page {
                bail!("artboards can only be reordered within a page");
            }
            let mut to = to + usize::from(position == DropPosition::After);
            if from < to {
                to -= 1;
            }
            return self.edit(None, |doc| {
                let artboard = doc.pages[page].artboards.remove(from);
                doc.pages[page].artboards.insert(to, artboard);
                Ok(())
            });
        }
        let (parent, index) = match position {
            DropPosition::Inside => (target, None),
            DropPosition::Before | DropPosition::After => {
                let parent = self.doc.parent(target).context("target has no parent")?;
                let index = self
                    .doc
                    .children(parent)
                    .iter()
                    .position(|c| *c == target)
                    .context("target is not a child of its parent")?;
                (
                    parent,
                    Some(index + usize::from(position == DropPosition::After)),
                )
            }
        };
        self.move_node(dragged, parent, index)?;
        self.select([dragged]);
        Ok(())
    }

    /// Move an artboard on the canvas.
    pub fn move_artboard(&mut self, root: NodeId, x: f32, y: f32, key: Option<&str>) -> Result<()> {
        self.edit(key, |doc| {
            let artboard = doc
                .artboard_mut(root)
                .ok_or_else(|| anyhow!("{root} is not an artboard"))?;
            artboard.x = x.round();
            artboard.y = y.round();
            Ok(())
        })
    }

    /// A free canvas position to the right of the current page's artboards.
    #[must_use]
    pub fn next_artboard_position(&self) -> (f32, f32) {
        let Some(page) = self.doc.pages.get(self.page) else {
            return (0.0, 0.0);
        };
        let mut right = None::<f32>;
        let mut top = None::<f32>;
        for artboard in &page.artboards {
            let width = self
                .doc
                .get(artboard.root)
                .and_then(|n| n.style.computed().width.px())
                .unwrap_or(400.0);
            right = Some(right.map_or(artboard.x + width, |r| r.max(artboard.x + width)));
            top = Some(top.map_or(artboard.y, |t| t.min(artboard.y)));
        }
        match (right, top) {
            (Some(right), Some(top)) => (right + 80.0, top),
            _ => (0.0, 0.0),
        }
    }

    /// Create an empty artboard.
    pub fn create_artboard(
        &mut self,
        name: &str,
        size: (f32, f32),
        position: Option<(f32, f32)>,
    ) -> Result<NodeId> {
        let position = position.unwrap_or_else(|| self.next_artboard_position());
        let page = self.page;
        let id = self.edit(None, |doc| {
            Ok(doc.create_artboard(page, name, position, size))
        })?;
        self.selection = vec![id];
        Ok(id)
    }

    /// Insert HTML. Returns the new top-level node ids, which become the selection.
    pub fn insert_html(&mut self, html: &str, target: InsertTarget) -> Result<Vec<NodeId>> {
        let page = self.page;
        let mut position = self.next_artboard_position();
        let ids = self.edit(None, |doc| {
            let roots = parse_fragment(doc, html, ImportOptions { keep_ids: false });
            if roots.is_empty() {
                bail!("the HTML contained no elements or text");
            }
            match target.parent {
                Some(parent) => {
                    let mut index = target.index;
                    for root in &roots {
                        doc.attach(*root, parent, index)?;
                        index = index.map(|i| i + 1);
                    }
                }
                None => {
                    for root in &roots {
                        if doc.get(*root).is_some_and(|n| n.is_text()) {
                            bail!("top-level text needs a parent element");
                        }
                        if let Some(node) = doc.get_mut(*root) {
                            let computed = node.style.computed();
                            if computed.width.px().is_none() {
                                node.style.set("width", "800px");
                            }
                            if computed.background.is_none() {
                                node.style.set("background-color", "#ffffff");
                            }
                            if computed.position == Position::Static {
                                node.style.set("position", "relative");
                            }
                        }
                        let width = doc
                            .get(*root)
                            .and_then(|n| n.style.computed().width.px())
                            .unwrap_or(800.0);
                        doc.add_artboard(page, *root, position.0, position.1);
                        position.0 += width + 80.0;
                    }
                }
            }
            Ok(roots)
        })?;
        self.selection = ids.clone();
        Ok(ids)
    }

    /// Replace a node with HTML (keeping its place).
    pub fn replace_html(&mut self, id: NodeId, html: &str) -> Result<Vec<NodeId>> {
        if self.doc.is_artboard(id) {
            let artboard = self
                .doc
                .artboard_of(id)
                .cloned()
                .context("artboard missing")?;
            let ids = self.edit(None, |doc| {
                let roots = parse_fragment(doc, html, ImportOptions { keep_ids: true });
                let [root] = roots.as_slice() else {
                    bail!("an artboard must be replaced by exactly one element");
                };
                let root = *root;
                doc.remove_subtree_keep_artboard(id);
                doc.replace_artboard_root(id, root);
                if let Some(slot) = doc.artboard_mut(root) {
                    slot.x = artboard.x;
                    slot.y = artboard.y;
                }
                Ok(vec![root])
            })?;
            self.selection = ids.clone();
            return Ok(ids);
        }
        let parent = self.doc.parent(id).context("node has no parent")?;
        let index = self.doc.children(parent).iter().position(|c| *c == id);
        let ids = self.edit(None, |doc| {
            doc.remove(id)?;
            let roots = parse_fragment(doc, html, ImportOptions { keep_ids: true });
            let mut at = index;
            for root in &roots {
                doc.attach(*root, parent, at)?;
                at = at.map(|i| i + 1);
            }
            Ok(roots)
        })?;
        self.selection = ids.clone();
        Ok(ids)
    }

    /// Insert a library component.
    pub fn insert_component(&mut self, key: &str, target: InsertTarget) -> Result<Vec<NodeId>> {
        let component =
            presets::component(key).ok_or_else(|| anyhow!("unknown component {key:?}"))?;
        self.insert_html(component.html, target)
    }

    /// Default insertion target for the current selection: inside a selected
    /// container, otherwise after the selected leaf, otherwise a new artboard.
    #[must_use]
    pub fn insertion_target(&self) -> InsertTarget {
        let Some(id) = self.primary() else {
            return InsertTarget::default();
        };
        let is_container = self.doc.get(id).is_some_and(|node| {
            matches!(node.kind, NodeKind::Element { .. })
                && !self.doc.is_text_layer(id)
                && !matches!(node.tag(), "img" | "input" | "hr" | "br")
        });
        if is_container {
            InsertTarget {
                parent: Some(id),
                index: None,
            }
        } else if let Some(parent) = self.doc.parent(id) {
            let index = self.doc.children(parent).iter().position(|c| *c == id);
            InsertTarget {
                parent: Some(parent),
                index: index.map(|i| i + 1),
            }
        } else {
            InsertTarget::default()
        }
    }

    /// Wrap nodes (siblings) in a new frame. Returns the frame.
    pub fn group(&mut self, ids: &[NodeId]) -> Result<NodeId> {
        let ids = self.topmost(ids);
        let first = *ids.first().context("nothing to group")?;
        let parent = self
            .doc
            .parent(first)
            .context("artboards cannot be grouped")?;
        if ids.iter().any(|id| self.doc.parent(*id) != Some(parent)) {
            bail!("only siblings can be grouped");
        }
        let mut ordered = ids.clone();
        let children = self.doc.children(parent).to_vec();
        ordered.sort_by_key(|id| children.iter().position(|c| c == id));
        let index = children.iter().position(|c| *c == ordered[0]);
        let parent_is_column = self
            .doc
            .get(parent)
            .is_some_and(|p| p.style.computed().direction.is_column());
        let frame = self.edit(None, |doc| {
            let frame = doc.new_element("div");
            if let Some(node) = doc.get_mut(frame) {
                node.name = Some("Group".to_owned());
                node.style = crate::model::Style::parse(if parent_is_column {
                    "display: flex; flex-direction: column; gap: 8px"
                } else {
                    "display: flex; gap: 8px"
                });
            }
            doc.attach(frame, parent, index)?;
            for id in &ordered {
                doc.move_node(*id, frame, None)?;
            }
            Ok(frame)
        })?;
        self.selection = vec![frame];
        Ok(frame)
    }

    /// Move a container's children into its parent and remove it.
    pub fn ungroup(&mut self, id: NodeId) -> Result<Vec<NodeId>> {
        let parent = self
            .doc
            .parent(id)
            .context("artboards cannot be ungrouped")?;
        let children = self.doc.children(id).to_vec();
        let index = self.doc.children(parent).iter().position(|c| *c == id);
        self.edit(None, |doc| {
            let mut at = index;
            for child in &children {
                doc.move_node(*child, parent, at)?;
                at = at.map(|i| i + 1);
            }
            doc.remove(id)?;
            Ok(())
        })?;
        self.selection = children.clone();
        Ok(children)
    }

    /// Turn a frame into a flex container (Shift+A).
    pub fn add_auto_layout(&mut self, id: NodeId) -> Result<()> {
        let computed = self
            .doc
            .get(id)
            .map(|n| n.style.computed())
            .context("node missing")?;
        if computed.display == crate::model::style::Display::Flex {
            return self.set_styles(
                &[id],
                &[("display".to_owned(), Some("block".to_owned()))],
                None,
            );
        }
        self.set_styles(
            &[id],
            &[
                ("display".to_owned(), Some("flex".to_owned())),
                ("flex-direction".to_owned(), Some("column".to_owned())),
                ("gap".to_owned(), Some("8px".to_owned())),
            ],
            None,
        )
    }

    /// Move a node one step forward (+1) or backward (-1) among siblings.
    pub fn reorder(&mut self, id: NodeId, delta: i32) -> Result<()> {
        if let Some((page, index)) = self.doc.artboard_index(id) {
            let len = self.doc.pages[page].artboards.len();
            let target = (index as i32 + delta).clamp(0, len as i32 - 1) as usize;
            return self.edit(None, |doc| {
                let artboard = doc.pages[page].artboards.remove(index);
                doc.pages[page].artboards.insert(target, artboard);
                Ok(())
            });
        }
        let parent = self.doc.parent(id).context("node has no parent")?;
        let siblings = self.doc.children(parent);
        let index = siblings.iter().position(|c| *c == id).context("missing")? as i32;
        let target = (index + delta).clamp(0, siblings.len() as i32 - 1) as usize;
        self.edit(None, |doc| {
            doc.detach(id);
            doc.attach(id, parent, Some(target))?;
            Ok(())
        })
    }

    /// Nudge absolutely positioned nodes and artboards by document pixels.
    pub fn nudge(&mut self, ids: &[NodeId], dx: f32, dy: f32) -> Result<()> {
        let ids = self.topmost(ids);
        self.edit(Some("nudge"), |doc| {
            for id in &ids {
                if let Some(artboard) = doc.artboard_mut(*id) {
                    artboard.x += dx;
                    artboard.y += dy;
                    continue;
                }
                let Some(node) = doc.get_mut(*id) else {
                    continue;
                };
                let computed = node.style.computed();
                if computed.position != Position::Absolute {
                    continue;
                }
                let left = computed.inset[3].px().unwrap_or(0.0) + dx;
                let top = computed.inset[0].px().unwrap_or(0.0) + dy;
                node.style.set("left", &format!("{}px", fmt_num(left)));
                node.style.set("top", &format!("{}px", fmt_num(top)));
            }
            Ok(())
        })
    }

    /// Add a page.
    pub fn add_page(&mut self) -> Result<usize> {
        let index = self.doc.pages.len();
        self.edit(None, |doc| {
            doc.pages
                .push(crate::model::Page::new(format!("Page {}", index + 1)));
            Ok(())
        })?;
        self.page = index;
        self.selection.clear();
        Ok(index)
    }

    /// Rename a page.
    pub fn rename_page(&mut self, page: usize, name: &str) -> Result<()> {
        self.edit(Some("rename-page"), |doc| {
            doc.pages.get_mut(page).context("no such page")?.name = name.to_owned();
            Ok(())
        })
    }

    /// Delete a page (keeps at least one).
    pub fn delete_page(&mut self, page: usize) -> Result<()> {
        if self.doc.pages.len() <= 1 {
            bail!("a project needs at least one page");
        }
        self.edit(None, |doc| {
            let removed = doc.pages.remove(page);
            for artboard in removed.artboards {
                // Detach so prune collects the subtree.
                let _ = artboard;
            }
            Ok(())
        })?;
        self.page = self.page.min(self.doc.pages.len() - 1);
        self.selection.clear();
        Ok(())
    }

    /// Serialized HTML of nodes (with ids, for agents) or clean code.
    #[must_use]
    pub fn html_of(&self, id: NodeId, with_ids: bool) -> String {
        let options = if with_ids {
            ExportOptions {
                editor_attrs: true,
                include_hidden: true,
            }
        } else {
            ExportOptions::CODE
        };
        to_html(&self.doc, id, options)
    }

    /// Connect two endpoints on the current page (or the page of an attached layer).
    pub fn connect(&mut self, from: Endpoint, to: Endpoint) -> Result<u64> {
        let page = [from, to]
            .iter()
            .filter_map(|e| e.node())
            .find_map(|n| self.doc.artboard_of(n).map(|a| a.root))
            .and_then(|root| self.doc.artboard_index(root))
            .map_or(self.page, |(page, _)| page);
        self.edit(None, |doc| Ok(doc.add_connection(page, from, to)?))
    }

    /// Change a connection's label, color, routing, or arrowheads.
    pub fn update_connection(
        &mut self,
        id: u64,
        label: Option<&str>,
        color: Option<&str>,
        style: Option<ConnectorStyle>,
        heads: Option<ArrowHeads>,
        key: Option<&str>,
    ) -> Result<()> {
        if let Some(color) = color
            && crate::model::Color::parse_loose(color).is_none()
        {
            bail!("invalid color {color:?}");
        }
        self.edit(key, |doc| {
            let connection = doc
                .connection_mut(id)
                .ok_or_else(|| anyhow!("connection {id} does not exist"))?;
            if let Some(label) = label {
                connection.label = label.trim().to_owned();
            }
            if let Some(color) = color.and_then(crate::model::Color::parse_loose) {
                connection.color = color.to_css();
            }
            if let Some(style) = style {
                connection.style = style;
            }
            if let Some(heads) = heads {
                connection.heads = heads;
            }
            Ok(())
        })
    }

    /// Delete a connection.
    pub fn delete_connection(&mut self, id: u64) -> Result<()> {
        self.edit(None, |doc| {
            if doc.remove_connection(id) {
                Ok(())
            } else {
                bail!("connection {id} does not exist")
            }
        })
    }

    /// Insert an absolutely positioned vector (inline SVG) into a parent,
    /// making the parent a positioning context so browsers place it the same.
    pub fn insert_vector(&mut self, parent: NodeId, svg: &str) -> Result<NodeId> {
        let parent_static = self
            .doc
            .get(parent)
            .is_some_and(|p| p.style.computed().position == Position::Static);
        let ids = self.edit(None, |doc| {
            if parent_static && let Some(node) = doc.get_mut(parent) {
                node.style.set("position", "relative");
            }
            let roots = parse_fragment(doc, svg, ImportOptions { keep_ids: false });
            let [root] = roots.as_slice() else {
                bail!("a vector must be a single <svg> element");
            };
            doc.attach(*root, parent, None)?;
            Ok(*root)
        })?;
        self.selection = vec![ids];
        Ok(ids)
    }

    /// Recolor or re-weight the strokes of vector layers.
    pub fn set_vector_stroke(
        &mut self,
        ids: &[NodeId],
        color: Option<&str>,
        width: Option<f32>,
        key: Option<&str>,
    ) -> Result<()> {
        if let Some(color) = color
            && crate::model::Color::parse_loose(color).is_none()
        {
            bail!("invalid color {color:?}");
        }
        let color = color
            .and_then(crate::model::Color::parse_loose)
            .map(|c| c.to_css());
        self.edit(key, |doc| {
            for id in ids {
                let node = doc
                    .get_mut(*id)
                    .ok_or_else(|| anyhow!("node {id} does not exist"))?;
                if let NodeKind::Svg(source) = &mut node.kind {
                    *source = crate::shapes::set_svg_stroke(source, color.as_deref(), width);
                }
            }
            Ok(())
        })
    }

    /// Move artboards and absolutely positioned layers by per-node offsets in
    /// one undoable step. Flow layers can't be positioned and are skipped.
    /// Returns how many moved.
    pub fn move_by(&mut self, moves: &[(NodeId, f32, f32)], key: Option<&str>) -> Result<usize> {
        let ids: Vec<NodeId> = moves.iter().map(|m| m.0).collect();
        let keep = self.topmost(&ids);
        let mut moved = 0;
        // (id, dx, dy, measured offset in the parent for missing insets)
        type Step = (NodeId, f32, f32, Option<(f32, f32)>);
        let mut plan: Vec<Step> = Vec::new();
        for (id, dx, dy) in moves {
            if !keep.contains(id) || (dx.abs() < 0.01 && dy.abs() < 0.01) {
                continue;
            }
            let offset = self.doc.parent(*id).and_then(|parent| {
                let r = self.measured.get(id)?;
                let p = self.measured.get(&parent)?;
                Some((r.x - p.x, r.y - p.y))
            });
            plan.push((*id, *dx, *dy, offset));
        }
        self.edit(key, |doc| {
            for (id, dx, dy, offset) in &plan {
                if let Some(artboard) = doc.artboard_mut(*id) {
                    artboard.x = (artboard.x + dx).round();
                    artboard.y = (artboard.y + dy).round();
                    moved += 1;
                    continue;
                }
                let Some(node) = doc.get(*id) else {
                    continue;
                };
                let computed = node.style.computed();
                if computed.position != Position::Absolute {
                    continue;
                }
                // Missing insets fall back to the measured offset in the parent.
                let left = computed.inset[3]
                    .px()
                    .or(offset.map(|o| o.0))
                    .unwrap_or(0.0);
                let top = computed.inset[0]
                    .px()
                    .or(offset.map(|o| o.1))
                    .unwrap_or(0.0);
                let Some(node) = doc.get_mut(*id) else {
                    continue;
                };
                node.style
                    .set("left", &format!("{}px", fmt_num((left + dx).round())));
                node.style
                    .set("top", &format!("{}px", fmt_num((top + dy).round())));
                if computed.inset[1].px().is_some() {
                    node.style.remove("right");
                }
                if computed.inset[2].px().is_some() {
                    node.style.remove("bottom");
                }
                moved += 1;
            }
            Ok(())
        })?;
        Ok(moved)
    }

    fn movable(&self, ids: &[NodeId]) -> Vec<(NodeId, Rect)> {
        self.topmost(ids)
            .into_iter()
            .filter(|id| {
                self.doc.is_artboard(*id)
                    || self
                        .doc
                        .get(*id)
                        .is_some_and(|n| n.style.computed().position == Position::Absolute)
            })
            .filter_map(|id| Some((id, *self.measured.get(&id)?)))
            .collect()
    }

    /// Align layers to each other, or a single layer to its parent. Uses the
    /// measured layout. Returns how many moved.
    pub fn align(&mut self, ids: &[NodeId], align: Align) -> Result<usize> {
        let items = self.movable(ids);
        if items.is_empty() {
            bail!("select artboards or absolutely positioned layers to align");
        }
        let target = if let [(id, _)] = items.as_slice() {
            let parent = self
                .doc
                .parent(*id)
                .context("a single artboard has nothing to align to")?;
            *self
                .measured
                .get(&parent)
                .context("the parent has not been laid out yet")?
        } else {
            Rect::union_all(items.iter().map(|i| i.1)).context("nothing to align")?
        };
        let rects: Vec<Rect> = items.iter().map(|i| i.1).collect();
        let moves: Vec<(NodeId, f32, f32)> = items
            .iter()
            .zip(align_deltas(&rects, target, align))
            .map(|((id, _), (dx, dy))| (*id, dx, dy))
            .collect();
        self.move_by(&moves, None)
    }

    /// Space three or more layers evenly along an axis. Returns how many moved.
    pub fn distribute(&mut self, ids: &[NodeId], axis: Axis) -> Result<usize> {
        let items = self.movable(ids);
        if items.len() < 3 {
            bail!("select three or more artboards or absolutely positioned layers to distribute");
        }
        let rects: Vec<Rect> = items.iter().map(|i| i.1).collect();
        let moves: Vec<(NodeId, f32, f32)> = items
            .iter()
            .zip(distribute_deltas(&rects, axis))
            .map(|((id, _), (dx, dy))| (*id, dx, dy))
            .collect();
        self.move_by(&moves, None)
    }

    /// Make a layer a main component. Returns its name.
    pub fn create_component(&mut self, id: NodeId, name: Option<&str>) -> Result<String> {
        use crate::model::components::{COMPONENT_ATTR, INSTANCE_ATTR};
        let node = self.doc.get(id).context("layer does not exist")?;
        if node.is_text() {
            bail!("text runs can't be components; use their parent");
        }
        if self.doc.instance_root(id).is_some() {
            bail!("layers inside an instance can't become components; detach it first");
        }
        if node.attr(INSTANCE_ATTR).is_some() {
            bail!("detach the instance before making it a component");
        }
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| self.doc.display_name(id), ToOwned::to_owned);
        let label = name.clone();
        self.edit(None, |doc| {
            let node = doc.get_mut(id).context("layer does not exist")?;
            node.set_attr(COMPONENT_ATTR, &label);
            if node.name.is_none() {
                node.name = Some(label.clone());
            }
            Ok(())
        })?;
        Ok(name)
    }

    /// Insert an instance of a main component. Without a parent it goes next
    /// to the main (or becomes a new artboard when the main is an artboard).
    pub fn create_instance(&mut self, main: NodeId, target: InsertTarget) -> Result<NodeId> {
        if self.doc.component_name(main).is_none() {
            bail!("{main} is not a component");
        }
        if let Some(parent) = target.parent
            && self.doc.is_ancestor_or_self(main, parent)
        {
            bail!("a component can't contain an instance of itself");
        }
        let page = self.page;
        let next = self.next_artboard_position();
        let main_artboard = self.doc.is_artboard(main);
        let (parent, index) = match target.parent {
            Some(parent) => (Some(parent), target.index),
            None if main_artboard => (None, None),
            None => {
                let parent = self.doc.parent(main).context("component has no parent")?;
                let index = self.doc.children(parent).iter().position(|c| *c == main);
                (Some(parent), index.map(|i| i + 1))
            }
        };
        let id = self.edit(None, |doc| {
            let id = doc.instantiate(main)?;
            match parent {
                Some(parent) => doc.attach(id, parent, index)?,
                None => {
                    doc.add_artboard(page, id, next.0, next.1);
                }
            }
            Ok(id)
        })?;
        self.select([id]);
        Ok(id)
    }

    /// Turn an instance into ordinary layers.
    pub fn detach_instance(&mut self, id: NodeId) -> Result<()> {
        if self
            .doc
            .get(id)
            .and_then(|n| n.attr(crate::model::components::INSTANCE_ATTR))
            .is_none()
        {
            bail!("{id} is not an instance");
        }
        self.edit(None, |doc| {
            doc.detach_instance(id);
            Ok(())
        })
    }

    /// Make an instance match its main again.
    pub fn reset_overrides(&mut self, id: NodeId) -> Result<()> {
        self.edit(None, |doc| Ok(doc.reset_overrides(id)?))
    }

    /// Stop a layer being a main component (its instances become detached).
    pub fn remove_component(&mut self, id: NodeId) -> Result<()> {
        if self.doc.component_name(id).is_none() {
            bail!("{id} is not a component");
        }
        self.edit(None, |doc| {
            for instance in doc.instances_of(id) {
                doc.detach_instance(instance);
            }
            if let Some(node) = doc.get_mut(id) {
                node.set_attr(crate::model::components::COMPONENT_ATTR, "");
            }
            Ok(())
        })
    }

    /// Set (or clear, with `None`) the prototype link on a layer.
    pub fn set_link(
        &mut self,
        id: NodeId,
        target: Option<crate::model::prototype::LinkTarget>,
        transition: crate::model::prototype::Transition,
    ) -> Result<()> {
        use crate::model::prototype::{LINK_ATTR, LinkTarget, TRANSITION_ATTR, Transition};
        if self.doc.get(id).is_none_or(crate::model::Node::is_text) {
            bail!("links go on layers, not text runs");
        }
        if let Some(LinkTarget::Artboard(board)) = target
            && !self.doc.is_artboard(board)
        {
            bail!("{board} is not an artboard");
        }
        self.edit(None, |doc| {
            let node = doc.get_mut(id).context("layer does not exist")?;
            match target {
                Some(target) => {
                    node.set_attr(LINK_ATTR, &target.to_attr());
                    node.set_attr(
                        TRANSITION_ATTR,
                        if transition == Transition::Instant {
                            ""
                        } else {
                            transition.as_str()
                        },
                    );
                }
                None => {
                    node.set_attr(LINK_ATTR, "");
                    node.set_attr(TRANSITION_ATTR, "");
                }
            }
            Ok(())
        })
    }

    /// Create or update a design variable. Returns its normalized name.
    pub fn set_variable(&mut self, name: &str, value: &str) -> Result<String> {
        let name = crate::model::variables::normalize_name(name)
            .context("variable names need letters or digits")?;
        let value = value.trim().trim_end_matches(';').trim();
        if value.is_empty() {
            bail!("variable --{name} needs a value");
        }
        if value.contains(['{', '}', '<']) {
            bail!("variable values cannot contain braces or markup");
        }
        let mut probe = self.doc.variables.clone();
        probe.insert(name.clone(), value.to_owned());
        if crate::model::variables::resolve(value, &probe).is_none() {
            bail!("--{name} refers to a missing or circular variable");
        }
        let value = value.to_owned();
        let key = format!("variable-{name}");
        self.edit(Some(&key), |doc| {
            doc.variables.insert(name.clone(), value);
            Ok(())
        })?;
        Ok(name)
    }

    /// Rename a variable and every reference to it.
    pub fn rename_variable(&mut self, from: &str, to: &str) -> Result<String> {
        let to = crate::model::variables::normalize_name(to)
            .context("variable names need letters or digits")?;
        if !self.doc.variables.contains_key(from) {
            bail!("no variable --{from}");
        }
        if to == from {
            return Ok(to);
        }
        if self.doc.variables.contains_key(&to) {
            bail!("--{to} already exists");
        }
        let from = from.to_owned();
        self.edit(None, |doc| {
            if let Some(value) = doc.variables.remove(&from) {
                doc.variables.insert(to.clone(), value);
            }
            for value in doc.variables.values_mut() {
                *value = crate::model::variables::rename_references(value, &from, &to);
            }
            doc.map_style_values(|v| crate::model::variables::rename_references(v, &from, &to));
            Ok(())
        })?;
        Ok(to)
    }

    /// Delete a variable, replacing references with its value. Returns how
    /// many layers were detached.
    pub fn delete_variable(&mut self, name: &str) -> Result<usize> {
        let value = self
            .doc
            .variables
            .get(name)
            .cloned()
            .with_context(|| format!("no variable --{name}"))?;
        // Inline the declared value, so references to other variables survive.
        let resolved = value;
        let name = name.to_owned();
        self.edit(None, |doc| {
            doc.variables.remove(&name);
            for v in doc.variables.values_mut() {
                *v = crate::model::variables::inline_references(v, &name, &resolved);
            }
            Ok(doc.map_style_values(|v| {
                crate::model::variables::inline_references(v, &name, &resolved)
            }))
        })
    }

    /// Set a fixed size in document pixels.
    pub fn set_size(
        &mut self,
        id: NodeId,
        width: Option<f32>,
        height: Option<f32>,
        key: Option<&str>,
    ) -> Result<()> {
        let mut changes = Vec::new();
        if let Some(width) = width {
            changes.push((
                "width".to_owned(),
                Some(Length::Px(width.max(1.0).round()).to_css()),
            ));
        }
        if let Some(height) = height {
            changes.push((
                "height".to_owned(),
                Some(Length::Px(height.max(1.0).round()).to_css()),
            ));
        }
        self.set_styles(&[id], &changes, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> (Editor, NodeId) {
        let mut doc = Document::new();
        let board = doc.create_artboard(0, "Home", (0.0, 0.0), (800.0, 600.0));
        (Editor::new(doc), board)
    }

    #[test]
    fn images_are_copied_into_the_project_and_placed() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = Editor::open(dir.path()).unwrap();
        let board = editor
            .create_artboard("Block", (400.0, 300.0), None)
            .unwrap();
        let png = crate::assets::tests_png();
        // Into a block-flow frame: absolutely positioned at the drop point.
        let id = editor
            .import_image(
                &png,
                "Hero Shot.png",
                ImagePlacement {
                    parent: Some(board),
                    at: Some((40.0, 60.0)),
                    index: None,
                },
            )
            .unwrap();
        let node = editor.doc.get(id).unwrap();
        assert_eq!(node.attr("src"), Some("assets/hero-shot.png"));
        assert_eq!(node.style.get("left"), Some("40px"));
        assert!(dir.path().join("artboards/assets/hero-shot.png").exists());
        // On empty canvas: a new artboard sized to the image.
        let board2 = editor
            .import_image(
                &png,
                "logo.png",
                ImagePlacement {
                    at: Some((5000.0, 0.0)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(editor.doc.is_artboard(board2));
        assert_eq!(editor.doc.artboard_of(board2).unwrap().x, 5000.0);
        assert_eq!(
            editor.doc.get(board2).unwrap().style.get("width"),
            Some("1px")
        );
        assert!(editor.undo());
        assert!(!editor.doc.contains(board2));
        assert!(
            Editor::new(Document::new())
                .import_image(&png, "a.png", ImagePlacement::default())
                .is_err()
        );
    }

    #[test]
    fn component_edits_sync_to_instances_in_one_undo_step() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<div style=\"display: flex; padding: 8px\"><p>Hi</p></div>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let main = ids[0];
        assert_eq!(editor.create_component(main, Some("Chip")).unwrap(), "Chip");
        editor.rename(main, "Pill").unwrap();
        assert_eq!(editor.doc.component_name(main), Some("Pill"));
        let instance = editor
            .create_instance(main, InsertTarget::default())
            .unwrap();
        assert_eq!(editor.doc.parent(instance), Some(board));
        assert_eq!(
            editor.doc.children(board)[1],
            instance,
            "placed after the main"
        );
        editor
            .set_styles(&[main], &[("padding".into(), Some("20px".into()))], None)
            .unwrap();
        assert_eq!(
            editor.doc.get(instance).unwrap().style.get("padding"),
            Some("20px")
        );
        assert!(editor.undo());
        assert_eq!(
            editor.doc.get(instance).unwrap().style.get("padding"),
            Some("8px")
        );
        assert!(
            editor
                .create_component(editor.doc.children(instance)[0], None)
                .is_err()
        );
        assert!(
            editor
                .create_instance(
                    main,
                    InsertTarget {
                        parent: Some(main),
                        index: None
                    }
                )
                .is_err()
        );
        editor.detach_instance(instance).unwrap();
        assert!(editor.doc.instances_of(main).is_empty());
        let html = editor.html_of(main, false);
        assert!(
            !html.contains("data-component"),
            "code export strips component markers"
        );
    }

    #[test]
    fn variables_resolve_rename_and_detach() {
        let (mut editor, board) = editor();
        let name = editor.set_variable("--Brand", "#ff5a36").unwrap();
        assert_eq!(name, "brand");
        editor
            .set_styles(
                &[board],
                &[("background-color".into(), Some("var(--brand)".into()))],
                None,
            )
            .unwrap();
        let node = editor.doc.get(board).unwrap();
        assert_eq!(
            editor.doc.computed(node).background,
            crate::model::Color::parse("#ff5a36")
        );
        assert!(editor.set_variable("loop", "var(--loop)").is_err());
        assert!(editor.set_variable("bad", "red; } body {").is_err());
        assert_eq!(editor.doc.variable_usage("brand"), 1);
        editor.rename_variable("brand", "primary").unwrap();
        assert_eq!(
            editor.doc.get(board).unwrap().style.get("background-color"),
            Some("var(--primary)")
        );
        assert_eq!(editor.delete_variable("primary").unwrap(), 1);
        assert_eq!(
            editor.doc.get(board).unwrap().style.get("background-color"),
            Some("#ff5a36")
        );
        assert!(editor.doc.variables.is_empty());
        assert!(editor.undo());
        assert!(editor.doc.variables.contains_key("primary"));
    }

    #[test]
    fn layers_drop_before_inside_and_after() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<div>A</div><div><p>B1</p></div><p>C</p>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        editor.drop_layer(c, a, DropPosition::Before).unwrap();
        assert_eq!(editor.doc.children(board), &[c, a, b]);
        editor.drop_layer(c, b, DropPosition::After).unwrap();
        assert_eq!(editor.doc.children(board), &[a, b, c]);
        editor.drop_layer(a, b, DropPosition::Inside).unwrap();
        assert_eq!(editor.doc.parent(a), Some(b));
        assert_eq!(editor.doc.children(b).last(), Some(&a));
        // Not into itself, its descendants, or a text layer; artboards stay top level.
        assert!(editor.drop_layer(b, a, DropPosition::Inside).is_err());
        assert!(!editor.can_drop_layer(b, c, DropPosition::Inside));
        assert!(!editor.can_drop_layer(board, a, DropPosition::Before));
        let other = editor
            .create_artboard("Other", (100.0, 100.0), None)
            .unwrap();
        editor
            .drop_layer(other, board, DropPosition::Before)
            .unwrap();
        assert_eq!(editor.doc.pages[0].artboards[0].root, other);
        assert!(editor.undo());
        assert_eq!(editor.doc.pages[0].artboards[0].root, board);
    }

    #[test]
    fn edits_are_undoable_and_revisioned() {
        let (mut editor, board) = editor();
        let r0 = editor.revision;
        let ids = editor
            .insert_html(
                "<p>One</p><p>Two</p>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(editor.selection, ids);
        assert!(editor.revision > r0);
        editor
            .set_styles(&ids, &[("color".into(), Some("red".into()))], None)
            .unwrap();
        assert_eq!(
            editor.doc.get(ids[1]).unwrap().style.get("color"),
            Some("red")
        );
        assert!(editor.undo());
        assert_eq!(editor.doc.get(ids[1]).unwrap().style.get("color"), None);
        assert!(editor.redo());
        assert_eq!(
            editor.doc.get(ids[1]).unwrap().style.get("color"),
            Some("red")
        );
    }

    #[test]
    fn failed_edits_leave_the_document_untouched() {
        let (mut editor, board) = editor();
        let before = editor.doc.clone();
        let revision = editor.revision;
        assert!(editor.move_node(board, board, None).is_err());
        assert!(
            editor
                .set_styles(
                    &[NodeId(999)],
                    &[("color".into(), Some("red".into()))],
                    None
                )
                .is_err()
        );
        assert_eq!(editor.doc, before);
        assert_eq!(editor.revision, revision);
    }

    #[test]
    fn group_ungroup_and_auto_layout() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<p>A</p><p>B</p><p>C</p>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let frame = editor.group(&[ids[2], ids[0]]).unwrap();
        assert_eq!(editor.doc.children(frame), &[ids[0], ids[2]]);
        assert_eq!(editor.doc.children(board), &[frame, ids[1]]);
        let children = editor.ungroup(frame).unwrap();
        assert_eq!(editor.doc.children(board), &[ids[0], ids[2], ids[1]]);
        assert_eq!(children, vec![ids[0], ids[2]]);
        editor.add_auto_layout(board).unwrap();
        assert_eq!(
            editor.doc.get(board).unwrap().style.get("display"),
            Some("flex")
        );
    }

    #[test]
    fn top_level_html_becomes_artboards_and_replace_keeps_slot() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<section style=\"width: 300px\">Hi</section>",
                InsertTarget::default(),
            )
            .unwrap();
        assert!(editor.doc.is_artboard(ids[0]));
        assert_eq!(editor.doc.artboard_of(ids[0]).unwrap().x, 880.0);
        let replaced = editor
            .replace_html(board, "<main style=\"width: 500px\"><h1>New</h1></main>")
            .unwrap();
        assert!(editor.doc.is_artboard(replaced[0]));
        assert_eq!(editor.doc.pages[0].artboards[0].root, replaced[0]);
    }

    #[test]
    fn connections_and_vectors_are_undoable_edits() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<div>A</div><div>B</div>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let c = editor
            .connect(Endpoint::Node(ids[0]), Endpoint::Node(ids[1]))
            .unwrap();
        editor
            .update_connection(
                c,
                Some("next"),
                Some("#ff0000"),
                Some(ConnectorStyle::Elbow),
                None,
                None,
            )
            .unwrap();
        let connection = editor.doc.connection(c).unwrap();
        assert_eq!(
            (connection.label.as_str(), connection.color.as_str()),
            ("next", "#ff0000")
        );
        assert!(
            editor
                .update_connection(c, None, Some("nope"), None, None, None)
                .is_err()
        );
        let svg = crate::shapes::line_svg(
            (0.0, 0.0),
            (50.0, 0.0),
            &crate::shapes::Stroke::default(),
            true,
        );
        let vector = editor.insert_vector(ids[0], &svg).unwrap();
        assert_eq!(
            editor.doc.get(ids[0]).unwrap().style.get("position"),
            Some("relative")
        );
        editor
            .set_vector_stroke(&[vector], Some("#00ff00"), Some(5.0), None)
            .unwrap();
        let NodeKind::Svg(source) = &editor.doc.get(vector).unwrap().kind else {
            panic!("svg")
        };
        assert!(source.contains("stroke=\"#00ff00\""));
        editor.delete_connection(c).unwrap();
        assert!(editor.doc.connection(c).is_none());
        editor.undo();
        assert!(editor.doc.connection(c).is_some());
    }

    #[test]
    fn delete_ignores_descendants_of_deleted_nodes() {
        let (mut editor, board) = editor();
        let ids = editor
            .insert_html(
                "<div><p>x</p></div>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        let child = editor.doc.children(ids[0])[0];
        assert_eq!(editor.delete(&[child, ids[0]]).unwrap(), 1);
        assert!(editor.doc.children(board).is_empty());
    }
}
