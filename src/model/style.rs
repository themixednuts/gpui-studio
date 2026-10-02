//! Inline CSS declarations and the computed style the canvas renders.
//!
//! A node's style is an ordered list of declarations, exactly like an HTML
//! `style` attribute. Later declarations win, so [`Style::set`] always moves
//! the written property to the end. [`Computed`] expands shorthands in source
//! order into the typed values the renderer and design panel read.

use super::color::Color;

/// Ordered inline CSS declarations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    decls: Vec<(String, String)>,
}

impl Style {
    /// Parse `prop: value; prop: value` text. Invalid fragments are skipped.
    #[must_use]
    pub fn parse(source: &str) -> Self {
        let mut style = Self::default();
        for (property, value) in parse_declarations(source) {
            style.set(&property, &value);
        }
        style
    }

    /// Declarations in source order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.decls.iter().map(|(p, v)| (p.as_str(), v.as_str()))
    }

    /// Whether no declarations are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }

    /// The last declared value of exactly this property.
    #[must_use]
    pub fn get(&self, property: &str) -> Option<&str> {
        self.decls
            .iter()
            .rev()
            .find(|(p, _)| p == property)
            .map(|(_, v)| v.as_str())
    }

    /// Set a property so it wins over every earlier declaration.
    ///
    /// Setting a shorthand also removes its longhands, which it would override
    /// anyway, keeping the serialized style minimal. An empty value removes it.
    pub fn set(&mut self, property: &str, value: &str) {
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches(';').trim();
        if property.is_empty() {
            return;
        }
        self.remove(&property);
        for longhand in longhands_of(&property) {
            self.remove(longhand);
        }
        if !value.is_empty() {
            self.decls.push((property, value.to_owned()));
        }
    }

    /// Remove every declaration of exactly this property.
    pub fn remove(&mut self, property: &str) {
        self.decls.retain(|(p, _)| p != property);
    }

    /// Remove a property and every shorthand/longhand that sets it.
    pub fn clear_family(&mut self, property: &str) {
        self.remove(property);
        for longhand in longhands_of(property) {
            self.remove(longhand);
        }
        for (shorthand, longhands) in SHORTHANDS {
            if longhands.contains(&property) {
                // Preserve the sibling longhands the shorthand also set.
                if let Some(value) = self.get(shorthand).map(ToOwned::to_owned) {
                    self.remove(shorthand);
                    for (longhand, longhand_value) in expand(shorthand, &value) {
                        if longhand != property {
                            self.decls.push((longhand.to_owned(), longhand_value));
                        }
                    }
                }
            }
        }
    }

    /// Serialize as an inline `style` attribute value.
    #[must_use]
    pub fn to_css(&self) -> String {
        self.decls
            .iter()
            .map(|(p, v)| format!("{p}: {v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Resolve typed values, expanding shorthands in declaration order.
    #[must_use]
    pub fn computed(&self) -> Computed {
        let mut computed = Computed::default();
        for (property, value) in &self.decls {
            let expanded = expand(property, value);
            if expanded.is_empty() {
                computed.apply(property, value);
            } else {
                for (longhand, longhand_value) in expanded {
                    computed.apply(longhand, &longhand_value);
                }
            }
        }
        computed
    }
}

/// Split declaration text into lowercase property/value pairs.
#[must_use]
pub fn parse_declarations(source: &str) -> Vec<(String, String)> {
    split_top_level(source, ';')
        .into_iter()
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim().trim_end_matches("!important").trim();
            (!property.is_empty() && !value.is_empty()).then(|| (property, value.to_owned()))
        })
        .collect()
}

/// Split on a separator that is not nested inside parentheses or quotes.
#[must_use]
pub fn split_top_level(source: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for c in source.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, _) if c == separator && depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Split a value on whitespace outside parentheses.
#[must_use]
pub fn split_words(value: &str) -> Vec<String> {
    split_top_level(&value.replace(['\t', '\n'], " "), ' ')
}

