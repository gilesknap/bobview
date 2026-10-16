//! Channel Access backend using the pure-Rust `epics-ca-rs` client.

use std::sync::Arc;

use epics_base_rs::server::snapshot::{DbrClass, Snapshot};
use epics_base_rs::types::DbFieldType;
use epics_ca_rs::EpicsValue;
use epics_ca_rs::client::{CaChannel, CaClient, ConnectionEvent};
use epics_ca_rs::protocol::{DBE_ALARM, DBE_VALUE};
use tokio::sync::broadcast;

use crate::{Put, PvState, Severity, Shared, Value, Writer};

/// The `DBR_CHAR` request type, which a long string is written as, as
/// `caput -S` does; the server converts it to the field's own type.
const DBR_CHAR: u16 = 4;

/// Whether to write `put` as a long string to a PV whose native type and
/// element count are `native` and `count`, as Phoebus's CA layer decides
/// it for text: when the PV is a char array (`DBF_CHAR`, more than one
/// element). That depends on the channel, not on the value last read, which
/// for an array holding one element can arrive as a scalar.
///
/// With a string `format` all text is written so. Otherwise text that is a
/// list of numbers is written as numbers, as Phoebus parses a text entry's
/// text for an array; its list splits on commas only, and its numbers are
/// finite, so `1 2`, `nan` or `inf` are text, as in Phoebus.
fn as_long_string(put: &Put, native: Option<DbFieldType>, count: Option<usize>) -> bool {
    let char_array = matches!(native, Some(DbFieldType::Char | DbFieldType::UChar))
        && count.is_some_and(|n| n > 1);
    char_array && (put.long_string || !is_number_list(&put.text))
}

/// Whether `text` is a list of finite numbers, `[1, 2]` or `1,2`, as
/// Phoebus's `FormatOptionHandler.parse` reads one for an array.
fn is_number_list(text: &str) -> bool {
    let t = text.trim();
    let t = t.strip_prefix('[').unwrap_or(t);
    let t = t.strip_suffix(']').unwrap_or(t);
    t.split(',')
        .all(|x| x.trim().parse::<f64>().is_ok_and(f64::is_finite))
}

