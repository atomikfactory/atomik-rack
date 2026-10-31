//! Single-device test harness: feed batches and notes to one device and read its outputs.

use crate::no_alloc::assert_no_alloc;
use atomik_core::{
    BATCH, Batch, MAX_EVENTS_PER_BATCH, NoteBuffer, NoteEvent, SampleRate, Tempo, TimeInfo,
};
use atomik_device_sdk::{Device, PrepareInfo, ProcessContext, SocketId, SocketKind};

/// Drives one device batch by batch. Every `process` call runs under [`assert_no_alloc`],
/// so the test binary must use [`crate::install_alloc_guard!`].
pub struct DeviceHarness {
    device: Box<dyn Device>,
    signals: Vec<Batch>,
    notes: Vec<NoteBuffer>,
    connected: Vec<bool>,
    time: TimeInfo,
    playback: Option<Vec<Batch>>,
}

impl DeviceHarness {
    /// Prepares `device` at `sample_rate` (tempo 120 BPM, seed 0). All sockets start disconnected.
    pub fn new(device: Box<dyn Device>, sample_rate: f64) -> Self {
        Self::with_options(device, sample_rate, 120.0, 0)
    }

    /// Like [`DeviceHarness::new`] with explicit tempo and seed.
    pub fn with_options(
        mut device: Box<dyn Device>,
        sample_rate: f64,
        bpm: f64,
        seed: u64,
    ) -> Self {
        let sample_rate = SampleRate::new(sample_rate).expect("valid sample rate");
        device.prepare(&PrepareInfo { sample_rate, seed });
        let sockets = device.descriptor().sockets;
        let notes = sockets
            .iter()
            .map(|s| {
                NoteBuffer::with_capacity(if s.kind == SocketKind::Notes {
                    MAX_EVENTS_PER_BATCH
                } else {
                    0
                })
            })
            .collect();
        Self {
            signals: vec![[0.0; BATCH]; sockets.len()],
            notes,
            connected: vec![false; sockets.len()],
            time: TimeInfo::at(0, sample_rate, Tempo::new(bpm).expect("valid tempo"), true),
            playback: None,
            device,
        }
    }

    fn socket(&self, key: &str) -> SocketId {
        self.device
            .descriptor()
            .socket_by_key(key)
            .unwrap_or_else(|| panic!("unknown socket `{key}`"))
            .id
    }

    /// Marks a socket as connected or not.
    pub fn set_connected(&mut self, key: &str, connected: bool) {
        let id = self.socket(key);
        self.connected[id.0 as usize] = connected;
    }

    /// Sets a parameter by key (clamped by its descriptor). Panics on unknown keys.
    pub fn set_param(&mut self, key: &str, plain: f32) {
        let p = self
            .device
            .descriptor()
            .param_by_key(key)
            .unwrap_or_else(|| panic!("unknown param `{key}`"));
        let (id, value) = (p.id, p.clamp(plain));
        self.device.set_param(id, value);
    }

    /// Mutable access to an input batch; marks the socket connected. Inputs persist across batches.
    pub fn input_mut(&mut self, key: &str) -> &mut Batch {
        let id = self.socket(key).0 as usize;
        self.connected[id] = true;
        &mut self.signals[id]
    }

    /// Sets the playback batches (one per channel) passed to the device with
    /// `ProcessContext::with_playback` on every following `process` call, until
    /// [`DeviceHarness::clear_playback`].
    pub fn set_playback(&mut self, channels: &[Batch]) {
        self.playback = Some(channels.to_vec());
    }

    /// Stops passing playback (the device sees `None` again).
    pub fn clear_playback(&mut self) {
        self.playback = None;
    }

    /// Queues a note for the next `process` call; marks the socket connected.
    pub fn push_note(&mut self, key: &str, event: NoteEvent) {
        let id = self.socket(key).0 as usize;
        self.connected[id] = true;
        self.notes[id].push(event).expect("note buffer full");
    }

    /// Processes one batch under the allocation guard, then advances time by [`BATCH`] frames.
    pub fn process(&mut self) {
        self.step(true);
    }

    /// [`DeviceHarness::process`] without the allocation guard, for benchmarks (the guard's
    /// bookkeeping would be part of the measurement, and a bench binary installs no guard).
    pub fn process_unguarded(&mut self) {
        self.step(false);
    }

    fn step(&mut self, guarded: bool) {
        let sockets = self.device.descriptor().sockets;
        for (i, s) in sockets.iter().enumerate() {
            if s.direction == atomik_device_sdk::Direction::Output {
                self.signals[i] = [0.0; BATCH];
                self.notes[i].clear();
            }
        }
        let device = &mut self.device;
        let mut ctx = ProcessContext::new(
            self.time,
            &mut self.signals,
            &mut self.notes,
            &self.connected,
        )
        .with_playback(self.playback.as_deref().unwrap_or(&[]));
        if guarded {
            assert_no_alloc(|| device.process(&mut ctx));
        } else {
            device.process(&mut ctx);
        }
        for (i, s) in sockets.iter().enumerate() {
            if s.direction.is_input() {
                self.notes[i].clear();
            }
        }
        self.time.frame += BATCH as u64;
        self.time.tick += BATCH as f64 * self.time.ticks_per_frame;
        self.time.loop_wrap = None;
    }

    /// The transport info the next `process` call will pass: tests set `tick`,
    /// `playing`, `tempo`/`ticks_per_frame` or a one-shot `loop_wrap` (cleared after each
    /// `process`) to script locates, stops and tempo changes. After each batch `frame` and
    /// `tick` advance by one batch at the batch's `ticks_per_frame`.
    pub fn time_mut(&mut self) -> &mut TimeInfo {
        &mut self.time
    }

    /// The last output batch of a socket.
    pub fn output(&self, key: &str) -> &Batch {
        &self.signals[self.socket(key).0 as usize]
    }

    /// The notes emitted on an output socket in the last batch.
    pub fn notes_out(&self, key: &str) -> &[NoteEvent] {
        self.notes[self.socket(key).0 as usize].as_slice()
    }

    /// Processes `batches` batches and returns the concatenated samples of each output key.
    pub fn render(&mut self, batches: usize, outputs: &[&str]) -> Vec<Vec<f32>> {
        let mut out: Vec<Vec<f32>> = outputs
            .iter()
            .map(|_| Vec::with_capacity(batches * BATCH))
            .collect();
        for _ in 0..batches {
            self.process();
            for (dst, key) in out.iter_mut().zip(outputs) {
                dst.extend_from_slice(self.output(key));
            }
        }
        out
    }

    /// The device under test.
    pub fn device(&self) -> &dyn Device {
        self.device.as_ref()
    }

    /// Mutable access to the device under test (e.g. to call `reset`).
    pub fn device_mut(&mut self) -> &mut dyn Device {
        self.device.as_mut()
    }

    /// Current absolute frame (start of the next batch).
    pub fn frame(&self) -> u64 {
        self.time.frame
    }
}
