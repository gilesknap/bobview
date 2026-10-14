//! Painting and interaction for each widget type.

use std::path::PathBuf;

use bob::{Action, Macros, Widget};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use pv::{PvManager, PvState, Value};

use crate::rules::{self, Outcome};
use crate::screen::{Builder, Node, Screen, group_inset, tab_elements};
use crate::style::{self, View, color, named, severity_color};

/// Requests that widgets pass up to the application.
pub enum Request {
    Open {
        path: PathBuf,
        macros: Macros,
        target: String,
    },
    Webpage(String),
    /// Edit a value with the on-screen keypad.
    Keypad {
        pv: pv::Pv,
        text: String,
    },
    /// Show the details of a widget's PV, from its context menu.
    Details(crate::pvmenu::Details),
}

/// Per-frame drawing context.
pub struct Cx<'a> {
    pub view: View,
    pub clip: Rect,
    pub pvs: &'a PvManager,
    pub requests: &'a mut Vec<Request>,
    pub depth: u32,
    /// Text entries open the on-screen keypad instead of taking keyboard focus.
    pub keypad: bool,
}

impl Cx<'_> {
    fn sub<'b>(&'b mut self, view: View, clip: Rect) -> Cx<'b> {
        Cx {
            view,
            clip,
            pvs: self.pvs,
            requests: self.requests,
            depth: self.depth + 1,
            keypad: self.keypad,
        }
    }
}

pub fn draw_screen(ui: &mut Ui, screen: &mut Screen, cx: &mut Cx, id: Id) {
    draw_nodes(ui, &mut screen.nodes, cx, id);
}

pub fn draw_nodes(ui: &mut Ui, nodes: &mut [Node], cx: &mut Cx, id: Id) {
    for (i, node) in nodes.iter_mut().enumerate() {
        draw_node(ui, node, cx, id.with(i));
    }
}

fn update_rules(node: &mut Node) {
    if node.rules.is_empty() {
        return;
    }
    let outcomes: Vec<(String, Outcome)> = node
        .rules
        .iter()
        .map(|r| (r.prop_id.clone(), r.evaluate()))
        .collect();
    if outcomes != node.rule_outcomes {
        let orig = node.orig_props.get_or_insert_with(|| node.w.props.clone());
        let mut props = orig.clone();
        rules::apply(&mut props, &outcomes);
        node.w.props = props;
        node.rule_outcomes = outcomes;
    }
}

