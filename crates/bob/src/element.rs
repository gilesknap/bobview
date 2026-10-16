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
/// It only has to be right for documents the parser (roxmltree) accepts, but
/// it fails closed: markup it does not understand counts as infinitely deep.
/// Comments, CDATA and processing instructions are skipped and quotes in tags
/// are honoured. A `<!DOCTYPE>` (which usvg accepts) is skipped the way
/// roxmltree reads it, but entities can expand to markup the scan never sees,
/// so a DTD declaring any entity value with markup or references in it, or
/// one roxmltree may read differently, counts as infinitely deep. Plain
/// entities, such as the namespace URLs Adobe Illustrator declares, are fine.
pub fn depth(xml: &str) -> usize {
    let b = xml.as_bytes();
    let (mut i, mut open, mut deepest) = (0, 0usize, 0usize);
    while let Some(p) = b[i..].iter().position(|&c| c == b'<') {
        i += p;
        let rest = &b[i..];
        let next = if rest.starts_with(b"<!--") {
            skip_past(b, i + 4, b"-->")
        } else if rest.starts_with(b"<![CDATA[") {
            skip_past(b, i + 9, b"]]>")
        } else if rest.starts_with(b"<?") {
            skip_past(b, i + 2, b"?>")
        } else if rest.starts_with(b"<!DOCTYPE") {
            doctype_end(b, i + 9)
        } else if rest.starts_with(b"<!") {
            None
        } else if rest.starts_with(b"</") {
            open = open.saturating_sub(1);
            skip_past(b, i + 2, b">")
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
            if b[j - 1] != b'/' {
                open += 1;
            }
            Some(j + 1)
        };
        match next {
            Some(next) if next < b.len() => i = next,
            Some(_) => break,
            None => return usize::MAX,
        }
    }
    deepest
}

/// Where the first `end` at or after `i` ends, or `None` if there is none.
fn skip_past(b: &[u8], i: usize, end: &[u8]) -> Option<usize> {
    let p = b.get(i..)?.windows(end.len()).position(|w| w == end)?;
    Some(i + p + end.len())
}

