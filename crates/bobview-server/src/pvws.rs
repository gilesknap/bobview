//! A PV Web Socket server in the dialect of <https://github.com/ornl-epics/pvws>.
//!
//! Clients send `subscribe`, `clear`, `write`, `list` and `echo` messages and
//! get one `update` per changed PV: full metadata first, then only what
//! changed. Like PVWS, an update with neither `value` nor `text` means the PV
//! is not connected.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
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
    /// The token every `write` message must carry.
    pub write_token: String,
    /// Origins other than the server's own whose pages may connect,
    /// e.g. `https://host.example.com`.
    pub allow_origins: Vec<String>,
    /// Names browsers may use for this server, besides IP addresses,
    /// `localhost`, this machine's name and the hosts of `allow_origins`;
    /// checked against `Host` to stop DNS rebinding.
    pub hosts: Vec<String>,
    /// Least time between two rounds of updates to one client.
    pub throttle: Duration,
    /// Most clients connected at once from one address.
    pub max_per_ip: usize,
    /// How often to ping a client; one that has said nothing for two
    /// intervals is dropped.
    pub ping: Duration,
}

/// What every client connection shares.
pub struct Hub {
    manager: PvManager,
    config: Config,
    /// Bumped after every PV change.
    changed: watch::Sender<()>,
    started: String,
    /// Clients connected now, by address.
    clients: Mutex<HashMap<IpAddr, usize>>,
    /// PVs subscribed to by all clients together.
    subscriptions: Arc<AtomicUsize>,
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
            clients: Mutex::new(HashMap::new()),
            subscriptions: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Counts a connected client until dropped.
struct ClientSlot(Arc<Hub>, IpAddr);

impl ClientSlot {
    /// A slot for a client at `ip`, or why there is none.
    fn take(hub: &Arc<Hub>, ip: IpAddr) -> Result<Self, String> {
        let mut clients = hub.clients.lock().unwrap();
        if clients.values().sum::<usize>() >= MAX_CLIENTS {
            return Err(format!("{MAX_CLIENTS} clients already connected"));
        }
        let n = clients.entry(ip).or_default();
        if *n >= hub.config.max_per_ip {
            return Err(format!(
                "{} clients already connected from {ip}",
                hub.config.max_per_ip
            ));
        }
        *n += 1;
        Ok(Self(hub.clone(), ip))
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        let mut clients = self.0.clients.lock().unwrap();
        if let Some(n) = clients.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                clients.remove(&self.1);
            }
        }
    }
}

