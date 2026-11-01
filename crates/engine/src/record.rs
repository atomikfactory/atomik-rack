//! The record tap: a lock-free path that copies selected host input
//! channels, or the outputs of routed devices, out of the audio callback, tagged with the engine
//! frame of every batch, for a non-RT writer thread (the app writes WAV files from it).
//!
//! # Design
//!
//! * A channel of a gained tap can read an output socket of a routed device instead of a host
//!   input ([`TapChannel::source`], resampling). The tap runs at the *end* of the batch, after
//!   the whole routing, so such a channel holds the 64 frames the device rendered in that batch
//!   (its latency against the song timeline is the path's delay compensation, which the app
//!   reads from the [`crate::PdcReport`]). A source that is not routed reads silence.
//! * `EngineHandle::record_tap(channels, capacity_frames)` (non-RT) allocates an SPSC ring of
//!   `u32` words and queues a [`TapRt`] through the ordered control queue, so the tap starts at
//!   a batch boundary; `EngineHandle::stop_record_tap` queues its removal the same way (the
//!   engine hands the old tap back instead of freeing it). One tap at a time: a new one
//!   replaces the old.
//! * At every batch the audio thread appends one *record* with a single wait-free commit:
//!   `[engine_frame as 2 words (low, high), then 64 words (f32 bits) per tapped channel]`.
//!   `engine_frame` is `Engine`'s batch frame (the timeline of `TransportState::engine_frame`
//!   and `SongSnapshot::position_at`), so the first frame of the record is host input frame
//!   `engine_frame` of the stream. Channels beyond the engine's input count read as silence.
//! * If the ring has no room for a whole record the batch is dropped and counted
//!   ([`RecordTap::overflow_frames`], in frames); nothing blocks or allocates.
//! * A tap made with `EngineHandle::record_tap_gained` sends every channel through a *strip*
//!   (one per recorded track) before it is written: the strip's gain, then, when its clip-safe
//!   switch is on, a safety limiter. See [`TapControls`].
//! * [`RecordTap::drain`] copies out whole records that are *contiguous* in engine frames, so
//!   the returned start tag is exact; a gap (overflow) ends the run, and the next call starts
//!   at the first frame after the gap.

use crate::arena::Arena;
use crate::graph::DeviceInstanceId;
use atomik_core::{BATCH, Batch};
use atomik_device_sdk::SocketId;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// One channel of a gained tap: the host input (or device output) it reads and the strip it
/// goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapChannel {
    /// Host input channel (0-based; a channel past the engine's inputs reads silence). Unused
    /// when `source` is set.
    pub host: usize,
    /// The strip (the recorded track) whose gain and limiter the channel goes through. The
    /// channels of one strip are limited together, so a stereo take keeps its image.
    pub strip: usize,
    /// An output socket of a routed device to read instead of a host input (resampling): the
    /// tap copies it at the end of every batch, after the whole routing has run. A device that
    /// is not in the arena or not routed, or a socket that is not an output, reads silence.
    pub source: Option<(DeviceInstanceId, SocketId)>,
}

impl TapChannel {
    /// A host input channel.
    pub fn host(host: usize, strip: usize) -> Self {
        Self {
            host,
            strip,
            source: None,
        }
    }

    /// The output `socket` of `device`.
    pub fn source(device: DeviceInstanceId, socket: SocketId, strip: usize) -> Self {
        Self {
            host: 0,
            strip,
            source: Some((device, socket)),
        }
    }
}

/// The live settings of the strips of a gained record tap: written by the app (any thread, any
/// time), read by the audio thread once per batch. Wait-free both ways.
///
/// # Gain
/// The linear gain of a strip is applied to the recorded signal first (before the limiter; the
/// track's monitor path applies the same gain, so a performer hears what the take gets). A
/// change glides: every batch the gain moves a quarter of the way to its target (a few
/// milliseconds), and inside a batch it ramps linearly from the old to the new value, so there
/// is no zipper and no step. The first batch of a tap starts at the target.
///
/// # Clip-safe
/// With the switch on, the strip's output never exceeds 0 dBFS: a feed-forward peak limiter
/// with instant attack and a release of about 40 ms (a time constant of
/// [`LIMIT_RELEASE_FRAMES`] frames) takes the gain down just enough that the loudest channel
/// of the strip at each frame is at most 1.0, and a final clamp makes the bound exact whatever
/// the rounding. It is deterministic (no look-ahead, one gain-reduction value of state per
/// strip) and it does not touch a take that stays below 0 dBFS, not by a bit. Without the
/// switch the take keeps its overs: the written files are 32-bit float, so nothing is lost, but
/// the take clips at the output.
pub struct TapControls {
    strips: Box<[StripControl]>,
}

