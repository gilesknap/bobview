//! Symbol and picture widgets: SVG or PNG images from files or URLs.
//!
//! A symbol shows one of its `symbols`, picked by its PV's value, and may run
//! its actions when clicked; a picture shows one `file`. Images load on a
//! thread and are kept, decoded, for every widget that names them; each
//! widget keeps its own texture at the size it is drawn.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use bob::Source;
use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Id, Mesh, Pos2, Rect, Response, Sense,
    TextureHandle, TextureOptions, Vec2,
};
use pv::{PvState, Value};
use resvg::{tiny_skia, usvg};

use crate::screen::Node;
use crate::style::{color, named};
use crate::widgets::{Cx, placeholder, run_action};

/// Largest texture side we render an SVG at.
const MAX_SIDE: f32 = 4096.0;

/// A decoded image.
#[derive(Clone)]
enum Image {
    /// Rendered again whenever the size it is drawn at changes.
    Svg(Arc<usvg::Tree>),
    Raster(Arc<ColorImage>),
}

impl Image {
    fn size(&self) -> Vec2 {
        match self {
            Image::Svg(tree) => Vec2::new(tree.size().width(), tree.size().height()),
            Image::Raster(img) => Vec2::new(img.size[0] as f32, img.size[1] as f32),
        }
    }
}

enum Entry {
    Loading,
    Ready(Image),
    Failed,
}

/// Images by where they come from, and the count of [`forget`] calls, so a
/// load started before a reload does not store what it read.
struct Images {
    entries: HashMap<Source, Entry>,
    generation: u64,
}

static IMAGES: LazyLock<Mutex<Images>> = LazyLock::new(|| {
    Mutex::new(Images {
        entries: HashMap::new(),
        generation: 0,
    })
});

/// Threads that load images; more images than this wait their turn.
#[cfg(not(target_arch = "wasm32"))]
const LOADERS: usize = 4;

/// The queue the loader threads take images from, started on first use.
#[cfg(not(target_arch = "wasm32"))]
static QUEUE: LazyLock<Mutex<std::sync::mpsc::Sender<Job>>> = LazyLock::new(|| {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..LOADERS {
        let rx = rx.clone();
        std::thread::spawn(move || {
            loop {
                let job = rx.lock().unwrap().recv();
                match job {
                    Ok(job) => load(job),
                    Err(_) => break,
                }
            }
        });
    }
    Mutex::new(tx)
});

struct Job {
    source: Source,
    generation: u64,
    ctx: egui::Context,
}

/// Drop every image, loaded or loading, so they are read again on a reload.
pub fn forget() {
    let mut images = IMAGES.lock().unwrap();
    images.entries.clear();
    images.generation += 1;
}

/// Whether any image is still loading.
#[cfg(not(target_arch = "wasm32"))]
pub fn loading() -> bool {
    IMAGES
        .lock()
        .unwrap()
        .entries
        .values()
        .any(|e| matches!(e, Entry::Loading))
}

/// The image at `source`: `Some(Ok)` when loaded, `Some(Err)` when it cannot
/// be, and `None` while it loads (a repaint follows when it has).
fn image(ctx: &egui::Context, source: &Source) -> Option<Result<Image, ()>> {
    let mut images = IMAGES.lock().unwrap();
    match images.entries.get(source) {
        Some(Entry::Ready(img)) => return Some(Ok(img.clone())),
        Some(Entry::Failed) => return Some(Err(())),
        Some(Entry::Loading) => return None,
        None => {}
    }
    images.entries.insert(source.clone(), Entry::Loading);
    let job = Job {
        source: source.clone(),
        generation: images.generation,
        ctx: ctx.clone(),
    };
    drop(images);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = QUEUE.lock().unwrap().send(job);
    // A browser has one thread: fetch without blocking, then decode.
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let data = crate::fetch::bytes_async(&job.source).await;
        finish(job, data);
    });
    None
}

/// Read and decode one image on a loader thread.
#[cfg(not(target_arch = "wasm32"))]
fn load(job: Job) {
    let data = crate::fetch::bytes(&job.source);
    finish(job, data);
}

