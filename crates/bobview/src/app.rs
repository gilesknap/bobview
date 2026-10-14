//! The application window: navigation bar, display area and status line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bob::Macros;
use eframe::egui::{self, Color32, Id, Vec2};
use pv::PvManager;

use crate::pvmenu;
use crate::screen::{Node, Screen};
use crate::style::{self, View};
use crate::widgets::{self, Cx, Request};

#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
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
    open_path: String,
    /// Use the on-screen keypad for text entries (switched on by the first touch).
    keypad_mode: bool,
    keypad: Option<Keypad>,
    details: Option<pvmenu::Details>,
}

struct Keypad {
    pv: pv::Pv,
    text: String,
}

impl BobApp {
    pub fn new(ctx: &egui::Context, start: Option<Location>, zoom: Option<f32>) -> Self {
        style::install_fonts(ctx);
        ctx.set_theme(egui::Theme::Light);

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
            keypad_mode: false,
            keypad: None,
            details: None,
            open_path: start
                .as_ref()
                .map(|l| l.path.display().to_string())
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
        match Screen::load(&loc.path, &loc.macros, &self.pvs) {
            Ok(screen) => {
                self.screen = Some(screen);
                self.error = None;
            }
            Err(e) => {
                log::error!("{e}");
                self.error = Some(e.to_string());
            }
        }
        self.open_path = loc.path.display().to_string();
        self.current = Some(loc);
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
        // near 100%; the display name is elided when there is no room.
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            if ui
                .add_enabled(!self.back.is_empty(), egui::Button::new("◀"))
                .on_hover_text("Back")
                .clicked()
            {
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
            if ui.button("⟳").on_hover_text("Reload").clicked()
                && let Some(cur) = self.current.clone()
            {
                self.show(cur);
            }
            ui.separator();
            if ui.button("−").clicked() {
                self.fit = false;
                self.zoom = (self.zoom / 1.25).max(0.2);
            }
            if ui.selectable_label(self.fit, "Fit").clicked() {
                self.fit = !self.fit;
            }
            if ui.button("+").clicked() {
                self.fit = false;
                self.zoom = (self.zoom * 1.25).min(5.0);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
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
            if let Some(s) = &self.screen {
                ui.add(egui::Label::new(egui::RichText::new(&s.display.name).strong()).truncate())
                    .on_hover_text(&s.display.name);
            }
        });
    }

    fn open_panel(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.label("File:");
            let edit = ui
                .add(egui::TextEdit::singleline(&mut self.open_path).desired_width(f32::INFINITY));
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                chosen = Some(PathBuf::from(self.open_path.trim()));
            }
        });
        let path = PathBuf::from(self.open_path.trim());
        let dir = if path.is_dir() {
            path
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        };
        egui::ScrollArea::vertical()
            .max_height(200.0)
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
                        chosen = Some(entry);
                    }
                }
            });
        if let Some(path) = chosen {
            self.show_open = false;
            if self.home.is_none() {
                self.home = Some(Location {
                    path: path.clone(),
                    macros: Macros::default(),
                });
            }
            self.open(Location {
                path,
                macros: Macros::default(),
            });
        }
    }

    fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(e) = &self.error {
                ui.colored_label(Color32::RED, e);
            } else if let Some(s) = &self.screen {
                let (connected, total) = count_pvs(&s.nodes);
                ui.label(format!("PVs {connected}/{total} connected"));
                if let Some(cur) = &self.current {
                    ui.weak(cur.path.display().to_string());
                }
            }
        });
    }
}

/// `.bob` files and subdirectories of `dir`, directories first.
fn list_dir(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| (e.path(), e.path().is_dir()))
                .filter(|(p, d)| {
                    let hidden = p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                    !hidden && (*d || p.extension().is_some_and(|x| x == "bob"))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

fn count_pvs(nodes: &[Node]) -> (usize, usize) {
    let mut acc = (0, 0);
    for n in nodes {
        let traces = n.traces.iter().flat_map(|t| t.x.iter().chain(&t.y));
        for pv in n.pv.iter().chain(traces) {
            acc.1 += 1;
            if pv.state().connected {
                acc.0 += 1;
            }
        }
        for (c, t) in std::iter::once(count_pvs(&n.children))
            .chain(n.tabs.iter().map(|t| count_pvs(t)))
            .chain(n.embedded.iter().map(|s| count_pvs(&s.nodes)))
        {
            acc.0 += c;
            acc.1 += t;
        }
    }
    acc
}

impl eframe::App for BobApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl BobApp {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if !self.keypad_mode && ui.input(|i| i.any_touches()) {
            self.keypad_mode = true;
        }
        widgets::touch_targets(ui.ctx(), self.keypad_mode);
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        if self.show_open {
            egui::Panel::top("open").show(ui, |ui| self.open_panel(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status(ui));

        let mut requests = Vec::new();
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
                    self.zoom = fit.clamp(0.2, 5.0);
                }
                // Pinch to zoom on a touch screen (or ctrl+scroll).
                let pinch = ui.input(|i| i.zoom_delta());
                if pinch != 1.0 {
                    self.fit = false;
                    self.zoom = (self.zoom * pinch).clamp(0.2, 5.0);
                }
                let size = Vec2::new(screen.width as f32, screen.height as f32) * self.zoom;
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 0.0, style::color(&screen.display.background));
                    let mut cx = Cx {
                        view: View {
                            origin: rect.min,
                            zoom: self.zoom,
                        },
                        clip: ui.clip_rect().intersect(rect),
                        pvs: &self.pvs,
                        requests: &mut requests,
                        depth: 0,
                        keypad: self.keypad_mode,
                    };
                    widgets::draw_screen(ui, screen, &mut cx, Id::new("display"));
                });
            });

        for req in requests {
            match req {
                Request::Open {
                    path,
                    macros,
                    target,
                } => {
                    log::info!("open {} ({target})", path.display());
                    self.open(Location { path, macros });
                }
                Request::Keypad { pv, text } => self.keypad = Some(Keypad { pv, text }),
                Request::Details(details) => self.details = Some(details),
                Request::Webpage(url) => {
                    if let Err(e) = std::process::Command::new("xdg-open").arg(&url).spawn() {
                        log::warn!("cannot open {url}: {e}");
                    }
                }
            }
        }
        self.keypad_window(ui);
        self.details_window(ui);
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
    fn keypad_title_is_the_plain_name() {
        let t = |n: &str| plain_name(&PvName::parse(n));
        assert_eq!(t("loc://t1<VDouble>(1.5)"), "loc://t1");
        assert_eq!(t("ca://BL01T:X"), "BL01T:X");
        assert_eq!(t("BL01T:X"), "BL01T:X");
        assert_eq!(t("pva://BL01T:Y"), "pva://BL01T:Y");
    }
}
