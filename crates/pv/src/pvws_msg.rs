//! Decoding the updates a PV Web Socket server sends (see `pvws`): kept
//! apart from the socket so it is tested natively.

use base64::Engine;
use serde_json::{Map, Value as Json};
use web_time::{Duration, SystemTime};

use crate::{PvState, Severity, Value};

/// Apply one update, which holds only what changed, to a PV's state.
pub(crate) fn apply(s: &mut PvState, vtype: &str, msg: &Map<String, Json>) {
    let f64_of = |key: &str| msg.get(key).and_then(Json::as_f64);
    // Limits may be "NaN" (or an infinity), which JSON has no number for.
    let limit = |key: &str| match msg.get(key)? {
        Json::String(t) => t.parse::<f64>().ok(),
        v => v.as_f64(),
    };
    let range = |lo: &str, hi: &str| Some((limit(lo)?, limit(hi)?));
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
        s.writable_known = true;
    }
    if let Some(units) = msg.get("units").and_then(Json::as_str) {
        s.units = units.to_string();
    }
    if let Some(p) = msg.get("precision").and_then(Json::as_i64) {
        s.precision = Some(p as i32);
    }
    if let Some(r) = range("min", "max") {
        s.display_range = Some(r);
    }
    if let Some(r) = range("warn_low", "warn_high") {
        s.warning_range = Some(r);
    }
    if let Some(r) = range("alarm_low", "alarm_high") {
        s.alarm_range = Some(r);
    }
    // Not in PVWS proper: bobview-server sends control limits too.
    if let Some(r) = range("ctrl_low", "ctrl_high") {
        s.control_range = Some(r);
    }
    if let Some(labels) = msg.get("labels").and_then(Json::as_array) {
        s.labels = labels
            .iter()
            .filter_map(|l| l.as_str().map(String::from))
            .collect();
    }
    if let Some(secs) = f64_of("seconds") {
        let nanos = f64_of("nanos").unwrap_or(0.0);
        if let Ok(t) = Duration::try_from_secs_f64(secs + nanos * 1e-9) {
            s.timestamp = Some(SystemTime::UNIX_EPOCH + t);
        }
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
    // bobview-server sends a string array's items under a key of its own,
    // besides the text PVWS sends it as.
    if let Some(Json::Array(items)) = msg.get("strings") {
        return Some(Value::StrArray(
            items
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(s: &mut PvState, msg: Json) {
        let Json::Object(msg) = msg else { panic!() };
        let vtype = msg["vtype"].as_str().unwrap_or("VDouble").to_string();
        apply(s, &vtype, &msg);
    }

    #[test]
    fn limits_come_through() {
        let mut s = PvState::default();
        update(
            &mut s,
            json!({"vtype": "VDouble", "value": 1.0, "min": 0, "max": 10,
                   "warn_low": 2, "warn_high": 8, "alarm_low": "NaN", "alarm_high": "NaN",
                   "ctrl_low": -1, "ctrl_high": 11}),
        );
        assert_eq!(s.display_range, Some((0.0, 10.0)));
        assert_eq!(s.warning_range, Some((2.0, 8.0)));
        assert!(
            s.alarm_range
                .is_some_and(|(lo, hi)| lo.is_nan() && hi.is_nan())
        );
        assert_eq!(s.control_range, Some((-1.0, 11.0)));
    }

    #[test]
    fn string_arrays_stay_arrays() {
        let mut s = PvState::default();
        update(
            &mut s,
            json!({"vtype": "VStringArray", "strings": ["a, b", "c"], "text": "a, b, c"}),
        );
        assert_eq!(
            s.value,
            Value::StrArray(vec!["a, b".into(), "c".into()].into())
        );
        // A PVWS server that sends only the text still shows something.
        update(&mut s, json!({"vtype": "VStringArray", "text": "a, b, c"}));
        assert_eq!(s.value, Value::Str("a, b, c".into()));
    }
}
