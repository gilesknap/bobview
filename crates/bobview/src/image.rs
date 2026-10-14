//! The image widget: a PVA NTNDArray (or a CA waveform shaped by
//! `data_width` x `data_height`) drawn through a colour map, with a colour bar.

use std::sync::Arc;

use bob::Widget;
use eframe::egui::{self, Align2, Color32, ColorImage, FontId, Pos2, Rect, TextureHandle, Vec2};
use pv::{Image, ImageData, PvState, Value};

use crate::colormaps::PREDEFINED;
use crate::screen::Node;
use crate::widgets::{bg, fg, fill, outline, placeholder, text_in};

/// Largest texture side we upload; bigger images are decimated.
const MAX_SIDE: usize = 2048;

pub struct ImageCache {
    /// Identifies the frame on the texture (unique id plus data pointer).
    key: (i64, usize),
    texture: TextureHandle,
    /// The data range the colours were scaled to.
    range: (f64, f64),
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
        (data.len() >= width * height).then(|| {
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

pub fn draw(ui: &mut egui::Ui, node: &mut Node, state: Option<&PvState>, rect: Rect, z: f32) {
    let w = node.w.clone();
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    fill(&painter, rect, bg(&w, "Background"));

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
    let (area, bar_rect) = if bar_visible && rect.width() > 3.0 * bar_size {
        let gap = 4.0 * z;
        let area = Rect::from_min_max(
            rect.min,
            Pos2::new(rect.right() - bar_size - gap, rect.bottom()),
        );
        let bar_rect = Rect::from_min_max(Pos2::new(rect.right() - bar_size, rect.top()), rect.max);
        (area, Some(bar_rect))
    } else {
        (rect, None)
    };

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
        return;
    };

    let lut = lut(&w);
    let unsigned = w.bool("unsigned").unwrap_or(false);
    let fixed = (w.bool("autoscale") == Some(false)).then(|| {
        (
            w.f64("minimum").unwrap_or(0.0),
            w.f64("maximum").unwrap_or(255.0),
        )
    });
    let key = (img.unique_id, data_key(&img));
    if node.image.as_ref().map(|c| c.key) != Some(key) {
        let (pixels, range) = colorize(&img, unsigned, fixed, &lut);
        match node.image.as_mut() {
            Some(cache) => {
                cache.texture.set(pixels, egui::TextureOptions::LINEAR);
                cache.key = key;
                cache.range = range;
            }
            None => {
                let texture =
                    ui.ctx()
                        .load_texture("bobview-image", pixels, egui::TextureOptions::LINEAR);
                node.image = Some(ImageCache {
                    key,
                    texture,
                    range,
                });
            }
        }
    }
    let Some(cache) = &node.image else { return };
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(cache.texture.id(), area, uv, Color32::WHITE);

    if let Some(bar_rect) = bar_rect {
        colour_bar(&painter, &w, bar_rect, &lut, cache.range, z);
    }
}

/// A vertical gradient with the data range written at its ends.
fn colour_bar(
    painter: &egui::Painter,
    w: &Widget,
    rect: Rect,
    lut: &[Color32; 256],
    range: (f64, f64),
    z: f32,
) {
    let font = FontId::proportional(11.0 * z.max(0.6));
    let label_h = 14.0 * z.max(0.6);
    let strip = Rect::from_min_max(
        rect.min + Vec2::new(0.0, label_h),
        Pos2::new(rect.left() + rect.width() * 0.5, rect.bottom() - label_h),
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
    let fmt = |v: f64| {
        if v.abs() >= 1e5 || (v != 0.0 && v.abs() < 1e-2) {
            format!("{v:.1e}")
        } else {
            format!("{v:.0}")
        }
    };
    let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + label_h));
    let bottom = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - label_h), rect.max);
    text_in(
        painter,
        top,
        &fmt(range.1),
        font.clone(),
        fg(w),
        Align2::LEFT_CENTER,
        false,
    );
    text_in(
        painter,
        bottom,
        &fmt(range.0),
        font,
        fg(w),
        Align2::LEFT_CENTER,
        false,
    );
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
