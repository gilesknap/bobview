//! A PV Web Socket server in the dialect of <https://github.com/ornl-epics/pvws>.
//!
//! Clients send `subscribe`, `clear`, `write`, `list` and `echo` messages and
//! get one `update` per changed PV: full metadata first, then only what
//! changed. Like PVWS, an update with neither `value` nor `text` means the PV
//! is not connected.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use pv::{Pv, PvManager, PvState, Severity, Value};
use serde_json::{Map, Value as Json, json};
use tokio::sync::watch;

pub struct Config {
    /// Accept `write` messages.
    pub write: bool,
    /// Origins other than the server's own whose pages may connect when
    /// writes are on, e.g. `https://host.example.com`.
    pub allow_origins: Vec<String>,
    /// Names browsers may use for this server when writes are on, besides
    /// IP addresses, `localhost`, this machine's name and the hosts of
    /// `allow_origins`; checked against `Host` to stop DNS rebinding.
    pub hosts: Vec<String>,
    /// Least time between two rounds of updates to one client.
    pub throttle: Duration,
}

/// What every client connection shares.
pub struct Hub {
    manager: PvManager,
    config: Config,
    /// Bumped after every PV change.
    changed: watch::Sender<()>,
    started: String,
}

impl Hub {
    pub fn new(manager: PvManager, config: Config) -> Self {
        let (changed, _) = watch::channel(());
        let tx = changed.clone();
        manager.set_notifier(Arc::new(move || tx.send_replace(())));
        let started = UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_secs()
            .to_string();
        Self {
            manager,
            config,
            changed,
            started,
        }
    }
}

/// The largest message a client may send.
const MAX_MESSAGE: usize = 1 << 20;
/// The most PVs one client may subscribe to.
const MAX_PVS: usize = 10_000;

pub async fn socket(
    State(hub): State<Arc<Hub>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Browsers let any page open a websocket to any server, so with writes
    // on, only pages from here (or allowed origins) may. A page can also
    // reach here by a name of its own that resolves to this server (DNS
    // rebinding), so the name it used must be one of ours too.
    if hub.config.write && !browser_allowed(&headers, &hub.config) {
        let origin = headers.get(ORIGIN).and_then(|o| o.to_str().ok());
        let host = headers.get(HOST).and_then(|h| h.to_str().ok());
        log::warn!(
            "refused a PVWS client from origin {origin:?} to host {host:?} \
             (see --allow-origin and --host)"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| serve(hub, socket))
}

/// Whether a request comes from a page of this server or an allowed
/// origin, by a name of this server; a request without an `Origin` is not
/// from a browser page.
fn browser_allowed(headers: &HeaderMap, config: &Config) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(host) = headers.get(HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if !host_allowed(host_name(&host), config) {
        return false;
    }
    let origin = normalise_origin(origin);
    if config
        .allow_origins
        .iter()
        .any(|a| normalise_origin(a) == origin)
    {
        return true;
    }
    // The page's own server, with any default port left out of both.
    let (scheme, origin_host) = origin.split_once("://").unwrap_or_default();
    origin_host == strip_default_port(scheme, &host)
}

/// Whether browsers may call this server `host` (lowercase, no port).
fn host_allowed(host: &str, config: &Config) -> bool {
    if host.parse::<Ipv4Addr>().is_ok()
        || host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .is_some_and(|h| h.parse::<Ipv6Addr>().is_ok())
        || host == "localhost"
    {
        return true;
    }
    let short = |name: &str| name.split('.').next().unwrap_or_default().to_string();
    let ours = machine_names().iter().any(|name| {
        let short = short(name);
        host == name
            || host == short
            || host
                .strip_prefix(&short)
                .is_some_and(|d| d.starts_with('.'))
    });
    ours || config.hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
        || config.allow_origins.iter().any(|a| {
            let origin = normalise_origin(a);
            let host_port = origin.split_once("://").map_or(&*origin, |(_, h)| h);
            host_name(host_port) == host
        })
}

/// The host of a `host[:port]`, keeping the brackets of an IPv6 address.
fn host_name(host_port: &str) -> &str {
    match host_port.find(']') {
        Some(end) if host_port.starts_with('[') => &host_port[..=end],
        _ => host_port.split(':').next().unwrap_or_default(),
    }
}

/// An origin in one spelling: lowercase, no trailing `/`, no default port.
fn normalise_origin(origin: &str) -> String {
    let origin = origin.trim_end_matches('/').to_ascii_lowercase();
    match origin.split_once("://") {
        Some((scheme, host)) => format!("{scheme}://{}", strip_default_port(scheme, host)),
        None => origin,
    }
}

fn strip_default_port<'a>(scheme: &str, host: &'a str) -> &'a str {
    let port = match scheme {
        "http" | "ws" => ":80",
        "https" | "wss" => ":443",
        _ => return host,
    };
    host.strip_suffix(port).unwrap_or(host)
}

