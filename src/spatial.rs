//! Layout-aware placement, spatial lookup, multi-selection bounds, and smart snapping.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use gpui_mcp::{Point, Rect};
use rstar::{AABB, RTree, RTreeObject};
use serde::{Deserialize, Serialize};

/// Primary placement axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Left-to-right ordering.
    Horizontal,
    /// Top-to-bottom ordering.
    Vertical,
}

/// Parent layout behavior used to choose the insertion algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LayoutMode {
    /// Ordered Flexbox children.
    Flex {
        /// Main flow axis.
        axis: Axis,
        /// Whether visual ordering is reversed.
        reverse: bool,
    },
    /// Ordered row-major grid children.
    Grid {
        /// Explicit or resolved column count.
        columns: usize,
    },
    /// Absolute/freeform placement.
    Freeform,
}

/// Computed graph destination for a pointer drop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropPlacement {
    /// Destination parent ID.
    pub parent: String,
    /// Ordered insertion index.
    pub index: usize,
    /// Freeform origin when the parent is not flow layout.
    pub position: Option<Point>,
}

/// One indexed node rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct IndexedRect {
    index: usize,
    rect: Rect,
}

impl RTreeObject for IndexedRect {
    type Envelope = AABB<[f32; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.rect.x, self.rect.y],
            [
                self.rect.x + self.rect.width,
                self.rect.y + self.rect.height,
            ],
        )
    }
}

/// The combined frame of a multi-selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionBounds {
    /// Stable selected IDs.
    pub ids: BTreeSet<String>,
    /// Axis-aligned group frame.
    pub bounds: Rect,
}

impl SelectionBounds {
    /// Compute the group frame, ignoring invalid empty selections.
    #[must_use]
    pub fn from_rects(rects: impl IntoIterator<Item = (String, Rect)>) -> Option<Self> {
        let mut iterator = rects.into_iter().filter(|(_, rect)| valid_rect(*rect));
        let (first_id, first) = iterator.next()?;
        let mut ids = BTreeSet::from([first_id]);
        let mut left = first.x;
        let mut top = first.y;
        let mut right = first.x + first.width;
        let mut bottom = first.y + first.height;
        for (id, rect) in iterator {
            ids.insert(id);
            left = left.min(rect.x);
            top = top.min(rect.y);
            right = right.max(rect.x + rect.width);
            bottom = bottom.max(rect.y + rect.height);
        }
        Some(Self {
            ids,
            bounds: Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
        })
    }
}

/// Kind of alignment represented by a smart guide.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideKind {
    /// Leading edges align.
    Start,
    /// Centers align.
    Center,
    /// Trailing edges align.
    End,
    /// Equal spacing was detected.
    Spacing,
    /// Grid line alignment.
    Grid,
}

/// One visible smart-guide segment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guide {
    /// Guide axis.
    pub axis: Axis,
    /// Coordinate after snapping.
    pub coordinate: f32,
    /// Guide classification.
    pub kind: GuideKind,
    /// IDs participating in the guide.
    pub targets: Vec<String>,
}

/// Snapped group origin and guide overlays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapResult {
    /// Corrected x origin.
    pub x: f32,
    /// Corrected y origin.
    pub y: f32,
    /// Horizontal movement correction.
    pub delta_x: f32,
    /// Vertical movement correction.
    pub delta_y: f32,
    /// Chosen guide per independently snapped axis.
    pub guides: Vec<Guide>,
}

/// R-tree backed editor placement service.
#[derive(Clone, Debug, Default)]
pub struct PlacementEngine {
    ids: Vec<String>,
    rects: Vec<Rect>,
    tree: RTree<IndexedRect>,
}

impl PlacementEngine {
    /// Replace the index from the latest painted semantic frame.
    #[must_use]
    pub fn rebuild(nodes: impl IntoIterator<Item = (String, Rect)>) -> Self {
        let mut ids = Vec::new();
        let mut rects = Vec::new();
        let mut indexed = Vec::new();
        for (id, rect) in nodes.into_iter().filter(|(_, rect)| valid_rect(*rect)) {
            let index = ids.len();
            ids.push(id);
            rects.push(rect);
            indexed.push(IndexedRect { index, rect });
        }
        Self {
            ids,
            rects,
            tree: RTree::bulk_load(indexed),
        }
    }

    /// Query nodes touching a pointer-sized or marquee rectangle.
    #[must_use]
    pub fn intersecting(&self, query: Rect) -> Vec<String> {
        let envelope = IndexedRect {
            index: 0,
            rect: query,
        }
        .envelope();
        self.tree
            .locate_in_envelope_intersecting(envelope)
            .map(|entry| self.ids[entry.index].clone())
            .collect()
    }

