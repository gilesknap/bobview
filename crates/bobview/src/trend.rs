//! Live trends: PVs plotted against time, in trend windows (opened from a
//! widget's context menu or a BOY "Graph" button) and the strip chart
//! widget. Each PV keeps the samples it has had since the trend opened;
//! there is no archived history.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use pv::{Listener, Pv, PvManager, PvState};

use crate::axis::{self, Scale};
use crate::plot::{self, text_size, vertical_text};
use crate::screen::Node;
use crate::style::{self, color, named};
use crate::widgets::{alarm_border, fill};

/// What a new trend window shows: the last 10 minutes.
pub const DEFAULT_SPAN: f64 = 600.0;

/// The spans a trend window offers, in seconds.
const SPANS: &[(f64, &str)] = &[
    (60.0, "1 minute"),
    (300.0, "5 minutes"),
    (600.0, "10 minutes"),
    (1800.0, "30 minutes"),
    (3600.0, "1 hour"),
    (7200.0, "2 hours"),
    (6.0 * 3600.0, "6 hours"),
    (12.0 * 3600.0, "12 hours"),
    (86_400.0, "1 day"),
];

/// Samples kept per PV: at most this many (1.6 MB), and none older than
/// the longest span.
const MAX_SAMPLES: usize = 100_000;
const MAX_AGE: f64 = 86_400.0;

/// Line colours, in the order PVs are added.
const COLORS: &[[u8; 3]] = &[
    [0, 0, 255],
    [255, 0, 0],
    [0, 160, 0],
    [255, 128, 0],
    [160, 0, 160],
    [0, 160, 160],
    [128, 64, 0],
    [255, 0, 192],
    [96, 96, 96],
    [128, 128, 0],
];

pub fn line_color(i: usize) -> Color32 {
    let [r, g, b] = COLORS[i % COLORS.len()];
    Color32::from_rgb(r, g, b)
}

/// One PV's samples, in time order: (seconds since 1970, value), a NaN
/// value marking where it disconnected.
#[derive(Default)]
pub struct Samples {
    points: VecDeque<(f64, f64)>,
}

impl Samples {
    pub fn push(&mut self, t: f64, v: f64) {
        // One gap is enough, and a trend starts with a value.
        if v.is_nan() && self.points.back().is_none_or(|p| p.1.is_nan()) {
            return;
        }
        // The clock may step back; the samples stay in order.
        let t = self.points.back().map_or(t, |p| t.max(p.0));
        self.points.push_back((t, v));
        while self.points.len() > MAX_SAMPLES
            || self.points.front().is_some_and(|p| p.0 < t - MAX_AGE)
        {
            self.points.pop_front();
        }
    }

    /// The samples from `from` on, and the one before it, which holds at
    /// `from`.
    pub fn since(&self, from: f64) -> impl Iterator<Item = (f64, f64)> + '_ {
        let i = self.points.partition_point(|p| p.0 < from);
        self.points.range(i.saturating_sub(1)..).copied()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.points.len()
    }
}

/// The time now, in seconds since 1970.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// What a PV update adds to its trend: its value, a gap if it disconnected,
/// or nothing if it has no number (yet).
fn sample(s: &PvState) -> Option<f64> {
    if s.connected {
        s.value.as_f64()
    } else {
        Some(f64::NAN)
    }
}

/// A PV being trended: a sample on each update, at the time it arrived.
pub struct Series {
    pub pv: Pv,
    /// Locked by the listener while the PV's state is locked, so never read
    /// `pv.state()` while holding this lock.
    samples: Arc<Mutex<Samples>>,
    _listener: Listener,
}

impl Series {
    pub fn new(pv: Pv) -> Self {
        let samples = Arc::new(Mutex::new(Samples::default()));
        let into = samples.clone();
        let listener: Listener = Arc::new(move |s: &PvState| {
            if let Some(v) = sample(s) {
                into.lock().unwrap().push(now(), v);
            }
        });
        pv.listen(&listener);
        Self {
            pv,
            samples,
            _listener: listener,
        }
    }

