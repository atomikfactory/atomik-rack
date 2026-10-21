//! The preview voice: plays a file being auditioned in the browser, outside the graph.
//!
//! The decoder thread fills a [`PreviewFeed`]; the matching [`PreviewStream`] travels to the
//! audio thread inside a control message and is mixed into interface outputs 1/2 after the
//! metronome (outside the routing and the master meters), resampled from the file's rate with
//! cubic Hermite interpolation. The voice fades in over 5 ms; a new start or a stop fades the
//! old stream out over 5 ms. Finished or replaced streams are handed back through the retire
//! queue, so the audio thread never frees. Live only: offline renders never receive a preview.
//!
//! # Ring
//!
//! A lock-free `rtrb` ring of interleaved stereo `f32` ([`PREVIEW_RING_FRAMES`] frames). The
//! feed commits whole frames (left and right together), so the voice never sees half a frame.
//! A shared block of atomics carries the rest: `finished` (feed to voice), `abandoned`,
//! `done`, `played` and the underrun counter (voice to feed).
//!
//! # Voice
//!
//! Two slots: the current stream and one that is fading out. A start moves the current stream
//! to the fading slot; if that is occupied its stream is cut and retired at once (only when
//! starts arrive less than 5 ms apart). A stop moves the current stream to the fading slot the
//! same way. Streams that end (faded out, or played to the end after `finish`) wait in a small
//! "dead" list from which the engine hands them to the retire queue at the next batch boundary
//! (with the engine's usual parking if the queue is full). Nothing is ever dropped, allocated
//! or locked on the audio thread; at most two streams exist in the voice and its dead list at
//! any time, and the engine drains the dead list before it applies a control message.
//!
//! Fades are linear over `round(0.005 * sample_rate)` frames and advance only on frames that
//! produced signal (an underrun does not eat the fade). The fade-in starts at exactly 0 and the
//! fade-out ends at exactly 0. The crossfade of a switch sums to unity. The gain is linear and
//! applied after interpolation.
//!
//! # Resampling
//!
//! Cubic Hermite (Catmull-Rom, 4 points) at `ratio = source_rate / engine_rate`, computed at
//! start. The fractional position and the 4-frame history persist across ring reads and
//! batches. At `ratio == 1` the fractional position stays exactly 0 and the interpolator is
//! skipped (the history sample itself is used), so the passthrough is bit-exact once the
//! fade-in is over. At the start of a stream the history is filled with the first frame (the
//! missing "previous" frame repeats it); at the end of the stream the missing lookahead frames
//! repeat the last frame, and the voice ends when the position moves past the last source
//! frame (the signal just stops: the data ended).
//!
//! # Underrun
//!
//! If the ring is empty before `finish`, the voice outputs silence for that frame, keeps its
//! position and counts it ([`PreviewFeed::underruns`]); it never waits. After `finish` and an
//! empty ring the voice ends and the stream is marked done.
//!
//! # Progress
//!
//! [`PreviewFeed::frames_played`] is the voice position in source frames (independent of the
//! resampling ratio); it is published once per batch.

use atomik_core::{BATCH, Batch};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Capacity of a preview ring, in stereo frames.
pub const PREVIEW_RING_FRAMES: usize = 65_536;

/// Fade length in seconds (fade-in, fade-out and the crossfade of a switch).
const FADE_SECONDS: f64 = 0.005;

/// State shared by a feed and its stream.
#[derive(Default)]
struct Shared {
    /// Set by the feed after its last push.
    finished: AtomicBool,
    /// Set by the voice when it lets go of the stream.
    abandoned: AtomicBool,
    /// Set by the voice when it played everything after `finish`.
    done: AtomicBool,
    /// Source frames consumed.
    played: AtomicU64,
    /// Output frames played as silence for lack of data.
    underruns: AtomicU64,
    /// Source sample rate in Hz, set by the feed before its first push (0 = engine rate).
    rate: AtomicU32,
}

/// Creates a connected feed (decoder side) and stream (engine side). Allocates the ring;
/// call it off the audio thread.
pub fn preview_channel() -> (PreviewFeed, PreviewStream) {
    let (tx, rx) = RingBuffer::new(PREVIEW_RING_FRAMES * 2);
    let shared = Arc::new(Shared::default());
    (
        PreviewFeed {
            tx,
            shared: Arc::clone(&shared),
        },
        PreviewStream { rx, shared },
    )
}