    /// Choose an insertion index using visual order rather than tree-row geometry.
    #[must_use]
    pub fn drop_placement(
        &self,
        parent: impl Into<String>,
        layout: LayoutMode,
        child_ids: &[String],
        pointer: Point,
    ) -> DropPlacement {
        let parent = parent.into();
        match layout {
            LayoutMode::Freeform => DropPlacement {
                parent,
                index: child_ids.len(),
                position: Some(pointer),
            },
            LayoutMode::Flex { axis, reverse } => {
                let mut ordered = child_ids
                    .iter()
                    .filter_map(|id| self.rect_for(id).map(|rect| (id, center(rect, axis))))
                    .collect::<Vec<_>>();
                ordered
                    .sort_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal));
                if reverse {
                    ordered.reverse();
                }
                let coordinate = match axis {
                    Axis::Horizontal => pointer.x,
                    Axis::Vertical => pointer.y,
                };
                let index = ordered
                    .iter()
                    .position(|(_, midpoint)| {
                        if reverse {
                            coordinate > *midpoint
                        } else {
                            coordinate < *midpoint
                        }
                    })
                    .unwrap_or(ordered.len());
                DropPlacement {
                    parent,
                    index,
                    position: None,
                }
            }
            LayoutMode::Grid { columns } => {
                let columns = columns.max(1);
                let mut ordered = child_ids
                    .iter()
                    .filter_map(|id| self.rect_for(id).map(|rect| (rect.y, rect.x)))
                    .collect::<Vec<_>>();
                ordered.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
                let row_height = median_extent(
                    child_ids
                        .iter()
                        .filter_map(|id| self.rect_for(id).map(|r| r.height)),
                )
                .max(1.0);
                let column_width = median_extent(
                    child_ids
                        .iter()
                        .filter_map(|id| self.rect_for(id).map(|r| r.width)),
                )
                .max(1.0);
                let origin_x = ordered.first().map_or(pointer.x, |(_, x)| *x);
                let origin_y = ordered.first().map_or(pointer.y, |(y, _)| *y);
                let column = ((pointer.x - origin_x) / column_width).round().max(0.0) as usize;
                let row = ((pointer.y - origin_y) / row_height).round().max(0.0) as usize;
                let index = row
                    .saturating_mul(columns)
                    .saturating_add(column.min(columns - 1))
                    .min(child_ids.len());
                DropPlacement {
                    parent,
                    index,
                    position: None,
                }
            }
        }
    }

    /// Snap a moving group to nearby edges, centers, and the editor grid.
    #[must_use]
    pub fn snap(
        &self,
        moving: &SelectionBounds,
        proposed_x: f32,
        proposed_y: f32,
        threshold: f32,
        grid: Option<f32>,
    ) -> SnapResult {
        let proposed = Rect {
            x: proposed_x,
            y: proposed_y,
            ..moving.bounds
        };
        let search = expand(proposed, threshold.max(0.0));
        let candidates = self.intersecting(search);
        let mut horizontal = Vec::new();
        let mut vertical = Vec::new();
        for id in candidates.into_iter().filter(|id| !moving.ids.contains(id)) {
            let Some(rect) = self.rect_for(&id) else {
                continue;
            };
            collect_axis_candidates(&mut horizontal, Axis::Horizontal, proposed, rect, &id);
            collect_axis_candidates(&mut vertical, Axis::Vertical, proposed, rect, &id);
        }
        if let Some(grid) = grid.filter(|grid| grid.is_finite() && *grid > 0.0) {
            horizontal.push(SnapCandidate::grid(Axis::Horizontal, proposed.x, grid));
            vertical.push(SnapCandidate::grid(Axis::Vertical, proposed.y, grid));
        }
        let x = best_candidate(horizontal, threshold);
        let y = best_candidate(vertical, threshold);
        let delta_x = x.as_ref().map_or(0.0, |candidate| candidate.delta);
        let delta_y = y.as_ref().map_or(0.0, |candidate| candidate.delta);
        let guides = x
            .into_iter()
            .chain(y)
            .map(|candidate| candidate.guide)
            .collect();
        SnapResult {
            x: proposed_x + delta_x,
            y: proposed_y + delta_y,
            delta_x,
            delta_y,
            guides,
        }
    }

    fn rect_for(&self, id: &str) -> Option<Rect> {
        self.ids
            .iter()
            .position(|candidate| candidate == id)
            .map(|index| self.rects[index])
    }
}

#[derive(Clone, Debug)]
struct SnapCandidate {
    delta: f32,
    guide: Guide,
}

impl SnapCandidate {
    fn grid(axis: Axis, coordinate: f32, grid: f32) -> Self {
        let snapped = (coordinate / grid).round() * grid;
        Self {
            delta: snapped - coordinate,
            guide: Guide {
                axis,
                coordinate: snapped,
                kind: GuideKind::Grid,
                targets: Vec::new(),
            },
        }
    }
}

