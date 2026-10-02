//! PNG/SVG export: lay the artboard out once at 1× off-screen, read back the
//! exact boxes and GPUI's text lines, and hand them to [`crate::export_image`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, ClipboardItem, Context, FontStyle, HighlightStyle,
    IntoElement as _, ParentElement as _, Pixels, Point, Rgba, Styled as _, Window, div, px,
};

use super::Studio;
use super::paint::{LayoutMap, Painter, TextCapture};
use crate::export_image::{ImageFormat, RunStyle, Scene, TextBlock, TextLine, rasterize, to_svg};
use crate::geometry::Rect;
use crate::model::style::TextAlign;
use crate::model::{Color, Document, NodeId, NodeKind};

/// Where a finished export goes.
#[derive(Clone, Debug)]
pub(crate) enum ExportTarget {
    /// Write to a file.
    File(PathBuf),
    /// Put a PNG on the clipboard.
    Clipboard,
}

/// An export waiting for its 1× layout.
pub(crate) struct ExportJob {
    node: NodeId,
    format: ImageFormat,
    scale: f32,
    target: ExportTarget,
    layout: LayoutMap,
    texts: RefCell<HashMap<NodeId, TextCapture>>,
    frames: u8,
    unsettled: bool,
}

fn color_of(hsla: gpui_kit::Hsla) -> Color {
    let rgba = Rgba::from(hsla);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::rgba(byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a))
}

fn ua_bold(tag: &str) -> bool {
    matches!(
        tag,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "b" | "strong" | "th"
    )
}

/// The inherited text style of a text layer (what the canvas cascade gives it).
fn base_style(doc: &Document, id: NodeId) -> (String, RunStyle, TextAlign) {
    let mut family = None;
    let mut color = None;
    let mut weight = None;
    let mut italic = None;
    let mut underline = None;
    let mut strike = None;
    let mut align = None;
    for n in std::iter::once(id).chain(doc.ancestors(id)) {
        let Some(node) = doc.get(n) else {
            continue;
        };
        let NodeKind::Element { tag } = &node.kind else {
            continue;
        };
        let c = doc.computed(node);
        family = family.or(c.font_family.clone());
        color = color.or(c.color);
        weight = weight.or(c.font_weight).or(ua_bold(tag).then_some(700));
        italic = italic
            .or(c.italic)
            .or(matches!(tag.as_str(), "em" | "i").then_some(true));
        underline = underline.or(c.underline).or((tag == "u").then_some(true));
        strike = strike.or(c.strikethrough);
        if align.is_none() && c.text_align != TextAlign::Inherit {
            align = Some(c.text_align);
        }
    }
    (
        family.unwrap_or_else(|| "Geist, sans-serif".to_owned()),
        RunStyle {
            color: color.unwrap_or(Color::BLACK),
            weight: weight.unwrap_or(400),
            italic: italic.unwrap_or(false),
            underline: underline.unwrap_or(false),
            strike: strike.unwrap_or(false),
        },
        align.unwrap_or(TextAlign::Left),
    )
}

fn apply(base: &RunStyle, h: &HighlightStyle) -> RunStyle {
    RunStyle {
        color: h.color.map_or(base.color, color_of),
        weight: h.font_weight.map_or(base.weight, |w| w.0.round() as u16),
        italic: h.font_style.map_or(base.italic, |s| s == FontStyle::Italic),
        underline: base.underline || h.underline.is_some(),
        strike: base.strike || h.strikethrough.is_some(),
    }
}

