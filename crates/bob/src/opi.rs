//! Legacy CS-Studio BOY `.opi` displays.
//!
//! An `.opi` is translated, element for element, into the `.bob` form the
//! rest of this crate reads: BOY widget types become Phoebus ones, and their
//! properties are renamed and converted as Phoebus's own legacy import does
//! it (in its widgets' `CustomConfigurator`s), so nothing downstream needs to
//! know where a display came from. Scripts are not translated, except the
//! DLS `get_pv_name.py` widget script (see [`crate::group_link`]) and the
//! scripts that choose a linking container's display (see
//! [`crate::opi_file_script`]); script actions stay as unsupported actions unless [`action`] recognises them.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::Element;

const PREFIX: &str = "org.csstudio.opibuilder.widgets.";

/// BOY font heights not given in pixels are points, which CS-Studio draws
/// at the 96 DPI of a Linux desktop, 4/3 of a pixel each. egui's text
/// looks a little larger than SWT's at the same size, so a point is 1.27
/// pixels here: side by side with CS-Studio, the text matches. (Phoebus
/// divides points by its `legacy_font_calibration`, 1.01 by default,
/// which leaves these screens' text far smaller than CS-Studio showed it.)
const PIXELS_PER_POINT: f64 = 1.27;

/// Fonts Phoebus defines itself; a BOY font with one of these names uses
/// the Phoebus definition rather than the height in the file.
const NAMED_FONTS: &[&str] = &[
    "Default",
    "Default Bold",
    "Header 1",
    "Header 2",
    "Header 3",
    "Comment",
    "Fine Print",
];

/// Properties that mean nothing to Phoebus, or would be misread.
const NOISE: &[&str] = &[
    "wuid",
    "widget_type",
    "scale_options",
    "boy_version",
    "pv_value",
    "scripts",
];

/// Whether `root` is a BOY display: it names a `typeId`, or its version is
/// below 2 (as Phoebus decides).
pub(crate) fn is_opi(root: &Element) -> bool {
    root.name == "display"
        && (root.attr("typeId").is_some()
            || root
                .attr("version")
                .and_then(|v| v.split('.').next()?.trim().parse::<u32>().ok())
                .is_some_and(|major| major < 2))
}

/// The `.bob` form of the BOY display `root`.
pub(crate) fn translate(mut root: Element) -> Element {
    normalize(&mut root);
    let mut display = Element::new("display").with_attr("version", "2.0.0");
    for child in root.children {
        match child.name.as_str() {
            "widget" => display.children.extend(widget(child)),
            "x" | "y" | "actions" | "rules" | "scripts" => {}
            n if NOISE.contains(&n) => {}
            _ => display.children.push(child),
        }
    }
    display
}

/// Convert colours, fonts and macros everywhere below `el`.
fn normalize(el: &mut Element) {
    if el.name == "color" {
        fill_color(el);
        return;
    }
    if el.name == "macros" {
        // Phoebus always inherits the parent's macros.
        el.children.retain(|c| c.name != "include_parent_macros");
    }
    for c in &mut el.children {
        if c.name == "opifont.name" || c.name == "fontdata" {
            *c = font(c);
        } else {
            normalize(c);
        }
    }
}

/// Give a colour that has only a name its RGB values, from the EDM colour
/// list; one it does not know is left to the Phoebus palette.
fn fill_color(el: &mut Element) {
    if el.attr("red").is_some() {
        return;
    }
    let Some([r, g, b]) = el.attr("name").and_then(edm_color) else {
        return;
    };
    el.set_attr("red", r.to_string());
    el.set_attr("green", g.to_string());
    el.set_attr("blue", b.to_string());
}

/// `<opifont.name>` or `<fontdata>` as a `.bob` `<font>`.
fn font(el: &Element) -> Element {
    let style = match el.attr("style").and_then(|s| s.trim().parse::<i32>().ok()) {
        Some(1) => "BOLD",
        Some(2) => "ITALIC",
        Some(3) => "BOLD_ITALIC",
        _ => "REGULAR",
    };
    let height = el.attr("height").and_then(|h| h.trim().parse::<f64>().ok());
    let mut out = Element::new("font");
    if el.name == "opifont.name" {
        let name = el.text.trim();
        if NAMED_FONTS.contains(&name) {
            return out.with_attr("name", name);
        }
        if !name.is_empty() {
            out.set_attr("name", name);
        }
    }
    let size = if el.attr("pixels") == Some("true") {
        height
    } else {
        height.map(|h| h * PIXELS_PER_POINT)
    };
    out.with_attr("family", el.attr("fontName").unwrap_or("Liberation Sans"))
        .with_attr("style", style)
        .with_attr("size", format!("{:.2}", size.unwrap_or(14.0)))
}

/// The EDM colour list DLS used to convert EDM screens: name → RGB.
static EDM_COLORS: LazyLock<HashMap<String, [u8; 3]>> =
    LazyLock::new(|| parse_colors_list(include_str!("../colors.list")));

fn edm_color(name: &str) -> Option<[u8; 3]> {
    EDM_COLORS.get(name).copied().or_else(|| {
        EDM_COLORS
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    })
}

/// Read the `static` colours and `alias`es of an EDM `colors.list`; rules,
/// menus and alarm settings are ignored. Values are 16 bit; a blinking
/// colour's second set of values is ignored.
fn parse_colors_list(text: &str) -> HashMap<String, [u8; 3]> {
    let mut colors = HashMap::new();
    let mut aliases = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let (words, rest) = words(line);
        match words.first().map(String::as_str) {
            Some("static") if words.len() >= 3 => {
                let values: Vec<u32> = rest
                    .split_once('{')
                    .and_then(|(_, v)| v.split_once('}'))
                    .map(|(v, _)| {
                        v.split_whitespace()
                            .filter_map(|n| n.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                if let [r, g, b, ..] = values[..] {
                    let c = |v: u32| (v.min(0xffff) >> 8) as u8;
                    colors.insert(words[2].clone(), [c(r), c(g), c(b)]);
                }
            }
            Some("alias") if words.len() >= 3 => aliases.push((words[1].clone(), words[2].clone())),
            _ => {}
        }
    }
    for (alias, name) in aliases {
        if let Some(c) = colors.get(&name).copied() {
            colors.entry(alias).or_insert(c);
        }
    }
    colors
}

/// The words of a `colors.list` line up to a `{` or `#`, with `"quoted
/// words"` kept whole, and the rest of the line from the `{`.
fn words(line: &str) -> (Vec<String>, &str) {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        match c {
            '{' => return (out, &line[i..]),
            '#' => break,
            '"' => {
                chars.next();
                let word: String = chars
                    .by_ref()
                    .map(|(_, c)| c)
                    .take_while(|&c| c != '"')
                    .collect();
                out.push(word);
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let mut word = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_whitespace() || c == '{' || c == '#' {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                out.push(word);
            }
        }
    }
    (out, "")
}

// --- widgets -----------------------------------------------------------------

/// Text of the property `name`, trimmed.
fn text<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.child_text(name).map(str::trim)
}

fn num(el: &Element, name: &str) -> Option<f64> {
    text(el, name)?.parse().ok()
}

fn flag(el: &Element, name: &str) -> Option<bool> {
    text(el, name).map(|t| t == "true")
}

fn set(el: &mut Element, name: &str, value: impl ToString) {
    el.set_child(Element::with_text(name, value.to_string()));
}

/// Set `name` unless the widget gives it (BOY defaults that Phoebus's differ from).
fn default(el: &mut Element, name: &str, value: &str) {
    if el.child(name).is_none() {
        set(el, name, value);
    }
}

fn remove(el: &mut Element, names: &[&str]) {
    el.children.retain(|c| !names.contains(&c.name.as_str()));
}

/// Rename property `from` to `to`, replacing any `to` there was; rules
/// that set it follow.
fn rename(el: &mut Element, from: &str, to: &str) {
    if let Some(mut c) = el.take_child(from) {
        el.take_child(to);
        c.name = to.to_string();
        el.children.push(c);
    }
    rename_rules(el, from, to);
}

/// Rules that set property `from` set `to` instead, whether or not the
/// widget sets `from` itself.
fn rename_rules(el: &mut Element, from: &str, to: &str) {
    for rule in el
        .child_mut("rules")
        .into_iter()
        .flat_map(|r| &mut r.children)
    {
        if rule.name == "rule" && rule.attr("prop_id") == Some(from) {
            rule.set_attr("prop_id", to);
        }
    }
}

/// Add `d` to the number in property `name`.
fn add(el: &mut Element, name: &str, d: f64) {
    let v = num(el, name).unwrap_or(0.0) + d;
    set(el, name, v);
}

/// Properties renamed for a widget kind (see [`rename`]).
fn renames(short: &str, kind: &str) -> &'static [(&'static str, &'static str)] {
    match (short, kind) {
        ("polyline", _) => &[("background_color", "line_color")],
        ("arc", _) => &[("foreground_color", "line_color")],
        (_, "led" | "multi_state_led") => &[
            ("bulb_border_color", "line_color"),
            ("square_led", "square"),
        ],
        (_, "byte_monitor") => &[("square_led", "square")],
        (_, "meter") => &[
            ("scale_font", "font"),
            ("show_value_label", "show_value"),
            ("level_lo", "level_low"),
            ("level_hi", "level_high"),
            ("color_lolo", "major_warning_color"),
            ("color_lo", "minor_warning_color"),
        ],
        (_, "textentry") => &[("multiline_input", "multi_line")],
        (_, "progressbar") => &[("color_fillbackground", "background_color")],
        (_, "picture") => &[("image_file", "file"), ("stretch_to_fit", "stretch_image")],
        _ => &[],
    }
}

/// The Phoebus widget type for BOY `short` typeId (prefix removed).
fn kind(short: &str, el: &Element) -> String {
    let k = match short {
        "Rectangle" | "RoundedRectangle" => "rectangle",
        "Ellipse" => "ellipse",
        "groupingContainer" => "group",
        "Label" => "label",
        "edm.symbolwidget"
        | "symbol.multistate.MultistateMonitorWidget"
        | "symbol.bool.BoolMonitorWidget"
        | "ImageBoolIndicator" => "symbol",
        "TextUpdate" | "TextInput" | "NativeText" if is_static_text(el) => "label",
        "TextUpdate" => "textupdate",
        "TextInput" | "NativeText" => "textentry",
        "polyline" => "polyline",
        "polygon" => "polygon",
        "arc" => "arc",
        "ActionButton" | "NativeButton" => "action_button",
        "linkingContainer" => "embedded",
        "bytemonitor" => "byte_monitor",
        "progressbar" => "progressbar",
        "MenuButton" if menu_is_combo(el) => "combo",
        "MenuButton" => "action_button",
        "LED" if el.child("state_count").is_some() => "multi_state_led",
        "LED" => "led",
        "choiceButton" => "choice",
        "radioBox" => "radio",
        "BoolButton" if is_message_button(el) => "action_button",
        "BoolButton" | "ImageBoolButton" => "bool_button",
        "BoolSwitch" => "slide_button",
        "checkbox" => "checkbox",
        "combo" => "combo",
        "spinner" => "spinner",
        "scrollbar" => "scrollbar",
        "tank" => "tank",
        "thermometer" => "thermometer",
        "meter" | "gauge" => "meter",
        "scaledslider" | "knob" => "scaledslider",
        "Image" => "picture",
        "detailpanel" => "group",
        "intensityGraph" => "image",
        "xyGraph" => "xyplot",
        "tab" => "tabs",
        "edm.menumux" => "combo",
        // Not drawn: a placeholder shows the BOY name.
        other => other.rsplit('.').next().unwrap_or(other),
    };
    k.to_string()
}

/// A text update or input with text but no PV is a label.
fn is_static_text(el: &Element) -> bool {
    let has_text = text(el, "text").is_some_and(|t| !t.is_empty());
    let no_pv = text(el, "pv_name").is_none_or(str::is_empty);
    let pv_rule = el.child("rules").is_some_and(|r| {
        r.children
            .iter()
            .any(|rule| rule.attr("prop_id") == Some("pv_name"))
    });
    has_text && no_pv && !pv_rule
}

/// Phoebus `shouldUseCombo`: a menu button that takes its items from its PV,
/// or has no actions, is a combo.
fn menu_is_combo(el: &Element) -> bool {
    flag(el, "actions_from_pv").unwrap_or(true) || action_count(el) == 0
}

/// An EDM message button: a bool button with no PV that writes with its actions.
fn is_message_button(el: &Element) -> bool {
    text(el, "pv_name").is_none_or(str::is_empty) && action_count(el) > 0
}

