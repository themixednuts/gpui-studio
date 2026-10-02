//! Vector layers drawn on the canvas: lines, arrows, and freehand strokes.
//!
//! Each becomes an absolutely positioned inline `<svg>` element, so drawings
//! are ordinary HTML that browsers render identically.

use crate::model::style::fmt_num;

/// Stroke appearance.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// CSS color.
    pub color: String,
    /// Width in px.
    pub width: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            color: "#17181c".to_owned(),
            width: 2.0,
        }
    }
}

fn wrap(
    name: &str,
    points: &[(f32, f32)],
    stroke: &Stroke,
    body: impl FnOnce(&dyn Fn((f32, f32)) -> (f32, f32)) -> String,
) -> String {
    let pad = (stroke.width * 4.0 + 6.0).ceil();
    let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let left = (min_x - pad).floor();
    let top = (min_y - pad).floor();
    let width = (max_x + pad).ceil() - left;
    let height = (max_y + pad).ceil() - top;
    let local = move |(x, y): (f32, f32)| ((x - left * 1.0), (y - top));
    let inner = body(&local);
    format!(
        "<svg data-name=\"{name}\" xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" fill=\"none\" style=\"position: absolute; left: {l}px; top: {t}px\">{inner}</svg>",
        w = fmt_num(width),
        h = fmt_num(height),
        l = fmt_num(left),
        t = fmt_num(top),
    )
}

fn stroke_attrs(stroke: &Stroke) -> String {
    format!(
        "stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
        stroke.color,
        fmt_num(stroke.width)
    )
}

fn point(p: (f32, f32)) -> String {
    format!("{} {}", fmt_num(p.0), fmt_num(p.1))
}

/// A straight line (optionally with an arrowhead at `to`) in parent coordinates.
#[must_use]
pub fn line_svg(from: (f32, f32), to: (f32, f32), stroke: &Stroke, arrow: bool) -> String {
    let name = if arrow { "Arrow" } else { "Line" };
    wrap(name, &[from, to], stroke, |local| {
        let a = local(from);
        let b = local(to);
        let mut d = format!("M {} L {}", point(a), point(b));
        if arrow {
            let angle = (b.1 - a.1).atan2(b.0 - a.0);
            let length = stroke.width * 4.0 + 6.0;
            for spread in [0.5_f32, -0.5] {
                let head = (
                    b.0 - length * (angle + spread).cos(),
                    b.1 - length * (angle + spread).sin(),
                );
                d.push_str(&format!(" M {} L {}", point(b), point(head)));
            }
        }
        format!("<path d=\"{d}\" {}/>", stroke_attrs(stroke))
    })
}

/// Ramer–Douglas–Peucker simplification.
#[must_use]
pub fn simplify(points: &[(f32, f32)], epsilon: f32) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let (dx, dy) = (last.0 - first.0, last.1 - first.1);
    let length = (dx * dx + dy * dy).sqrt();
    let distance = |p: (f32, f32)| {
        if length <= f32::EPSILON {
            ((p.0 - first.0).powi(2) + (p.1 - first.1).powi(2)).sqrt()
        } else {
            ((p.0 - first.0) * dy - (p.1 - first.1) * dx).abs() / length
        }
    };
    let (index, max) = points[1..points.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, distance(*p)))
        .fold(
            (0, 0.0_f32),
            |best, item| if item.1 > best.1 { item } else { best },
        );
    if max > epsilon {
        let mut left = simplify(&points[..=index], epsilon);
        let right = simplify(&points[index..], epsilon);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![first, last]
    }
}

