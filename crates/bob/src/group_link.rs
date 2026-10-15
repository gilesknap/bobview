//! The DLS vacuum `get_pv_name.py` widget script, which bobview does
//! natively instead of running it: a widget that shows whichever group a
//! vacuum space links to. The script reads one of the space's link fields,
//! such as `SR03S-VA-SPACE-01:IONPSTA.INPA` reading
//! `SR03S-VA-GIONP-01:STA NPP NMS`, and points the widget at that group: a
//! label shows the group's device name, anything else reads the group's
//! `:STA` (and is named after the group, so `$(name)` in its actions is the
//! group), and an EDM symbol shows the frame of its strip for that status.
//!
//! An `.opi` widget with the script is translated with a `<group_link>`
//! property, bobview's own, saying all that the script would decide:
//!
//! ```xml
//! <group_link>
//!   <pv_name>$(device):MAXSTA.INPA</pv_name>  <!-- the link field -->
//!   <sets>pv</sets>                           <!-- or "text", for a label -->
//!   <frames>9</frames>                        <!-- a symbol's top frame -->
//! </group_link>
//! ```

use crate::{Element, Widget};

/// The script, recognised by its file name.
const SCRIPT: &str = "get_pv_name.py";

/// The top frame of each symbol strip the script knows, by image file name:
/// frame `status + 1`, frame 0 being the picture for a PV that is not
/// connected. A symbol with any other image keeps its `image_index`.
const SYMBOL_FRAMES: &[(&str, i64)] = &[
    ("mks937aImg-symbol-34.png", 16),
    ("mks937bImg-symbol-34.png", 16),
    ("mks937aPirg-symbol-34.png", 16),
    ("mks937bPirg-symbol-34.png", 16),
    ("digitelMpcIonp-symbol-34.png", 9),
    ("vacuumValve-symbol-34.png", 5),
];

/// The macro the first action, opening the group's display, passes the
/// group's device name to.
const DEVICE_MACRO: &str = "device";

/// What a widget's [`GroupLink`] points at the group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSets {
    /// The `text`: the group's device name.
    Text,
    /// The `pv_name`: the group's `:STA`, with `name`, `tooltip` and the
    /// first action's `device` macro following.
    Pv,
}

/// A widget that shows the group a vacuum space's link field names.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLink {
    /// The link field, macros unexpanded.
    pub pv_name: String,
    pub sets: LinkSets,
    /// For an EDM symbol with a known strip, its top frame: its
    /// `image_index` then follows the group's `:STA`.
    pub frames: Option<i64>,
}

impl Widget {
    /// The widget's `<group_link>`, if it has one.
    pub fn group_link(&self) -> Option<GroupLink> {
        let el = self.get("group_link")?;
        let pv_name = el.child_text("pv_name")?.trim().to_string();
        let sets = match el.child_text("sets").map(str::trim) {
            Some("text") => LinkSets::Text,
            _ => LinkSets::Pv,
        };
        let frames = el.child_text("frames").and_then(|f| f.trim().parse().ok());
        (!pv_name.is_empty()).then_some(GroupLink {
            pv_name,
            sets,
            frames,
        })
    }
}

impl GroupLink {
    /// Point the widget properties `props` at the group `device`, as the
    /// script does with `setPropertyValue`.
    pub fn point_at(&self, props: &mut Element, device: &str) {
        let mut set = |name: &str, value: String| props.set_child(Element::with_text(name, value));
        match self.sets {
            LinkSets::Text => set("text", device.to_string()),
            LinkSets::Pv => {
                set("pv_name", format!("{device}:STA"));
                set("name", device.to_string());
                // BOY expanded the tooltip once, at load, so the script
                // names the PV in it; bobview expands it as it draws.
                set("tooltip", "$(pv_name)\n$(pv_value)".to_string());
                if let Some(macros) = props
                    .child_mut("actions")
                    .and_then(|a| a.children.first_mut())
                    .filter(|a| a.attr("type") == Some("open_display"))
                    .map(|a| {
                        if a.child("macros").is_none() {
                            a.children.push(Element::new("macros"));
                        }
                        a.child_mut("macros").unwrap()
                    })
                {
                    macros.set_child(Element::with_text(DEVICE_MACRO, device));
                }
            }
        }
    }
}

/// The group's device name in the value of a link field: everything before
/// the first `:` of its first word, so `SR03S-VA-GPIRG-01:CTL:SP NPP MS` is
/// `SR03S-VA-GPIRG-01`. A `scheme://` before the name is kept with it, so a
/// link to `loc://G:STA` names the group `loc://G`.
pub fn group_device(link: &str) -> Option<String> {
    let pv = link.split_whitespace().next()?;
    let (scheme, name) = match pv.split_once("://") {
        Some((scheme, name)) => (&pv[..scheme.len() + 3], name),
        None => ("", pv),
    };
    let device = name.split(':').next().unwrap_or("");
    (!device.is_empty()).then(|| format!("{scheme}{device}"))
}

