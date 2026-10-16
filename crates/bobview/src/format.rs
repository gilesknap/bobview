//! Values as text and back, as Phoebus's `FormatOptionHandler` does it for
//! a widget's `format`, `precision` and `show_units`.

use bob::Widget;
use pv::{PvState, Value};

/// Phoebus `FormatOption`, by its number in a `.bob` file.
mod option {
    pub const DEFAULT: i64 = 0;
    pub const DECIMAL: i64 = 1;
    pub const EXPONENTIAL: i64 = 2;
    pub const ENGINEERING: i64 = 3;
    pub const HEX: i64 = 4;
    pub const COMPACT: i64 = 5;
    pub const STRING: i64 = 6;
    pub const SEXAGESIMAL: i64 = 7;
    pub const SEXAGESIMAL_HMS: i64 = 8;
    pub const SEXAGESIMAL_DMS: i64 = 9;
    pub const BINARY: i64 = 10;
    pub const SIGNIFICANT: i64 = 11;
}
use option::*;

/// Phoebus `max_array_formatting`.
const MAX_ARRAY: usize = 256;

/// Value text as Phoebus formats it for the widget's `format` and `precision`.
pub(crate) fn format_value(w: &Widget, s: &PvState) -> String {
    let precision = w.i64("precision").filter(|p| *p >= 0).map(|p| p as i32);
    let show_units = w.bool("show_units").unwrap_or(true);
    let format = w.i64("format").unwrap_or(DEFAULT);
    // Phoebus `actualPrecision`: the widget's, else the PV's, else 2.
    let p = precision.or(s.precision).unwrap_or(2).clamp(0, 17) as usize;
    let units = |text: String| {
        if show_units && !s.units.is_empty() {
            format!("{text} {}", s.units)
        } else {
            text
        }
    };
    match (format, &s.value) {
        // An enum shows its label only in the default and string formats,
        // else its index.
        (DEFAULT | STRING, Value::Enum(_)) => s.format(precision, false),
        (_, Value::Enum(i)) => i.to_string(),
        (DEFAULT | DECIMAL, _) => s.format(precision, show_units),
        (STRING, Value::LongArray(a)) => a
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| (*c as u8) as char)
            .collect(),
        (_, Value::Double(_) | Value::Long(_)) => units(number(&s.value, format, p)),
        (_, Value::DoubleArray(a)) => units(array(a.iter().map(|v| Value::Double(*v)), format, p)),
        (_, Value::LongArray(a)) => units(array(a.iter().map(|v| Value::Long(*v)), format, p)),
        _ => s.format(precision, show_units),
    }
}

fn array(items: impl ExactSizeIterator<Item = Value>, format: i64, p: usize) -> String {
    let more = items.len() > MAX_ARRAY;
    let mut text: Vec<String> = items
        .take(MAX_ARRAY)
        .map(|v| number(&v, format, p))
        .collect();
    if more {
        text.push("...".into());
    }
    format!("[{}]", text.join(", "))
}

/// One number, `Value::Double` or `Value::Long`, in `format` with
/// precision `p` (Phoebus `formatNumber`).
fn number(v: &Value, format: i64, p: usize) -> String {
    let x = v.as_f64().unwrap_or(f64::NAN);
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // Java's `longValue`: integers as they are, doubles truncated.
    let long = match v {
        Value::Long(n) => *n,
        _ => x as i64,
    };
    match format {
        EXPONENTIAL => exponential(x, p),
        ENGINEERING if x == 0.0 => exponential(x, p),
        ENGINEERING => {
            let power = 3 * (x.abs().log10() / 3.0).floor() as i32;
            format!("{:.p$}E{power}", x / 10f64.powi(power))
        }
        HEX => format!("0x{long:0p$X}"),
        BINARY => format!("0b{long:0p$b}"),
        STRING => {
            let b = long as u8;
            if b.is_ascii() {
                (b as char).to_string()
            } else {
                '\u{FFFD}'.to_string()
            }
        }
        COMPACT if x.abs() > 0.0001 && x.abs() < 10000.0 => format!("{x:.p$}"),
        COMPACT => exponential(x, p),
        SIGNIFICANT => significant(x, p),
        SEXAGESIMAL => sexagesimal(x, p),
        SEXAGESIMAL_HMS => sexagesimal(x * 12.0 / std::f64::consts::PI, p),
        SEXAGESIMAL_DMS => sexagesimal(x * 180.0 / std::f64::consts::PI, p),
        _ => format!("{x:.p$}"),
    }
}

