//! Reading displays from files or over HTTP(S).
//!
//! Fetched displays are kept in memory, so going back and forward or
//! reopening a screen doesn't fetch it again; [`forget`] clears them for a
//! reload. Fetching blocks, so the app builds a fetched display on a thread.
//!
//! A browser cannot block, nor read files: there [`load`] only reads the
//! cache, starting a fetch for what is missing and failing with [`LOADING`]
//! until it arrives, and the app builds the display again once nothing is
//! [`pending`].
//!
//! Natively, files that a display served from a public address names are
//! never fetched from a private one (loopback, a private network, link-local
//! and the like), so a display from the web cannot make bobview reach into
//! the network it runs on; nor does a redirect lead from a public address to
//! a private one, or from https to http. Files are found through
//! [`resolve`], which keeps track of who named them. A browser applies its
//! own rules (CORS, mixed content) instead.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use bob::{Display, Error, Source};
use parking_lot::Mutex;

/// Largest display file we will read.
const MAX_SIZE: u64 = 16 << 20;

/// A fetched display: the URL it was finally served from (after any
/// redirects, for resolving the files it names) and its text.
#[derive(Clone)]
struct Fetched {
    url: String,
    text: Arc<str>,
}

static CACHE: LazyLock<Mutex<HashMap<String, Fetched>>> = LazyLock::new(Default::default);

/// Bumped by [`forget`], so a fetch started before a reload cannot put
/// what it read back into the cache after it.
#[cfg(not(target_arch = "wasm32"))]
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Read and parse the display at `source`.
pub fn load(source: &Source) -> Result<Display, Error> {
    match source {
        #[cfg(not(target_arch = "wasm32"))]
        Source::File(path) => match native::bob_instead(path) {
            Some(bob) => {
                log::info!("{}: using {}", path.display(), bob.display());
                Display::load(&bob)
            }
            None => Display::load(path),
        },
        #[cfg(target_arch = "wasm32")]
        Source::File(path) => Err(no_files(path)),
        Source::Url(url) => {
            let fetched = get(url)?;
            let mut display = Display::parse(&fetched.text)?;
            display.source = Some(Source::Url(fetched.url));
            Ok(display)
        }
    }
}

/// Drop every cached display, so the next load fetches it again. Which
/// files displays from public addresses named is kept: fetches already
/// queued are still checked, and a reload does not lift the check.
pub fn forget() {
    #[cfg(not(target_arch = "wasm32"))]
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    CACHE.lock().clear();
    #[cfg(target_arch = "wasm32")]
    web::FAILED.lock().clear();
}

/// The file or URL `text` that the user named (on the command line, in the
/// Open panel, or in a saved layout). The user's own choice is fetched
/// wherever it is, whether a display from a public address named it before
/// or names it later.
pub fn user_named(text: &str) -> Source {
    let source = Source::parse(text);
    #[cfg(not(target_arch = "wasm32"))]
    if let Source::Url(url) = &source {
        native::user_named(url);
    }
    source
}

/// Whether `source` was refused because a display from a public address
/// named it, so it is not one to keep (in a layout, say) as the user's.
pub fn refused(source: &Source) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if let Source::Url(url) = source {
        return native::refused(url);
    }
    let _ = source;
    false
}

