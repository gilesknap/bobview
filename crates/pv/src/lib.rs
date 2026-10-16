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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;

pub use name::{PvName, Scheme};
#[cfg(target_arch = "wasm32")]
pub use pvws::{set_pvws, set_write_token};
pub use value::{Image, ImageData, PvState, Severity, Value, split_array, split_strings};

/// Called whenever any PV changes, so a UI can schedule a repaint.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Called with a PV's new state on each change, from the thread making it,
/// for as long as the caller keeps the `Arc` (see [`Pv::listen`]).
pub type Listener = Arc<ListenFn>;
pub type ListenFn = dyn Fn(&PvState) + Send + Sync;

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

/// Settings and messages shared by every PV of one [`PvManager`].
#[derive(Default)]
pub(crate) struct Control {
    /// Refuse every write except to `loc://` PVs (which only live here).
    read_only: AtomicBool,
    /// The latest message for the user (a refused or failed write), with a
    /// number that changes with each one.
    notice: Mutex<(u64, String)>,
}

impl Control {
    pub(crate) fn notify(&self, text: String) {
        log::warn!("{text}");
        let mut n = self.notice.lock().unwrap();
        *n = (n.0 + 1, text);
    }
}

/// State shared between a [`Pv`] handle and the task serving it.
pub(crate) struct Shared {
    ctl: Arc<Control>,
    name: String,
    state: Mutex<PvState>,
    writer: Mutex<Option<Writer>>,
    notify: Arc<RwLock<Option<Notifier>>>,
    listeners: Mutex<Vec<Weak<ListenFn>>>,
}

