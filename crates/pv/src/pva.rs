//! PV Access backend using the pure-Rust `epics-pva-rs` client.
//!
//! Normative Types are flattened into [`PvState`]: NTScalar, NTScalarArray
//! and NTEnum fill the value and metadata, NTNDArray becomes an
//! [`Image`]. Other structures show their `value` field if they have one.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use epics_pva_rs::PvaError;
use epics_pva_rs::client::PvaClient;
use epics_pva_rs::client_native::ops_v2::{MonitorConnEvent, MonitorTermination};
use epics_pva_rs::pvdata::{PvField, PvStructure, ScalarValue, TypedScalarArray};
use tokio::sync::mpsc;

use crate::{Image, ImageData, PvState, Severity, Shared, Value, Writer};

pub(crate) async fn run(client: &PvaClient, pv: &str, shared: Arc<Shared>) {
    let (tx, mut writes) = mpsc::unbounded_channel::<String>();
    shared.set_writer(Writer::Channel(tx));

    // Writes run beside the monitor, each as its own put. Weak: the state
    // holds the sender this task waits on, so a strong reference would
    // keep both, and the PV's last value, alive after the PV is dropped.
    let put_client = client.clone();
    let put_pv = pv.to_string();
    let put_shared = Arc::downgrade(&shared);
    tokio::spawn(async move {
        while let Some(text) = writes.recv().await {
            let result = put_client.pvput(&put_pv, &text).await;
            if let Some(shared) = put_shared.upgrade() {
                shared.write_result(&text, result.err().map(|e| e.to_string()));
            }
        }
    });

    let mut retry = Duration::from_millis(500);
    loop {
        // Wait for the channel here, not in the monitor: dropping this wait,
        // as aborting the task does, cancels the search, while a monitor
        // dropped before it connects keeps searching. A search that times
        // out waits before the next, as the client's own searches slow down.
        match client.pvconnect(pv).await {
            Ok(_) => retry = Duration::from_millis(500),
            Err(e) => {
                if !matches!(e, PvaError::Timeout) {
                    log::debug!("{pv}: {e}; retrying in {retry:?}");
                    shared.update(|s| s.error = Some(e.to_string()));
                }
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(Duration::from_secs(30));
                continue;
            }
        }
        // A handle monitor, not `pvmonitor_events`: dropping the handle
        // sends the server DESTROY and ends the subscription, where
        // aborting a `pvmonitor_events` future left it registered and the
        // IOC still sending. Weak: the monitor's task can outlive the
        // handle while it reconnects, and must not keep the state alive.
        let data = Arc::downgrade(&shared);
        let conn = Arc::downgrade(&shared);
        let monitor = client
            .pvmonitor_handle(
                pv,
                move |_, value| {
                    if let Some(shared) = data.upgrade() {
                        shared.update(|s| apply(s, value));
                    }
                },
                move |event| {
                    let Some(shared) = conn.upgrade() else { return };
                    match event {
                        MonitorConnEvent::Connected { .. } => shared.update(|s| s.error = None),
                        MonitorConnEvent::Disconnected => shared.update(|s| s.connected = false),
                        MonitorConnEvent::Finished => {}
                    }
                },
            )
            .await;
        // The monitor reconnects by itself; it only ends when the channel
        // closes, the subscription finishes or fails, so try again later.
        let error = match monitor {
            Ok(handle) => match handle.wait_terminal().await {
                MonitorTermination::Ended => None,
                MonitorTermination::Failed(e) => Some(e),
            },
            Err(e) => Some(e),
        };
        shared.update(|s| {
            s.connected = false;
            if let Some(e) = &error {
                s.error = Some(e.to_string());
            }
        });
        if let Some(e) = error {
            log::debug!("{pv}: {e}; retrying in {retry:?}");
        }
        tokio::time::sleep(retry).await;
        retry = (retry * 2).min(Duration::from_secs(30));
    }
}