/// The file `file` named by the display at `base` (with no display, a plain
/// [`Source::parse`]): relative names are found next to the display. One named by a
/// display from a public address is fetched only from a public one.
pub fn resolve(base: Option<&Source>, file: &str) -> Source {
    let Some(base) = base else {
        return Source::parse(file);
    };
    let source = base.resolve(file);
    #[cfg(not(target_arch = "wasm32"))]
    if let (Source::Url(base), Source::Url(url)) = (base, &source) {
        native::named_by(base, url);
    }
    source
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::bytes;
#[cfg(not(target_arch = "wasm32"))]
use native::get;
#[cfg(target_arch = "wasm32")]
use web::get;
#[cfg(target_arch = "wasm32")]
pub use web::{LOADING, bytes_async, pending, same_origin_only};

#[cfg(target_arch = "wasm32")]
fn no_files(path: &std::path::Path) -> Error {
    Error::Io(
        path.into(),
        std::io::Error::other("a browser cannot read files"),
    )
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashSet;
    use std::io::Read;
    use std::net::{IpAddr, ToSocketAddrs};
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;
    use std::time::{Duration, Instant};

    use bob::{Error, Source};
    use parking_lot::Mutex;
    use ureq::Body;
    use ureq::http::Response;

    use super::{CACHE, Fetched, GENERATION, MAX_SIZE};

    /// Most redirects followed for one file.
    const MAX_REDIRECTS: usize = 10;

    /// Longest one fetch may take, all its redirects included.
    const FETCH_TIME: Duration = Duration::from_secs(10);

    /// Redirects are followed here, not by ureq, so that each is checked.
    static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .user_agent(concat!("bobview/", env!("CARGO_PKG_VERSION")))
            .max_redirects(0)
            .build()
            .into()
    });

    /// URLs (as asked for, and where they were served from) of displays
    /// served from a public address.
    static PUBLIC: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// URLs that displays in [`PUBLIC`] name: fetched only from a public
    /// address, whoever asks for them, until the user names one.
    static FROM_PUBLIC: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// Addresses that tests give hosts on 127.0.0.1, by port, in place of
    /// looking them up.
    #[cfg(test)]
    pub(super) static PRETEND: LazyLock<Mutex<std::collections::HashMap<u16, Vec<IpAddr>>>> =
        LazyLock::new(Default::default);

    /// URLs the user named: fetched wherever they are, whoever names them.
    static USER: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// URLs in [`FROM_PUBLIC`] refused for being on a private address.
    static REFUSED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// Note that the display at `base` names `url`.
    pub(super) fn named_by(base: &str, url: &str) {
        if PUBLIC.lock().contains(base) && !USER.lock().contains(url) {
            let mut from_public = FROM_PUBLIC.lock();
            if !from_public.contains(url) {
                from_public.insert(url.to_string());
            }
        }
    }

    /// Note that the user named `url`, so it is fetched wherever it is.
    pub(super) fn user_named(url: &str) {
        USER.lock().insert(url.to_string());
        FROM_PUBLIC.lock().remove(url);
        REFUSED.lock().remove(url);
    }

    /// Whether `url` was refused for a display from a public address.
    pub(super) fn refused(url: &str) -> bool {
        REFUSED.lock().contains(url)
    }

    /// GET `url`, following redirects, but not from https to http, nor from
    /// a public address to a private one, nor to a private one at all for
    /// a file a display from a public address named. Returns where it was
    /// finally served from, and the response.
    fn call(url: &str) -> Result<(String, Response<Body>), String> {
        call_within(url, FETCH_TIME)
    }

    /// [`call`], all of it (redirects, and reading the body) within `time`.
    pub(super) fn call_within(
        url: &str,
        time: Duration,
    ) -> Result<(String, Response<Body>), String> {
        let deadline = Instant::now() + time;
        // Why a private address is refused, if it is.
        let mut refuse = FROM_PUBLIC
            .lock()
            .contains(url)
            .then_some("refused: a display from a public address named a private one");
        let named = url;
        let mut url = url.to_string();
        for hop in 0..=MAX_REDIRECTS {
            let addrs = addresses(&url);
            if let Some(why) = refuse
                && target_private(&addrs)
            {
                if hop == 0 {
                    REFUSED.lock().insert(named.to_string());
                }
                return Err(format!("{why} ({url})"));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timed out after {}s", time.as_secs_f32()));
            }
            let resp = AGENT
                .get(&url)
                .config()
                .timeout_global(Some(left))
                .build()
                .call()
                .map_err(describe)?;
            let status = resp.status();
            if !status.is_redirection() {
                return Ok((url, resp));
            }
            let Some(location) = resp
                .headers()
                .get(ureq::http::header::LOCATION)
                .and_then(|l| l.to_str().ok())
            else {
                return Err(format!("HTTP {}", status.as_u16()));
            };
            if let Some(scheme) = scheme(location)
                && !scheme.eq_ignore_ascii_case("http")
                && !scheme.eq_ignore_ascii_case("https")
            {
                return Err(format!("refused a redirect to {location}"));
            }
            let next = Source::Url(url.clone()).resolve(location).to_string();
            if downgrade(&url, &next) {
                return Err(format!("refused a redirect from https to {next}"));
            }
            log::info!("{url} redirects to {next}");
            if !origin_private(&addrs) {
                refuse = Some("refused a redirect from a public address to a private one");
            }
            url = next;
        }
        Err("too many redirects".into())
    }

    /// The scheme of `url`, if it has one (else it is relative).
    fn scheme(url: &str) -> Option<&str> {
        let (scheme, _) = url.split_once(':')?;
        let mut chars = scheme.chars();
        let first = chars.next()?;
        (first.is_ascii_alphabetic()
            && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
        .then_some(scheme)
    }

    /// Whether a redirect from `from` to `to` leaves https.
    pub(super) fn downgrade(from: &str, to: &str) -> bool {
        let https = |u: &str| {
            u.get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        };
        https(from) && !https(to)
    }

    /// The addresses of the host of `url`, from the same lookup ureq
    /// connects with; none if it cannot be looked up here (a proxy may reach
    /// it).
    pub(super) fn addresses(url: &str) -> Vec<IpAddr> {
        let Ok(uri) = url.parse::<ureq::http::Uri>() else {
            return Vec::new();
        };
        let Some(host) = uri.host() else {
            return Vec::new();
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let https = uri
            .scheme_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("https"));
        let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
        #[cfg(test)]
        if host == "127.0.0.1"
            && let Some(addrs) = PRETEND.lock().get(&port)
        {
            return addrs.clone();
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => (host, port)
                .to_socket_addrs()
                .map(|addrs| addrs.map(|a| a.ip()).collect())
                .unwrap_or_default(),
        }
    }

    /// Whether a display served from a host with addresses `addrs` is on a
    /// private network: only if all of them are private ([`private_ip`]),
    /// since the display may have come from any of them.
    pub(super) fn origin_private(addrs: &[IpAddr]) -> bool {
        !addrs.is_empty() && addrs.iter().all(|&ip| private_ip(ip))
    }

    /// Whether a fetch from a host with addresses `addrs` could reach a
    /// private network: if any of them is private ([`private_ip`]).
    pub(super) fn target_private(addrs: &[IpAddr]) -> bool {
        addrs.iter().any(|&ip| private_ip(ip))
    }

    /// Whether `ip` is not a public internet address: loopback, private
    /// networks, link-local (cloud metadata), carrier-grade NAT,
    /// unspecified, broadcast, IPv6 unique-local and site-local, and IPv4
    /// in IPv6 (mapped, compatible, translated, NAT64 and 6to4), by the
    /// IPv4 address within.
    pub(super) fn private_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let [a, b, c, _] = v4.octets();
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || a == 0
                    || (a == 100 && (64..128).contains(&b))
                    || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
                    || (a == 198 && b & 0xfe == 18) // benchmarking, used inside sites
                    || a >= 240 // reserved
            }
            IpAddr::V6(v6) => {
                let s = v6.segments();
                let v4 = |hi: u16, lo: u16| {
                    private_ip(IpAddr::V4(((u32::from(hi) << 16) | u32::from(lo)).into()))
                };
                match s {
                    // IPv4-mapped ::ffff:a.b.c.d, and SIIT ::ffff:0:a.b.c.d.
                    [0, 0, 0, 0, 0xffff, 0, hi, lo] | [0, 0, 0, 0, 0, 0xffff, hi, lo] => v4(hi, lo),
                    // IPv4-compatible ::a.b.c.d (deprecated), :: and ::1.
                    [0, 0, 0, 0, 0, 0, _, _] => true,
                    // NAT64 64:ff9b::a.b.c.d; local-use NAT64 64:ff9b:1::/48
                    // may embed any address anywhere.
                    [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => v4(hi, lo),
                    [0x64, 0xff9b, 1, ..] => true,
                    // 6to4 2002:a.b.c.d::/48.
                    [0x2002, hi, lo, ..] => v4(hi, lo),
                    [first, ..] => {
                        first & 0xfe00 == 0xfc00 // unique-local
                            || first & 0xffc0 == 0xfe80 // link-local
                            || first & 0xffc0 == 0xfec0 // site-local
                    }
                }
            }
        }
    }

    /// Read the file at `source` as bytes, such as an image a display names.
    /// Nothing is cached here: the caller keeps what it decodes.
    pub fn bytes(source: &Source) -> Result<Vec<u8>, Error> {
        match source {
            Source::File(path) => {
                let mut data = Vec::new();
                std::fs::File::open(bob::project::locate(path))
                    .and_then(|f| f.take(MAX_SIZE + 1).read_to_end(&mut data))
                    .map_err(|e| Error::Io(path.clone(), e))?;
                if data.len() as u64 > MAX_SIZE {
                    let why = format!("larger than {} MiB", MAX_SIZE >> 20);
                    return Err(Error::Io(path.clone(), std::io::Error::other(why)));
                }
                Ok(data)
            }
            Source::Url(url) => {
                log::info!("fetching {url}");
                let fail = |why: String| Error::Fetch(url.to_string(), why);
                call(url)
                    .map_err(fail)?
                    .1
                    .body_mut()
                    .with_config()
                    .limit(MAX_SIZE)
                    .read_to_vec()
                    .map_err(|e| fail(describe(e)))
            }
        }
    }

    /// The `.bob` next to an `.opi` file, which Phoebus opens in its place.
    pub(super) fn bob_instead(path: &Path) -> Option<PathBuf> {
        if !path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("opi"))
        {
            return None;
        }
        let bob = path.with_extension("bob");
        bob::project::locate(&bob).exists().then_some(bob)
    }

    pub(super) fn get(url: &str) -> Result<Fetched, Error> {
        if let Some(fetched) = CACHE.lock().get(url).cloned() {
            return Ok(fetched);
        }
        let generation = GENERATION.load(std::sync::atomic::Ordering::SeqCst);
        log::info!("fetching {url}");
        let fail = |why: String| Error::Fetch(url.to_string(), why);
        let (served, mut resp) = call(url).map_err(fail)?;
        let text: std::sync::Arc<str> = resp
            .body_mut()
            .with_config()
            .limit(MAX_SIZE)
            .read_to_string()
            .map_err(|e| fail(describe(e)))?
            .into();
        // Kept across reloads, so files named by a display already shown
        // are still checked; updated as the display is fetched again.
        let private = origin_private(&addresses(&served));
        let mut public = PUBLIC.lock();
        for u in [url, &served] {
            if private {
                public.remove(u);
            } else {
                public.insert(u.to_string());
            }
        }
        drop(public);
        let fetched = Fetched { url: served, text };
        let mut cache = CACHE.lock();
        // Forgotten while it was being fetched: it may be stale, so it is
        // used this once but not kept.
        if GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
            cache.insert(url.to_string(), fetched.clone());
        }
        Ok(fetched)
    }

    fn describe(e: ureq::Error) -> String {
        match e {
            ureq::Error::StatusCode(code) => format!("HTTP {code}"),
            ureq::Error::Io(e) => e.to_string(),
            e => e.to_string(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::collections::{HashMap, HashSet};
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    use bob::{Error, Source};
    use parking_lot::Mutex;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    use super::{CACHE, Fetched, MAX_SIZE};

    /// Why [`super::load`] failed while the display is still on its way.
    pub const LOADING: &str = "loading";

    /// Displays being fetched, and those that could not be, with why.
    static PENDING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
    pub(super) static FAILED: LazyLock<Mutex<HashMap<String, String>>> =
        LazyLock::new(Default::default);

    /// Whether any display is still being fetched.
    pub fn pending() -> bool {
        !PENDING.lock().is_empty()
    }

    /// Load displays only from the page's own server: a display from
    /// another, opened by a link, could write PVs with the page's token.
    static SAME_ORIGIN: AtomicBool = AtomicBool::new(false);

    pub fn same_origin_only() {
        SAME_ORIGIN.store(true, Ordering::Relaxed);
    }

    /// Why `url` may not be loaded, if it may not.
    fn refused(url: &str) -> Option<String> {
        if !SAME_ORIGIN.load(Ordering::Relaxed) {
            return None;
        }
        let page = web_sys::window()?.location().href().ok()?;
        (!pv::same_origin(&page, url))
            .then(|| "not loading displays from other servers while writes are on".into())
    }

    pub(super) fn get(url: &str) -> Result<Fetched, Error> {
        if let Some(why) = refused(url) {
            return Err(Error::Fetch(url.to_string(), why));
        }
        if let Some(fetched) = CACHE.lock().get(url).cloned() {
            return Ok(fetched);
        }
        if let Some(why) = FAILED.lock().get(url) {
            return Err(Error::Fetch(url.to_string(), why.clone()));
        }
        if PENDING.lock().insert(url.to_string()) {
            let url = url.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                // A redirect may lead elsewhere.
                let result = fetch(&url)
                    .await
                    .and_then(|(served, data)| match refused(&served) {
                        Some(why) => Err(why),
                        None => Ok((served, data)),
                    });
                match result {
                    Ok((served, data)) => {
                        let text = String::from_utf8_lossy(&data).into();
                        let fetched = Fetched { url: served, text };
                        CACHE.lock().insert(url.clone(), fetched);
                    }
                    Err(why) => {
                        log::warn!("cannot fetch {url}: {why}");
                        FAILED.lock().insert(url.clone(), why);
                    }
                }
                PENDING.lock().remove(&url);
            });
        }
        Err(Error::Fetch(url.to_string(), LOADING.into()))
    }

    /// Read the file at `source` as bytes, such as an image a display names.
    pub async fn bytes_async(source: &Source) -> Result<Vec<u8>, Error> {
        match source {
            Source::File(path) => Err(super::no_files(path)),
            Source::Url(url) => fetch(url)
                .await
                .map(|(_, data)| data)
                .map_err(|why| Error::Fetch(url.clone(), why)),
        }
    }

    /// GET `url`, returning where it was served from and its body. The server
    /// must allow this page's origin (CORS) unless it is the page's own.
    async fn fetch(url: &str) -> Result<(String, Vec<u8>), String> {
        log::info!("fetching {url}");
        let js = |e: wasm_bindgen::JsValue| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
                .unwrap_or_else(|| format!("{e:?}"))
        };
        let window = web_sys::window().ok_or("no window")?;
        let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(js)?
            .unchecked_into();
        if !resp.ok() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let body = JsFuture::from(resp.array_buffer().map_err(js)?)
            .await
            .map_err(js)?;
        let data = js_sys::Uint8Array::new(&body);
        if data.length() as u64 > MAX_SIZE {
            return Err(format!("larger than {} MiB", MAX_SIZE >> 20));
        }
        Ok((resp.url(), data.to_vec()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    const DISPLAY: &str = r#"<display version="2.0.0"><name>Served</name></display>"#;

    /// Held by tests that rely on what the cache holds, or that clear it.
    pub(crate) static CACHE_USERS: Mutex<()> = Mutex::new(());

    /// Serve `DISPLAY` at /opi/a.bob and 404 elsewhere; returns the base URL
    /// and a count of requests handled.
    pub(crate) fn serve() -> (String, Arc<Mutex<usize>>) {
        serve_to("/opi/a.bob")
    }

    /// [`serve`], with /away redirecting to `away`, /file to a file, and
    /// /slow slowly to itself, while /stall answers after 3 s.
    fn serve_to(away: &str) -> (String, Arc<Mutex<usize>>) {
        let away = format!("302 Found\r\nLocation: {away}");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(Mutex::new(0));
        let count = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                *count.lock() += 1;
                let (status, body) = if first.starts_with("GET /opi/a.bob ") {
                    ("200 OK", DISPLAY)
                } else if first.starts_with("GET /old/a.bob ") {
                    ("301 Moved Permanently\r\nLocation: /opi/a.bob", "")
                } else if first.starts_with("GET /away ") {
                    (away.as_str(), "")
                } else if first.starts_with("GET /file ") {
                    ("302 Found\r\nLocation: file:///etc/passwd", "")
                } else if first.starts_with("GET /slow ") {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    ("302 Found\r\nLocation: /slow", "")
                } else if first.starts_with("GET /stall ") {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    ("200 OK", DISPLAY)
                } else {
                    ("404 Not Found", "")
                };
                let mut stream = stream;
                // The client may have given up.
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, hits)
    }

    #[test]
    fn fetches_caches_and_reports_errors() {
        let _cache = CACHE_USERS.lock();
        let (base, hits) = serve();
        let src = Source::parse(&format!("{base}/opi/a.bob"));
        load(&src).unwrap();
        let d = load(&src).unwrap();
        assert_eq!(d.name, "Served");
        assert_eq!(d.source.as_ref(), Some(&src));
        assert_eq!(*hits.lock(), 1, "second load came from the cache");

        let missing = src.resolve("../missing.bob");
        let err = load(&missing).unwrap_err().to_string();
        assert_eq!(err, format!("cannot fetch {base}/missing.bob: HTTP 404"));
    }

    #[test]
    fn names_resolve_from_where_a_redirect_lands() {
        let (base, _) = serve();
        let d = load(&Source::parse(&format!("{base}/old/a.bob"))).unwrap();
        assert_eq!(d.name, "Served");
        assert_eq!(d.resolve("b.bob"), Source::Url(format!("{base}/opi/b.bob")));
    }

    /// A public address (TEST-NET-3).
    const PUBLIC_IP: &str = "203.0.113.5";

    /// [`serve_to`], on a host with addresses `addrs`.
    fn serve_at(addrs: &[&str], away: &str) -> (String, Arc<Mutex<usize>>) {
        let (base, hits) = serve_to(away);
        let port = base.rsplit(':').next().unwrap().parse().unwrap();
        let addrs = addrs.iter().map(|a| a.parse().unwrap()).collect();
        native::PRETEND.lock().insert(port, addrs);
        (base, hits)
    }

    /// A server that counts as on a public address.
    pub(crate) fn serve_public(away: &str) -> (String, Arc<Mutex<usize>>) {
        serve_at(&[PUBLIC_IP], away)
    }

    #[test]
    fn a_public_display_cannot_name_a_private_address() {
        let (private, private_hits) = serve();
        let (public, _) = serve_public("/opi/a.bob");
        let d = load(&Source::parse(&format!("{public}/opi/a.bob"))).unwrap();
        // Its own files, by relative or absolute name, are fetched.
        let own = resolve(d.source.as_ref(), "a.bob");
        assert_eq!(own, Source::Url(format!("{public}/opi/a.bob")));
        load(&own).unwrap();
        // A private address it names is not, nor its images.
        let inside = resolve(d.source.as_ref(), &format!("{private}/opi/a.bob"));
        let err = load(&inside).unwrap_err().to_string();
        assert!(err.contains("refused"), "{err}");
        let err = bytes(&inside).unwrap_err().to_string();
        assert!(err.contains("refused"), "{err}");
        assert_eq!(*private_hits.lock(), 0);
        // A private URL named by a private display, or by the user, is
        // fetched.
        let (other, _) = serve();
        let named = format!("{private}/old/a.bob");
        let d = load(&Source::parse(&format!("{other}/opi/a.bob"))).unwrap();
        load(&resolve(d.source.as_ref(), &named)).unwrap();
        // The redirect, and where it leads.
        assert_eq!(*private_hits.lock(), 2);
        assert_eq!(user_named(&inside.to_string()), inside);
        load(&inside).unwrap();
        assert_eq!(*private_hits.lock(), 3);
    }

    #[test]
    fn a_host_with_a_public_address_is_not_private() {
        // Say a public host also gives a private address, which it is not
        // served from: its displays still cannot name a private one.
        let (private, private_hits) = serve();
        let (mixed, _) = serve_at(&["::1", PUBLIC_IP], "/opi/a.bob");
        let d = load(&Source::parse(&format!("{mixed}/opi/a.bob"))).unwrap();
        let inside = resolve(d.source.as_ref(), &format!("{private}/opi/a.bob"));
        let err = bytes(&inside).unwrap_err().to_string();
        assert!(err.contains("named a private one"), "{err}");
        assert_eq!(*private_hits.lock(), 0);
        // Nor can a public display name such a host.
        let (public, _) = serve_public("/opi/a.bob");
        let d = load(&Source::parse(&format!("{public}/opi/a.bob"))).unwrap();
        let err = bytes(&resolve(d.source.as_ref(), &format!("{mixed}/opi/a.bob")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("named a private one"), "{err}");

        use native::{origin_private, target_private};
        let ips = |a: &[&str]| a.iter().map(|a| a.parse().unwrap()).collect::<Vec<_>>();
        let mixed = ips(&["::1", PUBLIC_IP]);
        assert!(!origin_private(&mixed) && target_private(&mixed));
        let inside = ips(&["10.0.0.1", "::1"]);
        assert!(origin_private(&inside) && target_private(&inside));
        let outside = ips(&[PUBLIC_IP, "2001:db8::1"]);
        assert!(!origin_private(&outside) && !target_private(&outside));
        // Not looked up here: left to a proxy, if any.
        assert!(!origin_private(&[]) && !target_private(&[]));
    }

    #[test]
    fn a_reload_still_refuses_what_a_public_display_named() {
        let (private, private_hits) = serve();
        let (public, _) = serve_public("/opi/a.bob");
        let d = load(&Source::parse(&format!("{public}/opi/a.bob"))).unwrap();
        let image = resolve(d.source.as_ref(), &format!("{private}/a.png"));
        assert!(bytes(&image).is_err());
        // An image queued before the reload is fetched after it.
        let _cache = CACHE_USERS.lock();
        forget();
        let err = bytes(&image).unwrap_err().to_string();
        assert!(err.contains("named a private one"), "{err}");
        assert_eq!(*private_hits.lock(), 0);
    }

    #[test]
    fn a_url_the_user_named_stays_theirs() {
        let (private, private_hits) = serve();
        let (public, _) = serve_public("/opi/a.bob");
        let mine = user_named(&format!("{private}/opi/a.bob"));
        load(&mine).unwrap();
        // A public display (in another window, say) naming it later does
        // not stop it loading again, as on a reload.
        let d = load(&Source::parse(&format!("{public}/opi/a.bob"))).unwrap();
        assert_eq!(resolve(d.source.as_ref(), &mine.to_string()), mine);
        let _cache = CACHE_USERS.lock();
        forget();
        load(&mine).unwrap();
        assert_eq!(*private_hits.lock(), 2);
        assert!(!refused(&mine));
        // Nor is it marked as refused; what the display named is.
        let other = resolve(d.source.as_ref(), &format!("{private}/opi/b.bob"));
        assert!(load(&other).is_err() && refused(&other));
        assert_eq!(user_named(&other.to_string()), other);
        assert!(!refused(&other));
    }

    #[test]
    fn redirects_do_not_lead_from_public_to_private() {
        let (private, private_hits) = serve();
        let (public, _) = serve_public(&format!("{private}/opi/a.bob"));
        let err = load(&Source::parse(&format!("{public}/away")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refused a redirect from a public address to a private one"),
            "{err}"
        );
        assert_eq!(*private_hits.lock(), 0);
        // From one private server to another is fine.
        let (inside, _) = serve_to(&format!("{private}/opi/a.bob"));
        load(&Source::parse(&format!("{inside}/away"))).unwrap();
        assert_eq!(*private_hits.lock(), 1);
    }

    #[test]
    fn redirects_do_not_leave_https() {
        use native::downgrade;
        assert!(downgrade("https://h/a.bob", "http://h/a.bob"));
        assert!(downgrade("HTTPS://h/a.bob", "http://other/a.bob"));
        assert!(!downgrade("https://h/a.bob", "https://other/a.bob"));
        assert!(!downgrade("http://h/a.bob", "http://other/a.bob"));
        assert!(!downgrade("http://h/a.bob", "https://h/a.bob"));
    }

    #[test]
    fn redirects_lead_only_to_http() {
        let (base, _) = serve();
        let err = load(&Source::parse(&format!("{base}/file")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refused a redirect to file:///etc/passwd"),
            "{err}"
        );
    }

    #[test]
    fn redirects_share_one_time_limit() {
        // Each of up to eleven requests takes 0.3 s: all of them, 3.3 s.
        let (base, _) = serve();
        let start = std::time::Instant::now();
        let limit = std::time::Duration::from_secs(1);
        let Err(err) = native::call_within(&format!("{base}/slow"), limit) else {
            panic!("a fetch redirecting forever succeeded");
        };
        assert!(err.contains("time"), "{err}");
        assert!(start.elapsed() < limit * 2, "{:?}", start.elapsed());
        // The limit holds within a request, too.
        let (base, _) = serve();
        let start = std::time::Instant::now();
        let Err(err) = native::call_within(&format!("{base}/stall"), limit) else {
            panic!("a stalled fetch succeeded");
        };
        assert!(err.contains("time"), "{err}");
        assert!(start.elapsed() < limit * 2, "{:?}", start.elapsed());
    }

    #[test]
    fn private_addresses() {
        use native::{addresses, private_ip, target_private};
        let is_private = |url: &str| target_private(&addresses(url));
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "::ffff:0:7f00:1",
            "64:ff9b::a00:1",
            "64:ff9b:1::8.8.8.8",
            "2002:a00:1::",
            "2002:7f00:1::1",
            "fec0::1",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "::ffff:198.18.0.1",
        ] {
            assert!(private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2001:db8::1",
            "::ffff:1.1.1.1",
            "::ffff:0:808:808",
            "64:ff9b::808:808",
            "2002:808:808::1",
            "2606:4700::1111",
            "192.0.1.1",
            "198.17.255.255",
            "198.20.0.1",
        ] {
            assert!(!private_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_private("http://localhost:8080/a.bob"));
        assert!(is_private("http://[::1]/a.bob"));
        assert!(is_private("https://192.168.0.1/a.bob"));
        assert!(!is_private("http://1.1.1.1/a.bob"));
        // Not looked up here: left to a proxy, if any.
        assert!(!is_private("http://no-such-host.invalid/a.bob"));
    }

    #[test]
    fn connection_refused_is_an_error() {
        // Bind then drop, so nothing is listening on the port.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = load(&Source::parse(&format!("http://127.0.0.1:{port}/x.bob"))).unwrap_err();
        assert!(matches!(err, Error::Fetch(..)), "{err}");
    }

    #[test]
    fn a_bob_instead_of_a_launcher_screen_is_real() {
        let dir = std::env::temp_dir().join(format!("bobview-fetch-runcss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("runcss.sh"),
            "project=m_${version}\nmodule=m\nlinks=\"${SCRIPT_DIR}=${project}/${module}\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("x.opi"), "<display/>").unwrap();
        std::fs::write(dir.join("x.bob"), r#"<display version="2.0.0"/>"#).unwrap();
        let virt = bob::project::launcher_path(&dir.join("x.opi")).unwrap();
        let d = load(&Source::File(virt)).unwrap();
        assert_eq!(d.source, Some(Source::File(dir.join("x.bob"))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_fetch_finishing_after_a_reload_is_not_cached() {
        let _cache = CACHE_USERS.lock();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/a.bob", listener.local_addr().unwrap());
        let (got, request) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                got.send(()).unwrap();
                held.recv().unwrap();
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DISPLAY}",
                    DISPLAY.len()
                )
                .unwrap();
            }
        });
        let src = Source::parse(&url);
        let loading = std::thread::spawn({
            let src = src.clone();
            move || load(&src).map(|d| d.name)
        });
        // A reload while the display is on its way.
        request.recv().unwrap();
        forget();
        release.send(()).unwrap();
        assert_eq!(loading.join().unwrap().unwrap(), "Served");
        assert!(!CACHE.lock().contains_key(&url), "stale text cached");
    }
}
