//! Parsing SVGs, and the images they embed, without trusting them.
//!
//! usvg parses, and resvg renders, by recursing once per level of nesting, and
//! a stack overflow aborts the viewer. So nesting is bounded twice: the XML
//! before usvg parses it, and the tree usvg builds (which `<use>`, embedded
//! SVGs and entities can make deeper than the XML) before it is rendered.
//! Embedded raster images are decoded at render time, so their declared
//! sizes are checked first.
//!
//! This module only depends on usvg, `bob` and `imagesize`, so a test build
//! for the browser can include it on its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use resvg::usvg;

/// Deepest element nesting of SVG XML handed to usvg, counting the SVGs
/// embedded in it together.
pub const MAX_XML_DEPTH: usize = 128;

/// Deepest nesting of groups in a parsed SVG (counting clip paths, masks,
/// patterns, filters and embedded SVGs) that we render. Rendering takes
/// about 28 KiB of stack a level in the browser, so this many fit even in
/// wasm's default 1 MiB stack (the build gives it 8 MiB, see `build.rs`).
/// Real symbols nest groups a few deep.
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
    let depth = bob::xml_depth(std::str::from_utf8(data).map_err(|_| "SVG is not UTF-8 text")?);
    if depth > MAX_XML_DEPTH {
        return Err("SVG nests elements too deeply".into());
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
                    let depth = xml_depth(&data);
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
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;
    if tree_depth(tree.root()) > MAX_TREE_DEPTH {
        return Err("SVG nests groups too deeply".into());
    }
    Ok(tree)
}

/// Element nesting of `data`, or `usize::MAX` if it is not text.
fn xml_depth(data: &[u8]) -> usize {
    std::str::from_utf8(data).map_or(usize::MAX, bob::xml_depth)
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
