//! Transactional editing for the canonical component graph.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    NativeAppearance, NativeLayout, NativeNode, NativeNodeKind, NativeSemanticRole,
    NativeSemanticState, NativeTypography,
};

const MAX_HISTORY: usize = 100;
const ORDER_RADIX: u16 = u16::MAX;

/// A dense, lexicographically ordered fractional key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderKey(Vec<u16>);

impl OrderKey {
    /// Create a key strictly between two optional neighbors.
    #[must_use]
    pub fn between(previous: Option<&Self>, next: Option<&Self>) -> Self {
        let previous = previous.map_or(&[][..], |key| key.0.as_slice());
        let next = next.map_or(&[][..], |key| key.0.as_slice());
        let mut key = Vec::new();
        for index in 0.. {
            let low = previous.get(index).copied().unwrap_or(0);
            let high = next.get(index).copied().unwrap_or(ORDER_RADIX);
            if high.saturating_sub(low) > 1 {
                key.push(low + (high - low) / 2);
                break;
            }
            key.push(low);
        }
        Self(key)
    }

    fn evenly_spaced(index: usize, count: usize) -> Self {
        let divisor = u32::try_from(count.saturating_add(1)).unwrap_or(u32::MAX);
        let numerator = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
        let value = numerator
            .saturating_mul(u32::from(ORDER_RADIX))
            .checked_div(divisor.max(1))
            .unwrap_or(1)
            .clamp(1, u32::from(ORDER_RADIX - 1));
        Self(vec![u16::try_from(value).unwrap_or(ORDER_RADIX - 1)])
    }
}

/// Partial node update used by the inspector and MCP mutation API.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePatch {
    /// Replace the semantic element behavior.
    pub kind: Option<NativeNodeKind>,
    /// Replace stronger accessibility semantics, including clearing with `Some(None)`.
    pub semantic_role: Option<Option<NativeSemanticRole>>,
    /// Replace accessibility state.
    pub state: Option<NativeSemanticState>,
    /// Replace layout properties.
    pub layout: Option<NativeLayout>,
    /// Replace paint properties.
    pub appearance: Option<NativeAppearance>,
    /// Replace typography properties.
    pub typography: Option<NativeTypography>,
    /// Replace text, including clearing it with `Some(None)`.
    pub text: Option<Option<String>>,
    /// Replace action, including clearing it with `Some(None)`.
    pub action: Option<Option<String>>,
}

/// One atomic component-graph operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum GraphCommand {
    /// Insert a new node at an ordered child index.
    Insert {
        /// Parent container ID.
        parent: String,
        /// Requested insertion index; values past the end append.
        index: usize,
        /// Complete canonical node.
        node: NativeNode,
    },
    /// Remove a node and its descendants.
    Remove {
        /// Node ID.
        id: String,
    },
    /// Move a node without changing its stable identity.
    Move {
        /// Node ID.
        id: String,
        /// Destination parent.
        parent: String,
        /// Destination index after removal.
        index: usize,
    },
    /// Duplicate a subtree, remapping every copied ID.
    Duplicate {
        /// Source subtree root.
        id: String,
        /// Destination parent.
        parent: String,
        /// Destination index.
        index: usize,
        /// Suffix appended to copied IDs.
        suffix: String,
    },
    /// Wrap same-parent nodes in a new container.
    Group {
        /// Nodes in desired group order.
        ids: Vec<String>,
        /// New container.
        group: NativeNode,
    },
    /// Replace a group with its children.
    Ungroup {
        /// Container ID.
        id: String,
    },
    /// Update editable values while preserving identity and children.
    Patch {
        /// Node ID.
        id: String,
        /// Typed replacement values.
        patch: NodePatch,
    },
}

/// Revision-checked mutation batch applied as one undo step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentTransaction {
    /// Revision the caller read before producing commands.
    pub expected_revision: u64,
    /// Human or agent identifier used for audit metadata.
    pub actor: String,
    /// Ordered atomic commands.
    pub commands: Vec<GraphCommand>,
}

/// Successful transaction summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphChange {
    /// Previous graph revision.
    pub previous_revision: u64,
    /// New graph revision.
    pub revision: u64,
    /// Actor supplied by the caller.
    pub actor: String,
    /// IDs directly touched by the batch.
    pub touched: BTreeSet<String>,
}

