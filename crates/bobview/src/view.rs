//! One display and its navigation: what a window shows.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, Color32, Id, Vec2};
use pv::PvManager;

use crate::app::Location;
use crate::screen::{Compat, Node, Screen};
use crate::style;
use crate::widgets::{self, Cx, Request};

/// The window a view is drawn in, and so the window a widget's request acts
/// on (Back, Reload and Close act on their own window).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowKey {
    /// The main window, with the toolbar.
    #[default]
    Main,
    /// A display window of its own.
    Child(egui::ViewportId),
}

/// Where a request came from: its window, and whether ctrl or shift was
/// held as it was made (a link then navigates in place, or opens a window
/// of its own).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    pub window: WindowKey,
    pub ctrl: bool,
    pub shift: bool,
}

/// What drawing a view gives back.
pub struct Drawn {
    /// The status line's height.
    pub status: f32,
    pub requests: Vec<(Origin, Request)>,
    /// The display area behind the widgets, in a display window, for its
    /// context menu.
    pub background: Option<egui::Response>,
}

/// What a view needs from the application to draw: owned or shared, so a
/// view can be drawn without the rest of the application.
pub struct ViewCx<'a> {
    pub window: WindowKey,
    /// Touch mode: finger-sized rows in drop-down lists.
    pub touch: bool,
    /// The open trend windows, (id, title), that a PV's context menu can
    /// add it to.
    pub trends: &'a [(u64, String)],
}

/// How long a PV write notice stays in the status line.
const NOTICE_SECONDS: f64 = 8.0;

/// A display shown in a window, with its history and zoom.
pub struct View {
    pvs: PvManager,
    pub screen: Option<Screen>,
    pub current: Option<Location>,
    pub back: Vec<Location>,
    pub forward: Vec<Location>,
    pub zoom: f32,
    pub fit: bool,
    error: Option<String>,
    /// The display being loaded is the current one again: keep the window size.
    reloading: bool,
    /// A new display (not a reload) was shown, for the window to size itself
    /// to; also set when one failed to load, so a window stops keeping the
    /// size a layout gave it.
    pub new_display: bool,
    /// The path in the Open panel, following the display shown.
    pub open_path: String,
    /// The display at `current`, being fetched and built on a thread.
    loading: Option<Loading>,
    /// The latest write notice seen (see [`PvManager::notice`]) and the
    /// time, in egui seconds, it was first shown, to hide it after a while.
    notice_seen: (u64, f64),
    /// Command actions still being worked out, so a double click runs one.
    pub launching: Vec<LaunchKey>,
    /// Bumped on every `show`, so a launch started on another display is dropped.
    pub shown: u64,
    /// The window title last set, the shown display's title.
    window_title: String,
}

#[cfg(not(target_arch = "wasm32"))]
type Loading = Receiver<Result<Screen, bob::Error>>;
/// In a browser, the display's files are fetched first and it is built
/// once they have all arrived.
#[cfg(target_arch = "wasm32")]
struct Loading;

/// A display window's title: the display's, after a pin when the window
/// is pinned.
pub fn window_title(title: &str, pinned: bool) -> String {
    if pinned {
        format!("📌 {title}")
    } else {
        title.to_string()
    }
}

/// What makes two command actions the same: base, directory and command.
pub type LaunchKey = (Option<String>, Option<String>, String);

impl View {
    /// An empty view: `zoom` fixed, or fit to the window when None.
    pub fn new(pvs: &PvManager, zoom: Option<f32>) -> Self {
        Self {
            pvs: pvs.clone(),
            screen: None,
            current: None,
            back: Vec::new(),
            forward: Vec::new(),
            zoom: zoom.unwrap_or(1.0),
            fit: zoom.is_none(),
            error: None,
            reloading: false,
            new_display: false,
            open_path: String::new(),
            loading: None,
            launching: Vec::new(),
            notice_seen: (0, 0.0),
            shown: 0,
            window_title: String::new(),
        }
    }

