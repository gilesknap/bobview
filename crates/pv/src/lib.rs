//! PV access for bobview.
//!
//! Widgets ask a [`PvManager`] for a [`Pv`] by name and read its latest
//! [`PvState`] each frame. The manager owns a tokio runtime on which one task
//! per PV talks to the backend chosen by the name's prefix:
//!
//! | prefix    | backend                                 |
//! |-----------|-----------------------------------------|
//! | `ca://`   | Channel Access (the default)            |
//! | `pva://`  | PV Access                               |
//! | `loc://`  | local PVs held in this process          |
//! | `sim://`  | simulated signals (sine, ramp, noise …) |
//!
//! In a browser (wasm32) there is no runtime: `sim://` runs on the page's
//! event loop, and `ca://` and `pva://` PVs come over a PV Web Socket
//! ([`set_pvws`]).

#[cfg(not(target_arch = "wasm32"))]
mod ca;
mod local;
mod name;
#[cfg(not(target_arch = "wasm32"))]
mod pva;
#[cfg(target_arch = "wasm32")]
mod pvws;
mod sim;
mod value;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;

pub use name::{PvName, Scheme};
#[cfg(target_arch = "wasm32")]
pub use pvws::set_pvws;
pub use value::{Image, ImageData, PvState, Severity, Value, split_array, split_strings};

/// Called whenever any PV changes, so a UI can schedule a repaint.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Where writes to a PV go.
pub(crate) enum Writer {
    /// To the backend task, which converts and sends them.
    #[cfg(not(target_arch = "wasm32"))]
    Channel(mpsc::UnboundedSender<String>),
    /// Straight into the local value.
    Local,
    /// Over the PV Web Socket, which converts them.
    #[cfg(target_arch = "wasm32")]
    Pvws(String),
}

/// State shared between a [`Pv`] handle and the task serving it.
pub(crate) struct Shared {
    state: Mutex<PvState>,
    writer: Mutex<Option<Writer>>,
    notify: Arc<RwLock<Option<Notifier>>>,
}

impl Shared {
    fn new(notify: Arc<RwLock<Option<Notifier>>>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PvState::default()),
            writer: Mutex::new(None),
            notify,
        })
    }

    /// Change the state and tell the UI about it.
    pub(crate) fn update(&self, f: impl FnOnce(&mut PvState)) {
        f(&mut self.state.lock().unwrap());
        if let Some(n) = self.notify.read().unwrap().as_ref() {
            n();
        }
    }

    pub(crate) fn snapshot(&self) -> PvState {
        self.state.lock().unwrap().clone()
    }

    pub(crate) fn set_writer(&self, writer: Writer) {
        *self.writer.lock().unwrap() = Some(writer);
    }
}

/// What serves a PV: stopped when the last [`Pv`] is dropped.
#[cfg(not(target_arch = "wasm32"))]
type Task = tokio::task::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
type Task = pvws::Task;

/// One subscription; the backend task stops when the last [`Pv`] is dropped.
struct Entry {
    name: PvName,
    shared: Arc<Shared>,
    task: Option<Task>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// A handle to a live PV. Cheap to clone.
#[derive(Clone)]
pub struct Pv(Arc<Entry>);

impl Pv {
    pub fn name(&self) -> &PvName {
        &self.0.name
    }

    /// The latest value, alarm and metadata.
    pub fn state(&self) -> PvState {
        self.0.shared.snapshot()
    }

