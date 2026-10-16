//! The application window: navigation bar, display area and status line.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bob::{Macros, Source};
use eframe::egui::{self, Id, Pos2, Rect, Vec2};
use pv::PvManager;

use crate::chrome::{self, Chrome, UiScale};
use crate::layout::{self, Geometry, Layout, TrendLayout, WindowLayout, Zoom};
use crate::pvmenu;
use crate::style;
use crate::trend::Trend;
use crate::view::{LaunchKey, Origin, View, ViewCx, WindowKey};
use crate::widgets::{self, Request};

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub source: Source,
    pub macros: Macros,
}

impl Location {
    /// The same display with the same effective macros, however they were
    /// layered (later definitions win).
    fn same_display(&self, other: &Location) -> bool {
        fn effective(m: &Macros) -> std::collections::BTreeMap<&str, &str> {
            m.iter().collect()
        }
        self.source == other.source && effective(&self.macros) == effective(&other.macros)
    }
}

pub struct BobApp {
    pvs: PvManager,
    /// Set by a PV update that has asked for a repaint, cleared by the
    /// frame that shows it.
    repaint_asked: Arc<AtomicBool>,
    /// The main window's display.
    main: View,
    /// Where the main window is, for saving a layout.
    geometry: Geometry,
    /// The display windows other than the main one, and the number the
    /// next one's id is made from.
    windows: Vec<DisplayWindow>,
    next_window: u64,
    /// Links open their displays in windows of their own.
    multi_window: bool,
    /// The layout last loaded or saved, which Save saves to.
    layout_name: Option<String>,
    /// A layout to load once the controls' scale has settled (and the
    /// monitors are known).
    pending_layout: Option<(Layout, Option<String>)>,
    /// The monitors, in desktop points, when the window system says.
    monitors: Vec<Rect>,
    settings: layout::Settings,
    /// The Save as… dialog, when open.
    save_as: Option<SaveAs>,
    home: Option<Location>,
    show_open: bool,
    /// Use the on-screen keypad for text entries (switched on by the first touch).
    keypad_mode: bool,
    keypad: Option<Keypad>,
    /// The PV details dialog, and the window it was asked for in.
    details: Option<(WindowKey, pvmenu::Details)>,
    /// A write from a display from the web, waiting for the user's say-so.
    confirm: Option<Confirm>,
    /// The saved layouts, for the toolbar's menus.
    layouts: LayoutList,
    /// Open trend windows, and the id the next one gets.
    trends: Vec<Trend>,
    next_trend: u64,
    /// Displays named by command actions, worked out on threads.
    launched: (
        std::sync::mpsc::Sender<Launched>,
        std::sync::mpsc::Receiver<Launched>,
    ),
    chrome: Chrome,
}

/// A command action's result, tagged with where it was started (its window
/// and whether ctrl was held) and the display it was started on.
struct Launched {
    key: LaunchKey,
    origin: Origin,
    shown: u64,
    target: Option<Location>,
}

struct Confirm {
    window: WindowKey,
    pv: pv::Pv,
    value: String,
}

struct Keypad {
    /// The window it was asked for in, which it is drawn in.
    window: WindowKey,
    pv: pv::Pv,
    text: String,
}

/// The saved layouts' names, read when a menu listing them opens and after
/// a save or delete, not on every frame it is open.
#[derive(Default)]
struct LayoutList {
    names: Vec<String>,
    /// The (main window's) frame they were last listed on.
    frame: Option<u64>,
}

impl LayoutList {
    fn get(&mut self, ctx: &egui::Context) -> Vec<String> {
        let frame = ctx.cumulative_frame_nr();
        if self.frame.is_none_or(|f| f + 1 < frame) {
            self.names = layout::list();
        }
        self.frame = Some(frame);
        self.names.clone()
    }

    /// Read them again next time.
    fn refresh(&mut self) {
        self.frame = None;
    }
}

/// A display in a native window of its own, opened in multi-window mode or
/// from a layout.
struct DisplayWindow {
    /// Stable for the window's life: made from a counter, never reused.
    id: egui::ViewportId,
    view: View,
    /// The size and place it is created with.
    builder: egui::ViewportBuilder,
    /// Size the window to the next display shown: not when a layout gave
    /// its size.
    fit_next: bool,
    /// Show the display toolbar (Ctrl+T, the chevron or the context menu).
    toolbar: bool,
    /// Where the window is, for saving a layout.
    geometry: Geometry,
    close: bool,
}

/// The Save as… dialog: the name typed, and whether replacing the layout
/// of that name has been asked about.
#[derive(Default)]
struct SaveAs {
    name: String,
    confirm: bool,
}

/// Where a link's display is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Route {
    /// In the window the link is in, which keeps its history.
    InPlace,
    /// Raise the window already showing it.
    Raise(WindowKey),
    /// A new window.
    NewWindow,
}

