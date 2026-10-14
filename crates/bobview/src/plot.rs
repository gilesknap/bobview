//! The XY plot widget: array PVs drawn against their index or another array,
//! with axes, grid and legend laid out as in Phoebus.

use std::f32::consts::FRAC_PI_2;

use bob::{Element, Widget};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2, epaint};
use pv::{Pv, PvManager, PvState, Value};

use crate::axis::{self, Scale};
use crate::screen::Node;
use crate::style::{self, color, named};
use crate::widgets::{alarm_border, fill};

/// The PVs of one trace, in `<traces>` order.
pub struct TracePvs {
    pub x: Option<Pv>,
    pub y: Option<Pv>,
}

fn trace_elements(w: &Widget) -> Vec<&Element> {
    w.get("traces")
        .map(|t| t.children.iter().filter(|c| c.name == "trace").collect())
        .unwrap_or_default()
}

/// Subscribe to each trace's x and y PVs. `expand` expands macros.
pub fn subscribe(w: &Widget, expand: &dyn Fn(&str) -> String, pvs: &PvManager) -> Vec<TracePvs> {
    trace_elements(w)
        .iter()
        .map(|t| {
            let pv = |k: &str| {
                t.child_text(k)
                    .map(expand)
                    .filter(|n| !n.trim().is_empty())
                    .map(|n| pvs.subscribe(&n))
            };
            TracePvs {
                x: pv("x_pv"),
                y: pv("y_pv"),
            }
        })
        .collect()
}

/// `traces[0].y_pv` and the like: a property of one trace, as Phoebus lets
/// trace names refer to them.
fn trace_prop(w: &Widget, path: &str) -> Option<String> {
    let rest = path.strip_prefix("traces[")?;
    let (index, prop) = rest.split_once("].")?;
    let t = *trace_elements(w).get(index.parse::<usize>().ok()?)?;
    t.child_text(prop).map(str::to_string)
}

/// Expand macros in `text`, including `$(traces[N].prop)`.
fn expand(node: &Node, text: &str) -> String {
    bob::expand(text, &|name| {
        node.macros
            .get(name)
            .map(str::to_string)
            .or_else(|| trace_prop(&node.w, name).map(|raw| node.expand(&raw)))
            .or_else(|| {
                node.w
                    .text(name)
                    .filter(|t| !t.contains('$'))
                    .map(str::to_string)
            })
    })
}

/// The numbers in a PV value, if it has any.
fn numbers(s: &PvState) -> Option<Vec<f64>> {
    if !s.connected {
        return None;
    }
    match &s.value {
        Value::DoubleArray(a) => Some(a.to_vec()),
        Value::LongArray(a) => Some(a.iter().map(|v| *v as f64).collect()),
        v => v.as_f64().map(|v| vec![v]),
    }
}

/// Points of a trace: y against x, or against the index when there is no x PV.
fn points(x: Option<&[f64]>, y: &[f64]) -> Vec<(f64, f64)> {
    match x {
        Some(x) => x.iter().copied().zip(y.iter().copied()).collect(),
        None => y.iter().enumerate().map(|(i, v)| (i as f64, *v)).collect(),
    }
}

/// One axis' settings, with the Phoebus defaults.
struct AxisCfg {
    title: String,
    autoscale: bool,
    log: bool,
    range: (f64, f64),
    grid: bool,
    visible: bool,
    on_right: bool,
    color: Color32,
    title_font: FontId,
    scale_font: FontId,
}

