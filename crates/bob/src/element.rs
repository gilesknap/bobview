/// An owned XML element: enough of the DOM to read `.bob` properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// Text content, trimmed, when the element has no child elements.
    pub text: String,
    pub children: Vec<Element>,
}

/// Deepest element nesting accepted. Everything that walks the tree
/// (`from_node`, `split_widgets`, `Drop`) recurses once per level.
/// The deepest real display nests about 8 levels; a small cap also bounds the
/// stack used when displays are embedded in one another.
pub const MAX_DEPTH: usize = 64;

/// The deepest element nesting in `xml`, found by a quick scan without
/// recursion, so it can run before the (recursive) XML parser.
///
/// It only has to be right for documents the parser accepts: comments, CDATA
/// and processing instructions are skipped, quotes in tags are honoured, and
/// anything else odd ends the scan. A `<!DOCTYPE>` (which usvg accepts) is
/// skipped too, but entities can expand to markup the scan never sees, so a
/// DTD declaring any entity value with markup or references in it counts as
/// infinitely deep. Plain entities, such as the namespace URLs Adobe
/// Illustrator declares, are fine.
pub fn depth(xml: &str) -> usize {
    let b = xml.as_bytes();
    let (mut i, mut open, mut deepest) = (0, 0usize, 0usize);
    let skip = |i: usize, end: &[u8]| -> usize {
        b[i..]
            .windows(end.len())
            .position(|w| w == end)
            .map_or(b.len(), |p| i + p + end.len())
    };
    while let Some(p) = b[i..].iter().position(|&c| c == b'<') {
        i += p;
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip(i + 2, b"?>");
        } else if rest.starts_with(b"<!DOCTYPE") {
            match doctype_end(b, i + 9) {
                Some(end) => i = end,
                None => return usize::MAX,
            }
        } else if rest.starts_with(b"<!") {
            break;
        } else if rest.starts_with(b"</") {
            open = open.saturating_sub(1);
            i = skip(i + 2, b">");
        } else {
            // A start tag: find its closing `>`, outside any quoted value.
            let mut j = i + 1;
            let mut quote = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (None, b'>') => break,
                    (None, q @ (b'"' | b'\'')) => quote = Some(q),
                    (Some(q), c) if c == q => quote = None,
                    _ => {}
                }
                j += 1;
            }
            deepest = deepest.max(open + 1);
            if j == 0 || b.get(j - 1) != Some(&b'/') {
                open += 1;
            }
            i = j + 1;
        }
        if i >= b.len() {
            break;
        }
    }
    deepest
}

/// Where the `<!DOCTYPE` whose name starts at `i` ends, or `None` if its
/// internal subset declares something that could expand to markup: a quoted
/// value holding `<`, `&` or `%` (character and parameter entity references
/// can produce markup too).
fn doctype_end(b: &[u8], mut i: usize) -> Option<usize> {
    let mut subset = false;
    while i < b.len() {
        let rest = &b[i..];
        if subset && rest.starts_with(b"<!--") {
            i += 4 + rest[4..].windows(3).position(|w| w == b"-->")? + 3;
            continue;
        }
        if subset && rest.starts_with(b"<?") {
            i += 2 + rest[2..].windows(2).position(|w| w == b"?>")? + 2;
            continue;
        }
        match b[i] {
            q @ (b'"' | b'\'') => {
                let len = rest[1..].iter().position(|&c| c == q)?;
                let value = &rest[1..1 + len];
                if subset && value.iter().any(|c| matches!(c, b'<' | b'&' | b'%')) {
                    return None;
                }
                i += len + 2;
                continue;
            }
            b'[' if !subset => subset = true,
            b']' if subset => subset = false,
            // A parameter entity reference in the subset can pull in
            // declarations from a value we have not seen expanded.
            b'%' if subset => return None,
            b'>' if !subset => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    Some(b.len())
}

impl Element {
    pub(crate) fn from_node(node: roxmltree::Node) -> Self {
        let children: Vec<Element> = node
            .children()
            .filter(|c| c.is_element())
            .map(Element::from_node)
            .collect();
        let text = if children.is_empty() {
            node.children()
                .filter_map(|c| c.text())
                .collect::<String>()
                .trim()
                .to_string()
        } else {
            String::new()
        };
        Self {
            name: node.tag_name().name().to_string(),
            attrs: node
                .attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect(),
            text,
            children,
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.as_str())
    }
}

/// Builders and edits, used to translate `.opi` files.
impl Element {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub(crate) fn with_text(name: &str, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::new(name)
        }
    }

    pub(crate) fn with_attr(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_attr(name, value);
        self
    }

    pub(crate) fn with_child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    pub(crate) fn set_attr(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((name.to_string(), value)),
        }
    }

    pub(crate) fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    /// Remove and return the first child called `name`.
    pub(crate) fn take_child(&mut self, name: &str) -> Option<Element> {
        let i = self.children.iter().position(|c| c.name == name)?;
        Some(self.children.remove(i))
    }

    /// Replace the child called `child.name`, or add it.
    pub(crate) fn set_child(&mut self, child: Element) {
        match self.child_mut(&child.name) {
            Some(c) => *c = child,
            None => self.children.push(child),
        }
    }
}

#[cfg(test)]
mod depth_tests {
    use super::depth;

    #[test]
    fn counts_nesting_ignoring_markup_that_is_not_elements() {
        assert_eq!(depth("<a/>"), 1);
        assert_eq!(depth("<a><b/><b><c x='>'/></b></a>"), 3);
        assert_eq!(
            depth("<a><!-- <b><b><b> --><![CDATA[<c><c>]]><?pi <d> ?></a>"),
            1
        );
        assert_eq!(depth("<a x=\"<\"><b></b><b></b></a>"), 2);
        assert_eq!(depth(""), 0);
        assert_eq!(depth("<a><b"), 2);
    }

    #[test]
    fn doctypes_are_skipped_but_markup_entities_are_too_deep() {
        let nest = format!("<svg>{}{}</svg>", "<g>".repeat(50), "</g>".repeat(50));
        let dt = |dtd: &str| depth(&format!("<?xml version=\"1.0\"?>\n{dtd}\n{nest}"));
        assert_eq!(dt("<!DOCTYPE svg>"), 51);
        assert_eq!(
            dt(
                r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">"#
            ),
            51
        );
        // As Adobe Illustrator writes them.
        let illustrator = r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "x.dtd" [
            <!ENTITY ns_extend "http://ns.adobe.com/Extensibility/1.0/">
            <!-- a ] comment > with "quotes -->
            <!ENTITY ns_svg "http://www.w3.org/2000/svg">
        ]>"#;
        assert_eq!(dt(illustrator), 51);
        for dtd in [
            r#"<!DOCTYPE svg [<!ENTITY e "<g><g></g></g>">]>"#,
            r#"<!DOCTYPE svg [<!ENTITY e '&#60;g>'>]>"#,
            r#"<!DOCTYPE svg [<!ENTITY e "&f;">]>"#,
            r#"<!DOCTYPE svg [<!ENTITY % p "x"> %p;]>"#,
            r#"<!DOCTYPE svg [<!ENTITY e "x"#,
        ] {
            assert_eq!(dt(dtd), usize::MAX, "{dtd}");
        }
    }
}