/// This machine's names, as the kernel and /etc/hostname have them.
fn machine_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<String> = ["/proc/sys/kernel/hostname", "/etc/hostname"]
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        names.dedup();
        names
    })
}

pub async fn info(State(hub): State<Arc<Hub>>) -> axum::Json<Json> {
    axum::Json(json!({
        "server": format!("bobview-server {}", env!("CARGO_PKG_VERSION")),
        "start_time": hub.started,
        "write": hub.config.write,
    }))
}

struct Sub {
    pv: Pv,
    /// The state the client last heard about.
    sent: Option<PvState>,
}

async fn serve(hub: Arc<Hub>, mut ws: WebSocket) {
    let mut subs = BTreeMap::new();
    let mut changed = hub.changed.subscribe();
    let mut last_round = Instant::now();
    loop {
        tokio::select! {
            msg = ws.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let replies = handle(&hub, &mut subs, &text);
                    // New subscriptions may already hold a value.
                    if !send_all(&mut ws, replies).await || !send_updates(&hub, &mut ws, &mut subs).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            result = changed.changed() => {
                if result.is_err() {
                    break;
                }
                tokio::time::sleep_until((last_round + hub.config.throttle).into()).await;
                last_round = Instant::now();
                if !send_updates(&hub, &mut ws, &mut subs).await {
                    break;
                }
            }
        }
    }
    log::debug!("PVWS client left with {} PVs", subs.len());
}

/// Act on one client message, returning any replies.
fn handle(hub: &Hub, subs: &mut BTreeMap<String, Sub>, text: &str) -> Vec<String> {
    let Ok(Json::Object(msg)) = serde_json::from_str::<Json>(text) else {
        return vec![error(format!("cannot parse {text:.100}"))];
    };
    let names = || {
        msg.get("pvs")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
    };
    match msg.get("type").and_then(Json::as_str) {
        Some("subscribe" | "monitor") => {
            let mut refused = 0;
            for name in names() {
                // A repeat subscription gets the full state again.
                if let Some(sub) = subs.get_mut(name) {
                    sub.sent = None;
                } else if subs.len() < MAX_PVS {
                    let pv = hub.manager.subscribe(name);
                    subs.insert(name.to_string(), Sub { pv, sent: None });
                } else {
                    refused += 1;
                }
            }
            if refused > 0 {
                return vec![error(format!(
                    "Cannot subscribe to {refused} more PVs: at most {MAX_PVS} per client"
                ))];
            }
            vec![]
        }
        Some("clear") => {
            for name in names() {
                subs.remove(name);
            }
            vec![]
        }
        Some("list") => {
            vec![json!({"type": "list", "pvs": subs.keys().collect::<Vec<_>>()}).to_string()]
        }
        Some("write") => {
            let name = msg.get("pv").and_then(Json::as_str).unwrap_or_default();
            let Some(sub) = subs.get(name) else {
                return vec![error(format!("Cannot write unknown PV {name}"))];
            };
            if !hub.config.write {
                return vec![error("writes are disabled on this server (--write)".into())];
            }
            match msg.get("value").and_then(write_text) {
                Some(value) => {
                    log::info!("write {name} = {value}");
                    sub.pv.write(value);
                    vec![]
                }
                None => vec![error(format!("Cannot write {name}: no value"))],
            }
        }
        Some("echo") => vec![text.to_string()],
        Some("ping") => vec![],
        other => vec![error(format!("unknown message type {other:?}"))],
    }
}

fn error(message: String) -> String {
    json!({"type": "error", "message": message}).to_string()
}

/// A written value as the text `Pv::write` takes; whole numbers lose their
/// `.0` so integer PVs accept them.
fn write_text(value: &Json) -> Option<String> {
    match value {
        Json::String(s) => Some(s.clone()),
        Json::Number(n) => Some(match n.as_f64() {
            Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => (x as i64).to_string(),
            _ => n.to_string(),
        }),
        Json::Bool(b) => Some(u8::from(*b).to_string()),
        _ => None,
    }
}

