//! Saved window layouts: which displays are open, in which windows, where.
//!
//! A layout is a JSON file in `$XDG_CONFIG_HOME/bobview/layouts` (by default
//! `~/.config/bobview/layouts`), named after the layout; `default.json` is
//! restored at startup. Positions and sizes are in desktop points (egui's
//! points times its zoom factor), so they don't depend on the controls' scale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bob::{Macros, Source};
use eframe::egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::app::Location;

/// The layout restored at startup.
pub const DEFAULT: &str = "default";

/// The layout file format's version.
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub version: u32,
    #[serde(default)]
    pub multi_window: bool,
    pub windows: Vec<WindowLayout>,
}

/// One window of a layout: the main window, or a display window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowLayout {
    #[serde(default, skip_serializing_if = "is_false")]
    pub main: bool,
    /// The display: an absolute path or a URL, as `Location::source`.
    pub display: String,
    /// The Eclipse project the display's path goes through, so its links
    /// resolve after a restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// The window's effective macros.
    #[serde(default)]
    pub macros: BTreeMap<String, String>,
    /// The outer position, or None where the window system doesn't say
    /// (Wayland).
    #[serde(default)]
    pub position: Option<[f32; 2]>,
    /// The inner size.
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    /// The main window's zoom: a factor, or "fit".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<Zoom>,
    /// Whether a display window shows its toolbar.
    #[serde(default, skip_serializing_if = "is_false")]
    pub toolbar: bool,
}

/// A number, or the string "fit".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ZoomText", into = "ZoomText")]
pub enum Zoom {
    Factor(f32),
    /// Fit the display to the window.
    Fit,
}

/// How a zoom is written.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ZoomText {
    Factor(f32),
    Text(String),
}

impl TryFrom<ZoomText> for Zoom {
    type Error = String;

    fn try_from(z: ZoomText) -> Result<Self, String> {
        match z {
            ZoomText::Factor(f) => Ok(Self::Factor(f)),
            ZoomText::Text(t) if t == "fit" => Ok(Self::Fit),
            ZoomText::Text(t) => Err(format!("zoom {t:?}: expected a number or \"fit\"")),
        }
    }
}

impl From<Zoom> for ZoomText {
    fn from(z: Zoom) -> Self {
        match z {
            Zoom::Factor(f) => Self::Factor(f),
            Zoom::Fit => Self::Text("fit".into()),
        }
    }
}

impl Zoom {
    pub fn new(fit: bool, zoom: f32) -> Self {
        if fit { Self::Fit } else { Self::Factor(zoom) }
    }

    /// A fixed zoom, or None to fit to the window.
    pub fn factor(&self) -> Option<f32> {
        match self {
            Self::Factor(f) => Some(f.clamp(0.2, 5.0)),
            Self::Fit => None,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Where a window is and how big, in desktop points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geometry {
    pub position: Option<Pos2>,
    pub size: Option<Vec2>,
}

impl WindowLayout {
    /// The layout of a window showing `loc`.
    pub fn new(loc: &Location, geometry: Geometry) -> Self {
        let mut macros = BTreeMap::new();
        // Later definitions win, as in `Macros::get`.
        for (k, v) in loc.macros.iter() {
            macros.insert(k.to_string(), v.to_string());
        }
        Self {
            main: false,
            display: loc.source.to_string(),
            project: match &loc.source {
                Source::File(path) => project_of(path),
                Source::Url(_) => None,
            },
            macros,
            position: geometry.position.map(|p| [p.x, p.y]),
            size: geometry.size.map(|s| [s.x, s.y]),
            zoom: None,
            toolbar: false,
        }
    }

    /// The display and macros to show.
    pub fn location(&self) -> Location {
        let mut source = Source::parse(&self.display);
        // A real path saved with its project goes back through the project.
        if let (Some(dir), Source::File(path)) = (&self.project, &source)
            && !path.starts_with(dir)
            && let Some(p) = bob::project::virtual_path(dir, path)
        {
            source = Source::File(p);
        }
        Location {
            source,
            macros: Macros::from_pairs(self.macros.clone()),
        }
    }

    /// Where to put the window, dropping a position on none of `monitors`
    /// (a laptop that was docked when the layout was saved); with no
    /// monitors known, positions are kept.
    pub fn geometry(&self, monitors: &[Rect]) -> Geometry {
        let size = self
            .size
            .map(|[w, h]| Vec2::new(w, h))
            .filter(|s| s.x >= 1.0 && s.y >= 1.0 && s.x.is_finite() && s.y.is_finite());
        let position = self
            .position
            .map(|[x, y]| Pos2::new(x, y))
            .filter(|p| p.x.is_finite() && p.y.is_finite())
            .filter(|p| monitors.is_empty() || on_screen(*p, size, monitors));
        Geometry { position, size }
    }
}

/// Whether enough of a window's top edge (its title bar) at `pos` lies on a
/// monitor to grab it.
fn on_screen(pos: Pos2, size: Option<Vec2>, monitors: &[Rect]) -> bool {
    let width = size.map_or(200.0, |s| s.x).max(1.0);
    let top = Rect::from_min_size(pos, Vec2::new(width, 30.0));
    monitors.iter().any(|m| {
        let seen = m.intersect(top);
        seen.is_positive() && seen.width() >= width.min(40.0) && seen.height() >= 10.0
    })
}

/// The project directory that `path` reaches its file through: the nearest
/// directory above it holding a `.project`, when `path` itself doesn't exist.
fn project_of(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return None;
    }
    path.ancestors()
        .skip(1)
        .find(|d| d.join(".project").is_file())
        .map(Path::to_path_buf)
}

impl Layout {
    pub fn new(multi_window: bool, windows: Vec<WindowLayout>) -> Self {
        Self {
            version: VERSION,
            multi_window,
            windows,
        }
    }
}

/// `$XDG_CONFIG_HOME/bobview`, by default `~/.config/bobview`.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("bobview"))
}

fn layouts_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("layouts"))
}

