//! The application window: navigation bar, display area and status line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bob::{Macros, Source};
use eframe::egui::{self, Color32, Id, Vec2};
use pv::PvManager;

use crate::chrome::{self, Chrome, UiScale};
use crate::pvmenu;
use crate::screen::{Node, Screen};
use crate::style::{self, View};
use crate::trend::Trend;
use crate::widgets::{self, Cx, Request};

#[derive(Clone, Debug)]
pub struct Location {
    pub source: Source,
    pub macros: Macros,
}

pub struct BobApp {
    pvs: PvManager,
    screen: Option<Screen>,
    current: Option<Location>,
    home: Option<Location>,
    back: Vec<Location>,
    forward: Vec<Location>,
    zoom: f32,
    fit: bool,
    error: Option<String>,
    show_open: bool,
    /// The display being loaded is the current one again: keep the window size.
    reloading: bool,
    open_path: String,
    /// Use the on-screen keypad for text entries (switched on by the first touch).
    keypad_mode: bool,
    keypad: Option<Keypad>,
    details: Option<pvmenu::Details>,
    /// Open trend windows, and the id the next one gets.
    trends: Vec<Trend>,
    next_trend: u64,
    /// The display at `current`, being fetched and built on a thread.
    loading: Option<Loading>,
    /// Displays named by command actions, worked out on threads.
    launched: (
        std::sync::mpsc::Sender<Launched>,
        std::sync::mpsc::Receiver<Launched>,
    ),
    /// Command actions still being worked out, so a double click runs one.
    launching: Vec<LaunchKey>,
    /// Bumped on every `show`, so a launch started on another display is dropped.
    shown: u64,
    chrome: Chrome,
    /// The window title last set, the shown display's title.
    window_title: String,
}

/// What makes two command actions the same: base, directory and command.
type LaunchKey = (Option<String>, Option<String>, String);

/// A command action's result, tagged with the display it was started on.
struct Launched {
    key: LaunchKey,
    shown: u64,
    target: Option<Location>,
}

#[cfg(not(target_arch = "wasm32"))]
type Loading = std::sync::mpsc::Receiver<Result<Screen, bob::Error>>;
/// In a browser, the display's files are fetched first and it is built
/// once they have all arrived.
#[cfg(target_arch = "wasm32")]
struct Loading;

struct Keypad {
    pv: pv::Pv,
    text: String,
}

impl BobApp {
    pub fn new(
        ctx: &egui::Context,
        start: Option<Location>,
        zoom: Option<f32>,
        ui_scale: UiScale,
    ) -> Self {
        style::install_fonts(ctx);
        ctx.set_theme(egui::Theme::Light);
        // The controls' zoom is ours (see `chrome`), not ctrl+plus/minus.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let pvs = PvManager::new();
        // Coalesce PV updates into at most ~10 repaints a second.
        let ctx = ctx.clone();
        pvs.set_notifier(std::sync::Arc::new(move || {
            ctx.request_repaint_after(Duration::from_millis(100))
        }));

        let mut app = Self {
            pvs,
            screen: None,
            current: None,
            home: start.clone(),
            back: Vec::new(),
            forward: Vec::new(),
            zoom: zoom.unwrap_or(1.0),
            fit: zoom.is_none(),
            error: None,
            show_open: start.is_none(),
            reloading: false,
            keypad_mode: false,
            keypad: None,
            details: None,
            trends: Vec::new(),
            next_trend: 0,
            loading: None,
            launched: std::sync::mpsc::channel(),
            launching: Vec::new(),
            shown: 0,
            chrome: Chrome::new(ui_scale),
            window_title: String::new(),
            open_path: start
                .as_ref()
                .map(|l| l.source.to_string())
                .unwrap_or_default(),
        };
        if let Some(loc) = start {
            app.show(loc);
        }
        app
    }

    pub fn set_keypad(&mut self, on: bool) {
        self.keypad_mode = on;
    }

