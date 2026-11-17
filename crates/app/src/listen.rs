//! Listen and grade: the pure part. No engine, no document, no UI: it turns what the input tap
//! delivered into played notes on the song's tick axis, sorts them into the passes of the loop
//! and hands finished passes back for grading ([`atomik_tab::grade`]). The glue that reads the
//! tap and the document is [`crate::AppCore`]'s `app_core_listen.rs`.
//!
//! # Time
//!
//! The tracker ([`atomik_dsp::listen::Listener`]) numbers notes by *input frame*, the engine's
//! host frame the tap tagged them with. Once a UI frame the transport state gives an
//! [`Anchor`]: the engine frame and the song tick at the start of the last batch. The [`Clock`]
//! keeps them and converts a frame to a tick by interpolating between two anchors of the same
//! pass (extrapolating with the measured rate past the newest, or across a loop wrap into the
//! next lap), so it holds at any practice speed and over tempo changes: the engine's tick is
//! what plays the notes, so the grade is against what was heard.
//!
//! What the player did at input frame `h` was played against what the engine rendered
//! `compensation` frames earlier (the input and output latency and the engine's FIFO, the sink's
//! [`crate::sink::EngineSink::record_offset_frames`]), plus the manual offset the player calibrated
//! ([`Listening::set_compensation`]; a positive offset places notes earlier).
//!
//! # Passes
//!
//! A pass is one lap of the loop within one run of the transport (a run begins at every start).
//! Notes are put in the pass whose time they fall in. A pass is *closed* when the transport has
//! gone on to the next lap or stopped and the tracker has delivered every note up to its end
//! (the tracker is a few hundred milliseconds behind the input). The loop is a circle: a note
//! played up to 40 ms before the loop end counts for the first notes of the next lap.
//!
//! # The clean-pass judge
//!
//! [`AccuracyJudge`] is the [`PassJudge`] the speed trainer asks while listening is on: a pass is
//! clean when its accuracy is at least the threshold (default 90 %).

use crate::practice::{PassJudge, PassReport};
use atomik_dsp::listen::{Listener, PlayedNote};
use atomik_tab::grade::{self, Expected, PassGrade, Played};
use std::collections::BTreeMap;

/// The most a note may lie before the first anchor of a run and still be placed, seconds.
const BACK_SECONDS: f64 = 0.15;
/// The most a note may lie after the newest anchor and still be placed, seconds.
const FORWARD_SECONDS: f64 = 1.0;
/// Frames of the engine batch an anchor stands for.
const BATCH_FRAMES: u64 = 64;
/// A note this many milliseconds before the loop end counts for the next lap's start (the on-time
/// zone of the default window).
const HIT_MS: f64 = 40.0;
/// Anchors kept, newest.
const MAX_ANCHORS: usize = 4096;

/// The default accuracy a pass needs to be clean, percent.
pub const DEFAULT_CLEAN_PERCENT: u8 = 90;
/// The lowest and highest accuracy the trainer can ask for, percent.
pub const CLEAN_PERCENT_RANGE: (u8, u8) = (50, 100);
/// Step of the threshold, percent.
pub const CLEAN_PERCENT_STEP: u8 = 5;
/// The manual offset's limit, milliseconds either way.
pub const MAX_OFFSET_MS: i32 = 500;
/// Step of the manual offset, milliseconds.
pub const OFFSET_STEP_MS: i32 = 5;

/// Which pass: the run of the transport and the lap of the loop within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PassKey {
    /// Counts every start of the transport.
    pub run: u32,
    /// The engine's loop count.
    pub lap: u64,
}

/// What the transport said at the start of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// The engine frame.
    pub frame: u64,
    /// The song tick.
    pub tick: i64,
    /// The pass it belongs to.
    pub key: PassKey,
    /// The loop, when it is on.
    pub loop_range: Option<(i64, i64)>,
}

/// The transport state as the listener needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportView {
    /// Playing (not counting in).
    pub playing: bool,
    /// Engine frame at the start of the last batch.
    pub engine_frame: u64,
    /// Song tick at that frame.
    pub tick: i64,
    /// Loop wraps so far.
    pub loop_count: u64,
    /// The loop range, when it is on.
    pub loop_range: Option<(i64, i64)>,
}

/// Frame to tick conversion from anchors (module docs).
#[derive(Debug, Default)]
pub struct Clock {
    anchors: Vec<Anchor>,
    back_frames: f64,
    forward_frames: f64,
}

