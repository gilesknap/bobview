//! The application window: navigation bar, display area and status line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bob::{Macros, Source};
use eframe::egui::{self, Id, Vec2};
use pv::PvManager;

use crate::chrome::{self, Chrome, UiScale};
use crate::pvmenu;
use crate::style;
use crate::trend::Trend;
use crate::view::{LaunchKey, View, ViewCx, WindowKey};
use crate::widgets::{self, Request};

#[derive(Clone, Debug)]
pub struct Location {
    pub source: Source,
    pub macros: Macros,
}

pub struct BobApp {
    pvs: PvManager,
    /// The main window's display.
    main: View,
    home: Option<Location>,
    show_open: bool,
    /// Use the on-screen keypad for text entries (switched on by the first touch).
    keypad_mode: bool,
    keypad: Option<Keypad>,
    details: Option<pvmenu::Details>,
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

/// A command action's result, tagged with the window and display it was
/// started on.
struct Launched {
    key: LaunchKey,
    window: WindowKey,
    shown: u64,
    target: Option<Location>,
}

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

        let mut main = View::new(&pvs, zoom);
        main.open_path = start
            .as_ref()
            .map(|l| l.source.to_string())
            .unwrap_or_default();
        let mut app = Self {
            pvs,
            main,
            home: start.clone(),
            show_open: start.is_none(),
            keypad_mode: false,
            keypad: None,
            details: None,
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

    pub fn set_keypad(&mut self, on: bool) {
        self.keypad_mode = on;
    }

    /// The view drawn in `window`, unless that window has closed.
    fn view_mut(&mut self, window: WindowKey) -> Option<&mut View> {
        match window {
            WindowKey::Main => Some(&mut self.main),
            WindowKey::Child(_) => None,
        }
    }

    /// Whether a display, or one named by a command action, is still being
    /// worked out.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_loading(&self) -> bool {
        self.main.is_loading()
    }

    /// Run a command action on a thread, so a slow launcher script or file
    /// system doesn't freeze the window, and open the display it names in
    /// `window`. A command already in flight is ignored.
    #[cfg(not(target_arch = "wasm32"))]
    fn launch(
        &mut self,
        ctx: &egui::Context,
        window: WindowKey,
        base: Option<Source>,
        dir: Option<Source>,
        cmd: String,
    ) {
        let tx = self.launched.0.clone();
        let Some(view) = self.view_mut(window) else {
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
                window,
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
        _: WindowKey,
        _: Option<Source>,
        _: Option<Source>,
        cmd: String,
    ) {
        log::warn!("command '{cmd}' is not run in a browser");
    }

    /// Open the displays named by finished command actions, unless the user
    /// has since moved to another display or closed the window.
    fn poll_launched(&mut self) {
        while let Ok(done) = self.launched.1.try_recv() {
            let Some(view) = self.view_mut(done.window) else {
                continue;
            };
            view.launching.retain(|k| *k != done.key);
            match done.target {
                Some(loc) if done.shown == view.shown => view.open(loc),
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
            }
            if let Some(s) = &view.screen {
                ui.add(egui::Label::new(egui::RichText::new(&s.title).strong()).truncate())
                    .on_hover_text(&s.title);
            }
        });
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl BobApp {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.main.poll_loading(ui.ctx());
        self.poll_launched();
        if !self.keypad_mode && ui.input(|i| i.any_touches()) {
            self.keypad_mode = true;
        }
        widgets::touch_targets(ui.ctx(), self.keypad_mode);
        self.chrome.begin(ui.ctx());
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
        let (status, requests) = self.main.draw(ui, &vcx);
        self.chrome.panels = panels + status;

        // A new display (not a reload, which keeps the size the user gave
        // the window) sizes the window to show it at 100% (or the fixed zoom).
        if std::mem::take(&mut self.main.new_display) {
            self.chrome.display_changed();
        }
        let display = self.main.natural_size(ui.ctx().zoom_factor());
        self.chrome.end(ui.ctx(), display);

        for (window, req) in requests {
            self.request(ui.ctx(), window, req);
        }
        self.keypad_window(ui);
        self.details_window(ui);
        self.trend_windows(ui);
    }

    /// Act on a request from a widget in `window`.
    fn request(&mut self, ctx: &egui::Context, window: WindowKey, req: Request) {
        match req {
            Request::Open {
                source,
                macros,
                target,
            } => {
                log::info!("open {source} ({target})");
                if let Some(view) = self.view_mut(window) {
                    view.open(Location { source, macros });
                }
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
                    raise_trend(ctx, id);
                }
            }
            Request::Command {
                base,
                directory,
                command,
            } => self.launch(ctx, window, base, directory, command),
            Request::Close => match self.view_mut(window) {
                Some(view) if !view.back.is_empty() => view.go_back(),
                _ => ctx.send_viewport_cmd_to(viewport(window), egui::ViewportCommand::Close),
            },
            Request::Webpage(url) => open_webpage(&url),
        }
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

/// The native window that `window` is.
fn viewport(window: WindowKey) -> egui::ViewportId {
    match window {
        WindowKey::Main => egui::ViewportId::ROOT,
        WindowKey::Child(id) => id,
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
        use super::{BobApp, Launched, Location, WindowKey};
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        // A double click runs the command once, and counts as loading.
        app.launch(&ctx, WindowKey::Main, None, None, "no-such-launcher".into());
        app.launch(&ctx, WindowKey::Main, None, None, "no-such-launcher".into());
        assert_eq!(app.main.launching.len(), 1);
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
        app.main.launching.push(key.clone());
        let stale = app.main.shown;
        app.main.shown += 1;
        app.launched
            .0
            .send(Launched {
                key,
                window: WindowKey::Main,
                shown: stale,
                target: Some(loc),
            })
            .unwrap();
        app.poll_launched();
        assert!(app.main.current.is_none() && !app.is_loading());
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
