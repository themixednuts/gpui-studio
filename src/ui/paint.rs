//! Renders design nodes as native GPUI elements.
//!
//! Each element node becomes a `div` styled from its computed CSS, scaled by
//! the canvas zoom. Text layers become one `StyledText` with highlight runs
//! for inline formatting. Every element records its laid-out bounds so the
//! canvas can hit-test, draw selection, and place handles.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::{
    AbsoluteLength, AlignItems, AnimationExt as _, AnyElement, BorderStyle, Bounds, BoxShadow,
    DefiniteLength, Div, FontStyle, FontWeight, HighlightStyle, Hsla, IntoElement,
    Length as GLength, ObjectFit, ParentElement as _, Pixels, SharedString, Stateful,
    StrikethroughStyle, Styled as _, StyledImage as _, StyledText, UnderlineStyle, canvas, div,
    img, point, prelude::*, px, relative,
};

use crate::model::grid::{ColumnPlacement, Track, TrackList, place};
use crate::model::style::{
    Align, Computed, Display, Length, LineHeight, Overflow, Position, TextAlign,
};
use crate::model::{Color, Document, Node, NodeId, NodeKind};

/// Laid-out bounds of every rendered element, in window coordinates.
pub(crate) type LayoutMap = Rc<RefCell<HashMap<NodeId, Bounds<Pixels>>>>;

/// Convert a model color.
#[must_use]
pub(crate) fn hsla(color: Color) -> Hsla {
    gpui_kit::rgba(color.to_u32()).into()
}

/// Installed font families, used to resolve CSS font stacks.
#[derive(Default)]
pub(crate) struct FontBook {
    families: BTreeSet<String>,
    lower: HashMap<String, String>,
}

impl FontBook {
    /// Build from the platform's font list.
    #[must_use]
    pub(crate) fn new(names: Vec<String>) -> Self {
        let mut book = Self::default();
        for name in names {
            book.lower.insert(name.to_ascii_lowercase(), name.clone());
            book.families.insert(name);
        }
        book
    }

    /// Families for menus.
    pub(crate) fn families(&self) -> impl Iterator<Item = &String> {
        self.families.iter()
    }

    /// Resolve a CSS `font-family` stack to one installed family.
    #[must_use]
    pub(crate) fn resolve(&self, stack: &str) -> SharedString {
        for raw in stack.split(',') {
            let name = raw.trim().trim_matches(['"', '\'']);
            let lowered = name.to_ascii_lowercase();
            match lowered.as_str() {
                "sans-serif" | "system-ui" | "ui-sans-serif" | "-apple-system"
                | "blinkmacsystemfont" | "inherit" => return "Geist".into(),
                "monospace" | "ui-monospace" => return "Geist Mono".into(),
                "serif" | "ui-serif" => {
                    for candidate in [
                        "Georgia",
                        "Times New Roman",
                        "DejaVu Serif",
                        "Liberation Serif",
                        "Noto Serif",
                    ] {
                        if let Some(found) = self.lower.get(&candidate.to_ascii_lowercase()) {
                            return found.clone().into();
                        }
                    }
                    return "Geist".into();
                }
                _ => {}
            }
            if let Some(found) = self.lower.get(&lowered) {
                return found.clone().into();
            }
        }
        "Geist".into()
    }
}

/// Inputs to one paint pass.
pub(crate) struct Painter<'a> {
    /// Document.
    pub doc: &'a Document,
    /// Canvas zoom factor.
    pub zoom: f32,
    /// Directory relative `src` paths resolve against.
    pub asset_dir: &'a Path,
    /// Where inline SVGs are materialized for the image loader.
    pub cache_dir: &'a Path,
    /// Bounds sink.
    pub layout: &'a LayoutMap,
    /// Font resolution.
    pub fonts: &'a FontBook,
    /// A text layer being edited in place (its text is hidden).
    pub editing: Option<NodeId>,
    /// Set when layout depended on measurements this frame did not have yet,
    /// so the caller should render one more frame.
    pub unsettled: std::cell::Cell<bool>,
    /// Bounds from the previous frame (window coordinates).
    pub measured: &'a HashMap<NodeId, Bounds<Pixels>>,
    /// When set, text layers record their laid-out text here (image export).
    pub texts: Option<&'a RefCell<HashMap<NodeId, TextCapture>>>,
    /// Shared-element offsets (screen px) to animate away, and a key that
    /// restarts the animation.
    pub morph: Option<(&'a HashMap<NodeId, (f32, f32)>, usize)>,
}