    /// The points to draw from `from` to `to`: the samples, with the last
    /// value held to `to` while the PV is connected.
    fn points(&self, from: f64, to: f64) -> Vec<(f64, f64)> {
        // Copied out before the PV's state is read: an update holds that
        // lock while it adds a sample.
        let mut pts: Vec<(f64, f64)> = self.samples.lock().unwrap().since(from).collect();
        if let Some(&(t, v)) = pts.last()
            && v.is_finite()
            && t < to
            && self.pv.state().connected
        {
            pts.push((to, v));
        }
        pts
    }
}

/// A line to draw, and how.
pub struct Line<'a> {
    pub series: &'a Series,
    pub color: Color32,
    pub width: f32,
}

/// How the plot looks.
pub struct Look {
    pub log: bool,
    /// A fixed y range, or `None` to fit the data.
    pub range: Option<(f64, f64)>,
    pub title: String,
    pub grid: bool,
    pub fg: Color32,
    pub back: Color32,
    pub font: FontId,
    pub title_font: FontId,
}

/// Plot `lines` over the `span` seconds up to `now`, with a time axis below
/// and a value axis on the left.
pub fn draw_plot(
    painter: &egui::Painter,
    rect: Rect,
    lines: &[Line],
    span: f64,
    now: f64,
    look: &Look,
    z: f32,
) {
    let painter = &painter.with_clip_rect(painter.clip_rect().intersect(rect));
    fill(painter, rect, look.back);
    let (pad, tick) = (4.0 * z, 5.0 * z);
    let from = now - span;
    let data: Vec<Vec<(f64, f64)>> = lines.iter().map(|l| l.series.points(from, now)).collect();

    let ok = |v: f64| v.is_finite() && (!look.log || v > 0.0);
    let in_view = data
        .iter()
        .flat_map(|pts| pts.iter().filter(|p| p.0 >= from).map(|p| p.1));
    let (lo, hi) = look
        .range
        .or_else(|| plot::autoscale(look.log, in_view, true))
        .unwrap_or(if look.log { (1.0, 10.0) } else { (0.0, 1.0) });

    let label_h = text_size(painter, "0", &look.font).y;
    let bottom = rect.bottom() - pad - label_h - tick;
    let top = rect.top() + pad + label_h / 2.0;
    let ys = Scale {
        lo,
        hi,
        log: look.log,
        from: bottom,
        to: top,
    };
    let (y_ticks, y_step) = ys.ticks(label_h * 2.5);
    let y_labels: Vec<(f64, String)> = y_ticks
        .iter()
        .map(|v| (*v, axis::label(*v, y_step)))
        .collect();
    let label_w = y_labels
        .iter()
        .map(|(_, l)| text_size(painter, l, &look.font).x)
        .fold(0.0, f32::max);
    let title_w = if look.title.is_empty() {
        0.0
    } else {
        text_size(painter, &look.title, &look.title_font).y + pad
    };
    let time_w = text_size(painter, "00:00:00", &look.font).x;
    let left = rect.left() + pad + title_w + label_w + pad / 2.0 + tick;
    let right = rect.right() - pad - time_w / 2.0;
    let plot = Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom));
    if plot.width() < 4.0 || plot.height() < 4.0 {
        return;
    }
    let xs = Scale {
        lo: from,
        hi: now,
        log: false,
        from: plot.left(),
        to: plot.right(),
    };
    let max_ticks = (plot.width() / (time_w + 20.0 * z)).floor().max(2.0) as usize;
    let (t_ticks, t_step) = time_ticks(from, now, max_ticks, local_offset(now));

    if look.grid {
        let grid = Stroke::new(1.0, named("Grid").gamma_multiply(0.4));
        for t in &t_ticks {
            let x = xs.pos(*t);
            painter.line_segment(
                [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                grid,
            );
        }
        for (v, _) in &y_labels {
            let y = ys.pos(*v);
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                grid,
            );
        }
    }

    let clipped = painter.with_clip_rect(painter.clip_rect().intersect(plot));
    // Far-off points are pulled in so the maths stays finite.
    let bounds = plot.expand(plot.width().max(plot.height()) * 4.0);
    for (line, pts) in lines.iter().zip(&data) {
        let screen = |(t, v): (f64, f64)| {
            ok(v).then(|| {
                Pos2::new(
                    xs.pos(t).clamp(bounds.left(), bounds.right()),
                    ys.pos(v).clamp(bounds.top(), bounds.bottom()),
                )
            })
        };
        let pts: Vec<Option<Pos2>> = pts.iter().map(|p| screen(*p)).collect();
        // A value holds until the next sample, so the line steps; a gap
        // (disconnected, or not a number) breaks it.
        for run in pts.split(Option::is_none).filter(|r| !r.is_empty()) {
            let mut steps: Vec<Pos2> = Vec::with_capacity(run.len() * 2);
            for p in run.iter().flatten() {
                if let Some(last) = steps.last().copied() {
                    steps.push(Pos2::new(p.x, last.y));
                }
                steps.push(*p);
            }
            plot::line(
                &clipped,
                plot::decimate(steps),
                Stroke::new(line.width, line.color),
                0,
            );
        }
    }

    // The axes over the plot's edges.
    let stroke = Stroke::new(z.max(1.0), look.fg);
    painter.rect_stroke(plot, 0.0, stroke, egui::StrokeKind::Middle);
    for t in &t_ticks {
        let x = xs.pos(*t);
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
            time_label(*t, t_step),
            look.font.clone(),
            look.fg,
        );
    }
    for (v, label) in &y_labels {
        let y = ys.pos(*v);
        painter.line_segment(
            [Pos2::new(plot.left() - tick, y), Pos2::new(plot.left(), y)],
            stroke,
        );
        painter.text(
            Pos2::new(plot.left() - tick - pad / 2.0, y),
            Align2::RIGHT_CENTER,
            label,
            look.font.clone(),
            look.fg,
        );
    }
    if !look.title.is_empty() {
        let h = title_w - pad;
        vertical_text(
            painter,
            Pos2::new(rect.left() + pad + h / 2.0, plot.center().y),
            &look.title,
            look.title_font.clone(),
            look.fg,
        );
    }
}

