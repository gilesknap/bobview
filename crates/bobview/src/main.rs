//! bobview: a lean viewer for Phoebus `.bob` displays (and CS-Studio BOY `.opi`).

mod app;
mod axis;
mod chrome;
mod colormaps;
mod fetch;
mod group_link;
mod image;
#[cfg(not(target_arch = "wasm32"))]
mod launch;
mod layout;
mod meter;
mod plot;
mod pvmenu;
mod rules;
mod screen;
#[cfg(not(target_arch = "wasm32"))]
mod snapshot;
mod style;
mod symbol;
mod trend;
mod view;
mod widgets;

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use bob::{Macros, Source};
#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;

#[cfg(not(target_arch = "wasm32"))]
const USAGE: &str = "\
usage: bobview [options] [display.bob | display.opi | http(s)://host/display.bob]

options:
  -m, --macros P=X,R=Y   macros for the display
  -p, --project DIR      open a display file through the linked folders of the
                         Eclipse project in DIR (its .project, or for a BOY
                         .opi screen the links of a DLS runcss.sh in DIR), so
                         that links between the project's modules work
  -z, --zoom FACTOR      fixed zoom instead of fit-to-window
  -f, --fullscreen       start full screen
  -k, --keypad           edit values with an on-screen keypad (automatic on touch)
  --multi-window         start in multi-window mode: links open their displays in
                         windows of their own (ctrl+click opens in place)
  --layout NAME          restore the saved window layout NAME
  --no-layout            don't restore the default layout
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

With no display, bobview restores the default window layout when one has
been saved (in $XDG_CONFIG_HOME/bobview/layouts, by default
~/.config/bobview/layouts), else opens index.bob from the bundled displays
(~/.local/share/bobview/displays) when present, else a file chooser.
A display may also be an http:// or https:// URL; files it names are
fetched relative to it. CS-Studio BOY .opi displays are translated as
Phoebus does; a path through an Eclipse project's linked folders (such as
project/module/screen.opi) finds the linked files. A BOY .opi screen beside
a DLS runcss.sh launcher is opened through the links that script gives
CS-Studio, with no .project or -p needed.
PVs without a prefix use Channel Access; EPICS_CA_ADDR_LIST and
EPICS_CA_NAME_SERVERS select the IOCs. Set BOBVIEW_LOG=debug for more output.";

#[cfg(not(target_arch = "wasm32"))]
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
    let mut multi_window = false;
    let mut layout_name = None;
    let mut no_layout = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-m" | "--macros" => macros = parse_macros(&args.next().unwrap_or_default()),
            "-p" | "--project" => project = args.next().map(PathBuf::from),
            "-z" | "--zoom" => zoom = args.next().and_then(|z| z.parse().ok()),
            "-f" | "--fullscreen" => fullscreen = true,
            "-k" | "--keypad" => keypad = true,
            "--multi-window" => multi_window = true,
            "--layout" => match args.next() {
                Some(name) => layout_name = Some(name),
                None => {
                    eprintln!("--layout needs a layout name");
                    std::process::exit(2);
                }
            },
            "--no-layout" => no_layout = true,
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
            "--wait" => wait = seconds(args.next()).unwrap_or(wait),
            "--settle" => settle = seconds(args.next()).unwrap_or(settle),
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
    let is_opi = |p: &std::path::Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("opi"));
    if let (Some(dir), Some(Source::File(path))) = (&project, &file) {
        let Some(project) = bob::project::project_dir(dir) else {
            eprintln!(
                "{} has no .project, nor a {} with links",
                dir.display(),
                bob::project::LAUNCHER
            );
            std::process::exit(2);
        };
        // A launcher's links are CS-Studio's, for BOY screens only.
        if bob::project::through_launcher(&project) && !is_opi(path) {
            eprintln!(
                "{}: {} links apply only to BOY .opi screens",
                path.display(),
                bob::project::LAUNCHER
            );
            std::process::exit(2);
        }
        match bob::project::virtual_path(&project, path) {
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
    } else if let Some(Source::File(path)) = &file
        && is_opi(path)
        && let Some(p) = bob::project::launcher_path(path)
    {
        // A DLS BOY screen beside its runcss.sh: through the project that
        // launcher would give CS-Studio.
        log::info!("{}: opening as {}", path.display(), p.display());
        file = Some(Source::File(p));
    }
    // An .opi opened directly (in an Eclipse project) gets the macros its
    // DLS launcher script would give it; those given here override them.
    if let Some(Source::File(path)) = &file
        && is_opi(path)
        && let Some(defaults) = launch::defaults(path)
    {
        macros = defaults.with(&macros);
    }
    // The named layout, else the default one unless a display was named.
    let layout = if snapshot.is_some() {
        None
    } else {
        let default = file.is_none() && !no_layout && layout::exists(layout::DEFAULT);
        match layout::startup(layout_name, default, layout::load) {
            Ok(layout) => layout,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    };
    // A layout's main window has a display of its own.
    let layout_has_main = layout
        .as_ref()
        .is_some_and(|(l, _)| l.windows.iter().any(|w| w.main));
    let start = file
        .or_else(|| {
            (!layout_has_main)
                .then(default_display)
                .flatten()
                .map(Source::File)
        })
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
        // No vsync: each window's buffer swap would wait for a screen
        // refresh, and every window is drawn in the main one's frame, so a
        // frame took a refresh per window (~120 ms with 7 windows). We only
        // repaint on change, so this doesn't spin.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "bobview",
        options,
        Box::new(move |cc| {
            let mut app = app::BobApp::new(&cc.egui_ctx, start, zoom, ui_scale);
            app.set_keypad(keypad);
            app.set_multi_window(multi_window);
            if let Some((mut layout, name)) = layout {
                layout.multi_window |= multi_window;
                app.restore_layout(layout, Some(name));
            }
            Ok(Box::new(app))
        }),
    )
}

