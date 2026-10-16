//! Painting and interaction for each widget type.

use bob::{Action, Macros, Source, Widget};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use pv::{PvManager, PvState, Value};

use crate::rules::{self, Outcome};
use crate::screen::{Builder, Node, Screen, group_inset, tab_elements};
use crate::style::{self, View, color, named, severity_color};
use crate::view::WindowKey;

/// Requests that widgets pass up to the application.
pub enum Request {
    Open {
        source: Source,
        macros: Macros,
        target: String,
    },
    Webpage(String),
    /// Show the details of a widget's PV, from its context menu.
    Details(crate::pvmenu::Details),
    /// Open a trend window of these PVs.
    Trend(Vec<String>),
    /// Add these PVs to the open trend window with this id.
    TrendAdd(u64, Vec<String>),
    /// Ask before writing `value` to `pv`, as a `write_pv` action of a
    /// display from the web asked to.
    ConfirmWrite {
        pv: pv::Pv,
        value: String,
    },
    /// Close the display: go back to the one before, or close the window.
    Close,
    /// Bring every window to the front, the one asking last.
    RaiseAll,
    /// Close every display and trend window but the main one and the one
    /// asking.
    CloseOthers,
    /// Run a command action of the display at `base` (see
    /// [`crate::launch::command_target`]) and open the display it names.
    Command {
        base: Option<Source>,
        directory: Option<Source>,
        command: String,
    },
}

/// Per-frame drawing context.
pub struct Cx<'a> {
    pub view: View,
    pub clip: Rect,
    pub pvs: &'a PvManager,
    pub requests: &'a mut Vec<Request>,
    pub depth: u32,
    /// Touch mode: finger-sized rows in drop-down lists.
    pub touch: bool,
    /// The open trend windows, (id, title), that a PV's context menu can
    /// add it to.
    pub trends: &'a [(u64, String)],
    /// The window being drawn, which its requests act on.
    pub window: WindowKey,
}

impl Cx<'_> {
    fn sub<'b>(&'b mut self, view: View, clip: Rect) -> Cx<'b> {
        Cx {
            view,
            clip,
            pvs: self.pvs,
            requests: self.requests,
            depth: self.depth + 1,
            touch: self.touch,
            trends: self.trends,
            window: self.window,
        }
    }
}

/// The smallest target, in points, a finger can hit reliably.
pub const TOUCH_TARGET: f32 = 44.0;

/// egui's own hit radius outside touch mode.
const MOUSE_RADIUS: f32 = 5.0;

/// How far outside a control a touch still hits it: enough to make a 20
/// point control (a pvi slide button or checkbox at 100%) a full
/// [`TOUCH_TARGET`].
const TOUCH_RADIUS: f32 = (TOUCH_TARGET - 20.0) / 2.0;

