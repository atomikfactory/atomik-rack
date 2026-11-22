//! How much captured input the render callback lets wait in the input ring before it discards the
//! oldest frames.
//!
//! The allowed backlog follows the buffer the stream was opened with (and the largest callback a
//! driver delivered lately, when it delivers more than it was asked for), never the size of the
//! callback that happens to be running: shared-mode drivers deliver callbacks of uneven size, and
//! a short one must not turn input that merely arrived in a burst into a backlog to be thrown
//! away.
//!
//! On top of that base the policy keeps an *extra* allowance for a driver that delivers its
//! input in packets larger than a buffer:
//!
//! - It grows by one step when a counted trim follows a **lump**: input that arrived in one
//!   callback exceeding a buffer by at least half a buffer (and at least [`MIN_LUMP_FRAMES`]).
//!   A trim after a steady arrival is clock drift (a frame or a few per callback); it is counted
//!   as lost but never moves the allowance, so two devices whose clocks drift do not get more
//!   latency for nothing.
//! - Time is cut into windows of [`WINDOW_CALLBACKS`] callbacks. At the end of a window the
//!   allowance shrinks by one step only if no lump was trimmed in it and the largest backlog it
//!   saw would still have fitted under the smaller threshold, so room that is being used is kept.
//! - The largest callback is the largest of the last two windows, so a single freak callback is
//!   forgotten, while a host that really delivers bigger buffers than asked is followed.
//!
//! Plain integers owned by the render callback: no allocation, lock or log. Counted in
//! callbacks, never in wall time.

/// The base allowance: the current read plus this many buffers waiting.
pub(crate) const BASE_SLACK_BUFFERS: u32 = 2;

/// One step of the extra allowance is this fraction of a buffer (`1 / STEP_DIVISOR`).
const STEP_DIVISOR: u32 = 2;

/// The smallest step, in frames.
const MIN_STEP_FRAMES: u32 = 16;

/// An arrival is a lump when it exceeds one buffer by this fraction of a buffer
/// (`1 / LUMP_DIVISOR`) ...
const LUMP_DIVISOR: u32 = 2;

/// ... and by at least this many frames (a small buffer's half is a handful of frames).
pub(crate) const MIN_LUMP_FRAMES: u32 = 32;

/// The extra allowance never exceeds this many buffers ...
const MAX_EXTRA_BUFFERS: u32 = 2;

/// ... but is never below this many milliseconds of audio (so a very small buffer still has room
/// for a driver that delivers several buffers at once) ...
const MIN_EXTRA_MS: u32 = 10;

/// ... and never above this many (so a large buffer does not add a large delay on top).
const MAX_EXTRA_MS: u32 = 30;

/// Callbacks in one window.
pub(crate) const WINDOW_CALLBACKS: u32 = 2000;

/// The adaptive trim threshold.
#[derive(Debug, Clone)]
pub(crate) struct InputSlack {
    /// The buffer the stream was opened with, in frames.
    configured: u32,
    /// The largest callback of the previous window, in frames.
    largest: u32,
    /// The largest callback of the current window.
    window_largest: u32,
    sample_rate: u32,
    /// The extra allowance now, in frames.
    extra: u32,
    /// Callbacks in the current window so far.
    window_callbacks: u32,
    /// The most frames the ring held before a trim in the current window.
    window_peak: u32,
    /// A lump was trimmed in the current window.
    lump_in_window: bool,
    /// Frames the ring held after the previous callback's read.
    held_after_read: u32,
}

impl InputSlack {
    /// A policy for a stream opened with `configured` frames per buffer at `sample_rate`.
    pub(crate) fn new(configured: u32, sample_rate: u32) -> Self {
        Self {
            configured,
            largest: 0,
            window_largest: 0,
            sample_rate,
            extra: 0,
            window_callbacks: 0,
            window_peak: 0,
            lump_in_window: false,
            held_after_read: 0,
        }
    }

    /// The buffer the thresholds are based on: the configured one or the largest recent callback.
    pub(crate) fn buffer(&self) -> u32 {
        self.configured
            .max(self.largest)
            .max(self.window_largest)
            .max(1)
    }

    /// Notes a callback of `frames` frames (before its trim).
    pub(crate) fn on_callback(&mut self, frames: u32) {
        self.window_largest = self.window_largest.max(frames);
    }

    fn base_keep(&self) -> u32 {
        self.buffer().saturating_mul(1 + BASE_SLACK_BUFFERS)
    }