const SHORTHANDS: &[(&str, &[&str])] = &[
    (
        "padding",
        &[
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ],
    ),
    (
        "margin",
        &["margin-top", "margin-right", "margin-bottom", "margin-left"],
    ),
    ("inset", &["top", "right", "bottom", "left"]),
    (
        "border-radius",
        &[
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
        ],
    ),
    ("gap", &["row-gap", "column-gap"]),
    ("flex", &["flex-grow", "flex-shrink", "flex-basis"]),
    ("flex-flow", &["flex-direction", "flex-wrap"]),
    ("border", &["border-width", "border-style", "border-color"]),
    (
        "border-width",
        &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
    ),
    ("overflow", &["overflow-x", "overflow-y"]),
    ("background", &["background-color"]),
];

fn longhands_of(property: &str) -> &'static [&'static str] {
    SHORTHANDS
        .iter()
        .find(|(shorthand, _)| *shorthand == property)
        .map_or(&[], |(_, longhands)| longhands)
}

fn four_sides(value: &str) -> Option<[String; 4]> {
    let words = split_words(value);
    let [top, right, bottom, left] = match words.as_slice() {
        [all] => [all, all, all, all],
        [vertical, horizontal] => [vertical, horizontal, vertical, horizontal],
        [top, horizontal, bottom] => [top, horizontal, bottom, horizontal],
        [top, right, bottom, left] => [top, right, bottom, left],
        _ => return None,
    };
    Some([top.clone(), right.clone(), bottom.clone(), left.clone()])
}

/// Expand one shorthand declaration into longhands; empty for non-shorthands.
#[must_use]
pub fn expand(property: &str, value: &str) -> Vec<(&'static str, String)> {
    let sides = |names: &'static [&'static str]| {
        four_sides(value).map_or_else(Vec::new, |values| {
            names.iter().copied().zip(values).collect::<Vec<_>>()
        })
    };
    match property {
        "padding" | "margin" | "inset" | "border-width" => sides(longhands_of(property)),
        "border-radius" => {
            let first = value.split('/').next().unwrap_or(value);
            four_sides(first).map_or_else(Vec::new, |values| {
                longhands_of("border-radius")
                    .iter()
                    .copied()
                    .zip(values)
                    .collect()
            })
        }
        "gap" => {
            let words = split_words(value);
            match words.as_slice() {
                [both] => vec![("row-gap", both.clone()), ("column-gap", both.clone())],
                [row, column] => vec![("row-gap", row.clone()), ("column-gap", column.clone())],
                _ => Vec::new(),
            }
        }
        "flex" => expand_flex(value),
        "flex-flow" => split_words(value)
            .into_iter()
            .map(|word| {
                if word.starts_with("row") || word.starts_with("column") {
                    ("flex-direction", word)
                } else {
                    ("flex-wrap", word)
                }
            })
            .collect(),
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            expand_border(property, value)
        }
        "overflow" => {
            let words = split_words(value);
            match words.as_slice() {
                [both] => vec![("overflow-x", both.clone()), ("overflow-y", both.clone())],
                [x, y] => vec![("overflow-x", x.clone()), ("overflow-y", y.clone())],
                _ => Vec::new(),
            }
        }
        "background" => split_words(value)
            .into_iter()
            .find(|word| Color::parse(word).is_some())
            .map(|color| vec![("background-color", color)])
            .unwrap_or_else(|| vec![("background-color", "transparent".to_owned())]),
        _ => Vec::new(),
    }
}