/// Decode what was read for `job` and keep it.
fn finish(job: Job, data: Result<Vec<u8>, bob::Error>) {
    let source = &job.source;
    let data = data.map_err(|e| e.to_string());
    // A decoder that panics on a bad file must not leave it loading.
    let read = || match data {
        Ok(data) => match decode(source, &data) {
            Ok(img) => Entry::Ready(img),
            Err(e) => {
                log::warn!("cannot show {}: {e}", describe(source));
                Entry::Failed
            }
        },
        Err(e) => {
            log::warn!("{e}");
            Entry::Failed
        }
    };
    let entry = std::panic::catch_unwind(read).unwrap_or(Entry::Failed);
    let mut images = IMAGES.lock().unwrap();
    if images.generation == job.generation {
        images.entries.insert(job.source, entry);
    }
    drop(images);
    job.ctx.request_repaint();
}

fn describe(source: &Source) -> String {
    match source {
        Source::File(p) => p.display().to_string(),
        Source::Url(u) => u.clone(),
    }
}

fn decode(source: &Source, data: &[u8]) -> Result<Image, String> {
    if data.starts_with(b"\x89PNG") {
        return decode_png(data).map(|img| Image::Raster(Arc::new(img)));
    }
    let name = describe(source).to_ascii_lowercase();
    let svg = name.ends_with(".svg")
        || name.ends_with(".svgz")
        || data.trim_ascii_start().starts_with(b"<");
    if !svg {
        return Err("only SVG and PNG images are supported".into());
    }
    // An SVG may embed images as data: URLs, but never names files or URLs:
    // a display from the web is not trusted with local files.
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|href, _| {
                log::warn!("SVG image '{href}' is not shown: only embedded images are");
                None
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;
    Ok(Image::Svg(Arc::new(tree)))
}

fn decode_png(data: &[u8]) -> Result<ColorImage, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let buf = &buf[..info.buffer_size()];
    let size = [info.width as usize, info.height as usize];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf.to_vec(),
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("unexpected indexed PNG".into()),
    };
    Ok(ColorImage::from_rgba_unmultiplied(size, &rgba))
}

/// What a symbol or picture widget keeps between frames.
#[derive(Default)]
pub struct State {
    texture: Option<Texture>,
    /// The symbol index last shown, kept while the PV has no usable value.
    index: Option<i64>,
}

/// A widget's texture and what is on it: the image and its size in pixels.
struct Texture {
    key: (Source, [usize; 2]),
    handle: TextureHandle,
}

/// The widget's texture showing `img` at `size` points, made or updated.
fn texture<'a>(
    ctx: &egui::Context,
    slot: &'a mut Option<Texture>,
    source: &Source,
    img: &Image,
    size: Vec2,
) -> Option<&'a TextureHandle> {
    let max_side = ctx.input(|i| i.max_texture_side) as f32;
    let px = match img {
        Image::Raster(r) if r.size.into_iter().any(|n| n as f32 > max_side) => {
            log::warn!("cannot show {}: too large for a texture", describe(source));
            return None;
        }
        Image::Raster(r) => r.size,
        Image::Svg(_) => {
            let px = (size * ctx.pixels_per_point())
                .round()
                .max(Vec2::splat(1.0));
            let px = px * (MAX_SIDE.min(max_side) / px.max_elem()).min(1.0);
            [px.x as usize, px.y as usize]
        }
    };
    let key = (source.clone(), px);
    if slot.as_ref().is_none_or(|t| t.key != key) {
        let pixels = match img {
            Image::Raster(r) => (**r).clone(),
            Image::Svg(tree) => render_svg(tree, px),
        };
        let options = TextureOptions::LINEAR;
        match slot {
            Some(t) => {
                t.handle.set(pixels, options);
                t.key = key;
            }
            None => {
                *slot = Some(Texture {
                    key,
                    handle: ctx.load_texture("bobview-symbol", pixels, options),
                });
            }
        }
    }
    slot.as_ref().map(|t| &t.handle)
}

fn render_svg(tree: &usvg::Tree, [w, h]: [usize; 2]) -> ColorImage {
    let Some(mut pixmap) = tiny_skia::Pixmap::new(w as u32, h as u32) else {
        return ColorImage::filled([w, h], Color32::TRANSPARENT);
    };
    let size = tree.size();
    let scale = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(tree, scale, &mut pixmap.as_mut());
    ColorImage::from_rgba_premultiplied([w, h], pixmap.data())
}