/// Rejected component graph mutation.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum GraphError {
    /// Caller edited an obsolete snapshot.
    #[error("component graph conflict: expected revision {expected}, current revision is {actual}")]
    Conflict {
        /// Requested base revision.
        expected: u64,
        /// Current graph revision.
        actual: u64,
    },
    /// A referenced node does not exist.
    #[error("component node `{0}` does not exist")]
    MissingNode(String),
    /// A new or remapped ID already exists.
    #[error("component node ID `{0}` already exists")]
    DuplicateId(String),
    /// Root removal or movement was attempted.
    #[error("the component root cannot be removed or moved")]
    RootMutation,
    /// The requested parent cannot contain children.
    #[error("component node `{0}` cannot contain children")]
    LeafParent(String),
    /// Moving a node beneath itself would introduce a cycle.
    #[error("moving `{node}` beneath `{parent}` would create a cycle")]
    Cycle {
        /// Moved node.
        node: String,
        /// Invalid destination.
        parent: String,
    },
    /// Group members do not have one parent.
    #[error("grouped nodes must be distinct siblings")]
    InvalidGroup,
    /// Revision counter cannot advance.
    #[error("component graph revision space is exhausted")]
    RevisionExhausted,
    /// No previous or later state is available.
    #[error("no {0} state is available")]
    History(&'static str),
}

/// Canonical graph plus bounded local history and concurrency metadata.
#[derive(Clone, Debug)]
pub struct ComponentGraph {
    root: NativeNode,
    revision: u64,
    order: BTreeMap<String, OrderKey>,
    undo: Vec<NativeNode>,
    redo: Vec<NativeNode>,
}

