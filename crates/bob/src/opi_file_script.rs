//! The embedded JavaScript a BOY linking container runs to choose its
//! display, which bobview reads when it translates the `.opi` instead of
//! running it. The DLS EDM converter writes one per container,
//! `OPIFileScript`, triggered by a local PV that nothing writes:
//!
//! ```js
//! var pv0 = PVUtil.getLong(pvs[0]);   // loc://menu0<VDouble>(0)
//! if(pv0==0){
//! var macroInput = DataUtil.createMacrosInput(true);
//! macroInput.put("device", "LI-DI-YAG-01");
//! widget.setPropertyValue("macros", macroInput);
//! widget.setPropertyValue("opi_file","../camera/cameras-line-gige.opi");
//! }
//! ```
//!
//! and the camera control panels pick theirs from a macro the same way:
//!
//! ```js
//! var pv0 = PVUtil.getString(pvs[0]); // loc://$(DID)dummy<VString>("$(control)")
//! widget.setPropertyValue("opi_file", pv0 + ".opi");
//! ```
//!
//! With the PV at its initial value, the script's `opi_file` and `macros`
//! are known up front: `$(control).opi` above, expanded as the container
//! loads like any other file name.

use crate::Element;

/// The `opi_file` and, if the script sets them, the `macros` the linking
/// container `el`'s script gives it, or `None` if it has no script of a
/// shape bobview knows.
pub(crate) fn from_opi(el: &Element) -> Option<(String, Option<Element>)> {
    let mut paths = el
        .child("scripts")?
        .children
        .iter()
        .filter(|p| p.name == "path");
    let path = paths.next().filter(|_| paths.next().is_none())?;
    let script = path.child_text("scriptText")?;
    let pv = path.children.iter().find(|p| p.name == "pv")?;
    let value = loc_initial(pv.text.trim())?;
    // The variable the script reads `pvs[0]` into.
    let var = script
        .split(';')
        .find_map(|s| {
            let (var, get) = s.trim().strip_prefix("var ")?.split_once('=')?;
            let get = get.trim();
            (get.starts_with("PVUtil.get") && get.ends_with("(pvs[0])")).then(|| var.trim())
        })
        .unwrap_or("");
    let body = branch(script, var, &value)?;

    let file = set_property(body, "opi_file")?;
    let file = file
        .split('+')
        .map(|term| match term.trim() {
            t if t == var || t == "PVUtil.getString(pvs[0])" => Some(value.clone()),
            t => string(t).map(str::to_string),
        })
        .collect::<Option<String>>()?;
    let macros = set_property(body, "macros").map(|_| {
        let mut macros = Element::new("macros");
        for put in body.split(".put(").skip(1) {
            if let Some((name, value)) = put
                .split_once(");")
                .and_then(|(args, _)| args.split_once(','))
                .and_then(|(n, v)| Some((string(n.trim())?, string(v.trim())?)))
            {
                macros.children.push(Element::with_text(name, value));
            }
        }
        macros
    });
    Some((file, macros))
}

/// The part of `script` that runs for `value` of `var`: the body of its
/// `if(var==K){...}` with `K` equal to `value`, or all of it if it has no
/// `if`.
fn branch<'a>(script: &'a str, var: &str, value: &str) -> Option<&'a str> {
    if !script.contains("if(") {
        return Some(script);
    }
    let value: f64 = value.trim().parse().ok()?;
    script.split("if(").skip(1).find_map(|s| {
        let (cond, rest) = s.split_once(')')?;
        let (lhs, k) = cond.split_once("==")?;
        let k: f64 = k.trim().parse().ok()?;
        let body = rest.trim_start().strip_prefix('{')?;
        (lhs.trim() == var && k == value).then(|| body.split('}').next().unwrap_or(body))
    })
}

/// The expression `script` passes to `widget.setPropertyValue("name", ...)`.
fn set_property<'a>(script: &'a str, name: &str) -> Option<&'a str> {
    let call = format!("widget.setPropertyValue(\"{name}\",");
    let (_, rest) = script.split_once(&call)?;
    Some(rest.split_once(");")?.0.trim())
}