/// A text layer's GPUI layout, kept for image export.
pub(crate) struct TextCapture {
    /// The laid-out text (valid after prepaint).
    pub layout: gpui_kit::TextLayout,
    /// The text that was shaped.
    pub text: String,
    /// Inline style runs over `text`.
    pub runs: Vec<(Range<usize>, HighlightStyle)>,
}

#[derive(Clone, Default)]
struct Inherited {
    transform: Option<String>,
    /// The parent lays children out in a horizontal flex line.
    in_flex_row: bool,
}

fn element_id(id: NodeId) -> SharedString {
    format!("node-{id}").into()
}

fn ua_font_size(tag: &str) -> Option<f32> {
    Some(match tag {
        "h1" => 32.0,
        "h2" => 24.0,
        "h3" => 18.72,
        "h4" => 16.0,
        "h5" => 13.28,
        "h6" => 10.72,
        "small" => 13.33,
        _ => return None,
    })
}

fn ua_bold(tag: &str) -> bool {
    matches!(
        tag,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "b" | "strong" | "th"
    )
}

fn ua_margin(tag: &str) -> Option<f32> {
    match tag {
        "p" | "ul" | "ol" | "blockquote" | "h4" => Some(16.0),
        "h1" => Some(21.44),
        "h2" => Some(19.92),
        "h3" => Some(18.72),
        "h5" => Some(22.18),
        "h6" => Some(24.97),
        _ => None,
    }
}

fn transform_text(text: &str, transform: Option<&str>) -> String {
    match transform {
        Some("uppercase") => text.to_uppercase(),
        Some("lowercase") => text.to_lowercase(),
        Some("capitalize") => text
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                })
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => text.to_owned(),
    }
}

fn align_items(align: Align) -> Option<AlignItems> {
    Some(match align {
        Align::Start => AlignItems::FlexStart,
        Align::Center => AlignItems::Center,
        Align::End => AlignItems::FlexEnd,
        Align::Stretch => AlignItems::Stretch,
        Align::Baseline => AlignItems::Baseline,
        _ => return None,
    })
}

fn justify(align: Align) -> Option<gpui_kit::JustifyContent> {
    use gpui_kit::JustifyContent as J;
    Some(match align {
        Align::Start => J::FlexStart,
        Align::Center => J::Center,
        Align::End => J::FlexEnd,
        Align::SpaceBetween => J::SpaceBetween,
        Align::SpaceAround => J::SpaceAround,
        Align::SpaceEvenly => J::SpaceEvenly,
        Align::Stretch => J::Stretch,
        _ => return None,
    })
}

