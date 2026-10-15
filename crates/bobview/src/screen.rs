//! A loaded display: widgets with resolved positions, macros and PVs.

use bob::{Display, Macros, Source, Widget};
use pv::{Pv, PvManager};

use crate::rules::CompiledRule;

/// How deep embedded displays may nest before we stop (guards against loops).
const MAX_DEPTH: u32 = 8;

/// A display ready to draw. Node rectangles are in this screen's own coordinates.
pub struct Screen {
    pub display: Display,
    pub nodes: Vec<Node>,
    pub width: f64,
    pub height: f64,
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
    /// The display file or URL this node came from, to resolve relative paths.
    pub base: Option<Source>,
    /// The image widget's texture and the frame it shows.
    pub image: Option<crate::image::ImageCache>,
    /// The symbol or picture widget's texture.
    pub symbol: Option<crate::symbol::Texture>,
    /// The x and y PVs of each trace of an XY plot.
    pub traces: Vec<crate::plot::TracePvs>,
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
        expand_for(&self.w, &self.macros, text)
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
        match &self.base {
            Some(base) => base.resolve(file),
            None => Source::parse(file),
        }
    }
}

fn expand_for(w: &Widget, macros: &Macros, text: &str) -> String {
    bob::expand(text, &|name| {
        macros.get(name).map(str::to_string).or_else(|| {
            w.text(name)
                .filter(|t| !t.contains('$'))
                .map(str::to_string)
        })
    })
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
        let nodes = self.nodes(&widgets, (0.0, 0.0), &macros, base.as_ref(), depth);
        // Phoebus shows widgets beyond the declared display size, so grow to fit them.
        let (width, height) = nodes
            .iter()
            .filter(|n| n.w.bool("visible") != Some(false))
            .fold((width, height), |(w, h), n| {
                (w.max(n.rect[0] + n.rect[2]), h.max(n.rect[1] + n.rect[3]))
            });
        Screen {
            display,
            nodes,
            width,
            height,
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
            action_pvs,
            edit: None,
            error: None,
            base: base.cloned(),
            image: None,
            symbol: None,
            traces: Vec::new(),
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
        if file.is_empty() {
            return;
        }
        if depth >= MAX_DEPTH {
            node.error = Some("embedded displays nested too deeply".into());
            return;
        }
        let source = node.resolve(file);
        match crate::fetch::load(&source) {
            Ok(d) => node.embedded = Some(Box::new(self.screen(d, macros, Some(group), depth + 1))),
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

fn find_group<'a>(widgets: &'a [Widget], name: &str) -> Option<&'a Widget> {
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
    fn display_macros_are_overridden_by_callers() {
        let d = Display::parse(
            "<display><macros><P>A</P><Q>q</Q></macros><widget type='led'><pv_name>loc://$(P)$(Q)</pv_name></widget></display>",
        )
        .unwrap();
        let pvs = PvManager::new();
        let s = Builder { pvs: &pvs }.screen(d, &Macros::from_pairs([("P", "B")]), None, 0);
        assert_eq!(s.nodes[0].pv_name, "loc://Bq");
    }
}

/// Checks over the repo's test display set.
#[cfg(test)]
pub mod display_set {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Widget types bobview draws (anything else gets a placeholder).
    const KNOWN: &[&str] = &[
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
    ];

    pub fn files() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "bob") {
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
                KNOWN.contains(&n.w.kind.as_str()),
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
