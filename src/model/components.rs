//! Components: main components and the instances synced from them.
//!
//! Everything is plain HTML. A main component is any layer carrying
//! `data-component="Name"`. An instance's root carries `data-instance="n42"`
//! (the main's id) and each element inside it `data-ref="n57"`, naming the
//! node of the main it mirrors.
//!
//! Syncing is a three-way merge against the document before the edit: when a
//! property of the main changes from A to B, every instance whose value is
//! still A takes B. Anything an instance changed itself is an override and
//! is kept. Children added to the main appear in every instance; children
//! removed from the main disappear; reorders follow the main.

use super::{Document, EditError, Node, NodeId, NodeKind};

/// Marks a main component; the value is its name.
pub const COMPONENT_ATTR: &str = "data-component";
/// Marks an instance root; the value is the main component's id.
pub const INSTANCE_ATTR: &str = "data-instance";
/// Inside an instance, the id of the main's node this element mirrors.
pub const REF_ATTR: &str = "data-ref";
/// Editor-only attributes stripped from code export.
pub const COMPONENT_ATTRS: [&str; 3] = [COMPONENT_ATTR, INSTANCE_ATTR, REF_ATTR];

/// Root properties each instance sets for itself (placement in its parent).
const ROOT_LOCAL: &[&str] = &[
    "position",
    "left",
    "top",
    "right",
    "bottom",
    "inset",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "z-index",
    "grid-column",
    "grid-row",
    "grid-area",
    "flex",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "align-self",
    "justify-self",
    "order",
];

const MAX_ROUNDS: usize = 8;

fn is_root_local(property: &str) -> bool {
    ROOT_LOCAL.contains(&property)
}

impl Document {
    /// The component name when `id` is a main component.
    #[must_use]
    pub fn component_name(&self, id: NodeId) -> Option<&str> {
        self.get(id)?.attr(COMPONENT_ATTR)
    }

    /// Every main component, in page and document order.
    #[must_use]
    pub fn components(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        for artboard in self.artboards() {
            for id in self.descendants(artboard.root) {
                if self.component_name(id).is_some() && self.instance_root(id).is_none() {
                    out.push(id);
                }
            }
        }
        out
    }

    /// The main component an instance root points at (when it still exists).
    #[must_use]
    pub fn main_of(&self, instance: NodeId) -> Option<NodeId> {
        let main = NodeId::parse(self.get(instance)?.attr(INSTANCE_ATTR)?)?;
        self.component_name(main).is_some().then_some(main)
    }

    /// The nearest instance root at or above `id`.
    #[must_use]
    pub fn instance_root(&self, id: NodeId) -> Option<NodeId> {
        std::iter::once(id).chain(self.ancestors(id)).find(|n| {
            self.get(*n)
                .is_some_and(|node| node.attr(INSTANCE_ATTR).is_some())
        })
    }

    /// Whether `id` is an instance root that syncs on its own (not one nested
    /// inside another instance, which syncs through its parent's main).
    #[must_use]
    pub fn is_synced_instance(&self, id: NodeId) -> bool {
        self.get(id)
            .is_some_and(|n| n.attr(INSTANCE_ATTR).is_some())
            && self
                .ancestors(id)
                .iter()
                .all(|a| self.get(*a).is_none_or(|n| n.attr(INSTANCE_ATTR).is_none()))
    }

    /// Synced instances of a main component, in document order.
    #[must_use]
    pub fn instances_of(&self, main: NodeId) -> Vec<NodeId> {
        let key = main.to_string();
        let mut out = Vec::new();
        for artboard in self.artboards() {
            for id in self.descendants(artboard.root) {
                if self.get(id).and_then(|n| n.attr(INSTANCE_ATTR)) == Some(key.as_str())
                    && self.is_synced_instance(id)
                    && !self.is_ancestor_or_self(main, id)
                {
                    out.push(id);
                }
            }
        }
        out
    }

