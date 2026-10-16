//! A CA write to a PV that has yet to connect waits for it, at most
//! `WRITE_CONNECT` (5 s), without holding up the PV's other events; one
//! that never connects fails, with the writes queued behind it.

use std::time::{Duration, Instant};

use pv::PvManager;
use testioc::{Part, build};

#[test]
fn a_write_waits_for_its_pv_then_gives_up() {
    let port = 30000 + ((std::process::id() + 7000) % 10000) as u16;
    // Safety: set before any other thread exists.
    unsafe {
        std::env::set_var("EPICS_CA_ADDR_LIST", "127.0.0.1");
        std::env::set_var("EPICS_CA_AUTO_ADDR_LIST", "NO");
        std::env::set_var("EPICS_CA_SERVER_PORT", port.to_string());
    }
    let pvs = PvManager::new();

    // Never there: the write waits; 16 more queue behind it (one more is
    // refused at once), and all fail together once the first has waited
    // 5 s, each as a failed write.
    let missing = pvs.subscribe("ca://BL01T-NOWHERE:NOTHING");
    let start = Instant::now();
    missing.write("first");
    std::thread::sleep(Duration::from_millis(200));
    for i in 0..17 {
        missing.write(i.to_string());
    }
    let (n, text) = pvs.notice();
    assert_eq!(n, 1, "{text}");
    assert!(
        text.contains("not writing '16', 16 writes still waiting"),
        "{text}"
    );
    while pvs.notice().0 < 18 {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            pvs.notice()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(4500), "{waited:?}");
    let (_, text) = pvs.notice();
    assert!(text.contains("write '15' failed: not connected"), "{text}");
    assert!(missing.state().write_error.is_some());

    // There a moment after the write: it waits, then lands. The IOC
    // starts only after the write is queued.
    let name = "ca://BL01T-DI-CAM-01:DET:NumImages";
    let pv = pvs.subscribe(name);
    pv.write("9");
    std::thread::sleep(Duration::from_millis(500));
    let ioc_rt = tokio::runtime::Runtime::new().unwrap();
    ioc_rt.block_on(async {
        let ioc = build(&[Part::Cam], Some(port), Some(0)).await.unwrap();
        tokio::spawn(ioc.run());
    });
    let start = Instant::now();
    loop {
        let s = pv.state();
        if s.connected && s.value.as_f64() == Some(9.0) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "not written: {s:?}, {:?}",
            pvs.notice()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(pv.state().write_error, None);

    // Connected, then gone (the IOC stops): refused at once, not queued.
    drop(ioc_rt);
    let start = Instant::now();
    while !pv.dropped() {
        assert!(start.elapsed() < Duration::from_secs(60), "never dropped");
        std::thread::sleep(Duration::from_millis(100));
    }
    let before = pvs.notice().0;
    pv.write("10");
    let (n, text) = pvs.notice();
    assert_eq!(n, before + 1);
    assert!(text.contains("not writing '10', not connected"), "{text}");
}
