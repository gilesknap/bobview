//! The context menu on widgets with a PV (right-click, or long-press on a
//! touch screen) and the PV details window it can open.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Ui};
use pv::{Pv, PvName, PvState, Scheme, Severity, Value};

use crate::screen::Node;
use crate::widgets::Request;

/// What the PV details window shows: the PV, read live, and where the
/// widget that uses it came from.
pub struct Details {
    pub pv: Pv,
    pub widget: String,
    pub file: Option<PathBuf>,
}

/// Fill the context menu for a widget's PV.
pub fn menu(ui: &mut Ui, node: &Node, state: Option<&PvState>, requests: &mut Vec<Request>) {
    let name = node.pv_name.as_str();
    let value = state.filter(|s| s.connected).map(|s| s.format(None, true));

    if ui.button("Copy PV name").clicked() {
        ui.ctx().copy_text(name.to_string());
    }
    if let (Some(state), Some(value)) = (state, &value) {
        if ui.button("Copy value").clicked() {
            ui.ctx().copy_text(value.clone());
        }
        if ui.button("Copy name and value").clicked() {
            ui.ctx().copy_text(format!("{name} = {value}"));
        }
        if ui.button("Copy name, value, alarm and time").clicked() {
            ui.ctx().copy_text(full_line(name, state));
        }
    }
    if let Some(cmd) = cli_command(&PvName::parse(name))
        && ui.button(format!("Copy \"{cmd}\"")).clicked()
    {
        ui.ctx().copy_text(cmd);
    }
    if let Some(pv) = &node.pv {
        ui.separator();
        if ui.button("PV details…").clicked() {
            requests.push(Request::Details(Details {
                pv: pv.clone(),
                widget: widget_label(node),
                file: node.base.clone(),
            }));
        }
    }
}

/// The body of the PV details window.
pub fn details_ui(ui: &mut Ui, details: &Details) {
    let rows = details_rows(details, &details.pv.state());
    egui::Grid::new("pv-details")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (key, val) in &rows {
                ui.label(*key);
                ui.label(val);
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    if ui.button("Copy all").clicked() {
        let text: Vec<String> = rows.iter().map(|(k, v)| format!("{k}: {v}")).collect();
        ui.ctx().copy_text(text.join("\n"));
    }
}

/// What the details window shows, as (label, value) rows.
fn details_rows(details: &Details, s: &PvState) -> Vec<(&'static str, String)> {
    let name = details.pv.name();
    let mut rows = vec![
        ("Name", name.to_string()),
        ("Protocol", protocol(name.scheme).into()),
        (
            "Connection",
            match (&s.connected, &s.error) {
                (true, _) => "connected".into(),
                (false, Some(e)) => format!("disconnected: {e}"),
                (false, None) => "disconnected".into(),
            },
        ),
    ];
    if s.connected {
        rows.push(("Value", s.format(None, true)));
        rows.push(("Type", value_type(&s.value)));
        rows.push((
            "Alarm",
            format!("{} (status {})", severity_name(s.severity), s.status),
        ));
        if let Some(t) = s.timestamp {
            rows.push(("Time", format_time(t)));
        }
        if !s.units.is_empty() {
            rows.push(("Units", s.units.clone()));
        }
        if let Some(p) = s.precision {
            rows.push(("Precision", p.to_string()));
        }
        if let Some((lo, hi)) = s.display_range {
            rows.push(("Display limits", format!("{lo} to {hi}")));
        }
        if let Some((lo, hi)) = s.control_range {
            rows.push(("Control limits", format!("{lo} to {hi}")));
        }
        if !s.labels.is_empty() {
            rows.push(("States", s.labels.join(", ")));
        }
        rows.push(("Writable", if s.writable { "yes" } else { "no" }.into()));
    }
    rows.push(("Widget", details.widget.clone()));
    if let Some(file) = &details.file {
        rows.push(("Display", file.display().to_string()));
    }
    rows
}

/// A one-line summary for bug reports and logbooks.
fn full_line(name: &str, s: &PvState) -> String {
    let mut line = format!(
        "{name} = {} {}",
        s.format(None, true),
        severity_name(s.severity)
    );
    if let Some(t) = s.timestamp {
        line.push(' ');
        line.push_str(&format_time(t));
    }
    line
}

/// A shell command that reads the PV, for the protocols that have one.
fn cli_command(name: &PvName) -> Option<String> {
    match name.scheme {
        Scheme::Ca => Some(format!("caget {}", name.pv)),
        Scheme::Pva => Some(format!("pvget {}", name.pv)),
        Scheme::Loc | Scheme::Sim => None,
    }
}

fn widget_label(node: &Node) -> String {
    match node.w.text("name") {
        Some(name) if !name.is_empty() => format!("{} \"{name}\"", node.w.kind),
        _ => node.w.kind.clone(),
    }
}

fn protocol(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Ca => "Channel Access",
        Scheme::Pva => "PV Access",
        Scheme::Loc => "local",
        Scheme::Sim => "simulated",
    }
}

fn severity_name(sevr: Severity) -> &'static str {
    match sevr {
        Severity::NoAlarm => "NO_ALARM",
        Severity::Minor => "MINOR",
        Severity::Major => "MAJOR",
        Severity::Invalid => "INVALID",
    }
}

fn value_type(v: &Value) -> String {
    match v {
        Value::None => "none".into(),
        Value::Double(_) => "double".into(),
        Value::Long(_) => "integer".into(),
        Value::Str(_) => "string".into(),
        Value::Enum(_) => "enum".into(),
        Value::DoubleArray(a) => format!("double[{}]", a.len()),
        Value::LongArray(a) => format!("integer[{}]", a.len()),
        Value::StrArray(a) => format!("string[{}]", a.len()),
        Value::Image(img) => format!("image {}x{}", img.width, img.height),
    }
}

/// ISO 8601 UTC with milliseconds, e.g. `2026-10-14T19:08:33.123Z`.
fn format_time(t: SystemTime) -> String {
    let Ok(d) = t.duration_since(UNIX_EPOCH) else {
        return "before 1970".into();
    };
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, day) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// Days since 1970-01-01 to (year, month, day), after Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_utc_time() {
        let t = UNIX_EPOCH + Duration::from_millis(1_792_004_913_123);
        assert_eq!(format_time(t), "2026-10-14T19:08:33.123Z");
        assert_eq!(format_time(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // a leap day
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(format_time(t), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn cli_commands_by_protocol() {
        let cmd = |n| cli_command(&PvName::parse(n));
        assert_eq!(cmd("BL01T:X").as_deref(), Some("caget BL01T:X"));
        assert_eq!(cmd("ca://BL01T:X").as_deref(), Some("caget BL01T:X"));
        assert_eq!(cmd("pva://BL01T:IMG").as_deref(), Some("pvget BL01T:IMG"));
        assert_eq!(cmd("loc://x<VDouble>(3)"), None);
        assert_eq!(cmd("sim://sine"), None);
    }

    #[test]
    fn full_line_has_value_alarm_and_time() {
        let s = PvState {
            connected: true,
            value: Value::Double(2.5),
            units: "mm".into(),
            precision: Some(2),
            severity: Severity::Minor,
            timestamp: Some(UNIX_EPOCH),
            ..Default::default()
        };
        assert_eq!(
            full_line("BL01T:M1", &s),
            "BL01T:M1 = 2.50 mm MINOR 1970-01-01T00:00:00.000Z"
        );
    }
}
