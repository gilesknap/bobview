//! Interop against a real EPICS IOC (softIoc / PVXS) and ca-gateway.
//!
//! Skipped, not failed, unless the environment names PVs to read, so
//! `cargo test --workspace` stays green without EPICS. CI sets them in the
//! `interop` job. Point `EPICS_CA_*` / `EPICS_PVA_*` at the servers as usual.
//!
//! - `BOBVIEW_INTEROP_CA_PV`: a PV served by the IOC over Channel Access
//! - `BOBVIEW_INTEROP_PVA_PV`: a PV served by the IOC over PV Access (PVXS)
//! - `BOBVIEW_INTEROP_GW_PV`: a PV reached through ca-gateway (CA)
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

fn connects(var: &str, scheme: &str) {
    let Some(name) = env(var) else {
        eprintln!("skipped: {var} not set");
        return;
    };
    let pvs = PvManager::new();
    let pv = pvs.subscribe(&format!("{scheme}://{name}"));
    wait_for(&format!("connect to {scheme}://{name}"), || {
        let s = pv.state();
        s.connected && !matches!(s.value, Value::None)
    });
}

#[test]
fn ca_against_real_ioc() {
    connects("BOBVIEW_INTEROP_CA_PV", "ca");
}

#[test]
fn pva_against_real_ioc() {
    connects("BOBVIEW_INTEROP_PVA_PV", "pva");
}

#[test]
fn ca_through_gateway() {
    connects("BOBVIEW_INTEROP_GW_PV", "ca");
}

#[test]
fn ca_write_round_trip() {
    let Some(name) = env("BOBVIEW_INTEROP_WRITE_PV") else {
        eprintln!("skipped: BOBVIEW_INTEROP_WRITE_PV not set");
        return;
    };
    let pvs = PvManager::new();
    let pv = pvs.subscribe(&format!("ca://{name}"));
    wait_for("writable", || {
        let s = pv.state();
        s.connected && s.writable
    });
    pv.write("7");
    wait_for("value 7", || {
        matches!(pv.state().value, Value::Double(v) if v == 7.0)
            || matches!(pv.state().value, Value::Long(7))
    });
}
