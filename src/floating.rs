//! Shared floating-surface placement and pointer-intent safe corridor.

use serde::{Deserialize, Serialize};

/// Minimal rectangle used by platform-neutral popup algorithms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl SurfaceRect {
    const fn right(self) -> f32 {
        self.x + self.width
    }
    const fn bottom(self) -> f32 {
        self.y + self.height
    }

    fn distance_squared(self, point: (f32, f32)) -> f32 {
        let x = point.0.clamp(self.x, self.right());
        let y = point.1.clamp(self.y, self.bottom());
        (point.0 - x).powi(2) + (point.1 - y).powi(2)
    }
}

/// Side of an anchor used for a menu, dropdown, tooltip, or popover.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FloatingSide {
    /// Below the anchor.
    #[default]
    Bottom,
    /// Above the anchor.
    Top,
    /// To the right of the anchor.
    Right,
    /// To the left of the anchor.
    Left,
}

impl FloatingSide {
    const fn opposite(self) -> Self {
        match self {
            Self::Bottom => Self::Top,
            Self::Top => Self::Bottom,
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }

    const fn perpendicular(self) -> [Self; 2] {
        match self {
            Self::Bottom | Self::Top => [Self::Right, Self::Left],
            Self::Right | Self::Left => [Self::Bottom, Self::Top],
        }
    }
}

/// Result of flip, shift, size, and hide placement middleware.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloatingPlacement {
    /// Resolved popup frame.
    pub rect: SurfaceRect,
    /// Side selected after overflow-aware flipping.
    pub side: FloatingSide,
    /// Available content width after viewport sizing.
    pub available_width: f32,
    /// Available content height after viewport sizing.
    pub available_height: f32,
    /// Whether the anchor is fully outside the viewport.
    pub anchor_hidden: bool,
}

impl FloatingPlacement {
    /// Place a floating surface using flip → shift → size → hide behavior.
    #[must_use]
    pub fn resolve(
        anchor: SurfaceRect,
        desired_size: (f32, f32),
        viewport: SurfaceRect,
        preferred: FloatingSide,
        gap: f32,
        padding: f32,
    ) -> Self {
        let padding = padding.max(0.0);
        let available_width = (viewport.width - padding * 2.0).max(1.0);
        let available_height = (viewport.height - padding * 2.0).max(1.0);
        let width = desired_size.0.clamp(1.0, available_width);
        let height = desired_size.1.clamp(1.0, available_height);
        let perpendicular = preferred.perpendicular();
        let sides = [
            preferred,
            preferred.opposite(),
            perpendicular[0],
            perpendicular[1],
        ];
        let mut selected = (
            preferred,
            raw_rect(anchor, width, height, preferred, gap),
            f32::INFINITY,
        );
        for side in sides {
            let candidate = raw_rect(anchor, width, height, side, gap);
            let overflow = side_overflow(candidate, viewport, padding, side);
            if overflow < selected.2 {
                selected = (side, candidate, overflow);
            }
            if overflow <= f32::EPSILON {
                break;
            }
        }
        let rect = shift_inside(selected.1, viewport, padding);
        let anchor_hidden = anchor.right() < viewport.x
            || anchor.x > viewport.right()
            || anchor.bottom() < viewport.y
            || anchor.y > viewport.bottom();
        Self {
            rect,
            side: selected.0,
            available_width,
            available_height,
            anchor_hidden,
        }
    }
}

/// Pointer-intent polygon that prevents submenu traversal from closing the menu.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeCorridor {
    polygon: Vec<(f32, f32)>,
    menu: SurfaceRect,
}

/// Stateful menu-aim controller shared by dropdowns and nested context menus.
/// It caps the safe-triangle delay so a stale submenu can never remain pinned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuAim {
    previous: Option<(f32, f32)>,
    corridor: Option<SafeCorridor>,
    delay_until_ms: u64,
}

impl MenuAim {
    /// Arm pointer intent toward an opened submenu.
    pub fn arm(&mut self, pointer: (f32, f32), submenu: SurfaceRect, now_ms: u64, delay_ms: u64) {
        self.previous = Some(pointer);
        self.corridor = Some(SafeCorridor::between(pointer, submenu, 8.0));
        self.delay_until_ms = now_ms.saturating_add(delay_ms.min(500));
    }

    /// Update the sampled pointer and report whether closing should be delayed.
    #[must_use]
    pub fn should_keep_open(&mut self, current: (f32, f32), now_ms: u64) -> bool {
        let previous = self.previous.replace(current).unwrap_or(current);
        now_ms <= self.delay_until_ms
            && self
                .corridor
                .as_ref()
                .is_some_and(|corridor| corridor.should_delay_close(previous, current))
    }

    /// Clear pointer intent when a submenu closes or focus leaves the menu tree.
    pub fn clear(&mut self) {
        self.previous = None;
        self.corridor = None;
        self.delay_until_ms = 0;
    }
}

