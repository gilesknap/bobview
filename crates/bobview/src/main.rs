//! bobview: a lean viewer for Phoebus `.bob` displays (and CS-Studio BOY `.opi`).

mod app;
mod axis;
mod chrome;
mod colormaps;
mod fetch;
mod format;
mod group_link;
mod icons;
mod image;
#[cfg(unix)]
mod launch;
/// DLS launcher scripts are shell scripts: not run on Windows.
#[cfg(not(any(unix, target_arch = "wasm32")))]
mod launch {
    use std::path::{Path, PathBuf};

    use bob::{Macros, Source};

    pub fn command_target(
        _base: Option<&Source>,
        _directory: Option<Source>,
        command: &str,
    ) -> Option<(PathBuf, Macros)> {
        // Called for every command link the tree finds, as well as for a
        // button pressed (which warns that the command is not supported).
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| log::warn!("launcher scripts are not run on this platform"));
        log::debug!("not running '{command}'");
        None
    }

    pub fn defaults(_file: &Path) -> Option<Macros> {
        None
    }
}
mod layout;
mod meter;
mod plot;
mod polygon;
mod pvmenu;
mod rules;
mod screen;
#[cfg(not(target_arch = "wasm32"))]
mod snapshot;
mod style;
mod svg;
mod symbol;
mod tree;
mod trend;
mod view;
mod widgets;

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use bob::{Macros, Source};
#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;

#[cfg(not(target_arch = "wasm32"))]
const AFTER_HELP: &str = "\
With no display, bobview restores the default window layout when one has
been saved (in $XDG_CONFIG_HOME/bobview/layouts, by default
~/.config/bobview/layouts, or %APPDATA%\\bobview\\layouts on Windows),
else opens index.bob from the bundled displays
(~/.local/share/bobview/displays) when present, else a file chooser.
A display may also be an http:// or https:// URL; files it names are
fetched relative to it. CS-Studio BOY .opi displays are translated as
Phoebus does; a path through an Eclipse project's linked folders (such as
project/module/screen.opi) finds the linked files. A BOY .opi screen beside
a DLS runcss.sh launcher is opened through the links that script gives
CS-Studio, with no .project or -p needed.
PVs without a prefix use Channel Access; EPICS_CA_ADDR_LIST and
EPICS_CA_NAME_SERVERS select the IOCs. Set BOBVIEW_LOG=debug for more output.";

/// A lean viewer for Phoebus .bob displays (and CS-Studio BOY .opi).
#[cfg(not(target_arch = "wasm32"))]
#[derive(clap::Parser, Debug)]
#[command(
    name = "bobview",
    version,
    after_help = AFTER_HELP,
    args_override_self = true,
    max_term_width = 100
)]
struct Cli {
    /// display.bob, display.opi or http(s)://host/display.bob
    #[arg(value_name = "DISPLAY")]
    display: Option<String>,
    /// Macros for the display
    #[arg(short, long, value_name = "P=X,R=Y")]
    macros: Option<String>,
    /// Open a display file through the linked folders of the Eclipse project
    /// in DIR (its .project, or for a BOY .opi screen the links of a DLS
    /// runcss.sh in DIR), so that links between the project's modules work
    #[arg(short, long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Fixed zoom (0.2 to 5) instead of fit-to-window
    #[arg(short, long, value_name = "FACTOR", value_parser = parse_zoom)]
    zoom: Option<f32>,
    /// Start full screen
    #[arg(short, long)]
    fullscreen: bool,
    /// No longer supported: text entries take the keyboard
    #[arg(short, long, hide = true)]
    keypad: bool,
    /// No longer needed: pin mode replaced it
    #[arg(long, hide = true)]
    multi_window: bool,
    /// Refuse every write to a PV (except loc:// ones, which stay in
    /// bobview): controls are disabled and a write action says why it did
    /// nothing
    #[arg(long)]
    read_only: bool,
    /// Restore the saved window layout NAME
    #[arg(long, value_name = "NAME")]
    layout: Option<String>,
    /// Don't restore the default layout
    #[arg(long)]
    no_layout: bool,
    /// Size of the toolbar and menus relative to the desktop's scale
    /// (default auto: smaller on a small high-DPI screen; also set by
    /// BOBVIEW_UI_SCALE)
    #[arg(long, value_name = "F|auto", value_parser = parse_ui_scale)]
    ui_scale: Option<chrome::UiScale>,
    /// Render to a PNG without a window and exit
    #[arg(long, value_name = "OUT.png")]
    snapshot: Option<PathBuf>,
    /// Snapshot size in pixels; 'auto' starts at the window's default size
    /// and resizes it as a window manager would, on a monitor of 1920x1080
    /// points
    #[arg(long, value_name = "WxH", default_value = "1280x800", value_parser = parse_size)]
    size: Size,
    /// Snapshot as on a screen with this desktop scale, e.g. 2.75 for a
    /// phone; --click positions are in pixels
    #[arg(long, value_name = "F", default_value_t = 1.0, value_parser = parse_scale_factor)]
    scale_factor: f32,
    /// Time for PVs to connect before a snapshot
    #[arg(long, value_name = "SECONDS", default_value_t = 2.0, value_parser = parse_seconds)]
    wait: f64,
    /// Click here before the snapshot (repeatable)
    #[arg(long, value_name = "X,Y", value_parser = parse_point)]
    click: Vec<egui::Pos2>,
    /// Right-click here before the snapshot (repeatable)
    #[arg(long, value_name = "X,Y", value_parser = parse_point)]
    right_click: Vec<egui::Pos2>,
    /// Time after the last click before the snapshot
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0, value_parser = parse_seconds)]
    settle: f64,
}

