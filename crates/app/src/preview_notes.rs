//! The notes of a library preview that is played by hand: a small queue between the threads that
//! take notes (the MIDI input, the UI thread) and the worker that renders the previewed patch.
//!
//! While a patch of the library is selected the notes target of the app is [`PREVIEW_TARGET`], a
//! device id no engine ever holds. The live note producer ([`crate::midi::MidiLink`]) sees that
//! id and queues the event here instead of in the engine, so the keys of a MIDI keyboard and of
//! the Piano Keys strip take the same path as always up to that point, and the note-off of a
//! key goes where its note-on went even when the selection left the patch while it was held.
//! Nothing here is on the audio thread: the queue is bounded and preallocated, a full queue
//! drops the event.

use crate::target::Target;
use atomik_core::NoteKind;
use atomik_device_sdk::SocketId;
use atomik_engine::DeviceInstanceId;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// The notes target while a library preview takes the keyboard: no engine device has this id.
pub const PREVIEW_TARGET: Target = (DeviceInstanceId(u32::MAX), SocketId(0));

/// How many events wait for the worker at most.
const CAPACITY: usize = 256;

/// The queue of a library preview's notes.
#[derive(Debug)]
pub struct PreviewNotes {
    queue: Mutex<VecDeque<NoteKind>>,
    wake: Condvar,
}

impl Default for PreviewNotes {
    fn default() -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            wake: Condvar::new(),
        }
    }
}

impl PreviewNotes {
    /// Queues one event for the worker; `false` when the queue is full (the event is dropped).
    pub fn push(&self, kind: NoteKind) -> bool {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() >= CAPACITY {
            return false;
        }
        queue.push_back(kind);
        drop(queue);
        self.wake.notify_one();
        true
    }

    /// The oldest event, if any.
    pub fn pop(&self) -> Option<NoteKind> {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// The oldest event, waiting up to `timeout` for one.
    pub fn wait(&self, timeout: Duration) -> Option<NoteKind> {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut queue, _) = self
            .wake
            .wait_timeout_while(queue, timeout, |q| q.is_empty())
            .unwrap_or_else(PoisonError::into_inner);
        queue.pop_front()
    }

    /// Forgets every waiting event.
    pub fn clear(&self) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// How many events wait.
    pub fn len(&self) -> usize {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether no event waits.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(key: u8) -> NoteKind {
        NoteKind::On { key, velocity: 0.5 }
    }

    #[test]
    fn events_come_out_in_order_and_a_full_queue_drops() {
        let q = PreviewNotes::default();
        assert!(q.is_empty());
        for key in 0..=u8::MAX {
            assert_eq!(q.push(on(key)), usize::from(key) < CAPACITY, "{key}");
        }
        assert_eq!(q.len(), CAPACITY);
        assert_eq!(q.pop(), Some(on(0)));
        assert_eq!(q.pop(), Some(on(1)));
        q.clear();
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn a_waiting_reader_wakes_for_a_push_and_gives_up_when_none_comes() {
        let q = std::sync::Arc::new(PreviewNotes::default());
        assert_eq!(q.wait(Duration::from_millis(1)), None);
        let reader = {
            let q = std::sync::Arc::clone(&q);
            std::thread::spawn(move || q.wait(Duration::from_secs(20)))
        };
        std::thread::sleep(Duration::from_millis(5));
        assert!(q.push(on(60)));
        assert_eq!(reader.join().unwrap(), Some(on(60)));
    }
}