/// Where a link goes: in multi-window mode to a window of its own (raising
/// one already `showing` the same display and macros), unless ctrl was held;
/// otherwise in place, whatever its target, since most links are `replace`.
fn route(multi_window: bool, ctrl: bool, showing: Option<WindowKey>) -> Route {
    match showing {
        _ if !multi_window || ctrl => Route::InPlace,
        Some(window) => Route::Raise(window),
        None => Route::NewWindow,
    }
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
        // Coalesce PV updates into at most ~10 repaints a second, asking
        // once per frame: egui keeps a record of every request until its
        // next frame, and frames stop while the desktop is locked.
        let repaint_asked = Arc::new(AtomicBool::new(false));
        let asked = repaint_asked.clone();
        let ctx = ctx.clone();
        pvs.set_notifier(Arc::new(move || {
            if !asked.swap(true, Ordering::Relaxed) {
                ctx.request_repaint_after_for(Duration::from_millis(100), egui::ViewportId::ROOT);
            }
        }));

        let mut main = View::new(&pvs, zoom);
        main.open_path = start
            .as_ref()
            .map(|l| l.source.to_string())
            .unwrap_or_default();
        let mut app = Self {
            pvs,
            repaint_asked,
            main,
            geometry: Geometry::default(),
            windows: Vec::new(),
            next_window: 0,
            multi_window: false,
            layout_name: None,
            pending_layout: None,
            monitors: Vec::new(),
            settings: layout::Settings::load(),
            save_as: None,
            home: start.clone(),
            show_open: start.is_none(),
            keypad_mode: false,
            keypad: None,
            details: None,
            confirm: None,
            layouts: LayoutList::default(),
            trends: Vec::new(),
            next_trend: 0,
            launched: std::sync::mpsc::channel(),
            chrome: Chrome::new(ui_scale),
        };
        if let Some(loc) = start {
            app.main.show(loc);
        }
        app
    }

    /// Refuse every write to a PV (see [`pv::PvManager::set_read_only`]).
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "a browser's PVs come from the server's policy")
    )]
    pub fn set_read_only(&mut self, on: bool) {
        self.pvs.set_read_only(on);
    }

    pub fn set_keypad(&mut self, on: bool) {
        self.keypad_mode = on;
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "a browser has one window")
    )]
    pub fn set_multi_window(&mut self, on: bool) {
        self.multi_window = on;
    }

    /// Restore `layout` (saved as `name`) once the window is up.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "a browser has one window")
    )]
    pub fn restore_layout(&mut self, layout: Layout, name: Option<String>) {
        if layout.windows.iter().any(|w| w.main) {
            self.show_open = false;
        }
        self.pending_layout = Some((layout, name));
    }

    /// The view drawn in `window`, unless that window has closed.
    fn view_mut(&mut self, window: WindowKey) -> Option<&mut View> {
        match window {
            WindowKey::Main => Some(&mut self.main),
            WindowKey::Child(id) => self
                .windows
                .iter_mut()
                .find(|w| w.id == id && !w.close)
                .map(|w| &mut w.view),
        }
    }

    /// The window showing `loc` (the same display and macros), if any.
    fn showing(&self, loc: &Location) -> Option<WindowKey> {
        let same = |cur: Option<&Location>| cur.is_some_and(|c| c.same_display(loc));
        if same(self.main.current.as_ref()) {
            return Some(WindowKey::Main);
        }
        self.windows
            .iter()
            .find(|w| !w.close && same(w.view.current.as_ref()))
            .map(|w| WindowKey::Child(w.id))
    }

    /// Show `loc`, from a link in `origin`'s window: in place, or in a
    /// window of its own in multi-window mode.
    fn follow(&mut self, ctx: &egui::Context, origin: Origin, loc: Location, target: &str) {
        match route(self.multi_window, origin.ctrl, self.showing(&loc)) {
            Route::InPlace => {
                log::info!("open {} ({target})", loc.source);
                if let Some(view) = self.view_mut(origin.window) {
                    view.open(loc);
                }
            }
            Route::Raise(window) => {
                log::info!("raise {}", loc.source);
                raise(ctx, window);
            }
            Route::NewWindow => {
                log::info!("open {} (new window)", loc.source);
                self.open_window(ctx, loc, Geometry::default(), false);
            }
        }
    }

    /// Open `loc` in a new display window, at `geometry` when known, else
    /// sized to the display at 100% (within the monitor).
    fn open_window(
        &mut self,
        ctx: &egui::Context,
        loc: Location,
        geometry: Geometry,
        toolbar: bool,
    ) -> WindowKey {
        let id = egui::ViewportId::from_hash_of(("display", self.next_window));
        self.next_window += 1;
        let mut view = View::new(&self.pvs, None);
        view.show(loc);
        // In desktop points: egui's are scaled by its zoom factor.
        let zoom = ctx.zoom_factor();
        let size = geometry.size.map(|s| s / zoom).or_else(|| {
            let monitor = ctx.input(|i| i.viewport().monitor_size);
            let display = view.natural_size(zoom)?;
            Some(chrome::window_size(display, CHILD_STATUS, monitor))
        });
        let mut builder = egui::ViewportBuilder::default()
            .with_app_id("bobview")
            .with_inner_size(size.unwrap_or(chrome::DEFAULT_WINDOW / zoom))
            .with_min_inner_size([160.0, 100.0]);
        if let Some(pos) = geometry.position {
            builder = builder.with_position(Pos2::ZERO + pos.to_vec2() / zoom);
        }
        self.windows.push(DisplayWindow {
            id,
            view,
            builder,
            fit_next: geometry.size.is_none(),
            toolbar,
            geometry,
            close: false,
        });
        WindowKey::Child(id)
    }

    /// Whether a display, or one named by a command action, is still being
    /// worked out.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_loading(&self) -> bool {
        self.main.is_loading() || self.windows.iter().any(|w| w.view.is_loading())
    }

    /// Run a command action on a thread, so a slow launcher script or file
    /// system doesn't freeze the window, and open the display it names in
    /// `origin`'s window, as a link there would. A command already in
    /// flight is ignored.
    #[cfg(not(target_arch = "wasm32"))]
    fn launch(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        base: Option<Source>,
        dir: Option<Source>,
        cmd: String,
    ) {
        let tx = self.launched.0.clone();
        let Some(view) = self.view_mut(origin.window) else {
            return;
        };
        let key = (
            base.as_ref().map(|s| s.to_string()),
            dir.as_ref().map(|s| s.to_string()),
            cmd.clone(),
        );
        if view.launching.contains(&key) {
            return;
        }
        view.launching.push(key.clone());
        let (ctx, shown) = (ctx.clone(), view.shown);
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
            tx.send(Launched {
                key,
                origin,
                shown,
                target,
            })
            .ok();
            ctx.request_repaint();
        });
    }

    /// A browser runs no commands.
    #[cfg(target_arch = "wasm32")]
    fn launch(
        &mut self,
        _: &egui::Context,
        _: Origin,
        _: Option<Source>,
        _: Option<Source>,
        cmd: String,
    ) {
        log::warn!("command '{cmd}' is not run in a browser");
    }

    /// Open the displays named by finished command actions, unless the user
    /// has since moved to another display or closed the window.
    fn poll_launched(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.launched.1.try_recv() {
            let Some(view) = self.view_mut(done.origin.window) else {
                continue;
            };
            view.launching.retain(|k| *k != done.key);
            let current = done.shown == view.shown;
            match done.target {
                Some(loc) if current => self.follow(ctx, done.origin, loc, "window"),
                Some(loc) => log::info!("drop {}: the display has changed", loc.source),
                None => {}
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        // One row whatever the width, so a window sized to the display shows it
        // near 100%; the display name is elided when there is no room. A narrow
        // window (a phone) keeps the navigation and Fit, and moves the rest
        // into a menu.
        let compact = ui.available_width() < chrome::FULL_TOOLBAR_WIDTH;
        let windows = can_multi_window(ui.ctx(), self.keypad_mode);
        let mut action = None;
        let view = &mut self.main;
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            let back = ui
                .add_enabled(!view.back.is_empty(), egui::Button::new("◀"))
                .on_hover_text("Back");
            self.chrome.button = back.rect.height();
            if back.clicked() {
                view.go_back();
            }
            if ui
                .add_enabled(!view.forward.is_empty(), egui::Button::new("▶"))
                .on_hover_text("Forward")
                .clicked()
            {
                view.go_forward();
            }
            if ui
                .add_enabled(self.home.is_some(), egui::Button::new("⌂"))
                .on_hover_text("Home")
                .clicked()
                && let Some(home) = self.home.clone()
            {
                view.open(home);
            }
            if compact {
                ui.separator();
                if ui.selectable_label(view.fit, "Fit").clicked() {
                    view.fit = !view.fit;
                }
                ui.label(format!("{:.0}%", view.zoom * 100.0));
                ui.menu_button("☰", |ui| {
                    if ui.button("⟳ Reload").clicked() {
                        view.reload();
                    }
                    if ui.button("+ Zoom in").clicked() {
                        view.zoom_by(1.25);
                    }
                    if ui.button("− Zoom out").clicked() {
                        view.zoom_by(1.0 / 1.25);
                    }
                    if ui
                        .selectable_label(self.chrome.auto_size, "🗖 Auto-size window")
                        .clicked()
                    {
                        toggle_auto_size(&mut self.chrome);
                    }
                    if ui.selectable_label(self.keypad_mode, "⌨ Keypad").clicked() {
                        self.keypad_mode = !self.keypad_mode;
                    }
                    if ui.selectable_label(self.show_open, "Open…").clicked() {
                        self.show_open = !self.show_open;
                    }
                    if windows {
                        ui.separator();
                        action = layout_controls(
                            ui,
                            &mut self.multi_window,
                            self.layout_name.as_deref(),
                            &mut self.settings,
                            &mut self.layouts,
                            true,
                        );
                    }
                });
            } else {
                if ui.button("⟳").on_hover_text("Reload").clicked() {
                    view.reload();
                }
                ui.separator();
                if ui.button("−").clicked() {
                    view.zoom_by(1.0 / 1.25);
                }
                if ui.selectable_label(view.fit, "Fit").clicked() {
                    view.fit = !view.fit;
                }
                if ui.button("+").clicked() {
                    view.zoom_by(1.25);
                }
                ui.label(format!("{:.0}%", view.zoom * 100.0));
                if ui
                    .selectable_label(self.chrome.auto_size, "🗖")
                    .on_hover_text("Auto-size: resize the window to fit each display opened")
                    .clicked()
                {
                    toggle_auto_size(&mut self.chrome);
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
                if windows {
                    ui.separator();
                    action = layout_controls(
                        ui,
                        &mut self.multi_window,
                        self.layout_name.as_deref(),
                        &mut self.settings,
                        &mut self.layouts,
                        false,
                    );
                }
            }
            if let Some(s) = &view.screen {
                ui.add(egui::Label::new(egui::RichText::new(&s.title).strong()).truncate())
                    .on_hover_text(&s.title);
            }
        });
        if let Some(action) = action {
            self.layout_action(action);
            // A layout loads on the next frame.
            ui.ctx().request_repaint();
        }
    }

    /// Load, save or delete a layout, as the toolbar asked.
    fn layout_action(&mut self, action: LayoutAction) {
        let done = match action {
            LayoutAction::Load(name) => layout::load(&name).map(|l| {
                self.pending_layout = Some((l, Some(name)));
            }),
            LayoutAction::Save(name) => self.save_layout(&name),
            LayoutAction::SaveAs => {
                self.save_as = Some(SaveAs {
                    name: self.layout_name.clone().unwrap_or_default(),
                    confirm: false,
                });
                Ok(())
            }
            LayoutAction::Delete(name) => layout::delete(&name).map(|()| {
                self.layouts.refresh();
                log::info!("deleted layout {name}");
                if self.layout_name.as_deref() == Some(name.as_str()) {
                    self.layout_name = None;
                }
            }),
        };
        if let Err(e) = done {
            log::error!("{e}");
        }
    }

    /// The open windows as a layout.
    fn layout(&self) -> Layout {
        let mut windows = Vec::new();
        if let Some(loc) = &self.main.current {
            let mut w = WindowLayout::new(loc, self.geometry);
            w.main = true;
            w.zoom = Some(Zoom::new(self.main.fit, self.main.zoom));
            windows.push(w);
        }
        for dw in self.windows.iter().filter(|w| !w.close) {
            if let Some(loc) = &dw.view.current {
                let mut w = WindowLayout::new(loc, dw.geometry);
                w.toolbar = dw.toolbar;
                windows.push(w);
            }
        }
        let trends: Vec<TrendLayout> = self
            .trends
            .iter()
            .map(|t| TrendLayout::new(t.names(), t.view(), t.geometry))
            .collect();
        // On X11 a window not drawn yet has no position either.
        let unplaced = windows.iter().any(|w| w.position.is_none())
            || trends.iter().any(|t| t.position.is_none());
        if on_wayland() && unplaced {
            positions_unknown();
        }
        Layout::new(self.multi_window, windows, trends)
    }

    fn save_layout(&mut self, name: &str) -> Result<(), String> {
        layout::save(name, &self.layout())?;
        self.layouts.refresh();
        self.layout_name = Some(name.trim().to_string());
        Ok(())
    }

    /// Close the display windows and open those of `layout`; the main
    /// window moves, takes the layout's size and shows its display.
    fn apply_layout(&mut self, ctx: &egui::Context, layout: Layout, name: Option<String>) {
        // Positions are kept when on a monitor: those the window system
        // lists, else the main window's.
        let zoom = ctx.zoom_factor();
        let monitors = if self.monitors.is_empty() {
            ctx.input(|i| i.viewport().monitor_size)
                .map(|m| vec![Rect::from_min_size(Pos2::ZERO, m * zoom)])
                .unwrap_or_default()
        } else {
            self.monitors.clone()
        };
        self.windows.clear();
        self.multi_window = layout.multi_window;
        for w in &layout.windows {
            let geometry = w.geometry(&monitors);
            if w.position.is_some() && geometry.position.is_none() {
                log::info!("{}: its saved position is off screen", w.display);
            }
            let loc = w.location();
            if !w.main {
                self.open_window(ctx, loc, geometry, w.toolbar);
                continue;
            }
            if let Some(size) = geometry.size {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size / zoom));
                self.chrome.hold_size();
            }
            if let Some(pos) = geometry.position {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                    Pos2::ZERO + pos.to_vec2() / zoom,
                ));
            }
            if let Some(z) = &w.zoom {
                self.main.fit = z.factor().is_none();
                self.main.zoom = z.factor().unwrap_or(self.main.zoom);
            }
            if self.home.is_none() {
                self.home = Some(loc.clone());
            }
            self.show_open = false;
            self.main.open(loc);
        }
        self.trends.clear();
        for t in &layout.trends {
            let mut trend = Trend::new(self.next_trend, &t.pvs, &self.pvs);
            self.next_trend += 1;
            trend.set_view(t.span, t.log);
            trend.place = t.geometry(&monitors);
            self.trends.push(trend);
        }
        let placed = layout.windows.iter().any(|w| w.position.is_some())
            || layout.trends.iter().any(|t| t.position.is_some());
        if placed && on_wayland() {
            positions_unknown();
        }
        self.layout_name = name;
    }

    /// The Save as… dialog: a name, and a question before replacing a
    /// layout.
    fn save_as_window(&mut self, ui: &mut egui::Ui) {
        let Some(dialog) = self.save_as.as_mut() else {
            return;
        };
        let mut open = true;
        let mut save = None;
        egui::Window::new("Save layout as")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                let name = dialog.name.trim().to_string();
                if dialog.confirm {
                    ui.label(format!("Replace the layout {name}?"));
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            save = Some(name);
                        }
                        if ui.button("Cancel").clicked() {
                            dialog.confirm = false;
                        }
                    });
                    return;
                }
                let edit = ui.text_edit_singleline(&mut dialog.name);
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let valid = layout::valid_name(&name);
                let clicked = ui.add_enabled(valid, egui::Button::new("Save")).clicked();
                if valid && (clicked || enter) {
                    if layout::exists(&name) {
                        dialog.confirm = true;
                    } else {
                        save = Some(name);
                    }
                }
            });
        if let Some(name) = save {
            if let Err(e) = self.save_layout(&name) {
                log::error!("{e}");
            }
            open = false;
        }
        if !open {
            self.save_as = None;
        }
    }

    fn open_panel(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        let path = &mut self.main.open_path;
        ui.horizontal(|ui| {
            ui.label("File or URL:");
            let edit = ui.add(egui::TextEdit::singleline(&mut *path).desired_width(f32::INFINITY));
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                chosen = Some(Source::parse(path));
            }
        });
        // Nothing to list for a URL (an empty path lists nothing).
        let dir = match Source::parse(path) {
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
                    *path = format!("{}/", parent.display());
                }
                for (entry, is_dir) in list_dir(&dir) {
                    let name = entry
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if is_dir {
                        if ui.button(format!("📁 {name}")).clicked() {
                            *path = format!("{}/", entry.display());
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
            self.main.open(Location {
                source,
                macros: Macros::default(),
            });
        }
    }
}

