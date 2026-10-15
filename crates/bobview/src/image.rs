//! The image widget: a PVA NTNDArray (or a CA waveform shaped by
//! `data_width` x `data_height`) drawn through a colour map, with a colour bar
//! and, when `show_toolbar` is set, the Phoebus zoom/pan/crosshair toolbar.

use std::sync::Arc;

use bob::Widget;
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, Vec2,
};
use pv::{Image, ImageData, PvState, Value};

use crate::axis::{self, Scale};
use crate::colormaps::PREDEFINED;
use crate::screen::Node;
use crate::widgets::{bg, button_box, fg, fill, outline, placeholder};

/// Largest texture side we upload; bigger images are decimated.
const MAX_SIDE: usize = 2048;

/// Height of the toolbar, before zoom.
const TOOLBAR: f32 = 26.0;

/// Smooth when shrinking a big image, square pixels when zoomed in.
const PIXELS: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    ..egui::TextureOptions::LINEAR
};

/// The texture and the user's view of one image widget.
#[derive(Default)]
pub struct ImageCache {
    tex: Option<Texture>,
    view: Zoom,
    mode: Mode,
    /// Whether the crosshair is on; starts from `cursor_crosshair`.
    crosshair: Option<bool>,
    /// Where the crosshair was placed, in image fractions.
    cross_at: Option<Pos2>,
    /// Screen position where a rubber-band zoom started.
    band_from: Option<Pos2>,
}

struct Texture {
    /// Identifies what is on the texture: frame unique id, data pointer and
    /// the display settings, so a rule changing the colour map redraws it.
    key: (i64, usize, String),
    texture: TextureHandle,
    /// The data range the colours were scaled to.
    range: (f64, f64),
}

/// What dragging and clicking on the image does, as picked on the toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Pointer,
    ZoomIn,
    ZoomOut,
    Pan,
}

/// The part of the image shown, in fractions of its size (y down), with
/// undo and redo history.
#[derive(Debug)]
struct Zoom {
    shown: Rect,
    undo: Vec<Rect>,
    redo: Vec<Rect>,
}

