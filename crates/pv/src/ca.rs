//! Channel Access backend using the pure-Rust `epics-ca-rs` client.

use std::sync::Arc;

use epics_base_rs::server::snapshot::{DbrClass, Snapshot};
use epics_base_rs::types::DbFieldType;
use epics_ca_rs::EpicsValue;
use epics_ca_rs::client::{CaChannel, CaClient, ConnectionEvent};
use tokio::sync::{broadcast, mpsc};

use crate::{Put, Severity, Shared, Value, Writer};

/// The `DBR_CHAR` request type, which a long string is written as, as
/// `caput -S` does; the server converts it to the field's own type.
const DBR_CHAR: u16 = 4;

/// Whether to write `put` as a long string to a PV holding `value`, whose
/// native type and element count are `native` and `count`.
///
/// With a string `format` it is one when the PV holds an array of numbers.
/// Otherwise, as Phoebus's CA layer does, it is one when the PV is a char
/// array and the text is not a list of numbers, which could not be
/// written as an array anyway.
fn as_long_string(
    put: &Put,
    value: &Value,
    native: Option<DbFieldType>,
    count: Option<usize>,
) -> bool {
    if put.long_string {
        return value.is_numeric_array();
    }
    let items = crate::split_array(&put.text);
    native == Some(DbFieldType::Char)
        && count.is_some_and(|n| n > 1)
        && (items.is_empty() || items.iter().any(|x| x.parse::<f64>().is_err()))
}

pub(crate) async fn run(client: &CaClient, pv: &str, shared: Arc<Shared>) {
    let channel = client.create_channel(pv);
    let mut events = channel.connection_events();
    let (tx, mut writes) = mpsc::unbounded_channel();
    shared.set_writer(Writer::Channel(tx));

    let mut retry = std::time::Duration::from_millis(500);
    let mut monitor = loop {
        match channel.subscribe().await {
            Ok(m) => break m,
            Err(e) => {
                log::warn!("{pv}: cannot subscribe, retrying in {retry:?}: {e}");
                shared.update(|s| s.error = Some(e.to_string()));
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(std::time::Duration::from_secs(30));
            }
        }
    };

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(ConnectionEvent::Connected) => {
                    fetch_metadata(&channel, &shared).await;
                }
                Ok(ConnectionEvent::Disconnected) => {
                    shared.update(|s| s.connected = false);
                }
                Ok(ConnectionEvent::AccessRightsChanged { write, .. }) => {
                    shared.update(|s| { s.writable = write; s.writable_known = true; });
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            update = monitor.recv() => match update {
                Some(Ok(snap)) => apply(&shared, &snap, false),
                Some(Err(e)) => {
                    log::debug!("{pv}: {e}");
                    shared.update(|s| s.connected = false);
                }
                None => return,
            },
            Some(put) = writes.recv() => {
                let text = &put.text;
                let value = shared.snapshot().value;
                let native = channel.native_field_type().ok();
                let count = channel.element_count().ok().map(|n| n as usize);
                // Plain CA_PROTO_WRITE, as Phoebus does: a put-callback on a
                // busy record (areaDetector Acquire) completes only when it
                // returns to 0, and the IOC queues any later put-callback on
                // that record behind it, so Stop could never reach it.
                // Sent here, not on a spawned task, so writes keep their order.
                let result = if as_long_string(&put, &value, native, count) {
                    let bytes = crate::long_string_bytes(text, count);
                    channel
                        .put_as_dbr_nowait(DBR_CHAR, &EpicsValue::CharArray(bytes.into()))
                        .await
                } else {
                    // Arrays are typed as they are shown, `[1, 2]` or
                    // `["a", "b"]`; the server converts each string element
                    // to the field type.
                    let items: Option<Vec<String>> = match value {
                        Value::DoubleArray(_) | Value::LongArray(_) => Some(
                            crate::split_array(text).into_iter().map(String::from).collect(),
                        ),
                        Value::StrArray(_) => Some(crate::split_strings(text)),
                        _ => None,
                    };
                    if let Some(items) = items {
                        channel.put_string_array_nowait(&items).await
                    } else {
                        channel.put_string_nowait(text).await
                    }
                };
                shared.write_result(text, result.err().map(|e| e.to_string()));
            }
        }
    }
}

/// Read units, precision, limits and enum labels once per connection.
async fn fetch_metadata(channel: &CaChannel, shared: &Shared) {
    match channel.get_with_metadata(DbrClass::Ctrl).await {
        Ok(snap) => apply(shared, &snap, true),
        Err(e) => {
            log::debug!("metadata read failed: {e}");
            shared.update(|s| s.connected = true);
        }
    }
}