/// The frame of a strip with top frame `top` for a group's status: `status
/// + 1`, or 0 for a status past the strip or none (a PV not connected).
pub fn group_frame(status: Option<i64>, top: i64) -> i64 {
    match status.map(|s| s + 1) {
        Some(frame) if (1..=top).contains(&frame) => frame,
        _ => 0,
    }
}

/// The `<group_link>` for a BOY widget `el` that runs the script, from its
/// `<scripts>` (the link field is the script's second PV), `widget_type`
/// and `image_file`.
pub(crate) fn from_opi(el: &Element) -> Option<Element> {
    let path = el
        .child("scripts")?
        .children
        .iter()
        .filter(|p| p.name == "path")
        .find(|p| {
            let path = p.attr("pathString").unwrap_or("");
            path.rsplit(['/', '\\']).next() == Some(SCRIPT)
        })?;
    let link = path.children.iter().filter(|p| p.name == "pv").nth(1)?;
    let widget_type = el.child_text("widget_type").unwrap_or("").trim();
    let mut out =
        Element::new("group_link").with_child(Element::with_text("pv_name", link.text.trim()));
    if widget_type == "Label" {
        return Some(out.with_child(Element::with_text("sets", "text")));
    }
    out.children.push(Element::with_text("sets", "pv"));
    if widget_type == "Edm Symbol Widget" {
        let image = el.child_text("image_file").unwrap_or("").trim();
        let image = image.rsplit(['/', '\\']).next().unwrap_or("");
        if let Some((_, top)) = SYMBOL_FRAMES.iter().find(|(f, _)| *f == image) {
            out.children
                .push(Element::with_text("frames", top.to_string()));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_fields_name_the_group() {
        let device = |l: &str| group_device(l);
        assert_eq!(
            device("SR03S-VA-GIONP-01:STA NPP NMS").as_deref(),
            Some("SR03S-VA-GIONP-01")
        );
        assert_eq!(
            device("SR03S-VA-GVALV-01:STA NPP MS").as_deref(),
            Some("SR03S-VA-GVALV-01")
        );
        assert_eq!(
            device("SR03S-VA-GPIRG-01:CTL:SP NPP MS").as_deref(),
            Some("SR03S-VA-GPIRG-01")
        );
        assert_eq!(
            device("  BL01T-VA-IMG-01 CA").as_deref(),
            Some("BL01T-VA-IMG-01")
        );
        assert_eq!(device("loc://G1:STA").as_deref(), Some("loc://G1"));
        assert_eq!(device(""), None);
        assert_eq!(device("   "), None);
        assert_eq!(device(":STA NPP"), None);
    }

    #[test]
    fn status_picks_a_frame_within_the_strip() {
        assert_eq!(group_frame(Some(0), 9), 1);
        assert_eq!(group_frame(Some(8), 9), 9);
        // Past the strip, below it, or not connected: the blank frame.
        assert_eq!(group_frame(Some(9), 9), 0);
        assert_eq!(group_frame(Some(-1), 9), 0);
        assert_eq!(group_frame(Some(5), 5), 0);
        assert_eq!(group_frame(None, 16), 0);
    }

    #[test]
    fn point_at_sets_what_the_script_does() {
        let props = |xml: &str| {
            let doc = roxmltree::Document::parse(xml).unwrap();
            Element::from_node(doc.root_element())
        };
        let mut sym = props(
            r#"<widget><name></name><pv_name></pv_name><actions>
            <action type="open_display"><file>g.opi</file><macros><device>$(name)</device></macros></action>
            </actions></widget>"#,
        );
        let link = GroupLink {
            pv_name: "L".into(),
            sets: LinkSets::Pv,
            frames: Some(9),
        };
        link.point_at(&mut sym, "SR03S-VA-GIONP-01");
        let w = Widget::from_element(sym);
        assert_eq!(w.text("pv_name"), Some("SR03S-VA-GIONP-01:STA"));
        assert_eq!(w.name(), "SR03S-VA-GIONP-01");
        let crate::Action::OpenDisplay { macros, .. } = &w.actions()[0] else {
            panic!("not an open display action")
        };
        assert_eq!(macros.get("device"), Some("SR03S-VA-GIONP-01"));

        let mut label = props("<widget><text>SRXXC-VA-GXXX-YY</text></widget>");
        let link = GroupLink {
            sets: LinkSets::Text,
            frames: None,
            ..link
        };
        link.point_at(&mut label, "SR03S-VA-GIONP-01");
        let w = Widget::from_element(label);
        assert_eq!(w.text("text"), Some("SR03S-VA-GIONP-01"));
        assert!(w.get("pv_name").is_none());
    }
}