    /// Load and show a display, without touching the history.
    fn show(&mut self, loc: Location) {
        // Drop the old screen first so its PVs disconnect before the new ones start.
        self.screen = None;
        self.shown += 1;
        self.open_path = loc.source.to_string();
        #[cfg(target_arch = "wasm32")]
        set_page_url(&loc);
        self.loading = None;
        // In a browser `poll_loading` fetches and builds every display.
        #[cfg(target_arch = "wasm32")]
        let fetch_first = true;
        #[cfg(not(target_arch = "wasm32"))]
        let fetch_first = loc.source.is_url();
        if fetch_first {
            self.loading = Some(self.start_loading(&loc));
            self.error = None;
            self.current = Some(loc);
            return;
        }
        let loaded = Screen::load(&loc.source, &loc.macros, &self.pvs);
        self.current = Some(loc);
        self.loaded(loaded);
    }

    /// Build a fetched display on a thread, embedded displays and all, so a
    /// slow server doesn't freeze the window; `poll_loading` shows it once
    /// it arrives.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_loading(&self, loc: &Location) -> Loading {
        let (tx, rx) = std::sync::mpsc::channel();
        let (source, macros, pvs) = (loc.source.clone(), loc.macros.clone(), self.pvs.clone());
        std::thread::spawn(move || {
            tx.send(Screen::load(&source, &macros, &pvs)).ok();
        });
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
                #[cfg(target_arch = "wasm32")]
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    doc.set_title(&screen.title);
                }
                self.screen = Some(screen);
                self.error = None;
                // A reload keeps the size the user gave the window.
                if !self.reloading {
                    self.chrome.display_changed();
                }
            }
            Err(e) => {
                log::error!("{e}");
                self.error = Some(e.to_string());
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
    fn poll_loading(&mut self, ctx: &egui::Context) {
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
    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Ok(loaded) => {
                self.loading = None;
                self.loaded(loaded);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.loading = None,
        }
    }

    /// Run a command action on a thread, so a slow launcher script or file
    /// system doesn't freeze the window, and open the display it names. A
    /// command already in flight is ignored.
    #[cfg(not(target_arch = "wasm32"))]
    fn launch(
        &mut self,
        ctx: &egui::Context,
        base: Option<Source>,
        dir: Option<Source>,
        cmd: String,
    ) {
        let key = (
            base.as_ref().map(|s| s.to_string()),
            dir.as_ref().map(|s| s.to_string()),
            cmd.clone(),
        );
        if self.launching.contains(&key) {
            return;
        }
        self.launching.push(key.clone());
        let (tx, ctx, shown) = (self.launched.0.clone(), ctx.clone(), self.shown);
        std::thread::spawn(move || {
            let target = match crate::launch::command_target(base.as_ref(), dir, &cmd) {
                Some((file, macros)) => {
                    let source = Source::File(file);
                    log::info!("open {source} (window) from '{cmd}'");
                    Some(Location { source, macros })
                }
                None => {
                    log::warn!("command '{cmd}' is not supported");
                    None
                }
            };
            tx.send(Launched { key, shown, target }).ok();
            ctx.request_repaint();
        });
    }

    /// A browser runs no commands.
    #[cfg(target_arch = "wasm32")]
    fn launch(&mut self, _: &egui::Context, _: Option<Source>, _: Option<Source>, cmd: String) {
        log::warn!("command '{cmd}' is not run in a browser");
    }

    /// Open the displays named by finished command actions, unless the user
    /// has since moved to another display.
    fn poll_launched(&mut self) {
        while let Ok(done) = self.launched.1.try_recv() {
            self.launching.retain(|k| *k != done.key);
            match done.target {
                Some(loc) if done.shown == self.shown => self.open(loc),
                Some(loc) => log::info!("drop {}: the display has changed", loc.source),
                None => {}
            }
        }
    }

    /// Navigate to a display, recording where we came from.
    pub fn open(&mut self, loc: Location) {
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        self.forward.clear();
        self.show(loc);
    }

    fn go_back(&mut self) {
        if let Some(loc) = self.back.pop() {
            if let Some(cur) = self.current.take() {
                self.forward.push(cur);
            }
            self.show(loc);
        }
    }

    fn go_forward(&mut self) {
        if let Some(loc) = self.forward.pop() {
            if let Some(cur) = self.current.take() {
                self.back.push(cur);
            }
            self.show(loc);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        // One row whatever the width, so a window sized to the display shows it
        // near 100%; the display name is elided when there is no room. A narrow
        // window (a phone) keeps the navigation and Fit, and moves the rest
        // into a menu.
        let compact = ui.available_width() < chrome::FULL_TOOLBAR_WIDTH;
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            let back = ui
                .add_enabled(!self.back.is_empty(), egui::Button::new("◀"))
                .on_hover_text("Back");
            self.chrome.button = back.rect.height();
            if back.clicked() {
                self.go_back();
            }
            if ui
                .add_enabled(!self.forward.is_empty(), egui::Button::new("▶"))
                .on_hover_text("Forward")
                .clicked()
            {
                self.go_forward();
            }
            if ui
                .add_enabled(self.home.is_some(), egui::Button::new("⌂"))
                .on_hover_text("Home")
                .clicked()
                && let Some(home) = self.home.clone()
            {
                self.open(home);
            }
            if compact {
                ui.separator();
                if ui.selectable_label(self.fit, "Fit").clicked() {
                    self.fit = !self.fit;
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                ui.menu_button("☰", |ui| {
                    if ui.button("⟳ Reload").clicked() {
                        self.reload();
                    }
                    if ui.button("+ Zoom in").clicked() {
                        self.zoom_by(1.25);
                    }
                    if ui.button("− Zoom out").clicked() {
                        self.zoom_by(1.0 / 1.25);
                    }
                    if ui
                        .selectable_label(self.chrome.auto_size, "🗖 Auto-size window")
                        .clicked()
                    {
                        self.toggle_auto_size();
                    }
                    if ui.selectable_label(self.keypad_mode, "⌨ Keypad").clicked() {
                        self.keypad_mode = !self.keypad_mode;
                    }
                    if ui.selectable_label(self.show_open, "Open…").clicked() {
                        self.show_open = !self.show_open;
                    }
                });
            } else {
                if ui.button("⟳").on_hover_text("Reload").clicked() {
                    self.reload();
                }
                ui.separator();
                if ui.button("−").clicked() {
                    self.zoom_by(1.0 / 1.25);
                }
                if ui.selectable_label(self.fit, "Fit").clicked() {
                    self.fit = !self.fit;
                }
                if ui.button("+").clicked() {
                    self.zoom_by(1.25);
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui
                    .selectable_label(self.chrome.auto_size, "🗖")
                    .on_hover_text("Auto-size: resize the window to fit each display opened")
                    .clicked()
                {
                    self.toggle_auto_size();
                }
                ui.separator();
                if ui
                    .selectable_label(self.keypad_mode, "⌨")
                    .on_hover_text("Edit values with an on-screen keypad")
                    .clicked()
                {
                    self.keypad_mode = !self.keypad_mode;
                }
                if ui.selectable_label(self.show_open, "Open…").clicked() {
                    self.show_open = !self.show_open;
                }
            }
            if let Some(s) = &self.screen {
                ui.add(egui::Label::new(egui::RichText::new(&s.title).strong()).truncate())
                    .on_hover_text(&s.title);
            }
        });
    }

    /// Turn Auto-size on or off; turning it on sizes the window to the
    /// current display straight away.
    fn toggle_auto_size(&mut self) {
        self.chrome.auto_size = !self.chrome.auto_size;
        if self.chrome.auto_size {
            self.chrome.display_changed();
        }
    }

    fn reload(&mut self) {
        if let Some(cur) = self.current.clone() {
            crate::fetch::forget();
            crate::symbol::forget();
            self.reloading = true;
            self.show(cur);
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        self.fit = false;
        self.zoom = (self.zoom * factor).clamp(0.2, 5.0);
    }

    fn open_panel(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.label("File or URL:");
            let edit = ui
                .add(egui::TextEdit::singleline(&mut self.open_path).desired_width(f32::INFINITY));
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                chosen = Some(Source::parse(&self.open_path));
            }
        });
        // Nothing to list for a URL (an empty path lists nothing).
        let dir = match Source::parse(&self.open_path) {
            Source::Url(_) => PathBuf::new(),
            Source::File(path) if bob::project::locate(&path).is_dir() => path,
            Source::File(path) => path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        };
        egui::ScrollArea::vertical()
            // Leave most of a short (landscape phone) window to the display.
            .max_height(200f32.min(ui.ctx().content_rect().height() * 0.35))
            .show(ui, |ui| {
                if let Some(parent) = dir.parent()
                    && ui.button("📁 ..").clicked()
                {
                    self.open_path = format!("{}/", parent.display());
                }
                for (entry, is_dir) in list_dir(&dir) {
                    let name = entry
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if is_dir {
                        if ui.button(format!("📁 {name}")).clicked() {
                            self.open_path = format!("{}/", entry.display());
                        }
                    } else if ui.button(format!("🗋 {name}")).clicked() {
                        chosen = Some(Source::File(entry));
                    }
                }
            });
        if let Some(source) = chosen {
            self.show_open = false;
            if self.home.is_none() {
                self.home = Some(Location {
                    source: source.clone(),
                    macros: Macros::default(),
                });
            }
            self.open(Location {
                source,
                macros: Macros::default(),
            });
        }
    }

    fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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
                if let Some(cur) = &self.current {
                    ui.weak(cur.source.to_string());
                }
            }
        });
    }
}

