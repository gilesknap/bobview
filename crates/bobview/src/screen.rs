//! A loaded display: widgets with resolved positions, macros and PVs.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use bob::{Display, Macros, Source, Widget};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use pv::{Pv, PvManager};

use crate::rules::CompiledRule;

/// How deep embedded displays may nest before we stop (guards against loops).
pub(crate) const MAX_DEPTH: u32 = 8;

/// How long the UI thread waits for a display being built on a thread
/// before showing it as loading: long enough for a local or already
/// fetched display to show at once, short enough not to stall the window.
#[cfg(not(target_arch = "wasm32"))]
pub const QUICK: Duration = Duration::from_millis(50);

/// The stack of a thread that builds a display: building recurses once per
/// nesting level, through every display it embeds (each nesting up to the
/// XML depth cap), and the default is platform dependent.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const BUILD_STACK: usize = 16 << 20;

/// The most embedded displays one load builds, however they nest (guards
/// against displays that embed themselves several times over).
const MAX_SCREENS: u32 = 256;

/// The most XML, in all the displays a load embeds, that it parses.
const MAX_EMBEDDED_BYTES: usize = 32 << 20;

/// The most widgets one load builds, counting those of embedded displays.
const MAX_WIDGETS: u32 = 100_000;

/// The most rules, and the most actions, of a widget that are used.
const MAX_PER_WIDGET: usize = 256;

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
    /// What the load that made this node may still build.
    budget: Arc<Budget>,
}

/// Embedded displays and widgets that a load may still make. It belongs to
/// the root screen and is shared with its nodes, so that displays embedded
/// later (as the web build fetches them) draw on the same budget.
struct Budget {
    screens: AtomicU32,
    widgets: AtomicU32,
    /// Bytes of expanded macro values and PV names.
    macros: AtomicUsize,
    /// Bytes of embedded display files.
    files: AtomicUsize,
}

impl Budget {
    /// The budget of a new load.
    fn new() -> Arc<Self> {
        Arc::new(Budget {
            screens: AtomicU32::new(MAX_SCREENS),
            widgets: AtomicU32::new(MAX_WIDGETS),
            macros: AtomicUsize::new(bob::MAX_LOAD_BYTES),
            files: AtomicUsize::new(MAX_EMBEDDED_BYTES),
        })
    }

    /// Take `n` from `left`, or fail, taking nothing, if less is left. Build
    /// threads embedding at once can't together take more than there is.
    fn take_bytes(left: &AtomicUsize, n: usize) -> bool {
        let mut l = left.load(Ordering::Relaxed);
        loop {
            let Some(rest) = l.checked_sub(n) else {
                return false;
            };
            match left.compare_exchange(l, rest, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return true,
                Err(now) => l = now,
            }
        }
    }

