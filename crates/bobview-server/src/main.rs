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
use axum::routing::get;
use tower_http::services::ServeDir;

const USAGE: &str = "\
usage: bobview-server [OPTIONS]

Serve the bobview web build, .bob files and a PV Web Socket on one port.
Open http://HOST:PORT/?display=FILE.bob in a browser (default index.bob).

  -p, --port PORT        port to listen on (default 8080)
  --bind ADDR            address to listen on (default 0.0.0.0)
  --web DIR              the web build from scripts/build-web.sh (default dist/web)
  --opi DIR              serve .bob files from here; they take precedence over
                         files of the same name in the web build
  --write                let browsers write to PVs (off by default, as in PVWS)
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
    let mut bind = "0.0.0.0".to_string();
    let mut web = PathBuf::from("dist/web");
    let mut opi = None;
    let mut config = pvws::Config {
        write: false,
        throttle: Duration::from_millis(100),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--port" => port = parse(&arg, args.next()),
            "--bind" => bind = args.next().unwrap_or(bind),
            "--web" => web = PathBuf::from(args.next().unwrap_or_default()),
            "--opi" => opi = args.next().map(PathBuf::from),
            "--write" => config.write = true,
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
    let files = ServeDir::new(opi.as_ref().unwrap_or(&web)).fallback(ServeDir::new(&web));
    let app = Router::new()
        .route("/pvws/pv", get(pvws::socket))
        .route("/pvws/info", get(pvws::info))
        .fallback_service(files)
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
        if let Err(e) = axum::serve(listener, app).await {
            log::error!("server stopped: {e}");
        }
    });
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
