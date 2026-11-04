//! Sizing of bobview's own controls (toolbar, status line, menus)
//! and of the native window.
//!
//! egui sizes everything in points and multiplies by the desktop's scale
//! factor, which on a phone (2.5-3) makes the controls take a large share of
//! a small screen. The display itself is unaffected: it is fitted to the space
//! left over. So the controls get their own scale, set through egui's zoom
//! factor: by default chosen from the screen size in physical pixels, or fixed
//! with `--ui-scale` / `BOBVIEW_UI_SCALE`.

use eframe::egui::{self, Rect, Vec2};

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
/// changes, and growing stops where the window, with `border` points of title
/// bar and borders, is as tall as `monitor` points when known, or where it
/// already is when it is taller than that.
pub fn resized_for_controls(inner: Vec2, by: f32, monitor: Option<Vec2>, border: f32) -> Vec2 {
    let tallest = monitor.map_or(f32::INFINITY, |m| (m.y - border).max(inner.y));
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

/// Show `popup` as a menu of `add`, no taller than the window: egui keeps a
/// popup in its window by moving it, so a taller one would be cut off. One
/// that fits looks as it would without this: its scroll area takes no room.
/// A context menu right-clicked again elsewhere moves there rather than
/// closing, deliberately: egui's `CloseOnClick` closed it first.
pub fn menu<R>(
    popup: egui::Popup,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    // Closed by `menu_contents` on a click in it, which isn't on the bar.
    popup
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| menu_contents(ui, add))
}

