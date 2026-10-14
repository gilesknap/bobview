//! Render a display to a PNG without a window: egui runs headless and its
//! triangles are rasterised in software. Used for checking layouts in CI and
//! on machines without a display.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureId, Vec2, epaint};

use crate::app::{BobApp, Location};

/// What to render and how.
pub struct Options {
    pub size: [u32; 2],
    pub zoom: Option<f32>,
    /// Time for PVs to connect before anything else happens.
    pub wait: Duration,
    /// Points to click and the button to use, in order, once `wait` has passed.
    pub clicks: Vec<(Pos2, egui::PointerButton)>,
    pub keypad: bool,
}

pub fn run(start: Location, opts: &Options, out: &Path) -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut app = BobApp::new(&ctx, Some(start), opts.zoom);
    app.set_keypad(opts.keypad);
    let mut textures: HashMap<TextureId, ColorImage> = HashMap::new();
    let begin = Instant::now();
    let screen = Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(opts.size[0] as f32, opts.size[1] as f32),
    );

    // Each click is a press frame then a release frame.
    let mut pending: std::collections::VecDeque<Vec<egui::Event>> = opts
        .clicks
        .iter()
        .flat_map(|&(pos, which)| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: which,
                pressed,
                modifiers: Default::default(),
            };
            // A few idle frames after each click let windows appear.
            [
                vec![egui::Event::PointerMoved(pos), button(true)],
                vec![button(false)],
                vec![],
                vec![],
                vec![],
                vec![],
            ]
        })
        .collect();
    let mut settle_until = None;

    // Run frames until PVs have had time to connect and clicks have settled.
    let output = loop {
        let started = begin.elapsed() >= opts.wait;
        let events = if started {
            pending.pop_front().unwrap_or_default()
        } else {
            Vec::new()
        };
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(begin.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                apply_texture(&mut textures, *id, delta);
            }
        }
        output.textures_delta.clear();
        if started && pending.is_empty() {
            let until = *settle_until.get_or_insert_with(|| {
                begin.elapsed()
                    + if opts.clicks.is_empty() {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(1)
                    }
            });
            if begin.elapsed() >= until {
                break output;
            }
        }
        std::thread::sleep(Duration::from_millis(if started { 50 } else { 100 }));
    };

    let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
    let mut canvas = Canvas::new(opts.size[0] as usize, opts.size[1] as usize);
    for prim in prims {
        if let epaint::Primitive::Mesh(mesh) = &prim.primitive {
            canvas.draw_mesh(mesh, prim.clip_rect, textures.get(&mesh.texture_id));
        }
    }
    canvas.save(out)
}

fn apply_texture(
    textures: &mut HashMap<TextureId, ColorImage>,
    id: TextureId,
    delta: &epaint::ImageDelta,
) {
    let egui::ImageData::Color(image) = &delta.image;
    match delta.pos {
        None => {
            textures.insert(id, (**image).clone());
        }
        Some([x0, y0]) => {
            if let Some(tex) = textures.get_mut(&id) {
                let [w, h] = image.size;
                for y in 0..h {
                    for x in 0..w {
                        let (tx, ty) = (x0 + x, y0 + y);
                        if tx < tex.size[0] && ty < tex.size[1] {
                            tex.pixels[ty * tex.size[0] + tx] = image.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }
}

/// Premultiplied RGBA float framebuffer.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![[0.0, 0.0, 0.0, 1.0]; w * h],
        }
    }

    fn draw_mesh(&mut self, mesh: &epaint::Mesh, clip: Rect, tex: Option<&ColorImage>) {
        let clip = clip.intersect(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(self.w as f32, self.h as f32),
        ));
        if clip.is_negative() {
            return;
        }
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = tri.map(|i| &mesh.vertices[i as usize]);
            self.triangle(v, clip, tex);
        }
    }

    fn triangle(&mut self, v: [&epaint::Vertex; 3], clip: Rect, tex: Option<&ColorImage>) {
        let (p0, p1, p2) = (v[0].pos, v[1].pos, v[2].pos);
        let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
        if area.abs() < 1e-6 {
            return;
        }
        let min_x = p0.x.min(p1.x).min(p2.x).max(clip.min.x).floor() as i64;
        let max_x = p0.x.max(p1.x).max(p2.x).min(clip.max.x).ceil() as i64;
        let min_y = p0.y.min(p1.y).min(p2.y).max(clip.min.y).floor() as i64;
        let max_y = p0.y.max(p1.y).max(p2.y).min(clip.max.y).ceil() as i64;
        let col = |c: Color32| {
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32].map(|x| x / 255.0)
        };
        let cs = [col(v[0].color), col(v[1].color), col(v[2].color)];
        for y in min_y.max(0)..max_y.min(self.h as i64) {
            for x in min_x.max(0)..max_x.min(self.w as i64) {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p1.x - p.x) * (p2.y - p.y) - (p2.x - p.x) * (p1.y - p.y)) / area;
                let w1 = ((p2.x - p.x) * (p0.y - p.y) - (p0.x - p.x) * (p2.y - p.y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let mut c = [0.0f32; 4];
                for (i, ch) in c.iter_mut().enumerate() {
                    *ch = cs[0][i] * w0 + cs[1][i] * w1 + cs[2][i] * w2;
                }
                if let Some(t) = tex {
                    let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                    let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                    let s = sample(t, u, vv);
                    for (i, ch) in c.iter_mut().enumerate() {
                        *ch *= s[i];
                    }
                }
                let d = &mut self.px[y as usize * self.w + x as usize];
                for i in 0..4 {
                    d[i] = c[i] + d[i] * (1.0 - c[3]);
                }
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc =
            png::Encoder::new(std::io::BufWriter::new(file), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        let data: Vec<u8> = self
            .px
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        writer.write_image_data(&data).map_err(|e| e.to_string())
    }
}

/// Bilinear sample of a premultiplied texture, as 0..1 floats.
fn sample(t: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [w, h] = t.size;
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let c = t.pixels[y * w + x];
        [c.r(), c.g(), c.b(), c.a()].map(|v| v as f32 / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        out[i] =
            (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_test_display() {
        let dir = std::env::temp_dir().join(format!("bobview-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in crate::screen::display_set::files() {
            let out = dir.join(f.file_name().unwrap()).with_extension("png");
            let start = Location {
                path: f.clone(),
                macros: bob::Macros::default(),
            };
            let opts = Options {
                size: [320, 240],
                zoom: Some(0.5),
                wait: Duration::ZERO,
                clicks: Vec::new(),
                keypad: false,
            };
            run(start, &opts, &out).unwrap();
            assert!(
                std::fs::metadata(&out).unwrap().len() > 1000,
                "{} rendered nothing",
                f.display()
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
