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
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::{Mutex, RwLock};

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
        let mut n = self.notice.lock();
        *n = (n.0 + 1, text);
    }
}

/// A write's last step, taken once the writer lock is released.
enum After {
    /// The backend task has stopped: the write is lost.
    #[cfg(not(target_arch = "wasm32"))]
    Stopped(String),
    /// Set a local PV's value.
    Local(String),
    /// The PV takes no writes.
    Refused,
}

/// State shared between a [`Pv`] handle and the task serving it.
pub(crate) struct Shared {
    ctl: Arc<Control>,
    name: String,
    state: Mutex<PvState>,
    writer: Mutex<Option<Writer>>,
    /// Writes made before the backend task set the writer, sent once it does.
    pending: Mutex<Vec<String>>,
    /// Whether a backend task sets a writer (`ca://`, `pva://`), so writes
    /// made before then wait for it.
    backend_writes: bool,
    /// Writes accepted but not yet answered by [`Shared::write_result`]: the
    /// task serving the PV outlives its last [`Pv`] until they are.
    #[cfg(not(target_arch = "wasm32"))]
    in_flight: AtomicUsize,
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
            pending: Mutex::new(Vec::new()),
            backend_writes: matches!(name.scheme, Scheme::Ca | Scheme::Pva),
            #[cfg(not(target_arch = "wasm32"))]
            in_flight: AtomicUsize::new(0),
            notify,
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// Change the state and tell the UI about it.
    pub(crate) fn update(&self, f: impl FnOnce(&mut PvState)) {
        {
            let mut state = self.state.lock();
            f(&mut state);
            // Under the state lock, so listeners see changes in order.
            // A listener that panics is dropped, so it cannot stop the PV's
            // task (or the UI thread writing a local PV) for good. Release
            // builds abort on any panic instead (`panic = "abort"`).
            let mut listeners = self.listeners.lock();
            listeners.retain(|l| match l.upgrade() {
                Some(l) => {
                    let call = std::panic::AssertUnwindSafe(|| l(&state));
                    let ok = std::panic::catch_unwind(call).is_ok();
                    if !ok {
                        log::error!("{}: a listener panicked and was removed", self.name);
                    }
                    ok
                }
                None => false,
            });
        }
        if let Some(n) = self.notify.read().as_ref() {
            n();
        }
    }

    pub(crate) fn snapshot(&self) -> PvState {
        self.state.lock().clone()
    }

    /// A put failed (or worked, with `None`): remember why, and tell the user.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn write_result(&self, text: &str, error: Option<String>) {
        let mut n = self.in_flight.load(Ordering::Acquire);
        while n > 0 {
            match self.in_flight.compare_exchange_weak(
                n,
                n - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
        if let Some(e) = &error {
            self.ctl
                .notify(format!("{}: write '{text}' failed: {e}", self.name));
        }
        self.update(|s| s.write_error = error);
    }

    /// Set where writes go, and send any made before now.
    pub(crate) fn set_writer(&self, writer: Writer) {
        let mut slot = self.writer.lock();
        *slot = Some(writer);
        let pending = std::mem::take(&mut *self.pending.lock());
        // Sent before the lock is released, so later writes follow them.
        let after: Vec<_> = pending
            .into_iter()
            .filter_map(|value| self.send(slot.as_ref(), value, false))
            .collect();
        drop(slot);
        after.into_iter().for_each(|a| self.finish(a));
    }

    /// Send a write through `writer`; with none yet, a `ca://` or `pva://`
    /// PV (whose task is still starting) keeps it until there is one. What
    /// is left to do once the writer lock is released, so that it and the
    /// state lock never nest. `new` counts the write as in flight: a
    /// pending one already is.
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    fn send(&self, writer: Option<&Writer>, value: String, new: bool) -> Option<After> {
        match writer {
            #[cfg(not(target_arch = "wasm32"))]
            Some(Writer::Channel(tx)) => {
                // Counted before the task can answer it; a failed send is
                // answered (and uncounted) by `finish`.
                if new {
                    self.in_flight.fetch_add(1, Ordering::AcqRel);
                }
                // The task serving the PV holds the receiver until it stops.
                tx.send(value).err().map(|e| After::Stopped(e.0))
            }
            Some(Writer::Local) => Some(After::Local(value)),
            #[cfg(target_arch = "wasm32")]
            Some(Writer::Pvws(name)) => {
                pvws::write(name, &self.snapshot(), value);
                None
            }
            None if self.backend_writes => {
                #[cfg(not(target_arch = "wasm32"))]
                self.in_flight.fetch_add(1, Ordering::AcqRel);
                self.pending.lock().push(value);
                None
            }
            None => Some(After::Refused),
        }
    }

    fn finish(&self, after: After) {
        match after {
            #[cfg(not(target_arch = "wasm32"))]
            After::Stopped(value) => {
                self.write_result(&value, Some("the PV's connection has stopped".into()))
            }
            After::Local(value) => {
                self.update(|s| s.value = local::apply(&s.value, &s.labels, &value))
            }
            After::Refused => self.ctl.notify(format!("{} cannot be written", self.name)),
        }
    }
}

/// What serves a PV: stopped when the last [`Pv`] is dropped.
#[cfg(not(target_arch = "wasm32"))]
type Task = tokio::task::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
type Task = pvws::Task;

/// How long the task of a dropped PV may go on to send writes made on it,
/// as a write to a PV that is not connected yet must first find it.
#[cfg(not(target_arch = "wasm32"))]
const WRITE_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// One subscription; the backend task stops when the last [`Pv`] is dropped,
/// or once the writes made on it are sent.
struct Entry {
    name: PvName,
    shared: Arc<Shared>,
    task: Option<Task>,
    /// The runtime the task runs on, to finish its writes after the drop.
    #[cfg(not(target_arch = "wasm32"))]
    rt: tokio::runtime::Handle,
}

impl Drop for Entry {
    fn drop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        // A write is often made on a PV subscribed just for it and dropped
        // straight after: let the task send it (or give up) first.
        #[cfg(not(target_arch = "wasm32"))]
        if self.shared.in_flight.load(Ordering::Acquire) > 0 {
            let shared = self.shared.clone();
            self.rt.spawn(async move {
                let start = tokio::time::Instant::now();
                while shared.in_flight.load(Ordering::Acquire) > 0 && start.elapsed() < WRITE_GRACE
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                task.abort();
            });
            return;
        }
        task.abort();
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
            && !matches!(*shared.writer.lock(), Some(Writer::Local))
    }

