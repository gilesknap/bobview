//! Long strings in char waveforms, as areaDetector's `FilePath`, against
//! the test IOC over CA and PVA.

use std::time::{Duration, Instant};

use pv::{Pv, PvManager};
use testioc::{Part, build};

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn reads(pv: &Pv, want: &str) {
    wait_for(&format!("{} to read {want:?}", pv.name()), || {
        let s = pv.state();
        s.connected && s.value.as_long_string().as_deref() == Some(want)
    });
}

#[test]
fn char_waveforms_take_long_strings() {
    // Three ports (CA, PVA, PVA broadcast), below the dynamic range that
    // Windows reserves blocks of.
    let port = 20000 + (std::process::id() % 5000) as u16 * 3;
    let pva_port = port + 1;
    // Safety: set before any other thread exists.
    unsafe {
        std::env::set_var("EPICS_CA_ADDR_LIST", "127.0.0.1");
        std::env::set_var("EPICS_CA_AUTO_ADDR_LIST", "NO");
        std::env::set_var("EPICS_CA_SERVER_PORT", port.to_string());
        std::env::set_var("EPICS_PVA_ADDR_LIST", "127.0.0.1");
        std::env::set_var("EPICS_PVA_AUTO_ADDR_LIST", "NO");
        std::env::set_var("EPICS_PVA_SERVER_PORT", pva_port.to_string());
        std::env::set_var("EPICS_PVA_BROADCAST_PORT", (pva_port + 1).to_string());
    }
    let ioc_rt = tokio::runtime::Runtime::new().unwrap();
    ioc_rt.block_on(async {
        let ioc = build(&[Part::Cam], Some(port), Some(pva_port))
            .await
            .unwrap();
        tokio::spawn(ioc.run());
    });

    let pvs = PvManager::new();
    let name = "BL01T-DI-CAM-01:DET:FilePath";
    let ca = pvs.subscribe(&format!("ca://{name}"));
    let pva = pvs.subscribe(&format!("pva://{name}"));
    wait_for("connect", || {
        ca.state().connected && ca.state().writable && pva.state().connected
    });

    ca.write_long_string("/data/2026/caf\u{e9}");
    reads(&ca, "/data/2026/caf\u{e9}");
    reads(&pva, "/data/2026/caf\u{e9}");

    // Cut to the 256 elements, leaving room for the NUL.
    let long = "x".repeat(300);
    ca.write_long_string(&long);
    reads(&ca, &long[..255]);

    // A shorter one ends at its NUL, though the old text is still behind it.
    ca.write_long_string("/tmp");
    reads(&ca, "/tmp");

    // As Phoebus's CA layer does, plain text that is not numbers is one too.
    ca.write("/plain");
    reads(&ca, "/plain");

    // An empty one leaves the array holding just the NUL, which CA sends as
    // a scalar: it stays an array, and a long string can still be written.
    ca.write_long_string("");
    reads(&ca, "");
    assert_eq!(ca.state().value, pv::Value::LongArray(vec![0].into()));
    ca.write_long_string("/after/empty");
    reads(&ca, "/after/empty");

    // Phoebus splits a list of numbers on commas only, and its numbers are
    // finite, so these are text too.
    ca.write("nan");
    reads(&ca, "nan");
    ca.write("1 2");
    reads(&ca, "1 2");
    assert_eq!(ca.state().write_error, None);

    pva.write_long_string("/over/pva");
    reads(&pva, "/over/pva");
    reads(&ca, "/over/pva");
    assert_eq!(pva.state().write_error, None);
}
