//! On-canvas padding and gap handles for the selected container.
//!
//! Padding shows as pink bands inside the edges and gaps as bands between
//! flex children. Dragging a band changes the value (Shift: all sides,
//! Alt: the opposite side too), like auto layout in Figma.

use gpui_kit::{Bounds, Hsla, Pixels, Point, point, px, size};

use super::Studio;
use crate::model::NodeId;
use crate::model::style::{Display, Position, fmt_num};

/// Pink used for spacing.
pub(crate) const SPACING_COLOR: Hsla = Hsla {
    h: 0.88,
    s: 0.95,
    l: 0.62,
    a: 1.0,
};

/// Smallest grab area for a zero-sized band (screen px).
const MIN_BAND: f32 = 6.0;

/// Which spacing a band edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spacing {
    /// `padding-top`.
    Top,
    /// `padding-right`.
    Right,
    /// `padding-bottom`.
    Bottom,
    /// `padding-left`.
    Left,
    /// `gap` between flex children along a row.
    RowGap,
    /// `gap` between flex children down a column.
    ColumnGap,
}

impl Spacing {
    fn vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom | Self::ColumnGap)
    }

    /// Cursor for dragging this band.
    pub(crate) fn cursor(self) -> gpui_kit::CursorStyle {
        if self.vertical() {
            gpui_kit::CursorStyle::ResizeUpDown
        } else {
            gpui_kit::CursorStyle::ResizeLeftRight
        }
    }
}

/// A draggable band.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Band {
    /// What it edits.
    pub spacing: Spacing,
    /// Where it is (window px).
    pub bounds: Bounds<Pixels>,
    /// Current value (document px).
    pub value: f32,
}

/// A spacing drag in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpacingDrag {
    pub id: NodeId,
    pub spacing: Spacing,
    pub start: Point<Pixels>,
    /// Padding (top, right, bottom, left) and gap when the drag began.
    pub padding: [f32; 4],
    pub gap: f32,
}

fn band(x: Pixels, y: Pixels, w: Pixels, h: Pixels) -> Bounds<Pixels> {
    Bounds {
        origin: point(x, y),
        size: size(w.max(px(0.0)), h.max(px(0.0))),
    }
}

impl Studio {
    /// The spacing bands of the single selected container.
    pub(crate) fn spacing_bands(&self) -> Vec<Band> {
        let [id] = self.editor.selection.as_slice() else {
            return Vec::new();
        };
        let doc = &self.editor.doc;
        let Some(node) = doc.get(*id) else {
            return Vec::new();
        };
        if node.is_text()
            || node.locked
            || doc.is_text_layer(*id)
            || self.canvas.text_edit_target_any()
        {
            return Vec::new();
        }
        let Some(b) = self.canvas.window_bounds_of(*id) else {
            return Vec::new();
        };
        let zoom = self.canvas.camera.zoom;
        let c = doc.computed(node);
        let [t, r, btm, l] = c.padding;
        let mut bands = Vec::new();
        let min = px(MIN_BAND);
        let (x0, y0, w, h) = (b.origin.x, b.origin.y, b.size.width, b.size.height);
        let along = |v: f32| px(v * zoom).max(min);
        // Only containers (things that lay out children) get padding bands.
        let container =
            matches!(c.display, Display::Flex | Display::Grid) || !node.children.is_empty();
        if container && w > px(24.0) && h > px(24.0) {
            bands.push(Band {
                spacing: Spacing::Top,
                bounds: band(x0, y0, w, along(t)),
                value: t,
            });
            bands.push(Band {
                spacing: Spacing::Bottom,
                bounds: band(x0, y0 + h - along(btm), w, along(btm)),
                value: btm,
            });
            bands.push(Band {
                spacing: Spacing::Left,
                bounds: band(x0, y0, along(l), h),
                value: l,
            });
            bands.push(Band {
                spacing: Spacing::Right,
                bounds: band(x0 + w - along(r), y0, along(r), h),
                value: r,
            });
        }
        if c.display == Display::Flex && !c.wrap {
            let column = c.direction.is_column();
            let gap = if column { c.row_gap } else { c.column_gap };
            let children: Vec<Bounds<Pixels>> = node
                .children
                .iter()
                .filter(|child| {
                    doc.get(**child).is_some_and(|n| {
                        !n.hidden && !n.is_text() && doc.computed(n).position != Position::Absolute
                    })
                })
                .filter_map(|child| self.canvas.window_bounds_of(*child))
                .collect();
            for pair in children.windows(2) {
                let (a, next) = (pair[0], pair[1]);
                let bounds = if column {
                    let top = a.origin.y + a.size.height;
                    let height = (next.origin.y - top).max(px(0.0));
                    let mid = top + height / 2.0;
                    let height = height.max(min);
                    band(
                        x0 + px(l * zoom),
                        mid - height / 2.0,
                        w - px((l + r) * zoom),
                        height,
                    )
                } else {
                    let left = a.origin.x + a.size.width;
                    let width = (next.origin.x - left).max(px(0.0));
                    let mid = left + width / 2.0;
                    let width = width.max(min);
                    band(
                        mid - width / 2.0,
                        y0 + px(t * zoom),
                        width,
                        h - px((t + btm) * zoom),
                    )
                };
                bands.push(Band {
                    spacing: if column {
                        Spacing::ColumnGap
                    } else {
                        Spacing::RowGap
                    },
                    bounds,
                    value: gap,
                });
            }
        }
        bands
    }

