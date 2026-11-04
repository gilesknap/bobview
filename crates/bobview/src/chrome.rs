//! Sizing of bobview's own controls (toolbar, status line, menus)
//! and of the native window.
//!
//! egui sizes everything in points and multiplies by the desktop's scale
//! factor, which on a phone (2.5-3) makes the controls take a large share of
//! a small screen. The display itself is unaffected: it is fitted to the space
//! left over. So the controls get their own scale, set through egui's zoom
//! factor: by default chosen from the screen size in physical pixels, or fixed
//! with `--ui-scale` / `BOBVIEW_UI_SCALE`.

use eframe::egui::{self, Vec2};

/// The toolbar's share of the screen height it should not exceed.
const TOOLBAR_FRACTION: f32 = 0.07;
/// Smallest toolbar button, in physical pixels: a usable touch target.
const MIN_BUTTON_PX: f32 = 40.0;
/// Toolbar width, in points, below which it switches to the compact layout.
pub const FULL_TOOLBAR_WIDTH: f32 = 600.0;
/// Width of the compact toolbar in points; windows are never sized narrower.
pub const COMPACT_TOOLBAR_WIDTH: f32 = 290.0;
/// Largest share of the monitor an automatically sized window may take.
const MAX_MONITOR_FRACTION: f32 = 0.9;
/// The window size before a display is shown, in points at the desktop scale.
pub const DEFAULT_WINDOW: Vec2 = Vec2::new(1024.0, 720.0);

/// The controls' scale relative to the desktop's (egui's zoom factor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiScale {
    /// Chosen from the screen size.
    Auto,
    /// Fixed by the user: 1 is the desktop's own scale.
    Fixed(f32),
}

impl UiScale {
    /// Parse `auto` or a positive factor.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        text.parse::<f32>()
            .ok()
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| Self::Fixed(f.clamp(0.25, 4.0)))
    }

    /// From `BOBVIEW_UI_SCALE`, else automatic.
    pub fn from_env() -> Self {
        std::env::var("BOBVIEW_UI_SCALE")
            .ok()
            .and_then(|v| {
                let scale = Self::parse(&v);
                if scale.is_none() {
                    log::warn!("ignoring BOBVIEW_UI_SCALE={v}: expected a number or auto");
                }
                scale
            })
            .unwrap_or(Self::Auto)
    }

    /// The zoom factor to give egui.
    ///
    /// `native` is the desktop's pixels per point, `screen_px` the height in
    /// physical pixels of the screen (or window when the screen is unknown),
    /// `toolbar` and `button` the toolbar and its buttons' heights in points.
    pub fn zoom_factor(self, native: f32, screen_px: f32, toolbar: f32, button: f32) -> f32 {
        match self {
            Self::Fixed(f) => f,
            Self::Auto => auto_pixels_per_point(native, screen_px, toolbar, button) / native,
        }
    }
}

/// Pixels per point for the controls: the desktop's own scale unless that
/// makes the toolbar more than [`TOOLBAR_FRACTION`] of the screen, but never so
/// small that a button is under [`MIN_BUTTON_PX`] (or that a point is under a
/// pixel), and never larger than the desktop asks for.
fn auto_pixels_per_point(native: f32, screen_px: f32, toolbar: f32, button: f32) -> f32 {
    let native = native.max(0.1);
    let cap = TOOLBAR_FRACTION * screen_px / toolbar.max(1.0);
    let floor = (MIN_BUTTON_PX / button.max(1.0)).max(1.0).min(native);
    cap.min(native).max(floor)
}

/// Whether the zoom factor has moved enough to be worth a relayout.
pub fn changed(old: f32, new: f32) -> bool {
    (old - new).abs() > 0.01 * old.abs().max(1e-3)
}

