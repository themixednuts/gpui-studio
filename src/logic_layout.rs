//! Deterministic layered placement for component state and interaction graphs.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One logical node with an editor-space size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicNode {
    /// Stable component-graph identifier.
    pub id: String,
    /// Desired card width.
    pub width: f32,
    /// Desired card height.
    pub height: f32,
}

/// Directed dependency or transition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicEdge {
    /// Stable edge identifier.
    pub id: String,
    /// Source node.
    pub from: String,
    /// Destination node.
    pub to: String,
}

/// Positioned node returned by the layered layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicPlacement {
    /// Stable node identifier.
    pub id: String,
    /// Zero-based dependency rank.
    pub rank: usize,
    /// Zero-based order within the rank.
    pub order: usize,
    /// Editor-space left coordinate.
    pub x: f32,
    /// Editor-space top coordinate.
    pub y: f32,
    /// Preserved node width.
    pub width: f32,
    /// Preserved node height.
    pub height: f32,
}

/// Tunable spacing for a left-to-right layered graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicLayoutOptions {
    /// Horizontal space between dependency ranks.
    pub rank_gap: f32,
    /// Vertical space between nodes in one rank.
    pub node_gap: f32,
    /// Number of alternating barycentric crossing-reduction passes.
    pub sweeps: usize,
}

impl Default for LogicLayoutOptions {
    fn default() -> Self {
        Self {
            rank_gap: 72.0,
            node_gap: 16.0,
            sweeps: 4,
        }
    }
}

/// Place a directed graph using longest-path ranking and deterministic
/// barycentric sweeps. Cycles are retained in a final fallback rank rather than
/// making layout fail, which keeps partially-authored logic editable.
#[must_use]
pub fn layout_logic_graph(
    nodes: &[LogicNode],
    edges: &[LogicEdge],
    options: LogicLayoutOptions,
) -> Vec<LogicPlacement> {
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<_, _>>();
    let mut incoming = nodes_by_id
        .keys()
        .map(|id| (id.clone(), Vec::<String>::new()))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = incoming.clone();
    for edge in edges {
        if edge.from != edge.to
            && nodes_by_id.contains_key(&edge.from)
            && nodes_by_id.contains_key(&edge.to)
        {
            outgoing
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
            incoming
                .entry(edge.to.clone())
                .or_default()
                .push(edge.from.clone());
        }
    }
    for neighbors in incoming.values_mut().chain(outgoing.values_mut()) {
        neighbors.sort();
        neighbors.dedup();
    }

    let mut remaining_indegree = incoming
        .iter()
        .map(|(id, parents)| (id.clone(), parents.len()))
        .collect::<BTreeMap<_, _>>();
    let mut ready = remaining_indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect::<BTreeSet<_>>();
    let mut ranks = BTreeMap::<String, usize>::new();
    let mut processed = BTreeSet::new();
    while let Some(id) = ready.pop_first() {
        let rank = incoming[&id]
            .iter()
            .filter_map(|parent| ranks.get(parent))
            .map(|rank| rank.saturating_add(1))
            .max()
            .unwrap_or(0);
        ranks.insert(id.clone(), rank);
        processed.insert(id.clone());
        for child in &outgoing[&id] {
            if let Some(degree) = remaining_indegree.get_mut(child) {
                *degree = degree.saturating_sub(1);
                if *degree == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }

    let fallback_rank = ranks
        .values()
        .copied()
        .max()
        .map_or(0, |rank| rank.saturating_add(1));
    for id in nodes_by_id.keys().filter(|id| !processed.contains(*id)) {
        ranks.insert(id.clone(), fallback_rank);
    }

    let rank_count = ranks
        .values()
        .copied()
        .max()
        .map_or(0, |rank| rank.saturating_add(1));
    let mut layers = vec![Vec::<String>::new(); rank_count];
    for (id, rank) in &ranks {
        layers[*rank].push(id.clone());
    }

    for sweep in 0..options.sweeps {
        let forward = sweep % 2 == 0;
        let rank_indexes: Vec<usize> = if forward {
            (1..layers.len()).collect()
        } else {
            (0..layers.len().saturating_sub(1)).rev().collect()
        };
        for rank in rank_indexes {
            let neighbor_rank = if forward { rank - 1 } else { rank + 1 };
            let neighbor_positions = layers[neighbor_rank]
                .iter()
                .enumerate()
                .map(|(index, id)| (id.clone(), index as f32))
                .collect::<BTreeMap<_, _>>();
            layers[rank].sort_by(|left, right| {
                let left_center = barycenter(
                    if forward {
                        &incoming[left]
                    } else {
                        &outgoing[left]
                    },
                    &neighbor_positions,
                );
                let right_center = barycenter(
                    if forward {
                        &incoming[right]
                    } else {
                        &outgoing[right]
                    },
                    &neighbor_positions,
                );
                left_center
                    .partial_cmp(&right_center)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| left.cmp(right))
            });
        }
    }

    let mut rank_x = Vec::with_capacity(layers.len());
    let mut x = 0.0;
    for layer in &layers {
        rank_x.push(x);
        let width = layer
            .iter()
            .filter_map(|id| nodes_by_id.get(id))
            .map(|node| node.width.max(0.0))
            .fold(0.0, f32::max);
        x += width + options.rank_gap.max(0.0);
    }
    let mut placements = Vec::with_capacity(nodes.len());
    for (rank, layer) in layers.into_iter().enumerate() {
        let mut y = 0.0;
        for (order, id) in layer.into_iter().enumerate() {
            let node = nodes_by_id[&id];
            placements.push(LogicPlacement {
                id,
                rank,
                order,
                x: rank_x[rank],
                y,
                width: node.width.max(0.0),
                height: node.height.max(0.0),
            });
            y += node.height.max(0.0) + options.node_gap.max(0.0);
        }
    }
    placements.sort_by_key(|placement| (placement.rank, placement.order));
    placements
}

fn barycenter(neighbors: &[String], positions: &BTreeMap<String, f32>) -> f32 {
    let values = neighbors
        .iter()
        .filter_map(|id| positions.get(id))
        .copied()
        .collect::<Vec<_>>();
    if values.is_empty() {
        f32::MAX
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> LogicNode {
        LogicNode {
            id: id.to_owned(),
            width: 120.0,
            height: 40.0,
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> LogicEdge {
        LogicEdge {
            id: id.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    #[test]
    fn layered_layout_is_stable_and_respects_dependency_ranks() {
        let nodes = [node("load"), node("hover"), node("idle"), node("open")];
        let edges = [
            edge("a", "idle", "hover"),
            edge("b", "hover", "open"),
            edge("c", "load", "open"),
        ];
        let first = layout_logic_graph(&nodes, &edges, LogicLayoutOptions::default());
        let second = layout_logic_graph(&nodes, &edges, LogicLayoutOptions::default());
        assert_eq!(first, second);
        let ranks = first
            .iter()
            .map(|placement| (placement.id.as_str(), placement.rank))
            .collect::<BTreeMap<_, _>>();
        assert!(ranks["idle"] < ranks["hover"]);
        assert!(ranks["hover"] < ranks["open"]);
        assert!(ranks["load"] < ranks["open"]);
    }

    #[test]
    fn cycles_remain_visible_without_overlap() {
        let nodes = [node("a"), node("b")];
        let edges = [edge("a-b", "a", "b"), edge("b-a", "b", "a")];
        let placements = layout_logic_graph(&nodes, &edges, LogicLayoutOptions::default());
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].rank, placements[1].rank);
        assert!(placements[1].y >= placements[0].y + placements[0].height);
    }
}
