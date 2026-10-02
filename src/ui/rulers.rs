//! Canvas rulers (Shift+R): ticks in document pixels measured from the
//! selected artboard's origin, with the selection's extent highlighted.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, BorderStyle, Bounds, Context, IntoElement, ParentElement as _, Pixels, Styled as _,
    canvas, div, fill, point, px, quad, size,
};

use super::Studio;
use crate::geometry::Rect;
use crate::model::style::fmt_num;

/// Ruler thickness (screen px).
pub(crate) const RULER: f32 = 20.0;

/// The tick spacing (document px) that keeps labels at least ~64px apart.
fn step_for(zoom: f32) -> f32 {
    const STEPS: [f32; 14] = [
        1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0,
    ];
    STEPS
        .into_iter()
        .find(|s| s * zoom >= 64.0)
        .unwrap_or(10_000.0)
}

impl Studio {
    /// Ruler overlay, or `None` when hidden.
    pub(crate) fn render_rulers(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.rulers {
            return None;
        }
        let theme = cx.theme().clone();
        let zoom = self.canvas.camera.zoom;
        let pan = self.canvas.camera.pan;
        let viewport = self.canvas.bounds.get().size;
        let doc = &self.editor.doc;
        // Measure from the selected (or first) artboard's origin.
        let origin = self
            .editor
            .primary()
            .and_then(|id| doc.artboard_of(id))
            .map_or((0.0, 0.0), |a| (a.x, a.y));
        let selection = Rect::union_all(
            self.editor
                .selection
                .iter()
                .filter_map(|id| self.canvas.doc_rect(*id)),
        );
        let step = step_for(zoom);
        let minor = step / 5.0;
        // Screen position of document coordinate `v` along an axis.
        let sx = move |v: f32| pan.x.as_f32() + v * zoom;
        let sy = move |v: f32| pan.y.as_f32() + v * zoom;
        let start_x = ((-pan.x.as_f32() / zoom - origin.0) / step).floor() * step;
        let end_x =
            ((viewport.width.as_f32() - pan.x.as_f32()) / zoom - origin.0) / step * step + step;
        let start_y = ((-pan.y.as_f32() / zoom - origin.1) / step).floor() * step;
        let end_y =
            ((viewport.height.as_f32() - pan.y.as_f32()) / zoom - origin.1) / step * step + step;
        let mut labels: Vec<AnyElement> = Vec::new();
        let label_color = theme.muted_foreground;
        let mut v = start_x;
        while v <= end_x && labels.len() < 200 {
            let x = sx(origin.0 + v);
            if x > RULER {
                labels.push(
                    div()
                        .absolute()
                        .left(px(x + 3.0))
                        .top(px(2.0))
                        .text_size(px(9.0))
                        .text_color(label_color)
                        .child(fmt_num(v))
                        .into_any_element(),
                );
            }
            v += step;
        }
        let mut v = start_y;
        while v <= end_y && labels.len() < 400 {
            let y = sy(origin.1 + v);
            if y > RULER {
                // Digits stack vertically, like a rotated label.
                let mut digits = div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(y + 3.0))
                    .w(px(RULER - 6.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_size(px(8.0))
                    .line_height(px(8.0))
                    .text_color(label_color);
                for ch in fmt_num(v).chars() {
                    digits = digits.child(ch.to_string());
                }
                labels.push(digits.into_any_element());
            }
            v += step;
        }
        let bg = theme.background;
        let border = theme.border;
        let tick = theme.muted_foreground.opacity(0.6);
        let accent = theme.primary;
        let ticks = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, (), window, _| {
                let o = bounds.origin;
                let w = bounds.size.width;
                let h = bounds.size.height;
                let r = px(RULER);
                window.paint_quad(fill(
                    Bounds {
                        origin: o,
                        size: size(w, r),
                    },
                    bg,
                ));
                window.paint_quad(fill(
                    Bounds {
                        origin: o,
                        size: size(r, h),
                    },
                    bg,
                ));
                if let Some(sel) = selection {
                    let (x0, x1) = (sx(sel.x), sx(sel.right()));
                    let (y0, y1) = (sy(sel.y), sy(sel.bottom()));
                    window.paint_quad(fill(
                        Bounds {
                            origin: o + point(px(x0), px(0.0)),
                            size: size(px((x1 - x0).max(1.0)), r),
                        },
                        accent.opacity(0.15),
                    ));
                    window.paint_quad(fill(
                        Bounds {
                            origin: o + point(px(0.0), px(y0)),
                            size: size(r, px((y1 - y0).max(1.0))),
                        },
                        accent.opacity(0.15),
                    ));
                }
                let mut v = start_x;
                while v <= end_x {
                    let x = sx(origin.0 + v);
                    let major = ((v / step).round() * step - v).abs() < minor / 2.0;
                    let len = if major { RULER } else { 5.0 };
                    if x >= RULER {
                        window.paint_quad(fill(
                            Bounds {
                                origin: o + point(px(x), px(RULER - len)),
                                size: size(px(1.0), px(len)),
                            },
                            tick,
                        ));
                    }
                    v += minor;
                }
                let mut v = start_y;
                while v <= end_y {
                    let y = sy(origin.1 + v);
                    let major = ((v / step).round() * step - v).abs() < minor / 2.0;
                    let len = if major { RULER } else { 5.0 };
                    if y >= RULER {
                        window.paint_quad(fill(
                            Bounds {
                                origin: o + point(px(RULER - len), px(y)),
                                size: size(px(len), px(1.0)),
                            },
                            tick,
                        ));
                    }
                    v += minor;
                }
                // Edges and the corner square.
                window.paint_quad(fill(
                    Bounds {
                        origin: o + point(px(0.0), r - px(1.0)),
                        size: size(w, px(1.0)),
                    },
                    border,
                ));
                window.paint_quad(fill(
                    Bounds {
                        origin: o + point(r - px(1.0), px(0.0)),
                        size: size(px(1.0), h),
                    },
                    border,
                ));
                window.paint_quad(quad(
                    Bounds {
                        origin: o,
                        size: size(r, r),
                    },
                    px(0.0),
                    bg,
                    px(0.0),
                    border,
                    BorderStyle::Solid,
                ));
            },
        )
        .absolute()
        .inset_0();
        Some(
            div()
                .absolute()
                .inset_0()
                .child(ticks)
                .children(labels)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::step_for;

    #[test]
    fn tick_steps_stay_readable() {
        assert_eq!(step_for(1.0), 100.0);
        assert_eq!(step_for(0.5), 200.0);
        assert_eq!(step_for(8.0), 10.0);
        assert_eq!(step_for(0.01), 10_000.0);
    }
}
