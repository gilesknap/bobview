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
pub(crate) const MAX_DEPTH: usize = 256;

/// The deepest element nesting in `xml`, found by a quick scan without
/// recursion, so it can run before the (recursive) XML parser.
///
/// It only has to be right for documents the parser accepts: comments, CDATA
/// and processing instructions are skipped, quotes in tags are honoured, and
/// anything else odd (a DTD, which the parser rejects) ends the scan.
pub(crate) fn depth(xml: &str) -> usize {
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
}