impl Clock {
    /// A clock for an engine at `rate` Hz.
    pub fn new(rate: u32) -> Self {
        Self {
            anchors: Vec::new(),
            back_frames: BACK_SECONDS * f64::from(rate),
            forward_frames: FORWARD_SECONDS * f64::from(rate),
        }
    }

    /// Adds an anchor; one at or before the newest frame is ignored.
    pub fn push(&mut self, a: Anchor) {
        if self.anchors.last().is_some_and(|l| a.frame <= l.frame) {
            return;
        }
        self.anchors.push(a);
        if self.anchors.len() > MAX_ANCHORS {
            self.anchors.drain(..MAX_ANCHORS / 4);
        }
    }

    /// Forgets every anchor.
    pub fn clear(&mut self) {
        self.anchors.clear();
    }

    /// The newest anchor.
    pub fn last(&self) -> Option<&Anchor> {
        self.anchors.last()
    }

    /// Ticks per frame around anchor `i` (from its pair with a neighbour of the same pass).
    fn rate_at(&self, i: usize) -> Option<f64> {
        let pair = |a: &Anchor, b: &Anchor| {
            (a.key == b.key && b.frame > a.frame)
                .then(|| (b.tick - a.tick) as f64 / (b.frame - a.frame) as f64)
        };
        let before = i
            .checked_sub(1)
            .and_then(|j| pair(&self.anchors[j], &self.anchors[i]));
        before.or_else(|| {
            self.anchors
                .get(i + 1)
                .and_then(|n| pair(&self.anchors[i], n))
        })
    }

    /// The pass and tick (fractional) at engine `frame`; `None` before the first anchor by more
    /// than 150 ms, past the newest by more than a second, or with no anchors.
    pub fn locate(&self, frame: f64) -> Option<(PassKey, f64, f64)> {
        let first = self.anchors.first()?;
        let idx = self.anchors.partition_point(|a| a.frame as f64 <= frame);
        if idx == 0 {
            if first.frame as f64 - frame > self.back_frames {
                return None;
            }
            let rate = self.rate_at(0)?;
            let tick = first.tick as f64 + (frame - first.frame as f64) * rate;
            return Some((first.key, tick, rate));
        }
        let prev = &self.anchors[idx - 1];
        let next = self.anchors.get(idx);
        if let Some(n) = next
            && n.key == prev.key
        {
            let t = (frame - prev.frame as f64) / (n.frame - prev.frame) as f64;
            let rate = (n.tick - prev.tick) as f64 / (n.frame - prev.frame) as f64;
            return Some((
                prev.key,
                prev.tick as f64 + t * (n.tick - prev.tick) as f64,
                rate,
            ));
        }
        if next.is_none() && frame - prev.frame as f64 > self.forward_frames {
            return None;
        }
        let rate = self.rate_at(idx - 1).unwrap_or(0.0);
        let tick = prev.tick as f64 + (frame - prev.frame as f64) * rate;
        // Past the loop end, before the next lap's first anchor: the next lap's start.
        if let (Some((start, end)), Some(n)) = (prev.loop_range, next)
            && tick >= end as f64
        {
            return Some((n.key, start as f64 + (tick - end as f64), rate));
        }
        Some((prev.key, tick, rate))
    }
}

/// One pass being collected.
#[derive(Debug, Clone)]
struct PassBuf {
    first_tick: i64,
    first_frame: u64,
    last_tick: i64,
    last_frame: u64,
    loop_range: Option<(i64, i64)>,
    speed: u16,
    events: Vec<Played>,
}

/// A pass that is finished and ready to grade.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedPass {
    /// Which pass.
    pub key: PassKey,
    /// The first tick the pass was seen at.
    pub first_tick: i64,
    /// The last tick the pass was seen at.
    pub last_tick: i64,
    /// The loop it ran in, when it was on.
    pub loop_range: Option<(i64, i64)>,
    /// The practice speed, percent.
    pub speed: u16,
    /// The pass ran to the end of the loop and the next lap followed (else the transport stopped).
    pub wrapped: bool,
    /// Song ticks per millisecond of wall-clock time over the pass.
    pub ticks_per_ms: f64,
    /// The notes heard, on the song's tick axis.
    pub events: Vec<Played>,
}

/// A note the tracker finished, placed on the song's axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// The pass it fell in.
    pub key: PassKey,
    /// What was heard.
    pub played: Played,
    /// Milliseconds of wall-clock time per song tick there (0 when unknown).
    pub ms_per_tick: f64,
}