/// Steps for the time axis, in seconds: whole seconds, minutes and hours.
const TIME_STEPS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
    10_800.0, 21_600.0, 43_200.0, 86_400.0,
];

/// Ticks from `from` to `to` (seconds since 1970) at the smallest step
/// giving at most `max_ticks`, on whole steps of local time (`offset`
/// seconds ahead of UTC). Returns the ticks and the step.
fn time_ticks(from: f64, to: f64, max_ticks: usize, offset: f64) -> (Vec<f64>, f64) {
    let span = to - from;
    if span <= 0.0 || !span.is_finite() {
        return (Vec::new(), 0.0);
    }
    let step = TIME_STEPS
        .iter()
        .copied()
        .find(|s| span / s <= max_ticks.max(1) as f64)
        .unwrap_or_else(|| 86_400.0 * (span / 86_400.0 / max_ticks.max(1) as f64).ceil());
    let first = ((from + offset) / step).ceil() * step - offset;
    let ticks = (0..)
        .map(|i| first + i as f64 * step)
        .take_while(|t| *t <= to)
        .take(1000)
        .collect();
    (ticks, step)
}

/// How far local time is ahead of UTC at `t`, in seconds.
fn local_offset(t: f64) -> f64 {
    Local
        .timestamp_opt(t as i64, 0)
        .single()
        .map_or(0.0, |d| d.offset().local_minus_utc() as f64)
}

/// A time axis label in local time, as precise as the step.
fn time_label(t: f64, step: f64) -> String {
    let Some(d) = Local.timestamp_opt(t.round() as i64, 0).single() else {
        return String::new();
    };
    let format = match step {
        s if s < 60.0 => "%H:%M:%S",
        s if s < 86_400.0 => "%H:%M",
        _ => "%d %b",
    };
    d.format(format).to_string()
}

// --- trend windows -----------------------------------------------------------