fn whole() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            shown: whole(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl Zoom {
    /// Show `r`, kept inside the image and no larger than it.
    fn set(&mut self, r: Rect) {
        let size = r.size().min(Vec2::splat(1.0)).max(Vec2::splat(1e-4));
        let min = r.min.clamp(Pos2::ZERO, (Vec2::splat(1.0) - size).to_pos2());
        let r = Rect::from_min_size(min, size);
        if r != self.shown {
            self.undo.push(self.shown);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.shown = r;
        }
    }

    /// Scale the view by `factor` (below 1 zooms in) about `at`, in image fractions.
    fn scale_about(&mut self, at: Pos2, factor: f32) {
        let s = self.shown;
        let min = at + (s.min - at) * factor;
        self.set(Rect::from_min_size(min, s.size() * factor));
    }

    fn pan(&mut self, by: Vec2) {
        let s = self.shown;
        let size = s.size();
        let min = (s.min - by).clamp(Pos2::ZERO, (Vec2::splat(1.0) - size).to_pos2());
        // Panning moves continuously, so it is not recorded step by step.
        self.shown = Rect::from_min_size(min, size);
    }

    fn undo(&mut self) {
        if let Some(r) = self.undo.pop() {
            self.redo.push(self.shown);
            self.shown = r;
        }
    }

    fn redo(&mut self) {
        if let Some(r) = self.redo.pop() {
            self.undo.push(self.shown);
            self.shown = r;
        }
    }

    /// Image fraction under screen point `p` of `area`.
    fn to_image(&self, area: Rect, p: Pos2) -> Pos2 {
        let t = (p - area.min) / area.size();
        self.shown.min + t * self.shown.size()
    }

    /// Screen point of image fraction `f` in `area`.
    fn to_screen(&self, area: Rect, f: Pos2) -> Pos2 {
        area.min + (f - self.shown.min) / self.shown.size() * area.size()
    }
}
/// 256 colours from the widget's `color_map` (default VIRIDIS).
fn lut(w: &Widget) -> [Color32; 256] {
    let el = w.get("color_map");
    let custom: Vec<[u8; 4]> = el
        .map(|e| {
            e.children
                .iter()
                .filter(|c| c.name == "section")
                .filter_map(|c| {
                    let a = |k: &str| c.attr(k)?.trim().parse::<u8>().ok();
                    Some([a("value")?, a("red")?, a("green")?, a("blue")?])
                })
                .collect()
        })
        .unwrap_or_default();
    let name = el.and_then(|e| e.child_text("name")).unwrap_or("VIRIDIS");
    let sections: &[[u8; 4]] = if custom.len() >= 2 {
        &custom
    } else {
        PREDEFINED
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .unwrap_or(&PREDEFINED[0])
            .1
    };
    let mut out = [Color32::BLACK; 256];
    for (i, c) in out.iter_mut().enumerate() {
        let i = i as f32;
        let hi = sections
            .iter()
            .position(|s| s[0] as f32 >= i)
            .unwrap_or(sections.len() - 1);
        let lo = hi.saturating_sub(1);
        let (a, b) = (sections[lo], sections[hi]);
        let t = if b[0] > a[0] {
            (i - a[0] as f32) / (b[0] - a[0]) as f32
        } else {
            0.0
        };
        let mix = |k: usize| {
            (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t.clamp(0.0, 1.0)).round() as u8
        };
        *c = Color32::from_rgb(mix(1), mix(2), mix(3));
    }
    out
}

/// The image in `s`: an NTNDArray, or a numeric array shaped by the widget.
fn image_of(w: &Widget, s: &PvState) -> Option<Arc<Image>> {
    let shaped = |data: ImageData| {
        let width = w.i64("data_width").unwrap_or(100).max(1) as usize;
        let height = w.i64("data_height").unwrap_or(100).max(1) as usize;
        width
            .checked_mul(height)
            .is_some_and(|n| data.len() >= n)
            .then(|| {
                Arc::new(Image {
                    width,
                    height,
                    rgb: false,
                    data,
                    unique_id: 0,
                })
            })
    };
    match &s.value {
        Value::Image(img) => Some(img.clone()),
        Value::DoubleArray(a) => shaped(ImageData::F64(a.clone())),
        Value::LongArray(a) => shaped(ImageData::I64(a.clone())),
        _ => None,
    }
}

fn data_key(img: &Image) -> usize {
    match &img.data {
        ImageData::I8(a) => a.as_ptr() as usize,
        ImageData::U8(a) => a.as_ptr() as usize,
        ImageData::I16(a) => a.as_ptr() as usize,
        ImageData::U16(a) => a.as_ptr() as usize,
        ImageData::I32(a) => a.as_ptr() as usize,
        ImageData::U32(a) => a.as_ptr() as usize,
        ImageData::I64(a) => a.as_ptr() as usize,
        ImageData::U64(a) => a.as_ptr() as usize,
        ImageData::F32(a) => a.as_ptr() as usize,
        ImageData::F64(a) => a.as_ptr() as usize,
    }
}

/// Colour the image. Returns the pixels and the data range used.
fn colorize(
    img: &Image,
    unsigned: bool,
    range: Option<(f64, f64)>,
    lut: &[Color32; 256],
) -> (ColorImage, (f64, f64)) {
    if let Some(fast) = colorize_bytes(img, unsigned, range, lut) {
        return fast;
    }
    let step = img.width.max(img.height).div_ceil(MAX_SIDE).max(1);
    let (w, h) = (img.width.div_ceil(step), img.height.div_ceil(step));
    let channels = if img.rgb { 3 } else { 1 };
    let sample = |x: usize, y: usize, c: usize| {
        img.data
            .get((y * step * img.width + x * step) * channels + c, unsigned)
    };

    let (lo, hi) = range.unwrap_or_else(|| {
        let mut r = (f64::INFINITY, f64::NEG_INFINITY);
        for y in 0..h {
            for x in 0..w {
                for c in 0..channels {
                    let v = sample(x, y, c);
                    if v.is_finite() {
                        r = (r.0.min(v), r.1.max(v));
                    }
                }
            }
        }
        if r.0 > r.1 { (0.0, 1.0) } else { r }
    });
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
    let level = |v: f64| ((v - lo) * scale).clamp(0.0, 255.0) as u8;

    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(if img.rgb {
                Color32::from_rgb(
                    level(sample(x, y, 0)),
                    level(sample(x, y, 1)),
                    level(sample(x, y, 2)),
                )
            } else {
                lut[level(sample(x, y, 0)) as usize]
            });
        }
    }
    (
        ColorImage {
            size: [w, h],
            source_size: Vec2::new(w as f32, h as f32),
            pixels,
        },
        (lo, hi),
    )
}

