//! Semantic Layers-tree projection shared by the UI and MCP context.

use std::collections::{BTreeMap, BTreeSet};

use gpui_mcp::{Role, UiNode, UiTree};
use serde::{Deserialize, Serialize};

/// Visual category used to choose a stable Layers icon.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    /// Reusable component instance; descendants belong to its own document tab.
    Component,
    /// Layout or semantic container.
    Frame,
    /// Static text.
    Text,
    /// Interactive control.
    Control,
    /// Image or illustration.
    Image,
}

/// One ordered row in the project Layers tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerRow {
    /// Fully namespaced runtime ID.
    pub runtime_id: String,
    /// Stable authored ID.
    pub authored_id: String,
    /// Human-readable name.
    pub label: String,
    /// Icon and editing category.
    pub kind: LayerKind,
    /// Visible nesting depth.
    pub depth: usize,
    /// Whether the row has project-document children.
    pub expandable: bool,
    /// Parent project row, if any.
    pub parent: Option<String>,
}

/// Ordered project graph derived from the live semantic snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerTree {
    roots: Vec<String>,
    rows: BTreeMap<String, LayerRow>,
    children: BTreeMap<String, Vec<String>>,
}

impl LayerTree {
    /// Build a project-only tree from a namespaced embedded HTML document.
    ///
    /// Technical HTML roots are omitted. A component instance remains a leaf here even when its
    /// native semantic subtree is visible to MCP; that subtree belongs to the component tab.
    #[must_use]
    pub fn from_semantics(tree: &UiTree, namespace: &str) -> Self {
        let prefix = format!("{namespace}--");
        let technical_root = format!("{prefix}html-root");
        let roots = tree
            .nodes
            .get(&technical_root)
            .map(|node| project_children(tree, node, &prefix))
            .filter(|roots| !roots.is_empty())
            .unwrap_or_else(|| {
                tree.nodes
                    .values()
                    .filter(|node| {
                        node.id.starts_with(&prefix)
                            && node
                                .parent
                                .as_ref()
                                .is_none_or(|parent| !parent.starts_with(&prefix))
                    })
                    .map(|node| node.id.clone())
                    .collect()
            });
        let mut model = Self {
            roots,
            rows: BTreeMap::new(),
            children: BTreeMap::new(),
        };
        for root in model.roots.clone() {
            model.visit(tree, &prefix, &root, None, 0);
        }
        model
    }

    /// Ordered visible rows for the current expansion set.
    #[must_use]
    pub fn visible_rows(&self, expanded: &BTreeSet<String>) -> Vec<LayerRow> {
        let mut output = Vec::new();
        for root in &self.roots {
            self.append_visible(root, expanded, &mut output);
        }
        output
    }

    /// Every expandable row, used to initialize an expanded project without hardcoded IDs.
    #[must_use]
    pub fn expandable_ids(&self) -> BTreeSet<String> {
        self.rows
            .values()
            .filter(|row| row.expandable)
            .map(|row| row.runtime_id.clone())
            .collect()
    }

    /// Return one contiguous visible range for shift-selection.
    #[must_use]
    pub fn range(
        &self,
        expanded: &BTreeSet<String>,
        anchor: &str,
        target: &str,
    ) -> BTreeSet<String> {
        let rows = self.visible_rows(expanded);
        let Some(anchor) = rows.iter().position(|row| row.runtime_id == anchor) else {
            return BTreeSet::new();
        };
        let Some(target) = rows.iter().position(|row| row.runtime_id == target) else {
            return BTreeSet::new();
        };
        let (start, end) = if anchor <= target {
            (anchor, target)
        } else {
            (target, anchor)
        };
        rows[start..=end]
            .iter()
            .map(|row| row.runtime_id.clone())
            .collect()
    }

    /// Move focus by one visible row, clamped at either end.
    #[must_use]
    pub fn adjacent(
        &self,
        expanded: &BTreeSet<String>,
        current: Option<&str>,
        delta: isize,
    ) -> Option<String> {
        let rows = self.visible_rows(expanded);
        if rows.is_empty() {
            return None;
        }
        let current = current
            .and_then(|id| rows.iter().position(|row| row.runtime_id == id))
            .unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(rows.len() - 1);
        Some(rows[next].runtime_id.clone())
    }

    fn visit(
        &mut self,
        tree: &UiTree,
        prefix: &str,
        id: &str,
        parent: Option<String>,
        depth: usize,
    ) {
        let Some(node) = tree.nodes.get(id) else {
            return;
        };
        let component = is_component(node);
        let children = if component {
            Vec::new()
        } else {
            project_children(tree, node, prefix)
        };
        let row = LayerRow {
            runtime_id: node.id.clone(),
            authored_id: authored_id(node, prefix),
            label: layer_label(node, prefix),
            kind: layer_kind(node),
            depth,
            expandable: !children.is_empty(),
            parent: parent.clone(),
        };
        self.rows.insert(node.id.clone(), row);
        self.children.insert(node.id.clone(), children.clone());
        for child in children {
            self.visit(
                tree,
                prefix,
                &child,
                Some(node.id.clone()),
                depth.saturating_add(1),
            );
        }
    }