/// How long a write to a PV that has yet to connect waits for it.
const WRITE_CONNECT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) async fn run(client: &CaClient, pv: &str, shared: Arc<Shared>) {
    let channel = client.create_channel(pv);
    let mut events = channel.connection_events();
    let (tx, mut writes) = crate::write_queue();
    shared.set_writer(Writer::Channel(tx));

    let mut retry = std::time::Duration::from_millis(500);
    let mut monitor = loop {
        // Value and alarm changes, as Phoebus asks for: not DBE_LOG, which
        // archive deadbands (ADEL) send, and the display does not need.
        match channel
            .subscribe_with_mask(0.0, DBE_VALUE | DBE_ALARM)
            .await
        {
            Ok(m) => break m,
            Err(e) => {
                log::warn!("{pv}: cannot subscribe, retrying in {retry:?}: {e}");
                shared.update(|s| s.error = Some(e.to_string()));
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(std::time::Duration::from_secs(30));
            }
        }
    };

    // When to read the value if no monitor update has come since the channel
    // connected; see `MONITOR_GRACE`.
    let mut fallback: Option<tokio::time::Instant> = None;
    // Whether the channel is connected, though its value may not have come.
    let mut linked = false;
    // A write to a PV that has yet to connect, as a `write_pv` action to a
    // PV not connected in advance makes, and when it gives up waiting. The
    // writes behind it stay queued meanwhile, so they keep their order, and
    // the PV's events are still served.
    let mut waiting: Option<(Put, tokio::time::Instant)> = None;
    loop {
        if let Some((put, _)) = waiting.take_if(|_| shared.connected())
            && shared.can_write(&put.text)
        {
            write(&channel, &shared, &put).await;
        }
        tokio::select! {
            event = events.recv() => match event {
                Ok(ConnectionEvent::Connected) => {
                    linked = true;
                    fetch_metadata(&channel, &shared).await;
                    fallback = Some(tokio::time::Instant::now() + MONITOR_GRACE);
                }
                Ok(ConnectionEvent::Disconnected) => {
                    linked = false;
                    fallback = None;
                    shared.update(|s| s.connected = false);
                }
                Ok(ConnectionEvent::AccessRightsChanged { write, .. }) => {
                    shared.update(|s| { s.writable = write; s.writable_known = true; });
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = tokio::time::sleep_until(fallback.unwrap_or_else(tokio::time::Instant::now)),
                if fallback.is_some() =>
            {
                fallback = None;
                if !shared.connected() {
                    read_value(&channel, &shared).await;
                }
            }
            update = monitor.recv() => match update {
                Some(Ok(snap)) => {
                    fallback = None;
                    apply(&shared, &channel, &snap, false);
                }
                Some(Err(e)) => {
                    log::debug!("{pv}: {e}");
                    shared.update(|s| s.connected = false);
                }
                None => return,
            },
            Some(put) = writes.recv(), if waiting.is_none() => {
                // Refused at once if the PV has dropped.
                if !shared.can_write(&put.text) {
                    continue;
                }
                if shared.connected() {
                    write(&channel, &shared, &put).await;
                } else {
                    waiting = Some((put, tokio::time::Instant::now() + WRITE_CONNECT));
                }
            }
            _ = tokio::time::sleep_until(
                waiting.as_ref().map_or_else(tokio::time::Instant::now, |w| w.1)
            ), if waiting.is_some() => {
                let Some((put, _)) = waiting.take() else { continue };
                if linked {
                    // Connected, but no value has come yet: send it as text.
                    write(&channel, &shared, &put).await;
                    continue;
                }
                // Those queued behind it fail too, rather than each waiting
                // in turn.
                let why = format!("not connected after {}s", WRITE_CONNECT.as_secs());
                shared.write_result(&put.text, Some(why.clone()));
                while let Ok(put) = writes.try_recv() {
                    shared.write_result(&put.text, Some(why.clone()));
                }
            }
        }
    }
}

/// Send one write to a connected channel and report how it went.
async fn write(channel: &CaChannel, shared: &Shared, put: &Put) {
    let text = &put.text;
    let value = shared.snapshot().value;
    let native = channel.native_field_type().ok();
    let count = channel.element_count().ok().map(|n| n as usize);
    // Plain CA_PROTO_WRITE, as Phoebus does: a put-callback on a busy
    // record (areaDetector Acquire) completes only when it returns to 0,
    // and the IOC queues any later put-callback on that record behind it,
    // so Stop could never reach it. Sent from the PV's task, not a spawned
    // one, so writes keep their order.
    let result = if as_long_string(put, native, count) {
        let bytes = crate::long_string_bytes(text, count);
        channel
            .put_as_dbr_nowait(DBR_CHAR, &EpicsValue::CharArray(bytes.into()))
            .await
    } else {
        // Arrays are typed as they are shown, `[1, 2]` or `["a", "b"]`; the
        // server converts each string element to the field type.
        let items: Option<Vec<String>> = match value {
            Value::DoubleArray(_) | Value::LongArray(_) => Some(
                crate::split_array(text)
                    .into_iter()
                    .map(String::from)
                    .collect(),
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

/// How long after a channel connects its monitor has to send the first
/// update before the value is read instead. The server sends one at once,
/// but epics-ca-rs leaves a subscription uninstalled when it cannot re-send
/// it on a reconnect (as C's `nciu::resubscribe` does), and the PV would
/// then never show as connected.
const MONITOR_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Read the value, alarm and time stamp, for a monitor that has sent
/// nothing since the channel connected.
async fn read_value(channel: &CaChannel, shared: &Shared) {
    match channel.get_with_metadata(DbrClass::Time).await {
        Ok(snap) => {
            log::warn!(
                "{}: no monitor update since connecting; showing a read value",
                shared.name
            );
            apply(shared, channel, &snap, false);
        }
        Err(e) => log::debug!("{}: value read failed: {e}", shared.name),
    }
}

/// Read units, precision, limits and enum labels once per connection.
async fn fetch_metadata(channel: &CaChannel, shared: &Shared) {
    match channel.get_with_metadata(DbrClass::Ctrl).await {
        Ok(snap) => apply(shared, channel, &snap, true),
        Err(e) => {
            log::debug!("metadata read failed: {e}");
            shared.update(|s| s.connected = true);
        }
    }
}

fn apply(shared: &Shared, channel: &CaChannel, snap: &Snapshot, metadata: bool) {
    let value = (!metadata).then(|| {
        let value = convert(&snap.value);
        // An array holding one element arrives as a scalar; keep it an
        // array, as Phoebus does for a channel of more than one element, so
        // that it is shown and written as one.
        if channel.element_count().is_ok_and(|n| n > 1) {
            into_array(value)
        } else {
            value
        }
    });
    shared.update(|s| apply_to(s, snap, value, metadata));
}

/// Apply a monitor update, or the metadata read once per connection. A
/// DBR_CTRL read carries no time stamp (it decodes as 1970), and may land
/// after the first monitor update, so it sets only the metadata: units,
/// precision, limits and labels.
fn apply_to(s: &mut PvState, snap: &Snapshot, value: Option<Value>, metadata: bool) {
    s.error = None;
    if let Some(value) = value {
        s.connected = true;
        s.value = value;
        s.severity = Severity::from_epics(snap.alarm.severity);
        s.status = snap.alarm.status;
        s.timestamp = Some(snap.timestamp.into());
    }
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
}

/// A scalar as an array of its one element; any other value as it is.
fn into_array(value: Value) -> Value {
    match value {
        Value::Double(x) => Value::DoubleArray(Arc::from([x])),
        Value::Long(x) => Value::LongArray(Arc::from([x])),
        Value::Enum(x) => Value::LongArray(Arc::from([x as i64])),
        Value::Str(x) => Value::StrArray(Arc::from([x])),
        other => other,
    }
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
        E::UInt64(x) => Value::from_u64(*x),
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
        E::UInt64Array(a) => Value::from_u64s(a),
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
    use epics_base_rs::server::snapshot::DisplayInfo;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn metadata_after_a_monitor_update_keeps_its_value_and_time() {
        let stamp = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let monitor = Snapshot::new(EpicsValue::Double(5.0), 3, 1, stamp);
        // DBR_CTRL has no time: it decodes as 1970. Its value and alarm may
        // be older than the monitor's on a reconnect.
        let mut ctrl = Snapshot::new(EpicsValue::Double(0.0), 0, 0, UNIX_EPOCH);
        ctrl.display = Some(DisplayInfo {
            units: "mm".into(),
            precision: 2,
            upper_disp_limit: 10.0,
            ..Default::default()
        });

        let mut s = PvState::default();
        apply_to(&mut s, &monitor, Some(convert(&monitor.value)), false);
        apply_to(&mut s, &ctrl, None, true);
        assert!(s.connected);
        assert_eq!(s.value, Value::Double(5.0));
        assert_eq!(s.severity, Severity::Minor);
        assert_eq!(s.status, 3);
        assert_eq!(s.timestamp, Some(stamp));
        assert_eq!(s.units, "mm");
        assert_eq!(s.precision, Some(2));
        assert_eq!(s.display_range, Some((0.0, 10.0)));
    }

    #[test]
    fn metadata_alone_does_not_make_a_value() {
        let ctrl = Snapshot::new(EpicsValue::Double(0.0), 0, 0, UNIX_EPOCH);
        let mut s = PvState::default();
        apply_to(&mut s, &ctrl, None, true);
        assert!(
            !s.connected,
            "connected comes with the first monitor update"
        );
        assert_eq!(s.value, Value::None);
        assert_eq!(s.timestamp, None);
    }

    #[test]
    fn a_value_read_connects_a_pv_its_monitor_left_waiting() {
        // The metadata read alone leaves it disconnected; the DBR_TIME read
        // `read_value` makes after `MONITOR_GRACE` is applied as a monitor
        // update, so the PV connects with its value, alarm and time stamp.
        let stamp = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ctrl = Snapshot::new(EpicsValue::Double(0.0), 0, 0, UNIX_EPOCH);
        let time = Snapshot::new(EpicsValue::Double(7.0), 3, 2, stamp);
        let mut s = PvState::default();
        apply_to(&mut s, &ctrl, None, true);
        assert!(!s.connected);
        apply_to(&mut s, &time, Some(convert(&time.value)), false);
        assert!(s.connected);
        assert_eq!(s.value, Value::Double(7.0));
        assert_eq!(s.severity, Severity::Major);
        assert_eq!(s.timestamp, Some(stamp));
    }

    fn put(text: &str, long_string: bool) -> Put {
        Put {
            text: text.into(),
            long_string,
        }
    }

    #[test]
    fn chooses_long_string_writes_as_phoebus_does() {
        let char_wf = (Some(DbFieldType::Char), Some(256));
        let long_string = |text, string_format, (native, count)| {
            as_long_string(&put(text, string_format), native, count)
        };
        // A string format writes all text into a char array, whatever
        // value was last read (one element arrives as a scalar).
        assert!(long_string("/data", true, char_wf));
        assert!(long_string("1, 2", true, char_wf));
        assert!(long_string("", true, char_wf));
        // ... but not into any other PV, which is written the usual way,
        // as Phoebus writes text to it.
        assert!(!long_string(
            "/data",
            true,
            (Some(DbFieldType::Long), Some(256))
        ));
        assert!(!long_string(
            "/data",
            true,
            (Some(DbFieldType::String), Some(1))
        ));
        assert!(!long_string("x", true, (Some(DbFieldType::Char), Some(1))));
        assert!(!long_string("x", true, (None, None)));
        // Otherwise text that is not a list of numbers, to a char array.
        assert!(long_string("/data", false, char_wf));
        assert!(long_string("", false, char_wf));
        assert!(!long_string("[1, 2]", false, char_wf));
        assert!(!long_string("5", false, char_wf));
        // Phoebus splits on commas only, and its numbers are finite.
        assert!(long_string("1 2", false, char_wf));
        assert!(long_string("nan", false, char_wf));
        assert!(long_string("inf", false, char_wf));
        assert!(!long_string(
            "/data",
            false,
            (Some(DbFieldType::Long), Some(256))
        ));
        assert!(!long_string("x", false, (Some(DbFieldType::Char), Some(1))));
    }

    #[test]
    fn unsigned_64_bit_values_never_wrap_negative() {
        assert_eq!(convert(&EpicsValue::UInt64(7)), Value::Long(7));
        assert_eq!(
            convert(&EpicsValue::UInt64(u64::MAX)),
            Value::Double(u64::MAX as f64)
        );
        assert_eq!(
            convert(&EpicsValue::UInt64Array(vec![1, u64::MAX].into())),
            Value::DoubleArray(Arc::from([1.0, u64::MAX as f64]))
        );
    }
}
