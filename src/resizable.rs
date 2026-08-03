use std::cell::RefCell;
use std::collections::BTreeMap;

/// Dimension controlled by a resize handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResizeAxis {
    Width,
    Height,
}

impl ResizeAxis {
    pub(crate) const fn binding_suffix(self) -> &'static str {
        match self {
            Self::Width => "width",
            Self::Height => "height",
        }
    }
}

/// Moving edge of a resizable surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResizeEdge {
    Left,
    Right,
    /// Supported by the reusable component even though no current panel uses it.
    #[allow(dead_code)]
    Top,
    Bottom,
}

/// Complete declaration for one resizable surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizableSpec {
    pub(crate) target: &'static str,
    pub(crate) axis: ResizeAxis,
    pub(crate) edge: ResizeEdge,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) default: f32,
}

impl ResizableSpec {
    pub(crate) fn binding_id(self) -> String {
        format!(
            "resizable_{}_{}",
            self.target.replace('-', "_"),
            self.axis.binding_suffix()
        )
    }

    pub(crate) fn drag(self) -> ResizeDrag {
        ResizeDrag {
            target: self.target.to_owned(),
            axis: self.axis,
            edge: self.edge,
        }
    }
}

/// Single registry for every resizable Studio surface.
pub(crate) const RESIZABLE_SPECS: &[ResizableSpec] = &[
    ResizableSpec {
        target: "project-rail",
        axis: ResizeAxis::Width,
        edge: ResizeEdge::Right,
        min: 190.0,
        max: 480.0,
        default: 250.0,
    },
    ResizableSpec {
        target: "inspector",
        axis: ResizeAxis::Width,
        edge: ResizeEdge::Left,
        min: 272.0,
        max: 560.0,
        default: 320.0,
    },
    ResizableSpec {
        target: "component-palette",
        axis: ResizeAxis::Height,
        edge: ResizeEdge::Bottom,
        min: 120.0,
        max: 620.0,
        default: 220.0,
    },
];

/// Active native or semantic resize gesture. GPUI dispatches drag moves to
/// every listener of this payload type, so the target discriminator must travel
/// with the gesture rather than being captured from the listener.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResizeDrag {
    pub(crate) target: String,
    pub(crate) axis: ResizeAxis,
    pub(crate) edge: ResizeEdge,
}

/// Window-relative target bounds used to derive a panel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizeBounds {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// Window-relative pointer position for a resize gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizePoint {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// Session-only size state and invariant-preserving operations for declared
/// resizable surfaces.
pub(crate) struct Resizable {
    specs: &'static [ResizableSpec],
    sizes: RefCell<BTreeMap<&'static str, f32>>,
}

impl Resizable {
    pub(crate) fn new(specs: &'static [ResizableSpec]) -> Self {
        Self {
            specs,
            sizes: RefCell::new(
                specs
                    .iter()
                    .map(|spec| (spec.target, spec.default))
                    .collect(),
            ),
        }
    }

    pub(crate) fn specs(&self) -> &'static [ResizableSpec] {
        self.specs
    }

    pub(crate) fn spec(&self, target: &str) -> Option<ResizableSpec> {
        self.specs
            .iter()
            .copied()
            .find(|spec| spec.target == target)
    }

    pub(crate) fn size(&self, target: &str) -> Option<f32> {
        let spec = self.spec(target)?;
        Some(
            self.sizes
                .borrow()
                .get(spec.target)
                .copied()
                .unwrap_or(spec.default),
        )
    }

    pub(crate) fn set_size(&self, target: &str, size: f32) -> bool {
        let Some(spec) = self.spec(target) else {
            return false;
        };
        if !size.is_finite() {
            return false;
        }
        let clamped = size.clamp(spec.min, spec.max);
        let previous = self.sizes.borrow_mut().insert(spec.target, clamped);
        previous != Some(clamped)
    }

    pub(crate) fn resize(
        &self,
        drag: &ResizeDrag,
        bounds: ResizeBounds,
        pointer: ResizePoint,
    ) -> bool {
        let Some(spec) = self.spec(&drag.target) else {
            return false;
        };
        if spec.axis != drag.axis || spec.edge != drag.edge {
            return false;
        }
        let raw = match (spec.axis, spec.edge) {
            (ResizeAxis::Width, ResizeEdge::Left) => bounds.x + bounds.width - pointer.x,
            (ResizeAxis::Width, ResizeEdge::Right) => pointer.x - bounds.x,
            (ResizeAxis::Height, ResizeEdge::Top) => bounds.y + bounds.height - pointer.y,
            (ResizeAxis::Height, ResizeEdge::Bottom) => pointer.y - bounds.y,
            (ResizeAxis::Width, ResizeEdge::Top | ResizeEdge::Bottom)
            | (ResizeAxis::Height, ResizeEdge::Left | ResizeEdge::Right) => return false,
        };
        self.set_size(spec.target, raw)
    }

    pub(crate) fn reset(&self, target: &str) -> bool {
        let Some(spec) = self.spec(target) else {
            return false;
        };
        let _changed = self.set_size(spec.target, spec.default);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{RESIZABLE_SPECS, Resizable, ResizeBounds, ResizePoint};

    #[test]
    fn active_drag_clamps_its_target_without_mutating_other_handles() {
        let resizable = Resizable::new(RESIZABLE_SPECS);
        let inspector_drag = resizable.spec("inspector").map(|spec| spec.drag());
        assert!(inspector_drag.is_some(), "inspector resize spec must exist");
        let Some(inspector_drag) = inspector_drag else {
            return;
        };
        let inspector_bounds = ResizeBounds {
            x: 1_000.0,
            y: 46.0,
            width: 320.0,
            height: 854.0,
        };

        // Simulate both width-handle listeners receiving the same active GPUI
        // payload. Both must resolve the payload target, not their own handle.
        for _listener in ["project-rail", "inspector"] {
            let _changed = resizable.resize(
                &inspector_drag,
                inspector_bounds,
                ResizePoint { x: 0.0, y: 0.0 },
            );
        }

        assert_eq!(resizable.size("inspector"), Some(560.0));
        assert_eq!(resizable.size("project-rail"), Some(250.0));

        let _changed = resizable.resize(
            &inspector_drag,
            inspector_bounds,
            ResizePoint { x: 1_500.0, y: 0.0 },
        );
        assert_eq!(resizable.size("inspector"), Some(272.0));
        assert_eq!(resizable.size("project-rail"), Some(250.0));
    }
}