/// Turn Auto-size on or off; turning it on sizes the window to the
/// current display straight away.
fn toggle_auto_size(chrome: &mut Chrome) {
    chrome.auto_size = !chrome.auto_size;
    if chrome.auto_size {
        chrome.display_changed();
    }
}

/// The status line's height in a display window, in points, for sizing a
/// new one before it is drawn.
const CHILD_STATUS: f32 = 24.0;

/// Whether displays can have native windows of their own: not in a browser
/// or where there's only one native window, nor on a touch or small screen
/// (a phone).
fn can_multi_window(ctx: &egui::Context, touch: bool) -> bool {
    let zoom = ctx.zoom_factor();
    let small = ctx.input(|i| chrome::small_screen(i.viewport(), zoom));
    !cfg!(target_arch = "wasm32") && !ctx.embed_viewports() && !touch && !small
}

/// What the main toolbar's layout controls ask for.
enum LayoutAction {
    Load(String),
    Save(String),
    SaveAs,
    Delete(String),
}

/// The multi-window toggle, the layout list and the layout menu; in the
/// `compact` toolbar's menu, the list is a submenu.
fn layout_controls(
    ui: &mut egui::Ui,
    multi_window: &mut bool,
    current: Option<&str>,
    settings: &mut layout::Settings,
    layouts: &mut LayoutList,
    compact: bool,
) -> Option<LayoutAction> {
    let mut action = None;
    let toggle = if compact { "🗗 Multi-window" } else { "🗗" };
    if ui
        .selectable_label(*multi_window, toggle)
        .on_hover_text("Multi-window: links open their displays in windows of their own (ctrl+click opens in place)")
        .clicked()
    {
        *multi_window = !*multi_window;
    }
    let mut load = |ui: &mut egui::Ui| {
        let names = layouts.get(ui.ctx());
        if names.is_empty() {
            ui.weak("No saved layouts");
        }
        for name in names {
            if ui
                .selectable_label(current == Some(name.as_str()), &name)
                .clicked()
            {
                action = Some(LayoutAction::Load(name));
            }
        }
    };
    if compact {
        ui.menu_button("Load layout", load);
    } else {
        egui::ComboBox::from_id_salt("layout")
            .selected_text(current.unwrap_or("Layout"))
            .show_ui(ui, &mut load);
    }
    let menu = if compact { "Layout" } else { "▾" };
    ui.menu_button(menu, |ui| {
        if ui
            .add_enabled(current.is_some(), egui::Button::new("Save"))
            .clicked()
            && let Some(name) = current
        {
            action = Some(LayoutAction::Save(name.to_string()));
        }
        if ui.button("Save as default").clicked() {
            action = Some(LayoutAction::Save(layout::DEFAULT.to_string()));
        }
        if ui.button("Save as…").clicked() {
            action = Some(LayoutAction::SaveAs);
        }
        ui.menu_button("Delete", |ui| {
            let names = layouts.get(ui.ctx());
            if names.is_empty() {
                ui.weak("No saved layouts");
            }
            for name in names {
                if ui.button(&name).clicked() {
                    action = Some(LayoutAction::Delete(name));
                }
            }
        });
        ui.separator();
        if ui
            .checkbox(&mut settings.save_default_on_exit, "Save default on exit")
            .changed()
        {
            settings.save();
        }
    })
    .response
    .on_hover_text("Save, save as or delete layouts");
    action
}

