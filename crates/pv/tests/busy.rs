//! Writes to an areaDetector `Acquire` busy record against the test IOC.

use std::time::{Duration, Instant};

use pv::{PvManager, Value};
use testioc::{Part, build};

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A put-callback on a busy record completes only when it returns to 0, and
/// the IOC queues any further put-callback on the record behind it: if writes
/// waited for completion, Stop could never get past Start.
#[test]
fn stop_reaches_a_busy_acquire() {
    let port = 30000 + (std::process::id() % 10000) as u16;
    // Safety: set before any other thread exists.
    unsafe {
        std::env::set_var("EPICS_CA_ADDR_LIST", "127.0.0.1");
        std::env::set_var("EPICS_CA_AUTO_ADDR_LIST", "NO");
        std::env::set_var("EPICS_CA_SERVER_PORT", port.to_string());
    }
    let ioc_rt = tokio::runtime::Runtime::new().unwrap();
    ioc_rt.block_on(async {
        let ioc = build(&[Part::Cam], Some(port), Some(0)).await.unwrap();
        tokio::spawn(ioc.run());
    });

    let pvs = PvManager::new();
    let acquire = pvs.subscribe("ca://BL01T-DI-CAM-01:DET:Acquire");
    let rbv = pvs.subscribe("ca://BL01T-DI-CAM-01:DET:Acquire_RBV");
    let is = |pv: &pv::Pv, want: u16| {
        let s = pv.state();
        s.connected && s.writable && matches!(s.value, Value::Enum(i) if i == want)
    };
    // The test IOC starts acquiring by itself; stop it first.
    wait_for("connect", || is(&acquire, 1) && rbv.state().connected);
    acquire.write("Done");
    wait_for("idle", || is(&acquire, 0) && is(&rbv, 0));

    acquire.write("Acquire");
    wait_for("acquiring", || is(&rbv, 1));
    acquire.write("Done");
    wait_for("stopped", || is(&acquire, 0) && is(&rbv, 0));
}