/// The decoder side of a preview ring.
pub struct PreviewFeed {
    tx: Producer<f32>,
    shared: Arc<Shared>,
}

impl PreviewFeed {
    /// Pushes frames `from..` of `planar` (one vector per channel, equal lengths) as stereo:
    /// mono plays on both sides, more than two channels keep the first two. Returns how many
    /// frames were accepted (fewer when the ring is full). Never blocks.
    pub fn push(&mut self, planar: &[Vec<f32>], from: usize) -> usize {
        let Some(left) = planar.first() else {
            return 0;
        };
        let right = planar.get(1).unwrap_or(left);
        let available = left.len().min(right.len()).saturating_sub(from);
        let n = available.min(self.free_frames());
        if n == 0 {
            return 0;
        }
        let Ok(chunk) = self.tx.write_chunk_uninit(n * 2) else {
            return 0;
        };
        let l = &left[from..from + n];
        let r = &right[from..from + n];
        chunk.fill_from_iter(l.iter().zip(r).flat_map(|(&a, &b)| [a, b]));
        n
    }

    /// Sets the source sample rate in Hz. Call it before the first [`Self::push`]; the voice
    /// reads it once, when the first frame arrives. 0 or never set means the engine rate.
    pub fn set_source_rate(&mut self, hz: u32) {
        self.shared.rate.store(hz, Ordering::Release);
    }

