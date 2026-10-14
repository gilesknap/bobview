//! A simulated simDetector driver loop: the counters and state a detector
//! screen shows, and a simulated image posted to the NDPluginPVA output.

use std::time::{Duration, Instant};

use epics_base_rs::error::CaResult;
use epics_base_rs::server::database::PvDatabase;
use epics_base_rs::server::database::db_access::DbChannel;
use epics_base_rs::types::EpicsValue;
use epics_pva_rs::server_native::SharedPV;

use crate::CAM_P;
use crate::image::{self, HEIGHT, ImageSim, WIDTH};

pub const PLUGINS: [&str; 2] = [":ARR:", ":PVA:"];

pub const PVA_NAME_DB: &str = r#"
record(stringin, "$(P):PVA:PvName_RBV") {
    field(VAL, "$(P):PVA:OUTPUT")
}
"#;

const DIMS: [i32; 2] = [WIDTH as i32, HEIGHT as i32];
const TICK: Duration = Duration::from_millis(10);

fn pv(r: &str, name: &str) -> String {
    format!("{CAM_P}{r}{name}")
}

fn det(name: &str) -> String {
    pv(":DET:", name)
}

fn get(db: &PvDatabase, name: &str) -> f64 {
    db.get_pv(name).ok().and_then(|v| v.to_f64()).unwrap_or(0.0)
}

/// Write VAL as a CA put would (dbPutField): clears UDF and processes the
/// record, so its alarm severity is recomputed.
async fn post(db: &PvDatabase, name: &str, value: EpicsValue) {
    if let Err(e) = db
        .put_record_field_from_ca_no_notify(name, "VAL", value)
        .await
    {
        eprintln!("testioc: post {name}: {e}");
    }
}

async fn post_long(db: &PvDatabase, name: &str, v: i32) {
    post(db, name, EpicsValue::Long(v)).await
}

async fn set_state(db: &PvDatabase, acquiring: bool, msg: &str) {
    post(db, &det("Acquire_RBV"), EpicsValue::Enum(acquiring as u16)).await;
    post(
        db,
        &det("DetectorState_RBV"),
        EpicsValue::Enum(acquiring as u16),
    )
    .await;
    post(
        db,
        &det("StatusMessage_RBV"),
        EpicsValue::String(msg.into()),
    )
    .await;
}

/// Apply the ioc.yaml startup puts and start the driver loop.
pub async fn start(db: &PvDatabase, output: SharedPV) -> CaResult<()> {
    let dims = EpicsValue::LongArray(DIMS.to_vec().into());
    for r in PLUGINS {
        post_long(db, &pv(r, "NDimensions"), 2).await;
        post_long(db, &pv(r, "NDimensions_RBV"), 2).await;
        post(db, &pv(r, "Dimensions"), dims.clone()).await;
        post(db, &pv(r, "Dimensions_RBV"), dims.clone()).await;
        DbChannel::new(db, &pv(r, "EnableCallbacks"))
            .put_i16_process(1)
            .await?;
    }
    DbChannel::new(db, &det("AcquireTime"))
        .put_f64_process(0.1)
        .await?;
    DbChannel::new(db, &det("Acquire"))
        .put_i16_process(1)
        .await?;
    tokio::spawn(run(db.clone(), output));
    Ok(())
}

/// A counter with a writable setpoint: a put to the setpoint resets it.
struct Counter {
    sp: String,
    rbv: String,
    last_sp: i32,
    value: i32,
}

impl Counter {
    fn new(r: &str) -> Self {
        Counter {
            sp: pv(r, "ArrayCounter"),
            rbv: pv(r, "ArrayCounter_RBV"),
            last_sp: 0,
            value: 0,
        }
    }

    async fn check_reset(&mut self, db: &PvDatabase) {
        let sp = get(db, &self.sp) as i32;
        if sp != self.last_sp {
            self.last_sp = sp;
            self.value = sp;
            post_long(db, &self.rbv, sp).await;
        }
    }

    async fn bump(&mut self, db: &PvDatabase) {
        self.value += 1;
        post_long(db, &self.rbv, self.value).await;
        // Track the count in the setpoint, unposted, so a put of 0 always
        // reads as a change.
        let _ = db.put_pv(&self.sp, EpicsValue::Long(self.value)).await;
        self.last_sp = self.value;
    }
}

async fn run(db: PvDatabase, output: SharedPV) {
    let mut sim = ImageSim::new();
    let mut acquiring = false;
    let mut images = 0;
    let mut unique_id = 0;
    let mut next_frame = Instant::now();
    let mut det_counter = Counter::new(":DET:");
    let mut plugin_counters = PLUGINS.map(Counter::new);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;
        let now = Instant::now();
        let period = Duration::from_secs_f64(
            get(&db, &det("AcquireTime")).max(get(&db, &det("AcquirePeriod"))),
        )
        .max(TICK);

        det_counter.check_reset(&db).await;
        for c in &mut plugin_counters {
            c.check_reset(&db).await;
        }

        let want = get(&db, &det("Acquire")) != 0.0;
        if want && !acquiring {
            acquiring = true;
            images = 0;
            next_frame = now + period;
            post_long(&db, &det("NumImagesCounter_RBV"), 0).await;
            set_state(&db, true, "Acquiring data").await;
        } else if !want && acquiring {
            acquiring = false;
            set_state(&db, false, "Acquisition aborted").await;
        }
        if !acquiring || now < next_frame {
            continue;
        }

        next_frame += period;
        if next_frame < now {
            next_frame = now + period;
        }
        images += 1;
        unique_id += 1;
        det_counter.bump(&db).await;
        post_long(&db, &det("UniqueId_RBV"), unique_id).await;
        post_long(&db, &det("NumImagesCounter_RBV"), images).await;
        if get(&db, &det("ArrayCallbacks")) != 0.0 {
            for (r, c) in PLUGINS.iter().zip(&mut plugin_counters) {
                if get(&db, &pv(r, "EnableCallbacks")) != 0.0 {
                    c.bump(&db).await;
                    post_long(&db, &pv(r, "UniqueId_RBV"), unique_id).await;
                    if *r == ":PVA:" {
                        image::post(&output, sim.frame(unique_id as u32), unique_id);
                    }
                }
            }
        }

        let target = match get(&db, &det("ImageMode")) as i32 {
            0 => 1,
            1 => (get(&db, &det("NumImages")) as i32).max(1),
            _ => i32::MAX,
        };
        if images >= target {
            acquiring = false;
            post(&db, &det("Acquire"), EpicsValue::Enum(0)).await;
            set_state(&db, false, "Acquisition finished").await;
        }
    }
}