    /// Write a value given as text; the backend converts it to the PV's type.
    pub fn write(&self, value: impl Into<String>) {
        let value = value.into();
        let shared = &self.0.shared;
        match shared.writer.lock().unwrap().as_ref() {
            #[cfg(not(target_arch = "wasm32"))]
            Some(Writer::Channel(tx)) => {
                let _ = tx.send(value);
            }
            Some(Writer::Local) => {
                shared.update(|s| s.value = local::apply(&s.value, &s.labels, &value));
            }
            #[cfg(target_arch = "wasm32")]
            Some(Writer::Pvws(name)) => pvws::write(name, &shared.snapshot(), value),
            None => log::warn!("{} is read-only", self.0.name),
        }
    }
}

/// Creates and shares PV subscriptions.
#[derive(Clone)]
pub struct PvManager {
    inner: Arc<Inner>,
}

struct Inner {
    /// Always `Some` until drop, which shuts it down without blocking.
    #[cfg(not(target_arch = "wasm32"))]
    rt: Option<tokio::runtime::Runtime>,
    entries: Mutex<HashMap<String, Weak<Entry>>>,
    locals: Mutex<HashMap<String, Arc<Shared>>>,
    // Shared with the PV tasks, which must not hold `Inner` itself: the last
    // reference could then be dropped on a runtime thread.
    #[cfg(not(target_arch = "wasm32"))]
    ca: Arc<tokio::sync::OnceCell<epics_ca_rs::client::CaClient>>,
    #[cfg(not(target_arch = "wasm32"))]
    pva: Arc<std::sync::OnceLock<epics_pva_rs::client::PvaClient>>,
    notify: Arc<RwLock<Option<Notifier>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl Default for PvManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PvManager {
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                locals: Mutex::new(HashMap::new()),
                notify: Arc::new(RwLock::new(None)),
            }),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("bobview-pv")
            .enable_all()
            .build()
            .expect("failed to start the PV runtime");
        Self {
            inner: Arc::new(Inner {
                rt: Some(rt),
                entries: Mutex::new(HashMap::new()),
                locals: Mutex::new(HashMap::new()),
                ca: Arc::new(tokio::sync::OnceCell::new()),
                pva: Arc::new(std::sync::OnceLock::new()),
                notify: Arc::new(RwLock::new(None)),
            }),
        }
    }

    /// Register a callback run after every PV change.
    pub fn set_notifier(&self, notifier: Notifier) {
        *self.inner.notify.write().unwrap() = Some(notifier);
    }

    /// The tokio runtime the PV tasks run on.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        self.inner.rt.as_ref().expect("runtime present until drop")
    }

    /// Get (or start) a subscription for `name`, e.g. `ca://X`, `sim://sine` or plain `X`.
    pub fn subscribe(&self, name: &str) -> Pv {
        let name = PvName::parse(name);
        let key = name.key();
        let mut entries = self.inner.entries.lock().unwrap();
        if let Some(entry) = entries.get(&key).and_then(Weak::upgrade) {
            return Pv(entry);
        }

        let (shared, task) = match name.scheme {
            Scheme::Loc => (self.local(&name), None),
            _ => {
                let shared = Shared::new(self.inner.notify.clone());
                let task = self.spawn_backend(&name, shared.clone());
                (shared, task)
            }
        };
        let entry = Arc::new(Entry { name, shared, task });
        entries.retain(|_, w| w.strong_count() > 0);
        entries.insert(key, Arc::downgrade(&entry));
        Pv(entry)
    }

    fn local(&self, name: &PvName) -> Arc<Shared> {
        let mut locals = self.inner.locals.lock().unwrap();
        locals
            .entry(name.key())
            .or_insert_with(|| local::create(name, Shared::new(self.inner.notify.clone())))
            .clone()
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn_backend(&self, name: &PvName, shared: Arc<Shared>) -> Option<Task> {
        Some(match name.scheme {
            Scheme::Sim => pvws::spawn(sim::run(name.clone(), shared)),
            Scheme::Ca | Scheme::Pva => pvws::subscribe(name, shared),
            Scheme::Loc => unreachable!("local PVs have no task"),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_backend(&self, name: &PvName, shared: Arc<Shared>) -> Option<Task> {
        let rt = self.runtime().handle();
        match name.scheme {
            Scheme::Ca => {
                let ca = self.inner.ca.clone();
                let pv = name.pv.clone();
                Some(rt.spawn(async move {
                    let client = ca.get_or_try_init(epics_ca_rs::client::CaClient::new).await;
                    match client {
                        Ok(client) => ca::run(client, &pv, shared).await,
                        Err(e) => {
                            log::error!("cannot start the CA client: {e}");
                            shared.update(|s| s.error = Some(format!("CA client: {e}")));
                        }
                    }
                }))
            }
            Scheme::Sim => Some(rt.spawn(sim::run(name.clone(), shared))),
            Scheme::Pva => {
                let pva = self.inner.pva.clone();
                let pv = name.pv.clone();
                Some(rt.spawn(async move {
                    let client =
                        pva.get_or_init(|| epics_pva_rs::client::PvaClient::builder().build());
                    pva::run(client, &pv, shared).await
                }))
            }
            Scheme::Loc => unreachable!("local PVs have no task"),
        }
    }
}
