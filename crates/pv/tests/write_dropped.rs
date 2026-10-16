//! A write made on a PV subscribed just for it, and dropped straight after,
//! as a `write_pv` action to a PV it did not connect in advance does.

use std::time::{Duration, Instant};

use pv::PvManager;
use testioc::{Part, build};

#[test]
fn a_write_outlives_its_pv() {
    let port = 30000 + ((std::process::id() + 5000) % 10000) as u16;
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
    let name = "ca://BL01T-DI-CAM-01:DET:NumImages";
    // Not connected yet: the write waits for the channel, and the task
    // serving it outlives the handle until it is sent.
    pvs.subscribe(name).write("7");
    // Nothing refused it for want of a connection.
    assert_eq!(pvs.notice().0, 0, "{}", pvs.notice().1);

    std::thread::sleep(Duration::from_millis(100));
    let pv = pvs.subscribe(name);
    let start = Instant::now();
    loop {
        let s = pv.state();
        if s.connected && s.value.as_f64() == Some(7.0) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "not written: {s:?}, {:?}",
            pvs.notice()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}