/// Whether `name` can name a layout file: not empty, no path in it.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

fn file(name: &str) -> Option<PathBuf> {
    valid_name(name)
        .then(|| layouts_dir().map(|d| d.join(format!("{}.json", name.trim()))))
        .flatten()
}

/// The saved layouts' names, default first and the rest in order.
pub fn list() -> Vec<String> {
    let Some(dir) = layouts_dir() else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension()? == "json")
                        .then(|| p.file_stem()?.to_str().map(str::to_string))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort_by(|a, b| (a != DEFAULT).cmp(&(b != DEFAULT)).then(a.cmp(b)));
    names
}

pub fn exists(name: &str) -> bool {
    file(name).is_some_and(|f| f.is_file())
}

pub fn load(name: &str) -> Result<Layout, String> {
    let path = file(name).ok_or_else(|| format!("no layout {name:?}"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let layout: Layout =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if layout.version > VERSION {
        return Err(format!(
            "{}: layout version {} is newer than this bobview's {VERSION}",
            path.display(),
            layout.version
        ));
    }
    Ok(layout)
}

/// The layout to start with: `named` (with `--layout`), which must load,
/// else the default one when `default`, which bobview starts without, with
/// a warning, if it doesn't load (written by a newer bobview, say).
#[cfg_attr(
    target_arch = "wasm32",
    expect(dead_code, reason = "a browser starts from its address")
)]
pub fn startup(
    named: Option<String>,
    default: bool,
    load: impl Fn(&str) -> Result<Layout, String>,
) -> Result<Option<(Layout, String)>, String> {
    if let Some(name) = named {
        return load(&name)
            .map(|l| Some((l, name.clone())))
            .map_err(|e| format!("cannot restore layout {name}: {e}"));
    }
    if !default {
        return Ok(None);
    }
    match load(DEFAULT) {
        Ok(l) => Ok(Some((l, DEFAULT.to_string()))),
        Err(e) => {
            log::warn!("not restoring the default layout: {e}");
            Ok(None)
        }
    }
}

pub fn save(name: &str, layout: &Layout) -> Result<(), String> {
    let path = file(name).ok_or_else(|| format!("not a layout name: {name:?}"))?;
    let write = || -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(layout).map_err(std::io::Error::other)?;
        write_atomic(&path, &(text + "\n"))
    };
    write().map_err(|e| format!("{}: {e}", path.display()))?;
    log::info!("saved layout {}", path.display());
    Ok(())
}

/// Write `text` to `path` through a file beside it, so that a crash or a
/// full disk leaves the old file whole rather than half written.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // Hidden and not .json, so never listed as a layout.
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            std::fs::remove_file(&tmp).ok();
        })
}

pub fn delete(name: &str) -> Result<(), String> {
    let path = file(name).ok_or_else(|| format!("no layout {name:?}"))?;
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Settings kept between runs, beside the layouts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Save the default layout when bobview exits.
    #[serde(default)]
    pub save_default_on_exit: bool,
}

