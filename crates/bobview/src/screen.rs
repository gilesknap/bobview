//! A loaded display: widgets with resolved positions, macros and PVs.

use bob::{Display, Macros, Source, Widget};
use pv::{Pv, PvManager};

use crate::rules::{CompiledRule, Lang};

/// How deep embedded displays may nest before we stop (guards against loops).
pub(crate) const MAX_DEPTH: u32 = 8;

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
    /// PVs written by this widget's actions, connected in advance.
    pub action_pvs: Vec<Pv>,
    /// Text being typed into a text entry, while it has focus.
    pub edit: Option<String>,
    /// Problems to show on the widget, such as a missing embedded file.
    pub error: Option<String>,
    /// The embedded display is still being fetched (in a browser), so
    /// should be loaded again once it is.
    pub loading: bool,
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

pub struct Builder<'a> {
    pub pvs: &'a PvManager,
}

impl Screen {
    /// Load `source` with `macros` layered over the display's own macros.
    pub fn load(source: &Source, macros: &Macros, pvs: &PvManager) -> Result<Self, bob::Error> {
        let display = crate::fetch::load(source)?;
        Ok(Builder { pvs }.screen(display, macros, None, 0))
    }
}

impl Node {
    /// Expand macros in a property value, falling back to the widget's own
    /// properties, so `$(pv_name)` and `$(name)` work as in Phoebus.
    pub fn expand(&self, text: &str) -> String {
        expand_for(&self.w, &self.macros, self.base.as_ref(), text)
    }

