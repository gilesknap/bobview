//! The browser backend: `ca://` and `pva://` PVs over a PV Web Socket
//! ([PVWS](https://github.com/ornl-epics/pvws)), as Daedalus uses, and the
//! page's event loop for `sim://` PVs.
//!
//! One socket serves every PV. The client sends `subscribe`, `clear` and
//! `write` messages; the server sends an `update` for each change, carrying
//! only what changed (the first one also has the metadata).

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use futures_util::future::{AbortHandle, abortable};
use serde_json::{Map, Value as Json, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};
use web_time::{Duration, SystemTime};

use crate::{PvName, PvState, Scheme, Severity, Shared, Value, Writer};

/// The socket's URL, if set before the first PV subscribes.
static URL: Mutex<Option<String>> = Mutex::new(None);

/// Use the PVWS server at `url`: `wss://host/pvws/pv`, or just `host[:port]`
/// for that. Without one, the page's own server is used.
pub fn set_pvws(url: &str) {
    let url = url.trim().trim_end_matches('/');
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("wss://{url}/pvws/pv")
    };
    *URL.lock().unwrap() = Some(url);
}

fn url() -> String {
    if let Some(url) = URL.lock().unwrap().clone() {
        return url;
    }
    let location = web_sys::window().map(|w| w.location());
    let host = location
        .as_ref()
        .and_then(|l| l.host().ok())
        .unwrap_or_default();
    let secure = location.and_then(|l| l.protocol().ok()).as_deref() == Some("https:");
    format!("{}://{host}/pvws/pv", if secure { "wss" } else { "ws" })
}

/// What serves a browser PV; [`Task::abort`] stops it.
pub(crate) enum Task {
    /// A future on the page's event loop.
    Local(AbortHandle),
    /// A PVWS subscription, by the name the server knows it by.
    Socket(String, Weak<Shared>),
}

impl Task {
    pub(crate) fn abort(&self) {
        match self {
            Task::Local(handle) => handle.abort(),
            Task::Socket(name, shared) => SOCKET.with_borrow_mut(|s| s.clear(name, shared)),
        }
    }
}

/// Run `future` on the page's event loop until it ends or is aborted.
pub(crate) fn spawn(future: impl Future<Output = ()> + 'static) -> Task {
    let (future, handle) = abortable(future);
    wasm_bindgen_futures::spawn_local(async move {
        let _ = future.await;
    });
    Task::Local(handle)
}

/// Subscribe to a `ca://` or `pva://` PV over the socket.
pub(crate) fn subscribe(name: &PvName, shared: Arc<Shared>) -> Task {
    // Plain names are Channel Access on the server as here.
    let wire = match name.scheme {
        Scheme::Pva => format!("pva://{}", name.pv),
        _ => name.pv.clone(),
    };
    shared.set_writer(Writer::Pvws(wire.clone()));
    SOCKET.with_borrow_mut(|s| s.subscribe(&wire, &shared));
    Task::Socket(wire, Arc::downgrade(&shared))
}

/// Write `text` to `name`: as a number when it reads as one, as the server
/// converts only text to an enum's or string's value.
pub(crate) fn write(name: &str, state: &PvState, text: String) {
    let numeric = !matches!(state.value, Value::Str(_) | Value::StrArray(_));
    let value = match text.trim().parse::<f64>() {
        Ok(x) if numeric && x.is_finite() => json!(x),
        _ => json!(text),
    };
    let msg = json!({"type": "write", "pv": name, "value": value});
    SOCKET.with_borrow_mut(|s| {
        if !s.send(&msg) {
            log::warn!("{name}: not connected, cannot write '{text}'");
        }
    });
}

thread_local! {
    static SOCKET: RefCell<Socket> = RefCell::new(Socket::default());
}

#[derive(Default)]
struct Socket {
    ws: Option<WebSocket>,
    open: bool,
    subs: HashMap<String, Sub>,
    /// Wait before the next connection attempt after one fails.
    retry: Duration,
}

