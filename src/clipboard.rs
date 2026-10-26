//! The system clipboard and the primary selection (select to copy, middle-click to paste).

use winit::window::Window;

/// Wayland's clipboard and primary selection. Without a Wayland display there's no clipboard.
#[cfg(not(windows))]
pub struct Clipboard(smithay_clipboard::Clipboard);

#[cfg(not(windows))]
impl Clipboard {
    pub fn new(window: &Window) -> Option<Clipboard> {
        use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        match window.display_handle().map(|h| h.as_raw()) {
            Ok(RawDisplayHandle::Wayland(h)) => Some(Clipboard(unsafe {
                smithay_clipboard::Clipboard::new(h.display.as_ptr())
            })),
            _ => None,
        }
    }

    pub fn load(&self) -> std::io::Result<String> {
        self.0.load()
    }

    pub fn store(&self, text: String) {
        self.0.store(text)
    }

    pub fn load_primary(&self) -> std::io::Result<String> {
        self.0.load_primary()
    }

    pub fn store_primary(&self, text: String) {
        self.0.store_primary(text)
    }
}

/// The Windows clipboard. Windows has no primary selection, so it's kept inside this process.
#[cfg(windows)]
pub struct Clipboard {
    system: std::sync::Mutex<arboard::Clipboard>,
    primary: std::sync::Mutex<String>,
}

#[cfg(windows)]
impl Clipboard {
    pub fn new(_window: &Window) -> Option<Clipboard> {
        let system = arboard::Clipboard::new()
            .map_err(|e| log::warn!("clipboard unavailable: {e}"))
            .ok()?;
        Some(Clipboard {
            system: std::sync::Mutex::new(system),
            primary: Default::default(),
        })
    }

    pub fn load(&self) -> std::io::Result<String> {
        let mut system = self.system.lock().unwrap_or_else(|e| e.into_inner());
        system.get_text().map_err(std::io::Error::other)
    }

    pub fn store(&self, text: String) {
        let mut system = self.system.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = system.set_text(text) {
            log::warn!("could not copy: {e}");
        }
    }

    pub fn load_primary(&self) -> std::io::Result<String> {
        Ok(self.primary.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    pub fn store_primary(&self, text: String) {
        *self.primary.lock().unwrap_or_else(|e| e.into_inner()) = text;
    }
}
