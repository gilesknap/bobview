//! Serve files from the served directories without following symbolic
//! links out of them (the default; see --follow-symlinks).
//!
//! Files are streamed, with `Last-Modified` and `ETag` for revalidation
//! (`If-None-Match`, `If-Modified-Since`) and single byte ranges, as
//! tower-http's `ServeDir` does.

use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{
    ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE,
    IF_NONE_MATCH, IF_RANGE, LAST_MODIFIED, LOCATION, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

/// The bytes read from a file at a time.
const CHUNK: usize = 64 * 1024;

/// Whether Windows would take `segment` for something other than the file
/// it names: a device (`NUL`, `COM1.txt`), or a name it trims (`a.bob.`,
/// `a.bob `) and so opens another file under.
fn windows_unsafe(segment: &str) -> bool {
    if segment.ends_with(['.', ' ']) {
        return true;
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => match (upper.get(..3), upper.get(3..)) {
            (Some("COM" | "LPT"), Some(port)) => {
                let mut c = port.chars();
                matches!(
                    (c.next(), c.next()),
                    (Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
                )
            }
            _ => false,
        },
    }
}

/// The directories files are served from, in order, each with its real
/// path (symbolic links resolved).
pub struct Roots(Vec<(PathBuf, Option<PathBuf>)>);

impl Roots {
    pub fn new(dirs: &[PathBuf]) -> Self {
        Self(
            dirs.iter()
                .map(|d| (d.clone(), std::fs::canonicalize(d).ok()))
                .collect(),
        )
    }

    /// Open the file a request path names, from the first root that has it
    /// (`index.html` for a directory), unless it is really outside that
    /// root: a symbolic link may lead anywhere on the machine. The file is
    /// checked after it is opened, and served from what was opened, so a
    /// link swapped in meanwhile is caught too.
    fn open(&self, path: &str) -> Found {
        let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
        let mut rel = PathBuf::new();
        for segment in decoded.split('/') {
            match segment {
                "" | "." => {}
                ".." => return Found::Missing,
                s if s.contains('\\') || s.contains('\0') => return Found::Missing,
                // A drive (`C:`) or a file's stream (`a.bob:s`) on Windows.
                s if cfg!(windows) && s.contains(':') => return Found::Missing,
                s if cfg!(windows) && windows_unsafe(s) => return Found::Missing,
                s => rel.push(s),
            }
        }
        for (dir, real_dir) in &self.0 {
            let inside = |real: &Path| real_dir.as_ref().is_some_and(|d| real.starts_with(d));
            let mut file = dir.join(&rel);
            match std::fs::metadata(&file) {
                Ok(m) if m.is_dir() => {
                    if !path.ends_with('/') {
                        // Not redirected if it leads out, which would show
                        // that the name is there.
                        if !std::fs::canonicalize(&file).is_ok_and(|r| inside(&r)) {
                            return Found::Refused;
                        }
                        // One path, not `//host/`, which a browser would
                        // take for another server.
                        return Found::Redirect(format!("/{}/", path.trim_start_matches('/')));
                    }
                    file.push("index.html");
                }
                Ok(m) if m.is_file() => {}
                // A FIFO or device: opening one may block, or do something.
                Ok(_) => return Found::Missing,
                // Not here (or a dangling link): look in the next.
                Err(_) => continue,
            }
            // Without blocking, should a FIFO be swapped in meanwhile; the
            // check below refuses anything but a regular file.
            let Ok(opened) = open_file(&file) else {
                continue;
            };
            let checked = || -> Option<Metadata> {
                let real = std::fs::canonicalize(&file).ok()?;
                let then = opened.metadata().ok()?;
                // The real path is inside, and is the file that was opened.
                (inside(&real) && then.is_file() && is_opened(&real, &opened, &then)?)
                    .then_some(then)
            };
            return match checked() {
                Some(meta) => Found::File(opened, file, meta),
                None => Found::Refused,
            };
        }
        Found::Missing
    }
}

/// Open `file` for reading, without blocking should it be a FIFO.
#[cfg(unix)]
fn open_file(file: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)
}

#[cfg(not(unix))]
fn open_file(file: &Path) -> std::io::Result<File> {
    File::open(file)
}

/// Whether the file at `real` is `opened` (whose metadata is `meta`).
#[cfg(unix)]
fn is_opened(real: &Path, _opened: &File, meta: &Metadata) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let now = std::fs::metadata(real).ok()?;
    Some((now.dev(), now.ino()) == (meta.dev(), meta.ino()))
}