/// The largest size of aspect `native` that fits in `room`.
fn fit(native: Vec2, room: Vec2) -> Vec2 {
    if native.x <= 0.0 || native.y <= 0.0 {
        return room;
    }
    let scale = (room.x / native.x).min(room.y / native.y);
    native * scale
}

/// A rectangle of `size` centred on `center`, turned by `angle` about it:
/// the whole of `mesh`'s texture, or plain colour on an untextured mesh.
fn quad(mesh: &mut Mesh, center: Pos2, size: Vec2, angle: f32, tint: Color32) {
    let start = mesh.vertices.len();
    let uv = if mesh.texture_id == egui::TextureId::default() {
        Rect::from_min_max(egui::epaint::WHITE_UV, egui::epaint::WHITE_UV)
    } else {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
    };
    mesh.add_rect_with_uv(Rect::from_center_size(center, size), uv, tint);
    let rot = egui::emath::Rot2::from_angle(angle);
    for v in &mut mesh.vertices[start..] {
        v.pos = center + rot * (v.pos - center);
    }
}

/// The symbol index a PV value selects, as Phoebus picks it; `None` for no
/// usable value.
fn value_index(w: &bob::Widget, s: &PvState) -> Option<i64> {
    let at = |len: usize| w.i64("array_index").unwrap_or(0).clamp(0, len as i64 - 1) as usize;
    match &s.value {
        Value::Str(t) => t.trim().parse().ok(),
        Value::DoubleArray(a) if !a.is_empty() => Some(a[at(a.len())] as i64),
        Value::LongArray(a) if !a.is_empty() => Some(a[at(a.len())]),
        Value::Double(_) | Value::Long(_) | Value::Enum(_) => s.value.as_i64(),
        _ => None,
    }
}

/// The symbol index to show: the PV's value; without a usable one, the
/// index last shown, else `initial_index`.
fn index(node: &Node, s: Option<&PvState>) -> i64 {
    let value = s
        .filter(|s| s.connected)
        .and_then(|s| value_index(&node.w, s));
    value
        .or(node.symbol.index)
        .unwrap_or_else(|| node.w.i64("initial_index").unwrap_or(0))
}