fn expand_flex(value: &str) -> Vec<(&'static str, String)> {
    let words = split_words(value);
    let triple = match words.as_slice() {
        [keyword] if keyword == "none" => ["0", "0", "auto"].map(ToOwned::to_owned),
        [keyword] if keyword == "auto" => ["1", "1", "auto"].map(ToOwned::to_owned),
        [keyword] if keyword == "initial" => ["0", "1", "auto"].map(ToOwned::to_owned),
        [grow] if grow.parse::<f32>().is_ok() => [grow.clone(), "1".to_owned(), "0%".to_owned()],
        [basis] => ["1".to_owned(), "1".to_owned(), basis.clone()],
        [grow, shrink] if shrink.parse::<f32>().is_ok() => {
            [grow.clone(), shrink.clone(), "0%".to_owned()]
        }
        [grow, basis] => [grow.clone(), "1".to_owned(), basis.clone()],
        [grow, shrink, basis] => [grow.clone(), shrink.clone(), basis.clone()],
        _ => return Vec::new(),
    };
    let [grow, shrink, basis] = triple;
    vec![
        ("flex-grow", grow),
        ("flex-shrink", shrink),
        ("flex-basis", basis),
    ]
}

fn expand_border(property: &str, value: &str) -> Vec<(&'static str, String)> {
    let mut width = None;
    let mut style = None;
    let mut color = None;
    for word in split_words(value) {
        if Length::parse(&word).is_some() || matches!(word.as_str(), "thin" | "medium" | "thick") {
            width = Some(word);
        } else if matches!(
            word.as_str(),
            "none" | "solid" | "dashed" | "dotted" | "double" | "hidden" | "groove" | "ridge"
        ) {
            style = Some(word);
        } else {
            color = Some(word);
        }
    }
    let width = width.unwrap_or_else(|| "medium".to_owned());
    let mut out = Vec::new();
    let width_names: &[&'static str] = match property {
        "border-top" => &["border-top-width"],
        "border-right" => &["border-right-width"],
        "border-bottom" => &["border-bottom-width"],
        "border-left" => &["border-left-width"],
        _ => longhands_of("border-width"),
    };
    for name in width_names {
        out.push((*name, width.clone()));
    }
    out.push(("border-style", style.unwrap_or_else(|| "none".to_owned())));
    if let Some(color) = color {
        out.push(("border-color", color));
    }
    out
}

/// A CSS length the renderer understands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// `auto` or unset.
    #[default]
    Auto,
    /// Absolute logical pixels (also `em`/`rem` resolved against 16px).
    Px(f32),
    /// Percent of the containing block.
    Percent(f32),
    /// `fit-content`, `max-content`, or `min-content` (hug contents).
    Fit,
}

impl Length {
    /// Parse a CSS length. Unitless zero and numbers are treated as pixels.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "auto" | "none" | "initial" | "unset" => return Some(Self::Auto),
            "fit-content" | "max-content" | "min-content" => return Some(Self::Fit),
            _ => {}
        }
        if let Some(number) = value.strip_suffix("px") {
            return number.trim().parse().ok().map(Self::Px);
        }
        if let Some(number) = value.strip_suffix('%') {
            return number.trim().parse().ok().map(Self::Percent);
        }
        if let Some(number) = value
            .strip_suffix("rem")
            .or_else(|| value.strip_suffix("em"))
        {
            return number
                .trim()
                .parse::<f32>()
                .ok()
                .map(|n| Self::Px(n * 16.0));
        }
        if let Some(number) = value.strip_suffix("pt") {
            return number
                .trim()
                .parse::<f32>()
                .ok()
                .map(|n| Self::Px(n * 4.0 / 3.0));
        }
        if let Some(number) = value
            .strip_suffix("vw")
            .or_else(|| value.strip_suffix("vh"))
        {
            return number.trim().parse().ok().map(Self::Percent);
        }
        value.parse().ok().map(Self::Px)
    }

    /// Pixel value, when absolute.
    #[must_use]
    pub fn px(self) -> Option<f32> {
        match self {
            Self::Px(value) => Some(value),
            _ => None,
        }
    }

    /// CSS text.
    #[must_use]
    pub fn to_css(self) -> String {
        match self {
            Self::Auto => "auto".to_owned(),
            Self::Px(value) => format!("{}px", fmt_num(value)),
            Self::Percent(value) => format!("{}%", fmt_num(value)),
            Self::Fit => "fit-content".to_owned(),
        }
    }
}

