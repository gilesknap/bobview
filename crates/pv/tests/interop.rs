//! Interop against a real EPICS IOC (softIoc / PVXS) and ca-gateway.
//!
//! Skipped, not failed, unless the environment names PVs to read, so
//! `cargo test --workspace` stays green without EPICS. CI sets them in the
//! `interop` job. Point `EPICS_CA_*` / `EPICS_PVA_*` at the servers as usual.
//!
//! - `BOBVIEW_INTEROP_CA_PV`: a PV served by the IOC over Channel Access
//! - `BOBVIEW_INTEROP_PVA_PV`: a PV served by the IOC over PV Access (PVXS)
//! - `BOBVIEW_INTEROP_GW_PV`: a PV reached through ca-gateway (CA)
//! - `BOBVIEW_INTEROP_REQUIRE=1`: panic instead of skipping when a variable is
//!   missing, so a typo in the CI environment cannot go green with nothing run
//! - `BOBVIEW_INTEROP_WRITE_PV`: optional writable numeric PV; set to 7 and read back

use std::time::{Duration, Instant};

use pv::{PvManager, Value};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The value of `var`, or None if the test should be skipped.
fn pv_var(var: &str) -> Option<String> {
    let v = env(var);
    if v.is_none() {
        assert!(
            env("BOBVIEW_INTEROP_REQUIRE").is_none(),
            "{var} is not set and BOBVIEW_INTEROP_REQUIRE is"
        );
        eprintln!("skipped: {var} not set");
    }
    v
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Double(d) => Some(*d),
        Value::Long(l) => Some(*l as f64),
        _ => None,
    }
}

/// Connects to the PV and waits for it to hold `expected`.
fn reads(var: &str, scheme: &str, expected: f64) {
    let Some(name) = pv_var(var) else {
        return;
    };
    let pvs = PvManager::new();
    let pv = pvs.subscribe(&format!("{scheme}://{name}"));
    wait_for(&format!("{scheme}://{name} to read {expected}"), || {
        let s = pv.state();
        s.connected && number(&s.value) == Some(expected)
    });
}

#[test]
fn ca_against_real_ioc() {
    reads("BOBVIEW_INTEROP_CA_PV", "ca", 1.25);
}

#[test]
fn pva_against_real_ioc() {
    reads("BOBVIEW_INTEROP_PVA_PV", "pva", 1.25);
}

#[test]
fn ca_through_gateway() {
    reads("BOBVIEW_INTEROP_GW_PV", "ca", 42.0);
}

#[test]
fn ca_write_round_trip() {
    let Some(name) = pv_var("BOBVIEW_INTEROP_WRITE_PV") else {
        return;
    };
    let pvs = PvManager::new();
    let pv = pvs.subscribe(&format!("ca://{name}"));
    wait_for("writable", || {
        let s = pv.state();
        s.connected && s.writable
    });
    pv.write("7");
    wait_for("value 7", || number(&pv.state().value) == Some(7.0));
}
