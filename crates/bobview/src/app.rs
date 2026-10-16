//! The application window: navigation bar, display area and status line.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bob::{Macros, Source};
use eframe::egui::{self, Id, Pos2, Rect, Vec2};
use pv::PvManager;

use crate::chrome::{self, Chrome, UiScale};
use crate::icons::{self, Icon};
use crate::layout::{self, Geometry, Layout, TrendLayout, WindowLayout, Zoom};
use crate::pvmenu;
use crate::style;
use crate::tree::{self, Navigator};
use crate::trend::Trend;
use crate::view::{LaunchKey, Origin, View, ViewCx, WindowKey, window_title};
use crate::widgets::{self, Request};

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub source: Source,
    pub macros: Macros,
}

impl Location {
    /// The same display with the same effective macros, however they were
    /// layered (later definitions win).
    pub(crate) fn same_display(&self, other: &Location) -> bool {
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
    /// Pin mode: a link in a pinned window opens its display in the viewer
    /// (the unpinned window used last) or a new window; one in an unpinned
    /// window navigates in place.
    main_pinned: bool,
    main_used: Used,
    /// When the main window was pinned, by `clock` (0: from the start).
    main_pinned_at: u64,
    /// Counts the windows' uses, to find the one used last.
    clock: u64,
    /// Displays can have native windows of their own (see `can_multi_window`).
    can_windows: bool,
    /// The layout last loaded or saved, which Save saves to.
    layout_name: Option<String>,
    /// A layout to load once the controls' scale has settled (and the
    /// monitors are known).
    pending_layout: Option<(Layout, Option<String>)>,
    /// The monitors, in desktop points, when the window system says, and
    /// when they were last asked for (seconds since start).
    monitors: Vec<Rect>,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "a browser has one window")
    )]
    monitors_at: Option<f64>,
    /// Where the main window is on the desktop, to open windows beside it.
    main_frame: Option<Frame>,
    /// The main window's place and size from a layout, and when (input
    /// time) the wait for them began, until it has them or `RESTORE_WAIT`
    /// has passed: a window manager that sized it before moving it may have
    /// cut it to the monitor it was on.
    restoring: Option<(Geometry, Option<f64>)>,
    settings: layout::Settings,
    /// The Save as… dialog, when open.
    save_as: Option<SaveAs>,
    home: Option<Location>,
    show_open: bool,
    /// The tree of the displays the main window's links to, its width as
    /// last drawn (0 when hidden), and whether its toolbar button has just
    /// shown or hidden it.
    tree: Navigator,
    tree_width: f32,
    tree_toggled: bool,
    /// Touch mode: finger-sized controls and no windows of their own
    /// (switched on by the first touch).
    touch: bool,
    /// The PV details dialog, and the window it was asked for in.
    details: Option<(WindowKey, pvmenu::Details)>,
    /// Writes from a display from the web, waiting for the user's say-so,
    /// asked about one at a time (see [`MAX_CONFIRMS`]).
    confirm: VecDeque<Confirm>,
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
    /// Whether Cancel has been given the focus, which happens once, as the
    /// dialog first shows this write.
    focused: bool,
}

/// Writes that may wait for confirmation at once. One click can ask for
/// several (an action list run together), but a display must not be able
/// to pile up writes behind the dialog; more are refused.
const MAX_CONFIRMS: usize = 16;

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

/// A display in a native window of its own, opened by a link or
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
    /// Where it is on the desktop, to open windows beside it.
    frame: Option<Frame>,
    /// The window it was opened from, to be placed beside again once it
    /// has been sized to its display.
    beside: Option<Frame>,
    pinned: bool,
    /// When it was pinned, by the application's clock.
    pinned_at: u64,
    used: Used,
    close: bool,
}

/// Where a window is on the desktop, in desktop points: its outer rectangle,
/// and how much bigger that is than its inside (the title bar and borders).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    outer: Rect,
    border: Vec2,
}

/// Where a window `size` inside (in desktop points) goes beside the window
/// at `origin`, clear of it and of the `pinned` windows (the one pinned last
/// first). The places tried are beside the window pinned last, so a row of
/// small windows grows beside a big one, then beside `origin`, then beside
/// each other pinned window: to its right (top edges level), below it (left
/// edges level), to its left and above it, each slid along that side to
/// stay on the monitor that window is most on.
/// The first that fits on its monitor clear of them all wins; else the one
/// that covers them least, or over `origin` on the side with more room.
fn beside(origin: Frame, pinned: &[Rect], size: Vec2, monitors: &[Rect]) -> Option<Pos2> {
    let monitor = |r: &Rect| {
        monitors
            .iter()
            .max_by(|a, b| overlap(a, r).total_cmp(&overlap(b, r)))
            .copied()
    };
    let o = origin.outer;
    let m = monitor(&o)?;
    let outer = size + origin.border;
    let clear: Vec<Rect> = std::iter::once(o).chain(pinned.iter().copied()).collect();
    let anchors = pinned.first().into_iter().chain(&clear);
    let mut places = Vec::new();
    for a in anchors {
        let Some(m) = monitor(a) else { continue };
        let x = a.min.x.min(m.max.x - outer.x).max(m.min.x);
        let y = a.min.y.min(m.max.y - outer.y).max(m.min.y);
        let sides = [
            Pos2::new(a.max.x, y),
            Pos2::new(x, a.max.y),
            Pos2::new(a.min.x - outer.x, y),
            Pos2::new(x, a.min.y - outer.y),
        ];
        places.extend(
            sides
                .into_iter()
                .filter(|p| m.contains_rect(Rect::from_min_size(*p, outer))),
        );
    }
    let covered = |p: &Pos2| {
        let r = Rect::from_min_size(*p, outer);
        clear.iter().map(|c| overlap(c, &r)).sum::<f32>()
    };
    if let Some(p) = places.iter().find(|p| covered(p) == 0.0) {
        return Some(*p);
    }
    let over = if m.max.x - o.max.x >= o.min.x - m.min.x {
        m.max.x - outer.x
    } else {
        m.min.x
    };
    let y = o.min.y.min(m.max.y - outer.y).max(m.min.y);
    places.push(Pos2::new(over.max(m.min.x), y));
    // The first of those that cover them least.
    places
        .into_iter()
        .min_by(|a, b| covered(a).total_cmp(&covered(b)))
}

/// The monitors and the pinned windows, for placing a display window.
struct Desk {
    monitors: Vec<Rect>,
    pinned: Vec<(WindowKey, Rect)>,
}

impl Desk {
    /// The pinned windows other than `id`.
    fn clear_of(&self, id: egui::ViewportId) -> Vec<Rect> {
        self.pinned
            .iter()
            .filter(|(key, _)| *key != WindowKey::Child(id))
            .map(|(_, r)| *r)
            .collect()
    }
}

/// The area two rectangles share.
fn overlap(a: &Rect, b: &Rect) -> f32 {
    let i = a.intersect(*b);
    if i.is_positive() { i.area() } else { 0.0 }
}

/// When a window was last used (focused, opened or unpinned), by the
/// application's clock, for the viewer to be the unpinned window used last.
#[derive(Clone, Copy, Debug, Default)]
struct Used {
    focused: bool,
    at: u64,
}

impl Used {
    /// Whether the window has the focus: used when it gains it.
    fn focus(&mut self, focused: bool, clock: &mut u64) {
        if focused && !self.focused {
            self.touch(clock);
        }
        self.focused = focused;
    }

    fn touch(&mut self, clock: &mut u64) {
        *clock += 1;
        self.at = *clock;
    }
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
    /// In the viewer, which keeps its history, raised.
    Viewer(WindowKey),
    /// A new window.
    NewWindow,
}

