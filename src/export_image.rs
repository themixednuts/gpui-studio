//! Image export: an artboard as SVG or PNG.
//!
//! The canvas lays the artboard out at 1× (boxes and GPUI's own text line
//! breaks) and hands the geometry to [`to_svg`], which draws backgrounds,
//! borders, radii, shadows, clipping, images, vectors, and text as plain SVG.
//! [`rasterize`] renders that SVG to PNG with resvg using the same fonts.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

use crate::geometry::Rect;
use crate::model::style::{Display, Overflow};
use crate::model::{Color, Document, NodeId, NodeKind};

/// Image formats Studio exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Scalable vector graphics.
    Svg,
    /// Raster PNG.
    Png,
}

impl ImageFormat {
    /// Parse `svg` or `png`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "svg" => Some(Self::Svg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }

    /// File extension.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

/// Style of one run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    /// Fill color.
    pub color: Color,
    /// CSS weight.
    pub weight: u16,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
    /// Struck through.
    pub strike: bool,
}

/// One visual line of a text block.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// Left edge of the line, relative to the artboard.
    pub x: f32,
    /// Baseline, relative to the artboard.
    pub baseline: f32,
    /// Runs in order.
    pub runs: Vec<(String, RunStyle)>,
}

/// A laid-out text layer.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// CSS font stack.
    pub family: String,
    /// Font size in px.
    pub size: f32,
    /// Visual lines.
    pub lines: Vec<TextLine>,
}

/// Loads an `<img src>` as (bytes, mime type).
pub type ImageLoader = dyn Fn(&str) -> Option<(Vec<u8>, &'static str)>;

/// Everything [`to_svg`] needs about a laid-out artboard.
pub struct Scene<'a> {
    /// The document.
    pub doc: &'a Document,
    /// The artboard root.
    pub root: NodeId,
    /// Layer rectangles relative to the artboard, at 1×.
    pub rects: &'a HashMap<NodeId, Rect>,
    /// Text layers' lines.
    pub text: &'a HashMap<NodeId, TextBlock>,
    /// Loads an `<img src>` as (bytes, mime type).
    pub load_image: &'a ImageLoader,
}

fn num(v: f32) -> String {
    crate::model::style::fmt_num((v * 100.0).round() / 100.0)
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn paint(color: Color) -> String {
    if color.a == 255 {
        format!("fill=\"{}\"", hex(color))
    } else {
        format!(
            "fill=\"{}\" fill-opacity=\"{}\"",
            hex(color),
            num(color.alpha())
        )
    }
}

/// A rounded-rectangle path with per-corner radii (tl, tr, br, bl).
fn rounded(r: Rect, radii: [f32; 4]) -> String {
    let max = (r.w.min(r.h) / 2.0).max(0.0);
    let [tl, tr, br, bl] = radii.map(|v| v.clamp(0.0, max));
    if tl + tr + br + bl == 0.0 {
        return format!(
            "M{} {}H{}V{}H{}Z",
            num(r.x),
            num(r.y),
            num(r.right()),
            num(r.bottom()),
            num(r.x)
        );
    }
    format!(
        "M{x0} {y0}H{x1}A{tr} {tr} 0 0 1 {xr} {y1}V{y2}A{br} {br} 0 0 1 {x2} {yb}H{x3}A{bl} {bl} 0 0 1 {xl} {y3}V{y4}A{tl} {tl} 0 0 1 {x0} {y0}Z",
        x0 = num(r.x + tl),
        y0 = num(r.y),
        x1 = num(r.right() - tr),
        tr = num(tr),
        xr = num(r.right()),
        y1 = num(r.y + tr),
        y2 = num(r.bottom() - br),
        br = num(br),
        x2 = num(r.right() - br),
        yb = num(r.bottom()),
        x3 = num(r.x + bl),
        bl = num(bl),
        xl = num(r.x),
        y3 = num(r.bottom() - bl),
        y4 = num(r.y + tl),
        tl = num(tl),
    )
}

struct Writer<'a> {
    scene: &'a Scene<'a>,
    defs: String,
    body: String,
    next_id: usize,
}