/// Inner window size, in points, that shows a display of `display` points
/// (already multiplied by its zoom) under `chrome` points of controls,
/// limited to `monitor` points when known.
pub fn window_size(display: Vec2, chrome: f32, monitor: Option<Vec2>) -> Vec2 {
    // On a monitor too small for the display at 100%, shrink the display
    // keeping its shape, so Fit fills the window without a blank band.
    let mut display = display;
    if let Some(m) = monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
        let room = m * MAX_MONITOR_FRACTION - Vec2::new(1.0, chrome.ceil() + 1.0);
        let fit = (room.x / display.x).min(room.y / display.y);
        if fit < 1.0 && fit > 0.0 {
            display *= fit;
        }
    }
    // A pixel either way keeps rounding from adding scroll bars.
    Vec2::new(
        display.x.ceil().max(COMPACT_TOOLBAR_WIDTH) + 1.0,
        display.y.ceil() + chrome.ceil() + 1.0,
    )
}

/// Inner window size, in points, for controls `by` points taller (or shorter
/// when negative) to leave the display the room it has: only the height
/// changes, and growing stops at `monitor` points when known, or where the
/// window already is when it is taller than that.
pub fn resized_for_controls(inner: Vec2, by: f32, monitor: Option<Vec2>) -> Vec2 {
    let tallest = monitor.map_or(f32::INFINITY, |m| m.y.max(inner.y));
    Vec2::new(inner.x, (inner.y + by).min(tallest).max(0.0))
}

/// Whether the window should follow the display's size: not when the user
/// or the window manager has made it full screen or maximised, nor on a
/// screen too small for the default window (a phone), where it fills the
/// screen anyway.
pub fn can_resize(info: &egui::ViewportInfo, zoom_factor: f32) -> bool {
    info.fullscreen != Some(true)
        && info.maximized != Some(true)
        && !small_screen(info, zoom_factor)
}

/// Whether the window's monitor is too small for the default window (a
/// phone).
pub fn small_screen(info: &egui::ViewportInfo, zoom_factor: f32) -> bool {
    // The monitor size is in points at the current zoom; the default window
    // is in points at the desktop scale.
    info.monitor_size
        .map(|m| m * zoom_factor)
        .is_some_and(|m| m.x < DEFAULT_WINDOW.x || m.y < DEFAULT_WINDOW.y)
}

/// Fill a menu (a popup) with `add`, no taller than the window: egui keeps a
/// popup in its window by moving it, so a taller one would be cut off. One
/// that fits looks as it would without this: its scroll area takes no room.
pub fn menu_contents<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let frame = egui::Frame::popup(ui.style()).total_margin().sum().y;
    let room = (ui.ctx().content_rect().height() - frame).max(0.0);
    // A popup has last frame's size to draw in (at first, a default): it may
    // be as tall as there is room for.
    ui.set_max_height(room);
    // A handle shows that there is more, only when there is.
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.dormant_handle_opacity = scroll.active_handle_opacity;
    egui::ScrollArea::vertical()
        .max_height(room)
        .min_scrolled_height(0.0)
        .show(ui, add)
        .inner
}

/// A submenu of the menu `ui` is in, filled as [`menu_contents`] does.
pub fn submenu<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    ui.menu_button(label, |ui| menu_contents(ui, add));
}

/// The controls' scale and the window size, carried between frames.
pub struct Chrome {
    pub scale: UiScale,
    /// Heights in points of the toolbar, one of its buttons, and all the
    /// panels around the display, as last laid out.
    pub toolbar: f32,
    pub button: f32,
    pub panels: f32,
    /// Whether the zoom factor is where `scale` wants it.
    settled: bool,
    /// A display was shown: size the window to it if allowed.
    resize: bool,
    /// Size the window to every display shown, not only the first; the
    /// toolbar's Auto-size toggle.
    pub auto_size: bool,
    /// The window has not been sized yet.
    first: bool,
    /// Keep the window's size for the next display shown: a layout gave it.
    hold: bool,
    /// Size the window to the display next frame, Auto-size or not.
    fit: bool,
}