/// Whether windows are on Wayland, which won't let a client place them.
fn on_wayland() -> bool {
    !cfg!(target_arch = "wasm32")
        && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

/// Say, once, that window positions are not known or can't be set.
fn positions_unknown() {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        log::info!(
            "window positions are left to the compositor on Wayland \
             (start bobview with WAYLAND_DISPLAY= to use XWayland, which keeps them)"
        );
    });
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

impl eframe::App for BobApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Before the PVs are read, so a later update asks again.
        self.repaint_asked.store(false, Ordering::Relaxed);
        // A layout's positions are checked against the monitors as they are
        // when it is loaded.
        #[cfg(not(target_arch = "wasm32"))]
        if self.pending_layout.is_some()
            && let Some(window) = frame.winit_window()
        {
            // Desktop points, as egui's (times its zoom factor).
            let scale = window.scale_factor() as f32;
            self.monitors = window
                .available_monitors()
                .map(|m| {
                    let (pos, size) = (m.position(), m.size());
                    Rect::from_min_size(
                        Pos2::new(pos.x as f32, pos.y as f32) / scale,
                        Vec2::new(size.width as f32, size.height as f32) / scale,
                    )
                })
                .collect();
        }
        #[cfg(target_arch = "wasm32")]
        let _ = frame;
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.settings.save_default_on_exit
            && let Err(e) = layout::save(layout::DEFAULT, &self.layout())
        {
            log::error!("{e}");
        }
    }
}

