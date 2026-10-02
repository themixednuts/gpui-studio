//! Canvas geometry: snapping guides, alignment, and distribution.
//!
//! Everything here works in document pixels and is independent of the UI, so
//! the canvas, the Design panel, and agents share one implementation.

/// An axis-aligned rectangle in document pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// New rectangle.
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge.
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// Horizontal center.
    #[must_use]
    pub fn cx(self) -> f32 {
        self.x + self.w / 2.0
    }

    /// Vertical center.
    #[must_use]
    pub fn cy(self) -> f32 {
        self.y + self.h / 2.0
    }

    /// Translated copy.
    #[must_use]
    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    /// Smallest rectangle containing both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    /// Union of many rectangles.
    #[must_use]
    pub fn union_all(rects: impl IntoIterator<Item = Self>) -> Option<Self> {
        rects.into_iter().reduce(Self::union)
    }

    /// Whether the point is inside (edges inclusive).
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    fn xs(self) -> [f32; 3] {
        [self.x, self.cx(), self.right()]
    }

    fn ys(self) -> [f32; 3] {
        [self.y, self.cy(), self.bottom()]
    }
}

/// An alignment command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Left edges.
    Left,
    /// Horizontal centers.
    HCenter,
    /// Right edges.
    Right,
    /// Top edges.
    Top,
    /// Vertical centers.
    VCenter,
    /// Bottom edges.
    Bottom,
}

impl Align {
    /// Every alignment, in toolbar order.
    pub const ALL: [Self; 6] = [
        Self::Left,
        Self::HCenter,
        Self::Right,
        Self::Top,
        Self::VCenter,
        Self::Bottom,
    ];

    /// Parse `left`, `center`, `right`, `top`, `middle`, or `bottom`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "left" => Self::Left,
            "center" | "hcenter" | "horizontal-center" => Self::HCenter,
            "right" => Self::Right,
            "top" => Self::Top,
            "middle" | "vcenter" | "vertical-center" => Self::VCenter,
            "bottom" => Self::Bottom,
            _ => return None,
        })
    }

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Align left",
            Self::HCenter => "Align horizontal centers",
            Self::Right => "Align right",
            Self::Top => "Align top",
            Self::VCenter => "Align vertical centers",
            Self::Bottom => "Align bottom",
        }
    }
}

/// A layout axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Left to right.
    Horizontal,
    /// Top to bottom.
    Vertical,
}

impl Axis {
    /// Parse `horizontal` or `vertical`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "horizontal" | "x" => Some(Self::Horizontal),
            "vertical" | "y" => Some(Self::Vertical),
            _ => None,
        }
    }
}

/// Offsets that align every rectangle to `target` (usually the union of the
/// selection, or the parent for a single layer).
#[must_use]
pub fn align_deltas(rects: &[Rect], target: Rect, align: Align) -> Vec<(f32, f32)> {
    rects
        .iter()
        .map(|r| match align {
            Align::Left => (target.x - r.x, 0.0),
            Align::HCenter => (target.cx() - r.cx(), 0.0),
            Align::Right => (target.right() - r.right(), 0.0),
            Align::Top => (0.0, target.y - r.y),
            Align::VCenter => (0.0, target.cy() - r.cy()),
            Align::Bottom => (0.0, target.bottom() - r.bottom()),
        })
        .collect()
}

/// Offsets that space rectangles evenly along an axis, keeping the first and
/// last in place (like Figma's "distribute spacing"). Fewer than three
/// rectangles are left alone.
#[must_use]
pub fn distribute_deltas(rects: &[Rect], axis: Axis) -> Vec<(f32, f32)> {
    let mut out = vec![(0.0, 0.0); rects.len()];
    if rects.len() < 3 {
        return out;
    }
    let start = |r: &Rect| match axis {
        Axis::Horizontal => r.x,
        Axis::Vertical => r.y,
    };
    let extent = |r: &Rect| match axis {
        Axis::Horizontal => r.w,
        Axis::Vertical => r.h,
    };
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|a, b| start(&rects[*a]).total_cmp(&start(&rects[*b])));
    let first = &rects[order[0]];
    let last = &rects[order[order.len() - 1]];
    let span = start(last) + extent(last) - start(first);
    let total: f32 = rects.iter().map(extent).sum();
    let gap = (span - total) / (rects.len() - 1) as f32;
    let mut cursor = start(first);
    for index in order {
        let r = &rects[index];
        let delta = (cursor - start(r)).round();
        out[index] = match axis {
            Axis::Horizontal => (delta, 0.0),
            Axis::Vertical => (0.0, delta),
        };
        cursor += extent(r) + gap;
    }
    out
}

/// A snapping guide line in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    /// `Vertical` draws a line at x = `at`; `Horizontal` at y = `at`.
    pub axis: Axis,
    /// Line position.
    pub at: f32,
    /// Span start along the line.
    pub from: f32,
    /// Span end along the line.
    pub to: f32,
}

/// Which edges of a rectangle participate in snapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    /// Left, center, right.
    pub x: [bool; 3],
    /// Top, middle, bottom.
    pub y: [bool; 3],
}

impl Edges {
    /// All edges and centers (moving).
    pub const ALL: Self = Self {
        x: [true; 3],
        y: [true; 3],
    };
}

