//! Widgets that show the group a vacuum space's link field names, as the
//! DLS `get_pv_name.py` script makes them (see [`bob::GroupLink`]).
//!
//! The link field is subscribed with the widget, and the widget is pointed
//! at the group each time the field names a different one: its properties
//! change as the script would set them, so its tooltip, actions and PV menu
//! follow, and its PV becomes the group's `:STA`, dropping the one before.
//! The script polls once a second; a link changes rarely, so this does
//! nothing until its value does.

use pv::{Pv, PvManager, PvState, Value};

use crate::screen::Node;

/// A widget's link field, and the group it last named.
pub struct Link {
    spec: bob::GroupLink,
    pv: Pv,
    /// The group the widget is pointed at, once the link has named one.
    device: Option<String>,
}

impl Link {
    /// The link of widget `w`, its field's name expanded with `expand`.
    pub fn new(w: &bob::Widget, expand: &dyn Fn(&str) -> String, pvs: &PvManager) -> Option<Self> {
        let spec = w.group_link()?;
        let name = expand(&spec.pv_name);
        if name.trim().is_empty() {
            return None;
        }
        Some(Link {
            pv: pvs.subscribe(&name),
            spec,
            device: None,
        })
    }
}

/// The group a link field's state names, if it is connected and names one.
fn device(s: &PvState) -> Option<String> {
    if !s.connected {
        return None;
    }
    match &s.value {
        Value::Str(t) => bob::group_device(t),
        _ => None,
    }
}

/// Point `node` at the group its link names now, if that is a new one.
/// A link that is not connected leaves the widget as it was.
pub fn update(node: &mut Node, pvs: &PvManager) {
    let Some(link) = &node.link else { return };
    let Some(device) = device(&link.pv.state()) else {
        return;
    };
    if link.device.as_deref() == Some(device.as_str()) {
        return;
    }
    log::debug!("{}: {} names {device}", node.w.name(), link.pv.name());
    let spec = link.spec.clone();
    spec.point_at(&mut node.w.props, &device);
    // Rules start from the properties as they were, which now include these.
    if let Some(orig) = &mut node.orig_props {
        spec.point_at(orig, &device);
    }
    if spec.sets == bob::LinkSets::Pv {
        node.pv_name = node.prop("pv_name");
        node.pv = Some(pvs.subscribe(&node.pv_name));
    }
    if let Some(link) = &mut node.link {
        link.device = Some(device);
    }
}

/// The frame a linked symbol shows for its group's status `s`, once it is
/// pointed at one; `None` leaves it to `image_index`.
pub fn frame(node: &Node, s: Option<&PvState>) -> Option<i64> {
    let link = node.link.as_ref()?;
    let top = link.spec.frames?;
    link.device.as_ref()?;
    let status = s.filter(|s| s.connected).and_then(|s| s.value.as_i64());
    Some(bob::group_frame(status, top))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Builder;
    use bob::{Display, Macros};

    #[test]
    fn linked_widgets_follow_their_link() {
        // As the DLS rack screens build them, with loc:// PVs: a symbol
        // whose click opens the group's display, and a label naming it.
        let d = Display::parse(
            r#"<display><macros><space>loc://repoint_test</space></macros>
            <widget type="symbol"><name></name><pv_name></pv_name>
              <tooltip>$(pv_name)
$(pv_value)</tooltip><sub_image_width>34</sub_image_width>
              <actions><action type="open_display"><file>gionp.opi</file><macros><device>$(name)</device></macros></action></actions>
              <group_link><pv_name>$(space)("")</pv_name><sets>pv</sets><frames>9</frames></group_link></widget>
            <widget type="label"><text>SRXXC-VA-GXXX-YY</text>
              <group_link><pv_name>$(space)</pv_name><sets>text</sets></group_link></widget>
            </display>"#,
        )
        .unwrap();
        let pvs = PvManager::new();
        let mut s = Builder::new(&pvs).screen(d, &Macros::default(), None, 0);
        let link = s.nodes[0].link.as_ref().unwrap().pv.clone();

        // An empty link leaves both as they were loaded.
        for n in &mut s.nodes {
            update(n, &pvs);
        }
        assert!(s.nodes[0].pv.is_none());
        assert_eq!(frame(&s.nodes[0], None), None);
        assert_eq!(s.nodes[1].prop("text"), "SRXXC-VA-GXXX-YY");

        link.write("loc://G1:STA NPP NMS");
        for n in &mut s.nodes {
            update(n, &pvs);
        }
        let sym = &s.nodes[0];
        assert_eq!(sym.pv_name, "loc://G1:STA");
        assert_eq!(sym.pv.as_ref().unwrap().name().to_string(), "loc://G1:STA");
        assert_eq!(sym.expand("$(name)"), "loc://G1");
        assert_eq!(s.nodes[1].prop("text"), "loc://G1");
        let bob::Action::OpenDisplay { macros, .. } = &sym.w.actions()[0] else {
            panic!("not an open display action")
        };
        assert_eq!(macros.get("device"), Some("loc://G1"));
        // The group's status picks the frame: status + 1, within the strip.
        let sta = sym.pv.as_ref().unwrap();
        assert_eq!(frame(sym, sym.state().as_ref()), Some(1));
        sta.write("3");
        assert_eq!(frame(sym, sym.state().as_ref()), Some(4));
        sta.write("9");
        assert_eq!(frame(sym, sym.state().as_ref()), Some(0));

        // A new group re-points the widget at its :STA.
        link.write("loc://G2:CTL:SP NPP MS");
        for n in &mut s.nodes {
            update(n, &pvs);
        }
        let sym = &s.nodes[0];
        assert_eq!(sym.pv_name, "loc://G2:STA");
        assert_eq!(sym.expand("$(name)"), "loc://G2");
        assert_eq!(s.nodes[1].prop("text"), "loc://G2");
        // A write to the same group, through another field, keeps it.
        link.write("loc://G2:STA NPP NMS");
        update(&mut s.nodes[0], &pvs);
        assert_eq!(s.nodes[0].pv_name, "loc://G2:STA");
    }
}