impl Writer<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn node(&mut self, id: NodeId, is_root: bool) {
        let doc = self.scene.doc;
        let Some(node) = doc.get(id) else {
            return;
        };
        if node.hidden {
            return;
        }
        let Some(r) = self.scene.rects.get(&id).copied() else {
            return;
        };
        match &node.kind {
            NodeKind::Text(_) => {}
            NodeKind::Svg(source) => {
                let data = base64::engine::general_purpose::STANDARD.encode(source.as_bytes());
                let _ = write!(
                    self.body,
                    "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"data:image/svg+xml;base64,{data}\"/>",
                    num(r.x),
                    num(r.y),
                    num(r.w),
                    num(r.h)
                );
            }
            NodeKind::Element { tag } => {
                let c = doc.computed(node);
                if c.display == Display::None {
                    return;
                }
                let radii = c.resolved_radii(Some(r.w), Some(r.h));
                let opacity = c.opacity.clamp(0.0, 1.0);
                if opacity < 1.0 {
                    let _ = write!(self.body, "<g opacity=\"{}\">", num(opacity));
                }
                for shadow in c.shadows.iter().filter(|s| !s.inset && s.color.a > 0) {
                    let spread = Rect::new(
                        r.x + shadow.x - shadow.spread,
                        r.y + shadow.y - shadow.spread,
                        r.w + shadow.spread * 2.0,
                        r.h + shadow.spread * 2.0,
                    );
                    let grow = radii.map(|v| if v > 0.0 { v + shadow.spread } else { 0.0 });
                    let filter = if shadow.blur > 0.0 {
                        let fid = self.id("blur");
                        let _ = write!(
                            self.defs,
                            "<filter id=\"{fid}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\"><feGaussianBlur stdDeviation=\"{}\"/></filter>",
                            num(shadow.blur / 2.0)
                        );
                        format!(" filter=\"url(#{fid})\"")
                    } else {
                        String::new()
                    };
                    let _ = write!(
                        self.body,
                        "<path d=\"{}\" {}{filter}/>",
                        rounded(spread, grow),
                        paint(shadow.color)
                    );
                }
                if let Some(bg) = c.background.filter(|b| b.a > 0) {
                    let _ = write!(
                        self.body,
                        "<path d=\"{}\" {}/>",
                        rounded(r, radii),
                        paint(bg)
                    );
                } else if is_root {
                    let _ = write!(
                        self.body,
                        "<path d=\"{}\" fill=\"#ffffff\"/>",
                        rounded(r, radii)
                    );
                }
                if tag == "img"
                    && let Some(src) = node.attr("src")
                    && let Some((bytes, mime)) = (self.scene.load_image)(src)
                {
                    let clip = self.id("clip");
                    let _ = write!(
                        self.defs,
                        "<clipPath id=\"{clip}\"><path d=\"{}\"/></clipPath>",
                        rounded(r, radii)
                    );
                    let aspect = match c.object_fit.as_deref() {
                        Some("cover") => "xMidYMid slice",
                        Some("contain" | "scale-down") => "xMidYMid meet",
                        Some("none") => "xMidYMid slice",
                        _ => "none",
                    };
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    let _ = write!(
                        self.body,
                        "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"{aspect}\" clip-path=\"url(#{clip})\" href=\"data:{mime};base64,{data}\"/>",
                        num(r.x),
                        num(r.y),
                        num(r.w),
                        num(r.h)
                    );
                }
                self.border(tag, &c, r, radii);
                if let Some(block) = self.scene.text.get(&id) {
                    self.text(block);
                } else {
                    let clip = c.overflow != Overflow::Visible || is_root;
                    if clip {
                        let cid = self.id("clip");
                        let _ = write!(
                            self.defs,
                            "<clipPath id=\"{cid}\"><path d=\"{}\"/></clipPath>",
                            rounded(r, radii)
                        );
                        let _ = write!(self.body, "<g clip-path=\"url(#{cid})\">");
                    }
                    // Absolutely positioned and z-indexed children paint later.
                    let mut children: Vec<(i32, usize, NodeId)> = node
                        .children
                        .iter()
                        .enumerate()
                        .map(|(i, child)| {
                            let z = doc.get(*child).map_or(0, |n| {
                                let c = doc.computed(n);
                                c.z_index.unwrap_or(0)
                            });
                            (z, i, *child)
                        })
                        .collect();
                    children.sort_by_key(|(z, i, _)| (*z, *i));
                    for (_, _, child) in children {
                        self.node(child, false);
                    }
                    if clip {
                        self.body.push_str("</g>");
                    }
                }
                if opacity < 1.0 {
                    self.body.push_str("</g>");
                }
            }
        }
    }

    fn border(&mut self, tag: &str, c: &crate::model::Computed, r: Rect, radii: [f32; 4]) {
        let styled = c
            .border_style
            .as_deref()
            .is_some_and(|s| !matches!(s, "none" | "hidden"));
        let (widths, color) = if styled {
            (
                c.border_width,
                c.border_color.or(c.color).unwrap_or(Color::BLACK),
            )
        } else if tag == "button" && c.border_style.is_none() {
            ([2.0; 4], Color::rgb(0x76, 0x76, 0x76))
        } else {
            return;
        };
        if widths.iter().all(|w| *w <= 0.0) || color.a == 0 {
            return;
        }
        let dash = if matches!(c.border_style.as_deref(), Some("dashed" | "dotted")) {
            let w = widths.iter().copied().fold(0.0, f32::max);
            format!(" stroke-dasharray=\"{} {}\"", num(w * 3.0), num(w * 2.0))
        } else {
            String::new()
        };
        let stroke = if color.a == 255 {
            format!("stroke=\"{}\"", hex(color))
        } else {
            format!(
                "stroke=\"{}\" stroke-opacity=\"{}\"",
                hex(color),
                num(color.alpha())
            )
        };
        if widths.iter().all(|w| (*w - widths[0]).abs() < 0.01) {
            let w = widths[0];
            let inner = Rect::new(r.x + w / 2.0, r.y + w / 2.0, r.w - w, r.h - w);
            let radii = radii.map(|v| (v - w / 2.0).max(0.0));
            let _ = write!(
                self.body,
                "<path d=\"{}\" fill=\"none\" {stroke} stroke-width=\"{}\"{dash}/>",
                rounded(inner, radii),
                num(w)
            );
            return;
        }
        let [top, right, bottom, left] = widths;
        let fill = paint(color);
        for side in [
            Rect::new(r.x, r.y, r.w, top),
            Rect::new(r.right() - right, r.y, right, r.h),
            Rect::new(r.x, r.bottom() - bottom, r.w, bottom),
            Rect::new(r.x, r.y, left, r.h),
        ] {
            if side.w > 0.0 && side.h > 0.0 {
                let _ = write!(
                    self.body,
                    "<path d=\"{}\" {fill}/>",
                    rounded(side, [0.0; 4])
                );
            }
        }
    }

    fn text(&mut self, block: &TextBlock) {
        for line in &block.lines {
            let _ = write!(
                self.body,
                "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" xml:space=\"preserve\">",
                num(line.x),
                num(line.baseline),
                esc(&block.family),
                num(block.size)
            );
            for (text, style) in &line.runs {
                let mut decoration = Vec::new();
                if style.underline {
                    decoration.push("underline");
                }
                if style.strike {
                    decoration.push("line-through");
                }
                let _ = write!(
                    self.body,
                    "<tspan font-weight=\"{}\"{}{} {}>{}</tspan>",
                    style.weight,
                    if style.italic {
                        " font-style=\"italic\""
                    } else {
                        ""
                    },
                    if decoration.is_empty() {
                        String::new()
                    } else {
                        format!(" text-decoration=\"{}\"", decoration.join(" "))
                    },
                    paint(style.color),
                    esc(text)
                );
            }
            self.body.push_str("</text>");
        }
    }
}

