//! Control surfaces: a driver ([`atomik_midi_io::surface`]) joined to a pair of MIDI ports.
//!
//! The driver is plain Rust; this module owns what it talks through. A [`SurfaceLink`] is the
//! two ports as the host sees them: raw messages coming in, raw messages going out.
//! [`SystemLink`] opens real ports with midir (the listener keeps SysEx; its callback only
//! queues the bytes, so the UI thread does the work), [`LoopbackSurface`] is the same thing in
//! memory for tests and for the MCP server's headless session. [`SurfaceHost`] is a driver and
//! its link with the throttle of the feedback.
//!
//! Everything runs on the UI thread, never the audio thread: input is polled and the feedback
//! sent from [`crate::AppCore::surface_frame`], at most [`FEEDBACK_PERIOD`] apart, and the
//! driver sends only what changed.

use atomik_midi_io::MidiError;
use atomik_midi_io::ports::{MidiListener, MidiSender, listen, open_output};
use atomik_midi_io::surface::SurfaceDriver;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

/// The shortest time between two feedback passes, in seconds (30 per second: motors and
/// displays need no more, and a busy engine needs no more traffic).
pub const FEEDBACK_PERIOD: f64 = 1.0 / 30.0;

/// The most messages kept from a surface between two frames.
const QUEUE_CAPACITY: usize = 4096;

/// The two ports of a surface as the host sees them.
pub trait SurfaceLink: Send {
    /// Moves the messages that arrived since the last call into `out`, oldest first.
    fn poll(&mut self, out: &mut Vec<Vec<u8>>);
    /// Sends one message (a channel message or a whole SysEx frame) to the surface.
    fn send(&mut self, bytes: &[u8]);
    /// What the link is, for messages (`Port A in, Port B out`).
    fn describe(&self) -> String;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The real ports, opened by name with midir.
pub struct SystemLink {
    _listener: Option<MidiListener>,
    output: Option<MidiSender>,
    queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    label: String,
}

impl std::fmt::Debug for SystemLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemLink")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl SystemLink {
    /// Opens the input and output ports named in the settings (either may be missing); a port
    /// that cannot be opened is left out and named in the second value.
    pub fn open(input: Option<&str>, output: Option<&str>) -> (Self, Vec<String>) {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let mut problems = Vec::new();
        let mut label = Vec::new();
        let listener = input.and_then(|name| {
            let q = Arc::clone(&queue);
            match listen(name, move |bytes| {
                let mut q = lock(&q);
                if q.len() < QUEUE_CAPACITY {
                    q.push_back(bytes.to_vec());
                }
            }) {
                Ok(l) => {
                    label.push(format!("{} in", l.port_name()));
                    Some(l)
                }
                Err(e) => {
                    problems.push(describe_error("input", &e));
                    None
                }
            }
        });
        let out = output.and_then(|name| match open_output(name) {
            Ok(o) => {
                label.push(format!("{} out", o.port_name()));
                Some(o)
            }
            Err(e) => {
                problems.push(describe_error("output", &e));
                None
            }
        });
        (
            Self {
                _listener: listener,
                output: out,
                queue,
                label: label.join(", "),
            },
            problems,
        )
    }

    /// True when at least one port is open.
    pub fn is_open(&self) -> bool {
        self._listener.is_some() || self.output.is_some()
    }
}

fn describe_error(what: &str, e: &MidiError) -> String {
    format!("surface {what}: {e}")
}