/// Display files (`.bob`, `.opi`) and subdirectories of `dir`, directories
/// first. The linked folders of an Eclipse project in `dir` are listed as
/// directories, and those are listed as if they were where they are linked.
fn list_dir(dir: &Path) -> Vec<(PathBuf, bool)> {
    let real = bob::project::locate(dir);
    let mut out: Vec<(PathBuf, bool)> = std::fs::read_dir(&real)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| (dir.join(e.file_name()), e.path().is_dir()))
                .filter(|(p, d)| {
                    let hidden = p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                    let display = p.extension().is_some_and(|x| x == "bob" || x == "opi");
                    !hidden && (*d || display)
                })
                .collect()
        })
        .unwrap_or_default();
    for (name, _) in bob::project::links(&real).unwrap_or_default() {
        let link = dir.join(name);
        if !out.iter().any(|(p, _)| *p == link) {
            out.push((link, true));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
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

impl eframe::App for BobApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl BobApp {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.poll_loading(ui.ctx());
        self.poll_launched();
        if !self.keypad_mode && ui.input(|i| i.any_touches()) {
            self.keypad_mode = true;
        }
        widgets::touch_targets(ui.ctx(), self.keypad_mode);
        self.chrome.begin(ui.ctx());
        if let Some(s) = &self.screen
            && s.title != self.window_title
        {
            self.window_title = s.title.clone();
            // A browser page's title is set when the display loads instead.
            #[cfg(not(target_arch = "wasm32"))]
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(self.window_title.clone()));
        }
        let toolbar = egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        self.chrome.toolbar = toolbar.response.rect.height();
        let mut panels = self.chrome.toolbar;
        if self.show_open {
            panels += egui::Panel::top("open")
                .show(ui, |ui| self.open_panel(ui))
                .response
                .rect
                .height();
        }
        panels += egui::Panel::bottom("status")
            .show(ui, |ui| self.status(ui))
            .response
            .rect
            .height();
        self.chrome.panels = panels;
        // The display's zoom is relative to the desktop's scale, not the controls'.
        let ui_zoom = ui.ctx().zoom_factor();

        let mut requests = Vec::new();
        let trends: Vec<(u64, String)> = self.trends.iter().map(|t| (t.id, t.title())).collect();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_gray(200)))
            .show(ui, |ui| {
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
                        view: View {
                            origin: rect.min,
                            zoom,
                        },
                        clip: ui.clip_rect().intersect(rect),
                        pvs: &self.pvs,
                        requests: &mut requests,
                        depth: 0,
                        keypad: self.keypad_mode,
                        trends: &trends,
                    };
                    widgets::draw_screen(ui, screen, &mut cx, Id::new("display"));
                });
            });

        // A new display sizes the window to show it at 100% (or the fixed zoom).
        let shown = if self.fit { 1.0 } else { self.zoom } / ui_zoom;
        let display = self
            .screen
            .as_ref()
            .map(|s| Vec2::new(s.width as f32, s.height as f32) * shown);
        self.chrome.end(ui.ctx(), display);

        for req in requests {
            match req {
                Request::Open {
                    source,
                    macros,
                    target,
                } => {
                    log::info!("open {source} ({target})");
                    self.open(Location { source, macros });
                }
                Request::Keypad { pv, text } => self.keypad = Some(Keypad { pv, text }),
                Request::Details(details) => self.details = Some(details),
                Request::Trend(names) => {
                    log::info!("trend {}", names.join(", "));
                    self.trends
                        .push(Trend::new(self.next_trend, &names, &self.pvs));
                    self.next_trend += 1;
                }
                Request::TrendAdd(id, names) => {
                    if let Some(trend) = self.trends.iter_mut().find(|t| t.id == id) {
                        log::info!("add {} to trend {id}", names.join(", "));
                        trend.extend(&names, &self.pvs);
                        raise_trend(ui.ctx(), id);
                    }
                }
                Request::Command {
                    base,
                    directory,
                    command,
                } => self.launch(ui.ctx(), base, directory, command),
                Request::Close if !self.back.is_empty() => self.go_back(),
                Request::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                Request::Webpage(url) => open_webpage(&url),
            }
        }
        self.keypad_window(ui);
        self.details_window(ui);
        self.trend_windows(ui);
    }

    /// Each trend in its own native window, until it is closed, or in a
    /// window inside this one where there's only one native window (the
    /// browser, snapshots).
    fn trend_windows(&mut self, ui: &mut egui::Ui) {
        let pvs = &self.pvs;
        let ctx = ui.ctx();
        if ctx.embed_viewports() {
            let screen = ctx.content_rect();
            self.trends.retain_mut(|trend| {
                let mut open = true;
                // Each new window a little below and right of the one before.
                let step = (trend.id % 8) as f32 * 24.0;
                let size = Vec2::new(640.0, 420.0).min(screen.size() * 0.9);
                egui::Window::new(trend.title())
                    .id(trend_window_id(trend.id))
                    .open(&mut open)
                    .collapsible(false)
                    .default_pos(screen.left_top() + Vec2::new(40.0, 60.0) + Vec2::splat(step))
                    .default_size(size)
                    .show(ctx, |ui| trend.ui(ui, pvs));
                open
            });
            return;
        }
        self.trends.retain_mut(|trend| {
            // The window manager places it: Wayland won't let us.
            let builder = egui::ViewportBuilder::default()
                .with_title(trend.title())
                .with_app_id("bobview")
                .with_inner_size([640.0, 420.0])
                .with_min_inner_size([320.0, 200.0]);
            ctx.show_viewport_immediate(trend_viewport(trend.id), builder, |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    // No native windows after all: a window in this one,
                    // which can't be closed.
                    trend.ui(ui, pvs);
                    return true;
                }
                egui::CentralPanel::default().show(ui, |ui| trend.ui(ui, pvs));
                // Closing the window drops the trend, and its PVs with it.
                !ui.input(|i| i.viewport().close_requested())
            })
        });
    }

    fn details_window(&mut self, ui: &mut egui::Ui) {
        let mut open = self.details.is_some();
        if let Some(details) = &self.details {
            egui::Window::new("PV details")
                .open(&mut open)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ui.ctx(), |ui| pvmenu::details_ui(ui, details));
        }
        if !open {
            self.details = None;
        }
    }

    fn keypad_window(&mut self, ui: &mut egui::Ui) {
        let Some(pad) = self.keypad.as_mut() else {
            return;
        };
        let mut done = None;
        let title = plain_name(pad.pv.name());
        let state = pad.pv.state();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -8.0))
            .show(ui.ctx(), |ui| {
                let size = Vec2::new(64.0, 48.0);
                let font = egui::FontId::proportional(22.0);
                ui.add(
                    egui::TextEdit::singleline(&mut pad.text)
                        .font(font.clone())
                        .desired_width(4.0 * size.x + 12.0),
                );
                // Enums get one button per state instead of digits.
                if !state.labels.is_empty() {
                    for label in &state.labels {
                        let b = egui::Button::new(egui::RichText::new(label).font(font.clone()));
                        if ui
                            .add_sized(Vec2::new(4.0 * size.x + 12.0, size.y), b)
                            .clicked()
                        {
                            done = Some(Some(label.clone()));
                        }
                    }
                } else {
                    let rows = [
                        ["7", "8", "9", "Del"],
                        ["4", "5", "6", "-"],
                        ["1", "2", "3", "e"],
                        ["0", ".", "C", "OK"],
                    ];
                    for row in rows {
                        ui.horizontal(|ui| {
                            for key in row {
                                let b =
                                    egui::Button::new(egui::RichText::new(key).font(font.clone()));
                                if ui.add_sized(size, b).clicked() {
                                    match key {
                                        "Del" => {
                                            pad.text.pop();
                                        }
                                        "C" => pad.text.clear(),
                                        "OK" => done = Some(Some(pad.text.clone())),
                                        k => pad.text.push_str(k),
                                    }
                                }
                            }
                        });
                    }
                }
                if ui.button("Cancel").clicked() {
                    done = Some(None);
                }
            });
        match done {
            Some(Some(value)) => {
                pad.pv.write(value);
                self.keypad = None;
            }
            Some(None) => self.keypad = None,
            None => {}
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open_webpage(url: &str) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
        log::warn!("cannot open {url}: {e}");
    }
}

