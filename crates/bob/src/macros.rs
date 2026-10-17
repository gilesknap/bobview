use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::Element;

/// An ordered set of macro definitions. Later definitions win.
///
/// A set layered over another (see [`Macros::with`]) shares it rather than
/// copying it, so each widget of a large display costs only its own macros.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Macros(Arc<Layer>);

#[derive(Debug, Default, PartialEq)]
struct Layer {
    parent: Option<Macros>,
    /// Consulted after `parent` and everything below it.
    base: Option<Macros>,
    own: Vec<(String, String)>,
    /// Position in `own` of the last definition of each name.
    index: HashMap<String, usize>,
    /// Layers below this one.
    depth: u32,
}

/// The most expanded text that [`Macros::with`] holds in one layer.
const MAX_LAYER_BYTES: usize = 1 << 20;

/// The most expanded macro text, counting what was expanded as well as what
/// it came to, that a whole load should do, for [`Macros::with_budget`] to be
/// given. Each expansion takes the size of its text and result, so this also
/// bounds the lookups, and so the time, of a load.
pub const MAX_LOAD_BYTES: usize = 4 << 20;

/// The most definitions in a layer, and the most layers; more are ignored.
const MAX_LAYER_MACROS: usize = 512;
const MAX_LAYERS: u32 = 32;

impl Macros {
    pub fn from_element(el: &Element) -> Self {
        Self::from_pairs(el.children.iter().map(|c| (c.name.clone(), c.text.clone())))
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let own: Vec<(String, String)> = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Self::layer(None, None, own)
    }

    fn layer(parent: Option<Macros>, base: Option<Macros>, own: Vec<(String, String)>) -> Self {
        let index = own
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
        let depth = parent.as_ref().map_or(0, |p| p.0.depth + 1);
        Self(Arc::new(Layer {
            parent,
            base,
            own,
            index,
            depth,
        }))
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        let own = self.0.index.get(name).map(|&i| self.0.own[i].1.as_str());
        own.or_else(|| self.0.parent.as_ref()?.get(name))
            .or_else(|| self.0.base.as_ref()?.get(name))
    }

    pub fn is_empty(&self) -> bool {
        self.0.own.is_empty()
            && self.0.parent.as_ref().is_none_or(|p| p.is_empty())
            && self.0.base.as_ref().is_none_or(|b| b.is_empty())
    }

