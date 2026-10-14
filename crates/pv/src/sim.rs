//! `sim://` PVs, following the Phoebus simulated PV names.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::name::split_args;
use crate::{PvName, Severity, Shared, Value};

pub(crate) async fn run(name: PvName, shared: Arc<Shared>) {
    let args: Vec<f64> = name
        .args()
        .map(split_args)
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    let arg = |i: usize, default: f64| args.get(i).copied().unwrap_or(default);
    let func = name.base().to_string();

    // Each signal is (min, max, period in seconds, next-value function of the step count).
    let (min, max, period): (f64, f64, f64);
    let next: Box<dyn Fn(u64) -> Value + Send> = match func.as_str() {
        "sine" | "sawtooth" => {
            let (lo, hi) = (arg(0, -5.0), arg(1, 5.0));
            let (steps, upd) = if args.len() >= 4 {
                (arg(2, 10.0), arg(3, 1.0))
            } else {
                (10.0, arg(2, 0.1))
            };
            (min, max, period) = (lo, hi, upd);
            let saw = func == "sawtooth";
            Box::new(move |n| {
                let phase = (n as f64 % steps.max(1.0)) / steps.max(1.0);
                let unit = if saw {
                    phase
                } else {
                    0.5 + 0.5 * (phase * std::f64::consts::TAU).sin()
                };
                Value::Double(lo + (hi - lo) * unit)
            })
        }
        "ramp" => {
            let (lo, hi, step) = (arg(0, -5.0), arg(1, 5.0), arg(2, 1.0));
            (min, max, period) = (lo, hi, arg(3, 1.0));
            let count = if step == 0.0 || !step.is_finite() {
                1
            } else {
                ((hi - lo) / step).abs().min(1e9).floor() as u64 + 1
            };
            Box::new(move |n| Value::Double(lo + step * (n % count.max(1)) as f64))
        }
        "noise" => {
            let (lo, hi) = (arg(0, -5.0), arg(1, 5.0));
            (min, max, period) = (lo, hi, arg(2, 1.0));
            Box::new(move |n| Value::Double(lo + (hi - lo) * random(n)))
        }
        "gaussianNoise" => {
            let (centre, sd) = (arg(0, 0.0), arg(1, 1.0));
            (min, max, period) = (centre - 4.0 * sd, centre + 4.0 * sd, arg(2, 0.1));
            Box::new(move |n| {
                // Box–Muller from two uniform samples.
                let (u1, u2) = (random(2 * n).max(1e-12), random(2 * n + 1));
                let z = (-2.0 * u1.ln()).sqrt() * (u2 * std::f64::consts::TAU).cos();
                Value::Double(centre + sd * z)
            })
        }
        "flipflop" => {
            (min, max, period) = (0.0, 1.0, arg(0, 1.0));
            Box::new(|n| Value::Long((n % 2) as i64))
        }
        "const" => {
            (min, max, period) = (arg(0, 0.0), arg(0, 0.0), 0.0);
            let v = arg(0, 0.0);
            Box::new(move |_| Value::Double(v))
        }
        other => {
            shared.update(|s| s.error = Some(format!("unknown simulated PV '{other}'")));
            return;
        }
    };

    shared.update(|s| {
        s.connected = true;
        s.display_range = Some((min, max));
        s.precision = Some(3);
    });

    let mut n = 0u64;
    let period = Duration::from_secs_f64(period.max(0.01));
    let mut ticker = tokio::time::interval(period);
    loop {
        ticker.tick().await;
        let value = next(n);
        shared.update(|s| {
            s.value = value;
            s.severity = Severity::NoAlarm;
            s.timestamp = Some(SystemTime::now());
        });
        if func == "const" {
            return;
        }
        n += 1;
    }
}

/// A deterministic pseudo-random number in [0, 1) from a counter (splitmix64).
fn random(n: u64) -> f64 {
    let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    #[test]
    fn random_is_in_range() {
        for n in 0..1000 {
            let r = super::random(n);
            assert!((0.0..1.0).contains(&r));
        }
    }
}
