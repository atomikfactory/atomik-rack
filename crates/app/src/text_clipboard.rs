//! The operating system's text clipboard for the text fields of the UI.
//!
//! [`install`] opens the system clipboard on the UI thread and hands it to
//! `atomik_ui::clipboard`; a field's copy, cut and paste then go to and from other programs. When
//! the system has no clipboard to give (a headless Linux session) the UI keeps its private
//! buffer and a copy still pastes inside the app.

use atomik_ui::clipboard::{self, ClipboardBackend};

struct System(arboard::Clipboard);

impl ClipboardBackend for System {
    fn read(&mut self) -> Option<String> {
        // An empty or non-text clipboard is an error here, not a failure worth a log line.
        self.0.get_text().ok()
    }

    fn write(&mut self, text: &str) {
        if let Err(e) = self.0.set_text(text) {
            log::warn!("could not put text on the clipboard: {e}");
        }
    }
}

/// Connects the text fields of this thread to the system clipboard.
pub fn install() {
    match arboard::Clipboard::new() {
        Ok(c) => clipboard::install(Box::new(System(c))),
        Err(e) => log::warn!("no system clipboard, text stays inside the app: {e}"),
    }
}
