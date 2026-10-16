//! A loaded display: widgets with resolved positions, macros and PVs.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use bob::{Display, Macros, Source, Widget};
use pv::{Pv, PvManager};

use crate::rules::CompiledRule;

/// How deep embedded displays may nest before we stop (guards against loops).
pub(crate) const MAX_DEPTH: u32 = 8;

/// How long the UI thread waits for a display being built on a thread
/// before showing it as loading: long enough for a local or already
/// fetched display to show at once, short enough not to stall the window.
#[cfg(not(target_arch = "wasm32"))]
pub const QUICK: Duration = Duration::from_millis(50);

/// A display ready to draw. Node rectangles are in this screen's own coordinates.
pub struct Screen {
    pub display: Display,
    pub nodes: Vec<Node>,
    pub width: f64,
    pub height: f64,
    /// The display's name with macros expanded, or else its file name.
    pub title: String,
}

/// One widget instance.
pub struct Node {
    pub w: Widget,
    /// x, y, width, height in screen coordinates.
    pub rect: [f64; 4],
    pub macros: Macros,
    pub pv: Option<Pv>,
    pub pv_name: String,
    pub children: Vec<Node>,
    /// Content of an embedded display or the active navigation tab.
    pub embedded: Option<Box<Screen>>,
    /// Child nodes per tab of a tabs widget.
    pub tabs: Vec<Vec<Node>>,
    pub active_tab: usize,
    pub rules: Vec<CompiledRule>,
    /// Properties before any rule changed them, and the last rule outcomes.
    pub orig_props: Option<bob::Element>,
    pub rule_outcomes: Vec<(String, crate::rules::Outcome)>,
    /// The rule PVs' versions when the rules were last evaluated.
    pub rule_versions: Option<Vec<u64>>,
    /// PVs written by this widget's actions, connected in advance.
    pub action_pvs: Vec<Pv>,
    /// Text being typed into a text entry, while it has focus.
    pub edit: Option<String>,
    /// Problems to show on the widget, such as a missing embedded file.
    pub error: Option<String>,
    /// The embedded display is still being fetched (in a browser), so
    /// should be loaded again once it is.
    pub loading: bool,
    /// The embedded display (or navigation tab) being read and built on a
    /// thread, so the UI thread never waits on a file or server.
    #[cfg(not(target_arch = "wasm32"))]
    pub building: Option<Building>,
    /// The display file or URL this node came from, to resolve relative paths.
    pub base: Option<Source>,
    /// The image widget's texture and the frame it shows.
    pub image: Option<crate::image::ImageCache>,
    /// The symbol or picture widget's texture and index.
    pub symbol: crate::symbol::State,
    /// The x and y PVs of each trace of an XY plot.
    pub traces: Vec<crate::plot::TracePvs>,
    /// The samples of each trace of a strip chart.
    pub series: Vec<Option<crate::trend::Series>>,
    /// The vacuum-space link field that points this widget at a group.
    pub link: Option<crate::group_link::Link>,
}

/// Widget types bobview draws (anything else gets a placeholder).
pub const DRAWN: &[&str] = &[
    "label",
    "textupdate",
    "textentry",
    "led",
    "multi_state_led",
    "byte_monitor",
    "action_button",
    "bool_button",
    "choice",
    "radio",
    "combo",
    "checkbox",
    "slide_button",
    "rectangle",
    "ellipse",
    "arc",
    "polyline",
    "polygon",
    "progressbar",
    "tank",
    "thermometer",
    "linearmeter",
    "meter",
    "scaledslider",
    "scrollbar",
    "spinner",
    "group",
    "tabs",
    "embedded",
    "navtabs",
    "image",
    "symbol",
    "picture",
    "xyplot",
    "stripchart",
];

/// What a display asks for that bobview does not do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compat {
    /// Widgets with Jython/JavaScript scripts (not scripts), which are ignored.
    pub scripts: usize,
    /// Widgets of a type bobview does not draw: a placeholder is shown.
    pub placeholders: usize,
}

impl Compat {
    /// Count over `nodes`, including groups, tabs and embedded displays.
    pub fn of(nodes: &[Node]) -> Self {
        let mut c = Self::default();
        for n in nodes {
            // A script bobview recognises (the vacuum group link) is run.
            if n.w.has_scripts() && n.link.is_none() {
                c.scripts += 1;
            }
            if !DRAWN.contains(&n.w.kind.as_str()) {
                c.placeholders += 1;
            }
            let mut add = |o: Self| {
                c.scripts += o.scripts;
                c.placeholders += o.placeholders;
            };
            add(Self::of(&n.children));
            n.tabs.iter().for_each(|t| add(Self::of(t)));
            if let Some(e) = &n.embedded {
                add(Self::of(&e.nodes));
            }
        }
        c
    }