    /// Expand macros with `macros` in place of the widget's own.
    pub fn expand_with(&self, macros: &Macros, text: &str) -> String {
        expand_for(&self.w, macros, self.base.as_ref(), text)
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

    /// Move and size the widget from geometry `before` (x, y, width,
    /// height as its properties gave them) to `after`, as a rule does; its
    /// children move with it. Changes, not the values, are applied, so a
    /// size set from the content (an embedded display's resize mode 2) or
    /// the parent's offset is kept.
    pub fn reshape(&mut self, before: [f64; 4], after: [f64; 4]) {
        for (r, (b, a)) in self.rect.iter_mut().zip(before.iter().zip(after)) {
            *r += a - b;
        }
        let (dx, dy) = (after[0] - before[0], after[1] - before[1]);
        if dx != 0.0 || dy != 0.0 {
            self.children
                .iter_mut()
                .chain(self.tabs.iter_mut().flatten())
                .for_each(|n| n.shift(dx, dy));
        }
    }

    /// Move the widget and its children by `dx`, `dy`.
    fn shift(&mut self, dx: f64, dy: f64) {
        self.rect[0] += dx;
        self.rect[1] += dy;
        self.children
            .iter_mut()
            .chain(self.tabs.iter_mut().flatten())
            .for_each(|n| n.shift(dx, dy));
    }

    /// Resolve a file named by this widget against the display it is in.
    pub fn resolve(&self, file: &str) -> Source {
        match &self.base {
            Some(base) => base.resolve(file),
            None => Source::parse(file),
        }
    }
}

/// Expand macros in `text` of widget `w`, from a display loaded from
/// `base`. As Phoebus's `MacroOrPropertyProvider`, a name that is neither a
/// macro nor a property falls back to an environment variable, but only in
/// a display from a file: one from the web must not read them (it could
/// send them out in a PV name or URL).
pub(crate) fn expand_for(w: &Widget, macros: &Macros, base: Option<&Source>, text: &str) -> String {
    let env = matches!(base, Some(Source::File(_)));
    expand_props(w, macros, env, text, &Default::default())
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
fn expand_props(w: &Widget, macros: &Macros, env: bool, text: &str, state: &Expanding) -> String {
    bob::expand(text, &|name| {
        macros.get(name).map(str::to_string).or_else(|| {
            if let Some(value) = state.borrow().1.get(name) {
                return value.clone();
            }
            if state.borrow().0.iter().any(|a| a == name) {
                return None;
            }
            let Some(prop) = w.text(name) else {
                return env.then(|| env_var(name)).flatten();
            };
            state.borrow_mut().0.push(name.to_string());
            let value = expand_props(w, macros, env, prop, state);
            let value = Some(value).filter(|v| !v.contains('$') && v.len() <= 1 << 16);
            let mut state = state.borrow_mut();
            state.0.pop();
            state.1.insert(name.to_string(), value.clone());
            value
        })
    })
}

/// An environment variable, for a macro no display sets.
#[cfg(not(target_arch = "wasm32"))]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(target_arch = "wasm32")]
fn env_var(_name: &str) -> Option<String> {
    None
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

/// A widget's x, y, width and height as its properties give them, relative
/// to its parent.
pub fn geometry(w: &Widget) -> [f64; 4] {
    let (dw, dh) = default_size(&w.kind);
    [
        w.f64("x").unwrap_or(0.0),
        w.f64("y").unwrap_or(0.0),
        w.f64("width").unwrap_or(dw),
        w.f64("height").unwrap_or(dh),
    ]
}

/// The offset of a tabs widget's content from its top-left corner: below
/// the tabs, or right of them when they run down the side (`direction` 1).
pub fn tabs_inset(w: &Widget) -> (f64, f64) {
    if w.i64("direction").unwrap_or(0) == 1 {
        (w.f64("tab_width").unwrap_or(100.0), 0.0)
    } else {
        (0.0, w.f64("tab_height").unwrap_or(30.0))
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

impl Builder<'_> {
    pub fn screen(
        &self,
        display: Display,
        macros: &Macros,
        group: Option<&str>,
        depth: u32,
    ) -> Screen {
        // Macros passed in override the display's own defaults.
        let macros = display.macros.with(macros);
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
        let lang = if display.legacy {
            Lang::JavaScript
        } else {
            Lang::Python
        };
        let nodes = self.nodes(&widgets, (0.0, 0.0), &macros, base.as_ref(), lang, depth);
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
        lang: Lang,
        depth: u32,
    ) -> Vec<Node> {
        widgets
            .iter()
            .map(|w| self.node(w, origin, macros, base, lang, depth))
            .collect()
    }

    fn node(
        &self,
        w: &Widget,
        origin: (f64, f64),
        parent: &Macros,
        base: Option<&Source>,
        lang: Lang,
        depth: u32,
    ) -> Node {
        let macros = parent.with(&w.macros());
        let [x, y, width, height] = geometry(w);
        let rect = [origin.0 + x, origin.1 + y, width, height];
        let expand = |t: &str| expand_for(w, &macros, base, t);
        let pv_name = w.text("pv_name").map(expand).unwrap_or_default();
        let pv = (!pv_name.is_empty()).then(|| self.pvs.subscribe(&pv_name));
        let rules = w
            .rules()
            .iter()
            .map(|r| CompiledRule::new(r, lang, &expand, self.pvs))
            .collect();

        let link = crate::group_link::Link::new(w, &expand, self.pvs);
        let action_pvs = w
            .actions()
            .iter()
            .filter_map(|a| match a {
                bob::Action::WritePv { pv_name, .. } => Some(self.pvs.subscribe(&expand(pv_name))),
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
            action_pvs,
            edit: None,
            error: None,
            loading: false,
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
                    lang,
                    depth,
                );
            }
            "tabs" => {
                let (ix, iy) = tabs_inset(w);
                let content = (rect[0] + ix, rect[1] + iy);
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
                        self.nodes(&children, content, &node.macros, base, lang, depth)
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
                node.traces = crate::plot::subscribe(w, &|t| node.expand(t), self.pvs);
            }
            "stripchart" => {
                node.traces = crate::plot::subscribe(w, &|t| node.expand(t), self.pvs);
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

    /// Load `file` into `node.embedded`, or record why it failed.
    pub fn embed(&self, node: &mut Node, file: &str, macros: &Macros, group: &str, depth: u32) {
        node.embedded = None;
        node.error = None;
        node.loading = false;
        if file.is_empty() {
            return;
        }
        if depth >= MAX_DEPTH {
            node.error = Some("embedded displays nested too deeply".into());
            return;
        }
        let source = node.resolve(file);
        match crate::fetch::load(&source) {
            Ok(d) => {
                let screen = self.screen(d, macros, Some(group), depth + 1);
                // Resize mode 2 sizes the widget to its content.
                if node.w.i64("resize") == Some(2) {
                    node.rect[2] = screen.width;
                    node.rect[3] = screen.height;
                }
                node.embedded = Some(Box::new(screen));
            }
            #[cfg(target_arch = "wasm32")]
            Err(bob::Error::Fetch(_, why)) if why == crate::fetch::LOADING => {
                node.loading = true;
                node.error = Some(format!("Loading {source}"));
            }
            Err(e) => {
                log::warn!("{e}");
                node.error = Some(match e {
                    bob::Error::Io(..) => format!("{source}: not found"),
                    e => e.to_string(),
                });
            }
        }
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
        let s = Builder { pvs: &pvs }.screen(d, &Macros::default(), None, 0);
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
            Builder { pvs: &pvs }.screen(d, &Macros::default(), None, 0)
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
        let s = Builder { pvs: &pvs }.screen(d, &Macros::default(), None, 0);
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
        let s = Builder { pvs: &pvs }.screen(d, &Macros::default(), None, 0);
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
        let s = Builder { pvs: &pvs }.screen(d, &Macros::default(), None, 0);
        assert_eq!(s.nodes[0].expand("$(p38)"), "xxxx");
        assert!(s.nodes[0].expand("$(p0)").starts_with("$(p0)"));
    }

    #[test]
    fn titles_expand_macros_or_use_the_file_name() {
        let pvs = PvManager::new();
        let b = Builder { pvs: &pvs };
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
        let b = Builder { pvs: &pvs };
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
        let s = Builder { pvs: &pvs }.screen(d, &Macros::from_pairs([("P", "B")]), None, 0);
        assert_eq!(s.nodes[0].pv_name, "loc://Bq");
    }

    fn build(xml: &str, pvs: &PvManager) -> Screen {
        let d = Display::parse(&format!("<display>{xml}</display>")).unwrap();
        Builder { pvs }.screen(d, &Macros::default(), None, 0)
    }

    fn wait_connected(pv: &Pv) {
        let start = std::time::Instant::now();
        while !pv.state().connected && start.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn geometry_rules_move_and_size_a_node_and_its_children() {
        let pvs = PvManager::new();
        let mut s = build(
            r#"<widget type="group"><x>10</x><y>20</y><width>100</width><style>3</style>
              <rules>
                <rule name="x" prop_id="x" out_exp="true">
                  <exp bool_exp="true"><expression>pv0</expression></exp>
                  <pv_name>loc://geo_x(50)</pv_name></rule>
                <rule name="bar" prop_id="width" out_exp="false">
                  <exp bool_exp="pv0 &gt; 10"><value>200</value></exp>
                  <pv_name>loc://geo_x</pv_name></rule>
              </rules>
              <widget type="label"><x>5</x><y>6</y></widget>
            </widget>"#,
            &pvs,
        );
        let g = &mut s.nodes[0];
        assert_eq!(g.rect, [10.0, 20.0, 100.0, 200.0]);
        assert_eq!(g.children[0].rect[..2], [15.0, 26.0]);
        wait_connected(g.rules[0].pvs().first().unwrap());
        crate::widgets::update_rules(g, &pvs);
        assert_eq!(g.rect, [50.0, 20.0, 200.0, 200.0]);
        assert_eq!(g.children[0].rect[..2], [55.0, 26.0]);
        // Back where it was when the rules no longer hold.
        g.rules[0].pvs()[0].write("5");
        crate::widgets::update_rules(g, &pvs);
        assert_eq!(g.rect, [5.0, 20.0, 100.0, 200.0]);
        assert_eq!(g.children[0].rect[..2], [10.0, 26.0]);
    }

    #[test]
    fn a_pv_name_rule_connects_the_widget_to_that_pv() {
        let pvs = PvManager::new();
        let mut s = build(
            r#"<widget type="textupdate"><pv_name>loc://rule_pv_a</pv_name>
              <rules><rule name="pv" prop_id="pv_name" out_exp="false">
                <exp bool_exp="pv0 == 1"><value>loc://rule_pv_$(which)</value></exp>
                <pv_name>loc://rule_pv_sel(1)</pv_name></rule></rules>
              <macros><which>b</which></macros>
            </widget>"#,
            &pvs,
        );
        let n = &mut s.nodes[0];
        let sel = n.rules[0].pvs()[0].clone();
        wait_connected(&sel);
        crate::widgets::update_rules(n, &pvs);
        assert_eq!(n.pv_name, "loc://rule_pv_b");
        assert_eq!(n.pv.as_ref().unwrap().name().to_string(), "loc://rule_pv_b");
        sel.write("0");
        crate::widgets::update_rules(n, &pvs);
        assert_eq!(n.pv.as_ref().unwrap().name().to_string(), "loc://rule_pv_a");
    }

    #[test]
    fn vertical_tabs_put_their_content_right_of_the_tabs() {
        let pvs = PvManager::new();
        let s = build(
            r#"<widget type="tabs"><x>10</x><y>20</y><direction>1</direction><tab_width>80</tab_width>
              <tabs><tab><name>A</name><children><widget type="label"><x>1</x><y>2</y></widget></children></tab></tabs>
            </widget>
            <widget type="tabs"><x>10</x><y>20</y><tab_height>25</tab_height>
              <tabs><tab><name>A</name><children><widget type="label"><x>1</x><y>2</y></widget></children></tab></tabs>
            </widget>"#,
            &pvs,
        );
        assert_eq!(s.nodes[0].tabs[0][0].rect[..2], [91.0, 22.0]);
        assert_eq!(s.nodes[1].tabs[0][0].rect[..2], [11.0, 47.0]);
    }

    #[test]
    fn macros_fall_back_to_the_environment_only_from_files() {
        let pvs = PvManager::new();
        let mut s = build(
            r#"<widget type="label"><text>$(PATH)</text></widget>"#,
            &pvs,
        );
        let n = &mut s.nodes[0];
        let path = std::env::var("PATH").unwrap();
        n.base = Some(Source::parse("/tmp/x.bob"));
        assert_eq!(n.prop("text"), path);
        // A display's own macros come first.
        let own = Macros::from_pairs([("PATH".to_string(), "mine".to_string())]);
        assert_eq!(n.expand_with(&own, "$(PATH)"), "mine");
        n.base = Some(Source::parse("https://example.com/x.bob"));
        assert_eq!(n.prop("text"), "$(PATH)");
        n.base = None;
        assert_eq!(n.prop("text"), "$(PATH)");
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
