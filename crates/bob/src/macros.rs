use crate::Element;

/// An ordered set of macro definitions. Later definitions win.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Macros(Vec<(String, String)>);

impl Macros {
    pub fn from_element(el: &Element) -> Self {
        Self(
            el.children
                .iter()
                .map(|c| (c.name.clone(), c.text.clone()))
                .collect(),
        )
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// These macros with `inner` layered on top. Values in `inner` are first
    /// expanded against `self`, so `<P>$(P):SUB</P>` works.
    pub fn with(&self, inner: &Macros) -> Macros {
        let mut out = self.clone();
        for (k, v) in &inner.0 {
            let v = expand(v, &|name| self.get(name).map(str::to_string));
            out.0.push((k.clone(), v));
        }
        out
    }
}

/// Replace `$(NAME)` and `${NAME}` using `lookup`. `$(NAME=default)` falls back
/// to the default; unknown names are left as written, like Phoebus does.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    expand_depth(text, lookup, 0)
}

fn expand_depth(text: &str, lookup: &dyn Fn(&str) -> Option<String>, depth: u32) -> String {
    if depth > 10 || !text.contains('$') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let close = match after.chars().next() {
            Some('(') => ')',
            Some('{') => '}',
            _ => {
                out.push('$');
                rest = after;
                continue;
            }
        };
        let Some(end) = matching(after, close) else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let body = expand_depth(&after[1..end], lookup, depth + 1);
        let (name, default) = match body.split_once('=') {
            Some((n, d)) => (n, Some(d)),
            None => (body.as_str(), None),
        };
        match lookup(name).or(default.map(str::to_string)) {
            Some(value) => out.push_str(&expand_depth(&value, lookup, depth + 1)),
            None => out.push_str(&rest[pos..pos + end + 2]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Index of the bracket closing the one at the start of `s`, allowing nesting.
fn matching(s: &str, close: char) -> Option<usize> {
    let open = s.chars().next()?;
    let mut level = 0;
    for (i, c) in s.char_indices() {
        if c == open {
            level += 1;
        } else if c == close {
            level -= 1;
            if level == 0 {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Macros {
        Macros::from_pairs([("P", "BL01T"), ("R", ":DET:"), ("N", "$(P)")])
    }

    fn ex(s: &str) -> String {
        let m = m();
        expand(s, &|n| m.get(n).map(str::to_string))
    }

    #[test]
    fn expands_both_bracket_styles() {
        assert_eq!(ex("$(P)$(R)Acquire"), "BL01T:DET:Acquire");
        assert_eq!(ex("${P}:X"), "BL01T:X");
    }

    #[test]
    fn keeps_unknown_and_uses_defaults() {
        assert_eq!(ex("$(Q):X"), "$(Q):X");
        assert_eq!(ex("$(Q=dflt)"), "dflt");
        assert_eq!(ex("cost $5"), "cost $5");
        assert_eq!(ex("$(P"), "$(P");
    }

    #[test]
    fn expands_recursively_and_layers() {
        assert_eq!(ex("$(N)"), "BL01T");
        let inner = Macros::from_pairs([("P", "$(P):SUB")]);
        assert_eq!(m().with(&inner).get("P"), Some("BL01T:SUB"));
    }
}