/// The text of a JavaScript string literal.
fn string(s: &str) -> Option<&str> {
    s.strip_prefix('"')?.strip_suffix('"')
}

/// The initial value of a local PV, `loc://name<VType>(value)` or
/// `loc://name(value)`, unquoted: `0`, or `$(control)` for
/// `loc://$(DID)dummy<VString>("$(control)")`.
pub(crate) fn loc_initial(pv: &str) -> Option<String> {
    let name = pv.strip_prefix("loc://")?;
    // The first `(` not opening a macro in the name.
    let open = name
        .char_indices()
        .find(|&(i, c)| c == '(' && !name[..i].ends_with('$'))?
        .0;
    let value = name[open + 1..].strip_suffix(')')?.trim();
    Some(string(value).unwrap_or(value).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(script: &str, pv: &str) -> Element {
        let pv = pv.replace('<', "&lt;").replace('>', "&gt;");
        let xml = format!(
            r#"<widget><scripts><path pathString="EmbeddedJs"><scriptName>S</scriptName>
            <scriptText><![CDATA[{script}]]></scriptText><pv trig="true">{pv}</pv></path></scripts></widget>"#
        );
        let doc = roxmltree::Document::parse(&xml).unwrap();
        Element::from_node(doc.root_element())
    }

    #[test]
    fn local_pvs_start_at_their_initial_value() {
        assert_eq!(loc_initial("loc://menu0<VDouble>(0)").as_deref(), Some("0"));
        assert_eq!(
            loc_initial(r#"loc://$(DID)dummy<VString>("$(control)")"#).as_deref(),
            Some("$(control)")
        );
        assert_eq!(loc_initial("loc://x(3)").as_deref(), Some("3"));
        assert_eq!(loc_initial("loc://x"), None);
        assert_eq!(loc_initial("SR-DI-X:Y"), None);
    }

    #[test]
    fn opi_file_scripts_pick_the_branch_for_the_initial_value() {
        let script = r#"importPackage(Packages.org.csstudio.opibuilder.scriptUtil);
var pv0 = PVUtil.getLong(pvs[0]);
if(pv0==1){
widget.setPropertyValue("opi_file","other.opi");
}
if(pv0==0){
var macroInput = DataUtil.createMacrosInput(true);
macroInput.put("camera", "LI-DI-CAM-01");
macroInput.put("alias", "");
widget.setPropertyValue("macros", macroInput);
widget.setPropertyValue("opi_file","../camera/cameras-line-gige.opi");
}
"#;
        let (file, macros) = from_opi(&container(script, "loc://menu0<VDouble>(0)")).unwrap();
        assert_eq!(file, "../camera/cameras-line-gige.opi");
        let macros = macros.unwrap();
        let m: Vec<_> = macros
            .children
            .iter()
            .map(|c| (c.name.as_str(), c.text.as_str()))
            .collect();
        assert_eq!(m, [("camera", "LI-DI-CAM-01"), ("alias", "")]);
        // No branch for the value: the script sets nothing.
        assert!(from_opi(&container(script, "loc://menu0<VDouble>(2)")).is_none());
    }

    #[test]
    fn opi_file_scripts_can_name_the_file_by_the_pv() {
        let script = r#"importPackage(Packages.org.csstudio.opibuilder.scriptUtil);

var pv0 = PVUtil.getString(pvs[0]);
widget.setPropertyValue("opi_file", pv0 + ".opi");
"#;
        let el = container(script, r#"loc://$(DID)dummy<VString>("$(control)")"#);
        assert_eq!(from_opi(&el), Some(("$(control).opi".to_string(), None)));
        // Other scripts, or a PV that is not local, are left alone.
        assert!(from_opi(&container(script, "SR-DI-X:Y")).is_none());
        let other = r#"widget.setPropertyValue("opi_file", PVUtil.getDouble(pvs[0]) * 2);"#;
        assert!(from_opi(&container(other, "loc://x(1)")).is_none());
    }
}