/// Java `DecimalFormat("0.000E0")`: `1.500E3`, `1.500E-3`.
fn exponential(x: f64, p: usize) -> String {
    format!("{x:.p$E}")
}

/// Java `%.<p>g`: `p` significant digits, in decimal from 10⁻⁴ up to
/// 10ᵖ, else in scientific notation as `1.23000e+05`. Unlike C it keeps
/// trailing zeros.
fn significant(x: f64, p: usize) -> String {
    let p = p.max(1);
    if x == 0.0 {
        return format!("{x:.*}", p - 1);
    }
    // Round to p digits first: the exponent is that of the rounded value.
    let sci = format!("{x:.*e}", p - 1);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..p as i32).contains(&exp) {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        format!("{x:.decimals$}")
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    }
}

/// Phoebus `SexagesimalFormat.format`: hours (or degrees), minutes and
/// seconds, `12:30:15.25`, with as many parts as the precision asks.
fn sexagesimal(mut value: f64, precision: usize) -> String {
    const PREC: [f64; 9] = [
        1.0,
        1.0 / 6.0,
        1.0 / 60.0,
        1.0 / 360.0,
        1.0 / 3.6e3,
        1.0 / 3.6e4,
        1.0 / 3.6e5,
        1.0 / 3.6e6,
        1.0 / 3.6e7,
    ];
    let prec_frac = match PREC.get(precision) {
        Some(f) => *f,
        None => PREC[8] * 0.1f64.powi(precision as i32 - 8),
    };
    value += 0.5 * prec_frac;
    let mut out = String::new();
    if value < 0.0 {
        out.push('-');
        value = -value + prec_frac;
    }
    let hrs = value.floor();
    value = (value - hrs) * 60.0;
    let min = value as i64;
    value = (value - min as f64) * 60.0;
    let sec = value as i64;
    out += &match precision {
        0 => format!("{hrs:.0}"),
        1 => format!("{hrs:.0}:{}", min / 10),
        2 => format!("{hrs:.0}:{min:02}"),
        3 => format!("{hrs:.0}:{min:02}:{}", sec / 10),
        4 => format!("{hrs:.0}:{min:02}:{sec:02}"),
        _ => {
            let frac = ((value - sec as f64) / (prec_frac * 3600.0)).floor();
            let digits = precision - 4;
            format!("{hrs:.0}:{min:02}:{sec:02}.{frac:0digits$.0}")
        }
    };
    out
}

/// Phoebus `SexagesimalFormat.parse`: `h[:m[:s]]`, signed.
fn parse_sexagesimal(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':');
    let first = parts.next()?;
    let negative = first.starts_with('-');
    let mut value: f64 = first.replace('-', "").parse().ok()?;
    if value != value.floor() || value < 0.0 {
        return None;
    }
    if let Some(m) = parts.next() {
        let m: f64 = m.parse().ok()?;
        if m != m.floor() || m < 0.0 {
            return None;
        }
        value += m / 60.0;
        if let Some(s) = parts.next() {
            let s: f64 = s.parse().ok()?;
            if s < 0.0 {
                return None;
            }
            value += s / 3600.0;
        }
    }
    Some(if negative { -value } else { value })
}

/// The text to write for what was typed into a text entry, as Phoebus's
/// `FormatOptionHandler.parse` makes it: trailing units (anything after
/// the last space) dropped from a number, `0x…` and `0b…` read as hex and
/// binary, an enum label as its index; the PV's backend converts the rest.
/// Text that does not parse is written as it is.
pub(crate) fn entry_value(w: &Widget, s: Option<&PvState>, text: &str) -> String {
    let format = w.i64("format").unwrap_or(DEFAULT);
    let Some(s) = s.filter(|s| s.connected) else {
        return text.to_string();
    };
    parse(&s.value, &s.labels, &s.units, format, text).unwrap_or_else(|| text.to_string())
}