/// Update `s` from one monitor snapshot.
fn apply(s: &mut PvState, field: &PvField) {
    s.connected = true;
    s.error = None;
    let PvField::Structure(st) = field else {
        s.value = convert(field);
        return;
    };
    if st.struct_id.starts_with("epics:nt/NTNDArray") {
        if let Some(img) = image(st) {
            s.value = Value::Image(Arc::new(img));
        }
    } else if let Some(v) = st.get_field("value") {
        match v {
            PvField::Structure(e) if e.get_field("choices").is_some() => {
                s.value = Value::Enum(int(e.get_field("index")).unwrap_or(0) as u16);
                s.labels = strings(e.get_field("choices"));
            }
            other => s.value = convert(other),
        }
    }
    if let Some(PvField::Structure(a)) = st.get_field("alarm") {
        s.severity = Severity::from_epics(int(a.get_field("severity")).unwrap_or(0) as u16);
        s.status = int(a.get_field("status")).unwrap_or(0) as u16;
    }
    if let Some(PvField::Structure(t)) = st.get_field("timeStamp") {
        let secs = int(t.get_field("secondsPastEpoch")).unwrap_or(0);
        let nanos = int(t.get_field("nanoseconds")).unwrap_or(0);
        if secs > 0 {
            s.timestamp =
                Some(UNIX_EPOCH + Duration::new(secs as u64, nanos.clamp(0, 999_999_999) as u32));
        }
    } else {
        s.timestamp = Some(SystemTime::now());
    }
    if let Some(PvField::Structure(d)) = st.get_field("display") {
        if let Some(PvField::Scalar(ScalarValue::String(u))) = d.get_field("units") {
            s.units = u.as_str_lossy().into_owned();
        }
        if let Some(p) = int(d.get_field("precision")) {
            s.precision = Some(p as i32);
        }
        if let (Some(lo), Some(hi)) = (
            float(d.get_field("limitLow")),
            float(d.get_field("limitHigh")),
        ) {
            s.display_range = Some((lo, hi));
        }
    }
    if let Some(PvField::Structure(c)) = st.get_field("control")
        && let (Some(lo), Some(hi)) = (
            float(c.get_field("limitLow")),
            float(c.get_field("limitHigh")),
        )
    {
        s.control_range = Some((lo, hi));
    }
    if let Some(PvField::Structure(a)) = st.get_field("valueAlarm") {
        let pair = |lo: &str, hi: &str| float(a.get_field(lo)).zip(float(a.get_field(hi)));
        s.alarm_range = pair("lowAlarmLimit", "highAlarmLimit");
        s.warning_range = pair("lowWarningLimit", "highWarningLimit");
    }
    // PV Access does not report access rights per channel, so every PV is
    // assumed writable (`writable_known` stays false); the server refuses a
    // put it does not allow, and that is shown as a write error.
    s.writable = true;
}

fn scalar_f64(v: &ScalarValue) -> Option<f64> {
    Some(match v {
        ScalarValue::Boolean(b) => *b as i32 as f64,
        ScalarValue::Byte(x) => *x as f64,
        ScalarValue::Short(x) => *x as f64,
        ScalarValue::Int(x) => *x as f64,
        ScalarValue::Long(x) => *x as f64,
        ScalarValue::UByte(x) => *x as f64,
        ScalarValue::UShort(x) => *x as f64,
        ScalarValue::UInt(x) => *x as f64,
        ScalarValue::ULong(x) => *x as f64,
        ScalarValue::Float(x) => *x as f64,
        ScalarValue::Double(x) => *x,
        ScalarValue::String(_) => return None,
    })
}

fn float(f: Option<&PvField>) -> Option<f64> {
    match f? {
        PvField::Scalar(v) => scalar_f64(v),
        _ => None,
    }
}

fn int(f: Option<&PvField>) -> Option<i64> {
    float(f).map(|v| v as i64)
}