/// Format a number without trailing zeros.
#[must_use]
pub fn fmt_num(value: f32) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        let text = format!("{rounded:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// `display`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    /// Block flow; children stack vertically.
    #[default]
    Block,
    /// Inline content.
    Inline,
    /// Flexbox.
    Flex,
    /// CSS grid.
    Grid,
    /// Not rendered.
    None,
}

/// `flex-direction`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Horizontal.
    #[default]
    Row,
    /// Vertical.
    Column,
    /// Horizontal, reversed.
    RowReverse,
    /// Vertical, reversed.
    ColumnReverse,
}

impl Direction {
    /// Whether the main axis is vertical.
    #[must_use]
    pub fn is_column(self) -> bool {
        matches!(self, Self::Column | Self::ColumnReverse)
    }
}

/// Main- and cross-axis alignment keywords.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Unset (`normal`).
    #[default]
    Normal,
    /// Start edge.
    Start,
    /// Centered.
    Center,
    /// End edge.
    End,
    /// Stretch on the cross axis.
    Stretch,
    /// Baseline.
    Baseline,
    /// Space between items.
    SpaceBetween,
    /// Space around items.
    SpaceAround,
    /// Space evenly.
    SpaceEvenly,
}

impl Align {
    fn parse(value: &str) -> Self {
        match value.trim() {
            "flex-start" | "start" | "left" | "self-start" => Self::Start,
            "center" => Self::Center,
            "flex-end" | "end" | "right" | "self-end" => Self::End,
            "stretch" => Self::Stretch,
            "baseline" => Self::Baseline,
            "space-between" => Self::SpaceBetween,
            "space-around" => Self::SpaceAround,
            "space-evenly" => Self::SpaceEvenly,
            _ => Self::Normal,
        }
    }

    /// Canonical CSS keyword.
    #[must_use]
    pub fn to_css(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Start => "flex-start",
            Self::Center => "center",
            Self::End => "flex-end",
            Self::Stretch => "stretch",
            Self::Baseline => "baseline",
            Self::SpaceBetween => "space-between",
            Self::SpaceAround => "space-around",
            Self::SpaceEvenly => "space-evenly",
        }
    }
}

/// `position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    /// In normal flow.
    #[default]
    Static,
    /// In flow, offset-capable.
    Relative,
    /// Out of flow, placed by insets relative to the parent.
    Absolute,
}

/// `overflow`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    /// Content may paint outside.
    #[default]
    Visible,
    /// Content is clipped.
    Hidden,
    /// Content is clipped and scrollable.
    Scroll,
}

/// `text-align`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// Inherit/unset.
    #[default]
    Inherit,
    /// Left.
    Left,
    /// Center.
    Center,
    /// Right.
    Right,
}

/// `line-height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    /// Absolute pixels.
    Px(f32),
    /// Multiple of the font size.
    Relative(f32),
}

/// One `box-shadow` layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Horizontal offset.
    pub x: f32,
    /// Vertical offset.
    pub y: f32,
    /// Blur radius.
    pub blur: f32,
    /// Spread radius.
    pub spread: f32,
    /// Shadow color.
    pub color: Color,
    /// Inset shadows are parsed but not painted.
    pub inset: bool,
}

impl Shadow {
    fn parse_list(value: &str) -> Vec<Self> {
        if value.trim() == "none" {
            return Vec::new();
        }
        split_top_level(value, ',')
            .iter()
            .filter_map(|layer| Self::parse(layer))
            .collect()
    }

