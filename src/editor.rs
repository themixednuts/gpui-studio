//! Editor state and every document operation.
//!
//! The canvas, panels, keyboard shortcuts, and MCP agents all call these
//! methods, so a change made by a person and the same change made by an agent
//! take one path: snapshot for undo, apply, bump the revision, autosave.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::comments::Comments;
use crate::history::History;
use crate::model::html::{ExportOptions, ImportOptions, parse_fragment, to_html};
use crate::model::style::{Position, fmt_num};
use crate::model::{Document, Length, NodeId, NodeKind};
use crate::presets;
use crate::project::Project;

const AUTOSAVE_QUIET: Duration = Duration::from_millis(400);

/// Canvas tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Select, move, resize.
    Select,
    /// Draw frames (artboards on empty canvas).
    Frame,
    /// Draw rectangles.
    Rectangle,
    /// Place text.
    Text,
    /// Pan the canvas.
    Hand,
    /// Drop comment pins.
    Comment,
}

impl Tool {
    /// Tooltip label with shortcut.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select (V)",
            Self::Frame => "Frame (F)",
            Self::Rectangle => "Rectangle (R)",
            Self::Text => "Text (T)",
            Self::Hand => "Hand (H)",
            Self::Comment => "Comment (C)",
        }
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
        match project.sync_external(&mut self.doc) {
            Ok(changed) if !changed.is_empty() => {
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
            doc.pages.push(crate::model::Page {
                name: format!("Page {}", index + 1),
                artboards: Vec::new(),
            });
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