async fn send_all(ws: &mut WebSocket, messages: Vec<String>) -> bool {
    for msg in messages {
        if ws.send(Message::Text(msg.into())).await.is_err() {
            return false;
        }
    }
    true
}

/// Tell the client about every PV that changed since it last heard.
async fn send_updates(hub: &Hub, ws: &mut WebSocket, subs: &mut BTreeMap<String, Sub>) -> bool {
    let mut messages = Vec::new();
    for (name, sub) in subs.iter_mut() {
        let state = sub.pv.state();
        if let Some(msg) = update(name, &state, sub.sent.as_ref(), hub.config.write) {
            messages.push(msg);
        }
        sub.sent = Some(state);
    }
    send_all(ws, messages).await
}

/// The `update` message taking a client from `last` to `s`, if anything
/// it would see changed.
fn update(name: &str, s: &PvState, last: Option<&PvState>, write: bool) -> Option<String> {
    let mut m = Map::new();
    let readonly = !(write && s.writable);
    // Connected without a value yet is still disconnected to a client, so
    // the metadata goes with the first value.
    let live = |s: &PvState| s.connected && s.value != Value::None;
    let was_connected = last.is_some_and(live);
    if last.is_none_or(|l| readonly != !(write && l.writable)) {
        m.insert("readonly".into(), json!(readonly));
    }
    if !live(s) {
        if was_connected {
            // No value or text: disconnected.
            m.insert("severity".into(), json!("UNDEFINED"));
        }
    } else {
        let first = !was_connected;
        let changed = |f: &dyn Fn(&PvState) -> bool| first || last.is_some_and(f);
        if first {
            m.insert("vtype".into(), json!(vtype(&s.value)));
        }
        if changed(&|l| l.units != s.units) && !matches!(s.value, Value::Enum(_)) {
            m.insert("units".into(), json!(s.units));
        }
        if let Some(p) = s
            .precision
            .filter(|_| changed(&|l| l.precision != s.precision))
        {
            m.insert("precision".into(), json!(p));
        }
        if let Some((lo, hi)) = s
            .display_range
            .filter(|_| changed(&|l| l.display_range != s.display_range))
        {
            m.insert("min".into(), number(lo));
            m.insert("max".into(), number(hi));
        }
        if matches!(s.value, Value::Enum(_)) && changed(&|l| l.labels != s.labels) {
            m.insert("labels".into(), json!(s.labels));
        }
        if changed(&|l| l.severity != s.severity) {
            m.insert("severity".into(), json!(severity(s.severity)));
        }
        if changed(&|l| !same_value(&l.value, &s.value)) {
            encode_value(&mut m, s);
        }
        if let Some(t) = s
            .timestamp
            .filter(|_| changed(&|l| l.timestamp != s.timestamp))
        {
            let t = t.duration_since(UNIX_EPOCH).unwrap_or_default();
            m.insert("seconds".into(), json!(t.as_secs()));
            m.insert("nanos".into(), json!(t.subsec_nanos()));
        }
    }
    if m.is_empty() {
        return None;
    }
    let mut msg = Map::new();
    msg.insert("type".into(), json!("update"));
    msg.insert("pv".into(), json!(name));
    msg.extend(m);
    Some(Json::Object(msg).to_string())
}

/// Whether two values are equal, without comparing arrays that are the
/// same allocation element by element, and with NaN equal to NaN.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Double(x), Value::Double(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::DoubleArray(x), Value::DoubleArray(y)) => Arc::ptr_eq(x, y) || x == y,
        (Value::LongArray(x), Value::LongArray(y)) => Arc::ptr_eq(x, y) || x == y,
        (Value::StrArray(x), Value::StrArray(y)) => Arc::ptr_eq(x, y) || x == y,
        (Value::Image(x), Value::Image(y)) => Arc::ptr_eq(x, y) || x == y,
        _ => a == b,
    }
}

fn vtype(value: &Value) -> &'static str {
    match value {
        Value::Double(_) | Value::None => "VDouble",
        Value::Long(_) => "VLong",
        Value::Str(_) => "VString",
        Value::Enum(_) => "VEnum",
        Value::DoubleArray(_) => "VDoubleArray",
        Value::LongArray(a) if fits_i32(a) => "VIntArray",
        Value::LongArray(_) => "VDoubleArray",
        Value::StrArray(_) => "VStringArray",
        Value::Image(_) => "VImage",
    }
}

