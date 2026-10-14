//! Reading displays from files or over HTTP(S).
//!
//! Fetched displays are kept in memory, so going back and forward or
//! reopening a screen doesn't fetch it again; [`forget`] clears them for a
//! reload. The first fetch of a display can run on a thread ([`prefetch`])
//! so a slow server doesn't freeze the window.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use bob::{Display, Error, Source};

/// Largest display file we will read.
const MAX_SIZE: u64 = 16 << 20;

static CACHE: LazyLock<Mutex<HashMap<String, Arc<str>>>> = LazyLock::new(Default::default);

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(concat!("bobview/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
});

/// Read and parse the display at `source`.
pub fn load(source: &Source) -> Result<Display, Error> {
    match source {
        Source::File(path) => Display::load(path),
        Source::Url(url) => {
            let text = get(url)?;
            let mut display = Display::parse(&text)?;
            display.source = Some(source.clone());
            Ok(display)
        }
    }
}

/// Start fetching `source` on a thread when it is a URL not yet cached;
/// the receiver gets the outcome, after which [`load`] won't block.
pub fn prefetch(source: &Source) -> Option<Receiver<Result<(), Error>>> {
    let Source::Url(url) = source else {
        return None;
    };
    if cached(url).is_some() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let url = url.clone();
    std::thread::spawn(move || {
        tx.send(get(&url).map(drop)).ok();
    });
    Some(rx)
}

/// Drop every cached display, so the next load fetches it again.
pub fn forget() {
    CACHE.lock().unwrap().clear();
}

fn cached(url: &str) -> Option<Arc<str>> {
    CACHE.lock().unwrap().get(url).cloned()
}

fn get(url: &str) -> Result<Arc<str>, Error> {
    if let Some(text) = cached(url) {
        return Ok(text);
    }
    log::info!("fetching {url}");
    let fail = |why: String| Error::Fetch(url.to_string(), why);
    let mut resp = AGENT.get(url).call().map_err(|e| fail(describe(e)))?;
    let text: Arc<str> = resp
        .body_mut()
        .with_config()
        .limit(MAX_SIZE)
        .read_to_string()
        .map_err(|e| fail(describe(e)))?
        .into();
    CACHE.lock().unwrap().insert(url.to_string(), text.clone());
    Ok(text)
}

fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        ureq::Error::Io(e) => e.to_string(),
        e => e.to_string(),
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
        let rx = prefetch(&src).expect("not cached yet");
        rx.recv().unwrap().unwrap();
        assert!(prefetch(&src).is_none(), "now cached");
        let d = load(&src).unwrap();
        assert_eq!(d.name, "Served");
        assert_eq!(d.source.as_ref(), Some(&src));
        assert_eq!(*hits.lock().unwrap(), 1, "second load came from the cache");

        let missing = src.resolve("../missing.bob");
        let err = load(&missing).unwrap_err().to_string();
        assert_eq!(err, format!("cannot fetch {base}/missing.bob: HTTP 404"));
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
}