    fn parse(value: &str) -> Option<Self> {
        let mut lengths = Vec::new();
        let mut color = Color::rgba(0, 0, 0, 64);
        let mut inset = false;
        for word in split_words(value) {
            if word == "inset" {
                inset = true;
            } else if let Some(length) = Length::parse(&word).and_then(Length::px) {
                lengths.push(length);
            } else if let Some(parsed) = Color::parse(&word) {
                color = parsed;
            }
        }
        if lengths.len() < 2 {
            return None;
        }
        Some(Self {
            x: lengths[0],
            y: lengths[1],
            blur: lengths.get(2).copied().unwrap_or(0.0),
            spread: lengths.get(3).copied().unwrap_or(0.0),
            color,
            inset,
        })
    }

    /// CSS text for one layer.
    #[must_use]
    pub fn to_css(&self) -> String {
        format!(
            "{}{}px {}px {}px {}px {}",
            if self.inset { "inset " } else { "" },
            fmt_num(self.x),
            fmt_num(self.y),
            fmt_num(self.blur),
            fmt_num(self.spread),
            self.color.to_css()
        )
    }
}

/// Typed resolved style of one node (not including inherited values).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct Computed {
    pub display: Display,
    pub direction: Direction,
    pub wrap: bool,
    pub justify: Align,
    pub align_items: Align,
    pub align_self: Align,
    pub row_gap: f32,
    pub column_gap: f32,
    /// Top, right, bottom, left.
    pub padding: [f32; 4],
    /// Top, right, bottom, left.
    pub margin: [Length; 4],
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub grow: f32,
    pub shrink: f32,
    pub basis: Length,
    pub position: Position,
    /// Top, right, bottom, left.
    pub inset: [Length; 4],
    pub background: Option<Color>,
    pub color: Option<Color>,
    /// Top, right, bottom, left.
    pub border_width: [f32; 4],
    pub border_color: Option<Color>,
    pub border_style: Option<String>,
    /// Top-left, top-right, bottom-right, bottom-left.
    pub radius: [f32; 4],
    /// Percentage radii (of the box size) for the same corners.
    pub radius_percent: [Option<f32>; 4],
    pub opacity: f32,
    pub overflow: Overflow,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    pub italic: Option<bool>,
    pub line_height: Option<LineHeight>,
    pub letter_spacing: Option<f32>,
    pub text_align: TextAlign,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub text_transform: Option<String>,
    pub nowrap: Option<bool>,
    pub shadows: Vec<Shadow>,
    pub grid_columns: Option<u16>,
    /// Raw `grid-template-columns` / `grid-template-rows` / `grid-column`.
    pub grid_template_columns: Option<String>,
    pub grid_template_rows: Option<String>,
    pub grid_column: Option<String>,
    pub grid_rows: Option<u16>,
    pub column_span: Option<u16>,
    pub row_span: Option<u16>,
    pub z_index: Option<i32>,
    pub object_fit: Option<String>,
}

impl Default for Computed {
    fn default() -> Self {
        Self {
            display: Display::Block,
            direction: Direction::Row,
            wrap: false,
            justify: Align::Normal,
            align_items: Align::Normal,
            align_self: Align::Normal,
            row_gap: 0.0,
            column_gap: 0.0,
            padding: [0.0; 4],
            margin: [Length::Px(0.0); 4],
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            grow: 0.0,
            shrink: 1.0,
            basis: Length::Auto,
            position: Position::Static,
            inset: [Length::Auto; 4],
            background: None,
            color: None,
            border_width: [0.0; 4],
            border_color: None,
            border_style: None,
            radius: [0.0; 4],
            radius_percent: [None; 4],
            opacity: 1.0,
            overflow: Overflow::Visible,
            font_family: None,
            font_size: None,
            font_weight: None,
            italic: None,
            line_height: None,
            letter_spacing: None,
            text_align: TextAlign::Inherit,
            underline: None,
            strikethrough: None,
            text_transform: None,
            nowrap: None,
            shadows: Vec::new(),
            grid_columns: None,
            grid_template_columns: None,
            grid_template_rows: None,
            grid_column: None,
            grid_rows: None,
            column_span: None,
            row_span: None,
            z_index: None,
            object_fit: None,
        }
    }
}

