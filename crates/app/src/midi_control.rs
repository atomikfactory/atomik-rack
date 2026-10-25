//! The control path of live MIDI: what the MIDI thread hands to the UI thread, and what the
//! UI thread tells the MIDI thread to keep for itself.
//!
//! Every parsed message of the MIDI input goes into the [`ControlHub`]'s inbox, a bounded queue
//! behind a mutex (the MIDI thread is not the audio thread, so a short lock is fine there; the
//! audio thread never sees the hub). The UI thread drains it once per frame, runs the messages
//! through the project's mapping table and turns them into commands, so a mapped move is an
//! ordinary edit: it is validated, undoable, smoothed by the device's own smoothers, and
//! recorded as automation like a mouse drag.
//!
//! The hub also holds what the MIDI thread must know to **consume** a message: the sources of
//! the mappings that consume (their messages must not also play notes into the armed track),
//! and the learn flag (while learning, everything is consumed). The UI thread replaces the list
//! whenever the mappings change; the MIDI thread reads it per message.

use atomik_core::ChannelMessage;
use atomik_model::MidiSource;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The most messages waiting for the UI thread; the newest are dropped beyond it.
pub const INBOX_CAPACITY: usize = 4096;

/// One message from a MIDI input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// The input port's name.
    pub port: Arc<str>,
    /// The message.
    pub message: ChannelMessage,
}

/// The queue between the MIDI thread and the UI thread, and the consume list.
#[derive(Debug, Default)]
pub struct ControlHub {
    inbox: Mutex<VecDeque<Inbound>>,
    dropped: AtomicU64,
    consume: Mutex<Arc<Vec<MidiSource>>>,
    learning: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ControlHub {
    /// An empty hub (nothing consumed, not learning).
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a message for the UI thread (MIDI thread). A full inbox drops it and counts it.
    pub fn push(&self, port: &Arc<str>, message: ChannelMessage) {
        let mut inbox = lock(&self.inbox);
        if inbox.len() >= INBOX_CAPACITY {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        inbox.push_back(Inbound {
            port: Arc::clone(port),
            message,
        });
    }

    /// Moves the queued messages into `out`, oldest first (UI thread).
    pub fn drain(&self, out: &mut Vec<Inbound>) {
        out.extend(lock(&self.inbox).drain(..));
    }

    /// Messages dropped because the inbox was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Replaces the sources whose messages are consumed (UI thread).
    pub fn set_consumed(&self, sources: Vec<MidiSource>) {
        *lock(&self.consume) = Arc::new(sources);
    }

    /// Switches learning on or off: while on, every message is consumed.
    pub fn set_learning(&self, on: bool) {
        self.learning.store(on, Ordering::Relaxed);
    }

    /// True while learning.
    pub fn is_learning(&self) -> bool {
        self.learning.load(Ordering::Relaxed)
    }

    /// True when `message` from `port` must not reach the notes of the armed instrument: while
    /// learning, or when a mapping that consumes is for it (MIDI thread).
    pub fn consumes(&self, port: &str, message: &ChannelMessage) -> bool {
        if self.is_learning() {
            return true;
        }
        let rules = Arc::clone(&lock(&self.consume));
        rules.iter().any(|s| s.matches(port, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::SourceKind;

    fn port() -> Arc<str> {
        Arc::from("Pad")
    }

    #[test]
    fn messages_come_out_in_order_and_the_inbox_is_bounded() {
        let hub = ControlHub::new();
        for i in 0..3 {
            hub.push(&port(), ChannelMessage::cc(0, 7, i));
        }
        let mut out = Vec::new();
        hub.drain(&mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].message, ChannelMessage::cc(0, 7, 2));
        assert_eq!(&*out[0].port, "Pad");
        hub.drain(&mut out);
        assert_eq!(out.len(), 3, "drained once");
        for _ in 0..INBOX_CAPACITY + 5 {
            hub.push(&port(), ChannelMessage::cc(0, 1, 1));
        }
        assert_eq!(hub.dropped(), 5);
        let mut out = Vec::new();
        hub.drain(&mut out);
        assert_eq!(out.len(), INBOX_CAPACITY);
    }

    #[test]
    fn a_message_is_consumed_when_a_rule_matches_or_while_learning() {
        let hub = ControlHub::new();
        let msg = ChannelMessage::cc(2, 7, 1);
        assert!(!hub.consumes("Pad", &msg));
        hub.set_consumed(vec![MidiSource {
            port: Some("Pad".into()),
            channel: Some(2),
            kind: SourceKind::Cc(7),
        }]);
        assert!(hub.consumes("Pad", &msg));
        assert!(!hub.consumes("Other", &msg));
        assert!(!hub.consumes("Pad", &ChannelMessage::cc(3, 7, 1)));
        assert!(!hub.consumes("Pad", &ChannelMessage::note_on(2, 60, 90)));
        hub.set_consumed(Vec::new());
        assert!(!hub.consumes("Pad", &msg));
        hub.set_learning(true);
        assert!(hub.is_learning());
        assert!(hub.consumes("Anything", &ChannelMessage::note_on(9, 1, 1)));
        hub.set_learning(false);
        assert!(!hub.consumes("Pad", &msg));
    }
}