fn apply(shared: &Shared, snap: &Snapshot, metadata: bool) {
    let value = convert(&snap.value);
    shared.update(|s| {
        s.connected = true;
        s.error = None;
        s.value = value;
        s.severity = Severity::from_epics(snap.alarm.severity);
        s.status = snap.alarm.status;
        s.timestamp = Some(snap.timestamp.into());
        if !metadata {
            return;
        }
        if let Some(d) = &snap.display {
            s.units = d.units.as_str_lossy().into_owned();
            s.precision = Some(d.precision as i32);
            s.display_range = Some((d.lower_disp_limit, d.upper_disp_limit));
            s.alarm_range = Some((d.lower_alarm_limit, d.upper_alarm_limit));
            s.warning_range = Some((d.lower_warning_limit, d.upper_warning_limit));
        }
        if let Some(c) = &snap.control {
            s.control_range = Some((c.lower_ctrl_limit, c.upper_ctrl_limit));
        }
        if let Some(e) = &snap.enums {
            s.labels = e
                .strings
                .iter()
                .map(|l| l.as_str_lossy().into_owned())
                .collect();
        }
    });
}

fn convert(v: &EpicsValue) -> Value {
    use EpicsValue as E;
    fn longs<T: Copy + Into<i64>>(a: &[T]) -> Value {
        Value::LongArray(a.iter().map(|x| (*x).into()).collect())
    }
    match v {
        E::String(s) => Value::Str(s.as_str_lossy().into_owned()),
        E::Short(x) => Value::Long(*x as i64),
        E::Float(x) => Value::Double(*x as f64),
        E::Enum(x) => Value::Enum(*x),
        E::EnumWithChoices { index, .. } => Value::Enum(*index),
        E::Char(x) => Value::Long(*x as i64),
        E::Long(x) => Value::Long(*x as i64),
        E::Double(x) => Value::Double(*x),
        E::Int64(x) => Value::Long(*x),
        E::UInt64(x) => Value::Long(*x as i64),
        E::UShort(x) => Value::Long(*x as i64),
        E::ULong(x) => Value::Long(*x as i64),
        E::UChar(x) => Value::Long(*x as i64),
        E::ShortArray(a) => longs(a),
        E::FloatArray(a) => Value::DoubleArray(a.iter().map(|x| *x as f64).collect()),
        E::EnumArray(a) => longs(a),
        E::DoubleArray(a) => Value::DoubleArray(Arc::from(a.as_slice())),
        E::LongArray(a) => longs(a),
        E::CharArray(a) => longs(a),
        E::Int64Array(a) => longs(a),
        E::UInt64Array(a) => Value::LongArray(a.iter().map(|x| *x as i64).collect()),
        E::UShortArray(a) => longs(a),
        E::ULongArray(a) => longs(a),
        E::UCharArray(a) => longs(a),
        E::StringArray(a) => {
            Value::StrArray(a.iter().map(|s| s.as_str_lossy().into_owned()).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(text: &str, long_string: bool) -> Put {
        Put {
            text: text.into(),
            long_string,
        }
    }

    #[test]
    fn chooses_long_string_writes_as_phoebus_does() {
        let chars = Value::LongArray(Arc::from(vec![0; 4]));
        let char_wf = (Some(DbFieldType::Char), Some(256));
        let long_wf = (Some(DbFieldType::Long), Some(256));
        // A string format writes text into any array of numbers.
        assert!(as_long_string(
            &put("/data", true),
            &chars,
            char_wf.0,
            char_wf.1
        ));
        assert!(as_long_string(
            &put("1 2", true),
            &chars,
            long_wf.0,
            long_wf.1
        ));
        // ... but not into a string or a scalar.
        let text = Value::Str(String::new());
        assert!(!as_long_string(
            &put("/data", true),
            &text,
            Some(DbFieldType::String),
            Some(1)
        ));
        assert!(!as_long_string(
            &put("3", true),
            &Value::Long(0),
            char_wf.0,
            Some(1)
        ));
        // Otherwise only text that is not numbers, to a char array.
        assert!(as_long_string(
            &put("/data", false),
            &chars,
            char_wf.0,
            char_wf.1
        ));
        assert!(as_long_string(
            &put("", false),
            &chars,
            char_wf.0,
            char_wf.1
        ));
        assert!(!as_long_string(
            &put("[1, 2]", false),
            &chars,
            char_wf.0,
            char_wf.1
        ));
        assert!(!as_long_string(
            &put("/data", false),
            &chars,
            long_wf.0,
            long_wf.1
        ));
        assert!(!as_long_string(
            &put("x", false),
            &Value::Long(0),
            char_wf.0,
            Some(1)
        ));
    }
}