/// The listener: tracker, clock and the passes being collected (module docs).
#[derive(Debug)]
pub struct Listening {
    rate: u32,
    listener: Listener,
    clock: Clock,
    run: u32,
    playing: bool,
    speed: u16,
    compensation: i64,
    offset_ms: i32,
    passes: BTreeMap<PassKey, PassBuf>,
    /// The notes played just before the end of the lap that closed last, as ticks before the next
    /// lap's start (module docs: the loop is a circle).
    carry: Option<(PassKey, Vec<Played>)>,
}

impl Listening {
    /// A listener for an engine at `rate` Hz.
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            listener: Listener::new(rate),
            clock: Clock::new(rate),
            run: 0,
            playing: false,
            speed: 100,
            compensation: 0,
            offset_ms: 0,
            passes: BTreeMap::new(),
            carry: None,
        }
    }

    /// The sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Sets what lies between a note played and its frame: the frames of latency (the sink's
    /// record compensation) and the manual offset in milliseconds.
    pub fn set_compensation(&mut self, frames: i64, offset_ms: i32) {
        self.compensation = frames;
        self.offset_ms = offset_ms;
    }

    fn offset_frames(&self) -> f64 {
        self.compensation as f64 + f64::from(self.offset_ms) * f64::from(self.rate) / 1000.0
    }

    /// Forgets everything heard and every pass (the engine restarted, or listening starts).
    pub fn reset(&mut self) {
        self.listener.reset();
        self.clock.clear();
        self.passes.clear();
        self.carry = None;
        self.playing = false;
    }

    /// Notes the transport state (once a UI frame). A state older than the last one means the
    /// engine restarted.
    pub fn anchor(&mut self, view: &TransportView, speed: u16) {
        if self
            .clock
            .last()
            .is_some_and(|a| view.engine_frame < a.frame)
        {
            self.reset();
        }
        if view.playing && !self.playing {
            self.run += 1;
        }
        self.playing = view.playing;
        self.speed = speed;
        if !view.playing {
            return;
        }
        let key = PassKey {
            run: self.run,
            lap: view.loop_count,
        };
        self.clock.push(Anchor {
            frame: view.engine_frame,
            tick: view.tick,
            key,
            loop_range: view.loop_range,
        });
        let buf = self.passes.entry(key).or_insert_with(|| PassBuf {
            first_tick: view.tick,
            first_frame: view.engine_frame,
            last_tick: view.tick,
            last_frame: view.engine_frame,
            loop_range: view.loop_range,
            speed,
            events: Vec::new(),
        });
        if view.engine_frame >= buf.last_frame {
            buf.last_tick = view.tick;
            buf.last_frame = view.engine_frame + BATCH_FRAMES;
        }
        buf.loop_range = view.loop_range;
    }

    /// Feeds mono input frames from `first_frame` on; returns the notes that became final, placed
    /// in their passes (module docs). Notes that fall outside every pass are dropped.
    pub fn samples(&mut self, first_frame: u64, mono: &[f32]) -> Vec<Located> {
        let shift = self.offset_frames();
        let mut out = Vec::new();
        for note in self.listener.feed(first_frame, mono) {
            let PlayedNote {
                frame, key, chroma, ..
            } = note;
            let Some((pass, tick, rate)) = self.clock.locate(frame as f64 - shift) else {
                continue;
            };
            let Some(buf) = self.passes.get_mut(&pass) else {
                continue;
            };
            let played = Played { tick, key, chroma };
            buf.events.push(played);
            out.push(Located {
                key: pass,
                played,
                ms_per_tick: if rate > 0.0 {
                    1000.0 / (rate * f64::from(self.rate))
                } else {
                    0.0
                },
            });
        }
        out
    }

    /// The frame up to which the tracker has delivered every note.
    pub fn known_until(&self) -> u64 {
        self.listener
            .next_frame()
            .saturating_sub(self.listener.latency_frames() as u64)
    }

    /// Takes the passes that are over and fully heard (module docs).
    pub fn take_closed(&mut self) -> Vec<ClosedPass> {
        let newest = self.clock.last().map(|a| a.key);
        let shift = self.offset_frames().max(0.0) as u64;
        let known = self.known_until();
        let ready: Vec<PassKey> = self
            .passes
            .iter()
            .filter(|(k, b)| {
                let over = !self.playing || Some(**k) != newest;
                over && known >= b.last_frame + shift
            })
            .map(|(k, _)| *k)
            .collect();
        let mut out = Vec::new();
        for key in ready {
            let Some(b) = self.passes.remove(&key) else {
                continue;
            };
            let wrapped = self
                .passes
                .keys()
                .chain(newest.iter())
                .any(|k| k.run == key.run && k.lap > key.lap);
            let span = b.last_frame.saturating_sub(b.first_frame);
            let ticks_per_frame = if span > 0 {
                (b.last_tick - b.first_tick) as f64 / span as f64
            } else {
                0.0
            };
            let ticks_per_ms = ticks_per_frame * f64::from(self.rate) / 1000.0;
            let mut events = b.events;
            // The loop is a circle: a note played a little before the loop's end belongs to the
            // start of the next lap. The tail of a lap that ran to the end is carried over, as
            // ticks before the next lap's start.
            if let Some((carried, tail)) = self.carry.take()
                && carried.run == key.run
                && carried.lap + 1 == key.lap
            {
                events.splice(0..0, tail);
            }
            if wrapped && let Some((start, end)) = b.loop_range {
                let from = (end as f64) - (HIT_MS * ticks_per_ms).max(1.0);
                let (own, tail): (Vec<Played>, Vec<Played>) =
                    events.into_iter().partition(|p| p.tick < from);
                events = own;
                let len = (end - start) as f64;
                let tail: Vec<Played> = tail
                    .into_iter()
                    .map(|p| Played {
                        tick: p.tick - len,
                        ..p
                    })
                    .collect();
                self.carry = Some((key, tail));
            }
            out.push(ClosedPass {
                key,
                first_tick: b.first_tick,
                last_tick: b.last_tick,
                loop_range: b.loop_range,
                speed: b.speed,
                wrapped,
                ticks_per_ms,
                events,
            });
        }
        out
    }

    /// Drops the passes being collected (results were cleared).
    pub fn forget_passes(&mut self) {
        self.passes.clear();
        self.carry = None;
    }
}

