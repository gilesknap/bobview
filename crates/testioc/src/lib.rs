//! A pure-Rust CA and PVA IOC serving a subset of the epics-containers
//! example-services BL01T beamline PVs.

mod cam;
pub mod image;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asyn_rs::interfaces::motor::AsynMotor;
use epics_base_rs::error::{CaError, CaResult};
use epics_base_rs::server::database::db_access::DbChannel;
use epics_base_rs::server::device_support::DeviceSupport;
use epics_base_rs::server::records::busy::BusyRecord;
use epics_base_rs::server::scan::ScanOwner;
use epics_ca_rs::server::CaServer;
use epics_pva_rs::server::native_source::PvDatabaseSource;
use epics_pva_rs::server_native::{CompositeSource, PvaServer, PvaServerConfig, SharedSource};
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

/// A running IOC: the CA server, and a PVA server that is already serving.
/// Dropping it stops scanning and PVA but not the CA server task.
pub struct Ioc {
    pub server: Arc<CaServer>,
    pub pva: PvaServer,
    _scan: ScanOwner,
}

impl Ioc {
    /// The bound PVA TCP and UDP (search) ports.
    pub fn pva_ports(&self) -> (u16, u16) {
        (self.pva.tcp_addr().port(), self.pva.config().udp_port)
    }

    /// Serve until either server stops or SIGINT.
    pub async fn run(self) -> Result<(), String> {
        let Ioc { server, pva, _scan } = self;
        tokio::select! {
            r = server.run() => r.map_err(|e| format!("CA: {e}")),
            r = pva.run() => r.map_err(|e| format!("PVA: {e}")),
        }
    }
}

fn macros(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Build the IOC for `parts` and run its startup actions. `ca_port` None
/// means EPICS_CA_SERVER_PORT or 5064; `pva_port` None means the
/// EPICS_PVA(S)_* variables or 5075/5076, Some(p) means TCP p and UDP p+1
/// (0: both ephemeral). PVA serves from here on; call `run()` for CA.
pub async fn build(parts: &[Part], ca_port: Option<u16>, pva_port: Option<u16>) -> CaResult<Ioc> {
    let mut b = CaServer::builder();
    if let Some(port) = ca_port {
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
        b = b.register_record_type("busy", || Box::new(BusyRecord::new()));
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

    // Records over PVA as QSRV serves them, plus the camera's image PV.
    let source = CompositeSource::new();
    let records = Arc::new(PvDatabaseSource::new(db.clone()));
    source
        .add_source("records", records, 0)
        .map_err(CaError::InvalidValue)?;
    if parts.contains(&Part::Cam) {
        let output = image::open_pv();
        let shared = Arc::new(SharedSource::new());
        shared.add(format!("{CAM_P}:PVA:OUTPUT"), output.clone());
        source
            .add_source("cam", shared, 1)
            .map_err(CaError::InvalidValue)?;
        cam::start(&db, output).await?;
    }
    let mut config = PvaServerConfig::default().with_env();
    if let Some(port) = pva_port {
        config.tcp_port = port;
        config.udp_port = if port == 0 { 0 } else { port + 1 };
    }
    let pva = PvaServer::start(source, config).map_err(|e| CaError::InvalidValue(e.to_string()))?;

    Ok(Ioc {
        server,
        pva,
        _scan: scan,
    })
}