fn collect_axis_candidates(
    output: &mut Vec<SnapCandidate>,
    axis: Axis,
    moving: Rect,
    target: Rect,
    id: &str,
) {
    let moving_lines = lines(moving, axis);
    let target_lines = lines(target, axis);
    for (moving_coordinate, moving_kind) in moving_lines {
        for (target_coordinate, target_kind) in target_lines {
            if moving_kind == target_kind {
                output.push(SnapCandidate {
                    delta: target_coordinate - moving_coordinate,
                    guide: Guide {
                        axis,
                        coordinate: target_coordinate,
                        kind: moving_kind,
                        targets: vec![id.to_owned()],
                    },
                });
            }
        }
    }
}

fn best_candidate(candidates: Vec<SnapCandidate>, threshold: f32) -> Option<SnapCandidate> {
    let eligible = candidates
        .into_iter()
        .filter(|candidate| candidate.delta.abs() <= threshold.max(0.0))
        .collect::<Vec<_>>();
    let preferred = eligible
        .iter()
        .filter(|candidate| candidate.guide.kind != GuideKind::Grid)
        .min_by(|left, right| compare_candidates(left, right))
        .cloned();
    preferred.or_else(|| eligible.into_iter().min_by(compare_candidates))
}

fn compare_candidates(left: &SnapCandidate, right: &SnapCandidate) -> Ordering {
    left.delta
        .abs()
        .partial_cmp(&right.delta.abs())
        .unwrap_or(Ordering::Equal)
        .then_with(|| guide_priority(left.guide.kind).cmp(&guide_priority(right.guide.kind)))
}

const fn guide_priority(kind: GuideKind) -> u8 {
    match kind {
        GuideKind::Start | GuideKind::End => 0,
        GuideKind::Center => 1,
        GuideKind::Spacing => 2,
        GuideKind::Grid => 3,
    }
}

fn lines(rect: Rect, axis: Axis) -> [(f32, GuideKind); 3] {
    match axis {
        Axis::Horizontal => [
            (rect.x, GuideKind::Start),
            (rect.x + rect.width / 2.0, GuideKind::Center),
            (rect.x + rect.width, GuideKind::End),
        ],
        Axis::Vertical => [
            (rect.y, GuideKind::Start),
            (rect.y + rect.height / 2.0, GuideKind::Center),
            (rect.y + rect.height, GuideKind::End),
        ],
    }
}

fn center(rect: Rect, axis: Axis) -> f32 {
    match axis {
        Axis::Horizontal => rect.x + rect.width / 2.0,
        Axis::Vertical => rect.y + rect.height / 2.0,
    }
}

fn expand(rect: Rect, amount: f32) -> Rect {
    Rect {
        x: rect.x - amount,
        y: rect.y - amount,
        width: rect.width + amount * 2.0,
        height: rect.height + amount * 2.0,
    }
}

fn median_extent(values: impl Iterator<Item = f32>) -> f32 {
    let mut values = values
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    values[values.len() / 2]
}

fn valid_rect(rect: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width >= 0.0
        && rect.height >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtree_query_and_flex_drop_follow_visual_geometry() {
        let engine = PlacementEngine::rebuild([
            (
                "a".to_owned(),
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 40.0,
                    height: 30.0,
                },
            ),
            (
                "b".to_owned(),
                Rect {
                    x: 60.0,
                    y: 0.0,
                    width: 40.0,
                    height: 30.0,
                },
            ),
        ]);
        assert_eq!(
            engine.intersecting(Rect {
                x: 10.0,
                y: 5.0,
                width: 1.0,
                height: 1.0
            }),
            ["a"]
        );
        let drop = engine.drop_placement(
            "root",
            LayoutMode::Flex {
                axis: Axis::Horizontal,
                reverse: false,
            },
            &["a".to_owned(), "b".to_owned()],
            Point { x: 50.0, y: 10.0 },
        );
        assert_eq!(drop.index, 1);
    }

    #[test]
    fn snapping_prefers_matching_edges_before_grid() {
        let engine = PlacementEngine::rebuild([(
            "target".to_owned(),
            Rect {
                x: 100.0,
                y: 40.0,
                width: 60.0,
                height: 30.0,
            },
        )]);
        let moving = SelectionBounds::from_rects([(
            "moving".to_owned(),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 30.0,
            },
        )]);
        let Some(moving) = moving else {
            return;
        };
        let result = engine.snap(&moving, 96.0, 41.0, 6.0, Some(8.0));
        assert_eq!((result.x, result.y), (100.0, 40.0));
        assert!(
            result
                .guides
                .iter()
                .all(|guide| guide.kind != GuideKind::Grid)
        );
    }

    #[test]
    fn multi_selection_uses_union_bounds() {
        let selection = SelectionBounds::from_rects([
            (
                "a".to_owned(),
                Rect {
                    x: 5.0,
                    y: 8.0,
                    width: 10.0,
                    height: 12.0,
                },
            ),
            (
                "b".to_owned(),
                Rect {
                    x: 25.0,
                    y: 2.0,
                    width: 5.0,
                    height: 9.0,
                },
            ),
        ]);
        assert_eq!(
            selection.map(|selection| selection.bounds),
            Some(Rect {
                x: 5.0,
                y: 2.0,
                width: 25.0,
                height: 18.0
            })
        );
    }
}