/// Where a link from `origin` goes, whatever its target, since most links
/// are `replace`. Ctrl+click navigates in place, as every link does where
/// displays have no `windows` of their own. Otherwise a display already
/// open with the same macros is raised (`showing`), or shown again in place
/// when that is the link's own window; shift+click opens a new
/// window; a link in an unpinned window navigates in place, and one in a
/// `pinned` window opens in the `viewer`, or a new window when every window
/// is pinned.
fn route(
    windows: bool,
    origin: Origin,
    pinned: bool,
    showing: Option<WindowKey>,
    viewer: Option<WindowKey>,
) -> Route {
    if !windows || origin.ctrl {
        return Route::InPlace;
    }
    match (showing, viewer) {
        // Shown again, with fresh PVs, as a Refresh button wants.
        (Some(window), _) if window == origin.window => Route::InPlace,
        (Some(window), _) => Route::Raise(window),
        _ if origin.shift => Route::NewWindow,
        _ if !pinned => Route::InPlace,
        (None, Some(window)) => Route::Viewer(window),
        (None, None) => Route::NewWindow,
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
            main_pinned: true,
            main_used: Used::default(),
            main_pinned_at: 0,
            clock: 0,
            can_windows: !cfg!(target_arch = "wasm32"),
            layout_name: None,
            pending_layout: None,
            monitors: Vec::new(),
            monitors_at: None,
            main_frame: None,
            restoring: None,
            settings: layout::Settings::load(),
            save_as: None,
            home: start.clone(),
            show_open: start.is_none(),
            tree: Navigator::default(),
            tree_width: 0.0,
            tree_toggled: false,
            touch: false,
            details: None,
            confirm: VecDeque::new(),
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

    /// Where `window` is on the desktop, when the window system says.
    fn frame(&self, window: WindowKey) -> Option<Frame> {
        match window {
            WindowKey::Main => self.main_frame,
            WindowKey::Child(id) => self.windows.iter().find(|w| w.id == id)?.frame,
        }
    }

    /// Where the pinned windows are on the desktop, those the window system
    /// says, the one pinned last first.
    fn pinned_frames(&self) -> Vec<(WindowKey, Rect)> {
        let main = self
            .main_frame
            .filter(|_| self.main_pinned)
            .map(|f| (WindowKey::Main, f.outer, self.main_pinned_at));
        let children = self
            .windows
            .iter()
            .filter(|w| w.pinned && !w.close)
            .filter_map(|w| Some((WindowKey::Child(w.id), w.frame?.outer, w.pinned_at)));
        let mut pinned: Vec<_> = main.into_iter().chain(children).collect();
        pinned.sort_by_key(|(_, _, at)| std::cmp::Reverse(*at));
        pinned.into_iter().map(|(key, r, _)| (key, r)).collect()
    }

    /// The monitors, in desktop points: those the window system lists, else
    /// the main window's.
    fn monitors(&self, ctx: &egui::Context) -> Vec<Rect> {
        if !self.monitors.is_empty() {
            return self.monitors.clone();
        }
        let zoom = ctx.zoom_factor();
        ctx.input(|i| i.viewport().monitor_size)
            .map(|m| vec![Rect::from_min_size(Pos2::ZERO, m * zoom)])
            .unwrap_or_default()
    }

    /// Whether `window` is pinned.
    fn pinned(&self, window: WindowKey) -> bool {
        match window {
            WindowKey::Main => self.main_pinned,
            WindowKey::Child(id) => self.windows.iter().any(|w| w.id == id && w.pinned),
        }
    }

    /// The viewer: the unpinned window used last, if any.
    fn viewer(&self) -> Option<WindowKey> {
        let main = (!self.main_pinned).then_some((WindowKey::Main, self.main_used.at));
        let children = self
            .windows
            .iter()
            .filter(|w| !w.close && !w.pinned)
            .map(|w| (WindowKey::Child(w.id), w.used.at));
        // Of those used at once, the newest.
        main.into_iter()
            .chain(children)
            .max_by_key(|(_, at)| *at)
            .map(|(window, _)| window)
    }

    /// The window showing `loc` (the same display and macros, see
    /// `View::shows`), if any.
    fn showing(&self, loc: &Location) -> Option<WindowKey> {
        if self.main.shows(loc) {
            return Some(WindowKey::Main);
        }
        self.windows
            .iter()
            .find(|w| !w.close && w.view.shows(loc))
            .map(|w| WindowKey::Child(w.id))
    }

    /// Show `loc`, from a link in `origin`'s window: in place, in the
    /// viewer or in a window of its own (see `route`).
    fn follow(&mut self, ctx: &egui::Context, origin: Origin, loc: Location, target: &str) {
        let pinned = self.pinned(origin.window);
        // The link's own window first: another may show the display too.
        let own = self.view_mut(origin.window).is_some_and(|v| v.shows(&loc));
        let showing = if own {
            Some(origin.window)
        } else {
            self.showing(&loc)
        };
        let viewer = self.viewer();
        match route(self.can_windows, origin, pinned, showing, viewer) {
            // The display it shows: read again, as a Refresh button wants,
            // without adding to Back.
            Route::InPlace if own => {
                log::info!("reload {} ({target})", loc.source);
                if let Some(view) = self.view_mut(origin.window) {
                    view.reload();
                }
            }
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
            Route::Viewer(window) => {
                log::info!("open {} (viewer)", loc.source);
                if let Some(view) = self.view_mut(window) {
                    view.open(loc);
                }
                // Sized to its new display and placed beside the link's
                // window again, as a new window is (not the main window).
                let frame = self.frame(origin.window).filter(|_| !on_wayland());
                if let Some(w) = self
                    .windows
                    .iter_mut()
                    .find(|w| WindowKey::Child(w.id) == window)
                {
                    w.fit_next = true;
                    w.beside = frame;
                }
                raise(ctx, window);
            }
            Route::NewWindow => {
                log::info!("open {} (new window)", loc.source);
                let window = self.open_window(ctx, loc, Geometry::default(), false, false);
                self.place_beside(ctx, window, origin.window);
            }
        }
    }

    /// Open `loc` in a new display window, at `geometry` when known, else
    /// sized to the display at 100% (within the monitor). Unpinned, it is
    /// the viewer, as the window used last.
    fn open_window(
        &mut self,
        ctx: &egui::Context,
        loc: Location,
        geometry: Geometry,
        toolbar: bool,
        pinned: bool,
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
        let mut used = Used::default();
        used.touch(&mut self.clock);
        // From a layout, pinned in the order it lists them.
        let pinned_at = if pinned { used.at } else { 0 };
        self.windows.push(DisplayWindow {
            id,
            view,
            builder,
            fit_next: geometry.size.is_none(),
            toolbar,
            geometry,
            frame: None,
            beside: None,
            pinned,
            pinned_at,
            used,
            close: false,
        });
        WindowKey::Child(id)
    }

    /// Place the new display `window` beside the `origin` window it was
    /// opened from, and again once it has been sized to its display. Not on
    /// Wayland, where the compositor places windows.
    fn place_beside(&mut self, ctx: &egui::Context, window: WindowKey, origin: WindowKey) {
        let Some(frame) = self.frame(origin).filter(|_| !on_wayland()) else {
            return;
        };
        let monitors = self.monitors(ctx);
        let pinned: Vec<Rect> = self
            .pinned_frames()
            .into_iter()
            .filter(|(key, _)| *key != window)
            .map(|(_, r)| r)
            .collect();
        let zoom = ctx.zoom_factor();
        let Some(w) = self
            .windows
            .iter_mut()
            .find(|w| WindowKey::Child(w.id) == window)
        else {
            return;
        };
        let size = w
            .builder
            .inner_size
            .unwrap_or(chrome::DEFAULT_WINDOW / zoom)
            * zoom;
        if let Some(pos) = beside(frame, &pinned, size, &monitors) {
            w.builder = w
                .builder
                .clone()
                .with_position(Pos2::ZERO + pos.to_vec2() / zoom);
        }
        w.beside = Some(frame);
    }

    /// Whether a display, or one named by a command action, is still being
    /// worked out.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_loading(&self) -> bool {
        self.main.is_loading()
            || self.windows.iter().any(|w| w.view.is_loading())
            || crate::screen::building()
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
        // One row whatever the width, so a window sized to the display shows
        // it near 100%: navigation, zoom and the panels on the left, the
        // display's name in the middle (elided when there is no room), the
        // window's pin, layouts and a menu of the rest on the right. A narrow
        // window (a phone) keeps Back, Home, the zoom and Fit, and moves the
        // rest into the menu.
        let compact = ui.available_width() < chrome::FULL_TOOLBAR_WIDTH;
        let windows = self.can_windows;
        let was_pinned = self.main_pinned;
        let mut action = None;
        let mut actual_size = false;
        ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        // Every item is as tall as a framed group (single buttons in an
        // invisible frame), all along the top of the row, so they line up.
        let button = icons::height(ui);
        ui.spacing_mut().interact_size.y = button;
        let row = Vec2::new(ui.available_width(), icons::group_height(ui));
        let along = egui::Layout::left_to_right(egui::Align::Min);
        ui.allocate_ui_with_layout(row, along, |ui| {
            let view = &mut self.main;
            icons::group(ui, true, |ui| {
                let back = icons::button(ui, Icon::Back, false, !view.back.is_empty())
                    .on_hover_text("Back (Alt+←)");
                self.chrome.button = back.rect.height();
                if back.clicked() {
                    view.go_back();
                }
                if !compact
                    && icons::button(ui, Icon::Forward, false, !view.forward.is_empty())
                        .on_hover_text("Forward (Alt+→)")
                        .clicked()
                {
                    view.go_forward();
                }
                if icons::button(ui, Icon::Home, false, self.home.is_some())
                    .on_hover_text("Home")
                    .clicked()
                    && let Some(home) = self.home.clone()
                {
                    view.open(home);
                }
                if !compact
                    && icons::button(ui, Icon::Reload, false, true)
                        .on_hover_text("Reload (Ctrl+R)")
                        .clicked()
                {
                    view.reload();
                }
            });
            icons::group(ui, true, |ui| {
                if !compact
                    && icons::button(ui, Icon::ZoomOut, false, true)
                        .on_hover_text("Zoom out")
                        .clicked()
                {
                    view.zoom_by(1.0 / 1.25);
                }
                let readout = egui::Button::new(format!("{:.0}%", view.zoom * 100.0))
                    .min_size(Vec2::new(56.0, button));
                actual_size = ui
                    .add(readout)
                    .on_hover_text("Actual size: 100%, and the window sized to fit the display")
                    .clicked();
                if !compact
                    && icons::button(ui, Icon::ZoomIn, false, true)
                        .on_hover_text("Zoom in")
                        .clicked()
                {
                    view.zoom_by(1.25);
                }
                if ui
                    .add(egui::Button::selectable(view.fit, "Fit").min_size(Vec2::new(0.0, button)))
                    .on_hover_text("Fit the display to the window")
                    .clicked()
                {
                    view.fit = !view.fit;
                }
            });
            if !compact {
                icons::group(ui, false, |ui| {
                    if icons::button(ui, Icon::Navigator, self.tree.open, true)
                        .on_hover_text(NAVIGATOR_HELP)
                        .clicked()
                    {
                        self.tree.open = !self.tree.open;
                        self.tree_toggled = true;
                    }
                    if icons::button(ui, Icon::Open, self.show_open, true)
                        .on_hover_text("Open a display file")
                        .clicked()
                    {
                        self.show_open = !self.show_open;
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let menu = icons::group(ui, false, |ui| icons::button(ui, Icon::Menu, false, true))
                    .on_hover_text("More");
                egui::Popup::menu(&menu).show(|ui| {
                    if compact {
                        if ui
                            .add_enabled(!view.forward.is_empty(), egui::Button::new("Forward"))
                            .clicked()
                        {
                            view.go_forward();
                        }
                        if ui.button("Reload").clicked() {
                            view.reload();
                        }
                        if ui.button("Zoom in").clicked() {
                            view.zoom_by(1.25);
                        }
                        if ui.button("Zoom out").clicked() {
                            view.zoom_by(1.0 / 1.25);
                        }
                        ui.separator();
                        if ui.checkbox(&mut self.tree.open, "Navigator").changed() {
                            self.tree_toggled = true;
                        }
                        ui.checkbox(&mut self.show_open, "Open…");
                    }
                    let mut auto_size = self.chrome.auto_size;
                    if ui
                        .checkbox(&mut auto_size, "Auto-size window")
                        .on_hover_text("Resize the window to fit each display opened")
                        .changed()
                    {
                        toggle_auto_size(&mut self.chrome);
                    }
                    if windows {
                        ui.separator();
                        if compact {
                            ui.checkbox(&mut self.main_pinned, "Pinned")
                                .on_hover_text(PIN_HELP);
                            ui.menu_button("Load layout", |ui| {
                                action =
                                    layout_list(ui, self.layout_name.as_deref(), &mut self.layouts);
                            });
                        }
                        action = layout_menu(
                            ui,
                            self.layout_name.as_deref(),
                            &mut self.settings,
                            &mut self.layouts,
                        )
                        .or(action.take());
                    }
                });
                if windows && !compact {
                    icons::group(ui, false, |ui| {
                        egui::ComboBox::from_id_salt("layout")
                            .selected_text(self.layout_name.as_deref().unwrap_or("Layout"))
                            .show_ui(ui, |ui| {
                                action =
                                    layout_list(ui, self.layout_name.as_deref(), &mut self.layouts);
                            })
                            .response
                            .on_hover_text("Load a saved layout");
                    });
                    let pin = icons::group(ui, false, |ui| {
                        icons::button(ui, Icon::Pin, self.main_pinned, true)
                    });
                    if pin.on_hover_text(PIN_HELP).clicked() {
                        self.main_pinned = !self.main_pinned;
                    }
                }
                if let Some(s) = &view.screen {
                    // The rest of the row, the name centred in it.
                    let rest = Vec2::new(ui.available_width(), icons::group_height(ui));
                    let centred =
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight);
                    ui.allocate_ui_with_layout(rest, centred, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&s.title).strong()).truncate())
                            .on_hover_text(&s.title);
                    });
                }
            });
        });
        // Actual size: 100%, and the window (with the navigator) sized to it.
        if actual_size {
            self.main.fit = false;
            self.main.zoom = 1.0;
            self.chrome.fit_window();
        }
        if was_pinned && !self.main_pinned {
            self.main_used.touch(&mut self.clock);
        }
        if !was_pinned && self.main_pinned {
            self.clock += 1;
            self.main_pinned_at = self.clock;
        }
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
        if let Some(loc) = self.main.saved_location() {
            let mut w = WindowLayout::new(loc, self.geometry);
            w.main = true;
            w.pinned = Some(self.main_pinned);
            w.tree = self.tree.open;
            w.zoom = Some(Zoom::new(self.main.fit, self.main.zoom));
            windows.push(w);
        }
        for dw in self.windows.iter().filter(|w| !w.close) {
            if let Some(loc) = dw.view.saved_location() {
                let mut w = WindowLayout::new(loc, dw.geometry);
                w.toolbar = dw.toolbar;
                w.pinned = Some(dw.pinned);
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
        Layout::new(windows, trends)
    }

    fn save_layout(&mut self, name: &str) -> Result<(), String> {
        layout::save(name, &self.layout())?;
        self.layouts.refresh();
        self.layout_name = Some(name.trim().to_string());
        Ok(())
    }

    /// Once the main window is where a layout put it, ask again for the
    /// layout's size if it hasn't that: a window manager may have cut it to
    /// fit the monitor the window started on.
    fn finish_restoring(&mut self, ctx: &egui::Context) {
        let Some((want, since)) = self.restoring else {
            return;
        };
        // One that never gets there (kept on screen, fullscreen, tiled) is
        // given up, so a later move across that place doesn't resize it.
        // The wait begins on the first frame after the layout was applied,
        // not in it: that frame loads every display, which may take a while.
        let time = ctx.input(|i| i.time);
        let since = since.unwrap_or(time);
        self.restoring = Some((want, Some(since)));
        if time - since > RESTORE_WAIT {
            self.restoring = None;
            return;
        }
        let (Some(size), Some(now)) = (want.size, self.geometry.size) else {
            return;
        };
        let near = |a: Vec2, b: Vec2| (a - b).length() < 2.0;
        let there = match (want.position, self.geometry.position) {
            (Some(p), Some(q)) => near(p.to_vec2(), q.to_vec2()),
            // The window system doesn't say (Wayland): once it has moved.
            _ => true,
        };
        if !there {
            return;
        }
        self.restoring = None;
        if !near(size, now) {
            log::info!("the main window is {now:?}, not {size:?}: resizing it");
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size / ctx.zoom_factor()));
        }
    }

    /// Close the display windows and open those of `layout`; the main
    /// window moves, takes the layout's size and shows its display.
    fn apply_layout(&mut self, ctx: &egui::Context, layout: Layout, name: Option<String>) {
        // Positions are kept when on a monitor: those the window system
        // lists, else the main window's.
        let zoom = ctx.zoom_factor();
        let monitors = self.monitors(ctx);
        self.windows.clear();
        self.restoring = None;
        for w in &layout.windows {
            let geometry = w.geometry(&monitors);
            if w.position.is_some() && geometry.position.is_none() {
                log::info!("{}: its saved position is off screen", w.display);
            }
            let loc = w.location();
            if !w.main {
                self.open_window(ctx, loc, geometry, w.toolbar, layout.pinned(w));
                continue;
            }
            log::debug!(
                "restore the main window: size {:?}, position {:?}, navigator {}",
                geometry.size,
                geometry.position,
                if w.tree { "shown" } else { "hidden" }
            );
            // Moved before it is sized: a window manager keeps a window
            // within the monitor it is on, and a new window starts on the
            // first, which may be smaller than the one it is going to.
            if let Some(pos) = geometry.position {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                    Pos2::ZERO + pos.to_vec2() / zoom,
                ));
            }
            if let Some(size) = geometry.size {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size / zoom));
                self.chrome.hold_size();
            }
            self.restoring = geometry.size.map(|_| (geometry, None));
            if let Some(z) = &w.zoom {
                self.main.fit = z.factor().is_none();
                self.main.zoom = z.factor().unwrap_or(self.main.zoom);
            }
            if self.home.is_none() {
                self.home = Some(loc.clone());
            }
            self.show_open = false;
            self.main_pinned = layout.pinned(w);
            self.tree.open = w.tree;
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
                chosen = Some(crate::fetch::user_named(path));
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