/// Whether the file at `real` is `opened`: the same volume and file index.
#[cfg(windows)]
fn is_opened(real: &Path, opened: &File, _meta: &Metadata) -> Option<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let id = |f: &File| {
        // SAFETY: an open file's handle, and a struct the call fills in.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) };
        let index = (info.nFileIndexHigh, info.nFileIndexLow);
        (ok != 0).then_some((info.dwVolumeSerialNumber, index))
    };
    Some(id(&File::open(real).ok()?)? == id(opened)?)
}

/// What a request path names.
enum Found {
    File(File, PathBuf, Metadata),
    /// A directory, named without its trailing `/`.
    Redirect(String),
    Missing,
    /// Something outside the served directories.
    Refused,
}

/// Serve a file from `roots`, refusing those reached through a symbolic
/// link that leads out of them (see --follow-symlinks).
pub async fn serve(roots: Arc<Roots>, request: Request) -> Response {
    let head = match *request.method() {
        Method::GET => false,
        Method::HEAD => true,
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let path = request.uri().path().to_string();
    let found = tokio::task::spawn_blocking(move || roots.open(&path)).await;
    match found {
        Ok(Found::File(file, name, meta)) => respond(request.headers(), head, file, &name, &meta),
        Ok(Found::Redirect(to)) => {
            let to = match request.uri().query() {
                Some(q) => format!("{to}?{q}"),
                None => to,
            };
            match HeaderValue::from_str(&to) {
                Ok(to) => (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, to)]).into_response(),
                Err(_) => StatusCode::NOT_FOUND.into_response(),
            }
        }
        Ok(Found::Refused) => {
            log::warn!(
                "refused {}: a link out of the served directories (see --follow-symlinks)",
                request.uri().path()
            );
            StatusCode::NOT_FOUND.into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The validators of a file: its `ETag` and its modification time.
struct Validators {
    etag: String,
    modified: Option<SystemTime>,
}

impl Validators {
    fn of(meta: &Metadata) -> Self {
        let modified = meta.modified().ok();
        let since = modified.and_then(|m| m.duration_since(UNIX_EPOCH).ok());
        let nanos = since.unwrap_or_default().as_nanos();
        #[cfg(unix)]
        let etag = {
            use std::os::unix::fs::MetadataExt;
            format!("\"{:x}-{:x}-{nanos:x}\"", meta.ino(), meta.len())
        };
        #[cfg(not(unix))]
        let etag = format!("\"{:x}-{nanos:x}\"", meta.len());
        Self { etag, modified }
    }

    fn last_modified(&self) -> Option<String> {
        self.modified.map(httpdate::fmt_http_date)
    }

    /// Whether the client's copy is current: `If-None-Match` if given,
    /// else `If-Modified-Since`, to the second.
    fn not_modified(&self, headers: &HeaderMap) -> bool {
        if let Some(tags) = headers.get(IF_NONE_MATCH) {
            let Ok(tags) = tags.to_str() else {
                return false;
            };
            // Compared weakly, as RFC 9110 has for If-None-Match.
            return tags
                .split(',')
                .map(str::trim)
                .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == self.etag);
        }
        let since = headers
            .get(IF_MODIFIED_SINCE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| httpdate::parse_http_date(v).ok());
        match (since, self.modified) {
            (Some(since), Some(modified)) => secs(modified) <= secs(since),
            _ => false,
        }
    }

    /// Whether `If-Range`, if given, names this version of the file.
    fn same(&self, headers: &HeaderMap) -> bool {
        match headers.get(IF_RANGE).map(HeaderValue::to_str) {
            None => true,
            Some(Ok(v)) if v.starts_with('"') => v == self.etag,
            Some(Ok(v)) => self.last_modified().is_some_and(|lm| lm == v),
            Some(Err(_)) => false,
        }
    }
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// The part of a file of `len` bytes that a `Range` header asks for.
#[derive(Debug, PartialEq)]
enum Part {
    Whole,
    /// From the first byte to the last, inclusive.
    Bytes(u64, u64),
    Unsatisfiable,
}

/// Read a `Range` header. Only one range of bytes is served; anything
/// else, several ranges or one that does not parse, gets the whole file.
fn part(range: Option<&str>, len: u64) -> Part {
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else {
        return Part::Whole;
    };
    let Some((first, last)) = spec.split_once('-') else {
        return Part::Whole;
    };
    let (first, last) = (first.trim(), last.trim());
    if spec.contains(',') {
        return Part::Whole;
    }
    if first.is_empty() {
        // The last `n` bytes.
        return match last.parse::<u64>() {
            Ok(0) => Part::Unsatisfiable,
            Ok(_) if len == 0 => Part::Unsatisfiable,
            Ok(n) => Part::Bytes(len.saturating_sub(n), len - 1),
            Err(_) => Part::Whole,
        };
    }
    let Ok(first) = first.parse::<u64>() else {
        return Part::Whole;
    };
    let last = match last {
        "" => u64::MAX,
        l => match l.parse::<u64>() {
            Ok(l) if l >= first => l,
            _ => return Part::Whole,
        },
    };
    if first >= len {
        return Part::Unsatisfiable;
    }
    Part::Bytes(first, last.min(len - 1))
}

/// The response for an opened file: 304, 206, 416 or 200, streamed.
fn respond(headers: &HeaderMap, head: bool, file: File, name: &Path, meta: &Metadata) -> Response {
    let len = meta.len();
    let valid = Validators::of(meta);
    let mut out = HeaderMap::new();
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    out.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    out.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(etag) = HeaderValue::from_str(&valid.etag) {
        out.insert(ETAG, etag);
    }
    if let Some(lm) = valid
        .last_modified()
        .and_then(|v| HeaderValue::from_str(&v).ok())
    {
        out.insert(LAST_MODIFIED, lm);
    }
    if valid.not_modified(headers) {
        out.remove(CONTENT_TYPE);
        return (StatusCode::NOT_MODIFIED, out).into_response();
    }
    let range = headers.get(RANGE).and_then(|r| r.to_str().ok());
    let wanted = if valid.same(headers) {
        part(range, len)
    } else {
        Part::Whole
    };
    let (status, first, count) = match wanted {
        Part::Whole => (StatusCode::OK, 0, len),
        Part::Bytes(first, last) => {
            let range = format!("bytes {first}-{last}/{len}");
            out.insert(CONTENT_RANGE, HeaderValue::from_str(&range).expect("ASCII"));
            (StatusCode::PARTIAL_CONTENT, first, last - first + 1)
        }
        Part::Unsatisfiable => {
            let range = format!("bytes */{len}");
            out.insert(CONTENT_RANGE, HeaderValue::from_str(&range).expect("ASCII"));
            return (StatusCode::RANGE_NOT_SATISFIABLE, out).into_response();
        }
    };
    out.insert(CONTENT_LENGTH, HeaderValue::from(count));
    if head {
        return (status, out).into_response();
    }
    let body = match stream(file, first, count) {
        Ok(body) => body,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    (status, out, body).into_response()
}

/// `count` bytes of `file` from `first`, read a chunk at a time as the
/// client takes them, so that memory does not grow with the file's size.
fn stream(mut file: File, first: u64, count: u64) -> std::io::Result<Body> {
    use std::io::{Seek, SeekFrom};
    use tokio::io::AsyncReadExt;
    if first > 0 {
        file.seek(SeekFrom::Start(first))?;
    }
    let file = tokio::fs::File::from_std(file).take(count);
    Ok(Body::from_stream(
        tokio_util::io::ReaderStream::with_capacity(file, CHUNK),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Found, Part, Roots, part, windows_unsafe};
    #[cfg(unix)]
    use std::path::PathBuf;

    #[test]
    fn windows_names() {
        for s in [
            "CON",
            "con",
            "Nul",
            "aux.txt",
            "PRN.a.b",
            "nul .txt",
            "COM1",
            "com9.bob",
            "LPT5",
            "COM\u{b9}",
            "lpt\u{b3}.x",
            "a.bob.",
            "a.bob ",
            "..bob.",
            " ",
        ] {
            assert!(windows_unsafe(s), "{s:?}");
        }
        for s in [
            "a.bob",
            "CONSOLE",
            "con1",
            "COM0",
            "COM10",
            "LPT",
            "nul_x",
            "xCON",
            ".nul",
            "COM\u{b4}",
        ] {
            assert!(!windows_unsafe(s), "{s:?}");
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(part(None, 10), Part::Whole);
        assert_eq!(part(Some("bytes=0-3"), 10), Part::Bytes(0, 3));
        assert_eq!(part(Some("bytes=5-"), 10), Part::Bytes(5, 9));
        assert_eq!(part(Some("bytes=5-100"), 10), Part::Bytes(5, 9));
        assert_eq!(part(Some("bytes=-3"), 10), Part::Bytes(7, 9));
        assert_eq!(part(Some("bytes=-30"), 10), Part::Bytes(0, 9));
        assert_eq!(part(Some("bytes=10-"), 10), Part::Unsatisfiable);
        assert_eq!(part(Some("bytes=-0"), 10), Part::Unsatisfiable);
        assert_eq!(part(Some("bytes=-3"), 0), Part::Unsatisfiable);
        // Served whole: several ranges, other units, nonsense.
        assert_eq!(part(Some("bytes=0-1,4-5"), 10), Part::Whole);
        assert_eq!(part(Some("items=0-1"), 10), Part::Whole);
        assert_eq!(part(Some("bytes=3-1"), 10), Part::Whole);
        assert_eq!(part(Some("bytes=x-"), 10), Part::Whole);
    }

    #[test]
    fn drives_and_streams_are_missing() {
        let base = std::env::temp_dir().join(format!("bobview-drives-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.bob"), "a").unwrap();
        let roots = Roots::new(std::slice::from_ref(&base));
        assert!(matches!(roots.open("/a.bob"), Found::File(..)));
        for path in [
            "/a.bob:s",
            "/a.bob::$DATA",
            "/C:/Windows/win.ini",
            "/%5C%5Chost/x",
        ] {
            assert!(matches!(roots.open(path), Found::Missing), "{path}");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_root_are_refused() {
        let base = std::env::temp_dir().join(format!("bobview-confine-{}", std::process::id()));
        let opi = base.join("opi");
        let web = base.join("web");
        let outside = base.join("outside");
        for d in [&opi, &web, &outside, &opi.join("sub")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(opi.join("a.bob"), "a").unwrap();
        std::fs::write(opi.join("sub/b.bob"), "b").unwrap();
        std::fs::write(web.join("index.html"), "w").unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        let link = |target: &PathBuf, name: &str| {
            std::os::unix::fs::symlink(target, opi.join(name)).unwrap();
        };
        link(&outside.join("secret"), "secret.bob");
        link(&outside, "out");
        // Links within the root are fine.
        link(&opi.join("sub"), "inside");

        let roots = Roots::new(&[opi.clone(), web.clone()]);
        let read = |path: &str| match roots.open(path) {
            Found::File(mut f, ..) => {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut f, &mut text).unwrap();
                text
            }
            Found::Redirect(to) => format!("-> {to}"),
            Found::Missing => "missing".into(),
            Found::Refused => "refused".into(),
        };
        assert_eq!(read("/a.bob"), "a");
        assert_eq!(read("/sub/b.bob"), "b");
        assert_eq!(read("/inside/b.bob"), "b");
        assert_eq!(read("/%73ub/b.bob"), "b");
        assert_eq!(read("/index.html"), "w");
        // The web build's index, as opi has none.
        assert_eq!(read("/"), "w");
        assert_eq!(read("/sub"), "-> /sub/");
        assert_eq!(read("/missing.bob"), "missing");
        assert_eq!(read("/../opi/a.bob"), "missing");
        assert_eq!(read("/secret.bob"), "refused");
        assert_eq!(read("/out/secret"), "refused");
        assert_eq!(read("/%73ecret.bob"), "refused");
        assert_eq!(read("/out%2Fsecret"), "refused");
        // A FIFO, here or out of the root, is not opened: that would block.
        let fifo = |p: &PathBuf| {
            let ok = std::process::Command::new("mkfifo")
                .arg(p)
                .status()
                .unwrap();
            assert!(ok.success());
        };
        fifo(&opi.join("f.bob"));
        fifo(&outside.join("f"));
        link(&outside.join("f"), "outf.bob");
        assert_eq!(read("/f.bob"), "missing");
        assert_eq!(read("/outf.bob"), "missing");
        // A directory out is refused before it would be redirected.
        assert_eq!(read("/out"), "refused");
        assert_eq!(read("//sub"), "-> /sub/");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[tokio::test]
    async fn files_are_revalidated_and_served_in_parts() {
        use axum::http::header::*;
        use axum::http::{HeaderMap, HeaderValue, StatusCode};
        let dir = std::env::temp_dir().join(format!("bobview-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wasm");
        std::fs::write(&path, "0123456789").unwrap();
        let get = async |head: bool, pairs: &[(HeaderName, String)]| {
            let mut headers = HeaderMap::new();
            for (k, v) in pairs {
                headers.insert(k, HeaderValue::from_str(v).unwrap());
            }
            let file = std::fs::File::open(&path).unwrap();
            let meta = file.metadata().unwrap();
            let r = super::respond(&headers, head, file, &path, &meta);
            let (parts, body) = r.into_parts();
            let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
            let text = String::from_utf8(body.to_vec()).unwrap();
            (parts.status, parts.headers, text)
        };
        let (status, h, body) = get(false, &[]).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "0123456789"));
        assert_eq!(h[CONTENT_TYPE], "application/wasm");
        assert_eq!(h[CONTENT_LENGTH], "10");
        assert_eq!(h[ACCEPT_RANGES], "bytes");
        let etag = h[ETAG].to_str().unwrap().to_string();
        let modified = h[LAST_MODIFIED].to_str().unwrap().to_string();

        let (status, _, body) = get(false, &[(IF_MODIFIED_SINCE, modified.clone())]).await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_MODIFIED, ""));
        let (status, _, _) = get(false, &[(IF_NONE_MATCH, etag.clone())]).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        let (status, _, _) = get(false, &[(IF_NONE_MATCH, format!("W/{etag}"))]).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        // A different tag wins over a date that would match.
        let other = [
            (IF_NONE_MATCH, "\"other\"".to_string()),
            (IF_MODIFIED_SINCE, modified.clone()),
        ];
        assert_eq!(get(false, &other).await.0, StatusCode::OK);
        let old = "Thu, 01 Jan 1970 00:00:00 GMT".to_string();
        assert_eq!(
            get(false, &[(IF_MODIFIED_SINCE, old)]).await.0,
            StatusCode::OK
        );

        let (status, h, body) = get(false, &[(RANGE, "bytes=2-4".into())]).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::PARTIAL_CONTENT, "234")
        );
        assert_eq!(h[CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(h[CONTENT_LENGTH], "3");
        let (_, _, body) = get(false, &[(RANGE, "bytes=-2".into())]).await;
        assert_eq!(body, "89");
        let (status, h, _) = get(false, &[(RANGE, "bytes=20-".into())]).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(h[CONTENT_RANGE], "bytes */10");
        // If-Range: the range only if the file is the one named.
        let same = [(RANGE, "bytes=0-0".into()), (IF_RANGE, etag.clone())];
        assert_eq!(get(false, &same).await.2, "0");
        let same = [(RANGE, "bytes=0-0".into()), (IF_RANGE, modified.clone())];
        assert_eq!(get(false, &same).await.2, "0");
        let changed = [(RANGE, "bytes=0-0".into()), (IF_RANGE, "\"old\"".into())];
        assert_eq!(get(false, &changed).await.2, "0123456789");

        // HEAD: the headers, with no body read.
        let (status, h, body) = get(true, &[]).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
        assert_eq!(h[CONTENT_LENGTH], "10");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
