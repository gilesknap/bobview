use std::sync::Arc;
use std::time::SystemTime;

/// A PV's value, reduced to the few shapes a display needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    /// No value received yet.
    #[default]
    None,
    Double(f64),
    Long(i64),
    Str(String),
    /// Index into [`PvState::labels`].
    Enum(u16),
    DoubleArray(Arc<[f64]>),
    LongArray(Arc<[i64]>),
    StrArray(Arc<[String]>),
    /// An areaDetector image (PVA NTNDArray).
    Image(Arc<Image>),
}

/// A 2D image, row-major with x varying fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// 3 interleaved samples per pixel (RGB1) instead of 1.
    pub rgb: bool,
    pub data: ImageData,
    pub unique_id: i64,
}

/// Image samples in their native type.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageData {
    I8(Arc<[i8]>),
    U8(Arc<[u8]>),
    I16(Arc<[i16]>),
    U16(Arc<[u16]>),
    I32(Arc<[i32]>),
    U32(Arc<[u32]>),
    I64(Arc<[i64]>),
    U64(Arc<[u64]>),
    F32(Arc<[f32]>),
    F64(Arc<[f64]>),
}

impl ImageData {
    pub fn len(&self) -> usize {
        match self {
            ImageData::I8(a) => a.len(),
            ImageData::U8(a) => a.len(),
            ImageData::I16(a) => a.len(),
            ImageData::U16(a) => a.len(),
            ImageData::I32(a) => a.len(),
            ImageData::U32(a) => a.len(),
            ImageData::I64(a) => a.len(),
            ImageData::U64(a) => a.len(),
            ImageData::F32(a) => a.len(),
            ImageData::F64(a) => a.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sample `i` as a float; signed integer types are reinterpreted as
    /// unsigned when `unsigned` is set, as the Phoebus image widget does.
    pub fn get(&self, i: usize, unsigned: bool) -> f64 {
        match self {
            ImageData::I8(a) if unsigned => a[i] as u8 as f64,
            ImageData::I8(a) => a[i] as f64,
            ImageData::U8(a) => a[i] as f64,
            ImageData::I16(a) if unsigned => a[i] as u16 as f64,
            ImageData::I16(a) => a[i] as f64,
            ImageData::U16(a) => a[i] as f64,
            ImageData::I32(a) if unsigned => a[i] as u32 as f64,
            ImageData::I32(a) => a[i] as f64,
            ImageData::U32(a) => a[i] as f64,
            ImageData::I64(a) if unsigned => a[i] as u64 as f64,
            ImageData::I64(a) => a[i] as f64,
            ImageData::U64(a) => a[i] as f64,
            ImageData::F32(a) => a[i] as f64,
            ImageData::F64(a) => a[i],
        }
    }
}

impl Value {
    /// The value as a number, using the first element of an array.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Double(v) => Some(*v),
            Value::Long(v) => Some(*v as f64),
            Value::Enum(v) => Some(*v as f64),
            Value::Str(s) => s.trim().parse().ok(),
            Value::DoubleArray(a) => a.first().copied(),
            Value::LongArray(a) => a.first().map(|v| *v as f64),
            Value::StrArray(_) | Value::Image(_) | Value::None => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Long(v) => Some(*v),
            Value::Enum(v) => Some(*v as i64),
            other => other.as_f64().map(|v| v as i64),
        }
    }
}

/// EPICS alarm severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    #[default]
    NoAlarm,
    Minor,
    Major,
    Invalid,
}

impl Severity {
    pub fn from_epics(sevr: u16) -> Self {
        match sevr {
            0 => Severity::NoAlarm,
            1 => Severity::Minor,
            2 => Severity::Major,
            _ => Severity::Invalid,
        }
    }
}

/// Everything known about a PV at one moment.
#[derive(Clone, Debug, Default)]
pub struct PvState {
    pub connected: bool,
    pub value: Value,
    pub severity: Severity,
    pub status: u16,
    pub timestamp: Option<SystemTime>,
    pub units: String,
    pub precision: Option<i32>,
    /// Enum state labels.
    pub labels: Vec<String>,
    pub display_range: Option<(f64, f64)>,
    pub control_range: Option<(f64, f64)>,
    pub writable: bool,
    /// Why the PV cannot connect, if the backend knows.
    pub error: Option<String>,
}

impl PvState {
    /// Format the value as text. `precision` overrides the PV's own precision.
    pub fn format(&self, precision: Option<i32>, with_units: bool) -> String {
        let prec = precision.or(self.precision);
        let text = match &self.value {
            Value::None => return String::new(),
            Value::Double(v) => format_double(*v, prec),
            Value::Long(v) => v.to_string(),
            Value::Str(s) => s.clone(),
            Value::Enum(i) => self
                .labels
                .get(*i as usize)
                .cloned()
                .unwrap_or_else(|| i.to_string()),
            Value::DoubleArray(a) => join(a.iter().map(|v| format_double(*v, prec))),
            Value::LongArray(a) => join(a.iter().map(|v| v.to_string())),
            Value::StrArray(a) => join(a.iter().cloned()),
            Value::Image(img) => format!("{}x{} image #{}", img.width, img.height, img.unique_id),
        };
        let numeric = matches!(
            self.value,
            Value::Double(_) | Value::Long(_) | Value::DoubleArray(_) | Value::LongArray(_)
        );
        if with_units && numeric && !self.units.is_empty() {
            format!("{text} {}", self.units)
        } else {
            text
        }
    }

    /// Display limits, falling back to the control limits, if set (low < high).
    pub fn range(&self) -> Option<(f64, f64)> {
        self.display_range
            .filter(|(lo, hi)| lo < hi)
            .or(self.control_range.filter(|(lo, hi)| lo < hi))
    }
}

fn format_double(v: f64, precision: Option<i32>) -> String {
    match precision {
        Some(p) if p >= 0 => format!("{v:.*}", p.min(17) as usize),
        _ => {
            let s = format!("{v}");
            if s.len() > 12 { format!("{v:.6e}") } else { s }
        }
    }
}

fn join(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_precision_and_units() {
        let s = PvState {
            value: Value::Double(2.54),
            units: "Volts".into(),
            precision: Some(3),
            ..Default::default()
        };
        assert_eq!(s.format(None, true), "2.540 Volts");
        assert_eq!(s.format(Some(1), false), "2.5");
    }

    #[test]
    fn formats_enum_labels() {
        let s = PvState {
            value: Value::Enum(1),
            labels: vec!["Done".into(), "Acquire".into()],
            ..Default::default()
        };
        assert_eq!(s.format(None, true), "Acquire");
    }
}