struct Sub {
    shared: Weak<Shared>,
    /// The PV's type (`VDouble`, `VEnum` …), sent with the first update only.
    vtype: String,
}

const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

impl Socket {
    fn subscribe(&mut self, wire: &str, shared: &Arc<Shared>) {
        let sub = Sub {
            shared: Arc::downgrade(shared),
            vtype: String::new(),
        };
        self.subs.insert(wire.to_string(), sub);
        if self.open {
            self.send(&json!({"type": "subscribe", "pvs": [wire]}));
        } else if self.ws.is_none() {
            self.connect();
        }
    }

    fn clear(&mut self, wire: &str, shared: &Weak<Shared>) {
        // A new subscription to the same name may have replaced this one.
        if self
            .subs
            .get(wire)
            .is_some_and(|s| Weak::ptr_eq(&s.shared, shared))
        {
            self.subs.remove(wire);
            if self.open {
                self.send(&json!({"type": "clear", "pvs": [wire]}));
            }
        }
    }

    /// Send a message if the socket is open.
    fn send(&self, msg: &Json) -> bool {
        match &self.ws {
            Some(ws) if self.open => ws.send_with_str(&msg.to_string()).is_ok(),
            _ => false,
        }
    }

    fn connect(&mut self) {
        let url = url();
        log::info!("connecting to PVWS at {url}");
        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(e) => {
                log::error!("cannot open {url}: {e:?}");
                self.failed(format!("cannot open {url}"));
                return;
            }
        };
        // The socket lives as long as the page, so its handlers can too.
        let onopen = Closure::<dyn FnMut()>::new(|| SOCKET.with_borrow_mut(Socket::opened));
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(|e: MessageEvent| {
            if let Some(text) = e.data().as_string() {
                SOCKET.with_borrow_mut(|s| s.received(&text));
            }
        });
        let onclose = Closure::<dyn FnMut()>::new(|| {
            SOCKET.with_borrow_mut(|s| s.failed("PVWS connection closed".into()))
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onopen.forget();
        onmessage.forget();
        onclose.forget();
        self.ws = Some(ws);
    }

    fn opened(&mut self) {
        log::info!("PVWS connected");
        self.open = true;
        self.retry = RETRY_MIN;
        self.subs.retain(|_, s| s.shared.strong_count() > 0);
        let pvs: Vec<&String> = self.subs.keys().collect();
        if !pvs.is_empty() {
            self.send(&json!({"type": "subscribe", "pvs": pvs}));
        }
    }

    /// The socket closed or never opened: show every PV disconnected and try
    /// again later.
    fn failed(&mut self, why: String) {
        if self.open {
            log::warn!("{why}");
        }
        self.open = false;
        self.ws = None;
        self.subs.retain(|_, s| s.shared.strong_count() > 0);
        for sub in self.subs.values() {
            if let Some(shared) = sub.shared.upgrade() {
                shared.update(|s| {
                    s.connected = false;
                    s.error = Some(why.clone());
                });
            }
        }
        let wait = self.retry.max(RETRY_MIN);
        self.retry = (wait * 2).min(RETRY_MAX);
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::sleep(wait).await;
            SOCKET.with_borrow_mut(|s| {
                if s.ws.is_none() && !s.subs.is_empty() {
                    s.connect();
                }
            });
        });
    }

    fn received(&mut self, text: &str) {
        let Ok(Json::Object(msg)) = serde_json::from_str::<Json>(text) else {
            log::warn!("PVWS sent something unexpected: {text:.200}");
            return;
        };
        match msg.get("type").and_then(Json::as_str) {
            Some("update") => {
                let Some(name) = msg.get("pv").and_then(Json::as_str) else {
                    return;
                };
                let Some(sub) = self.subs.get_mut(name) else {
                    return;
                };
                if let Some(vtype) = msg.get("vtype").and_then(Json::as_str) {
                    sub.vtype = vtype.to_string();
                }
                if let Some(shared) = sub.shared.upgrade() {
                    shared.update(|s| apply(s, &sub.vtype, &msg));
                }
            }
            Some("error") => log::warn!("PVWS: {}", msg.get("message").unwrap_or(&Json::Null)),
            _ => {}
        }
    }
}

