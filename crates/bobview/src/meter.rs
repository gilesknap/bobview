//! The linear meter: a scale with coloured warning regions and a needle at
//! the value, after Phoebus' RTLinearMeter.

use bob::Widget;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Shape, Stroke, Vec2};
use pv::PvState;

use crate::axis::{self, Scale};
use crate::style::color;
use crate::widgets::{bg, fg, fill, font, outline};

/// Phoebus' default region colours.
const NORMAL: Color32 = Color32::from_rgb(194, 198, 195);
const MINOR: Color32 = Color32::from_rgb(242, 148, 141);
const MAJOR: Color32 = Color32::from_rgb(240, 60, 46);

fn prop_color(w: &Widget, key: &str, default: Color32) -> Color32 {
    w.color(key).map(|c| color(&c)).unwrap_or(default)
}

/// The scale range: the PV's display limits when `limits_from_pv` is set and
/// it has them, else the widget's minimum and maximum.
pub fn limits(w: &Widget, s: Option<&PvState>) -> (f64, f64) {
    let own = (
        w.f64("minimum").unwrap_or(0.0),
        w.f64("maximum").unwrap_or(100.0),
    );
    s.filter(|_| w.bool("limits_from_pv").unwrap_or(true))
        .and_then(|s| s.range())
        .unwrap_or(own)
}

/// Coloured regions along the scale, low to high: (from, to, colour).
pub fn regions(w: &Widget, lo: f64, hi: f64) -> Vec<(f64, f64, Color32)> {
    let normal = prop_color(w, "normal_status_color", NORMAL);
    let minor = prop_color(w, "minor_warning_color", MINOR);
    let major = prop_color(w, "major_warning_color", MAJOR);
    let level = |k: &str| w.f64(k).filter(|v| v.is_finite());
    let clamp = |v: f64| v.clamp(lo, hi);
    let lolo = level("level_lolo").map_or(lo, clamp);
    let low = level("level_low").map_or(lo, clamp).max(lolo);
    let hihi = level("level_hihi").map_or(hi, clamp);
    let high = level("level_high").map_or(hi, clamp).min(hihi).max(low);
    [
        (lo, lolo, major),
        (lolo, low, minor),
        (low, high, normal),
        (high, hihi, minor),
        (hihi, hi, major),
    ]
    .into_iter()
    .filter(|(a, b, _)| b > a)
    .collect()
}