fn parse(value: &Value, labels: &[String], units: &str, format: i64, text: &str) -> Option<String> {
    // Phoebus drops anything after the last space: units or part of them.
    let number_text = || {
        let t = text.trim();
        match t.rfind(' ') {
            Some(sep) if sep > 0 => t[..sep].trim(),
            _ => t,
        }
    };
    let radix = |t: &str, prefix: [&str; 2], radix: u32| {
        let t = t
            .strip_prefix(prefix[0])
            .or_else(|| t.strip_prefix(prefix[1]))
            .unwrap_or(t);
        // Unsigned, so 0x8000000000000000 sets the top bit.
        u64::from_str_radix(t, radix)
            .ok()
            .map(|n| (n as i64).to_string())
    };
    let pi = std::f64::consts::PI;
    match format {
        STRING => None,
        HEX => radix(number_text(), ["0x", "0X"], 16),
        BINARY => radix(number_text(), ["0b", "0B"], 2),
        SEXAGESIMAL => parse_sexagesimal(text).map(|v| v.to_string()),
        SEXAGESIMAL_HMS => parse_sexagesimal(text).map(|v| (v * pi / 12.0).to_string()),
        SEXAGESIMAL_DMS => parse_sexagesimal(text).map(|v| (v * pi / 180.0).to_string()),
        _ => match value {
            Value::Double(_) | Value::Long(_) => {
                let t = number_text();
                if let Some(n) = radix_prefixed(t) {
                    return Some(n);
                }
                match value {
                    Value::Double(_) => t.parse::<f64>().ok().map(|_| t.to_string()),
                    _ => t.parse::<i64>().ok().map(|_| t.to_string()),
                }
            }
            Value::Enum(_) => {
                let t = text.trim();
                labels.iter().position(|l| l == t).map(|i| i.to_string())
            }
            // Not in Phoebus, which then writes the text as typed: the
            // units an array is shown with are not one more element.
            Value::DoubleArray(_) | Value::LongArray(_) if !units.is_empty() => text
                .trim()
                .strip_suffix(units)
                .map(|t| t.trim_end().to_string()),
            _ => None,
        },
    }
}