impl BobApp {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.main.poll_loading(&ctx);
        for w in &mut self.windows {
            w.view.poll_loading(&ctx);
        }
        self.poll_launched(&ctx);
        if !self.keypad_mode && ui.input(|i| i.any_touches()) {
            self.keypad_mode = true;
        }
        widgets::touch_targets(ui.ctx(), self.keypad_mode);
        self.chrome.begin(ui.ctx());
        // Once the controls' scale has settled, as positions and sizes
        // depend on it.
        if self.chrome.settled()
            && let Some((layout, name)) = self.pending_layout.take()
        {
            self.apply_layout(&ctx, layout, name);
        }
        if shortcuts(ui, &mut self.main).close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.geometry = geometry(ui);
        self.main.update_title(ui.ctx());
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

        let trends: Vec<(u64, String)> = self.trends.iter().map(|t| (t.id, t.title())).collect();
        let vcx = ViewCx {
            window: WindowKey::Main,
            keypad: self.keypad_mode,
            trends: &trends,
        };
        let drawn = self.main.draw(ui, &vcx);
        self.chrome.panels = panels + drawn.status;

        // A new display (not a reload, which keeps the size the user gave
        // the window) sizes the window to show it at 100% (or the fixed zoom).
        if std::mem::take(&mut self.main.new_display) {
            self.chrome.display_changed();
        }
        let display = self.main.natural_size(ui.ctx().zoom_factor());
        self.chrome.end(ui.ctx(), display);

