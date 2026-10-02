//! Connectors: arrows drawn on the canvas between layers or points.
//!
//! Connections belong to a page, not to an artboard's HTML, so they can link
//! layers across artboards (user flows, annotations, architecture sketches).
//! An endpoint attached to a layer follows it as the design changes.

use serde::{Deserialize, Serialize};

use super::NodeId;

/// One end of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Endpoint {
    /// Attached to a layer; follows it.
    Node(NodeId),
    /// A free point in document coordinates.
    Point {
        /// X.
        x: f32,
        /// Y.
        y: f32,
    },
}

impl Endpoint {
    /// The attached layer, if any.
    #[must_use]
    pub fn node(self) -> Option<NodeId> {
        match self {
            Self::Node(id) => Some(id),
            Self::Point { .. } => None,
        }
    }
}

/// How the connector is routed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStyle {
    /// Smooth cubic curve leaving and entering perpendicular to the sides.
    #[default]
    Curved,
    /// Straight segment.
    Straight,
    /// Right-angled elbow.
    Elbow,
}

impl ConnectorStyle {
    /// All styles in UI order.
    pub const ALL: [Self; 3] = [Self::Curved, Self::Straight, Self::Elbow];

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Curved => "Curved",
            Self::Straight => "Straight",
            Self::Elbow => "Elbow",
        }
    }

    /// Parse the wire name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "curved" => Some(Self::Curved),
            "straight" => Some(Self::Straight),
            "elbow" => Some(Self::Elbow),
            _ => None,
        }
    }
}

/// Which ends carry arrowheads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHeads {
    /// Arrow at the target.
    #[default]
    End,
    /// Arrows at both ends.
    Both,
    /// Plain line.
    None,
}

impl ArrowHeads {
    /// All options in UI order.
    pub const ALL: [Self; 3] = [Self::End, Self::Both, Self::None];

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::End => "Arrow",
            Self::Both => "Both",
            Self::None => "Line",
        }
    }

    /// Parse the wire name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "end" => Some(Self::End),
            "both" => Some(Self::Both),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// A connector between two endpoints on one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Project-unique id.
    pub id: u64,
    /// Start.
    pub from: Endpoint,
    /// End.
    pub to: Endpoint,
    /// Optional text shown at the midpoint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// CSS color.
    #[serde(default = "default_color")]
    pub color: String,
    /// Routing.
    #[serde(default)]
    pub style: ConnectorStyle,
    /// Arrowheads.
    #[serde(default)]
    pub heads: ArrowHeads,
}

fn default_color() -> String {
    "#0d99ff".to_owned()
}

impl Connection {
    /// A default-styled connection.
    #[must_use]
    pub fn new(id: u64, from: Endpoint, to: Endpoint) -> Self {
        Self {
            id,
            from,
            to,
            label: String::new(),
            color: default_color(),
            style: ConnectorStyle::default(),
            heads: ArrowHeads::default(),
        }
    }

    /// Whether an endpoint is attached to `id`.
    #[must_use]
    pub fn touches(&self, id: NodeId) -> bool {
        self.from.node() == Some(id) || self.to.node() == Some(id)
    }
}

/// What an endpoint resolves to in document space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// A layer's rectangle `(x, y, w, h)`.
    Rect(f32, f32, f32, f32),
    /// A point.
    Point(f32, f32),
}

impl Anchor {
    fn center(self) -> (f32, f32) {
        match self {
            Self::Rect(x, y, w, h) => (x + w / 2.0, y + h / 2.0),
            Self::Point(x, y) => (x, y),
        }
    }

    /// Exit point on this anchor toward `toward`, with the outward direction.
    fn port(self, toward: (f32, f32)) -> ((f32, f32), (f32, f32)) {
        match self {
            Self::Point(x, y) => {
                let (dx, dy) = (toward.0 - x, toward.1 - y);
                let dir = if dx.abs() >= dy.abs() {
                    (dx.signum(), 0.0)
                } else {
                    (0.0, dy.signum())
                };
                ((x, y), dir)
            }
            Self::Rect(x, y, w, h) => {
                let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                let (dx, dy) = (toward.0 - cx, toward.1 - cy);
                // Compare against the rectangle's aspect so the chosen side
                // is the one the line to the target actually crosses.
                if dx.abs() * h.max(1.0) >= dy.abs() * w.max(1.0) {
                    if dx >= 0.0 {
                        ((x + w, cy), (1.0, 0.0))
                    } else {
                        ((x, cy), (-1.0, 0.0))
                    }
                } else if dy >= 0.0 {
                    ((cx, y + h), (0.0, 1.0))
                } else {
                    ((cx, y), (0.0, -1.0))
                }
            }
        }
    }
}

fn cubic(p0: (f32, f32), c0: (f32, f32), c1: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * c0.0 + c * c1.0 + d * p1.0,
        a * p0.1 + b * c0.1 + c * c1.1 + d * p1.1,
    )
}

