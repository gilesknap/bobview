//! bobview: a lean viewer for Phoebus `.bob` displays (and CS-Studio BOY `.opi`).

mod app;
mod axis;
mod chrome;
mod colormaps;
mod fetch;
mod group_link;
mod image;
mod launch;
mod meter;
mod plot;
mod pvmenu;
mod rules;
mod screen;
mod snapshot;
mod style;
mod symbol;
mod trend;
mod widgets;

use std::path::PathBuf;

use bob::{Macros, Source};
use eframe::egui;

const USAGE: &str = "\
usage: bobview [options] [display.bob | display.opi | http(s)://host/display.bob]

options:
  -m, --macros P=X,R=Y   macros for the display
  -p, --project DIR      open a display file through the linked folders of the
                         Eclipse project in DIR (its .project), so that links
                         between the project's modules work
  -z, --zoom FACTOR      fixed zoom instead of fit-to-window
  -f, --fullscreen       start full screen
  -k, --keypad           edit values with an on-screen keypad (automatic on touch)
  --ui-scale F|auto      size of the toolbar and menus relative to the desktop's
                         scale (default auto: smaller on a small high-DPI screen;
                         also set by BOBVIEW_UI_SCALE)
  --snapshot OUT.png     render to a PNG without a window and exit
  --size WxH             snapshot size in pixels (default 1280x800); 'auto' starts at
                         the window's default size and resizes it as a window
                         manager would, on a monitor of 1920x1080 points
  --scale-factor F       snapshot as on a screen with this desktop scale (default 1),
                         e.g. 2.75 for a phone; --click positions are in pixels
  --wait SECONDS         time for PVs to connect before a snapshot (default 2)
  --click X,Y            click here before the snapshot (repeatable)
  --right-click X,Y      right-click here before the snapshot (repeatable)
  --settle SECONDS       time after the last click before the snapshot (default 1)
  -h, --help             show this help
  -V, --version          show the version

With no display, bobview opens index.bob from the bundled displays
(~/.local/share/bobview/displays) when present, else a file chooser.
A display may also be an http:// or https:// URL; files it names are
fetched relative to it. CS-Studio BOY .opi displays are translated as
Phoebus does; a path through an Eclipse project's linked folders (such as
project/module/screen.opi) finds the linked files.
PVs without a prefix use Channel Access; EPICS_CA_ADDR_LIST and
EPICS_CA_NAME_SERVERS select the IOCs. Set BOBVIEW_LOG=debug for more output.";

fn main() -> eframe::Result {
    init_logging();
    let mut file = None;
    let mut macros = Macros::default();
    let mut zoom = None;
    let mut fullscreen = false;
    let mut snapshot = None;
    let mut size = Some([1280, 800]);
    let mut scale_factor = 1.0;
    let mut ui_scale = None;
    let mut wait = 2.0;
    let mut settle = 1.0;
    let mut clicks = Vec::new();
    let mut keypad = false;
    let mut project = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-m" | "--macros" => macros = parse_macros(&args.next().unwrap_or_default()),
            "-p" | "--project" => project = args.next().map(PathBuf::from),
            "-z" | "--zoom" => zoom = args.next().and_then(|z| z.parse().ok()),
            "-f" | "--fullscreen" => fullscreen = true,
            "-k" | "--keypad" => keypad = true,
            "--ui-scale" => {
                let text = args.next().unwrap_or_default();
                ui_scale = chrome::UiScale::parse(&text);
                if ui_scale.is_none() {
                    eprintln!("--ui-scale: expected a number or auto, not {text:?}");
                    std::process::exit(2);
                }
            }
            "--scale-factor" => {
                scale_factor = args
                    .next()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|s| *s > 0.0)
                    .unwrap_or(scale_factor)
            }
            "--click" | "--right-click" => {
                let button = if arg == "--click" {
                    egui::PointerButton::Primary
                } else {
                    egui::PointerButton::Secondary
                };
                if let Some((x, y)) = args.next().as_deref().and_then(|s| s.split_once(','))
                    && let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse())
                {
                    clicks.push((egui::pos2(x, y), button));
                }
            }
            "--snapshot" => snapshot = args.next().map(PathBuf::from),
            "--size" => match args.next().as_deref() {
                Some("auto") => size = None,
                Some(s) => {
                    if let Some((w, h)) = s.split_once('x')
                        && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                    {
                        size = Some([w, h]);
                    }
                }
                None => {}
            },
            "--wait" => wait = args.next().and_then(|w| w.parse().ok()).unwrap_or(wait),
            "--settle" => settle = args.next().and_then(|w| w.parse().ok()).unwrap_or(settle),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("bobview {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => file = Some(Source::parse(&arg)),
        }
    }
    let ui_scale = ui_scale.unwrap_or_else(chrome::UiScale::from_env);
    if let (Some(dir), Some(Source::File(path))) = (&project, &file) {
        match bob::project::virtual_path(dir, path) {
            Some(p) => file = Some(Source::File(p)),
            None => {
                eprintln!(
                    "{} is not in a linked folder of {}",
                    path.display(),
                    dir.display()
                );
                std::process::exit(2);
            }
        }
    }
    // An .opi opened directly gets the macros its DLS launcher script would
    // give it; those given here override them.
    if let Some(Source::File(path)) = &file
        && path.extension().is_some_and(|x| x == "opi")
        && let Some(defaults) = launch::defaults(path)
    {
        macros = defaults.with(&macros);
    }
    let start = file
        .or_else(|| default_display().map(Source::File))
        .map(|source| app::Location { source, macros });

    if let Some(out) = snapshot {
        let Some(start) = start else {
            eprintln!("--snapshot needs a display file");
            std::process::exit(2);
        };
        let opts = snapshot::Options {
            size,
            scale_factor,
            ui_scale,
            zoom,
            wait: std::time::Duration::from_secs_f64(wait),
            clicks,
            settle: std::time::Duration::from_secs_f64(settle),
            keypad,
        };
        if let Err(e) = snapshot::run(start, &opts, &out) {
            eprintln!("snapshot failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("bobview")
            .with_app_id("bobview")
            .with_inner_size(chrome::DEFAULT_WINDOW)
            .with_fullscreen(fullscreen),
        ..Default::default()
    };
    eframe::run_native(
        "bobview",
        options,
        Box::new(move |cc| {
            let mut app = app::BobApp::new(&cc.egui_ctx, start, zoom, ui_scale);
            app.set_keypad(keypad);
            Ok(Box::new(app))
        }),
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
