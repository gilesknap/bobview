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
//! can make deeper than the XML) before it is rendered. Embedded raster
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
/// together. A plain chain of references may be about 30 long; usvg needed
/// about 700 to exhaust Chromium's stack, and real SVGs follow two or three.
pub const MAX_XML_DEPTH: usize = 128;

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
    let depth = svg_depth(&doc);
    if depth > MAX_XML_DEPTH {
        return Err("SVG nests elements or references too deeply".into());
    }
    // How deep the XML being parsed is so far, embedded SVGs included.
    let used = Arc::new(AtomicUsize::new(depth));
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
                    // A nested SVG is parsed now, deeper on the same stack.
                    let depth = std::str::from_utf8(&data)
                        .ok()
                        .and_then(|text| parse_xml(text).ok())
                        .map_or(usize::MAX, |doc| svg_depth(&doc));
                    let outer = used.load(Ordering::Relaxed);
                    if outer.saturating_add(depth) > MAX_XML_DEPTH {
                        log::warn!("embedded SVG is not shown: nested too deeply");
                        return None;
                    }
                    used.store(outer + depth, Ordering::Relaxed);
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

/// The deepest element nesting in `doc` with the references usvg follows
/// (`url(#id)` in an attribute, a `style` or a style sheet, and an `href` to
/// `#id`) counted as `REF_LEVELS` levels each: how deep usvg recurses to
/// parse it. `usize::MAX` if references form a loop. Found without
/// recursion.
pub fn svg_depth(doc: &roxmltree::Document) -> usize {
    let mut ids: HashMap<&str, Vec<roxmltree::NodeId>> = HashMap::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_default().push(node.id());
        }
    }
    // Style sheets, as usvg reads them, keeping only the rules that refer to
    // something.
    let mut sheet = simplecss::StyleSheet::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("style")) {
        if matches!(node.attribute("type"), None | Some("text/css"))
            && let Some(text) = node.text()
        {
            sheet.parse_more(text);
        }
    }
    sheet
        .rules
        .retain(|rule| rule.declarations.iter().any(|d| d.value.contains("url(")));

    // The elements `node` refers to.
    let refs = |node: roxmltree::Node| -> Vec<roxmltree::NodeId> {
        let mut names = Vec::new();
        for attr in node.attributes() {
            if attr.name() == "href" && HREF_ELEMENTS.contains(&node.tag_name().name()) {
                names.extend(attr.value().split_once('#').map(|(_, id)| id.trim()));
            }
            url_ids(attr.value(), &mut names);
        }
        for rule in &sheet.rules {
            if rule.selector.matches(&CssNode(node)) {
                for declaration in &rule.declarations {
                    url_ids(declaration.value, &mut names);
                }
            }
        }
        names
            .iter()
            .filter_map(|name| ids.get(name))
            .flatten()
            .copied()
            .collect()
    };

    // What each node leads to, with the levels each step costs.
    type Steps = Vec<(roxmltree::NodeId, usize)>;
    let next = |id: roxmltree::NodeId| -> Steps {
        let node = doc.get_node(id).expect("a node of this document");
        let children = node.children().filter(|c| c.is_element());
        let refs = refs(node).into_iter().map(|r| (r, REF_LEVELS));
        children.map(|c| (c.id(), 1)).chain(refs).collect()
    };

    // A depth-first walk, keeping the deepest each node leads to once all
    // it leads to is known.
    const NEW: usize = 0;
    const WALKING: usize = usize::MAX;
    let mut depth = vec![NEW; doc.descendants().count()];
    let index = |id: roxmltree::NodeId| id.get_usize();
    // A node, what it leads to and how far through that the walk is.
    let mut stack: Vec<(roxmltree::NodeId, Steps, usize)> = Vec::new();
    let mut deepest = 0;
    for start in doc.descendants().filter(|n| n.is_element()) {
        if depth[index(start.id())] != NEW {
            continue;
        }
        depth[index(start.id())] = WALKING;
        stack.push((start.id(), next(start.id()), 0));
        while let Some((node, leads_to, at)) = stack.last_mut() {
            let Some(&(id, _)) = leads_to.get(*at) else {
                let d = leads_to
                    .iter()
                    .map(|&(id, levels)| levels.saturating_add(depth[index(id)]))
                    .fold(1, usize::max);
                depth[index(*node)] = d.min(WALKING - 1);
                stack.pop();
                continue;
            };
            *at += 1;
            match depth[index(id)] {
                NEW => {
                    depth[index(id)] = WALKING;
                    stack.push((id, next(id), 0));
                }
                // A loop, which usvg may or may not break.
                WALKING => return usize::MAX,
                _ => {}
            }
        }
        deepest = deepest.max(depth[index(start.id())]);
    }
    deepest
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
        ]
    }

    #[test]
    fn long_reference_chains_are_refused_before_usvg_follows_them() {
        on_small_stack(|| {
            for (kind, svg) in chains(5) {
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
    fn reference_loops_are_refused() {
        let svg = format!(
            r#"{SVG}<defs><pattern id="a" width="1" height="1"><rect width="1" height="1" fill="url(#b)"/></pattern><pattern id="b" width="1" height="1"><rect width="1" height="1" style="fill: url( '#a' )"/></pattern></defs><rect width="10" height="10" fill="url(#a)"/></svg>"#
        );
        let doc = parse_xml(&svg).unwrap();
        assert_eq!(svg_depth(&doc), usize::MAX);
        assert!(parse(svg.as_bytes()).is_err());
    }

    #[test]
    fn references_count_towards_the_depth() {
        let depth = |body: &str| svg_depth(&parse_xml(&format!("{SVG}{body}</svg>")).unwrap());
        assert_eq!(depth("<g><rect/></g>"), 3);
        // svg > rect, then the pattern the rect names: pattern > rect.
        let pattern = r#"<pattern id="p"><rect/></pattern>"#;
        assert_eq!(
            depth(&format!(r#"{pattern}<rect fill="url(#p)"/>"#)),
            2 + REF_LEVELS - 1 + 2
        );
        assert_eq!(
            depth(&format!(
                r#"{pattern}<rect style="stroke:url(&quot;#p&quot;)"/>"#
            )),
            2 + REF_LEVELS - 1 + 2
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