    /// Load and show a display, without touching the history.
    pub fn show(&mut self, loc: Location) {
        // Drop the old screen first so its PVs disconnect before the new ones start.
        self.screen = None;
        self.shown += 1;
        self.open_path = loc.source.to_string();
        #[cfg(target_arch = "wasm32")]
        set_page_url(&loc);
        // In a browser `poll_loading` fetches and builds every display.
        self.loading = Some(self.start_loading(&loc));
        self.error = None;
        self.current = Some(loc);
        // A local or already fetched display is usually built at once: show
        // it without a frame of "Loading".
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.loading
            && let Ok(loaded) = rx.recv_timeout(crate::screen::QUICK)
        {
            self.loading = None;
            self.loaded(loaded);
        }
    }

    /// Build the display on a thread, embedded displays and all, so a slow
    /// server or file system doesn't freeze the window; `poll_loading`
    /// shows it once it arrives.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_loading(&self, loc: &Location) -> Loading {
        let (tx, rx) = std::sync::mpsc::channel();
        let (source, macros, pvs) = (loc.source.clone(), loc.macros.clone(), self.pvs.clone());
        // An explicit stack: building a display recurses once per nesting
        // level (capped by `bob`), and the default is platform dependent.
        let spawned = std::thread::Builder::new()
            .name("display-loader".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                tx.send(Screen::load(&source, &macros, &pvs)).ok();
            });
        if let Err(e) = spawned {
            log::error!("cannot start the display loader: {e}");
        }
        rx
    }

    #[cfg(target_arch = "wasm32")]
    fn start_loading(&self, _loc: &Location) -> Loading {
        Loading
    }

    /// Show a loaded display, or why it could not be loaded.
    fn loaded(&mut self, loaded: Result<Screen, bob::Error>) {
        match loaded {
            Ok(screen) => {
                self.screen = Some(screen);
                self.error = None;
                // A reload keeps the size the user gave the window.
                if !self.reloading {
                    self.new_display = true;
                }
            }
            Err(e) => {
                log::error!("{e}");
                self.error = Some(e.to_string());
                if !self.reloading {
                    self.new_display = true;
                }
            }
        }
        self.reloading = false;
    }

    /// Whether a display, or one named by a command action, is still being
    /// worked out.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_loading(&self) -> bool {
        self.loading.is_some() || !self.launching.is_empty()
    }

    /// Build the display once its files have arrived, fetching more if
    /// building it finds it names others.
    #[cfg(target_arch = "wasm32")]
    pub fn poll_loading(&mut self, ctx: &egui::Context) {
        if self.loading.is_none() {
            return;
        }
        if !crate::fetch::pending()
            && let Some(cur) = &self.current
        {
            let loaded = Screen::load(&cur.source, &cur.macros, &self.pvs);
            if !crate::fetch::pending() {
                self.loading = None;
                self.loaded(loaded);
                return;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// Show the display being fetched, or its error, once the fetch is done.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn poll_loading(&mut self, ctx: &egui::Context) {
        // Embedded displays built on threads, including ones not drawn.
        if let Some(screen) = &mut self.screen
            && crate::screen::poll_built(&mut screen.nodes)
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Ok(loaded) => {
                self.loading = None;
                self.loaded(loaded);
            }
            Err(TryRecvError::Disconnected) => {
                // The thread panicked building it: say so, not a blank view.
                self.loading = None;
                if let Some(cur) = &self.current {
                    let e = crate::screen::build_failed(&cur.source);
                    self.loaded(Err(e));
                }
            }
        }
    }

    /// Whether this shows `loc`: the same display with the same macros, its
    /// own defaults under both, as a link on it has them (while it loads,
    /// as it was opened). Macros from a link's group or widget still count.
    pub fn shows(&self, loc: &Location) -> bool {
        let Some(cur) = &self.current else {
            return false;
        };
        let Some(screen) = &self.screen else {
            return cur.same_display(loc);
        };
        let defaults = |l: &Location| Location {
            source: l.source.clone(),
            macros: screen.display.macros.with(&l.macros),
        };
        defaults(cur).same_display(&defaults(loc))
    }

    /// Navigate to a display, recording where we came from.
    pub fn open(&mut self, loc: Location) {
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        self.forward.clear();
        self.show(loc);
    }

    pub fn go_back(&mut self) {
        if let Some(loc) = self.back.pop() {
            if let Some(cur) = self.current.take() {
                self.forward.push(cur);
            }
            self.show(loc);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(loc) = self.forward.pop() {
            if let Some(cur) = self.current.take() {
                self.back.push(cur);
            }
            self.show(loc);
        }
    }

    pub fn reload(&mut self) {
        if let Some(cur) = self.current.clone() {
            crate::fetch::forget();
            crate::symbol::forget();
            bob::project::forget();
            self.reloading = true;
            self.show(cur);
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.fit = false;
        self.zoom = (self.zoom * factor).clamp(0.2, 5.0);
    }

    /// Title the window this view is drawn in (or the browser page) after
    /// its display, marked when the window is `pinned`.
    pub fn update_title(&mut self, ctx: &egui::Context, pinned: bool) {
        let Some(title) = self.screen.as_ref().map(|s| window_title(&s.title, pinned)) else {
            return;
        };
        if title != self.window_title {
            self.window_title = title;
            #[cfg(not(target_arch = "wasm32"))]
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title.clone()));
            #[cfg(target_arch = "wasm32")]
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let _ = ctx;
                doc.set_title(&self.window_title);
            }
        }
    }

    /// The size in points of the display at 100% (or the fixed zoom), for
    /// sizing the window to it.
    pub fn natural_size(&self, ui_zoom: f32) -> Option<Vec2> {
        let shown = if self.fit { 1.0 } else { self.zoom } / ui_zoom;
        self.screen
            .as_ref()
            .map(|s| Vec2::new(s.width as f32, s.height as f32) * shown)
    }

    /// Draw the status line and the display, and return the status line's
    /// height and the requests its widgets made, tagged with where they came
    /// from.
    pub fn draw(&mut self, ui: &mut egui::Ui, vcx: &ViewCx) -> Drawn {
        // Display windows drawn inside the main one (no native windows) must
        // not share its ids.
        let id = |name: &str| match vcx.window {
            WindowKey::Main => Id::new(name),
            WindowKey::Child(v) => Id::new((name, v)),
        };
        let status = egui::Panel::bottom(id("status"))
            .show(ui, |ui| self.status(ui))
            .response
            .rect
            .height();
        // The display's zoom is relative to the desktop's scale, not the controls'.
        let ui_zoom = ui.ctx().zoom_factor();

        let mut requests = Vec::new();
        let mut background = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_gray(200)))
            .show(ui, |ui| {
                // Sensed before the widgets, so that they win a click on them.
                if let WindowKey::Child(_) = vcx.window {
                    let rect = ui.max_rect();
                    background = Some(ui.interact(rect, id("background"), egui::Sense::click()));
                }
                let Some(screen) = self.screen.as_mut() else {
                    return;
                };
                let avail = ui.available_size();
                if self.fit && screen.width > 0.0 {
                    let fit = (avail.x / screen.width as f32)
                        .min(avail.y / screen.height.max(1.0) as f32);
                    self.zoom = (fit * ui_zoom).clamp(0.2, 5.0);
                }
                // Pinch to zoom on a touch screen (or ctrl+scroll).
                let pinch = ui.input(|i| i.zoom_delta());
                if pinch != 1.0 {
                    self.fit = false;
                    self.zoom = (self.zoom * pinch).clamp(0.2, 5.0);
                }
                let zoom = self.zoom / ui_zoom;
                let size = Vec2::new(screen.width as f32, screen.height as f32) * zoom;
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 0.0, style::color(&screen.display.background));
                    let mut cx = Cx {
                        view: style::View {
                            origin: rect.min,
                            zoom,
                        },
                        clip: ui.clip_rect().intersect(rect),
                        pvs: &self.pvs,
                        requests: &mut requests,
                        depth: 0,
                        touch: vcx.touch,
                        trends: vcx.trends,
                        window: vcx.window,
                    };
                    widgets::draw_screen(ui, screen, &mut cx, id("display"));
                });
            });
        let origin = Origin {
            window: vcx.window,
            ctrl: ui.input(|i| i.modifiers.command),
            shift: ui.input(|i| i.modifiers.shift),
        };
        Drawn {
            status,
            requests: requests.into_iter().map(|r| (origin, r)).collect(),
            background,
        }
    }

    /// The PV write notice to show now, if one is recent: shown for
    /// [`NOTICE_SECONDS`] from the frame it first appears in.
    fn notice(&mut self, ui: &egui::Ui) -> Option<String> {
        let (n, text) = self.pvs.notice();
        let now = ui.input(|i| i.time);
        if n != self.notice_seen.0 {
            self.notice_seen = (n, now);
        }
        let left = NOTICE_SECONDS - (now - self.notice_seen.1);
        if n == 0 || left <= 0.0 {
            return None;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left));
        Some(text)
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        let notice = self.notice(ui);
        ui.horizontal(|ui| {
            if let Some(text) = notice {
                ui.colored_label(Color32::from_rgb(0xb0, 0x30, 0x00), text);
            }
            if let Some(e) = &self.error {
                ui.colored_label(Color32::RED, e);
            } else if self.loading.is_some()
                && let Some(cur) = &self.current
            {
                ui.spinner();
                ui.label(format!("Loading {}", cur.source));
            } else if let Some(s) = &self.screen {
                let (connected, total) = count_pvs(&s.nodes);
                ui.label(format!("PVs {connected}/{total} connected"));
                if let Some(text) = Compat::of(&s.nodes).text() {
                    ui.weak(format!("| {text}"))
                        .on_hover_text("Scripts are not run, and widget types bobview does not draw show as placeholders");
                }
                if let Some(cur) = &self.current {
                    ui.weak(cur.source.to_string());
                }
            }
        });
    }
}