/// Apply one update, which holds only what changed, to a PV's state.
fn apply(s: &mut PvState, vtype: &str, msg: &Map<String, Json>) {
    let f64_of = |key: &str| msg.get(key).and_then(Json::as_f64);
    if let Some(sevr) = msg.get("severity").and_then(Json::as_str) {
        s.severity = match sevr {
            "NONE" => Severity::NoAlarm,
            "MINOR" => Severity::Minor,
            "MAJOR" => Severity::Major,
            _ => Severity::Invalid,
        };
        // A lost connection is an UNDEFINED alarm with no value.
        if sevr == "UNDEFINED" && !msg.contains_key("value") && !msg.contains_key("text") {
            s.connected = false;
        }
    }
    if let Some(readonly) = msg.get("readonly").and_then(Json::as_bool) {
        s.writable = !readonly;
    }
    if let Some(units) = msg.get("units").and_then(Json::as_str) {
        s.units = units.to_string();
    }
    if let Some(p) = msg.get("precision").and_then(Json::as_i64) {
        s.precision = Some(p as i32);
    }
    if let (Some(lo), Some(hi)) = (f64_of("min"), f64_of("max")) {
        s.display_range = Some((lo, hi));
    }
    if let Some(labels) = msg.get("labels").and_then(Json::as_array) {
        s.labels = labels
            .iter()
            .filter_map(|l| l.as_str().map(String::from))
            .collect();
    }
    if let Some(secs) = f64_of("seconds") {
        let nanos = f64_of("nanos").unwrap_or(0.0);
        s.timestamp = Some(SystemTime::UNIX_EPOCH + Duration::from_secs_f64(secs + nanos * 1e-9));
    }
    if let Some(value) = value(vtype, msg) {
        s.value = value;
        s.error = None;
        // PVWS sends a PV it cannot reach as a bare readonly/timestamp
        // update, so only a value means connected.
        s.connected = true;
    }
}

fn value(vtype: &str, msg: &Map<String, Json>) -> Option<Value> {
    for (key, size) in [
        ("b64dbl", 8),
        ("b64flt", 4),
        ("b64int", 4),
        ("b64srt", 2),
        ("b64byt", 1),
    ] {
        let Some(b64) = msg.get(key).and_then(Json::as_str) else {
            continue;
        };
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        let items = bytes.chunks_exact(size);
        return Some(match key {
            "b64dbl" => Value::DoubleArray(
                items
                    .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
                    .collect(),
            ),
            "b64flt" => Value::DoubleArray(
                items
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64)
                    .collect(),
            ),
            "b64int" => Value::LongArray(
                items
                    .map(|b| i32::from_le_bytes(b.try_into().unwrap()) as i64)
                    .collect(),
            ),
            "b64srt" => Value::LongArray(
                items
                    .map(|b| i16::from_le_bytes(b.try_into().unwrap()) as i64)
                    .collect(),
            ),
            _ => Value::LongArray(items.map(|b| b[0] as i8 as i64).collect()),
        });
    }
    let text = msg.get("text").and_then(Json::as_str);
    match msg.get("value") {
        Some(v) if vtype == "VEnum" => Some(Value::Enum(v.as_f64()? as u16)),
        // Non-finite doubles come as text.
        Some(Json::String(_)) => Some(Value::Double(f64::NAN)),
        Some(v) if matches!(vtype, "VDouble" | "VFloat") || v.is_f64() => {
            Some(Value::Double(v.as_f64()?))
        }
        Some(v) => Some(Value::Long(v.as_i64()?)),
        None => text.map(|t| Value::Str(t.to_string())),
    }
}
