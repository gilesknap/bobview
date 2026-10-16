//! The EDM menu mux of a BOY `.opi`: a drop-down whose items each stand
//! for a set of values, one per target PV (often a `loc://` PV that stands
//! in for a macro). Picking item `i` writes `values{k}[i]` to `target{k}`.
//!
//! An `.opi` menu mux becomes a combo with a `<menu_mux>` property,
//! bobview's own, that keeps the sets:
//!
//! ```xml
//! <menu_mux>
//!   <index>false</index>     <!-- true: pv_name is the mux's own, and gets i -->
//!   <initial>1</initial>     <!-- the item whose values are written at load -->
//!   <set>
//!     <target>loc://$(DID)ll&lt;VString&gt;</target>
//!     <values><value>RATE</value><value>LOG</value></values>
//!   </set>
//! </menu_mux>
//! ```
//!
//! Without its own `pv_name` the combo is on the first target, and shows
//! the item whose value that target holds.

use crate::{Element, Widget};

/// One target and the value each item writes to it.
#[derive(Clone, Debug, PartialEq)]
pub struct MuxSet {
    /// The target PV, macros unexpanded.
    pub target: String,
    pub values: Vec<String>,
}

/// A combo's menu mux sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuMux {
    /// The combo's `pv_name` is the mux's own, not its first target: a pick
    /// writes the item's index to it, and its value is the index shown.
    pub index: bool,
    /// The item whose values are written when the display opens.
    pub initial: Option<usize>,
    pub sets: Vec<MuxSet>,
}

impl MenuMux {
    /// The item for a PV value: its index, or where it is among the first
    /// set's values (the first target's value is `text`).
    pub fn current(&self, index: Option<usize>, text: &str) -> Option<usize> {
        if self.index {
            return index;
        }
        let values = &self.sets.first()?.values;
        values.iter().position(|v| v == text)
    }

    /// The `(target, value)` writes for picking item `i`.
    pub fn writes(&self, i: usize) -> Vec<(&str, &str)> {
        self.sets
            .iter()
            .filter_map(|s| Some((s.target.as_str(), s.values.get(i)?.as_str())))
            .collect()
    }
}

impl Widget {
    /// The widget's `<menu_mux>`, if it has one.
    pub fn menu_mux(&self) -> Option<MenuMux> {
        let el = self.get("menu_mux")?;
        let sets = el
            .children
            .iter()
            .filter(|c| c.name == "set")
            .map(|s| MuxSet {
                target: s.child_text("target").unwrap_or("").trim().to_string(),
                values: s
                    .child("values")
                    .map(|v| v.children.iter().map(|c| c.text.clone()).collect())
                    .unwrap_or_default(),
            })
            .collect();
        Some(MenuMux {
            index: el.child_text("index").map(str::trim) == Some("true"),
            initial: el.child_text("initial").and_then(|t| t.trim().parse().ok()),
            sets,
        })
    }
}

/// The `<menu_mux>` for a BOY menu mux `el`, from its `pv_name`,
/// `num_sets`, `target0`…`target7`, `values0`…`values7` and `initial`.
/// Sets with no target are left out, as there is nothing to write.
pub(crate) fn from_opi(el: &Element) -> Element {
    let text = |name: &str| el.child_text(name).map(str::trim).unwrap_or("");
    let num_sets = text("num_sets").parse().unwrap_or(1usize).min(8);
    let mut out = Element::new("menu_mux").with_child(Element::with_text(
        "index",
        (!text("pv_name").is_empty()).to_string(),
    ));
    if !text("initial").is_empty() {
        out.children
            .push(Element::with_text("initial", text("initial")));
    }
    for k in 0..num_sets {
        let target = text(&format!("target{k}"));
        if target.is_empty() {
            continue;
        }
        let mut values = Element::new("values");
        if let Some(v) = el.child(&format!("values{k}")) {
            values.children = v
                .children
                .iter()
                .map(|s| Element::with_text("value", s.text.clone()))
                .collect();
        }
        out.children.push(
            Element::new("set")
                .with_child(Element::with_text("target", target))
                .with_child(values),
        );
    }
    out
}
