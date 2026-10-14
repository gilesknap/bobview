//! A pure-Rust CA IOC serving a subset of the epics-containers
//! example-services BL01T beamline PVs.

mod cam;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asyn_rs::interfaces::motor::AsynMotor;
use epics_base_rs::error::CaResult;
use epics_base_rs::server::database::db_access::DbChannel;
use epics_base_rs::server::device_support::DeviceSupport;
use epics_base_rs::server::scan::ScanOwner;
use epics_ca_rs::server::CaServer;
use motor_rs::poll_loop::PollCommand;
use motor_rs::sim_motor::SimMotor;
use motor_rs::{MotorBuilder, MotorRecord};

const EA_DB: &str = include_str!("db/ea.db");
const MOTOR_DB: &str = include_str!("db/motor.db");
const CAM_DB: &str = include_str!("db/cam.db");
const PLUGIN_DB: &str = include_str!("db/plugin.db");

pub const MOTOR_P: &str = "BL01T-MO-SIMC-01:";
pub const CAM_P: &str = "BL01T-DI-CAM-01";
const AXES: [(&str, &str); 2] = [("M1", "Motor 1"), ("M2", "Motor 2")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Ea,
    Motor,
    Cam,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Ea, Part::Motor, Part::Cam];

    pub fn parse(s: &str) -> Option<Part> {
        match s {
            "ea" => Some(Part::Ea),
            "motor" => Some(Part::Motor),
            "cam" => Some(Part::Cam),
            _ => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Part::Ea => "bl01t:A, bl01t:B, bl01t:SUM",
            Part::Motor => "BL01T-MO-SIMC-01:M1, :M2",
            Part::Cam => "BL01T-DI-CAM-01:DET:, :ARR:, :PVA:",
        }
    }
}

/// A running IOC. Dropping it stops scanning but not the CA server task.
pub struct Ioc {
    pub server: Arc<CaServer>,
    _scan: ScanOwner,
}

fn macros(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Build the IOC for `parts` on `port` (None: EPICS_CA_SERVER_PORT or 5064)
/// and run its startup actions. Call `server.run()` to serve.
pub async fn build(parts: &[Part], port: Option<u16>) -> CaResult<Ioc> {
    let mut b = CaServer::builder();
    if let Some(port) = port {
        b = b.port(port);
    }
    let mut poll_txs = Vec::new();

    if parts.contains(&Part::Ea) {
        b = b.db_string(EA_DB, &HashMap::new())?;
    }
    if parts.contains(&Part::Motor) {
        b = b.register_record_type("motor", || Box::new(MotorRecord::default()));
        for (m, desc) in AXES {
            let sim: Arc<Mutex<dyn AsynMotor>> =
                Arc::new(Mutex::new(SimMotor::new().with_limits(-110.0, 110.0)));
            let setup = MotorBuilder::new(sim)
                .moving_poll_interval(Duration::from_millis(100))
                .idle_poll_interval(Duration::from_secs(1))
                .build();
            let dtyp = format!("simMotor_{m}");
            let ds = Mutex::new(Some(setup.device_support.with_dtyp_name(dtyp.clone())));
            tokio::spawn(setup.poll_loop.run());
            poll_txs.push(setup.poll_cmd_tx);
            b = b.register_device_support(&dtyp, move || {
                let ds = ds.lock().unwrap().take();
                Box::new(ds.expect("one record per sim axis")) as Box<dyn DeviceSupport>
            });
            b = b.db_string(
                MOTOR_DB,
                &macros(&[("P", MOTOR_P), ("M", m), ("DESC", desc)]),
            )?;
        }
    }
    if parts.contains(&Part::Cam) {
        b = b.db_string(CAM_DB, &macros(&[("P", CAM_P), ("R", ":DET:")]))?;
        for r in cam::PLUGINS {
            b = b.db_string(PLUGIN_DB, &macros(&[("P", CAM_P), ("R", r)]))?;
        }
        b = b.db_string(cam::PVA_NAME_DB, &macros(&[("P", CAM_P)]))?;
    }

    let server = Arc::new(b.build().await?);
    let db = server.database().clone();
    // CaServerBuilder stops short of these iocInit steps; CP links need them.
    db.initialize_link_locality().await;
    db.setup_cp_links().await;
    let scan = ScanOwner::start(db.clone());

    if parts.contains(&Part::Ea) {
        // As ioc.yaml: dbpf bl01t:A 2.54, dbpf bl01t:B 2.61
        DbChannel::new(&db, "bl01t:A").put_f64_process(2.54).await?;
        DbChannel::new(&db, "bl01t:B").put_f64_process(2.61).await?;
    }
    for tx in poll_txs {
        let _ = tx.send(PollCommand::StartPolling).await;
    }
    if parts.contains(&Part::Cam) {
        cam::start(&db).await?;
    }

    Ok(Ioc {
        server,
        _scan: scan,
    })
}
