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
//! | `=`, `eq://` | formulas over other PVs              |
//!
//! In a browser (wasm32) there is no runtime: `sim://` runs on the page's
//! event loop, and `ca://` and `pva://` PVs come over a PV Web Socket
//! ([`set_pvws`]).

#[cfg(not(target_arch = "wasm32"))]
mod ca;
mod formula;
mod local;
mod name;
mod origin;
#[cfg(not(target_arch = "wasm32"))]
mod pva;
#[cfg(target_arch = "wasm32")]
mod pvws;
#[cfg(any(target_arch = "wasm32", test))]
mod pvws_msg;
mod sim;
mod value;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::{Mutex, RwLock};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;

pub use name::{PvName, Scheme};
pub use origin::{same_origin, url_origin};
#[cfg(target_arch = "wasm32")]
pub use pvws::{set_pvws, set_write_token};
pub use value::{
    Image, ImageData, PvState, Severity, Value, long_string, long_string_bytes, split_array,
    split_strings,
};

/// Called whenever any PV changes, so a UI can schedule a repaint.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Called with a PV's new state on each change, from the thread making it,
/// for as long as the caller keeps the `Arc` (see [`Pv::listen`]).
pub type Listener = Arc<ListenFn>;
pub type ListenFn = dyn Fn(&PvState) + Send + Sync;

/// A write of text to a PV.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Put {
    pub text: String,
    /// Write the text into an array of numbers as a long string (see
    /// [`Pv::write_long_string`]).
    pub long_string: bool,
}

/// Where writes to a PV go.
pub(crate) enum Writer {
    /// To the backend task, which converts and sends them.
    #[cfg(not(target_arch = "wasm32"))]
    Channel(mpsc::Sender<Put>),
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
    /// The UI's repaint callback (see [`PvManager::set_notifier`]), so a
    /// new notice is shown without waiting for some PV to change.
    repaint: Arc<RwLock<Option<Notifier>>>,
}

impl Control {
    pub(crate) fn notify(&self, text: String) {
        log::warn!("{text}");
        {
            let mut n = self.notice.lock();
            *n = (n.0 + 1, text);
        }
        if let Some(n) = self.repaint.read().as_ref() {
            n();
        }
    }
}

/// Writes to one PV that may wait for the backend at once; more are
/// refused, rather than queued to be applied late.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const WRITE_QUEUE: usize = 16;

/// The queue of a backend's writes, for [`Writer::Channel`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_queue() -> (mpsc::Sender<Put>, mpsc::Receiver<Put>) {
    mpsc::channel(WRITE_QUEUE)
}