/// A trend window's PVs and settings.
pub struct Trend {
    /// Tells this window from the others.
    pub id: u64,
    series: Vec<Series>,
    span: f64,
    log: bool,
    /// A PV name being typed into "Add".
    adding: String,
}

impl Trend {
    /// A trend of the PVs `names`, each once.
    pub fn new(id: u64, names: &[String], pvs: &PvManager) -> Self {
        let mut t = Self {
            id,
            series: Vec::new(),
            span: DEFAULT_SPAN,
            log: false,
            adding: String::new(),
        };
        for name in names {
            t.add(name, pvs);
        }
        t
    }

    fn add(&mut self, name: &str, pvs: &PvManager) {
        let name = name.trim();
        let key = pv::PvName::parse(name).key();
        if !name.is_empty() && !self.series.iter().any(|s| s.pv.name().key() == key) {
            self.series.push(Series::new(pvs.subscribe(name)));
        }
    }

    /// The window title: its PVs, or the first and how many more.
    pub fn title(&self) -> String {
        let names: Vec<String> = self
            .series
            .iter()
            .map(|s| crate::app::plain_name(s.pv.name()))
            .collect();
        match names.as_slice() {
            [] => "Trend".into(),
            [one] => format!("Trend: {one}"),
            [a, b] => format!("Trend: {a}, {b}"),
            [first, rest @ ..] => format!("Trend: {first} and {} more", rest.len()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, pvs: &PvManager) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("trend-span", self.id))
                .selected_text(span_name(self.span))
                .show_ui(ui, |ui| {
                    for (secs, name) in SPANS {
                        ui.selectable_value(&mut self.span, *secs, *name);
                    }
                });
            ui.checkbox(&mut self.log, "Log scale");
            ui.separator();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.adding)
                    .hint_text("Add PV")
                    .desired_width(160.0),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let name = std::mem::take(&mut self.adding);
                self.add(&name, pvs);
            }
        });

        // The legend under the plot: a row per PV.
        let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
        let legend_h = row * self.series.len() as f32 + ui.spacing().item_spacing.y;
        let size = Vec2::new(
            ui.available_width(),
            (ui.available_height() - legend_h).max(80.0),
        );
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let lines: Vec<Line> = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| Line {
                series: s,
                color: line_color(i),
                width: 1.5,
            })
            .collect();
        let font = egui::TextStyle::Small.resolve(ui.style());
        let look = Look {
            log: self.log,
            range: None,
            title: common_units(&self.series),
            grid: true,
            fg: ui.visuals().text_color(),
            back: ui.visuals().extreme_bg_color,
            title_font: font.clone(),
            font,
        };
        draw_plot(ui.painter(), rect, &lines, self.span, now(), &look, 1.0);

        let mut remove = None;
        egui::Grid::new(("trend-legend", self.id))
            .num_columns(4)
            .show(ui, |ui| {
                for (i, s) in self.series.iter().enumerate() {
                    let state = s.pv.state();
                    let (swatch, _) =
                        ui.allocate_exact_size(Vec2::new(24.0, 12.0), egui::Sense::hover());
                    ui.painter().line_segment(
                        [swatch.left_center(), swatch.right_center()],
                        Stroke::new(3.0, line_color(i)),
                    );
                    ui.label(crate::app::plain_name(s.pv.name()));
                    if state.connected {
                        let text = egui::RichText::new(state.format(None, true));
                        ui.label(match style::severity_color(state.severity) {
                            Some(c) => text.color(c),
                            None => text,
                        });
                    } else {
                        ui.label(egui::RichText::new("Disconnected").color(Color32::GRAY));
                    }
                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            self.series.remove(i);
        }

        // Keep time moving: a repaint for each pixel the plot scrolls.
        let per_pixel = self.span / rect.width().max(1.0) as f64;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                per_pixel.clamp(0.1, 1.0),
            ));
    }
}

fn span_name(span: f64) -> String {
    SPANS
        .iter()
        .find(|(s, _)| *s == span)
        .map_or_else(|| format!("{span} s"), |(_, n)| n.to_string())
}

