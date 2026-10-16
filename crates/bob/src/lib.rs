//! Parser for Phoebus Display Builder `.bob` files, and for CS-Studio BOY
//! `.opi` files, which are translated into the same form (see [`opi`]).
//!
//! A display is kept close to the XML: each [`Widget`] holds its property
//! elements as they were written, and typed accessors read them with the
//! Phoebus defaults. Macros are expanded later, by whoever renders the
//! display, because their values depend on where a widget sits.

mod element;
mod group_link;
mod macros;
mod model;
mod opi;
pub mod project;
mod source;
mod trend;

use std::path::{Path, PathBuf};

pub use element::Element;
pub use group_link::{GroupLink, LinkSets, group_device, group_frame};
pub use macros::{MAX_LOAD_BYTES, Macros, expand, rearm_warning, spend, warn_spent};
pub use model::{Action, Color, Font, FontStyle, Rule, RuleExpr, named_color};
pub use source::Source;
pub use trend::{databrowser_pvs, strip_tool_pvs};

/// The largest display file read from disk, as for those fetched by URL.
const MAX_FILE_SIZE: u64 = 16 << 20;

/// The most XML nodes in a display, so a small file can't expand to a huge tree.
const MAX_NODES: u32 = 1 << 20;

/// A parsed `.bob` (or translated `.opi`) display.
#[derive(Clone, Debug)]
pub struct Display {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub background: Color,
    pub macros: Macros,
    pub widgets: Vec<Widget>,
    /// The file or URL it was read from, used to resolve relative paths.
    pub source: Option<Source>,
    /// Translated from a BOY `.opi` file. BOY clipped widgets to the
    /// declared size, where Phoebus grows the display to show them.
    pub legacy: bool,
    /// The size of the file's XML, in bytes.
    pub size: usize,
}

