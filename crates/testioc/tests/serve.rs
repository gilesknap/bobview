use std::time::Duration;

use epics_ca_rs::client::CaClient;
use epics_pva_rs::client_native::context::PvaClient;
use epics_pva_rs::pvdata::{PvField, ScalarValue};
use testioc::{Part, build};

async fn get_f64(client: &CaClient, pv: &str) -> f64 {
    let (_, v) = client.caget(pv).await.expect(pv);
    v.to_f64().expect(pv)
}

fn field<'a>(v: &'a PvField, name: &str) -> &'a PvField {
    match v {
        PvField::Structure(s) => s.get_field(name).expect(name),
        _ => panic!("{name}: not a structure: {v:?}"),
    }
}

fn array_len(v: &PvField) -> usize {
    match v {
        PvField::ScalarArrayTyped(a) => a.len(),
        PvField::ScalarArray(a) => a.len(),
        _ => panic!("not an array: {v:?}"),
    }
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
        let ioc = build(&Part::ALL, Some(port), Some(0)).await.unwrap();
        let (pva_port, _) = ioc.pva_ports();
        tokio::spawn(ioc.run());

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

        // The same records over PVA, and an image frame.
        let pva = PvaClient::builder()
            .server_addr(([127, 0, 0, 1], pva_port).into())
            .timeout(Duration::from_secs(5))
            .build();
        let sum = pva.pvget("bl01t:SUM").await.unwrap();
        let PvField::Structure(s) = &sum else {
            panic!("SUM {sum:?}")
        };
        assert_eq!(s.struct_id, "epics:nt/NTScalar:1.0");
        match field(&sum, "value") {
            PvField::Scalar(ScalarValue::Double(v)) => assert!((v - 5.15).abs() < 1e-9),
            v => panic!("SUM value {v:?}"),
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            pva.pvmonitor("BL01T-DI-CAM-01:PVA:OUTPUT", move |v| {
                let _ = tx.send(v.clone());
            })
            .await
        });
        let frame = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v = rx.recv().await.expect("monitor ended");
                if let PvField::Union { variant_name, .. } = field(&v, "value")
                    && variant_name == "byteValue"
                {
                    return v;
                }
            }
        })
        .await
        .expect("an image frame");
        let PvField::Union { value, .. } = field(&frame, "value") else {
            unreachable!()
        };
        assert_eq!(array_len(value), 1024 * 1024);
    });
}