        let mut requests = drawn.requests;
        requests.extend(self.display_windows(ui, &trends));
        for (origin, req) in requests {
            self.request(ui.ctx(), origin, req);
        }
        // A dialog goes with its window.
        let closed = |w: WindowKey| match w {
            WindowKey::Main => false,
            WindowKey::Child(id) => !self.windows.iter().any(|dw| dw.id == id),
        };
        if self.keypad.as_ref().is_some_and(|k| closed(k.window)) {
            self.keypad = None;
        }
        if self.details.as_ref().is_some_and(|(w, _)| closed(*w)) {
            self.details = None;
        }
        if self.confirm.as_ref().is_some_and(|c| closed(c.window)) {
            self.confirm = None;
        }
        dialogs(
            ui.ctx(),
            WindowKey::Main,
            &mut self.keypad,
            &mut self.details,
            &mut self.confirm,
        );
        self.save_as_window(ui);
        self.trend_windows(ui);
    }

    /// Act on a request from a widget in `origin`'s window.
    fn request(&mut self, ctx: &egui::Context, origin: Origin, req: Request) {
        let window = origin.window;
        match req {
            Request::Open {
                source,
                macros,
                target,
            } => self.follow(ctx, origin, Location { source, macros }, &target),
            // Shown in the window asked from, which may have been drawn
            // already this frame.
            Request::Keypad { pv, text } => {
                self.keypad = Some(Keypad { window, pv, text });
                ctx.request_repaint();
            }
            // A second one replaces a pending one: the dialog is modal in
            // spirit, and a display cannot make writes pile up behind it.
            Request::ConfirmWrite { pv, value } => {
                self.confirm = Some(Confirm { window, pv, value });
                ctx.request_repaint();
            }
            Request::Details(details) => {
                self.details = Some((window, details));
                ctx.request_repaint();
            }
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
                    raise_trend(ctx, id);
                }
            }
            Request::Command {
                base,
                directory,
                command,
            } => self.launch(ctx, origin, base, directory, command),
            // Exit in a display window closes it; the main window goes back
            // first, as one window used to.
            Request::Close => match window {
                WindowKey::Main if !self.main.back.is_empty() => self.main.go_back(),
                WindowKey::Main => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                WindowKey::Child(id) => {
                    if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                        w.close = true;
                    }
                }
            },
            Request::Webpage(url) => open_webpage(&url),
        }
    }

    /// Each display window in its own native window, until it is closed, or
    /// in a window inside this one where there's only one native window.
    /// Returns the requests their widgets made.
    fn display_windows(
        &mut self,
        ui: &mut egui::Ui,
        trends: &[(u64, String)],
    ) -> Vec<(Origin, Request)> {
        let ctx = ui.ctx().clone();
        let mut requests = Vec::new();
        // Closed by Exit or Ctrl+W since they were last drawn.
        self.windows.retain(|w| !w.close);
        let (home, keypad, auto_size) = (&self.home, self.keypad_mode, self.chrome.auto_size);
        let (pad, details, confirm) = (&mut self.keypad, &mut self.details, &mut self.confirm);
        for w in &mut self.windows {
            let key = WindowKey::Child(w.id);
            let vcx = ViewCx {
                window: key,
                keypad,
                trends,
            };
            let title = w
                .view
                .screen
                .as_ref()
                .map_or_else(|| "bobview".to_string(), |s| s.title.clone());
            if ctx.embed_viewports() {
                let mut open = true;
                egui::Window::new(&title)
                    .id(display_window_id(w.id))
                    .open(&mut open)
                    .collapsible(false)
                    .default_size(Vec2::new(640.0, 420.0))
                    .show(&ctx, |ui| {
                        requests.extend(child_ui(ui, w, home, &vcx, None));
                    });
                dialogs(&ctx, key, pad, details, confirm);
                w.close |= !open;
                continue;
            }
            let builder = w.builder.clone().with_title(title);
            ctx.show_viewport_immediate(w.id, builder, |ui, class| {
                // No native windows after all: a window in this one.
                let native = (class != egui::ViewportClass::EmbeddedWindow).then_some(auto_size);
                requests.extend(child_ui(ui, w, home, &vcx, native));
                dialogs(ui.ctx(), key, pad, details, confirm);
                // Closing the window drops its view, and its PVs with it.
                if ui.input(|i| i.viewport().close_requested()) {
                    w.close = true;
                }
            });
        }
        self.windows.retain(|w| !w.close);
        requests
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
        // In desktop points: egui's are scaled by its zoom factor.
        let zoom = ctx.zoom_factor();
        self.trends.retain_mut(|trend| {
            // The window manager places it, unless a layout says where;
            // Wayland won't let us.
            let size = trend
                .place
                .size
                .map_or(Vec2::new(640.0, 420.0), |s| s / zoom);
            let mut builder = egui::ViewportBuilder::default()
                .with_title(trend.title())
                .with_app_id("bobview")
                .with_inner_size(size)
                .with_min_inner_size([320.0, 200.0]);
            if let Some(pos) = trend.place.position {
                builder = builder.with_position(Pos2::ZERO + pos.to_vec2() / zoom);
            }
            ctx.show_viewport_immediate(trend_viewport(trend.id), builder, |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    // No native windows after all: a window in this one,
                    // which can't be closed.
                    trend.ui(ui, pvs);
                    return true;
                }
                trend.geometry = geometry(ui);
                egui::CentralPanel::default().show(ui, |ui| trend.ui(ui, pvs));
                // Closing the window drops the trend, and its PVs with it.
                !ui.input(|i| i.viewport().close_requested())
            })
        });
    }
}

/// The keypad and PV details dialogs, when asked for in `window`: drawn in
/// that window.
fn dialogs(
    ctx: &egui::Context,
    window: WindowKey,
    keypad: &mut Option<Keypad>,
    details: &mut Option<(WindowKey, pvmenu::Details)>,
    confirm: &mut Option<Confirm>,
) {
    if confirm.as_ref().is_some_and(|c| c.window == window) {
        confirm_window(ctx, confirm);
    }
    if keypad.as_ref().is_some_and(|k| k.window == window) {
        keypad_window(ctx, keypad);
    }
    if details.as_ref().is_some_and(|(w, _)| *w == window) {
        details_window(ctx, details);
    }
}