impl Painter<'_> {
    fn len(&self, length: Length) -> Option<GLength> {
        match length {
            Length::Px(value) => Some(px(value * self.zoom).into()),
            Length::Percent(value) => Some(relative(value / 100.0).into()),
            Length::Auto | Length::Fit => None,
        }
    }

    fn margin_len(&self, length: Length) -> GLength {
        match length {
            Length::Auto => GLength::Auto,
            other => self.len(other).unwrap_or(GLength::Auto),
        }
    }

    fn px(&self, value: f32) -> Pixels {
        px(value * self.zoom)
    }

    fn probe(&self, id: NodeId) -> impl IntoElement {
        let layout = self.layout.clone();
        canvas(
            move |bounds, _, _| {
                layout.borrow_mut().insert(id, bounds);
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Render an artboard root (sized and clipped like a page).
    pub(crate) fn artboard(&self, root: NodeId) -> Option<AnyElement> {
        self.node(root, &Inherited::default(), true)
    }

    fn node(&self, id: NodeId, inherited: &Inherited, is_root: bool) -> Option<AnyElement> {
        let node = self.doc.get(id)?;
        if node.hidden {
            return None;
        }
        match &node.kind {
            NodeKind::Text(text) => {
                if text.trim().is_empty() {
                    return None;
                }
                let text = transform_text(text, inherited.transform.as_deref());
                Some(div().child(text).into_any_element())
            }
            NodeKind::Svg(source) => Some(self.svg(id, node, source)),
            NodeKind::Element { tag } => {
                let computed = self.doc.computed(node);
                if computed.display == Display::None {
                    return None;
                }
                Some(self.element(id, node, tag, &computed, inherited, is_root))
            }
        }
    }

    fn styled(
        &self,
        id: NodeId,
        node: &Node,
        tag: &str,
        c: &Computed,
        is_root: bool,
    ) -> Stateful<Div> {
        // Agents find layers by name through the semantic tree.
        let mut el = div()
            .id(element_id(id))
            .role(match tag {
                "img" => gpui_kit::Role::Image,
                "button" => gpui_kit::Role::Button,
                "a" => gpui_kit::Role::Link,
                _ if self.doc.is_text_layer(id) => gpui_kit::Role::Paragraph,
                _ => gpui_kit::Role::Group,
            })
            .aria_label(self.doc.display_name(id))
            .aria_description(format!("<{tag}> layer {id}"));
        let mut emulated_grid = false;
        // Layout model. Block flow stacks children like a column flexbox whose
        // items stretch, which matches CSS block layout for non-inline boxes.
        match c.display {
            Display::Flex => {
                el = el.flex();
                el = match c.direction {
                    crate::model::style::Direction::Row => el.flex_row(),
                    crate::model::style::Direction::Column => el.flex_col(),
                    crate::model::style::Direction::RowReverse => el.flex_row_reverse(),
                    crate::model::style::Direction::ColumnReverse => el.flex_col_reverse(),
                };
                if c.wrap {
                    el = el.flex_wrap();
                }
            }
            Display::Grid if self.grid_plan(id, c).is_some() => {
                // Rows of flex items emulate track lists GPUI cannot express.
                el = el.flex().flex_col();
                emulated_grid = true;
            }
            Display::Grid => {
                el = el.grid();
                let columns = c
                    .grid_template_columns
                    .as_deref()
                    .and_then(TrackList::parse)
                    .and_then(|list| list.uniform_fr())
                    .or(c.grid_columns)
                    .unwrap_or(1);
                el = el.grid_cols(columns);
                if let Some(rows) = c.grid_rows {
                    el = el.grid_rows(rows);
                }
            }
            Display::Block | Display::Inline | Display::None => {
                el = el.flex().flex_col();
            }
        }
        if let Some(span) = c.column_span {
            el = el.col_span(span);
        }
        if let Some(span) = c.row_span {
            el = el.row_span(span);
        }
        {
            let style = el.style();
            style.align_items = align_items(c.align_items);
            style.align_self = align_items(c.align_self);
            style.justify_content = justify(c.justify);
            if emulated_grid {
                // Rows stretch; grid alignment is applied inside each row.
                style.align_items = Some(AlignItems::Stretch);
                style.justify_content = None;
            }
            style.gap.width = Some(DefiniteLength::from(self.px(c.column_gap)));
            style.gap.height = Some(DefiniteLength::from(self.px(c.row_gap)));
            let [pt, pr, pb, pl] = c.padding;
            style.padding.top = Some(self.px(pt).into());
            style.padding.right = Some(self.px(pr).into());
            style.padding.bottom = Some(self.px(pb).into());
            style.padding.left = Some(self.px(pl).into());
            if !is_root {
                let mut margin = c.margin;
                if node.style.iter().all(|(p, _)| !p.starts_with("margin"))
                    && let Some(ua) = ua_margin(tag)
                {
                    margin[0] = Length::Px(ua);
                    margin[2] = Length::Px(ua);
                }
                style.margin.top = Some(self.margin_len(margin[0]));
                style.margin.right = Some(self.margin_len(margin[1]));
                style.margin.bottom = Some(self.margin_len(margin[2]));
                style.margin.left = Some(self.margin_len(margin[3]));
            }
            if let Some(width) = self.len(c.width) {
                style.size.width = Some(width);
            }
            if let Some(height) = self.len(c.height) {
                style.size.height = Some(height);
            }
            if let Some(value) = self.len(c.min_width) {
                style.min_size.width = Some(value);
            }
            if let Some(value) = self.len(c.min_height) {
                style.min_size.height = Some(value);
            }
            if let Some(value) = self.len(c.max_width) {
                style.max_size.width = Some(value);
            }
            if let Some(value) = self.len(c.max_height) {
                style.max_size.height = Some(value);
            }
            style.flex_grow = Some(c.grow);
            style.flex_shrink = Some(c.shrink);
            if let Some(basis) = self.len(c.basis) {
                style.flex_basis = Some(basis);
            }
            if c.position == Position::Absolute && !is_root {
                style.position = Some(gpui_kit::Position::Absolute);
                for (slot, value) in [
                    (&mut style.inset.top, c.inset[0]),
                    (&mut style.inset.right, c.inset[1]),
                    (&mut style.inset.bottom, c.inset[2]),
                    (&mut style.inset.left, c.inset[3]),
                ] {
                    if let Some(length) = self.len(value) {
                        *slot = Some(length);
                    }
                }
            } else {
                style.position = Some(gpui_kit::Position::Relative);
                if c.position == Position::Relative && !is_root {
                    for (slot, value) in [
                        (&mut style.inset.top, c.inset[0]),
                        (&mut style.inset.left, c.inset[3]),
                    ] {
                        if let Some(length) = self.len(value) {
                            *slot = Some(length);
                        }
                    }
                }
            }
            let has_border = c
                .border_style
                .as_deref()
                .is_some_and(|s| !matches!(s, "none" | "hidden"));
            if tag == "button" && c.border_style.is_none() {
                // Browsers draw a 2px outset border on unstyled buttons.
                let width = Some(AbsoluteLength::from(self.px(2.0)));
                style.border_widths.top = width;
                style.border_widths.right = width;
                style.border_widths.bottom = width;
                style.border_widths.left = width;
                style.border_color = Some(hsla(Color::rgb(0x76, 0x76, 0x76)));
            }
            if has_border {
                let [bt, br, bb, bl] = c.border_width;
                style.border_widths.top = Some(AbsoluteLength::from(self.px(bt)));
                style.border_widths.right = Some(AbsoluteLength::from(self.px(br)));
                style.border_widths.bottom = Some(AbsoluteLength::from(self.px(bb)));
                style.border_widths.left = Some(AbsoluteLength::from(self.px(bl)));
                style.border_color = Some(hsla(c.border_color.or(c.color).unwrap_or(Color::BLACK)));
                if matches!(c.border_style.as_deref(), Some("dashed" | "dotted")) {
                    style.border_style = Some(BorderStyle::Dashed);
                }
            }
            let [tl, tr, br, bl] = c.resolved_radii(c.width.px(), c.height.px());
            style.corner_radii.top_left = Some(AbsoluteLength::from(self.px(tl)));
            style.corner_radii.top_right = Some(AbsoluteLength::from(self.px(tr)));
            style.corner_radii.bottom_right = Some(AbsoluteLength::from(self.px(br)));
            style.corner_radii.bottom_left = Some(AbsoluteLength::from(self.px(bl)));
            if c.opacity < 1.0 {
                style.opacity = Some(c.opacity.clamp(0.0, 1.0));
            }
        }
        if let Some(background) = c.background.filter(|b| b.a > 0) {
            el = el.bg(hsla(background));
        }
        if !c.shadows.is_empty() {
            el = el.shadow(
                c.shadows
                    .iter()
                    .map(|s| BoxShadow {
                        color: hsla(s.color),
                        offset: point(self.px(s.x), self.px(s.y)),
                        blur_radius: self.px(s.blur / 2.0),
                        spread_radius: self.px(s.spread),
                        inset: s.inset,
                    })
                    .collect(),
            );
        }
        if c.overflow != Overflow::Visible || is_root {
            el = el.overflow_hidden();
        }
        // Text properties inherit through GPUI's text style cascade.
        if let Some(color) = c.color {
            el = el.text_color(hsla(color));
        }
        if let Some(size) = c.font_size.or_else(|| ua_font_size(tag)) {
            el = el.text_size(self.px(size));
        }
        if let Some(weight) = c.font_weight {
            el = el.font_weight(FontWeight(f32::from(weight)));
        } else if ua_bold(tag) {
            el = el.font_weight(FontWeight::BOLD);
        }
        if let Some(family) = &c.font_family {
            el = el.font_family(self.fonts.resolve(family));
        }
        match c.italic {
            Some(true) => el = el.italic(),
            Some(false) => el = el.not_italic(),
            None if matches!(tag, "em" | "i") => el = el.italic(),
            None => {}
        }
        match c.line_height {
            Some(LineHeight::Px(value)) => el = el.line_height(self.px(value)),
            Some(LineHeight::Relative(value)) => el = el.line_height(relative(value)),
            None => {}
        }
        match c.text_align {
            TextAlign::Center => el = el.text_center(),
            TextAlign::Right => el = el.text_right(),
            TextAlign::Left => el = el.text_left(),
            TextAlign::Inherit => {}
        }
        if c.underline == Some(true) || (tag == "u" && c.underline.is_none()) {
            el = el.underline();
        }
        if c.strikethrough == Some(true) {
            el = el.line_through();
        }
        if c.nowrap == Some(true) {
            el = el.whitespace_nowrap();
        }
        if is_root {
            // Browser defaults for the page: black 16px text, `normal` line height.
            if c.color.is_none() {
                el = el.text_color(hsla(Color::BLACK));
            }
            if c.font_size.is_none() {
                el = el.text_size(self.px(16.0));
            }
            if c.line_height.is_none() {
                el = el.line_height(relative(1.25));
            }
            if c.font_family.is_none() {
                el = el.font_family("Geist");
            }
            if c.font_weight.is_none() {
                el = el.font_weight(FontWeight::NORMAL);
            }
        }
        el
    }

    fn element(
        &self,
        id: NodeId,
        node: &Node,
        tag: &str,
        c: &Computed,
        inherited: &Inherited,
        is_root: bool,
    ) -> AnyElement {
        let mut el = self.styled(id, node, tag, c, is_root);
        let in_flex_row_item = inherited.in_flex_row;
        let inherited = Inherited {
            transform: c
                .text_transform
                .clone()
                .or_else(|| inherited.transform.clone()),
            in_flex_row: c.display == Display::Flex && !c.direction.is_column(),
        };
        if in_flex_row_item && c.min_width == Length::Auto {
            // CSS `min-width: auto` lets a flex item shrink to its min-content
            // width so text wraps; taffy measures text at max-content instead.
            el.style().min_size.width = Some(px(0.0).into());
        }
        match tag {
            "img" => {
                el = el.child(self.image(node, c));
            }
            "input" | "select" => {
                let value = node.attr("value").map(ToOwned::to_owned);
                let (text, placeholder) = match value {
                    Some(value) => (value, false),
                    None => (node.attr("placeholder").unwrap_or("").to_owned(), true),
                };
                let text_el = div().child(text);
                el = el.child(if placeholder {
                    text_el.opacity(0.5).into_any_element()
                } else {
                    text_el.into_any_element()
                });
            }
            "hr" => {
                if c.height == Length::Auto && c.border_width == [0.0; 4] {
                    el = el.h(self.px(1.0)).bg(gpui_kit::rgb(0xd4d4d4));
                }
            }
            _ => {
                if self.doc.is_text_layer(id) {
                    let (text, runs) = self.inline_runs(id, &inherited);
                    let captured = self.texts.map(|_| (text.clone(), runs.clone()));
                    let text = StyledText::new(text).with_highlights(runs);
                    if let (Some(texts), Some((source, runs))) = (self.texts, captured) {
                        texts.borrow_mut().insert(
                            id,
                            TextCapture {
                                layout: text.layout().clone(),
                                text: source,
                                runs,
                            },
                        );
                    }
                    if self.editing == Some(id) {
                        // Keep the layout while the in-place editor covers it.
                        el = el.child(div().opacity(0.0).child(text));
                    } else {
                        el = el.child(text);
                    }
                } else if let Some(tracks) = self.grid_plan(id, c) {
                    el = self.grid_rows(el, node, c, &tracks, &inherited);
                } else {
                    for child in &node.children {
                        if let Some(child) = self.node(*child, &inherited, false) {
                            el = el.child(child);
                        }
                    }
                }
            }
        }
        let el = el.child(self.probe(id));
        // Shared-element transition: glide from the previous screen's spot.
        if let Some((offsets, generation)) = self.morph
            && let Some((dx, dy)) = offsets.get(&id).copied()
        {
            return el
                .with_animation(
                    SharedString::from(format!("morph-{id}-{generation}")),
                    gpui_kit::Animation::new(std::time::Duration::from_millis(320))
                        .with_easing(gpui_kit::ease_in_out),
                    move |el, t| {
                        el.relative()
                            .left(px(dx * (1.0 - t)))
                            .top(px(dy * (1.0 - t)))
                    },
                )
                .into_any_element();
        }
        el.into_any_element()
    }

    fn image(&self, node: &Node, c: &Computed) -> AnyElement {
        let fit = match c.object_fit.as_deref() {
            Some("contain") => ObjectFit::Contain,
            Some("cover") => ObjectFit::Cover,
            Some("none") => ObjectFit::None,
            Some("scale-down") => ObjectFit::ScaleDown,
            _ => ObjectFit::Fill,
        };
        let src = node.attr("src").unwrap_or("");
        let placeholder = || {
            div()
                .size_full()
                .min_w(px(24.0))
                .min_h(px(24.0))
                .bg(gpui_kit::rgb(0xe8e6e1))
                .flex()
                .items_center()
                .justify_center()
                .text_color(gpui_kit::rgb(0x8a8780))
                .child("Image")
                .into_any_element()
        };
        if src.is_empty() || src.starts_with("data:") {
            return placeholder();
        }
        let image = if src.starts_with("http://") || src.starts_with("https://") {
            img(SharedString::from(src.to_owned()))
        } else {
            let path = PathBuf::from(src);
            let path = if path.is_absolute() {
                path
            } else {
                self.asset_dir.join(path)
            };
            img(path)
        };
        let mut image = image.object_fit(fit).with_fallback(placeholder);
        if c.width != Length::Auto {
            image = image.w_full();
        }
        if c.height != Length::Auto {
            image = image.h_full();
        }
        // The box's radii clip its image, as in a browser.
        let [tl, tr, br, bl] = c.resolved_radii(c.width.px(), c.height.px());
        image = image
            .rounded_tl(self.px(tl))
            .rounded_tr(self.px(tr))
            .rounded_br(self.px(br))
            .rounded_bl(self.px(bl));
        image.into_any_element()
    }

    fn svg(&self, id: NodeId, node: &Node, source: &str) -> AnyElement {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let path = self
            .cache_dir
            .join(format!("svg-{:016x}.svg", hasher.finish()));
        if !path.exists() {
            let _ = std::fs::create_dir_all(self.cache_dir);
            let _ = std::fs::write(&path, source);
        }
        let c = self.doc.computed(node);
        let attr_px = |name: &str| {
            let tag_end = source.find('>').unwrap_or(source.len());
            let head = &source[..tag_end];
            let key = format!(" {name}=\"");
            head.find(&key).and_then(|start| {
                let rest = &head[start + key.len()..];
                rest.split('"')
                    .next()
                    .and_then(Length::parse)
                    .and_then(Length::px)
            })
        };
        let width = c.width.px().or_else(|| attr_px("width")).unwrap_or(24.0);
        let height = c.height.px().or_else(|| attr_px("height")).unwrap_or(width);
        let mut el = div()
            .id(element_id(id))
            .role(gpui_kit::Role::Image)
            .aria_label(self.doc.display_name(id))
            .aria_description(format!("<svg> layer {id}"))
            .relative()
            .w(self.px(width))
            .h(self.px(height))
            .flex_shrink_0();
        if c.position == Position::Absolute {
            el = el.absolute();
            if let Some(left) = self.len(c.inset[3]) {
                el.style().inset.left = Some(left);
            }
            if let Some(top) = self.len(c.inset[0]) {
                el.style().inset.top = Some(top);
            }
        }
        el.child(img(path).size_full())
            .child(self.probe(id))
            .into_any_element()
    }

    /// Concrete column tracks when a grid needs emulation (non-uniform tracks).
    fn grid_plan(&self, id: NodeId, c: &Computed) -> Option<Vec<Track>> {
        if c.display != Display::Grid {
            return None;
        }
        let list = TrackList::parse(c.grid_template_columns.as_deref()?)?;
        if list.uniform_fr().is_some() {
            return None;
        }
        let horizontal_padding = c.padding[1] + c.padding[3];
        let container = c.width.px().map(|w| w - horizontal_padding).or_else(|| {
            self.measured
                .get(&id)
                .map(|b| b.size.width.as_f32() / self.zoom - horizontal_padding)
        });
        if container.is_none() && list.auto.is_some() {
            self.unsettled.set(true);
        }
        let items = self
            .doc
            .children(id)
            .iter()
            .filter(|child| self.is_grid_item(**child))
            .count();
        Some(list.resolve(container, c.column_gap, items))
    }

    fn is_grid_item(&self, id: NodeId) -> bool {
        self.doc.get(id).is_some_and(|n| {
            !n.hidden
                && !matches!(&n.kind, NodeKind::Text(t) if t.trim().is_empty())
                && self.doc.computed(n).position != Position::Absolute
                && self.doc.computed(n).display != Display::None
        })
    }

    fn grid_rows(
        &self,
        mut el: Stateful<Div>,
        node: &Node,
        c: &Computed,
        tracks: &[Track],
        inherited: &Inherited,
    ) -> Stateful<Div> {
        let container = c.width.px().map(|w| w - c.padding[1] - c.padding[3]);
        let row_tracks = c.grid_template_rows.as_deref().and_then(TrackList::parse);
        let items: Vec<NodeId> = node
            .children
            .iter()
            .copied()
            .filter(|id| self.is_grid_item(*id))
            .collect();
        let placements: Vec<ColumnPlacement> = items
            .iter()
            .map(|id| {
                let computed = self
                    .doc
                    .get(*id)
                    .map(|n| self.doc.computed(n))
                    .unwrap_or_default();
                ColumnPlacement::parse(computed.grid_column.as_deref())
            })
            .collect();
        let columns = tracks.len().max(1);
        let cell = |tracks: &[Track]| -> Div {
            let (mut fixed, mut fr, mut hug) = (0.0_f32, 0.0_f32, false);
            for track in tracks {
                let (px_part, fr_part, hugs) = track.sizing(container);
                fixed += px_part;
                fr += fr_part;
                hug |= hugs;
            }
            fixed += c.column_gap * tracks.len().saturating_sub(1) as f32;
            let mut wrapper = div().flex().flex_col();
            if fr > 0.0 {
                wrapper.style().flex_grow = Some(fr);
                wrapper.style().flex_shrink = Some(1.0);
                wrapper.style().flex_basis = Some(self.px(fixed).into());
                wrapper = wrapper.min_w(px(0.0));
            } else if hug {
                wrapper = wrapper.flex_none();
            } else {
                wrapper = wrapper.w(self.px(fixed)).flex_shrink_0();
            }
            wrapper
        };
        for (row_index, row) in place(&placements, columns).into_iter().enumerate() {
            let mut line = div().w_full().flex().flex_row().gap(self.px(c.column_gap));
            // Grid alignment applies to items within a row (default stretch).
            line.style().align_items = align_items(c.align_items).or(Some(AlignItems::Stretch));
            if let Some(Track::Px(height)) = row_tracks
                .as_ref()
                .map(|list| list.resolve(None, c.row_gap, items.len()))
                .and_then(|rows| rows.get(row_index).cloned())
            {
                line = line.h(self.px(height));
            }
            let mut column = 0;
            for (item, start, span) in row {
                // Empty tracks before an explicitly placed item keep their size.
                for gap_column in column..start {
                    line = line.child(cell(&tracks[gap_column..=gap_column]));
                }
                let mut wrapper = cell(&tracks[start..(start + span).min(columns)]);
                if let Some(child) = self.node(items[item], inherited, false) {
                    wrapper = wrapper.child(child);
                }
                line = line.child(wrapper);
                column = start + span;
            }
            for rest in column..columns {
                line = line.child(cell(&tracks[rest..=rest]));
            }
            el = el.child(line);
        }
        // Absolutely positioned children keep their own placement.
        for child in &node.children {
            if self
                .doc
                .get(*child)
                .is_some_and(|n| self.doc.computed(n).position == Position::Absolute)
                && let Some(child) = self.node(*child, inherited, false)
            {
                el = el.child(child);
            }
        }
        el
    }

    /// Flatten an inline subtree into text plus non-overlapping highlight runs.
    fn inline_runs(
        &self,
        id: NodeId,
        inherited: &Inherited,
    ) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
        let mut text = String::new();
        let mut runs = Vec::new();
        if let Some(node) = self.doc.get(id) {
            for child in &node.children {
                self.collect_inline(
                    *child,
                    HighlightStyle::default(),
                    inherited,
                    &mut text,
                    &mut runs,
                );
            }
        }
        // Collapse whitespace at the block edges like a browser would.
        let trimmed_start = text.len() - text.trim_start().len();
        if trimmed_start > 0 || text.ends_with(' ') {
            let end = text.trim_end().len().max(trimmed_start);
            let shifted: Vec<_> = runs
                .into_iter()
                .filter_map(|(range, style): (Range<usize>, HighlightStyle)| {
                    let start = range.start.clamp(trimmed_start, end) - trimmed_start;
                    let stop = range.end.clamp(trimmed_start, end) - trimmed_start;
                    (stop > start).then_some((start..stop, style))
                })
                .collect();
            return (text[trimmed_start..end].to_owned(), shifted);
        }
        (text, runs)
    }

    fn collect_inline(
        &self,
        id: NodeId,
        style: HighlightStyle,
        inherited: &Inherited,
        text: &mut String,
        runs: &mut Vec<(Range<usize>, HighlightStyle)>,
    ) {
        let Some(node) = self.doc.get(id) else {
            return;
        };
        if node.hidden {
            return;
        }
        match &node.kind {
            NodeKind::Text(value) => {
                let value = transform_text(value, inherited.transform.as_deref());
                let start = text.len();
                text.push_str(&value);
                if style != HighlightStyle::default() && !value.is_empty() {
                    runs.push((start..text.len(), style));
                }
            }
            NodeKind::Svg(_) => {}
            NodeKind::Element { tag } => {
                if tag == "br" {
                    text.push('\n');
                    return;
                }
                let c = self.doc.computed(node);
                let mut style = style;
                if let Some(color) = c.color {
                    style.color = Some(hsla(color));
                } else if tag == "a" {
                    style.color = Some(hsla(Color::rgb(0x25, 0x63, 0xeb)));
                }
                if let Some(weight) = c.font_weight {
                    style.font_weight = Some(FontWeight(f32::from(weight)));
                } else if ua_bold(tag) {
                    style.font_weight = Some(FontWeight::BOLD);
                }
                if c.italic == Some(true)
                    || (c.italic.is_none() && matches!(tag.as_str(), "em" | "i" | "cite"))
                {
                    style.font_style = Some(FontStyle::Italic);
                }
                if c.underline == Some(true)
                    || (c.underline.is_none() && matches!(tag.as_str(), "u" | "a" | "ins"))
                {
                    style.underline = Some(UnderlineStyle {
                        thickness: px(1.0),
                        ..UnderlineStyle::default()
                    });
                }
                if c.strikethrough == Some(true)
                    || (c.strikethrough.is_none() && matches!(tag.as_str(), "s" | "del"))
                {
                    style.strikethrough = Some(StrikethroughStyle {
                        thickness: px(1.0),
                        ..StrikethroughStyle::default()
                    });
                }
                if let Some(background) = c.background.filter(|b| b.a > 0) {
                    style.background_color = Some(hsla(background));
                } else if tag == "mark" {
                    style.background_color = Some(hsla(Color::rgb(0xff, 0xf0, 0x85)));
                }
                let inherited = Inherited {
                    transform: c
                        .text_transform
                        .clone()
                        .or_else(|| inherited.transform.clone()),
                    in_flex_row: false,
                };
                for child in &node.children {
                    self.collect_inline(*child, style, &inherited, text, runs);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_font_stacks() {
        let book = FontBook::new(vec!["Inter".into(), "DejaVu Serif".into()]);
        assert_eq!(
            book.resolve("\"Inter\", sans-serif"),
            SharedString::from("Inter")
        );
        assert_eq!(
            book.resolve("Helvetica, sans-serif"),
            SharedString::from("Geist")
        );
        assert_eq!(book.resolve("serif"), SharedString::from("DejaVu Serif"));
        assert_eq!(
            book.resolve("ui-monospace"),
            SharedString::from("Geist Mono")
        );
    }

    #[test]
    fn text_transform() {
        assert_eq!(
            transform_text("hello world", Some("capitalize")),
            "Hello World"
        );
        assert_eq!(transform_text("Hi", Some("uppercase")), "HI");
    }
}