/// Fast path for 8-bit mono images: colour every byte value once.
fn colorize_bytes(
    img: &Image,
    unsigned: bool,
    range: Option<(f64, f64)>,
    lut: &[Color32; 256],
) -> Option<(ColorImage, (f64, f64))> {
    let n = img.width * img.height;
    if img.rgb || img.width > MAX_SIDE || img.height > MAX_SIDE {
        return None;
    }
    let (bytes, signed): (&[u8], bool) = match &img.data {
        ImageData::U8(a) => (&a[..n], false),
        // Reinterpreting i8 as u8 keeps the bit patterns; the table maps them back.
        ImageData::I8(a) => (
            // SAFETY: i8 and u8 have the same size and alignment.
            unsafe { std::slice::from_raw_parts(a.as_ptr().cast::<u8>(), n) },
            !unsigned,
        ),
        _ => return None,
    };
    let value = |b: u8| if signed { b as i8 as f64 } else { b as f64 };
    let (lo, hi) = range.unwrap_or_else(|| {
        let mut seen = [false; 256];
        for &b in bytes {
            seen[b as usize] = true;
        }
        let vals = (0..=255u8).filter(|b| seen[*b as usize]).map(value);
        vals.fold((f64::INFINITY, f64::NEG_INFINITY), |r, v| {
            (r.0.min(v), r.1.max(v))
        })
    });
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
    let table: [Color32; 256] =
        std::array::from_fn(|b| lut[((value(b as u8) - lo) * scale).clamp(0.0, 255.0) as usize]);
    let pixels = bytes.iter().map(|b| table[*b as usize]).collect();
    Some((
        ColorImage {
            size: [img.width, img.height],
            source_size: Vec2::new(img.width as f32, img.height as f32),
            pixels,
        },
        (lo, hi),
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    node: &mut Node,
    state: Option<&PvState>,
    rect: Rect,
    clip: Rect,
    id: Id,
    z: f32,
) -> Response {
    let w = node.w.clone();
    let clip = clip.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip.intersect(rect));
    fill(&painter, rect, bg(&w, "Background"));
    let cache = node.image.get_or_insert_with(ImageCache::default);
    let crosshair = *cache
        .crosshair
        .get_or_insert_with(|| w.bool("cursor_crosshair").unwrap_or(false));

    let mut body = rect;
    if w.bool("show_toolbar") == Some(true) {
        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TOOLBAR * z));
        body.min.y = bar.bottom();
        toolbar(ui, &painter, cache, bar, clip, id, z);
    }

    let bar = w.get("color_bar");
    let bar_visible = bar
        .and_then(|b| b.child_text("visible"))
        .map(|v| v.trim() != "false")
        .unwrap_or(true);
    let bar_size = bar
        .and_then(|b| b.child_text("bar_size"))
        .and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(40.0)
        * z;
    let (area, bar_rect) = if bar_visible && body.width() > 3.0 * bar_size {
        let gap = 4.0 * z;
        let area = Rect::from_min_max(
            body.min,
            Pos2::new(body.right() - bar_size - gap, body.bottom()),
        );
        let bar_rect = Rect::from_min_max(Pos2::new(body.right() - bar_size, body.top()), body.max);
        (area, Some(bar_rect))
    } else {
        (body, None)
    };

    // Only the zoom-in and pan modes take drags, so that otherwise a drag on
    // the image scrolls the display, as it must on a touch screen.
    let sense = match cache.mode {
        Mode::ZoomIn | Mode::Pan => Sense::click_and_drag(),
        Mode::Pointer | Mode::ZoomOut => Sense::click(),
    };
    let resp = ui.interact(area.intersect(clip), id, sense);
    mouse(ui, cache, &resp, area, crosshair);

    let img = state.filter(|s| s.connected).and_then(|s| image_of(&w, s));
    let Some(img) = img else {
        let msg = match state {
            Some(s) if s.connected => "no image yet".to_string(),
            Some(s) => s
                .error
                .clone()
                .unwrap_or_else(|| format!("<{}>", node.pv_name)),
            None => "image".to_string(),
        };
        placeholder(&painter, area, &msg, z);
        return resp;
    };

    let lut = lut(&w);
    let unsigned = w.bool("unsigned").unwrap_or(false);
    let fixed = (w.bool("autoscale") == Some(false)).then(|| {
        (
            w.f64("minimum").unwrap_or(0.0),
            w.f64("maximum").unwrap_or(255.0),
        )
    });
    let settings = format!("{:?}{unsigned}{fixed:?}", w.get("color_map"));
    let key = (img.unique_id, data_key(&img), settings);
    if cache.tex.as_ref().map(|c| &c.key) != Some(&key) {
        let (pixels, range) = colorize(&img, unsigned, fixed, &lut);
        match cache.tex.as_mut() {
            Some(tex) => {
                tex.texture.set(pixels, PIXELS);
                tex.key = key;
                tex.range = range;
            }
            None => {
                let texture = ui.ctx().load_texture("bobview-image", pixels, PIXELS);
                cache.tex = Some(Texture {
                    key,
                    texture,
                    range,
                });
            }
        }
    }
    let Some(tex) = &cache.tex else { return resp };
    // The whole image, placed so the zoomed part fills the area.
    let full = Rect::from_min_max(
        cache.view.to_screen(area, Pos2::ZERO),
        cache.view.to_screen(area, Pos2::new(1.0, 1.0)),
    );
    let inside = painter.with_clip_rect(painter.clip_rect().intersect(area));
    inside.image(tex.texture.id(), full, whole(), Color32::WHITE);

    if let Some(from) = cache.band_from
        && let Some(to) = resp.interact_pointer_pos()
    {
        inside.rect_stroke(
            Rect::from_two_pos(from, to),
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
    }
    if crosshair && let Some(at) = cache.cross_at {
        draw_crosshair(
            &inside,
            &img,
            unsigned,
            cache.view.to_screen(area, at),
            at,
            area,
            z,
        );
    }

    if let Some(bar_rect) = bar_rect {
        colour_bar(&painter, &w, bar_rect, &lut, tex.range, z);
    }
    resp
}

