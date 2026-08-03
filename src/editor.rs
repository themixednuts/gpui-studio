use serde::{Deserialize, Serialize};

/// Structured inspector surface shown for the active selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectorTab {
    /// Geometry, constraints, responsive sizing, and transforms.
    #[default]
    Layout,
    /// Paint and typography.
    Style,
    /// Props, state, events, actions, and source projections.
    Logic,
}

/// Active bottom-dock surface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DockTab {
    /// Local runtime and MCP event stream.
    #[default]
    Console,
    /// Component props, state, and interaction variants.
    States,
}

/// Horizontal behavior retained when a component's parent viewport changes size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum HorizontalConstraint {
    /// Preserve the distance from the parent's left edge.
    #[default]
    Left,
    /// Preserve the distance between the element center and parent center.
    Center,
    /// Preserve the distance from the parent's right edge.
    Right,
    /// Scale the element position and width with its parent.
    Scale,
}

impl HorizontalConstraint {
    /// Human-readable constraint name used by the structured inspector.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
            Self::Scale => "Scale",
        }
    }

    /// Resolve an element's horizontal frame after its parent changes width.
    ///
    /// Returns `(x, width)`. Invalid parent sizes leave the frame unchanged so a malformed or
    /// not-yet-painted document cannot introduce infinities into the editor.
    pub fn resolve(
        self,
        old_parent_width: f32,
        new_parent_width: f32,
        x: f32,
        width: f32,
    ) -> (f32, f32) {
        if !old_parent_width.is_finite()
            || !new_parent_width.is_finite()
            || !x.is_finite()
            || !width.is_finite()
            || old_parent_width <= 0.0
            || new_parent_width <= 0.0
        {
            return (x, width);
        }
        match self {
            Self::Left => (x, width),
            Self::Center => {
                let center_offset = x + width / 2.0 - old_parent_width / 2.0;
                (new_parent_width / 2.0 + center_offset - width / 2.0, width)
            }
            Self::Right => {
                let right_margin = old_parent_width - x - width;
                (new_parent_width - right_margin - width, width)
            }
            Self::Scale => {
                let scale = new_parent_width / old_parent_width;
                (x * scale, width * scale)
            }
        }
    }
}

/// Logical viewport being edited on the canvas.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ViewportPreset {
    /// Fill the currently available canvas while retaining safe minimums.
    #[default]
    Responsive,
    /// Desktop application viewport.
    Desktop,
    /// Tablet portrait viewport.
    Tablet,
    /// Phone portrait viewport.
    Mobile,
}

impl ViewportPreset {
    /// Human-readable preset name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Responsive => "Responsive",
            Self::Desktop => "Desktop",
            Self::Tablet => "Tablet",
            Self::Mobile => "Mobile",
        }
    }

    const fn base_frame(self) -> Option<(f32, f32)> {
        match self {
            Self::Responsive => None,
            Self::Desktop => Some((840.0, 520.0)),
            Self::Tablet => Some((620.0, 600.0)),
            Self::Mobile => Some((390.0, 600.0)),
        }
    }
}

/// Native operating-system decoration policy emitted with the application window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum OutputDecorations {
    /// Let Windows, macOS, or the Linux window manager draw native decorations.
    #[default]
    Native,
    /// Use client-side decorations so the document can supply its own semantic titlebar.
    #[serde(alias = "Browser", alias = "None")]
    Custom,
}

impl OutputDecorations {
    /// Human-readable output decoration policy.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Native => "Native decorations on",
            Self::Custom => "Native decorations off",
        }
    }

    /// GPUI `TitlebarOptions::appears_transparent` for Windows and macOS output.
    pub const fn titlebar_appears_transparent(self) -> bool {
        matches!(self, Self::Custom)
    }

    /// GPUI `WindowDecorations` policy used by Wayland output.
    pub const fn linux_policy(self) -> &'static str {
        match self {
            Self::Native => "server",
            Self::Custom => "client",
        }
    }
}

/// Persistable viewport and canvas behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasSettings {
    /// Logical viewport preset.
    pub preset: ViewportPreset,
    /// Output-window decoration policy. `chrome` remains an input alias for version-3 settings.
    #[serde(alias = "chrome")]
    pub decorations: OutputDecorations,
    /// Bounded visual zoom from 25 to 200.
    pub zoom_percent: u16,
    /// Clockwise viewport orientation in quarter turns.
    pub quarter_turns: u8,
    /// Whether coordinate edits snap to the grid.
    pub snap_enabled: bool,
    /// Grid size in logical pixels.
    pub snap_grid: u16,
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self {
            preset: ViewportPreset::Responsive,
            decorations: OutputDecorations::Native,
            zoom_percent: 100,
            quarter_turns: 0,
            snap_enabled: true,
            snap_grid: 8,
        }
    }
}

