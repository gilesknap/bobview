//! A simulated detector image: diagonal fringes drifting across the frame,
//! three gaussian spots on Lissajous paths and a little noise, as 8-bit
//! pixels. Built from precomputed tables so a 1024x1024 frame costs about a
//! millisecond, into a small pool of reused buffers.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use epics_pva_rs::nt::nd_array::{
    NdAlarm, NdArrayBuffer, NdCodec, NdDimension, NdTimeStamp, NtNdArray, nt_nd_array_desc,
    nt_nd_array_value,
};
use epics_pva_rs::server_native::SharedPV;

pub const WIDTH: usize = 1024;
pub const HEIGHT: usize = 1024;

const FRINGE_PERIOD: usize = 128;
const NOISE_LEN: usize = 1 << 16;
const SPOT_RADIUS: usize = 120;
const POOL: usize = 3;

struct Spot {
    sigma: f32,
    amp: f32,
    // Lissajous frequencies (radians per frame) and phases.
    fx: f32,
    fy: f32,
    px: f32,
    py: f32,
}

const SPOTS: [Spot; 3] = [
    Spot {
        sigma: 40.0,
        amp: 170.0,
        fx: 0.031,
        fy: 0.047,
        px: 0.0,
        py: 1.0,
    },
    Spot {
        sigma: 22.0,
        amp: 150.0,
        fx: 0.053,
        fy: 0.029,
        px: 2.0,
        py: 0.5,
    },
    Spot {
        sigma: 36.0,
        amp: 130.0,
        fx: 0.019,
        fy: 0.037,
        px: 4.0,
        py: 3.0,
    },
];

pub struct ImageSim {
    fringe: Vec<u8>,
    noise: Vec<u8>,
    // Per spot, a unit gaussian profile over -SPOT_RADIUS..=SPOT_RADIUS
    // scaled to 0..=255 and the same scaled by the spot amplitude.
    profiles: Vec<(Vec<u16>, Vec<u16>)>,
    pool: Vec<Arc<[i8]>>,
    rng: u32,
}

impl Default for ImageSim {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageSim {
    pub fn new() -> Self {
        let fringe = (0..WIDTH + FRINGE_PERIOD)
            .map(|i| {
                let s = (TAU * i as f32 / FRINGE_PERIOD as f32).sin();
                (50.0 + 35.0 * s) as u8
            })
            .collect();
        let mut rng = 0x2545_f491;
        let noise = (0..NOISE_LEN + WIDTH)
            .map(|_| (xorshift(&mut rng) % 25) as u8)
            .collect();
        let profiles = SPOTS
            .iter()
            .map(|s| {
                let g = |d: usize| {
                    let d = d as f32 - SPOT_RADIUS as f32;
                    (-d * d / (2.0 * s.sigma * s.sigma)).exp()
                };
                let unit = (0..=2 * SPOT_RADIUS)
                    .map(|d| (255.0 * g(d)) as u16)
                    .collect();
                let scaled = (0..=2 * SPOT_RADIUS)
                    .map(|d| (s.amp * g(d)) as u16)
                    .collect();
                (unit, scaled)
            })
            .collect();
        ImageSim {
            fringe,
            noise,
            profiles,
            pool: Vec::new(),
            rng,
        }
    }

    /// Render frame number `n` into a pooled buffer. Pixels are 0..=255
    /// stored as i8 bit patterns, as an Int8 simDetector produces.
    pub fn frame(&mut self, n: u32) -> Arc<[i8]> {
        let mut buf = match self.pool.iter().position(|b| Arc::strong_count(b) == 1) {
            Some(i) => self.pool.swap_remove(i),
            None => Arc::from(vec![0i8; WIDTH * HEIGHT]),
        };
        let px = Arc::get_mut(&mut buf).expect("unshared pool buffer");
        let t = n as f32;
        let shift = (n as usize * 3) % FRINGE_PERIOD;

        for (y, row) in px.as_chunks_mut::<WIDTH>().0.iter_mut().enumerate() {
            let f0 = (y + shift) % FRINGE_PERIOD;
            let n0 = xorshift(&mut self.rng) as usize % NOISE_LEN;
            let fringe = &self.fringe[f0..f0 + WIDTH];
            let noise = &self.noise[n0..n0 + WIDTH];
            for ((p, &f), &z) in row.iter_mut().zip(fringe).zip(noise) {
                *p = (f + z) as i8;
            }
        }

        for (s, (unit, scaled)) in SPOTS.iter().zip(&self.profiles) {
            let margin = SPOT_RADIUS as f32 + 1.0;
            let cx = WIDTH as f32 / 2.0 + (WIDTH as f32 / 2.0 - margin) * (s.fx * t + s.px).sin();
            let cy = HEIGHT as f32 / 2.0 + (HEIGHT as f32 / 2.0 - margin) * (s.fy * t + s.py).sin();
            let (x0, y0) = (cx as usize - SPOT_RADIUS, cy as usize - SPOT_RADIUS);
            let xs = &unit[..];
            for (dy, &gy) in scaled.iter().enumerate() {
                let start = (y0 + dy) * WIDTH + x0;
                let row = &mut px[start..start + xs.len()];
                for (p, &gx) in row.iter_mut().zip(xs) {
                    let add = ((gx * gy) >> 8) as u8;
                    *p = (*p as u8).saturating_add(add) as i8;
                }
            }
        }

        self.pool.push(buf.clone());
        if self.pool.len() > POOL {
            self.pool.remove(0);
        }
        buf
    }
}

fn xorshift(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

fn now() -> NdTimeStamp {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    NdTimeStamp {
        seconds_past_epoch: d.as_secs() as i64,
        nanoseconds: d.subsec_nanos() as i32,
        user_tag: 0,
    }
}

fn nt(pixels: Arc<[i8]>, dims: &[usize], unique_id: i32) -> NtNdArray {
    let ts = now();
    let size = pixels.len() as i64;
    NtNdArray {
        value: NdArrayBuffer::Byte(pixels.into()),
        codec: NdCodec::default(),
        compressed_size: size,
        uncompressed_size: size,
        unique_id,
        data_time_stamp: ts.clone(),
        alarm: NdAlarm::default(),
        time_stamp: ts,
        dimension: dims
            .iter()
            .map(|&d| NdDimension {
                size: d as i32,
                full_size: d as i32,
                binning: 1,
                ..NdDimension::default()
            })
            .collect(),
        attribute: Vec::new(),
    }
}

/// The NTNDArray PV, opened with an empty array until the first frame.
pub fn open_pv() -> SharedPV {
    let pv = SharedPV::build_readonly();
    let empty = nt(Arc::from(Vec::new()), &[], 0);
    pv.open(nt_nd_array_desc(), nt_nd_array_value(&empty))
        .expect("NTNDArray value fits its descriptor");
    pv
}

/// Post a frame as NDPluginPVA would: dimension [x, y], uncompressed.
pub fn post(pv: &SharedPV, pixels: Arc<[i8]>, unique_id: i32) {
    pv.try_post(nt_nd_array_value(&nt(pixels, &[WIDTH, HEIGHT], unique_id)));
}