/// A snapshot's size in pixels, or `None` for `auto`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Size(Option<[u32; 2]>);

#[cfg(not(target_arch = "wasm32"))]
impl Cli {
    /// Parse the command line, exiting with a message if it is wrong.
    fn from_env() -> (Self, Vec<(egui::Pos2, egui::PointerButton)>) {
        use clap::{CommandFactory, FromArgMatches};
        let matches = Self::command().get_matches();
        let cli = Self::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
        let clicks = cli.clicks(&matches);
        (cli, clicks)
    }

    /// Both kinds of click, in the order given.
    fn clicks(&self, matches: &clap::ArgMatches) -> Vec<(egui::Pos2, egui::PointerButton)> {
        let at = |id| matches.indices_of(id).into_iter().flatten();
        let mut clicks: Vec<_> = (at("click").zip(&self.click))
            .map(|(i, p)| (i, *p, egui::PointerButton::Primary))
            .chain(
                at("right_click")
                    .zip(&self.right_click)
                    .map(|(i, p)| (i, *p, egui::PointerButton::Secondary)),
            )
            .collect();
        clicks.sort_by_key(|(i, ..)| *i);
        clicks.into_iter().map(|(_, p, b)| (p, b)).collect()
    }
}

/// A zoom factor in the range the view allows.
fn parse_zoom(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|z| (0.2..=5.0).contains(z))
        .ok_or_else(|| format!("expected a factor from 0.2 to 5, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_ui_scale(text: &str) -> Result<chrome::UiScale, String> {
    chrome::UiScale::parse(text).ok_or_else(|| format!("expected a number or auto, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_size(text: &str) -> Result<Size, String> {
    if text.trim() == "auto" {
        return Ok(Size(None));
    }
    text.split_once('x')
        .and_then(|(w, h)| Some([w.trim().parse().ok()?, h.trim().parse().ok()?]))
        .filter(|[w, h]: &[u32; 2]| *w > 0 && *h > 0)
        .map(|wh| Size(Some(wh)))
        .ok_or_else(|| format!("expected WIDTHxHEIGHT in pixels or auto, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_scale_factor(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or_else(|| format!("expected a scale above 0, not {text:?}"))
}

/// A time in seconds: a number, not negative (nor too big for a Duration).
#[cfg(not(target_arch = "wasm32"))]
fn parse_seconds(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|s| (0.0..1e9).contains(s))
        .ok_or_else(|| format!("expected a number of seconds, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_point(text: &str) -> Result<egui::Pos2, String> {
    text.split_once(',')
        .and_then(|(x, y)| Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?)))
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .ok_or_else(|| format!("expected X,Y in pixels, not {text:?}"))
}

/// Why bobview stopped early, and the exit status to say so.
#[cfg(not(target_arch = "wasm32"))]
struct Failure {
    code: u8,
    message: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Failure {
    /// The command line asks for something that cannot be done.
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    init_logging();
    let (cli, clicks) = Cli::from_env();
    match run(cli, clicks) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("bobview: {}", f.message);
            std::process::ExitCode::from(f.code)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(cli: Cli, clicks: Vec<(egui::Pos2, egui::PointerButton)>) -> Result<(), Failure> {
    if cli.keypad {
        eprintln!("--keypad is no longer supported: ignored");
    }
    if cli.multi_window {
        // Pin mode replaced multi-window mode: shift+click follows a link
        // as it did.
        eprintln!(
            "--multi-window is no longer needed: links from the pinned main \
             window open in another window, shift+click in a new one"
        );
    }
    let mut file = cli.display.as_deref().map(fetch::user_named);
    let mut macros = parse_macros(cli.macros.as_deref().unwrap_or_default());
    let ui_scale = cli.ui_scale.unwrap_or_else(chrome::UiScale::from_env);
    let is_opi = |p: &std::path::Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("opi"));
    if let (Some(dir), Some(Source::File(path))) = (&cli.project, &file) {
        let Some(project) = bob::project::project_dir(dir) else {
            return Err(Failure::usage(format!(
                "{} has no .project, nor a {} with links",
                dir.display(),
                bob::project::LAUNCHER
            )));
        };
        // A launcher's links are CS-Studio's, for BOY screens only.
        if bob::project::through_launcher(&project) && !is_opi(path) {
            return Err(Failure::usage(format!(
                "{}: {} links apply only to BOY .opi screens",
                path.display(),
                bob::project::LAUNCHER
            )));
        }
        match bob::project::virtual_path(&project, path) {
            Some(p) => file = Some(Source::File(p)),
            None => {
                return Err(Failure::usage(format!(
                    "{} is not in a linked folder of {}",
                    path.display(),
                    dir.display()
                )));
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
    let layout = if cli.snapshot.is_some() {
        None
    } else {
        let default = file.is_none() && !cli.no_layout && layout::exists(layout::DEFAULT);
        layout::startup(cli.layout, default, layout::load).map_err(Failure::usage)?
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

    if let Some(out) = cli.snapshot {
        let start = start.ok_or_else(|| Failure::usage("--snapshot needs a display file"))?;
        let opts = snapshot::Options {
            size: cli.size.0,
            scale_factor: cli.scale_factor,
            ui_scale,
            zoom: cli.zoom,
            wait: std::time::Duration::from_secs_f64(cli.wait),
            clicks,
            settle: std::time::Duration::from_secs_f64(cli.settle),
            read_only: cli.read_only,
        };
        return snapshot::run(start, &opts, &out)
            .map_err(|e| Failure::failed(format!("snapshot failed: {e}")));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("bobview")
            .with_app_id("bobview")
            .with_inner_size(chrome::DEFAULT_WINDOW)
            .with_fullscreen(cli.fullscreen),
        // No vsync: each window's buffer swap would wait for a screen
        // refresh, and eframe draws the windows one after another on one
        // thread, so windows repainting together would take a refresh each
        // (~120 ms for 7, when they were all drawn in the main one's frame).
        // We only repaint on change, so this doesn't spin.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let (zoom, read_only) = (cli.zoom, cli.read_only);
    eframe::run_native(
        "bobview",
        options,
        Box::new(move |cc| {
            let mut app = app::BobApp::new(&cc.egui_ctx, start, zoom, ui_scale);
            app.set_read_only(read_only);
            if let Some((layout, name)) = layout {
                app.restore_layout(layout, Some(name));
            }
            Ok(Box::new(app::App::new(app)))
        }),
    )
    .map_err(|e| Failure::failed(e.to_string()))
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
/// (and `zoom=1.5`, `token=SECRET` for a server with `--write`).
/// A relative display is found next to the page; the default is `index.bob`.
/// With a token, displays come only from the page's server, and writes
/// carry the token only to a PVWS there.
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
        // The token goes only to the page's own server, and only its
        // displays may use it.
        pv::set_write_token(&token);
        fetch::same_origin_only();
    }
    let zoom = query.get("zoom").and_then(|z| {
        parse_zoom(&z)
            .inspect_err(|e| log::warn!("zoom: {e}; fitting to the window"))
            .ok()
    });

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
                    let app = app::BobApp::new(&cc.egui_ctx, Some(start), zoom, ui_scale);
                    Ok(Box::new(app::App::new(app)))
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
    fn zoom_is_in_the_views_range() {
        assert_eq!(super::parse_zoom("1.5"), Ok(1.5));
        for bad in ["0", "-1", "6", "NaN", "x", ""] {
            assert!(super::parse_zoom(bad).is_err(), "{bad}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod cli {
        use clap::{CommandFactory, FromArgMatches};
        use eframe::egui::{PointerButton, pos2};

        use super::super::{Cli, Size};

        fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
            let args = std::iter::once("bobview").chain(args.iter().copied());
            let matches = Cli::command().try_get_matches_from(args)?;
            Cli::from_arg_matches(&matches)
        }

        #[test]
        fn the_command_is_well_formed() {
            Cli::command().debug_assert();
        }

        #[test]
        fn defaults() {
            let cli = parse(&["screen.bob"]).unwrap();
            assert_eq!(cli.display.as_deref(), Some("screen.bob"));
            assert_eq!(cli.size, Size(Some([1280, 800])));
            assert_eq!((cli.scale_factor, cli.wait, cli.settle), (1.0, 2.0, 1.0));
            assert_eq!(cli.zoom, None);
        }

        #[test]
        fn reads_every_option() {
            let cli = parse(&[
                "-m",
                "P=X",
                "-z",
                "1.5",
                "-f",
                "--read-only",
                "--size",
                "auto",
                "--scale-factor",
                "2.75",
                "--wait",
                "0",
                "--settle",
                "0.5",
                "--ui-scale",
                "auto",
                "--snapshot",
                "out.png",
                "x.bob",
            ])
            .unwrap();
            assert_eq!(cli.macros.as_deref(), Some("P=X"));
            assert_eq!(cli.zoom, Some(1.5));
            assert!(cli.fullscreen && cli.read_only);
            assert_eq!(cli.size, Size(None));
            assert_eq!((cli.scale_factor, cli.wait, cli.settle), (2.75, 0.0, 0.5));
        }

        #[test]
        fn refuses_bad_values() {
            for args in [
                &["--zoom", "big"][..],
                &["-z", "0"],
                &["--size", "800"],
                &["--size", "0x600"],
                &["--size", "800x-1"],
                &["--click", "10"],
                &["--click", "a,b"],
                &["--right-click", "1;2"],
                &["--scale-factor", "0"],
                &["--scale-factor", "x"],
                &["--wait", "-1"],
                &["--settle", "NaN"],
                &["--ui-scale", "0"],
                &["--no-such-option"],
                &["a.bob", "b.bob"],
            ] {
                let e = parse(args).expect_err(&format!("{args:?}"));
                assert_eq!(e.exit_code(), 2, "{args:?}");
            }
        }

        #[test]
        fn keeps_clicks_in_order() {
            let args = ["--click", "1,2", "--right-click", "3, 4", "--click", "5,6"];
            let argv = std::iter::once("bobview").chain(args);
            let matches = Cli::command().try_get_matches_from(argv).unwrap();
            let cli = Cli::from_arg_matches(&matches).unwrap();
            assert_eq!(
                cli.clicks(&matches),
                [
                    (pos2(1.0, 2.0), PointerButton::Primary),
                    (pos2(3.0, 4.0), PointerButton::Secondary),
                    (pos2(5.0, 6.0), PointerButton::Primary),
                ]
            );
        }

        #[test]
        fn a_repeated_option_takes_the_last_value() {
            let cli = parse(&["-z", "2", "-z", "3"]).unwrap();
            assert_eq!(cli.zoom, Some(3.0));
        }

        #[test]
        fn retired_options_are_still_accepted() {
            let cli = parse(&["-k", "--multi-window"]).unwrap();
            assert!(cli.keypad && cli.multi_window);
        }
    }
}
