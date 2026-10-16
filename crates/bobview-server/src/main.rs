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
use axum::http::header::{CONTENT_SECURITY_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
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
  --follow-symlinks      serve files through symbolic links that lead out of
                         --opi or --web (refused by default)
  --write                let browsers write to PVs (off by default, as in PVWS);
                         every write must carry the write token
  --write-token TOKEN    the token --write wants (default: $BOBVIEW_WRITE_TOKEN
                         if set and not empty, else a random one printed at
                         startup); a page passes it as &token=TOKEN. Prefer
                         the variable: arguments show in `ps`
  --allow-origin ORIGIN  also let pages from ORIGIN connect, e.g.
                         https://host.example.com (may be repeated); other
                         pages than this server's own are refused
  --host NAME            let browsers call this server NAME, as well as an IP
                         address, localhost, this machine's name and the hosts
                         of --allow-origin (may be repeated)
  --max-per-ip N         most clients at once from one address (default 10;
                         raise it behind a reverse proxy, which is one address)
  --throttle-ms MS       least time between two batches of updates to one
                         client (default 100); a PV that changes faster sends
                         only its latest value
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
    let mut follow_symlinks = false;
    // The token and where it came from, for messages about it. An empty
    // variable is as good as unset.
    let mut token = std::env::var(TOKEN_VAR)
        .ok()
        .filter(|t| !t.is_empty())
        .map(|t| (t, TOKEN_VAR));
    let mut config = pvws::Config {
        write: false,
        write_token: String::new(),
        allow_origins: Vec::new(),
        hosts: Vec::new(),
        throttle: Duration::from_millis(100),
        max_per_ip: 10,
        ping: Duration::from_secs(30),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--port" => port = parse(&arg, args.next()),
            "--bind" => bind = args.next().unwrap_or(bind),
            "--web" => web = Some(PathBuf::from(args.next().unwrap_or_default())),
            "--opi" => opi = args.next().map(PathBuf::from),
            "--write" => config.write = true,
            "--write-token" => token = args.next().map(|t| (t, "--write-token")),
            "--follow-symlinks" => follow_symlinks = true,
            "--allow-origin" => config.allow_origins.extend(args.next()),
            "--host" => config.hosts.extend(args.next()),
            "--max-per-ip" => config.max_per_ip = parse(&arg, args.next()),
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
            Some((token, _)) if !token.is_empty() => token,
            Some((_, from)) => {
                eprintln!("{from}: the token is empty");
                std::process::exit(2);
            }
            None => {
                let token = random_token();
                // Not the log: RUST_LOG=warn would hide it.
                eprintln!("writes are on; pages must pass &token={token}");
                token
            }
        };
        log::info!("writes are on, and need the write token");
    } else if let Some((_, from)) = token {
        // A variable exported for every start is no mistake; a flag is.
        if from == TOKEN_VAR {
            log::debug!("{TOKEN_VAR} is set, but writes are off (--write)");
        } else {
            log::warn!("{from} does nothing without --write");
        }
    }
    let web = web.unwrap_or_else(default_web);
    if !web.join("index.html").is_file() {
        eprintln!(
            "no web build in {} - run scripts/build-web.sh or pass --web DIR",
            web.display()
        );
        std::process::exit(1);
    }
    let addr = match bind.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Ok(ip) => SocketAddr::new(ip, port),
        Err(e) => {
            eprintln!("--bind {bind}: {e}");
            std::process::exit(2);
        }
    };

    let manager = pv::PvManager::new();
    let hub = Arc::new(pvws::Hub::new(manager.clone(), config));
    let app = Router::new()
        .route("/pvws/pv", get(pvws::socket))
        .route("/pvws/info", get(pvws::info));
    let roots: Vec<PathBuf> = opi.iter().chain([&web]).cloned().collect();
    let app = match &opi {
        Some(opi) => app.fallback_service(ServeDir::new(opi).fallback(ServeDir::new(&web))),
        None => app.fallback_service(ServeDir::new(&web)),
    };
    let app = if follow_symlinks {
        app
    } else {
        let roots = Arc::new(Roots::new(&roots));
        app.layer(middleware::from_fn(move |request, next| {
            confine(roots.clone(), request, next)
        }))
    }
    .layer(middleware::from_fn(hide_dot_files))
    .layer(middleware::from_fn(no_framing))
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

/// The environment variable holding the write token.
const TOKEN_VAR: &str = "BOBVIEW_WRITE_TOKEN";

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

/// Stop other sites' pages framing this server's (clickjacking: a page
/// could lay its own over a display's buttons).
async fn no_framing(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// The directories files are served from, in order, each with its real
/// path (symbolic links resolved).
struct Roots(Vec<(PathBuf, Option<PathBuf>)>);

impl Roots {
    fn new(dirs: &[PathBuf]) -> Self {
        Self(
            dirs.iter()
                .map(|d| (d.clone(), std::fs::canonicalize(d).ok()))
                .collect(),
        )
    }

    /// Whether the file a request path names, in the first root that has
    /// it (as ServeDir picks), is really inside that root: a symbolic link
    /// may lead anywhere on the machine.
    fn contains(&self, path: &str) -> bool {
        let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
        let rel: PathBuf = path
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        for (dir, real_dir) in &self.0 {
            let file = dir.join(&rel);
            let Ok(real) = std::fs::canonicalize(&file) else {
                // Not here (or a dangling link): ServeDir looks in the next.
                continue;
            };
            let Some(real_dir) = real_dir else {
                return false;
            };
            if !real.starts_with(real_dir) {
                return false;
            }
            // A directory is served as its index.html.
            if real.is_dir()
                && let Ok(index) = std::fs::canonicalize(real.join("index.html"))
            {
                return index.starts_with(real_dir);
            }
            return true;
        }
        true
    }
}

/// Refuse files reached through a symbolic link that leads out of the
/// directory served (see --follow-symlinks).
async fn confine(roots: Arc<Roots>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let inside = tokio::task::spawn_blocking(move || roots.contains(&path))
        .await
        .unwrap_or(false);
    if inside {
        next.run(request).await
    } else {
        log::warn!(
            "refused {}: a link out of the served directories (see --follow-symlinks)",
            request.uri().path()
        );
        StatusCode::NOT_FOUND.into_response()
    }
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
    use super::{Roots, is_hidden};
    use std::path::PathBuf;

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
        assert!(roots.contains("/a.bob"));
        assert!(roots.contains("/sub/b.bob"));
        assert!(roots.contains("/inside/b.bob"));
        assert!(roots.contains("/index.html"));
        assert!(roots.contains("/"));
        assert!(roots.contains("/missing.bob"));
        assert!(!roots.contains("/secret.bob"));
        assert!(!roots.contains("/out/secret"));
        assert!(!roots.contains("/%73ecret.bob"));
        std::fs::remove_dir_all(&base).unwrap();
    }

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