fn menu_contents<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let margin = egui::Frame::popup(ui.style()).total_margin().sum();
    let window = ui.ctx().content_rect().size();
    let room = (window.y - margin.y).max(0.0);
    // A popup has last frame's size to draw in (at first, a default): it may
    // be as tall as there is room for.
    ui.set_max_height(room);
    // Beside the items, not over them, and only there when there is more.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    let width = ui.spacing().scroll.allocated_width();
    // While there is a bar the rows keep the width they were first laid out
    // at, as egui would, the menu widening by it if the window has room (and
    // a point, as a scroll area rounds its width).
    let id = egui::containers::menu::find_menu_root(ui).id;
    let seen = id.with("menu scrolls");
    let (scrolled, mut rows): (bool, f32) = ui.data(|d| d.get_temp(seen)).unwrap_or_default();
    let sizing = ui.is_sizing_pass();
    let widened = rows + width + 1.0;
    if scrolled && !sizing && widened + margin.x <= window.x {
        ui.set_max_width(widened);
    }
    let out = egui::ScrollArea::vertical()
        .max_height(room)
        .min_scrolled_height(0.0)
        .show(ui, add);
    let inner = out.inner_rect;
    let scrolls = out.content_size.y > inner.height();
    if sizing {
        rows = out.content_size.x;
    }
    ui.data_mut(|d| d.insert_temp(seen, (scrolls, rows)));
    let bar = scrolls
        .then(|| Rect::from_x_y_ranges(inner.right()..=inner.right() + width, inner.y_range()));
    // As egui's `CloseOnClick`, but for that bar. Not the click that opened
    // it, nor while a submenu is open, which then decides.
    let shown = ui.ctx().read_response(id).map(|r| r.rect);
    // Not the latest position: a touch's release has none.
    let click = ui.input(|i| i.pointer.interact_pos().filter(|_| i.pointer.any_click()));
    if let (Some(pos), Some(shown)) = (click, shown)
        && shown.contains(pos)
        && !bar.is_some_and(|bar| bar.contains(pos))
        && egui::containers::menu::MenuState::is_deepest_open_sub_menu(ui.ctx(), id)
    {
        ui.close();
    }
    out.inner
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
        let shown = resized_for_controls(inner, 26.5, monitor, 0.0);
        assert_eq!(shown, Vec2::new(400.0, 326.5));
        assert_eq!(resized_for_controls(shown, -26.5, monitor, 0.0), inner);
        assert_eq!(
            resized_for_controls(inner, 26.5, None, 30.0),
            Vec2::new(400.0, 326.5)
        );
        // Stopped by the monitor, however little room is left.
        let near = Vec2::new(400.0, 1070.0);
        assert_eq!(resized_for_controls(near, 26.5, monitor, 0.0).y, 1080.0);
        // Less the title bar and borders.
        let near = Vec2::new(400.0, 1040.0);
        assert_eq!(resized_for_controls(near, 26.5, monitor, 30.0).y, 1050.0);
        // Already taller than the monitor: not cut to it, by growing or
        // shrinking.
        let tall = Vec2::new(400.0, 1200.0);
        assert_eq!(resized_for_controls(tall, 26.5, monitor, 0.0).y, 1200.0);
        assert_eq!(resized_for_controls(tall, -26.5, monitor, 0.0).y, 1173.5);
        // Never a negative size.
        let tiny = Vec2::splat(10.0);
        assert_eq!(resized_for_controls(tiny, -26.5, None, 0.0).y, 0.0);
    }

    /// A window holding a button with a menu, drawn pass by pass.
    struct MenuWindow {
        ctx: egui::Context,
        width: f32,
        height: f32,
        /// Drawn as egui would, not by `menu`.
        plain: bool,
        /// The menu's frame, as last drawn.
        menu: Option<egui::Rect>,
    }

    impl MenuWindow {
        fn new(height: f32) -> Self {
            Self {
                ctx: egui::Context::default(),
                width: 800.0,
                height,
                plain: false,
                menu: None,
            }
        }

        fn pass(&mut self, events: Vec<egui::Event>, mut fill: impl FnMut(&mut egui::Ui)) {
            let screen =
                egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(self.width, self.height));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let plain = self.plain;
            let mut shown_rect = None;
            let mut out = self.ctx.run_ui(input, |ui| {
                let button = ui.button("Menu");
                let popup = egui::Popup::menu(&button);
                let shown = if plain {
                    popup.show(|ui| fill(ui))
                } else {
                    menu(popup, |ui| fill(ui))
                };
                shown_rect = shown.map(|r| r.response.rect);
            });
            out.textures_delta.clear();
            self.menu = shown_rect;
        }

        /// Click the button, and draw the menu until it has settled.
        fn open(&mut self, mut fill: impl FnMut(&mut egui::Ui)) -> egui::Rect {
            let pos = egui::Pos2::new(15.0, 15.0);
            self.pass(Vec::new(), &mut fill);
            self.pass(vec![egui::Event::PointerMoved(pos)], &mut fill);
            self.pass(vec![press(pos, true)], &mut fill);
            self.pass(vec![press(pos, false)], &mut fill);
            // The scroll bar slides in, and the menu widens by it.
            for _ in 0..30 {
                self.pass(Vec::new(), &mut fill);
            }
            self.menu.expect("the menu is open")
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

    /// The primary button pressed or released at `pos`.
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The wheel turned far enough to reach the end of any list here.
    fn wheel_to_the_end() -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -5000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Press and release the primary button at `pos`, the pointer there
    /// first.
    fn click_at(win: &mut MenuWindow, pos: egui::Pos2, fill: &mut impl FnMut(&mut egui::Ui)) {
        win.pass(vec![egui::Event::PointerMoved(pos)], &mut *fill);
        win.pass(vec![], &mut *fill);
        win.pass(vec![press(pos, true)], &mut *fill);
        win.pass(vec![press(pos, false)], &mut *fill);
        win.pass(vec![], &mut *fill);
    }

    #[test]
    fn egui_moves_a_menu_into_the_window_and_cuts_off_one_too_tall() {
        // Why menus scroll: this is egui's own behaviour.
        let mut win = MenuWindow::new(200.0);
        win.plain = true;
        let fits = win.open(|ui| items(ui, 3));
        assert!(fits.bottom() <= 200.0, "moved to fit: {fits:?}");
        let mut win = MenuWindow::new(200.0);
        win.plain = true;
        let tall = win.open(|ui| items(ui, 30));
        assert!(tall.bottom() > 400.0, "cut off: {tall:?}");
    }

    #[test]
    fn a_menu_too_tall_for_the_window_scrolls_inside_it() {
        use std::cell::Cell;
        for height in [100.0, 200.0, 500.0] {
            let mut win = MenuWindow::new(height);
            let last = Cell::new(None);
            let mut fill = |ui: &mut egui::Ui| {
                items(ui, 60);
                last.set(Some(ui.button("The last item").rect));
            };
            let shown = win.open(&mut fill);
            assert!(shown.top() >= 0.0 && shown.bottom() <= height, "{shown:?}");
            assert!(shown.height() > height - 20.0, "uses the room: {shown:?}");
            // Below the window, until scrolled to.
            win.pass(Vec::new(), &mut fill);
            assert!(last.get().unwrap().top() > height);
            win.pass(vec![egui::Event::PointerMoved(shown.center())], &mut fill);
            win.pass(vec![wheel_to_the_end()], &mut fill);
            for _ in 0..60 {
                win.pass(Vec::new(), &mut fill);
            }
            let last = last.get().unwrap();
            assert!(last.bottom() <= height, "reachable: {last:?} in {height}");
            assert_eq!(win.menu.unwrap().size(), shown.size());
        }
    }

    #[test]
    fn a_click_on_any_part_of_a_scrolling_menus_item_chooses_it() {
        use std::cell::Cell;
        // Each edge of the item in the middle of the menu.
        let item = |x: fn(egui::Rect) -> f32| {
            let mut win = MenuWindow::new(200.0);
            let (rect, hit) = (Cell::new(None), Cell::new(None));
            let mut fill = |ui: &mut egui::Ui| {
                for row in 0..60 {
                    let resp = ui.button(format!("Item number {row}"));
                    if row == 3 {
                        rect.set(Some(resp.rect));
                    }
                    if resp.clicked() {
                        hit.set(Some(row));
                    }
                }
            };
            win.open(&mut fill);
            let r = rect.get().unwrap();
            click_at(&mut win, egui::pos2(x(r), r.center().y), &mut fill);
            (hit.get(), win.menu.is_some(), r)
        };
        for edge in [
            (|r| r.left() + 1.0) as fn(egui::Rect) -> f32,
            |r| r.center().x,
            |r| r.right() - 3.0,
            |r| r.right() - 1.0,
            |r| r.right() - 0.5,
        ] {
            let (hit, open, r) = item(edge);
            assert_eq!(hit, Some(3), "at {} of {r:?}", edge(r));
            assert!(!open, "the menu closes on its choice");
        }
    }

    /// What a menu's contents saw as they were drawn.
    type Seen<T> = std::rc::Rc<std::cell::Cell<Option<T>>>;

    /// A menu too tall for its window: the window, where its first item is,
    /// the row last chosen and what fills it.
    fn scrolled_menu() -> (
        MenuWindow,
        Seen<egui::Rect>,
        Seen<usize>,
        impl FnMut(&mut egui::Ui),
    ) {
        use std::{cell::Cell, rc::Rc};
        let win = MenuWindow::new(200.0);
        let first = Rc::new(Cell::new(None));
        let hit = Rc::new(Cell::new(None));
        let (f, h) = (first.clone(), hit.clone());
        let fill = move |ui: &mut egui::Ui| {
            for row in 0..60 {
                let resp = ui.button(format!("Item number {row}"));
                if row == 0 {
                    f.set(Some(resp.rect));
                }
                if resp.clicked() {
                    h.set(Some(row));
                }
            }
        };
        (win, first, hit, fill)
    }

    #[test]
    fn a_click_on_a_menus_scroll_bar_keeps_it_open() {
        for (name, down) in [("handle", 0.05), ("track", 0.8)] {
            let (mut win, first, hit, mut fill) = scrolled_menu();
            let shown = win.open(&mut fill);
            let y = shown.top() + shown.height() * down;
            let on_bar = egui::pos2(first.get().unwrap().right() + 7.0, y);
            let top = first.get().unwrap().top();
            click_at(&mut win, on_bar, &mut fill);
            assert!(win.menu.is_some(), "{name}: still open");
            assert_eq!(hit.get(), None, "{name}: nothing chosen");
            if name == "track" {
                assert!(first.get().unwrap().top() < top, "the track scrolls");
            }
            // And it still closes on an item, and a click outside.
            let item = shown.center();
            click_at(&mut win, item, &mut fill);
            assert!(hit.get().is_some() && win.menu.is_none(), "{name}");
        }
        let (mut win, _, hit, mut fill) = scrolled_menu();
        let shown = win.open(&mut fill);
        click_at(
            &mut win,
            egui::pos2(shown.right() + 200.0, 100.0),
            &mut fill,
        );
        assert!(win.menu.is_none() && hit.get().is_none());
    }

    #[test]
    fn dragging_a_menus_scroll_bar_scrolls_it() {
        let (mut win, first, hit, mut fill) = scrolled_menu();
        let shown = win.open(&mut fill);
        let top = first.get().unwrap();
        let from = egui::pos2(top.right() + 7.0, shown.top() + 12.0);
        let to = from + Vec2::new(0.0, 100.0);
        win.pass(vec![egui::Event::PointerMoved(from)], &mut fill);
        win.pass(vec![], &mut fill);
        win.pass(vec![press(from, true)], &mut fill);
        win.pass(vec![egui::Event::PointerMoved(to)], &mut fill);
        win.pass(vec![egui::Event::PointerMoved(to)], &mut fill);
        win.pass(vec![press(to, false)], &mut fill);
        win.pass(vec![], &mut fill);
        assert!(first.get().unwrap().top() < top.top() - 100.0, "scrolled");
        assert!(win.menu.is_some() && hit.get().is_none());
    }

    #[test]
    fn a_menu_that_fits_looks_as_it_would_without_scrolling() {
        // Short, and taller than an area's first size (400 points), in a
        // window with room for it.
        for rows in [2, 40] {
            let mut plain = MenuWindow::new(1200.0);
            plain.plain = true;
            let plain = plain.open(|ui| items(ui, rows));
            let mut capped = MenuWindow::new(1200.0);
            let shown = capped.open(|ui| items(ui, rows));
            assert_eq!(shown.min, plain.min, "{rows} rows");
            assert_eq!(shown.height(), plain.height(), "{rows} rows");
            // A scroll area rounds its width, by less than a point.
            let narrower = plain.width() - shown.width();
            assert!((0.0..1.0).contains(&narrower), "{shown:?} {plain:?}");
        }
    }

    #[test]
    fn a_click_in_a_menu_that_fits_closes_it_as_ever() {
        use std::cell::Cell;
        // On an item and on the frame's margin; the click that opens it over
        // its button doesn't count.
        let rect = Cell::new(None);
        let mut fill = |ui: &mut egui::Ui| {
            rect.set(Some(ui.button("An item").rect));
        };
        let mut win = MenuWindow::new(400.0);
        let shown = win.open(&mut fill);
        click_at(&mut win, rect.get().unwrap().center(), &mut fill);
        assert!(win.menu.is_none(), "the item");
        let mut win = MenuWindow::new(400.0);
        let shown2 = win.open(&mut fill);
        assert_eq!(shown, shown2);
        let margin = egui::pos2(shown.right() - 2.0, shown.center().y);
        click_at(&mut win, margin, &mut fill);
        assert!(win.menu.is_none(), "the margin");
        // Opened where the menu lies over the button (moved to fit): the
        // opening click doesn't close it.
        let mut win = MenuWindow::new(60.0);
        win.open(|ui| items(ui, 6));
        assert!(win.menu.is_some(), "open over its button");
    }

    #[test]
    fn a_tap_on_a_menus_item_chooses_it_and_closes_the_menu() {
        use std::cell::Cell;
        // A touch's release also says that the pointer has gone.
        let (rect, hit) = (Cell::new(None), Cell::new(false));
        let mut fill = |ui: &mut egui::Ui| {
            let resp = ui.button("An item");
            rect.set(Some(resp.rect));
            hit.set(hit.get() || resp.clicked());
        };
        let mut win = MenuWindow::new(400.0);
        win.open(&mut fill);
        let pos = rect.get().unwrap().center();
        win.pass(vec![egui::Event::PointerMoved(pos)], &mut fill);
        win.pass(vec![press(pos, true)], &mut fill);
        let release = vec![press(pos, false), egui::Event::PointerGone];
        win.pass(release, &mut fill);
        win.pass(vec![], &mut fill);
        assert!(hit.get(), "chosen");
        assert!(win.menu.is_none(), "and closed");
    }

    /// The height of each row of a menu of `labels`, in a window `width` wide
    /// and `height` tall, and the menu.
    fn rows(width: f32, height: f32, labels: &[String]) -> (Vec<f32>, egui::Rect) {
        use std::cell::RefCell;
        let heights = RefCell::new(Vec::new());
        let mut fill = |ui: &mut egui::Ui| {
            heights.borrow_mut().clear();
            for label in labels {
                heights.borrow_mut().push(ui.button(label).rect.height());
            }
        };
        let mut win = MenuWindow::new(height);
        win.width = width;
        let shown = win.open(&mut fill);
        (heights.into_inner(), shown)
    }

    #[test]
    fn a_scrolling_menus_rows_keep_their_one_line() {
        let labels: Vec<String> = (0..60).map(|i| format!("Item {i}")).collect();
        let (fits, _) = rows(800.0, 1200.0, &labels[..4]);
        let (scrolls, shown) = rows(800.0, 200.0, &labels);
        assert!(scrolls.iter().all(|h| *h == fits[0]), "{scrolls:?}");
        assert!(shown.right() <= 800.0);
        // No wider than the window for it: the labels wrap instead.
        let long: Vec<String> = (0..60).map(|i| format!("A long label {i}")).collect();
        let (_, wide) = rows(800.0, 200.0, &long);
        let (narrow, shown) = rows(wide.width() - 4.0, 200.0, &long);
        assert!(shown.right() <= wide.width() - 4.0, "{shown:?}");
        assert!(narrow.iter().any(|h| *h > fits[0]), "{narrow:?}");
        // A menu that fits wraps as it would without scrolling.
        let window = 60.0;
        let (plain, capped) = (
            {
                let mut win = MenuWindow::new(800.0);
                win.width = window;
                win.plain = true;
                win.open(|ui| {
                    ui.button("A label too long for this narrow window")
                        .clicked();
                })
            },
            {
                let mut win = MenuWindow::new(800.0);
                win.width = window;
                win.open(|ui| {
                    ui.button("A label too long for this narrow window")
                        .clicked();
                })
            },
        );
        assert_eq!(plain.height(), capped.height());
        assert!(plain.height() > 40.0, "it wraps: {plain:?}");
    }

    /// Where a menu of `labels` ends in a window `width` by `height`, as egui
    /// draws it or not: its frame, and the furthest right of its rows.
    fn right_edges(width: f32, height: f32, labels: &[&str], plain: bool) -> (f32, f32) {
        use std::cell::Cell;
        let rows = Cell::new(0.0_f32);
        let mut fill = |ui: &mut egui::Ui| {
            rows.set(0.0);
            for label in labels {
                rows.set(rows.get().max(ui.button(*label).rect.right()));
            }
        };
        let mut win = MenuWindow::new(height);
        win.width = width;
        win.plain = plain;
        let shown = win.open(&mut fill);
        (shown.right(), rows.get())
    }

    #[test]
    fn a_menu_stays_in_a_window_it_would_have_stayed_in() {
        // As a PV's context menu, and as a menu too tall only for wrapping.
        let pv: &[&str] = &[
            "Copy PV name",
            "Copy value",
            "Copy name and value",
            "Copy name, value, alarm and time",
            "Copy \"caget BL01T-MO-SIMC-01:M1.RBV\"",
            "Trend",
            "Trend with rule PVs (3)",
            "Add to \"A trend window\"",
            "PV details…",
        ];
        let generic: &[&str] = &[
            "Bring all windows to front",
            "Close all other windows",
            "Save layout as…",
            "Save as default",
            "Save default on exit",
            "Auto-size window",
            "Show the toolbar",
            "Close this window",
        ];
        for (labels, height) in [(pv, 120.0), (generic, 200.0)] {
            // Narrower than the menu, then with room for it and its bar.
            for width in (100..=420).step_by(4) {
                let width = width as f32;
                let plain = right_edges(width, height, labels, true);
                let (menu, rows) = right_edges(width, height, labels, false);
                let at = format!("{width}x{height}, {} labels", labels.len());
                if plain.0 <= width {
                    assert!(menu <= width, "the menu ends at {menu} in {at}");
                }
                if plain.1 <= width {
                    assert!(rows <= width, "the rows end at {rows} in {at}");
                }
            }
        }
    }

    #[test]
    fn a_menu_too_tall_because_its_rows_wrap_stays_narrow() {
        let labels: Vec<String> = (0..8)
            .map(|i| format!("A label of number {i} which wraps here"))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut wide = MenuWindow::new(200.0);
        let mut tall = MenuWindow::new(200.0);
        tall.width = 172.0;
        tall.plain = true;
        let fill = |ui: &mut egui::Ui| {
            for label in &labels {
                ui.button(*label).clicked();
            }
        };
        // It fits in a wide window; here it wraps, and doesn't fit.
        assert!(wide.open(fill).height() < 200.0);
        assert!(tall.open(fill).height() > 200.0);
        let (plain, _) = right_edges(172.0, 200.0, &labels, true);
        let (menu, rows) = right_edges(172.0, 200.0, &labels, false);
        assert!(plain <= 172.0, "{plain}");
        assert!(menu <= 172.0 && rows <= 172.0, "{menu} {rows}");
    }

    #[test]
    fn a_menu_follows_the_room_as_the_window_changes() {
        let mut win = MenuWindow::new(200.0);
        let mut fill = |ui: &mut egui::Ui| items(ui, 40);
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
    fn a_submenu_scrolls_and_chooses_inside_a_scrolled_menu() {
        use std::cell::Cell;
        let mut win = MenuWindow::new(200.0);
        let (above, below, last, hit) = (
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
            Cell::new(false),
        );
        let mut fill = |ui: &mut egui::Ui| {
            above.set(Some(ui.button("Above").rect));
            submenu(ui, "A submenu", |ui| {
                for i in 0..40 {
                    ui.button(format!("Sub {i}")).clicked();
                }
                let resp = ui.button("The last");
                last.set(Some(resp.rect));
                hit.set(hit.get() || resp.clicked());
            });
            below.set(Some(ui.button("Below").rect));
            for row in 0..40 {
                ui.button(format!("Item {row}")).clicked();
            }
        };
        win.open(&mut fill);
        let (a, b): (egui::Rect, egui::Rect) = (above.get().unwrap(), below.get().unwrap());
        let over = egui::pos2(a.center().x, (a.center().y + b.center().y) / 2.0);
        win.pass(vec![egui::Event::PointerMoved(over)], &mut fill);
        for _ in 0..30 {
            win.pass(Vec::new(), &mut fill);
        }
        // A click on its button keeps both open.
        click_at(&mut win, over, &mut fill);
        assert!(win.menu.is_some());
        win.pass(vec![egui::Event::PointerMoved(over)], &mut fill);
        for _ in 0..4 {
            win.pass(Vec::new(), &mut fill);
        }
        // Its last item, below the window until scrolled to.
        let r: egui::Rect = last.get().expect("the submenu is open");
        assert!(r.top() > 200.0, "{r:?}");
        let inside = egui::pos2(r.center().x, 100.0);
        win.pass(vec![egui::Event::PointerMoved(inside)], &mut fill);
        win.pass(vec![], &mut fill);
        win.pass(vec![wheel_to_the_end()], &mut fill);
        for _ in 0..60 {
            win.pass(Vec::new(), &mut fill);
        }
        let r: egui::Rect = last.get().unwrap();
        assert!(r.bottom() <= 200.0, "reachable: {r:?}");
        // It chooses, and closes the menu.
        click_at(&mut win, r.center(), &mut fill);
        assert!(hit.get(), "the submenu's item is chosen");
        assert!(win.menu.is_none(), "and the menu closes");
    }

    #[test]
    fn a_combo_box_already_scrolls_inside_a_short_window() {
        // So the layout list is left as egui draws it.
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 120.0));
        let mut last = None;
        let pos = egui::Pos2::new(15.0, 15.0);
        let mut pass = |events| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::ComboBox::from_id_salt("combo")
                    .selected_text("Combo")
                    .show_ui(ui, |ui| {
                        for i in 0..40 {
                            last = Some(ui.button(format!("Item {i}")).rect);
                        }
                    });
            });
            out.textures_delta.clear();
            last
        };
        pass(vec![]);
        pass(vec![egui::Event::PointerMoved(pos)]);
        pass(vec![press(pos, true)]);
        pass(vec![press(pos, false)]);
        for _ in 0..3 {
            pass(vec![]);
        }
        let end: egui::Rect = pass(vec![]).expect("the list is open");
        assert!(end.top() > 120.0, "{end:?}");
        // Its last item reached, in the window.
        let inside = egui::pos2(end.center().x, 60.0);
        pass(vec![egui::Event::PointerMoved(inside)]);
        pass(vec![]);
        pass(vec![wheel_to_the_end()]);
        for _ in 0..60 {
            pass(vec![]);
        }
        let end: egui::Rect = pass(vec![]).unwrap();
        assert!(end.bottom() <= 120.0, "reached: {end:?}");
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