#[cfg(target_arch = "wasm32")]
fn open_webpage(url: &str) {
    if let Some(w) = web_sys::window()
        && w.open_with_url_and_target(url, "_blank").is_err()
    {
        log::warn!("cannot open {url}");
    }
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

/// The native window of the trend `id`.
fn trend_viewport(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("trend", id))
}

/// The trend `id`'s window inside the main one, where windows are embedded.
fn trend_window_id(id: u64) -> Id {
    Id::new(("trend", id))
}

/// Bring the trend `id`'s window to the front, as a PV is added to it.
fn raise_trend(ctx: &egui::Context, id: u64) {
    if ctx.embed_viewports() {
        ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, trend_window_id(id)));
    } else {
        ctx.send_viewport_cmd_to(trend_viewport(id), egui::ViewportCommand::Focus);
    }
}

/// A PV's name as a user would type it: no `ca://` prefix, and no type or
/// initial value for a local PV (`loc://x`, not `loc://x<VDouble>(1.5)`).
pub(crate) fn plain_name(name: &pv::PvName) -> String {
    match name.scheme {
        pv::Scheme::Ca => name.base().to_string(),
        _ => {
            let full = name.to_string();
            let prefix = &full[..full.len() - name.pv.len()];
            format!("{prefix}{}", name.base())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plain_name;
    use pv::PvName;

    #[test]
    fn lists_displays_and_project_links() {
        let tmp = std::env::temp_dir().join(format!("bobview-list-{}", std::process::id()));
        let (real, proj) = (tmp.join("real"), tmp.join("proj"));
        std::fs::create_dir_all(real.join("sub")).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        for f in ["a.opi", "b.bob", "c.txt"] {
            std::fs::write(real.join(f), "").unwrap();
        }
        std::fs::write(
            proj.join(".project"),
            format!(
                "<projectDescription><linkedResources><link><name>m</name><location>{}</location></link></linkedResources></projectDescription>",
                real.display()
            ),
        )
        .unwrap();
        let names = |dir: &std::path::Path| -> Vec<(String, bool)> {
            super::list_dir(dir)
                .into_iter()
                .map(|(p, d)| (p.strip_prefix(&tmp).unwrap().display().to_string(), d))
                .collect()
        };
        assert_eq!(names(&proj), [("proj/m".to_string(), true)]);
        assert_eq!(
            names(&proj.join("m")),
            [
                ("proj/m/sub".to_string(), true),
                ("proj/m/a.opi".to_string(), false),
                ("proj/m/b.bob".to_string(), false)
            ]
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn launches_once_and_drops_stale_results() {
        use super::{BobApp, Launched, Location};
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        // A double click runs the command once, and counts as loading.
        app.launch(&ctx, None, None, "no-such-launcher".into());
        app.launch(&ctx, None, None, "no-such-launcher".into());
        assert_eq!(app.launching.len(), 1);
        assert!(app.is_loading());
        while app.is_loading() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.poll_launched();
        }
        // A result from before the display changed is dropped.
        let loc = Location {
            source: bob::Source::File("/no/such.bob".into()),
            macros: Default::default(),
        };
        let key = (None, None, "x".to_string());
        app.launching.push(key.clone());
        let stale = app.shown;
        app.shown += 1;
        app.launched
            .0
            .send(Launched {
                key,
                shown: stale,
                target: Some(loc),
            })
            .unwrap();
        app.poll_launched();
        assert!(app.current.is_none() && !app.is_loading());
    }

    #[test]
    fn keypad_title_is_the_plain_name() {
        let t = |n: &str| plain_name(&PvName::parse(n));
        assert_eq!(t("loc://t1<VDouble>(1.5)"), "loc://t1");
        assert_eq!(t("ca://BL01T:X"), "BL01T:X");
        assert_eq!(t("BL01T:X"), "BL01T:X");
        assert_eq!(t("pva://BL01T:Y"), "pva://BL01T:Y");
    }
}
