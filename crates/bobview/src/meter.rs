//! The linear meter: a scale with coloured warning regions and a needle at
//! the value, after Phoebus' RTLinearMeter; and the round meter, a dial.

use bob::Widget;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Shape, Stroke, Vec2};
use pv::PvState;

use crate::axis::{self, Scale};
use crate::style::color;
use crate::widgets::{bg, fg, fill, font, format_value, outline};

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
    // Finite, or the meters' `hi <= lo` checks and `clamp` fail on NaN.
    let finite = |k, default| w.f64(k).filter(|v| v.is_finite()).unwrap_or(default);
    let own = (finite("minimum", 0.0), finite("maximum", 100.0));
    s.filter(|_| w.bool("limits_from_pv").unwrap_or(true))
        .and_then(|s| s.range())
        .filter(|(lo, hi)| lo.is_finite() && hi.is_finite())
        .unwrap_or(own)
}

/// The levels (LOLO, LOW, HIGH, HIHI) the regions follow: the PV's alarm
/// and warning limits when `limits_from_pv` is set, as Phoebus does, so
/// none until it connects or for limits it leaves unset (a range that is
/// empty); else the widget's `level_*`.
pub fn levels(w: &Widget, s: Option<&PvState>) -> [Option<f64>; 4] {
    if w.bool("limits_from_pv").unwrap_or(true) {
        let range = |r: Option<(f64, f64)>| r.filter(|(a, b)| a < b);
        let alarm = range(s.and_then(|s| s.alarm_range));
        let warning = range(s.and_then(|s| s.warning_range));
        return [
            alarm.map(|r| r.0),
            warning.map(|r| r.0),
            warning.map(|r| r.1),
            alarm.map(|r| r.1),
        ];
    }
    ["level_lolo", "level_low", "level_high", "level_hihi"]
        .map(|k| w.f64(k).filter(|v| v.is_finite()))
}

