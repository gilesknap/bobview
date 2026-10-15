//! A PV Web Socket server in the dialect of <https://github.com/ornl-epics/pvws>.
//!
//! Clients send `subscribe`, `clear`, `write`, `list` and `echo` messages and
//! get one `update` per changed PV: full metadata first, then only what
//! changed. Like PVWS, an update with neither `value` nor `text` means the PV
//! is not connected.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use base64::Engine;
use pv::{Pv, PvManager, PvState, Severity, Value};
use serde_json::{Map, Value as Json, json};
use tokio::sync::watch;

pub struct Config {
    /// Accept `write` messages.
    pub write: bool,
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

pub async fn socket(State(hub): State<Arc<Hub>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve(hub, socket))
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
            for name in names() {
                // A repeat subscription gets the full state again.
                subs.entry(name.to_string())
                    .or_insert_with(|| Sub {
                        pv: hub.manager.subscribe(name),
                        sent: None,
                    })
                    .sent = None;
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
    let was_connected = last.is_some_and(|l| l.connected);
    if last.is_none_or(|l| readonly != !(write && l.writable)) {
        m.insert("readonly".into(), json!(readonly));
    }
    if !s.connected || s.value == Value::None {
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
        if changed(&|l| l.value != s.value) {
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

    #[test]
    fn written_numbers() {
        assert_eq!(write_text(&json!(5.0)).unwrap(), "5");
        assert_eq!(write_text(&json!(2.5)).unwrap(), "2.5");
        assert_eq!(write_text(&json!("abc")).unwrap(), "abc");
    }
}