fn strings(f: Option<&PvField>) -> Vec<String> {
    match f {
        Some(PvField::ScalarArrayTyped(TypedScalarArray::String(a))) => {
            a.iter().map(|s| s.as_str_lossy().into_owned()).collect()
        }
        Some(PvField::ScalarArray(a)) => a
            .iter()
            .map(|v| match v {
                ScalarValue::String(s) => s.as_str_lossy().into_owned(),
                other => scalar_f64(other).map(|x| x.to_string()).unwrap_or_default(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn convert(f: &PvField) -> Value {
    match f {
        PvField::Scalar(ScalarValue::String(s)) => Value::Str(s.as_str_lossy().into_owned()),
        PvField::Scalar(v @ (ScalarValue::Float(_) | ScalarValue::Double(_))) => {
            Value::Double(scalar_f64(v).unwrap_or(0.0))
        }
        PvField::Scalar(v) => Value::Long(scalar_f64(v).unwrap_or(0.0) as i64),
        PvField::ScalarArrayTyped(a) => match a {
            TypedScalarArray::String(_) => Value::StrArray(strings(Some(f)).into()),
            TypedScalarArray::Float(x) => Value::DoubleArray(x.iter().map(|v| *v as f64).collect()),
            TypedScalarArray::Double(x) => Value::DoubleArray(x.iter().copied().collect()),
            other => Value::LongArray(typed_longs(other)),
        },
        PvField::ScalarArray(a) => {
            if a.iter().any(|v| matches!(v, ScalarValue::String(_))) {
                Value::StrArray(strings(Some(f)).into())
            } else {
                Value::DoubleArray(a.iter().map(|v| scalar_f64(v).unwrap_or(0.0)).collect())
            }
        }
        PvField::Union { value, .. } => convert(value),
        PvField::Variant(v) => convert(&v.value),
        _ => Value::None,
    }
}

fn typed_longs(a: &TypedScalarArray) -> Arc<[i64]> {
    match a {
        TypedScalarArray::Boolean(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Byte(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::UByte(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Short(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::UShort(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Int(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::UInt(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Long(x) => x.iter().copied().collect(),
        TypedScalarArray::ULong(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Float(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::Double(x) => x.iter().map(|v| *v as i64).collect(),
        TypedScalarArray::String(_) => Arc::from(Vec::new()),
    }
}

/// Decode an NTNDArray: dimensions, colour mode and the pixel union.
fn image(st: &PvStructure) -> Option<Image> {
    let dims: Vec<usize> = match st.get_field("dimension") {
        Some(PvField::StructureArray(ds)) => ds
            .iter()
            .flatten()
            .map(|d| int(d.get_field("size")).unwrap_or(0).max(0) as usize)
            .collect(),
        _ => Vec::new(),
    };
    if let Some(PvField::Structure(codec)) = st.get_field("codec")
        && let Some(PvField::Scalar(ScalarValue::String(name))) = codec.get_field("name")
        && !name.is_empty()
    {
        log::warn!(
            "compressed images ({}) are not supported",
            name.as_str_lossy()
        );
        return None;
    }
    let data = match st.get_field("value")? {
        PvField::Union { value, .. } => pixels(value)?,
        other => pixels(other)?,
    };
    // NDArray dimension order is x fastest; RGB1 puts the colour first.
    let (width, height, rgb) = match dims.as_slice() {
        [3, w, h] => (*w, *h, true),
        [w, h] => (*w, *h, false),
        [w, h, 1] => (*w, *h, false),
        [w] => (*w, 1, false),
        _ => return None,
    };
    let needed = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(if rgb { 3 } else { 1 }));
    if width == 0 || height == 0 || needed.is_none_or(|n| data.len() < n) {
        log::debug!(
            "image dimensions {dims:?} do not match {} samples",
            data.len()
        );
        return None;
    }
    Some(Image {
        width,
        height,
        rgb,
        data,
        unique_id: int(st.get_field("uniqueId")).unwrap_or(0),
    })
}

fn pixels(f: &PvField) -> Option<ImageData> {
    fn arc<T: Copy>(a: &epics_pva_rs::pvdata::PvArray<T>) -> Arc<[T]> {
        a.as_arc()
            .cloned()
            .unwrap_or_else(|| Arc::from(a.as_slice()))
    }
    let PvField::ScalarArrayTyped(a) = f else {
        return None;
    };
    Some(match a {
        TypedScalarArray::Byte(x) => ImageData::I8(arc(x)),
        TypedScalarArray::UByte(x) => ImageData::U8(arc(x)),
        TypedScalarArray::Short(x) => ImageData::I16(arc(x)),
        TypedScalarArray::UShort(x) => ImageData::U16(arc(x)),
        TypedScalarArray::Int(x) => ImageData::I32(arc(x)),
        TypedScalarArray::UInt(x) => ImageData::U32(arc(x)),
        TypedScalarArray::Long(x) => ImageData::I64(arc(x)),
        TypedScalarArray::ULong(x) => ImageData::U64(arc(x)),
        TypedScalarArray::Float(x) => ImageData::F32(arc(x)),
        TypedScalarArray::Double(x) => ImageData::F64(arc(x)),
        TypedScalarArray::Boolean(_) | TypedScalarArray::String(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use epics_pva_rs::nt::nd_array::{NdArrayBuffer, NdDimension, NtNdArray, nt_nd_array_value};

    #[test]
    fn decodes_ntndarray() {
        let dim = |size| NdDimension {
            size,
            offset: 0,
            full_size: size,
            binning: 1,
            reverse: false,
        };
        let nt = NtNdArray {
            value: NdArrayBuffer::Byte(vec![1i8, -1, 3, 4, 5, 6].into()),
            codec: Default::default(),
            compressed_size: 6,
            uncompressed_size: 6,
            unique_id: 42,
            data_time_stamp: Default::default(),
            alarm: Default::default(),
            time_stamp: Default::default(),
            dimension: vec![dim(3), dim(2)],
            attribute: Vec::new(),
        };
        let mut s = PvState::default();
        apply(&mut s, &nt_nd_array_value(&nt));
        let Value::Image(img) = &s.value else {
            panic!("not an image: {:?}", s.value)
        };
        assert_eq!(
            (img.width, img.height, img.rgb, img.unique_id),
            (3, 2, false, 42)
        );
        assert_eq!(img.data.get(1, true), 255.0);
        assert_eq!(img.data.get(1, false), -1.0);
    }

    #[test]
    fn writability_is_optimistic_but_not_known() {
        let mut nt = PvStructure::new("epics:nt/NTScalar:1.0");
        nt.set("value", PvField::Scalar(ScalarValue::Double(1.0)));
        let mut s = PvState::default();
        apply(&mut s, &PvField::Structure(nt));
        assert!(s.writable && !s.writable_known);
    }

    #[test]
    fn decodes_ntscalar_and_ntenum() {
        let mut display = PvStructure::new("display_t");
        display.set(
            "units",
            PvField::Scalar(ScalarValue::String("Volts".into())),
        );
        display.set("precision", PvField::Scalar(ScalarValue::Int(3)));
        let mut alarm = PvStructure::new("alarm_t");
        alarm.set("severity", PvField::Scalar(ScalarValue::Int(1)));
        let mut nt = PvStructure::new("epics:nt/NTScalar:1.0");
        nt.set("value", PvField::Scalar(ScalarValue::Double(5.15)));
        nt.set("alarm", PvField::Structure(alarm));
        nt.set("display", PvField::Structure(display));
        let mut s = PvState::default();
        apply(&mut s, &PvField::Structure(nt));
        assert_eq!(s.format(None, true), "5.150 Volts");
        assert_eq!(s.severity, Severity::Minor);

        let mut e = PvStructure::new("enum_t");
        e.set("index", PvField::Scalar(ScalarValue::Int(1)));
        e.set(
            "choices",
            PvField::ScalarArray(vec![
                ScalarValue::String("Done".into()),
                ScalarValue::String("Acquire".into()),
            ]),
        );
        let mut nt = PvStructure::new("epics:nt/NTEnum:1.0");
        nt.set("value", PvField::Structure(e));
        apply(&mut s, &PvField::Structure(nt));
        assert_eq!(s.format(None, false), "Acquire");
    }
}