    /// The band under the pointer (gaps win over padding).
    pub(crate) fn spacing_hit(&self, p: Point<Pixels>) -> Option<Band> {
        let bands = self.spacing_bands();
        bands
            .iter()
            .find(|b| {
                matches!(b.spacing, Spacing::RowGap | Spacing::ColumnGap) && b.bounds.contains(&p)
            })
            .or_else(|| bands.iter().find(|b| b.bounds.contains(&p)))
            .copied()
    }

    /// Start dragging a band.
    pub(crate) fn begin_spacing_drag(
        &self,
        id: NodeId,
        band: Band,
        start: Point<Pixels>,
    ) -> SpacingDrag {
        let c = self
            .editor
            .doc
            .get(id)
            .map(|n| self.editor.doc.computed(n))
            .unwrap_or_default();
        SpacingDrag {
            id,
            spacing: band.spacing,
            start,
            padding: c.padding,
            gap: if c.direction.is_column() {
                c.row_gap
            } else {
                c.column_gap
            },
        }
    }

    /// Apply a spacing drag at pointer `p`.
    pub(crate) fn apply_spacing_drag(
        &mut self,
        drag: SpacingDrag,
        p: Point<Pixels>,
        modifiers: gpui_kit::Modifiers,
        key: &str,
    ) {
        let zoom = self.canvas.camera.zoom;
        let dx = (p.x - drag.start.x).as_f32() / zoom;
        let dy = (p.y - drag.start.y).as_f32() / zoom;
        let value = |v: f32| format!("{}px", fmt_num(v.max(0.0).round()));
        let [t, r, b, l] = drag.padding;
        let changes: Vec<(String, Option<String>)> = match drag.spacing {
            Spacing::RowGap => vec![("gap".into(), Some(value(drag.gap + dx)))],
            Spacing::ColumnGap => vec![("gap".into(), Some(value(drag.gap + dy)))],
            side => {
                // Dragging into the box grows its padding.
                let (delta, own) = match side {
                    Spacing::Top => (dy, t),
                    Spacing::Bottom => (-dy, b),
                    Spacing::Left => (dx, l),
                    _ => (-dx, r),
                };
                let next = (own + delta).max(0.0).round();
                let mut sides = [t, r, b, l];
                let index = match side {
                    Spacing::Top => 0,
                    Spacing::Right => 1,
                    Spacing::Bottom => 2,
                    _ => 3,
                };
                if modifiers.shift {
                    sides = [next; 4];
                } else {
                    sides[index] = next;
                    if modifiers.alt {
                        sides[(index + 2) % 4] = next;
                    }
                }
                vec![(
                    "padding".into(),
                    Some(
                        sides
                            .iter()
                            .map(|v| value(*v))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                )]
            }
        };
        let _ = self.editor.set_styles(&[drag.id], &changes, Some(key));
        self.inspector.invalidate();
    }
}