    /// Every definition, in the order [`Macros::get`] lets later ones win:
    /// a layer's `base` first, then its `parent` (and all below it), then
    /// its own.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        let mut out = Vec::new();
        self.collect_into(&mut out);
        out.into_iter()
    }

    fn collect_into<'a>(&'a self, out: &mut Vec<(&'a str, &'a str)>) {
        if let Some(b) = &self.0.base {
            b.collect_into(out);
        }
        if let Some(p) = &self.0.parent {
            p.collect_into(out);
        }
        out.extend(self.0.own.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    /// These macros with `inner` layered on top. Values in `inner` are first
    /// expanded against `self`, so `<P>$(P):SUB</P>` works. Only `inner`'s
    /// own definitions are used, not the layers under it: for a whole chain,
    /// such as a link's, use [`Macros::over`].
    pub fn with(&self, inner: &Macros) -> Macros {
        self.with_budget(inner, &AtomicUsize::new(MAX_LAYER_BYTES))
    }

    /// [`Macros::with`], where expanding the values takes the size of their
    /// text and result from `left`, which several layers may share. Once it
    /// is spent, values are kept as written, not expanded, so nested
    /// definitions that each double the last, in many widgets, can't use
    /// memory or time without bound. Only the definitions made in `inner`
    /// itself are used, at most [`MAX_LAYER_MACROS`] of them, and no more
    /// than [`MAX_LAYERS`] deep.
    pub fn with_budget(&self, inner: &Macros, left: &AtomicUsize) -> Macros {
        if inner.0.own.is_empty() {
            return self.clone();
        }
        if self.0.depth >= MAX_LAYERS {
            log::warn!("macros nested over {MAX_LAYERS} deep are ignored");
            return self.clone();
        }
        if inner.0.own.len() > MAX_LAYER_MACROS {
            log::warn!("over {MAX_LAYER_MACROS} macros in a widget; the rest are ignored");
        }
        let own = inner
            .0
            .own
            .iter()
            .take(MAX_LAYER_MACROS)
            .map(|(k, v)| {
                let v = if left.load(Ordering::Relaxed) > 0 {
                    let out = expand(v, &|name| self.get(name).map(str::to_string));
                    spend(left, v.len() + out.len());
                    out
                } else {
                    warn_spent();
                    v.clone()
                };
                (k.clone(), v)
            })
            .collect();
        Self::layer(Some(self.clone()), None, own)
    }

    /// `defaults` with `over` on top of them, sharing both: `over` is the
    /// macros an embedding passes to the display it embeds, and is a whole
    /// chain of the embedder's layers, which are not copied.
    pub fn over(defaults: &Macros, over: &Macros, left: &AtomicUsize) -> Macros {
        if over.0.parent.is_none() && over.0.base.is_none() {
            return defaults.with_budget(over, left);
        }
        // A value naming its own key, as `P=$(P):SUB` does, means the next
        // layer down, which here is the display's default.
        let own = over
            .iter()
            .filter_map(|(k, v)| {
                let default = defaults.get(k)?;
                let fixed = v
                    .replace(&format!("$({k})"), default)
                    .replace(&format!("${{{k}}}"), default);
                (fixed != v && over.get(k) == Some(v)).then(|| (k.to_string(), fixed))
            })
            .collect();
        Self::layer(Some(over.clone()), Some(defaults.clone()), own)
    }

    /// The bytes of the definitions in this layer alone.
    pub fn own_bytes(&self) -> usize {
        self.0.own.iter().map(|(k, v)| k.len() + v.len()).sum()
    }
}

/// Take `n` from `left`, down to nothing.
pub fn spend(left: &AtomicUsize, n: usize) {
    let mut l = left.load(Ordering::Relaxed);
    while let Err(now) =
        left.compare_exchange(l, l.saturating_sub(n), Ordering::Relaxed, Ordering::Relaxed)
    {
        l = now;
    }
}

/// Warn, once until [`rearm_warning`], that an expansion was left as written.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Let the next expansion that gives up warn again, as at the start of a load.
pub fn rearm_warning() {
    WARNED.store(false, Ordering::Relaxed);
    SPENT_WARNED.store(false, Ordering::Relaxed);
}

static SPENT_WARNED: AtomicBool = AtomicBool::new(false);

/// Warn, once per load, that the macro budget is spent and text is left as written.
pub fn warn_spent() {
    if !SPENT_WARNED.swap(true, Ordering::Relaxed) {
        log::warn!("macro expansion budget spent; the rest of the load is left as written");
    }
}

/// Replace `$(NAME)` and `${NAME}` using `lookup`. `$(NAME=default)` falls back
/// to the default; unknown names are left as written, like Phoebus does.
/// Text whose expansion would take more than [`MAX_WORK`] (16 KiB) of output,
/// such as nested `$(M)` that each name `$(M)` several times, is returned
/// unexpanded, as written, rather than truncated.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut work = 0;
    expand_depth(text, lookup, 0, &mut work).unwrap_or_else(|| {
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!("macro expansion over {MAX_WORK} bytes left as written: {text:.80}");
        }
        text.to_string()
    })
}

/// The most output, counted over every nested expansion, that [`expand`] builds.
/// Kept at 16 KiB rather than more because an expansion that fails costs all
/// of it each time, and widgets expand text as they draw.
const MAX_WORK: usize = 16 << 10;

