//! CSS grid track lists and auto-placement.
//!
//! GPUI only exposes equal `repeat(n, 1fr)` grids. This module parses real
//! `grid-template-columns` values (`200px 1fr auto`, `minmax(...)`,
//! `repeat(auto-fill, minmax(240px, 1fr))`, percentages) and places items in
//! rows so the renderer can lay out any common grid with flex rows.

use super::style::{Length, split_words};

/// One track size.
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    /// Fixed pixels.
    Px(f32),
    /// Percent of the container's content width.
    Percent(f32),
    /// Flexible fraction of the leftover space.
    Fr(f32),
    /// Sized to content.
    Auto,
    /// `minmax(min, max)`.
    MinMax(Box<Track>, Box<Track>),
}

impl Track {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(inner) = value
            .strip_prefix("minmax(")
            .and_then(|v| v.strip_suffix(')'))
        {
            let (min, max) = inner.split_once(',')?;
            return Some(Self::MinMax(
                Box::new(Self::parse(min)?),
                Box::new(Self::parse(max)?),
            ));
        }
        if let Some(inner) = value
            .strip_prefix("fit-content(")
            .and_then(|v| v.strip_suffix(')'))
        {
            // Treated as auto capped by the argument; auto is the closest.
            let _ = Self::parse(inner)?;
            return Some(Self::Auto);
        }
        if let Some(fr) = value.strip_suffix("fr") {
            return fr.trim().parse().ok().map(Self::Fr);
        }
        match value {
            "auto" | "min-content" | "max-content" => return Some(Self::Auto),
            _ => {}
        }
        match Length::parse(value)? {
            Length::Px(px) => Some(Self::Px(px)),
            Length::Percent(percent) => Some(Self::Percent(percent)),
            _ => Some(Self::Auto),
        }
    }

    /// Minimum width this track needs, when fixed.
    fn min_px(&self, container: Option<f32>) -> Option<f32> {
        match self {
            Self::Px(px) => Some(*px),
            Self::Percent(percent) => container.map(|c| c * percent / 100.0),
            Self::MinMax(min, _) => min.min_px(container),
            Self::Fr(_) | Self::Auto => None,
        }
    }

    /// How a cell spanning this track sizes: `(fixed px, fr share, hugs content)`.
    #[must_use]
    pub fn sizing(&self, container: Option<f32>) -> (f32, f32, bool) {
        match self {
            Self::Px(px) => (*px, 0.0, false),
            Self::Percent(percent) => (
                container.map_or(0.0, |c| c * percent / 100.0),
                0.0,
                container.is_none(),
            ),
            Self::Fr(fr) => (0.0, *fr, false),
            Self::Auto => (0.0, 0.0, true),
            Self::MinMax(_, max) => max.sizing(container),
        }
    }
}

/// `repeat(auto-fill | auto-fit, ...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoRepeat {
    /// Keep empty tracks.
    Fill,
    /// Collapse empty tracks.
    Fit,
}

/// A parsed `grid-template-columns` / `grid-template-rows` value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackList {
    /// Tracks before an automatic repetition.
    pub before: Vec<Track>,
    /// The automatic repetition, if any.
    pub auto: Option<(AutoRepeat, Vec<Track>)>,
    /// Tracks after an automatic repetition.
    pub after: Vec<Track>,
}