impl ComponentGraph {
    /// Start editing one component tree at a known persisted revision.
    #[must_use]
    pub fn new(root: NativeNode, revision: u64) -> Self {
        let mut graph = Self {
            root,
            revision,
            order: BTreeMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
        graph.rebuild_order();
        graph
    }

    /// Current immutable root.
    #[must_use]
    pub const fn root(&self) -> &NativeNode {
        &self.root
    }

    /// Current optimistic-concurrency revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Stable sibling order key for an existing node.
    #[must_use]
    pub fn order_key(&self, id: &str) -> Option<&OrderKey> {
        self.order.get(id)
    }

    /// Apply all commands to a clone, validate it, then publish atomically.
    pub fn apply(&mut self, transaction: ComponentTransaction) -> Result<GraphChange, GraphError> {
        if transaction.expected_revision != self.revision {
            return Err(GraphError::Conflict {
                expected: transaction.expected_revision,
                actual: self.revision,
            });
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(GraphError::RevisionExhausted)?;
        let mut candidate = self.root.clone();
        let mut touched = BTreeSet::new();
        for command in &transaction.commands {
            apply_command(&mut candidate, command, &mut touched)?;
        }
        validate_unique_ids(&candidate)?;
        if transaction.commands.is_empty() {
            return Ok(GraphChange {
                previous_revision: self.revision,
                revision: self.revision,
                actor: transaction.actor,
                touched,
            });
        }
        push_bounded(&mut self.undo, self.root.clone());
        self.redo.clear();
        self.root = candidate;
        let previous_revision = self.revision;
        self.revision = revision;
        self.rebuild_order();
        Ok(GraphChange {
            previous_revision,
            revision,
            actor: transaction.actor,
            touched,
        })
    }

    /// Restore the previous graph state as a new revision.
    pub fn undo(&mut self) -> Result<u64, GraphError> {
        let previous = self.undo.pop().ok_or(GraphError::History("undo"))?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(GraphError::RevisionExhausted)?;
        push_bounded(&mut self.redo, self.root.clone());
        self.root = previous;
        self.revision = revision;
        self.rebuild_order();
        Ok(revision)
    }

    /// Restore the next graph state as a new revision.
    pub fn redo(&mut self) -> Result<u64, GraphError> {
        let next = self.redo.pop().ok_or(GraphError::History("redo"))?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(GraphError::RevisionExhausted)?;
        push_bounded(&mut self.undo, self.root.clone());
        self.root = next;
        self.revision = revision;
        self.rebuild_order();
        Ok(revision)
    }

    fn rebuild_order(&mut self) {
        self.order.clear();
        collect_order(&self.root, &mut self.order);
    }
}

fn apply_command(
    root: &mut NativeNode,
    command: &GraphCommand,
    touched: &mut BTreeSet<String>,
) -> Result<(), GraphError> {
    match command {
        GraphCommand::Insert {
            parent,
            index,
            node,
        } => {
            validate_unique_ids(node)?;
            for inserted_id in collect_ids(node) {
                ensure_absent(root, &inserted_id)?;
            }
            let destination =
                find_mut(root, parent).ok_or_else(|| GraphError::MissingNode(parent.clone()))?;
            ensure_can_parent(destination)?;
            destination
                .children
                .insert((*index).min(destination.children.len()), node.clone());
            touched.insert(parent.clone());
            touched.insert(node.id.clone());
        }
        GraphCommand::Remove { id } => {
            if root.id == *id {
                return Err(GraphError::RootMutation);
            }
            take_node(root, id).ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            touched.insert(id.clone());
        }
        GraphCommand::Move { id, parent, index } => {
            if root.id == *id {
                return Err(GraphError::RootMutation);
            }
            let moving = find(root, id).ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            if contains(moving, parent) {
                return Err(GraphError::Cycle {
                    node: id.clone(),
                    parent: parent.clone(),
                });
            }
            if find(root, parent).is_none() {
                return Err(GraphError::MissingNode(parent.clone()));
            }
            let node = take_node(root, id).ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            let destination =
                find_mut(root, parent).ok_or_else(|| GraphError::MissingNode(parent.clone()))?;
            ensure_can_parent(destination)?;
            destination
                .children
                .insert((*index).min(destination.children.len()), node);
            touched.extend([id.clone(), parent.clone()]);
        }
        GraphCommand::Duplicate {
            id,
            parent,
            index,
            suffix,
        } => {
            let mut copy = find(root, id)
                .cloned()
                .ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            remap_ids(&mut copy, suffix);
            for copied_id in collect_ids(&copy) {
                ensure_absent(root, &copied_id)?;
                touched.insert(copied_id);
            }
            let destination =
                find_mut(root, parent).ok_or_else(|| GraphError::MissingNode(parent.clone()))?;
            ensure_can_parent(destination)?;
            destination
                .children
                .insert((*index).min(destination.children.len()), copy);
            touched.insert(parent.clone());
        }
        GraphCommand::Group { ids, group } => group_nodes(root, ids, group, touched)?,
        GraphCommand::Ungroup { id } => ungroup_node(root, id, touched)?,
        GraphCommand::Patch { id, patch } => {
            let node = find_mut(root, id).ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            if let Some(kind) = patch.kind {
                node.kind = kind;
            }
            if let Some(role) = patch.semantic_role {
                node.semantic_role = role;
            }
            if let Some(state) = patch.state {
                node.state = state;
            }
            if let Some(layout) = patch.layout {
                node.layout = layout;
            }
            if let Some(appearance) = patch.appearance {
                node.appearance = appearance;
            }
            if let Some(typography) = &patch.typography {
                node.typography.clone_from(typography);
            }
            if let Some(text) = &patch.text {
                node.text.clone_from(text);
            }
            if let Some(action) = &patch.action {
                node.action.clone_from(action);
            }
            validate_node_shape(node)?;
            touched.insert(id.clone());
        }
    }
    Ok(())
}

fn group_nodes(
    root: &mut NativeNode,
    ids: &[String],
    group: &NativeNode,
    touched: &mut BTreeSet<String>,
) -> Result<(), GraphError> {
    if ids.is_empty()
        || !group.children.is_empty()
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(GraphError::InvalidGroup);
    }
    ensure_absent(root, &group.id)?;
    ensure_can_parent(group)?;
    let (parent_id, mut indexes) = sibling_indexes(root, ids).ok_or(GraphError::InvalidGroup)?;
    indexes.sort_unstable();
    let insert_at = indexes[0];
    let parent =
        find_mut(root, &parent_id).ok_or_else(|| GraphError::MissingNode(parent_id.clone()))?;
    let mut children = Vec::with_capacity(indexes.len());
    for index in indexes.into_iter().rev() {
        children.push(parent.children.remove(index));
    }
    children.reverse();
    let mut container = group.clone();
    container.children = children;
    parent.children.insert(insert_at, container);
    touched.extend(ids.iter().cloned());
    touched.extend([parent_id, group.id.clone()]);
    Ok(())
}

fn ungroup_node(
    root: &mut NativeNode,
    id: &str,
    touched: &mut BTreeSet<String>,
) -> Result<(), GraphError> {
    if root.id == id {
        return Err(GraphError::RootMutation);
    }
    let (parent_id, index) =
        parent_and_index(root, id).ok_or_else(|| GraphError::MissingNode(id.to_owned()))?;
    let parent =
        find_mut(root, &parent_id).ok_or_else(|| GraphError::MissingNode(parent_id.clone()))?;
    let mut group = parent.children.remove(index);
    for (offset, child) in group.children.drain(..).enumerate() {
        touched.insert(child.id.clone());
        parent.children.insert(index + offset, child);
    }
    touched.extend([parent_id, id.to_owned()]);
    Ok(())
}

fn ensure_can_parent(node: &NativeNode) -> Result<(), GraphError> {
    if matches!(node.kind, NativeNodeKind::Text | NativeNodeKind::Button) {
        return Err(GraphError::LeafParent(node.id.clone()));
    }
    Ok(())
}

fn validate_node_shape(node: &NativeNode) -> Result<(), GraphError> {
    if matches!(node.kind, NativeNodeKind::Text | NativeNodeKind::Button)
        && !node.children.is_empty()
    {
        return Err(GraphError::LeafParent(node.id.clone()));
    }
    Ok(())
}

fn find<'a>(node: &'a NativeNode, id: &str) -> Option<&'a NativeNode> {
    (node.id == id)
        .then_some(node)
        .or_else(|| node.children.iter().find_map(|child| find(child, id)))
}