/// The PVs' units, when they all have the same.
fn common_units(series: &[Series]) -> String {
    let units: Vec<String> = series.iter().map(|s| s.pv.state().units).collect();
    match units.split_first() {
        Some((first, rest)) if rest.iter().all(|u| u == first) => first.clone(),
        _ => String::new(),
    }
}

// --- the strip chart widget --------------------------------------------------

/// A strip chart's `time_range`, such as `1 minute` or `1 hour 30 min`, in
/// seconds.
pub fn parse_span(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut words = text.split_whitespace().peekable();
    let mut any = false;
    while let Some(word) = words.next() {
        // A number and its unit, apart (`10 min`) or together (`10min`).
        let split = word
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(word.len());
        let (num, unit) = word.split_at(split);
        let n: f64 = num.parse().ok()?;
        let unit = if unit.is_empty() {
            words.next().unwrap_or("s")
        } else {
            unit
        };
        let secs = match unit.to_ascii_lowercase().trim_end_matches('s') {
            "" | "sec" | "second" => 1.0,
            "m" | "min" | "minute" => 60.0,
            "h" | "hour" => 3600.0,
            "d" | "day" => 86_400.0,
            "w" | "week" => 7.0 * 86_400.0,
            _ => return None,
        };
        total += n * secs;
        any = true;
    }
    (any && total > 0.0).then_some(total)
}

