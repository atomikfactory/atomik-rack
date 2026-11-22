//! How much captured input the render callback lets wait in the input ring before it discards the
//! oldest frames.
//!
//! The allowed backlog follows the buffer the stream was opened with (and the largest callback a
//! driver delivered, when it delivers more than it was asked for), never the size of the callback
//! that happens to be running: shared-mode drivers deliver callbacks of uneven size, and a short
//! one must not turn input that merely arrived in a burst into a backlog to be thrown away.
//!
//! On top of that base the policy keeps an *extra* allowance. Every counted trim raises it by
//! one step, up to a named maximum; a long run of callbacks without a trim lowers it by one
//! step. A trim that keeps recurring at the maximum is real drift between two clocks, and the
//! ring is trimmed as before so the monitoring latency stays bounded.
//!
//! Plain integers owned by the render callback: no allocation, lock or log. Counted in
//! callbacks, never in wall time.

/// The base allowance: the current read plus this many buffers waiting.
pub(crate) const BASE_SLACK_BUFFERS: u32 = 2;

/// One step of the extra allowance is this fraction of a buffer (`1 / STEP_DIVISOR`).
const STEP_DIVISOR: u32 = 2;

/// The smallest step, in frames.
const MIN_STEP_FRAMES: u32 = 16;

/// The extra allowance never exceeds this many buffers ...
const MAX_EXTRA_BUFFERS: u32 = 2;

/// ... but is never below this many milliseconds of audio (so a very small buffer still has room
/// for a driver that delivers several buffers at once) ...
const MIN_EXTRA_MS: u32 = 10;

/// ... and never above this many (so a large buffer does not add a large delay on top).
const MAX_EXTRA_MS: u32 = 30;

/// Callbacks in a row without a trim before the extra allowance shrinks by one step.
pub(crate) const QUIET_CALLBACKS_PER_STEP: u32 = 2000;

/// The adaptive trim threshold.
#[derive(Debug, Clone)]
pub(crate) struct InputSlack {
    /// The buffer the stream was opened with, in frames.
    configured: u32,
    /// The largest callback seen, in frames.
    largest: u32,
    sample_rate: u32,
    /// The extra allowance now, in frames.
    extra: u32,
    /// Callbacks since the last trim or shrink.
    quiet: u32,
}

impl InputSlack {
    /// A policy for a stream opened with `configured` frames per buffer at `sample_rate`.
    pub(crate) fn new(configured: u32, sample_rate: u32) -> Self {
        Self {
            configured,
            largest: 0,
            sample_rate,
            extra: 0,
            quiet: 0,
        }
    }

    /// The buffer the thresholds are based on: the configured one or the largest callback seen.
    pub(crate) fn buffer(&self) -> u32 {
        self.configured.max(self.largest).max(1)
    }

    /// Notes a callback of `frames` frames (before its trim).
    pub(crate) fn on_callback(&mut self, frames: u32) {
        self.largest = self.largest.max(frames);
    }

