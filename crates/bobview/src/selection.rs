//! The primary selection: the text a middle-click pastes on X11 and
//! Wayland, which a middle-click on a widget sets to its PV name, as in EDM
//! and MEDM, and a middle-click on a trend reads to add the PVs it names.
//! egui sets only the clipboard, and reads neither.

use std::sync::mpsc;

/// Somewhere to put the primary selection: the desktop's, or a test's.
pub trait Primary: Send {
    /// Make `text` the primary selection, or say why it can't be.
    fn set(&mut self, text: &str) -> Result<(), String>;
}

/// The desktop's primary selection, where it has one.
pub fn system() -> Box<dyn Primary> {
    Box::new(System::default())
}

/// The X11 or Wayland primary selection. The connection is made at the
/// first copy, and kept: on X11 the selection is served by this process, so
/// it lasts until another application takes it or bobview exits. Windows,
/// macOS and the browser have none, so there the clipboard alone has the
/// text.
#[derive(Default)]
struct System {
    #[cfg(all(unix, not(target_os = "macos")))]
    clipboard: Option<arboard::Clipboard>,
}

impl Primary for System {
    #[cfg(all(unix, not(target_os = "macos")))]
    fn set(&mut self, text: &str) -> Result<(), String> {
        use arboard::{LinuxClipboardKind, SetExtLinux};
        let clipboard = match &mut self.clipboard {
            Some(c) => c,
            None => self
                .clipboard
                .insert(arboard::Clipboard::new().map_err(|e| e.to_string())?),
        };
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text)
            .map_err(|e| e.to_string())
    }

    #[cfg(not(all(unix, not(target_os = "macos"))))]
    fn set(&mut self, _text: &str) -> Result<(), String> {
        Err("no primary selection here".into())
    }
}

/// No primary selection, for snapshots.
#[cfg(not(target_arch = "wasm32"))]
pub struct NoPrimary;

#[cfg(not(target_arch = "wasm32"))]
impl Primary for NoPrimary {
    fn set(&mut self, _text: &str) -> Result<(), String> {
        Err("not set in a snapshot".into())
    }
}

/// Somewhere to read the primary selection from: the desktop's, or a
/// test's.
pub trait ReadPrimary {
    /// The primary selection's text, `None` when nothing is selected or it
    /// can't be read.
    fn get(&mut self) -> Option<String>;
}

/// Makes a [`ReadPrimary`] on a [`Reader`]'s thread.
pub type MakeRead = Box<dyn FnOnce() -> Box<dyn ReadPrimary> + Send>;

/// The desktop's primary selection to read, where it has one: X11 and
/// Wayland, through arboard as [`system`] writes it. `None` on Windows,
/// macOS and in the browser, which have no primary selection.
pub fn system_reader() -> Option<MakeRead> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(Box::new(|| Box::new(SystemRead::default())))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        None
    }
}

/// The X11 or Wayland primary selection, read on a [`Reader`]'s thread.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Default)]
struct SystemRead {
    clipboard: Option<arboard::Clipboard>,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl ReadPrimary for SystemRead {
    fn get(&mut self) -> Option<String> {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        let clipboard = match &mut self.clipboard {
            Some(c) => c,
            None => self.clipboard.insert(
                arboard::Clipboard::new()
                    .inspect_err(|e| log::debug!("no primary selection: {e}"))
                    .ok()?,
            ),
        };
        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .inspect_err(|e| log::debug!("could not read the primary selection: {e}"))
            .ok()
            .filter(|t| !t.trim().is_empty())
    }
}

/// A [`ReadPrimary`] on a thread of its own, as reading can be slow: X11
/// asks the selection's owner, which may hang, for seconds. Reads answer
/// later, tagged with what asked, and never block the caller.
///
/// Reads run one at a time, so one stuck on a hung owner holds up those
/// after it, which the caller times out. Dropped, it stops taking reads
/// and leaves the thread to end after the one in hand (if any), without
/// waiting for it.
pub struct Reader<T> {
    jobs: mpsc::Sender<T>,
    answers: mpsc::Receiver<(T, Option<String>)>,
}

impl<T: Clone + Send + 'static> Reader<T> {
    /// Start the thread, making the selection there with `make`; `wake`
    /// is called with each answer's tag once it is in (to repaint).
    pub fn spawn(make: MakeRead, wake: impl Fn(&T) + Send + 'static) -> Self {
        let (jobs, rx) = mpsc::channel::<T>();
        let (tx, answers) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("bobview-selection".into())
            .spawn(move || {
                let mut primary = make();
                for tag in rx {
                    let text = primary.get();
                    if tx.send((tag.clone(), text)).is_err() {
                        break;
                    }
                    wake(&tag);
                }
            });
        if let Err(e) = spawned {
            log::warn!("no thread to read the primary selection: {e}");
        }
        Self { jobs, answers }
    }

    /// Read the primary selection; the answer comes with `tag`.
    pub fn read(&self, tag: T) {
        let _ = self.jobs.send(tag);
    }

    /// An answer, if one has come.
    pub fn answer(&self) -> Option<(T, Option<String>)> {
        self.answers.try_recv().ok()
    }
}