/// The connector as a polyline in document coordinates (start to end).
#[must_use]
pub fn route(from: Anchor, to: Anchor, style: ConnectorStyle) -> Vec<(f32, f32)> {
    let (start, start_dir) = from.port(to.center());
    let (end, end_dir) = to.port(from.center());
    match style {
        ConnectorStyle::Straight => vec![start, end],
        ConnectorStyle::Curved => {
            let distance = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
            let reach = (distance * 0.45).clamp(24.0, 240.0);
            let c0 = (start.0 + start_dir.0 * reach, start.1 + start_dir.1 * reach);
            let c1 = (end.0 + end_dir.0 * reach, end.1 + end_dir.1 * reach);
            (0..=32)
                .map(|i| cubic(start, c0, c1, end, i as f32 / 32.0))
                .collect()
        }
        ConnectorStyle::Elbow => {
            let horizontal_start = start_dir.1 == 0.0;
            let horizontal_end = end_dir.1 == 0.0;
            let mut points = vec![start];
            match (horizontal_start, horizontal_end) {
                (true, true) => {
                    let mid = (start.0 + end.0) / 2.0;
                    points.push((mid, start.1));
                    points.push((mid, end.1));
                }
                (false, false) => {
                    let mid = (start.1 + end.1) / 2.0;
                    points.push((start.0, mid));
                    points.push((end.0, mid));
                }
                (true, false) => points.push((end.0, start.1)),
                (false, true) => points.push((start.0, end.1)),
            }
            points.push(end);
            points
        }
    }
}

/// Distance from a point to a polyline.
#[must_use]
pub fn distance_to_polyline(points: &[(f32, f32)], p: (f32, f32)) -> f32 {
    points
        .windows(2)
        .map(|segment| {
            let (a, b) = (segment[0], segment[1]);
            let (abx, aby) = (b.0 - a.0, b.1 - a.1);
            let length = abx * abx + aby * aby;
            let t = if length <= f32::EPSILON {
                0.0
            } else {
                (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / length).clamp(0.0, 1.0)
            };
            let (x, y) = (a.0 + abx * t, a.1 + aby * t);
            ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
        })
        .fold(f32::INFINITY, f32::min)
}

/// The point halfway along a polyline, by length.
#[must_use]
pub fn midpoint(points: &[(f32, f32)]) -> (f32, f32) {
    let lengths: Vec<f32> = points
        .windows(2)
        .map(|s| ((s[1].0 - s[0].0).powi(2) + (s[1].1 - s[0].1).powi(2)).sqrt())
        .collect();
    let total: f32 = lengths.iter().sum();
    let mut remaining = total / 2.0;
    for (segment, length) in points.windows(2).zip(lengths) {
        if remaining <= length && length > 0.0 {
            let t = remaining / length;
            return (
                segment[0].0 + (segment[1].0 - segment[0].0) * t,
                segment[0].1 + (segment[1].1 - segment[0].1) * t,
            );
        }
        remaining -= length;
    }
    points.first().copied().unwrap_or((0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_face_each_other() {
        let a = Anchor::Rect(0.0, 0.0, 100.0, 50.0);
        let b = Anchor::Rect(300.0, 0.0, 100.0, 50.0);
        let line = route(a, b, ConnectorStyle::Straight);
        assert_eq!(line, vec![(100.0, 25.0), (300.0, 25.0)]);
        let below = Anchor::Rect(0.0, 300.0, 100.0, 50.0);
        let line = route(a, below, ConnectorStyle::Straight);
        assert_eq!(line, vec![(50.0, 50.0), (50.0, 300.0)]);
    }

    #[test]
    fn curves_and_elbows_connect_the_ports() {
        let a = Anchor::Rect(0.0, 0.0, 100.0, 50.0);
        let b = Anchor::Point(400.0, 200.0);
        let curve = route(a, b, ConnectorStyle::Curved);
        assert_eq!(curve.first(), Some(&(100.0, 25.0)));
        assert_eq!(curve.last(), Some(&(400.0, 200.0)));
        let elbow = route(
            a,
            Anchor::Rect(500.0, 100.0, 100.0, 50.0),
            ConnectorStyle::Elbow,
        );
        assert_eq!(
            elbow,
            vec![(100.0, 25.0), (300.0, 25.0), (300.0, 125.0), (500.0, 125.0)]
        );
        assert!(distance_to_polyline(&elbow, (300.0, 80.0)) < 0.01);
        // A target mostly below exits through the bottom.
        let down = route(
            a,
            Anchor::Rect(300.0, 200.0, 100.0, 50.0),
            ConnectorStyle::Elbow,
        );
        assert_eq!(down.first(), Some(&(50.0, 50.0)));
        assert_eq!(midpoint(&[(0.0, 0.0), (10.0, 0.0)]), (5.0, 0.0));
    }
}