struct StripControl {
    /// Linear gain, f32 bits.
    gain: AtomicU32,
    safe: AtomicBool,
}

/// Highest linear gain a strip takes (+36 dB, past the +24 dB of the model's range).
const MAX_GAIN: f32 = 64.0;

/// Time constant of the clip-safe release, in frames (about 43 ms at 48 kHz).
pub const LIMIT_RELEASE_FRAMES: f32 = 2048.0;

impl TapControls {
    /// `strips` strips at unity gain with clip-safe off.
    pub fn new(strips: usize) -> Self {
        Self {
            strips: (0..strips)
                .map(|_| StripControl {
                    gain: AtomicU32::new(1.0f32.to_bits()),
                    safe: AtomicBool::new(false),
                })
                .collect(),
        }
    }

    /// Number of strips.
    pub fn len(&self) -> usize {
        self.strips.len()
    }

    /// True when there are no strips.
    pub fn is_empty(&self) -> bool {
        self.strips.is_empty()
    }

    /// Sets the linear `gain` (clamped to `0..=64`; not a number reads 1) and the clip-safe
    /// switch of `strip`; a strip that does not exist is ignored.
    pub fn set(&self, strip: usize, gain: f32, clip_safe: bool) {
        let Some(s) = self.strips.get(strip) else {
            return;
        };
        let gain = if gain.is_nan() {
            1.0
        } else {
            gain.clamp(0.0, MAX_GAIN)
        };
        s.gain.store(gain.to_bits(), Ordering::Relaxed);
        s.safe.store(clip_safe, Ordering::Relaxed);
    }

    /// The linear gain and clip-safe switch of `strip` as last set (`None`: no such strip).
    pub fn get(&self, strip: usize) -> Option<(f32, bool)> {
        let s = self.strips.get(strip)?;
        Some((
            f32::from_bits(s.gain.load(Ordering::Relaxed)),
            s.safe.load(Ordering::Relaxed),
        ))
    }
}

impl std::fmt::Debug for TapControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TapControls")
            .field("strips", &self.strips.len())
            .finish()
    }
}

/// The audio thread's state of a gained tap.
struct Strips {
    controls: Arc<TapControls>,
    /// The strip of each tapped channel.
    of: Vec<usize>,
    /// Current gain per strip.
    gain: Vec<f32>,
    /// Gain at the end of this batch per strip (scratch).
    next: Vec<f32>,
    /// The limiter's gain reduction per strip (1 = none).
    limit: Vec<f32>,
    /// The first batch has not run: gains start at their targets.
    fresh: bool,
}

impl Strips {
    /// Moves every strip's gain towards its target for this batch.
    fn glide(&mut self) {
        for (s, c) in self.controls.strips.iter().enumerate() {
            let target = f32::from_bits(c.gain.load(Ordering::Relaxed));
            if self.fresh {
                self.gain[s] = target;
            }
            let from = self.gain[s];
            self.next[s] = if (target - from).abs() <= 1.0e-6 * target.abs().max(1.0e-3) {
                target
            } else {
                from + (target - from) * 0.25
            };
        }
        self.fresh = false;
    }

    /// The clip-safe pass over the words of one record (blocks of [`BATCH`] words per channel
    /// after the two header words).
    fn limit(&mut self, record: &mut [u32]) {
        for (s, c) in self.controls.strips.iter().enumerate() {
            if !c.safe.load(Ordering::Relaxed) {
                self.limit[s] = 1.0;
                continue;
            }
            let mut gr = self.limit[s];
            for n in 0..BATCH {
                let mut peak = 0.0f32;
                for (k, &of) in self.of.iter().enumerate() {
                    if of == s {
                        let v = f32::from_bits(record[2 + k * BATCH + n]);
                        peak = peak.max(if v.is_finite() { v.abs() } else { 0.0 });
                    }
                }
                let need = if peak > 1.0 { 1.0 / peak } else { 1.0 };
                gr = (gr + (1.0 - gr) / LIMIT_RELEASE_FRAMES).min(need);
                for (k, &of) in self.of.iter().enumerate() {
                    if of == s {
                        let w = &mut record[2 + k * BATCH + n];
                        let v = f32::from_bits(*w);
                        let v = if v.is_finite() { v * gr } else { 0.0 };
                        *w = v.clamp(-1.0, 1.0).to_bits();
                    }
                }
            }
            self.limit[s] = gr;
        }
    }
}

