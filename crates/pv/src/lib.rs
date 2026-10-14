//! PV access for bobview.
//!
//! Widgets ask a [`PvManager`] for a [`Pv`] by name and read its latest
//! [`PvState`] each frame. The manager owns a tokio runtime on which one task
//! per PV talks to the backend chosen by the name's prefix:
//!
//! | prefix    | backend                                 |
//! |-----------|-----------------------------------------|
//! | `ca://`   | Channel Access (the default)            |
//! | `pva://`  | PV Access (not implemented yet)         |
//! | `loc://`  | local PVs held in this process          |
//! | `sim://`  | simulated signals (sine, ramp, noise …) |

mod ca;
mod local;
mod name;
mod sim;
mod value;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

use tokio::sync::mpsc;

pub use name::{PvName, Scheme};
pub use value::{PvState, Severity, Value};

/// Called whenever any PV changes, so a UI can schedule a repaint.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Where writes to a PV go.
pub(crate) enum Writer {
    /// To the backend task, which converts and sends them.
    Channel(mpsc::UnboundedSender<String>),
    /// Straight into the local value.
    Local,
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

/// One subscription; the backend task stops when the last [`Pv`] is dropped.
struct Entry {
    name: PvName,
    shared: Arc<Shared>,
    task: Option<tokio::task::JoinHandle<()>>,
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
            Some(Writer::Channel(tx)) => {
                let _ = tx.send(value);
            }
            Some(Writer::Local) => {
                shared.update(|s| s.value = local::apply(&s.value, &s.labels, &value));
            }
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
    rt: tokio::runtime::Runtime,
    entries: Mutex<HashMap<String, Weak<Entry>>>,
    locals: Mutex<HashMap<String, Arc<Shared>>>,
    ca: tokio::sync::OnceCell<epics_ca_rs::client::CaClient>,
    notify: Arc<RwLock<Option<Notifier>>>,
}

impl Default for PvManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PvManager {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("bobview-pv")
            .enable_all()
            .build()
            .expect("failed to start the PV runtime");
        Self {
            inner: Arc::new(Inner {
                rt,
                entries: Mutex::new(HashMap::new()),
                locals: Mutex::new(HashMap::new()),
                ca: tokio::sync::OnceCell::new(),
                notify: Arc::new(RwLock::new(None)),
            }),
        }
    }

    /// Register a callback run after every PV change.
    pub fn set_notifier(&self, notifier: Notifier) {
        *self.inner.notify.write().unwrap() = Some(notifier);
    }

    /// The tokio runtime the PV tasks run on.
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.inner.rt
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

    fn spawn_backend(
        &self,
        name: &PvName,
        shared: Arc<Shared>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let rt = self.inner.rt.handle();
        match name.scheme {
            Scheme::Ca => {
                let inner = self.inner.clone();
                let pv = name.pv.clone();
                Some(rt.spawn(async move {
                    let client = inner
                        .ca
                        .get_or_try_init(epics_ca_rs::client::CaClient::new)
                        .await;
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
                shared.update(|s| s.error = Some("PV Access is not supported yet".into()));
                None
            }
            Scheme::Loc => unreachable!("local PVs have no task"),
        }
    }
}