/// Where the `<!DOCTYPE` whose name starts at `i` ends, read as roxmltree
/// reads it (`tokenizer.rs`, `parse_doctype`). `None` if it declares
/// something that could expand to markup (an entity value holding `<`, `&`
/// or `%`, a parameter or external entity), or holds anything else roxmltree
/// would not skip in the same way.
fn doctype_end(b: &[u8], mut i: usize) -> Option<usize> {
    let spaces = |mut i: usize| {
        while b
            .get(i)
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            i += 1;
        }
        i
    };
    let name = |mut i: usize| {
        while b.get(i).is_some_and(|c| {
            !matches!(
                c,
                b' ' | b'\t' | b'\n' | b'\r' | b'[' | b'>' | b'"' | b'\'' | b'%'
            )
        }) {
            i += 1;
        }
        i
    };
    // A quoted literal starting at `i`: where it ends, and its value.
    let quoted = |i: usize| -> Option<(usize, &[u8])> {
        let q = *b.get(i).filter(|q| matches!(q, b'"' | b'\''))?;
        let len = b[i + 1..].iter().position(|&c| c == q)?;
        Some((i + len + 2, &b[i + 1..i + 1 + len]))
    };
    i = name(spaces(i));
    i = spaces(i);
    if b[i..].starts_with(b"SYSTEM") {
        i = quoted(spaces(i + 6))?.0;
    } else if b[i..].starts_with(b"PUBLIC") {
        i = quoted(spaces(i + 6))?.0;
        i = quoted(spaces(i))?.0;
    }
    i = spaces(i);
    match b.get(i)? {
        b'>' => return Some(i + 1),
        b'[' => i += 1,
        _ => return None,
    }
    // The internal subset.
    loop {
        i = spaces(i);
        let rest = b.get(i..)?;
        i = if rest.starts_with(b"<!ENTITY") {
            // Only `<!ENTITY name "value">`, with a value free of markup and
            // references: no parameter or external entities.
            let (end, value) = quoted(spaces(name(spaces(i + 8))))?;
            if value.iter().any(|c| matches!(c, b'<' | b'&' | b'%')) {
                return None;
            }
            let end = spaces(end);
            (b.get(end) == Some(&b'>')).then_some(end + 1)?
        } else if rest.starts_with(b"<!--") {
            skip_past(b, i + 4, b"-->")?
        } else if rest.starts_with(b"<?") {
            skip_past(b, i + 2, b"?>")?
        } else if rest.starts_with(b"]") {
            let end = spaces(i + 1);
            return (b.get(end) == Some(&b'>')).then_some(end + 1);
        } else if rest.starts_with(b"<!ELEMENT")
            || rest.starts_with(b"<!ATTLIST")
            || rest.starts_with(b"<!NOTATION")
        {
            // roxmltree skips these to the next `>`, quoted or not.
            skip_past(b, i, b">")?
        } else {
            return None;
        };
    }
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
            r#"<!DOCTYPE svg [<!ENTITY e SYSTEM "x.xml">]>"#,
            // roxmltree skips declarations to the next `>`, so this `]`
            // does not end the subset and the entity is still declared.
            r#"<!DOCTYPE svg [<!ELEMENT x ]><!ENTITY e "<g>">]>"#,
            r#"<!DOCTYPE svg [<!ATTLIST x y CDATA "]>"><!ENTITY e "<g>">]>"#,
            // Anything the scan does not know.
            r#"<!DOCTYPE svg [<!FOO>]>"#,
            r#"<!DOCTYPE svg [] x>"#,
        ] {
            assert_eq!(dt(dtd), usize::MAX, "{dtd}");
        }
        // Declarations roxmltree skips are skipped.
        assert_eq!(
            dt(
                r#"<!DOCTYPE svg [<!ELEMENT x ]><!ATTLIST a b CDATA "x"><!NOTATION n SYSTEM "n">]>"#
            ),
            51
        );
        // Markup the parser would reject is too deep, not the end of the scan.
        assert_eq!(depth("<a><!FOO><b><c/></b></a>"), usize::MAX);
    }

    /// The deepest element nesting roxmltree finds in `xml`, if it parses.
    fn parsed_depth(xml: &str) -> Option<usize> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, options).ok()?;
        doc.descendants()
            .filter(|n| n.is_element())
            .map(|n| n.ancestors().filter(|a| a.is_element()).count())
            .max()
    }

    /// Random documents made of the pieces that matter to the scan: it never
    /// finds a document roxmltree parses shallower than roxmltree does.
    #[test]
    fn never_shallower_than_roxmltree() {
        const DTD: &[&str] = &[
            " ",
            "<!ELEMENT x ]>",
            "<!ELEMENT x (a|b)*>",
            r#"<!ATTLIST a b CDATA "]>">"#,
            r#"<!ATTLIST a b CDATA ']'>"#,
            "<!NOTATION n SYSTEM 'n'>",
            r#"<!ENTITY p "v">"#,
            r#"<!ENTITY m "<a><a></a></a>">"#,
            r#"<!ENTITY r "&p;">"#,
            r#"<!ENTITY q ']>'>"#,
            "<!-- ] > -->",
            "<?p ]> ?>",
            "]",
            ">",
            "]>",
            "\"",
            "'",
            "<!ENTITY",
            "<!",
            "<a>",
            "</a>",
            "&m;",
        ];
        const BODY: &[&str] = &[
            "<a>",
            "</a>",
            "<a/>",
            "<b x='>'/>",
            "<b x=\"/\">",
            "</b>",
            "<!--<a>-->",
            "<![CDATA[<a>]]>",
            "<?p <a> ?>",
            "t",
            "&p;",
            "&m;",
            "&q;",
            "&r;",
            "<!",
            ">",
            "]]>",
            "&lt;a>",
            "\"",
            "<!DOCTYPE a>",
            "<b x=\"/\"></b>",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rand = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let (mut parsed, mut deep) = (0, 0);
        for _ in 0..200_000 {
            let mut xml = String::new();
            if rand(4) > 0 {
                xml += "<!DOCTYPE r [";
                for _ in 0..rand(5) {
                    xml += DTD[rand(DTD.len())];
                }
                xml += "]>";
            }
            // Mostly balanced, so that many parse.
            xml += "<r>";
            let mut open = 0;
            for _ in 0..rand(12) {
                let token = BODY[rand(BODY.len())];
                match token {
                    "<a>" => open += 1,
                    "</a>" if open == 0 => continue,
                    "</a>" => open -= 1,
                    _ => {}
                }
                xml += token;
            }
            xml += &"</a>".repeat(open);
            xml += "</r>";
            if let Some(real) = parsed_depth(&xml) {
                parsed += 1;
                deep += usize::from(real > 2);
                assert!(depth(&xml) >= real, "{xml}: scan {} < {real}", depth(&xml));
            }
        }
        // The generator does reach documents worth checking.
        assert!(parsed > 1000 && deep > 100, "{parsed} parsed, {deep} deep");
    }
}