    /// Copy a main's node as an instance node (fresh ids, `data-ref` links).
    fn copy_for_instance(&mut self, source: NodeId) -> Result<NodeId, EditError> {
        let original = self
            .get(source)
            .cloned()
            .ok_or(EditError::Missing(source))?;
        let id = self.alloc_id();
        let mut copy = original.clone();
        copy.id = id;
        copy.parent = None;
        copy.children = Vec::new();
        copy.set_attr(COMPONENT_ATTR, "");
        if !copy.is_text() {
            copy.set_attr(REF_ATTR, &source.to_string());
        }
        self.add(copy);
        for child in original.children {
            let child_copy = self.copy_for_instance(child)?;
            self.attach(child_copy, id, None)?;
        }
        Ok(id)
    }

    /// A new detached instance of a main component.
    pub fn instantiate(&mut self, main: NodeId) -> Result<NodeId, EditError> {
        if self.component_name(main).is_none() {
            return Err(EditError::Missing(main));
        }
        let name = self.display_name(main);
        let id = self.copy_for_instance(main)?;
        if let Some(node) = self.get_mut(id) {
            node.set_attr(REF_ATTR, "");
            node.set_attr(INSTANCE_ATTR, &main.to_string());
            node.name = Some(name);
            node.locked = false;
        }
        Ok(id)
    }

    /// Turn an instance back into ordinary layers. Nested instances inside it
    /// are relinked to their own mains so they keep syncing.
    pub fn detach_instance(&mut self, instance: NodeId) {
        let nested: Vec<NodeId> = self
            .descendants(instance)
            .into_iter()
            .filter(|id| {
                *id != instance
                    && self
                        .get(*id)
                        .is_some_and(|n| n.attr(INSTANCE_ATTR).is_some())
            })
            .collect();
        for id in self.descendants(instance) {
            if let Some(node) = self.get_mut(id) {
                node.set_attr(REF_ATTR, "");
            }
        }
        if let Some(node) = self.get_mut(instance) {
            node.set_attr(INSTANCE_ATTR, "");
        }
        for id in nested {
            if self.is_synced_instance(id)
                && let Some(main) = self.main_of(id)
            {
                self.relink(id, main);
            }
        }
    }

    /// Re-derive `data-ref` links by matching children positionally.
    fn relink(&mut self, instance: NodeId, main: NodeId) {
        let pairs: Vec<(NodeId, NodeId)> = self
            .children(instance)
            .iter()
            .copied()
            .zip(self.children(main).iter().copied())
            .collect();
        for (i, m) in pairs {
            let same_kind = match (self.get(i), self.get(m)) {
                (Some(a), Some(b)) => a.tag() == b.tag(),
                _ => false,
            };
            if !same_kind {
                continue;
            }
            if let Some(node) = self.get_mut(i)
                && !node.is_text()
            {
                node.set_attr(REF_ATTR, &m.to_string());
            }
            self.relink(i, m);
        }
    }

    /// Structural signature of a node for override detection.
    fn signature(&self, id: NodeId, root: bool, out: &mut String) {
        let Some(node) = self.get(id) else {
            return;
        };
        out.push('<');
        match &node.kind {
            NodeKind::Element { tag } => out.push_str(tag),
            NodeKind::Text(text) => {
                out.push('"');
                out.push_str(text);
            }
            NodeKind::Svg(source) => out.push_str(source),
        }
        if !root {
            out.push_str(node.name.as_deref().unwrap_or(""));
            if node.hidden {
                out.push_str(" hidden");
            }
        }
        for (name, value) in &node.attrs {
            if COMPONENT_ATTRS.contains(&name.as_str()) {
                continue;
            }
            out.push_str(&format!(" {name}={value}"));
        }
        out.push_str(" {");
        for (property, value) in node.style.iter() {
            if root && is_root_local(property) {
                continue;
            }
            out.push_str(&format!("{property}:{value};"));
        }
        out.push('}');
        for child in &node.children {
            self.signature(*child, false, out);
        }
        out.push('>');
    }

    /// Whether an instance differs from its main (beyond its own placement).
    #[must_use]
    pub fn has_overrides(&self, instance: NodeId) -> bool {
        let Some(main) = self.main_of(instance) else {
            return false;
        };
        let (mut a, mut b) = (String::new(), String::new());
        self.signature(instance, true, &mut a);
        self.signature(main, true, &mut b);
        a != b
    }