fn axis_cfg(el: Option<&Element>, title: &str, node: &Node, z: f32) -> AxisCfg {
    let w = Widget::from_element(el.cloned().unwrap_or_default());
    let font = |k: &str, bold: bool| {
        let f = w.font(k).unwrap_or_else(|| bob::Font {
            style: if bold {
                bob::FontStyle::Bold
            } else {
                bob::FontStyle::Regular
            },
            ..Default::default()
        });
        style::font_id(&f, z)
    };
    AxisCfg {
        title: w
            .text("title")
            .map(|t| expand(node, t))
            .unwrap_or_else(|| title.to_string()),
        autoscale: w.bool("autoscale").unwrap_or(false),
        log: w.bool("log_scale").unwrap_or(false),
        range: (
            w.f64("minimum").unwrap_or(0.0),
            w.f64("maximum").unwrap_or(100.0),
        ),
        grid: w.bool("show_grid").unwrap_or(false),
        visible: w.bool("visible").unwrap_or(true),
        on_right: w.bool("on_right").unwrap_or(false),
        color: w
            .color("color")
            .map(|c| color(&c))
            .unwrap_or(Color32::BLACK),
        title_font: font("title_font", true),
        scale_font: font("scale_font", false),
    }
}

/// The range an axis shows: its own limits, or the data's when autoscaling.
fn axis_range(cfg: &AxisCfg, data: impl Iterator<Item = f64>, nice: bool) -> (f64, f64) {
    let ok = |v: &f64| v.is_finite() && (!cfg.log || *v > 0.0);
    let (lo, hi) = data
        .filter(ok)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    if !cfg.autoscale || lo > hi {
        return cfg.range;
    }
    if cfg.log {
        let (a, b) = (
            lo.log10().floor(),
            hi.log10().ceil().max(lo.log10().floor() + 1.0),
        );
        (10f64.powf(a), 10f64.powf(b))
    } else if nice || lo == hi {
        axis::nice_range(lo, hi, 8)
    } else {
        (lo, hi)
    }
}

struct Trace {
    name: String,
    axis: usize,
    kind: i64,
    color: Color32,
    width: f32,
    line_style: i64,
    point: i64,
    point_size: f32,
    visible: bool,
    points: Vec<(f64, f64)>,
}

fn traces(node: &Node, z: f32) -> Vec<Trace> {
    trace_elements(&node.w)
        .iter()
        .enumerate()
        .map(|(i, el)| {
            let t = Widget::from_element((*el).clone());
            let pvs = node.traces.get(i);
            let data = |pv: Option<&Pv>| pv.and_then(|p| numbers(&p.state()));
            let y = data(pvs.and_then(|p| p.y.as_ref()));
            let x = data(pvs.and_then(|p| p.x.as_ref()));
            let has_x = pvs.is_some_and(|p| p.x.is_some());
            let points = match (y, has_x) {
                (Some(y), false) => points(None, &y),
                (Some(y), true) => x.map(|x| points(Some(&x), &y)).unwrap_or_default(),
                (None, _) => Vec::new(),
            };
            Trace {
                name: t.text("name").map(|n| expand(node, n)).unwrap_or_default(),
                axis: t.i64("axis").unwrap_or(0).max(0) as usize,
                kind: t.i64("trace_type").unwrap_or(1),
                color: t
                    .color("color")
                    .map(|c| color(&c))
                    .unwrap_or(Color32::from_rgb(0, 0, 255)),
                width: (t.f64("line_width").unwrap_or(1.0) as f32 * z).max(0.5),
                line_style: t.i64("line_style").unwrap_or(0),
                point: t.i64("point_type").unwrap_or(0),
                point_size: t.f64("point_size").unwrap_or(10.0) as f32 * z,
                visible: t.bool("visible").unwrap_or(true),
                points,
            }
        })
        .collect()
}

/// The worst state of the plot's PVs, for the alarm border.
fn worst_state(node: &Node) -> Option<PvState> {
    let states: Vec<PvState> = node
        .traces
        .iter()
        .flat_map(|t| t.x.iter().chain(&t.y))
        .map(|p| p.state())
        .collect();
    states
        .iter()
        .find(|s| !s.connected)
        .or_else(|| states.iter().max_by_key(|s| s.severity))
        .cloned()
}

fn text_size(painter: &egui::Painter, text: &str, font: &FontId) -> Vec2 {
    painter
        .layout_no_wrap(text.to_string(), font.clone(), Color32::BLACK)
        .size()
}