/// Zoom, pan and crosshair placement with the mouse, per the toolbar mode.
fn mouse(ui: &egui::Ui, cache: &mut ImageCache, resp: &Response, area: Rect, crosshair: bool) {
    let view = &mut cache.view;
    match cache.mode {
        Mode::ZoomIn => {
            if resp.drag_started() {
                cache.band_from = resp.interact_pointer_pos();
            }
            if resp.drag_stopped()
                && let (Some(a), Some(b)) = (cache.band_from.take(), resp.interact_pointer_pos())
            {
                let band = Rect::from_two_pos(a, b);
                if band.width() > 4.0 && band.height() > 4.0 {
                    let r = Rect::from_two_pos(view.to_image(area, a), view.to_image(area, b));
                    view.set(r);
                }
            } else if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                view.scale_about(view.to_image(area, p), 0.5);
            }
        }
        Mode::ZoomOut => {
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                view.scale_about(view.to_image(area, p), 2.0);
            }
        }
        Mode::Pan => {
            if resp.drag_started() {
                // One undo step per drag.
                let shown = view.shown;
                view.undo.push(shown);
                view.redo.clear();
            }
            if resp.dragged() {
                let d = resp.drag_delta() / area.size() * view.shown.size();
                view.pan(d);
            }
        }
        Mode::Pointer => {}
    }
    if crosshair
        && cache.mode == Mode::Pointer
        && resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        cache.cross_at = Some(cache.view.to_image(area, p));
    }
    if !resp.dragged() && !resp.drag_stopped() {
        cache.band_from = None;
    }
    if resp.hovered() {
        let icon = match cache.mode {
            Mode::Pan => egui::CursorIcon::Grab,
            Mode::ZoomIn => egui::CursorIcon::ZoomIn,
            Mode::ZoomOut => egui::CursorIcon::ZoomOut,
            Mode::Pointer if crosshair => egui::CursorIcon::Crosshair,
            Mode::Pointer => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }
}