impl SurfaceLink for SystemLink {
    fn poll(&mut self, out: &mut Vec<Vec<u8>>) {
        out.extend(lock(&self.queue).drain(..));
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Some(o) = &mut self.output {
            // A port that refuses a message (unplugged) is not worth a message per frame.
            let _ = o.send(bytes);
        }
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

/// A surface in memory: what a test or a headless session plays the surface with. Clones share
/// the queues.
#[derive(Debug, Clone, Default)]
pub struct LoopbackSurface {
    input: Arc<Mutex<VecDeque<Vec<u8>>>>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl LoopbackSurface {
    /// An empty loopback.
    pub fn new() -> Self {
        Self::default()
    }

    /// A message from the surface to the host.
    pub fn feed(&self, bytes: &[u8]) {
        lock(&self.input).push_back(bytes.to_vec());
    }

    /// The messages the host sent to the surface since the last call.
    pub fn take_sent(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *lock(&self.sent))
    }
}

impl SurfaceLink for LoopbackSurface {
    fn poll(&mut self, out: &mut Vec<Vec<u8>>) {
        out.extend(lock(&self.input).drain(..));
    }

    fn send(&mut self, bytes: &[u8]) {
        lock(&self.sent).push(bytes.to_vec());
    }

    fn describe(&self) -> String {
        "loopback".to_owned()
    }
}

/// A driver with its ports and the feedback throttle.
pub struct SurfaceHost {
    /// The surface's name (its settings name): what mappings and locks refer to.
    pub(crate) id: String,
    /// The input port's name, as the mappings of a generic controller see it.
    pub(crate) port: std::sync::Arc<str>,
    /// Running status of the messages of a generic controller.
    pub(crate) parser: atomik_midi_io::RunningStatus,
    pub(crate) driver: Box<dyn SurfaceDriver>,
    pub(crate) link: Box<dyn SurfaceLink>,
    /// The time before which no feedback is sent.
    pub(crate) next_feedback: f64,
    /// Messages sent to the surface in all.
    pub(crate) sent: u64,
    /// Messages received from the surface in all.
    pub(crate) received: u64,
    /// The driver has been connected (the first frame does it).
    pub(crate) connected: bool,
}

impl std::fmt::Debug for SurfaceHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceHost")
            .field("driver", &self.driver.name())
            .field("link", &self.link.describe())
            .finish_non_exhaustive()
    }
}

impl SurfaceHost {
    /// A host that connects the driver on its first frame, named after its driver.
    pub fn new(driver: Box<dyn SurfaceDriver>, link: Box<dyn SurfaceLink>) -> Self {
        let id = driver.name().to_owned();
        Self {
            port: std::sync::Arc::from(id.as_str()),
            id,
            parser: atomik_midi_io::RunningStatus::default(),
            driver,
            link,
            next_feedback: 0.0,
            sent: 0,
            received: 0,
            connected: false,
        }
    }

    /// Names the surface `id` (the name of its settings entry) and its input port `port`, which
    /// the mappings of a generic controller see messages arrive from.
    #[must_use]
    pub fn named(mut self, id: impl Into<String>, port: &str) -> Self {
        self.id = id.into();
        self.port = std::sync::Arc::from(port);
        self
    }

    /// The surface's name.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The driver's short name.
    pub fn driver_name(&self) -> &'static str {
        self.driver.name()
    }

    /// The ports, as text.
    pub fn describe(&self) -> String {
        self.link.describe()
    }

    /// Messages sent to the surface so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Messages received from the surface so far.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Sends every message of `out`.
    pub(crate) fn send_all(&mut self, out: &mut Vec<Vec<u8>>) {
        for m in out.drain(..) {
            self.link.send(&m);
            self.sent += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loopback_passes_messages_both_ways() {
        let surface = LoopbackSurface::new();
        let mut link = surface.clone();
        surface.feed(&[0x90, 1, 127]);
        surface.feed(&[0xE0, 0, 64]);
        let mut got = Vec::new();
        link.poll(&mut got);
        assert_eq!(got, vec![vec![0x90, 1, 127], vec![0xE0, 0, 64]]);
        link.poll(&mut got);
        assert_eq!(got.len(), 2, "polled once");
        link.send(&[0xB0, 0x30, 5]);
        assert_eq!(surface.take_sent(), vec![vec![0xB0, 0x30, 5]]);
        assert!(surface.take_sent().is_empty());
        assert_eq!(link.describe(), "loopback");
    }

    #[test]
    fn a_system_link_with_no_ports_named_is_closed() {
        let (link, problems) = SystemLink::open(None, None);
        assert!(!link.is_open());
        assert!(problems.is_empty());
        assert_eq!(link.describe(), "");
    }

    #[test]
    fn a_missing_port_is_reported_and_left_out() {
        let (link, problems) =
            SystemLink::open(Some("No Such Surface Port"), Some("No Such Surface Port"));
        assert!(!link.is_open());
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("surface input"));
        assert!(problems[1].starts_with("surface output"));
    }
}
