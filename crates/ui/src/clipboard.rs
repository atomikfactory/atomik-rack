//! The text clipboard of the UI thread.
//!
//! A text field copies, cuts and pastes through [`read`] and [`write`]. The hosting app installs
//! the operating system's clipboard with [`install`] on its UI thread; until it does (in tests,
//! and in a host without a clipboard) the text goes to a private buffer of the thread, so a copy
//! followed by a paste still works inside the app.
//!
//! The backend lives in a thread-local: the UI runs on one thread, and tests, which run one per
//! thread, never see each other's text.

use std::cell::RefCell;

/// A source and sink of clipboard text.
pub trait ClipboardBackend {
    /// The text on the clipboard, `None` when it holds none (or cannot be read).
    fn read(&mut self) -> Option<String>;
    /// Puts `text` on the clipboard; a failure is the backend's to log.
    fn write(&mut self, text: &str);
}

thread_local! {
    static BACKEND: RefCell<Option<Box<dyn ClipboardBackend>>> = const { RefCell::new(None) };
    static LOCAL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Makes `backend` the clipboard of this thread.
pub fn install(backend: Box<dyn ClipboardBackend>) {
    BACKEND.with(|b| *b.borrow_mut() = Some(backend));
}

/// The clipboard text, if any.
pub fn read() -> Option<String> {
    let from_backend = BACKEND.with(|b| b.borrow_mut().as_mut().map(|b| b.read()));
    match from_backend {
        Some(text) => text.filter(|t| !t.is_empty()),
        None => LOCAL.with(|l| Some(l.borrow().clone()).filter(|t| !t.is_empty())),
    }
}

/// Puts `text` on the clipboard.
pub fn write(text: &str) {
    let done = BACKEND.with(|b| b.borrow_mut().as_mut().map(|b| b.write(text)).is_some());
    if !done {
        LOCAL.with(|l| *l.borrow_mut() = text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn without_a_backend_the_text_stays_in_the_app() {
        assert_eq!(read(), None);
        write("hello");
        assert_eq!(read().as_deref(), Some("hello"));
    }

    #[test]
    fn an_installed_backend_takes_over() {
        struct Mem(Rc<RefCell<Option<String>>>);
        impl ClipboardBackend for Mem {
            fn read(&mut self) -> Option<String> {
                self.0.borrow().clone()
            }
            fn write(&mut self, text: &str) {
                *self.0.borrow_mut() = Some(text.to_owned());
            }
        }
        let cell = Rc::new(RefCell::new(None));
        install(Box::new(Mem(cell.clone())));
        write("os text");
        assert_eq!(cell.borrow().as_deref(), Some("os text"));
        assert_eq!(read().as_deref(), Some("os text"));
    }
}