/// Ticks along a horizontal scale, spaced so their labels do not overlap.
fn fit_ticks(
    painter: &egui::Painter,
    scale: &Scale,
    font: &FontId,
    gap: f32,
) -> (Vec<(f64, String)>, f64) {
    let mut spacing = text_size(painter, "0.00", font).x + gap;
    let labelled = |spacing: f32| {
        let (ticks, step) = scale.ticks(spacing);
        let labels: Vec<(f64, String)> = ticks
            .into_iter()
            .map(|v| (v, axis::label(v, step)))
            .collect();
        let widest = labels
            .iter()
            .map(|(_, l)| text_size(painter, l, font).x)
            .fold(0.0, f32::max);
        (labels, step, widest + gap)
    };
    for _ in 0..3 {
        let (labels, step, needed) = labelled(spacing);
        if needed <= spacing {
            return (labels, step);
        }
        spacing = needed;
    }
    let (labels, step, _) = labelled(spacing);
    (labels, step)
}

/// Text turned to read upwards, centred on `center`.
fn vertical_text(painter: &egui::Painter, center: Pos2, text: &str, font: FontId, c: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), font, c);
    let size = galley.size();
    let pos = Pos2::new(center.x - size.y / 2.0, center.y + size.x / 2.0);
    painter.add(epaint::TextShape::new(pos, galley, c).with_angle(-FRAC_PI_2));
}

