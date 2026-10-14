//! Colours, fonts and coordinates shared by the widget painters.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontFamily, FontId, Pos2, Rect, Vec2};

/// Maps display coordinates to screen points.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub origin: Pos2,
    pub zoom: f32,
}

impl View {
    pub fn rect(&self, r: &[f64; 4]) -> Rect {
        Rect::from_min_size(
            self.origin + Vec2::new(r[0] as f32, r[1] as f32) * self.zoom,
            Vec2::new(r[2] as f32, r[3] as f32) * self.zoom,
        )
    }

    pub fn pos(&self, x: f64, y: f64) -> Pos2 {
        self.origin + Vec2::new(x as f32, y as f32) * self.zoom
    }
}

pub fn color(c: &bob::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

pub fn named(name: &str) -> Color32 {
    color(&bob::named_color(name))
}

/// Alarm colours from the default Phoebus palette.
pub fn severity_color(sev: pv::Severity) -> Option<Color32> {
    match sev {
        pv::Severity::NoAlarm => None,
        pv::Severity::Minor => Some(named("MINOR")),
        pv::Severity::Major => Some(named("MAJOR")),
        pv::Severity::Invalid => Some(named("INVALID")),
    }
}

/// Font families registered at startup; regular is egui's proportional.
pub const BOLD: &str = "bold";
pub const ITALIC: &str = "italic";

pub fn font_id(font: &bob::Font, zoom: f32) -> FontId {
    let family = if font.is_bold() {
        FontFamily::Name(BOLD.into())
    } else if font.is_italic() {
        FontFamily::Name(ITALIC.into())
    } else if font.family.to_lowercase().contains("mono") {
        FontFamily::Monospace
    } else {
        FontFamily::Proportional
    };
    // Phoebus sizes are in pixels; egui sizes the em box, which renders a little larger.
    FontId::new((font.size * zoom * 0.95).max(4.0), family)
}

/// Install Liberation Sans, the font Phoebus uses, ahead of egui's built-in
/// fonts (which stay as fallbacks for symbols).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |key: &str, data: &'static [u8]| {
        fonts
            .font_data
            .insert(key.into(), Arc::new(egui::FontData::from_static(data)));
        key.to_string()
    };
    let regular = add(
        "liberation",
        include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    );
    let bold = add(
        "liberation-bold",
        include_bytes!("../fonts/LiberationSans-Bold.ttf"),
    );
    let italic = add(
        "liberation-italic",
        include_bytes!("../fonts/LiberationSans-Italic.ttf"),
    );
    // Symbols that neither Liberation nor egui's fonts have, such as the
    // '⧉' pvi puts on buttons that open a display (Phoebus finds them
    // through system font fallback).
    let symbols = add(
        "noto-math-symbols",
        include_bytes!("../fonts/NotoSansMath-Symbols.ttf"),
    );

    let mut fallback = fonts.families[&FontFamily::Proportional].clone();
    fallback.push(symbols.clone());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push(symbols);
    let with = |first: String| {
        std::iter::once(first)
            .chain(fallback.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Name(BOLD.into()), with(bold));
    fonts
        .families
        .insert(FontFamily::Name(ITALIC.into()), with(italic));
    fonts
        .families
        .insert(FontFamily::Proportional, with(regular));
    ctx.set_fonts(fonts);
}

/// Horizontal / vertical alignment as Phoebus numbers them (0 start, 1 centre, 2 end).
pub fn align(h: i64, v: i64) -> egui::Align2 {
    let a = |n| match n {
        1 => egui::Align::Center,
        2 => egui::Align::Max,
        _ => egui::Align::Min,
    };
    egui::Align2([a(h), a(v)])
}