/// Draw a laid-out artboard as a standalone SVG document.
#[must_use]
pub fn to_svg(scene: &Scene<'_>) -> String {
    let root = scene.rects.get(&scene.root).copied().unwrap_or_default();
    let mut writer = Writer {
        scene,
        defs: String::new(),
        body: String::new(),
        next_id: 0,
    };
    writer.node(scene.root, scene.doc.is_artboard(scene.root));
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {w} {h}\"><defs>{}</defs>{}</svg>\n",
        writer.defs,
        writer.body,
        x = num(root.x),
        y = num(root.y),
        w = num(root.w),
        h = num(root.h),
    )
}

/// Render an SVG to PNG at `scale` with the given font files loaded.
pub fn rasterize(svg: &str, scale: f32, fonts: &[Vec<u8>]) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8");
    }
    let mut options = resvg::usvg::Options::default();
    let db = options.fontdb_mut();
    db.load_system_fonts();
    for font in fonts {
        db.load_font_data(font.clone());
    }
    db.set_sans_serif_family("Geist");
    let tree = resvg::usvg::Tree::from_str(svg, &options).context("parse exported SVG")?;
    let size = tree.size();
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 16_384 * 16_384 {
        bail!("export size {width}×{height} is out of range");
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).context("allocate image")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().context("encode PNG")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::html::{ImportOptions, parse_fragment};

    #[test]
    fn svg_draws_boxes_text_and_images_and_rasterizes() {
        let mut doc = Document::new();
        let board = doc.create_artboard(0, "Card", (0.0, 0.0), (200.0, 120.0));
        let ids = parse_fragment(
            &mut doc,
            r#"<div style="background-color: #0d99ff; border-radius: 12px; border: 2px solid #111; box-shadow: 0 4px 8px rgba(0,0,0,0.2); opacity: 0.5"><p>Hi</p></div><img src="a.png" style="object-fit: cover; border-radius: 50%">"#,
            ImportOptions { keep_ids: false },
        );
        for id in &ids {
            doc.attach(*id, board, None).unwrap();
        }
        let (card, image) = (ids[0], ids[1]);
        let text = doc.children(card)[0];
        let rects = HashMap::from([
            (board, Rect::new(0.0, 0.0, 200.0, 120.0)),
            (card, Rect::new(10.0, 10.0, 100.0, 60.0)),
            (text, Rect::new(20.0, 20.0, 40.0, 20.0)),
            (image, Rect::new(120.0, 10.0, 40.0, 40.0)),
        ]);
        let blocks = HashMap::from([(
            text,
            TextBlock {
                family: "Geist, sans-serif".into(),
                size: 16.0,
                lines: vec![TextLine {
                    x: 20.0,
                    baseline: 34.0,
                    runs: vec![(
                        "Hi & <you>".into(),
                        RunStyle {
                            color: Color::BLACK,
                            weight: 700,
                            italic: false,
                            underline: true,
                            strike: false,
                        },
                    )],
                }],
            },
        )]);
        let png = super::super::assets::tests_png();
        let load = move |src: &str| (src == "a.png").then(|| (png.clone(), "image/png"));
        let svg = to_svg(&Scene {
            doc: &doc,
            root: board,
            rects: &rects,
            text: &blocks,
            load_image: &load,
        });
        assert!(
            svg.starts_with(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"120\""
            )
        );
        assert!(svg.contains("fill=\"#0d99ff\""), "{svg}");
        assert!(svg.contains("A12 12"), "rounded corners");
        assert!(svg.contains("stroke=\"#111111\" stroke-width=\"2\""));
        assert!(svg.contains("feGaussianBlur"));
        assert!(svg.contains("<g opacity=\"0.5\">"));
        assert!(svg.contains("Hi &amp; &lt;you&gt;"));
        assert!(svg.contains("text-decoration=\"underline\""));
        assert!(svg.contains("xMidYMid slice") && svg.contains("data:image/png;base64,"));
        let raster = rasterize(&svg, 2.0, &[]).unwrap();
        assert!(raster.starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(
            crate::assets::image_size(&raster, "png"),
            Some((400.0, 240.0))
        );
        assert!(rasterize(&svg, 20.0, &[]).is_err());
        assert_eq!(ImageFormat::parse("PNG"), Some(ImageFormat::Png));
    }
}