    /// Drop every override: the instance matches its main again, keeping only
    /// its own placement.
    pub fn reset_overrides(&mut self, instance: NodeId) -> Result<(), EditError> {
        let main = self.main_of(instance).ok_or(EditError::Missing(instance))?;
        let main_node = self.get(main).cloned().ok_or(EditError::Missing(main))?;
        for child in self.children(instance).to_vec() {
            self.remove(child)?;
        }
        for child in main_node.children.clone() {
            let copy = self.copy_for_instance(child)?;
            self.attach(copy, instance, None)?;
        }
        if let Some(node) = self.get_mut(instance) {
            let local: Vec<(String, String)> = node
                .style
                .iter()
                .filter(|(p, _)| is_root_local(p))
                .map(|(p, v)| (p.to_owned(), v.to_owned()))
                .collect();
            let mut style = main_node.style.clone();
            for property in ROOT_LOCAL {
                style.remove(property);
            }
            for (property, value) in local {
                style.set(&property, &value);
            }
            node.style = style;
            node.kind = main_node.kind.clone();
            let keep = node.attr(INSTANCE_ATTR).map(ToOwned::to_owned);
            node.attrs = main_node
                .attrs
                .iter()
                .filter(|(n, _)| !COMPONENT_ATTRS.contains(&n.as_str()))
                .cloned()
                .collect();
            if let Some(main_id) = keep {
                node.set_attr(INSTANCE_ATTR, &main_id);
            }
        }
        Ok(())
    }

    fn subtree_changed(&self, before: &Document, root: NodeId) -> bool {
        self.descendants(root)
            .into_iter()
            .any(|id| self.get(id) != before.get(id))
    }

    /// Propagate edits of main components into their instances. Returns
    /// whether any instance changed.
    pub fn sync_components(&mut self, before: &Document) -> bool {
        let mut changed = false;
        for _ in 0..MAX_ROUNDS {
            let mut round = false;
            for main in self.components() {
                if before.get(main).is_none() || !self.subtree_changed(before, main) {
                    continue;
                }
                for instance in self.instances_of(main) {
                    round |= self.sync_node(before, main, instance, true);
                }
            }
            if !round {
                break;
            }
            changed = true;
        }
        changed
    }

    fn sync_node(&mut self, before: &Document, main: NodeId, instance: NodeId, root: bool) -> bool {
        let (Some(new), Some(old), Some(mut node)) = (
            self.get(main).cloned(),
            before.get(main).cloned(),
            self.get(instance).cloned(),
        ) else {
            return false;
        };
        let mut changed = merge_node(&old, &new, &mut node, root);
        if changed && let Some(slot) = self.get_mut(instance) {
            *slot = node;
        }
        let text_block = self.is_text_layer(main) || before.is_text_layer(main);
        if text_block {
            let (mut old_sig, mut new_sig, mut inst_sig) =
                (String::new(), String::new(), String::new());
            for child in &old.children {
                before.signature(*child, false, &mut old_sig);
            }
            for child in &new.children {
                self.signature(*child, false, &mut new_sig);
            }
            for child in self.children(instance).to_vec() {
                self.signature(child, false, &mut inst_sig);
            }
            if old_sig != new_sig && inst_sig == old_sig {
                for child in self.children(instance).to_vec() {
                    let _ = self.remove(child);
                }
                for child in new.children {
                    if let Ok(copy) = self.copy_for_instance(child) {
                        let _ = self.attach(copy, instance, None);
                    }
                }
                changed = true;
            }
            return changed;
        }
        let ref_of = |doc: &Document, id: NodeId| {
            doc.get(id)
                .and_then(|n| n.attr(REF_ATTR))
                .and_then(NodeId::parse)
        };
        // Removed from the main: remove from the instance.
        for child in self.children(instance).to_vec() {
            if let Some(r) = ref_of(self, child)
                && old.children.contains(&r)
                && !new.children.contains(&r)
            {
                let _ = self.remove(child);
                changed = true;
            }
        }
        // Shared children sync recursively; new ones are added.
        for main_child in &new.children {
            if self.get(*main_child).is_some_and(Node::is_text) {
                continue;
            }
            let existing = self
                .children(instance)
                .iter()
                .copied()
                .find(|c| ref_of(self, *c) == Some(*main_child));
            match existing {
                Some(child) => {
                    changed |= self.sync_node(before, *main_child, child, false);
                }
                None if !old.children.contains(main_child) => {
                    if let Ok(copy) = self.copy_for_instance(*main_child) {
                        let _ = self.attach(copy, instance, None);
                        changed = true;
                    }
                }
                None => {}
            }
        }
        // Follow the main's order for linked children; others keep theirs after.
        let current = self.children(instance).to_vec();
        let mut ordered: Vec<NodeId> = new
            .children
            .iter()
            .filter_map(|m| {
                current
                    .iter()
                    .copied()
                    .find(|c| ref_of(self, *c) == Some(*m))
            })
            .collect();
        let linked: Vec<NodeId> = current
            .iter()
            .copied()
            .filter(|c| ordered.contains(c))
            .collect();
        if linked != ordered {
            let rest: Vec<NodeId> = current
                .iter()
                .copied()
                .filter(|c| !ordered.contains(c))
                .collect();
            ordered.extend(rest);
            if let Some(node) = self.get_mut(instance) {
                node.children = ordered;
            }
            changed = true;
        }
        changed
    }
}