    /// The status line text, or none when there is nothing to say.
    pub fn text(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.scripts > 0 {
            let s = if self.scripts == 1 {
                "widget"
            } else {
                "widgets"
            };
            parts.push(format!("{} {s} with scripts ignored", self.scripts));
        }
        if self.placeholders > 0 {
            let s = if self.placeholders == 1 {
                "widget"
            } else {
                "widgets"
            };
            parts.push(format!("{} unsupported {s}", self.placeholders));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// An embedded display being built on a thread: where it comes from, and
/// the screen or error once built.
#[cfg(not(target_arch = "wasm32"))]
pub struct Building {
    source: Source,
    rx: Receiver<(Result<Screen, bob::Error>, InFlight)>,
}

/// Embedded displays being built whose results have not been shown yet.
#[cfg(not(target_arch = "wasm32"))]
static IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts an embedded display in [`IN_FLIGHT`] from when its thread starts
/// until its result is shown or thrown away (or the thread panics).
#[cfg(not(target_arch = "wasm32"))]
struct InFlight;

#[cfg(not(target_arch = "wasm32"))]
impl InFlight {
    fn new() -> Self {
        IN_FLIGHT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        InFlight
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Show every embedded display under `nodes` whose thread has built it,
/// drawn or not (a hidden tab's too), so none stays counted by
/// [`building`]; true while any is still being built.
#[cfg(not(target_arch = "wasm32"))]
pub fn poll_built(nodes: &mut [Node]) -> bool {
    let mut any = false;
    for n in nodes {
        any |= n.poll_building(Duration::ZERO);
        any |= poll_built(&mut n.children);
        for tab in &mut n.tabs {
            any |= poll_built(tab);
        }
        if let Some(s) = &mut n.embedded {
            any |= poll_built(&mut s.nodes);
        }
    }
    any
}

/// The error for a display whose build thread panicked.
#[cfg(not(target_arch = "wasm32"))]
pub fn build_failed(source: &Source) -> bob::Error {
    let why = "the build failed (see the log)";
    match source {
        Source::File(path) => bob::Error::Io(path.clone(), std::io::Error::other(why)),
        Source::Url(url) => bob::Error::Fetch(url.clone(), why.into()),
    }
}

/// Whether any embedded display or navigation tab is still being built on
/// a thread, or built but not yet shown, so a snapshot waits for it.
#[cfg(not(target_arch = "wasm32"))]
pub fn building() -> bool {
    IN_FLIGHT.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// Builds screens from displays, loading the displays they embed.
pub struct Builder<'a> {
    pub pvs: &'a PvManager,
    /// Whether this thread may wait on files and servers; on the UI thread
    /// an embedded display is built on a thread of its own instead.
    #[cfg(not(target_arch = "wasm32"))]
    wait: bool,
}

impl<'a> Builder<'a> {
    /// A builder that reads embedded displays as it goes, for a thread that
    /// may wait on them (in a browser nothing waits: see [`crate::fetch`]).
    pub fn new(pvs: &'a PvManager) -> Self {
        Self {
            pvs,
            #[cfg(not(target_arch = "wasm32"))]
            wait: true,
        }
    }

    /// A builder for the UI thread, which builds each embedded display on a
    /// thread of its own, showing it as loading until it is built.
    pub fn ui(pvs: &'a PvManager) -> Self {
        Self {
            pvs,
            #[cfg(not(target_arch = "wasm32"))]
            wait: false,
        }
    }
}

impl Screen {
    /// Load `source` with `macros` layered over the display's own macros.
    pub fn load(source: &Source, macros: &Macros, pvs: &PvManager) -> Result<Self, bob::Error> {
        let display = crate::fetch::load(source)?;
        Ok(Builder::new(pvs).screen(display, macros, None, 0))
    }
}

impl Node {
    /// Expand macros in a property value, falling back to the widget's own
    /// properties, so `$(pv_name)` and `$(name)` work as in Phoebus.
    pub fn expand(&self, text: &str) -> String {
        expand_for(&self.w, &self.macros, text)
    }

    /// Expand macros with `macros` in place of the widget's own.
    pub fn expand_with(&self, macros: &Macros, text: &str) -> String {
        expand_for(&self.w, macros, text)
    }

    /// An expanded simple property.
    pub fn prop(&self, name: &str) -> String {
        self.w
            .text(name)
            .map(|t| self.expand(t))
            .unwrap_or_default()
    }

    pub fn state(&self) -> Option<pv::PvState> {
        self.pv.as_ref().map(|p| p.state())
    }

    /// Resolve a file named by this widget against the display it is in.
    pub fn resolve(&self, file: &str) -> Source {
        crate::fetch::resolve(self.base.as_ref(), file)
    }

    /// Show an embedded display once built from `source`, or why it was not.
    fn place(&mut self, source: &Source, built: Result<Screen, bob::Error>) {
        match built {
            Ok(screen) => {
                // Resize mode 2 sizes the widget to its content.
                if self.w.i64("resize") == Some(2) {
                    self.rect[2] = screen.width;
                    self.rect[3] = screen.height;
                }
                self.error = None;
                self.embedded = Some(Box::new(screen));
            }
            #[cfg(target_arch = "wasm32")]
            Err(bob::Error::Fetch(_, why)) if why == crate::fetch::LOADING => {
                self.loading = true;
                self.error = Some(format!("Loading {source}"));
            }
            Err(e) => {
                log::warn!("{e}");
                self.error = Some(match e {
                    bob::Error::Io(_, e) if e.kind() == std::io::ErrorKind::NotFound => {
                        format!("{source}: not found")
                    }
                    e => e.to_string(),
                });
            }
        }
    }

    /// Show the embedded display being built on a thread if it now is,
    /// waiting up to `wait` for it; true while it is still being built.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn poll_building(&mut self, wait: Duration) -> bool {
        let Some(b) = &self.building else {
            return false;
        };
        let built = match b.rx.recv_timeout(wait) {
            // Shown below, so it stops counting as in flight after this.
            Ok((built, _in_flight)) => built,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return true,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The thread panicked building it.
                Err(build_failed(&b.source))
            }
        };
        let source = b.source.clone();
        self.building = None;
        self.place(&source, built);
        false
    }
}

pub(crate) fn expand_for(w: &Widget, macros: &Macros, text: &str) -> String {
    expand_props(w, macros, text, &Default::default())
}

/// The properties being expanded, and those already expanded.
type Expanding = std::cell::RefCell<(
    Vec<String>,
    std::collections::HashMap<String, Option<String>>,
)>;

/// [`expand_for`], with the properties being expanded and their values so
/// far in `state`. A widget property may name others, as BOY's rule PV
/// `$(pv_name)` does with `$(name):STA` and the name
/// `$(prefix)$(cell)S-VA-PIRG-01`; one that names itself, directly or not,
/// or anything unknown or too long is left as written. Each is expanded
/// once, so properties that name others several times don't take
/// exponential time.
fn expand_props(w: &Widget, macros: &Macros, text: &str, state: &Expanding) -> String {
    bob::expand(text, &|name| {
        macros.get(name).map(str::to_string).or_else(|| {
            if let Some(value) = state.borrow().1.get(name) {
                return value.clone();
            }
            if state.borrow().0.iter().any(|a| a == name) {
                return None;
            }
            let prop = w.text(name)?;
            state.borrow_mut().0.push(name.to_string());
            let value = expand_props(w, macros, prop, state);
            let value = Some(value).filter(|v| !v.contains('$') && v.len() <= 1 << 16);
            let mut state = state.borrow_mut();
            state.0.pop();
            state.1.insert(name.to_string(), value.clone());
            value
        })
    })
}

/// The display's name with `macros` expanded; a display with no name is
/// called after its file, as in BOY and Phoebus.
pub(crate) fn title(display: &Display, macros: &Macros) -> String {
    let name = bob::expand(&display.name, &|n| macros.get(n).map(str::to_string));
    if !name.trim().is_empty() {
        return name;
    }
    let file = match &display.source {
        Some(Source::File(p)) => p.to_string_lossy().into_owned(),
        Some(Source::Url(u)) => u.split(['?', '#']).next().unwrap_or("").to_string(),
        None => String::new(),
    };
    let base = file.rsplit('/').next().unwrap_or("");
    base.rsplit_once('.')
        .map_or(base, |(stem, _)| stem)
        .to_string()
}

/// Phoebus default sizes by widget type.
fn default_size(kind: &str) -> (f64, f64) {
    match kind {
        "led" | "multi_state_led" => (20.0, 20.0),
        "action_button" | "bool_button" | "combo" | "slide_button" => (100.0, 30.0),
        "choice" => (100.0, 43.0),
        "radio" => (100.0, 60.0),
        "byte_monitor" => (160.0, 20.0),
        "ellipse" => (100.0, 50.0),
        "group" => (300.0, 200.0),
        "picture" => (150.0, 100.0),
        "tank" | "thermometer" => (150.0, 200.0),
        "scaledslider" => (400.0, 55.0),
        "tabs" | "embedded" | "navtabs" | "xyplot" | "stripchart" | "image" => (400.0, 300.0),
        "meter" => (240.0, 120.0),
        _ => (100.0, 20.0),
    }
}

/// Offset of a group's children from its top-left corner, by group style.
pub fn group_inset(w: &Widget) -> (f64, f64) {
    let font = w.font("font").unwrap_or_default();
    let inset = 2.0 + (1.2 * font.size as f64).floor();
    match w.i64("style").unwrap_or(0) {
        0 => (inset, inset),
        1 => (1.0, inset + 1.0),
        2 => (1.0, 1.0),
        _ => (0.0, 0.0),
    }
}

/// A fresh `DID_<n>`, BOY's id for an opened display.
fn display_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("DID_{n}")
}

impl Builder<'_> {
    pub fn screen(
        &self,
        display: Display,
        macros: &Macros,
        group: Option<&str>,
        depth: u32,
    ) -> Screen {
        // Macros passed in override the display's own defaults.
        let mut macros = display.macros.with(macros);
        // BOY gives each display it opens a `$(DID)`, which screens use to
        // keep their `loc://$(DID)…` PVs apart. Widgets of an embedded
        // display become the host's, so they share the host's id.
        if display.legacy && (depth == 0 || macros.get("DID").is_none()) {
            macros = macros.with(&Macros::from_pairs([("DID", display_id())]));
        }
        let base = display.source.clone();
        let (widgets, width, height) = match group.filter(|g| !g.is_empty()) {
            Some(name) => match find_group(&display.widgets, name) {
                Some(g) => {
                    let (w, h) = (
                        g.f64("width").unwrap_or(300.0),
                        g.f64("height").unwrap_or(200.0),
                    );
                    (g.children.clone(), w, h)
                }
                None => {
                    log::warn!("group '{name}' not found");
                    (Vec::new(), display.width, display.height)
                }
            },
            None => (display.widgets.clone(), display.width, display.height),
        };
        let nodes = self.nodes(&widgets, (0.0, 0.0), &macros, base.as_ref(), depth);
        // Phoebus shows widgets beyond the declared display size, so grow to
        // fit them. BOY clipped them, and EDM conversions leave stray widgets
        // far outside the display, so an `.opi` keeps its declared size.
        let (width, height) = nodes
            .iter()
            .filter(|n| !display.legacy && n.w.bool("visible") != Some(false))
            .fold((width, height), |(w, h), n| {
                (w.max(n.rect[0] + n.rect[2]), h.max(n.rect[1] + n.rect[3]))
            });
        let title = title(&display, &macros);
        Screen {
            display,
            nodes,
            width,
            height,
            title,
        }
    }

    fn nodes(
        &self,
        widgets: &[Widget],
        origin: (f64, f64),
        macros: &Macros,
        base: Option<&Source>,
        depth: u32,
    ) -> Vec<Node> {
        widgets
            .iter()
            .map(|w| self.node(w, origin, macros, base, depth))
            .collect()
    }

    fn node(
        &self,
        w: &Widget,
        origin: (f64, f64),
        parent: &Macros,
        base: Option<&Source>,
        depth: u32,
    ) -> Node {
        let macros = parent.with(&w.macros());
        let (dw, dh) = default_size(&w.kind);
        let rect = [
            origin.0 + w.f64("x").unwrap_or(0.0),
            origin.1 + w.f64("y").unwrap_or(0.0),
            w.f64("width").unwrap_or(dw),
            w.f64("height").unwrap_or(dh),
        ];
        let pv_name = w
            .text("pv_name")
            .map(|t| expand_for(w, &macros, t))
            .unwrap_or_default();
        let pv = (!pv_name.is_empty()).then(|| self.pvs.subscribe(&pv_name));
        let rules = w
            .rules()
            .iter()
            .map(|r| CompiledRule::new(r, &|t| expand_for(w, &macros, t), self.pvs))
            .collect();

        let link = crate::group_link::Link::new(w, &|t| expand_for(w, &macros, t), self.pvs);
        let action_pvs = w
            .actions()
            .iter()
            .filter_map(|a| match a {
                bob::Action::WritePv { pv_name, .. } => {
                    Some(self.pvs.subscribe(&expand_for(w, &macros, pv_name)))
                }
                _ => None,
            })
            .collect();

        let mut node = Node {
            w: w.clone(),
            rect,
            macros,
            pv,
            pv_name,
            children: Vec::new(),
            embedded: None,
            tabs: Vec::new(),
            active_tab: 0,
            rules,
            orig_props: None,
            rule_outcomes: Vec::new(),
            rule_versions: None,
            action_pvs,
            edit: None,
            error: None,
            loading: false,
            #[cfg(not(target_arch = "wasm32"))]
            building: None,
            base: base.cloned(),
            image: None,
            symbol: Default::default(),
            traces: Vec::new(),
            series: Vec::new(),
            link,
        };

        match w.kind.as_str() {
            "group" => {
                let (ix, iy) = group_inset(w);
                node.children = self.nodes(
                    &w.children,
                    (rect[0] + ix, rect[1] + iy),
                    &node.macros,
                    base,
                    depth,
                );
            }
            "tabs" => {
                let tab_height = w.f64("tab_height").unwrap_or(30.0);
                let horizontal = w.i64("direction").unwrap_or(0) == 0;
                let content = if horizontal {
                    (rect[0], rect[1] + tab_height)
                } else {
                    (rect[0] + 100.0, rect[1])
                };
                node.active_tab = w.i64("active_tab").unwrap_or(0).max(0) as usize;
                node.tabs = tab_elements(w)
                    .iter()
                    .map(|tab| {
                        let children: Vec<Widget> = tab
                            .child("children")
                            .map(|c| {
                                c.children
                                    .iter()
                                    .filter(|e| e.name == "widget")
                                    .map(|e| Widget::from_element(e.clone()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.nodes(&children, content, &node.macros, base, depth)
                    })
                    .collect();
            }
            "embedded" => {
                let file = node.prop("file");
                let group = node.prop("group_name");
                let macros = node.macros.clone();
                self.embed(&mut node, &file, &macros, &group, depth);
            }
            "xyplot" => {
                node.traces =
                    crate::plot::subscribe(w, &|t| expand_for(w, &node.macros, t), self.pvs);
            }
            "stripchart" => {
                node.traces =
                    crate::plot::subscribe(w, &|t| expand_for(w, &node.macros, t), self.pvs);
                node.series = node
                    .traces
                    .iter()
                    .map(|t| t.y.clone().map(crate::trend::Series::new))
                    .collect();
            }
            "navtabs" => {
                node.active_tab = w.i64("active_tab").unwrap_or(0).max(0) as usize;
                let active = node.active_tab;
                self.select_navtab(&mut node, active, depth);
            }
            _ => {}
        }
        node
    }

    /// Load `file` into `node.embedded`, or record why it failed. On the UI
    /// thread ([`Builder::ui`]) the display is read and built on a thread,
    /// and the node shows it as loading until [`Node::poll_building`] finds
    /// it built.
    pub fn embed(&self, node: &mut Node, file: &str, macros: &Macros, group: &str, depth: u32) {
        node.embedded = None;
        node.error = None;
        node.loading = false;
        // Dropping a build still under way discards it when it finishes.
        #[cfg(not(target_arch = "wasm32"))]
        {
            node.building = None;
        }
        if file.is_empty() {
            return;
        }
        if depth >= MAX_DEPTH {
            node.error = Some("embedded displays nested too deeply".into());
            return;
        }
        let source = node.resolve(file);
        #[cfg(not(target_arch = "wasm32"))]
        if !self.wait {
            let (tx, rx) = std::sync::mpsc::channel();
            let (pvs, src) = (self.pvs.clone(), source.clone());
            let (macros, group) = (macros.clone(), group.to_string());
            let in_flight = InFlight::new();
            std::thread::spawn(move || {
                let built = crate::fetch::load(&src)
                    .map(|d| Builder::new(&pvs).screen(d, &macros, Some(&group), depth + 1));
                // Unsent (the node moved on), the result and its count drop here.
                tx.send((built, in_flight)).ok();
            });
            node.error = Some(format!("Loading {source}"));
            node.building = Some(Building { source, rx });
            node.poll_building(QUICK);
            return;
        }
        let built =
            crate::fetch::load(&source).map(|d| self.screen(d, macros, Some(group), depth + 1));
        node.place(&source, built);
    }

    /// Show tab `index` of a navigation tabs widget.
    pub fn select_navtab(&self, node: &mut Node, index: usize, depth: u32) {
        let tabs = tab_elements(&node.w);
        let Some(tab) = tabs.get(index) else { return };
        node.active_tab = index;
        let file = node.expand(tab.child_text("file").unwrap_or(""));
        let group = node.expand(tab.child_text("group_name").unwrap_or(""));
        let macros = node.macros.with(
            &tab.child("macros")
                .map(Macros::from_element)
                .unwrap_or_default(),
        );
        self.embed(node, &file, &macros, &group, depth);
    }
}

/// The `<tab>` elements of a tabs or navtabs widget.
pub fn tab_elements(w: &Widget) -> Vec<bob::Element> {
    w.get("tabs")
        .map(|t| {
            t.children
                .iter()
                .filter(|c| c.name == "tab")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn find_group<'a>(widgets: &'a [Widget], name: &str) -> Option<&'a Widget> {
    widgets.iter().find_map(|w| {
        if w.kind == "group" && w.name() == name {
            Some(w)
        } else {
            find_group(&w.children, name)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_ignored_scripts_and_placeholders() {
        let d = Display::parse(
            r#"<display>
            <widget type="table"/>
            <widget type="label"><scripts><script file="a.py"/></scripts></widget>
            <widget type="label"><scripts/></widget>
            <widget type="group">
              <widget type="webbrowser"/>
              <widget type="led"><scripts><script file="b.js"/></scripts></widget>
            </widget></display>"#,
        )
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let c = Compat::of(&s.nodes);
        assert_eq!(
            c,
            Compat {
                scripts: 2,
                placeholders: 2
            }
        );
        assert_eq!(
            c.text().as_deref(),
            Some("2 widgets with scripts ignored, 2 unsupported widgets")
        );
        assert_eq!(Compat::default().text(), None);
    }

    #[test]
    fn counts_singular_and_through_embedded_and_tabs() {
        let one = |xml: &str| {
            let d = Display::parse(&format!("<display>{xml}</display>")).unwrap();
            let pvs = PvManager::new();
            Builder::new(&pvs).screen(d, &Macros::default(), None, 0)
        };
        let s = one(r#"<widget type="table"/>"#);
        assert_eq!(
            Compat::of(&s.nodes).text().as_deref(),
            Some("1 unsupported widget")
        );
        let s = one(r#"<widget type="led"><scripts><script file="a"/></scripts></widget>"#);
        assert_eq!(
            Compat::of(&s.nodes).text().as_deref(),
            Some("1 widget with scripts ignored")
        );
        // Embedded screens and tabs are walked.
        let mut s = one(r#"<widget type="group"/>"#);
        let inner = one(r#"<widget type="table"/>"#);
        let tab = one(r#"<widget type="webbrowser"/>"#).nodes;
        s.nodes[0].embedded = Some(Box::new(inner));
        s.nodes[0].tabs = vec![tab];
        assert_eq!(Compat::of(&s.nodes).placeholders, 2);
    }

    #[test]
    fn positions_children_inside_groups() {
        let d = Display::parse(
            r#"<display><macros><P>X</P></macros>
            <widget type="group"><x>10</x><y>20</y><macros><R>:R:</R></macros>
              <widget type="textupdate"><x>5</x><y>6</y><pv_name>loc://$(P)$(R)v</pv_name></widget>
            </widget>
            <widget type="action_button"><pv_name>loc://b</pv_name>
              <actions><action type="write_pv"><pv_name>$(pv_name)</pv_name><value>1</value></action></actions>
            </widget></display>"#,
        )
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let child = &s.nodes[0].children[0];
        assert_eq!(
            child.rect,
            [10.0 + 18.0 + 5.0, 20.0 + 18.0 + 6.0, 100.0, 20.0]
        );
        assert_eq!(child.pv_name, "loc://X:R:v");
        let button = &s.nodes[1];
        assert_eq!(button.expand("$(pv_name)"), "loc://b");
        assert_eq!(button.rect[2..], [100.0, 30.0]);
    }

    #[test]
    fn property_macros_expand_their_own_macros() {
        let d = Display::parse(
            r#"<display><macros><cell>04</cell></macros>
            <widget type="symbol"><name>SR$(cell)S-PIRG</name><pv_name>loc://$(name):STA</pv_name></widget>
            <widget type="symbol"><name>$(name)x</name><pv_name>$(pv_name)</pv_name></widget>
            </display>"#,
        )
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        assert_eq!(s.nodes[0].pv_name, "loc://SR04S-PIRG:STA");
        // Through two properties, as a BOY symbol's rule PV `$(pv_name)` is.
        assert_eq!(s.nodes[0].expand("$(pv_name)"), "loc://SR04S-PIRG:STA");
        // Properties naming themselves are left alone, not expanded forever.
        assert_eq!(s.nodes[1].expand("$(name)"), "$(name)");
    }

    #[test]
    fn property_macros_naming_others_twice_stay_quick() {
        // p0 names p1 twice, which names p2 twice … : 2^40 expansions, and
        // a 2^40 byte value, if each were expanded each time it is named.
        let props: String = (0..40)
            .map(|i| format!("<p{i}>$(p{n})$(p{n})</p{i}>", n = i + 1))
            .collect();
        let d = Display::parse(&format!(
            "<display><widget type='label'>{props}<p40>x</p40></widget></display>"
        ))
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        assert_eq!(s.nodes[0].expand("$(p38)"), "xxxx");
        assert!(s.nodes[0].expand("$(p0)").starts_with("$(p0)"));
    }

    #[test]
    fn titles_expand_macros_or_use_the_file_name() {
        let pvs = PvManager::new();
        let b = Builder::new(&pvs);
        let d = Display::parse("<display><name>RGA - $(device)</name></display>").unwrap();
        let s = b.screen(
            d,
            &Macros::from_pairs([("device", "BR-VA-RGA-01")]),
            None,
            0,
        );
        assert_eq!(s.title, "RGA - BR-VA-RGA-01");
        let mut d = Display::parse("<display><name> </name></display>").unwrap();
        d.source = Some(Source::File("/a/boosterVacuum.opi".into()));
        assert_eq!(
            b.screen(d, &Macros::default(), None, 0).title,
            "boosterVacuum"
        );
    }

    #[test]
    fn opi_screens_keep_their_declared_size() {
        let pvs = PvManager::new();
        let b = Builder::new(&pvs);
        let xml = |root: &str| {
            format!(
                "<display {root}><width>300</width><height>200</height>
                 <widget type='label'><x>10</x><y>5000</y></widget></display>"
            )
        };
        let opi = Display::parse(&xml(
            "typeId='org.csstudio.opibuilder.Display' version='1.0'",
        ));
        let s = b.screen(opi.unwrap(), &Macros::default(), None, 0);
        assert_eq!((s.width, s.height), (300.0, 200.0));
        let bob = Display::parse(&xml("")).unwrap();
        let s = b.screen(bob, &Macros::default(), None, 0);
        assert_eq!((s.width, s.height), (300.0, 5020.0));
    }

    #[test]
    fn display_macros_are_overridden_by_callers() {
        let d = Display::parse(
            "<display><macros><P>A</P><Q>q</Q></macros><widget type='led'><pv_name>loc://$(P)$(Q)</pv_name></widget></display>",
        )
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::from_pairs([("P", "B")]), None, 0);
        assert_eq!(s.nodes[0].pv_name, "loc://Bq");
    }

    #[test]
    fn opi_displays_get_their_own_did() {
        let opi = r#"<display typeId="org.csstudio.opibuilder.Display" version="1.0.0">
            <widget typeId="org.csstudio.opibuilder.widgets.TextUpdate" version="1.0.0">
            <pv_name>loc://$(DID)ll</pv_name></widget></display>"#;
        let pvs = PvManager::new();
        let b = Builder::new(&pvs);
        let open = |macros: &Macros, depth| {
            let d = Display::parse(opi).unwrap();
            b.screen(d, macros, None, depth).nodes[0].pv_name.clone()
        };
        let (one, two) = (open(&Macros::default(), 0), open(&Macros::default(), 0));
        assert!(
            one.starts_with("loc://DID_") && one.ends_with("ll"),
            "{one}"
        );
        assert_ne!(one, two);
        // An embedded display shares its host's id; a new one does not.
        let host = Macros::from_pairs([("DID", "DID_7")]);
        assert_eq!(open(&host, 1), "loc://DID_7ll");
        assert_ne!(open(&host, 0), "loc://DID_7ll");
        // DID is a BOY built-in, so a .bob has none.
        let bob = "<display><widget type='led'><pv_name>loc://$(DID)x</pv_name></widget></display>";
        let s = b.screen(Display::parse(bob).unwrap(), &Macros::default(), None, 0);
        assert_eq!(s.nodes[0].pv_name, "loc://$(DID)x");
    }

    /// Serve a display at every path but /missing.bob (a 404), holding each
    /// reply until `release` is sent to; returns the base URL.
    #[cfg(not(target_arch = "wasm32"))]
    fn held_server() -> (String, std::sync::mpsc::Sender<()>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (release, held) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                if held.recv().is_err() {
                    return;
                }
                let body = r#"<display version="2.0.0"><name>Tab</name></display>"#;
                let (status, body) = if first.starts_with("GET /missing.bob ") {
                    ("404 Not Found", "")
                } else {
                    ("200 OK", body)
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (base, release)
    }

    /// Held by tests that check whether anything is still being built.
    #[cfg(not(target_arch = "wasm32"))]
    static BUILDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Poll `node` until its embedded display is built, or panic.
    #[cfg(not(target_arch = "wasm32"))]
    fn built(node: &mut Node) {
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while node.poll_building(Duration::from_millis(10)) {
            assert!(std::time::Instant::now() < until, "never built");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn navtabs_are_fetched_off_the_ui_thread() {
        let _builds = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
        let _cache = crate::fetch::tests::CACHE_USERS.lock();
        let (base, release) = held_server();
        let dir = std::env::temp_dir().join(format!("bobview-navtabs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("local.bob"),
            r#"<display version="2.0.0"><name>Local</name></display>"#,
        )
        .unwrap();
        let main = dir.join("main.bob");
        std::fs::write(
            &main,
            format!(
                r#"<display version="2.0.0">
                  <widget type="navtabs" version="2.0.0"><name>Nav</name>
                    <tabs>
                      <tab><name>Local</name><file>local.bob</file></tab>
                      <tab><name>Remote</name><file>{base}/tab.bob</file></tab>
                      <tab><name>Missing</name><file>{base}/missing.bob</file></tab>
                    </tabs>
                  </widget>
                </display>"#
            ),
        )
        .unwrap();
        let pvs = PvManager::new();
        let mut s = Screen::load(&Source::File(main), &Macros::default(), &pvs).unwrap();
        let ui = Builder::ui(&pvs);
        let nav = &mut s.nodes[0];
        assert_eq!(nav.embedded.as_ref().unwrap().display.name, "Local");

        // The server holds its reply, yet selecting the tab returns, showing
        // it as loading.
        ui.select_navtab(nav, 1, 0);
        assert!(nav.embedded.is_none() && nav.building.is_some());
        let loading = format!("Loading {base}/tab.bob");
        assert_eq!(nav.error.as_deref(), Some(loading.as_str()));
        assert!(nav.poll_building(Duration::ZERO), "still fetching");
        // A snapshot waits for it.
        assert!(building());
        let ctx = eframe::egui::Context::default();
        let app = crate::app::BobApp::new(&ctx, None, Some(1.0), crate::chrome::UiScale::Auto);
        assert!(app.is_loading());
        release.send(()).unwrap();
        built(nav);
        assert!(!building() && !app.is_loading());
        assert_eq!(nav.embedded.as_ref().unwrap().display.name, "Tab");
        assert_eq!(nav.error, None);

        // A failed fetch shows its error, as other fetched displays do.
        ui.select_navtab(nav, 2, 0);
        release.send(()).unwrap();
        built(nav);
        assert!(nav.embedded.is_none());
        let failed = format!("cannot fetch {base}/missing.bob: HTTP 404");
        assert_eq!(nav.error.as_deref(), Some(failed.as_str()));

        // A tab already fetched comes from the cache, with no request (the
        // server would hold one, never released, so it would never build).
        ui.select_navtab(nav, 1, 0);
        built(nav);
        assert_eq!(nav.embedded.as_ref().unwrap().display.name, "Tab");

        // A local tab still shows at once.
        ui.select_navtab(nav, 0, 0);
        built(nav);
        assert_eq!(nav.embedded.as_ref().unwrap().display.name, "Local");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_tab_switched_away_from_is_dropped_when_built() {
        let _builds = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
        let (base, release) = held_server();
        let d = Display::parse(&format!(
            r#"<display version="2.0.0">
              <widget type="navtabs" version="2.0.0"><name>Nav</name>
                <tabs>
                  <tab><name>Empty</name><file></file></tab>
                  <tab><name>Slow</name><file>{base}/slow.bob</file></tab>
                </tabs>
              </widget>
            </display>"#
        ))
        .unwrap();
        let pvs = PvManager::new();
        let mut s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let ui = Builder::ui(&pvs);
        let nav = &mut s.nodes[0];
        ui.select_navtab(nav, 1, 0);
        assert!(nav.building.is_some());
        ui.select_navtab(nav, 0, 0);
        assert!(nav.building.is_none() && nav.error.is_none());
        release.send(()).unwrap();
        assert!(!nav.poll_building(Duration::from_millis(100)));
        assert!(nav.embedded.is_none() && nav.error.is_none());
        // Thrown away, it stops counting once its thread ends.
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while building() {
            assert!(std::time::Instant::now() < until, "still counted");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(nav.active_tab, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_tab_built_while_hidden_is_still_collected() {
        let _builds = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
        let (base, release) = held_server();
        let d = Display::parse(&format!(
            r#"<display version="2.0.0">
              <widget type="navtabs" version="2.0.0"><name>Nav</name>
                <tabs>
                  <tab><name>Empty</name><file></file></tab>
                  <tab><name>Slow</name><file>{base}/slow.bob</file></tab>
                </tabs>
              </widget>
            </display>"#
        ))
        .unwrap();
        let pvs = PvManager::new();
        let mut screen = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        Builder::ui(&pvs).select_navtab(&mut screen.nodes[0], 1, 0);
        let mut view = crate::view::View::new(&pvs, Some(1.0));
        view.screen = Some(screen);
        release.send(()).unwrap();
        // Never drawn (as when hidden), the view still collects it, so a
        // snapshot does not wait for it.
        let ctx = eframe::egui::Context::default();
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while building() {
            assert!(std::time::Instant::now() < until, "never collected");
            std::thread::sleep(Duration::from_millis(10));
            view.poll_loading(&ctx);
        }
        let nav = &view.screen.as_ref().unwrap().nodes[0];
        assert_eq!(nav.embedded.as_ref().unwrap().display.name, "Tab");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_local_display_embedding_a_url_loads_off_the_ui_thread() {
        let (base, release) = held_server();
        let dir = std::env::temp_dir().join(format!("bobview-embedurl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.bob");
        std::fs::write(
            &main,
            format!(
                r#"<display version="2.0.0"><name>Main</name>
                  <widget type="embedded" version="2.0.0"><name>E</name>
                    <file>{base}/remote.bob</file>
                  </widget>
                </display>"#
            ),
        )
        .unwrap();
        let ctx = eframe::egui::Context::default();
        let mut view = crate::view::View::new(&PvManager::new(), Some(1.0));
        // The server holds its reply, yet showing the display returns.
        view.show(crate::app::Location {
            source: Source::File(main),
            macros: Macros::default(),
        });
        assert!(view.is_loading() && view.screen.is_none());
        release.send(()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while view.is_loading() {
            assert!(std::time::Instant::now() < until, "never loaded");
            std::thread::sleep(Duration::from_millis(10));
            view.poll_loading(&ctx);
        }
        let screen = view.screen.as_ref().unwrap();
        assert_eq!(screen.title, "Main");
        let embedded = screen.nodes[0].embedded.as_ref().unwrap();
        assert_eq!(embedded.display.name, "Tab");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Checks over the repo's test display set.
#[cfg(test)]
pub mod display_set {
    use super::*;
    use std::path::{Path, PathBuf};

    pub fn files() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "bob" || x == "opi") {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/displays"),
            &mut out,
        );
        out.sort();
        assert!(out.len() >= 8, "test displays missing");
        out
    }

    fn check(nodes: &[Node], file: &Path) {
        for n in nodes {
            assert!(
                DRAWN.contains(&n.w.kind.as_str()),
                "{}: unknown widget {}",
                file.display(),
                n.w.kind
            );
            assert!(
                n.error.is_none(),
                "{}: {}: {:?}",
                file.display(),
                n.w.name(),
                n.error
            );
            check(&n.children, file);
            n.tabs.iter().for_each(|t| check(t, file));
            if let Some(s) = &n.embedded {
                check(&s.nodes, file);
            }
        }
    }

    #[test]
    fn all_test_displays_load() {
        let pvs = PvManager::new();
        for f in files() {
            let s = Screen::load(&f.clone().into(), &Macros::default(), &pvs)
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(!s.nodes.is_empty(), "{} is empty", f.display());
            check(&s.nodes, &f);
        }
    }

    /// Load every `.opi` in the linked folders of the Eclipse project in
    /// `$OPI_SWEEP` (a `.project`'s folder, or a DLS module's screens beside
    /// their `runcss.sh`) and report what does not translate: run with
    /// `OPI_SWEEP=/path/to/project cargo test -- --ignored opi_sweep --nocapture`.
    #[test]
    #[ignore]
    fn opi_sweep() {
        use std::collections::BTreeMap;
        let Some(dir) = std::env::var_os("OPI_SWEEP").map(PathBuf::from) else {
            panic!("set OPI_SWEEP to an Eclipse project directory");
        };
        let Some(project) = bob::project::project_dir(&dir) else {
            panic!(
                "{} has no .project, nor a runcss.sh with links",
                dir.display()
            );
        };
        fn walk(virt: &Path, out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(bob::project::locate(virt)) else {
                return;
            };
            for e in rd.flatten() {
                let p = virt.join(e.file_name());
                if e.path().is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "opi") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        for (name, _) in bob::project::links(&project).unwrap_or_default() {
            walk(&project.join(name), &mut files);
        }
        files.sort();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        let mut problems: BTreeMap<String, usize> = BTreeMap::new();
        let mut failed = Vec::new();
        fn tally(
            nodes: &[Node],
            kinds: &mut BTreeMap<String, usize>,
            problems: &mut BTreeMap<String, usize>,
        ) {
            for n in nodes {
                if !DRAWN.contains(&n.w.kind.as_str()) {
                    *kinds.entry(n.w.kind.clone()).or_default() += 1;
                }
                if let Some(e) = &n.error {
                    *problems.entry(e.clone()).or_default() += 1;
                }
                if n.w.kind == "symbol" {
                    for f in n.w.list("symbols", "symbol") {
                        let f = n.expand(&f);
                        if let Source::File(p) = n.resolve(&f)
                            && !f.contains("$(")
                            && !bob::project::locate(&p).exists()
                        {
                            *problems
                                .entry(format!("{}: not found", p.display()))
                                .or_default() += 1;
                        }
                    }
                }
                tally(&n.children, kinds, problems);
                if let Some(s) = &n.embedded {
                    tally(&s.nodes, kinds, problems);
                }
            }
        }
        let pvs = PvManager::new();
        for f in &files {
            match Screen::load(&f.clone().into(), &Macros::default(), &pvs) {
                Ok(s) => tally(&s.nodes, &mut kinds, &mut problems),
                Err(e) => failed.push(format!("{}: {e}", f.display())),
            }
        }
        println!("{} files, {} failed to load", files.len(), failed.len());
        failed.iter().for_each(|f| println!("  {f}"));
        println!("widgets drawn as placeholders:");
        kinds.iter().for_each(|(k, n)| println!("  {n:6} {k}"));
        println!("problems:");
        problems.iter().for_each(|(k, n)| println!("  {n:6} {k}"));
        assert!(failed.is_empty());
    }

    #[test]
    fn embedded_screens_get_macros() {
        let pvs = PvManager::new();
        let path = files()
            .into_iter()
            .find(|f| f.ends_with("motors.bob"))
            .unwrap();
        let s = Screen::load(&path.into(), &Macros::default(), &pvs).unwrap();
        let axis2 = s.nodes.iter().find(|n| n.w.name() == "Axis2").unwrap();
        let group = &axis2.embedded.as_ref().unwrap().nodes[0];
        let rbv = group.children.iter().find(|n| n.w.name() == "Rbv").unwrap();
        assert_eq!(rbv.pv_name, "BL01T-MO-SIMC-01:M2.RBV");
    }
}