    /// Take one from `left`, or fail if none is left.
    fn take(left: &AtomicU32) -> bool {
        loop {
            let n = left.load(Ordering::Relaxed);
            if n == 0 {
                return false;
            }
            if left
                .compare_exchange(n, n - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }
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
    refused(source, "the build failed (see the log)")
}

/// The error for a display at `source` that was not read or built, and why.
fn refused(source: &Source, why: &str) -> bob::Error {
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
    budget: Arc<Budget>,
    /// Whether this thread may wait on files and servers; on the UI thread
    /// an embedded display is built on a thread of its own instead.
    #[cfg(not(target_arch = "wasm32"))]
    wait: bool,
}

impl<'a> Builder<'a> {
    /// A builder that reads embedded displays as it goes, for a thread that
    /// may wait on them (in a browser nothing waits: see [`crate::fetch`]).
    /// It starts a load, with a budget of its own.
    pub fn new(pvs: &'a PvManager) -> Self {
        Self::sharing(pvs, Budget::new(), true)
    }

    /// A builder for the UI thread, which builds each embedded display on a
    /// thread of its own, showing it as loading until it is built.
    pub fn ui(pvs: &'a PvManager) -> Self {
        Self::sharing(pvs, Budget::new(), false)
    }

    /// A builder drawing on `budget`, that of the load it continues.
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    fn sharing(pvs: &'a PvManager, budget: Arc<Budget>, wait: bool) -> Self {
        Self {
            pvs,
            budget,
            #[cfg(not(target_arch = "wasm32"))]
            wait,
        }
    }
}

impl Screen {
    /// Load `source` with `macros` layered over the display's own macros.
    pub fn load(source: &Source, macros: &Macros, pvs: &PvManager) -> Result<Self, bob::Error> {
        let display = crate::fetch::load(source)?;
        bob::rearm_warning();
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

/// Deepest chain of properties naming one another that is expanded.
const MAX_PROP_DEPTH: usize = 16;

/// The properties being expanded, and those already expanded.
type Expanding = std::cell::RefCell<(
    Vec<String>,
    std::collections::HashMap<String, Option<String>>,
)>;

/// [`expand_for`], with the properties being expanded and their values so
/// far in `state`. A widget property may name others, as BOY's rule PV
/// `$(pv_name)` does with `$(name):STA` and the name
/// `$(prefix)$(cell)S-VA-PIRG-01`; one that names itself, directly or not,
/// or anything unknown, too long or naming others too deeply is left as
/// written. Each is expanded once, so properties that name others several
/// times don't take exponential time.
fn expand_props(w: &Widget, macros: &Macros, text: &str, state: &Expanding) -> String {
    bob::expand(text, &|name| {
        macros.get(name).map(str::to_string).or_else(|| {
            if let Some(value) = state.borrow().1.get(name) {
                return value.clone();
            }
            // Each property named recurses, so a long chain of them must not
            // exhaust the stack.
            let expanding = state.borrow();
            if expanding.0.len() >= MAX_PROP_DEPTH || expanding.0.iter().any(|a| a == name) {
                return None;
            }
            drop(expanding);
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
    /// [`expand_for`], taking the size of the text and result from the load's
    /// macro budget; once it is spent, text is left as written.
    fn expand_charged(&self, w: &Widget, macros: &Macros, text: &str) -> String {
        if self.budget.macros.load(Ordering::Relaxed) == 0 {
            bob::warn_spent();
            return text.to_string();
        }
        let out = expand_for(w, macros, text);
        bob::spend(&self.budget.macros, text.len() + out.len());
        out
    }

    pub fn screen(
        &self,
        display: Display,
        macros: &Macros,
        group: Option<&str>,
        depth: u32,
    ) -> Screen {
        // Macros passed in override the display's own defaults.
        let mut macros = Macros::over(&display.macros, macros, &self.budget.macros);
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
        let mut nodes = Vec::new();
        for w in widgets {
            // Widgets past the budget are left out, after one that says so.
            let over = !Budget::take(&self.budget.widgets);
            let mut node = self.node(w, origin, macros, base, depth);
            if over {
                log::warn!("too many widgets; the rest are left out");
                node.error = Some("too many widgets; the rest are left out".into());
                nodes.push(node);
                break;
            }
            nodes.push(node);
        }
        nodes
    }

    fn node(
        &self,
        w: &Widget,
        origin: (f64, f64),
        parent: &Macros,
        base: Option<&Source>,
        depth: u32,
    ) -> Node {
        let macros = parent.with_budget(&w.macros(), &self.budget.macros);
        let (dw, dh) = default_size(&w.kind);
        let rect = [
            origin.0 + w.f64("x").unwrap_or(0.0),
            origin.1 + w.f64("y").unwrap_or(0.0),
            w.f64("width").unwrap_or(dw),
            w.f64("height").unwrap_or(dh),
        ];
        let ex = |t: &str| self.expand_charged(w, &macros, t);
        let pv_name = w.text("pv_name").map(ex).unwrap_or_default();
        let pv = (!pv_name.is_empty()).then(|| self.pvs.subscribe(&pv_name));
        let (all_rules, all_actions) = (w.rules(), w.actions());
        if all_rules.len() > MAX_PER_WIDGET || all_actions.len() > MAX_PER_WIDGET {
            log::warn!("over {MAX_PER_WIDGET} rules or actions in a widget; the rest are ignored");
        }
        let rules = all_rules
            .iter()
            .take(MAX_PER_WIDGET)
            .map(|r| CompiledRule::new(r, &ex, self.pvs))
            .collect();

        let link = crate::group_link::Link::new(w, &ex, self.pvs);
        let mut action_pvs: Vec<Pv> = all_actions
            .iter()
            .take(MAX_PER_WIDGET)
            .filter_map(|a| match a {
                bob::Action::WritePv { pv_name, .. } => Some(self.pvs.subscribe(&ex(pv_name))),
                _ => None,
            })
            .collect();
        // A menu mux's targets, which a pick writes, and the values of its
        // initial item, which BOY writes as the display opens.
        if let Some(mux) = w.menu_mux() {
            let sets = w
                .get("menu_mux")
                .map_or(0, |m| m.children.iter().filter(|c| c.name == "set").count());
            if sets > bob::MAX_MUX_SETS {
                log::warn!(
                    "over {} sets in a menu mux; the rest are ignored",
                    bob::MAX_MUX_SETS
                );
            }
            for set in &mux.sets {
                let target = self.pvs.subscribe(&ex(&set.target));
                if let Some(value) = mux.initial.and_then(|i| set.values.get(i)) {
                    target.write(ex(value));
                }
                action_pvs.push(target);
            }
            if let (true, Some(i), Some(pv)) = (mux.index, mux.initial, &pv) {
                pv.write(i.to_string());
            }
        }

        let mut node = Node {
            // Without the children, which the nodes hold, so that nested
            // groups don't each keep a copy of everything below them.
            w: Widget {
                kind: w.kind.clone(),
                props: w.props.clone(),
                children: Vec::new(),
            },
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
            budget: self.budget.clone(),
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
                node.traces = crate::plot::subscribe(
                    w,
                    &|t| self.expand_charged(w, &node.macros, t),
                    self.pvs,
                );
            }
            "stripchart" => {
                node.traces = crate::plot::subscribe(
                    w,
                    &|t| self.expand_charged(w, &node.macros, t),
                    self.pvs,
                );
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
        // Embedded again once the web build has fetched it: counted already.
        let counted = std::mem::take(&mut node.loading);
        node.embedded = None;
        node.error = None;
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
        // The node's budget, not this builder's, which may be a fresh one:
        // the display is part of the load that made the node.
        let this = Builder::sharing(self.pvs, node.budget.clone(), self.waits());
        if !counted && !Budget::take(&this.budget.screens) {
            node.error = Some("too many embedded displays".into());
            return;
        }
        let source = node.resolve(file);
        #[cfg(not(target_arch = "wasm32"))]
        if !this.wait {
            let (tx, rx) = std::sync::mpsc::channel();
            let (pvs, src, budget) = (self.pvs.clone(), source.clone(), this.budget.clone());
            let (macros, group) = (macros.clone(), group.to_string());
            let in_flight = InFlight::new();
            let spawned = std::thread::Builder::new()
                .name("display-builder".into())
                .stack_size(BUILD_STACK)
                .spawn(move || {
                    // Still the same load, so on the same budget: a display
                    // that embeds itself can't get a fresh one per thread.
                    let built =
                        Builder::sharing(&pvs, budget, true).build(&src, &macros, &group, depth);
                    // Unsent (the node moved on), the result and its count drop here.
                    tx.send((built, in_flight)).ok();
                });
            if let Err(e) = spawned {
                let why = format!("cannot start a thread to build it: {e}");
                node.place(&source, Err(refused(&source, &why)));
                return;
            }
            node.error = Some(format!("Loading {source}"));
            node.building = Some(Building { source, rx });
            node.poll_building(QUICK);
            return;
        }
        let built = this.build(&source, macros, group, depth);
        node.place(&source, built);
    }

    /// Read and build the display at `source`, embedded `depth` deep, if
    /// the load has enough of its budget for embedded files left.
    fn build(
        &self,
        source: &Source,
        macros: &Macros,
        group: &str,
        depth: u32,
    ) -> Result<Screen, bob::Error> {
        let d = crate::fetch::load(source)?;
        if !Budget::take_bytes(&self.budget.files, d.size) {
            return Err(refused(source, "embedded displays too large in total"));
        }
        Ok(self.screen(d, macros, Some(group), depth + 1))
    }

    /// Whether this builder may wait on files and servers.
    fn waits(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.wait;
        #[cfg(target_arch = "wasm32")]
        true
    }

    /// Show tab `index` of a navigation tabs widget because the user picked
    /// it: a load of its own, on this builder's budget, so that switching
    /// tabs all day never spends the budget of the load that made the widget.
    pub fn pick_navtab(&self, node: &mut Node, index: usize, depth: u32) {
        node.budget = self.budget.clone();
        self.select_navtab(node, index, depth);
    }

    /// Show tab `index` of a navigation tabs widget.
    pub fn select_navtab(&self, node: &mut Node, index: usize, depth: u32) {
        let tabs = tab_elements(&node.w);
        let Some(tab) = tabs.get(index) else { return };
        node.active_tab = index;
        let file = node.expand(tab.child_text("file").unwrap_or(""));
        let group = node.expand(tab.child_text("group_name").unwrap_or(""));
        let macros = node.macros.with_budget(
            &tab.child("macros")
                .map(Macros::from_element)
                .unwrap_or_default(),
            &node.budget.macros,
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
    fn displays_embedding_themselves_stay_bounded() {
        // Four embeds of itself, 8 deep: 4^8 screens if each were built.
        let dir = std::env::temp_dir().join(format!("bobview-bomb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bomb.bob");
        let embed = "<widget type='embedded'><file>bomb.bob</file></widget>";
        std::fs::write(&file, format!("<display>{}</display>", embed.repeat(4))).unwrap();
        let start = std::time::Instant::now();
        let pvs = PvManager::new();
        let s = Screen::load(&Source::File(file), &Macros::default(), &pvs).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        fn count(nodes: &[Node], errors: &mut u32) -> u32 {
            nodes
                .iter()
                .map(|n| {
                    *errors += n.error.is_some() as u32;
                    1 + n.embedded.as_ref().map_or(0, |e| count(&e.nodes, errors))
                })
                .sum()
        }
        let mut errors = 0;
        let total = count(&s.nodes, &mut errors);
        assert!(total <= MAX_SCREENS * 4 + 4, "{total} nodes");
        assert!(errors > 0);
        assert!(start.elapsed().as_secs() < 10);
    }

    /// Embedded displays under `nodes`, with how many show an error.
    fn count_screens(nodes: &[Node], errors: &mut u32) -> u32 {
        nodes
            .iter()
            .map(|n| {
                *errors += n.error.is_some() as u32;
                n.embedded
                    .as_ref()
                    .map_or(0, |e| 1 + count_screens(&e.nodes, errors))
            })
            .sum()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn displays_built_on_threads_share_the_load_budget() {
        // Embeds and navigation tabs of itself, four ways, built from the UI
        // thread: each is built on a thread of its own, which must draw on
        // the budget of the load, not start a fresh one (4 x 256 screens).
        let _builds = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("bobview-threads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bomb.bob");
        let embed = "<widget type='embedded'><file>bomb.bob</file></widget>";
        let tabs = "<widget type='navtabs'><tabs><tab><file>bomb.bob</file></tab></tabs></widget>";
        let xml = format!("<display>{embed}{embed}{tabs}{tabs}</display>");
        std::fs::write(&file, &xml).unwrap();
        let mut d = Display::parse(&xml).unwrap();
        d.source = Some(Source::File(file));
        let pvs = PvManager::new();
        let mut s = Builder::ui(&pvs).screen(d, &Macros::default(), None, 0);
        let start = std::time::Instant::now();
        while poll_built(&mut s.nodes) {
            assert!(start.elapsed() < patience(), "still building");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_dir_all(&dir).ok();
        let mut errors = 0;
        let screens = count_screens(&s.nodes, &mut errors);
        assert!(screens <= MAX_SCREENS, "{screens} screens");
        assert!(errors > 0);
        assert!(s.nodes.iter().all(|n| n.building.is_none()));
    }

    #[test]
    fn picking_navigation_tabs_never_spends_the_load_budget() {
        let dir = std::env::temp_dir().join(format!("bobview-picks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tab.bob"),
            "<display><widget type='label'/></display>",
        )
        .unwrap();
        let tab = "<tab><file>tab.bob</file></tab>";
        let main = dir.join("main.bob");
        let xml =
            format!("<display><widget type='navtabs'><tabs>{tab}{tab}</tabs></widget></display>");
        std::fs::write(&main, xml).unwrap();
        let pvs = PvManager::new();
        let mut s = Screen::load(&Source::File(main), &Macros::default(), &pvs).unwrap();
        for i in 0..2 * MAX_SCREENS as usize {
            Builder::new(&pvs).pick_navtab(&mut s.nodes[0], i % 2, 0);
        }
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(s.nodes[0].error, None);
        assert!(s.nodes[0].embedded.is_some());
    }

    #[test]
    fn menu_mux_sets_are_capped() {
        // A hand-written `<menu_mux>` with far more sets than BOY allows.
        let set = |i: usize| {
            format!(
                "<set><target>loc://t{i}&lt;VString&gt;</target><values><v>a{i}</v><v>b{i}</v></values></set>"
            )
        };
        let sets: String = (0..10_000).map(set).collect();
        let xml = format!(
            "<display><widget type='combo'><menu_mux><initial>1</initial>{sets}</menu_mux></widget></display>"
        );
        let (s, took) = load_hostile("mux", &xml);
        let pvs = &s.nodes[0].action_pvs;
        assert_eq!(pvs.len(), bob::MAX_MUX_SETS);
        // Each target is written its own set's value for the initial item.
        assert!(pvs[3].name().to_string().starts_with("loc://t3"));
        assert_eq!(pvs[3].state().format(None, false), "b3");
        assert!(took < patience(), "{took:?}");
    }

    #[test]
    fn a_display_embedded_again_once_fetched_is_counted_once() {
        let dir = std::env::temp_dir().join(format!("bobview-refetch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("inner.bob"), "<display/>").unwrap();
        let xml = "<display><widget type='embedded'><file>inner.bob</file></widget></display>";
        std::fs::write(dir.join("main.bob"), xml).unwrap();
        let pvs = PvManager::new();
        let s = Screen::load(
            &Source::File(dir.join("main.bob")),
            &Macros::default(),
            &pvs,
        );
        let mut s = s.unwrap();
        let node = &mut s.nodes[0];
        let before = node.budget.screens.load(Ordering::Relaxed);
        // As the web build does once the file it was fetching arrives.
        node.loading = true;
        Builder::new(&pvs).embed(node, "inner.bob", &Macros::default(), "", 0);
        assert_eq!(node.budget.screens.load(Ordering::Relaxed), before);
        assert!(node.embedded.is_some() && !node.loading);
        // Embedded afresh, it is counted again.
        Builder::new(&pvs).embed(node, "inner.bob", &Macros::default(), "", 0);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(node.budget.screens.load(Ordering::Relaxed), before - 1);
    }

    /// How long a hostile display may take; debug builds are far slower.
    fn patience() -> std::time::Duration {
        std::time::Duration::from_secs(if cfg!(debug_assertions) { 60 } else { 10 })
    }

    /// Root macros V0..V9, where V9 is 2 KiB.
    fn doubling() -> String {
        let v: String = (1..=9)
            .map(|i| format!("<V{i}>$(V{n})$(V{n})</V{i}>", n = i - 1))
            .collect();
        format!("<macros><V0>xxxx</V0>{v}</macros>")
    }

    /// Load `xml` from a file, as an untrusted display, and say how long it took.
    fn load_hostile(name: &str, xml: &str) -> (Screen, std::time::Duration) {
        let dir = std::env::temp_dir().join(format!("bobview-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hostile.bob");
        std::fs::write(&file, xml).unwrap();
        let start = std::time::Instant::now();
        let pvs = PvManager::new();
        let s = Screen::load(&Source::File(file), &Macros::default(), &pvs).unwrap();
        let took = start.elapsed();
        std::fs::remove_dir_all(&dir).ok();
        (s, took)
    }

    #[test]
    fn macro_text_is_bounded_across_a_load() {
        // Every widget has 256 macros naming the 2 KiB V9, and a PV name that
        // does: 100 widgets would hold 50 MB if each were expanded.
        let own: String = (0..256).map(|i| format!("<N{i}>$(V9)</N{i}>")).collect();
        let widget = format!(
            "<widget type='label'><pv_name>loc://$(V9)</pv_name><macros>{own}</macros></widget>"
        );
        let xml = format!("<display>{}{}</display>", doubling(), widget.repeat(100));
        let (s, took) = load_hostile("macros", &xml);
        let held: usize = s
            .nodes
            .iter()
            .map(|n| n.macros.own_bytes() + n.pv_name.len())
            .sum();
        assert!(held < 2 * bob::MAX_LOAD_BYTES, "{held} bytes held");
        assert!(took < patience(), "{took:?}");
    }

    #[test]
    fn rules_and_actions_per_widget_are_capped() {
        let rules: String = (0..5000)
            .map(|i| format!("<rule name='r{i}' prop_id='visible'><exp bool_exp='true'><value>true</value></exp><pv_name>$(V9)u{i}</pv_name></rule>"))
            .collect();
        let actions: String = (0..5000)
            .map(|i| {
                format!(
                    "<action type='write_pv'><pv_name>$(V9)w{i}</pv_name><value>1</value></action>"
                )
            })
            .collect();
        let xml = format!(
            "<display>{}<widget type='label'><rules>{rules}</rules><actions>{actions}</actions></widget></display>",
            doubling()
        );
        let (s, took) = load_hostile("rules", &xml);
        assert!(s.nodes[0].rules.len() <= MAX_PER_WIDGET);
        assert!(s.nodes[0].action_pvs.len() <= MAX_PER_WIDGET);
        assert!(took < patience(), "{took:?}");
    }

    #[test]
    fn deeply_nested_groups_of_macros_load_quickly() {
        // 30 groups, each in the last and defining 256 macros that name V9.
        let own: String = (0..256).map(|i| format!("<N{i}>$(V9)</N{i}>")).collect();
        let mut inner = "<widget type='label'><pv_name>loc://$(N0)</pv_name></widget>".to_string();
        for _ in 0..30 {
            inner = format!("<widget type='group'><macros>{own}</macros>{inner}</widget>");
        }
        let xml = format!("<display>{}{inner}</display>", doubling());
        let (_, took) = load_hostile("nest", &xml);
        assert!(took < patience(), "{took:?}");
    }

    #[test]
    fn embedding_does_not_copy_the_caller_chain() {
        // A self-embedding display with large root macros, passing 256 more
        // at each level: the ancestors' macros must be shared, not copied.
        let big: String = (0..2000)
            .map(|i| format!("<B{i}>{}</B{i}>", "b".repeat(500)))
            .collect();
        let own: String = (0..256)
            .map(|i| format!("<N{i}>{}</N{i}>", "n".repeat(50)))
            .collect();
        let embed = format!(
            "<widget type='embedded'><file>hostile.bob</file><macros>{own}</macros></widget>"
        );
        let xml = format!(
            "<display><macros>{big}</macros>{}</display>",
            embed.repeat(4)
        );
        let (_, took) = load_hostile("embed", &xml);
        assert!(took < patience(), "{took:?}");
    }

    /// The text held by a widget's properties.
    fn prop_bytes(e: &bob::Element) -> usize {
        e.text.len() + e.children.iter().map(prop_bytes).sum::<usize>()
    }

    /// Text held by the widgets of `nodes` and everything below them.
    fn held(nodes: &[Node]) -> usize {
        nodes
            .iter()
            .map(|n| prop_bytes(&n.w.props) + n.w.children.len() + held(&n.children))
            .sum()
    }

    #[test]
    fn nested_groups_do_not_each_keep_their_subtree() {
        // 50 groups in each other and 1 MB of text at the bottom: kept once
        // per node would be 50 MB.
        let mut inner = format!(
            "<widget type='label'><text>{}</text></widget>",
            "t".repeat(1 << 20)
        );
        for _ in 0..50 {
            inner = format!("<widget type='group'><x>1</x>{inner}</widget>");
        }
        let d = Display::parse(&format!("<display>{inner}</display>")).unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        assert!(held(&s.nodes) < 2 << 20, "{} bytes", held(&s.nodes));
    }

    #[test]
    fn groups_nested_too_deeply_are_refused_as_they_parse() {
        // Bounded by the XML depth cap, before any widget is built, so a
        // screen never nests more than that within one display.
        let mut inner = "<widget type='label'/>".to_string();
        for _ in 0..100 {
            inner = format!("<widget type='group'>{inner}</widget>");
        }
        let d = Display::parse(&format!("<display>{inner}</display>"));
        assert!(matches!(d, Err(bob::Error::TooDeep)));
    }

    #[test]
    fn ordinary_nested_groups_keep_their_positions() {
        let mut inner = "<widget type='label'><x>5</x><y>7</y></widget>".to_string();
        for _ in 0..10 {
            inner = format!("<widget type='group'><x>10</x><y>20</y>{inner}</widget>");
        }
        let d = Display::parse(&format!("<display>{inner}</display>")).unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let mut n = &s.nodes[0];
        while let Some(c) = n.children.first() {
            n = c;
        }
        assert!(n.error.is_none());
        assert_eq!(n.w.kind, "label");
        // Each group adds its position and its 18 pixel inset.
        assert_eq!((n.rect[0], n.rect[1]), (285.0, 387.0));
    }

    #[test]
    fn long_chains_of_property_macros_do_not_overflow() {
        // p0 names p1, which names p2 … : one level of recursion each.
        let n = 100_000;
        let props: String = (0..n)
            .map(|i| format!("<p{i}>$(p{})</p{i}>", i + 1))
            .collect();
        let d = Display::parse(&format!(
            "<display><widget type='label'>{props}<p{n}>x</p{n}></widget></display>"
        ))
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let w = s.nodes[0].w.clone();
        let expanded = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                let m = Macros::default();
                (
                    expand_for(&w, &m, "$(p0)"),
                    expand_for(&w, &m, &format!("$(p{})", n - 3)),
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(expanded, ("$(p0)".to_string(), "x".to_string()));
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
        let host = Macros::from_pairs([("DID", "DID_host")]);
        assert_eq!(open(&host, 1), "loc://DID_hostll");
        assert_ne!(open(&host, 0), "loc://DID_hostll");
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
    fn deep_displays_embedding_themselves_build_on_a_thread() {
        // Groups as deep as the XML depth cap allows, embedding the file
        // again at the bottom: built on one thread, the recursion is that
        // depth for each of the 8 embedding levels.
        let _builds = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("bobview-deep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut inner = "<widget type='embedded'><file>deep.bob</file></widget>".to_string();
        for _ in 0..61 {
            inner = format!("<widget type='group'>{inner}</widget>");
        }
        let deep = format!("<display>{inner}</display>");
        assert!(Display::parse(&deep).is_ok(), "within the XML depth cap");
        std::fs::write(dir.join("deep.bob"), deep).unwrap();
        let xml = "<display><widget type='embedded'><file>deep.bob</file></widget></display>";
        let mut d = Display::parse(xml).unwrap();
        d.source = Some(Source::File(dir.join("main.bob")));
        let pvs = PvManager::new();
        let mut s = Builder::ui(&pvs).screen(d, &Macros::default(), None, 0);
        built(&mut s.nodes[0]);
        std::fs::remove_dir_all(&dir).ok();
        // Down the groups and into each embedded display.
        let (mut n, mut screens) = (&s.nodes[0], 0);
        loop {
            match (n.embedded.as_ref(), n.children.first()) {
                (Some(e), _) => {
                    screens += 1;
                    n = &e.nodes[0];
                }
                (None, Some(c)) => n = c,
                (None, None) => break,
            }
        }
        assert_eq!(screens, MAX_DEPTH);
        assert_eq!(
            n.error.as_deref(),
            Some("embedded displays nested too deeply")
        );
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