/// Draw a `.bob` strip chart: its traces' PVs over the last `time_range`,
/// on its first value axis.
pub fn draw_stripchart(painter: &egui::Painter, node: &Node, rect: Rect, z: f32) {
    let w = &node.w;
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
    let top = plot::title(painter, node, rect, fg, z);

    let traces = plot::traces(node, z);
    let shown: Vec<(&plot::Trace, &Series)> = traces
        .iter()
        .zip(&node.series)
        .filter(|(t, _)| t.visible)
        .filter_map(|(t, s)| Some((t, s.as_ref()?)))
        .collect();
    let legend_font = style::font_id(&w.font("legend_font").unwrap_or_default(), z);
    let legend_h = if w.bool("show_legend").unwrap_or(true) && !shown.is_empty() {
        text_size(painter, "Xg", &legend_font).y + pad
    } else {
        0.0
    };
    if legend_h > 0.0 {
        let named: Vec<&plot::Trace> = shown.iter().map(|(t, _)| *t).collect();
        plot::legend(painter, &named, rect, legend_h, pad, &legend_font, fg, z);
    }

    let axis = plot::axis_cfg(
        w.get("y_axes").and_then(|e| e.child("y_axis")),
        "Value",
        node,
        z,
    );
    let span = w
        .text("time_range")
        .map(|t| node.expand(t))
        .and_then(|t| parse_span(&t))
        .unwrap_or(60.0);
    let look = Look {
        log: axis.log,
        range: (!axis.autoscale).then_some(axis.range),
        title: if axis.visible {
            axis.title.clone()
        } else {
            String::new()
        },
        grid: axis.grid,
        fg: axis.color,
        back,
        font: axis.scale_font,
        title_font: axis.title_font,
    };
    let lines: Vec<Line> = shown
        .iter()
        .map(|(t, s)| Line {
            series: s,
            color: t.color,
            width: t.width,
        })
        .collect();
    let area = Rect::from_min_max(
        Pos2::new(rect.left(), top),
        Pos2::new(rect.right(), rect.bottom() - legend_h),
    );
    draw_plot(painter, area, &lines, span, now(), &look, z);
    // Keep time moving while the PVs are quiet, as `Trend::ui` does.
    let per_pixel = span / area.width().max(1.0) as f64;
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(
            per_pixel.clamp(0.1, 1.0),
        ));

    if let Some(s) = plot::worst_state(node) {
        alarm_border(painter, w, &s, rect, z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_keep_gaps_once_and_stay_in_order() {
        let mut s = Samples::default();
        s.push(1.0, f64::NAN);
        assert_eq!(s.len(), 0, "a trend starts with a value");
        s.push(1.0, 5.0);
        s.push(2.0, f64::NAN);
        s.push(3.0, f64::NAN);
        s.push(1.5, 6.0);
        let pts: Vec<(f64, f64)> = s.since(0.0).collect();
        assert_eq!(pts.len(), 3);
        assert!(pts[1].1.is_nan());
        // A clock stepping back does not reorder them.
        assert_eq!(pts[2], (2.0, 6.0));
    }

    #[test]
    fn samples_are_bounded_by_count_and_age() {
        let mut s = Samples::default();
        for i in 0..MAX_SAMPLES + 10 {
            s.push(i as f64 * 0.001, i as f64);
        }
        assert_eq!(s.len(), MAX_SAMPLES);
        assert_eq!(s.since(0.0).next().map(|p| p.1), Some(10.0));
        s.push(MAX_AGE + 200.0, 1.0);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn since_includes_the_sample_that_holds_at_the_start() {
        let mut s = Samples::default();
        for t in [10.0, 20.0, 30.0, 40.0] {
            s.push(t, t);
        }
        let ts: Vec<f64> = s.since(25.0).map(|p| p.0).collect();
        assert_eq!(ts, [20.0, 30.0, 40.0]);
        assert_eq!(s.since(50.0).count(), 1);
        assert_eq!(s.since(0.0).count(), 4);
    }

    #[test]
    fn series_sample_every_update() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://trend_test(1)");
        let series = Series::new(pv.clone());
        pv.write("2");
        pv.write("3");
        let values: Vec<f64> = series
            .points(0.0, now() + 1.0)
            .iter()
            .map(|p| p.1)
            .collect();
        // Three samples, the last held to now.
        assert_eq!(values, [1.0, 2.0, 3.0, 3.0]);
    }

    #[test]
    fn disconnected_pvs_leave_a_gap() {
        let off = PvState::default();
        assert!(sample(&off).unwrap().is_nan());
        let on = PvState {
            connected: true,
            value: pv::Value::Double(2.0),
            ..Default::default()
        };
        assert_eq!(sample(&on), Some(2.0));
        let text = PvState {
            connected: true,
            value: pv::Value::Str("idle".into()),
            ..Default::default()
        };
        assert_eq!(sample(&text), None);
    }

    #[test]
    fn time_ticks_fall_on_whole_local_steps() {
        let (t, step) = time_ticks(1000.0, 1600.0, 6, 0.0);
        assert_eq!(step, 120.0);
        assert_eq!(t, [1080.0, 1200.0, 1320.0, 1440.0, 1560.0]);
        // An hour ahead of UTC, hours are whole in local time.
        let (t, step) = time_ticks(0.0, 4.0 * 3600.0, 4, 1800.0);
        assert_eq!(step, 3600.0);
        assert_eq!(t[0], 1800.0);
        // Longer than a day per tick: whole days.
        let (_, step) = time_ticks(0.0, 30.0 * 86_400.0, 5, 0.0);
        assert_eq!(step, 6.0 * 86_400.0);
        assert!(time_ticks(5.0, 5.0, 5, 0.0).0.is_empty());
    }

    #[test]
    fn time_ranges_parse() {
        assert_eq!(parse_span("1 minute"), Some(60.0));
        assert_eq!(parse_span("10 min"), Some(600.0));
        assert_eq!(parse_span("1 hour 30 minutes"), Some(5400.0));
        assert_eq!(parse_span("30s"), Some(30.0));
        assert_eq!(parse_span("2 days"), Some(172_800.0));
        assert_eq!(parse_span("soon"), None);
        assert_eq!(parse_span(""), None);
    }

    #[test]
    fn titles_name_the_pvs() {
        let pvs = PvManager::new();
        let names = |n: &[&str]| -> Vec<String> { n.iter().map(|s| s.to_string()).collect() };
        let t = Trend::new(0, &names(&["A:B", "ca://A:B", " "]), &pvs);
        assert_eq!(t.series.len(), 1, "each PV once");
        assert_eq!(t.title(), "Trend: A:B");
        let t = Trend::new(1, &names(&["A", "B", "C"]), &pvs);
        assert_eq!(t.title(), "Trend: A and 2 more");
    }
}