impl SafeCorridor {
    /// Build a buffered triangle from the pointer toward the popup's nearest edge.
    #[must_use]
    pub fn between(pointer: (f32, f32), menu: SurfaceRect, buffer: f32) -> Self {
        let buffer = buffer.max(0.0);
        let center = (menu.x + menu.width / 2.0, menu.y + menu.height / 2.0);
        let dx = center.0 - pointer.0;
        let dy = center.1 - pointer.1;
        let edge = if dx.abs() >= dy.abs() {
            if dx >= 0.0 {
                [
                    (menu.x - buffer, menu.y - buffer),
                    (menu.x - buffer, menu.bottom() + buffer),
                ]
            } else {
                [
                    (menu.right() + buffer, menu.y - buffer),
                    (menu.right() + buffer, menu.bottom() + buffer),
                ]
            }
        } else if dy >= 0.0 {
            [
                (menu.x - buffer, menu.y - buffer),
                (menu.right() + buffer, menu.y - buffer),
            ]
        } else {
            [
                (menu.x - buffer, menu.bottom() + buffer),
                (menu.right() + buffer, menu.bottom() + buffer),
            ]
        };
        let polygon = vec![pointer, edge[0], edge[1]];
        Self { polygon, menu }
    }

    /// Whether a pointer is inside the active safe polygon or popup.
    #[must_use]
    pub fn contains(&self, point: (f32, f32)) -> bool {
        point.0 >= self.menu.x
            && point.0 <= self.menu.right()
            && point.1 >= self.menu.y
            && point.1 <= self.menu.bottom()
            || point_in_polygon(point, &self.polygon)
    }

    /// Delay closing only while the pointer remains in the corridor and moves toward the popup.
    #[must_use]
    pub fn should_delay_close(&self, previous: (f32, f32), current: (f32, f32)) -> bool {
        self.contains(current)
            && self.menu.distance_squared(current) <= self.menu.distance_squared(previous)
    }

    /// Polygon vertices for a debug or design-canvas overlay.
    #[must_use]
    pub fn polygon(&self) -> &[(f32, f32)] {
        &self.polygon
    }
}

fn raw_rect(
    anchor: SurfaceRect,
    width: f32,
    height: f32,
    side: FloatingSide,
    gap: f32,
) -> SurfaceRect {
    match side {
        FloatingSide::Bottom => SurfaceRect {
            x: anchor.x,
            y: anchor.bottom() + gap,
            width,
            height,
        },
        FloatingSide::Top => SurfaceRect {
            x: anchor.x,
            y: anchor.y - height - gap,
            width,
            height,
        },
        FloatingSide::Right => SurfaceRect {
            x: anchor.right() + gap,
            y: anchor.y,
            width,
            height,
        },
        FloatingSide::Left => SurfaceRect {
            x: anchor.x - width - gap,
            y: anchor.y,
            width,
            height,
        },
    }
}

fn shift_inside(mut rect: SurfaceRect, viewport: SurfaceRect, padding: f32) -> SurfaceRect {
    let left = viewport.x + padding;
    let top = viewport.y + padding;
    let right = viewport.right() - padding;
    let bottom = viewport.bottom() - padding;
    rect.x = rect.x.clamp(left, (right - rect.width).max(left));
    rect.y = rect.y.clamp(top, (bottom - rect.height).max(top));
    rect
}

fn side_overflow(
    rect: SurfaceRect,
    viewport: SurfaceRect,
    padding: f32,
    side: FloatingSide,
) -> f32 {
    match side {
        FloatingSide::Bottom => (rect.bottom() - viewport.bottom() + padding).max(0.0),
        FloatingSide::Top => (viewport.y + padding - rect.y).max(0.0),
        FloatingSide::Right => (rect.right() - viewport.right() + padding).max(0.0),
        FloatingSide::Left => (viewport.x + padding - rect.x).max(0.0),
    }
}

fn point_in_polygon(point: (f32, f32), polygon: &[(f32, f32)]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let (xi, yi) = polygon[current];
        let (xj, yj) = polygon[previous];
        if (yi > point.1) != (yj > point.1) && point.0 < (xj - xi) * (point.1 - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_flips_and_shifts_inside_viewport() {
        let placement = FloatingPlacement::resolve(
            SurfaceRect {
                x: 260.0,
                y: 180.0,
                width: 30.0,
                height: 20.0,
            },
            (140.0, 100.0),
            SurfaceRect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 220.0,
            },
            FloatingSide::Bottom,
            6.0,
            8.0,
        );
        assert_eq!(placement.side, FloatingSide::Top);
        assert!(placement.rect.x >= 8.0 && placement.rect.right() <= 292.0);
        assert!(placement.rect.y >= 8.0 && placement.rect.bottom() <= 212.0);
    }

    #[test]
    fn safety_triangle_delays_only_intentional_travel() {
        let menu = SurfaceRect {
            x: 100.0,
            y: 20.0,
            width: 120.0,
            height: 100.0,
        };
        let corridor = SafeCorridor::between((20.0, 60.0), menu, 6.0);
        assert!(corridor.should_delay_close((20.0, 60.0), (60.0, 62.0)));
        assert!(!corridor.should_delay_close((60.0, 62.0), (40.0, 100.0)));
        assert!(corridor.contains((110.0, 40.0)));
    }

    #[test]
    fn menu_aim_is_buffered_but_strictly_time_bounded() {
        let menu = SurfaceRect {
            x: 100.0,
            y: 20.0,
            width: 120.0,
            height: 100.0,
        };
        let mut aim = MenuAim::default();
        aim.arm((20.0, 60.0), menu, 1_000, 300);
        assert!(aim.should_keep_open((60.0, 62.0), 1_100));
        assert!(!aim.should_keep_open((80.0, 63.0), 1_301));
        aim.clear();
        assert!(!aim.should_keep_open((100.0, 60.0), 1_150));
    }
}
