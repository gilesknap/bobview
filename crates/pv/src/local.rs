//! `loc://` PVs: values held in this process, shared by every widget using the name.

use std::sync::Arc;

use crate::name::split_args;
use crate::{PvName, Shared, Value, Writer};

/// Initialise a local PV from its `<VType>(initial)` and wire up writes.
pub(crate) fn create(name: &PvName, shared: Arc<Shared>) -> Arc<Shared> {
    let args: Vec<String> = name.args().map(split_args).unwrap_or_default();
    let vtype = name.vtype().unwrap_or("");
    shared.update(|s| {
        s.connected = true;
        s.writable = true;
        if vtype == "VEnum" {
            s.value = Value::Enum(args.first().and_then(|a| a.parse().ok()).unwrap_or(0));
            s.labels = args.iter().skip(1).map(|a| unquote(a)).collect();
        } else {
            s.value = initial_value(vtype, &args);
        }
    });

    shared.set_writer(Writer::Local);
    shared
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

fn initial_value(vtype: &str, args: &[String]) -> Value {
    let quoted = args.first().is_some_and(|a| a.starts_with('"'));
    match (vtype, args.len()) {
        (_, 0) => match vtype {
            "VString" => Value::Str(String::new()),
            "VLong" => Value::Long(0),
            _ => Value::Double(0.0),
        },
        ("VString", _) => Value::Str(unquote(&args[0])),
        ("VStringArray", _) => Value::StrArray(args.iter().map(|a| unquote(a)).collect()),
        _ if quoted && args.len() == 1 => Value::Str(unquote(&args[0])),
        _ if quoted => Value::StrArray(args.iter().map(|a| unquote(a)).collect()),
        ("VLong", 1) => Value::Long(args[0].parse().unwrap_or(0)),
        ("VLongArray", _) => {
            Value::LongArray(args.iter().map(|a| a.parse().unwrap_or(0)).collect())
        }
        (_, 1) => Value::Double(args[0].parse().unwrap_or(0.0)),
        _ => Value::DoubleArray(args.iter().map(|a| a.parse().unwrap_or(0.0)).collect()),
    }
}

/// The new value after writing `text` to a PV holding `old`.
pub(crate) fn apply(old: &Value, labels: &[String], text: &str) -> Value {
    let t = text.trim();
    match old {
        Value::Str(_) => Value::Str(text.to_string()),
        Value::Long(v) => Value::Long(
            t.parse()
                .or_else(|_| t.parse::<f64>().map(|f| f as i64))
                .unwrap_or(*v),
        ),
        Value::Enum(i) => Value::Enum(
            labels
                .iter()
                .position(|l| l == t)
                .map(|p| p as u16)
                .or_else(|| t.parse::<f64>().ok().map(|f| f as u16))
                .unwrap_or(*i),
        ),
        Value::Double(v) => Value::Double(t.parse().unwrap_or(*v)),
        Value::DoubleArray(_) => {
            Value::DoubleArray(t.split([' ', ',']).filter_map(|x| x.parse().ok()).collect())
        }
        Value::LongArray(_) => {
            Value::LongArray(t.split([' ', ',']).filter_map(|x| x.parse().ok()).collect())
        }
        Value::StrArray(_) => Value::StrArray(Arc::from(vec![text.to_string()])),
        Value::Image(_) => old.clone(),
        Value::None => t
            .parse()
            .map(Value::Double)
            .unwrap_or_else(|_| Value::Str(text.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_values() {
        let v = |s: &str| {
            let n = PvName::parse(s);
            initial_value(
                n.vtype().unwrap_or(""),
                &n.args().map(split_args).unwrap_or_default(),
            )
        };
        assert_eq!(v("loc://a(3.5)"), Value::Double(3.5));
        assert_eq!(v("loc://a<VLong>(7)"), Value::Long(7));
        assert_eq!(v("loc://a(\"hi\")"), Value::Str("hi".into()));
        assert_eq!(
            v("loc://a(1, 2)"),
            Value::DoubleArray(Arc::from(vec![1.0, 2.0]))
        );
        assert_eq!(v("loc://a"), Value::Double(0.0));
    }

    #[test]
    fn writes_convert() {
        let labels = vec!["Off".to_string(), "On".to_string()];
        assert_eq!(apply(&Value::Enum(0), &labels, "On"), Value::Enum(1));
        assert_eq!(apply(&Value::Enum(1), &labels, "0"), Value::Enum(0));
        assert_eq!(apply(&Value::Long(1), &[], "4"), Value::Long(4));
        assert_eq!(apply(&Value::Double(1.0), &[], "x"), Value::Double(1.0));
    }
}