/// Three-way merge of one node's own properties. Returns whether it changed.
fn merge_node(old: &Node, new: &Node, node: &mut Node, root: bool) -> bool {
    let mut changed = false;
    if old.kind != new.kind && node.kind == old.kind && !new.is_text() {
        node.kind = new.kind.clone();
        changed = true;
    }
    if !root && old.name != new.name && node.name == old.name {
        node.name.clone_from(&new.name);
        changed = true;
    }
    if old.hidden != new.hidden && node.hidden == old.hidden {
        node.hidden = new.hidden;
        changed = true;
    }
    let mut names: Vec<&str> = old.attrs.iter().map(|(n, _)| n.as_str()).collect();
    names.extend(new.attrs.iter().map(|(n, _)| n.as_str()));
    names.sort_unstable();
    names.dedup();
    for name in names {
        if COMPONENT_ATTRS.contains(&name) {
            continue;
        }
        let (a, b) = (old.attr(name), new.attr(name));
        if a != b && node.attr(name) == a {
            node.set_attr(name, b.unwrap_or(""));
            changed = true;
        }
    }
    let mut properties: Vec<&str> = old.style.iter().map(|(p, _)| p).collect();
    properties.extend(new.style.iter().map(|(p, _)| p));
    properties.sort_unstable();
    properties.dedup();
    for property in properties {
        if root && is_root_local(property) {
            continue;
        }
        let (a, b) = (old.style.get(property), new.style.get(property));
        if a != b && node.style.get(property) == a {
            match b {
                Some(value) => node.style.set(property, value),
                None => node.style.remove(property),
            }
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::html::{ImportOptions, parse_fragment};

    fn setup() -> (Document, NodeId, NodeId) {
        let mut doc = Document::new();
        let board = doc.create_artboard(0, "Board", (0.0, 0.0), (800.0, 600.0));
        let roots = parse_fragment(
            &mut doc,
            r#"<button data-component="Button" style="display: flex; gap: 4px; padding: 8px; background-color: #111; color: #fff"><span>Label</span><i style="color: red">!</i></button>"#,
            ImportOptions { keep_ids: false },
        );
        let main = roots[0];
        doc.attach(main, board, None).unwrap();
        let instance = doc.instantiate(main).unwrap();
        doc.attach(instance, board, None).unwrap();
        (doc, main, instance)
    }

    fn child(doc: &Document, id: NodeId, index: usize) -> NodeId {
        doc.children(id)[index]
    }

    #[test]
    fn instances_follow_the_main_except_overrides() {
        let (mut doc, main, instance) = setup();
        assert_eq!(doc.components(), vec![main]);
        assert_eq!(doc.instances_of(main), vec![instance]);
        assert_eq!(doc.main_of(instance), Some(main));
        assert!(!doc.has_overrides(instance));

        // Override the instance's background and label.
        let label = child(&doc, instance, 0);
        doc.get_mut(instance)
            .unwrap()
            .style
            .set("background-color", "#f00");
        doc.set_text(label, "Buy").unwrap();
        doc.get_mut(instance).unwrap().style.set("left", "40px");
        assert!(doc.has_overrides(instance));

        // Edit the main: padding, background, label, and a new child.
        let before = doc.clone();
        let main_label = child(&doc, main, 0);
        doc.get_mut(main).unwrap().style.set("padding", "12px");
        doc.get_mut(main)
            .unwrap()
            .style
            .set("background-color", "#222");
        doc.set_text(main_label, "Go").unwrap();
        let extra = doc.new_element("b");
        doc.attach(extra, main, None).unwrap();
        assert!(doc.sync_components(&before));

        let node = doc.get(instance).unwrap();
        assert_eq!(node.style.get("padding"), Some("12px"), "inherited change");
        assert_eq!(
            node.style.get("background-color"),
            Some("#f00"),
            "override kept"
        );
        assert_eq!(node.style.get("left"), Some("40px"), "placement is local");
        assert_eq!(
            doc.text_content(child(&doc, instance, 0)).as_deref(),
            Some("Buy")
        );
        assert_eq!(doc.children(instance).len(), 3, "new child added");

        // Removing and reordering in the main follows through.
        let before = doc.clone();
        let icon = child(&doc, main, 1);
        doc.remove(icon).unwrap();
        doc.get_mut(main).unwrap().children.reverse();
        assert!(doc.sync_components(&before));
        let tags: Vec<String> = doc
            .children(instance)
            .iter()
            .map(|c| doc.get(*c).unwrap().tag().to_owned())
            .collect();
        assert_eq!(tags, ["b", "span"]);

        doc.reset_overrides(instance).unwrap();
        assert!(!doc.has_overrides(instance));
        assert_eq!(doc.get(instance).unwrap().style.get("left"), Some("40px"));
        assert_eq!(doc.text_content(instance).as_deref(), Some("Go"));
    }

    #[test]
    fn nested_instances_sync_through_their_parent_and_detach_cleanly() {
        let (mut doc, button, _) = setup();
        let board = doc.pages[0].artboards[0].root;
        // A Card main that contains a Button instance.
        let card = doc.new_element("div");
        doc.attach(card, board, None).unwrap();
        doc.get_mut(card).unwrap().set_attr(COMPONENT_ATTR, "Card");
        let inner = doc.instantiate(button).unwrap();
        doc.attach(inner, card, None).unwrap();
        let card_instance = doc.instantiate(card).unwrap();
        doc.attach(card_instance, board, None).unwrap();
        let nested = child(&doc, card_instance, 0);
        assert!(!doc.is_synced_instance(nested));

        // Editing the Button main reaches the button inside the Card instance.
        let before = doc.clone();
        doc.get_mut(button)
            .unwrap()
            .style
            .set("border-radius", "9px");
        assert!(doc.sync_components(&before));
        assert_eq!(
            doc.get(inner).unwrap().style.get("border-radius"),
            Some("9px")
        );
        assert_eq!(
            doc.get(nested).unwrap().style.get("border-radius"),
            Some("9px")
        );

        // Detaching the Card instance leaves a Button instance that still syncs.
        doc.detach_instance(card_instance);
        assert!(
            doc.get(card_instance)
                .unwrap()
                .attr(INSTANCE_ATTR)
                .is_none()
        );
        assert!(doc.is_synced_instance(nested));
        let before = doc.clone();
        doc.get_mut(button)
            .unwrap()
            .style
            .set("border-radius", "2px");
        doc.sync_components(&before);
        assert_eq!(
            doc.get(nested).unwrap().style.get("border-radius"),
            Some("2px")
        );
    }
}