/// The audio-thread half of a tap (travels through the control queue).
pub(crate) struct TapRt {
    tx: Producer<u32>,
    channels: Vec<TapChannel>,
    record: Vec<u32>,
    overflow: Arc<AtomicU64>,
    /// Gain and limiter per strip; `None`: the plain copy.
    strips: Option<Strips>,
}

impl TapRt {
    /// Appends the batch starting at engine frame `frame`: host channels from `inputs`, source
    /// channels from the outputs the routing just wrote into `arena`, so call it after the
    /// batch has run. Never allocates or blocks.
    pub(crate) fn capture(&mut self, frame: u64, inputs: &[Batch], arena: &Arena) {
        if self.tx.slots() < self.record.len() {
            self.overflow.fetch_add(BATCH as u64, Ordering::Relaxed);
            return;
        }
        self.record[0] = frame as u32;
        self.record[1] = (frame >> 32) as u32;
        if let Some(st) = &mut self.strips {
            st.glide();
        }
        for (k, channel) in self.channels.iter().enumerate() {
            let dst = &mut self.record[2 + k * BATCH..2 + (k + 1) * BATCH];
            let src = match channel.source {
                Some((device, socket)) => arena.output(device, socket),
                None => inputs.get(channel.host),
            };
            match (src, &self.strips) {
                (Some(src), Some(st)) => {
                    let (from, to) = (st.gain[st.of[k]], st.next[st.of[k]]);
                    for (n, (d, s)) in dst.iter_mut().zip(src).enumerate() {
                        let g = from + (to - from) * ((n + 1) as f32 / BATCH as f32);
                        *d = (s * g).to_bits();
                    }
                }
                (Some(src), None) => {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = s.to_bits();
                    }
                }
                (None, _) => dst.fill(0.0f32.to_bits()),
            }
        }
        if let Some(st) = &mut self.strips {
            st.limit(&mut self.record);
            st.gain.clone_from_slice(&st.next);
        }
        if self.tx.push_entire_slice(&self.record).is_err() {
            self.overflow.fetch_add(BATCH as u64, Ordering::Relaxed);
        }
    }
}

/// The non-RT consumer of a record tap.
pub struct RecordTap {
    rx: Consumer<u32>,
    channels: usize,
    overflow: Arc<AtomicU64>,
    record: Vec<u32>,
}

impl std::fmt::Debug for RecordTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordTap")
            .field("channels", &self.channels)
            .field("overflow_frames", &self.overflow_frames())
            .finish_non_exhaustive()
    }
}

/// Creates both halves: the tap sized for `capacity_frames` (rounded up to whole batches, at
/// least one) of host `channels`.
pub(crate) fn new_tap(channels: &[usize], capacity_frames: usize) -> (Box<TapRt>, RecordTap) {
    let channels: Vec<TapChannel> = channels.iter().map(|&h| TapChannel::host(h, 0)).collect();
    new_tap_with(channels, None, capacity_frames)
}

/// [`new_tap`] with strips: channel `k` reads host `channels[k].host` (or its `source`) and goes
/// through strip `channels[k].strip` of `controls` (a strip past the last one uses the last).
pub(crate) fn new_gained_tap(
    channels: &[TapChannel],
    controls: Arc<TapControls>,
    capacity_frames: usize,
) -> (Box<TapRt>, RecordTap) {
    let n = controls.len();
    let of: Vec<usize> = channels
        .iter()
        .map(|c| c.strip.min(n.saturating_sub(1)))
        .collect();
    let strips = (n > 0).then(|| Strips {
        controls,
        of,
        gain: vec![1.0; n],
        next: vec![1.0; n],
        limit: vec![1.0; n],
        fresh: true,
    });
    new_tap_with(channels.to_vec(), strips, capacity_frames)
}