    /// The most frames the ring may hold once the callback has trimmed it, including the
    /// callback's own read.
    pub(crate) fn keep_frames(&self) -> u32 {
        self.base_keep().saturating_add(self.extra_frames())
    }

    /// The most frames that may wait beyond one callback's read: the bound on the latency this
    /// adds to the input.
    pub(crate) fn allowed_backlog_frames(&self) -> u32 {
        self.keep_frames().saturating_sub(self.buffer())
    }

    /// The largest the extra allowance can get at the current buffer, in frames.
    pub(crate) fn max_extra_frames(&self) -> u32 {
        let ms = |ms: u32| {
            u32::try_from(u64::from(self.sample_rate) * u64::from(ms) / 1000).unwrap_or(u32::MAX)
        };
        let low = ms(MIN_EXTRA_MS);
        self.buffer()
            .saturating_mul(MAX_EXTRA_BUFFERS)
            .clamp(low, ms(MAX_EXTRA_MS).max(low))
    }

    /// The extra allowance now, in frames (zero: the base).
    pub(crate) fn extra_frames(&self) -> u32 {
        self.extra.min(self.max_extra_frames())
    }

    fn step(&self) -> u32 {
        (self.buffer() / STEP_DIVISOR).max(MIN_STEP_FRAMES)
    }

    /// How far above one buffer an arrival has to be to count as a lump.
    fn lump_frames(&self) -> u32 {
        (self.buffer() / LUMP_DIVISOR).max(MIN_LUMP_FRAMES)
    }

    /// The callback's trim is done. `backlog_frames` is what the ring held before the trim and
    /// `counted` says the trim discarded frames that count as lost.
    pub(crate) fn after_trim(&mut self, backlog_frames: u32, counted: bool) {
        self.window_peak = self.window_peak.max(backlog_frames);
        let arrived = backlog_frames.saturating_sub(self.held_after_read);
        let lump = arrived >= self.buffer().saturating_add(self.lump_frames());
        if counted && lump {
            self.lump_in_window = true;
            self.extra = self
                .extra_frames()
                .saturating_add(self.step())
                .min(self.max_extra_frames());
        }
        self.window_callbacks += 1;
        if self.window_callbacks >= WINDOW_CALLBACKS {
            self.end_window();
        }
    }

    /// The callback's read is done: the ring holds `frames` frames.
    pub(crate) fn after_read(&mut self, frames: u32) {
        self.held_after_read = frames;
    }