fn fits_i32(a: &[i64]) -> bool {
    a.iter().all(|&v| i32::try_from(v).is_ok())
}

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::NoAlarm => "NONE",
        Severity::Minor => "MINOR",
        Severity::Major => "MAJOR",
        Severity::Invalid => "INVALID",
    }
}

/// A double as PVWS sends it: non-finite values as text.
fn number(x: f64) -> Json {
    if x.is_finite() {
        json!(x)
    } else {
        json!("NaN")
    }
}

fn encode_value(m: &mut Map<String, Json>, s: &PvState) {
    let b64 = |bytes: Vec<u8>| json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    match &s.value {
        Value::None => {}
        Value::Double(v) => {
            m.insert("value".into(), number(*v));
        }
        Value::Long(v) => {
            m.insert("value".into(), json!(v));
        }
        Value::Str(t) => {
            m.insert("text".into(), json!(t));
        }
        Value::Enum(i) => {
            m.insert("value".into(), json!(i));
            let label = s
                .labels
                .get(*i as usize)
                .cloned()
                .unwrap_or_else(|| i.to_string());
            m.insert("text".into(), json!(label));
        }
        Value::DoubleArray(a) => {
            m.insert(
                "b64dbl".into(),
                b64(a.iter().flat_map(|v| v.to_le_bytes()).collect()),
            );
        }
        Value::LongArray(a) if fits_i32(a) => {
            let bytes = a.iter().flat_map(|&v| (v as i32).to_le_bytes()).collect();
            m.insert("b64int".into(), b64(bytes));
        }
        Value::LongArray(a) => {
            let bytes = a.iter().flat_map(|&v| (v as f64).to_le_bytes()).collect();
            m.insert("b64dbl".into(), b64(bytes));
        }
        Value::StrArray(a) => {
            m.insert("text".into(), json!(a.join(", ")));
        }
        Value::Image(img) => {
            // Images need a video stream, not JSON (bobview#20).
            m.insert(
                "text".into(),
                json!(format!("image {}x{}", img.width, img.height)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(msg: Option<String>) -> Map<String, Json> {
        match serde_json::from_str(&msg.expect("an update")).unwrap() {
            Json::Object(m) => m,
            _ => panic!("not an object"),
        }
    }

    fn double(v: f64) -> PvState {
        PvState {
            connected: true,
            value: Value::Double(v),
            units: "mA".into(),
            precision: Some(3),
            writable: true,
            ..Default::default()
        }
    }

    #[test]
    fn first_update_has_metadata_then_only_changes() {
        let a = double(1.5);
        let m = parse(update("X", &a, None, true));
        assert_eq!(m["vtype"], "VDouble");
        assert_eq!(m["units"], "mA");
        assert_eq!(m["severity"], "NONE");
        assert_eq!(m["value"], 1.5);
        assert_eq!(m["readonly"], false);

        let b = double(2.5);
        let m = parse(update("X", &b, Some(&a), true));
        let mut keys: Vec<_> = m.keys().collect();
        keys.sort();
        assert_eq!(keys, ["pv", "type", "value"]);
        assert!(update("X", &b, Some(&b), true).is_none());
    }

    #[test]
    fn unreachable_pv_has_no_value() {
        let m = parse(update("X", &PvState::default(), None, true));
        assert_eq!(m["readonly"], true);
        assert!(!m.contains_key("value") && !m.contains_key("text"));
        // ... and a lost connection says so once.
        let a = double(1.0);
        let gone = PvState {
            writable: true,
            ..Default::default()
        };
        let m = parse(update("X", &gone, Some(&a), true));
        assert_eq!(m["severity"], "UNDEFINED");
        assert!(update("X", &gone, Some(&gone), true).is_none());
    }

    #[test]
    fn metadata_comes_with_the_first_value() {
        // Connected, but the first value has yet to arrive.
        let empty = PvState {
            connected: true,
            writable: true,
            ..Default::default()
        };
        let m = parse(update("X", &empty, None, true));
        assert!(!m.contains_key("vtype") && !m.contains_key("severity"));
        assert!(update("X", &empty, Some(&empty), true).is_none());

        let m = parse(update("X", &double(1.0), Some(&empty), true));
        assert_eq!(m["vtype"], "VDouble");
        assert_eq!(m["units"], "mA");
        assert_eq!(m["value"], 1.0);
    }

    #[test]
    fn no_change_no_update() {
        let states = [
            PvState::default(),
            PvState {
                connected: true,
                ..Default::default()
            },
            double(1.0),
            double(f64::NAN),
            PvState {
                connected: true,
                value: Value::DoubleArray(vec![f64::NAN, 1.0].into()),
                ..Default::default()
            },
        ];
        for s in &states {
            for write in [false, true] {
                assert!(update("X", s, Some(s), write).is_none(), "{s:?}");
            }
        }
    }

    #[test]
    fn writes_off_means_readonly() {
        let m = parse(update("X", &double(1.0), None, false));
        assert_eq!(m["readonly"], true);
    }

    #[test]
    fn enums_and_arrays() {
        let e = PvState {
            connected: true,
            value: Value::Enum(1),
            labels: vec!["Off".into(), "On".into()],
            ..Default::default()
        };
        let m = parse(update("X", &e, None, false));
        assert_eq!(m["vtype"], "VEnum");
        assert_eq!(m["value"], 1);
        assert_eq!(m["text"], "On");
        assert_eq!(m["labels"], json!(["Off", "On"]));

        let a = PvState {
            connected: true,
            value: Value::LongArray(vec![1, -2].into()),
            ..Default::default()
        };
        let m = parse(update("X", &a, None, false));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(m["b64int"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, [1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff]);
    }

    fn config(allow_origins: &[&str], hosts: &[&str]) -> Config {
        Config {
            write: true,
            allow_origins: allow_origins.iter().map(|s| s.to_string()).collect(),
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
            throttle: Duration::ZERO,
        }
    }

    fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(ORIGIN, o.parse().unwrap());
        }
        h
    }

    #[test]
    fn origins() {
        let none = config(&[], &["server"]);
        let allowed = config(&["https://Other.example.com:443/"], &["server"]);
        let ok = |host, origin, config: &Config| browser_allowed(&headers(host, origin), config);
        assert!(ok("server:8080", None, &none));
        assert!(ok("server:8080", Some("http://server:8080"), &none));
        assert!(ok("Server:8080", Some("http://server:8080"), &none));
        assert!(!ok("server:8080", Some("http://evil:8080"), &none));
        assert!(!ok("server:8080", Some("http://server"), &none));
        assert!(ok("server", Some("http://server"), &none));
        assert!(ok("server:80", Some("http://server"), &none));
        assert!(!ok("server:8080", Some("https://other.example.com"), &none));
        assert!(ok(
            "server:8080",
            Some("https://other.example.com"),
            &allowed
        ));
        // DNS rebinding: the attacker's name for this server.
        assert!(!ok(
            "evil.example:8080",
            Some("http://evil.example:8080"),
            &none
        ));
        // But no Origin is not a browser.
        assert!(ok("evil.example:8080", None, &none));
        assert!(ok("127.0.0.1:8080", Some("http://127.0.0.1:8080"), &none));
        assert!(ok("[::1]:8080", Some("http://[::1]:8080"), &none));
        assert!(ok("localhost:8080", Some("http://localhost:8080"), &none));
        assert!(!ok("[evil]:8080", Some("http://[evil]:8080"), &none));
        // The host of an allowed origin is one of ours.
        assert!(ok(
            "other.example.com",
            Some("http://other.example.com"),
            &allowed
        ));
    }

    #[test]
    fn machine_names_are_hosts() {
        let Some(name) = machine_names().first() else {
            return;
        };
        let short = name.split('.').next().unwrap();
        let config = config(&[], &[]);
        assert!(host_allowed(name, &config));
        assert!(host_allowed(short, &config));
        assert!(host_allowed(&format!("{short}.some.domain"), &config));
        assert!(!host_allowed(&format!("{short}x.some.domain"), &config));
    }

    #[test]
    fn normalised_origins() {
        assert_eq!(
            normalise_origin("HTTPS://Host.Example:443/"),
            "https://host.example"
        );
        assert_eq!(normalise_origin("http://host:80"), "http://host");
        assert_eq!(normalise_origin("http://host:8080"), "http://host:8080");
        assert_eq!(normalise_origin("https://host:80"), "https://host:80");
    }

    #[test]
    fn written_numbers() {
        assert_eq!(write_text(&json!(5.0)).unwrap(), "5");
        assert_eq!(write_text(&json!(2.5)).unwrap(), "2.5");
        assert_eq!(write_text(&json!("abc")).unwrap(), "abc");
    }
}