/// [`expand`] one level down, adding the size of what it builds to `work`;
/// `None` once that passes [`MAX_WORK`].
fn expand_depth(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    depth: u32,
    work: &mut usize,
) -> Option<String> {
    *work += text.len();
    if *work > MAX_WORK {
        return None;
    }
    if depth > 10 || !text.contains('$') {
        return Some(text.to_string());
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
            return Some(out);
        };
        let body = expand_depth(&after[1..end], lookup, depth + 1, work)?;
        let (name, default) = match body.split_once('=') {
            Some((n, d)) => (n, Some(d)),
            None => (body.as_str(), None),
        };
        match lookup(name).or(default.map(str::to_string)) {
            Some(value) => out.push_str(&expand_depth(&value, lookup, depth + 1, work)?),
            None => out.push_str(&rest[pos..pos + end + 2]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
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
    fn nested_expansion_is_bounded() {
        // Each level names the next six times: 6^10 bytes if expanded in full.
        let mut pairs = vec![("M0".to_string(), "x".to_string())];
        for i in 1..=10 {
            pairs.push((format!("M{i}"), format!("$(M{})", i - 1).repeat(6)));
        }
        let m = Macros::from_pairs(pairs);
        let start = std::time::Instant::now();
        let text = "$(M10)";
        let out = expand(text, &|n| m.get(n).map(str::to_string));
        assert_eq!(out, text);
        assert!(start.elapsed().as_secs() < 2);
        // Ordinary long values still expand.
        let m = Macros::from_pairs([("L", "y".repeat(10_000))]);
        assert_eq!(
            expand("$(L)", &|n| m.get(n).map(str::to_string)).len(),
            10_000
        );
    }

    #[test]
    fn layers_share_a_budget() {
        let root = Macros::from_pairs([("V", "x".repeat(1000))]);
        let left = AtomicUsize::new(5000);
        let inner = Macros::from_pairs((0..100).map(|i| (format!("N{i}"), "$(V)")));
        let set = root.with_budget(&inner, &left);
        // Expanded until spent, then kept as written.
        assert_eq!(set.get("N0").map(str::len), Some(1000));
        assert_eq!(set.get("N99"), Some("$(V)"));
        assert!(set.own_bytes() < 10_000);
        let more = set.with_budget(&inner, &left);
        assert_eq!(more.get("N0"), Some("$(V)"));
    }

    #[test]
    fn iter_lists_every_layer_as_get_sees_them() {
        // An embedded display's defaults (the base) under its caller's
        // chain: iterating, later definitions win as `get` has them.
        let defaults = Macros::from_pairs([("D", "dflt"), ("P", "D")]);
        let caller = Macros::from_pairs([("P", "CALL")]).with(&Macros::from_pairs([("id", "01")]));
        let m = Macros::over(&defaults, &caller, &AtomicUsize::new(1 << 20));
        let last: std::collections::BTreeMap<_, _> = m.iter().collect();
        for (k, v) in &last {
            assert_eq!(m.get(k), Some(*v), "{k}");
        }
        assert_eq!(
            last.into_iter().collect::<Vec<_>>(),
            [("D", "dflt"), ("P", "CALL"), ("id", "01")]
        );
    }

    #[test]
    fn caller_self_references_use_the_displays_defaults() {
        let ex = |m: &Macros, t: &str| expand(t, &|n| m.get(n).map(str::to_string));
        let defaults = Macros::from_pairs([("P", "D")]);
        let own = Macros::from_pairs([("P", "$(P):SUB")]);
        let none = Macros::from_pairs([("X", "1")]);
        let caller = none.with(&own);
        let m = Macros::over(&defaults, &caller, &AtomicUsize::new(1 << 20));
        assert_eq!(ex(&m, "$(P)"), "D:SUB");
        let caller = Macros::from_pairs([("P", "CALL")]).with(&own);
        let m = Macros::over(&defaults, &caller, &AtomicUsize::new(1 << 20));
        assert_eq!(ex(&m, "$(P)"), "CALL:SUB");
    }

    #[test]
    fn expands_recursively_and_layers() {
        assert_eq!(ex("$(N)"), "BL01T");
        let inner = Macros::from_pairs([("P", "$(P):SUB")]);
        assert_eq!(m().with(&inner).get("P"), Some("BL01T:SUB"));
    }
}
