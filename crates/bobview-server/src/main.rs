//! Serve bobview to browsers.
//!
//! One process gives a browser everything the web build needs, all from the
//! same origin: the app itself, the .bob files and a PV Web Socket (PVWS) at
//! `/pvws/pv` that speaks the protocol of <https://github.com/ornl-epics/pvws>
//! and reaches PVs through bobview's own CA/PVA layer. Run it anywhere
//! `caget` works and point a browser at it.

mod pvws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeDir;

const USAGE: &str = "\
usage: bobview-server [OPTIONS]

Serve the bobview web build, .bob files and a PV Web Socket on one port.
Open http://HOST:PORT/?display=FILE.bob in a browser (default index.bob).

  -p, --port PORT        port to listen on (default 8080)
  --bind ADDR            address to listen on (default 127.0.0.1, this machine
                         only; 0.0.0.0 is every interface)
  --web DIR              the web build from scripts/build-web.sh (default
                         ../share/bobview/web next to this binary, else dist/web)
  --opi DIR              serve .bob files from here; they take precedence over
                         files of the same name in the web build
  --write                let browsers write to PVs (off by default, as in PVWS);
                         every write must carry the write token
  --write-token TOKEN    the token --write wants (default: a random one,
                         printed at startup); a page passes it as &token=TOKEN
  --allow-origin ORIGIN  also let pages from ORIGIN connect, e.g.
                         https://host.example.com (may be repeated); other
                         pages than this server's own are refused
  --host NAME            let browsers call this server NAME, as well as an IP
                         address, localhost, this machine's name and the hosts
                         of --allow-origin (may be repeated)
  --throttle-ms MS       least time between updates of one PV (default 100)
  -h, --help             show this help
  -V, --version          show the version
";

fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("epics", log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    let mut port = 8080;
    let mut bind = "127.0.0.1".to_string();
    let mut web = None;
    let mut opi = None;
    let mut token = None;
    let mut config = pvws::Config {
        write: false,
        write_token: String::new(),
        allow_origins: Vec::new(),
        hosts: Vec::new(),
        throttle: Duration::from_millis(100),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--port" => port = parse(&arg, args.next()),
            "--bind" => bind = args.next().unwrap_or(bind),
            "--web" => web = Some(PathBuf::from(args.next().unwrap_or_default())),
            "--opi" => opi = args.next().map(PathBuf::from),
            "--write" => config.write = true,
            "--write-token" => token = args.next(),
            "--allow-origin" => config.allow_origins.extend(args.next()),
            "--host" => config.hosts.extend(args.next()),
            "--throttle-ms" => config.throttle = Duration::from_millis(parse(&arg, args.next())),
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("bobview-server {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {
                eprintln!("unexpected argument {arg:?}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if config.write {
        config.write_token = match token {
            Some(token) if !token.is_empty() => token,
            Some(_) => {
                eprintln!("--write-token: the token is empty");
                std::process::exit(2);
            }
            None => random_token(),
        };
        log::info!(
            "writes are on; pages must pass &token={} (or the token in --write-token)",
            config.write_token
        );
    } else if token.is_some() {
        log::warn!("--write-token does nothing without --write");
    }
    let web = web.unwrap_or_else(default_web);
    if !web.join("index.html").is_file() {
        eprintln!(
            "no web build in {} - run scripts/build-web.sh or pass --web DIR",
            web.display()
        );
        std::process::exit(1);
    }
    let addr: SocketAddr = match format!("{bind}:{port}").parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("bad address {bind}:{port}: {e}");
            std::process::exit(2);
        }
    };

    let manager = pv::PvManager::new();
    let hub = Arc::new(pvws::Hub::new(manager.clone(), config));
    let app = Router::new()
        .route("/pvws/pv", get(pvws::socket))
        .route("/pvws/info", get(pvws::info));
    let app = match &opi {
        Some(opi) => app.fallback_service(ServeDir::new(opi).fallback(ServeDir::new(&web))),
        None => app.fallback_service(ServeDir::new(&web)),
    }
    .layer(middleware::from_fn(hide_dot_files))
    .with_state(hub);

    manager.runtime().block_on(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cannot listen on {addr}: {e}");
                std::process::exit(1);
            }
        };
        log::info!("serving {} on http://{addr}/", web.display());
        if let Some(opi) = &opi {
            log::info!("displays from {}", opi.display());
        }
        let app = app.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, app).await {
            log::error!("server stopped: {e}");
        }
    });
}

/// 128 random bits as hex, from the operating system.
fn random_token() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if let Err(e) = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
        eprintln!("cannot make a write token ({e}): pass --write-token TOKEN");
        std::process::exit(1);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The web build installed next to this binary, else the one
/// scripts/build-web.sh writes in a checkout.
fn default_web() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share/bobview/web")))
        .filter(|web| web.join("index.html").is_file())
        .unwrap_or_else(|| PathBuf::from("dist/web"))
}

/// Hide dot-files and dot-dirs, such as `.git` when `--opi` is a checkout.
async fn hide_dot_files(request: Request, next: Next) -> Response {
    if is_hidden(request.uri().path()) {
        StatusCode::NOT_FOUND.into_response()
    } else {
        next.run(request).await
    }
}

/// Whether a path names a dot-file or something in a dot-dir. ServeDir
/// decodes the whole path before splitting it, so this does too: `%2F` is
/// a separator to it.
fn is_hidden(path: &str) -> bool {
    percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .split('/')
        .any(|segment| segment.starts_with('.'))
}

fn parse<T: std::str::FromStr>(flag: &str, value: Option<String>) -> T {
    match value.as_deref().map(str::parse) {
        Some(Ok(v)) => v,
        _ => {
            eprintln!("{flag}: expected a number, not {value:?}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_hidden;

    #[test]
    fn hides_dot_files() {
        assert!(is_hidden("/.git/config"));
        assert!(is_hidden("/%2egit/config"));
        assert!(is_hidden("/%2Egit/config"));
        assert!(is_hidden("/sub%2F.git%2Fconfig"));
        assert!(is_hidden("/a/.env"));
        assert!(!is_hidden("/a/b.bob"));
        assert!(!is_hidden("/a..b/c.bob"));
        assert!(!is_hidden("/"));
    }
}