impl Shared {
    fn new(name: &PvName, notify: Arc<RwLock<Option<Notifier>>>, ctl: Arc<Control>) -> Arc<Self> {
        Arc::new(Self {
            ctl,
            name: name.to_string(),
            state: Mutex::new(PvState::default()),
            writer: Mutex::new(None),
            notify,
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// Change the state and tell the UI about it.
    pub(crate) fn update(&self, f: impl FnOnce(&mut PvState)) {
        {
            let mut state = self.state.lock().unwrap();
            f(&mut state);
            // Under the state lock, so listeners see changes in order.
            let mut listeners = self.listeners.lock().unwrap();
            listeners.retain(|l| match l.upgrade() {
                Some(l) => {
                    l(&state);
                    true
                }
                None => false,
            });
        }
        if let Some(n) = self.notify.read().unwrap().as_ref() {
            n();
        }
    }

    pub(crate) fn snapshot(&self) -> PvState {
        self.state.lock().unwrap().clone()
    }

    /// A put failed (or worked, with `None`): remember why, and tell the user.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn write_result(&self, text: &str, error: Option<String>) {
        if let Some(e) = &error {
            self.ctl
                .notify(format!("{}: write '{text}' failed: {e}", self.name));
        }
        self.update(|s| s.write_error = error);
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
        let mut state = self.0.shared.snapshot();
        if self.refuses_writes() {
            state.writable = false;
            state.writable_known = true;
        }
        state
    }

    /// Whether read-only mode (see [`PvManager::set_read_only`]) stops writes
    /// to this PV: all but local ones, which cannot reach a control system.
    fn refuses_writes(&self) -> bool {
        let shared = &self.0.shared;
        shared.ctl.read_only.load(Ordering::Relaxed)
            && !matches!(*shared.writer.lock().unwrap(), Some(Writer::Local))
    }

    /// Call `listener` with the current state now and with each change
    /// after, until the caller drops it: unlike [`Pv::state`] read once a
    /// frame, this sees every monitor update.
    pub fn listen(&self, listener: &Listener) {
        let shared = &self.0.shared;
        let state = shared.state.lock().unwrap();
        listener(&state);
        let mut listeners = shared.listeners.lock().unwrap();
        // Drop dead ones here too: a PV that never changes is never updated.
        listeners.retain(|l| l.strong_count() > 0);
        listeners.push(Arc::downgrade(listener));
    }

    /// Write a value given as text; the backend converts it to the PV's type.
    pub fn write(&self, value: impl Into<String>) {
        let value = value.into();
        let shared = &self.0.shared;
        if self.refuses_writes() {
            shared.ctl.notify(format!(
                "{}: not writing '{value}', bobview is read-only",
                shared.name
            ));
            return;
        }
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
            None => shared
                .ctl
                .notify(format!("{} cannot be written", shared.name)),
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
    entries: Mutex<Entries>,
    locals: Mutex<HashMap<String, Arc<Shared>>>,
    // Shared with the PV tasks, which must not hold `Inner` itself: the last
    // reference could then be dropped on a runtime thread.
    #[cfg(not(target_arch = "wasm32"))]
    ca: Arc<tokio::sync::OnceCell<epics_ca_rs::client::CaClient>>,
    #[cfg(not(target_arch = "wasm32"))]
    pva: Arc<std::sync::OnceLock<epics_pva_rs::client::PvaClient>>,
    notify: Arc<RwLock<Option<Notifier>>>,
    ctl: Arc<Control>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// Every subscription by key, including dropped ones until the next prune.
#[derive(Default)]
struct Entries {
    map: HashMap<String, Weak<Entry>>,
    /// Prune dropped entries once the map is this big, so a batch of
    /// subscriptions doesn't scan the map for each one.
    prune_at: usize,
}

impl Entries {
    fn insert(&mut self, key: String, entry: Weak<Entry>) {
        if self.map.len() >= self.prune_at {
            self.map.retain(|_, w| w.strong_count() > 0);
            self.prune_at = (2 * self.map.len()).max(64);
        }
        self.map.insert(key, entry);
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
                entries: Mutex::default(),
                locals: Mutex::new(HashMap::new()),
                notify: Arc::new(RwLock::new(None)),
                ctl: Arc::default(),
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
                entries: Mutex::default(),
                locals: Mutex::new(HashMap::new()),
                ca: Arc::new(tokio::sync::OnceCell::new()),
                pva: Arc::new(std::sync::OnceLock::new()),
                notify: Arc::new(RwLock::new(None)),
                ctl: Arc::default(),
            }),
        }
    }

    /// Register a callback run after every PV change.
    pub fn set_notifier(&self, notifier: Notifier) {
        *self.inner.notify.write().unwrap() = Some(notifier);
    }

    /// Refuse (or allow again) writes to every PV but `loc://` ones; PVs then
    /// report themselves not writable, so controls are disabled.
    pub fn set_read_only(&self, read_only: bool) {
        self.inner.ctl.read_only.store(read_only, Ordering::Relaxed);
    }

    pub fn read_only(&self) -> bool {
        self.inner.ctl.read_only.load(Ordering::Relaxed)
    }

    /// The latest message for the user about a refused or failed write, and a
    /// number that changes with each new one (starting at 0 for none).
    pub fn notice(&self) -> (u64, String) {
        self.inner.ctl.notice.lock().unwrap().clone()
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
        if let Some(entry) = entries.map.get(&key).and_then(Weak::upgrade) {
            return Pv(entry);
        }

        let (shared, task) = match name.scheme {
            Scheme::Loc => (self.local(&name), None),
            _ => {
                let shared = Shared::new(&name, self.inner.notify.clone(), self.inner.ctl.clone());
                let task = self.spawn_backend(&name, shared.clone());
                (shared, task)
            }
        };
        let entry = Arc::new(Entry { name, shared, task });
        entries.insert(key, Arc::downgrade(&entry));
        Pv(entry)
    }

    fn local(&self, name: &PvName) -> Arc<Shared> {
        let mut locals = self.inner.locals.lock().unwrap();
        locals
            .entry(name.key())
            .or_insert_with(|| {
                local::create(
                    name,
                    Shared::new(name, self.inner.notify.clone(), self.inner.ctl.clone()),
                )
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_see_each_change_until_dropped() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://listen_test(1)");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let listener: Listener = Arc::new(move |st: &PvState| {
            s.lock().unwrap().push(st.value.as_f64());
        });
        pv.listen(&listener);
        pv.write("2");
        pv.write("3");
        drop(listener);
        pv.write("4");
        assert_eq!(*seen.lock().unwrap(), [Some(1.0), Some(2.0), Some(3.0)]);
    }

    #[test]
    fn read_only_refuses_writes_but_not_local_ones() {
        let pvs = PvManager::new();
        pvs.set_read_only(true);
        let sim = pvs.subscribe("sim://sine");
        let state = sim.state();
        assert!(!state.writable && state.writable_known);
        sim.write("1");
        let (n, text) = pvs.notice();
        assert_eq!(n, 1);
        assert!(text.contains("read-only"), "{text}");

        let loc = pvs.subscribe("loc://ro_test(1)");
        assert!(loc.state().writable);
        loc.write("5");
        assert_eq!(loc.state().value.as_f64(), Some(5.0));
        assert_eq!(pvs.notice().0, 1);

        pvs.set_read_only(false);
        assert!(!pvs.read_only());
    }

    #[test]
    fn a_failed_put_is_remembered_until_one_works() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("sim://sine");
        let shared = &pv.0.shared;
        shared.write_result("3", Some("no write access".into()));
        assert_eq!(pv.state().write_error.as_deref(), Some("no write access"));
        let (n, text) = pvs.notice();
        assert_eq!(n, 1);
        assert!(text.contains("write '3' failed: no write access"), "{text}");
        shared.write_result("4", None);
        assert_eq!(pv.state().write_error, None);
    }

    #[test]
    fn listen_drops_dead_listeners() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://listen_prune(1)");
        for _ in 0..3 {
            let listener: Listener = Arc::new(|_: &PvState| {});
            pv.listen(&listener);
        }
        assert_eq!(pv.0.shared.listeners.lock().unwrap().len(), 1);
    }
}
