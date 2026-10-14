//! Print live values of PVs through bobview's PV layer.
//!
//! Usage: bobview-probe [--seconds N] [--put NAME=VALUE]... NAME...

use std::time::{Duration, Instant};

fn main() {
    let mut seconds = 5.0;
    let mut puts = Vec::new();
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(seconds),
            "--put" => puts.extend(args.next()),
            "-h" | "--help" => {
                println!("usage: bobview-probe [--seconds N] [--put NAME=VALUE]... NAME...");
                return;
            }
            _ => names.push(arg),
        }
    }

    let manager = pv::PvManager::new();
    let mut pvs: Vec<_> = names
        .iter()
        .map(|n| (manager.subscribe(n), String::new()))
        .collect();
    let writers: Vec<_> = puts
        .iter()
        .filter_map(|p| {
            let pair = p.split_once('=');
            if pair.is_none() {
                eprintln!("ignoring --put {p}: expected NAME=VALUE");
            }
            pair
        })
        .map(|(n, v)| (manager.subscribe(n), v.to_string()))
        .collect();
    let mut written = false;

    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        if !written && writers.iter().all(|(pv, _)| pv.state().connected) {
            for (pv, value) in &writers {
                pv.write(value.clone());
            }
            written = true;
        }
        for (pv, last) in &mut pvs {
            let s = pv.state();
            let line = if let Some(e) = &s.error {
                format!("error: {e}")
            } else if !s.connected {
                "disconnected".to_string()
            } else {
                format!("{} {:?}", s.format(None, true), s.severity)
            };
            if line != *last {
                println!("{:<30} {line}", pv.name().to_string());
                *last = line;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