impl Chrome {
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            toolbar: 32.0,
            button: 26.0,
            panels: 56.0,
            settled: false,
            resize: false,
            auto_size: true,
            first: true,
            hold: false,
            fit: false,
        }
    }

    /// Whether the zoom factor is where the scale wants it.
    pub fn settled(&self) -> bool {
        self.settled
    }

    /// The window has been given its size (by a layout): keep it when the
    /// next display is shown.
    pub fn hold_size(&mut self) {
        self.first = false;
        self.hold = true;
    }

    /// Size the window to the display, as shown at its zoom, Auto-size or
    /// not: the toolbar's zoom readout, which sets 100%.
    pub fn fit_window(&mut self) {
        self.resize = true;
        self.hold = false;
        self.fit = true;
    }

    /// A new display is being shown.
    pub fn display_changed(&mut self) {
        self.resize = true;
    }

    /// Set egui's zoom factor for this frame's controls (it applies from the
    /// next frame).
    pub fn begin(&mut self, ctx: &egui::Context) {
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let ppp = ctx.pixels_per_point();
        let (window, monitor) = ctx.input(|i| (i.content_rect().size(), i.viewport().monitor_size));
        let screen_px = monitor.filter(|m| m.y > 0.0).unwrap_or(window).y * ppp;
        let zoom = self
            .scale
            .zoom_factor(native, screen_px, self.toolbar, self.button);
        self.settled = !changed(ctx.zoom_factor(), zoom);
        if !self.settled {
            ctx.set_zoom_factor(zoom);
        }
    }

    /// Size the window to a newly shown display, `display` points at the zoom
    /// it will be shown at, once the controls have their final size.
    pub fn end(&mut self, ctx: &egui::Context, display: Option<Vec2>) {
        if !self.resize || !self.settled {
            return;
        }
        // A display that failed to load still ends a layout's hold on
        // the size.
        let Some(display) = display else {
            self.resize = false;
            self.hold = false;
            self.fit = false;
            return;
        };
        let (info, inner) = ctx.input(|i| (i.viewport().clone(), i.content_rect().size()));
        // Wait for the window to know its monitor, so as never to size a
        // window larger than the screen.
        if info.monitor_size.is_none() {
            return;
        }
        self.resize = false;
        if std::mem::take(&mut self.hold) {
            return;
        }
        let fit = std::mem::take(&mut self.fit);
        let follow = self.first || self.auto_size || fit;
        self.first = false;
        if !follow || !can_resize(&info, ctx.zoom_factor()) {
            return;
        }
        let size = window_size(display, self.panels, info.monitor_size);
        if (size - inner).length() >= 1.0 {
            log::debug!("size the window from {inner:?} to {size:?} for the display");
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scales() {
        assert_eq!(UiScale::parse("auto"), Some(UiScale::Auto));
        assert_eq!(UiScale::parse(" 0.5 "), Some(UiScale::Fixed(0.5)));
        assert_eq!(UiScale::parse("100"), Some(UiScale::Fixed(4.0)));
        assert_eq!(UiScale::parse("0"), None);
        assert_eq!(UiScale::parse("-1"), None);
        assert_eq!(UiScale::parse("big"), None);
    }

    #[test]
    fn desktop_keeps_its_scale() {
        // 1080p at 1x and 4k at 2x: the toolbar is a few percent of the height.
        assert_eq!(auto_pixels_per_point(1.0, 1080.0, 32.0, 26.0), 1.0);
        assert_eq!(auto_pixels_per_point(2.0, 2160.0, 32.0, 26.0), 2.0);
        // A small screen at 1x is never shrunk below a pixel per point.
        assert_eq!(auto_pixels_per_point(1.0, 300.0, 32.0, 26.0), 1.0);
    }

    #[test]
    fn phone_shrinks_to_a_share_of_the_height() {
        // Portrait 2340 px at 2.75: the toolbar is under 7% already.
        assert_eq!(auto_pixels_per_point(2.75, 2340.0, 32.0, 26.0), 2.75);
        // Landscape 1080 px: 7% of 1080 is 75.6 px for a 32 point toolbar.
        let p = auto_pixels_per_point(2.75, 1080.0, 32.0, 26.0);
        assert!((p * 32.0 - 75.6).abs() < 0.1, "{p}");
        // A lower resolution screen stops at a 40 px touch target.
        let p = auto_pixels_per_point(2.75, 540.0, 32.0, 26.0);
        assert!((p * 26.0 - 40.0).abs() < 0.1, "{p}");
    }

    #[test]
    fn fixed_scale_wins() {
        assert_eq!(
            UiScale::Fixed(0.5).zoom_factor(3.0, 2000.0, 32.0, 26.0),
            0.5
        );
        let auto = UiScale::Auto.zoom_factor(2.75, 1080.0, 32.0, 26.0);
        assert!(auto < 1.0 && auto > 0.5, "{auto}");
    }

    #[test]
    fn windows_fit_the_display_and_the_monitor() {
        let s = window_size(
            Vec2::new(438.0, 80.0),
            60.0,
            Some(Vec2::new(1920.0, 1080.0)),
        );
        assert_eq!(s, Vec2::new(439.0, 141.0));
        // Narrow displays still leave room for the toolbar.
        let s = window_size(Vec2::new(100.0, 80.0), 60.0, None);
        assert_eq!(s.x, COMPACT_TOOLBAR_WIDTH + 1.0);
        // Large ones are limited to most of the monitor, keeping the
        // display's shape so Fit leaves no blank band.
        let monitor = Some(Vec2::new(1920.0, 1080.0));
        let s = window_size(Vec2::new(3000.0, 2000.0), 60.0, monitor);
        assert!(s.y <= 973.0 && (s.x - 1368.0).abs() <= 1.0, "{s:?}");
        // A wide one is limited by width, and its height shrinks with it.
        let s = window_size(Vec2::new(3000.0, 500.0), 60.0, monitor);
        assert!((s.x - 1728.0).abs() <= 1.0, "{s:?}");
        assert!(
            (s.y - (500.0 * 1727.0 / 3000.0 + 61.0)).abs() <= 1.5,
            "{s:?}"
        );
    }

    #[test]
    fn controls_resize_the_window_by_their_height() {
        let inner = Vec2::new(400.0, 300.0);
        let monitor = Some(Vec2::new(1920.0, 1080.0));
        // Shown then hidden: back to where it was, the width untouched.
        let shown = resized_for_controls(inner, 26.5, monitor);
        assert_eq!(shown, Vec2::new(400.0, 326.5));
        assert_eq!(resized_for_controls(shown, -26.5, monitor), inner);
        assert_eq!(
            resized_for_controls(inner, 26.5, None),
            Vec2::new(400.0, 326.5)
        );
        // Stopped by the monitor, however little room is left.
        let near = Vec2::new(400.0, 1070.0);
        assert_eq!(resized_for_controls(near, 26.5, monitor).y, 1080.0);
        // Already taller than the monitor: not cut to it, by growing or
        // shrinking.
        let tall = Vec2::new(400.0, 1200.0);
        assert_eq!(resized_for_controls(tall, 26.5, monitor).y, 1200.0);
        assert_eq!(resized_for_controls(tall, -26.5, monitor).y, 1173.5);
        // Never a negative size.
        assert_eq!(resized_for_controls(Vec2::splat(10.0), -26.5, None).y, 0.0);
    }

    /// A window holding a button with a menu, drawn pass by pass.
    struct MenuWindow {
        ctx: egui::Context,
        height: f32,
        /// The menu's frame, as last drawn.
        menu: Option<egui::Rect>,
    }

    impl MenuWindow {
        fn new(height: f32) -> Self {
            Self {
                ctx: egui::Context::default(),
                height,
                menu: None,
            }
        }

        fn pass(&mut self, events: Vec<egui::Event>, mut fill: impl FnMut(&mut egui::Ui)) {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, self.height));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut menu = None;
            let mut out = self.ctx.run_ui(input, |ui| {
                let button = ui.button("Menu");
                menu = egui::Popup::menu(&button)
                    .show(|ui| fill(ui))
                    .map(|r| r.response.rect);
            });
            out.textures_delta.clear();
            self.menu = menu;
        }

        /// Click the button, and draw the menu until it has settled.
        fn open(&mut self, mut fill: impl FnMut(&mut egui::Ui)) -> egui::Rect {
            self.click(egui::Pos2::new(15.0, 15.0), &mut fill);
            for _ in 0..5 {
                self.pass(Vec::new(), &mut fill);
            }
            self.menu.expect("the menu is open")
        }

        fn click(&mut self, pos: egui::Pos2, fill: &mut impl FnMut(&mut egui::Ui)) {
            let press = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.pass(Vec::new(), &mut *fill);
            self.pass(vec![egui::Event::PointerMoved(pos)], &mut *fill);
            self.pass(vec![press(true)], &mut *fill);
            self.pass(vec![press(false)], &mut *fill);
        }
    }

    /// A menu of everything menus here hold: `rows` of buttons, a checkbox,
    /// a separator and a submenu.
    fn items(ui: &mut egui::Ui, rows: usize) {
        for row in 0..rows {
            ui.button(format!("Item {row}")).clicked();
        }
        ui.separator();
        ui.checkbox(&mut true, "A checkbox with a long label");
        submenu(ui, "A submenu", |ui| {
            ui.button("In the submenu").clicked();
        });
    }

    #[test]
    fn egui_moves_a_menu_into_the_window_and_cuts_off_one_too_tall() {
        // Why menus scroll: this is egui's own behaviour.
        let mut win = MenuWindow::new(200.0);
        let fits = win.open(|ui| items(ui, 3));
        assert!(fits.bottom() <= 200.0, "moved to fit: {fits:?}");
        let mut win = MenuWindow::new(200.0);
        let tall = win.open(|ui| items(ui, 30));
        assert!(tall.bottom() > 400.0, "cut off: {tall:?}");
    }

    #[test]
    fn a_menu_too_tall_for_the_window_scrolls_inside_it() {
        for height in [100.0, 200.0, 500.0] {
            let mut win = MenuWindow::new(height);
            let mut last = None;
            let mut fill = |ui: &mut egui::Ui| {
                menu_contents(ui, |ui| {
                    items(ui, 60);
                    last = Some(ui.button("The last item").rect);
                });
            };
            let menu = win.open(&mut fill);
            assert!(menu.top() >= 0.0 && menu.bottom() <= height, "{menu:?}");
            assert!(menu.height() > height - 20.0, "uses the room: {menu:?}");
            // Below the window, until scrolled to.
            win.pass(Vec::new(), &mut fill);
            drop(fill);
            assert!(last.unwrap().top() > height);
            let mut last = None;
            let mut fill = |ui: &mut egui::Ui| {
                menu_contents(ui, |ui| {
                    items(ui, 60);
                    last = Some(ui.button("The last item").rect);
                });
            };
            let scroll = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -5000.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            };
            let inside = menu.center();
            win.pass(vec![egui::Event::PointerMoved(inside)], &mut fill);
            win.pass(vec![scroll], &mut fill);
            for _ in 0..60 {
                win.pass(Vec::new(), &mut fill);
            }
            drop(fill);
            let last = last.unwrap();
            assert!(last.bottom() <= height, "reachable: {last:?} in {height}");
            assert_eq!(win.menu.unwrap().size(), menu.size());
        }
    }

    #[test]
    fn a_menu_that_fits_looks_as_it_would_without_scrolling() {
        // Short, and taller than an area's first size (400 points), in a
        // window with room for it.
        for rows in [2, 40] {
            let mut plain = MenuWindow::new(1200.0);
            let plain = plain.open(|ui| items(ui, rows));
            let mut capped = MenuWindow::new(1200.0);
            let menu = capped.open(|ui| {
                menu_contents(ui, |ui| items(ui, rows));
            });
            assert_eq!(menu.min, plain.min, "{rows} rows");
            assert_eq!(menu.height(), plain.height(), "{rows} rows");
            // A scroll area rounds its width, by less than a point.
            let narrower = plain.width() - menu.width();
            assert!((0.0..1.0).contains(&narrower), "{menu:?} {plain:?}");
        }
    }

    #[test]
    fn a_menu_follows_the_room_as_the_window_changes() {
        let mut win = MenuWindow::new(200.0);
        let mut fill = |ui: &mut egui::Ui| {
            menu_contents(ui, |ui| items(ui, 40));
        };
        let small = win.open(&mut fill);
        assert!(small.height() <= 200.0);
        win.height = 1200.0;
        for _ in 0..3 {
            win.pass(Vec::new(), &mut fill);
        }
        let big = win.menu.unwrap();
        assert!(big.height() > 800.0, "no longer scrolling: {big:?}");
        win.height = 150.0;
        for _ in 0..3 {
            win.pass(Vec::new(), &mut fill);
        }
        let again = win.menu.unwrap();
        assert!(
            again.height() <= 150.0 && again.bottom() <= 150.0,
            "{again:?}"
        );
    }

    #[test]
    fn a_submenu_scrolls_inside_a_scrolled_menu() {
        use std::cell::Cell;
        let mut win = MenuWindow::new(200.0);
        let (button, sub) = (Cell::new(None), Cell::new(None));
        let mut fill = |ui: &mut egui::Ui| {
            menu_contents(ui, |ui| {
                for row in 0..40 {
                    if row == 2 {
                        let resp = ui.menu_button("A submenu", |ui| {
                            menu_contents(ui, |ui| {
                                for i in 0..40 {
                                    ui.button(format!("Sub {i}")).clicked();
                                }
                                sub.set(Some(ui.clip_rect()));
                            });
                        });
                        button.set(Some(resp.response.rect));
                    } else {
                        ui.button(format!("Item {row}")).clicked();
                    }
                }
            });
        };
        win.open(&mut fill);
        let over = button.get().unwrap().center();
        win.pass(vec![egui::Event::PointerMoved(over)], &mut fill);
        for _ in 0..4 {
            win.pass(Vec::new(), &mut fill);
        }
        let clip = sub.get().expect("the submenu is open");
        assert!(clip.top() >= 0.0 && clip.bottom() <= 200.0, "{clip:?}");
    }

    #[test]
    fn a_combo_box_already_scrolls_inside_a_short_window() {
        // So the layout list is left as egui draws it.
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 120.0));
        let mut rect = None;
        let pos = egui::Pos2::new(15.0, 15.0);
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut last = None;
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(pos)],
            vec![press(true)],
            vec![press(false)],
            vec![],
            vec![],
            vec![],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::ComboBox::from_id_salt("combo")
                    .selected_text("Combo")
                    .show_ui(ui, |ui| {
                        rect = Some(ui.clip_rect());
                        for i in 0..40 {
                            last = Some(ui.button(format!("Item {i}")).rect);
                        }
                    });
            });
            out.textures_delta.clear();
        }
        let clip = rect.expect("the list is open");
        assert!(clip.bottom() <= 120.0, "{clip:?}");
        assert!(last.unwrap().bottom() > 120.0, "and it scrolls");
    }

    #[test]
    fn resizes_only_free_windows_on_big_screens() {
        let mut info = egui::ViewportInfo {
            monitor_size: Some(Vec2::new(1920.0, 1080.0)),
            ..Default::default()
        };
        assert!(can_resize(&info, 1.0));
        info.maximized = Some(true);
        assert!(!can_resize(&info, 1.0));
        info.maximized = Some(false);
        info.fullscreen = Some(true);
        assert!(!can_resize(&info, 1.0));
        info.fullscreen = None;
        info.monitor_size = Some(Vec2::new(392.0, 851.0));
        assert!(!can_resize(&info, 1.0));
        // Shrinking the controls does not make a phone a big screen.
        assert!(!can_resize(&info, 0.8));
    }
}