    /// A read-only view of the progress flags that can be shared across threads.
    pub fn monitor(&self) -> PreviewMonitor {
        PreviewMonitor {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Free space in frames.
    pub fn free_frames(&self) -> usize {
        self.tx.slots() / 2
    }

    /// Marks the end of the stream: the voice plays out what is buffered, then ends.
    pub fn finish(&mut self) {
        self.shared.finished.store(true, Ordering::Release);
    }

    /// True once the engine has let go of the stream (stopped, replaced or finished and
    /// retired) or the stream was dropped: stop decoding.
    pub fn is_abandoned(&self) -> bool {
        self.shared.abandoned.load(Ordering::Acquire) || self.tx.is_abandoned()
    }

    /// Source frames the voice has consumed so far (for a progress display).
    pub fn frames_played(&self) -> u64 {
        self.shared.played.load(Ordering::Relaxed)
    }

    /// Output frames the voice had to play as silence because the ring was empty.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// True once the voice has played everything after [`Self::finish`].
    pub fn is_done(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }
}

/// The engine side of a preview ring. Opaque; pass it to `EngineHandle::preview_start`.
pub struct PreviewStream {
    rx: Consumer<f32>,
    shared: Arc<Shared>,
}

impl PreviewStream {
    /// A read-only view of the progress flags; take it before handing the stream to the engine.
    pub fn monitor(&self) -> PreviewMonitor {
        PreviewMonitor {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// A cloneable, thread-safe, read-only view of a preview's progress. Stays valid after the
/// feed and stream are gone (it keeps the shared flags alive).
#[derive(Clone)]
pub struct PreviewMonitor {
    shared: Arc<Shared>,
}

impl PreviewMonitor {
    /// Source frames the voice has consumed so far.
    pub fn frames_played(&self) -> u64 {
        self.shared.played.load(Ordering::Relaxed)
    }

    /// True once the voice has played everything after `finish`.
    pub fn is_done(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }

    /// True once the engine has let go of the stream.
    pub fn is_abandoned(&self) -> bool {
        self.shared.abandoned.load(Ordering::Acquire)
    }

    /// Output frames played as silence for lack of data.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Source rate in Hz as set by the feed; 0 until set.
    pub fn source_rate(&self) -> u32 {
        self.shared.rate.load(Ordering::Acquire)
    }
}

impl Drop for PreviewStream {
    fn drop(&mut self) {
        // Dropped on the handle side (never on the audio thread): the feed sees it at once.
        self.shared.abandoned.store(true, Ordering::Release);
    }
}

/// Fade state of a slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Ramping up; `pos` counts frames of signal produced since the start.
    In,
    /// Full level.
    Steady,
    /// Ramping down; `pos` counts the frames left.
    Out,
}

/// What one output frame of a slot is.
enum Step {
    Sample([f32; 2]),
    /// No data yet: silence, position unchanged (an underrun, counted).
    Silence,
    /// The first data has not arrived: nothing sounds, nothing is counted, the fade waits.
    Pending,
    /// The slot is over (faded out, or the data ended).
    End,
}

/// One playing stream with its resampler and fade.
struct Slot {
    stream: PreviewStream,
    /// Engine sample rate in Hz.
    engine_rate: f64,
    /// Source frames per output frame; valid once `started`.
    ratio: f64,
    /// The first data has been seen and the ratio computed.
    started: bool,
    gain: f32,
    /// Index of the source frame at `hist[1]`.
    base: u64,
    /// Frames shifted into the history so far, padding included.
    popped: u64,
    /// Real frames read from the ring.
    real: u64,
    /// Position between `hist[1]` and `hist[2]`, in `[0, 1)`.
    frac: f64,
    hist: [[f32; 2]; 4],
    mode: Mode,
    pos: u32,
    /// The slot ended because its data ran out after `finish`.
    exhausted: bool,
}

impl Slot {
    fn new(stream: PreviewStream, engine_rate: f64, gain: f32) -> Self {
        Self {
            stream,
            engine_rate,
            ratio: 1.0,
            started: false,
            gain,
            base: 0,
            popped: 0,
            real: 0,
            frac: 0.0,
            hist: [[0.0; 2]; 4],
            mode: Mode::In,
            pos: 0,
            exhausted: false,
        }
    }

    fn shift_in(&mut self, frame: [f32; 2]) {
        if self.popped == 0 {
            self.hist = [frame; 4];
        } else {
            self.hist.copy_within(1..4, 0);
            self.hist[3] = frame;
        }
        self.popped += 1;
    }

    fn pop_frame(&mut self) -> Option<[f32; 2]> {
        if self.stream.rx.slots() < 2 {
            return None;
        }
        let l = self.stream.rx.pop().ok()?;
        let r = self.stream.rx.pop().ok()?;
        Some([l, r])
    }

    /// Produces the next output frame.
    fn step(&mut self, fade: u32) -> Step {
        if self.mode == Mode::Out && self.pos == 0 {
            return Step::End;
        }
        if !self.started {
            // Read the flag before the ring: the feed pushes, then finishes.
            let finished = self.stream.shared.finished.load(Ordering::Acquire);
            if self.stream.rx.slots() >= 2 {
                // The feed stored the rate before its first push.
                let hz = self.stream.shared.rate.load(Ordering::Acquire);
                self.ratio = if hz == 0 {
                    1.0
                } else {
                    f64::from(hz) / self.engine_rate
                };
                self.started = true;
            } else if finished || self.stream.rx.is_abandoned() {
                self.exhausted = finished;
                return Step::End;
            } else {
                return Step::Pending;
            }
        }
        // Lookahead: output at `base + frac` needs history up to source frame `base + 2`.
        while self.popped < self.base + 3 {
            // Read the flag before the ring: the feed pushes, then finishes.
            let finished = self.stream.shared.finished.load(Ordering::Acquire);
            if let Some(f) = self.pop_frame() {
                self.real += 1;
                self.shift_in(f);
            } else if finished {
                if self.real == 0 {
                    self.exhausted = true;
                    return Step::End;
                }
                let last = self.hist[3];
                self.shift_in(last);
            } else {
                self.stream.shared.underruns.fetch_add(1, Ordering::Relaxed);
                return Step::Silence;
            }
        }
        if self.base >= self.real {
            self.exhausted = true;
            return Step::End;
        }
        let frame = if self.frac == 0.0 {
            self.hist[1]
        } else {
            let t = self.frac as f32;
            let h = &self.hist;
            [
                hermite(h[0][0], h[1][0], h[2][0], h[3][0], t),
                hermite(h[0][1], h[1][1], h[2][1], h[3][1], t),
            ]
        };
        let g = match self.mode {
            Mode::Steady => self.gain,
            Mode::In | Mode::Out => self.gain * (self.pos as f32 / fade as f32),
        };
        match self.mode {
            Mode::In => {
                self.pos += 1;
                if self.pos >= fade {
                    self.mode = Mode::Steady;
                }
            }
            Mode::Out => self.pos -= 1,
            Mode::Steady => {}
        }
        self.frac += self.ratio;
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.base += 1;
        }
        Step::Sample([frame[0] * g, frame[1] * g])
    }

    /// Starts the fade-out from the current level.
    fn fade_out(&mut self, fade: u32) {
        if self.mode == Mode::Steady {
            self.pos = fade;
        }
        self.mode = Mode::Out;
    }

    fn publish_position(&self) {
        let played = self.base.min(self.real);
        self.stream.shared.played.store(played, Ordering::Relaxed);
    }
}

/// Cubic Hermite (Catmull-Rom) between `y1` and `y2` at `t` in `[0, 1)`.
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Marks a stream the voice lets go of.
fn release(stream: PreviewStream) -> PreviewStream {
    stream.shared.abandoned.store(true, Ordering::Release);
    stream
}

/// The voice on the audio thread (owned by the engine).
pub(crate) struct PreviewVoice {
    fade: u32,
    engine_rate: f64,
    current: Option<Slot>,
    fading: Option<Slot>,
    /// Ended streams waiting to be retired. At most two (the two slots).
    dead: [Option<PreviewStream>; 2],
}

impl PreviewVoice {
    pub(crate) fn new(sample_rate: f64) -> Self {
        let fade = (FADE_SECONDS * sample_rate).round().max(1.0) as u32;
        Self {
            fade,
            engine_rate: sample_rate,
            current: None,
            fading: None,
            dead: [None, None],
        }
    }

    /// True while a slot is sounding (including a fade-out).
    pub(crate) fn is_active(&self) -> bool {
        self.current.is_some() || self.fading.is_some()
    }

    /// Starts `stream`; the current stream fades out. Returns the stream that had to be cut
    /// (the previous fading one), to be retired by the caller.
    pub(crate) fn start(&mut self, stream: PreviewStream, gain: f32) -> Option<PreviewStream> {
        let mut evicted = None;
        if let Some(mut old) = self.current.take() {
            old.fade_out(self.fade);
            evicted = self.fading.replace(old).map(|cut| release(cut.stream));
        }
        self.current = Some(Slot::new(stream, self.engine_rate, gain));
        evicted
    }

    /// Fades the current stream out. Returns a stream that had to be cut, as for `start`.
    pub(crate) fn stop(&mut self) -> Option<PreviewStream> {
        let mut old = self.current.take()?;
        old.fade_out(self.fade);
        self.fading.replace(old).map(|cut| release(cut.stream))
    }

    /// Hands out one ended stream, if any.
    pub(crate) fn take_dead(&mut self) -> Option<PreviewStream> {
        self.dead.iter_mut().find_map(Option::take)
    }

    /// Adds the voice to interface outputs 1 and 2 for one batch.
    pub(crate) fn mix(&mut self, out: &mut [Batch]) {
        if !self.is_active() {
            return;
        }
        let fade = self.fade;
        for which in 0..2 {
            let slot = if which == 0 {
                &mut self.fading
            } else {
                &mut self.current
            };
            let Some(s) = slot.as_mut() else { continue };
            let mut ended = false;
            for i in 0..BATCH {
                match s.step(fade) {
                    Step::Sample(frame) => {
                        if let Some(ch) = out.get_mut(0) {
                            ch[i] += frame[0];
                        }
                        if let Some(ch) = out.get_mut(1) {
                            ch[i] += frame[1];
                        }
                    }
                    Step::Silence | Step::Pending => {}
                    Step::End => {
                        ended = true;
                        break;
                    }
                }
            }
            s.publish_position();
            if ended && let Some(gone) = slot.take() {
                if gone.exhausted {
                    gone.stream.shared.done.store(true, Ordering::Release);
                }
                let stream = release(gone.stream);
                // Never full: at most two streams live in the voice and its dead list.
                if let Some(free) = self.dead.iter_mut().find(|d| d.is_none()) {
                    *free = Some(stream);
                }
            }
        }
    }
}