/// Coloured regions along the scale, low to high: (from, to, colour).
pub fn regions(w: &Widget, levels: [Option<f64>; 4], lo: f64, hi: f64) -> Vec<(f64, f64, Color32)> {
    let normal = prop_color(w, "normal_status_color", NORMAL);
    let minor = prop_color(w, "minor_warning_color", MINOR);
    let major = prop_color(w, "major_warning_color", MAJOR);
    if lo.partial_cmp(&hi) != Some(std::cmp::Ordering::Less) {
        return Vec::new();
    }
    let clamp = |v: f64| v.clamp(lo, hi);
    let [lolo, low, high, hihi] = levels;
    let lolo = lolo.map_or(lo, clamp);
    let low = low.map_or(lo, clamp).max(lolo);
    let hihi = hihi.map_or(hi, clamp);
    let high = high.map_or(hi, clamp).min(hihi).max(low);
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
    let levels = levels(w, s);
    for (a, b, c) in regions(w, levels, lo, hi) {
        fill(painter, span(a, b), c);
    }
    if w.bool("show_limits") != Some(false) {
        let stroke = Stroke::new(z.max(1.0), Color32::from_gray(90));
        for level in levels {
            if let Some(v) = level.filter(|v| *v > lo && *v < hi) {
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

/// Where the dial's scale starts and ends, in degrees anticlockwise from
/// three o'clock: a 120 degree arc over the pivot, as BOY's meter.
const DIAL_FROM: f32 = 150.0;
const DIAL_TO: f32 = 30.0;

/// The point at `radius` from `pivot`, `deg` degrees anticlockwise from
/// three o'clock (screen y points down).
fn polar(pivot: Pos2, radius: f32, deg: f32) -> Pos2 {
    let a = deg.to_radians();
    pivot + Vec2::new(a.cos(), -a.sin()) * radius
}

/// The ticks to label on a dial scale `length` pixels long: every decade of
/// a log scale, thinned until labels `label_len` apart fit.
fn dial_ticks(scale: &Scale, label_len: f32) -> (Vec<f64>, f64) {
    let (ticks, step) = scale.ticks(label_len);
    if !scale.log || ticks.len() < 3 {
        return (ticks, step);
    }
    let length = (scale.to - scale.from).abs();
    let fit = (length / label_len.max(1.0)).floor().max(1.0) as usize + 1;
    let every = ticks.len().div_ceil(fit).max(1);
    (ticks.into_iter().step_by(every).collect(), step)
}

/// A tick label: decades of a log scale as `1e-3`.
fn dial_label(v: f64, step: f64, log: bool) -> String {
    if log && !(0.01..=1000.0).contains(&v) {
        format!("{v:e}")
    } else {
        axis::label(v, step)
    }
}

/// The round meter: a dial over a pivot at the bottom centre, with the
/// warning regions along its rim, labelled ticks and a needle at the value.
pub fn dial(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let (lo, hi) = limits(w, s);
    let log = w.bool("log_scale") == Some(true) && lo > 0.0;
    if w.bool("transparent") != Some(true) {
        fill(painter, rect, bg(w, "Background"));
    }
    if hi <= lo {
        return;
    }
    let fnt = font(w, z);
    let text = fg(w);
    let pad = 3.0 * z;
    let half = (DIAL_FROM - 90.0).to_radians();
    let pivot = Pos2::new(rect.center().x, rect.bottom() - pad);
    let radius = ((rect.width() / 2.0 - pad) / half.sin()).min(rect.height() - 2.0 * pad);
    if radius <= 4.0 * z {
        return;
    }
    let sweep = (DIAL_FROM - DIAL_TO).to_radians() * radius;
    let scale = Scale {
        lo,
        hi,
        log,
        from: 0.0,
        to: sweep,
    };
    let angle = |v: f64| DIAL_FROM - (DIAL_FROM - DIAL_TO) * scale.pos(v.clamp(lo, hi)) / sweep;
    let arc = |a: f64, b: f64, r: f32| -> Vec<Pos2> {
        let (from, to) = (angle(a), angle(b));
        let n = ((from - to).abs() / 3.0).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| polar(pivot, r, from + (to - from) * i as f32 / n as f32))
            .collect()
    };

    // The warning regions as a band along the rim.
    let band = (radius * 0.08).max(3.0 * z);
    for (a, b, c) in regions(w, levels(w, s), lo, hi) {
        painter.add(Shape::line(
            arc(a, b, radius - band / 2.0),
            Stroke::new(band, c),
        ));
    }
    painter.add(Shape::line(
        arc(lo, hi, radius - band),
        Stroke::new(1.0, text),
    ));

    if w.bool("show_scale") != Some(false) {
        let line_h = painter
            .layout_no_wrap("0".into(), fnt.clone(), text)
            .size()
            .y;
        let label_w = |v: f64| {
            painter
                .layout_no_wrap(dial_label(v, (hi - lo) / 10.0, log), fnt.clone(), text)
                .size()
                .x
        };
        let widest = label_w(lo).max(label_w(hi));
        let tick = 4.0 * z;
        let (ticks, step) = dial_ticks(&scale, widest + 6.0 * z);
        let stroke = Stroke::new(1.0, text);
        for v in ticks {
            let a = angle(v);
            let r = radius - band;
            painter.line_segment([polar(pivot, r, a), polar(pivot, r - tick, a)], stroke);
            painter.text(
                polar(pivot, r - tick - line_h * 0.8, a),
                Align2::CENTER_CENTER,
                dial_label(v, step, log),
                fnt.clone(),
                text,
            );
        }
    }

    let value = s
        .filter(|s| s.connected)
        .and_then(|s| s.value.as_f64().map(|v| (s, v)))
        .filter(|(_, v)| v.is_finite());
    if let Some((s, v)) = value {
        if w.bool("show_value") == Some(true) {
            painter.text(
                pivot - Vec2::new(0.0, radius * 0.35),
                Align2::CENTER_CENTER,
                format_value(w, s),
                fnt.clone(),
                text,
            );
        }
        let needle = prop_color(w, "needle_color", Color32::BLACK);
        let nw = (w.f64("needle_width").unwrap_or(2.0) as f32 * z).max(1.0);
        painter.line_segment(
            [pivot, polar(pivot, radius - band, angle(v))],
            Stroke::new(nw, needle),
        );
    }
    let knob = prop_color(
        w,
        "knob_color",
        prop_color(w, "needle_color", Color32::BLACK),
    );
    painter.circle_filled(pivot, 3.0 * z, knob);
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
            "<widget type='linearmeter'><limits_from_pv>false</limits_from_pv>
             <minimum>-25</minimum><maximum>25</maximum>
             <level_lolo>-25</level_lolo><level_low>-25</level_low>
             <level_high>25</level_high><level_hihi>25</level_hihi></widget>",
        );
        let levels = levels(&w, None);
        assert_eq!(
            regions(&w, levels, -25.0, 25.0),
            vec![(-25.0, 25.0, NORMAL)]
        );
    }

    #[test]
    fn warning_regions_follow_the_levels() {
        let w = widget(
            "<widget type='linearmeter'><limits_from_pv>false</limits_from_pv>
             <level_lolo>10</level_lolo><level_low>20</level_low>
             <level_high>80</level_high><level_hihi>90</level_hihi></widget>",
        );
        let r: Vec<(f64, f64)> = regions(&w, levels(&w, None), 0.0, 100.0)
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
    fn levels_from_the_pv_ignore_the_widget_levels() {
        // As the BOY gauge meters: static levels far above a log scale.
        let w = widget(
            "<widget type='meter'><level_lolo>10</level_lolo><level_low>20</level_low>
             <level_high>80</level_high><level_hihi>90</level_hihi></widget>",
        );
        assert_eq!(levels(&w, None), [None; 4]);
        assert_eq!(regions(&w, levels(&w, None), 1e-11, 0.01).len(), 1);
        // CA reports 0 for limits a record leaves unset.
        let unset = PvState {
            alarm_range: Some((0.0, 0.0)),
            warning_range: Some((0.0, 0.0)),
            ..Default::default()
        };
        assert_eq!(levels(&w, Some(&unset)), [None; 4]);
        let s = PvState {
            alarm_range: Some((1e-10, 1e-3)),
            warning_range: Some((1e-9, 1e-4)),
            ..Default::default()
        };
        assert_eq!(
            levels(&w, Some(&s)),
            [Some(1e-10), Some(1e-9), Some(1e-4), Some(1e-3)]
        );
        assert_eq!(regions(&w, levels(&w, Some(&s)), 1e-11, 0.01).len(), 5);
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
        // Never NaN, which `clamp` panics on.
        let w = widget(
            "<widget type='meter'><limits_from_pv>false</limits_from_pv>
             <minimum>NaN</minimum><maximum>inf</maximum></widget>",
        );
        assert_eq!(limits(&w, None), (0.0, 100.0));
        assert!(regions(&w, [None; 4], f64::NAN, 1.0).is_empty());
    }

    #[test]
    fn dial_thins_log_decades_to_fit() {
        let scale = Scale {
            lo: 0.001,
            hi: 1000.0,
            log: true,
            from: 0.0,
            to: 200.0,
        };
        let (t, _) = dial_ticks(&scale, 40.0);
        assert_eq!(t, vec![0.001, 0.1, 10.0, 1000.0]);
        let (t, _) = dial_ticks(&scale, 10.0);
        assert_eq!(t.len(), 7);
        assert_eq!(dial_label(0.001, 0.0, true), "1e-3");
        assert_eq!(dial_label(10.0, 0.0, true), "10");
    }
}