/// Resolved physical frame and logical CSS viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewLayout {
    /// Rendered frame width.
    pub frame_width: f32,
    /// Rendered frame height.
    pub frame_height: f32,
    /// Logical CSS viewport width.
    pub viewport_width: f32,
    /// Logical CSS viewport height; document-authored titlebars are part of this space.
    pub viewport_height: f32,
    /// Physical preview height reserved for the current platform's native window decorator.
    pub native_decorator_height: f32,
    /// Effective visual zoom after the preview is fitted within its canvas safe area.
    pub effective_zoom_percent: f32,
    /// Centered frame origin on the canvas x axis.
    pub center_x: f32,
    /// Centered frame origin on the canvas y axis.
    pub center_y: f32,
}

impl CanvasSettings {
    /// Resolve a centered preview frame for the available canvas size.
    pub fn layout(self, available_width: f32, available_height: f32) -> PreviewLayout {
        let safe_width = available_width.max(1.0);
        let safe_height = available_height.max(1.0);
        let (mut base_width, mut base_height) = self.preset.base_frame().unwrap_or((
            safe_width.clamp(240.0, 840.0),
            safe_height.clamp(220.0, 520.0),
        ));
        if self.quarter_turns % 2 == 1 {
            std::mem::swap(&mut base_width, &mut base_height);
        }
        let native_decorator_height = if self.decorations == OutputDecorations::Native {
            32.0
        } else {
            0.0
        };
        let outer_height = base_height + native_decorator_height;
        let requested_zoom = f32::from(self.zoom_percent.clamp(25, 200)) / 100.0;
        let fit_zoom = (safe_width / base_width)
            .min(safe_height / outer_height)
            .min(1.0);
        let zoom = requested_zoom.min(fit_zoom);
        let frame_width = base_width * zoom;
        let frame_height = outer_height * zoom;
        let viewport_width = (base_width - 2.0).max(1.0);
        let viewport_height = (base_height - 2.0).max(1.0);
        PreviewLayout {
            frame_width,
            frame_height,
            viewport_width,
            viewport_height,
            native_decorator_height: native_decorator_height * zoom,
            effective_zoom_percent: zoom * 100.0,
            center_x: (safe_width - frame_width) / 2.0,
            center_y: (safe_height - frame_height) / 2.0,
        }
    }

    /// Calculate a stable five-percent fit zoom without exceeding 200%.
    pub fn fit_zoom(self, available_width: f32, available_height: f32) -> u16 {
        let (mut base_width, mut base_height) = self.preset.base_frame().unwrap_or((
            available_width.clamp(240.0, 840.0),
            available_height.clamp(220.0, 520.0),
        ));
        if self.quarter_turns % 2 == 1 {
            std::mem::swap(&mut base_width, &mut base_height);
        }
        if self.decorations == OutputDecorations::Native {
            base_height += 32.0;
        }
        let ratio = (available_width / base_width)
            .min(available_height / base_height)
            .clamp(0.25, 2.0);
        (((ratio * 100.0) / 5.0).floor() * 5.0) as u16
    }

    /// Increase or decrease zoom while enforcing safe bounds.
    pub fn zoom_by(&mut self, delta: i16) {
        self.zoom_percent = (self.zoom_percent as i32 + i32::from(delta)).clamp(25, 200) as u16;
    }

    /// Rotate the viewport orientation clockwise by 90 degrees.
    pub fn rotate_clockwise(&mut self) {
        self.quarter_turns = self.quarter_turns.wrapping_add(1) % 4;
    }

    /// Snap one coordinate when snapping is enabled.
    pub fn snapped(self, value: f32) -> f32 {
        if !self.snap_enabled || self.snap_grid == 0 || !value.is_finite() {
            return value;
        }
        let grid = f32::from(self.snap_grid);
        (value / grid).round() * grid
    }
}

/// Calculate canvas room using the authored responsive rail defaults.
pub fn available_canvas(window_width: f32, window_height: f32, dock_collapsed: bool) -> (f32, f32) {
    let (left_rail_width, right_rail_width) = if window_width <= 900.0 {
        (190.0, 270.0)
    } else if window_width <= 1_120.0 {
        (210.0, 300.0)
    } else {
        (250.0, 320.0)
    };
    available_canvas_with_rails(
        window_width,
        window_height,
        dock_collapsed,
        left_rail_width,
        right_rail_width,
    )
}

/// Calculate canvas room after the current side-rail widths, resize handles,
/// dock, toolbar, and canvas padding are applied.
pub fn available_canvas_with_rails(
    window_width: f32,
    window_height: f32,
    dock_collapsed: bool,
    left_rail_width: f32,
    right_rail_width: f32,
) -> (f32, f32) {
    let dock = if dock_collapsed {
        36.0
    } else if window_height <= 720.0 {
        150.0
    } else if window_width <= 1_120.0 {
        170.0
    } else {
        206.0
    };
    let resize_handles = 12.0;
    let canvas_horizontal_padding = 32.0;
    (
        (window_width
            - left_rail_width
            - right_rail_width
            - resize_handles
            - canvas_horizontal_padding)
            .max(1.0),
        (window_height - 46.0 - dock - 76.0).max(1.0),
    )
}