/// The result of snapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snap {
    /// Horizontal correction.
    pub dx: f32,
    /// Vertical correction.
    pub dy: f32,
    /// Guides to draw for the snapped position.
    pub guides: Vec<Guide>,
}

fn best(moving: [f32; 3], enabled: [bool; 3], targets: &[[f32; 3]], threshold: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (m, on) in moving.iter().zip(enabled) {
        if !on {
            continue;
        }
        for t in targets.iter().flatten() {
            let d = t - m;
            if d.abs() <= threshold && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best
}

/// Snap `moving` to the edges and centers of `targets` within `threshold`
/// (document pixels), returning the correction and the guides to show.
#[must_use]
pub fn snap(moving: Rect, edges: Edges, targets: &[Rect], threshold: f32) -> Snap {
    let xs: Vec<[f32; 3]> = targets.iter().map(|r| r.xs()).collect();
    let ys: Vec<[f32; 3]> = targets.iter().map(|r| r.ys()).collect();
    let dx = best(moving.xs(), edges.x, &xs, threshold).unwrap_or(0.0);
    let dy = best(moving.ys(), edges.y, &ys, threshold).unwrap_or(0.0);
    let snapped = moving.offset(dx, dy);
    Snap {
        dx,
        dy,
        guides: guides(snapped, edges, targets),
    }
}

/// Guides for every target edge that lines up with `rect`.
#[must_use]
pub fn guides(rect: Rect, edges: Edges, targets: &[Rect]) -> Vec<Guide> {
    const EPSILON: f32 = 0.5;
    let mut out: Vec<Guide> = Vec::new();
    let mut push = |guide: Guide| {
        if let Some(existing) = out
            .iter_mut()
            .find(|g| g.axis == guide.axis && (g.at - guide.at).abs() < EPSILON)
        {
            existing.from = existing.from.min(guide.from);
            existing.to = existing.to.max(guide.to);
        } else {
            out.push(guide);
        }
    };
    for target in targets {
        for (m, on) in rect.xs().into_iter().zip(edges.x) {
            if on && target.xs().iter().any(|t| (t - m).abs() < EPSILON) {
                push(Guide {
                    axis: Axis::Vertical,
                    at: m,
                    from: rect.y.min(target.y),
                    to: rect.bottom().max(target.bottom()),
                });
            }
        }
        for (m, on) in rect.ys().into_iter().zip(edges.y) {
            if on && target.ys().iter().any(|t| (t - m).abs() < EPSILON) {
                push(Guide {
                    axis: Axis::Horizontal,
                    at: m,
                    from: rect.x.min(target.x),
                    to: rect.right().max(target.right()),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_nearest_edge_or_center_and_reports_guides() {
        let targets = [Rect::new(0.0, 0.0, 100.0, 100.0)];
        // Left edge 3px right of the target's right edge snaps flush.
        let s = snap(
            Rect::new(103.0, 120.0, 50.0, 50.0),
            Edges::ALL,
            &targets,
            5.0,
        );
        assert_eq!((s.dx, s.dy), (-3.0, 0.0));
        assert!(
            s.guides
                .iter()
                .any(|g| g.axis == Axis::Vertical && g.at == 100.0)
        );
        // Centers snap too.
        let s = snap(
            Rect::new(26.0, 200.0, 50.0, 10.0),
            Edges::ALL,
            &targets,
            5.0,
        );
        assert_eq!(s.dx, -1.0);
        // Out of range: nothing.
        let s = snap(
            Rect::new(300.0, 300.0, 10.0, 10.0),
            Edges::ALL,
            &targets,
            5.0,
        );
        assert_eq!(s, Snap::default());
        // Only enabled edges snap (a right-edge resize ignores the left edge).
        let right_only = Edges {
            x: [false, false, true],
            y: [false; 3],
        };
        let s = snap(Rect::new(98.0, 0.0, 10.0, 10.0), right_only, &targets, 5.0);
        assert_eq!(s.dx, 0.0);
    }

    #[test]
    fn aligns_and_distributes() {
        let rects = [
            Rect::new(10.0, 0.0, 20.0, 20.0),
            Rect::new(50.0, 30.0, 40.0, 10.0),
            Rect::new(200.0, 5.0, 20.0, 20.0),
        ];
        let target = Rect::union_all(rects).unwrap();
        assert_eq!(
            align_deltas(&rects, target, Align::Left),
            vec![(0.0, 0.0), (-40.0, 0.0), (-190.0, 0.0)]
        );
        assert_eq!(align_deltas(&rects, target, Align::Bottom)[0], (0.0, 20.0));
        assert_eq!(align_deltas(&rects, target, Align::HCenter)[1], (45.0, 0.0));
        // Span 10..220 = 210, widths 80, so gaps of 65: the middle moves to 95.
        let d = distribute_deltas(&rects, Axis::Horizontal);
        assert_eq!(d, vec![(0.0, 0.0), (45.0, 0.0), (0.0, 0.0)]);
        assert_eq!(
            distribute_deltas(&rects[..2], Axis::Vertical),
            vec![(0.0, 0.0); 2]
        );
        assert_eq!(Align::parse("middle"), Some(Align::VCenter));
        assert_eq!(Axis::parse("x"), Some(Axis::Horizontal));
    }
}
