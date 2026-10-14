//! bobview: a lean viewer for Phoebus `.bob` displays.

mod app;
mod rules;
mod screen;
mod snapshot;
mod style;
mod widgets;

use std::path::PathBuf;

use bob::Macros;
use eframe::egui;

const USAGE: &str = "\
usage: bobview [options] [display.bob]

options:
  -m, --macros P=X,R=Y   macros for the display
  -z, --zoom FACTOR      fixed zoom instead of fit-to-window
  -f, --fullscreen       start full screen
  --snapshot OUT.png     render to a PNG without a window and exit
  --size WxH             snapshot size (default 1280x800)
  --wait SECONDS         time for PVs to connect before a snapshot (default 2)
  -h, --help             show this help
  -V, --version          show the version

With no display, bobview opens index.bob from the bundled displays
(~/.local/share/bobview/displays) when present, else a file chooser.
PVs without a prefix use Channel Access; EPICS_CA_ADDR_LIST and
EPICS_CA_NAME_SERVERS select the IOCs. Set BOBVIEW_LOG=debug for more output.";

fn main() -> eframe::Result {
    init_logging();
    let mut file = None;
    let mut macros = Macros::default();
    let mut zoom = None;
    let mut fullscreen = false;
    let mut snapshot = None;
    let mut size = [1280, 800];
    let mut wait = 2.0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-m" | "--macros" => macros = parse_macros(&args.next().unwrap_or_default()),
            "-z" | "--zoom" => zoom = args.next().and_then(|z| z.parse().ok()),
            "-f" | "--fullscreen" => fullscreen = true,
            "--snapshot" => snapshot = args.next().map(PathBuf::from),
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x')) {
                    size = [w.parse().unwrap_or(size[0]), h.parse().unwrap_or(size[1])];
                }
            }
            "--wait" => wait = args.next().and_then(|w| w.parse().ok()).unwrap_or(wait),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("bobview {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => file = Some(PathBuf::from(arg)),
        }
    }
    let start = file
        .or_else(default_display)
        .map(|path| app::Location { path, macros });

    if let Some(out) = snapshot {
        let Some(start) = start else {
            eprintln!("--snapshot needs a display file");
            std::process::exit(2);
        };
        if let Err(e) = snapshot::run(
            start,
            size,
            zoom,
            std::time::Duration::from_secs_f64(wait),
            &out,
        ) {
            eprintln!("snapshot failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("bobview")
            .with_app_id("bobview")
            .with_inner_size([1024.0, 720.0])
            .with_fullscreen(fullscreen),
        ..Default::default()
    };
    eframe::run_native(
        "bobview",
        options,
        Box::new(move |cc| Ok(Box::new(app::BobApp::new(&cc.egui_ctx, start, zoom)))),
    )
}

/// Parse `P=X,R=Y` into macros.
fn parse_macros(text: &str) -> Macros {
    Macros::from_pairs(text.split(',').filter_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        Some((k.trim().to_string(), v.trim().to_string()))
    }))
}

/// The bundled index display, next to the binary or in ~/.local/share.
fn default_display() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|bin| bin.parent())
    {
        candidates.push(prefix.join("share/bobview/displays/index.bob"));
        candidates.push(prefix.join("displays/index.bob"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/share/bobview/displays/index.bob"));
    }
    candidates.into_iter().find(|p| p.exists())
}

struct Logger(log::LevelFilter);

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // Library chatter (one line per CA channel connect) only when debugging.
        let library = ["eframe", "egui", "epics", "winit", "glutin"]
            .iter()
            .any(|t| m.target().starts_with(t));
        let limit = if library && self.0 < log::LevelFilter::Debug {
            log::LevelFilter::Warn
        } else {
            self.0
        };
        m.level() <= limit
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{:<5} {}", r.level(), r.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    let level = match std::env::var("BOBVIEW_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("error") => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    };
    let logger = Box::leak(Box::new(Logger(level)));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_macros() {
        let m = super::parse_macros("P=BL01T, R=:DET:");
        assert_eq!(m.get("P"), Some("BL01T"));
        assert_eq!(m.get("R"), Some(":DET:"));
    }
}