/// A smoothed freehand stroke through `points` (parent coordinates).
#[must_use]
pub fn freehand_svg(points: &[(f32, f32)], stroke: &Stroke) -> Option<String> {
    let points = simplify(points, 0.75);
    if points.len() < 2 {
        return None;
    }
    Some(wrap("Drawing", &points, stroke, |local| {
        let local: Vec<(f32, f32)> = points.iter().map(|p| local(*p)).collect();
        let mut d = format!("M {}", point(local[0]));
        if local.len() == 2 {
            d.push_str(&format!(" L {}", point(local[1])));
        } else {
            // Quadratic segments through midpoints give a smooth, compact path.
            for pair in local.windows(2).skip(1) {
                let mid = ((pair[0].0 + pair[1].0) / 2.0, (pair[0].1 + pair[1].1) / 2.0);
                d.push_str(&format!(" Q {} {}", point(pair[0]), point(mid)));
            }
            if let Some(last) = local.last() {
                d.push_str(&format!(" L {}", point(*last)));
            }
        }
        format!("<path d=\"{d}\" {}/>", stroke_attrs(stroke))
    }))
}

fn attr_value<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = source.find(&key)? + key.len();
    let end = source[start..].find('"')?;
    Some(&source[start..start + end])
}

/// The first stroke color and width in an SVG, for the design panel.
#[must_use]
pub fn svg_stroke(source: &str) -> Option<Stroke> {
    let color = attr_value(source, "stroke")?.to_owned();
    let width = attr_value(source, "stroke-width")
        .and_then(|w| w.trim_end_matches("px").parse().ok())
        .unwrap_or(1.0);
    Some(Stroke { color, width })
}

fn replace_attr(source: &str, name: &str, value: &str) -> String {
    let key = format!(" {name}=\"");
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find(&key) {
        let value_start = start + key.len();
        let Some(end) = rest[value_start..].find('"') else {
            break;
        };
        out.push_str(&rest[..value_start]);
        out.push_str(value);
        rest = &rest[value_start + end..];
    }
    out.push_str(rest);
    out
}

/// Recolor every stroke in an SVG (fills of `none` are untouched).
#[must_use]
pub fn set_svg_stroke(source: &str, color: Option<&str>, width: Option<f32>) -> String {
    let mut out = source.to_owned();
    if let Some(color) = color {
        out = replace_attr(&out, "stroke", &crate::model::html::escape_attr(color));
    }
    if let Some(width) = width {
        out = replace_attr(&out, "stroke-width", &fmt_num(width.max(0.0)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::html::{ImportOptions, parse_fragment};
    use crate::model::{Document, NodeKind};

    #[test]
    fn arrows_are_positioned_svg_layers() {
        let html = line_svg((10.0, 20.0), (110.0, 20.0), &Stroke::default(), true);
        assert!(html.starts_with("<svg data-name=\"Arrow\""));
        assert!(html.contains("left: -4px; top: 6px"), "{html}");
        let mut doc = Document::new();
        let roots = parse_fragment(&mut doc, &html, ImportOptions { keep_ids: false });
        let node = doc.get(roots[0]).unwrap();
        assert!(matches!(node.kind, NodeKind::Svg(_)));
        assert_eq!(node.name.as_deref(), Some("Arrow"));
        assert_eq!(node.style.get("position"), Some("absolute"));
    }

    #[test]
    fn freehand_simplifies_and_recolors() {
        let points: Vec<(f32, f32)> = (0..50)
            .map(|i| (i as f32, (i as f32 * 0.3).sin() * 10.0))
            .collect();
        let html = freehand_svg(&points, &Stroke::default()).unwrap();
        assert!(html.contains(" Q "));
        assert!(simplify(&points, 0.75).len() < points.len());
        assert_eq!(
            simplify(&[(0.0, 0.0), (1.0, 0.01), (2.0, 0.0)], 0.5).len(),
            2
        );
        let recolored = set_svg_stroke(&html, Some("#ff0000"), Some(4.0));
        let stroke = svg_stroke(&recolored).unwrap();
        assert_eq!(
            stroke,
            Stroke {
                color: "#ff0000".into(),
                width: 4.0
            }
        );
        assert!(freehand_svg(&[(1.0, 1.0)], &Stroke::default()).is_none());
    }
}