pub fn linear_meter(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let (lo, hi) = limits(w, s);
    let horizontal = w.bool("display_horizontal").unwrap_or(true);
    if w.bool("transparent") != Some(true) {
        fill(painter, rect, bg(w, "Background"));
    }
    let fnt = font(w, z);
    let text = fg(w);
    let show_scale = w.bool("scale_visible").unwrap_or(true);
    let knob = w.f64("knob_size").unwrap_or(8.0).max(0.0) as f32 * z;
    let tick = 5.0 * z;
    let pad = 2.0 * z;
    let line_h = painter
        .layout_no_wrap("0".into(), fnt.clone(), text)
        .size()
        .y;

    // Room for the scale labels at the ends, so the first and last fit.
    let probe = Scale {
        lo,
        hi,
        log: false,
        from: 0.0,
        to: if horizontal {
            rect.width()
        } else {
            rect.height()
        },
    };
    let label_w = |v: f64, step: f64| {
        painter
            .layout_no_wrap(axis::label(v, step), fnt.clone(), text)
            .size()
            .x
    };
    let widest = label_w(lo, (hi - lo) / 10.0).max(label_w(hi, (hi - lo) / 10.0));
    let (ticks, step) = probe.ticks(if horizontal {
        widest + 12.0 * z
    } else {
        line_h * 2.0
    });

    // The bar, and the scale beside it.
    let (bar, scale) = if horizontal {
        let side = widest / 2.0 + pad;
        let scale_h = if show_scale { tick + line_h } else { 0.0 };
        let bar = Rect::from_min_max(
            Pos2::new(rect.left() + side, rect.top() + pad + knob),
            Pos2::new(rect.right() - side, rect.bottom() - pad - scale_h),
        );
        let scale = Scale {
            lo,
            hi,
            log: false,
            from: bar.left(),
            to: bar.right(),
        };
        (bar, scale)
    } else {
        let scale_w = if show_scale { tick + pad + widest } else { 0.0 };
        let bar = Rect::from_min_max(
            Pos2::new(rect.left() + pad + knob, rect.top() + line_h / 2.0 + pad),
            Pos2::new(
                rect.right() - pad - scale_w,
                rect.bottom() - line_h / 2.0 - pad,
            ),
        );
        let scale = Scale {
            lo,
            hi,
            log: false,
            from: bar.bottom(),
            to: bar.top(),
        };
        (bar, scale)
    };
    if bar.width() <= 1.0 || bar.height() <= 1.0 || hi <= lo {
        return;
    }

    let span = |a: f64, b: f64| {
        let (p, q) = (scale.pos(a), scale.pos(b));
        if horizontal {
            Rect::from_x_y_ranges(p..=q, bar.y_range())
        } else {
            Rect::from_x_y_ranges(bar.x_range(), q..=p)
        }
    };
    for (a, b, c) in regions(w, lo, hi) {
        fill(painter, span(a, b), c);
    }
    if w.bool("show_limits") != Some(false) {
        let stroke = Stroke::new(z.max(1.0), Color32::from_gray(90));
        for k in ["level_lolo", "level_low", "level_high", "level_hihi"] {
            if let Some(v) = w.f64(k).filter(|v| *v > lo && *v < hi) {
                let p = scale.pos(v);
                let seg = if horizontal {
                    [Pos2::new(p, bar.top()), Pos2::new(p, bar.bottom())]
                } else {
                    [Pos2::new(bar.left(), p), Pos2::new(bar.right(), p)]
                };
                painter.line_segment(seg, stroke);
            }
        }
    }
    outline(painter, bar, 1.0, Color32::from_gray(120));

    if show_scale {
        let stroke = Stroke::new(1.0, text);
        for v in &ticks {
            let p = scale.pos(*v);
            let label = axis::label(*v, step);
            if horizontal {
                let y = bar.bottom();
                painter.line_segment([Pos2::new(p, y), Pos2::new(p, y + tick)], stroke);
                painter.text(
                    Pos2::new(p, y + tick),
                    Align2::CENTER_TOP,
                    label,
                    fnt.clone(),
                    text,
                );
            } else {
                let x = bar.right();
                painter.line_segment([Pos2::new(x, p), Pos2::new(x + tick, p)], stroke);
                painter.text(
                    Pos2::new(x + tick + pad, p),
                    Align2::LEFT_CENTER,
                    label,
                    fnt.clone(),
                    text,
                );
            }
        }
    }

    // The needle and the knob that points at it.
    let Some(v) = s
        .filter(|s| s.connected)
        .and_then(|s| s.value.as_f64())
        .filter(|v| v.is_finite())
    else {
        return;
    };
    let p = scale.pos(v.clamp(lo, hi));
    let needle = prop_color(w, "needle_color", Color32::BLACK);
    let knob_c = prop_color(w, "knob_color", Color32::BLACK);
    let nw = (w.f64("needle_width").unwrap_or(2.0) as f32 * z).max(1.0);
    if horizontal {
        painter.line_segment(
            [Pos2::new(p, bar.top()), Pos2::new(p, bar.bottom())],
            Stroke::new(nw, needle),
        );
        if knob > 0.0 {
            let tip = Pos2::new(p, bar.top());
            painter.add(Shape::convex_polygon(
                vec![
                    tip,
                    tip + Vec2::new(knob * 0.6, -knob),
                    tip + Vec2::new(-knob * 0.6, -knob),
                ],
                knob_c,
                Stroke::NONE,
            ));
        }
    } else {
        painter.line_segment(
            [Pos2::new(bar.left(), p), Pos2::new(bar.right(), p)],
            Stroke::new(nw, needle),
        );
        if knob > 0.0 {
            let tip = Pos2::new(bar.left(), p);
            painter.add(Shape::convex_polygon(
                vec![
                    tip,
                    tip + Vec2::new(-knob, -knob * 0.6),
                    tip + Vec2::new(-knob, knob * 0.6),
                ],
                knob_c,
                Stroke::NONE,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(xml: &str) -> Widget {
        let d = bob::Display::parse(&format!("<display>{xml}</display>")).unwrap();
        d.widgets[0].clone()
    }

    #[test]
    fn levels_at_the_limits_leave_one_normal_region() {
        let w = widget(
            "<widget type='linearmeter'><minimum>-25</minimum><maximum>25</maximum>
             <level_lolo>-25</level_lolo><level_low>-25</level_low>
             <level_high>25</level_high><level_hihi>25</level_hihi></widget>",
        );
        assert_eq!(regions(&w, -25.0, 25.0), vec![(-25.0, 25.0, NORMAL)]);
    }

    #[test]
    fn warning_regions_follow_the_levels() {
        let w = widget(
            "<widget type='linearmeter'><level_lolo>10</level_lolo><level_low>20</level_low>
             <level_high>80</level_high><level_hihi>90</level_hihi></widget>",
        );
        let r: Vec<(f64, f64)> = regions(&w, 0.0, 100.0)
            .iter()
            .map(|(a, b, _)| (*a, *b))
            .collect();
        assert_eq!(
            r,
            vec![
                (0.0, 10.0),
                (10.0, 20.0),
                (20.0, 80.0),
                (80.0, 90.0),
                (90.0, 100.0)
            ]
        );
    }

    #[test]
    fn limits_come_from_the_widget_unless_the_pv_has_them() {
        let w = widget(
            "<widget type='linearmeter'><limits_from_pv>false</limits_from_pv>
             <minimum>-360</minimum><maximum>360</maximum></widget>",
        );
        let s = PvState {
            display_range: Some((0.0, 10.0)),
            ..Default::default()
        };
        assert_eq!(limits(&w, Some(&s)), (-360.0, 360.0));
        let w = widget("<widget type='linearmeter'></widget>");
        assert_eq!(limits(&w, Some(&s)), (0.0, 10.0));
        assert_eq!(limits(&w, None), (0.0, 100.0));
    }
}