/// Convert a captured GPUI text layout into SVG lines relative to `origin`.
fn text_block(
    doc: &Document,
    id: NodeId,
    capture: &TextCapture,
    origin: Point<Pixels>,
) -> Option<TextBlock> {
    let layout = &capture.layout;
    let bounds = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout.bounds())).ok()?;
    let line_height = layout.line_height().as_f32();
    let (family, base, align) = base_style(doc, id);
    let mut lines = Vec::new();
    let mut y = (bounds.origin.y - origin.y).as_f32();
    let mut offset = 0;
    let mut size = 16.0;
    for hard in layout.line_layouts() {
        let unwrapped = &hard.unwrapped_layout;
        size = unwrapped.font_size.as_f32();
        let (ascent, descent) = (unwrapped.ascent.as_f32(), unwrapped.descent.as_f32());
        let glyph = |run: usize, glyph: usize| {
            unwrapped
                .runs
                .get(run)
                .and_then(|r| r.glyphs.get(glyph))
                .map(|g| (g.index, g.position.x.as_f32()))
        };
        let mut breaks: Vec<(usize, f32)> = vec![(0, 0.0)];
        for boundary in &hard.wrap_boundaries {
            if let Some(b) = glyph(boundary.run_ix, boundary.glyph_ix) {
                breaks.push(b);
            }
        }
        breaks.push((unwrapped.len, unwrapped.width.as_f32()));
        for pair in breaks.windows(2) {
            let ((start, x0), (end, x1)) = (pair[0], pair[1]);
            let width = x1 - x0;
            let free = bounds.size.width.as_f32() - width;
            let shift = match align {
                TextAlign::Center => free / 2.0,
                TextAlign::Right => free,
                _ => 0.0,
            };
            let (a, b) = (offset + start, offset + end);
            // Split the segment at style-run boundaries.
            let mut cuts = vec![a, b];
            for (range, _) in &capture.runs {
                for cut in [range.start, range.end] {
                    if cut > a && cut < b {
                        cuts.push(cut);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            let mut runs = Vec::new();
            for piece in cuts.windows(2) {
                let (p0, p1) = (piece[0], piece[1]);
                let Some(text) = capture.text.get(p0..p1) else {
                    continue;
                };
                let text = text.trim_end_matches('\n');
                if text.is_empty() {
                    continue;
                }
                let style = capture
                    .runs
                    .iter()
                    .find(|(range, _)| range.start <= p0 && p1 <= range.end)
                    .map_or_else(|| base.clone(), |(_, h)| apply(&base, h));
                runs.push((text.to_owned(), style));
            }
            if let Some(first) = runs.first_mut() {
                first.0 = first.0.trim_start().to_owned();
            }
            lines.push(TextLine {
                x: (bounds.origin.x - origin.x).as_f32() + shift,
                baseline: y + (line_height - (ascent + descent)) / 2.0 + ascent,
                runs,
            });
            y += line_height;
        }
        offset += unwrapped.len + 1;
    }
    Some(TextBlock {
        family,
        size,
        lines,
    })
}

impl Studio {
    /// Export a layer (artboards and anything inside them) as an image.
    pub(crate) fn start_image_export(
        &mut self,
        node: NodeId,
        format: ImageFormat,
        scale: f32,
        target: ExportTarget,
        cx: &mut Context<Self>,
    ) {
        self.export_job = Some(ExportJob {
            node,
            format,
            scale: scale.clamp(0.1, 8.0),
            target,
            layout: LayoutMap::default(),
            texts: RefCell::new(HashMap::new()),
            frames: 0,
            unsettled: false,
        });
        cx.notify();
    }

    /// Default file for an export of `node`: `exports/<name>[@Nx].<ext>`.
    pub(crate) fn export_path(
        &self,
        node: NodeId,
        format: ImageFormat,
        scale: f32,
    ) -> Option<PathBuf> {
        let project = self.editor.project.as_ref()?;
        let mut slug: String = self
            .editor
            .doc
            .display_name(node)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "export" } else { slug };
        let suffix = if format == ImageFormat::Png && (scale - 1.0).abs() > f32::EPSILON {
            format!("@{}x", crate::model::style::fmt_num(scale))
        } else {
            String::new()
        };
        Some(
            project
                .root()
                .join("exports")
                .join(format!("{slug}{suffix}.{}", format.extension())),
        )
    }

    /// The off-screen 1× stage for a pending export (rendered by the canvas).
    pub(super) fn render_export_stage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The stage is laid out by the frame that renders it, so its geometry
        // is readable on the next render. Measurement-dependent layout (auto
        // fill grids) gets a couple more frames to settle.
        let job = self.export_job.as_mut()?;
        job.frames += 1;
        if job.frames > 2 && (!job.unsettled || job.frames > 4) {
            self.finish_export(window, cx);
            return None;
        }
        let job = self.export_job.as_ref()?;
        let doc = &self.editor.doc;
        let root = doc.root_of(job.node);
        if !doc.is_artboard(root) {
            self.export_job = None;
            return None;
        }
        let asset_dir = self.asset_dir();
        let cache_dir = self.cache_dir();
        let measured = job.layout.borrow().clone();
        let painter = Painter {
            doc,
            zoom: 1.0,
            asset_dir: &asset_dir,
            cache_dir: &cache_dir,
            layout: &job.layout,
            fonts: &self.fonts,
            editing: None,
            unsettled: std::cell::Cell::new(false),
            measured: &measured,
            texts: Some(&job.texts),
        };
        let element = painter.artboard(root)?;
        let unsettled = painter.unsettled.get();
        if let Some(job) = &mut self.export_job {
            job.unsettled = unsettled;
        }
        window.request_animation_frame();
        Some(
            div()
                .absolute()
                .left(px(-200_000.0))
                .top(px(0.0))
                .child(element)
                .into_any_element(),
        )
    }

    fn finish_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.export_job.take() else {
            return;
        };
        let doc = &self.editor.doc;
        let root = doc.root_of(job.node);
        let layout = job.layout.borrow();
        let Some(origin) = layout.get(&root).map(|b| b.origin) else {
            window.push_notification(
                Notification::error("Export failed: the artboard was not laid out."),
                cx,
            );
            return;
        };
        let to_rect = |b: &Bounds<Pixels>| {
            Rect::new(
                (b.origin.x - origin.x).as_f32(),
                (b.origin.y - origin.y).as_f32(),
                b.size.width.as_f32(),
                b.size.height.as_f32(),
            )
        };
        let rects: HashMap<NodeId, Rect> = layout.iter().map(|(id, b)| (*id, to_rect(b))).collect();
        let texts: HashMap<NodeId, TextBlock> = job
            .texts
            .borrow()
            .iter()
            .filter_map(|(id, capture)| Some((*id, text_block(doc, *id, capture, origin)?)))
            .collect();
        let asset_dir = self.asset_dir();
        let load = move |src: &str| -> Option<(Vec<u8>, &'static str)> {
            if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:")
            {
                return None;
            }
            let path = asset_dir.join(src);
            let mime = match crate::assets::extension(src)?.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "svg" => "image/svg+xml",
                "bmp" => "image/bmp",
                _ => return None,
            };
            let meta = std::fs::metadata(&path).ok()?;
            (meta.len() <= crate::assets::MAX_IMAGE_BYTES as u64)
                .then(|| std::fs::read(&path).ok())
                .flatten()
                .map(|bytes| (bytes, mime))
        };
        let svg = to_svg(&Scene {
            doc,
            root: job.node,
            rects: &rects,
            text: &texts,
            load_image: &load,
        });
        drop(layout);
        let name = doc.display_name(job.node);
        let (format, scale, target) = (job.format, job.scale, job.target);
        let label = job_target_label(&target);
        let fonts: Vec<Vec<u8>> = super::bundled_fonts()
            .into_iter()
            .map(|f| f.into_owned())
            .collect();
        let task = cx.background_spawn(async move {
            let bytes = match format {
                ImageFormat::Svg => svg.into_bytes(),
                ImageFormat::Png => rasterize(&svg, scale, &fonts)?,
            };
            if let ExportTarget::File(path) = &target {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, &bytes)?;
            }
            anyhow::Ok(bytes)
        });
        let target = label;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(bytes) => {
                    let message = match &target {
                        None => {
                            cx.write_to_clipboard(ClipboardItem::new_image(
                                &gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, bytes),
                            ));
                            format!("Copied {name} as PNG")
                        }
                        Some(path) => {
                            let shown = this
                                .editor
                                .project
                                .as_ref()
                                .and_then(|p| path.strip_prefix(p.root()).ok())
                                .unwrap_or(path)
                                .display()
                                .to_string();
                            format!("Exported {shown}")
                        }
                    };
                    this.editor.status = message.clone();
                    window.push_notification(Notification::success(message), cx);
                }
                Err(error) => {
                    window.push_notification(
                        Notification::error(format!("Export failed: {error:#}")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(crate) fn asset_dir(&self) -> PathBuf {
        self.editor
            .project
            .as_ref()
            .map(|p| p.root().join("artboards"))
            .unwrap_or_else(std::env::temp_dir)
    }

    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.editor
            .project
            .as_ref()
            .map(|p| p.studio_dir().join("cache"))
            .unwrap_or_else(|| std::env::temp_dir().join("gpui-studio-cache"))
    }
}

fn job_target_label(target: &ExportTarget) -> Option<PathBuf> {
    match target {
        ExportTarget::File(path) => Some(path.clone()),
        ExportTarget::Clipboard => None,
    }
}