/// How long (seconds) the main window has to reach a layout's place before
/// `finish_restoring` gives up on it.
const RESTORE_WAIT: f64 = 2.0;

/// What the navigator shows.
const NAVIGATOR_HELP: &str = "Navigator: a tree of the displays the home display links to, \
     and those they link to. Click one to open it as a link would (shift and ctrl too).";

/// What a window's pin does.
const PIN_HELP: &str = "Pinned: links open their displays in the unpinned window used last, \
     or a new one (shift+click: a new window, ctrl+click: in place). \
     Unpinned: links open in place. \
     A display already open in another window is raised.";

/// What the main toolbar's layout controls ask for.
enum LayoutAction {
    Load(String),
    Save(String),
    SaveAs,
    Delete(String),
}

/// The saved layouts, to load one.
fn layout_list(
    ui: &mut egui::Ui,
    current: Option<&str>,
    layouts: &mut LayoutList,
) -> Option<LayoutAction> {
    let names = layouts.get(ui.ctx());
    if names.is_empty() {
        ui.weak("No saved layouts");
    }
    let mut action = None;
    for name in names {
        if ui
            .selectable_label(current == Some(name.as_str()), &name)
            .clicked()
        {
            action = Some(LayoutAction::Load(name));
        }
    }
    action
}

