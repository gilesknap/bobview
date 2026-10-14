//! Where a display comes from: a local file or an http(s) URL, and how the
//! files it names (in actions, embedded displays, tabs) are found from there.

use std::fmt;
use std::path::PathBuf;

/// A display's location.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    File(PathBuf),
    /// An absolute `http://` or `https://` URL.
    Url(String),
}

impl Source {
    /// A path or URL as typed by the user; `file://` URLs become paths.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if is_http(text) {
            Source::Url(text.to_string())
        } else if let Some(path) = text.strip_prefix("file://") {
            Source::File(PathBuf::from(path))
        } else {
            Source::File(PathBuf::from(text))
        }
    }

    pub fn is_url(&self) -> bool {
        matches!(self, Source::Url(_))
    }

    /// Resolve a file named by the display at `self`: relative names are
    /// found next to it, and absolute URLs are used as they are.
    pub fn resolve(&self, file: &str) -> Source {
        let file = file.trim();
        if is_http(file) {
            return Source::Url(file.to_string());
        }
        match self {
            Source::Url(base) => Source::Url(join_url(base, file)),
            Source::File(path) => {
                let file = file.strip_prefix("file://").unwrap_or(file);
                let rel = std::path::Path::new(file);
                match path.parent() {
                    Some(dir) if rel.is_relative() => Source::File(dir.join(rel)),
                    _ => Source::File(rel.to_path_buf()),
                }
            }
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::File(p) => write!(f, "{}", p.display()),
            Source::Url(u) => f.write_str(u),
        }
    }
}

impl From<PathBuf> for Source {
    fn from(p: PathBuf) -> Self {
        Source::File(p)
    }
}

fn is_http(text: &str) -> bool {
    let lower = |n: usize| text.get(..n).map(str::to_ascii_lowercase);
    lower(7).as_deref() == Some("http://") || lower(8).as_deref() == Some("https://")
}

/// Resolve the reference `rel` against the absolute URL `base` (RFC 3986 §5.2).
fn join_url(base: &str, rel: &str) -> String {
    let rel = encode(rel);
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let rest = rest.split('#').next().unwrap_or("");
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path_query) = rest.split_at(auth_end);
    let (base_path, base_query) = match path_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_query, None),
    };
    let rel = rel.split('#').next().unwrap_or("");
    let (rel_path, rel_query) = match rel.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rel, None),
    };

    if let Some(network) = rel.strip_prefix("//") {
        return format!("{scheme}://{network}");
    }
    let (path, query) = if rel_path.is_empty() {
        (base_path.to_string(), rel_query.or(base_query))
    } else if rel_path.starts_with('/') {
        (remove_dots(rel_path), rel_query)
    } else {
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..=i],
            None => "/",
        };
        (remove_dots(&format!("{dir}{rel_path}")), rel_query)
    };
    let path = if path.is_empty() { "/".into() } else { path };
    match query {
        Some(q) => format!("{scheme}://{authority}{path}?{q}"),
        None => format!("{scheme}://{authority}{path}"),
    }
}

/// Drop `.` and `..` segments from an absolute path.
fn remove_dots(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *seg {
            "." | ".." => {
                // Never pop the empty segment before the leading '/'.
                if *seg == ".." && out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Percent-encode the characters a URL may not contain (spaces in file names).
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let ok = c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c);
        if ok {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(base: &str, rel: &str) -> String {
        Source::Url(base.into()).resolve(rel).to_string()
    }

    #[test]
    fn parses_paths_and_urls() {
        assert_eq!(
            Source::parse(" http://h/opi/demo.bob "),
            Source::Url("http://h/opi/demo.bob".into())
        );
        assert!(Source::parse("HTTPS://h/a.bob").is_url());
        assert_eq!(
            Source::parse("file:///tmp/a.bob"),
            Source::File("/tmp/a.bob".into())
        );
        assert_eq!(Source::parse("a/b.bob"), Source::File("a/b.bob".into()));
        assert_eq!(Source::parse("httpd.bob"), Source::File("httpd.bob".into()));
    }

    #[test]
    fn resolves_relative_to_a_url() {
        let base = "http://host:8000/opi/demo.bob";
        assert_eq!(
            url(base, "auto-generated/x/y.bob"),
            "http://host:8000/opi/auto-generated/x/y.bob"
        );
        assert_eq!(url(base, "./a.bob"), "http://host:8000/opi/a.bob");
        assert_eq!(url(base, "../a.bob"), "http://host:8000/a.bob");
        assert_eq!(url(base, "../../../a.bob"), "http://host:8000/a.bob");
        assert_eq!(url(base, "x/../a.bob"), "http://host:8000/opi/a.bob");
        assert_eq!(url(base, "/top/a.bob"), "http://host:8000/top/a.bob");
        assert_eq!(url(base, "//other/a.bob"), "http://other/a.bob");
        assert_eq!(url(base, "a.bob?x=1#f"), "http://host:8000/opi/a.bob?x=1");
        assert_eq!(
            url(base, "My Screen.bob"),
            "http://host:8000/opi/My%20Screen.bob"
        );
        assert_eq!(url(base, "x/.."), "http://host:8000/opi/");
        assert_eq!(url("http://host", "a.bob"), "http://host/a.bob");
        assert_eq!(url("http://host?q", "a.bob"), "http://host/a.bob");
        assert_eq!(
            url("https://h/a/b.bob?v=2#top", ""),
            "https://h/a/b.bob?v=2"
        );
    }

    #[test]
    fn absolute_urls_win() {
        assert_eq!(url("http://a/x.bob", "https://b/y.bob"), "https://b/y.bob");
        assert_eq!(
            Source::File("/opi/x.bob".into()).resolve("http://b/y.bob"),
            Source::Url("http://b/y.bob".into())
        );
    }

    #[test]
    fn resolves_relative_to_a_file() {
        let f = Source::File("/opi/demo.bob".into());
        assert_eq!(f.resolve("a/b.bob"), Source::File("/opi/a/b.bob".into()));
        assert_eq!(f.resolve("/abs/b.bob"), Source::File("/abs/b.bob".into()));
        assert_eq!(
            f.resolve("file:///abs/b.bob"),
            Source::File("/abs/b.bob".into())
        );
    }

    #[test]
    fn removes_dot_segments() {
        assert_eq!(remove_dots("/a/b/c/./../../g"), "/a/g");
        assert_eq!(remove_dots("/a/b/.."), "/a/");
        assert_eq!(remove_dots("/.."), "/");
        assert_eq!(remove_dots("/a//b"), "/a//b");
    }
}