impl Settings {
    pub fn load() -> Self {
        config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(dir) = config_dir() else { return };
        let written = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|text| write_atomic(&dir.join("settings.json"), &(text + "\n")));
        if let Err(e) = written {
            log::warn!("cannot save settings in {}: {e}", dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_urls_macros_and_unknown_positions() {
        let main = Location {
            source: Source::parse("/dls_sw/prod/vacuum/opi/diamondVacuumOverview.opi"),
            macros: Macros::default(),
        };
        let cell = Location {
            source: Source::parse("https://host/opi/cell.bob"),
            macros: Macros::from_pairs([("cell", "03"), ("prefix", "SR"), ("cell", "04")]),
        };
        let mut first = WindowLayout::new(
            &main,
            Geometry {
                position: Some(Pos2::new(0.0, 0.0)),
                size: Some(Vec2::new(1040.0, 610.0)),
            },
        );
        first.main = true;
        first.zoom = Some(Zoom::new(true, 1.0));
        let mut second = WindowLayout::new(
            &cell,
            Geometry {
                position: None,
                size: Some(Vec2::new(1080.0, 530.0)),
            },
        );
        second.toolbar = true;
        let layout = Layout::new(true, vec![first, second]);
        let text = serde_json::to_string_pretty(&layout).unwrap();
        assert!(text.contains("\"position\": null"), "{text}");
        assert!(text.contains("\"zoom\": \"fit\""), "{text}");
        let back: Layout = serde_json::from_str(&text).unwrap();
        assert_eq!(back, layout);

        let loc = back.windows[1].location();
        assert_eq!(loc.source, cell.source);
        // The effective macros: the later `cell` wins.
        assert_eq!(loc.macros.get("cell"), Some("04"));
        assert_eq!(loc.macros.get("prefix"), Some("SR"));
        assert_eq!(back.windows[0].zoom.as_ref().unwrap().factor(), None);
        assert_eq!(Zoom::new(false, 1.5).factor(), Some(1.5));
    }

    #[test]
    fn reads_the_documented_format() {
        let text = r#"{
          "version": 1,
          "multi_window": true,
          "windows": [
            { "main": true, "display": "/a/b.opi", "project": "/scratch/me/proj",
              "macros": {}, "position": [0, 0], "size": [1040, 610], "zoom": "fit" },
            { "display": "/scratch/me/proj/vacuum/c.opi",
              "macros": { "prefix": "SR" }, "position": [1050, 0], "size": [1080, 530],
              "toolbar": false }
          ]
        }"#;
        let layout: Layout = serde_json::from_str(text).unwrap();
        assert!(layout.multi_window && layout.windows[0].main && !layout.windows[1].main);
        assert_eq!(layout.windows[1].position, Some([1050.0, 0.0]));
        assert_eq!(
            layout.windows[1].location().macros.get("prefix"),
            Some("SR")
        );
    }

    #[test]
    fn restore_drops_off_screen_positions() {
        let w = |x: f32, y: f32| WindowLayout {
            main: false,
            display: "/a.bob".into(),
            project: None,
            macros: BTreeMap::new(),
            position: Some([x, y]),
            size: Some([800.0, 600.0]),
            zoom: None,
            toolbar: false,
        };
        let laptop = [Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0))];
        let docked = [
            laptop[0],
            Rect::from_min_size(Pos2::new(1920.0, 0.0), Vec2::new(2560.0, 1440.0)),
        ];
        // On the second monitor, which is gone.
        assert_eq!(
            w(2500.0, 100.0).geometry(&docked).position,
            Some(Pos2::new(2500.0, 100.0))
        );
        assert_eq!(w(2500.0, 100.0).geometry(&laptop).position, None);
        // Hanging off an edge but with its title bar reachable.
        assert!(w(1500.0, 50.0).geometry(&laptop).position.is_some());
        assert!(w(-700.0, 50.0).geometry(&laptop).position.is_some());
        // Above the top of the screen, or only a sliver showing.
        assert_eq!(w(100.0, -200.0).geometry(&laptop).position, None);
        assert_eq!(w(1910.0, 50.0).geometry(&laptop).position, None);
        // The size is kept either way, and nothing is dropped when the
        // monitors are unknown.
        assert_eq!(
            w(2500.0, 100.0).geometry(&laptop).size,
            Some(Vec2::new(800.0, 600.0))
        );
        assert!(w(9000.0, 9000.0).geometry(&[]).position.is_some());
    }

    #[test]
    fn zoom_is_a_number_or_fit() {
        let zoom = |t: &str| serde_json::from_str::<Zoom>(t);
        assert_eq!(zoom("1.5").unwrap(), Zoom::Factor(1.5));
        assert_eq!(zoom("\"fit\"").unwrap(), Zoom::Fit);
        assert!(zoom("\"fill\"").is_err());
        assert!(zoom("\"\"").is_err());
        assert_eq!(serde_json::to_string(&Zoom::Fit).unwrap(), "\"fit\"");
        assert_eq!(serde_json::to_string(&Zoom::Factor(2.0)).unwrap(), "2.0");
    }

    #[test]
    fn only_a_named_layout_must_load() {
        let layout = Layout::new(false, Vec::new());
        let good = |_: &str| Ok(layout.clone());
        let bad = |_: &str| Err::<Layout, _>("newer".to_string());
        // --layout: loaded, or an error.
        assert_eq!(
            startup(Some("rf".into()), true, good).unwrap(),
            Some((layout.clone(), "rf".to_string()))
        );
        assert!(startup(Some("rf".into()), true, bad).is_err());
        // The default: loaded, or passed over.
        assert_eq!(
            startup(None, true, good).unwrap(),
            Some((layout.clone(), DEFAULT.to_string()))
        );
        assert_eq!(startup(None, true, bad).unwrap(), None);
        assert_eq!(startup(None, false, good).unwrap(), None);
    }

    #[test]
    fn saves_replace_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("bobview-layout-{}", std::process::id()));
        let path = dir.join("a.json");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn layout_names_are_file_names() {
        assert!(valid_name("rf-shift"));
        assert!(!valid_name(""));
        assert!(!valid_name("  "));
        assert!(!valid_name("../x"));
        assert!(!valid_name(".hidden"));
    }
}