/// The menu's layout items: save, save as, delete, and saving the default
/// on exit.
fn layout_menu(
    ui: &mut egui::Ui,
    current: Option<&str>,
    settings: &mut layout::Settings,
    layouts: &mut LayoutList,
) -> Option<LayoutAction> {
    let mut action = None;
    let save = match current {
        Some(name) => format!("Save layout {name}"),
        None => "Save layout".to_string(),
    };
    if ui
        .add_enabled(current.is_some(), egui::Button::new(save))
        .clicked()
        && let Some(name) = current
    {
        action = Some(LayoutAction::Save(name.to_string()));
    }
    if ui.button("Save layout as…").clicked() {
        action = Some(LayoutAction::SaveAs);
    }
    if ui.button("Save as default").clicked() {
        action = Some(LayoutAction::Save(layout::DEFAULT.to_string()));
    }
    ui.menu_button("Delete layout", |ui| {
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
    if ui
        .checkbox(&mut settings.save_default_on_exit, "Save default on exit")
        .changed()
    {
        settings.save();
    }
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
    /// Run at the start of every pass, including those eframe runs without
    /// `ui` while every window is hidden.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Before the PVs are read, so a later update asks again.
        self.repaint_asked.store(false, Ordering::Relaxed);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // A layout's positions are checked against the monitors as they are
        // when it is loaded; new windows are placed within them, asked for
        // every few seconds (a laptop is docked).
        #[cfg(not(target_arch = "wasm32"))]
        let now = ui.input(|i| i.time);
        #[cfg(not(target_arch = "wasm32"))]
        if (self.pending_layout.is_some() || self.monitors_at.is_none_or(|t| now - t > 3.0))
            && let Some(window) = frame.winit_window()
        {
            self.monitors_at = Some(now);
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
        if !self.settings.save_default_on_exit {
            return;
        }
        let layout = self.layout();
        if let Some(main) = layout.windows.iter().find(|w| w.main) {
            log::debug!(
                "save the default layout: main window size {:?}, position {:?}, navigator {}",
                main.size,
                main.position,
                if main.tree { "shown" } else { "hidden" }
            );
        }
        if let Err(e) = layout::save(layout::DEFAULT, &layout) {
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
        if !self.touch && ui.input(|i| i.any_touches()) {
            self.touch = true;
        }
        widgets::touch_targets(ui.ctx(), self.touch);
        self.can_windows = can_multi_window(&ctx, self.touch);
        self.main_used
            .focus(ui.input(|i| i.focused), &mut self.clock);
        self.chrome.begin(ui.ctx());
        // Before a layout is applied, so the wait for one applied this
        // frame begins on the next, after this frame's loading.
        self.geometry = geometry(ui);
        self.finish_restoring(&ctx);
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
        self.main_frame = frame(ui);
        self.main
            .update_title(ui.ctx(), self.main_pinned && self.can_windows);
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

        let (side, clicked) = self.navigator(ui);
        // Shown or hidden from the toolbar, the navigator widens or narrows
        // the window, so the display keeps its size and zoom.
        let grow = side - std::mem::replace(&mut self.tree_width, side);
        if std::mem::take(&mut self.tree_toggled) && grow != 0.0 {
            self.resize_main_by(&ctx, Vec2::new(grow, 0.0));
        }

        let trends: Vec<(u64, String)> = self.trends.iter().map(|t| (t.id, t.title())).collect();
        let vcx = ViewCx {
            window: WindowKey::Main,
            touch: self.touch,
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
        self.chrome
            .end(ui.ctx(), display.map(|d| d + Vec2::new(side, 0.0)));
        if let Some(clicked) = clicked {
            self.open_from_tree(&ctx, clicked);
        }

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
        if self.details.as_ref().is_some_and(|(w, _)| closed(*w)) {
            self.details = None;
        }
        self.confirm.retain(|c| !closed(c.window));
        dialogs(
            ui.ctx(),
            WindowKey::Main,
            &mut self.details,
            &mut self.confirm,
        );
        self.save_as_window(ui);
        self.trend_windows(ui);
    }

    /// Make the main window `by` points bigger (or smaller), within its
    /// monitor, unless it is maximized or full screen.
    fn resize_main_by(&self, ctx: &egui::Context, by: Vec2) {
        let info = ctx.input(|i| i.viewport().clone());
        if !chrome::can_resize(&info, ctx.zoom_factor()) {
            return;
        }
        let Some(inner) = info.inner_rect else {
            return;
        };
        let mut size = inner.size() + by;
        if let Some(monitor) = info.monitor_size {
            size = size.min(monitor);
        }
        log::debug!(
            "resize the window from {:?} to {size:?} for the navigator",
            inner.size()
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// The navigator beside the main window's display, when shown: its
    /// width, and the node clicked. Its root is the home display.
    fn navigator(&mut self, ui: &mut egui::Ui) -> (f32, Option<tree::Clicked>) {
        let root = self.home.clone().or_else(|| self.main.current.clone());
        let Some(root) = root.filter(|_| self.tree.open) else {
            return (0.0, None);
        };
        // Shown in bold by the same test a click makes (see `View::shows`).
        let (main, windows) = (&self.main, &self.windows);
        let here = |loc: &Location| {
            std::iter::once(main)
                .chain(windows.iter().filter(|w| !w.close).map(|w| &w.view))
                .any(|v| v.shows(loc))
        };
        let panel = egui::Panel::left("navigator")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.tree.ui(ui, &root, &here));
        (panel.response.rect.width(), panel.inner)
    }

    /// Open what a navigator node names, as a link in the main window would;
    /// the display the main window shows is not opened again.
    fn open_from_tree(&mut self, ctx: &egui::Context, clicked: tree::Clicked) {
        let origin = Origin {
            window: WindowKey::Main,
            ctrl: clicked.ctrl,
            shift: clicked.shift,
        };
        match clicked.target {
            // The main window's own display (shown bold) is left as it is,
            // its PVs connected, whatever the modifiers.
            tree::Target::Display(loc) if self.main.shows(&loc) => {}
            tree::Target::Display(loc) => self.follow(ctx, origin, loc, "navigator"),
            tree::Target::Command {
                base,
                directory,
                command,
            } => self.launch(ctx, origin, base, directory, command),
        }
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
            // already this frame, after any asked for before it.
            Request::ConfirmWrite { pv, value } => {
                if self.confirm.len() < MAX_CONFIRMS {
                    self.confirm.push_back(Confirm {
                        window,
                        pv,
                        value,
                        focused: false,
                    });
                } else {
                    log::warn!(
                        "not writing '{value}' to {}: {MAX_CONFIRMS} writes already wait to be confirmed",
                        pv.name()
                    );
                }
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
        let (home, touch, auto_size) = (&self.home, self.touch, self.chrome.auto_size);
        let desk = Desk {
            monitors: self.monitors(&ctx),
            pinned: self.pinned_frames(),
        };
        let (details, confirm) = (&mut self.details, &mut self.confirm);
        let clock = &mut self.clock;
        let can_windows = self.can_windows;
        for w in &mut self.windows {
            let key = WindowKey::Child(w.id);
            let vcx = ViewCx {
                window: key,
                touch,
                trends,
            };
            let title = w.view.screen.as_ref().map_or_else(
                || "bobview".to_string(),
                |s| window_title(&s.title, w.pinned && can_windows),
            );
            if ctx.embed_viewports() {
                let mut open = true;
                egui::Window::new(&title)
                    .id(display_window_id(w.id))
                    .open(&mut open)
                    .collapsible(false)
                    .default_size(Vec2::new(640.0, 420.0))
                    .show(&ctx, |ui| {
                        requests.extend(child_ui(ui, w, home, &vcx, None, clock, &desk));
                    });
                dialogs(&ctx, key, details, confirm);
                w.close |= !open;
                continue;
            }
            let builder = w.builder.clone().with_title(title);
            ctx.show_viewport_immediate(w.id, builder, |ui, class| {
                // No native windows after all: a window in this one.
                let native = (class != egui::ViewportClass::EmbeddedWindow).then_some(auto_size);
                requests.extend(child_ui(ui, w, home, &vcx, native, clock, &desk));
                dialogs(ui.ctx(), key, details, confirm);
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

/// The write confirmation and PV details dialogs, when asked for in
/// `window`: drawn in that window.
fn dialogs(
    ctx: &egui::Context,
    window: WindowKey,
    details: &mut Option<(WindowKey, pvmenu::Details)>,
    confirm: &mut VecDeque<Confirm>,
) {
    if confirm.front().is_some_and(|c| c.window == window) {
        confirm_window(ctx, confirm);
    }
    if details.as_ref().is_some_and(|(w, _)| *w == window) {
        details_window(ctx, details);
    }
}

/// Ask whether to write the first value a display from the web asked for.
fn confirm_window(ctx: &egui::Context, confirm: &mut VecDeque<Confirm>) {
    let waiting = confirm.len().saturating_sub(1);
    let Some(c) = confirm.front_mut() else {
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
            if waiting > 0 {
                ui.label(format!("{waiting} more to ask about after this one"));
            }
            ui.horizontal(|ui| {
                let write = ui.button("Write").clicked();
                let cancel = ui.button("Cancel");
                // Cancel takes the focus from whatever had it (the button
                // that asked for the write, perhaps), so Enter cancels; only
                // a click writes. Once: the user may then tab to Write.
                if !c.focused {
                    cancel.request_focus();
                    c.focused = true;
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
        confirm.pop_front();
        // The next one, if any, is shown next frame.
        ctx.request_repaint();
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

/// Where the window being drawn is on the desktop (see `Frame`); None where
/// the window system doesn't say.
fn frame(ui: &egui::Ui) -> Option<Frame> {
    let zoom = ui.ctx().zoom_factor();
    ui.input(|i| {
        let info = i.viewport();
        let (outer, inner) = (info.outer_rect?, info.inner_rect?);
        Some(Frame {
            outer: Rect::from_min_size(
                Pos2::ZERO + outer.min.to_vec2() * zoom,
                outer.size() * zoom,
            ),
            border: (outer.size() - inner.size()) * zoom,
        })
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
    clock: &mut u64,
    desk: &Desk,
) -> Vec<(Origin, Request)> {
    if native.is_some() {
        w.used.focus(ui.input(|i| i.focused), clock);
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
    corner(ui, w, clock);
    w.geometry = geometry(ui);
    w.frame = frame(ui);
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
        // Taken with the display it was for, sized or not (the window may
        // be maximized, or the display failed), so it can't move the window
        // later.
        let origin = w.beside.take();
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
            // Beside the window it was opened from, at its new size.
            if let Some(origin) = origin
                && let Some(pos) = beside(origin, &desk.clear_of(w.id), size * zoom, &desk.monitors)
            {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                        Pos2::ZERO + pos.to_vec2() / zoom,
                    ));
            }
        }
    }
    drawn.requests
}

/// Small buttons in the top-right corner while the pointer is over the
/// window, drawn over the display so they take no room: Back (while the
/// toolbar is hidden), the pin, and a chevron that shows or hides the
/// toolbar.
fn corner(ui: &egui::Ui, w: &mut DisplayWindow, clock: &mut u64) {
    if !ui.input(|i| i.pointer.has_pointer()) {
        return;
    }
    egui::Area::new(Id::new(("chevron", w.id)))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-4.0, 4.0))
        .show(ui.ctx(), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                if !w.toolbar {
                    let enabled = !w.view.back.is_empty();
                    let resp = corner_button(ui, false, enabled);
                    corner_glyph(ui, &resp, Icon::Back);
                    if resp.on_hover_text("Back (Alt+←)").clicked() {
                        w.view.go_back();
                    }
                }
                let resp = corner_button(ui, w.pinned, true);
                corner_glyph(ui, &resp, Icon::Pin);
                if resp.on_hover_text(PIN_HELP).clicked() {
                    w.pinned = !w.pinned;
                    // Unpinned, it is the viewer; pinned, new windows go
                    // beside it.
                    if w.pinned {
                        *clock += 1;
                        w.pinned_at = *clock;
                    } else {
                        w.used.touch(clock);
                    }
                }
                let resp = corner_button(ui, false, true);
                let visuals = ui.style().interact(&resp);
                // ⌄ to show the toolbar, ⌃ to hide it (drawn: the fonts have no ⌄).
                let c = resp.rect.center();
                let (dx, dy) = (5.0, if w.toolbar { -2.5 } else { 2.5 });
                ui.painter().line(
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
        });
}

/// One of the `corner` buttons, its background drawn: highlighted when
/// `selected`, inert unless `enabled`.
fn corner_button(ui: &mut egui::Ui, selected: bool, enabled: bool) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, 16.0), sense);
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else {
        ui.style().interact(&resp).weak_bg_fill
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    resp
}

/// A `corner` button's icon, faded when the button is disabled.
fn corner_glyph(ui: &egui::Ui, resp: &egui::Response, icon: Icon) {
    let color = if resp.sense.senses_click() {
        ui.style().interact(resp).fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    icon.paint(ui.painter(), resp.rect, 12.0, color);
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
        pv::Scheme::Unsupported => name.pv.clone(),
        _ => {
            let full = name.to_string();
            let prefix = &full[..full.len() - name.pv.len()];
            format!("{prefix}{}", name.base())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Confirm, WindowKey, confirm_window, plain_name};
    use eframe::egui;
    use pv::PvName;
    use std::collections::VecDeque;

    #[test]
    fn writes_are_confirmed_one_at_a_time_and_enter_cancels() {
        let pvs = pv::PvManager::new();
        let pv = pvs.subscribe("loc://confirm_test(0)");
        let ctx = egui::Context::default();
        let mut confirm: VecDeque<Confirm> = ["1", "2"]
            .map(|value| Confirm {
                window: WindowKey::Main,
                pv: pv.clone(),
                value: value.into(),
                focused: false,
            })
            .into();
        // One frame; the display's button, which asked for the writes, has
        // the focus to begin with, as one pressed with the keyboard keeps it.
        let frame = |enter: bool, first: bool, confirm: &mut VecDeque<Confirm>| {
            let mut input = egui::RawInput::default();
            if enter {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let out = ctx.run_ui(input, |ui| {
                let asked = ui.button("Ask");
                if first {
                    asked.request_focus();
                }
                confirm_window(ui.ctx(), confirm);
            });
            out.drop_without_applying_deltas();
        };
        frame(false, true, &mut confirm);
        frame(false, false, &mut confirm);
        assert_eq!(confirm.len(), 2);
        assert!(confirm[0].focused);
        // Enter cancels the first write, not presses the display's button.
        frame(true, false, &mut confirm);
        assert_eq!(confirm.len(), 1);
        assert_eq!(confirm[0].value, "2");
        // The second gets Cancel focused in its turn.
        frame(false, false, &mut confirm);
        frame(false, false, &mut confirm);
        assert!(confirm[0].focused);
        frame(true, false, &mut confirm);
        assert!(confirm.is_empty());
        assert_eq!(pv.state().value.as_f64(), Some(0.0));
    }

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
            ..Origin::default()
        };
        app.launch(&ctx, main, None, None, "no-such-launcher".into());
        app.launch(&ctx, main, None, None, "no-such-launcher".into());
        assert_eq!(app.main.launching.len(), 1);
        // The main view's own loading: app.is_loading() also counts tab
        // builds of other tests running at the same time.
        assert!(app.main.is_loading());
        while app.main.is_loading() {
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
        assert!(app.main.current.is_none() && !app.main.is_loading());
        // In the pinned main window a ctrl+clicked command opens in place,
        // as a ctrl+clicked link does.
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
    fn links_route_by_pin_modifier_and_open_windows() {
        use super::{Origin, Route, WindowKey, route};
        let child = WindowKey::Child(eframe::egui::ViewportId::from_hash_of("x"));
        let viewer = WindowKey::Child(eframe::egui::ViewportId::from_hash_of("v"));
        let plain = Origin::default();
        let ctrl = Origin {
            ctrl: true,
            ..plain
        };
        let shift = Origin {
            shift: true,
            ..plain
        };
        // No windows of their own: always in place, whatever is open.
        assert_eq!(route(false, plain, true, None, None), Route::InPlace);
        assert_eq!(route(false, shift, true, Some(child), None), Route::InPlace);
        // From a pinned window: the viewer, else a new window.
        assert_eq!(
            route(true, plain, true, None, Some(viewer)),
            Route::Viewer(viewer)
        );
        assert_eq!(route(true, plain, true, None, None), Route::NewWindow);
        // From an unpinned window: in place.
        assert_eq!(
            route(true, plain, false, None, Some(viewer)),
            Route::InPlace
        );
        // Shift+click: a new window, whatever the pin.
        assert_eq!(
            route(true, shift, false, None, Some(viewer)),
            Route::NewWindow
        );
        assert_eq!(
            route(true, shift, true, None, Some(viewer)),
            Route::NewWindow
        );
        // A display already open is raised, with or without shift.
        for (origin, pinned) in [(plain, true), (plain, false), (shift, true)] {
            assert_eq!(
                route(true, origin, pinned, Some(child), Some(viewer)),
                Route::Raise(child)
            );
        }
        let from_child = Origin {
            window: child,
            ..plain
        };
        assert_eq!(
            route(true, from_child, true, Some(WindowKey::Main), None),
            Route::Raise(WindowKey::Main)
        );
        // A link to the display its own window shows: shown again in place,
        // as a Refresh button wants.
        for (origin, pinned) in [(plain, true), (plain, false), (shift, true)] {
            assert_eq!(
                route(true, origin, pinned, Some(WindowKey::Main), Some(viewer)),
                Route::InPlace
            );
        }
        assert_eq!(
            route(true, from_child, true, Some(child), None),
            Route::InPlace
        );
        // Ctrl+click navigates in place, even to a display already open.
        assert_eq!(
            route(true, ctrl, true, Some(child), Some(viewer)),
            Route::InPlace
        );
    }

    #[test]
    fn new_windows_open_beside_their_origin_then_below() {
        use super::{Frame, beside};
        use eframe::egui::{Pos2, Rect, Vec2};
        let beside_alone = |o, size, monitors: &[Rect]| beside(o, &[], size, monitors);
        let left = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0));
        let right = Rect::from_min_size(Pos2::new(1920.0, 0.0), Vec2::new(1920.0, 1080.0));
        let at = |x: f32, y: f32, w: f32| Frame {
            outer: Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 600.0)),
            border: Vec2::new(0.0, 30.0),
        };
        let size = Vec2::new(800.0, 500.0);
        // To the right, top edges level.
        assert_eq!(
            beside_alone(at(100.0, 50.0, 600.0), size, &[left]),
            Some(Pos2::new(700.0, 50.0))
        );
        // No room on the right: to the left.
        assert_eq!(
            beside_alone(at(1000.0, 50.0, 600.0), size, &[left]),
            Some(Pos2::new(200.0, 50.0))
        );
        // Room on neither side: on the side with more room (left), over it.
        assert_eq!(
            beside_alone(at(300.0, 50.0, 1400.0), size, &[left]),
            Some(Pos2::new(0.0, 50.0))
        );
        // Kept on the monitor the window is on, and above its bottom.
        assert_eq!(
            beside_alone(at(2100.0, 900.0, 600.0), size, &[left, right]),
            Some(Pos2::new(2700.0, 550.0))
        );
        assert_eq!(
            beside_alone(at(3000.0, 0.0, 600.0), size, &[left, right]),
            Some(Pos2::new(2200.0, 0.0))
        );
        // No room on the right: below, left edges level, slid back onto
        // the monitor when it would go off it; above when only that fits.
        let short = |x: f32, y: f32, w: f32| Frame {
            outer: Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 300.0)),
            border: Vec2::new(0.0, 30.0),
        };
        assert_eq!(
            beside_alone(short(1000.0, 0.0, 600.0), size, &[left]),
            Some(Pos2::new(1000.0, 300.0))
        );
        assert_eq!(
            beside_alone(short(1300.0, 0.0, 600.0), size, &[left]),
            Some(Pos2::new(1120.0, 300.0))
        );
        assert_eq!(
            beside_alone(short(300.0, 600.0, 1400.0), size, &[left]),
            Some(Pos2::new(300.0, 70.0))
        );
        // No monitors known: no place.
        assert_eq!(beside_alone(at(0.0, 0.0, 600.0), size, &[]), None);

        // Beside the window pinned last, clear of the others: a row of
        // small windows grows beside a big one.
        let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1200.0));
        let main = at(0.0, 0.0, 600.0);
        let small = Vec2::new(300.0, 270.0);
        let first = Rect::from_min_size(Pos2::new(600.0, 0.0), Vec2::new(300.0, 300.0));
        assert_eq!(
            beside(main, &[first], small, &[tall]),
            Some(Pos2::new(900.0, 0.0))
        );
        // No room beside it: below it, and failing that beside the others.
        let right_of_main = Rect::from_min_size(Pos2::new(600.0, 0.0), Vec2::new(800.0, 600.0));
        let below_main = Rect::from_min_size(Pos2::new(0.0, 600.0), Vec2::new(600.0, 600.0));
        assert_eq!(
            beside(main, &[right_of_main], size, &[tall]),
            Some(Pos2::new(600.0, 600.0))
        );
        let bottom = Rect::from_min_size(Pos2::new(0.0, 600.0), Vec2::new(1920.0, 600.0));
        assert_eq!(
            beside(main, &[bottom], size, &[tall]),
            Some(Pos2::new(600.0, 0.0))
        );
        assert_eq!(
            beside(main, &[right_of_main, below_main], size, &[tall]),
            Some(Pos2::new(600.0, 600.0))
        );
        // Nowhere clear: where it covers them least.
        let rest = Rect::from_min_size(Pos2::new(600.0, 600.0), Vec2::new(1320.0, 600.0));
        let far = Rect::from_min_size(Pos2::new(1400.0, 0.0), Vec2::new(520.0, 600.0));
        let pos = beside(main, &[right_of_main, below_main, rest, far], size, &[tall]);
        assert!(
            pos.is_some_and(|p| tall.contains_rect(Rect::from_min_size(p, size + main.border)))
        );
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
        let open = |app: &mut BobApp, origin: Origin, loc: Location| {
            let req = Request::Open {
                source: loc.source,
                macros: loc.macros,
                target: "replace".into(),
            };
            app.request(&ctx, origin, req);
        };
        let plain = |window| Origin {
            window,
            ..Origin::default()
        };
        let ctrl = |window| Origin {
            window,
            ctrl: true,
            shift: false,
        };
        let shift = |window| Origin {
            window,
            ctrl: false,
            shift: true,
        };
        let main = WindowKey::Main;
        let current = |app: &BobApp, i: usize| app.windows[i].view.current.clone();
        // Ctrl+click in the pinned main window navigates it.
        open(&mut app, ctrl(main), display("a"));
        assert_eq!(app.main.current, Some(display("a")));

        // A link opens a new window, the viewer, and again raises it.
        open(&mut app, plain(main), display("b"));
        assert_eq!(app.windows.len(), 1);
        let first = WindowKey::Child(app.windows[0].id);
        open(&mut app, plain(main), display("b"));
        assert_eq!(app.windows.len(), 1);
        // The same display with other macros goes to the viewer, which keeps
        // its history.
        let mut other = display("b");
        other.macros = bob::Macros::from_pairs([("P", "c")]);
        open(&mut app, plain(main), other.clone());
        assert_eq!(app.windows.len(), 1);
        assert_eq!(current(&app, 0), Some(other.clone()));
        assert_eq!(app.windows[0].view.back, [display("b")]);
        // Shift+click opens a new window, which becomes the viewer.
        open(&mut app, shift(main), display("d"));
        assert_eq!(app.windows.len(), 2);
        let second = WindowKey::Child(app.windows[1].id);
        open(&mut app, plain(main), display("e"));
        assert_eq!(current(&app, 1), Some(display("e")));
        assert_eq!(current(&app, 0), Some(other));
        // A link in an unpinned window navigates it.
        open(&mut app, plain(first), display("f"));
        assert_eq!(current(&app, 0), Some(display("f")));
        // Pinned, a window is no longer the viewer: the other one is.
        app.windows[1].pinned = true;
        open(&mut app, plain(main), display("g"));
        assert_eq!(current(&app, 0), Some(display("g")));
        // A link in a pinned window goes to the viewer too.
        open(&mut app, plain(second), display("h"));
        assert_eq!(current(&app, 0), Some(display("h")));
        assert_eq!(current(&app, 1), Some(display("e")));
        // Every window pinned: a link opens a new one.
        app.windows[0].pinned = true;
        open(&mut app, plain(main), display("i"));
        assert_eq!(app.windows.len(), 3);
        // Ctrl+click in a pinned window navigates it.
        open(&mut app, ctrl(second), display("j"));
        assert_eq!(current(&app, 1), Some(display("j")));
        // The main window unpinned (in its toolbar) is the viewer, as the
        // window used last.
        app.main_pinned = false;
        app.main_used.touch(&mut app.clock);
        open(&mut app, plain(second), display("k"));
        assert_eq!(app.main.current, Some(display("k")));
        // Where displays have no windows of their own, every link is in place.
        app.can_windows = false;
        open(&mut app, shift(second), display("l"));
        assert_eq!(current(&app, 1), Some(display("l")));
        assert_eq!(app.windows.len(), 3);

        // Exit closes a display window even when it has history.
        app.request(&ctx, plain(second), Request::Close);
        assert!(app.view_mut(second).is_none());
        // The main window still goes back first.
        app.request(&ctx, plain(main), Request::Close);
        assert_eq!(app.main.current, Some(display("a")));
    }

    #[test]
    fn a_link_to_its_own_windows_display_reloads_it() {
        use super::{BobApp, Location, Origin, WindowKey};
        use crate::tree::{Clicked, Target};
        use crate::widgets::Request;
        // Following a link to the display shown reloads it, clearing the
        // fetch cache.
        let _cache = crate::fetch::tests::CACHE_USERS.lock();
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
            let origin = Origin {
                window,
                ctrl,
                shift: false,
            };
            app.request(&ctx, origin, req);
        };
        let main = WindowKey::Main;
        // The main window and a child both show a; the child came from b.
        open(&mut app, main, true, display("a"));
        open(&mut app, main, false, display("b"));
        let child = WindowKey::Child(app.windows[0].id);
        open(&mut app, child, true, display("a"));
        assert_eq!(app.windows[0].view.back, [display("b")]);
        // The child's link to a reloads the child, not raising the main
        // window, and adds nothing to Back however often it is pressed.
        let shown = (app.main.shown, app.windows[0].view.shown);
        open(&mut app, child, false, display("a"));
        open(&mut app, child, false, display("a"));
        assert_eq!(app.windows[0].view.current, Some(display("a")));
        assert_eq!(app.windows[0].view.back, [display("b")]);
        assert_eq!(app.windows[0].view.shown, shown.1 + 2);
        assert_eq!(app.main.shown, shown.0);
        assert_eq!(app.windows.len(), 1);
        // The main window's from its own link too.
        open(&mut app, main, false, display("a"));
        assert!(app.main.back.is_empty());
        assert_eq!(app.main.shown, shown.0 + 1);

        // A navigator click on the main window's display leaves it be.
        for (ctrl, shift) in [(false, false), (true, false), (false, true)] {
            app.open_from_tree(
                &ctx,
                Clicked {
                    target: Target::Display(display("a")),
                    ctrl,
                    shift,
                },
            );
        }
        assert_eq!(app.main.shown, shown.0 + 1);
        assert!(app.main.back.is_empty());
        assert_eq!(app.windows.len(), 1);
    }

    #[test]
    fn a_link_to_its_own_display_counts_the_displays_own_macros() {
        use super::{BobApp, Location, Origin, WindowKey};
        use crate::widgets::Request;
        let dir = std::env::temp_dir().join(format!("bobview-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("own.bob");
        std::fs::write(
            &file,
            r#"<display version="2.0.0"><name>Own</name><macros><X>1</X></macros></display>"#,
        )
        .unwrap();
        let at = |pairs: &[(&str, &str)]| Location {
            source: bob::Source::File(file.clone()),
            macros: bob::Macros::from_pairs(pairs.iter().copied()),
        };
        // Following a link to the display shown reloads it, clearing the
        // fetch cache.
        let _cache = crate::fetch::tests::CACHE_USERS.lock();
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        let open = |app: &mut BobApp, ctrl, loc: Location| {
            let req = Request::Open {
                source: loc.source,
                macros: loc.macros,
                target: "replace".into(),
            };
            let origin = Origin {
                window: WindowKey::Main,
                ctrl,
                shift: false,
            };
            app.request(&ctx, origin, req);
        };
        open(&mut app, true, at(&[("P", "a")]));
        assert!(app.main.screen.is_some());
        let shown = app.main.shown;
        // Its link has the display's own macros too: reloaded in place, in
        // the pinned main window, not opened in a new one.
        open(&mut app, false, at(&[("X", "1"), ("P", "a")]));
        std::fs::remove_dir_all(&dir).ok();
        assert!(app.main.back.is_empty());
        assert_eq!(app.main.shown, shown + 1);
        assert!(app.windows.is_empty());
    }

    #[test]
    fn plain_names_drop_the_ca_prefix_and_type() {
        let t = |n: &str| plain_name(&PvName::parse(n));
        assert_eq!(t("loc://t1<VDouble>(1.5)"), "loc://t1");
        assert_eq!(t("ca://BL01T:X"), "BL01T:X");
        assert_eq!(t("BL01T:X"), "BL01T:X");
        assert_eq!(t("pva://BL01T:Y"), "pva://BL01T:Y");
    }

    /// Draw `frames` frames of `app` in a window of `size` points, as a
    /// window manager would: the window takes the sizes the app asks for.
    fn run(
        ctx: &eframe::egui::Context,
        app: &mut super::BobApp,
        size: &mut eframe::egui::Vec2,
        frames: usize,
    ) {
        use eframe::egui::{self, Pos2, Rect, Vec2};
        let monitor = Vec2::new(1920.0, 1080.0);
        for _ in 0..frames {
            let zoom = ctx.zoom_factor();
            let screen = Rect::from_min_size(Pos2::ZERO, *size / zoom);
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            info.native_pixels_per_point = Some(1.0);
            info.inner_rect = Some(screen);
            info.outer_rect = Some(screen);
            info.monitor_size = Some(monitor / zoom);
            let mut output = ctx.run_ui(input, |ui| app.draw(ui));
            output.textures_delta.clear();
            if let Some(vp) = output.viewport_output.get(&egui::ViewportId::ROOT) {
                for cmd in &vp.commands {
                    if let egui::ViewportCommand::InnerSize(s) = cmd {
                        *size = (*s * zoom).round().min(monitor);
                    }
                }
            }
            // Displays load, and the navigator reads them, on threads.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_saved_navigator_is_restored_with_its_room() {
        use super::{BobApp, Location};
        use crate::chrome::UiScale;
        let display = Location {
            source: bob::Source::File(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/displays/index.bob"),
            ),
            macros: bob::Macros::default(),
        };
        // Open the display: the window takes its size.
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, Some(display), None, UiScale::Fixed(1.0));
        let mut size = crate::chrome::DEFAULT_WINDOW;
        run(&ctx, &mut app, &mut size, 40);
        let alone = size;
        // Show the navigator: the window widens by it.
        app.tree.open = true;
        app.tree_toggled = true;
        run(&ctx, &mut app, &mut size, 20);
        let with_tree = size;
        assert!(with_tree.x > alone.x + 100.0, "{alone:?} -> {with_tree:?}");
        // Saved and restored: the same size, the navigator shown.
        let layout = app.layout();
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, None, UiScale::Fixed(1.0));
        app.restore_layout(layout, Some("default".into()));
        let mut size = crate::chrome::DEFAULT_WINDOW;
        run(&ctx, &mut app, &mut size, 60);
        assert!(app.tree.open);
        assert_eq!(size, with_tree);
    }

    #[test]
    fn a_restored_window_cut_to_its_first_monitor_is_sized_again() {
        use super::BobApp;
        use crate::layout::Geometry;
        use eframe::egui::{Pos2, Vec2};
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, None, crate::chrome::UiScale::Fixed(1.0));
        // The layout's place reached, but the width cut to a 1200 wide
        // monitor the window started on.
        app.restoring = Some((
            Geometry {
                position: Some(Pos2::ZERO),
                size: Some(Vec2::new(1471.0, 900.0)),
            },
            None,
        ));
        let mut size = Vec2::new(1200.0, 900.0);
        run(&ctx, &mut app, &mut size, 3);
        assert_eq!(size, Vec2::new(1471.0, 900.0));
        assert!(app.restoring.is_none());
    }

    #[test]
    fn a_restored_window_that_never_gets_there_is_left_alone() {
        use super::{BobApp, RESTORE_WAIT};
        use crate::layout::Geometry;
        use eframe::egui::{Pos2, Vec2};
        let ctx = eframe::egui::Context::default();
        let mut app = BobApp::new(&ctx, None, None, crate::chrome::UiScale::Fixed(1.0));
        // Kept away from the layout's place (by the window manager, or
        // fullscreen): not sized while it waits, then given up.
        app.restoring = Some((
            Geometry {
                position: Some(Pos2::new(500.0, 300.0)),
                size: Some(Vec2::new(1471.0, 900.0)),
            },
            None,
        ));
        let mut size = Vec2::new(1200.0, 900.0);
        run(&ctx, &mut app, &mut size, 3);
        assert!(app.restoring.is_some());
        // Frames are 1/60 s apart when the input gives no time.
        let frames = (RESTORE_WAIT * 60.0) as usize + 10;
        run(&ctx, &mut app, &mut size, frames);
        assert!(app.restoring.is_none());
        assert_eq!(size, Vec2::new(1200.0, 900.0));
    }
}
