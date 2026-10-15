/// An owned XML element: enough of the DOM to read `.bob` properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// Text content, trimmed, when the element has no child elements.
    pub text: String,
    pub children: Vec<Element>,
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