thread_local! {
    /// How many PVs' listeners this thread is calling, under their locks.
    static CALLING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Work a listener left for when this thread holds no PV's lock.
    static DEFERRED: std::cell::RefCell<Vec<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Call listeners in `f`, under a PV's lock; [`after_listeners`] once the
/// lock is let go.
fn calling_listeners<R>(f: impl FnOnce() -> R) -> R {
    struct Calling;
    impl Drop for Calling {
        fn drop(&mut self) {
            CALLING.with(|c| c.set(c.get() - 1));
        }
    }
    CALLING.with(|c| c.set(c.get() + 1));
    let _calling = Calling;
    f()
}

/// Run the work listeners left, once this thread holds no PV's lock.
fn after_listeners() {
    while CALLING.with(|c| c.get()) == 0 {
        let work = DEFERRED.with(|d| std::mem::take(&mut *d.borrow_mut()));
        if work.is_empty() {
            break;
        }
        for w in work {
            w();
        }
    }
}

/// Run `work` now, or, from a listener, once this thread holds no PV's
/// lock: work that locks a PV (subscribing, listening) could otherwise
/// wait for one this thread holds, or take two in another thread's order.
pub(crate) fn outside_listeners(work: Box<dyn FnOnce()>) {
    if CALLING.with(|c| c.get()) == 0 {
        work();
        after_listeners();
    } else {
        DEFERRED.with(|d| d.borrow_mut().push(work));
    }
}

/// State shared between a [`Pv`] handle and the task serving it.
pub(crate) struct Shared {
    ctl: Arc<Control>,
    name: String,
    state: Mutex<PvState>,
    /// How many times the state has changed.
    version: AtomicU64,
    writer: Mutex<Option<Writer>>,
    notify: Arc<RwLock<Option<Notifier>>>,
    listeners: Mutex<Vec<Weak<ListenFn>>>,
    /// Set (under the state lock) once the PV has first connected.
    was_connected: AtomicBool,
}

impl Shared {
    fn new(name: &PvName, notify: Arc<RwLock<Option<Notifier>>>, ctl: Arc<Control>) -> Arc<Self> {
        Arc::new(Self {
            ctl,
            name: name.to_string(),
            state: Mutex::new(PvState::default()),
            version: AtomicU64::new(0),
            writer: Mutex::new(None),
            notify,
            listeners: Mutex::new(Vec::new()),
            was_connected: AtomicBool::new(false),
        })
    }

    /// Change the state and tell the UI about it.
    pub(crate) fn update(&self, f: impl FnOnce(&mut PvState)) {
        {
            let mut state = self.state.lock();
            f(&mut state);
            self.version.fetch_add(1, Ordering::Relaxed);
            if state.connected {
                self.was_connected.store(true, Ordering::Relaxed);
            }
            // Under the state lock, so listeners see changes in order.
            // A listener that panics is dropped, so it cannot stop the PV's
            // task (or the UI thread writing a local PV) for good. Release
            // builds abort on any panic instead (`panic = "abort"`).
            let mut listeners = self.listeners.lock();
            calling_listeners(|| {
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
                })
            });
        }
        after_listeners();
        if let Some(n) = self.notify.read().as_ref() {
            n();
        }
    }

    /// Whether the PV connected once and has since lost its connection;
    /// not one that has yet to connect.
    pub(crate) fn dropped(&self) -> bool {
        let state = self.state.lock();
        !state.connected && self.was_connected.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> PvState {
        self.state.lock().clone()
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

    /// A put that the server does not answer was sent, or failed to be: a
    /// failure is remembered as [`Shared::write_result`] does, but a send
    /// confirms nothing, so it leaves an earlier failure shown.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn write_sent(&self, text: &str, error: Option<String>) {
        if error.is_some() {
            self.write_result(text, error);
        }
    }

    /// Whether a write taken from the queue can go now; if not, say why.
    /// One to a PV that has yet to connect goes, for the backend to wait
    /// for or fail.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn can_write(&self, text: &str) -> bool {
        let dropped = self.dropped();
        if dropped {
            self.write_result(text, Some("not connected".into()));
        }
        !dropped
    }

    pub(crate) fn set_writer(&self, writer: Writer) {
        *self.writer.lock() = Some(writer);
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
    /// A formula's inputs, followed for as long as it is held.
    #[cfg_attr(not(test), allow(dead_code, reason = "only held"))]
    formula: Option<Arc<formula::Formula>>,
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

    /// Whether the PV connected once and has since lost its connection. A
    /// write to it is refused at once; one to a PV that has yet to connect
    /// is queued.
    pub fn dropped(&self) -> bool {
        self.0.shared.dropped()
    }

    /// Mark the PV connected or not, as its backend would; for tests of
    /// code that uses PVs.
    #[doc(hidden)]
    pub fn set_connected_for_test(&self, connected: bool) {
        self.0.shared.update(|s| s.connected = connected);
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

    /// Whether it is connected, without copying its state.
    pub fn connected(&self) -> bool {
        self.0.shared.state.lock().connected
    }

    /// A number that changes whenever its state does (bar read-only mode),
    /// so a caller can skip work on a PV that has not changed.
    pub fn version(&self) -> u64 {
        self.0.shared.version.load(Ordering::Relaxed)
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
        {
            let state = shared.state.lock();
            calling_listeners(|| listener(&state));
            let mut listeners = shared.listeners.lock();
            // Drop dead ones here too: a PV that never changes is never updated.
            listeners.retain(|l| l.strong_count() > 0);
            listeners.push(Arc::downgrade(listener));
        }
        after_listeners();
    }

    /// Write a value given as text; the backend converts it to the PV's type.
    pub fn write(&self, value: impl Into<String>) {
        self.put(value.into(), false);
    }

    /// Write text as Phoebus does for a widget whose `format` is string: into
    /// a char array as a long string (see [`long_string_bytes`]). Any other
    /// PV is written as by [`Pv::write`].
    ///
    /// Over CA, as in Phoebus, a char array is a `DBF_CHAR` channel of more
    /// than one element, and the text is cut to that element count with its
    /// NUL kept. Over PVA and `loc://` it is any array of numbers. A PVA
    /// array has no fixed element count to cut to, so an IOC that stores
    /// only its first NELM elements may lose the NUL (and part of a
    /// multi-byte character), as it does with Phoebus.
    pub fn write_long_string(&self, value: impl Into<String>) {
        self.put(value.into(), true);
    }

    fn put(&self, value: String, long_string: bool) {
        let shared = &self.0.shared;
        if self.refuses_writes() {
            shared.ctl.notify(format!(
                "{}: not writing '{value}', bobview is read-only",
                shared.name
            ));
            return;
        }
        // Read first: the state and writer locks never nest.
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = shared.dropped();
        // Released before the state changes, so the two locks never nest.
        let writer = shared.writer.lock();
        match writer.as_ref() {
            #[cfg(not(target_arch = "wasm32"))]
            Some(Writer::Channel(tx)) => {
                // A write to a PV that has gone would wait for it to come
                // back, and land late, perhaps long after the user moved on.
                // One that has not connected yet is queued: it was most
                // likely just subscribed.
                if dropped {
                    drop(writer);
                    shared.ctl.notify(format!(
                        "{}: not writing '{value}', not connected",
                        shared.name
                    ));
                    return;
                }
                // The task serving the PV holds the receiver until it stops.
                let sent = tx.try_send(Put {
                    text: value,
                    long_string,
                });
                drop(writer);
                match sent {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(put)) => shared.ctl.notify(format!(
                        "{}: not writing '{}', {WRITE_QUEUE} writes still waiting",
                        shared.name, put.text
                    )),
                    Err(mpsc::error::TrySendError::Closed(put)) => shared
                        .write_result(&put.text, Some("the PV's connection has stopped".into())),
                }
            }
            Some(Writer::Local) => {
                drop(writer);
                shared.update(|s| {
                    s.value = if long_string && s.value.is_numeric_array() {
                        Value::LongArray(
                            long_string_bytes(&value, None)
                                .iter()
                                .map(|b| *b as i64)
                                .collect(),
                        )
                    } else {
                        local::apply(&s.value, &s.labels, &value)
                    }
                })
            }
            #[cfg(target_arch = "wasm32")]
            Some(Writer::Pvws(name)) => pvws::write(name, &shared.snapshot(), value, long_string),
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
        let ctl = Arc::new(Control::default());
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::default(),
                locals: Mutex::new(HashMap::new()),
                notify: ctl.repaint.clone(),
                ctl,
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
        let ctl = Arc::new(Control::default());
        Self {
            inner: Arc::new(Inner {
                rt: Some(rt),
                entries: Mutex::default(),
                locals: Mutex::new(HashMap::new()),
                ca: Arc::new(tokio::sync::OnceCell::new()),
                pva: Arc::new(std::sync::OnceLock::new()),
                notify: ctl.repaint.clone(),
                ctl,
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
        // A macro left unexpanded makes a name that is not what was meant,
        // and a local one would share its state with every other such PV.
        if name.pv.contains("$(") || name.pv.contains("${") {
            log::warn!("{}: PV name has an undefined macro", name.pv);
        }

        let shared = || Shared::new(&name, self.inner.notify.clone(), self.inner.ctl.clone());
        let (shared, task, formula) = match name.scheme {
            Scheme::Loc => (self.local(&name), None, None),
            Scheme::Formula => {
                let shared = shared();
                let f = formula::Formula::new(&name, shared.clone(), Arc::downgrade(&self.inner));
                (shared, None, Some(f))
            }
            _ => {
                let shared = shared();
                let task = self.spawn_backend(&name, shared.clone());
                (shared, task, None)
            }
        };
        let entry = Arc::new(Entry {
            name,
            shared,
            task,
            formula: formula.clone(),
        });
        entries.insert(key, Arc::downgrade(&entry));
        // Its inputs are subscribed to with the entries unlocked.
        drop(entries);
        if let Some(f) = formula {
            f.start();
        }
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
            Scheme::Loc | Scheme::Formula => unreachable!("local PVs and formulas have no task"),
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
            Scheme::Loc | Scheme::Formula => unreachable!("local PVs and formulas have no task"),
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
    fn a_sent_put_leaves_an_earlier_failure_shown() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("sim://sine");
        let shared = &pv.0.shared;
        shared.write_sent("3", Some("not connected".into()));
        assert_eq!(pv.state().write_error.as_deref(), Some("not connected"));
        assert_eq!(pvs.notice().0, 1);
        // A CA put with no reply was only sent, not confirmed.
        shared.write_sent("4", None);
        assert_eq!(pv.state().write_error.as_deref(), Some("not connected"));
        assert_eq!(pvs.notice().0, 1);
    }

    #[test]
    fn a_new_notice_asks_for_a_repaint() {
        let pvs = PvManager::new();
        // No backend task, so nothing else changes the PV's state.
        let (pv, _rx) = queued(&pvs, true);
        let calls = Arc::new(AtomicU64::new(0));
        let c = calls.clone();
        pvs.set_notifier(Arc::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        }));
        pvs.set_read_only(true);
        pv.write("1");
        assert_eq!(pvs.notice().0, 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    /// A PV whose writes go to a queue no backend reads.
    fn queued(pvs: &PvManager, connected: bool) -> (Pv, mpsc::Receiver<Put>) {
        let name = PvName::parse("ca://QUEUED");
        let shared = Shared::new(&name, pvs.inner.notify.clone(), pvs.inner.ctl.clone());
        let (tx, rx) = write_queue();
        shared.set_writer(Writer::Channel(tx));
        shared.update(|s| s.connected = connected);
        let entry = Entry {
            name,
            shared,
            task: None,
            formula: None,
        };
        (Pv(Arc::new(entry)), rx)
    }

    #[test]
    fn writes_to_a_dropped_pv_fail_at_once() {
        let pvs = PvManager::new();
        let (pv, mut rx) = queued(&pvs, true);
        pv.0.shared.update(|s| s.connected = false);
        assert!(pv.dropped());
        pv.write("1");
        assert!(rx.try_recv().is_err());
        let (n, text) = pvs.notice();
        assert_eq!(n, 1);
        assert!(text.contains("not connected"), "{text}");
        // And one that disconnects while a write waits reports it.
        assert!(!pv.0.shared.can_write("1"));
        assert_eq!(pv.state().write_error.as_deref(), Some("not connected"));
    }

    #[test]
    fn writes_to_a_pv_yet_to_connect_are_queued() {
        let pvs = PvManager::new();
        let (pv, mut rx) = queued(&pvs, false);
        assert!(!pv.dropped());
        pv.write("1");
        pv.write_long_string("a");
        assert_eq!(pvs.notice().0, 0);
        let put = rx.try_recv().unwrap();
        assert_eq!((put.text.as_str(), put.long_string), ("1", false));
        let put = rx.try_recv().unwrap();
        assert_eq!((put.text.as_str(), put.long_string), ("a", true));
        // The backend goes ahead with them, to wait for the PV or fail.
        assert!(pv.0.shared.can_write("1"));
        assert_eq!(pv.state().write_error, None);
        // Still bounded.
        for i in 0..WRITE_QUEUE + 1 {
            pv.write(i.to_string());
        }
        assert_eq!(pvs.notice().0, 1);
    }

    #[test]
    fn the_write_queue_is_bounded() {
        let pvs = PvManager::new();
        let (pv, mut rx) = queued(&pvs, true);
        for i in 0..WRITE_QUEUE + 3 {
            pv.write(i.to_string());
        }
        let (n, text) = pvs.notice();
        assert_eq!(n, 3);
        assert!(text.contains("writes still waiting"), "{text}");
        let mut got = Vec::new();
        while let Ok(v) = rx.try_recv() {
            got.push(v.text);
        }
        assert_eq!(got.len(), WRITE_QUEUE);
        assert_eq!(got[0], "0");
        drop(rx);
        pv.write("x");
        assert!(pvs.notice().1.contains("stopped"));
    }

    #[test]
    fn long_strings_are_written_into_number_arrays_only() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("loc://long_string_test<VLongArray>(0, 0)");
        pv.write_long_string("/tmp/a");
        let state = pv.state();
        assert_eq!(state.value.as_long_string().as_deref(), Some("/tmp/a"));
        let Value::LongArray(a) = &state.value else {
            panic!("{:?}", state.value)
        };
        assert_eq!(a.last(), Some(&0));
        // Other writes are unchanged.
        pv.write("[1, 2]");
        assert_eq!(pv.state().value, Value::LongArray(Arc::from(vec![1, 2])));
        let s = pvs.subscribe("loc://long_string_str(\"x\")");
        s.write_long_string("y");
        assert_eq!(s.state().value, Value::Str("y".into()));
    }

    #[test]
    fn a_write_with_no_task_to_send_it_is_reported() {
        let pvs = PvManager::new();
        let pv = pvs.subscribe("sim://sine");
        let (tx, rx) = write_queue();
        drop(rx);
        pv.0.shared.set_writer(Writer::Channel(tx));
        pv.0.shared.update(|s| s.connected = true);
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
}