/// A time in seconds: a number, not negative (nor too big for a Duration).
#[cfg(not(target_arch = "wasm32"))]
fn seconds(text: Option<String>) -> Option<f64> {
    text?.parse::<f64>().ok().filter(|s| (0.0..1e9).contains(s))
}

/// Parse `P=X,R=Y` into macros; `\,` and `\\` in a value stand for `,`
/// and `\`.
fn parse_macros(text: &str) -> Macros {
    // Split on unescaped commas, keeping escapes for now.
    let mut pairs = vec![String::new()];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ',' => pairs.push(String::new()),
            '\\' => {
                let last = pairs.last_mut().expect("a pair");
                last.push(c);
                last.extend(chars.next());
            }
            _ => pairs.last_mut().expect("a pair").push(c),
        }
    }
    Macros::from_pairs(pairs.iter().filter_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        Some((k.trim().to_string(), unescape_macro(v.trim())))
    }))
}

fn unescape_macro(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(next @ (',' | '\\'))) => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// The browser build: the page's query string names the display, its
/// macros and the PV Web Socket, e.g.
/// `?display=https://host/opi/index.bob&macros=P=X&pvws=pvws.host:443`
/// (and `zoom=1.5`, `keypad`, `token=SECRET` for a server with `--write`).
/// A relative display is found next to the page; the default is `index.bob`.
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;

    init_logging();
    let window = web_sys::window().expect("a browser window");
    let location = window.location();
    let query = web_sys::UrlSearchParams::new_with_str(&location.search().unwrap_or_default())
        .expect("a query string");
    let page = Source::Url(location.href().unwrap_or_default());
    let display = query.get("display").unwrap_or_else(|| "index.bob".into());
    let start = app::Location {
        source: page.resolve(&display),
        macros: parse_macros(&query.get("macros").unwrap_or_default()),
    };
    if let Some(pvws) = query.get("pvws") {
        pv::set_pvws(&pvws);
    }
    if let Some(token) = query.get("token") {
        pv::set_write_token(&token);
    }
    let zoom = query.get("zoom").and_then(|z| z.parse().ok());
    let keypad = query.has("keypad");

    let canvas = window
        .document()
        .and_then(|d| d.get_element_by_id("bobview"))
        .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .expect("a <canvas id=\"bobview\"> on the page");
    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    let ui_scale = chrome::UiScale::from_env();
                    let mut app = app::BobApp::new(&cc.egui_ctx, Some(start), zoom, ui_scale);
                    app.set_keypad(keypad);
                    Ok(Box::new(app))
                }),
            )
            .await;
        if let Err(e) = started {
            log::error!("cannot start: {e:?}");
        }
    });
}

/// The bundled index display, next to the binary or in ~/.local/share.
#[cfg(not(target_arch = "wasm32"))]
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
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("{:<5} {}", r.level(), r.args());
            #[cfg(target_arch = "wasm32")]
            web_sys::console::log_1(&format!("{:<5} {}", r.level(), r.args()).into());
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

    #[test]
    fn parses_escaped_macros() {
        let m = super::parse_macros(r"LIST=a\,b,P=X");
        assert_eq!(m.get("LIST"), Some("a,b"));
        assert_eq!(m.get("P"), Some("X"));
        let m = super::parse_macros(r"D=C:\\dir\\,E=a\b");
        assert_eq!(m.get("D"), Some(r"C:\dir\"));
        assert_eq!(m.get("E"), Some(r"a\b"));
    }

    #[test]
    fn seconds_are_not_negative() {
        let s = |t: &str| super::seconds(Some(t.into()));
        assert_eq!(s("1.5"), Some(1.5));
        for bad in ["-1", "NaN", "inf", "x"] {
            assert_eq!(s(bad), None);
        }
    }
}