fn action_count(el: &Element) -> usize {
    el.child("actions")
        .map(|a| a.children.iter().filter(|c| c.name == "action").count())
        .unwrap_or(0)
}

/// Widgets with no PV of their own: a BOY `pv_name` becomes a macro.
fn pv_less(kind: &str) -> bool {
    matches!(
        kind,
        "label"
            | "rectangle"
            | "ellipse"
            | "arc"
            | "polyline"
            | "polygon"
            | "group"
            | "picture"
            | "embedded"
    )
}

/// A BOY `pv_name` as the widget's `pv_name` macro, unless empty.
fn pv_macro(el: &mut Element, pv: Element) {
    if pv.text.is_empty() {
        return;
    }
    // As Phoebus `MacroWidget.importPVName`: rules and tooltips use it.
    let mut macros = el
        .take_child("macros")
        .unwrap_or_else(|| Element::new("macros"));
    macros.children.insert(0, pv);
    el.children.push(macros);
}

/// Widgets that run their actions themselves, so BOY's click hooks don't apply.
fn is_button(kind: &str) -> bool {
    matches!(kind, "action_button" | "bool_button" | "combo" | "choice")
}

/// Translate one BOY `<widget>`; some become two (a progress bar and its
/// label) or are wrapped (a linking container in a group box).
fn widget(mut el: Element) -> Vec<Element> {
    let type_id = el.attr("typeId").unwrap_or("").to_string();
    let short = match type_id.strip_prefix(PREFIX).unwrap_or(&type_id) {
        // DAWN's XY and intensity graphs have the BOY ones' properties, and
        // Phoebus reads them as those.
        "dawn.xygraph" => "xyGraph",
        "dawn.intensitygraph" => "intensityGraph",
        s => s,
    }
    .to_string();
    let (kids, props): (Vec<Element>, Vec<Element>) =
        el.children.drain(..).partition(|c| c.name == "widget");
    el.children = props;
    // Read before the scripts and widget type are dropped as noise.
    let group_link = crate::group_link::from_opi(&el);
    let opi_file = (short == "linkingContainer")
        .then(|| crate::opi_file_script::from_opi(&el))
        .flatten();
    let shown_by = visible_scripts(&mut el);
    // Kept for `has_scripts` unless bobview runs them as a group link, or
    // knows the display they choose.
    let scripts = el
        .child("scripts")
        .filter(|s| group_link.is_none() && opi_file.is_none() && !s.children.is_empty())
        .cloned();
    if let Some((file, macros)) = opi_file {
        el.set_child(Element::with_text("opi_file", file));
        // The script's macros replace the container's own.
        if let Some(macros) = macros {
            el.set_child(macros);
        }
    }
    remove(&mut el, NOISE);
    el.children.extend(group_link);
    el.children.extend(scripts);
    let mut children: Vec<Element> = kids.into_iter().flat_map(widget).collect();

    alarm_sensitive(&mut el);
    let kind = kind(&short, &el);
    for (from, to) in renames(&short, &kind) {
        rename(&mut el, from, to);
    }
    rules(&mut el);
    if !shown_by.is_empty() {
        // As the script's first line: hidden until its PV says otherwise.
        set(&mut el, "visible", false);
        let rules = match el.child_mut("rules") {
            Some(r) => r,
            None => {
                el.children.push(Element::new("rules"));
                el.children.last_mut().unwrap()
            }
        };
        rules
            .children
            .extend(shown_by.into_iter().flatten().map(visible_rule));
    }
    actions(&mut el, &kind);
    if pv_less(&kind)
        && let Some(pv) = el.take_child("pv_name")
    {
        pv_macro(&mut el, pv);
    }

    let mut extra = Vec::new();
    match (short.as_str(), kind.as_str()) {
        (_, "rectangle" | "ellipse") => shape(&mut el),
        ("xyGraph", _) => xy_graph(&mut el),
        ("tab", _) => {
            tabs(&mut el, std::mem::take(&mut children));
            border(&mut el);
        }
        ("detailpanel", _) => {
            children = detail_rows(&el, children);
            remove(&mut el, &["border_style", "border_width", "border_color"]);
            set(&mut el, "style", 3);
            set(&mut el, "transparent", true);
        }
        (_, "group") => group(&mut el),
        (_, "label") => {
            default(&mut el, "transparent", "true");
            default(&mut el, "wrap_words", "false");
            default(&mut el, "vertical_alignment", "1");
            rotation(&mut el);
            border(&mut el);
        }
        (_, "textupdate" | "textentry") => {
            format(&mut el);
            default(&mut el, "wrap_words", "false");
            rotation(&mut el);
            border(&mut el);
            if kind == "textentry" && flag(&el, "transparent") == Some(true) {
                transparent_background(&mut el);
            }
        }
        (_, "polyline" | "polygon") => points(&mut el),
        ("arc", _) => {
            if let Some(fill) = flag(&el, "fill") {
                set(&mut el, "transparent", !fill);
            }
            remove(&mut el, &["border_style", "border_width", "border_color"]);
        }
        ("edm.symbolwidget", _) => {
            if let Some(file) = el.take_child("image_file") {
                el.children.push(symbols(&[file.text]));
            }
            set(&mut el, "preserve_ratio", false);
            // BOY drew frame 0 of a disconnected symbol with only the
            // alarm border, not Phoebus' overlay.
            let clear = Element::new("color")
                .with_attr("red", "0")
                .with_attr("green", "0")
                .with_attr("blue", "0")
                .with_attr("alpha", "0");
            el.set_child(Element::new("disconnect_overlay_color").with_child(clear));
        }
        ("ImageBoolIndicator", _) => {
            let files: Vec<String> = ["off_image", "on_image"]
                .iter()
                .filter_map(|n| text(&el, n))
                .filter(|f| !f.is_empty())
                .map(str::to_string)
                .collect();
            el.children.push(symbols(&files));
            stretch(&mut el);
        }
        (_, "symbol") => {
            // A multi-state symbol names one state's image, e.g. "ionp 4.svg",
            // of a numbered set.
            if let Some(file) = el.take_child("image_file") {
                let files = numbered(&file.text);
                if files.len() > 1 {
                    // A set has fewer images than the 16 listed: BOY drew
                    // nothing for a state without one.
                    set(&mut el, "fallback_symbol", "");
                }
                el.children.push(symbols(&files));
            }
            stretch(&mut el);
        }
        (_, "action_button") if short == "MenuButton" => rename(&mut el, "label", "text"),
        (_, "action_button") if short == "BoolButton" => {
            // Writes with its push action, labelled as the button was.
            let label = text(&el, "off_label").unwrap_or("").to_string();
            set(&mut el, "text", label);
            push_first(&mut el, "push_action_index");
            if let Some(a) = el.child_mut("actions") {
                a.children.truncate(1);
            }
        }
        (_, "action_button") => {
            push_first(&mut el, "push_action_index");
            if let Some(t) = el.child_mut("tooltip") {
                t.text = t.text.replace("$(pv_value)", "$(actions)");
            }
        }
        (_, "combo") if short == "edm.menumux" => menu_mux(&mut el),
        (_, "combo") if short == "MenuButton" => {
            let from_pv = flag(&el, "actions_from_pv").unwrap_or(true);
            set(&mut el, "items_from_pv", from_pv);
        }
        (_, "embedded") => {
            if let Some(wrapper) = embedded(&mut el, &mut children) {
                return vec![wrapper];
            }
        }
        (_, "byte_monitor") => {
            if let Some(mut labels) = el.take_child("label") {
                labels.name = "labels".into();
                labels
                    .children
                    .iter_mut()
                    .for_each(|s| s.name = "text".into());
                el.children.push(labels);
            }
        }
        (_, "progressbar") => {
            if flag(&el, "show_markers").unwrap_or(true) {
                add(&mut el, "y", 25.0);
                add(&mut el, "height", -25.0);
            }
            if flag(&el, "show_label").unwrap_or(true) {
                extra.push(progress_label(&el));
            }
        }
        (_, "meter") => {
            // BOY's ramp is green between the low and high levels.
            if el.child("normal_status_color").is_none() {
                let green = Element::new("color")
                    .with_attr("red", "0")
                    .with_attr("green", "255")
                    .with_attr("blue", "0");
                el.children
                    .push(Element::new("normal_status_color").with_child(green));
            }
            remove(&mut el, &["color_hi", "color_hihi"]);
        }
        (_, "led") => {
            led_border(&mut el);
            if flag(&el, "show_boolean_label") == Some(false) {
                set(&mut el, "on_label", "");
                set(&mut el, "off_label", "");
            }
        }
        (_, "multi_state_led") => {
            led_border(&mut el);
            let labels = flag(&el, "show_boolean_label") != Some(false);
            led_states(&mut el, labels);
            if !labels {
                set(&mut el, "fallback_label", "");
            }
        }
        (_, "choice" | "radio" | "combo") => items(&mut el),
        (_, "bool_button") => {
            if flag(&el, "toggle_button") == Some(false) {
                set(&mut el, "mode", 1);
            }
            if flag(&el, "show_boolean_label") == Some(false) {
                set(&mut el, "on_label", "");
                set(&mut el, "off_label", "");
            }
        }
        _ => {}
    }

    let mut out = Element::new("widget").with_attr("type", kind);
    out.children = el.children;
    out.children.extend(children);
    let mut all = vec![out];
    all.extend(extra);
    all
}

/// The most axes, traces or LED states a widget may have: more than BOY
/// offers.
const MAX_COUNT: f64 = 64.0;

/// An XY graph: BOY's numbered `axis_N_*` and `trace_N_*` properties as
/// Phoebus' `x_axis`, `y_axes` and `traces`. Axis 0 is the x axis; further
/// x axes are dropped.
fn xy_graph(el: &mut Element) {
    let take = |el: &mut Element, prefix: &str, props: &[(&str, &str)]| {
        let mut out = Vec::new();
        for (from, to) in props {
            if let Some(mut c) = el.take_child(&format!("{prefix}{from}")) {
                c.name = to.to_string();
                out.push(c);
            }
        }
        out
    };
    const AXIS: &[(&str, &str)] = &[
        ("axis_title", "title"),
        ("auto_scale", "autoscale"),
        ("log_scale", "log_scale"),
        ("minimum", "minimum"),
        ("maximum", "maximum"),
        ("show_grid", "show_grid"),
        ("axis_color", "color"),
        ("title_font", "title_font"),
        ("scale_font", "scale_font"),
        ("visible", "visible"),
    ];
    const TRACE: &[(&str, &str)] = &[
        ("name", "name"),
        ("x_pv", "x_pv"),
        ("y_pv", "y_pv"),
        ("trace_color", "color"),
        ("line_width", "line_width"),
        ("point_size", "point_size"),
        ("visible", "visible"),
        // BOY plots the last `buffer_size` updates when concatenating;
        // Phoebus has no such properties, so only bobview reads these.
        ("concatenate_data", "concatenate_data"),
        ("buffer_size", "buffer_size"),
        ("update_mode", "update_mode"),
    ];
    // Counts are bounded (NaN is 0 as usize), so a bad file can't hang us.
    let axes = num(el, "axis_count").unwrap_or(2.0).clamp(1.0, MAX_COUNT) as usize;
    let mut x_axis = Element::new("x_axis");
    x_axis.children = take(el, "axis_0_", AXIS);
    // BOY axis index -> Phoebus y axis index.
    let mut y_index = HashMap::new();
    let mut y_axes = Element::new("y_axes");
    for i in 1..axes {
        let p = format!("axis_{i}_");
        let is_y = i == 1 || flag(el, &format!("{p}y_axis")) == Some(true);
        let on_right = flag(el, &format!("{p}left_bottom_side")) == Some(false);
        let mut axis = Element::new("y_axis");
        axis.children = take(el, &p, AXIS);
        if is_y {
            set(&mut axis, "on_right", on_right);
            y_index.insert(i, y_axes.children.len());
            y_axes.children.push(axis);
        }
    }
    let count = num(el, "trace_count").unwrap_or(1.0).clamp(0.0, MAX_COUNT) as usize;
    let mut traces = Element::new("traces");
    for i in 0..count {
        let p = format!("trace_{i}_");
        let boy_axis = num(el, &format!("{p}y_axis_index")).unwrap_or(1.0) as usize;
        // BOY: 0 solid line, 1 dashed, 2 points, 3 bars, 4 area, 5 and 6
        // steps, 7 to 9 dash-dot, dash-dot-dot and dotted lines.
        let (kind, line_style) = match num(el, &format!("{p}trace_type")).map(|t| t as i64) {
            Some(1) => (1, 1),
            Some(2) => (0, 0),
            Some(3) => (5, 0),
            Some(5 | 6) => (2, 0),
            Some(7) => (1, 3),
            Some(8) => (1, 4),
            Some(9) => (1, 2),
            _ => (1, 0),
        };
        // BOY: 0 none, 1 point, 2 circle, 3 and 4 triangles, 5 and 6
        // squares, 7 and 8 diamonds, 9 and 10 crosses, 11 bar.
        let point = match num(el, &format!("{p}point_style")).map(|t| t as i64) {
            Some(1 | 2) => 2,
            Some(3 | 4) => 5,
            Some(5 | 6 | 11) => 1,
            Some(7 | 8) => 3,
            Some(9 | 10) => 4,
            _ if kind == 0 => 1,
            _ => 0,
        };
        let mut trace = Element::new("trace");
        trace.children = take(el, &p, TRACE);
        set(
            &mut trace,
            "axis",
            y_index.get(&boy_axis).copied().unwrap_or(0),
        );
        set(&mut trace, "trace_type", kind);
        set(&mut trace, "line_style", line_style);
        set(&mut trace, "point_type", point);
        traces.children.push(trace);
    }
    // The rest of the numbered properties, and those of unused axes and traces.
    el.children
        .retain(|c| !(c.name.starts_with("axis_") || c.name.starts_with("trace_")));
    el.children.extend([x_axis, y_axes, traces]);
    rename(el, "plot_area_background_color", "background_color");
    // `$(trace_0_y_pv)` and rules on `trace_0_visible` name one trace.
    let trace_ref = |t: &str| trace_refs(t, TRACE);
    for trace in el
        .child_mut("traces")
        .into_iter()
        .flat_map(|t| &mut t.children)
    {
        if let Some(name) = trace.child_mut("name") {
            name.text = trace_ref(&name.text);
        }
    }
    if let Some(tip) = el.child_mut("tooltip") {
        tip.text = trace_ref(&tip.text);
    }
    for rule in el
        .child_mut("rules")
        .into_iter()
        .flat_map(|r| &mut r.children)
    {
        if let Some(prop) = rule.attr("prop_id").map(trace_ref) {
            rule.set_attr("prop_id", prop);
        }
    }
    // Phoebus' XY plot has no PV of its own; BOY's is usually
    // `$(trace_0_y_pv)`, which no macro defines, so it is that trace's PV,
    // kept for rules and tooltips that use `$(pv_name)`.
    if let Some(mut pv) = el.take_child("pv_name") {
        for (i, trace) in el
            .child("traces")
            .into_iter()
            .flat_map(|t| &t.children)
            .enumerate()
        {
            let y_pv = trace.child_text("y_pv").unwrap_or("");
            pv.text = pv.text.replace(&format!("$(trace_{i}_y_pv)"), y_pv);
        }
        pv_macro(el, pv);
    }
}

