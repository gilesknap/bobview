//! The primary selection: the text a middle-click pastes on X11 and
//! Wayland, which a middle-click on a widget sets to its PV name, as in EDM
//! and MEDM. egui sets only the clipboard.

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