/// Ask whether to write the value a display from the web asked for.
fn confirm_window(ctx: &egui::Context, confirm: &mut Option<Confirm>) {
    let Some(c) = confirm.as_ref() else {
        return;
    };
    let mut answer = None;
    egui::Window::new("Write to PV?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("A display loaded from the web asks to write");
            ui.monospace(format!("{} = {}", plain_name(c.pv.name()), c.value));
            ui.horizontal(|ui| {
                let write = ui.button("Write").clicked();
                let cancel = ui.button("Cancel");
                // Cancel has the focus, so Enter cancels; only a click writes.
                if !ui.memory(|m| m.focused().is_some()) {
                    cancel.request_focus();
                }
                answer = if write {
                    Some(true)
                } else if cancel.clicked() {
                    Some(false)
                } else {
                    None
                };
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    if let Some(yes) = answer {
        if yes {
            c.pv.write(c.value.clone());
        }
        *confirm = None;
    }
}

fn details_window(ctx: &egui::Context, details: &mut Option<(WindowKey, pvmenu::Details)>) {
    let mut open = details.is_some();
    if let Some((_, d)) = details.as_ref() {
        egui::Window::new("PV details")
            .open(&mut open)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| pvmenu::details_ui(ui, d));
    }
    if !open {
        *details = None;
    }
}

fn keypad_window(ctx: &egui::Context, keypad: &mut Option<Keypad>) {
    let Some(pad) = keypad.as_mut() else {
        return;
    };
    let mut done = None;
    let title = plain_name(pad.pv.name());
    let state = pad.pv.state();
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -8.0))
        .show(ctx, |ui| {
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
                            let b = egui::Button::new(egui::RichText::new(key).font(font.clone()));
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
            *keypad = None;
        }
        Some(None) => *keypad = None,
        None => {}
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

/// The native window that `window` is.
fn viewport(window: WindowKey) -> egui::ViewportId {
    match window {
        WindowKey::Main => egui::ViewportId::ROOT,
        WindowKey::Child(id) => id,
    }
}

/// The display window `id` inside the main one, where windows are embedded.
fn display_window_id(id: egui::ViewportId) -> Id {
    Id::new(("display window", id))
}

/// Bring `window` to the front, as a link to its display is followed.
fn raise(ctx: &egui::Context, window: WindowKey) {
    match window {
        WindowKey::Child(id) if ctx.embed_viewports() => ctx.move_to_top(egui::LayerId::new(
            egui::Order::Middle,
            display_window_id(id),
        )),
        _ => ctx.send_viewport_cmd_to(viewport(window), egui::ViewportCommand::Focus),
    }
}

/// Where the window being drawn is, in desktop points (egui's times its
/// zoom factor); None where the window system doesn't say.
fn geometry(ui: &egui::Ui) -> Geometry {
    let zoom = ui.ctx().zoom_factor();
    ui.input(|i| {
        let info = i.viewport();
        Geometry {
            position: info.outer_rect.map(|r| Pos2::ZERO + r.min.to_vec2() * zoom),
            size: info.inner_rect.map(|r| r.size() * zoom),
        }
    })
}

/// What the keyboard shortcuts ask of the window beyond its view.
#[derive(Default)]
struct Shortcuts {
    close: bool,
    toolbar: bool,
}

/// The shortcuts every window has: Ctrl+R reloads, Alt+Left and Alt+Right go
/// back and forward, Ctrl+W closes the window and Ctrl+T shows or hides a
/// display window's toolbar.
fn shortcuts(ui: &egui::Ui, view: &mut View) -> Shortcuts {
    use egui::{Key, Modifiers};
    // Not while typing in the window (focus is as the last frame left it):
    // there Ctrl+W deletes a word and Alt+arrows move by word.
    if ui.ctx().text_edit_focused() {
        return Shortcuts::default();
    }
    ui.input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND, Key::R) {
            view.reload();
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowLeft) {
            view.go_back();
        }
        if i.consume_key(Modifiers::ALT, Key::ArrowRight) {
            view.go_forward();
        }
        Shortcuts {
            close: i.consume_key(Modifiers::COMMAND, Key::W),
            toolbar: i.consume_key(Modifiers::COMMAND, Key::T),
        }
    })
}

/// A display window's contents: its toolbar when shown, the display, the
/// chevron that shows or hides the toolbar and the context menu on the
/// display's background. `native` is Some(auto-size) in a native window,
/// which is sized to each new display (only the first unless auto-size).
fn child_ui(
    ui: &mut egui::Ui,
    w: &mut DisplayWindow,
    home: &Option<Location>,
    vcx: &ViewCx,
    native: Option<bool>,
) -> Vec<(Origin, Request)> {
    if native.is_some() {
        let keys = shortcuts(ui, &mut w.view);
        w.close |= keys.close;
        w.toolbar ^= keys.toolbar;
    }
    let mut panels = 0.0;
    if w.toolbar {
        panels += egui::Panel::top(Id::new(("toolbar", w.id)))
            .show(ui, |ui| display_toolbar(ui, &mut w.view, home))
            .response
            .rect
            .height();
    }
    let drawn = w.view.draw(ui, vcx);
    panels += drawn.status;
    if let Some(background) = &drawn.background {
        background.context_menu(|ui| {
            let label = if w.toolbar {
                "Hide toolbar"
            } else {
                "Show toolbar"
            };
            if ui.button(label).clicked() {
                w.toolbar = !w.toolbar;
            }
            if ui.button("Close").clicked() {
                w.close = true;
            }
        });
    }
    let Some(auto_size) = native else {
        w.view.new_display = false;
        return drawn.requests;
    };
    chevron(ui, w);
    w.geometry = geometry(ui);
    // Sized to a new display at 100%, within the monitor, once the window
    // knows its monitor; then the display fits the window as it is resized.
    let info = ui.input(|i| i.viewport().clone());
    if w.view.new_display && info.monitor_size.is_some() {
        w.view.new_display = false;
        let zoom = ui.ctx().zoom_factor();
        let display = w.view.natural_size(zoom);
        // One that failed to load leaves a window not yet sized to be
        // sized to the next.
        let fit = w.fit_next;
        w.fit_next = auto_size || fit && display.is_none();
        if fit
            && chrome::can_resize(&info, zoom)
            && let Some(display) = display
        {
            let size = chrome::window_size(display, panels, info.monitor_size);
            let inner = ui.input(|i| i.content_rect().size());
            if (size - inner).length() >= 1.0 {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            }
        }
    }
    drawn.requests
}

/// A small chevron in the top-right corner while the pointer is over the
/// window, that shows or hides its toolbar: drawn over the display, so it
/// takes no room.
fn chevron(ui: &egui::Ui, w: &mut DisplayWindow) {
    if !ui.input(|i| i.pointer.has_pointer()) {
        return;
    }
    egui::Area::new(Id::new(("chevron", w.id)))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-4.0, 4.0))
        .show(ui.ctx(), |ui| {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, 14.0), egui::Sense::click());
            let visuals = ui.style().interact(&resp);
            let painter = ui.painter();
            painter.rect_filled(rect, 3.0, visuals.weak_bg_fill);
            // ⌄ to show the toolbar, ⌃ to hide it (drawn: the fonts have no ⌄).
            let c = rect.center();
            let (dx, dy) = (5.0, if w.toolbar { -2.5 } else { 2.5 });
            painter.line(
                vec![
                    c + Vec2::new(-dx, -dy),
                    c + Vec2::new(0.0, dy),
                    c + Vec2::new(dx, -dy),
                ],
                visuals.fg_stroke,
            );
            let tip = if w.toolbar {
                "Hide the toolbar (Ctrl+T)"
            } else {
                "Show the toolbar (Ctrl+T)"
            };
            if resp.on_hover_text(tip).clicked() {
                w.toolbar = !w.toolbar;
            }
        });
}