    fn end_window(&mut self) {
        self.largest = self.window_largest;
        self.window_largest = 0;
        self.extra = self.extra_frames();
        if self.extra > 0 && !self.lump_in_window {
            let smaller = self.extra.saturating_sub(self.step());
            if self.window_peak <= self.base_keep().saturating_add(smaller) {
                self.extra = smaller;
            }
        }
        self.window_callbacks = 0;
        self.window_peak = 0;
        self.lump_in_window = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{RingReader, RingWriter, sample_ring};

    /// A mono run of callbacks against a real ring. `old_rule` applies the threshold this policy
    /// replaced (three buffers of the running callback).
    struct Sim {
        slack: InputSlack,
        writer: RingWriter,
        reader: RingReader,
        scratch: Vec<f32>,
        old_rule: bool,
        first: bool,
        lost: u64,
        trims: u64,
        /// The most frames the ring held after a trim.
        held_max: usize,
    }

    impl Sim {
        fn new(buffer: u32, old_rule: bool) -> Self {
            let (writer, reader) = sample_ring(1 << 17);
            Self {
                slack: InputSlack::new(buffer, 48_000),
                writer,
                reader,
                scratch: vec![0.0; 16_384],
                old_rule,
                first: true,
                lost: 0,
                trims: 0,
                held_max: 0,
            }
        }

        /// One callback of `frames` frames after `capture` frames arrived.
        fn step(&mut self, frames: usize, capture: usize) {
            self.writer.write(&self.scratch[..capture]);
            let backlog = self.reader.available();
            self.slack.on_callback(frames as u32);
            let keep = if self.old_rule {
                frames * (1 + BASE_SLACK_BUFFERS as usize)
            } else {
                self.slack.keep_frames() as usize
            };
            let discarded = self.reader.trim(keep);
            let counted = !self.first && discarded > 0;
            self.first = false;
            if counted {
                self.lost += discarded as u64;
                self.trims += 1;
            }
            self.held_max = self.held_max.max(self.reader.available());
            self.slack.after_trim(backlog as u32, counted);
            self.reader.read(&mut self.scratch[..frames]);
            self.slack.after_read(self.reader.available() as u32);
        }

        /// `callbacks` callbacks of `frames` frames with input in packets of `packet` frames
        /// (as many frames arrive as are read, over time).
        fn packets(&mut self, frames: usize, packet: usize, callbacks: usize, owed: &mut usize) {
            for _ in 0..callbacks {
                *owed += frames;
                let capture = if *owed >= packet {
                    *owed -= packet;
                    packet
                } else {
                    0
                };
                self.step(frames, capture);
            }
        }

        /// `callbacks` callbacks with input `ppm` parts per million above the output rate.
        fn drifting(&mut self, frames: usize, ppm: usize, callbacks: usize) {
            let mut acc = 0usize;
            for _ in 0..callbacks {
                acc += frames * ppm;
                let extra = acc / 1_000_000;
                acc %= 1_000_000;
                self.step(frames, frames + extra);
            }
        }
    }

    #[test]
    fn bursts_against_callbacks_of_uneven_size_lose_nothing() {
        // Callbacks of 96, 480 and 512 frames in irregular order; the input arrives in lumps of
        // up to two buffers, and as many frames arrive as are read over a cycle.
        let sizes = [96, 480, 512, 480, 96, 512];
        let lumps = [1024, 0, 96, 544, 512, 0];
        assert_eq!(lumps.iter().sum::<usize>(), sizes.iter().sum::<usize>());
        let mut now = Sim::new(512, false);
        let mut old = Sim::new(512, true);
        for n in 0..600 {
            now.step(sizes[n % 6], lumps[n % 6]);
            old.step(sizes[n % 6], lumps[n % 6]);
        }
        assert_eq!((now.lost, now.trims), (0, 0), "no frame is discarded");
        assert!(old.lost > 0, "the rule it replaced dropped {}", old.lost);
    }

    #[test]
    fn the_threshold_ignores_a_short_callback_and_follows_a_larger_one() {
        let mut s = InputSlack::new(512, 48_000);
        s.on_callback(96);
        assert_eq!(s.keep_frames(), 512 * 3);
        s.on_callback(1024);
        assert_eq!(s.buffer(), 1024, "a driver delivering more than asked");
        assert_eq!(s.keep_frames(), 1024 * 3);
        s.on_callback(96);
        assert_eq!(s.keep_frames(), 1024 * 3);
        assert_eq!(s.allowed_backlog_frames(), 1024 * 2);
    }

    #[test]
    fn drift_is_counted_but_never_moves_the_allowance() {
        for buffer in [64usize, 480, 4096] {
            for ppm in [1000usize, 100] {
                let mut sim = Sim::new(buffer as u32, false);
                sim.drifting(buffer, ppm, 60_000);
                let base = buffer * 3;
                assert!(sim.trims > 5, "{buffer} at {ppm} ppm trimmed {}", sim.trims);
                assert!(sim.lost >= sim.trims, "every trim is counted as lost");
                assert_eq!(sim.slack.extra_frames(), 0, "{buffer} at {ppm} ppm");
                assert!(sim.held_max <= base, "{buffer}: held {}", sim.held_max);
            }
        }
    }

    #[test]
    fn packets_of_ten_milliseconds_grow_the_allowance_until_nothing_is_lost() {
        for buffer in [64usize, 128] {
            let mut sim = Sim::new(buffer as u32, false);
            let mut owed = 0;
            // Ten milliseconds at 48 kHz.
            sim.packets(buffer, 480, 3 * WINDOW_CALLBACKS as usize, &mut owed);
            assert!(sim.slack.extra_frames() > 0, "{buffer}: it made room");
            assert!(sim.lost > 0, "the first packets were trimmed");
            let lost = sim.lost;
            let extra = sim.slack.extra_frames();
            sim.packets(buffer, 480, 10 * WINDOW_CALLBACKS as usize, &mut owed);
            assert_eq!(sim.lost, lost, "{buffer}: no loss in the next ten windows");
            assert_eq!(sim.slack.extra_frames(), extra, "{buffer}: and it stays");
            assert!(sim.slack.extra_frames() <= sim.slack.max_extra_frames());
        }
    }

    #[test]
    fn the_allowance_comes_down_window_by_window_when_the_bursts_stop() {
        let mut sim = Sim::new(64, false);
        let mut owed = 0;
        sim.packets(64, 480, 3 * WINDOW_CALLBACKS as usize, &mut owed);
        let mut extra = sim.slack.extra_frames();
        assert!(extra > 0);
        let lost = sim.lost;
        // The input stalls for a moment (the ring drains), then runs steadily.
        for _ in 0..16 {
            sim.step(64, 0);
        }
        let mut windows = 0;
        while extra > 0 {
            for _ in 0..WINDOW_CALLBACKS {
                sim.step(64, 64);
            }
            let now = sim.slack.extra_frames();
            assert!(now < extra, "one step down per window");
            extra = now;
            windows += 1;
            assert!(windows < 50, "it comes all the way down");
        }
        assert_eq!(sim.lost, lost, "steady input loses nothing on the way");
    }

    #[test]
    fn one_freak_callback_is_forgotten_after_a_window() {
        let mut sim = Sim::new(64, false);
        for _ in 0..10 {
            sim.step(64, 64);
        }
        assert_eq!(sim.slack.keep_frames(), 64 * 3);
        sim.step(4096, 4096);
        assert_eq!(sim.slack.keep_frames(), 4096 * 3, "followed at once");
        // The rest of that window and the whole next one: the outlier is the previous window's
        // largest only until the window after it ends.
        for _ in 0..2 * WINDOW_CALLBACKS {
            sim.step(64, 64);
        }
        assert_eq!(sim.slack.keep_frames(), 64 * 3, "back at the base");
        // A host that keeps delivering bigger buffers is followed for good.
        for _ in 0..3 * WINDOW_CALLBACKS {
            sim.step(256, 256);
        }
        assert_eq!(sim.slack.keep_frames(), 256 * 3);
    }

    #[test]
    fn bursts_and_drift_together_stay_under_the_ceiling() {
        let mut sim = Sim::new(64, false);
        let (mut owed, mut acc) = (0usize, 0usize);
        for _ in 0..8 * WINDOW_CALLBACKS {
            owed += 64;
            acc += 64 * 1000;
            let drift = acc / 1_000_000;
            acc %= 1_000_000;
            let capture = if owed >= 480 {
                owed -= 480;
                480 + drift
            } else {
                drift
            };
            sim.step(64, capture);
        }
        let ceiling = 64 * 3 + sim.slack.max_extra_frames() as usize;
        assert!(sim.slack.extra_frames() <= sim.slack.max_extra_frames());
        assert!(
            sim.held_max <= ceiling,
            "held {} of {ceiling}",
            sim.held_max
        );
    }

    #[test]
    fn the_extra_allowance_is_bounded_in_time_for_small_and_large_buffers() {
        assert_eq!(InputSlack::new(64, 48_000).max_extra_frames(), 480);
        assert_eq!(InputSlack::new(4096, 48_000).max_extra_frames(), 1440);
        assert_eq!(InputSlack::new(256, 48_000).max_extra_frames(), 512);
    }

    #[test]
    fn a_take_lands_on_the_same_frame_at_the_smallest_and_the_largest_allowance() {
        // An impulse at capture frame 3000, input arriving in the same lumps both times and
        // nothing trimmed: it is read at the same output frame whatever the allowance, because
        // the allowance only decides what is discarded, never how the ring is read.
        fn read_position(raise: bool) -> Option<usize> {
            let mut s = InputSlack::new(512, 48_000);
            if raise {
                // Lumps of three buffers, trimmed, raise the allowance to its maximum.
                for _ in 0..100 {
                    s.after_read(0);
                    s.after_trim(3000, true);
                }
                assert_eq!(s.extra_frames(), s.max_extra_frames());
            }
            let (mut writer, mut reader) = sample_ring(65_536);
            let sizes = [96, 480, 512, 480];
            let (mut captured, mut played) = (0usize, 0usize);
            for n in 0..200 {
                let frames = sizes[n % 4];
                let upto = played + frames + 256;
                let lump: Vec<f32> = (captured..upto.max(captured))
                    .map(|i| if i == 3000 { 1.0 } else { 0.0 })
                    .collect();
                captured += lump.len();
                writer.write(&lump);
                s.on_callback(frames as u32);
                assert_eq!(reader.trim(s.keep_frames() as usize), 0, "nothing to trim");
                let mut out = vec![0.0; frames];
                reader.read(&mut out);
                if let Some(i) = out.iter().position(|x| *x > 0.5) {
                    return Some(played + i);
                }
                played += frames;
            }
            None
        }
        let low = read_position(false);
        assert_eq!(low, Some(3000));
        assert_eq!(low, read_position(true));
    }
}