/// The tick range a closed pass is graded over: `(from, to)`, to exclusive. A lap after a wrap
/// starts at the loop start; the first lap of a run starts where it was first seen (less half the
/// on-time zone); a pass that ran to the loop end goes to the loop end, else to where it was
/// last seen.
pub fn graded_range(p: &ClosedPass, params: &grade::Params) -> (f64, f64) {
    let from = match p.loop_range {
        Some((start, _)) if p.key.lap > 0 || p.first_tick <= start + 1 => start as f64,
        _ => p.first_tick as f64 - params.hit_ticks,
    };
    let to = match p.loop_range {
        Some((_, end)) if p.wrapped => end as f64,
        _ => p.last_tick as f64,
    };
    (from, to)
}

/// Grades a closed pass against the notes `(song tick, key)` of the clip: the groups inside the
/// graded range, with the notes heard. `None` when the range holds no note.
pub fn grade_closed(
    pass: &ClosedPass,
    notes: &[(i64, u8)],
    window_ms: f64,
) -> Option<(PassGrade, Vec<Expected>)> {
    let params = grade::Params::from_ms(window_ms, pass.ticks_per_ms.max(0.0));
    let (from, to) = graded_range(pass, &params);
    let expected = grade::group_notes(
        notes
            .iter()
            .copied()
            .filter(|(t, _)| (*t as f64) >= from && (*t as f64) < to),
    );
    if expected.is_empty() {
        return None;
    }
    let heard: Vec<Played> = pass
        .events
        .iter()
        .copied()
        .filter(|p| p.tick >= from - params.window_ticks && p.tick < to + params.window_ticks)
        .collect();
    let g = grade::grade_pass(&expected, &heard, &params);
    Some((g, expected))
}

/// The [`PassJudge`] of a listening trainer: a pass is clean when its accuracy reaches the
/// threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccuracyJudge {
    /// The least accuracy, as a fraction.
    pub threshold: f32,
}

impl AccuracyJudge {
    /// A judge asking for `percent` (clamped to the settable range).
    pub fn new(percent: u8) -> Self {
        Self {
            threshold: f32::from(percent.clamp(CLEAN_PERCENT_RANGE.0, CLEAN_PERCENT_RANGE.1))
                / 100.0,
        }
    }
}

impl PassJudge for AccuracyJudge {
    fn is_clean(&mut self, pass: &PassReport) -> bool {
        // A pass that was not graded (nothing was heard to grade it with, or its range held no
        // note) counts as it did before listening: it is clean.
        let Some(a) = pass.accuracy else {
            return true;
        };
        // Rounded the way the bar shows it, so 90 % on the screen is clean at 90 %.
        (a * 100.0).round() / 100.0 >= self.threshold - 1e-6
    }
}
#[cfg(test)]
pub(crate) mod tests;