impl TrackList {
    /// Parse a track list; `None` for `none` or unparseable values.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value == "none" {
            return None;
        }
        let mut list = Self::default();
        for word in split_words(value) {
            // Line names like `[full-start]` carry no size.
            if word.starts_with('[') {
                continue;
            }
            if let Some(inner) = word
                .strip_prefix("repeat(")
                .and_then(|w| w.strip_suffix(')'))
            {
                let (count, tracks) = inner.split_once(',')?;
                let tracks: Vec<Track> = split_words(tracks)
                    .iter()
                    .filter(|w| !w.starts_with('['))
                    .map(|w| Track::parse(w))
                    .collect::<Option<_>>()?;
                match count.trim() {
                    "auto-fill" => list.auto = Some((AutoRepeat::Fill, tracks)),
                    "auto-fit" => list.auto = Some((AutoRepeat::Fit, tracks)),
                    n => {
                        let n: usize = n.parse().ok()?;
                        let target = if list.auto.is_some() {
                            &mut list.after
                        } else {
                            &mut list.before
                        };
                        for _ in 0..n.min(1000) {
                            target.extend(tracks.iter().cloned());
                        }
                    }
                }
            } else {
                let track = Track::parse(&word)?;
                if list.auto.is_some() {
                    list.after.push(track);
                } else {
                    list.before.push(track);
                }
            }
        }
        (!list.before.is_empty() || list.auto.is_some() || !list.after.is_empty()).then_some(list)
    }

    /// Concrete tracks for a container content width (needed for auto repeat).
    /// `items` bounds `auto-fit` so empty tracks collapse.
    #[must_use]
    pub fn resolve(&self, container: Option<f32>, gap: f32, items: usize) -> Vec<Track> {
        let mut tracks = self.before.clone();
        if let Some((mode, repeat)) = &self.auto {
            let fixed: f32 = self
                .before
                .iter()
                .chain(&self.after)
                .map(|t| t.min_px(container).unwrap_or(0.0) + gap)
                .sum();
            let unit: f32 = repeat
                .iter()
                .map(|t| t.min_px(container).unwrap_or(0.0))
                .sum::<f32>()
                + gap * repeat.len() as f32;
            let count = match container {
                Some(width) if unit > 0.0 => {
                    (((width - fixed + gap) / unit).floor() as usize).max(1)
                }
                _ => 1,
            };
            let count = match mode {
                AutoRepeat::Fill => count,
                AutoRepeat::Fit => count.min(items.div_ceil(repeat.len().max(1)).max(1)),
            };
            for _ in 0..count.min(1000) {
                tracks.extend(repeat.iter().cloned());
            }
        }
        tracks.extend(self.after.iter().cloned());
        tracks
    }

    /// The column count when every track is the same `fr` (GPUI's native grid).
    #[must_use]
    pub fn uniform_fr(&self) -> Option<u16> {
        if self.auto.is_some() {
            return None;
        }
        let tracks: Vec<&Track> = self.before.iter().chain(&self.after).collect();
        let first = tracks.first()?;
        let is_fr = |t: &Track| match t {
            Track::Fr(_) => Some(t.clone()),
            Track::MinMax(min, max)
                if matches!(**min, Track::Px(0.0)) && matches!(**max, Track::Fr(_)) =>
            {
                Some((**max).clone())
            }
            _ => None,
        };
        let unit = is_fr(first)?;
        tracks
            .iter()
            .all(|t| is_fr(t).as_ref() == Some(&unit))
            .then(|| u16::try_from(tracks.len()).ok())
            .flatten()
    }
}

/// Where an item asks to be placed horizontally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColumnPlacement {
    /// 1-based start line, if explicit.
    pub start: Option<u16>,
    /// Number of columns spanned.
    pub span: u16,
}

impl ColumnPlacement {
    /// Parse `grid-column` (`span 2`, `2 / 4`, `1 / span 3`, `3`).
    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        let mut placement = Self {
            start: None,
            span: 1,
        };
        let Some(value) = value.map(str::trim) else {
            return placement;
        };
        let (start, end) = match value.split_once('/') {
            Some((s, e)) => (s.trim(), Some(e.trim())),
            None => (value, None),
        };
        if let Some(span) = start.strip_prefix("span") {
            placement.span = span.trim().parse().unwrap_or(1);
        } else if let Ok(line) = start.parse::<u16>() {
            placement.start = Some(line.max(1));
        }
        if let Some(end) = end {
            if let Some(span) = end.strip_prefix("span") {
                placement.span = span.trim().parse().unwrap_or(1);
            } else if let (Some(start), Ok(end)) = (placement.start, end.parse::<i32>())
                && end > i32::from(start)
            {
                placement.span = u16::try_from(end - i32::from(start)).unwrap_or(1);
            }
        }
        placement.span = placement.span.max(1);
        placement
    }
}