pub fn draw_symbol(
    ui: &mut egui::Ui,
    node: &mut Node,
    s: Option<&PvState>,
    rect: Rect,
    cx: &mut Cx,
    id: Id,
) -> Option<Response> {
    let w = &node.w;
    let painter = ui.painter().with_clip_rect(cx.clip);
    let z = cx.view.zoom;
    let angle = (w.f64("rotation").unwrap_or(0.0) as f32).to_radians();

    // Clicks run every action, as Phoebus does when the symbol is set to.
    let actions = w.actions();
    let clickable = w.bool("run_actions_on_mouse_click") == Some(true)
        && w.bool("enabled") != Some(false)
        && !actions.is_empty();
    let resp = clickable.then(|| {
        ui.interact(rect.intersect(cx.clip), id, Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    });

    if w.bool("transparent") == Some(false) {
        let bg = w
            .color("background_color")
            .map(|c| color(&c))
            .unwrap_or(named("Background"));
        let mut mesh = Mesh::default();
        quad(&mut mesh, rect.center(), rect.size(), angle, bg);
        painter.add(mesh);
    }

    let symbols = w.list("symbols", "symbol");
    let idx = index(node, s);
    node.symbol.index = Some(idx);
    let file = match usize::try_from(idx) {
        Ok(i) => symbols
            .get(i)
            .cloned()
            .or_else(|| w.text("fallback_symbol").map(str::to_string)),
        Err(_) => None,
    };
    let file = file
        .map(|f| node.expand(&f))
        .filter(|f| !f.trim().is_empty());
    let fallback = w
        .text("fallback_symbol")
        .map(|f| node.expand(f))
        .filter(|f| !f.trim().is_empty());

    // Brighter under the pointer and darker while pressed.
    let tint = match &resp {
        Some(r) if r.is_pointer_button_down_on() => Color32::from_gray(180),
        _ => Color32::WHITE,
    };
    let glow = resp
        .as_ref()
        .is_some_and(|r| r.hovered() && !r.is_pointer_button_down_on());

    if let Some(file) = file {
        let shown = show(
            ui,
            node,
            &[
                &file,
                fallback.as_deref().filter(|f| *f != file).unwrap_or(""),
            ],
            rect,
            angle,
            tint,
            glow,
            &painter,
        );
        if !shown {
            placeholder(&painter, rect, "symbol", z);
        }
    } else if idx >= 0 {
        placeholder(&painter, rect, "symbol", z);
    }

    let w = &node.w;
    if w.bool("show_index") == Some(true) && rect.width() >= 20.0 && rect.height() >= 20.0 {
        let text = idx.to_string();
        let font = FontId::proportional(16.0 * z);
        let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
        let label = Rect::from_center_size(rect.center(), galley.size() + Vec2::splat(4.0 * z));
        painter.rect_filled(label, CornerRadius::same(3), Color32::from_black_alpha(140));
        painter.galley(
            Align2::CENTER_CENTER
                .align_size_within_rect(galley.size(), label)
                .min,
            galley,
            Color32::WHITE,
        );
    }

    // A PV that is not connected is marked with an overlay.
    if s.is_some_and(|s| !s.connected) {
        let overlay = w
            .color("disconnect_overlay_color")
            .map(|c| color(&c))
            .unwrap_or_else(|| named("INVALID").gamma_multiply(0.5));
        let mut mesh = Mesh::default();
        quad(&mut mesh, rect.center(), rect.size(), angle, overlay);
        painter.add(mesh);
    }

    if let Some(r) = &resp
        && r.clicked()
    {
        for a in &actions {
            run_action(node, a, cx);
        }
    }
    resp
}

pub fn draw_picture(ui: &mut egui::Ui, node: &mut Node, rect: Rect, cx: &Cx) {
    let painter = ui.painter().with_clip_rect(cx.clip);
    let file = node.prop("file");
    if file.trim().is_empty() {
        placeholder(&painter, rect, "picture", cx.view.zoom);
        return;
    }
    let angle = (node.w.f64("rotation").unwrap_or(0.0) as f32).to_radians();
    let opacity = node.w.f64("opacity").unwrap_or(1.0).clamp(0.0, 1.0) as f32;
    let tint = Color32::WHITE.gamma_multiply(opacity);
    if !show(ui, node, &[&file], rect, angle, tint, false, &painter) {
        placeholder(&painter, rect, "picture", cx.view.zoom);
    }
}

/// Draw the first of `files` that loads, fitted to `rect` and turned by
/// `angle`. False if none can be shown; true while one is still loading.
#[allow(clippy::too_many_arguments)]
fn show(
    ui: &egui::Ui,
    node: &mut Node,
    files: &[&str],
    rect: Rect,
    angle: f32,
    tint: Color32,
    glow: bool,
    painter: &egui::Painter,
) -> bool {
    let ctx = ui.ctx();
    for file in files.iter().filter(|f| !f.trim().is_empty()) {
        let source = node.resolve(file);
        let img = match image(ctx, &source) {
            None => return true,
            Some(Err(())) => continue,
            Some(Ok(img)) => img,
        };
        let size = picture_size(&node.w, img.size(), rect.size(), angle);
        let Some(handle) = texture(ctx, &mut node.symbol.texture, &source, &img, size) else {
            continue;
        };
        let mut mesh = Mesh::with_texture(handle.id());
        quad(&mut mesh, rect.center(), size, angle, tint);
        if glow {
            // Drawn again additively (zero alpha): 30% brighter.
            quad(
                &mut mesh,
                rect.center(),
                size,
                angle,
                Color32::from_rgba_premultiplied(77, 77, 77, 0),
            );
        }
        painter.add(mesh);
        return true;
    }
    false
}

/// The size an image of `native` size is drawn at in a widget of `room`.
/// A symbol fits the widget before turning; a picture is shrunk so that,
/// turned, it still fits.
fn picture_size(w: &bob::Widget, native: Vec2, room: Vec2, angle: f32) -> Vec2 {
    if w.kind != "picture" {
        return if w.bool("preserve_ratio") != Some(false) {
            fit(native, room)
        } else {
            room
        };
    }
    let size = if w.bool("stretch_image") == Some(true) {
        room
    } else {
        fit(native, room)
    };
    let (sin, cos) = (angle.sin().abs(), angle.cos().abs());
    let bounds = Vec2::new(size.x * cos + size.y * sin, size.x * sin + size.y * cos);
    let scale = (room.x / bounds.x).min(room.y / bounds.y);
    if scale < 1.0 { size * scale } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(xml: &str) -> bob::Widget {
        let d = bob::Display::parse(&format!("<display>{xml}</display>")).unwrap();
        d.widgets.into_iter().next().unwrap()
    }

    #[test]
    fn fits_keeping_the_aspect_ratio() {
        assert_eq!(
            fit(Vec2::new(10.0, 20.0), Vec2::new(40.0, 40.0)),
            Vec2::new(20.0, 40.0)
        );
        assert_eq!(
            fit(Vec2::new(20.0, 10.0), Vec2::new(40.0, 40.0)),
            Vec2::new(40.0, 20.0)
        );
    }

    #[test]
    fn symbols_fit_before_turning_and_pictures_after() {
        let room = Vec2::new(100.0, 100.0);
        let native = Vec2::new(10.0, 10.0);
        let angle = 45f32.to_radians();
        let sym = widget(r#"<widget type="symbol"/>"#);
        assert_eq!(picture_size(&sym, native, room, angle), room);
        let pic = widget(r#"<widget type="picture"/>"#);
        let s = picture_size(&pic, native, room, angle);
        assert!((s.x - 100.0 / 2f32.sqrt()).abs() < 0.01, "{s:?}");
        let stretch =
            widget(r#"<widget type="symbol"><preserve_ratio>false</preserve_ratio></widget>"#);
        assert_eq!(
            picture_size(&stretch, Vec2::new(10.0, 20.0), room, 0.0),
            room
        );
    }

    #[test]
    fn pv_values_pick_the_symbol() {
        let w = widget(r#"<widget type="symbol"><array_index>1</array_index></widget>"#);
        let state = |value| PvState {
            connected: true,
            value,
            ..Default::default()
        };
        assert_eq!(value_index(&w, &state(Value::Double(2.7))), Some(2));
        assert_eq!(value_index(&w, &state(Value::Enum(3))), Some(3));
        assert_eq!(value_index(&w, &state(Value::Str(" 4 ".into()))), Some(4));
        assert_eq!(value_index(&w, &state(Value::Str("on".into()))), None);
        let arr = Value::LongArray(Arc::from([5, 6, 7]));
        assert_eq!(value_index(&w, &state(arr)), Some(6));
        assert_eq!(value_index(&w, &state(Value::None)), None);
    }

    #[test]
    fn decodes_svg_and_png() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="8" height="4" fill="red"/></svg>"#;
        let src = Source::parse("a.svg");
        let Ok(Image::Svg(tree)) = decode(&src, svg) else {
            panic!("not an SVG")
        };
        let img = render_svg(&tree, [8, 4]);
        assert_eq!(img.pixels[0], Color32::RED);

        let mut png = Vec::new();
        let mut enc = png::Encoder::new(&mut png, 2, 1);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&[0, 0, 255, 0, 255, 0])
            .unwrap();
        let Ok(Image::Raster(img)) = decode(&Source::parse("b.png"), &png) else {
            panic!("not a PNG")
        };
        assert_eq!(img.size, [2, 1]);
        assert_eq!(img.pixels[0], Color32::BLUE);
        assert!(decode(&Source::parse("c.gif"), b"GIF89a").is_err());

        // An SVG naming a file draws nothing for it, rather than reading it.
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/displays/symbols/stripe.png");
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><image href="{}" width="4" height="4"/></svg>"#,
            here.canonicalize().unwrap().display()
        );
        let Ok(Image::Svg(tree)) = decode(&src, svg.as_bytes()) else {
            panic!("not an SVG")
        };
        assert!(!tree.root().has_children());
    }
}