/// Crosshair lines through `p`, with the pixel and its value.
fn draw_crosshair(
    painter: &egui::Painter,
    img: &Image,
    unsigned: bool,
    p: Pos2,
    at: Pos2,
    area: Rect,
    z: f32,
) {
    for (c, width) in [(Color32::BLACK, 3.0), (Color32::WHITE, 1.0)] {
        let s = Stroke::new(width * z.max(1.0), c);
        painter.line_segment(
            [Pos2::new(area.left(), p.y), Pos2::new(area.right(), p.y)],
            s,
        );
        painter.line_segment(
            [Pos2::new(p.x, area.top()), Pos2::new(p.x, area.bottom())],
            s,
        );
    }
    let x = ((at.x * img.width as f32) as usize).min(img.width.saturating_sub(1));
    let y = ((at.y * img.height as f32) as usize).min(img.height.saturating_sub(1));
    let channels = if img.rgb { 3 } else { 1 };
    let i = (y * img.width + x) * channels;
    let values: Vec<String> = (0..channels)
        .filter(|c| i + c < img.data.len())
        .map(|c| {
            let v = img.data.get(i + c, unsigned);
            if v.fract() == 0.0 {
                format!("{v}")
            } else {
                format!("{v:.3}")
            }
        })
        .collect();
    let text = format!("{x}, {y}: {}", values.join(", "));
    let font = FontId::proportional(12.0 * z.max(0.6));
    let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
    let at = area.min + Vec2::splat(4.0 * z);
    let back = Rect::from_min_size(at, galley.size()).expand(2.0 * z);
    painter.rect_filled(back, 2.0, Color32::from_black_alpha(160));
    painter.galley(at, galley, Color32::WHITE);
}

/// The toolbar buttons, left to right.
const BUTTONS: [(&str, Button); 7] = [
    ("Crosshair", Button::Crosshair),
    ("Zoom in", Button::Mode(Mode::ZoomIn)),
    ("Zoom out", Button::Mode(Mode::ZoomOut)),
    ("Pan", Button::Mode(Mode::Pan)),
    ("Pointer", Button::Mode(Mode::Pointer)),
    ("Undo zoom", Button::Undo),
    ("Redo zoom", Button::Redo),
];

#[derive(Clone, Copy)]
enum Button {
    Crosshair,
    Mode(Mode),
    Undo,
    Redo,
}

fn toolbar(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    cache: &mut ImageCache,
    bar: Rect,
    clip: Rect,
    id: Id,
    z: f32,
) {
    fill(painter, bar, Color32::from_gray(236));
    painter.line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        Stroke::new(1.0, Color32::from_gray(190)),
    );
    let size = bar.height() - 4.0 * z;
    let mut x = bar.left() + 2.0 * z;
    for (i, (tip, button)) in BUTTONS.iter().enumerate() {
        // Separators after the crosshair and after the mouse modes.
        if i == 1 || i == 5 {
            x += 6.0 * z;
        }
        let r = Rect::from_min_size(Pos2::new(x, bar.top() + 2.0 * z), Vec2::splat(size));
        x += size + 2.0 * z;
        let resp = ui.interact(r.intersect(clip), id.with(("toolbar", i)), Sense::click());
        let (on, enabled) = match button {
            Button::Crosshair => (cache.crosshair == Some(true), true),
            Button::Mode(m) => (cache.mode == *m, true),
            Button::Undo => (false, !cache.view.undo.is_empty()),
            Button::Redo => (false, !cache.view.redo.is_empty()),
        };
        button_box(
            painter,
            r,
            if on {
                Color32::from_gray(200)
            } else {
                Color32::from_gray(236)
            },
            on || resp.is_pointer_button_down_on(),
            z,
        );
        let ink = if enabled {
            Color32::from_gray(40)
        } else {
            Color32::from_gray(160)
        };
        icon(painter, *button, r.shrink(5.0 * z), ink, z);
        if resp.clicked() && enabled {
            match button {
                Button::Crosshair => cache.crosshair = Some(cache.crosshair != Some(true)),
                Button::Mode(m) => cache.mode = *m,
                Button::Undo => cache.view.undo(),
                Button::Redo => cache.view.redo(),
            }
        }
        resp.on_hover_text(*tip);
    }
}