    fn append_visible(&self, id: &str, expanded: &BTreeSet<String>, output: &mut Vec<LayerRow>) {
        let Some(row) = self.rows.get(id) else {
            return;
        };
        output.push(row.clone());
        if expanded.contains(id) {
            for child in self.children.get(id).into_iter().flatten() {
                self.append_visible(child, expanded, output);
            }
        }
    }
}

fn project_children(tree: &UiTree, node: &UiNode, prefix: &str) -> Vec<String> {
    node.children
        .iter()
        .filter(|id| id.starts_with(prefix) && tree.nodes.contains_key(*id))
        .cloned()
        .collect()
}

fn authored_id(node: &UiNode, prefix: &str) -> String {
    node.metadata
        .get("authored_id")
        .cloned()
        .unwrap_or_else(|| node.id.strip_prefix(prefix).unwrap_or(&node.id).to_owned())
}

fn layer_label(node: &UiNode, prefix: &str) -> String {
    let authored = authored_id(node, prefix);
    if let Some(label) = node.metadata.get("component_name").or(node.label.as_ref())
        && !label.trim().is_empty()
        && label.len() <= 48
    {
        return label.clone();
    }
    authored
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect()
}

fn is_component(node: &UiNode) -> bool {
    node.metadata.contains_key("component_id")
        || node
            .metadata
            .get("html_tag")
            .is_some_and(|tag| tag == "gpui-component")
}

fn layer_kind(node: &UiNode) -> LayerKind {
    if is_component(node) {
        return LayerKind::Component;
    }
    match node.role {
        Role::Text | Role::TextInput | Role::SearchInput => LayerKind::Text,
        Role::Button
        | Role::Checkbox
        | Role::Radio
        | Role::Switch
        | Role::Combobox
        | Role::Option
        | Role::MenuItem
        | Role::Slider
        | Role::Link => LayerKind::Control,
        Role::Image => LayerKind::Image,
        _ => LayerKind::Frame,
    }
}

#[cfg(test)]
mod tests {
    use gpui_mcp::{NodeState, UiNode};

    use super::*;

    fn node(id: &str, parent: Option<&str>, children: &[&str], role: Role) -> UiNode {
        UiNode {
            id: id.to_owned(),
            parent: parent.map(ToOwned::to_owned),
            children: children.iter().map(|child| (*child).to_owned()).collect(),
            role,
            label: None,
            description: None,
            bounds: None,
            state: NodeState::default(),
            actions: Vec::new(),
            text: None,
            value: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn technical_root_is_hidden_and_component_contents_stay_in_component_tab() {
        let mut tree = UiTree::default();
        tree.nodes.insert(
            "project--html-root".to_owned(),
            node("project--html-root", None, &["project--page"], Role::Group),
        );
        tree.nodes.insert(
            "project--page".to_owned(),
            node(
                "project--page",
                Some("project--html-root"),
                &["project--title", "project--instance"],
                Role::Group,
            ),
        );
        tree.nodes.insert(
            "project--title".to_owned(),
            node("project--title", Some("project--page"), &[], Role::Text),
        );
        let mut instance = node(
            "project--instance",
            Some("project--page"),
            &["component/card/root"],
            Role::Group,
        );
        instance
            .metadata
            .insert("component_id".to_owned(), "card".to_owned());
        tree.nodes.insert(instance.id.clone(), instance);
        tree.nodes.insert(
            "component/card/root".to_owned(),
            node(
                "component/card/root",
                Some("project--instance"),
                &[],
                Role::Group,
            ),
        );
        let model = LayerTree::from_semantics(&tree, "project");
        let expanded = model.expandable_ids();
        let rows = model.visible_rows(&expanded);
        assert_eq!(
            rows.iter()
                .map(|row| row.authored_id.as_str())
                .collect::<Vec<_>>(),
            ["page", "title", "instance"]
        );
        assert_eq!(rows[2].kind, LayerKind::Component);
        assert!(!rows[2].expandable);
    }

    #[test]
    fn visible_range_tracks_collapsed_preorder() {
        let mut tree = UiTree::default();
        tree.nodes.insert(
            "p--html-root".to_owned(),
            node("p--html-root", None, &["p--a", "p--b"], Role::Group),
        );
        tree.nodes.insert(
            "p--a".to_owned(),
            node("p--a", Some("p--html-root"), &["p--a1"], Role::Group),
        );
        tree.nodes.insert(
            "p--a1".to_owned(),
            node("p--a1", Some("p--a"), &[], Role::Text),
        );
        tree.nodes.insert(
            "p--b".to_owned(),
            node("p--b", Some("p--html-root"), &[], Role::Button),
        );
        let model = LayerTree::from_semantics(&tree, "p");
        let expanded = BTreeSet::from(["p--a".to_owned()]);
        assert_eq!(
            model.range(&expanded, "p--a1", "p--b"),
            BTreeSet::from(["p--a1".to_owned(), "p--b".to_owned()])
        );
    }
}