fn find_mut<'a>(node: &'a mut NativeNode, id: &str) -> Option<&'a mut NativeNode> {
    if node.id == id {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_mut(child, id))
}

fn contains(node: &NativeNode, id: &str) -> bool {
    find(node, id).is_some()
}

fn take_node(node: &mut NativeNode, id: &str) -> Option<NativeNode> {
    if let Some(index) = node.children.iter().position(|child| child.id == id) {
        return Some(node.children.remove(index));
    }
    node.children
        .iter_mut()
        .find_map(|child| take_node(child, id))
}

fn parent_and_index(node: &NativeNode, id: &str) -> Option<(String, usize)> {
    if let Some(index) = node.children.iter().position(|child| child.id == id) {
        return Some((node.id.clone(), index));
    }
    node.children
        .iter()
        .find_map(|child| parent_and_index(child, id))
}

fn sibling_indexes(root: &NativeNode, ids: &[String]) -> Option<(String, Vec<usize>)> {
    let mut parent = None;
    let mut indexes = Vec::with_capacity(ids.len());
    for id in ids {
        let (candidate, index) = parent_and_index(root, id)?;
        if parent.as_ref().is_some_and(|parent| parent != &candidate) {
            return None;
        }
        parent = Some(candidate);
        indexes.push(index);
    }
    Some((parent?, indexes))
}

fn ensure_absent(root: &NativeNode, id: &str) -> Result<(), GraphError> {
    if contains(root, id) {
        Err(GraphError::DuplicateId(id.to_owned()))
    } else {
        Ok(())
    }
}

fn remap_ids(node: &mut NativeNode, suffix: &str) {
    node.id.push_str(suffix);
    for child in &mut node.children {
        remap_ids(child, suffix);
    }
}

fn collect_ids(node: &NativeNode) -> Vec<String> {
    let mut output = vec![node.id.clone()];
    for child in &node.children {
        output.extend(collect_ids(child));
    }
    output
}

