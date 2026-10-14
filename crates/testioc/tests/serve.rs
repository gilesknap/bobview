use std::time::Duration;

use epics_ca_rs::client::CaClient;
use testioc::{Part, build};

async fn get_f64(client: &CaClient, pv: &str) -> f64 {
    let (_, v) = client.caget(pv).await.expect(pv);
    v.to_f64().expect(pv)
}

#[test]
fn serves_the_example_pvs() {
    let port = 20000 + (std::process::id() % 10000) as u16;
    // Safety: set before any other thread exists.
    unsafe {
        std::env::set_var("EPICS_CA_ADDR_LIST", "127.0.0.1");
        std::env::set_var("EPICS_CA_AUTO_ADDR_LIST", "NO");
        std::env::set_var("EPICS_CA_SERVER_PORT", port.to_string());
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ioc = build(&Part::ALL, Some(port)).await.unwrap();
        let server = ioc.server.clone();
        tokio::spawn(async move { server.run().await });

        let client = CaClient::new().await.unwrap();
        let sum = get_f64(&client, "bl01t:SUM").await;
        assert!((sum - 5.15).abs() < 1e-9, "SUM {sum}");

        let m1 = "BL01T-MO-SIMC-01:M1";
        client.caput(m1, "0.5").await.unwrap();
        let mut rbv = 0.0;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            rbv = get_f64(&client, &format!("{m1}.RBV")).await;
            if (rbv - 0.5).abs() < 1e-6 {
                break;
            }
        }
        assert!((rbv - 0.5).abs() < 1e-6, "RBV {rbv}");

        let counter = "BL01T-DI-CAM-01:DET:ArrayCounter_RBV";
        let a = get_f64(&client, counter).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let b = get_f64(&client, counter).await;
        assert!(b > a, "ArrayCounter_RBV {a} -> {b}");
    });
}
