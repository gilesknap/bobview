//! Write simulated camera frames as greyscale PNGs, to eyeball the pattern.
//! usage: cargo run -p testioc --example frame_png -- OUT_PREFIX [FRAME]...

use std::fs::File;
use std::io::BufWriter;

use testioc::image::{HEIGHT, ImageSim, WIDTH};

fn main() {
    let mut args = std::env::args().skip(1);
    let prefix = args.next().expect("usage: frame_png OUT_PREFIX [FRAME]...");
    let mut frames: Vec<u32> = args.map(|a| a.parse().expect("frame number")).collect();
    if frames.is_empty() {
        frames.push(1);
    }
    let mut sim = ImageSim::new();
    for n in frames {
        let pixels: Vec<u8> = sim.frame(n).iter().map(|&p| p as u8).collect();
        let path = format!("{prefix}-{n}.png");
        let file = BufWriter::new(File::create(&path).expect("create PNG"));
        let mut enc = png::Encoder::new(file, WIDTH as u32, HEIGHT as u32);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("PNG header");
        w.write_image_data(&pixels).expect("PNG data");
        println!("{path}");
    }
}