    /// The most frames the ring may hold once the callback has trimmed it, including the
    /// callback's own read.
    pub(crate) fn keep_frames(&self) -> u32 {
        self.buffer()
            .saturating_mul(1 + BASE_SLACK_BUFFERS)
            .saturating_add(self.extra)
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

    /// The extra allowance now, in frames.
    #[cfg(test)]
    pub(crate) fn extra_frames(&self) -> u32 {
        self.extra
    }

    fn step(&self) -> u32 {
        (self.buffer() / STEP_DIVISOR).max(MIN_STEP_FRAMES)
    }

    /// The callback's trim is done: `counted` says it discarded frames that count as lost.
    pub(crate) fn after_trim(&mut self, counted: bool) {
        if counted {
            self.quiet = 0;
            self.extra = self
                .extra
                .saturating_add(self.step())
                .min(self.max_extra_frames());
        } else {
            self.quiet = self.quiet.saturating_add(1);
            if self.quiet >= QUIET_CALLBACKS_PER_STEP {
                self.quiet = 0;
                self.extra = self.extra.saturating_sub(self.step());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::sample_ring;

    /// Mono input of `capture(n)` frames before callback `n`, against output callbacks of the
    /// sizes in `sizes` (cycled). Returns (lost frames, counted trims). The first trim is not
    /// counted, as in the stream. `by_current_callback` applies the rule this policy replaced.
    fn run(
        slack: &mut InputSlack,
        sizes: &[usize],
        callbacks: usize,
        mut capture: impl FnMut(usize) -> usize,
        by_current_callback: bool,
    ) -> (u64, u64) {
        let (mut writer, mut reader) = sample_ring(65_536);
        let (mut lost, mut trims) = (0u64, 0u64);
        let mut first = true;
        for n in 0..callbacks {
            let frames = sizes[n % sizes.len()];
            writer.write(&vec![0.5; capture(n)]);
            slack.on_callback(frames as u32);
            let keep = if by_current_callback {
                frames * (1 + BASE_SLACK_BUFFERS as usize)
            } else {
                slack.keep_frames() as usize
            };
            let discarded = reader.trim(keep);
            let counted = !first && discarded > 0;
            first = false;
            if counted {
                lost += discarded as u64;
                trims += 1;
            }
            slack.after_trim(counted);
            reader.read(&mut vec![0.0; frames]);
        }
        (lost, trims)
    }

    #[test]
    fn bursts_against_callbacks_of_uneven_size_lose_nothing() {
        // Callbacks of 96, 480 and 512 frames in irregular order; the input arrives in lumps of
        // up to two buffers, and as many frames arrive as are read over a cycle.
        let sizes = [96, 480, 512, 480, 96, 512];
        let lumps = [1024, 0, 96, 544, 512, 0];
        assert_eq!(lumps.iter().sum::<usize>(), sizes.iter().sum::<usize>());
        let mut now = InputSlack::new(512, 48_000);
        let (lost, trims) = run(&mut now, &sizes, 600, |n| lumps[n % 6], false);
        assert_eq!((lost, trims), (0, 0), "no frame is discarded");
        let mut old = InputSlack::new(512, 48_000);
        let (lost, trims) = run(&mut old, &sizes, 600, |n| lumps[n % 6], true);
        assert!(lost > 0 && trims > 0, "the rule it replaced dropped {lost}");
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
    fn steady_drift_is_trimmed_counted_and_bounded() {
        let mut s = InputSlack::new(512, 48_000);
        // The capture clock runs 0.1 % fast: 1 frame more every 1000 frames read (every other
        // callback of 512 adds one, 0.098 %).
        let (lost, trims) = run(
            &mut s,
            &[512],
            20_000,
            |n| 512 + usize::from(n % 2 == 0),
            false,
        );
        assert!(trims > 0, "the drift is trimmed");
        // Everything the drift added beyond the allowed backlog is lost, and counted.
        assert_eq!(lost, 10_000 - u64::from(s.allowed_backlog_frames()));
        assert_eq!(s.extra_frames(), s.max_extra_frames(), "recurring trims");
        assert_eq!(
            s.allowed_backlog_frames(),
            512 * 2 + 1024,
            "the bound holds"
        );
    }

    #[test]
    fn the_extra_allowance_grows_on_trims_stops_at_its_maximum_and_shrinks_when_quiet() {
        let mut s = InputSlack::new(512, 48_000);
        assert_eq!(s.max_extra_frames(), 1024);
        assert_eq!(s.extra_frames(), 0);
        s.after_trim(true);
        assert_eq!(s.extra_frames(), 256);
        for _ in 0..10 {
            s.after_trim(true);
        }
        assert_eq!(s.extra_frames(), 1024, "stops at the maximum");
        assert_eq!(s.allowed_backlog_frames(), 1024 + 1024);
        // One short of the quiet stretch changes nothing, the next callback shrinks one step.
        for _ in 0..QUIET_CALLBACKS_PER_STEP - 1 {
            s.after_trim(false);
        }
        assert_eq!(s.extra_frames(), 1024);
        s.after_trim(false);
        assert_eq!(s.extra_frames(), 768);
        // A trim restarts the count.
        s.after_trim(true);
        for _ in 0..QUIET_CALLBACKS_PER_STEP - 1 {
            s.after_trim(false);
        }
        assert_eq!(s.extra_frames(), 1024);
        for _ in 0..QUIET_CALLBACKS_PER_STEP * 6 {
            s.after_trim(false);
        }
        assert_eq!(s.extra_frames(), 0, "back to the base, never below");
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
        fn read_position(extra_steps: usize) -> Option<usize> {
            let mut s = InputSlack::new(512, 48_000);
            for _ in 0..extra_steps {
                s.after_trim(true);
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
        let low = read_position(0);
        assert_eq!(low, Some(3000));
        assert_eq!(low, read_position(100));
    }
}