/// One widget: its type, property elements and child widgets.
#[derive(Clone, Debug)]
pub struct Widget {
    pub kind: String,
    pub props: Element,
    pub children: Vec<Widget>,
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Xml(roxmltree::Error),
    NotADisplay(String),
    /// A URL that could not be fetched, and why.
    Fetch(String, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            Error::Xml(e) => write!(f, "invalid XML: {e}"),
            Error::NotADisplay(root) => write!(f, "expected a <display>, found <{root}>"),
            Error::Fetch(url, why) => write!(f, "cannot fetch {url}: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl Display {
    /// Read the display file at `path`, which may lie in an Eclipse
    /// project's linked folder (see [`project`]). A real BOY screen beside a
    /// DLS launcher is taken through the launcher's project (see
    /// [`project::launcher_path`]), so the files it names do too; any other
    /// display reached through a launcher's project goes by its real path.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let real = project::locate(path);
        use std::io::Read;
        let io = |e| Error::Io(path.into(), e);
        let file = std::fs::File::open(&real).map_err(io)?;
        let mut text = String::new();
        file.take(MAX_FILE_SIZE + 1)
            .read_to_string(&mut text)
            .map_err(io)?;
        if text.len() as u64 > MAX_FILE_SIZE {
            let why = format!("larger than {} MiB", MAX_FILE_SIZE >> 20);
            return Err(io(std::io::Error::other(why)));
        }
        let mut display = Self::parse(&text)?;
        // A launcher's project is for BOY screens: a `.bob` reached through
        // one names its files from where it really is.
        let path = if display.legacy {
            project::launcher_path(path).unwrap_or_else(|| path.to_path_buf())
        } else if project::through_launcher(path) {
            real
        } else {
            path.to_path_buf()
        };
        display.source = Some(Source::File(path));
        Ok(display)
    }

    pub fn parse(xml: &str) -> Result<Self, Error> {
        let options = roxmltree::ParsingOptions {
            nodes_limit: MAX_NODES,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, options).map_err(Error::Xml)?;
        let mut root = Element::from_node(doc.root_element());
        if root.name != "display" {
            return Err(Error::NotADisplay(root.name));
        }
        let legacy = opi::is_opi(&root);
        if legacy {
            root = opi::translate(root);
        }
        let (props, widgets) = split_widgets(root);
        let as_widget = Widget {
            kind: "display".into(),
            props,
            children: Vec::new(),
        };
        Ok(Self {
            name: as_widget.text("name").unwrap_or("").to_string(),
            width: as_widget.f64("width").unwrap_or(800.0),
            height: as_widget.f64("height").unwrap_or(600.0),
            background: as_widget
                .color("background_color")
                .unwrap_or(named_color("Background")),
            macros: as_widget.macros(),
            widgets,
            source: None,
            legacy,
            size: xml.len(),
        })
    }

    /// Resolve a file named by this display (e.g. in an action) against its location.
    pub fn resolve(&self, file: &str) -> Source {
        match &self.source {
            Some(s) => s.resolve(file),
            None => Source::parse(file),
        }
    }

    /// Every widget, depth first.
    pub fn all_widgets(&self) -> Vec<&Widget> {
        fn walk<'a>(ws: &'a [Widget], out: &mut Vec<&'a Widget>) {
            for w in ws {
                out.push(w);
                walk(&w.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.widgets, &mut out);
        out
    }
}

/// Separate `<widget>` children from the property elements of `el`.
///
/// A property given more than once keeps its last value, as Phoebus reads it
/// (pvi writes a placeholder `<items>` list followed by the real one).
fn split_widgets(mut el: Element) -> (Element, Vec<Widget>) {
    let mut widgets = Vec::new();
    let mut props: Vec<Element> = Vec::new();
    for child in el.children.drain(..) {
        if child.name == "widget" {
            widgets.push(Widget::from_element(child));
        } else if let Some(prev) = props.iter_mut().find(|p| p.name == child.name) {
            *prev = child;
        } else {
            props.push(child);
        }
    }
    el.children = props;
    (el, widgets)
}

impl Widget {
    pub fn from_element(el: Element) -> Self {
        let kind = el.attr("type").unwrap_or("unknown").to_string();
        let (props, children) = split_widgets(el);
        Self {
            kind,
            props,
            children,
        }
    }

    /// The property element called `name`.
    pub fn get(&self, name: &str) -> Option<&Element> {
        self.props.child(name)
    }

    /// The text of a simple property.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).map(|e| e.text.as_str())
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.text(name).and_then(|t| t.trim().parse().ok())
    }

    pub fn i64(&self, name: &str) -> Option<i64> {
        self.f64(name).map(|v| v as i64)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.text(name).map(|t| t.trim() == "true")
    }

    pub fn color(&self, name: &str) -> Option<Color> {
        self.get(name)?.child("color").map(Color::from_element)
    }

    pub fn font(&self, name: &str) -> Option<Font> {
        self.get(name)?.child("font").map(Font::from_element)
    }

    pub fn name(&self) -> &str {
        self.text("name").unwrap_or("")
    }

    /// The widget's own `<macros>`.
    pub fn macros(&self) -> Macros {
        self.get("macros")
            .map(Macros::from_element)
            .unwrap_or_default()
    }

    /// `<items><item>…</item></items>`, as used by combo and choice widgets.
    pub fn items(&self) -> Vec<String> {
        self.list("items", "item")
    }

    /// Texts of `<outer><inner>…</inner>…</outer>`.
    pub fn list(&self, outer: &str, inner: &str) -> Vec<String> {
        self.get(outer)
            .map(|e| {
                e.children
                    .iter()
                    .filter(|c| c.name == inner)
                    .map(|c| c.text.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Points of a polyline or polygon, relative to the widget.
    pub fn points(&self) -> Vec<(f64, f64)> {
        let Some(points) = self.get("points") else {
            return Vec::new();
        };
        points
            .children
            .iter()
            .filter(|p| p.name == "point")
            .filter_map(|p| Some((p.attr("x")?.parse().ok()?, p.attr("y")?.parse().ok()?)))
            .collect()
    }

    pub fn actions(&self) -> Vec<Action> {
        self.get("actions")
            .map(|a| {
                a.children
                    .iter()
                    .filter(|c| c.name == "action")
                    .map(Action::from_element)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.get("rules")
            .map(|r| {
                r.children
                    .iter()
                    .filter(|c| c.name == "rule")
                    .map(Rule::from_element)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_scripts(&self) -> bool {
        self.get("scripts").is_some_and(|s| !s.children.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<display version="2.0.0">
  <name>Test</name>
  <width>400</width>
  <macros><P>BL01T</P></macros>
  <widget type="group" version="3.0.0">
    <name>G</name>
    <macros><R>:DET:</R></macros>
    <x>10</x>
    <widget type="textupdate" version="2.0.0">
      <name>TU</name>
      <pv_name>$(P)$(R)Acquire_RBV</pv_name>
      <foreground_color><color name="Text" red="1" green="2" blue="3"></color></foreground_color>
      <font><font name="Default Bold" family="Liberation Sans" style="BOLD" size="14.0"></font></font>
    </widget>
  </widget>
  <widget type="polyline" version="2.0.0">
    <points><point x="0.0" y="1.0"></point><point x="5.0" y="6.0"></point></points>
  </widget>
</display>"#;

    #[test]
    fn parses_tree_and_properties() {
        let d = Display::parse(SAMPLE).unwrap();
        assert_eq!(d.name, "Test");
        assert_eq!(d.width, 400.0);
        assert_eq!(d.height, 600.0);
        assert_eq!(d.macros.get("P"), Some("BL01T"));
        let g = &d.widgets[0];
        assert_eq!(g.kind, "group");
        assert_eq!(g.f64("x"), Some(10.0));
        assert_eq!(g.macros().get("R"), Some(":DET:"));
        let tu = &g.children[0];
        assert_eq!(tu.text("pv_name"), Some("$(P)$(R)Acquire_RBV"));
        assert_eq!(tu.color("foreground_color").unwrap().rgba(), [1, 2, 3, 255]);
        let f = tu.font("font").unwrap();
        assert_eq!((f.size, f.style), (14.0, FontStyle::Bold));
        assert_eq!(d.widgets[1].points(), vec![(0.0, 1.0), (5.0, 6.0)]);
        assert_eq!(d.all_widgets().len(), 3);
    }

    #[test]
    fn rejects_other_roots() {
        assert!(matches!(Display::parse("<x/>"), Err(Error::NotADisplay(_))));
    }

    #[test]
    fn repeated_property_keeps_the_last() {
        let d = Display::parse(
            r#"<display version="2.0.0"><widget type="combo">
              <items><item>item 0</item><item>Item 1</item><item>Item 2</item></items>
              <x>1</x>
              <items><item>Disable</item><item>Enable</item></items>
            </widget></display>"#,
        )
        .unwrap();
        let w = &d.widgets[0];
        assert_eq!(w.items(), vec!["Disable", "Enable"]);
        assert_eq!(w.f64("x"), Some(1.0));
    }
}