/// Canonical element transform used by both HTML and GPUI projections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementTransform {
    /// Horizontal translation in logical pixels.
    pub x: f32,
    /// Vertical translation in logical pixels.
    pub y: f32,
    /// Clockwise rotation in degrees.
    pub rotation_degrees: f32,
}

impl ElementTransform {
    /// Snap translations and normalize rotation into `[0, 360)`.
    pub fn normalized(mut self, canvas: CanvasSettings) -> Self {
        self.x = canvas.snapped(self.x);
        self.y = canvas.snapped(self.y);
        self.rotation_degrees = self.rotation_degrees.rem_euclid(360.0);
        self
    }

    /// Axis-aligned bounds after applying rotation.
    pub fn rotated_bounds(self, width: f32, height: f32) -> (f32, f32) {
        let radians = self.rotation_degrees.to_radians();
        let sin = radians.sin().abs();
        let cos = radians.cos().abs();
        (width * cos + height * sin, width * sin + height * cos)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CanvasSettings, ElementTransform, HorizontalConstraint, OutputDecorations, ViewportPreset,
        available_canvas, available_canvas_with_rails,
    };

    #[test]
    fn responsive_layout_matches_reference_shell_at_desktop_size() {
        let available = available_canvas(1_440.0, 900.0, false);
        assert_eq!(available, (826.0, 572.0));
        let layout = CanvasSettings::default().layout(available.0, available.1);
        assert_eq!((layout.frame_width, layout.frame_height), (826.0, 552.0));
        assert_eq!(
            (layout.viewport_width, layout.viewport_height),
            (824.0, 518.0)
        );
        assert_eq!((layout.center_x, layout.center_y), (0.0, 10.0));
        assert!((layout.native_decorator_height - 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resized_side_rails_change_available_canvas_width() {
        assert_eq!(
            available_canvas_with_rails(1_440.0, 900.0, false, 250.0, 320.0),
            (826.0, 572.0)
        );
        assert_eq!(
            available_canvas_with_rails(1_440.0, 900.0, false, 300.0, 420.0),
            (676.0, 572.0)
        );
    }

    #[test]
    fn rotation_swaps_device_axes_without_reserving_editor_chrome() {
        let mut settings = CanvasSettings {
            preset: ViewportPreset::Mobile,
            decorations: OutputDecorations::Custom,
            ..CanvasSettings::default()
        };
        settings.rotate_clockwise();
        let layout = settings.layout(856.0, 572.0);
        assert_eq!((layout.frame_width, layout.frame_height), (600.0, 390.0));
        assert_eq!(
            (layout.viewport_width, layout.viewport_height),
            (598.0, 388.0)
        );
        assert!(layout.native_decorator_height.abs() < f32::EPSILON);
    }

    #[test]
    fn fit_zoom_snap_and_rotated_bounds_are_deterministic() {
        let settings = CanvasSettings {
            preset: ViewportPreset::Desktop,
            ..CanvasSettings::default()
        };
        assert_eq!(settings.fit_zoom(420.0, 260.0), 45);
        assert_eq!(settings.snapped(13.0), 16.0);
        let transform = ElementTransform {
            x: 13.0,
            y: 19.0,
            rotation_degrees: 90.0,
        }
        .normalized(settings);
        assert_eq!((transform.x, transform.y), (16.0, 16.0));
        let bounds = transform.rotated_bounds(120.0, 40.0);
        assert!((bounds.0 - 40.0).abs() < 0.001);
        assert!((bounds.1 - 120.0).abs() < 0.001);
    }

    #[test]
    fn horizontal_constraints_preserve_the_expected_anchor() {
        assert_eq!(
            HorizontalConstraint::Left.resolve(400.0, 600.0, 40.0, 120.0),
            (40.0, 120.0)
        );
        assert_eq!(
            HorizontalConstraint::Center.resolve(400.0, 600.0, 140.0, 120.0),
            (240.0, 120.0)
        );
        assert_eq!(
            HorizontalConstraint::Right.resolve(400.0, 600.0, 240.0, 120.0),
            (440.0, 120.0)
        );
        assert_eq!(
            HorizontalConstraint::Scale.resolve(400.0, 600.0, 40.0, 120.0),
            (60.0, 180.0)
        );
        assert_eq!(
            HorizontalConstraint::Scale.resolve(0.0, 600.0, 40.0, 120.0),
            (40.0, 120.0)
        );
    }
}