/// `trace_N_prop` in `text` as `traces[N].prop`, for the trace properties
/// in `props` (renamed as they are).
fn trace_refs(text: &str, props: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("trace_") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "trace_".len()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let prop = after[digits..].strip_prefix('_').and_then(|p| {
            props
                .iter()
                .find(|(from, _)| p.starts_with(from))
                .map(|(from, to)| (*to, &p[from.len()..]))
        });
        match prop {
            Some((to, tail)) if digits > 0 => {
                out.push_str(&format!("traces[{}].{to}", &after[..digits]));
                rest = tail;
            }
            _ => {
                out.push_str("trace_");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A tabbed container: each child (a grouping container) is one tab's
/// content, titled by `tab_N_title`; the first tab's colours and font serve
/// for all.
fn tabs(el: &mut Element, children: Vec<Element>) {
    let mut tabs = Element::new("tabs");
    for (i, child) in children.into_iter().enumerate() {
        let title = text(el, &format!("tab_{i}_title"))
            .unwrap_or("")
            .to_string();
        tabs.children.push(
            Element::new("tab")
                .with_child(Element::with_text("name", title))
                .with_child(Element::new("children").with_child(child)),
        );
    }
    for (from, to) in [
        ("tab_0_background_color", "background_color"),
        ("tab_0_foreground_color", "foreground_color"),
        ("tab_0_font", "font"),
    ] {
        rename(el, from, to);
    }
    el.children.retain(|c| !c.name.starts_with("tab_"));
    el.children.push(tabs);
    let horizontal = flag(el, "horizontal_tabs") != Some(false);
    set(el, "direction", if horizontal { 0 } else { 1 });
    let min = num(el, "minimum_tab_height").unwrap_or(0.0);
    set(el, "tab_height", min.max(30.0));
}

/// BOY's `<items><s>` as Phoebus' `<items><item>`.
fn items(el: &mut Element) {
    if let Some(items) = el.child_mut("items") {
        items
            .children
            .iter_mut()
            .for_each(|s| s.name = "item".into());
    }
}

/// An EDM menu mux, a drop-down that sets local PVs that stand in for
/// macros: a combo on its own PV or, without one, its first target, with
/// the sets kept in a `<menu_mux>` (see [`crate::menu_mux`]).
fn menu_mux(el: &mut Element) {
    let mux = crate::menu_mux::from_opi(el);
    if text(el, "pv_name").is_none_or(str::is_empty)
        && let Some(target) = mux
            .children
            .iter()
            .find_map(|s| s.child_text("target"))
            .map(str::to_string)
    {
        set(el, "pv_name", target);
    }
    set(el, "items_from_pv", false);
    items(el);
    el.children.retain(|c| {
        !(c.name.starts_with("target") || c.name.starts_with("values"))
            && !["initial", "num_sets"].contains(&c.name.as_str())
    });
    el.children.push(mux);
}

/// A DLS detail panel's rows: each child is a row's (already placed)
/// content, and its name goes in the column to the left on a background
/// that alternates row by row.
fn detail_rows(el: &Element, rows: Vec<Element>) -> Vec<Element> {
    let width = num(el, "width").unwrap_or(0.0);
    let mut out = Vec::new();
    let mut labels = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let (x, y, h) = (
            num(row, "x").unwrap_or(0.0),
            num(row, "y").unwrap_or(0.0),
            num(row, "height").unwrap_or(0.0),
        );
        if h <= 1.0 {
            continue;
        }
        let parity = if i % 2 == 0 { "even" } else { "odd" };
        let mut back = Element::new("widget").with_attr("type", "rectangle");
        for (k, v) in [
            ("x", 0.0),
            ("y", y),
            ("width", width),
            ("height", h),
            ("line_width", 0.0),
        ] {
            set(&mut back, k, v);
        }
        if let Some(c) = el.child(&format!("color_{parity}_row_back")) {
            back.children.push(Element {
                name: "background_color".into(),
                ..c.clone()
            });
        }
        out.push(back);
        let name = text(el, &format!("row_{i}_row_name")).unwrap_or("");
        if x > 0.0 && !name.is_empty() {
            let mut label = Element::new("widget").with_attr("type", "label");
            set(&mut label, "text", name);
            for (k, v) in [
                ("x", 4.0),
                ("y", y),
                ("width", x - 4.0),
                ("height", h.min(27.0)),
            ] {
                set(&mut label, k, v);
            }
            set(&mut label, "vertical_alignment", 1);
            if let Some(c) = el.child("font") {
                label.children.push(c.clone());
            }
            if let Some(c) = el.child(&format!("color_{parity}_row_fore")) {
                label.children.push(Element {
                    name: "foreground_color".into(),
                    ..c.clone()
                });
            }
            labels.push(label);
        }
    }
    out.extend(labels);
    out.extend(rows);
    out
}

/// `<symbols>` naming `files`.
fn symbols(files: &[String]) -> Element {
    let mut el = Element::new("symbols");
    for f in files {
        el.children.push(Element::with_text("symbol", f.clone()));
    }
    el
}

/// The images of a numbered set, from one of them: `"ionp 4.svg"` gives
/// `"ionp 0.svg"` … `"ionp 15.svg"`; a name without a number is used alone.
fn numbered(file: &str) -> Vec<String> {
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    if base.len() == stem.len() || !base.ends_with(' ') {
        return vec![file.to_string()];
    }
    let dot = if ext.is_empty() { "" } else { "." };
    (0..16).map(|i| format!("{base}{i}{dot}{ext}")).collect()
}

/// BOY `stretch_to_fit` is the opposite of Phoebus `preserve_ratio`.
fn stretch(el: &mut Element) {
    if let Some(s) = flag(el, "stretch_to_fit") {
        set(el, "preserve_ratio", !s);
    }
}

/// Run the button's `index` action first: BOY ran only that one on a click.
fn push_first(el: &mut Element, index: &str) {
    let i = num(el, index).unwrap_or(0.0).max(0.0) as usize;
    if let Some(a) = el.child_mut("actions")
        && i > 0
        && i < a.children.len()
    {
        let first = a.children.remove(i);
        a.children.insert(0, first);
    }
}

/// Rectangles and ellipses: the border as outline, and `alpha`.
fn shape(el: &mut Element) {
    default(el, "line_width", "0");
    outline(el);
    if let Some(alpha) = num(el, "alpha").filter(|a| *a < 255.0) {
        for prop in ["background_color", "line_color"] {
            if let Some(c) = el.child_mut(prop).and_then(|p| p.child_mut("color")) {
                let a = c
                    .attr("alpha")
                    .and_then(|a| a.parse::<f64>().ok())
                    .unwrap_or(255.0);
                c.set_attr("alpha", ((a * alpha / 255.0).round() as i64).to_string());
            }
        }
    }
}

/// The BOY border style of a widget: `None` when it draws no border.
fn border_style(el: &Element) -> Option<i64> {
    let style = text(el, "border_style")?;
    let style: i64 = style.parse().ok()?;
    (!matches!(style, 0 | 3 | 15)).then_some(style)
}

/// Phoebus `OutlineSupport`: a border at least as wide as the line becomes it.
fn outline(el: &mut Element) {
    let Some(style) = border_style(el) else {
        return;
    };
    let bw = num(el, "border_width").unwrap_or(0.0);
    if num(el, "line_width").unwrap_or(0.0) > bw {
        return;
    }
    rename(el, "border_color", "line_color");
    set(el, "line_width", bw);
    let dash = match style {
        8 => Some(2),
        9 => Some(1),
        10 => Some(3),
        11 => Some(4),
        _ => None,
    };
    if let Some(d) = dash {
        set(el, "line_style", d);
    }
}

/// Phoebus `BorderSupport`: BOY drew borders inside the widget, Phoebus
/// draws them around it, so the widget shrinks by the border's width.
fn border(el: &mut Element) {
    let Some(style) = border_style(el) else {
        set(el, "border_width", 0);
        return;
    };
    match style {
        2 | 4 | 5 | 6 | 7 => {
            let text = Element::new("color")
                .with_attr("name", "Text")
                .with_attr("red", "0")
                .with_attr("green", "0")
                .with_attr("blue", "0");
            el.set_child(Element::new("border_color").with_child(text));
        }
        14 => set(el, "transparent", false),
        _ => {}
    }
    let bw = num(el, "border_width").unwrap_or(0.0);
    if bw > 0.0 {
        add(el, "x", bw);
        add(el, "y", bw);
        add(el, "width", -2.0 * bw);
        add(el, "height", -2.0 * bw);
    }
}

/// BOY `rotation_angle` (degrees) as Phoebus `rotation_step`.
fn rotation(el: &mut Element) {
    let Some(a) = el.take_child("rotation_angle") else {
        return;
    };
    let a: f64 = a.text.trim().parse().unwrap_or(0.0);
    let step = match a.rem_euclid(360.0).round() as i64 {
        90 => 3,
        180 => 2,
        270 => 1,
        _ => return,
    };
    set(el, "rotation_step", step);
}

/// BOY `format_type` and `precision_from_pv` as Phoebus `format` and
/// `precision`; rules that set the format type set the format.
fn format(el: &mut Element) {
    if let Some(f) = el.take_child("format_type") {
        let (format, precision) = format_code(&f.text);
        set(el, "format", format);
        if let Some(p) = precision {
            set(el, "precision", p);
        }
    }
    if flag(el, "precision_from_pv") == Some(true) {
        set(el, "precision", -1);
    }
    for rule in el
        .child_mut("rules")
        .into_iter()
        .flat_map(|r| &mut r.children)
    {
        // An `out_exp` rule computes the BOY code, which can't be mapped here,
        // so it stays on `format_type` and is left untranslated.
        if rule.name != "rule"
            || rule.attr("prop_id") != Some("format_type")
            || rule.attr("out_exp") == Some("true")
        {
            continue;
        }
        rule.set_attr("prop_id", "format");
        for value in rule
            .children
            .iter_mut()
            .filter(|c| c.name == "exp")
            .flat_map(|e| e.children.iter_mut().filter(|v| v.name == "value"))
        {
            value.text = format_code(&value.text).0.to_string();
        }
    }
}

/// A BOY `format_type` as a Phoebus `format`, and the precision it implies.
fn format_code(format_type: &str) -> (i64, Option<i64>) {
    match format_type.trim() {
        "1" => (1, None),
        "2" => (2, None),
        "3" => (4, Some(8)),
        "4" => (6, None),
        "5" => (4, Some(16)),
        "6" => (5, None),
        "7" => (3, None),
        "8" => (7, None),
        "9" => (8, None),
        "10" => (9, None),
        _ => (0, None),
    }
}

fn transparent_background(el: &mut Element) {
    if let Some(c) = el
        .child_mut("background_color")
        .and_then(|p| p.child_mut("color"))
    {
        c.set_attr("alpha", "0");
    }
}

/// BOY points are in the parent's coordinates; Phoebus's are the widget's.
fn points(el: &mut Element) {
    let (x0, y0) = (num(el, "x").unwrap_or(0.0), num(el, "y").unwrap_or(0.0));
    let Some(points) = el.child_mut("points") else {
        return;
    };
    for p in points.children.iter_mut().filter(|p| p.name == "point") {
        let get = |p: &Element, k: &str| p.attr(k).and_then(|v| v.trim().parse::<f64>().ok());
        if let (Some(x), Some(y)) = (get(p, "x"), get(p, "y")) {
            p.set_attr("x", (x - x0).to_string());
            p.set_attr("y", (y - y0).to_string());
        }
    }
}

/// Phoebus group style from the BOY border, and the border colour as line
/// and text colour (`GroupWidget.convertLegacyStyle`).
fn group(el: &mut Element) {
    let style = match num(el, "border_style").map(|s| s as i64) {
        Some(0 | 15) | None => 3,
        Some(12) => 1,
        Some(13) => 0,
        Some(_) => 2,
    };
    set(el, "style", style);
    rename_rules(el, "border_color", "line_color");
    if let Some(c) = el.child("border_color").cloned() {
        el.set_child(Element {
            name: "line_color".into(),
            ..c.clone()
        });
        if style != 1 {
            el.set_child(Element {
                name: "foreground_color".into(),
                ..c
            });
        }
    }
    if style == 1
        && let Some(bg) = el.child("background_color").cloned()
    {
        el.set_child(Element {
            name: "foreground_color".into(),
            ..bg
        });
    }
    remove(el, &["border_style", "border_width", "border_color"]);
}

/// A linking container: `opi_file`, `resize_behaviour` and a border, which
/// for a title bar or group box becomes a group around it (returned, with
/// `children` moved into it).
fn embedded(el: &mut Element, children: &mut Vec<Element>) -> Option<Element> {
    if el.child("file").is_none() {
        rename(el, "opi_file", "file");
    }
    remove(el, &["opi_file"]);
    set(el, "transparent", true);
    if let Some(r) = el.take_child("resize_behaviour") {
        let resize = match r.text.trim() {
            "0" => 1,
            "1" => 2,
            "2" => 4,
            _ => 0,
        };
        set(el, "resize", resize);
    }
    let style = num(el, "border_style").map(|s| s as i64);
    let (group_style, dx, dy) = match style {
        Some(12) => (1, 2.0, 20.0),
        Some(13) => (0, 30.0, 30.0),
        _ => {
            border(el);
            return None;
        }
    };
    let mut group = Element::new("widget").with_attr("type", "group");
    let name = text(el, "name").unwrap_or("").to_string();
    set(&mut group, "name", &name);
    set(&mut group, "style", group_style);
    set(&mut group, "transparent", true);
    for p in ["x", "y", "width", "height", "font", "visible"] {
        if let Some(c) = el.child(p) {
            group.children.push(c.clone());
        }
    }
    if let Some(c) = el.child("border_color") {
        group.children.push(Element {
            name: "line_color".into(),
            ..c.clone()
        });
        if group_style != 1 {
            group.children.push(Element {
                name: "foreground_color".into(),
                ..c.clone()
            });
        }
    }
    if group_style == 1
        && let Some(bg) = el.child("background_color")
    {
        group.children.push(Element {
            name: "foreground_color".into(),
            ..bg.clone()
        });
    }
    remove(
        el,
        &["border_style", "border_width", "border_color", "x", "y"],
    );
    set(el, "name", format!("{name}_Content"));
    add(el, "width", -dx);
    add(el, "height", -dy);
    let mut inner = Element::new("widget").with_attr("type", "embedded");
    inner.children = std::mem::take(&mut el.children);
    inner.children.append(children);
    group.children.push(inner);
    Some(group)
}

/// The text update Phoebus adds for a BOY progress bar's value label.
fn progress_label(el: &Element) -> Element {
    let mut label = Element::new("widget").with_attr("type", "textupdate");
    let name = text(el, "name").unwrap_or("").to_string();
    set(&mut label, "name", format!("{name} Label"));
    for p in ["x", "y", "width", "height", "pv_name", "font", "visible"] {
        if let Some(c) = el.child(p) {
            label.children.push(c.clone());
        }
    }
    set(&mut label, "transparent", true);
    set(&mut label, "horizontal_alignment", 1);
    set(&mut label, "border_alarm_sensitive", false);
    label
}

/// Phoebus `BaseLEDWidget`: an alarm-sensitive BOY LED drew its border
/// inside its bounds.
fn led_border(el: &mut Element) {
    if flag(el, "border_alarm_sensitive") != Some(true) {
        return;
    }
    let style = num(el, "border_style").unwrap_or(0.0);
    let b = if style <= 0.0 {
        2.0
    } else {
        num(el, "border_width").unwrap_or(1.0)
    };
    add(el, "x", b);
    add(el, "y", b);
    add(el, "width", -2.0 * b);
    add(el, "height", -2.0 * b);
}

/// A BOY multi-state LED's numbered state properties as Phoebus `<states>`,
/// with blank labels when BOY hid them (`show_boolean_label` false).
fn led_states(el: &mut Element, labels: bool) {
    let n = num(el, "state_count").unwrap_or(0.0).clamp(0.0, MAX_COUNT) as usize;
    let mut states = Element::new("states");
    for i in 0..n {
        let value = el.take_child(&format!("state_value_{i}"));
        let label = el.take_child(&format!("state_label_{i}"));
        let color = el.take_child(&format!("state_color_{i}"));
        let mut state = Element::new("state");
        state.children.push(Element::with_text(
            "value",
            value.map(|v| v.text).unwrap_or_else(|| i.to_string()),
        ));
        state.children.push(Element::with_text(
            "label",
            label.filter(|_| labels).map(|l| l.text).unwrap_or_default(),
        ));
        if let Some(mut c) = color {
            c.name = "color".into();
            state.children.push(c);
        }
        states.children.push(state);
    }
    el.children.push(states);
    rename(el, "state_color_fallback", "fallback_color");
    rename(el, "state_label_fallback", "fallback_label");
}

// --- actions and rules -------------------------------------------------------

/// BOY actions as `.bob` actions. A click hook on a widget that isn't a
/// button runs its actions when it is clicked: all of them with `hook_all`,
/// else the first.
fn actions(el: &mut Element, kind: &str) {
    let Some(a) = el.take_child("actions") else {
        return;
    };
    let hook = a.attr("hook") == Some("true");
    let hook_all = a.attr("hook_all") == Some("true");
    let mut list: Vec<Element> = a
        .children
        .iter()
        .filter(|c| c.name == "action")
        .map(action)
        .collect();
    if list.is_empty() {
        return;
    }
    if !is_button(kind) && (hook || hook_all) {
        if !hook_all {
            list.truncate(1);
        }
        set(el, "run_actions_on_mouse_click", true);
    }
    let mut out = Element::new("actions");
    out.children = list;
    el.children.push(out);
}

fn action(a: &Element) -> Element {
    let kind = a.attr("type").unwrap_or("");
    let child = |n: &str| a.child(n).cloned();
    let mut out = Element::new("action");
    let mut push = |name: &str, el: Option<Element>| {
        if let Some(mut el) = el {
            el.name = name.to_string();
            out.children.push(el);
        }
    };
    let new_kind = match kind {
        "OPEN_DISPLAY" | "OPEN_OPI_IN_VIEW" => {
            push("file", child("path"));
            push("macros", child("macros"));
            let target = match (text(a, "replace"), text(a, "mode")) {
                (Some("0"), _) => "tab",
                (Some("2"), _) => "window",
                (Some(_), _) => "replace",
                (None, Some("7" | "8")) => "window",
                (None, Some("1" | "2" | "3" | "4" | "5" | "6")) => "tab",
                _ => "replace",
            };
            push("target", Some(Element::with_text("target", target)));
            "open_display".to_string()
        }
        "WRITE_PV" => {
            push("pv_name", child("pv_name"));
            push("value", child("value"));
            "write_pv".to_string()
        }
        "EXECUTE_CMD" => {
            push("command", child("command"));
            // BOY's default, the display's own directory, is `.bob`'s too.
            if let Some(dir) = child("command_directory").filter(|d| d.text.trim() != "$(opi.dir)")
            {
                push("command_directory", Some(dir));
            }
            "command".to_string()
        }
        "OPEN_WEBPAGE" => {
            push("url", child("hyperlink"));
            "open_webpage".to_string()
        }
        "EXECUTE_JAVASCRIPT" if closes_display(a) => "close_display".to_string(),
        "EXECUTE_JAVASCRIPT" | "EXECUTE_PYTHONSCRIPT"
            if let Some(pvs) = text(a, "scriptText").and_then(crate::databrowser_pvs) =>
        {
            for pv in pvs {
                push("pv_name", Some(Element::with_text("pv_name", pv)));
            }
            "trend".to_string()
        }
        other => other.to_ascii_lowercase(),
    };
    push("description", child("description"));
    out.with_attr("type", new_kind)
}

/// Whether a script action is the EDM "Exit" button's embedded script,
/// which only closes the display.
fn closes_display(a: &Element) -> bool {
    let script: String = text(a, "scriptText")
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let script = script
        .strip_prefix("importPackage(Packages.org.csstudio.opibuilder.scriptUtil);")
        .unwrap_or(&script);
    matches!(
        script.trim_end_matches(';'),
        "ScriptUtil.closeAssociatedOPI(widget)"
    )
}

/// Take the DLS `visible.py` scripts out of `el`'s scripts, giving the PV
/// each shows the widget by. The script hides the widget, reads a PV name
/// from the initial value of its local trigger PV, and shows the widget
/// while that PV is true:
///
/// ```xml
/// <path pathString="visible.py">
///   <pv trig="true">loc://$(DID)eloss1("='$(motor):ELOSS' == 1 || '$(motor):ELOSS' == 3")</pv>
/// </path>
/// ```
///
/// `None` for a script whose PV names nothing: it only hides the widget.
fn visible_scripts(el: &mut Element) -> Vec<Option<String>> {
    let Some(scripts) = el.child_mut("scripts") else {
        return Vec::new();
    };
    let (visible, others): (Vec<Element>, Vec<Element>) = std::mem::take(&mut scripts.children)
        .into_iter()
        .partition(|p| {
            p.name == "path"
                && p.attr("pathString")
                    .is_some_and(|s| s.rsplit(['/', '\\']).next() == Some("visible.py"))
        });
    scripts.children = others;
    visible
        .iter()
        .map(|p| {
            let pv = p.children.iter().find(|c| c.name == "pv")?;
            crate::opi_file_script::loc_initial(pv.text.trim()).filter(|n| !n.is_empty())
        })
        .collect()
}

/// A rule showing a widget while `pv` is true, as a `visible.py` script.
fn visible_rule(pv: String) -> Element {
    Element::new("rule")
        .with_attr("name", "visible.py")
        .with_attr("prop_id", "visible")
        .with_attr("out_exp", "false")
        .with_child(
            Element::new("exp")
                .with_attr("bool_exp", "pv0")
                .with_child(Element::with_text("value", "true")),
        )
        .with_child(Element::with_text("pv_name", pv))
}

/// BOY's alarm-sensitive foreground and background colours as rules on the
/// widget's PV (as Phoebus imports them), in DLS's BOY alarm colours:
/// MINOR yellow, MAJOR red and INVALID (or disconnected) white. Written as
/// BOY rules, before [`rules`] and the renames translate them. An
/// alarm-sensitive border gets the bobview-only `<boy_alarm_border>`, which
/// draws INVALID and disconnected solid white rather than Phoebus's dashed
/// DISCONNECTED.
fn alarm_sensitive(el: &mut Element) {
    if !text(el, "pv_name").is_some_and(|p| !p.is_empty()) {
        return;
    }
    if flag(el, "border_alarm_sensitive") != Some(false) {
        set(el, "boy_alarm_border", true);
    }
    for (sensitive, prop) in [
        ("forecolor_alarm_sensitive", "foreground_color"),
        ("backcolor_alarm_sensitive", "background_color"),
    ] {
        if el.take_child(sensitive).map(|f| f.text.trim() == "true") != Some(true) {
            continue;
        }
        let mut rule = Element::new("rule")
            .with_attr("name", sensitive)
            .with_attr("prop_id", prop)
            .with_attr("out_exp", "false");
        // BOY's numbering: 1 MAJOR, 2 MINOR, -1 INVALID.
        for (sev, name, [r, g, b]) in [
            (1, "Major", [255, 0, 0]),
            (2, "Minor", [255, 241, 0]),
            (-1, "Invalid", [255, 255, 255]),
        ] {
            let color = Element::new("color")
                .with_attr("name", name)
                .with_attr("red", r.to_string())
                .with_attr("green", g.to_string())
                .with_attr("blue", b.to_string());
            rule.children.push(
                Element::new("exp")
                    .with_attr("bool_exp", format!("pvSev0=={sev}"))
                    .with_child(Element::new("value").with_child(color)),
            );
        }
        rule.children
            .push(Element::with_text("pv", "$(pv_name)").with_attr("trig", "true"));
        match el.child_mut("rules") {
            Some(rules) => rules.children.push(rule),
            None => el.children.push(Element::new("rules").with_child(rule)),
        }
    }
}

/// BOY rules as `.bob` rules: `<pv>` is `<pv_name>`, BOY severities are
/// `pvLegacySev` (as Phoebus patches them), the widget's own value
/// (`widget.getValue()`) is its PV as one more rule PV (or 0 if it has
/// none). Rules on renamed properties follow them (see [`rename`]).
fn rules(el: &mut Element) {
    let has_pv = text(el, "pv_name").is_some_and(|p| !p.is_empty());
    let Some(rules) = el.child_mut("rules") else {
        return;
    };
    for rule in rules.children.iter_mut().filter(|r| r.name == "rule") {
        // BOY runs a rule while its PVs are disconnected, so that an alarm
        // rule shows them as INVALID.
        rule.set_attr("check_connections", "false");
        let own = format!(
            "pv{}",
            rule.children.iter().filter(|c| c.name == "pv").count()
        );
        let mut uses_own = false;
        for c in &mut rule.children {
            match c.name.as_str() {
                "pv" => {
                    c.name = "pv_name".into();
                    c.attrs.clear();
                }
                "exp" => {
                    let Some(e) = c.attr("bool_exp") else {
                        continue;
                    };
                    let mut patched = e.replace("pvSev", "pvLegacySev");
                    if has_pv && patched.contains("widget.getValue()") {
                        patched = patched.replace("widget.getValue()", &own);
                        uses_own = true;
                    } else {
                        // Without a PV the widget keeps its initial value
                        // (a bool button's is off).
                        patched = patched.replace("widget.getValue()", "0");
                    }
                    c.set_attr("bool_exp", patched);
                }
                _ => {}
            }
        }
        if uses_own {
            rule.children
                .push(Element::with_text("pv_name", "$(pv_name)"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Display, FontStyle, GroupLink, LinkSets, Widget};

    /// Parse an `.opi` display holding `widgets`.
    fn opi(widgets: &str) -> Display {
        let xml = format!(
            r#"<display typeId="org.csstudio.opibuilder.Display" version="1.0.0">
            <name>T</name><width>300</width><x>700</x>{widgets}</display>"#
        );
        Display::parse(&xml).unwrap()
    }

    fn one(type_id: &str, props: &str) -> Widget {
        let d = opi(&format!(
            r#"<widget typeId="org.csstudio.opibuilder.widgets.{type_id}" version="1.0.0">{props}</widget>"#
        ));
        d.widgets.into_iter().next().unwrap()
    }

    #[test]
    fn detects_opi_files() {
        let root = |xml: &str| {
            let doc = roxmltree::Document::parse(xml).unwrap();
            Element::from_node(doc.root_element())
        };
        assert!(is_opi(&root(
            r#"<display typeId="org.csstudio.opibuilder.Display"/>"#
        )));
        assert!(is_opi(&root(r#"<display version="1.0"/>"#)));
        assert!(!is_opi(&root(r#"<display version="2.0.0"/>"#)));
        assert!(!is_opi(&root("<display/>")));
        let d = opi("");
        assert_eq!((d.name.as_str(), d.width), ("T", 300.0));
    }

    #[test]
    fn colours_keep_rgb_or_come_from_colors_list() {
        let w = one(
            "Rectangle",
            r#"<background_color><color name="Grey 75%" red="192" green="192" blue="192"/></background_color>
            <line_color><color name="Canvas"/></line_color>
            <foreground_color><color name="canvas"/></foreground_color>
            <border_color><color name="OK"/></border_color>"#,
        );
        assert_eq!(
            w.color("background_color").unwrap().rgba(),
            [192, 192, 192, 255]
        );
        assert_eq!(w.color("line_color").unwrap().rgba(), [200, 200, 200, 255]);
        assert_eq!(
            w.color("foreground_color").unwrap().rgba(),
            [200, 200, 200, 255]
        );
        // Not an EDM colour: the Phoebus palette.
        assert_eq!(w.color("border_color").unwrap().rgba(), [0, 255, 0, 255]);
    }

    #[test]
    fn reads_colors_list() {
        let c = parse_colors_list(
            r#"blinkms=750
alias trace0 red-21
static 3   "Canvas"           { 51400 51400 51400 }    #
#static 36  "Old"   { 0 0 0 }
static 79  "79"               {     0     0     0 65000 65000 65000 }    #
static 21  red-21  { 57568 0 0 }
rule 84  alert {
 >=0 && <1 : "Canvas"
}
menumap {
 "Disconn/Invalid"
}"#,
        );
        assert_eq!(c["Canvas"], [200, 200, 200]);
        assert_eq!(c["79"], [0, 0, 0]);
        assert_eq!(c["red-21"], [224, 0, 0]);
        assert_eq!(c["trace0"], [224, 0, 0]);
        assert!(!c.contains_key("Old") && !c.contains_key("alert"));
        assert_eq!(edm_color("Monitor: MAJOR"), Some([224, 0, 0]));
        assert_eq!(edm_color("disconn/invalid"), Some([255, 255, 255]));
    }

    #[test]
    fn fonts_follow_boy() {
        let w = one(
            "Label",
            r#"<font><opifont.name fontName="Liberation Sans" height="15" style="0" pixels="true">Default</opifont.name></font>
            <scale_font><fontdata fontName="liberation sans" height="16" style="1" pixels="true"/></scale_font>
            <title_font><fontdata fontName="Sans" height="10" style="3"/></title_font>
            <other_font><opifont.name fontName="Arial" height="20" style="2">Site Big</opifont.name></other_font>
            <pixel_font><opifont.name fontName="Arial" height="20" style="0" pixels="true">Site Big</opifont.name></pixel_font>"#,
        );
        let f = w.font("font").unwrap();
        assert_eq!(
            (f.size, f.style, f.name.as_deref()),
            (14.0, FontStyle::Regular, Some("Default"))
        );
        let f = w.font("scale_font").unwrap();
        assert_eq!((f.size, f.style), (16.0, FontStyle::Bold));
        let f = w.font("title_font").unwrap();
        assert_eq!((f.size, f.style), (12.7, FontStyle::BoldItalic));
        let f = w.font("other_font").unwrap();
        assert_eq!((f.size, f.family.as_str()), (25.4, "Arial"));
        assert_eq!(f.name.as_deref(), Some("Site Big"));
        let f = w.font("pixel_font").unwrap();
        assert_eq!((f.size, f.family.as_str()), (20.0, "Arial"));
    }

    #[test]
    fn maps_type_ids() {
        let kinds = [
            ("Rectangle", "rectangle"),
            ("RoundedRectangle", "rectangle"),
            ("Ellipse", "ellipse"),
            ("groupingContainer", "group"),
            ("Label", "label"),
            ("edm.symbolwidget", "symbol"),
            ("TextUpdate", "textupdate"),
            ("polyline", "polyline"),
            ("ActionButton", "action_button"),
            ("linkingContainer", "embedded"),
            ("bytemonitor", "byte_monitor"),
            ("progressbar", "progressbar"),
            ("MenuButton", "combo"),
            ("TextInput", "textentry"),
            ("polygon", "polygon"),
            ("LED", "led"),
            ("arc", "arc"),
            ("symbol.multistate.MultistateMonitorWidget", "symbol"),
            ("detailpanel", "group"),
            ("choiceButton", "choice"),
            ("BoolButton", "bool_button"),
            ("meter", "meter"),
            ("ImageBoolIndicator", "symbol"),
            ("scaledslider", "scaledslider"),
            ("xyGraph", "xyplot"),
            ("dawn.xygraph", "xyplot"),
            ("dawn.intensitygraph", "image"),
            ("tab", "tabs"),
            ("edm.menumux", "combo"),
            ("NoSuchWidget", "NoSuchWidget"),
            ("Image", "picture"),
        ];
        for (type_id, kind) in kinds {
            assert_eq!(one(type_id, "").kind, kind, "{type_id}");
        }
    }

    #[test]
    fn shapes_take_pv_name_as_a_macro_and_border_as_outline() {
        let w = one(
            "Rectangle",
            r#"<pv_name>$(P):STA</pv_name><macros><include_parent_macros>true</include_parent_macros><Q>1</Q></macros>
            <border_style>1</border_style><border_width>2</border_width><line_width>1</line_width>
            <border_color><color red="1" green="2" blue="3"/></border_color><alpha>128</alpha>
            <background_color><color red="9" green="9" blue="9"/></background_color>"#,
        );
        assert!(w.get("pv_name").is_none());
        let m = w.macros();
        assert_eq!(
            (m.get("pv_name"), m.get("Q")),
            (Some("$(P):STA"), Some("1"))
        );
        assert!(m.get("include_parent_macros").is_none());
        assert_eq!(w.f64("line_width"), Some(2.0));
        assert_eq!(w.color("line_color").unwrap().rgba(), [1, 2, 3, 128]);
        assert_eq!(w.color("background_color").unwrap().a, 128);
        // A border style that draws nothing leaves the line alone.
        let w = one(
            "Rectangle",
            "<border_style>0</border_style><border_width>3</border_width><line_width>1</line_width>",
        );
        assert_eq!(w.f64("line_width"), Some(1.0));
        assert_eq!(one("Ellipse", "").f64("line_width"), Some(0.0));
    }

    #[test]
    fn groups_take_their_style_from_the_border() {
        let style = |b: &str| {
            one(
                "groupingContainer",
                &format!("<border_style>{b}</border_style>"),
            )
            .i64("style")
        };
        assert_eq!(style("0"), Some(3));
        assert_eq!(style("1"), Some(2));
        assert_eq!(style("12"), Some(1));
        assert_eq!(style("13"), Some(0));
        let g = one(
            "groupingContainer",
            r#"<border_style>13</border_style><border_color><color red="5" green="5" blue="5"/></border_color>
            <widget typeId="org.csstudio.opibuilder.widgets.Label"><text>in</text></widget>"#,
        );
        assert_eq!(g.color("foreground_color").unwrap().r, 5);
        assert_eq!(g.color("line_color").unwrap().r, 5);
        assert_eq!(g.children[0].kind, "label");
        assert_eq!(g.children[0].text("text"), Some("in"));
    }

    #[test]
    fn alarm_sensitive_colours_are_rules_on_the_pv() {
        let w = one(
            "TextUpdate",
            r#"<pv_name>X</pv_name><forecolor_alarm_sensitive>true</forecolor_alarm_sensitive>
            <backcolor_alarm_sensitive>false</backcolor_alarm_sensitive><border_alarm_sensitive>true</border_alarm_sensitive>
            <rules><rule name="r" prop_id="visible" out_exp="false"><exp bool_exp="pv0==1"><value>false</value></exp><pv trig="true">Y</pv></rule></rules>"#,
        );
        assert_eq!(w.bool("boy_alarm_border"), Some(true));
        let rules = w.rules();
        assert_eq!(rules.len(), 2);
        let r = &rules[1];
        assert_eq!(r.prop_id, "foreground_color");
        assert_eq!(r.pv_names, ["$(pv_name)"]);
        assert!(!r.check_connections);
        let sev: Vec<_> = r.exprs.iter().map(|e| e.condition.as_str()).collect();
        assert_eq!(
            sev,
            ["pvLegacySev0==1", "pvLegacySev0==2", "pvLegacySev0==-1"]
        );
        let rgba =
            |i: usize| crate::Color::from_element(r.exprs[i].value.child("color").unwrap()).rgba();
        assert_eq!(rgba(0), [255, 0, 0, 255]);
        assert_eq!(rgba(1), [255, 241, 0, 255]);
        assert_eq!(rgba(2), [255, 255, 255, 255]);

        // A polyline's background is its line, and the rule follows it.
        let w = one(
            "polyline",
            r#"<pv_name>X</pv_name><backcolor_alarm_sensitive>true</backcolor_alarm_sensitive>
            <border_alarm_sensitive>false</border_alarm_sensitive>"#,
        );
        assert_eq!(w.rules()[0].prop_id, "line_color");
        assert_eq!(w.get("boy_alarm_border"), None);
        // Without a PV there is no alarm.
        let w = one(
            "TextUpdate",
            r#"<pv_name></pv_name><forecolor_alarm_sensitive>true</forecolor_alarm_sensitive>"#,
        );
        assert!(w.rules().is_empty());
        assert_eq!(w.get("boy_alarm_border"), None);
    }

    #[test]
    fn polylines_are_relative_and_use_background_for_the_line() {
        let w = one(
            "polyline",
            r#"<x>11</x><y>24</y><points><point x="11" y="24"/><point x="23" y="30"/></points>
            <background_color><color red="7" green="0" blue="0"/></background_color>
            <rules><rule name="r" prop_id="background_color" out_exp="false"><exp bool_exp="pvSev0==1"><value><color name="Canvas"/></value></exp><pv trig="true">X</pv></rule></rules>"#,
        );
        assert_eq!(w.points(), vec![(0.0, 0.0), (12.0, 6.0)]);
        assert_eq!(w.color("line_color").unwrap().r, 7);
        assert!(w.get("background_color").is_none());
        let r = &w.rules()[0];
        assert_eq!(r.prop_id, "line_color");
        assert_eq!(r.pv_names, ["X"]);
        assert_eq!(r.exprs[0].condition, "pvLegacySev0==1");
        assert!(!r.check_connections);
        let c = crate::Color::from_element(r.exprs[0].value.child("color").unwrap());
        assert_eq!(c.rgba(), [200, 200, 200, 255]);
        let w = one(
            "BoolButton",
            r#"<pv_name>X</pv_name><rules><rule name="r" prop_id="background_color" out_exp="false">
            <exp bool_exp="widget.getValue() == 1 &amp;&amp; pv0 &gt; 2"><value><color red="1" green="1" blue="1"/></value></exp>
            <pv trig="true">Y</pv></rule></rules>"#,
        );
        let r = &w.rules()[0];
        assert_eq!(r.exprs[0].condition, "pv1 == 1 && pv0 > 2");
        assert_eq!(r.pv_names, ["Y", "$(pv_name)"]);
        let w = one(
            "BoolButton",
            r#"<pv_name></pv_name><rules><rule name="r" prop_id="background_color" out_exp="false">
            <exp bool_exp="widget.getValue() == 1"><value><color red="1" green="1" blue="1"/></value></exp>
            <pv trig="true">Y</pv></rule></rules>"#,
        );
        let r = &w.rules()[0];
        assert_eq!(r.exprs[0].condition, "0 == 1");
        assert_eq!(r.pv_names, ["Y"]);
    }

    #[test]
    fn arcs_use_foreground_and_fill() {
        let w = one(
            "arc",
            r#"<fill>false</fill><foreground_color><color red="3" green="0" blue="0"/></foreground_color><border_width>2</border_width>"#,
        );
        assert_eq!(w.color("line_color").unwrap().r, 3);
        assert_eq!(w.bool("transparent"), Some(true));
        assert!(w.get("border_width").is_none());
    }

    #[test]
    fn labels_rotate_and_shrink_inside_their_border() {
        let w = one(
            "Label",
            "<x>10</x><y>10</y><width>50</width><height>20</height><border_style>1</border_style><border_width>2</border_width><rotation_angle>90.0</rotation_angle>",
        );
        assert_eq!(
            [w.f64("x"), w.f64("y"), w.f64("width"), w.f64("height")],
            [Some(12.0), Some(12.0), Some(46.0), Some(16.0)]
        );
        assert_eq!(w.i64("rotation_step"), Some(3));
        assert_eq!(w.bool("transparent"), Some(true));
        let w = one(
            "Label",
            "<x>10</x><border_style>0</border_style><border_width>2</border_width>",
        );
        assert_eq!((w.f64("x"), w.f64("border_width")), (Some(10.0), Some(0.0)));
        let w = one(
            "Label",
            "<border_style>6</border_style><border_width>1</border_width>",
        );
        assert_eq!(
            w.color("border_color").unwrap().name.as_deref(),
            Some("Text")
        );
    }

    #[test]
    fn text_widgets_convert_format_and_static_text() {
        let w = one(
            "TextUpdate",
            "<pv_name>X</pv_name><format_type>3</format_type><precision_from_pv>false</precision_from_pv><text>###</text>",
        );
        assert_eq!(
            (w.kind.as_str(), w.i64("format"), w.i64("precision")),
            ("textupdate", Some(4), Some(8))
        );
        let w = one(
            "TextUpdate",
            "<pv_name>X</pv_name><format_type>7</format_type><precision_from_pv>true</precision_from_pv>",
        );
        assert_eq!((w.i64("format"), w.i64("precision")), (Some(3), Some(-1)));
        let w = one("TextUpdate", "<pv_name></pv_name><text>Title</text>");
        assert_eq!((w.kind.as_str(), w.text("text")), ("label", Some("Title")));
        let w = one(
            "TextInput",
            "<pv_name>X</pv_name><multiline_input>true</multiline_input><transparent>true</transparent><background_color><color red='1' green='1' blue='1'/></background_color>",
        );
        assert_eq!(w.kind, "textentry");
        assert_eq!(w.bool("multi_line"), Some(true));
        assert_eq!(w.color("background_color").unwrap().a, 0);
        assert_eq!(one("TextInput", "<text>t</text>").kind, "label");
    }

    /// A rule setting `prop` to `value` when PV `Y` is 1.
    fn rule(prop: &str, value: &str) -> String {
        format!(
            r#"<rules><rule name="r" prop_id="{prop}" out_exp="false"><exp bool_exp="pv0==1">
            <value>{value}</value></exp><pv trig="true">Y</pv></rule></rules>"#
        )
    }

    #[test]
    fn rules_follow_renamed_properties() {
        let prop = |w: &Widget| {
            let r = &w.rules()[0];
            (r.prop_id.clone(), r.exprs[0].value.text.clone())
        };
        // As in dlsPLC's NX102 screens: the format type, as a format.
        let w = one(
            "TextUpdate",
            &format!("<pv_name>X</pv_name>{}", rule("format_type", "4")),
        );
        assert_eq!(prop(&w), ("format".into(), "6".into()));
        // An `out_exp` rule's computed code can't be mapped: left alone.
        let out = rule("format_type", "4").replace(r#"out_exp="false""#, r#"out_exp="true""#);
        let w = one("TextUpdate", &format!("<pv_name>X</pv_name>{out}"));
        assert_eq!(prop(&w), ("format_type".into(), "4".into()));
        let w = one("linkingContainer", &rule("opi_file", "b.opi"));
        assert_eq!(prop(&w).0, "file");
        let menu = r#"<actions_from_pv>false</actions_from_pv><actions>
            <action type="OPEN_DISPLAY"><path>a.opi</path></action></actions>"#;
        let w = one("MenuButton", &format!("{menu}{}", rule("label", "L")));
        assert_eq!(prop(&w).0, "text");
        let w = one("groupingContainer", &rule("border_color", ""));
        assert_eq!(prop(&w).0, "line_color");
        let w = one(
            "Rectangle",
            &format!(
                "<border_style>1</border_style><border_width>2</border_width>{}",
                rule("border_color", "")
            ),
        );
        assert_eq!(prop(&w).0, "line_color");
        let w = one("xyGraph", &rule("plot_area_background_color", ""));
        assert_eq!(prop(&w).0, "background_color");
    }

    #[test]
    fn counts_are_bounded() {
        let w = one(
            "xyGraph",
            "<axis_count>1e9</axis_count><trace_count>1e9</trace_count>",
        );
        assert_eq!(w.get("traces").unwrap().children.len(), 64);
        let w = one(
            "xyGraph",
            "<axis_count>NaN</axis_count><trace_count>-3</trace_count>",
        );
        assert!(w.get("traces").unwrap().children.is_empty());
        let w = one("LED", "<state_count>1e12</state_count>");
        assert_eq!(w.get("states").unwrap().children.len(), 64);
    }

    #[test]
    fn edm_symbols_keep_their_strip_and_rules() {
        let w = one(
            "edm.symbolwidget",
            r#"<image_file>../dlsPLC/vacuumValve-symbol-34.png</image_file><image_index>0</image_index>
            <sub_image_width>34.0</sub_image_width><pv_name>SR:STA</pv_name>
            <rules><rule name="s" prop_id="image_index" out_exp="true"><exp bool_exp="pv0&gt;=1"><value>2</value></exp><pv trig="true">$(pv_name)</pv></rule></rules>
            <actions hook="true" hook_all="false"><action type="OPEN_DISPLAY"><path>a.opi</path><mode>8</mode></action><action type="OPEN_DISPLAY"><path>b.opi</path></action></actions>"#,
        );
        assert_eq!(w.kind, "symbol");
        assert_eq!(
            w.list("symbols", "symbol"),
            ["../dlsPLC/vacuumValve-symbol-34.png"]
        );
        assert_eq!(
            (w.f64("sub_image_width"), w.text("pv_name")),
            (Some(34.0), Some("SR:STA"))
        );
        let r = &w.rules()[0];
        assert!(r.out_exp);
        assert_eq!(
            (r.prop_id.as_str(), r.exprs[0].value.text.as_str()),
            ("image_index", "2")
        );
        // The click hook runs the first action only.
        assert_eq!(w.bool("run_actions_on_mouse_click"), Some(true));
        assert_eq!(w.actions().len(), 1);
    }

    #[test]
    fn numbered_symbol_sets() {
        let w = one(
            "symbol.multistate.MultistateMonitorWidget",
            "<image_file>symbol/ionp 4.svg</image_file><stretch_to_fit>true</stretch_to_fit>",
        );
        let s = w.list("symbols", "symbol");
        assert_eq!(
            (s.len(), s[0].as_str(), s[15].as_str()),
            (16, "symbol/ionp 0.svg", "symbol/ionp 15.svg")
        );
        assert_eq!(w.bool("preserve_ratio"), Some(false));
        // Draws nothing for a state past the images there are.
        assert_eq!(w.text("fallback_symbol"), Some(""));
        assert_eq!(numbered("plain.svg"), ["plain.svg"]);
        assert_eq!(numbered("v2.png"), ["v2.png"]);
        let w = one(
            "ImageBoolIndicator",
            "<off_image>off.png</off_image><on_image>on.png</on_image>",
        );
        assert_eq!(w.list("symbols", "symbol"), ["off.png", "on.png"]);
    }

    #[test]
    fn actions_translate() {
        let w = one(
            "ActionButton",
            r#"<push_action_index>2</push_action_index><tooltip>$(pv_value)</tooltip><actions hook="false" hook_all="false">
              <action type="EXECUTE_CMD"><command>ls</command><command_directory>$(opi.dir)</command_directory><description>Run</description></action>
              <action type="EXECUTE_JAVASCRIPT"><path>a.js</path></action>
              <action type="OPEN_DISPLAY"><path>../mks937a/g.opi</path><macros><include_parent_macros>true</include_parent_macros><dev>X</dev></macros><mode>3</mode><description>Gauge</description></action>
              <action type="OPEN_DISPLAY"><path>r.opi</path><replace>1</replace></action>
              <action type="OPEN_DISPLAY"><path>w.opi</path><mode>8</mode></action>
              <action type="WRITE_PV"><pv_name>$(P):X</pv_name><value> 1 </value><description>Set</description></action>
              <action type="OPEN_WEBPAGE"><hyperlink>http://x</hyperlink></action>
            </actions>"#,
        );
        let a = w.actions();
        assert_eq!(
            a[0],
            Action::OpenDisplay {
                file: "../mks937a/g.opi".into(),
                macros: crate::Macros::from_pairs([("dev", "X")]),
                target: "tab".into(),
                description: "Gauge".into(),
            }
        );
        assert!(
            matches!(&a[1], Action::Command { command, directory, .. } if command == "ls" && directory.is_empty())
        );
        assert!(matches!(&a[2], Action::Unsupported { kind, .. } if kind == "execute_javascript"));
        assert!(matches!(&a[3], Action::OpenDisplay { target, .. } if target == "replace"));
        assert!(matches!(&a[4], Action::OpenDisplay { target, .. } if target == "window"));
        assert!(
            matches!(&a[5], Action::WritePv { pv_name, value, .. } if pv_name == "$(P):X" && value == "1")
        );
        assert!(matches!(&a[6], Action::OpenWebpage { url, .. } if url == "http://x"));
        assert_eq!(w.text("tooltip"), Some("$(actions)"));
        // The EDM "Exit" button's script closes the display.
        let exit = one(
            "ActionButton",
            r#"<actions hook="false" hook_all="false"><action type="EXECUTE_JAVASCRIPT"><path></path>
              <scriptText><![CDATA[importPackage(Packages.org.csstudio.opibuilder.scriptUtil);ScriptUtil.closeAssociatedOPI(widget);]]></scriptText>
              <embedded>true</embedded><description></description></action></actions>"#,
        );
        assert!(matches!(&exit.actions()[0], Action::CloseDisplay { .. }));
        // A "Graph" button's script opens a Data Browser: a trend here.
        let graph = one(
            "ActionButton",
            r#"<actions hook="false" hook_all="false"><action type="EXECUTE_PYTHONSCRIPT"><path></path>
              <scriptText><![CDATA[from org.csstudio.opibuilder.scriptUtil import ScriptUtil

command_id="org.csstudio.trends.databrowser2.newWindow"
ScriptUtil.executeEclipseCommand(command_id, ['pvnames', '$(dom)-VA-GAUGE-01:PLOG,$(dom)-VA-IMG-01:PLOG'])]]></scriptText>
              <embedded>true</embedded><description>Graph</description></action></actions>"#,
        );
        assert_eq!(
            graph.actions()[0],
            Action::Trend {
                pv_names: vec![
                    "$(dom)-VA-GAUGE-01:PLOG".into(),
                    "$(dom)-VA-IMG-01:PLOG".into()
                ],
                description: "Graph".into(),
            }
        );
        // Buttons ignore click hooks.
        assert!(w.get("run_actions_on_mouse_click").is_none());
    }

    #[test]
    fn menu_buttons_are_combos_or_buttons() {
        let w = one("MenuButton", "<pv_name>X</pv_name>");
        assert_eq!(
            (w.kind.as_str(), w.bool("items_from_pv")),
            ("combo", Some(true))
        );
        let w = one(
            "MenuButton",
            r#"<actions_from_pv>false</actions_from_pv><label>Spaces</label><actions><action type="OPEN_DISPLAY"><path>a.opi</path></action><action type="OPEN_DISPLAY"><path>b.opi</path></action></actions>"#,
        );
        assert_eq!(
            (w.kind.as_str(), w.text("text")),
            ("action_button", Some("Spaces"))
        );
        assert_eq!(w.actions().len(), 2);
    }

    #[test]
    fn linking_container_scripts_choose_the_display() {
        let script = |text: &str, pv: &str| {
            format!(
                r#"<macros><include_parent_macros>true</include_parent_macros><a>1</a></macros>
                <scripts><path pathString="EmbeddedJs" checkConnect="true"><scriptName>OPIFileScript</scriptName>
                <scriptText><![CDATA[{text}]]></scriptText><pv trig="true">{pv}</pv></path></scripts>"#
            )
        };
        // The EDM converter's script, at its PV's initial value.
        let w = one(
            "linkingContainer",
            &script(
                r#"var pv0 = PVUtil.getLong(pvs[0]);
if(pv0==0){
var macroInput = DataUtil.createMacrosInput(true);
macroInput.put("device", "LI-DI-YAG-01");
widget.setPropertyValue("macros", macroInput);
widget.setPropertyValue("opi_file","../xbpm/xbpmsi.opi");
}"#,
                "loc://menu0&lt;VDouble&gt;(0)",
            ),
        );
        assert_eq!(w.text("file"), Some("../xbpm/xbpmsi.opi"));
        assert_eq!(
            (w.macros().get("device"), w.macros().get("a")),
            (Some("LI-DI-YAG-01"), None)
        );
        assert!(!w.has_scripts());
        // A file named by a macro, through the PV.
        let w = one(
            "linkingContainer",
            &script(
                r#"var pv0 = PVUtil.getString(pvs[0]);
widget.setPropertyValue("opi_file", pv0 + ".opi");"#,
                r#"loc://$(DID)dummy&lt;VString&gt;("$(control)")"#,
            ),
        );
        assert_eq!(w.text("file"), Some("$(control).opi"));
        assert_eq!(w.macros().get("a"), Some("1"));
        assert!(!w.has_scripts());
    }

    #[test]
    fn linking_containers_embed() {
        let w = one(
            "linkingContainer",
            r#"<opi_file>../mks937a/gauge.opi</opi_file><resize_behaviour>1</resize_behaviour><group_name>G</group_name>
            <pv_name>P</pv_name><border_style>0</border_style><x>5</x>"#,
        );
        assert_eq!(w.kind, "embedded");
        assert_eq!(
            (w.text("file"), w.i64("resize"), w.text("group_name")),
            (Some("../mks937a/gauge.opi"), Some(2), Some("G"))
        );
        assert_eq!(w.bool("transparent"), Some(true));
        assert_eq!(w.macros().get("pv_name"), Some("P"));
        let g = one(
            "linkingContainer",
            "<opi_file>a.opi</opi_file><name>Box</name><border_style>13</border_style><x>5</x><y>6</y><width>100</width><height>80</height>",
        );
        assert_eq!(
            (g.kind.as_str(), g.i64("style"), g.name()),
            ("group", Some(0), "Box")
        );
        assert_eq!((g.f64("x"), g.f64("width")), (Some(5.0), Some(100.0)));
        let e = &g.children[0];
        assert_eq!(
            (e.kind.as_str(), e.text("file")),
            ("embedded", Some("a.opi"))
        );
        assert_eq!(
            (e.f64("x"), e.f64("width"), e.f64("height")),
            (None, Some(70.0), Some(50.0))
        );
    }

    #[test]
    fn progress_bars_get_a_label() {
        let d = opi(
            r#"<widget typeId="org.csstudio.opibuilder.widgets.progressbar"><pv_name>X</pv_name><y>10</y><height>50</height>
            <color_fillbackground><color red="4" green="4" blue="4"/></color_fillbackground></widget>"#,
        );
        let (bar, label) = (&d.widgets[0], &d.widgets[1]);
        assert_eq!((bar.f64("y"), bar.f64("height")), (Some(35.0), Some(25.0)));
        assert_eq!(bar.color("background_color").unwrap().r, 4);
        assert_eq!(
            (label.kind.as_str(), label.text("pv_name"), label.f64("y")),
            ("textupdate", Some("X"), Some(35.0))
        );
    }

    #[test]
    fn leds_and_byte_monitors() {
        let w = one(
            "LED",
            r#"<square_led>true</square_led><bulb_border_color><color red="9" green="0" blue="0"/></bulb_border_color>
            <show_boolean_label>false</show_boolean_label><on_label>ON</on_label><border_alarm_sensitive>true</border_alarm_sensitive><border_style>0</border_style><x>0</x><width>20</width>"#,
        );
        assert_eq!(
            (w.kind.as_str(), w.bool("square"), w.text("on_label")),
            ("led", Some(true), Some(""))
        );
        assert_eq!(w.color("line_color").unwrap().r, 9);
        assert_eq!((w.f64("x"), w.f64("width")), (Some(2.0), Some(16.0)));
        let w = one(
            "LED",
            r#"<state_count>2</state_count><state_value_0>0</state_value_0><state_label_0>Off</state_label_0><state_color_0><color red="1" green="0" blue="0"/></state_color_0>
            <state_value_1>1</state_value_1><state_label_1>On</state_label_1><state_color_fallback><color red="2" green="0" blue="0"/></state_color_fallback>"#,
        );
        assert_eq!(w.kind, "multi_state_led");
        let states = &w.get("states").unwrap().children;
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].child_text("label"), Some("Off"));
        assert_eq!(states[1].child_text("value"), Some("1"));
        assert_eq!(
            states[0]
                .child("color")
                .unwrap()
                .child("color")
                .unwrap()
                .attr("red"),
            Some("1")
        );
        assert_eq!(w.color("fallback_color").unwrap().r, 2);
        let w = one(
            "LED",
            r#"<state_count>1</state_count><state_label_0>Idle</state_label_0><state_label_fallback>Err</state_label_fallback>
            <show_boolean_label>false</show_boolean_label>"#,
        );
        assert_eq!(
            w.get("states").unwrap().children[0].child_text("label"),
            Some("")
        );
        assert_eq!(w.text("fallback_label"), Some(""));
        let w = one(
            "bytemonitor",
            "<square_led>true</square_led><numBits>4</numBits><label><s>a</s><s>b</s></label>",
        );
        assert_eq!((w.bool("square"), w.i64("numBits")), (Some(true), Some(4)));
        assert_eq!(w.list("labels", "text"), ["a", "b"]);
    }

    #[test]
    fn buttons_choices_and_pictures() {
        let w = one("choiceButton", "<items><s>Start</s><s>Stop</s></items>");
        assert_eq!(w.items(), ["Start", "Stop"]);
        let w = one(
            "BoolButton",
            "<pv_name>X</pv_name><toggle_button>false</toggle_button>",
        );
        assert_eq!((w.kind.as_str(), w.i64("mode")), ("bool_button", Some(1)));
        let w = one(
            "BoolButton",
            r#"<off_label>Reboot</off_label><push_action_index>0</push_action_index><actions hook="true" hook_all="true">
            <action type="WRITE_PV"><pv_name>X</pv_name><value>1</value></action><action type="WRITE_PV"><pv_name>X</pv_name><value>0</value></action></actions>"#,
        );
        assert_eq!(
            (w.kind.as_str(), w.text("text"), w.actions().len()),
            ("action_button", Some("Reboot"), 1)
        );
        let w = one(
            "Image",
            "<image_file>a.png</image_file><stretch_to_fit>true</stretch_to_fit>",
        );
        assert_eq!(
            (w.kind.as_str(), w.text("file"), w.bool("stretch_image")),
            ("picture", Some("a.png"), Some(true))
        );
        let w = one(
            "meter",
            "<scale_font><fontdata fontName='x' height='9' style='0' pixels='true'/></scale_font>",
        );
        assert_eq!(w.font("font").unwrap().size, 9.0);
    }

    #[test]
    fn detail_panels_are_groups_with_named_rows() {
        let g = one(
            "detailpanel",
            r#"<width>360</width><row_count>2</row_count><row_0_row_name>Status</row_0_row_name><row_1_row_name>Mode</row_1_row_name>
            <color_even_row_back><color red="1" green="1" blue="1"/></color_even_row_back>
            <widget typeId="org.csstudio.opibuilder.widgets.groupingContainer"><x>160</x><y>0</y><width>200</width><height>27</height></widget>
            <widget typeId="org.csstudio.opibuilder.widgets.groupingContainer"><x>160</x><y>27</y><width>200</width><height>27</height></widget>"#,
        );
        assert_eq!((g.kind.as_str(), g.i64("style")), ("group", Some(3)));
        let kinds: Vec<&str> = g.children.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["rectangle", "rectangle", "label", "label", "group", "group"]
        );
        assert_eq!(g.children[0].color("background_color").unwrap().r, 1);
        let mode = &g.children[3];
        assert_eq!(
            (mode.text("text"), mode.f64("y"), mode.f64("width")),
            (Some("Mode"), Some(27.0), Some(156.0))
        );
    }

    #[test]
    fn xy_graphs_number_their_axes_and_traces() {
        let w = one(
            "xyGraph",
            r#"<axis_count>3</axis_count><axis_0_axis_title>Time</axis_0_axis_title>
            <axis_1_log_scale>true</axis_1_log_scale><axis_1_auto_scale>true</axis_1_auto_scale>
            <axis_2_y_axis>true</axis_2_y_axis><axis_2_left_bottom_side>false</axis_2_left_bottom_side>
            <trace_count>2</trace_count>
            <trace_0_y_pv>G1</trace_0_y_pv><trace_0_name>$(trace_0_y_pv)</trace_0_name>
            <trace_0_trace_color><color red="21" green="21" blue="196"/></trace_0_trace_color>
            <trace_1_y_pv>G2</trace_1_y_pv><trace_1_y_axis_index>2</trace_1_y_axis_index>
            <trace_1_trace_type>2</trace_1_trace_type><trace_1_point_style>9</trace_1_point_style>
            <trace_1_concatenate_data>true</trace_1_concatenate_data>
            <trace_1_buffer_size>250</trace_1_buffer_size><trace_1_update_mode>3</trace_1_update_mode>
            <trace_2_y_pv>unused</trace_2_y_pv>
            <plot_area_background_color><color red="255" green="255" blue="255"/></plot_area_background_color>
            <tooltip>$(trace_0_y_pv)</tooltip>
            <rules><rule name="v" prop_id="trace_1_visible" out_exp="false">
              <exp bool_exp="(pv0 &amp; 4) == 4"><value>true</value></exp><pv trig="true">M</pv>
            </rule></rules>"#,
        );
        let x = w.get("x_axis").unwrap();
        assert_eq!(x.child_text("title"), Some("Time"));
        let y: Vec<&Element> = w.get("y_axes").unwrap().children.iter().collect();
        assert_eq!(y.len(), 2);
        assert_eq!(y[0].child_text("log_scale"), Some("true"));
        assert_eq!(y[0].child_text("autoscale"), Some("true"));
        assert_eq!(y[1].child_text("on_right"), Some("true"));
        let t = &w.get("traces").unwrap().children;
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].child_text("name"), Some("$(traces[0].y_pv)"));
        assert_eq!(
            t[0].child("color").unwrap().children[0].attr("blue"),
            Some("196")
        );
        assert_eq!(
            ["axis", "trace_type", "point_type"].map(|k| t[1].child_text(k)),
            [Some("1"), Some("0"), Some("4")]
        );
        assert_eq!(
            ["concatenate_data", "buffer_size", "update_mode"].map(|k| t[1].child_text(k)),
            [Some("true"), Some("250"), Some("3")]
        );
        assert_eq!(w.text("tooltip"), Some("$(traces[0].y_pv)"));
        assert_eq!(w.rules()[0].prop_id, "traces[1].visible");
        assert_eq!(w.color("background_color").unwrap().r, 255);
        assert!(w.get("trace_2_y_pv").is_none());
        assert_eq!(
            trace_refs(
                "trace_10_visible, trace_x, trace_0_other",
                &[("visible", "visible")]
            ),
            "traces[10].visible, trace_x, trace_0_other"
        );
    }

    #[test]
    fn xy_graphs_keep_their_pv_as_a_macro() {
        let w = one(
            "xyGraph",
            r#"<pv_name>$(trace_1_y_pv)</pv_name><trace_count>2</trace_count>
            <trace_0_y_pv>G1</trace_0_y_pv><trace_1_y_pv>$(P):G2</trace_1_y_pv>
            <tooltip>$(pv_name)</tooltip>"#,
        );
        assert!(w.get("pv_name").is_none());
        assert_eq!(w.macros().get("pv_name"), Some("$(P):G2"));
        let w = one("xyGraph", "<pv_name></pv_name>");
        assert!(w.get("pv_name").is_none());
        assert_eq!(w.macros().get("pv_name"), None);
    }

    #[test]
    fn tabbed_containers_hold_one_group_per_tab() {
        let w = one(
            "tab",
            r#"<tab_count>2</tab_count><tab_0_title>G1</tab_0_title><tab_1_title>G2</tab_1_title>
            <tab_0_foreground_color><color red="1" green="2" blue="3"/></tab_0_foreground_color>
            <foreground_color><color red="192" green="192" blue="192"/></foreground_color>
            <horizontal_tabs>true</horizontal_tabs><minimum_tab_height>10</minimum_tab_height>
            <widget typeId="org.csstudio.opibuilder.widgets.groupingContainer"><name>G1</name>
              <widget typeId="org.csstudio.opibuilder.widgets.Label"><text>one</text></widget>
            </widget>
            <widget typeId="org.csstudio.opibuilder.widgets.groupingContainer"><name>G2</name></widget>"#,
        );
        assert!(w.children.is_empty());
        let tabs = &w.get("tabs").unwrap().children;
        let names: Vec<_> = tabs.iter().map(|t| t.child_text("name").unwrap()).collect();
        assert_eq!(names, ["G1", "G2"]);
        let group = &tabs[0].child("children").unwrap().children[0];
        assert_eq!(group.attr("type"), Some("group"));
        assert_eq!(group.children.last().unwrap().attr("type"), Some("label"));
        assert_eq!(w.color("foreground_color").unwrap().r, 1);
        assert_eq!(
            (w.i64("direction"), w.f64("tab_height")),
            (Some(0), Some(30.0))
        );
        assert!(w.get("tab_1_title").is_none());
    }

    #[test]
    fn menu_muxes_are_combos_on_their_first_target() {
        let w = one(
            "edm.menumux",
            r#"<pv_name></pv_name><items><s>NORMAL</s><s>BL</s></items>
            <target0>loc://$(DID)CON&lt;VString&gt;</target0><values0><s>CON</s><s>BLCON</s></values0>"#,
        );
        assert_eq!(w.text("pv_name"), Some("loc://$(DID)CON<VString>"));
        assert_eq!(w.list("items", "item"), ["NORMAL", "BL"]);
        assert_eq!(w.bool("items_from_pv"), Some(false));
        assert!(w.get("values0").is_none());
        // The sets are kept: picking an item writes its value to each target.
        let mux = w.menu_mux().unwrap();
        assert!(!mux.index);
        assert_eq!(mux.writes(1), [("loc://$(DID)CON<VString>", "BLCON")]);
        assert_eq!(mux.current(None, "BLCON"), Some(1));
    }

    #[test]
    fn menu_muxes_write_every_target_and_their_initial_item() {
        let w = one(
            "edm.menumux",
            r#"<pv_name></pv_name><initial>1</initial><num_sets>2</num_sets>
            <items><s>Rate 1s</s><s>Log Rate 1s</s></items>
            <target0>loc://$(DID)ll&lt;VString&gt;</target0><target1>loc://$(DID)lld&lt;VString&gt;</target1>
            <target2></target2>
            <values0><s>RATE</s><s>LOG</s></values0><values1><s>LOG</s><s>LOG</s></values1><values2 />"#,
        );
        let mux = w.menu_mux().unwrap();
        assert_eq!(mux.initial, Some(1));
        assert_eq!(
            mux.writes(0),
            [
                ("loc://$(DID)ll<VString>", "RATE"),
                ("loc://$(DID)lld<VString>", "LOG")
            ]
        );
        assert_eq!(mux.current(None, "LOG"), Some(1));
        assert_eq!(mux.current(None, "DID"), None);
    }

    #[test]
    fn menu_muxes_with_their_own_pv_write_the_index() {
        let w = one(
            "edm.menumux",
            r#"<initial></initial><num_sets>1</num_sets>
            <items><s>Hide Required BPMs</s><s>Show Required BPMs</s></items>
            <pv_name>loc://show-required&lt;VEnum&gt;(0, "hide", "show")</pv_name>
            <target0></target0><values0><s>0</s><s>1</s></values0>"#,
        );
        assert_eq!(
            w.text("pv_name"),
            Some(r#"loc://show-required<VEnum>(0, "hide", "show")"#)
        );
        let mux = w.menu_mux().unwrap();
        assert!(mux.index && mux.sets.is_empty() && mux.initial.is_none());
        assert_eq!(mux.current(Some(1), "show"), Some(1));
    }

    #[test]
    fn visible_scripts_become_rules_on_their_formula() {
        // As motor.opi's alarm labels, stacked at one place.
        let script = |pv: &str| {
            format!(
                r#"<rules /><visible>true</visible><scripts>
                <path pathString="visible.py" checkConnect="true"><pv trig="true">{pv}</pv></path>
                </scripts>"#
            )
        };
        let w = one(
            "Label",
            &script(r#"loc://$(DID)eloss2("='$(motor):ELOSS' == 2 || '$(motor):ELOSS' == 3")"#),
        );
        assert!(!w.has_scripts());
        assert_eq!(w.text("visible"), Some("false"));
        let r = &w.rules()[0];
        assert_eq!(r.prop_id, "visible");
        assert!(!r.out_exp && r.check_connections);
        assert_eq!(
            r.pv_names,
            ["='$(motor):ELOSS' == 2 || '$(motor):ELOSS' == 3"]
        );
        assert_eq!(r.exprs[0].condition, "pv0");
        assert_eq!(r.exprs[0].value.text, "true");
        // A PV that names nothing: hidden, as the script's first line.
        let w = one("Label", &script("X:Y"));
        assert!(!w.has_scripts() && w.rules().is_empty());
        assert_eq!(w.text("visible"), Some("false"));
        // Other scripts are kept.
        let w = one(
            "Label",
            r#"<scripts><path pathString="scripts/visible.py.bak"><pv>A</pv></path></scripts>"#,
        );
        assert!(w.has_scripts() && w.text("visible").is_none());
    }

    #[test]
    fn display_props_and_noise_are_dropped() {
        let d = opi(
            r#"<background_color><color name="Canvas" red="200" green="200" blue="200"/></background_color>
            <macros><include_parent_macros>true</include_parent_macros><dom>SR</dom></macros>
            <widget typeId="org.csstudio.opibuilder.widgets.Label"><wuid>1:2</wuid><widget_type>Label</widget_type><scripts><path>x.js</path></scripts></widget>"#,
        );
        assert_eq!(d.background.rgba(), [200, 200, 200, 255]);
        assert_eq!(d.macros.get("dom"), Some("SR"));
        let w = &d.widgets[0];
        assert!(w.get("wuid").is_none() && w.get("widget_type").is_none() && w.has_scripts());
    }

    #[test]
    fn get_pv_name_scripts_become_group_links() {
        let script = |pv: &str| {
            format!(
                r#"<scripts><path pathString="../../scripts/get_pv_name.py" checkConnect="false" sfe="false" seoe="false">
                <pv trig="true">sim://ramp(0,1,1,1)</pv><pv trig="true">{pv}</pv></path></scripts>"#
            )
        };
        let sym = one(
            "edm.symbolwidget",
            &format!(
                "<widget_type>Edm Symbol Widget</widget_type><name>LOOKATME</name><pv_name></pv_name>
                <image_file>../../../digitelMpc/digitelMpcIonp-symbol-34.png</image_file>{}",
                script("$(device):MAXSTA.INP$(inp)")
            ),
        );
        assert_eq!(
            sym.group_link(),
            Some(GroupLink {
                pv_name: "$(device):MAXSTA.INP$(inp)".into(),
                sets: LinkSets::Pv,
                frames: Some(9),
            })
        );
        assert!(!sym.has_scripts());
        let label = one(
            "Label",
            &format!(
                "<widget_type>Label</widget_type><text>SRXXC-VA-GXXX-YY</text>{}",
                script("$(dom)-VA-SPACE-$(id):IONPSTA.INPA")
            ),
        );
        let link = label.group_link().unwrap();
        assert_eq!(link.sets, LinkSets::Text);
        assert_eq!(link.pv_name, "$(dom)-VA-SPACE-$(id):IONPSTA.INPA");
        // A strip it has no frame count for keeps its image_index.
        let other = one(
            "edm.symbolwidget",
            &format!(
                "<widget_type>Edm Symbol Widget</widget_type><image_file>x-symbol.png</image_file>{}",
                script("L")
            ),
        );
        assert_eq!(other.group_link().unwrap().frames, None);
        // Other scripts, or this one without its link PV, are ignored.
        let none = one(
            "Label",
            r#"<scripts><path pathString="scripts/other.py"><pv>A</pv><pv>B</pv></path>
            <path pathString="get_pv_name.py"><pv>A</pv></path></scripts>"#,
        );
        assert_eq!(none.group_link(), None);
        // Those ignored scripts are still counted; a group link's are not.
        assert!(none.has_scripts());
        assert!(!sym.has_scripts() && !label.has_scripts());
    }
}
