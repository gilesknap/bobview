//! Parsing SVGs, and the images they embed, without trusting them.
//!
//! usvg parses, and resvg renders, by recursing once per level of nesting,
//! and once per reference followed (a pattern, marker, mask, clip path or
//! filter image drawn from another one, a `<use>`, ...). A stack overflow
//! aborts the native viewer. In the browser it throws out of the wasm with
//! nothing unwound, which leaves the image loading for ever and, after a
//! few, crashes the tab: it is not survivable either, so these limits must
//! keep every input well inside the stack rather than rely on a trap.
//!
//! So nesting is bounded twice: the XML, with references followed, before
//! usvg parses it, and the tree usvg builds (which `<use>` and embedded SVGs
//! can make deeper than the XML) before it is rendered. usvg also converts
//! a marker, pattern, mask, clip path or `<use>` again wherever it is used,
//! so how much references make it convert is bounded too. Embedded raster
//! images are decoded at render time, so their declared sizes are checked
//! first.
//!
//! This module only depends on usvg, roxmltree, simplecss, `bob` and
//! `imagesize`, so a test build for the browser can include it on its own.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use resvg::usvg;

/// Deepest element nesting of SVG XML handed to usvg, counting each
/// reference followed as `REF_LEVELS` levels and the SVGs embedded in it
/// together. A plain chain of references may be about 20 long; usvg needed
/// about 700 to exhaust Chromium's stack, and real SVGs follow two or three
/// (and score at most 44).
pub const MAX_XML_DEPTH: usize = 128;

/// Most elements usvg may convert for an SVG, embedded SVGs included,
/// counting each as often as references make it convert them (see
/// [`Cost`]). usvg caches little, so references to references multiply:
/// a few kilobytes can ask for billions. Real SVGs need at most 2000.
pub const MAX_WORK: usize = 100_000;

/// Levels of nesting that following one reference counts as. usvg recurses
/// through several frames for each, more than for a nested group.
pub const REF_LEVELS: usize = 4;

/// Deepest nesting of groups in a parsed SVG (counting clip paths, masks,
/// patterns, filters and embedded SVGs) that we render. Rendering takes
/// about 28 KiB of stack a level in the browser, so this many fit even in
/// wasm's default 1 MiB stack. The build gives wasm 8 MiB (see `build.rs`),
/// but Chromium's own stack limit is often reached first, so that is no
/// reason to raise this. Real symbols nest groups a few deep.
pub const MAX_TREE_DEPTH: usize = 24;

/// Most pixels, and longest side, of a raster image we decode
/// (32 Mpx is 128 MB as RGBA).
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
pub const MAX_SIDE: u32 = 16384;

pub fn size_ok(width: u32, height: u32) -> bool {
    width <= MAX_SIDE && height <= MAX_SIDE && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

/// The SVG in `data`, parsed with images allowed only as data: URLs.
pub fn parse(data: &[u8]) -> Result<usvg::Tree, String> {
    let text = std::str::from_utf8(data).map_err(|_| "SVG is not UTF-8 text")?;
    let doc = parse_xml(text)?;
    let cost = svg_cost(&doc);
    if cost.depth > MAX_XML_DEPTH {
        return Err("SVG nests elements or references too deeply".into());
    }
    if cost.work > MAX_WORK {
        return Err("SVG is too large or complex".into());
    }
    // How deep the XML being parsed is so far, and how much has been
    // converted, embedded SVGs included.
    let used = Arc::new(AtomicUsize::new(cost.depth));
    let worked = Arc::new(AtomicUsize::new(cost.work));
    // An SVG may embed images as data: URLs, but never names files or URLs:
    // a display from the web is not trusted with local files.
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: {
                let default = usvg::ImageHrefResolver::default_data_resolver();
                Box::new(move |mime, data, opts| {
                    if !is_svg(mime, &data) {
                        if !raster_ok(mime, &data) {
                            log::warn!("embedded {mime} image is not shown: too large");
                            return None;
                        }
                        return default(mime, data, opts);
                    }
                    // A nested SVG is parsed now, deeper on the same stack,
                    // as often as the image is converted.
                    let cost = std::str::from_utf8(&data)
                        .ok()
                        .and_then(|text| parse_xml(text).ok())
                        .map_or(Cost::LOOP, |doc| svg_cost(&doc));
                    let outer = used.load(Ordering::Relaxed);
                    if outer.saturating_add(cost.depth) > MAX_XML_DEPTH {
                        log::warn!("embedded SVG is not shown: nested too deeply");
                        return None;
                    }
                    let work = worked.load(Ordering::Relaxed).saturating_add(cost.work);
                    if work > MAX_WORK {
                        log::warn!("embedded SVG is not shown: too large or complex");
                        return None;
                    }
                    worked.store(work, Ordering::Relaxed);
                    used.store(outer + cost.depth, Ordering::Relaxed);
                    let kind = default(mime, data, opts);
                    used.store(outer, Ordering::Relaxed);
                    kind
                })
            },
            resolve_string: Box::new(|href, _| {
                log::warn!("SVG image '{href}' is not shown: only embedded images are");
                None
            }),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_xmltree(&doc, &options).map_err(|e| e.to_string())?;
    if tree_depth(tree.root()) > MAX_TREE_DEPTH {
        return Err("SVG nests groups too deeply".into());
    }
    Ok(tree)
}

/// `text` parsed as usvg parses it, once its nesting is known to be small
/// enough for the parser.
fn parse_xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    if bob::xml_depth(text) > MAX_XML_DEPTH {
        return Err("SVG nests elements too deeply".into());
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(text, options)
        .map_err(|e| usvg::Error::ParsingFailed(e).to_string())
}

/// Elements whose `href` usvg follows to another element in the document.
const HREF_ELEMENTS: &[&str] = &[
    "use",
    "image",
    "feImage",
    "textPath",
    "tref",
    "linearGradient",
    "radialGradient",
    "pattern",
    "filter",
];

/// Elements that a reference uses, but whose content is never drawn, so
/// inherits nothing: a gradient names its stops and a filter its
/// primitives (and a filter image what it draws by its own reference).
const PAINT_SERVERS: &[&str] = &["linearGradient", "radialGradient", "filter"];

/// More vertices than usvg gives a rect, circle or ellipse. It converts
/// these and lines, polylines, polygons and paths to paths, and puts
/// markers on all of them (`convert_element_impl` in its `converter.rs`).
const SHAPE_VERTICES: usize = 64;

/// Properties holding references that are not inherited, but that an
/// element may take from its parent by `inherit`.
const NOT_INHERITED: [&str; 3] = ["clip-path", "mask", "filter"];

/// What it costs usvg to parse an SVG.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cost {
    /// How deep it recurses.
    pub depth: usize,
    /// About how many elements it converts.
    pub work: usize,
}