/// The largest message a client may send.
const MAX_MESSAGE: usize = 1 << 20;
/// The most PVs one client may subscribe to.
const MAX_PVS: usize = 10_000;
/// The most clients connected at once.
const MAX_CLIENTS: usize = 100;
/// The most PVs all clients together may subscribe to.
const MAX_TOTAL_PVS: usize = 50_000;
/// The longest PV name accepted.
const MAX_NAME: usize = 256;
/// The longest a send to one client may take before it is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn socket(
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Browsers let any page open a websocket to any server, so only pages
    // from here (or allowed origins) may. A page can also reach here by a
    // name of its own that resolves to this server (DNS rebinding), so the
    // name it used must be one of ours too.
    if !browser_allowed(&headers, &hub.config) {
        let origin = headers.get(ORIGIN).and_then(|o| o.to_str().ok());
        let host = headers.get(HOST).and_then(|h| h.to_str().ok());
        log::warn!(
            "refused a PVWS client {peer} from origin {origin:?} to host {host:?} \
             (see --allow-origin and --host)"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    let slot = match ClientSlot::take(&hub, peer.ip()) {
        Ok(slot) => slot,
        Err(why) => {
            log::warn!("refused PVWS client {peer}: {why}");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    ws.max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            serve(&hub, socket, peer).await;
            drop(slot);
        })
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
    /// Gives this subscription back to the server-wide count when dropped.
    _slot: SubSlot,
}

struct SubSlot(Arc<AtomicUsize>);

impl SubSlot {
    fn take(total: &Arc<AtomicUsize>) -> Option<Self> {
        let n = total.fetch_add(1, Ordering::SeqCst);
        let slot = Self(total.clone());
        (n < MAX_TOTAL_PVS).then_some(slot)
    }
}

impl Drop for SubSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether `name` is a plausible PV name: not too long, and only printable
/// ASCII with no spaces.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether `given` is the write token, without stopping at the first
/// difference.
fn token_ok(given: &str, token: &str) -> bool {
    let (a, b) = (given.as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

async fn serve(hub: &Hub, mut ws: WebSocket, peer: SocketAddr) {
    let mut subs = BTreeMap::new();
    let mut changed = hub.changed.subscribe();
    let mut last_round = Instant::now();
    // Without pings a client that has vanished (or stopped reading) would
    // keep its slot for ever.
    let mut ping = tokio::time::interval(hub.config.ping);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            msg = ws.recv() => {
                heard = Instant::now();
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let replies = handle(hub, &mut subs, &text, peer);
                        // New subscriptions may already hold a value.
                        if !sent(send_all(&mut ws, replies)).await
                            || !sent(send_updates(hub, &mut ws, &mut subs)).await
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() > hub.config.ping * 2 {
                    log::info!("dropping PVWS client {peer}: no answer to pings");
                    break;
                }
                if !sent(async { ws.send(Message::Ping(Default::default())).await.is_ok() }).await {
                    break;
                }
            }
            result = changed.changed() => {
                if result.is_err() {
                    break;
                }
                tokio::time::sleep_until((last_round + hub.config.throttle).into()).await;
                last_round = Instant::now();
                if !sent(send_updates(hub, &mut ws, &mut subs)).await {
                    break;
                }
            }
        }
    }
    log::debug!("PVWS client {peer} left with {} PVs", subs.len());
}

/// Whether a send finished, and in time.
async fn sent(send: impl std::future::Future<Output = bool>) -> bool {
    tokio::time::timeout(SEND_TIMEOUT, send)
        .await
        .unwrap_or(false)
}

/// Act on one client message, returning any replies.
fn handle(
    hub: &Hub,
    subs: &mut BTreeMap<String, Sub>,
    text: &str,
    peer: SocketAddr,
) -> Vec<String> {
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
            let mut refused_total = 0;
            let mut invalid = 0;
            for name in names() {
                // A repeat subscription gets the full state again.
                if let Some(sub) = subs.get_mut(name) {
                    sub.sent = None;
                } else if !valid_name(name) {
                    invalid += 1;
                } else if subs.len() >= MAX_PVS {
                    refused += 1;
                } else if let Some(_slot) = SubSlot::take(&hub.subscriptions) {
                    let pv = hub.manager.subscribe(name);
                    subs.insert(
                        name.to_string(),
                        Sub {
                            pv,
                            sent: None,
                            _slot,
                        },
                    );
                } else {
                    refused_total += 1;
                }
            }
            if invalid > 0 {
                log::warn!("PVWS client {peer} asked for {invalid} invalid PV names");
                return vec![error(format!(
                    "Cannot subscribe to {invalid} PVs: names are at most {MAX_NAME} \
                     printable characters with no spaces"
                ))];
            }
            if refused > 0 {
                return vec![error(format!(
                    "Cannot subscribe to {refused} more PVs: at most {MAX_PVS} per client"
                ))];
            }
            if refused_total > 0 {
                return vec![error(format!(
                    "Cannot subscribe to {refused_total} more PVs: the server is at its \
                     limit of {MAX_TOTAL_PVS} PVs for all clients"
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
            let given = msg.get("token").and_then(Json::as_str).unwrap_or_default();
            if !token_ok(given, &hub.config.write_token) {
                log::warn!("refused a write to {name} from {peer}: bad token");
                return vec![error(format!("Cannot write {name}: bad or missing token"))];
            }
            match msg.get("value").and_then(write_text) {
                Some(value) => {
                    log::info!("write {name} = {value} from {peer}");
                    // bobview's own mark for a string-format write (see
                    // `pv::Pv::write_long_string`).
                    if msg.get("long_string").and_then(Json::as_bool) == Some(true) {
                        sub.pv.write_long_string(value);
                    } else {
                        sub.pv.write(value);
                    }
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
        // Alarm and warning limits as PVWS sends them, and control limits,
        // which it does not: meters and sliders take their ranges from them.
        let limits = [
            (s.warning_range, "warn_low", "warn_high"),
            (s.alarm_range, "alarm_low", "alarm_high"),
            (s.control_range, "ctrl_low", "ctrl_high"),
        ];
        let last_limits = last.map(|l| [l.warning_range, l.alarm_range, l.control_range]);
        for (i, (range, lo_key, hi_key)) in limits.into_iter().enumerate() {
            let Some((lo, hi)) = range else { continue };
            if first || last_limits.is_some_and(|l| !same_range(l[i], range)) {
                m.insert(lo_key.into(), number(lo));
                m.insert(hi_key.into(), number(hi));
            }
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

/// Whether two limit pairs are equal, with NaN equal to NaN (unset alarm
/// limits are NaN, which would otherwise be sent on every update).
fn same_range(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> bool {
    let same = |x: f64, y: f64| x == y || (x.is_nan() && y.is_nan());
    match (a, b) {
        (Some(a), Some(b)) => same(a.0, b.0) && same(a.1, b.1),
        (None, None) => true,
        _ => false,
    }
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
            // The text for PVWS clients, the items for bobview.
            m.insert("text".into(), json!(a.join(", ")));
            m.insert("value".into(), json!(a.as_ref()));
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
    fn limits_and_string_arrays() {
        let mut a = PvState {
            alarm_range: Some((f64::NAN, f64::NAN)),
            warning_range: Some((1.0, 9.0)),
            control_range: Some((0.0, 10.0)),
            ..double(5.0)
        };
        let m = parse(update("X", &a, None, false));
        assert_eq!(m["warn_low"], 1.0);
        assert_eq!(m["warn_high"], 9.0);
        assert_eq!(m["alarm_low"], "NaN");
        assert_eq!(m["ctrl_high"], 10.0);
        // Unchanged NaN limits are not sent again; a changed limit is.
        assert!(update("X", &a.clone(), Some(&a), false).is_none());
        let before = a.clone();
        a.control_range = Some((0.0, 20.0));
        let m = parse(update("X", &a, Some(&before), false));
        assert_eq!(m["ctrl_high"], 20.0);
        assert!(!m.contains_key("warn_low"));

        let s = PvState {
            connected: true,
            value: Value::StrArray(vec!["a, b".into(), "c".into()].into()),
            ..Default::default()
        };
        let m = parse(update("X", &s, None, false));
        assert_eq!(m["vtype"], "VStringArray");
        assert_eq!(m["value"], json!(["a, b", "c"]));
        assert_eq!(m["text"], "a, b, c");
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
            write_token: "secret".into(),
            allow_origins: allow_origins.iter().map(|s| s.to_string()).collect(),
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
            throttle: Duration::ZERO,
            max_per_ip: 10,
            ping: Duration::from_secs(30),
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

    /// A server on a free port with `config`, and its hub.
    fn start(config: Config) -> (Arc<Hub>, SocketAddr, PvManager) {
        let manager = PvManager::new();
        let hub = Arc::new(Hub::new(manager.clone(), config));
        let app = axum::Router::new()
            .route("/pvws/pv", axum::routing::get(socket))
            .with_state(hub.clone())
            .into_make_service_with_connect_info::<SocketAddr>();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        manager.runtime().spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
        (hub, addr, manager)
    }

    /// Open a websocket by hand; returns the stream and the status line.
    async fn upgrade(addr: SocketAddr, origin: Option<&str>) -> (tokio::net::TcpStream, String) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        let request = format!(
            "GET /pvws/pv HTTP/1.1\r\nHost: {addr}\r\n{origin}Connection: Upgrade\r\n\
             Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut status = String::new();
        BufReader::new(&mut stream)
            .read_line(&mut status)
            .await
            .unwrap();
        (stream, status)
    }

    #[test]
    fn origin_is_checked_without_writes() {
        let mut read_only = config(&[], &[]);
        read_only.write = false;
        let (_hub, addr, manager) = start(read_only);
        manager.runtime().block_on(async {
            let (_s, status) = upgrade(addr, Some("http://evil.example")).await;
            assert!(status.contains("403"), "{status}");
            let (_s, status) = upgrade(addr, Some(&format!("http://{addr}"))).await;
            assert!(status.contains("101"), "{status}");
        });
    }

    #[test]
    fn silent_clients_are_dropped() {
        let mut quick = config(&[], &[]);
        quick.ping = Duration::from_millis(50);
        let (hub, addr, manager) = start(quick);
        manager.runtime().block_on(async {
            // Connects, then neither reads nor answers pings.
            let (stream, status) = upgrade(addr, None).await;
            assert!(status.contains("101"), "{status}");
            assert_eq!(hub.clients.lock().unwrap().len(), 1);
            let mut waited = 0;
            while !hub.clients.lock().unwrap().is_empty() {
                assert!(waited < 100, "the silent client kept its slot");
                tokio::time::sleep(Duration::from_millis(50)).await;
                waited += 1;
            }
            drop(stream);
        });
    }

    #[test]
    fn pv_names() {
        assert!(valid_name("SR-DI-DCCT-01:SIGNAL"));
        assert!(valid_name("pva://a.b[1]{x}"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("new\nline"));
        assert!(!valid_name("caf\u{e9}"));
        assert!(valid_name(&"x".repeat(MAX_NAME)));
        assert!(!valid_name(&"x".repeat(MAX_NAME + 1)));
    }

    #[test]
    fn write_tokens() {
        assert!(token_ok("secret", "secret"));
        assert!(!token_ok("", "secret"));
        assert!(!token_ok("secreT", "secret"));
        assert!(!token_ok("secret1", "secret"));
    }

    #[test]
    fn client_and_subscription_caps() {
        let hub = Arc::new(Hub::new(PvManager::new(), config(&[], &[])));
        let ip = |n: usize| IpAddr::from([10, 0, (n / 250) as u8, (n % 250) as u8]);
        let slots: Vec<_> = (0..MAX_CLIENTS)
            .map(|n| ClientSlot::take(&hub, ip(n)).expect("under the cap"))
            .collect();
        assert!(ClientSlot::take(&hub, ip(MAX_CLIENTS)).is_err());
        drop(slots);
        assert!(hub.clients.lock().unwrap().is_empty());
        assert!(ClientSlot::take(&hub, ip(0)).is_ok());

        // One address gets only its share.
        let mine: Vec<_> = (0..10)
            .map(|_| ClientSlot::take(&hub, ip(1)).expect("under the per-address cap"))
            .collect();
        assert!(ClientSlot::take(&hub, ip(1)).is_err());
        assert!(ClientSlot::take(&hub, ip(2)).is_ok());
        drop(mine);
        assert!(ClientSlot::take(&hub, ip(1)).is_ok());

        let total = Arc::new(AtomicUsize::new(MAX_TOTAL_PVS - 1));
        let last = SubSlot::take(&total).expect("one left");
        assert!(SubSlot::take(&total).is_none());
        drop(last);
        assert_eq!(total.load(Ordering::SeqCst), MAX_TOTAL_PVS - 1);
    }

    #[test]
    fn writes_need_the_token() {
        let hub = Hub::new(PvManager::new(), config(&[], &[]));
        let peer: SocketAddr = "192.0.2.1:5000".parse().unwrap();
        let mut subs = BTreeMap::new();
        let sub = |subs: &mut BTreeMap<String, Sub>, name: &str| {
            let msg = json!({"type": "subscribe", "pvs": [name]}).to_string();
            handle(&hub, subs, &msg, peer)
        };
        assert!(sub(&mut subs, "loc://x(1)").is_empty());
        assert_eq!(hub.subscriptions.load(Ordering::SeqCst), 1);
        let write = |subs: &mut BTreeMap<String, Sub>, token: Option<&str>| {
            let mut msg = json!({"type": "write", "pv": "loc://x(1)", "value": 2});
            if let Some(t) = token {
                msg["token"] = json!(t);
            }
            handle(&hub, subs, &msg.to_string(), peer)
        };
        assert!(write(&mut subs, None)[0].contains("token"));
        assert!(write(&mut subs, Some("wrong"))[0].contains("token"));
        assert!(write(&mut subs, Some("secret")).is_empty());
        // Bad names are refused and not counted.
        assert!(sub(&mut subs, "bad name")[0].contains("Cannot subscribe"));
        assert_eq!(subs.len(), 1);
        subs.clear();
        assert_eq!(hub.subscriptions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn long_string_writes_are_marked() {
        let hub = Hub::new(PvManager::new(), config(&[], &[]));
        let peer: SocketAddr = "192.0.2.1:5000".parse().unwrap();
        let mut subs = BTreeMap::new();
        let name = "loc://srv_long_string<VLongArray>(0,0)";
        let msg = json!({"type": "subscribe", "pvs": [name]}).to_string();
        assert!(handle(&hub, &mut subs, &msg, peer).is_empty());
        let write = |subs: &mut BTreeMap<String, Sub>, value: &str, long_string: bool| {
            let mut msg = json!({"type": "write", "pv": name, "value": value, "token": "secret"});
            if long_string {
                msg["long_string"] = json!(true);
            }
            assert!(handle(&hub, subs, &msg.to_string(), peer).is_empty());
            subs[name].pv.state().value
        };
        assert_eq!(
            write(&mut subs, "ab", true),
            Value::LongArray(Arc::from(vec![97, 98, 0]))
        );
        assert_eq!(
            write(&mut subs, "[1, 2]", false),
            Value::LongArray(Arc::from(vec![1, 2]))
        );
    }

    #[test]
    fn written_numbers() {
        assert_eq!(write_text(&json!(5.0)).unwrap(), "5");
        assert_eq!(write_text(&json!(2.5)).unwrap(), "2.5");
        assert_eq!(write_text(&json!("abc")).unwrap(), "abc");
    }
}