fn draw_node(ui: &mut Ui, node: &mut Node, cx: &mut Cx, id: Id) {
    update_rules(node);
    if node.w.bool("visible") == Some(false) {
        return;
    }
    let rect = cx.view.rect(&node.rect);
    if !rect.intersects(cx.clip) {
        return;
    }
    let painter = ui.painter().with_clip_rect(cx.clip);
    let z = cx.view.zoom;
    let state = node.state();

    let response = match node.w.kind.as_str() {
        "label" => {
            label(&painter, node, rect, z);
            None
        }
        "textupdate" => {
            text_update(&painter, node, state.as_ref(), rect, z);
            None
        }
        "textentry" => Some(text_entry(ui, node, state.as_ref(), rect, cx, id)),
        "led" => {
            led(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "multi_state_led" => {
            multi_state_led(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "byte_monitor" => {
            byte_monitor(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "action_button" => Some(action_button(ui, node, rect, cx, id)),
        "bool_button" => Some(bool_button(ui, node, state.as_ref(), rect, cx, id)),
        "choice" | "radio" => Some(choice(ui, node, state.as_ref(), rect, cx, id)),
        "combo" => Some(combo(ui, node, state.as_ref(), rect, cx, id)),
        "checkbox" => Some(checkbox(ui, node, state.as_ref(), rect, cx, id)),
        "slide_button" => Some(slide_button(ui, node, state.as_ref(), rect, cx, id)),
        "rectangle" | "ellipse" | "arc" => {
            shape(&painter, &node.w, rect, z);
            None
        }
        "polyline" | "polygon" => {
            poly(&painter, &node.w, cx.view, &node.rect);
            None
        }
        "progressbar" | "tank" | "thermometer" | "linearmeter" | "meter" => {
            bar(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "scaledslider" | "scrollbar" | "spinner" => {
            Some(slider(ui, node, state.as_ref(), rect, cx, id))
        }
        "group" => {
            group(ui, node, rect, cx, id);
            None
        }
        "tabs" => {
            tabs(ui, node, rect, cx, id);
            None
        }
        "embedded" => {
            embedded(ui, node, rect, cx, id);
            None
        }
        "navtabs" => {
            navtabs(ui, node, rect, cx, id);
            None
        }
        "image" => {
            crate::image::draw(ui, node, state.as_ref(), rect, z);
            None
        }
        other => {
            let what = if node.pv_name.is_empty() {
                other.to_string()
            } else {
                format!("{other}\n{}", node.pv_name)
            };
            placeholder(&painter, rect, &what, z);
            None
        }
    };

    if let Some(state) = &state {
        alarm_border(&painter, &node.w, state, rect, z);
    }

    // Hover shows the PV name and value. Right-click (long-press on a touch
    // screen) opens a menu to copy them or show the PV's details.
    if !node.pv_name.is_empty() && cx.clip.contains_rect(rect) {
        let resp = response.unwrap_or_else(|| ui.interact(rect, id.with("hover"), Sense::click()));
        let text = match &state {
            Some(s) if s.connected => format!(
                "{}\n{} {:?}",
                node.pv_name,
                s.format(None, true),
                s.severity
            ),
            Some(s) => format!(
                "{}\n{}",
                node.pv_name,
                s.error.as_deref().unwrap_or("disconnected")
            ),
            None => node.pv_name.clone(),
        };
        resp.context_menu(|ui| crate::pvmenu::menu(ui, node, state.as_ref(), cx.requests));
        resp.on_hover_text(text);
    }
}

// --- helpers -----------------------------------------------------------------

pub(crate) fn fg(w: &Widget) -> Color32 {
    w.color("foreground_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::BLACK)
}

pub(crate) fn bg(w: &Widget, default: &str) -> Color32 {
    w.color("background_color")
        .map(|c| color(&c))
        .unwrap_or_else(|| named(default))
}

pub(crate) fn font(w: &Widget, z: f32) -> FontId {
    style::font_id(&w.font("font").unwrap_or_default(), z)
}

pub(crate) fn fill(painter: &egui::Painter, rect: Rect, c: Color32) {
    painter.rect_filled(rect, CornerRadius::ZERO, c);
}

pub(crate) fn outline(painter: &egui::Painter, rect: Rect, width: f32, c: Color32) {
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(width, c),
        StrokeKind::Inside,
    );
}

/// Draw `text` in `rect` with the widget's alignment, optionally wrapping.
pub(crate) fn text_in(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    c: Color32,
    align: Align2,
    wrap: bool,
) {
    let width = if wrap {
        rect.width().max(1.0)
    } else {
        f32::INFINITY
    };
    let galley = painter.layout(text.to_string(), font, c, width);
    let size = galley.size();
    let x = match align.x() {
        egui::Align::Min => rect.left() + 1.0,
        egui::Align::Center => rect.center().x - size.x / 2.0,
        egui::Align::Max => rect.right() - size.x - 1.0,
    };
    let y = match align.y() {
        egui::Align::Min => rect.top(),
        egui::Align::Center => rect.center().y - size.y / 2.0,
        egui::Align::Max => rect.bottom() - size.y,
    };
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .galley(Pos2::new(x, y), galley, c);
}

fn alignment(w: &Widget, v_default: i64) -> Align2 {
    style::align(
        w.i64("horizontal_alignment").unwrap_or(0),
        w.i64("vertical_alignment").unwrap_or(v_default),
    )
}

/// Button-like 3D box.
fn button_box(painter: &egui::Painter, rect: Rect, fill_color: Color32, pressed: bool, z: f32) {
    let r = CornerRadius::same((2.0 * z).min(255.0) as u8);
    painter.rect_filled(rect, r, fill_color);
    let (light, dark) = (
        Color32::from_white_alpha(160),
        Color32::from_black_alpha(90),
    );
    let (tl, br) = if pressed {
        (dark, light)
    } else {
        (light, dark)
    };
    let w = (1.0 * z).max(1.0);
    painter.line_segment([rect.left_bottom(), rect.left_top()], Stroke::new(w, tl));
    painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(w, tl));
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(w, br));
    painter.line_segment(
        [rect.right_bottom(), rect.left_bottom()],
        Stroke::new(w, br),
    );
}

pub(crate) fn placeholder(painter: &egui::Painter, rect: Rect, what: &str, z: f32) {
    fill(painter, rect, Color32::from_gray(235));
    outline(painter, rect, 1.0, Color32::from_gray(160));
    text_in(
        painter,
        rect,
        what,
        FontId::proportional(11.0 * z.max(0.6)),
        Color32::from_gray(90),
        Align2::CENTER_CENTER,
        true,
    );
}

/// Value text as Phoebus formats it for the widget's `format` and `precision`.
fn format_value(w: &Widget, s: &PvState) -> String {
    let precision = w.i64("precision").filter(|p| *p >= 0).map(|p| p as i32);
    let units = w.bool("show_units").unwrap_or(true);
    let format = w.i64("format").unwrap_or(0);
    match (format, &s.value) {
        (6, Value::LongArray(a)) => a
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| (*c as u8) as char)
            .collect(),
        (4, v) if v.as_i64().is_some() => format!("0x{:X}", v.as_i64().unwrap_or(0)),
        (10, v) if v.as_i64().is_some() => format!("0b{:b}", v.as_i64().unwrap_or(0)),
        (2, Value::Double(v)) => {
            let p = precision.or(s.precision).unwrap_or(3).max(0) as usize;
            let text = format!("{v:.p$E}");
            if units && !s.units.is_empty() {
                format!("{text} {}", s.units)
            } else {
                text
            }
        }
        _ => s.format(precision, units),
    }
}

/// The `bit` property: Phoebus defaults it to 0 for checkbox and slide_button,
/// -1 (whole value) for LEDs and bool buttons.
fn bit(w: &Widget) -> i64 {
    let default = match w.kind.as_str() {
        "checkbox" | "slide_button" => 0,
        _ => -1,
    };
    w.i64("bit").unwrap_or(default)
}

fn bit_on(v: i64, bit: i64) -> bool {
    if bit < 0 { v != 0 } else { (v >> bit) & 1 == 1 }
}

/// The value a checkbox or slide button writes when clicked: bit < 0 writes
/// 0/1, otherwise flips that bit of the current value.
fn toggled(v: i64, bit: i64) -> i64 {
    if bit < 0 {
        (v == 0) as i64
    } else {
        v ^ (1 << bit)
    }
}

fn is_on(w: &Widget, s: &PvState) -> bool {
    bit_on(s.value.as_i64().unwrap_or(0), bit(w))
}

/// Write the toggled value of a checkbox or slide button.
fn toggle(node: &Node, s: Option<&PvState>) {
    let v = s.and_then(|s| s.value.as_i64()).unwrap_or(0);
    write(node, toggled(v, bit(&node.w)).to_string());
}

fn write(node: &Node, value: impl Into<String>) {
    if let Some(pv) = &node.pv {
        pv.write(value);
    }
}

// --- static and monitor widgets --------------------------------------------

fn label(painter: &egui::Painter, node: &Node, rect: Rect, z: f32) {
    let w = &node.w;
    if w.bool("transparent") == Some(false) {
        fill(painter, rect, bg(w, "Background"));
    }
    let text = node.prop("text");
    let wrap = w.bool("wrap_words").unwrap_or(true);
    text_in(
        painter,
        rect,
        &text,
        font(w, z),
        fg(w),
        alignment(w, 0),
        wrap,
    );
}

fn text_update(painter: &egui::Painter, node: &Node, state: Option<&PvState>, rect: Rect, z: f32) {
    let w = &node.w;
    if w.bool("transparent") != Some(true) {
        fill(painter, rect, bg(w, "Read_Background"));
    }
    let text = match state {
        Some(s) if s.connected => format_value(w, s),
        _ => format!("<{}>", node.pv_name),
    };
    let wrap = w.bool("wrap_words").unwrap_or(true);
    text_in(
        painter,
        rect.shrink(1.0),
        &text,
        font(w, z),
        fg(w),
        alignment(w, 1),
        wrap,
    );
}

fn led_color(w: &Widget, s: Option<&PvState>) -> Color32 {
    match s {
        Some(s) if s.connected => {
            let (key, default) = if is_on(w, s) {
                ("on_color", "On")
            } else {
                ("off_color", "Off")
            };
            w.color(key)
                .map(|c| color(&c))
                .unwrap_or_else(|| named(default))
        }
        _ => named("DISCONNECTED"),
    }
}

fn draw_led(
    painter: &egui::Painter,
    w: &Widget,
    rect: Rect,
    fill_color: Color32,
    text: &str,
    z: f32,
) {
    let line = w
        .color("line_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgba_unmultiplied(50, 50, 50, 178));
    if w.bool("square") == Some(true) {
        fill(painter, rect, fill_color);
        outline(painter, rect, z.max(1.0), line);
    } else {
        let r = rect.width().min(rect.height()) / 2.0;
        painter.circle(
            rect.center(),
            r - 0.5,
            fill_color,
            Stroke::new(z.max(1.0), line),
        );
        // A little highlight so it reads as a lamp.
        painter.circle_filled(
            rect.center() - Vec2::splat(r * 0.3),
            r * 0.25,
            Color32::from_white_alpha(90),
        );
    }
    if !text.is_empty() {
        text_in(
            painter,
            rect,
            text,
            font(w, z),
            fg(w),
            Align2::CENTER_CENTER,
            false,
        );
    }
}

fn led(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let text = match s {
        Some(s) if s.connected && is_on(w, s) => w.text("on_label"),
        Some(s) if s.connected => w.text("off_label"),
        _ => None,
    };
    draw_led(painter, w, rect, led_color(w, s), text.unwrap_or(""), z);
}

fn multi_state_led(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let (c, text) = match s {
        Some(s) if s.connected => {
            let v = s.value.as_i64().unwrap_or(i64::MIN);
            let states = w
                .get("states")
                .map(|e| e.children.clone())
                .unwrap_or_default();
            let hit = states.iter().find(|st| {
                st.child_text("value")
                    .and_then(|t| t.trim().parse::<f64>().ok())
                    == Some(v as f64)
            });
            match hit {
                Some(st) => (
                    st.child("color")
                        .and_then(|c| c.child("color"))
                        .map(|c| color(&bob::Color::from_element(c)))
                        .unwrap_or(Color32::GRAY),
                    st.child_text("label").unwrap_or("").to_string(),
                ),
                None => (
                    w.color("fallback_color")
                        .map(|c| color(&c))
                        .unwrap_or_else(|| named("INVALID")),
                    w.text("fallback_label").unwrap_or("Err").to_string(),
                ),
            }
        }
        _ => (named("DISCONNECTED"), String::new()),
    };
    draw_led(painter, w, rect, c, &text, z);
}

fn byte_monitor(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let n = w.i64("numBits").unwrap_or(8).clamp(1, 64) as usize;
    let start = w.i64("startBit").unwrap_or(0).clamp(0, 63);
    let reverse = w.bool("bitReverse").unwrap_or(false);
    let horizontal = w.bool("horizontal").unwrap_or(true);
    let on = w
        .color("on_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(60, 255, 60));
    let off = w
        .color("off_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(60, 100, 60));
    let value = s.filter(|s| s.connected).and_then(|s| s.value.as_i64());
    let step = if horizontal {
        rect.width() / n as f32
    } else {
        rect.height() / n as f32
    };
    for i in 0..n {
        // Phoebus draws the highest bit first unless bitReverse is set.
        let bit = if reverse {
            start + i as i64
        } else {
            start + (n - 1 - i) as i64
        };
        let cell = if horizontal {
            Rect::from_min_size(
                rect.min + Vec2::new(step * i as f32, 0.0),
                Vec2::new(step, rect.height()),
            )
        } else {
            Rect::from_min_size(
                rect.min + Vec2::new(0.0, step * i as f32),
                Vec2::new(rect.width(), step),
            )
        };
        let c = match value {
            Some(v) if (v >> bit) & 1 == 1 => on,
            Some(_) => off,
            None => named("DISCONNECTED"),
        };
        draw_led(painter, w, cell.shrink(0.5 * z), c, "", z);
    }
}

fn shape(painter: &egui::Painter, w: &Widget, rect: Rect, z: f32) {
    let transparent = w.bool("transparent").unwrap_or(false);
    let fill_c = if transparent {
        Color32::TRANSPARENT
    } else {
        w.color("background_color")
            .map(|c| color(&c))
            .unwrap_or(Color32::from_rgb(30, 144, 255))
    };
    let lw = w.f64("line_width").unwrap_or(3.0) as f32 * z;
    let line = w
        .color("line_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(0, 0, 255));
    let stroke = if lw > 0.0 {
        Stroke::new(lw, line)
    } else {
        Stroke::NONE
    };
    if w.kind == "rectangle" {
        let cr = w
            .f64("corner_width")
            .unwrap_or(0.0)
            .max(w.f64("corner_height").unwrap_or(0.0)) as f32
            * z
            / 2.0;
        let r = CornerRadius::same(cr.min(255.0) as u8);
        painter.rect(rect, r, fill_c, stroke, StrokeKind::Inside);
    } else {
        let inner = rect.shrink(lw / 2.0);
        painter.add(Shape::ellipse_filled(
            inner.center(),
            inner.size() / 2.0,
            fill_c,
        ));
        painter.add(Shape::ellipse_stroke(
            inner.center(),
            inner.size() / 2.0,
            stroke,
        ));
    }
}

fn poly(painter: &egui::Painter, w: &Widget, view: View, r: &[f64; 4]) {
    let points: Vec<Pos2> = w
        .points()
        .iter()
        .map(|(x, y)| view.pos(r[0] + x, r[1] + y))
        .collect();
    if points.len() < 2 {
        return;
    }
    let lw = w.f64("line_width").unwrap_or(3.0) as f32 * view.zoom;
    let line = w
        .color("line_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(0, 0, 255));
    if w.kind == "polygon" {
        let fill_c = w
            .color("background_color")
            .map(|c| color(&c))
            .unwrap_or(Color32::from_rgb(50, 50, 255));
        painter.add(Shape::convex_polygon(points.clone(), fill_c, Stroke::NONE));
        let mut closed = points;
        closed.push(closed[0]);
        painter.add(Shape::line(closed, Stroke::new(lw, line)));
    } else {
        painter.add(Shape::line(points, Stroke::new(lw, line)));
    }
}

fn bar(painter: &egui::Painter, w: &Widget, s: Option<&PvState>, rect: Rect, z: f32) {
    let (min, max) = s
        .filter(|_| w.bool("limits_from_pv").unwrap_or(true))
        .and_then(|s| s.range())
        .unwrap_or((
            w.f64("minimum").unwrap_or(0.0),
            w.f64("maximum").unwrap_or(100.0),
        ));
    let vertical = match w.kind.as_str() {
        "tank" | "thermometer" => true,
        "progressbar" => w.bool("horizontal") == Some(false),
        _ => false,
    };
    let fill_c = w
        .color("fill_color")
        .map(|c| color(&c))
        .unwrap_or(match w.kind.as_str() {
            "tank" => Color32::from_rgb(0, 0, 255),
            "thermometer" => Color32::from_rgb(255, 0, 0),
            _ => Color32::from_rgb(60, 255, 60),
        });
    fill(painter, rect, bg(w, "Read_Background"));
    outline(painter, rect, 1.0, Color32::from_gray(150));
    let Some(s) = s.filter(|s| s.connected) else {
        return;
    };
    let v = s.value.as_f64().unwrap_or(min);
    let frac = if max > min {
        ((v - min) / (max - min)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let inner = rect.shrink(z.max(1.0));
    let filled = if vertical {
        Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - inner.height() * frac),
            inner.max,
        )
    } else {
        Rect::from_min_max(
            inner.min,
            Pos2::new(inner.left() + inner.width() * frac, inner.bottom()),
        )
    };
    fill(painter, filled, fill_c);
    if matches!(w.kind.as_str(), "linearmeter" | "meter") {
        let text = s.format(None, true);
        text_in(
            painter,
            rect,
            &text,
            FontId::proportional(12.0 * z),
            Color32::BLACK,
            Align2::CENTER_CENTER,
            false,
        );
    }
}

// --- controls ----------------------------------------------------------------

fn text_entry(
    ui: &mut Ui,
    node: &mut Node,
    state: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let connected = state.is_some_and(|s| s.connected);
    let shown = match state {
        Some(s) if s.connected => format_value(&w, s),
        _ => format!("<{}>", node.pv_name),
    };
    let align = alignment(&w, 1);
    let enabled = connected && w.bool("enabled") != Some(false);
    if cx.keypad {
        let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
        let painter = ui.painter().with_clip_rect(cx.clip);
        fill(&painter, rect, bg(&w, "Write_Background"));
        text_in(
            &painter,
            rect.shrink2(Vec2::new(2.0, 0.0)),
            &shown,
            font(&w, cx.view.zoom),
            fg(&w),
            align,
            false,
        );
        if resp.clicked()
            && enabled
            && let Some(pv) = &node.pv
        {
            let text = state.map(|s| {
                s.format(
                    w.i64("precision").filter(|p| *p >= 0).map(|p| p as i32),
                    false,
                )
            });
            cx.requests.push(Request::Keypad {
                pv: pv.clone(),
                text: text.unwrap_or_default(),
            });
        }
        return resp;
    }
    let mut buf = node.edit.clone().unwrap_or(shown);
    let edit = egui::TextEdit::singleline(&mut buf)
        .id(id)
        .font(font(&w, cx.view.zoom))
        .text_color(fg(&w))
        .background_color(bg(&w, "Write_Background"))
        .horizontal_align(align.x())
        .vertical_align(egui::Align::Center)
        .margin(Vec2::new(2.0, 0.0))
        .interactive(enabled)
        .min_size(rect.size());
    let resp = put_clipped(ui, rect, cx.clip, edit);
    if resp.has_focus() {
        node.edit = Some(buf);
    } else if resp.lost_focus() {
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            write(node, buf.trim().to_string());
        }
        node.edit = None;
    }
    resp
}

fn put_clipped(ui: &mut Ui, rect: Rect, clip: Rect, widget: impl egui::Widget) -> Response {
    ui.scope(|ui| {
        ui.set_clip_rect(clip.intersect(ui.clip_rect()));
        ui.put(rect, widget)
    })
    .inner
}

fn action_button(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) -> Response {
    let w = &node.w;
    let actions = w.actions();
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    button_box(
        ui.painter(),
        rect,
        bg(w, "Button_Background"),
        resp.is_pointer_button_down_on(),
        cx.view.zoom,
    );
    let mut text = node.prop("text");
    if text.is_empty() || text.contains("$(actions)") {
        text = actions
            .first()
            .map(|a| node.expand(a.description()))
            .unwrap_or_default();
    }
    let painter = ui.painter().with_clip_rect(cx.clip);
    text_in(
        &painter,
        rect,
        &text,
        font(w, cx.view.zoom),
        fg(w),
        Align2::CENTER_CENTER,
        true,
    );

    if actions.len() == 1 && resp.clicked() {
        run_action(node, &actions[0], cx);
    } else if actions.len() > 1 {
        egui::Popup::menu(&resp).show(|ui| {
            for a in &actions {
                if ui.button(node.expand(a.description())).clicked() {
                    run_action(node, a, cx);
                }
            }
        });
    }
    resp
}

fn run_action(node: &Node, action: &Action, cx: &mut Cx) {
    match action {
        Action::OpenDisplay {
            file,
            macros,
            target,
            ..
        } => {
            let file = node.expand(file);
            let expanded =
                Macros::from_pairs(macros.iter().map(|(k, v)| (k.to_string(), node.expand(v))));
            cx.requests.push(Request::Open {
                path: node.resolve(&file),
                macros: node.macros.with(&expanded),
                target: target.clone(),
            });
        }
        Action::WritePv { pv_name, value, .. } => {
            let name = node.expand(pv_name);
            let value = node.expand(value);
            match node
                .action_pvs
                .iter()
                .find(|p| p.name().to_string() == pv::PvName::parse(&name).to_string())
            {
                Some(pv) => pv.write(value),
                None => cx.pvs.subscribe(&name).write(value),
            }
        }
        Action::OpenWebpage { url, .. } => cx.requests.push(Request::Webpage(node.expand(url))),
        Action::Unsupported { kind, .. } => log::warn!("action '{kind}' is not supported"),
    }
}

fn bool_button(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let on = s.is_some_and(|s| s.connected && is_on(&w, s));
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    button_box(
        ui.painter(),
        rect,
        bg(&w, "Button_Background"),
        resp.is_pointer_button_down_on() || on,
        z,
    );
    let painter = ui.painter().with_clip_rect(cx.clip);
    let label = if on {
        w.text("on_label").unwrap_or("On")
    } else {
        w.text("off_label").unwrap_or("Off")
    };
    let mut text_rect = rect;
    if w.bool("show_led").unwrap_or(true) {
        let d = (rect.height() * 0.5).min(rect.width() * 0.3);
        let led_rect =
            Rect::from_center_size(Pos2::new(rect.left() + d, rect.center().y), Vec2::splat(d));
        draw_led(&painter, &w, led_rect, led_color(&w, s), "", z);
        text_rect.min.x += d * 1.6;
    }
    text_in(
        &painter,
        text_rect,
        label,
        font(&w, z),
        fg(&w),
        Align2::CENTER_CENTER,
        false,
    );
    // mode 0 toggles; 1 and 2 push (2 inverted)
    match w.i64("mode").unwrap_or(0) {
        0 if resp.clicked() => write(node, if on { "0" } else { "1" }),
        m @ (1 | 2) => {
            let (press, release) = if m == 1 { ("1", "0") } else { ("0", "1") };
            if resp.drag_started() || (resp.clicked() && !resp.dragged()) {
                write(node, press);
            }
            if resp.drag_stopped() || resp.clicked() {
                write(node, release);
            }
        }
        _ => {}
    }
    resp
}

/// The choices for a choice/radio/combo widget and the index of the current one.
fn choices(w: &Widget, s: Option<&PvState>) -> (Vec<String>, Option<usize>) {
    let from_pv = w.bool("items_from_pv").unwrap_or(true);
    let items = match s {
        Some(s) if from_pv && !s.labels.is_empty() => s.labels.clone(),
        _ => w.items(),
    };
    let current = s.filter(|s| s.connected).and_then(|s| match &s.value {
        Value::Enum(i) => Some(*i as usize),
        Value::Str(t) => items.iter().position(|i| i == t),
        v => v.as_i64().map(|i| i as usize),
    });
    (items, current)
}

fn choice(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let (items, current) = choices(&w, s);
    let horizontal = w.bool("horizontal").unwrap_or(true);
    let n = items.len().max(1);
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::hover());
    let painter = ui.painter().with_clip_rect(cx.clip);
    let selected = w
        .color("selected_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_gray(200));
    let (dx, dy) = if horizontal {
        (rect.width() / n as f32, 0.0)
    } else {
        (0.0, rect.height() / n as f32)
    };
    let size = if horizontal {
        Vec2::new(dx, rect.height())
    } else {
        Vec2::new(rect.width(), dy)
    };
    for (i, item) in items.iter().enumerate() {
        let cell =
            Rect::from_min_size(rect.min + Vec2::new(dx * i as f32, dy * i as f32), size).shrink(z);
        let r = ui.interact(cell.intersect(cx.clip), id.with(i), Sense::click());
        let is_sel = current == Some(i);
        if w.kind == "radio" {
            let d = cell.height().min(14.0 * z);
            let c = Pos2::new(cell.left() + d / 2.0 + 2.0, cell.center().y);
            painter.circle(
                c,
                d / 2.0,
                Color32::WHITE,
                Stroke::new(1.0, Color32::DARK_GRAY),
            );
            if is_sel {
                painter.circle_filled(c, d / 4.0, Color32::BLACK);
            }
            let tr = Rect::from_min_max(Pos2::new(cell.left() + d + 6.0, cell.top()), cell.max);
            text_in(
                &painter,
                tr,
                item,
                font(&w, z),
                fg(&w),
                Align2::LEFT_CENTER,
                false,
            );
        } else {
            let fill_c = if is_sel {
                selected
            } else {
                bg(&w, "Button_Background")
            };
            button_box(
                &painter,
                cell,
                fill_c,
                is_sel || r.is_pointer_button_down_on(),
                z,
            );
            text_in(
                &painter,
                cell,
                item,
                font(&w, z),
                fg(&w),
                Align2::CENTER_CENTER,
                false,
            );
        }
        if r.clicked() {
            write(node, item.clone());
        }
    }
    resp
}

fn combo(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let (items, current) = choices(&w, s);
    let text = match (current.and_then(|i| items.get(i)), s) {
        (Some(t), _) => t.clone(),
        (None, Some(s)) if s.connected => s.format(None, false),
        _ => format!("<{}>", node.pv_name),
    };
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    let z = cx.view.zoom;
    let painter = ui.painter().with_clip_rect(cx.clip);
    button_box(&painter, rect, bg(&w, "Button_Background"), false, z);
    let arrow = rect.height().min(rect.width() / 4.0);
    let text_rect = Rect::from_min_max(
        rect.min + Vec2::new(4.0, 0.0),
        Pos2::new(rect.right() - arrow, rect.bottom()),
    );
    text_in(
        &painter,
        text_rect,
        &text,
        font(&w, z),
        fg(&w),
        Align2::LEFT_CENTER,
        false,
    );
    let c = Pos2::new(rect.right() - arrow / 2.0, rect.center().y);
    let a = arrow * 0.2;
    painter.add(Shape::convex_polygon(
        vec![
            c + Vec2::new(-a, -a / 2.0),
            c + Vec2::new(a, -a / 2.0),
            c + Vec2::new(0.0, a),
        ],
        fg(&w),
        Stroke::NONE,
    ));
    egui::Popup::menu(&resp).show(|ui| {
        for item in &items {
            if ui
                .selectable_label(Some(item) == current.and_then(|i| items.get(i)), item)
                .clicked()
            {
                write(node, item.clone());
            }
        }
    });
    resp
}

fn checkbox(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let on = s.is_some_and(|s| s.connected && is_on(&w, s));
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    let painter = ui.painter().with_clip_rect(cx.clip);
    let d = (14.0 * z).min(rect.height());
    let b = Rect::from_min_size(
        Pos2::new(rect.left() + 1.0, rect.center().y - d / 2.0),
        Vec2::splat(d),
    );
    painter.rect(
        b,
        CornerRadius::same(2),
        Color32::WHITE,
        Stroke::new(1.0, Color32::DARK_GRAY),
        StrokeKind::Inside,
    );
    if on {
        painter.line(
            vec![
                b.left_center() + Vec2::new(d * 0.2, 0.0),
                b.center_bottom() - Vec2::new(0.0, d * 0.2),
                b.right_top() + Vec2::new(-d * 0.2, d * 0.2),
            ],
            Stroke::new(2.0 * z, Color32::BLACK),
        );
    }
    let tr = Rect::from_min_max(Pos2::new(b.right() + 4.0, rect.top()), rect.max);
    text_in(
        &painter,
        tr,
        &node.prop("label"),
        font(&w, z),
        fg(&w),
        Align2::LEFT_CENTER,
        false,
    );
    if resp.clicked() {
        toggle(node, s);
    }
    resp
}

/// Phoebus SlideButton (a JavaFX ToggleSwitch): the label, then a pill-shaped
/// track in off_color/on_color with a round thumb that slides to the right
/// when the selected bit is set. A click toggles the bit like a checkbox.
fn slide_button(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let on = s.is_some_and(|s| s.connected && is_on(&w, s));
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    let painter = ui.painter().with_clip_rect(cx.clip);
    let (track, label_rect) = slide_layout(rect, z, !node.prop("label").is_empty());
    let track_color = if on {
        w.color("on_color")
            .map(|c| color(&c))
            .unwrap_or(Color32::from_rgb(60, 255, 60))
    } else {
        w.color("off_color")
            .map(|c| color(&c))
            .unwrap_or(Color32::from_rgb(60, 100, 60))
    };
    let r = CornerRadius::same((track.height() / 2.0).min(255.0) as u8);
    painter.rect(
        track,
        r,
        track_color,
        Stroke::new(1.0, Color32::from_gray(150)),
        StrokeKind::Inside,
    );
    let d = track.height() - 2.0 * (z.max(1.0));
    let cx_thumb = if on {
        track.right() - track.height() / 2.0
    } else {
        track.left() + track.height() / 2.0
    };
    let thumb = Pos2::new(cx_thumb, track.center().y);
    painter.circle(
        thumb,
        d / 2.0,
        Color32::WHITE,
        Stroke::new(1.0, Color32::from_gray(140)),
    );
    if let Some(lr) = label_rect {
        text_in(
            &painter,
            lr,
            &node.prop("label"),
            font(&w, z),
            fg(&w),
            Align2::LEFT_CENTER,
            false,
        );
    }
    if resp.clicked() {
        toggle(node, s);
    }
    resp
}

/// Track rect and optional label rect of a slide button. The track is a
/// pill 1.8 times as wide as it is tall, at most 16px tall at zoom 1,
/// fitted into the widget; with a label, the label is left and the track
/// right, otherwise the track is centred.
fn slide_layout(rect: Rect, z: f32, has_label: bool) -> (Rect, Option<Rect>) {
    let h = (16.0 * z)
        .min(rect.height() - 2.0)
        .min((rect.width() - 2.0) / 1.8)
        .max(2.0);
    let tw = h * 1.8;
    if has_label {
        let track = Rect::from_min_size(
            Pos2::new(rect.right() - 1.0 - tw, rect.center().y - h / 2.0),
            Vec2::new(tw, h),
        );
        let label = Rect::from_min_max(rect.min, Pos2::new(track.left() - 4.0 * z, rect.bottom()));
        (track, Some(label))
    } else {
        (
            Rect::from_center_size(rect.center(), Vec2::new(tw, h)),
            None,
        )
    }
}

fn slider(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    let w = node.w.clone();
    let (min, max) = s
        .filter(|_| w.bool("limits_from_pv").unwrap_or(true))
        .and_then(|s| s.range())
        .unwrap_or((
            w.f64("minimum").unwrap_or(0.0),
            w.f64("maximum").unwrap_or(100.0),
        ));
    let mut v = s.and_then(|s| s.value.as_f64()).unwrap_or(min);
    let resp = ui
        .scope(|ui| {
            ui.set_clip_rect(cx.clip.intersect(ui.clip_rect()));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(id), |ui| {
                ui.spacing_mut().slider_width = (rect.width() - 60.0 * cx.view.zoom).max(20.0);
                if w.kind == "spinner" {
                    let step = w.f64("increment").unwrap_or(1.0);
                    ui.add_sized(
                        rect.size(),
                        egui::DragValue::new(&mut v).range(min..=max).speed(step),
                    )
                } else {
                    ui.add(egui::Slider::new(&mut v, min..=max))
                }
            })
            .inner
        })
        .inner;
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        write(node, v.to_string());
    }
    resp
}

// --- containers ----------------------------------------------------------------

fn group(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let painter = ui.painter().with_clip_rect(cx.clip);
    let style_n = w.i64("style").unwrap_or(0);
    if w.bool("transparent") != Some(true) {
        fill(&painter, rect, bg(&w, "Background"));
    }
    let line = w
        .color("line_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::BLACK);
    let (_, inset) = group_inset(&w);
    let inset = inset as f32 * z;
    let fnt = font(&w, z);
    match style_n {
        0 => {
            let border = rect.shrink(inset / 2.0);
            outline(&painter, border, 1.0, line);
            let galley = painter.layout_no_wrap(node.prop("name"), fnt, fg(&w));
            let at = Pos2::new(rect.left() + inset, rect.top());
            let bg_rect = Rect::from_min_size(at, galley.size()).expand2(Vec2::new(2.0, 0.0));
            let back = bg(&w, "Background");
            fill(&painter, bg_rect, back);
            painter.galley(at, galley, fg(&w));
        }
        1 => {
            let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), inset));
            fill(&painter, bar, line);
            outline(&painter, rect, 1.0, line);
            let tr = bar.shrink2(Vec2::new(3.0, 0.0));
            text_in(
                &painter,
                tr,
                &node.prop("name"),
                fnt,
                bg(&w, "Background"),
                Align2::LEFT_CENTER,
                false,
            );
        }
        2 => outline(&painter, rect, 1.0, line),
        _ => {}
    }
    let clip = cx.clip.intersect(rect);
    let view = cx.view;
    draw_nodes(ui, &mut node.children, &mut cx.sub(view, clip), id);
}

fn tabs(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let names: Vec<String> = tab_elements(&w)
        .iter()
        .map(|t| node.expand(t.child_text("name").unwrap_or("")))
        .collect();
    let tab_h = w.f64("tab_height").unwrap_or(30.0) as f32 * z;
    let painter = ui.painter().with_clip_rect(cx.clip);
    let body = Rect::from_min_max(rect.min + Vec2::new(0.0, tab_h), rect.max);
    fill(&painter, body, bg(&w, "Background"));
    outline(&painter, body, 1.0, Color32::GRAY);
    let mut x = rect.left();
    for (i, name) in names.iter().enumerate() {
        let tw = painter
            .layout_no_wrap(name.clone(), font(&w, z), Color32::BLACK)
            .size()
            .x
            + 20.0 * z;
        let tab = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(tw, tab_h));
        let r = ui.interact(tab.intersect(cx.clip), id.with(("tab", i)), Sense::click());
        let active = i == node.active_tab;
        button_box(
            &painter,
            tab,
            if active {
                bg(&w, "Background")
            } else {
                named("Button_Background")
            },
            active,
            z,
        );
        text_in(
            &painter,
            tab,
            name,
            font(&w, z),
            fg(&w),
            Align2::CENTER_CENTER,
            false,
        );
        if r.clicked() {
            node.active_tab = i;
        }
        x += tw;
    }
    let active = node.active_tab;
    if let Some(children) = node.tabs.get_mut(active) {
        let view = cx.view;
        draw_nodes(
            ui,
            children,
            &mut cx.sub(view, cx.clip.intersect(body)),
            id.with(active),
        );
    }
}

/// Draw an embedded screen into `area`, scaled per the widget's resize mode.
fn draw_embedded(ui: &mut Ui, w: &Widget, screen: &mut Screen, area: Rect, cx: &mut Cx, id: Id) {
    let z = cx.view.zoom;
    let scale = match w.i64("resize").unwrap_or(0) {
        1 | 4 if screen.width > 0.0 && screen.height > 0.0 => (area.width()
            / (screen.width as f32 * z))
            .min(area.height() / (screen.height as f32 * z)),
        _ => 1.0,
    };
    if w.bool("transparent") != Some(true) {
        fill(
            &ui.painter().with_clip_rect(cx.clip),
            area,
            color(&screen.display.background),
        );
    }
    let view = View {
        origin: area.min,
        zoom: z * scale,
    };
    let clip = cx.clip.intersect(area);
    draw_screen(ui, screen, &mut cx.sub(view, clip), id);
}

fn embedded(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) {
    let w = node.w.clone();
    match node.embedded.as_mut() {
        Some(screen) => draw_embedded(ui, &w, screen, rect, cx, id),
        None => {
            let painter = ui.painter().with_clip_rect(cx.clip);
            let msg = node.error.clone().unwrap_or_else(|| node.prop("file"));
            if !msg.is_empty() {
                placeholder(&painter, rect, &msg, cx.view.zoom);
            }
        }
    }
}

fn navtabs(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) {
    let w = node.w.clone();
    let z = cx.view.zoom;
    let names: Vec<String> = tab_elements(&w)
        .iter()
        .map(|t| node.expand(t.child_text("name").unwrap_or("")))
        .collect();
    let vertical = w.i64("direction").unwrap_or(1) == 1;
    let tw = w.f64("tab_width").unwrap_or(100.0) as f32 * z;
    let th = w.f64("tab_height").unwrap_or(30.0) as f32 * z;
    let gap = w.f64("tab_spacing").unwrap_or(2.0) as f32 * z;
    let sel = w
        .color("selected_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(236, 236, 236));
    let desel = w
        .color("deselected_color")
        .map(|c| color(&c))
        .unwrap_or(Color32::from_rgb(200, 200, 200));
    let painter = ui.painter().with_clip_rect(cx.clip);
    let mut clicked = None;
    for (i, name) in names.iter().enumerate() {
        let off = i as f32 * if vertical { th + gap } else { tw + gap };
        let pos = if vertical {
            Vec2::new(0.0, off)
        } else {
            Vec2::new(off, 0.0)
        };
        let tab = Rect::from_min_size(rect.min + pos, Vec2::new(tw, th));
        let r = ui.interact(tab.intersect(cx.clip), id.with(("nav", i)), Sense::click());
        let active = i == node.active_tab;
        button_box(&painter, tab, if active { sel } else { desel }, active, z);
        text_in(
            &painter,
            tab,
            name,
            font(&w, z),
            fg(&w),
            Align2::CENTER_CENTER,
            true,
        );
        if r.clicked() && !active {
            clicked = Some(i);
        }
    }
    if let Some(i) = clicked {
        Builder { pvs: cx.pvs }.select_navtab(node, i, cx.depth);
    }
    let body = if vertical {
        Rect::from_min_max(rect.min + Vec2::new(tw + gap, 0.0), rect.max)
    } else {
        Rect::from_min_max(rect.min + Vec2::new(0.0, th + gap), rect.max)
    };
    match node.embedded.as_mut() {
        Some(screen) => draw_embedded(ui, &w, screen, body, cx, id.with("body")),
        None => {
            if let Some(e) = &node.error {
                placeholder(&painter, body, e, z);
            }
        }
    }
}

fn alarm_border(painter: &egui::Painter, w: &Widget, s: &PvState, rect: Rect, z: f32) {
    if w.bool("border_alarm_sensitive") == Some(false) {
        return;
    }
    let c = if s.connected {
        severity_color(s.severity)
    } else {
        Some(named("DISCONNECTED"))
    };
    if let Some(c) = c {
        let width = (2.0 * z).max(1.0);
        if s.connected {
            outline(painter, rect.expand(width), width, c);
        } else {
            // Dashed, as Phoebus marks disconnected PVs.
            let r = rect.expand(width / 2.0);
            let pts = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            painter.extend(Shape::dashed_line(
                &pts,
                Stroke::new(width, c),
                4.0 * z,
                3.0 * z,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_whole_value() {
        assert_eq!(toggled(0, -1), 1);
        assert_eq!(toggled(1, -1), 0);
        assert_eq!(toggled(5, -1), 0);
    }

    #[test]
    fn toggle_bit() {
        assert_eq!(toggled(0, 0), 1);
        assert_eq!(toggled(1, 0), 0);
        assert_eq!(toggled(2, 0), 3);
        assert_eq!(toggled(0, 3), 8);
        assert_eq!(toggled(9, 3), 1);
    }

    #[test]
    fn bit_selection() {
        assert!(bit_on(1, 0));
        assert!(!bit_on(2, 0));
        assert!(bit_on(2, 1));
        assert!(bit_on(2, -1));
        assert!(!bit_on(0, -1));
    }

    #[test]
    fn slide_track_fits_small_widget() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(20.0));
        let (track, label) = slide_layout(rect, 1.0, false);
        assert!(label.is_none());
        assert!(rect.contains_rect(track));
        assert!(track.width() > track.height());
        assert_eq!(track.center(), rect.center());
    }

    #[test]
    fn slide_track_right_of_label() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 30.0));
        let (track, label) = slide_layout(rect, 1.0, true);
        let label = label.unwrap();
        assert!(rect.contains_rect(track));
        assert!(label.right() < track.left());
        assert_eq!(track.height(), 16.0);
    }
}