    /// Call `listener` with the current state now and with each change
    /// after, until the caller drops it: unlike [`Pv::state`] read once a
    /// frame, this sees every monitor update.
    pub fn listen(&self, listener: &Listener) {
        let shared = &self.0.shared;
        let state = shared.state.lock();
        listener(&state);
        let mut listeners = shared.listeners.lock();
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
        let writer = shared.writer.lock();
        let after = shared.send(writer.as_ref(), value, true);
        drop(writer);
        if let Some(after) = after {
            shared.finish(after);
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
        *self.inner.notify.write() = Some(notifier);
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
        self.inner.ctl.notice.lock().clone()
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
        let mut entries = self.inner.entries.lock();
        if let Some(entry) = entries.map.get(&key).and_then(Weak::upgrade) {
            return Pv(entry);
        }

        let (shared, task) = match name.scheme {
            Scheme::Loc => (self.local(&name), None),
            Scheme::Unsupported => {
                let shared = Shared::new(&name, self.inner.notify.clone(), self.inner.ctl.clone());
                log::warn!("{name}: formula and other PV types are not supported");
                shared.update(|s| s.error = Some("PV type not supported".into()));
                (shared, None)
            }
            _ => {
                let shared = Shared::new(&name, self.inner.notify.clone(), self.inner.ctl.clone());
                let task = self.spawn_backend(&name, shared.clone());
                (shared, task)
            }
        };
        let entry = Arc::new(Entry {
            name,
            shared,
            task,
            #[cfg(not(target_arch = "wasm32"))]
            rt: self.runtime().handle().clone(),
        });
        entries.insert(key, Arc::downgrade(&entry));
        Pv(entry)
    }

    fn local(&self, name: &PvName) -> Arc<Shared> {
        let mut locals = self.inner.locals.lock();
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
            Scheme::Loc | Scheme::Unsupported => unreachable!("no task"),
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
            Scheme::Loc | Scheme::Unsupported => unreachable!("no task"),
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
            s.lock().push(st.value.as_f64());
        });
        pv.listen(&listener);
        pv.write("2");
        pv.write("3");
        drop(listener);
        pv.write("4");
        assert_eq!(*seen.lock(), [Some(1.0), Some(2.0), Some(3.0)]);
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
    fn a_write_with_no_task_to_send_it_is_reported() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("sim://sine");
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        pv.0.shared.set_writer(Writer::Channel(tx));
        pv.write("7");
        let (n, text) = pvs.notice();
        assert_eq!(n, 1);
        assert!(text.contains("write '7' failed"), "{text}");
        assert!(pv.state().write_error.is_some());
    }

    #[test]
    fn a_panicking_listener_is_dropped_and_the_pv_goes_on() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://listen_panic(1)");
        let listener: Listener = Arc::new(|st: &PvState| {
            if st.value.as_f64() == Some(2.0) {
                panic!("listener bug");
            }
        });
        pv.listen(&listener);
        pv.write("2");
        pv.write("3");
        assert_eq!(pv.state().value.as_f64(), Some(3.0));
        assert!(pv.0.shared.listeners.lock().is_empty());
    }

    #[test]
    fn listen_drops_dead_listeners() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://listen_prune(1)");
        for _ in 0..3 {
            let listener: Listener = Arc::new(|_: &PvState| {});
            pv.listen(&listener);
        }
        assert_eq!(pv.0.shared.listeners.lock().len(), 1);
    }

    #[test]
    fn a_write_made_before_the_writer_is_sent_and_counted_once() {
        let shared = Shared::new(
            &PvName::parse("ca://early"),
            Arc::new(RwLock::new(None)),
            Arc::default(),
        );
        assert!(shared.send(None, "1".into(), true).is_none());
        assert_eq!(shared.in_flight.load(Ordering::Acquire), 1);
        let (tx, mut rx) = mpsc::unbounded_channel();
        shared.set_writer(Writer::Channel(tx));
        assert_eq!(rx.try_recv().ok().as_deref(), Some("1"));
        assert_eq!(shared.in_flight.load(Ordering::Acquire), 1);
        shared.write_result("1", None);
        assert_eq!(shared.in_flight.load(Ordering::Acquire), 0);
    }

    #[test]
    fn formulas_never_connect_and_refuse_writes() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("=pv0 + 1");
        let s = pv.state();
        assert!(!s.connected);
        assert!(s.error.is_some());
        pv.write("1");
        assert!(pvs.notice().1.contains("cannot be written"));
    }
}
