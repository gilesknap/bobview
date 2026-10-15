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
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;
    use std::time::Duration;

    use bob::{Error, Source};
    use ureq::ResponseExt;

    use super::{CACHE, Fetched, MAX_SIZE};

    static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent(concat!("bobview/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    });

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
                AGENT
                    .get(url)
                    .call()
                    .map_err(|e| fail(describe(e)))?
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
        let mut resp = AGENT.get(url).call().map_err(|e| fail(describe(e)))?;
        let served = resp.get_uri().to_string();
        let text: std::sync::Arc<str> = resp
            .body_mut()
            .with_config()
            .limit(MAX_SIZE)
            .read_to_string()
            .map_err(|e| fail(describe(e)))?
            .into();
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