fn px_or_zero(value: &str) -> f32 {
    Length::parse(value).and_then(Length::px).unwrap_or(0.0)
}

fn border_width_px(value: &str) -> f32 {
    match value.trim() {
        "thin" => 1.0,
        "medium" => 3.0,
        "thick" => 5.0,
        other => px_or_zero(other),
    }
}

fn track_count(value: &str) -> Option<u16> {
    let value = value.trim();
    if value == "none" {
        return None;
    }
    if let Some(inner) = value.strip_prefix("repeat(") {
        let count = inner.split(',').next()?.trim();
        return count.parse().ok();
    }
    u16::try_from(split_words(value).len())
        .ok()
        .filter(|n| *n > 0)
}

fn span(value: &str) -> Option<u16> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("span") {
        return rest.trim().parse().ok();
    }
    if let Some((start, end)) = value.split_once('/') {
        let end = end.trim();
        if let Some(rest) = end.strip_prefix("span") {
            return rest.trim().parse().ok();
        }
        let start: i32 = start.trim().parse().ok()?;
        let end: i32 = end.parse().ok()?;
        return u16::try_from(end - start).ok().filter(|n| *n > 0);
    }
    None
}

impl Computed {
    fn apply(&mut self, property: &str, value: &str) {
        let value = value.trim();
        let length = || Length::parse(value).unwrap_or(Length::Auto);
        match property {
            "display" => {
                self.display = match value {
                    "flex" | "inline-flex" => Display::Flex,
                    "grid" | "inline-grid" => Display::Grid,
                    "none" => Display::None,
                    "inline" | "inline-block" => Display::Inline,
                    _ => Display::Block,
                }
            }
            "flex-direction" => {
                self.direction = match value {
                    "column" => Direction::Column,
                    "row-reverse" => Direction::RowReverse,
                    "column-reverse" => Direction::ColumnReverse,
                    _ => Direction::Row,
                }
            }
            "flex-wrap" => self.wrap = value.starts_with("wrap"),
            "justify-content" => self.justify = Align::parse(value),
            "align-items" => self.align_items = Align::parse(value),
            "align-self" => self.align_self = Align::parse(value),
            "row-gap" => self.row_gap = px_or_zero(value),
            "column-gap" => self.column_gap = px_or_zero(value),
            "padding-top" => self.padding[0] = px_or_zero(value),
            "padding-right" => self.padding[1] = px_or_zero(value),
            "padding-bottom" => self.padding[2] = px_or_zero(value),
            "padding-left" => self.padding[3] = px_or_zero(value),
            "margin-top" => self.margin[0] = length(),
            "margin-right" => self.margin[1] = length(),
            "margin-bottom" => self.margin[2] = length(),
            "margin-left" => self.margin[3] = length(),
            "width" => self.width = length(),
            "height" => self.height = length(),
            "min-width" => self.min_width = length(),
            "min-height" => self.min_height = length(),
            "max-width" => self.max_width = length(),
            "max-height" => self.max_height = length(),
            "flex-grow" => self.grow = value.parse().unwrap_or(0.0),
            "flex-shrink" => self.shrink = value.parse().unwrap_or(1.0),
            "flex-basis" => self.basis = length(),
            "position" => {
                self.position = match value {
                    "absolute" | "fixed" => Position::Absolute,
                    "relative" | "sticky" => Position::Relative,
                    _ => Position::Static,
                }
            }
            "top" => self.inset[0] = length(),
            "right" => self.inset[1] = length(),
            "bottom" => self.inset[2] = length(),
            "left" => self.inset[3] = length(),
            "background-color" => self.background = Color::parse(value),
            "color" => self.color = Color::parse(value),
            "border-top-width" => self.border_width[0] = border_width_px(value),
            "border-right-width" => self.border_width[1] = border_width_px(value),
            "border-bottom-width" => self.border_width[2] = border_width_px(value),
            "border-left-width" => self.border_width[3] = border_width_px(value),
            "border-color" => self.border_color = Color::parse(value),
            "border-style" => self.border_style = Some(value.to_owned()),
            "border-top-left-radius" => {
                self.radius_percent[0] = match Length::parse(value) {
                    Some(Length::Percent(percent)) => Some(percent),
                    _ => None,
                };
                self.radius[0] = px_or_zero(value);
            }
            "border-top-right-radius" => {
                self.radius_percent[1] = match Length::parse(value) {
                    Some(Length::Percent(percent)) => Some(percent),
                    _ => None,
                };
                self.radius[1] = px_or_zero(value);
            }
            "border-bottom-right-radius" => {
                self.radius_percent[2] = match Length::parse(value) {
                    Some(Length::Percent(percent)) => Some(percent),
                    _ => None,
                };
                self.radius[2] = px_or_zero(value);
            }
            "border-bottom-left-radius" => {
                self.radius_percent[3] = match Length::parse(value) {
                    Some(Length::Percent(percent)) => Some(percent),
                    _ => None,
                };
                self.radius[3] = px_or_zero(value);
            }
            "opacity" => {
                self.opacity = if let Some(percent) = value.strip_suffix('%') {
                    percent.parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    value.parse().unwrap_or(1.0)
                }
            }
            "overflow-x" | "overflow-y" => {
                let overflow = match value {
                    "hidden" | "clip" => Overflow::Hidden,
                    "scroll" | "auto" => Overflow::Scroll,
                    _ => Overflow::Visible,
                };
                if overflow != Overflow::Visible || property == "overflow-x" {
                    self.overflow = overflow;
                }
            }
            "font-family" => self.font_family = Some(value.to_owned()),
            "font-size" => self.font_size = Length::parse(value).and_then(Length::px),
            "font-weight" => {
                self.font_weight = match value {
                    "normal" => Some(400),
                    "bold" => Some(700),
                    "lighter" => Some(300),
                    "bolder" => Some(800),
                    other => other.parse().ok(),
                }
            }
            "font-style" => self.italic = Some(value == "italic" || value == "oblique"),
            "line-height" => {
                self.line_height = if value == "normal" {
                    None
                } else if let Ok(multiple) = value.parse::<f32>() {
                    Some(LineHeight::Relative(multiple))
                } else {
                    match Length::parse(value) {
                        Some(Length::Px(px)) => Some(LineHeight::Px(px)),
                        Some(Length::Percent(percent)) => {
                            Some(LineHeight::Relative(percent / 100.0))
                        }
                        _ => None,
                    }
                }
            }
            "letter-spacing" => self.letter_spacing = Length::parse(value).and_then(Length::px),
            "text-align" => {
                self.text_align = match value {
                    "center" => TextAlign::Center,
                    "right" | "end" => TextAlign::Right,
                    "left" | "start" | "justify" => TextAlign::Left,
                    _ => TextAlign::Inherit,
                }
            }
            "text-decoration" | "text-decoration-line" => {
                self.underline = Some(value.contains("underline"));
                self.strikethrough = Some(value.contains("line-through"));
            }
            "text-transform" => self.text_transform = Some(value.to_owned()),
            "white-space" => self.nowrap = Some(value == "nowrap" || value == "pre"),
            "box-shadow" => self.shadows = Shadow::parse_list(value),
            "grid-template-columns" => {
                self.grid_columns = track_count(value);
                self.grid_template_columns = Some(value.to_owned());
            }
            "grid-template-rows" => {
                self.grid_rows = track_count(value);
                self.grid_template_rows = Some(value.to_owned());
            }
            "grid-column" => {
                self.column_span = span(value);
                self.grid_column = Some(value.to_owned());
            }
            "grid-row" => self.row_span = span(value),
            "z-index" => self.z_index = value.parse().ok(),
            "object-fit" => self.object_fit = Some(value.to_owned()),
            _ => {}
        }
    }