/// How many of the distinct PVs that widgets, plots and rules use are
/// connected, and how many there are.
fn count_pvs(nodes: &[Node]) -> (usize, usize) {
    fn walk(nodes: &[Node], seen: &mut std::collections::HashMap<String, bool>) {
        for n in nodes {
            let traces = n.traces.iter().flat_map(|t| t.x.iter().chain(&t.y));
            let rules = n.rules.iter().flat_map(|r| r.pvs());
            for pv in n.pv.iter().chain(traces).chain(rules) {
                seen.entry(pv.name().to_string())
                    .or_insert_with(|| pv.state().connected);
            }
            walk(&n.children, seen);
            n.tabs.iter().for_each(|t| walk(t, seen));
            if let Some(s) = &n.embedded {
                walk(&s.nodes, seen);
            }
        }
    }
    let mut seen = std::collections::HashMap::new();
    walk(nodes, &mut seen);
    (seen.values().filter(|c| **c).count(), seen.len())
}

/// Keep the page's address on the display shown, so a bookmark or a reload
/// comes back to it: `display` relative to the page when it is beside it,
/// and `macros`; other parameters (`pvws`, `zoom` …) stay as they were.
#[cfg(target_arch = "wasm32")]
fn set_page_url(loc: &Location) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(url) = window
        .location()
        .href()
        .ok()
        .and_then(|href| web_sys::Url::new(&href).ok())
    else {
        return;
    };
    let path = url.pathname();
    let dir = format!("{}{}", url.origin(), &path[..=path.rfind('/').unwrap_or(0)]);
    let display = loc.source.to_string();
    let params = url.search_params();
    // The write token is read at startup; keep it out of the address bar,
    // history and bookmarks.
    params.delete("token");
    params.set("display", display.strip_prefix(&dir).unwrap_or(&display));
    // Escaped for parse_macros.
    let macros: Vec<String> = loc
        .macros
        .iter()
        .map(|(k, v)| format!("{k}={}", v.replace('\\', r"\\").replace(',', r"\,")))
        .collect();
    if macros.is_empty() {
        params.delete("macros");
    } else {
        params.set("macros", &macros.join(","));
    }
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url.href()));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_panicked_build_shows_an_error() {
        let ctx = egui::Context::default();
        let mut view = View::new(&PvManager::new(), Some(1.0));
        view.current = Some(Location {
            source: bob::Source::File("/x/a.bob".into()),
            macros: Default::default(),
        });
        // The thread's sender dropped without sending, as when it panics.
        let (_, rx) = std::sync::mpsc::channel();
        view.loading = Some(rx);
        view.poll_loading(&ctx);
        assert!(!view.is_loading() && view.screen.is_none());
        assert_eq!(
            view.error.as_deref(),
            Some("cannot read /x/a.bob: the build failed (see the log)")
        );
    }
}