pub fn draw(outer: &egui::Painter, node: &Node, rect: Rect, z: f32) {
    let w = &node.w;
    let painter = &outer.with_clip_rect(outer.clip_rect().intersect(rect));
    let back = w
        .color("background_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::WHITE);
    fill(painter, rect, back);
    let fg = w
        .color("foreground_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::BLACK);
    let pad = 4.0 * z;
    let tick = 5.0 * z;
    let traces = traces(node, z);

    // Title across the top.
    let mut top = rect.top() + pad;
    let title = node
        .w
        .text("title")
        .map(|t| expand(node, t))
        .unwrap_or_default();
    if !title.is_empty() {
        let f = w.font("title_font").unwrap_or(bob::Font {
            style: bob::FontStyle::Bold,
            size: 18.0,
            ..Default::default()
        });
        let f = style::font_id(&f, z);
        let h = text_size(painter, &title, &f).y;
        painter.text(
            Pos2::new(rect.center().x, top),
            Align2::CENTER_TOP,
            &title,
            f,
            fg,
        );
        top += h + pad;
    }

    // Legend along the bottom.
    let legend_font = style::font_id(&w.font("legend_font").unwrap_or_default(), z);
    let shown: Vec<&Trace> = traces.iter().filter(|t| t.visible).collect();
    let legend_h = if w.bool("show_legend").unwrap_or(true) && !shown.is_empty() {
        text_size(painter, "Xg", &legend_font).y + pad
    } else {
        0.0
    };
    let mut bottom = rect.bottom() - pad - legend_h;

    // The x axis below the plot.
    let x_cfg = axis_cfg(w.get("x_axis"), "X", node, z);
    let x_range = axis_range(
        &x_cfg,
        shown.iter().flat_map(|t| t.points.iter().map(|p| p.0)),
        false,
    );
    let x_label_h = text_size(painter, "0", &x_cfg.scale_font).y;
    if x_cfg.visible {
        bottom -= tick + x_label_h;
        if !x_cfg.title.is_empty() {
            bottom -= text_size(painter, &x_cfg.title, &x_cfg.title_font).y;
        }
    }

    // The y axes beside it, with their widths.
    let y_els: Vec<Element> = w
        .get("y_axes")
        .map(|e| {
            e.children
                .iter()
                .filter(|c| c.name == "y_axis")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let y_els = if y_els.is_empty() {
        vec![Element::default()]
    } else {
        y_els
    };
    // The top tick label is centred on the top edge, so leave room for its upper half.
    let half_label = y_els
        .iter()
        .map(|el| text_size(painter, "0", &axis_cfg(Some(el), "Y", node, z).scale_font).y)
        .fold(0.0, f32::max)
        / 2.0;
    let top = top + (half_label - pad).max(0.0);
    let ys: Vec<(AxisCfg, Scale, Vec<f64>, f64, f32)> = y_els
        .iter()
        .enumerate()
        .map(|(i, el)| {
            let cfg = axis_cfg(Some(el), "Y", node, z);
            let data = shown
                .iter()
                .filter(|t| t.axis == i)
                .flat_map(|t| t.points.iter().map(|p| p.1));
            let (lo, hi) = axis_range(&cfg, data, true);
            let scale = Scale {
                lo,
                hi,
                log: cfg.log,
                from: bottom,
                to: top,
            };
            let line_h = text_size(painter, "0", &cfg.scale_font).y;
            let (ticks, step) = scale.ticks(line_h * 2.5);
            let label_w = ticks
                .iter()
                .map(|v| text_size(painter, &axis::label(*v, step), &cfg.scale_font).x)
                .fold(0.0, f32::max);
            let mut width = tick + pad + label_w;
            if !cfg.title.is_empty() {
                width += text_size(painter, &cfg.title, &cfg.title_font).y + pad;
            }
            (cfg, scale, ticks, step, width)
        })
        .collect();
    let side = |right: bool| {
        ys.iter()
            .filter(|y| y.0.visible && y.0.on_right == right)
            .map(|y| y.4)
            .sum::<f32>()
    };
    let left = rect.left() + pad + side(false);
    let x_scale = |right: f32| Scale {
        lo: x_range.0,
        hi: x_range.1,
        log: x_cfg.log,
        from: left,
        to: right,
    };
    // Leave room for half of the last x label when there is no right axis.
    let rough = x_scale(rect.right() - pad - side(true));
    let x_end_room = fit_ticks(painter, &rough, &x_cfg.scale_font, 20.0 * z)
        .0
        .last()
        .map_or(0.0, |(_, l)| {
            text_size(painter, l, &x_cfg.scale_font).x / 2.0
        });
    let right = rect.right() - pad - side(true).max(x_end_room);
    let plot = Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom));
    if plot.width() < 4.0 || plot.height() < 4.0 {
        return;
    }
    let xs = x_scale(plot.right());
    let (x_ticks, _) = fit_ticks(painter, &xs, &x_cfg.scale_font, 20.0 * z);

    // Grid lines first, so traces go over them.
    let grid = Stroke::new(1.0, named("Grid").gamma_multiply(0.4));
    if x_cfg.grid {
        for (v, _) in &x_ticks {
            let x = xs.pos(*v);
            painter.line_segment(
                [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                grid,
            );
        }
    }
    for (cfg, scale, ticks, _, _) in &ys {
        if cfg.grid {
            for v in ticks {
                let y = scale.pos(*v);
                painter.line_segment(
                    [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                    grid,
                );
            }
        }
    }

    // Traces, clipped to the plot area.
    for t in &shown {
        let Some((_, ys_scale, ..)) = ys.get(t.axis).or(ys.first()) else {
            continue;
        };
        draw_trace(painter, t, &xs, ys_scale, plot);
    }

    // Axes over the plot edges.
    if x_cfg.visible {
        let stroke = Stroke::new(z.max(1.0), x_cfg.color);
        painter.line_segment([plot.left_bottom(), plot.right_bottom()], stroke);
        for (v, label) in &x_ticks {
            let x = xs.pos(*v);
            painter.line_segment(
                [
                    Pos2::new(x, plot.bottom()),
                    Pos2::new(x, plot.bottom() + tick),
                ],
                stroke,
            );
            painter.text(
                Pos2::new(x, plot.bottom() + tick),
                Align2::CENTER_TOP,
                label,
                x_cfg.scale_font.clone(),
                x_cfg.color,
            );
        }
        if !x_cfg.title.is_empty() {
            painter.text(
                Pos2::new(plot.center().x, plot.bottom() + tick + x_label_h),
                Align2::CENTER_TOP,
                &x_cfg.title,
                x_cfg.title_font.clone(),
                x_cfg.color,
            );
        }
    }
    let (mut left_x, mut right_x) = (plot.left(), plot.right());
    for (cfg, scale, ticks, step, width) in ys.iter().filter(|y| y.0.visible) {
        let stroke = Stroke::new(z.max(1.0), cfg.color);
        let (x, dir) = if cfg.on_right {
            (right_x, 1.0)
        } else {
            (left_x, -1.0)
        };
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            stroke,
        );
        for v in ticks {
            let y = scale.pos(*v);
            painter.line_segment([Pos2::new(x, y), Pos2::new(x + dir * tick, y)], stroke);
            painter.text(
                Pos2::new(x + dir * (tick + pad / 2.0), y),
                if cfg.on_right {
                    Align2::LEFT_CENTER
                } else {
                    Align2::RIGHT_CENTER
                },
                axis::label(*v, *step),
                cfg.scale_font.clone(),
                cfg.color,
            );
        }
        if !cfg.title.is_empty() {
            let h = text_size(painter, &cfg.title, &cfg.title_font).y;
            let cx = x + dir * (width - h / 2.0);
            vertical_text(
                painter,
                Pos2::new(cx, plot.center().y),
                &cfg.title,
                cfg.title_font.clone(),
                cfg.color,
            );
        }
        if cfg.on_right {
            right_x += width;
        } else {
            left_x -= width;
        }
    }

    if legend_h > 0.0 {
        legend(painter, &shown, rect, legend_h, pad, &legend_font, fg, z);
    }

    if let Some(s) = worst_state(node) {
        alarm_border(outer, w, &s, rect, z);
    }
}

/// Trace names with a sample of their line, centred along the bottom.
#[allow(clippy::too_many_arguments)]
fn legend(
    painter: &egui::Painter,
    shown: &[&Trace],
    rect: Rect,
    height: f32,
    pad: f32,
    font: &FontId,
    fg: Color32,
    z: f32,
) {
    let sample = 20.0 * z;
    let widths: Vec<f32> = shown
        .iter()
        .map(|t| sample + pad + text_size(painter, &t.name, font).x)
        .collect();
    let total = widths.iter().sum::<f32>() + 3.0 * pad * (widths.len() as f32 - 1.0);
    let mut x = (rect.center().x - total / 2.0).max(rect.left() + pad);
    let y = rect.bottom() - pad - height / 2.0 + pad / 2.0;
    for (t, w) in shown.iter().zip(widths) {
        painter.line_segment(
            [Pos2::new(x, y), Pos2::new(x + sample, y)],
            Stroke::new(t.width.max(2.0 * z), t.color),
        );
        painter.text(
            Pos2::new(x + sample + pad, y),
            Align2::LEFT_CENTER,
            &t.name,
            font.clone(),
            fg,
        );
        x += w + 3.0 * pad;
    }
}

fn draw_trace(outer: &egui::Painter, t: &Trace, xs: &Scale, ys: &Scale, plot: Rect) {
    let painter = &outer.with_clip_rect(outer.clip_rect().intersect(plot));
    // Markers may poke out of the plot by half their size.
    let markers =
        outer.with_clip_rect(outer.clip_rect().intersect(plot.expand(t.point_size / 2.0)));
    // Far-off points are pulled in so the maths stays finite.
    let bounds = plot.expand(plot.width().max(plot.height()) * 4.0);
    let screen = |(x, y): (f64, f64)| {
        (x.is_finite() && y.is_finite()).then(|| {
            Pos2::new(
                xs.pos(x).clamp(bounds.left(), bounds.right()),
                ys.pos(y).clamp(bounds.top(), bounds.bottom()),
            )
        })
    };
    let pts: Vec<Option<Pos2>> = t.points.iter().map(|p| screen(*p)).collect();
    let stroke = Stroke::new(t.width, t.color);

    // Runs of finite points; a NaN breaks the line.
    let runs = pts
        .split(Option::is_none)
        .filter(|r| !r.is_empty())
        .map(|r| {
            r.iter()
                .map(|p| p.unwrap_or_default())
                .collect::<Vec<Pos2>>()
        });
    match t.kind {
        1 | 4 => runs.for_each(|run| line(painter, decimate(run), stroke, t.line_style)),
        2 => runs.for_each(|run| {
            let mut steps = Vec::with_capacity(run.len() * 2);
            for (i, p) in run.iter().enumerate() {
                if i > 0 {
                    steps.push(Pos2::new(p.x, run[i - 1].y));
                }
                steps.push(*p);
            }
            line(painter, steps, stroke, t.line_style)
        }),
        5 => {
            let base = ys.pos(if ys.log {
                ys.lo
            } else {
                0.0f64.clamp(ys.lo, ys.hi)
            });
            let n = pts.iter().flatten().count().max(1) as f32;
            let half = (plot.width() / n * 0.4).max(0.5);
            for p in pts.iter().flatten() {
                let bar =
                    Rect::from_x_y_ranges(p.x - half..=p.x + half, p.y.min(base)..=p.y.max(base));
                painter.rect_filled(bar, 0.0, t.color);
            }
        }
        _ => {}
    }
    if t.point > 0 {
        for p in pts.iter().flatten() {
            marker(&markers, *p, t.point, t.point_size, t.color);
        }
    }
}

/// Thin a line with many points per pixel column to the first, lowest,
/// highest and last point of each column, which draws the same.
fn decimate(run: Vec<Pos2>) -> Vec<Pos2> {
    if run.len() < 4096 {
        return run;
    }
    let mut out = Vec::new();
    for column in run.chunk_by(|a, b| a.x.floor() == b.x.floor()) {
        let lowest = column.iter().max_by(|a, b| a.y.total_cmp(&b.y));
        let highest = column.iter().min_by(|a, b| a.y.total_cmp(&b.y));
        let (first, last) = (column.first(), column.last());
        let mut keep: Vec<Pos2> = [first, lowest, highest, last]
            .into_iter()
            .flatten()
            .copied()
            .collect();
        keep.dedup();
        out.extend(keep);
    }
    out
}

fn line(painter: &egui::Painter, pts: Vec<Pos2>, stroke: Stroke, style: i64) {
    if pts.len() < 2 {
        if let Some(p) = pts.first() {
            painter.circle_filled(*p, stroke.width, stroke.color);
        }
        return;
    }
    let w = stroke.width.max(1.0);
    let (dash, gap) = match style {
        1 => (6.0 * w, 4.0 * w),
        2 => (w, 2.0 * w),
        3 | 4 => (6.0 * w, 2.0 * w),
        _ => {
            painter.add(Shape::line(pts, stroke));
            return;
        }
    };
    painter.extend(Shape::dashed_line(&pts, stroke, dash, gap));
}

fn marker(painter: &egui::Painter, p: Pos2, kind: i64, size: f32, c: Color32) {
    let r = size / 2.0;
    match kind {
        1 => {
            painter.rect_filled(Rect::from_center_size(p, Vec2::splat(size)), 0.0, c);
        }
        2 => {
            painter.circle_filled(p, r, c);
        }
        3 => {
            painter.add(Shape::convex_polygon(
                vec![
                    p + Vec2::new(0.0, -r),
                    p + Vec2::new(r, 0.0),
                    p + Vec2::new(0.0, r),
                    p + Vec2::new(-r, 0.0),
                ],
                c,
                Stroke::NONE,
            ));
        }
        4 => {
            let s = Stroke::new((size / 6.0).max(1.0), c);
            painter.line_segment([p + Vec2::new(-r, -r), p + Vec2::new(r, r)], s);
            painter.line_segment([p + Vec2::new(-r, r), p + Vec2::new(r, -r)], s);
        }
        _ => {
            painter.add(Shape::convex_polygon(
                vec![
                    p + Vec2::new(0.0, -r),
                    p + Vec2::new(r, r),
                    p + Vec2::new(-r, r),
                ],
                c,
                Stroke::NONE,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bob::{Display, Macros};

    const PLOT: &str = r#"<display><widget type="xyplot">
      <y_axes><y_axis><title>Y</title><autoscale>false</autoscale>
        <minimum>0.0</minimum><maximum>100.0</maximum></y_axis></y_axes>
      <traces><trace><name>$(traces[0].y_pv)</name><x_pv/>
        <y_pv>loc://$(P)ts&lt;VDoubleArray&gt;(1, 2, 3)</y_pv></trace></traces>
      <x_axis><title></title><autoscale>true</autoscale></x_axis>
      <y_axes><y_axis><title>Total</title><autoscale>true</autoscale></y_axis></y_axes>
    </widget></display>"#;

    fn node(pvs: &PvManager) -> Node {
        let d = Display::parse(PLOT).unwrap();
        let s =
            crate::screen::Builder { pvs }.screen(d, &Macros::from_pairs([("P", "A:")]), None, 0);
        s.nodes.into_iter().next().unwrap()
    }

    #[test]
    fn traces_subscribe_and_name_themselves_after_their_pv() {
        let pvs = PvManager::new();
        let n = node(&pvs);
        assert_eq!(n.traces.len(), 1);
        assert!(n.traces[0].x.is_none());
        let y = n.traces[0].y.as_ref().unwrap();
        assert!(y.name().to_string().starts_with("loc://A:ts"));
        let t = traces(&n, 1.0);
        assert_eq!(t[0].name, "loc://A:ts<VDoubleArray>(1, 2, 3)");
        assert_eq!(t[0].points, vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)]);
    }

    #[test]
    fn the_last_y_axes_wins() {
        // pvi writes <y_axes> twice; Phoebus reads properties in order, so
        // the second one (its title and autoscale) is what it shows.
        let pvs = PvManager::new();
        let n = node(&pvs);
        let cfg = axis_cfg(
            n.w.get("y_axes").and_then(|e| e.child("y_axis")),
            "Y",
            &n,
            1.0,
        );
        assert_eq!(cfg.title, "Total");
        assert!(cfg.autoscale);
        let (lo, hi) = axis_range(&cfg, [5.0, 500.0].into_iter(), true);
        assert!(lo <= 5.0 && hi >= 500.0, "{lo}..{hi}");
        let x = axis_cfg(n.w.get("x_axis"), "X", &n, 1.0);
        assert_eq!(x.title, "");
        assert_eq!(
            axis_range(&x, [0.0, 1023.0].into_iter(), false),
            (0.0, 1023.0)
        );
        // A missing axis gets the Phoebus defaults.
        let d = axis_cfg(None, "X", &n, 1.0);
        assert_eq!((d.title.as_str(), d.range), ("X", (0.0, 100.0)));
    }

    #[test]
    fn decimation_keeps_each_columns_extremes() {
        let run: Vec<Pos2> = (0..10_000)
            .map(|i| Pos2::new(i as f32 / 100.0, (i % 7) as f32))
            .collect();
        let out = decimate(run);
        assert!(out.len() <= 400);
        let first: Vec<f32> = out.iter().filter(|p| p.x < 1.0).map(|p| p.y).collect();
        assert_eq!(first.iter().copied().fold(f32::MAX, f32::min), 0.0);
        assert_eq!(first.iter().copied().fold(0.0, f32::max), 6.0);
        let short = vec![Pos2::ZERO, Pos2::new(1.0, 1.0)];
        assert_eq!(decimate(short.clone()), short);
    }

    #[test]
    fn points_pair_x_with_y() {
        assert_eq!(
            points(Some(&[10.0, 20.0, 30.0]), &[1.0, 2.0]),
            vec![(10.0, 1.0), (20.0, 2.0)]
        );
        assert_eq!(points(None, &[7.0]), vec![(0.0, 7.0)]);
    }
}
