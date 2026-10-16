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
use std::sync::{Arc, LazyLock, Mutex};

use bob::{Display, Error, Source};

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

/// Drop every cached display, so the next load fetches it again.
pub fn forget() {
    CACHE.lock().unwrap().clear();
    #[cfg(target_arch = "wasm32")]
    web::FAILED.lock().unwrap().clear();
    #[cfg(not(target_arch = "wasm32"))]
    native::forget();
}

/// The file `file` named by the display at `base` (or by the user, with no
/// display): relative names are found next to the display. One named by a
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
pub use web::{LOADING, bytes_async, pending};

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
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    use bob::{Error, Source};
    use ureq::Body;
    use ureq::http::Response;

    use super::{CACHE, Fetched, MAX_SIZE};

    /// Most redirects followed for one file.
    const MAX_REDIRECTS: usize = 10;

    /// Redirects are followed here, not by ureq, so that each is checked.
    static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent(concat!("bobview/", env!("CARGO_PKG_VERSION")))
            .max_redirects(0)
            .build()
            .into()
    });

    /// URLs (as asked for, and where they were served from) of displays
    /// served from a public address.
    static PUBLIC: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// URLs that displays in [`PUBLIC`] name: fetched only from a public
    /// address, whoever asks for them, until [`forget`].
    static FROM_PUBLIC: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

    /// Ports on 127.0.0.1 that tests treat as a public address.
    #[cfg(test)]
    pub(super) static PRETEND_PUBLIC: LazyLock<Mutex<HashSet<u16>>> =
        LazyLock::new(Default::default);

    /// Note that the display at `base` names `url`.
    pub(super) fn named_by(base: &str, url: &str) {
        if PUBLIC.lock().unwrap().contains(base) {
            let mut from_public = FROM_PUBLIC.lock().unwrap();
            if !from_public.contains(url) {
                from_public.insert(url.to_string());
            }
        }
    }

    /// Forget which displays came from public addresses.
    pub(super) fn forget() {
        PUBLIC.lock().unwrap().clear();
        FROM_PUBLIC.lock().unwrap().clear();
    }

    /// GET `url`, following redirects, but not from https to http, nor from
    /// a public address to a private one, nor to a private one at all for
    /// a file a display from a public address named. Returns where it was
    /// finally served from, and the response.
    fn call(url: &str) -> Result<(String, Response<Body>), String> {
        let mut public_only = FROM_PUBLIC.lock().unwrap().contains(url);
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            if public_only && is_private(&url) {
                return Err("refused: a display from a public address named a private one".into());
            }
            let resp = AGENT.get(&url).call().map_err(describe)?;
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
            let next = Source::Url(url.clone()).resolve(location).to_string();
            if downgrade(&url, &next) {
                return Err(format!("refused a redirect from https to {next}"));
            }
            log::info!("{url} redirects to {next}");
            public_only = public_only || !is_private(&url);
            url = next;
        }
        Err("too many redirects".into())
    }

    /// Whether a redirect from `from` to `to` leaves https.
    pub(super) fn downgrade(from: &str, to: &str) -> bool {
        let https = |u: &str| {
            u.get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        };
        https(from) && !https(to)
    }

    /// Whether the host of `url` has a private address (see
    /// [`private_ip`]). A host that cannot be looked up here is not (a
    /// proxy may reach it).
    pub(super) fn is_private(url: &str) -> bool {
        let Ok(uri) = url.parse::<ureq::http::Uri>() else {
            return false;
        };
        let Some(host) = uri.host() else {
            return false;
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let https = uri
            .scheme_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("https"));
        let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
        #[cfg(test)]
        if host == "127.0.0.1" && PRETEND_PUBLIC.lock().unwrap().contains(&port) {
            return false;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => private_ip(ip),
            Err(_) => (host, port)
                .to_socket_addrs()
                .is_ok_and(|mut addrs| addrs.any(|a| private_ip(a.ip()))),
        }
    }

    /// Whether `ip` is not a public internet address: loopback, private
    /// networks, link-local (cloud metadata), carrier-grade NAT,
    /// unspecified, broadcast, IPv6 unique-local, and IPv4 in IPv6.
    pub(super) fn private_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let [a, b, ..] = v4.octets();
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || a == 0
                    || (a == 100 && (64..128).contains(&b))
            }
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => private_ip(IpAddr::V4(v4)),
                None => {
                    let first = v6.segments()[0];
                    v6.is_loopback()
                        || v6.is_unspecified()
                        || first & 0xfe00 == 0xfc00
                        || first & 0xffc0 == 0xfe80
                }
            },
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
        if let Some(fetched) = CACHE.lock().unwrap().get(url).cloned() {
            return Ok(fetched);
        }
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
        if !is_private(&served) {
            let mut public = PUBLIC.lock().unwrap();
            public.insert(url.to_string());
            public.insert(served.clone());
        }
        let fetched = Fetched { url: served, text };
        CACHE
            .lock()
            .unwrap()
            .insert(url.to_string(), fetched.clone());
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
    use std::sync::{LazyLock, Mutex};

    use bob::{Error, Source};
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
        !PENDING.lock().unwrap().is_empty()
    }

    pub(super) fn get(url: &str) -> Result<Fetched, Error> {
        if let Some(fetched) = CACHE.lock().unwrap().get(url).cloned() {
            return Ok(fetched);
        }
        if let Some(why) = FAILED.lock().unwrap().get(url) {
            return Err(Error::Fetch(url.to_string(), why.clone()));
        }
        if PENDING.lock().unwrap().insert(url.to_string()) {
            let url = url.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch(&url).await {
                    Ok((served, data)) => {
                        let text = String::from_utf8_lossy(&data).into();
                        let fetched = Fetched { url: served, text };
                        CACHE.lock().unwrap().insert(url.clone(), fetched);
                    }
                    Err(why) => {
                        log::warn!("cannot fetch {url}: {why}");
                        FAILED.lock().unwrap().insert(url.clone(), why);
                    }
                }
                PENDING.lock().unwrap().remove(&url);
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
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    const DISPLAY: &str = r#"<display version="2.0.0"><name>Served</name></display>"#;

    /// Serve `DISPLAY` at /opi/a.bob and 404 elsewhere; returns the base URL
    /// and a count of requests handled.
    fn serve() -> (String, Arc<Mutex<usize>>) {
        serve_to("/opi/a.bob")
    }

    /// [`serve`], with /away redirecting to `away`.
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
                *count.lock().unwrap() += 1;
                let (status, body) = if first.starts_with("GET /opi/a.bob ") {
                    ("200 OK", DISPLAY)
                } else if first.starts_with("GET /old/a.bob ") {
                    ("301 Moved Permanently\r\nLocation: /opi/a.bob", "")
                } else if first.starts_with("GET /away ") {
                    (away.as_str(), "")
                } else {
                    ("404 Not Found", "")
                };
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (base, hits)
    }

    #[test]
    fn fetches_caches_and_reports_errors() {
        let (base, hits) = serve();
        let src = Source::parse(&format!("{base}/opi/a.bob"));
        load(&src).unwrap();
        let d = load(&src).unwrap();
        assert_eq!(d.name, "Served");
        assert_eq!(d.source.as_ref(), Some(&src));
        assert_eq!(*hits.lock().unwrap(), 1, "second load came from the cache");

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

    /// A server that counts as on a public address.
    fn serve_public(away: &str) -> (String, Arc<Mutex<usize>>) {
        let (base, hits) = serve_to(away);
        let port = base.rsplit(':').next().unwrap().parse().unwrap();
        native::PRETEND_PUBLIC.lock().unwrap().insert(port);
        (base, hits)
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
        assert_eq!(*private_hits.lock().unwrap(), 0);
        // A private URL named by the user, or by a private display, is
        // fetched. (Once a public display has named a URL, it stays refused
        // until a reload.)
        let (other, _) = serve();
        let named = Source::parse(&format!("{private}/old/a.bob"));
        let d = load(&Source::parse(&format!("{other}/opi/a.bob"))).unwrap();
        assert_eq!(resolve(None, &named.to_string()), named);
        load(&resolve(d.source.as_ref(), &named.to_string())).unwrap();
        // The redirect, and where it leads.
        assert_eq!(*private_hits.lock().unwrap(), 2);
    }

    #[test]
    fn redirects_do_not_lead_from_public_to_private() {
        let (private, private_hits) = serve();
        let (public, _) = serve_public(&format!("{private}/opi/a.bob"));
        let err = load(&Source::parse(&format!("{public}/away")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("refused"), "{err}");
        assert_eq!(*private_hits.lock().unwrap(), 0);
        // From one private server to another is fine.
        let (inside, _) = serve_to(&format!("{private}/opi/a.bob"));
        load(&Source::parse(&format!("{inside}/away"))).unwrap();
        assert_eq!(*private_hits.lock().unwrap(), 1);
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
    fn private_addresses() {
        use native::{is_private, private_ip};
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
        ] {
            assert!(private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2001:db8::1",
            "::ffff:1.1.1.1",
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
}