impl Cost {
    /// References that loop, which usvg may follow for ever.
    const LOOP: Cost = Cost {
        depth: usize::MAX,
        work: usize::MAX,
    };
}

/// The cost of `doc`. Its depth is the deepest element nesting with the
/// references usvg follows (`url(#id)` in an attribute, a `style` or a style
/// sheet, and an `href` to `#id`) counted as `REF_LEVELS` levels each: how
/// deep usvg recurses to parse it. Its work counts each element once for
/// each time usvg may convert it: usvg converts a marker at every vertex
/// it marks, and a pattern, mask, clip path or filter for each element
/// using it, so references from references multiply. Found without
/// recursion, in time linear in the size of `doc` and its style sheets.
///
/// An element's references count from the deepest element below it, as
/// that may be where an inherited `fill` or marker is resolved, or where
/// the content of a `<use>` (which inherits from the `<use>`) resolves
/// others. Following one into an element also leads to the references it
/// inherits from its own ancestors, as usvg converts patterns, markers,
/// masks, clip paths and the elements a filter image names where they are.
pub fn svg_cost(doc: &roxmltree::Document) -> Cost {
    let count = doc.descendants().count();
    let index = |id: roxmltree::NodeId| id.get_usize();
    let elements = || doc.descendants().filter(|n| n.is_element());
    // Each id, and the elements that have it.
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let mut having: Vec<Vec<usize>> = Vec::new();
    for node in elements() {
        if let Some(id) = node.attribute("id") {
            let i = *ids.entry(id).or_insert_with(|| {
                having.push(Vec::new());
                having.len() - 1
            });
            having[i].push(index(node.id()));
        }
    }
    // Style sheets, as usvg reads them, keeping only the rules that refer to
    // something or inherit.
    let mut sheet = simplecss::StyleSheet::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("style")) {
        if matches!(node.attribute("type"), None | Some("text/css"))
            && let Some(text) = node.text()
        {
            sheet.parse_more(text);
        }
    }
    let inherit = |value: &str| value.trim() == "inherit";
    let relevant = |value: &str| value.contains("url(") || inherit(value);
    sheet
        .rules
        .retain(|rule| rule.declarations.iter().any(|d| relevant(d.value)));

    // The ids each element names: by `href`; in what its descendants
    // inherit, converted once for each (paint, start and end markers) or at
    // each vertex (middle markers); in its other properties; and in each of
    // `NOT_INHERITED`, with which of those it takes from its parent. And
    // how many vertices it may put middle markers on.
    #[derive(Clone, Default)]
    struct Refs {
        href: Vec<usize>,
        paint: Vec<usize>,
        markers: Vec<usize>,
        other: Vec<usize>,
        not_inherited: [Vec<usize>; 3],
        inherits: [bool; 3],
        vertices: usize,
    }
    let mut refs = vec![Refs::default(); count];
    for node in elements() {
        let r = &mut refs[index(node.id())];
        let mut props: Vec<(&str, &str)> = Vec::new();
        for attr in node.attributes() {
            if attr.name() == "style" {
                for d in simplecss::DeclarationTokenizer::from(attr.value()) {
                    props.push((d.name, d.value));
                }
            } else {
                props.push((attr.name(), attr.value()));
            }
        }
        for rule in &sheet.rules {
            if rule.selector.matches(&CssNode(node)) {
                props.extend(rule.declarations.iter().map(|d| (d.name, d.value)));
            }
        }
        let named = |value: &str, to: &mut Vec<usize>| {
            let mut names = Vec::new();
            url_ids(value, &mut names);
            to.extend(names.iter().filter_map(|n| ids.get(n)));
        };
        // The ids each property names. A property set several times (by
        // attribute, `style` and style sheet) has one value in the end, but
        // usvg converts what different properties name once for each.
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (name, value) in props.into_iter().filter(|(_, v)| relevant(v)) {
            let k = NOT_INHERITED.iter().position(|n| *n == name);
            if inherit(value) {
                if let Some(k) = k {
                    r.inherits[k] = true;
                }
                continue;
            }
            if let Some(k) = k {
                named(value, &mut r.not_inherited[k]);
            }
            named(value, by_name.entry(name).or_default());
        }
        let mut unknown = Vec::new();
        for (name, mut list) in by_name {
            list.sort_unstable();
            list.dedup();
            // usvg draws a start and an end marker once a path, like a fill,
            // and a middle one at each vertex between.
            match name {
                "fill" | "stroke" | "marker-start" | "marker-end" => r.paint.extend(list),
                "marker-mid" => r.markers.extend(list),
                "marker" => {
                    r.paint.extend(&list);
                    r.paint.extend(&list);
                    r.markers.extend(list);
                }
                _ if NOT_INHERITED.contains(&name) => r.other.extend(list),
                // usvg follows no other property, so these lead nowhere new.
                _ => unknown.extend(list),
            }
        }
        unknown.sort_unstable();
        unknown.dedup();
        r.other.extend(unknown);
        if HREF_ELEMENTS.contains(&node.tag_name().name()) {
            for a in node.attributes().filter(|a| a.name() == "href") {
                if let Some((_, id)) = a.value().split_once('#') {
                    r.href.extend(ids.get(id.trim()));
                }
            }
        }
        // usvg follows one `href` (or `xlink:href`).
        r.href.sort_unstable();
        r.href.dedup();
        // A generous bound on the vertices middle markers go on: each takes
        // at least two characters to write (usvg puts none on a close), an
        // arc more than twice the segments usvg splits it into, and a rect
        // or an ellipse has fewer than `SHAPE_VERTICES`.
        let text = |name| node.attribute(name).map_or(0, str::len);
        r.vertices = match node.tag_name().name() {
            "rect" | "circle" | "ellipse" => SHAPE_VERTICES,
            "line" | "polyline" | "polygon" | "path" => 2 + (text("d") + text("points")) / 2,
            _ => 0,
        };
    }
    // The element whose `NOT_INHERITED` property each element has, taking
    // its parent's by `inherit` (parents come first).
    let mut owner: Vec<[Option<usize>; 3]> = vec![[None; 3]; count];
    for node in elements() {
        let (e, parent) = (index(node.id()), node.parent().map(|p| index(p.id())));
        owner[e] = std::array::from_fn(|k| {
            if refs[e].inherits[k] {
                parent.and_then(|p| owner[p][k])
            } else if refs[e].not_inherited[k].is_empty() {
                None
            } else {
                Some(e)
            }
        });
    }

    // The walk's nodes: each element; each element as followed to by a
    // reference (`count +`); the paint (`2 * count +`) and the markers
    // (`3 * count +`) each node inherits from itself and its ancestors; and
    // each id (`4 * count +`), which leads to every element with that id.
    let (followed, paint, markers, id) = (count, 2 * count, 3 * count, 4 * count);
    #[derive(Clone, Copy)]
    struct Step {
        to: usize,
        /// How much deeper.
        levels: usize,
        /// Deeper from the deepest step that is not.
        across: bool,
        /// How many times it is converted.
        times: Times,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Times {
        /// A child element, once.
        Child,
        /// Once, as the element's content: what is cloned by a `<use>`
        /// (and by any other `href`, as a pattern uses another's content),
        /// or what a reference or an id leads to.
        Content,
        Once,
        /// Once for each element below the one it is followed from.
        Each,
        /// Once for each vertex there.
        Vertex,
    }
    let step = |to, levels, across, times| Step {
        to,
        levels,
        across,
        times,
    };
    let to_ids = |list: &[usize], levels, across, times, steps: &mut Vec<Step>| {
        steps.extend(list.iter().map(|i| step(id + i, levels, across, times)));
    };
    // What a walk node leads to.
    let next = |v: usize| -> Vec<Step> {
        let mut steps = Vec::new();
        if v >= id {
            for e in &having[v - id] {
                steps.push(step(followed + e, 0, false, Times::Content));
            }
            return steps;
        }
        let e = v % count;
        let node = doc
            .get_node(roxmltree::NodeId::from(e))
            .expect("a node of this document");
        let parent = node.parent().map(|p| index(p.id()));
        if v >= paint {
            let base = v - e;
            steps.extend(parent.map(|p| step(base + p, 0, false, Times::Once)));
            let list = if base == paint {
                &refs[e].paint
            } else {
                &refs[e].markers
            };
            to_ids(list, REF_LEVELS, false, Times::Once, &mut steps);
            return steps;
        }
        if v >= followed {
            steps.push(step(e, 0, false, Times::Content));
            // What it inherits from where it is, unless it draws nothing.
            if !PAINT_SERVERS.contains(&node.tag_name().name()) {
                steps.extend(parent.map(|p| step(paint + p, 0, true, Times::Each)));
                steps.extend(parent.map(|p| step(markers + p, 0, true, Times::Vertex)));
            }
            return steps;
        }
        for c in node.children().filter(|c| c.is_element()) {
            steps.push(step(index(c.id()), 1, false, Times::Child));
        }
        let r = &refs[e];
        to_ids(&r.href, REF_LEVELS, false, Times::Content, &mut steps);
        to_ids(&r.paint, REF_LEVELS, true, Times::Each, &mut steps);
        to_ids(&r.markers, REF_LEVELS, true, Times::Vertex, &mut steps);
        to_ids(&r.other, REF_LEVELS, true, Times::Once, &mut steps);
        // An `inherit` of what is not inherited takes the parent's.
        for (k, owner) in owner[e].iter().enumerate() {
            if let (true, Some(o)) = (r.inherits[k], owner) {
                let list = &refs[*o].not_inherited[k];
                to_ids(list, REF_LEVELS, true, Times::Once, &mut steps);
            }
        }
        steps
    };

    // A depth-first walk, keeping the deepest each node leads to, and how
    // many elements converting it converts, once all it leads to is known;
    // and for each node, how many elements and vertices are drawn as its
    // content, which inherit its paint and markers.
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        New,
        Walking,
        Done,
    }
    let mut state = vec![State::New; id + having.len()];
    let mut depth = vec![0usize; state.len()];
    let mut work = vec![0usize; state.len()];
    let (mut below, mut vertices) = (vec![0usize; state.len()], vec![0usize; state.len()]);
    // A node, what it leads to and how far through that the walk is.
    let mut stack: Vec<(usize, Vec<Step>, usize)> = Vec::new();
    let mut deepest = 0;
    for start in elements().map(|n| index(n.id())) {
        if state[start] != State::New {
            continue;
        }
        state[start] = State::Walking;
        stack.push((start, next(start), 0));
        while let Some((node, leads_to, at)) = stack.last_mut() {
            let node = *node;
            let Some(&Step { to, .. }) = leads_to.get(*at) else {
                let element = node < followed;
                if element {
                    below[node] = 1;
                    vertices[node] = refs[node].vertices;
                }
                for s in leads_to.iter() {
                    if !matches!(s.times, Times::Child | Times::Content) {
                        continue;
                    }
                    // Only one element with an id is the one followed.
                    let add = |a: usize, b| {
                        if node >= id {
                            a.max(b)
                        } else {
                            a.saturating_add(b)
                        }
                    };
                    below[node] = add(below[node], below[s.to]);
                    vertices[node] = add(vertices[node], vertices[s.to]);
                }
                let (mut down, mut across) = (usize::from(element), 0usize);
                let mut w = usize::from(element);
                for s in leads_to.iter() {
                    let d = s.levels.saturating_add(depth[s.to]);
                    if s.across {
                        across = across.max(d);
                    } else {
                        down = down.max(d);
                    }
                    let times = match s.times {
                        Times::Child | Times::Content | Times::Once => 1,
                        Times::Each => below[node],
                        Times::Vertex => vertices[node],
                    };
                    let converts = times.saturating_mul(work[s.to]);
                    // Only one element with an id is the one followed.
                    w = if node >= id {
                        w.max(converts)
                    } else {
                        w.saturating_add(converts)
                    };
                }
                depth[node] = down.saturating_add(across);
                work[node] = w;
                state[node] = State::Done;
                stack.pop();
                continue;
            };
            *at += 1;
            match state[to] {
                State::New => {
                    state[to] = State::Walking;
                    stack.push((to, next(to), 0));
                }
                // A loop, which usvg may or may not break.
                State::Walking => return Cost::LOOP,
                State::Done => {}
            }
        }
        deepest = deepest.max(depth[start]);
    }
    Cost {
        depth: deepest,
        work: work[index(doc.root_element().id())],
    }
}

