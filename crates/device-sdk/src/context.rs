//! The per-batch processing context handed to [`crate::Device::process`].

use crate::SocketId;
use atomik_core::{Batch, NoteBuffer, NoteEvent, TimeInfo};

/// Borrowed view of one device's sockets for one batch. Contains no owned or growable data.
///
/// All slices are indexed by [`SocketId`] and have one entry per socket in the device's
/// descriptor. Signal entries of Notes sockets and note entries of Audio/Cv sockets are unused.
pub struct ProcessContext<'a> {
    /// Transport information for this batch.
    pub time: TimeInfo,
    signals: &'a mut [Batch],
    notes: &'a mut [NoteBuffer],
    connected: &'a [bool],
    playback: &'a [Batch],
}

impl<'a> ProcessContext<'a> {
    /// Builds a context. Used by the engine and the test kit.
    ///
    /// # Panics
    /// If the three slices do not have equal length.
    pub fn new(
        time: TimeInfo,
        signals: &'a mut [Batch],
        notes: &'a mut [NoteBuffer],
        connected: &'a [bool],
    ) -> Self {
        assert!(
            signals.len() == notes.len() && notes.len() == connected.len(),
            "ProcessContext slices must have equal length"
        );
        Self {
            time,
            signals,
            notes,
            connected,
            playback: &[],
        }
    }

    /// Attaches the audio the engine rendered for this device in this batch (audio-track clips,
    /// one batch per channel, channel 0 left. Without it [`ProcessContext::playback`]
    /// returns `None`.
    #[must_use]
    pub fn with_playback(mut self, playback: &'a [Batch]) -> Self {
        self.playback = playback;
        self
    }

    /// The rendered playback batch of `channel`, or `None` if the engine rendered no playback
    /// for this device (or fewer channels).
    pub fn playback(&self, channel: usize) -> Option<&Batch> {
        self.playback.get(channel)
    }

    /// The batch of an Audio or Cv input socket (zeros when unconnected).
    pub fn input(&self, socket: SocketId) -> &Batch {
        &self.signals[socket.0 as usize]
    }

    /// The batch of an Audio or Cv output socket (arrives zeroed).
    pub fn output(&mut self, socket: SocketId) -> &mut Batch {
        &mut self.signals[socket.0 as usize]
    }

    /// The note events received on a Notes input socket this batch.
    pub fn notes_in(&self, socket: SocketId) -> &[NoteEvent] {
        self.notes[socket.0 as usize].as_slice()
    }

    /// The note buffer of a Notes output socket (arrives cleared).
    pub fn notes_out(&mut self, socket: SocketId) -> &mut NoteBuffer {
        &mut self.notes[socket.0 as usize]
    }

    /// Whether a cable is attached to the socket.
    pub fn is_connected(&self, socket: SocketId) -> bool {
        self.connected[socket.0 as usize]
    }

    /// Number of sockets in this context.
    pub fn socket_count(&self) -> usize {
        self.connected.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::{BATCH, NoteKind, SampleRate, Tempo};

    fn time() -> TimeInfo {
        TimeInfo::at(
            0,
            SampleRate::new(48_000.0).unwrap(),
            Tempo::new(120.0).unwrap(),
            true,
        )
    }

    #[test]
    fn context_unconnected_input_reads_zero() {
        let mut signals = vec![[0.0; BATCH]; 2];
        let mut notes = vec![NoteBuffer::with_capacity(0), NoteBuffer::with_capacity(0)];
        let connected = [false, true];
        let ctx = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
        assert!(ctx.input(SocketId(0)).iter().all(|s| s.to_bits() == 0));
        assert!(!ctx.is_connected(SocketId(0)));
        assert!(ctx.is_connected(SocketId(1)));
        assert_eq!(ctx.socket_count(), 2);
    }

    #[test]
    fn context_output_is_writable_per_socket() {
        let mut signals = vec![[0.0; BATCH]; 2];
        let mut notes = vec![NoteBuffer::with_capacity(0), NoteBuffer::with_capacity(0)];
        let connected = [true, true];
        {
            let mut ctx = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
            ctx.output(SocketId(1))[3] = 0.5;
        }
        assert!((signals[1][3] - 0.5).abs() < f32::EPSILON);
        assert!(signals[0].iter().all(|s| s.to_bits() == 0));
    }

    #[test]
    fn context_notes_in_and_out() {
        let mut signals = vec![[0.0; BATCH]; 2];
        let mut inbox = NoteBuffer::with_capacity(4);
        inbox
            .push(NoteEvent {
                offset: 2,
                kind: NoteKind::Off { key: 1 },
            })
            .unwrap();
        let mut notes = vec![inbox, NoteBuffer::with_capacity(4)];
        let connected = [true, true];
        let mut ctx = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
        assert_eq!(ctx.notes_in(SocketId(0)).len(), 1);
        ctx.notes_out(SocketId(1))
            .push(NoteEvent {
                offset: 0,
                kind: NoteKind::AllOff,
            })
            .unwrap();
        assert_eq!(ctx.notes_out(SocketId(1)).len(), 1);
    }

    #[test]
    fn context_playback_is_none_without_with_playback() {
        let mut signals = vec![[0.0; BATCH]; 1];
        let mut notes = vec![NoteBuffer::with_capacity(0)];
        let connected = [true];
        let ctx = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
        assert!(ctx.playback(0).is_none());
        assert!(ctx.playback(7).is_none());
    }

    #[test]
    fn context_with_playback_exposes_channels_and_none_beyond() {
        let mut signals = vec![[0.0; BATCH]; 1];
        let mut notes = vec![NoteBuffer::with_capacity(0)];
        let connected = [true];
        let pb = [[0.25; BATCH], [0.5; BATCH]];
        let ctx =
            ProcessContext::new(time(), &mut signals, &mut notes, &connected).with_playback(&pb);
        assert_eq!(ctx.playback(0), Some(&pb[0]));
        assert_eq!(ctx.playback(1), Some(&pb[1]));
        assert!(ctx.playback(2).is_none());
        let empty =
            ProcessContext::new(time(), &mut signals, &mut notes, &connected).with_playback(&[]);
        assert!(empty.playback(0).is_none());
    }

    #[test]
    #[should_panic(expected = "equal length")]
    fn context_new_rejects_mismatched_lengths() {
        let mut signals = vec![[0.0; BATCH]; 2];
        let mut notes = vec![NoteBuffer::with_capacity(0)];
        let connected = [true, true];
        let _ = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
    }
}
