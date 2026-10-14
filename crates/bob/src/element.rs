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