    /// Corner radii in px for a box of the given size (percentages resolve
    /// against the smaller side; unknown sizes treat 50% as fully round).
    #[must_use]
    pub fn resolved_radii(&self, width: Option<f32>, height: Option<f32>) -> [f32; 4] {
        let mut out = self.radius;
        for (slot, percent) in out.iter_mut().zip(self.radius_percent) {
            if let Some(percent) = percent {
                *slot = match (width, height) {
                    (Some(w), Some(h)) => w.min(h) * percent / 100.0,
                    _ if percent >= 50.0 => 10_000.0,
                    _ => 0.0,
                };
            }
        }
        out
    }

    /// Uniform padding, when all sides match.
    #[must_use]
    pub fn uniform_padding(&self) -> Option<f32> {
        let [top, right, bottom, left] = self.padding;
        (top == right && right == bottom && bottom == left).then_some(top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_declarations_win_and_shorthands_expand() {
        let style = Style::parse(
            "padding: 8px 16px; padding-left: 4px; display: flex; flex-direction: column; gap: 12px",
        );
        let computed = style.computed();
        assert_eq!(computed.padding, [8.0, 16.0, 8.0, 4.0]);
        assert_eq!(computed.display, Display::Flex);
        assert!(computed.direction.is_column());
        assert_eq!((computed.row_gap, computed.column_gap), (12.0, 12.0));
    }

    #[test]
    fn set_moves_property_last_and_shorthand_clears_longhands() {
        let mut style = Style::parse("padding-top: 4px; color: red");
        style.set("padding", "10px");
        assert_eq!(style.to_css(), "color: red; padding: 10px");
        style.set("padding-top", "2px");
        assert_eq!(style.computed().padding, [2.0, 10.0, 10.0, 10.0]);
        style.set("color", "");
        assert_eq!(style.get("color"), None);
    }

    #[test]
    fn clear_family_preserves_sibling_sides() {
        let mut style = Style::parse("padding: 1px 2px 3px 4px");
        style.clear_family("padding-left");
        assert_eq!(style.computed().padding, [1.0, 2.0, 3.0, 0.0]);
    }

    #[test]
    fn parses_border_flex_shadow_and_grid() {
        let computed = Style::parse(
            "border: 1px solid #ddd; flex: 1; box-shadow: 0 4px 12px rgba(0,0,0,0.2), inset 0 0 1px red; grid-template-columns: repeat(3, 1fr); grid-column: span 2",
        )
        .computed();
        assert_eq!(computed.border_width, [1.0; 4]);
        assert_eq!(computed.border_color, Color::parse("#ddd"));
        assert_eq!(
            (computed.grow, computed.shrink, computed.basis),
            (1.0, 1.0, Length::Percent(0.0))
        );
        assert_eq!(computed.shadows.len(), 2);
        assert_eq!(computed.shadows[0].blur, 12.0);
        assert_eq!(computed.grid_columns, Some(3));
        assert_eq!(computed.column_span, Some(2));
        let round = Style::parse("width: 40px; height: 20px; border-radius: 50%").computed();
        assert_eq!(round.resolved_radii(Some(40.0), Some(20.0)), [10.0; 4]);
        assert_eq!(round.resolved_radii(None, None), [10_000.0; 4]);
    }

    #[test]
    fn lengths_and_numbers() {
        assert_eq!(Length::parse("1.5rem"), Some(Length::Px(24.0)));
        assert_eq!(Length::parse("50%"), Some(Length::Percent(50.0)));
        assert_eq!(Length::parse("fit-content"), Some(Length::Fit));
        assert_eq!(fmt_num(12.0), "12");
        assert_eq!(fmt_num(1.25), "1.25");
        assert_eq!(Length::Px(3.5).to_css(), "3.5px");
    }
}