/// A number typed as `0x…` or `0b…` in a format that is not hex or binary.
fn radix_prefixed(t: &str) -> Option<String> {
    let (digits, radix) = if let Some(h) = t.strip_prefix("0x").or(t.strip_prefix("0X")) {
        (h, 16)
    } else {
        (t.strip_prefix("0b").or(t.strip_prefix("0B"))?, 2)
    };
    i64::from_str_radix(digits, radix)
        .ok()
        .map(|n| n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(format: i64, precision: i64) -> Widget {
        let xml = format!(
            r#"<display><widget type="textentry"><format>{format}</format><precision>{precision}</precision></widget></display>"#
        );
        bob::Display::parse(&xml).unwrap().widgets.remove(0)
    }

    fn state(value: Value, units: &str) -> PvState {
        PvState {
            connected: true,
            value,
            units: units.into(),
            precision: Some(3),
            labels: vec!["Off".into(), "On".into()],
            ..Default::default()
        }
    }

    #[test]
    fn typed_units_are_not_written() {
        let w = widget(0, -1);
        let s = state(Value::Double(5.0), "mm");
        // What the entry shows, and so what an edit starts from.
        assert_eq!(format_value(&w, &s), "5.000 mm");
        assert_eq!(entry_value(&w, Some(&s), "5.5 mm"), "5.5");
        assert_eq!(entry_value(&w, Some(&s), " 5.5 "), "5.5");
        assert_eq!(entry_value(&w, Some(&s), "0x1F mm"), "31");
        assert_eq!(entry_value(&w, Some(&s), "0b101"), "5");
        assert_eq!(entry_value(&w, Some(&s), "1e3"), "1e3");
        // Text that is not a number goes as typed: the IOC says no.
        assert_eq!(entry_value(&w, Some(&s), "abc"), "abc");
        let l = state(Value::Long(1), "cts");
        assert_eq!(entry_value(&w, Some(&l), "42 cts"), "42");
        // An array's units are not one more element.
        let a = state(Value::DoubleArray(vec![1.0, 2.0].into()), "mm");
        assert_eq!(format_value(&w, &a), "[1.000, 2.000] mm");
        assert_eq!(entry_value(&w, Some(&a), "[1, 2.5] mm"), "[1, 2.5]");
        // A string is written as typed.
        let t = state(Value::Str("a b".into()), "");
        assert_eq!(entry_value(&w, Some(&t), "x y"), "x y");
        // An enum label is written as its index.
        let e = state(Value::Enum(0), "");
        assert_eq!(entry_value(&w, Some(&e), "On"), "1");
        assert_eq!(entry_value(&w, Some(&e), "Maybe"), "Maybe");
    }

    #[test]
    fn hex_and_binary_entries_parse_in_their_radix() {
        let s = state(Value::Long(31), "");
        let hex = widget(HEX, -1);
        assert_eq!(format_value(&hex, &s), "0x01F");
        assert_eq!(entry_value(&hex, Some(&s), "0x20"), "32");
        assert_eq!(entry_value(&hex, Some(&s), "ff"), "255");
        assert_eq!(
            entry_value(&hex, Some(&s), "0x8000000000000000"),
            i64::MIN.to_string()
        );
        let bin = widget(BINARY, 0);
        assert_eq!(format_value(&bin, &s), "0b11111");
        assert_eq!(entry_value(&bin, Some(&s), "0b110 units"), "6");
        assert_eq!(entry_value(&bin, Some(&s), "102"), "102");
    }

    #[test]
    fn formats_numbers_as_phoebus() {
        let f = |format: i64, precision: i64, v: f64| {
            format_value(&widget(format, precision), &state(Value::Double(v), ""))
        };
        assert_eq!(f(EXPONENTIAL, 3, 1500.0), "1.500E3");
        assert_eq!(f(ENGINEERING, 2, 12345.0), "12.35E3");
        assert_eq!(f(ENGINEERING, 2, 0.00123), "1.23E-3");
        assert_eq!(f(ENGINEERING, 1, 0.0), "0.0E0");
        assert_eq!(f(COMPACT, 2, 12.345), "12.35");
        assert_eq!(f(COMPACT, 2, 123456.0), "1.23E5");
        assert_eq!(f(SIGNIFICANT, 3, 12.345), "12.3");
        assert_eq!(f(SIGNIFICANT, 3, 123456.0), "1.23e+05");
        assert_eq!(f(SIGNIFICANT, 3, 0.00012345), "0.000123");
        assert_eq!(f(SEXAGESIMAL, 4, 12.5), "12:30:00");
        assert_eq!(f(SEXAGESIMAL, 2, 1.75), "1:45");
        assert_eq!(f(SEXAGESIMAL, 6, 12.504_166_666), "12:30:15.00");
        assert_eq!(f(SEXAGESIMAL_DMS, 2, std::f64::consts::PI), "180:00");
        assert_eq!(f(HEX, 0, 255.9), "0xFF");
        assert_eq!(f(STRING, 0, 65.0), "A");
        let s = state(Value::Double(12.5), "deg");
        let w = widget(SEXAGESIMAL, 4);
        assert_eq!(format_value(&w, &s), "12:30:00 deg");
        assert_eq!(entry_value(&w, Some(&s), "12:30:00"), "12.5");
        assert_eq!(entry_value(&w, Some(&s), "-1:30"), "-1.5");
        // Each element of an array.
        let a = state(Value::LongArray(vec![10, 255].into()), "");
        assert_eq!(format_value(&widget(HEX, 2), &a), "[0x0A, 0xFF]");
    }

    #[test]
    fn enums_show_their_index_in_number_formats() {
        let s = state(Value::Enum(1), "");
        assert_eq!(format_value(&widget(DEFAULT, -1), &s), "On");
        assert_eq!(format_value(&widget(STRING, -1), &s), "On");
        assert_eq!(format_value(&widget(HEX, -1), &s), "1");
        assert_eq!(format_value(&widget(DECIMAL, -1), &s), "1");
    }
}