/// The ids named by `url(#id)`s in `value`.
fn url_ids<'a>(value: &'a str, ids: &mut Vec<&'a str>) {
    for (_, rest) in value
        .match_indices("url(")
        .map(|(i, _)| value.split_at(i + 4))
    {
        let inner = rest.split(')').next().unwrap_or(rest);
        if let Some((_, id)) = inner.split_once('#') {
            ids.push(id.trim().trim_end_matches(['"', '\'']).trim());
        }
    }
}

/// An element, for matching style sheet selectors as usvg does.
struct CssNode<'a, 'input>(roxmltree::Node<'a, 'input>);

impl simplecss::Element for CssNode<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(CssNode)
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(CssNode)
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }

    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator) -> bool {
        self.0.attribute(name).is_some_and(|v| operator.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        class == simplecss::PseudoClass::FirstChild && self.0.prev_sibling_element().is_none()
    }
}

/// Whether usvg reads an embedded image as an SVG (as it does for plain
/// text it cannot tell is a raster image).
fn is_svg(mime: &str, data: &[u8]) -> bool {
    use imagesize::ImageType::{Gif, Jpeg, Png, Webp};
    match mime {
        "image/svg+xml" => true,
        "text/plain" => !matches!(imagesize::image_type(data), Ok(Gif | Jpeg | Png | Webp)),
        _ => false,
    }
}

