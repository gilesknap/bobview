//! Sizing of bobview's own controls (toolbar, status line, menus, keypad)
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
pub const FULL_TOOLBAR_WIDTH: f32 = 520.0;
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
    // A pixel either way keeps rounding from adding scroll bars.
    let mut size = Vec2::new(
        display.x.ceil().max(COMPACT_TOOLBAR_WIDTH) + 1.0,
        display.y.ceil() + chrome.ceil() + 1.0,
    );
    if let Some(m) = monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
        size = size.min(m * MAX_MONITOR_FRACTION);
    }
    size
}

/// Whether the window should follow the display's size: not when the user
/// or the window manager has made it full screen or maximised, nor on a
/// screen too small for the default window (a phone), where it fills the
/// screen anyway.
pub fn can_resize(info: &egui::ViewportInfo, zoom_factor: f32) -> bool {
    // The monitor size is in points at the current zoom; the default window
    // is in points at the desktop scale.
    let small = info
        .monitor_size
        .map(|m| m * zoom_factor)
        .is_some_and(|m| m.x < DEFAULT_WINDOW.x || m.y < DEFAULT_WINDOW.y);
    info.fullscreen != Some(true) && info.maximized != Some(true) && !small
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
    /// The size, in physical pixels, the window was last given; `None` before
    /// the first display, and after the user (or window manager) chose another.
    sized_px: Option<Vec2>,
    /// The window has not been sized yet (only the first display may size it
    /// when `sized_px` is `None`).
    first: bool,
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
            sized_px: None,
            first: true,
        }
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
        let Some(display) = display else {
            self.resize = false;
            return;
        };
        let ppp = ctx.pixels_per_point();
        let (info, inner) = ctx.input(|i| (i.viewport().clone(), i.content_rect().size()));
        // Wait for the window to know its monitor, so as never to size a
        // window larger than the screen.
        if info.monitor_size.is_none() {
            return;
        }
        self.resize = false;
        let follow = match self.sized_px {
            Some(px) => (inner * ppp - px).length() < 4.0,
            None => self.first,
        };
        self.first = false;
        if !follow || !can_resize(&info, ctx.zoom_factor()) {
            self.sized_px = None;
            return;
        }
        let size = window_size(display, self.panels, info.monitor_size);
        if (size - inner).length() >= 1.0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        self.sized_px = Some(size * ppp);
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
        // Large ones are limited to most of the monitor.
        let s = window_size(
            Vec2::new(3000.0, 2000.0),
            60.0,
            Some(Vec2::new(1920.0, 1080.0)),
        );
        assert_eq!(s, Vec2::new(1728.0, 972.0));
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