fn validate_unique_ids(root: &NativeNode) -> Result<(), GraphError> {
    fn visit(node: &NativeNode, ids: &mut BTreeSet<String>) -> Result<(), GraphError> {
        if !ids.insert(node.id.clone()) {
            return Err(GraphError::DuplicateId(node.id.clone()));
        }
        for child in &node.children {
            visit(child, ids)?;
        }
        Ok(())
    }
    visit(root, &mut BTreeSet::new())
}

fn collect_order(node: &NativeNode, order: &mut BTreeMap<String, OrderKey>) {
    for (index, child) in node.children.iter().enumerate() {
        order.insert(
            child.id.clone(),
            OrderKey::evenly_spaced(index, node.children.len()),
        );
        collect_order(child, order);
    }
}

fn push_bounded(history: &mut Vec<NativeNode>, state: NativeNode) {
    if history.len() == MAX_HISTORY {
        history.remove(0);
    }
    history.push(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NativeAlign, NativeSize};

    fn node(id: &str, kind: NativeNodeKind) -> NativeNode {
        NativeNode {
            id: id.to_owned(),
            kind,
            semantic_role: None,
            state: crate::NativeSemanticState::default(),
            layout: NativeLayout::default(),
            appearance: NativeAppearance::default(),
            typography: NativeTypography::default(),
            text: None,
            action: None,
            instance_of: None,
            children: Vec::new(),
        }
    }

    fn graph() -> ComponentGraph {
        let mut root = node("root", NativeNodeKind::Column);
        root.layout.width = NativeSize::Fill;
        root.layout.align = NativeAlign::Center;
        root.children = vec![
            node("a", NativeNodeKind::Row),
            node("b", NativeNodeKind::Text),
        ];
        ComponentGraph::new(root, 3)
    }

    #[test]
    fn transaction_is_atomic_and_revision_checked() {
        let mut graph = graph();
        let result = graph.apply(ComponentTransaction {
            expected_revision: 3,
            actor: "test".to_owned(),
            commands: vec![GraphCommand::Move {
                id: "a".to_owned(),
                parent: "missing".to_owned(),
                index: 0,
            }],
        });
        assert!(matches!(result, Err(GraphError::MissingNode(_))));
        assert_eq!(graph.root.children[0].id, "a");
        assert_eq!(graph.revision(), 3);
        assert!(matches!(
            graph.apply(ComponentTransaction {
                expected_revision: 2,
                actor: String::new(),
                commands: Vec::new()
            }),
            Err(GraphError::Conflict { .. })
        ));
    }

    #[test]
    fn grouping_undo_and_redo_preserve_identity() -> Result<(), GraphError> {
        let mut graph = graph();
        let group = node("group", NativeNodeKind::Column);
        graph.apply(ComponentTransaction {
            expected_revision: 3,
            actor: "human".to_owned(),
            commands: vec![GraphCommand::Group {
                ids: vec!["a".to_owned(), "b".to_owned()],
                group,
            }],
        })?;
        assert_eq!(graph.root.children[0].children.len(), 2);
        graph.undo()?;
        assert_eq!(
            graph
                .root
                .children
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        graph.redo()?;
        assert_eq!(graph.root.children[0].id, "group");
        Ok(())
    }

    #[test]
    fn fractional_keys_always_fit_between_neighbors() {
        let low = OrderKey(vec![10]);
        let high = OrderKey(vec![11]);
        let middle = OrderKey::between(Some(&low), Some(&high));
        assert!(low < middle && middle < high);
        let before = OrderKey::between(None, Some(&low));
        let after = OrderKey::between(Some(&high), None);
        assert!(before < low && after > high);
    }

    #[test]
    fn inserting_a_subtree_rejects_any_descendant_id_collision() {
        let mut graph = graph();
        let mut subtree = node("new-parent", NativeNodeKind::Column);
        subtree.children.push(node("b", NativeNodeKind::Text));
        let result = graph.apply(ComponentTransaction {
            expected_revision: 3,
            actor: "test".to_owned(),
            commands: vec![GraphCommand::Insert {
                parent: "root".to_owned(),
                index: 2,
                node: subtree,
            }],
        });
        assert_eq!(result, Err(GraphError::DuplicateId("b".to_owned())));
        assert_eq!(graph.revision(), 3);
        assert_eq!(graph.root.children.len(), 2);
    }
}