/// Whether an embedded raster image (decoded later, at render time) declares
/// a size we can afford. An image whose size we cannot read is refused.
fn raster_ok(mime: &str, data: &[u8]) -> bool {
    let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let declared =
        imagesize::blob_size(data).is_ok_and(|size| size_ok(side(size.width), side(size.height)));
    // resvg decodes a GIF's first frame, which may be larger than the
    // "screen" size imagesize reads.
    let gif =
        matches!(imagesize::image_type(data), Ok(imagesize::ImageType::Gif)) || mime == "image/gif";
    declared && (!gif || gif_frame_size(data).is_some_and(|(w, h)| size_ok(w, h)))
}

/// The size of the first frame of a GIF.
fn gif_frame_size(data: &[u8]) -> Option<(u32, u32)> {
    if !data.starts_with(b"GIF8") {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes([*data.get(i)?, *data.get(i + 1)?]));
    let flags = *data.get(10)?;
    let mut i = 13;
    if flags & 0x80 != 0 {
        i += 3 << ((flags & 7) + 1);
    }
    loop {
        match *data.get(i)? {
            // An image descriptor: left, top, width, height.
            0x2C => return Some((u16_at(i + 5)?.into(), u16_at(i + 7)?.into())),
            // An extension: its label, then data sub-blocks up to an empty one.
            0x21 => {
                i += 2;
                loop {
                    let len = usize::from(*data.get(i)?);
                    i += 1 + len;
                    if len == 0 {
                        break;
                    }
                }
            }
            _ => return None,
        }
    }
}