fn new_tap_with(
    channels: Vec<TapChannel>,
    strips: Option<Strips>,
    capacity_frames: usize,
) -> (Box<TapRt>, RecordTap) {
    let words = 2 + channels.len() * BATCH;
    let records = capacity_frames.div_ceil(BATCH).max(1);
    let (tx, rx) = RingBuffer::new(records * words);
    let overflow = Arc::new(AtomicU64::new(0));
    let count = channels.len();
    let rt = TapRt {
        tx,
        channels,
        record: vec![0; words],
        overflow: Arc::clone(&overflow),
        strips,
    };
    let tap = RecordTap {
        rx,
        channels: count,
        overflow,
        record: vec![0; words],
    };
    (Box::new(rt), tap)
}

impl RecordTap {
    /// Number of tapped channels (the length `out` gets in [`RecordTap::drain`]).
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames dropped so far because the ring was full (a multiple of 64).
    pub fn overflow_frames(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }

    /// The overflow counter itself, to read it while another thread drains the tap (for the
    /// status bar): the same number as [`RecordTap::overflow_frames`].
    pub fn overflow_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.overflow)
    }

    /// Appends the available input frames to `out` (one `Vec` per tapped channel, in the order
    /// given to `record_tap`; `out` is grown to that many vectors if shorter) and returns
    /// `(first engine frame, frames appended)`, or `None` if nothing was available.
    ///
    /// Only frames that are contiguous in engine time are appended in one call, so the first
    /// returned frame number applies to every appended frame; after an overflow the rest is
    /// returned by the next call, with its own start frame.
    pub fn drain(&mut self, out: &mut Vec<Vec<f32>>) -> Option<(u64, usize)> {
        let words = self.record.len();
        if out.len() < self.channels {
            out.resize_with(self.channels, Vec::new);
        }
        let mut run: Option<(u64, usize)> = None;
        while self.rx.slots() >= words {
            let Ok(chunk) = self.rx.read_chunk(words) else {
                break;
            };
            let (a, b) = chunk.as_slices();
            self.record[..a.len()].copy_from_slice(a);
            self.record[a.len()..].copy_from_slice(b);
            let frame = u64::from(self.record[0]) | u64::from(self.record[1]) << 32;
            if let Some((start, n)) = run
                && frame != start + n as u64
            {
                break; // a gap: leave this record for the next call
            }
            for (k, dst) in out.iter_mut().take(self.channels).enumerate() {
                let src = &self.record[2 + k * BATCH..2 + (k + 1) * BATCH];
                dst.extend(src.iter().map(|w| f32::from_bits(*w)));
            }
            chunk.commit_all();
            run = Some(match run {
                Some((start, n)) => (start, n + BATCH),
                None => (frame, BATCH),
            });
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(v: f32) -> Batch {
        [v; BATCH]
    }

    #[test]
    fn drain_returns_contiguous_runs_with_exact_tags_and_counts_overflow() {
        let arena = Arena::new(0);
        let (mut rt, mut tap) = new_tap(&[1, 0], 3 * BATCH);
        let inputs = [batch(0.25), batch(-0.5)];
        for k in 0..5u64 {
            rt.capture(k * BATCH as u64, &inputs, &arena);
        }
        // Room for three batches: two were dropped.
        assert_eq!(tap.overflow_frames(), 2 * BATCH as u64);
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), Some((0, 3 * BATCH)));
        assert_eq!(out.len(), 2);
        assert!(
            out[0].iter().all(|s| s.to_bits() == (-0.5f32).to_bits()),
            "channel 1 first"
        );
        assert!(out[1].iter().all(|s| s.to_bits() == 0.25f32.to_bits()));
        assert_eq!(tap.drain(&mut out), None);
        // After the overflow a gap: the run restarts at the new tag.
        rt.capture(7 * BATCH as u64, &inputs, &arena);
        rt.capture(8 * BATCH as u64, &inputs, &arena);
        rt.capture(10 * BATCH as u64, &inputs, &arena);
        let mut again = Vec::new();
        assert_eq!(tap.drain(&mut again), Some((7 * BATCH as u64, 2 * BATCH)));
        assert_eq!(tap.drain(&mut again), Some((10 * BATCH as u64, BATCH)));
        assert_eq!(again[0].len(), 3 * BATCH);
    }

    #[test]
    fn frame_tags_use_both_words_and_missing_channels_read_zero() {
        let (mut rt, mut tap) = new_tap(&[5], 2 * BATCH);
        let big = (1u64 << 40) + 192;
        rt.capture(big, &[batch(1.0)], &Arena::new(0));
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), Some((big, BATCH)));
        assert!(out[0].iter().all(|s| s.to_bits() == 0));
    }
}