/// Grow the hit area of every control in touch mode, without
/// changing how anything is drawn. egui hit-tests in two steps: a touch
/// directly on a control always goes to that control (the topmost one), and
/// only a touch on no control at all goes to the nearest control within
/// `interact_radius`. So a small control gets a finger-sized target in the
/// space around it, but never takes a touch from a neighbour that was hit
/// directly, and the gap between two controls is shared by distance.
pub fn touch_targets(ctx: &egui::Context, touch: bool) {
    let radius = if touch { TOUCH_RADIUS } else { MOUSE_RADIUS };
    if ctx.global_style().interaction.interact_radius != radius {
        ctx.all_styles_mut(|s| s.interaction.interact_radius = radius);
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
    // Rules depend only on their PVs: none changed, nothing to do.
    let versions: Vec<u64> = node
        .rules
        .iter()
        .flat_map(|r| r.pvs())
        .map(pv::Pv::version)
        .collect();
    if node.rule_versions.as_ref() == Some(&versions) {
        return;
    }
    node.rule_versions = Some(versions);
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
    crate::group_link::update(node, cx.pvs);
    update_rules(node);
    if node.w.bool("visible") == Some(false) {
        return;
    }
    if node.w.kind == "label" && node.w.bool("auto_size") == Some(true) {
        auto_size(ui.painter(), node, cx.view.zoom);
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
        "stripchart" => {
            crate::trend::draw_stripchart(&painter, node, rect, z);
            None
        }
        "meter" => {
            crate::meter::dial(&painter, &node.w, state.as_ref(), rect, z);
            None
        }
        "progressbar" | "tank" | "thermometer" => {
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
        "symbol" => crate::symbol::draw_symbol(ui, node, state.as_ref(), rect, cx, id),
        "picture" => {
            crate::symbol::draw_picture(ui, node, rect, cx);
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

    if matches!(
        node.w.kind.as_str(),
        "label" | "textupdate" | "textentry" | "embedded"
    ) {
        border(&painter, &node.w, rect, z);
    }
    if let Some(state) = &state {
        alarm_border(&painter, &node.w, state, rect, z);
    }
    // A shape or monitor (such as an EDM related-display hotspot from an
    // `.opi`) that runs its actions when clicked.
    let control = response.is_some();
    let response = match response {
        None if !is_container(&node.w.kind) => click_hook(ui, node, rect, cx, id),
        r => r,
    };

    // A control that is disabled or whose PV is read-only is greyed out and
    // shows a "not allowed" cursor; its writes are refused in `write`. The
    // image and symbol widgets respond to the pointer but are not controls,
    // nor is a monitor that only runs actions when clicked.
    let disabled = control
        && !matches!(node.w.kind.as_str(), "image" | "symbol")
        && is_disabled(&node.w, state.as_ref());
    // A slide button greys only its track: the rest of its box is see-through.
    if disabled && node.w.kind != "slide_button" {
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
            .show(|ui| crate::pvmenu::menu(ui, node, state.as_ref(), cx.trends, cx.requests));
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

/// The widget's `border_width` and `border_color`, drawn around it.
fn border(painter: &egui::Painter, w: &Widget, rect: Rect, z: f32) {
    let width = w.f64("border_width").unwrap_or(0.0) as f32 * z;
    if width <= 0.0 {
        return;
    }
    let c = w
        .color("border_color")
        .map(|c| color(&c))
        .unwrap_or_else(|| named("Text"));
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(width, c),
        StrokeKind::Outside,
    );
}

/// Run the actions of a widget with `run_actions_on_mouse_click` when it is
/// clicked; its response, if it has any such actions.
fn click_hook(ui: &mut Ui, node: &Node, rect: Rect, cx: &mut Cx, id: Id) -> Option<Response> {
    let w = &node.w;
    if w.bool("run_actions_on_mouse_click") != Some(true) || w.bool("enabled") == Some(false) {
        return None;
    }
    let actions = w.actions();
    if actions.is_empty() {
        return None;
    }
    let resp = ui
        .interact(rect.intersect(cx.clip), id.with("click"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        for a in &actions {
            run_action(node, a, cx);
        }
    }
    Some(resp)
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
        Some(t) => {
            // `$(actions)` lists what the widget's actions do, as in Phoebus.
            let actions = node.w.actions();
            let t = if t.contains("$(actions)") {
                let list: Vec<&str> = actions
                    .iter()
                    .map(|a| a.description())
                    .filter(|d| !d.trim().is_empty())
                    .collect();
                t.replace("$(actions)", &list.join("\n"))
            } else {
                t.to_string()
            };
            tooltip_text(&t, &node.pv_name, &value, &|t| node.expand(t))
        }
        None => {
            let name = plain_pv_name(&node.pv_name);
            match s {
                Some(s) if s.connected => format!("{name}\n{value} {:?}", s.severity),
                Some(_) => format!("{name}\n{value}"),
                None => name,
            }
        }
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

/// A PV name as a user reads it, without a local PV's type and initial value
/// (`loc://arr`, not `loc://arr<VDoubleArray>(1, 3, 2)`).
pub(crate) fn plain_pv_name(name: &str) -> String {
    crate::app::plain_name(&pv::PvName::parse(name))
}

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
    text_turned(painter, rect, text, font, c, align, wrap, 0);
}

/// [`text_in`], turned by Phoebus `rotation_step`: 1 is 90° anticlockwise
/// (reading upwards), 2 is 180° and 3 is 90° clockwise.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_turned(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    c: Color32,
    align: Align2,
    wrap: bool,
    step: i64,
) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let angle = match step {
        1 => -FRAC_PI_2,
        2 => PI,
        3 => FRAC_PI_2,
        _ => 0.0,
    };
    // Laid out in the box the text has before turning.
    let size = if step == 1 || step == 3 {
        Vec2::new(rect.height(), rect.width())
    } else {
        rect.size()
    };
    let area = Rect::from_center_size(rect.center(), size);
    let row_height = painter.fonts_mut(|f| f.row_height(&font));
    let job = text_job(text, font, c, size, row_height, align.x(), wrap);
    let galley = painter.layout_job(job);
    let x = match align.x() {
        egui::Align::Min => area.left() + 1.0,
        egui::Align::Center => area.center().x,
        egui::Align::Max => area.right() - 1.0,
    };
    let height = galley.size().y;
    let y = match align.y() {
        egui::Align::Min => area.top(),
        egui::Align::Center => area.center().y - height / 2.0,
        egui::Align::Max => area.bottom() - height,
    };
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if angle == 0.0 {
        painter.galley(Pos2::new(x, y), galley, c);
    } else {
        let center = rect.center();
        let pos = center + egui::emath::Rot2::from_angle(angle) * (Pos2::new(x, y) - center);
        painter.add(egui::epaint::TextShape::new(pos, galley, c).with_angle(angle));
    }
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
        max_rows: rows,
        break_anywhere: rows == 1,
        ..elide_wrap(size.x)
    };
    job
}

/// One row `width` wide, with what does not fit elided with `…`.
fn elide_wrap(width: f32) -> egui::text::TextWrapping {
    egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    }
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

/// The `format` that shows (and writes) an array of numbers as a long
/// string, as areaDetector's `FilePath` and other char waveforms are.
const STRING_FORMAT: i64 = 6;

/// Whether a control writes its text as a long string, as Phoebus does
/// when its `format` is string: see [`pv::Pv::write_long_string`].
fn writes_long_string(w: &Widget) -> bool {
    w.i64("format") == Some(STRING_FORMAT)
}

/// Value text as Phoebus formats it for the widget's `format` and `precision`.
pub(crate) fn format_value(w: &Widget, s: &PvState) -> String {
    let precision = w.i64("precision").filter(|p| *p >= 0).map(|p| p as i32);
    let units = w.bool("show_units").unwrap_or(true);
    let format = w.i64("format").unwrap_or(0);
    match (format, &s.value) {
        (STRING_FORMAT, v) if v.is_numeric_array() => v.as_long_string().unwrap_or_default(),
        // A number as the character of its low byte, as Phoebus shows it.
        (STRING_FORMAT, Value::Long(_) | Value::Double(_)) => {
            let byte = s.value.as_i64().unwrap_or(0) as u8;
            let text = pv::long_string([byte]);
            if units && !s.units.is_empty() {
                format!("{text} {}", s.units)
            } else {
                text
            }
        }
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

/// The `bit` property: Phoebus defaults it to 0 for checkbox, slide_button and
/// bool_button, and to -1 (the whole value) for LEDs.
fn bit(w: &Widget) -> i64 {
    let default = match w.kind.as_str() {
        "checkbox" | "slide_button" | "bool_button" => 0,
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

/// A write action of a display from the web (which anyone may have written)
/// needs the user's say-so; one of a local file does not.
fn write_needs_confirmation(base: Option<&Source>) -> bool {
    base.is_some_and(Source::is_url)
}

fn write(node: &Node, value: impl Into<String>) {
    write_with(node, |pv| pv.write(value));
}

/// Write to the widget's PV with `put`, if the control may write it.
fn write_with(node: &Node, put: impl FnOnce(&pv::Pv)) {
    if let Some(pv) = &node.pv {
        if can_write(&node.w, Some(&pv.state())) {
            put(pv);
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
    text_turned(
        painter,
        rect,
        &text,
        font(w, z),
        fg(w),
        alignment(w, 0),
        wrap,
        w.i64("rotation_step").unwrap_or(0),
    );
}

/// Size an `auto_size` label to its text, as Phoebus does: the width and
/// height become those of the expanded text on one row per line, with x and
/// y kept. Done each frame, so a rule that changes the text resizes it too.
fn auto_size(painter: &egui::Painter, node: &mut Node, z: f32) {
    let w = &node.w;
    let galley = painter.layout_no_wrap(node.prop("text"), font(w, z), fg(w));
    let size = galley.size() / z;
    let (width, height) = match w.i64("rotation_step") {
        Some(1 | 3) => (size.y, size.x),
        _ => (size.x, size.y),
    };
    node.rect[2] = width.ceil() as f64;
    node.rect[3] = height.ceil() as f64;
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
    text_turned(
        painter,
        rect.shrink(1.0),
        &text,
        font(w, z),
        fg(w),
        alignment(w, 1),
        wrap,
        w.i64("rotation_step").unwrap_or(0),
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
    if w.kind == "arc" {
        let start = w.f64("start_angle").unwrap_or(0.0) as f32;
        let total = w.f64("total_angle").unwrap_or(90.0) as f32;
        let inner = rect.shrink(lw / 2.0);
        let points = arc_points(inner, start, total);
        if !transparent && fill_c.a() > 0 {
            // A pie wedge, as a fan from the centre (it may be concave).
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(inner.center(), fill_c);
            for p in &points {
                mesh.colored_vertex(*p, fill_c);
            }
            for i in 1..points.len() as u32 {
                mesh.add_triangle(0, i, i + 1);
            }
            painter.add(mesh);
        }
        if lw > 0.0 {
            let mut line = points;
            if !transparent {
                line.insert(0, inner.center());
                line.push(inner.center());
            }
            painter.add(Shape::line(line, stroke));
        }
    } else if w.kind == "rectangle" {
        let cr = w
            .f64("corner_width")
            .unwrap_or(0.0)
            .max(w.f64("corner_height").unwrap_or(0.0)) as f32
            * z
            / 2.0;
        let r = CornerRadius::same(cr.min(255.0) as u8);
        let style = w.i64("line_style").unwrap_or(0);
        if style == 0 || cr > 0.0 || lw <= 0.0 {
            painter.rect(rect, r, fill_c, stroke, StrokeKind::Inside);
        } else {
            painter.rect_filled(rect, r, fill_c);
            let inner = rect.shrink(lw / 2.0);
            let corners = [
                inner.left_top(),
                inner.right_top(),
                inner.right_bottom(),
                inner.left_bottom(),
                inner.left_top(),
            ];
            crate::plot::line(painter, corners.to_vec(), stroke, style);
        }
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

/// Points along the arc of the ellipse in `rect` from `start` degrees over
/// `total` degrees, anticlockwise from three o'clock, as Phoebus draws arcs.
fn arc_points(rect: Rect, start: f32, total: f32) -> Vec<Pos2> {
    let n = ((total.abs() / 5.0).ceil() as usize).clamp(2, 73);
    let (c, r) = (rect.center(), rect.size() / 2.0);
    (0..=n)
        .map(|i| {
            let a = (start + total * i as f32 / n as f32).to_radians();
            Pos2::new(c.x + r.x * a.cos(), c.y - r.y * a.sin())
        })
        .collect()
}

fn poly(painter: &egui::Painter, w: &Widget, view: View, r: &[f64; 4]) {
    let display = w.points();
    let points: Vec<Pos2> = display
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
        crate::polygon::fill(painter, &display, points.clone(), fill_c);
        let mut closed = points;
        closed.push(closed[0]);
        painter.add(Shape::line(closed, Stroke::new(lw, line)));
    } else {
        let style = w.i64("line_style").unwrap_or(0);
        crate::plot::line(painter, points, Stroke::new(lw, line), style);
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
    let mut buf = node.edit.clone().unwrap_or(shown);
    // Text that does not fit is elided with `…` like a text update's, except
    // while it is being edited, when every character must be there for the
    // cursor. (A click that focuses it lands in the elided layout, which
    // matches the full text up to the `…`.)
    let editing = ui.memory(|m| m.has_focus(id));
    let fnt = font(&w, cx.view.zoom);
    let color = fg(&w);
    let margin = Vec2::new(2.0, 0.0);
    let text_width = rect.width() - 2.0 * margin.x;
    let mut elided = |ui: &Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
        let job = entry_job(
            text.as_str(),
            fnt.clone(),
            color,
            align.x(),
            text_width,
            editing,
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let edit = egui::TextEdit::singleline(&mut buf)
        .layouter(&mut elided)
        .id(id)
        .font(font(&w, cx.view.zoom))
        .text_color(fg(&w))
        .background_color(bg(&w, "Write_Background"))
        .horizontal_align(align.x())
        .vertical_align(egui::Align::Center)
        .margin(margin)
        .interactive(enabled)
        .min_size(rect.size());
    let resp = put_clipped(ui, rect, cx.clip, edit);
    if resp.has_focus() {
        node.edit = Some(buf);
    } else if resp.lost_focus() {
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            // Phoebus keeps the spaces of text written as a string.
            if writes_long_string(&node.w) {
                write_with(node, |pv| pv.write_long_string(buf));
            } else {
                write(node, buf.trim().to_string());
            }
        }
        node.edit = None;
    }
    resp
}

/// The layout of a text entry's text: one row, as egui's own single-line
/// layout, but elided with `…` to `width` unless it is being edited.
fn entry_job(
    text: &str,
    font: FontId,
    c: Color32,
    halign: egui::Align,
    width: f32,
    editing: bool,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, c);
    job.halign = halign;
    job.keep_trailing_whitespace = true;
    if !editing {
        job.wrap = elide_wrap(width);
    }
    job
}

/// Put a widget at `rect`, clipped. The scope starts at `rect`, not at the
/// cursor: one there would take in the space after the last widget each
/// time, and grow the display's scrolled size below its bottom.
fn put_clipped(ui: &mut Ui, rect: Rect, clip: Rect, widget: impl egui::Widget) -> Response {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(clip.intersect(ui.clip_rect()));
        ui.put(rect, widget)
    })
    .inner
}

fn action_button(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) -> Response {
    let w = &node.w;
    let actions = w.actions();
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
    // A transparent button draws only its text, over what lies beneath.
    if w.bool("transparent") != Some(true) {
        button_box(
            ui.painter(),
            rect,
            bg(w, "Button_Background"),
            resp.is_pointer_button_down_on(),
            cx.view.zoom,
        );
    }
    let text = button_text(node, &actions);
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

/// An action button's label. As in Phoebus, only text naming `$(actions)`
/// (the default when there is none) is replaced by the first action's
/// description; empty text stays empty.
fn button_text(node: &Node, actions: &[Action]) -> String {
    if let Some(text) = node.w.text("text")
        && !text.contains("$(actions)")
    {
        return node.prop("text");
    }
    actions
        .first()
        .map(|a| node.expand(a.description()))
        .unwrap_or_default()
}

pub(crate) fn run_action(node: &Node, action: &Action, cx: &mut Cx) {
    match action {
        Action::OpenDisplay {
            file,
            macros,
            target,
            ..
        } => {
            // The file name may use the action's own macros, as in
            // `<file>$(IOC)/$(P).bob</file>` with IOC and P set by the action.
            let expanded =
                Macros::from_pairs(macros.iter().map(|(k, v)| (k.to_string(), node.expand(v))));
            let macros = node.macros.with(&expanded);
            let file = node.expand_with(&macros, file);
            cx.requests.push(Request::Open {
                source: node.resolve(&file),
                macros,
                target: target.clone(),
            });
        }
        Action::WritePv { pv_name, value, .. } => {
            let pv = action_pv(node, cx.pvs, &node.expand(pv_name));
            let value = node.expand(value);
            if write_needs_confirmation(node.base.as_ref()) && !cx.pvs.read_only() {
                cx.requests.push(Request::ConfirmWrite { pv, value });
            } else {
                // In read-only mode the PV refuses, and says so.
                pv.write(value);
            }
        }
        Action::OpenWebpage { url, .. } => {
            let url = node.expand(url);
            if webpage_allowed(node.base.as_ref(), &url) {
                cx.requests.push(Request::Webpage(url));
            } else {
                log::warn!("not opening '{url}': a display from the web may only open web pages");
            }
        }
        Action::Execute {
            text, description, ..
        } => {
            if let Some(names) = bob::databrowser_pvs(text) {
                trend(node, &names, cx);
            } else if is_toggle_script(text) {
                if let Some(s) = node.state() {
                    // As `(PVUtil.getDouble(pv) == 0) ? 1 : 0`: a value
                    // that is not a number is NaN there, so it writes 0.
                    let zero = s.value.as_f64() == Some(0.0);
                    let value = if zero { "1" } else { "0" };
                    match &node.pv {
                        Some(pv)
                            if write_needs_confirmation(node.base.as_ref())
                                && !cx.pvs.read_only()
                                && can_write(&node.w, Some(&s)) =>
                        {
                            cx.requests.push(Request::ConfirmWrite {
                                pv: pv.clone(),
                                value: value.into(),
                            });
                        }
                        _ => write(node, value),
                    }
                }
            } else {
                log::warn!("script '{description}' is not supported: bobview runs no scripts");
            }
        }
        Action::Command {
            command, directory, ..
        } => {
            let command = node.expand(command);
            // A StripTool launcher is shown as a trend, and not run.
            if let Some(names) = bob::strip_tool_pvs(&command) {
                cx.requests.push(Request::Trend(names));
                return;
            }
            let directory = node.expand(directory);
            let directory = (!directory.trim().is_empty()).then(|| node.resolve(&directory));
            // Only DLS launcher scripts of local displays, which open a
            // display; run on a thread, as a script may take seconds.
            cx.requests.push(Request::Command {
                base: node.base.clone(),
                directory,
                command,
            });
        }
        Action::CloseDisplay { .. } => cx.requests.push(Request::Close),
        Action::Trend { pv_names, .. } => trend(node, pv_names, cx),
        Action::Unsupported { kind, .. } => log::warn!("action '{kind}' is not supported"),
    }
}

/// Open a trend of PVs named by a widget's action, macros expanded.
fn trend(node: &Node, names: &[String], cx: &mut Cx) {
    let names = names.iter().map(|n| node.expand(n)).collect();
    cx.requests.push(Request::Trend(names));
}

/// Whether `script` is the toggle that techui-builder embeds in synoptic
/// symbols, which bobview runs natively since it runs no scripts:
///
/// ```js
/// pv = ScriptUtil.getPrimaryPV(widget)
/// var pv_value = PVUtil.getDouble(pv);
/// pv_value = (pv_value == 0) ? 1 : 0;
/// pv.setValue(pv_value);
/// ```
fn is_toggle_script(script: &str) -> bool {
    let code: String = script
        .lines()
        .filter(|l| !l.trim_start().starts_with("importClass") && !l.trim_start().starts_with("/*"))
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace() && *c != ';')
        .collect();
    code == "pv=ScriptUtil.getPrimaryPV(widget)varpv_value=PVUtil.getDouble(pv)\
             pv_value=(pv_value==0)?1:0pv.setValue(pv_value)"
}

/// Whether a display at `base` may hand `url` to `xdg-open`. One fetched
/// over HTTP is not trusted with local files or other URL schemes (which
/// could open or run something on this machine), only with web pages; and
/// nothing may pass `xdg-open` an option.
fn webpage_allowed(base: Option<&Source>, url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return false;
    }
    !base.is_some_and(Source::is_url) || Source::parse(url).is_url()
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
            // Only the primary button pushes; a right-click opens the PV menu.
            let down = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
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
    // Click sensing (here and merged from each choice below) lets the PV
    // context menu open from anywhere on the widget.
    let mut resp = ui.interact(rect.intersect(cx.clip), id, Sense::click());
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
    let (items, current) = choices(&w, s);
    let mux = w.menu_mux();
    // Like Phoebus, show the PV's value whatever the items are; the items
    // only fill the dropdown. A menu mux shows the item the value is for.
    let text = match s {
        Some(s) if s.connected => mux
            .as_ref()
            .and_then(|m| m.current(current, &s.format(None, false)))
            .and_then(|i| items.get(i).cloned())
            .unwrap_or_else(|| s.format(None, false)),
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
    // Like Phoebus, the list is at least as wide as the combo and in its
    // font (but never smaller than the rest of the UI, so it stays readable
    // when the display is zoomed out); a finger-sized row in touch mode.
    let item_font = combo_item_font(
        font(&w, z),
        &ui.style().text_styles[&egui::TextStyle::Button],
    );
    let row_h = if cx.touch { TOUCH_TARGET } else { 0.0 };
    egui::Popup::menu(&resp).width(rect.width()).show(|ui| {
        ui.set_min_width(rect.width() - ui.spacing().menu_margin.sum().x);
        for (i, item) in items.iter().enumerate() {
            let label = egui::RichText::new(item).font(item_font.clone());
            let button = egui::Button::selectable(*item == text, label)
                .min_size(Vec2::new(ui.available_width(), row_h));
            if ui.add(button).clicked() {
                match &mux {
                    Some(mux) => mux_pick(node, cx.pvs, mux, i),
                    None => write(node, item.clone()),
                }
            }
        }
    });
    resp
}

/// Pick item `i` of a menu mux: write its value to each target, and its
/// index to the mux's own PV.
fn mux_pick(node: &Node, pvs: &pv::PvManager, mux: &bob::MenuMux, i: usize) {
    for (target, value) in mux.writes(i) {
        action_pv(node, pvs, &node.expand(target)).write(node.expand(value));
    }
    if mux.index {
        write(node, i.to_string());
    }
}

/// The PV `name` that an action of `node` writes: the one connected in
/// advance, or else a new one.
fn action_pv(node: &Node, pvs: &pv::PvManager, name: &str) -> pv::Pv {
    let parsed = pv::PvName::parse(name).to_string();
    match node
        .action_pvs
        .iter()
        .find(|p| p.name().to_string() == parsed)
    {
        Some(pv) => pv.clone(),
        None => pvs.subscribe(name),
    }
}

/// The font for a combo's dropdown rows: the widget's, unless that is
/// smaller than the UI's own button font.
fn combo_item_font(widget: FontId, ui: &FontId) -> FontId {
    if widget.size < ui.size {
        FontId::new(ui.size, widget.family)
    } else {
        widget
    }
}

/// Phoebus labels a checkbox or slide button "Label" unless the file says
/// otherwise; an explicit empty `<label>` stays empty.
fn raw_label(w: &Widget) -> &str {
    w.text("label").unwrap_or("Label")
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
        &node.expand(raw_label(&w)),
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

/// Phoebus SlideButton (a JavaFX ToggleSwitch): a pill-shaped track in
/// off_color/on_color with a round thumb that slides to the right when the
/// selected bit is set, then the label. A click toggles the bit like a checkbox.
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
    let label = node.expand(raw_label(&w));
    let (track, label_rect) = slide_layout(rect, z, !label.is_empty());
    let track_color = if on {
        w.color("on_color")
            .map(|c| color(&c))
            .unwrap_or_else(|| named("OK"))
    } else {
        w.color("off_color")
            .map(|c| color(&c))
            .unwrap_or_else(|| named("Button_Background"))
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
    if is_disabled(&w, s) {
        painter.rect_filled(track, r, Color32::from_white_alpha(110));
    }
    if let Some(lr) = label_rect {
        text_in(
            &painter,
            lr,
            &label,
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
/// fitted into the widget at its left, with the label to its right, as
/// Phoebus lays out its toggle switch and label.
fn slide_layout(rect: Rect, z: f32, has_label: bool) -> (Rect, Option<Rect>) {
    let h = (16.0 * z)
        .min(rect.height() - 2.0)
        .min((rect.width() - 2.0) / 1.8)
        .max(2.0);
    let tw = h * 1.8;
    let track = Rect::from_min_size(
        Pos2::new(rect.left() + 1.0, rect.center().y - h / 2.0),
        Vec2::new(tw, h),
    );
    let label = has_label
        .then(|| Rect::from_min_max(Pos2::new(track.right() + 6.0 * z, rect.top()), rect.max));
    (track, label)
}

/// Phoebus Spinner: the value, dragged or typed, kept within the range and
/// stepped by `increment`.
fn spinner(ui: &mut Ui, node: &mut Node, s: Option<&PvState>, rect: Rect, cx: &mut Cx) -> Response {
    let w = node.w.clone();
    let (min, max) = crate::meter::limits(&w, s);
    let (min, max) = (min.min(max), max.max(min));
    let mut v = s.and_then(|s| s.value.as_f64()).unwrap_or(min);
    let step = w.f64("increment").unwrap_or(1.0);
    let drag = egui::DragValue::new(&mut v).range(min..=max).speed(step);
    let resp = put_clipped(ui, rect, cx.clip.intersect(rect), drag);
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        write(node, v.to_string());
    }
    resp
}

/// Where a slider's parts go, along its length and across it.
#[derive(Debug)]
struct SliderLayout {
    /// The band the thumb slides along.
    band: Rect,
    /// The thumb's length along the slider.
    thumb: f32,
    /// The thumb's centre at the minimum and at the maximum.
    from: f32,
    to: f32,
    /// The band on the far side of the thumb's for the scale's ticks, and
    /// whether its labels fit in it too.
    scale: Option<Rect>,
    labels: bool,
    /// The band on the near side for the alarm level markers.
    markers: Option<Rect>,
}

/// Lay out a slider in `rect`: the thumb's band, the scale on its far side
/// (below a horizontal slider, right of a vertical one) and the level
/// markers on its near side, all centred across it, as a JavaFX Slider
/// centres its track and tick marks. `scale` is the room the scale's
/// labels take across the slider and half of their length along it, and
/// `markers` the room the markers' labels take across it; `None` leaves
/// either out. What the box has no room for is left out, the markers
/// first, then the scale's labels and then its ticks, before the thumb
/// gets thin.
fn slider_layout(
    rect: Rect,
    horizontal: bool,
    z: f32,
    scale: Option<(f32, f32)>,
    markers: Option<f32>,
) -> SliderLayout {
    let (along, across) = if horizontal {
        (rect.x_range(), rect.y_range())
    } else {
        (rect.y_range(), rect.x_range())
    };
    let pad = 2.0 * z;
    let tick = 5.0 * z;
    let room = (across.span() - 2.0 * pad).max(1.0);
    let (full, thin) = (16.0 * z, 8.0 * z);
    let ticks = scale.is_some();
    let label = scale.map_or(0.0, |(l, _)| l);
    let (mark, depth, labels) = [
        (markers.map(|m| m + tick), ticks, ticks),
        (None, ticks, ticks),
        (None, ticks, false),
        (None, false, false),
    ]
    .into_iter()
    .map(|(m, t, l)| {
        let depth = if t { tick } else { 0.0 } + if l { label } else { 0.0 };
        (m.unwrap_or(0.0), depth, l)
    })
    .find(|(m, depth, _)| room - m - depth >= thin)
    .unwrap_or((0.0, 0.0, false));
    let thick = (room - mark - depth).clamp(1.0, full);
    let start = across.center() - (mark + thick + depth) / 2.0;
    let thumb = (thick * 0.6).max(1.0);
    let half_label = scale.filter(|_| labels).map_or(0.0, |(_, half)| half);
    let end = (thumb / 2.0).max(half_label) + pad;
    let (lo, hi) = if along.span() > 2.0 * end {
        (along.min + end, along.max - end)
    } else {
        (along.center(), along.center())
    };
    let band = (start + mark, start + mark + thick);
    let part = |(a, b): (f32, f32)| {
        if horizontal {
            Rect::from_x_y_ranges(along, a..=b)
        } else {
            Rect::from_x_y_ranges(a..=b, along)
        }
    };
    // A vertical slider has its minimum at the bottom.
    let (from, to) = if horizontal { (lo, hi) } else { (hi, lo) };
    SliderLayout {
        band: part(band),
        thumb,
        from,
        to,
        scale: (depth > 0.0).then(|| part((band.1, band.1 + depth))),
        labels,
        markers: (mark > 0.0).then(|| part((start, band.0))),
    }
}

/// Phoebus ScaledSlider and ScrollBar: a groove along the widget with a
/// thumb at the value, and for a scaled slider with `show_scale` labelled
/// ticks on one side and with `show_markers` its alarm levels on the
/// other, as Phoebus' JavaFX Slider; no value box. A click or
/// drag moves the thumb, and its value is written when the button is let
/// go. An empty range (`maximum` equal to `minimum`) leaves the thumb at
/// the start and writes nothing.
fn slider(
    ui: &mut Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Response {
    if node.w.kind == "spinner" {
        return spinner(ui, node, s, rect, cx);
    }
    let w = node.w.clone();
    let z = cx.view.zoom;
    let (min, max) = crate::meter::limits(&w, s);
    let live = min != max;
    let horizontal = w.bool("horizontal").unwrap_or(true);
    let show_scale = w.kind == "scaledslider" && w.bool("show_scale").unwrap_or(true) && live;
    let painter = ui.painter().with_clip_rect(cx.clip.intersect(rect));
    if w.kind == "scaledslider" && w.bool("transparent") == Some(false) {
        fill(&painter, rect, bg(&w, "Background"));
    }
    let fnt = font(&w, z);
    let text = fg(&w);
    let size_of = |s: &str| {
        painter
            .layout_no_wrap(s.to_string(), fnt.clone(), text)
            .size()
    };
    let guess = (max - min).abs() / 10.0;
    let size = |v: f64| size_of(&crate::axis::label(v, guess));
    let (lo_size, hi_size) = (size(min), size(max));
    let widest = lo_size.x.max(hi_size.x);
    let line_h = lo_size.y;
    let room = if horizontal {
        (line_h, widest / 2.0)
    } else {
        (widest + 2.0 * z, line_h / 2.0)
    };
    // Phoebus' markers: the alarm levels inside the range, named.
    let markers: Vec<(f64, &str)> = crate::meter::levels(&w, s)
        .into_iter()
        .zip(["LOLO", "LOW", "HIGH", "HIHI"])
        .filter_map(|(level, name)| Some((level?, name)))
        .filter(|(level, _)| (min.min(max)..=max.max(min)).contains(level))
        .collect();
    let mark_room = (w.kind == "scaledslider" && w.bool("show_markers") == Some(true) && live)
        .then(|| {
            if horizontal {
                line_h
            } else {
                size_of("HIHI").x + 2.0 * z
            }
        });
    let lay = slider_layout(rect, horizontal, z, show_scale.then_some(room), mark_room);
    let scale = crate::axis::Scale {
        lo: min,
        hi: max,
        log: false,
        from: lay.from,
        to: lay.to,
    };

    // Follow the pointer while it is down on an enabled slider, and write
    // where it was let go.
    let resp = ui.interact(rect.intersect(cx.clip), id, Sense::click_and_drag());
    let mut v = s
        .and_then(|s| s.value.as_f64())
        .filter(|v| v.is_finite())
        .unwrap_or(min);
    let enabled = live && !is_disabled(&w, s);
    if enabled && let Some(p) = resp.interact_pointer_pos() {
        let at = if horizontal { p.x } else { p.y };
        let t = if lay.to != lay.from {
            ((at - lay.from) / (lay.to - lay.from)).clamp(0.0, 1.0) as f64
        } else {
            0.0
        };
        v = min + (max - min) * t;
        ui.data_mut(|d| d.insert_temp(id, v));
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(v) = ui.data_mut(|d| d.remove_temp::<f64>(id))
    {
        write(node, v.to_string());
    }

    // The groove, then the scale, then the thumb over both.
    let band = lay.band;
    let groove_w = (band.height().min(band.width()) / 4.0).max(2.0 * z);
    let groove = if horizontal {
        Rect::from_center_size(
            Pos2::new((lay.from + lay.to) / 2.0, band.center().y),
            Vec2::new((lay.to - lay.from).abs() + lay.thumb, groove_w),
        )
    } else {
        Rect::from_center_size(
            Pos2::new(band.center().x, (lay.from + lay.to) / 2.0),
            Vec2::new(groove_w, (lay.to - lay.from).abs() + lay.thumb),
        )
    };
    painter.rect(
        groove,
        CornerRadius::same((groove_w / 2.0).min(255.0) as u8),
        Color32::from_gray(225),
        Stroke::new(1.0, Color32::from_gray(150)),
        StrokeKind::Inside,
    );
    // A tick at `p` along the slider, out from `edge` across it (`out` is
    // 1 to the far side, -1 to the near one), with its label beyond.
    let stroke = Stroke::new(1.0, text);
    let tick_at = |edge: f32, p: f32, out: f32, label: Option<String>| {
        let len = 5.0 * z * out;
        let (a, b) = if horizontal {
            (Pos2::new(p, edge), Pos2::new(p, edge + len))
        } else {
            (Pos2::new(edge, p), Pos2::new(edge + len, p))
        };
        painter.line_segment([a, b], stroke);
        if let Some(label) = label {
            let (align, gap) = match (horizontal, out > 0.0) {
                (true, true) => (Align2::CENTER_TOP, 0.0),
                (true, false) => (Align2::CENTER_BOTTOM, 0.0),
                (false, true) => (Align2::LEFT_CENTER, 2.0 * z),
                (false, false) => (Align2::RIGHT_CENTER, -2.0 * z),
            };
            painter.text(b + Vec2::new(gap, 0.0), align, label, fnt.clone(), text);
        }
    };
    if let Some(area) = lay.scale {
        // Major ticks at least `major_tick_step_hint` apart, and far
        // enough apart for their labels.
        let hint = w.f64("major_tick_step_hint").unwrap_or(20.0) as f32 * z;
        let label_len = if horizontal {
            widest + 8.0 * z
        } else {
            line_h * 1.5
        };
        let gaps = ((lay.to - lay.from).abs() / hint.max(label_len)).floor();
        let (ticks, step) = crate::axis::ticks(min, max, gaps as usize + 1);
        let edge = if horizontal { area.top() } else { area.left() };
        for t in ticks {
            let label = lay.labels.then(|| crate::axis::label(t, step));
            tick_at(edge, scale.pos(t), 1.0, label);
        }
    }
    if let Some(area) = lay.markers {
        let edge = if horizontal {
            area.bottom()
        } else {
            area.right()
        };
        for (level, name) in &markers {
            tick_at(edge, scale.pos(*level), -1.0, Some(name.to_string()));
        }
    }
    let at = if live {
        scale.pos(v.clamp(min.min(max), max.max(min)))
    } else {
        lay.from
    };
    let thumb = if horizontal {
        Rect::from_center_size(
            Pos2::new(at, band.center().y),
            Vec2::new(lay.thumb, band.height()),
        )
    } else {
        Rect::from_center_size(
            Pos2::new(band.center().x, at),
            Vec2::new(band.width(), lay.thumb),
        )
    };
    button_box(
        &painter,
        thumb,
        named("Button_Background"),
        enabled && resp.is_pointer_button_down_on(),
        z,
    );
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
    let scale = embed_scale(w, screen, area.size() / z);
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

/// Whether the display `node` embeds, fetched in the background (in a
/// browser), has arrived, so should be embedded again.
#[cfg(target_arch = "wasm32")]
fn fetched(ui: &Ui, node: &Node) -> bool {
    if node.loading && crate::fetch::pending() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    node.loading && !crate::fetch::pending()
}

/// Show the display `node` embeds once its thread has built it, keeping
/// the window repainting until then. It is never embedded again, so false.
#[cfg(not(target_arch = "wasm32"))]
fn fetched(ui: &Ui, node: &mut Node) -> bool {
    if node.poll_building(std::time::Duration::ZERO) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    false
}

/// How much an embedded display is scaled to fit a widget `size` (unzoomed):
/// resize mode 1 scales the content to fit; the others draw it 1:1, clipped
/// to the widget (4, Crop, included).
fn embed_scale(w: &Widget, screen: &Screen, size: Vec2) -> f32 {
    match w.i64("resize").unwrap_or(0) {
        1 if screen.width > 0.0 && screen.height > 0.0 => {
            (size.x / screen.width as f32).min(size.y / screen.height as f32)
        }
        _ => 1.0,
    }
}

fn embedded(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut Cx, id: Id) {
    if fetched(ui, node) {
        let file = node.prop("file");
        let group = node.prop("group_name");
        let macros = node.macros.clone();
        Builder::ui(cx.pvs).embed(node, &file, &macros, &group, cx.depth);
    }
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
        Builder::ui(cx.pvs).select_navtab(node, i, cx.depth);
    }
    if fetched(ui, node) {
        Builder::ui(cx.pvs).select_navtab(node, node.active_tab, cx.depth);
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
    // A BOY widget (from an `.opi` file) drew INVALID and disconnected
    // solid white, DLS's BOY INVALID colour.
    if w.bool("boy_alarm_border") == Some(true)
        && (!s.connected || s.severity == pv::Severity::Invalid)
    {
        let width = (2.0 * z).max(1.0);
        outline(painter, rect.expand(width), width, Color32::WHITE);
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
    fn writes_of_displays_from_the_web_need_confirmation() {
        let url = Source::parse("https://host/a.bob");
        assert!(write_needs_confirmation(Some(&url)));
        assert!(!write_needs_confirmation(Some(&Source::parse(
            "/tmp/a.bob"
        ))));
        assert!(!write_needs_confirmation(None));
    }

    #[test]
    fn only_resize_mode_1_scales_embedded_content() {
        let d = bob::Display::parse(
            "<display><width>400</width><height>100</height><widget type='label'/></display>",
        )
        .unwrap();
        let pvs = PvManager::new();
        let screen = crate::screen::Builder::new(&pvs).screen(d, &bob::Macros::default(), None, 0);
        let widget = |resize: i64| {
            bob::Display::parse(&format!(
                "<display><widget type='embedded'><resize>{resize}</resize></widget></display>"
            ))
            .unwrap()
            .widgets[0]
                .clone()
        };
        let size = Vec2::new(200.0, 100.0);
        assert_eq!(embed_scale(&widget(1), &screen, size), 0.5);
        assert_eq!(embed_scale(&widget(4), &screen, size), 1.0);
        assert_eq!(embed_scale(&widget(0), &screen, size), 1.0);
    }
    #[test]
    fn commands_of_a_display_from_the_web_do_nothing() {
        let actions = |dir: &str| {
            format!(
                "<display><widget type='action_button'><actions><action type='command'>
                <command>./launch 04</command><command_directory>{dir}</command_directory>
                </action></actions></widget></display>"
            )
        };
        let pvs = PvManager::new();
        for dir in ["", "/tmp", "file:///tmp"] {
            let mut d = bob::Display::parse(&actions(dir)).unwrap();
            d.source = Some(Source::Url("https://example.com/x.bob".into()));
            let screen = crate::screen::Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
            let node = &screen.nodes[0];
            let mut requests = Vec::new();
            let mut cx = Cx {
                view: View {
                    origin: Pos2::ZERO,
                    zoom: 1.0,
                },
                clip: Rect::EVERYTHING,
                pvs: &pvs,
                requests: &mut requests,
                depth: 0,
                touch: false,
                trends: &[],
                window: WindowKey::Main,
            };
            let actions = node.w.actions();
            assert_eq!(actions.len(), 1);
            for a in actions {
                assert!(matches!(a, Action::Command { .. }));
                run_action(node, &a, &mut cx);
            }
            // The command is run on a thread, where it is refused.
            assert_eq!(requests.len(), 1, "{dir}");
            for r in requests {
                let Request::Command {
                    base,
                    directory,
                    command,
                } = r
                else {
                    panic!("not a command: {dir}");
                };
                let target = crate::launch::command_target(base.as_ref(), directory, &command);
                assert!(target.is_none(), "{dir}");
            }
        }
    }

    use std::sync::Arc;

    /// An action button with properties `props` and one action, "Go".
    fn button_screen(pvs: &PvManager, props: &str) -> crate::screen::Screen {
        let d = bob::Display::parse(&format!(
            "<display><widget type='action_button'>{props}<actions>
            <action type='open_display'><file>a.bob</file><description>Go</description>
            </action></actions></widget></display>"
        ))
        .unwrap();
        crate::screen::Builder::new(pvs).screen(d, &Macros::default(), None, 0)
    }

    #[test]
    fn only_action_button_text_naming_actions_shows_them() {
        let pvs = PvManager::new();
        for (props, shown) in [
            ("", "Go"),
            ("<text>$(actions)</text>", "Go"),
            ("<text></text>", ""),
            ("<text>Open</text>", "Open"),
        ] {
            let screen = button_screen(&pvs, props);
            let node = &screen.nodes[0];
            assert_eq!(button_text(node, &node.w.actions()), shown, "{props}");
        }
    }

    #[test]
    fn transparent_action_buttons_have_no_face() {
        let pvs = PvManager::new();
        let filled = |props: &str| {
            let mut screen = button_screen(&pvs, props);
            let ctx = egui::Context::default();
            let mut requests = Vec::new();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut cx = Cx {
                    view: View {
                        origin: Pos2::ZERO,
                        zoom: 1.0,
                    },
                    clip: Rect::EVERYTHING,
                    pvs: &pvs,
                    requests: &mut requests,
                    depth: 0,
                    touch: false,
                    trends: &[],
                    window: WindowKey::Main,
                };
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 30.0));
                action_button(ui, &mut screen.nodes[0], rect, &mut cx, Id::new("b"));
            });
            out.textures_delta.clear();
            out.shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill.a() > 0))
        };
        assert!(filled("<text></text>"));
        assert!(!filled("<text></text><transparent>true</transparent>"));
    }

    #[test]
    fn arcs_go_anticlockwise_from_three_o_clock() {
        let rect = Rect::from_center_size(Pos2::ZERO, Vec2::new(20.0, 10.0));
        let p = arc_points(rect, 0.0, 90.0);
        let near = |a: Pos2, b: Pos2| (a - b).length() < 1e-4;
        assert!(near(p[0], Pos2::new(10.0, 0.0)));
        assert!(near(*p.last().unwrap(), Pos2::new(0.0, -5.0)));
        let full = arc_points(rect, 90.0, -360.0);
        assert!(near(full[0], *full.last().unwrap()));
        assert!(full.len() > 60);
    }

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
    fn auto_size_labels_fit_their_expanded_text() {
        let xml = r#"<display><macros><camera>LI-DI-CAMERA-01</camera></macros>
            <widget type="label"><x>10</x><y>20</y><width>66</width><height>20</height>
              <text>$(camera)</text><auto_size>true</auto_size></widget>
            <widget type="label"><x>10</x><y>50</y><width>66</width><height>20</height>
              <text>$(camera)</text></widget></display>"#;
        let pvs = PvManager::new();
        let display = bob::Display::parse(xml).unwrap();
        let mut screen = Builder::new(&pvs).screen(display, &Macros::default(), None, 0);
        let ctx = egui::Context::default();
        style::install_fonts(&ctx);
        let mut natural = 0.0;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut requests = Vec::new();
            let mut cx = Cx {
                view: View {
                    origin: Pos2::ZERO,
                    zoom: 1.0,
                },
                clip: Rect::EVERYTHING,
                pvs: &pvs,
                requests: &mut requests,
                depth: 0,
                touch: false,
                trends: &[],
                window: WindowKey::Main,
            };
            draw_screen(ui, &mut screen, &mut cx, Id::new("auto_size"));
            let font = font(&screen.nodes[0].w, 1.0);
            natural = ui
                .painter()
                .layout_no_wrap("LI-DI-CAMERA-01".into(), font, Color32::BLACK)
                .size()
                .x;
        });
        out.textures_delta.clear();
        let [x, y, w, h] = screen.nodes[0].rect;
        assert_eq!((x, y), (10.0, 20.0));
        // Wide enough for the whole text, so nothing is elided.
        assert!(
            w > 66.0 && w >= natural as f64 && w < natural as f64 + 1.0,
            "{w} {natural}"
        );
        assert!(h > 0.0);
        // Without auto_size the label keeps its size.
        assert_eq!(screen.nodes[1].rect, [10.0, 50.0, 66.0, 20.0]);
    }

    /// Lay out a text entry's text `w` wide, as `text_entry` does.
    fn entry(text: &str, w: f32, editing: bool) -> Arc<egui::Galley> {
        let ctx = egui::Context::default();
        style::install_fonts(&ctx);
        let mut galley = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = FontId::new(14.0, egui::FontFamily::Name(style::BOLD.into()));
            let job = entry_job(text, font, Color32::BLACK, egui::Align::Min, w, editing);
            galley = Some(ui.painter().layout_job(job));
        });
        out.textures_delta.clear();
        galley.unwrap()
    }

    #[test]
    fn text_entry_elides_like_a_text_update_except_while_editing() {
        let shown = entry(PV, 100.0, false);
        assert_eq!(shown.rows.len(), 1);
        assert!(shown.elided);
        assert!(shown.rows[0].text().starts_with("<BL01T"));
        assert!(shown.rows[0].text().ends_with('…'));
        assert!(shown.size().x <= 100.0);
        // Editing keeps every character for the cursor.
        let editing = entry(PV, 100.0, true);
        assert!(!editing.elided);
        assert_eq!(editing.rows[0].text(), PV);
        // Text that fits is untouched.
        assert!(!entry("1.5", 100.0, false).elided);
    }

    #[test]
    fn combo_items_never_shrink_below_the_ui_font() {
        let ui = FontId::proportional(14.0);
        assert_eq!(combo_item_font(FontId::proportional(8.0), &ui).size, 14.0);
        assert_eq!(combo_item_font(FontId::proportional(28.0), &ui).size, 28.0);
        let mono = combo_item_font(FontId::monospace(8.0), &ui);
        assert_eq!(mono.family, egui::FontFamily::Monospace);
    }

    #[test]
    fn touch_mode_widens_the_hit_radius_and_back() {
        let ctx = egui::Context::default();
        touch_targets(&ctx, true);
        let radius = ctx.global_style().interaction.interact_radius;
        assert_eq!(radius, TOUCH_RADIUS);
        // A 20 point control is a full touch target.
        assert_eq!(20.0 + 2.0 * radius, TOUCH_TARGET);
        touch_targets(&ctx, false);
        assert_eq!(ctx.global_style().interaction.interact_radius, MOUSE_RADIUS);
    }

    #[test]
    fn web_displays_only_open_web_pages() {
        let web = Source::parse("http://h/opi/a.bob");
        let file = Source::parse("/opi/a.bob");
        assert!(webpage_allowed(Some(&web), "https://example.com/x"));
        assert!(!webpage_allowed(Some(&web), "/home/me/run.sh"));
        assert!(!webpage_allowed(Some(&web), "file:///etc/passwd"));
        assert!(!webpage_allowed(Some(&web), "ssh://h"));
        assert!(webpage_allowed(Some(&file), "/opi/manual.pdf"));
        assert!(webpage_allowed(None, "https://example.com"));
        assert!(!webpage_allowed(Some(&file), "--help"));
        assert!(!webpage_allowed(None, " "));
    }

    #[test]
    fn pv_names_are_shown_without_local_initialisers() {
        assert_eq!(plain_pv_name("loc://arr<VDoubleArray>(1,3,2)"), "loc://arr");
        assert_eq!(plain_pv_name("BL01T:X"), "BL01T:X");
    }

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
    fn string_format_shows_char_arrays_as_text() {
        let chars = |codes: &[i64]| PvState {
            connected: true,
            value: Value::LongArray(Arc::from(codes)),
            ..Default::default()
        };
        let path = chars(&[47, 116, 109, 112, 0, 120, 121]);
        let string = widget("<format>6</format>");
        assert!(writes_long_string(&string));
        assert_eq!(format_value(&string, &path), "/tmp");
        assert_eq!(format_value(&string, &chars(&[104, 105])), "hi");
        // Other formats, and values that are not arrays of numbers, are as before.
        let default = widget("");
        assert!(!writes_long_string(&default));
        assert_eq!(
            format_value(&default, &path),
            "[47, 116, 109, 112, 0, 120, 121]"
        );
        let text = PvState {
            connected: true,
            value: Value::Str("a b".into()),
            ..Default::default()
        };
        assert_eq!(format_value(&string, &text), "a b");
        // A number shows as the character of its low byte, as in Phoebus,
        // and NUL as nothing.
        assert_eq!(format_value(&string, &state(65, true)), "A");
        assert_eq!(format_value(&string, &state(0x141, true)), "A");
        assert_eq!(format_value(&string, &state(0, true)), "");
        let double = PvState {
            connected: true,
            value: Value::Double(104.7),
            units: "mm".into(),
            ..Default::default()
        };
        assert_eq!(format_value(&string, &double), "h mm");
    }

    #[test]
    fn label_defaults_to_label_only_when_absent() {
        assert_eq!(raw_label(&widget("")), "Label");
        assert_eq!(raw_label(&widget("<label></label>")), "");
        assert_eq!(raw_label(&widget("<label>Go</label>")), "Go");
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
    fn open_display_files_use_the_action_macros() {
        let xml = r#"<display><widget type="symbol"><name>S</name>
            <macros><D>sub</D></macros>
            <actions><action type="open_display"><file>$(IOC)/$(P).bob</file>
            <macros><P>BL01C-MO-01</P><IOC>../$(D)/ioc</IOC></macros>
            </action></actions></widget></display>"#;
        let pvs = PvManager::new();
        let display = bob::Display::parse(xml).unwrap();
        let screen = Builder::new(&pvs).screen(display, &Macros::default(), None, 0);
        let node = &screen.nodes[0];
        let mut requests = Vec::new();
        let mut cx = Cx {
            view: View {
                origin: Pos2::ZERO,
                zoom: 1.0,
            },
            clip: Rect::EVERYTHING,
            pvs: &pvs,
            requests: &mut requests,
            depth: 0,
            touch: false,
            trends: &[],
            window: WindowKey::Main,
        };
        run_action(node, &node.w.actions()[0], &mut cx);
        let Some(Request::Open { source, macros, .. }) = requests.first() else {
            panic!("no display opened")
        };
        assert_eq!(*source, Source::parse("../sub/ioc/BL01C-MO-01.bob"));
        assert_eq!(macros.get("P"), Some("BL01C-MO-01"));
    }

    #[test]
    fn text_entries_and_sliders_take_no_space_below_the_display() {
        // Each used to take the space from the cursor, below the display, so
        // a display with many of them could scroll past its bottom.
        let mut xml = String::from("<display><width>300</width><height>100</height>");
        for (i, kind) in ["textentry", "spinner", "scaledslider"].iter().enumerate() {
            for j in 0..4 {
                xml += &format!(
                    r#"<widget type="{kind}"><x>{}</x><y>{}</y><width>60</width><height>20</height></widget>"#,
                    70 * j,
                    30 * i
                );
            }
        }
        xml += "</display>";
        let pvs = PvManager::new();
        let display = bob::Display::parse(&xml).unwrap();
        let mut screen = Builder::new(&pvs).screen(display, &Macros::default(), None, 0);
        let ctx = egui::Context::default();
        let mut used = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let size = Vec2::new(screen.width as f32, screen.height as f32);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let mut requests = Vec::new();
            let mut cx = Cx {
                view: View {
                    origin: rect.min,
                    zoom: 1.0,
                },
                clip: rect,
                pvs: &pvs,
                requests: &mut requests,
                depth: 0,
                touch: false,
                trends: &[],
                window: WindowKey::Main,
            };
            draw_screen(ui, &mut screen, &mut cx, Id::new("display"));
            used = Some((rect, ui.min_rect()));
        });
        out.textures_delta.clear();
        let (rect, used) = used.unwrap();
        assert_eq!(used, rect);
    }

    #[test]
    fn trend_actions_and_strip_tool_commands_open_trends() {
        let xml = r#"<display><macros><dom>BL01T</dom></macros>
            <widget type="action_button"><actions>
            <action type="trend"><pv_name>$(dom)-VA-GAUGE-01:PLOG</pv_name><pv_name>$(dom)-VA-IMG-01:PLOG</pv_name></action>
            <action type="command"><command>strip.py $(dom)-VA-GAUGE-02:PLOG log_mbar</command></action>
            <action type="execute"><script file="EmbeddedPy"><text>ScriptUtil.executeEclipseCommand("org.csstudio.trends.databrowser2.newWindow", ['pvnames', '$(dom):X'])</text></script></action>
            </actions></widget></display>"#;
        let pvs = PvManager::new();
        let display = bob::Display::parse(xml).unwrap();
        let screen = Builder::new(&pvs).screen(display, &Macros::default(), None, 0);
        let node = &screen.nodes[0];
        let mut requests = Vec::new();
        let mut cx = Cx {
            view: View {
                origin: Pos2::ZERO,
                zoom: 1.0,
            },
            clip: Rect::EVERYTHING,
            pvs: &pvs,
            requests: &mut requests,
            depth: 0,
            touch: false,
            trends: &[],
            window: WindowKey::Main,
        };
        for a in node.w.actions() {
            run_action(node, &a, &mut cx);
        }
        let trends: Vec<&Vec<String>> = requests
            .iter()
            .filter_map(|r| match r {
                Request::Trend(names) => Some(names),
                _ => None,
            })
            .collect();
        assert_eq!(
            trends,
            [
                &vec!["BL01T-VA-GAUGE-01:PLOG", "BL01T-VA-IMG-01:PLOG"],
                &vec!["BL01T-VA-GAUGE-02:PLOG"],
                &vec!["BL01T:X"]
            ]
        );
    }

    #[test]
    fn writes_from_web_displays_ask_first_but_local_ones_write() {
        let xml = r#"<display><widget type="action_button"><pv_name>loc://confirm_t(0)</pv_name><actions>
            <action type="write_pv"><pv_name>loc://confirm_t(0)</pv_name><value>5</value></action>
            <action type="execute"><script file="EmbeddedPy"><text>pv = ScriptUtil.getPrimaryPV(widget)
var pv_value = PVUtil.getDouble(pv);
pv_value = (pv_value == 0) ? 1 : 0;
pv.setValue(pv_value);
</text></script></action>
            </actions></widget></display>"#;
        let run = |source: &str| {
            let pvs = PvManager::new();
            let mut display = bob::Display::parse(xml).unwrap();
            display.source = Some(Source::parse(source));
            let screen = Builder::new(&pvs).screen(display, &Macros::default(), None, 0);
            let node = &screen.nodes[0];
            let pv = node.pv.clone().unwrap();
            let mut requests = Vec::new();
            let mut cx = Cx {
                view: View {
                    origin: Pos2::ZERO,
                    zoom: 1.0,
                },
                clip: Rect::EVERYTHING,
                pvs: &pvs,
                requests: &mut requests,
                depth: 0,
                touch: false,
                trends: &[],
                window: WindowKey::Main,
            };
            for a in node.w.actions() {
                run_action(node, &a, &mut cx);
            }
            (pv.state().value.as_f64(), requests)
        };
        // From the web: nothing written, both actions ask.
        let (value, requests) = run("https://host/a.bob");
        assert_eq!(value, Some(0.0));
        let asked: Vec<&str> = requests
            .iter()
            .filter_map(|r| match r {
                Request::ConfirmWrite { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(asked, ["5", "1"]);
        // A local file: written at once (the toggle then flips 5 to 0).
        let (value, requests) = run("/tmp/a.bob");
        assert!(requests.is_empty());
        assert_eq!(value, Some(0.0));
    }

    #[test]
    fn recognises_the_techui_toggle_script() {
        let script = "/* Embedded javascript */
importClass(org.csstudio.display.builder.runtime.script.PVUtil);
importClass(org.csstudio.display.builder.runtime.script.ScriptUtil);
pv = ScriptUtil.getPrimaryPV(widget)
var pv_value = PVUtil.getDouble(pv);
pv_value = (pv_value == 0) ? 1 : 0;
pv.setValue(pv_value);
";
        assert!(is_toggle_script(script));
        assert!(is_toggle_script(&script.replace('\n', "\n        ")));
        assert!(!is_toggle_script(&script.replace("? 1 : 0", "? 2 : 0")));
        assert!(!is_toggle_script(""));
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
    fn bit_defaults_follow_phoebus() {
        let kind = |k: &str| {
            let xml = format!(r#"<display><widget type="{k}"/></display>"#);
            bit(&bob::Display::parse(&xml).unwrap().widgets[0])
        };
        assert_eq!(kind("bool_button"), 0);
        assert_eq!(kind("checkbox"), 0);
        assert_eq!(kind("slide_button"), 0);
        assert_eq!(kind("led"), -1);
    }

    #[test]
    fn slide_track_fits_small_widget() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(20.0));
        let (track, label) = slide_layout(rect, 1.0, false);
        assert!(label.is_none());
        assert!(rect.contains_rect(track));
        assert!(track.width() > track.height());
        assert_eq!(track.center().y, rect.center().y);
        assert_eq!(track.left(), rect.left() + 1.0);
    }

    #[test]
    fn slide_track_without_label_stays_at_the_left() {
        // Phoebus: HBox(button, label) with the label growing, so an empty
        // label still leaves the switch at the left.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(80.0, 20.0));
        let (track, label) = slide_layout(rect, 1.0, false);
        assert!(label.is_none());
        assert_eq!(track.left(), rect.left() + 1.0);
    }

    #[test]
    fn slide_track_left_of_label() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 30.0));
        let (track, label) = slide_layout(rect, 1.0, true);
        let label = label.unwrap();
        assert!(rect.contains_rect(track));
        assert!(track.right() < label.left());
        assert_eq!(label.right(), rect.right());
        assert_eq!(track.height(), 16.0);
    }

    #[test]
    fn slider_is_centred_in_its_box() {
        // sr/pinger.opi: a tall slider with no scale sits in the middle of
        // its box, not at the top.
        let rect = Rect::from_min_size(Pos2::new(64.0, 3.0), Vec2::new(384.0, 76.0));
        let lay = slider_layout(rect, true, 1.0, None, None);
        assert_eq!(lay.band.center().y, rect.center().y);
        assert!(lay.scale.is_none());
        assert!(rect.contains_rect(lay.band));
        assert!(lay.from - lay.thumb / 2.0 >= rect.left());
        assert!(lay.to + lay.thumb / 2.0 <= rect.right());
    }

    #[test]
    fn slider_scale_sits_beside_the_thumb() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(384.0, 76.0));
        let lay = slider_layout(rect, true, 1.0, Some((14.0, 20.0)), None);
        let scale = lay.scale.unwrap();
        assert!(lay.labels);
        assert_eq!(scale.top(), lay.band.bottom());
        assert!(rect.contains_rect(scale) && rect.contains_rect(lay.band));
        // Room at the ends for the first and last labels.
        assert!(lay.from >= 20.0 && lay.to <= 364.0);
        // The two together are centred.
        assert_eq!((lay.band.top() + scale.bottom()) / 2.0, rect.center().y);
        // Vertical: the minimum at the bottom, the scale to the right.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(76.0, 384.0));
        let lay = slider_layout(rect, false, 1.0, Some((30.0, 7.0)), None);
        assert!(lay.from > lay.to);
        assert_eq!(lay.scale.unwrap().left(), lay.band.right());
    }

    #[test]
    fn small_slider_drops_labels_before_its_thumb() {
        // cameraControl-gige.opi: 168x24, no room for labels under the thumb.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(168.0, 24.0));
        let lay = slider_layout(rect, true, 1.0, Some((14.0, 20.0)), None);
        assert!(!lay.labels);
        assert!(lay.scale.is_some());
        assert!(lay.band.height() >= 8.0);
        assert!(rect.contains_rect(lay.band) && rect.contains_rect(lay.scale.unwrap()));
        // Too narrow for any travel: the thumb stays in the middle.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(6.0, 24.0));
        let lay = slider_layout(rect, true, 1.0, None, None);
        assert_eq!((lay.from, lay.to), (3.0, 3.0));
        // Markers go before the scale's labels.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(168.0, 40.0));
        let lay = slider_layout(rect, true, 1.0, Some((14.0, 20.0)), Some(14.0));
        assert!(lay.markers.is_none() && lay.labels);
    }

    #[test]
    fn slider_markers_sit_on_the_near_side() {
        // sr/pinger.opi: markers above the thumb and the scale below it,
        // the three centred, so the thumb is clear of the title label that
        // covers the top 29px of the box.
        let rect = Rect::from_min_size(Pos2::new(64.0, 3.0), Vec2::new(384.0, 76.0));
        let lay = slider_layout(rect, true, 1.0, Some((14.0, 20.0)), Some(14.0));
        let (markers, scale) = (lay.markers.unwrap(), lay.scale.unwrap());
        assert_eq!(markers.bottom(), lay.band.top());
        assert_eq!(scale.top(), lay.band.bottom());
        assert!(rect.contains_rect(markers) && rect.contains_rect(scale));
        assert_eq!(lay.band.center().y, rect.center().y);
        assert!(lay.band.top() > rect.top() + 29.0);
        // Vertical: markers to the left.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 300.0));
        let lay = slider_layout(rect, false, 1.0, Some((40.0, 7.0)), Some(36.0));
        assert_eq!(lay.markers.unwrap().right(), lay.band.left());
    }
}