/// The deepest nesting of groups under `root`, counting the groups a render
/// recurses into besides children (clip paths, masks, patterns, filter
/// images, text and embedded SVGs). Found without recursion.
pub fn tree_depth(root: &usvg::Group) -> usize {
    let mut deepest = 0;
    let mut todo = vec![(root, 1)];
    while let Some((group, depth)) = todo.pop() {
        deepest = deepest.max(depth);
        if depth > MAX_TREE_DEPTH {
            // Deep enough to refuse; the rest need not be walked.
            break;
        }
        let mut sub = |g| todo.push((g, depth + 1));
        let mut clip = group.clip_path();
        while let Some(c) = clip {
            sub(c.root());
            clip = c.clip_path();
        }
        let mut mask = group.mask();
        while let Some(m) = mask {
            sub(m.root());
            mask = m.mask();
        }
        for filter in group.filters() {
            for p in filter.primitives() {
                if let usvg::filter::Kind::Image(image) = p.kind() {
                    sub(image.root());
                }
            }
        }
        for node in group.children() {
            match node {
                usvg::Node::Group(g) => sub(g),
                usvg::Node::Path(path) => {
                    let fill = path.fill().map(usvg::Fill::paint);
                    let stroke = path.stroke().map(usvg::Stroke::paint);
                    for paint in [fill, stroke].into_iter().flatten() {
                        if let usvg::Paint::Pattern(p) = paint {
                            sub(p.root());
                        }
                    }
                }
                usvg::Node::Image(image) => {
                    if let usvg::ImageKind::SVG(tree) = image.kind() {
                        sub(tree.root());
                    }
                }
                usvg::Node::Text(text) => sub(text.flattened()),
            }
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">"#;

    /// `n` groups, each needing its own canvas, around a square.
    fn nest(n: usize) -> String {
        format!(
            r#"{}<rect width="5" height="5"/>{}"#,
            r#"<g opacity="0.9">"#.repeat(n),
            "</g>".repeat(n)
        )
    }

    /// Run `f` on a small stack, as the browser has, so that an unbounded
    /// recursion fails the test rather than passing on a big one.
    fn on_small_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    /// Plain base64, for data: URLs.
    fn base64(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                out.push(if i <= c.len() {
                    T[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        out
    }

    #[test]
    fn deeply_nested_xml_is_an_error_not_a_stack_overflow() {
        on_small_stack(|| {
            let deep = format!("{SVG}{}</svg>", nest(50_000));
            assert!(parse(deep.as_bytes()).is_err());
            // A DOCTYPE does not hide it.
            let doctype = format!("<?xml version=\"1.0\"?>\n<!DOCTYPE svg>\n{deep}");
            assert!(parse(doctype.as_bytes()).is_err());
            // Nor do entities that expand to more groups.
            let mut dtd = String::from("<!DOCTYPE svg [");
            dtd += &format!(r#"<!ENTITY e0 "{}">"#, "<g/>");
            for i in 1..8 {
                let (open, close) = (r#"<g opacity="0.9">"#.repeat(10), "</g>".repeat(10));
                dtd += &format!(r#"<!ENTITY e{i} '{open}&e{};{close}'>"#, i - 1);
            }
            dtd += "]>";
            let entities = format!("{dtd}{SVG}&e7;</svg>");
            assert!(parse(entities.as_bytes()).is_err());
        });
    }

    #[test]
    fn illustrator_doctypes_are_fine() {
        let svg = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_extend "http://ns.adobe.com/Extensibility/1.0/">
	<!ENTITY ns_ai "http://ns.adobe.com/AdobeIllustrator/10.0/">
]>
{SVG}{}</svg>"#,
            nest(4)
        );
        assert!(parse(svg.as_bytes()).is_ok());
    }

    #[test]
    fn deep_trees_from_shallow_xml_are_refused() {
        on_small_stack(|| {
            // Groups the renderer recurses into: fine a few deep, not many.
            let ok = format!("{SVG}{}</svg>", nest(MAX_TREE_DEPTH - 2));
            assert!(parse(ok.as_bytes()).is_ok());
            let deep = format!("{SVG}{}</svg>", nest(MAX_TREE_DEPTH + 1));
            assert!(parse(deep.as_bytes()).is_err());

            // A chain of `<use>`s, each a level deeper, with little XML.
            let mut defs = String::from(r#"<defs><g id="g0"><rect width="5" height="5"/></g>"#);
            for i in 1..=500 {
                defs += &format!(
                    r##"<g id="g{i}" opacity="0.9"><use xlink:href="#g{}"/></g>"##,
                    i - 1
                );
            }
            let uses = format!(r##"{SVG}{defs}</defs><use xlink:href="#g500"/></svg>"##);
            assert!(parse(uses.as_bytes()).is_err());

            // Embedded SVGs, each nested in the last.
            let mut svg = format!("{SVG}{}</svg>", nest(20));
            for _ in 0..3 {
                svg = format!(
                    r#"{SVG}{}<image width="10" height="10" xlink:href="data:image/svg+xml;base64,{}"/>{}</svg>"#,
                    "<g>".repeat(20),
                    base64(svg.as_bytes()),
                    "</g>".repeat(20)
                );
            }
            assert!(parse(svg.as_bytes()).is_err());
        });
    }

    /// Chains of `n` references of each kind usvg follows while parsing,
    /// each a few elements deep.
    fn chains(n: usize) -> Vec<(&'static str, String)> {
        let chain = |first: &str, link: &dyn Fn(usize) -> String, last: String| {
            let links: String = (1..n).map(link).collect();
            format!("{SVG}<defs>{first}{links}</defs>{last}</svg>")
        };
        vec![
            (
                "pattern",
                chain(
                    r#"<pattern id="p0" width="1" height="1"><rect width="1" height="1"/></pattern>"#,
                    &|i| {
                        format!(
                            r#"<pattern id="p{i}" width="1" height="1"><rect width="1" height="1" fill="url(#p{})"/></pattern>"#,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" fill="url(#p{})"/>"#, n - 1),
                ),
            ),
            (
                "marker",
                chain(
                    r#"<marker id="k0"><rect width="1" height="1"/></marker>"#,
                    &|i| {
                        format!(
                            r#"<marker id="k{i}"><path d="M0 0L1 1" stroke="black" marker-start="url(#k{})"/></marker>"#,
                            i - 1
                        )
                    },
                    format!(
                        r#"<path d="M1 1L5 5" stroke="black" marker-start="url(#k{})"/>"#,
                        n - 1
                    ),
                ),
            ),
            (
                "mask",
                chain(
                    r#"<mask id="m0"><rect width="5" height="5" fill="white"/></mask>"#,
                    &|i| {
                        format!(
                            r#"<mask id="m{i}"><rect width="5" height="5" fill="white" mask="url(#m{})"/></mask>"#,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" mask="url(#m{})"/>"#, n - 1),
                ),
            ),
            (
                "clip-path",
                chain(
                    r#"<clipPath id="c0"><rect width="5" height="5"/></clipPath>"#,
                    &|i| {
                        format!(
                            r#"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="5" height="5"/></clipPath>"#,
                            i - 1
                        )
                    },
                    format!(
                        r#"<rect width="10" height="10" clip-path="url(#c{})"/>"#,
                        n - 1
                    ),
                ),
            ),
            (
                "feImage",
                chain(
                    r#"<rect id="r0" width="5" height="5"/>"#,
                    &|i| {
                        format!(
                            r##"<filter id="f{i}"><feImage href="#r{}"/></filter><rect id="r{i}" width="5" height="5" filter="url(#f{i})"/>"##,
                            i - 1
                        )
                    },
                    format!(r##"<use href="#r{}"/>"##, n - 1),
                ),
            ),
            (
                // References a style sheet makes.
                "css",
                chain(
                    &format!(
                        "<style>{}</style>{}",
                        (1..n)
                            .map(|i| format!(".s{i}{{fill:url(#q{})}}", i - 1))
                            .collect::<String>(),
                        r#"<pattern id="q0" width="1" height="1"><rect width="1" height="1"/></pattern>"#
                    ),
                    &|i| {
                        format!(
                            r#"<pattern id="q{i}" width="1" height="1"><rect class="s{i}" width="1" height="1"/></pattern>"#
                        )
                    },
                    format!(r#"<rect width="10" height="10" fill="url(#q{})"/>"#, n - 1),
                ),
            ),
            (
                // A fill each pattern inherits from where it is.
                "inherited fill",
                chain(
                    r#"<pattern id="a0" width="1" height="1"><rect width="1" height="1"/></pattern>"#,
                    &|i| {
                        format!(
                            r#"<g fill="url(#a{})"><pattern id="a{i}" width="1" height="1"><rect width="1" height="1"/></pattern></g>"#,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" fill="url(#a{})"/>"#, n - 1),
                ),
            ),
            (
                // A mask each mask takes from its parent by `inherit`.
                "mask inherit",
                chain(
                    r#"<mask id="m0"><rect width="5" height="5" fill="white"/></mask>"#,
                    &|i| {
                        format!(
                            r#"<g mask="url(#m{})"><mask id="m{i}" mask="inherit"><rect width="5" height="5" fill="white"/></mask></g>"#,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" mask="url(#m{})"/>"#, n - 1),
                ),
            ),
            (
                // A filter image, converted where it is, inheriting a fill.
                "feImage fill",
                chain(
                    r#"<pattern id="p0" width="1" height="1"><rect width="1" height="1"/></pattern>"#,
                    &|i| {
                        format!(
                            r##"<g fill="url(#p{})"><rect id="r{i}" width="5" height="5"/></g><filter id="f{i}"><feImage href="#r{i}"/></filter><pattern id="p{i}" width="1" height="1"><rect width="1" height="1" filter="url(#f{i})"/></pattern>"##,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" fill="url(#p{})"/>"#, n - 1),
                ),
            ),
            (
                // ... a marker.
                "feImage marker",
                chain(
                    r#"<marker id="k0"><rect width="1" height="1"/></marker>"#,
                    &|i| {
                        format!(
                            r##"<g marker-start="url(#k{})"><path id="r{i}" d="M0 0L1 1" stroke="black"/></g><filter id="f{i}"><feImage href="#r{i}"/></filter><marker id="k{i}"><rect width="1" height="1" filter="url(#f{i})"/></marker>"##,
                            i - 1
                        )
                    },
                    format!(
                        r#"<path d="M1 1L5 5" stroke="black" marker-start="url(#k{})"/>"#,
                        n - 1
                    ),
                ),
            ),
            (
                // ... or its parent's mask, by `inherit`.
                "feImage mask inherit",
                chain(
                    r#"<mask id="m0"><rect width="5" height="5" fill="white"/></mask>"#,
                    &|i| {
                        format!(
                            r##"<g mask="url(#m{})"><rect id="r{i}" width="5" height="5" mask="inherit"/></g><filter id="f{i}"><feImage href="#r{i}"/></filter><mask id="m{i}"><rect width="5" height="5" fill="white" filter="url(#f{i})"/></mask>"##,
                            i - 1
                        )
                    },
                    format!(r#"<rect width="10" height="10" mask="url(#m{})"/>"#, n - 1),
                ),
            ),
        ]
    }

    #[test]
    fn long_reference_chains_are_refused_before_usvg_follows_them() {
        on_small_stack(|| {
            for (kind, svg) in chains(4) {
                assert!(parse(svg.as_bytes()).is_ok(), "{kind}");
            }
            for (kind, svg) in chains(3000) {
                let e = parse(svg.as_bytes()).err();
                assert_eq!(
                    e.as_deref(),
                    Some("SVG nests elements or references too deeply"),
                    "{kind}"
                );
            }
        });
    }

    #[test]
    fn inherited_references_count_from_where_they_are_used() {
        // Each link of these chains is followed from 60 levels down, which
        // the XML or the references alone do not show.
        let deep = |leaf: &str| format!("{}{leaf}{}", "<g>".repeat(60), "</g>".repeat(60));
        let rect = r#"<rect width="1" height="1"/>"#;
        let chain = |link: &dyn Fn(usize) -> String| {
            let links: String = (1..14).map(link).collect();
            format!(
                r#"{SVG}<defs><g id="deep">{}</g><pattern id="p0" width="1" height="1">{rect}</pattern>{links}</defs><rect width="10" height="10" fill="url(#p13)"/></svg>"#,
                deep(rect)
            )
        };
        let pattern = |i: usize, content: String| {
            format!(r#"<pattern id="p{i}" width="1" height="1">{content}</pattern>"#)
        };
        let chains = [
            // A fill on a group, used by the rect deep inside it.
            chain(&|i| {
                pattern(
                    i,
                    format!(r#"<g fill="url(#p{})">{}</g>"#, i - 1, deep(rect)),
                )
            }),
            // A fill the pattern's content inherits from the pattern's parent.
            chain(&|i| {
                format!(
                    r#"<g fill="url(#p{})">{}</g>"#,
                    i - 1,
                    pattern(i, deep(rect))
                )
            }),
            // A fill the content of a `<use>` inherits from the `<use>`.
            chain(&|i| {
                pattern(
                    i,
                    format!(r##"<use href="#deep" fill="url(#p{})"/>"##, i - 1),
                )
            }),
        ];
        on_small_stack(move || {
            for svg in chains {
                let doc = parse_xml(&svg).unwrap();
                assert!(svg_cost(&doc).depth > 13 * 60, "{}", svg_cost(&doc).depth);
                assert!(parse(svg.as_bytes()).is_err());
            }
        });
    }

    #[test]
    fn references_repeated_by_references_are_refused() {
        // `n` levels of `wrap`, each holding `per` copies of `link` naming
        // the level before: `per^n` copies of the first, as usvg caches
        // none of these.
        let levels = |n: usize, per: usize, first: &str, link: &str, wrap: &str| {
            let links: String = (1..=n)
                .map(|i| {
                    let content = link.replace("PREV", &(i - 1).to_string()).repeat(per);
                    wrap.replace("ID", &i.to_string())
                        .replace("CONTENT", &content)
                })
                .collect();
            format!(r##"{SVG}<defs>{first}{links}</defs><use href="#x{n}"/></svg>"##)
        };
        let marker = r#"<marker id="x0"><rect width="1" height="1"/></marker>"#;
        let path = format!(
            r#"<path id="P" d="M0 0{}" stroke="black"/>"#,
            (1..=20)
                .map(|i| format!(" L{i} {}", i % 2))
                .collect::<String>()
        );
        let svgs = [
            levels(
                6,
                10,
                marker,
                r#"<path d="M0 0L1 1" stroke="black" marker-start="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            // Markers repeat at each vertex too.
            levels(
                4,
                1,
                marker,
                &format!(
                    r#"<path d="M0 0{}" stroke="black" marker-mid="url(#xPREV)"/>"#,
                    "L1 1".repeat(20)
                ),
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            // Different properties naming the same thing convert it twice.
            levels(
                20,
                1,
                marker,
                r#"<path d="M0 0L1 1" stroke="black" marker-start="url(#xPREV)" marker-end="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            levels(
                20,
                1,
                marker,
                r#"<path d="M0 0L1 1" stroke="black" marker="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            levels(
                20,
                1,
                r#"<pattern id="x0" width="1" height="1" patternContentUnits="objectBoundingBox"><rect width="1" height="1"/></pattern>"#,
                r#"<rect width="1" height="1" fill="url(#xPREV)" stroke="url(#xPREV)"/>"#,
                r#"<pattern id="xID" width="1" height="1" patternContentUnits="objectBoundingBox">CONTENT</pattern>"#,
            ),
            // Every shape is converted to a path that markers go on.
            levels(
                6,
                3,
                marker,
                r#"<circle r="1" stroke="black" marker-mid="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            levels(
                6,
                3,
                marker,
                r#"<rect width="1" height="1" rx="0.5" stroke="black" marker-mid="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            levels(
                6,
                3,
                marker,
                r#"<ellipse rx="1" ry="2" stroke="black" marker-mid="url(#xPREV)"/>"#,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            // What a `<use>` clones inherits from it, as markers on a path
            // of 20 vertices ...
            levels(
                6,
                1,
                &format!(r#"{path}{marker}"#),
                r##"<g marker-mid="url(#xPREV)"><use href="#P"/></g>"##,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            levels(
                6,
                1,
                &format!(r#"{path}{marker}"#),
                r##"<use href="#P" marker-mid="url(#xPREV)"/>"##,
                r#"<marker id="xID">CONTENT</marker>"#,
            ),
            // ... or a fill for each of ten rects, cloned ten times.
            levels(
                4,
                1,
                &format!(
                    r#"<g id="R">{}</g><pattern id="x0" width="1" height="1" patternContentUnits="objectBoundingBox"><rect width="1" height="1"/></pattern>"#,
                    r#"<rect width="1" height="1"/>"#.repeat(10)
                ),
                &format!(
                    r#"<g fill="url(#xPREV)">{}</g>"#,
                    r##"<use href="#R"/>"##.repeat(10)
                ),
                r#"<pattern id="xID" width="1" height="1" patternContentUnits="objectBoundingBox">CONTENT</pattern>"#,
            ),
            levels(
                6,
                10,
                r#"<mask id="x0"><rect width="5" height="5" fill="white"/></mask>"#,
                r#"<rect width="5" height="5" fill="white" mask="url(#xPREV)"/>"#,
                r#"<mask id="xID">CONTENT</mask>"#,
            ),
            levels(
                6,
                10,
                r#"<pattern id="x0" width="1" height="1"><rect width="1" height="1"/></pattern>"#,
                r#"<rect width="1" height="1" fill="url(#xPREV)"/>"#,
                r#"<pattern id="xID" width="1" height="1">CONTENT</pattern>"#,
            ),
            levels(
                6,
                10,
                r#"<g id="x0"><rect width="1" height="1"/></g>"#,
                r##"<use href="#xPREV"/>"##,
                r#"<g id="xID">CONTENT</g>"#,
            ),
        ];
        for svg in svgs {
            let doc = parse_xml(&svg).unwrap();
            assert!(svg_cost(&doc).work > MAX_WORK, "{}", svg_cost(&doc).work);
            let e = parse(svg.as_bytes()).err();
            assert_eq!(e.as_deref(), Some("SVG is too large or complex"));
        }
        // So is a big SVG without references.
        let big = format!(
            "{SVG}{}</svg>",
            r#"<rect width="1" height="1"/>"#.repeat(MAX_WORK)
        );
        let e = parse(big.as_bytes()).err();
        assert_eq!(e.as_deref(), Some("SVG is too large or complex"));
        // Start and end markers go once on each path, so arrowheads on
        // long paths are fine.
        let curve = format!("M0 0{}", " C1 2 3 4 5 6".repeat(80));
        let arrows = format!(
            r#"{SVG}<defs><marker id="a"><path d="M0 0 5 -5 -12 0 5 5Z"/></marker></defs>{}</svg>"#,
            format!(
                r#"<path d="{curve}" stroke="black" marker-start="url(#a)" marker-end="url(#a)"/>"#
            )
            .repeat(50)
        );
        assert!(parse(arrows.as_bytes()).is_ok());
        // A few repeats are fine.
        let svg = levels(
            3,
            10,
            r#"<g id="x0"><rect width="1" height="1"/></g>"#,
            r##"<use href="#xPREV"/>"##,
            r#"<g id="xID">CONTENT</g>"#,
        );
        assert!(parse(svg.as_bytes()).is_ok());
        // Embedded SVGs share the budget, however many times each is used.
        let work = svg_cost(&parse_xml(&svg).unwrap()).work;
        let image = format!(
            r#"<image id="i" width="10" height="10" xlink:href="data:image/svg+xml;base64,{}"/>"#,
            base64(svg.as_bytes())
        );
        let n = MAX_WORK / work + 2;
        let outer = format!(
            r##"{SVG}<defs>{image}</defs>{}</svg>"##,
            r##"<use href="#i"/>"##.repeat(n)
        );
        let tree = parse(outer.as_bytes()).unwrap();
        let mut images = 0;
        let mut todo = vec![tree.root()];
        while let Some(g) = todo.pop() {
            for node in g.children() {
                match node {
                    usvg::Node::Group(g) => todo.push(g),
                    usvg::Node::Image(_) => images += 1,
                    _ => {}
                }
            }
        }
        assert!(images > 0 && images < n, "{images} of {n}");
    }

    #[test]
    fn reference_loops_are_refused() {
        // usvg itself follows a pattern that inherits itself for ever.
        let svg = format!(
            r#"{SVG}<g fill="url(#p)"><pattern id="p" width="1" height="1"><rect width="1" height="1"/></pattern><rect width="5" height="5"/></g></svg>"#
        );
        assert_eq!(svg_cost(&parse_xml(&svg).unwrap()).depth, usize::MAX);
        let svg = format!(
            r#"{SVG}<defs><pattern id="a" width="1" height="1"><rect width="1" height="1" fill="url(#b)"/></pattern><pattern id="b" width="1" height="1"><rect width="1" height="1" style="fill: url( '#a' )"/></pattern></defs><rect width="10" height="10" fill="url(#a)"/></svg>"#
        );
        let doc = parse_xml(&svg).unwrap();
        assert_eq!(svg_cost(&doc).depth, usize::MAX);
        assert!(parse(svg.as_bytes()).is_err());
        // A gradient inside what it fills is no loop: its stops draw nothing.
        let svg = format!(
            r#"{SVG}<g fill="url(#g)"><linearGradient id="g"><stop offset="0"/></linearGradient><rect width="5" height="5"/></g></svg>"#
        );
        assert!(parse(svg.as_bytes()).is_ok());
    }

    #[test]
    fn references_count_towards_the_depth() {
        let depth = |body: &str| svg_cost(&parse_xml(&format!("{SVG}{body}</svg>")).unwrap()).depth;
        assert_eq!(depth("<g><rect/></g>"), 3);
        // svg > rect, then the pattern the rect names: pattern > rect.
        let pattern = r#"<pattern id="p"><rect/></pattern>"#;
        assert_eq!(
            depth(&format!(r#"{pattern}<rect fill="url(#p)"/>"#)),
            2 + REF_LEVELS + 2
        );
        assert_eq!(
            depth(&format!(
                r#"{pattern}<rect style="stroke:url(&quot;#p&quot;)"/>"#
            )),
            2 + REF_LEVELS + 2
        );
        // An `<a>` links to, rather than draws, what it names.
        assert_eq!(depth(r##"<g id="g"><a href="#g"><rect/></a></g>"##), 4);
        // Names that are not there lead nowhere.
        assert_eq!(depth(r#"<rect fill="url(#nothing)"/>"#), 2);
    }

    #[test]
    fn embedded_svgs_share_the_xml_depth() {
        // XML nesting that draws nothing, so only the XML depth counts.
        let metadata = |n: usize| {
            format!(
                "<metadata>{}{}</metadata>",
                "<m>".repeat(n),
                "</m>".repeat(n)
            )
        };
        let inner = format!(
            r#"{SVG}{}<rect width="5" height="5"/></svg>"#,
            metadata(100)
        );
        let outer = |depth: usize| {
            format!(
                r#"{SVG}{}<image width="10" height="10" xlink:href="data:image/svg+xml;base64,{}"/></svg>"#,
                metadata(depth),
                base64(inner.as_bytes()),
            )
        };
        let has_image = |svg: &str| parse(svg.as_bytes()).unwrap().root().has_children();
        assert!(has_image(&outer(10)));
        assert!(!has_image(&outer(40)));
    }

    #[test]
    fn oversized_jpeg_webp_and_gif_are_refused() {
        let (w, h) = (20_000u32, 20_000u32);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8];
        jpeg.extend((h as u16).to_be_bytes());
        jpeg.extend((w as u16).to_be_bytes());
        jpeg.extend([3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
        let mut webp = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend(&(w - 1).to_le_bytes()[..3]);
        webp.extend(&(h - 1).to_le_bytes()[..3]);
        let gif = |screen: u16, frame: u16| {
            let mut gif = b"GIF89a".to_vec();
            gif.extend(screen.to_le_bytes());
            gif.extend(screen.to_le_bytes());
            // A two-entry global colour table.
            gif.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
            // A graphic control extension.
            gif.extend([0x21, 0xF9, 4, 0, 0, 0, 0, 0]);
            gif.extend([0x2C, 0, 0, 0, 0]);
            gif.extend(frame.to_le_bytes());
            gif.extend(frame.to_le_bytes());
            gif.extend([0, 2, 2, 0x4C, 0x01, 0, 0x3B]);
            gif
        };
        assert_eq!(gif_frame_size(&gif(1, 7)), Some((7, 7)));
        assert!(raster_ok("image/gif", &gif(1, 1)));
        for (mime, data) in [
            ("image/jpeg", jpeg),
            ("image/webp", webp),
            ("image/gif", gif(w as u16, 1)),
            // A small screen does not hide a large frame.
            ("image/gif", gif(1, w as u16)),
            ("text/plain", gif(1, w as u16)),
            // Nor does a header we cannot read.
            ("image/png", b"\x89PNG".to_vec()),
            ("image/jpeg", b"\xFF\xD8".to_vec()),
        ] {
            assert!(!raster_ok(mime, &data), "{mime} {data:?}");
        }
        assert!(!size_ok(8192 * 8, 1));
        assert!(size_ok(4096, 4096));
    }
}