fn icon(painter: &egui::Painter, button: Button, r: Rect, c: Color32, z: f32) {
    let s = Stroke::new(1.5 * z.max(0.7), c);
    let lens = |sign: Option<bool>| {
        let centre = r.min + r.size() * 0.4;
        let rad = r.width() * 0.33;
        painter.circle_stroke(centre, rad, s);
        let d = Vec2::splat(rad * std::f32::consts::FRAC_1_SQRT_2);
        painter.line_segment([centre + d, r.max], Stroke::new(s.width * 1.6, c));
        if let Some(plus) = sign {
            let h = rad * 0.55;
            painter.line_segment([centre - Vec2::new(h, 0.0), centre + Vec2::new(h, 0.0)], s);
            if plus {
                painter.line_segment([centre - Vec2::new(0.0, h), centre + Vec2::new(0.0, h)], s);
            }
        }
    };
    let arrow_head = |tip: Pos2, dir: Vec2| {
        let n = Vec2::new(-dir.y, dir.x);
        let l = r.width() * 0.25;
        painter.add(Shape::convex_polygon(
            vec![
                tip,
                tip - dir * l + n * l * 0.6,
                tip - dir * l - n * l * 0.6,
            ],
            c,
            Stroke::NONE,
        ));
    };
    match button {
        Button::Crosshair => {
            painter.circle_stroke(r.center(), r.width() * 0.3, s);
            painter.line_segment([r.left_center(), r.right_center()], s);
            painter.line_segment([r.center_top(), r.center_bottom()], s);
        }
        Button::Mode(Mode::ZoomIn) => lens(Some(true)),
        Button::Mode(Mode::ZoomOut) => lens(Some(false)),
        Button::Mode(Mode::Pan) => {
            painter.line_segment([r.left_center(), r.right_center()], s);
            painter.line_segment([r.center_top(), r.center_bottom()], s);
            arrow_head(r.left_center(), Vec2::new(-1.0, 0.0));
            arrow_head(r.right_center(), Vec2::new(1.0, 0.0));
            arrow_head(r.center_top(), Vec2::new(0.0, -1.0));
            arrow_head(r.center_bottom(), Vec2::new(0.0, 1.0));
        }
        Button::Mode(Mode::Pointer) => {
            let p = |x: f32, y: f32| r.min + r.size() * Vec2::new(x, y);
            painter.add(Shape::convex_polygon(
                vec![p(0.2, 0.0), p(0.85, 0.6), p(0.5, 0.62), p(0.2, 0.95)],
                c,
                Stroke::NONE,
            ));
            painter.line_segment([p(0.5, 0.6), p(0.7, 1.0)], Stroke::new(s.width * 1.5, c));
        }
        Button::Undo | Button::Redo => {
            let redo = matches!(button, Button::Redo);
            let flip = |p: Pos2| {
                if redo {
                    Pos2::new(r.left() + r.right() - p.x, p.y)
                } else {
                    p
                }
            };
            let centre = r.center() + Vec2::new(0.0, r.height() * 0.15);
            let rad = r.width() * 0.38;
            // An arc over the top, ending in a head pointing down-left.
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32 / 12.0);
                    flip(centre + Vec2::new(rad * a.cos(), -rad * a.sin()))
                })
                .collect();
            let tip = *pts.last().unwrap_or(&centre);
            painter.add(Shape::line(pts, s));
            arrow_head(tip + Vec2::new(0.0, r.height() * 0.15), Vec2::new(0.0, 1.0));
        }
    }
}