/// A display window's toolbar: navigation, reload and zoom. The window and
/// layout controls stay on the main window.
fn display_toolbar(ui: &mut egui::Ui, view: &mut View, home: &Option<Location>) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!view.back.is_empty(), egui::Button::new("◀"))
            .on_hover_text("Back")
            .clicked()
        {
            view.go_back();
        }
        if ui
            .add_enabled(!view.forward.is_empty(), egui::Button::new("▶"))
            .on_hover_text("Forward")
            .clicked()
        {
            view.go_forward();
        }
        if ui.button("⟳").on_hover_text("Reload").clicked() {
            view.reload();
        }
        ui.separator();
        if ui.button("−").clicked() {
            view.zoom_by(1.0 / 1.25);
        }
        if ui.selectable_label(view.fit, "Fit").clicked() {
            view.fit = !view.fit;
        }
        if ui.button("+").clicked() {
            view.zoom_by(1.25);
        }
        ui.label(format!("{:.0}%", view.zoom * 100.0));
        ui.separator();
        if ui
            .add_enabled(home.is_some(), egui::Button::new("⌂"))
            .on_hover_text("Home")
            .clicked()
            && let Some(home) = home.clone()
        {
            view.open(home);
        }
    });
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
        use super::{BobApp, Launched, Location, Origin, WindowKey};
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        // A double click runs the command once, and counts as loading.
        let main = Origin {
            window: WindowKey::Main,
            ctrl: false,
        };
        app.launch(&ctx, main, None, None, "no-such-launcher".into());
        app.launch(&ctx, main, None, None, "no-such-launcher".into());
        assert_eq!(app.main.launching.len(), 1);
        assert!(app.is_loading());
        while app.is_loading() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.poll_launched(&ctx);
        }
        // A result from before the display changed is dropped.
        let loc = Location {
            source: bob::Source::File("/no/such.bob".into()),
            macros: Default::default(),
        };
        let key = (None, None, "x".to_string());
        app.main.launching.push(key.clone());
        let stale = app.main.shown;
        app.main.shown += 1;
        app.launched
            .0
            .send(Launched {
                key,
                origin: main,
                shown: stale,
                target: Some(loc.clone()),
            })
            .unwrap();
        app.poll_launched(&ctx);
        assert!(app.main.current.is_none() && !app.is_loading());
        // In multi-window mode a ctrl+clicked command opens in place, as a
        // ctrl+clicked link does.
        app.set_multi_window(true);
        app.launched
            .0
            .send(Launched {
                key: (None, None, "y".to_string()),
                origin: Origin { ctrl: true, ..main },
                shown: app.main.shown,
                target: Some(loc.clone()),
            })
            .unwrap();
        app.poll_launched(&ctx);
        assert!(app.windows.is_empty());
        assert_eq!(app.main.current, Some(loc));
    }

    #[test]
    fn links_route_by_mode_modifier_and_open_windows() {
        use super::{Route, WindowKey, route};
        let child = WindowKey::Child(eframe::egui::ViewportId::from_hash_of("x"));
        // One window: always in place, whatever is open.
        assert_eq!(route(false, false, None), Route::InPlace);
        assert_eq!(route(false, false, Some(child)), Route::InPlace);
        assert_eq!(route(false, true, None), Route::InPlace);
        // Multi-window: a new window, or the one already showing it.
        assert_eq!(route(true, false, None), Route::NewWindow);
        assert_eq!(route(true, false, Some(child)), Route::Raise(child));
        assert_eq!(
            route(true, false, Some(WindowKey::Main)),
            Route::Raise(WindowKey::Main)
        );
        // Ctrl+click navigates in place.
        assert_eq!(route(true, true, None), Route::InPlace);
        assert_eq!(route(true, true, Some(child)), Route::InPlace);
    }

    #[test]
    fn same_display_compares_effective_macros() {
        use super::Location;
        use bob::{Macros, Source};
        let loc = |pairs: &[(&str, &str)]| Location {
            source: Source::parse("a.bob"),
            macros: Macros::from_pairs(pairs.iter().copied()),
        };
        let flat = loc(&[("P", "X"), ("R", "1")]);
        assert!(flat.same_display(&loc(&[("R", "0"), ("P", "X"), ("R", "1")])));
        assert!(!flat.same_display(&loc(&[("P", "X"), ("R", "2")])));
        let other = Location {
            source: Source::parse("b.bob"),
            ..flat.clone()
        };
        assert!(!flat.same_display(&other));
    }

    #[test]
    fn requests_act_on_their_own_window() {
        use super::{BobApp, Location, Origin, WindowKey};
        use crate::widgets::Request;
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        let display = |name: &str| Location {
            source: bob::Source::File(format!("/no/such/{name}.bob").into()),
            macros: bob::Macros::from_pairs([("P", name)]),
        };
        let open = |app: &mut BobApp, window, ctrl, loc: Location| {
            let req = Request::Open {
                source: loc.source,
                macros: loc.macros,
                target: "replace".into(),
            };
            app.request(&ctx, Origin { window, ctrl }, req);
        };
        let main = WindowKey::Main;
        open(&mut app, main, false, display("a"));
        assert_eq!(app.main.current, Some(display("a")));

        app.set_multi_window(true);
        // A link opens a new window, and again raises it.
        open(&mut app, main, false, display("b"));
        assert_eq!(app.windows.len(), 1);
        let child = WindowKey::Child(app.windows[0].id);
        open(&mut app, main, false, display("b"));
        assert_eq!(app.windows.len(), 1);
        // The same display with other macros is another window.
        let mut other = display("b");
        other.macros = bob::Macros::from_pairs([("P", "c")]);
        open(&mut app, main, false, other);
        assert_eq!(app.windows.len(), 2);
        // Ctrl+click in a window navigates it, keeping its history.
        open(&mut app, child, true, display("d"));
        assert_eq!(app.windows[0].view.current, Some(display("d")));
        assert_eq!(app.windows[0].view.back, [display("b")]);
        assert_eq!(app.main.current, Some(display("a")));
        // Exit closes a display window even when it has history.
        app.request(
            &ctx,
            Origin {
                window: child,
                ctrl: false,
            },
            Request::Close,
        );
        assert!(app.view_mut(child).is_none());
        // The main window still goes back first.
        open(&mut app, main, true, display("e"));
        app.request(
            &ctx,
            Origin {
                window: main,
                ctrl: false,
            },
            Request::Close,
        );
        assert_eq!(app.main.current, Some(display("a")));
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
