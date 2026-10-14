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
        "linearmeter" => {
            crate::meter::linear_meter(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "xyplot" => {
            crate::plot::draw(&painter, node, rect, z);
            None
        }
        "progressbar" | "tank" | "thermometer" | "meter" => {
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
        "image" => Some(crate::image::draw(
            ui,
            node,
            state.as_ref(),
            rect,
            cx.clip,
            id,
            z,
        )),
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

    // A control that is disabled or whose PV is read-only is greyed out and
    // shows a "not allowed" cursor; its writes are refused in `write`.
    let disabled = response.is_some() && is_disabled(&node.w, state.as_ref());
    if disabled {
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_white_alpha(110));
    }

    // Hover shows the tooltip: the widget's own, or the PV name and value.
    // Right-click (long-press on a touch screen) opens a menu to copy them or
    // show the PV's details.
    if !cx.clip.contains_rect(rect) {
        return;
    }
    if !node.pv_name.is_empty() {
        let resp = response.unwrap_or_else(|| ui.interact(rect, id.with("hover"), Sense::click()));
        let resp = if disabled {
            resp.on_hover_cursor(egui::CursorIcon::NotAllowed)
        } else {
            resp
        };
        // The context menu has its own popup id, so it does not fight a
        // combo or action button's own menu (which uses the default id).
        egui::Popup::context_menu(&resp)
            .id(id.with("context-menu"))
            .show(|ui| crate::pvmenu::menu(ui, node, state.as_ref(), cx.requests));
        let text = tooltip(node, state.as_ref());
        if !text.is_empty() {
            resp.on_hover_text(text);
        }
    } else if node.w.get("tooltip").is_some() && !is_container(&node.w.kind) {
        let text = tooltip(node, None);
        if !text.is_empty() {
            let resp =
                response.unwrap_or_else(|| ui.interact(rect, id.with("hover"), Sense::hover()));
            resp.on_hover_text(text);
        }
    }
}

/// Widgets that hold others; their tooltips would cover their children's.
fn is_container(kind: &str) -> bool {
    matches!(kind, "group" | "tabs" | "embedded" | "navtabs")
}

/// The hover text: the widget's `tooltip` with `$(pv_name)` and `$(pv_value)`
/// filled in, or, without one, the PV name and value as Phoebus's default
/// `$(pv_name)\n$(pv_value)` shows them (with the alarm severity).
fn tooltip(node: &Node, s: Option<&PvState>) -> String {
    let value = match s {
        Some(s) if s.connected => s.format(None, true),
        Some(s) => s.error.clone().unwrap_or_else(|| "disconnected".into()),
        None => String::new(),
    };
    match node.w.text("tooltip") {
        Some(t) => tooltip_text(t, &node.pv_name, &value, &|t| node.expand(t)),
        None => match s {
            Some(s) if s.connected => format!("{}\n{value} {:?}", node.pv_name, s.severity),
            Some(_) => format!("{}\n{value}", node.pv_name),
            None => node.pv_name.clone(),
        },
    }
}

fn tooltip_text(
    template: &str,
    pv_name: &str,
    value: &str,
    expand: &dyn Fn(&str) -> String,
) -> String {
    let t = template
        .replace("$(pv_name)", pv_name)
        .replace("$(pv_value)", value);
    expand(&t).trim().to_string()
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

/// Paint `text` in `rect` the way a JavaFX `Label` does: each row aligned on
/// its own, only whole rows that fit the height, and overflow elided with `…`
/// so the start of the text always stays visible.
pub(crate) fn text_in(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    c: Color32,
    align: Align2,
    wrap: bool,
) {
    let row_height = painter.fonts_mut(|f| f.row_height(&font));
    let job = text_job(text, font, c, rect.size(), row_height, align.x(), wrap);
    let galley = painter.layout_job(job);
    let x = match align.x() {
        egui::Align::Min => rect.left() + 1.0,
        egui::Align::Center => rect.center().x,
        egui::Align::Max => rect.right() - 1.0,
    };
    let height = galley.size().y;
    let y = match align.y() {
        egui::Align::Min => rect.top(),
        egui::Align::Center => rect.center().y - height / 2.0,
        egui::Align::Max => rect.bottom() - height,
    };
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .galley(Pos2::new(x, y), galley, c);
}

/// The layout for [`text_in`]: rows aligned on their own and limited to what
/// fits in `size`, with the overflow elided.
fn text_job(
    text: &str,
    font: FontId,
    c: Color32,
    size: Vec2,
    row_height: f32,
    halign: egui::Align,
    wrap: bool,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, c, f32::INFINITY);
    job.halign = halign;
    if !wrap && text.contains('\n') {
        // like a JavaFX Label without wrapping: keep every explicit line,
        // unwrapped, and let the widget's clip trim what doesn't fit
        return job;
    }
    let rows = max_rows(size.y, row_height, wrap);
    job.wrap = egui::text::TextWrapping {
        // the full width: the widget clip hides the 1 px the text may be
        // inset by, and any less elides text that fits at fractional zooms
        max_width: size.x.max(1.0),
        max_rows: rows,
        break_anywhere: rows == 1,
        overflow_character: Some('…'),
    };
    job
}

/// How many rows of text to lay out in a box `height` high: as many whole
/// rows as fit when wrapping (allowing a little slack for font metrics), and
/// always at least one.
fn max_rows(height: f32, row_height: f32, wrap: bool) -> usize {
    if !wrap || row_height <= 0.0 {
        return 1;
    }
    (((height + 2.0) / row_height).floor() as usize).max(1)
}

fn alignment(w: &Widget, v_default: i64) -> Align2 {
    style::align(
        w.i64("horizontal_alignment").unwrap_or(0),
        w.i64("vertical_alignment").unwrap_or(v_default),
    )
}

/// Button-like 3D box.
pub(crate) fn button_box(
    painter: &egui::Painter,
    rect: Rect,
    fill_color: Color32,
    pressed: bool,
    z: f32,
) {
    // Like a JavaFX Button: rounded, with a grey outline on all four sides so
    // it reads as a button on any background, a light top half for the
    // gradient, and a darker fill while pressed.
    let r = CornerRadius::same((3.0 * z).min(255.0) as u8);
    let w = (1.0 * z).max(1.0);
    let fill_color = if pressed {
        fill_color.lerp_to_gamma(Color32::BLACK, 0.12)
    } else {
        fill_color
    };
    painter.rect_filled(rect, r, fill_color);
    if !pressed {
        let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y)).shrink(w);
        let tr = CornerRadius {
            sw: 0,
            se: 0,
            ..CornerRadius::same(r.nw.saturating_sub(1))
        };
        painter.rect_filled(top, tr, Color32::from_white_alpha(40));
    }
    painter.rect_stroke(
        rect,
        r,
        Stroke::new(w, Color32::from_gray(150)),
        StrokeKind::Inside,
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

fn is_on(w: &Widget, s: &PvState) -> bool {
    bit_on(s.value.as_i64().unwrap_or(0), bit(w))
}

/// `value` with the widget's `bit` set to `on`, or 0/1 when `bit` is -1 (the
/// whole value), as Phoebus's checkbox, bool and slide buttons write it.
fn with_bit(bit: i64, value: i64, on: bool) -> i64 {
    if !(0..63).contains(&bit) {
        on as i64
    } else if on {
        value | (1 << bit)
    } else {
        value & !(1 << bit)
    }
}

/// The value that toggles the widget's bit (or the whole value).
fn toggled(w: &Widget, s: Option<&PvState>) -> i64 {
    let bit = bit(w);
    let on = s.is_some_and(|s| s.connected && is_on(w, s));
    let v = s.and_then(|s| s.value.as_i64()).unwrap_or(0);
    with_bit(bit, v, !on)
}

/// Phoebus disables a control whose `enabled` property is false or whose PV
/// is connected without write access.
fn is_disabled(w: &Widget, s: Option<&PvState>) -> bool {
    w.bool("enabled") == Some(false) || s.is_some_and(|s| s.connected && !s.writable)
}

/// Whether a control may write its PV: enabled, connected and writable.
fn can_write(w: &Widget, s: Option<&PvState>) -> bool {
    !is_disabled(w, s) && s.is_some_and(|s| s.connected)
}

fn write(node: &Node, value: impl Into<String>) {
    if let Some(pv) = &node.pv {
        if can_write(&node.w, Some(&pv.state())) {
            pv.write(value);
        } else {
            log::debug!("{}: not writing, the control is disabled", node.pv_name);
        }
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
    if w.kind == "meter" {
        let text = format_value(w, s);
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
    let shown = match state {
        Some(s) if s.connected => format_value(&w, s),
        _ => format!("<{}>", node.pv_name),
    };
    let align = alignment(&w, 1);
    let enabled = can_write(&w, state);
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

    // A disabled button, or one whose PV is read-only, runs no actions.
    let allowed = !is_disabled(&node.w, node.state().as_ref());
    if allowed && actions.len() == 1 && resp.clicked() {
        run_action(node, &actions[0], cx);
    } else if allowed && actions.len() > 1 {
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
    // Mode 0 toggles; 1 and 2 push (2 inverted): the pressed value is written
    // when the button goes down and the released one when it comes up.
    let bit = bit(&w);
    match w.i64("mode").unwrap_or(0) {
        0 if resp.clicked() => write(node, toggled(&w, s).to_string()),
        m @ (1 | 2) => {
            let v = s.and_then(|s| s.value.as_i64()).unwrap_or(0);
            let value = |pressed: bool| with_bit(bit, v, pressed == (m == 1)).to_string();
            let down = resp.is_pointer_button_down_on();
            let was_down = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
            if down != was_down {
                ui.data_mut(|d| d.insert_temp(id, down));
            }
            match push_edge(was_down, down, resp.clicked()) {
                Push::Press => write(node, value(true)),
                Push::Release => write(node, value(false)),
                Push::Both => {
                    write(node, value(true));
                    write(node, value(false));
                }
                Push::None => {}
            }
        }
        _ => {}
    }
    resp
}

#[derive(Debug, PartialEq)]
enum Push {
    None,
    Press,
    Release,
    /// Pressed and released within one frame.
    Both,
}

/// What a push button writes, from whether it was down last frame, is down
/// now, and was clicked this frame.
fn push_edge(was_down: bool, down: bool, clicked: bool) -> Push {
    match (was_down, down) {
        (false, true) => Push::Press,
        (true, false) => Push::Release,
        (false, false) if clicked => Push::Both,
        _ => Push::None,
    }
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
    let mut resp = ui.interact(rect.intersect(cx.clip), id, Sense::hover());
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
        // The cells take the clicks, so the PV menu needs their right-clicks.
        resp = resp.union(r);
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
    let (items, _) = choices(&w, s);
    // Like Phoebus, show the PV's value whatever the items are; the items
    // only fill the dropdown.
    let text = match s {
        Some(s) if s.connected => s.format(None, false),
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
    if !can_write(&w, s) {
        return resp;
    }
    egui::Popup::menu(&resp).show(|ui| {
        for item in &items {
            if ui.selectable_label(*item == text, item).clicked() {
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
        write(node, toggled(&w, s).to_string());
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
        write(node, toggled(&w, s).to_string());
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

pub(crate) fn alarm_border(painter: &egui::Painter, w: &Widget, s: &PvState, rect: Rect, z: f32) {
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
    use std::sync::Arc;

    #[test]
    fn rows_that_fit() {
        // A 20px textupdate with a 14px font holds a single row.
        assert_eq!(max_rows(18.0, 17.0, true), 1);
        assert_eq!(max_rows(40.0, 17.0, true), 2);
        // Never fewer than one row, even in a box shorter than the font.
        assert_eq!(max_rows(5.0, 17.0, true), 1);
        // Without wrapping it's always one row.
        assert_eq!(max_rows(400.0, 17.0, false), 1);
    }

    /// Lay out `text` in a `w`x`h` box with Liberation Sans 14, as text_in does.
    fn layout(text: &str, w: f32, h: f32, halign: egui::Align, wrap: bool) -> Arc<egui::Galley> {
        let ctx = egui::Context::default();
        style::install_fonts(&ctx);
        let mut galley = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = FontId::new(14.0, egui::FontFamily::Name(style::BOLD.into()));
            let row_height = ui.fonts_mut(|f| f.row_height(&font));
            let job = text_job(
                text,
                font,
                Color32::BLACK,
                Vec2::new(w, h),
                row_height,
                halign,
                wrap,
            );
            galley = Some(ui.painter().layout_job(job));
        });
        out.textures_delta.clear();
        galley.unwrap()
    }

    const PV: &str = "<BL01T-DI-CAM-01:DET:PortName_RBV>";

    #[test]
    fn overflow_in_one_row_keeps_the_start() {
        // A 20px-high textupdate shows one whole row, elided, never a middle fragment.
        for halign in [egui::Align::Min, egui::Align::Center, egui::Align::Max] {
            for wrap in [true, false] {
                let g = layout(PV, 205.0, 18.0, halign, wrap);
                assert_eq!(g.rows.len(), 1);
                assert!(g.elided);
                assert!(g.rows[0].text().starts_with("<BL01T-DI-CAM-01"));
                assert!(g.rows[0].text().ends_with('…'));
                assert!(g.size().x <= 205.5);
            }
        }
    }

    #[test]
    fn wrapped_rows_fit_and_align_on_their_own() {
        let g = layout(PV, 205.0, 40.0, egui::Align::Center, true);
        assert_eq!(g.rows.len(), 2);
        assert!(!g.elided);
        // Each row centred on x = 0, so the shorter row starts further right.
        for row in &g.rows {
            let r = row.rect();
            assert!((r.center().x).abs() < 1.0, "{r:?}");
        }
        let g = layout(PV, 205.0, 40.0, egui::Align::Max, true);
        for row in &g.rows {
            assert!(row.rect().max.x.abs() < 1.0);
        }
    }

    #[test]
    fn text_that_just_fits_is_not_elided() {
        // At fractional zooms text can fit with under a pixel to spare.
        let natural = layout(PV, 10_000.0, 18.0, egui::Align::Center, true)
            .size()
            .x;
        let g = layout(PV, natural.ceil(), 18.0, egui::Align::Center, true);
        assert!(!g.elided);
        assert_eq!(g.text(), PV);
    }

    #[test]
    fn explicit_lines_are_kept_without_wrapping() {
        // wrap_words=false still shows every line, as a JavaFX Label does.
        let g = layout(
            "Line one\nLine two longer",
            200.0,
            50.0,
            egui::Align::Min,
            false,
        );
        assert_eq!(g.rows.len(), 2);
        assert!(!g.elided);
    }

    #[test]
    fn short_text_is_untouched() {
        let g = layout("12.5 mm", 100.0, 18.0, egui::Align::Center, true);
        assert_eq!(g.rows.len(), 1);
        assert!(!g.elided);
        assert_eq!(g.text(), "12.5 mm");
    }

    #[test]
    fn pvi_open_display_symbol_has_a_glyph() {
        let ctx = egui::Context::default();
        style::install_fonts(&ctx);
        let mut found = vec![];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for family in [
                egui::FontFamily::Proportional,
                egui::FontFamily::Name(style::BOLD.into()),
                egui::FontFamily::Monospace,
            ] {
                let font = FontId::new(14.0, family);
                found.push(ui.fonts_mut(|f| f.has_glyph(&font, '\u{29C9}')));
            }
        });
        out.textures_delta.clear();
        assert_eq!(found, [true, true, true]);
    }

    fn widget(props: &str) -> Widget {
        let xml = format!(r#"<display><widget type="slide_button">{props}</widget></display>"#);
        bob::Display::parse(&xml).unwrap().widgets.remove(0)
    }

    fn state(value: i64, writable: bool) -> PvState {
        PvState {
            connected: true,
            value: Value::Long(value),
            writable,
            ..Default::default()
        }
    }

    #[test]
    fn toggles_the_whole_value_or_one_bit() {
        // A slide button's bit defaults to 0, as in Phoebus.
        let default = widget("");
        assert_eq!(toggled(&default, Some(&state(0, true))), 1);
        assert_eq!(toggled(&default, Some(&state(5, true))), 4);
        let whole = widget("<bit>-1</bit>");
        assert_eq!(toggled(&whole, Some(&state(0, true))), 1);
        assert_eq!(toggled(&whole, Some(&state(5, true))), 0);
        let bit1 = widget("<bit>1</bit>");
        assert_eq!(toggled(&bit1, Some(&state(1, true))), 3);
        assert_eq!(toggled(&bit1, Some(&state(3, true))), 1);
        let bit3 = widget("<bit>3</bit>");
        assert_eq!(toggled(&bit3, Some(&state(1, true))), 9);
    }

    #[test]
    fn push_values_set_and_clear_the_bit() {
        assert_eq!(with_bit(-1, 6, true), 1);
        assert_eq!(with_bit(-1, 6, false), 0);
        assert_eq!(with_bit(2, 1, true), 5);
        assert_eq!(with_bit(2, 5, false), 1);
    }

    #[test]
    fn push_writes_on_press_and_release() {
        assert_eq!(push_edge(false, true, false), Push::Press);
        assert_eq!(push_edge(true, true, false), Push::None);
        assert_eq!(push_edge(true, false, true), Push::Release);
        assert_eq!(push_edge(false, false, true), Push::Both);
        assert_eq!(push_edge(false, false, false), Push::None);
    }

    #[test]
    fn disabled_or_read_only_controls_do_not_write() {
        let w = widget("");
        assert!(can_write(&w, Some(&state(0, true))));
        assert!(!can_write(&w, Some(&state(0, false))));
        assert!(is_disabled(&w, Some(&state(0, false))));
        assert!(!can_write(&w, None));
        let disconnected = PvState::default();
        assert!(!can_write(&w, Some(&disconnected)));
        assert!(!is_disabled(&w, Some(&disconnected)));
        let off = widget("<enabled>false</enabled>");
        assert!(is_disabled(&off, None));
        assert!(!can_write(&off, Some(&state(0, true))));
    }

    #[test]
    fn tooltip_fills_in_pv_name_and_value() {
        let expand = |t: &str| t.replace("$(P)", "BL01T:");
        assert_eq!(
            tooltip_text("$(pv_name)\n$(pv_value)", "X:Y", "1.5 mm", &expand),
            "X:Y\n1.5 mm"
        );
        assert_eq!(
            tooltip_text("$(P)M1.STOP = 1", "", "", &expand),
            "BL01T:M1.STOP = 1"
        );
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