/// One placed item: `(item index, first column, span)`.
pub type Cell = (usize, usize, usize);

/// Row-major auto-placement (the CSS `grid-auto-flow: row` algorithm without
/// row spans). Returns the cells of each row.
#[must_use]
pub fn place(items: &[ColumnPlacement], columns: usize) -> Vec<Vec<Cell>> {
    let columns = columns.max(1);
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut cursor = 0;
    for (index, item) in items.iter().enumerate() {
        let span = usize::from(item.span).min(columns);
        let wanted = item.start.map(|s| usize::from(s - 1).min(columns - 1));
        if let Some(start) = wanted {
            if start < cursor {
                rows.push(std::mem::take(&mut row));
            }
            cursor = start;
        }
        if cursor + span > columns {
            rows.push(std::mem::take(&mut row));
            cursor = wanted.unwrap_or(0).min(columns - span);
        }
        row.push((index, cursor, span));
        cursor += span;
        if cursor >= columns {
            rows.push(std::mem::take(&mut row));
            cursor = 0;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_track_lists() {
        let list = TrackList::parse("200px 1fr auto").unwrap();
        assert_eq!(
            list.before,
            vec![Track::Px(200.0), Track::Fr(1.0), Track::Auto]
        );
        assert_eq!(
            TrackList::parse("repeat(3, 1fr)").unwrap().uniform_fr(),
            Some(3)
        );
        assert_eq!(
            TrackList::parse("repeat(2, minmax(0, 1fr))")
                .unwrap()
                .uniform_fr(),
            Some(2)
        );
        assert_eq!(TrackList::parse("1fr 2fr").unwrap().uniform_fr(), None);
        assert_eq!(
            TrackList::parse("[a] 100px [b] 1fr").unwrap().before.len(),
            2
        );
        assert!(TrackList::parse("none").is_none());
        assert!(TrackList::parse("subgrid weird(").is_none());
    }

    #[test]
    fn auto_fill_uses_the_container_width() {
        let list = TrackList::parse("repeat(auto-fill, minmax(200px, 1fr))").unwrap();
        // (1000 + 16) / (200 + 16) = 4.7 → 4 columns.
        assert_eq!(list.resolve(Some(1000.0), 16.0, 10).len(), 4);
        assert_eq!(list.resolve(None, 16.0, 10).len(), 1);
        let fit = TrackList::parse("repeat(auto-fit, minmax(200px, 1fr))").unwrap();
        assert_eq!(
            fit.resolve(Some(1000.0), 16.0, 2).len(),
            2,
            "auto-fit collapses empty tracks"
        );
        assert_eq!(
            Track::MinMax(Box::new(Track::Px(200.0)), Box::new(Track::Fr(1.0))).sizing(None),
            (0.0, 1.0, false)
        );
    }

    #[test]
    fn places_items_with_spans_and_explicit_starts() {
        let items = [
            ColumnPlacement::parse(Some("span 2")),
            ColumnPlacement::parse(None),
            ColumnPlacement::parse(None),
            ColumnPlacement::parse(Some("2 / 4")),
            ColumnPlacement::parse(Some("span 9")),
        ];
        assert_eq!(
            items[3],
            ColumnPlacement {
                start: Some(2),
                span: 2
            }
        );
        let rows = place(&items, 3);
        assert_eq!(
            rows,
            vec![
                vec![(0, 0, 2), (1, 2, 1)],
                vec![(2, 0, 1), (3, 1, 2)],
                vec![(4, 0, 3)],
            ]
        );
    }
}