/// A vertical gradient with a tick scale of the data range beside it.
fn colour_bar(
    painter: &egui::Painter,
    w: &Widget,
    rect: Rect,
    lut: &[Color32; 256],
    range: (f64, f64),
    z: f32,
) {
    let font = FontId::proportional(11.0 * z.max(0.6));
    let label_h = painter
        .layout_no_wrap("0".into(), font.clone(), Color32::BLACK)
        .size()
        .y;
    let strip = Rect::from_min_max(
        rect.min + Vec2::new(0.0, label_h / 2.0),
        Pos2::new(
            rect.left() + (rect.width() * 0.4).max(4.0),
            rect.bottom() - label_h / 2.0,
        ),
    );
    let mut mesh = egui::Mesh::default();
    for (i, c) in lut
        .iter()
        .enumerate()
        .step_by(4)
        .chain(std::iter::once((255, &lut[255])))
    {
        let y = strip.bottom() - strip.height() * i as f32 / 255.0;
        let idx = mesh.vertices.len() as u32;
        mesh.colored_vertex(Pos2::new(strip.left(), y), *c);
        mesh.colored_vertex(Pos2::new(strip.right(), y), *c);
        if idx >= 2 {
            mesh.add_triangle(idx - 2, idx - 1, idx);
            mesh.add_triangle(idx - 1, idx, idx + 1);
        }
    }
    painter.add(mesh);
    outline(painter, strip, 1.0, Color32::GRAY);

    let scale = Scale {
        lo: range.0,
        hi: range.1,
        log: false,
        from: strip.bottom(),
        to: strip.top(),
    };
    let (ticks, step) = scale.ticks(label_h * 2.5);
    let tick = 3.0 * z;
    let stroke = Stroke::new(1.0, fg(w));
    for v in ticks {
        let y = scale.pos(v);
        painter.line_segment(
            [
                Pos2::new(strip.right(), y),
                Pos2::new(strip.right() + tick, y),
            ],
            stroke,
        );
        painter.text(
            Pos2::new(strip.right() + tick + 1.0, y),
            Align2::LEFT_CENTER,
            axis::label(v, step),
            font.clone(),
            fg(w),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viridis_ends() {
        let w = Widget::from_element(bob::Element {
            name: "widget".into(),
            ..Default::default()
        });
        let l = lut(&w);
        assert_eq!(l[0], Color32::from_rgb(68, 1, 84));
        assert_eq!(l[255], Color32::from_rgb(253, 231, 36));
    }

    #[test]
    fn colorizes_unsigned_bytes() {
        let img = Image {
            width: 2,
            height: 1,
            rgb: false,
            data: ImageData::I8(Arc::from(vec![0i8, -1])),
            unique_id: 1,
        };
        let gray: [Color32; 256] = std::array::from_fn(|i| Color32::from_gray(i as u8));
        let (pix, range) = colorize(&img, true, None, &gray);
        assert_eq!(range, (0.0, 255.0));
        assert_eq!(
            pix.pixels,
            vec![Color32::from_gray(0), Color32::from_gray(255)]
        );
        let (_, range) = colorize(&img, false, None, &gray);
        assert_eq!(range, (-1.0, 0.0));
    }

    #[test]
    fn fast_path_matches_general_path() {
        let data: Vec<i8> = (0..64 * 64).map(|i| (i * 7 % 256) as u8 as i8).collect();
        let img = Image {
            width: 64,
            height: 64,
            rgb: false,
            data: ImageData::I8(Arc::from(data.clone())),
            unique_id: 1,
        };
        let as_i16 = Image {
            data: ImageData::I16(data.iter().map(|b| *b as u8 as i16).collect()),
            ..img.clone()
        };
        let l = lut(&Widget::from_element(Default::default()));
        let (fast, r1) = colorize(&img, true, None, &l);
        let (slow, r2) = colorize(&as_i16, false, None, &l);
        assert_eq!(r1, r2);
        assert_eq!(fast.pixels, slow.pixels);
    }

    #[test]
    #[ignore = "timing, run with --ignored --nocapture"]
    fn colorize_megapixel_timing() {
        let data: Vec<i8> = (0..1024 * 1024).map(|i| (i % 251) as i8).collect();
        let img = Image {
            width: 1024,
            height: 1024,
            rgb: false,
            data: ImageData::I8(Arc::from(data)),
            unique_id: 1,
        };
        let l = lut(&Widget::from_element(Default::default()));
        let t = std::time::Instant::now();
        for _ in 0..10 {
            colorize(&img, true, None, &l);
        }
        println!("1024x1024 i8: {:?} per frame", t.elapsed() / 10);
    }
}
