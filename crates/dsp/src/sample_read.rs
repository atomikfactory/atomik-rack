//! Sample playback maths: cubic Hermite reads, a playhead with loops, an equal-power loop
//! crossfade and pan gains.
//!
//! Everything here is pure and deterministic: only IEEE `+ - * /`, `floor` and integer
//! arithmetic, plus the nested-Horner polynomial [`sin_quarter`] (no libm call, no fused
//! multiply-add), so every platform rounds identically.
//!
//! **Position.** A playhead position is an integer frame (`u64`) plus a fraction (`f64` in
//! `0..1`). Advancing adds the ratio to the fraction, carries `floor(fraction)` into the frame
//! and wraps loops with integer arithmetic, so a loop wrap lands on the exact frame and a
//! ratio of 1 or 2 never accumulates error (the fraction stays exactly 0). There is no
//! accumulating floating-point phase.
//!
//! **Loops.** Positions of a forward loop stay in `loop_start..loop_end`; a ping-pong loop
//! plays `loop_start..=loop_end − 1` forwards and backwards, each end frame once per turn (the
//! turning points are `loop_end − 1` and `loop_start`). A loop of length 1 holds its frame.
//!
//! **Crossfade.** Only forward loops crossfade (ping-pong ignores the crossfade length).
//! For the last `crossfade` frames before `loop_end` the output blends the tail audio at `x`
//! with the audio at `x − loop_len` (just before `loop_start`), so at the wrap the signal
//! continues without a step. The gains are equal power (`g_tail² + g_pre² = 1`).
//!
//! **Reverse.** A reversed region ([`PlayRegion::reversed`]) has no loop. Its playhead starts at
//! `end − 1`, moves downwards by the ratio (same integer-plus-fraction position, so a ratio of 1
//! reads every frame exactly) and is finished once the position is below `start` (the frame
//! `start` itself, with any fraction, is still played). The forward paths are untouched.
//!
//! **Edges.** Reads clamp the four interpolation frames to the buffer, so no position, ratio
//! or zone can read out of bounds; [`PlayRegion::new`] sanitises arbitrary zone fields.

/// How a [`PlayRegion`] loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopKind {
    /// Play from `start` to `end` once.
    #[default]
    Off,
    /// Wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Bounce between `loop_start` and `loop_end − 1`.
    PingPong,
}

/// Largest ratio a playhead advances by per sample (larger or non-finite values are limited).
pub const MAX_RATIO: f64 = 4_294_967_296.0;

/// `sin(t·π/2)` for `t` in `[0, 1]` (clamped) using only IEEE `f64` arithmetic in a fixed order
/// (nested Horner Taylor series to `x^23`, error below `1e-20`).
pub fn sin_quarter(t: f64) -> f64 {
    let x = t.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
    let x2 = x * x;
    let mut acc = 1.0f64;
    let mut k = 11u32;
    while k >= 1 {
        let d = f64::from(2 * k * (2 * k + 1));
        acc = 1.0 - acc * x2 / d;
        k -= 1;
    }
    x * acc
}

/// Equal-power crossfade gains at fade position `t` (clamped to `0..=1`):
/// `(tail, pre) = (cos(t·π/2), sin(t·π/2))`, so `tail² + pre² = 1` (to rounding).
pub fn crossfade_gains(t: f64) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    (sin_quarter(1.0 - t) as f32, sin_quarter(t) as f32)
}

/// Constant-power pan for a mono source: `pan` −1..=1 (clamped), −3 dB at the centre.
pub fn pan_gains_mono(pan: f32) -> (f32, f32) {
    let u = (f64::from(pan.clamp(-1.0, 1.0)) + 1.0) * 0.5;
    (sin_quarter(1.0 - u) as f32, sin_quarter(u) as f32)
}

/// Balance gains for a stereo source: unity at the centre, the far side falls as
/// `cos(|pan|·π/2)` and the near side stays at 1.
pub fn balance_gains(pan: f32) -> (f32, f32) {
    let p = f64::from(pan.clamp(-1.0, 1.0));
    (
        sin_quarter(1.0 - p.max(0.0)) as f32,
        sin_quarter(1.0 + p.min(0.0)) as f32,
    )
}

/// Four-point, third-order Hermite (Catmull-Rom) interpolation between `y1` (at `t = 0`) and
/// `y2` (at `t = 1`) with neighbours `y0` and `y3`. Exactly `y1` at `t = 0`.
#[inline]
pub fn hermite4(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    if t == 0.0 {
        return y1;
    }
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Reads channel `ch` at frame `pos` plus fraction `frac` (`0..1`) with [`hermite4`]. The four
/// frames are clamped to the buffer, so any `pos` is safe; an empty channel reads 0.
#[inline]
pub fn read_hermite(ch: &[f32], pos: u64, frac: f64) -> f32 {
    let Some(last) = ch.len().checked_sub(1) else {
        return 0.0;
    };
    let p = usize::try_from(pos).unwrap_or(usize::MAX);
    let at = |i: usize| ch[i.min(last)];
    hermite4(
        at(p.saturating_sub(1)),
        at(p),
        at(p.saturating_add(1)),
        at(p.saturating_add(2)),
        frac as f32,
    )
}

/// The playable frames of a zone, sanitised so every playhead operation is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayRegion {
    start: u64,
    end: u64,
    kind: LoopKind,
    loop_start: u64,
    loop_end: u64,
    crossfade: u64,
    reverse: bool,
}

impl PlayRegion {
    /// A region over `frames` buffer frames. Any input is accepted: `end` is limited to
    /// `frames`, `start` is below `end` (otherwise the region is empty), the loop lies inside
    /// `start..end` with at least one frame, the crossfade is at most the loop length and at
    /// most `loop_start − start`, and ping-pong has no crossfade. Loop fields are ignored for
    /// [`LoopKind::Off`].
    pub fn new(
        start: u64,
        end: u64,
        kind: LoopKind,
        loop_start: u64,
        loop_end: u64,
        crossfade: u64,
        frames: u64,
    ) -> Self {
        let end = end.min(frames);
        if start >= end {
            return Self {
                start: 0,
                end: 0,
                kind: LoopKind::Off,
                loop_start: 0,
                loop_end: 0,
                crossfade: 0,
                reverse: false,
            };
        }
        if kind == LoopKind::Off {
            return Self {
                start,
                end,
                kind,
                loop_start: 0,
                loop_end: 0,
                crossfade: 0,
                reverse: false,
            };
        }
        let ls = loop_start.max(start).min(end - 1);
        let le = loop_end.max(ls + 1).min(end);
        let crossfade = if kind == LoopKind::Forward {
            crossfade.min(le - ls).min(ls - start)
        } else {
            0
        };
        Self {
            start,
            end,
            kind,
            loop_start: ls,
            loop_end: le,
            crossfade,
            reverse: false,
        }
    }

    /// A reversed region over `frames` buffer frames: plays `end − 1` down to `start`, no loop.
    /// `end` is limited to `frames`; an empty range gives an empty region.
    pub fn reversed(start: u64, end: u64, frames: u64) -> Self {
        let mut r = Self::new(start, end, LoopKind::Off, 0, 0, 0, frames);
        r.reverse = !r.is_empty();
        r
    }

    /// True when the region plays backwards.
    pub fn reverse(&self) -> bool {
        self.reverse
    }

    /// First frame played.
    pub fn start(&self) -> u64 {
        self.start
    }
    /// End frame (exclusive).
    pub fn end(&self) -> u64 {
        self.end
    }
    /// Loop kind.
    pub fn kind(&self) -> LoopKind {
        self.kind
    }
    /// Loop start frame (0 when not looping).
    pub fn loop_start(&self) -> u64 {
        self.loop_start
    }
    /// Loop end frame, exclusive (0 when not looping).
    pub fn loop_end(&self) -> u64 {
        self.loop_end
    }
    /// Crossfade length in frames (0 unless a forward loop).
    pub fn crossfade(&self) -> u64 {
        self.crossfade
    }
    /// True when no frame can be played.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
    fn loop_len(&self) -> u64 {
        self.loop_end - self.loop_start
    }
    /// Half of the ping-pong period (frames between the turning points).
    fn half(&self) -> u64 {
        self.loop_len() - 1
    }
    /// Ping-pong period in phase units.
    fn period(&self) -> u64 {
        (2 * self.half()).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Straight playback (the run-in, or the whole region without a ping-pong loop).
    Linear { pos: u64, frac: f64 },
    /// Inside a ping-pong loop: `n + frac` is the unfolded distance in `0..period` from
    /// `loop_start`; the first half runs forwards, the second backwards.
    PingPong { n: u64, frac: f64 },
    /// Backwards playback of a reversed region: the position is `pos + frac`.
    Reverse { pos: u64, frac: f64 },
}

/// A sample playhead over a [`PlayRegion`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playhead {
    region: PlayRegion,
    phase: Phase,
    finished: bool,
}

impl Playhead {
    /// A playhead at the region's start.
    pub fn new(region: PlayRegion) -> Self {
        if region.reverse {
            return Self::at(region, region.end - 1, 0.0);
        }
        Self::at(region, region.start, 0.0)
    }

    /// A playhead at `pos + frac` (`pos` is limited to the region; the direction of a
    /// ping-pong loop is forwards). Used to move a sounding voice onto a changed zone.
    pub fn at(region: PlayRegion, pos: u64, frac: f64) -> Self {
        let mut h = Self {
            region,
            phase: Phase::Linear { pos: 0, frac: 0.0 },
            finished: region.is_empty(),
        };
        if h.finished {
            return h;
        }
        let frac = if (0.0..1.0).contains(&frac) {
            frac
        } else {
            0.0
        };
        if region.reverse {
            let pos = pos.min(region.end - 1);
            h.phase = Phase::Reverse { pos, frac };
            h.finished = pos < region.start;
            return h;
        }
        let pos = pos.max(region.start);
        h.phase = Phase::Linear { pos, frac };
        h.normalise();
        h
    }

    /// Applies the region's end and loop rules to a linear position.
    fn normalise(&mut self) {
        let r = self.region;
        let Phase::Linear { pos, frac } = self.phase else {
            return;
        };
        match r.kind {
            LoopKind::Off => {
                if pos >= r.end {
                    self.finished = true;
                }
            }
            LoopKind::Forward => {
                if pos >= r.loop_end {
                    let p = r.loop_start + (pos - r.loop_end) % r.loop_len();
                    self.phase = Phase::Linear { pos: p, frac };
                }
            }
            LoopKind::PingPong => {
                if pos >= r.loop_start {
                    let n = (pos - r.loop_start) % r.period();
                    self.phase = Phase::PingPong { n, frac };
                }
            }
        }
    }

    /// Advances by `ratio` frames (negative, NaN and zero hold the position; values above
    /// [`MAX_RATIO`] are limited). A no-op once finished.
    pub fn advance(&mut self, ratio: f64) {
        if self.finished {
            return;
        }
        let ratio = if ratio > 0.0 {
            ratio.min(MAX_RATIO)
        } else {
            0.0
        };
        match &mut self.phase {
            Phase::Linear { pos, frac } => {
                let f = *frac + ratio;
                let whole = f.floor();
                *frac = f - whole;
                *pos = pos.saturating_add(whole as u64);
                self.normalise();
            }
            Phase::PingPong { n, frac } => {
                let p = self.region.period();
                let f = *frac + ratio;
                let whole = f.floor();
                *frac = f - whole;
                *n = (*n + (whole as u64) % p) % p;
            }
            Phase::Reverse { pos, frac } => {
                let f = *frac - ratio;
                let whole = f.floor();
                *frac = f - whole;
                let down = (-whole) as u64;
                if down > *pos || *pos - down < self.region.start {
                    self.finished = true;
                } else {
                    *pos -= down;
                }
            }
        }
    }

    /// True once a non-looping region has played past its end (a reversed one, below its
    /// start), or the region is empty.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// The region this playhead plays.
    pub fn region(&self) -> &PlayRegion {
        &self.region
    }

    /// The read position as frame and fraction (`0 <= frac < 1`).
    pub fn position(&self) -> (u64, f64) {
        match self.phase {
            Phase::Linear { pos, frac } | Phase::Reverse { pos, frac } => (pos, frac),
            Phase::PingPong { n, frac } => {
                let r = &self.region;
                let w = r.half();
                if w == 0 {
                    (r.loop_start, 0.0)
                } else if n < w {
                    (r.loop_start + n, frac)
                } else {
                    let m = 2 * w - n;
                    if frac == 0.0 {
                        (r.loop_start + m, 0.0)
                    } else {
                        (r.loop_start + m - 1, 1.0 - frac)
                    }
                }
            }
        }
    }

    /// Crossfade state of a forward loop: `(t, partner frame)` while inside the fade zone
    /// (the last `crossfade` frames before `loop_end`), where `t` runs 0..1 and the partner
    /// audio is read at `partner frame` plus the same fraction.
    pub fn crossfade_state(&self) -> Option<(f64, u64)> {
        let r = &self.region;
        if r.kind != LoopKind::Forward || r.crossfade == 0 || self.finished {
            return None;
        }
        let Phase::Linear { pos, frac } = self.phase else {
            return None;
        };
        let fade_start = r.loop_end - r.crossfade;
        if pos < fade_start {
            return None;
        }
        let t = ((pos - fade_start) as f64 + frac) / r.crossfade as f64;
        Some((t, pos - r.loop_len()))
    }

    /// Reads channel `ch` at the playhead: Hermite interpolation, with the loop crossfade
    /// applied inside the fade zone. A finished playhead reads 0.
    pub fn read(&self, ch: &[f32]) -> f32 {
        if self.finished {
            return 0.0;
        }
        let (pos, frac) = self.position();
        let a = read_hermite(ch, pos, frac);
        match self.crossfade_state() {
            Some((t, partner)) => {
                let (g_tail, g_pre) = crossfade_gains(t);
                a * g_tail + read_hermite(ch, partner, frac) * g_pre
            }
            None => a,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    fn region(kind: LoopKind, ls: u64, le: u64, xf: u64) -> PlayRegion {
        PlayRegion::new(0, 100, kind, ls, le, xf, 100)
    }

    /// Frames visited (integer part) over `n` steps of ratio 1.
    fn visit(r: PlayRegion, n: usize) -> Vec<u64> {
        let mut h = Playhead::new(r);
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(h.position().0);
            h.advance(1.0);
        }
        v
    }

    #[test]
    fn root_ratio_reads_every_frame_bit_exactly() {
        let data: Vec<f32> = (0..64)
            .map(|i| ((i * 37 % 11) as f32 - 5.0) / 3.0)
            .collect();
        let r = PlayRegion::new(8, 60, LoopKind::Off, 0, 0, 0, 64);
        let mut h = Playhead::new(r);
        let mut got = Vec::new();
        while !h.finished() {
            got.push(h.read(&data));
            h.advance(1.0);
        }
        let want: Vec<u32> = data[8..60].iter().map(|x| x.to_bits()).collect();
        assert_eq!(got.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), want);
    }

    #[test]
    fn octave_up_reads_every_second_frame() {
        let data = ramp(64);
        let r = PlayRegion::new(0, 64, LoopKind::Off, 0, 0, 0, 64);
        let mut h = Playhead::new(r);
        let mut got = Vec::new();
        while !h.finished() {
            got.push(h.read(&data));
            h.advance(2.0);
        }
        let want: Vec<f32> = (0..32).map(|i| (2 * i) as f32).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn hermite_sine_at_ratio_1_5_stays_within_error_bound() {
        // 0.02 cycles per frame (a 50-frame period), read at 1.5 frames per output sample.
        let w = std::f64::consts::TAU * 0.02;
        let data: Vec<f32> = (0..400).map(|i| (w * i as f64).sin() as f32).collect();
        let r = PlayRegion::new(0, 400, LoopKind::Off, 0, 0, 0, 400);
        let mut h = Playhead::new(r);
        let mut worst = 0.0f64;
        let mut n = 0u32;
        // Stay two frames inside the buffer: the clamped edge frames are not interpolation.
        while !h.finished() && h.position().0 < 397 {
            let (p, f) = h.position();
            if p >= 1 {
                let x = p as f64 + f;
                worst = worst.max((f64::from(h.read(&data)) - (w * x).sin()).abs());
            }
            h.advance(1.5);
            n += 1;
        }
        assert!(n > 200);
        assert!(worst < 5e-4, "worst interpolation error {worst}");
    }

    #[test]
    fn off_ends_exactly_at_end() {
        let r = PlayRegion::new(10, 15, LoopKind::Off, 0, 0, 0, 100);
        assert_eq!(visit(r, 5), vec![10, 11, 12, 13, 14]);
        let mut h = Playhead::new(r);
        for _ in 0..4 {
            h.advance(1.0);
        }
        assert!(!h.finished());
        h.advance(1.0);
        assert!(h.finished());
        assert_eq!(h.read(&ramp(100)), 0.0);
    }

    #[test]
    fn forward_loop_wraps_at_exact_frames() {
        let v = visit(region(LoopKind::Forward, 10, 14, 0), 14);
        assert_eq!(v, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        let v = visit(region(LoopKind::Forward, 2, 5, 0), 12);
        assert_eq!(v, vec![0, 1, 2, 3, 4, 2, 3, 4, 2, 3, 4, 2]);
        // A fractional ratio keeps its remainder across the wrap.
        let mut h = Playhead::new(region(LoopKind::Forward, 2, 5, 0));
        h.advance(5.75);
        assert_eq!(h.position().0, 2);
        assert!((h.position().1 - 0.75).abs() < 1e-12);
    }

    #[test]
    fn ping_pong_turns_at_exact_frames() {
        let v = visit(region(LoopKind::PingPong, 2, 6, 0), 16);
        // Run-in 0, 1; then 2..=5 forwards, 4..=3 backwards, each end frame once per turn.
        assert_eq!(v, vec![0, 1, 2, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3, 2, 3]);
        // Ratio 0.5: the fractional positions mirror.
        let mut h = Playhead::new(region(LoopKind::PingPong, 2, 6, 0));
        let mut got = Vec::new();
        for _ in 0..16 {
            let (p, f) = h.position();
            got.push(p as f64 + f);
            h.advance(0.5);
        }
        let want = [
            0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 4.5, 4.0, 3.5, 3.0, 2.5,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn ping_pong_from_the_loop_start_and_length_one() {
        let r = PlayRegion::new(2, 6, LoopKind::PingPong, 2, 6, 0, 10);
        assert_eq!(
            visit(r, 9),
            vec![2, 3, 4, 5, 4, 3, 2, 3, 4],
            "no run-in when the loop starts at start"
        );
        let one = PlayRegion::new(0, 10, LoopKind::PingPong, 4, 5, 0, 10);
        assert_eq!(visit(one, 8), vec![0, 1, 2, 3, 4, 4, 4, 4]);
        let one = PlayRegion::new(0, 10, LoopKind::Forward, 4, 5, 0, 10);
        assert_eq!(visit(one, 8), vec![0, 1, 2, 3, 4, 4, 4, 4]);
    }

    #[test]
    fn huge_ratios_stay_inside_the_loop() {
        for kind in [LoopKind::Forward, LoopKind::PingPong] {
            let mut h = Playhead::new(region(kind, 20, 50, 0));
            for ratio in [1e3, 12_345.678, 1e12, f64::INFINITY, f64::NAN, -3.0] {
                h.advance(ratio);
                let (p, f) = h.position();
                assert!((0..100).contains(&p) && (0.0..1.0).contains(&f), "{p} {f}");
            }
        }
    }

    #[test]
    fn region_sanitises_degenerate_fields() {
        let r = PlayRegion::new(5, 5, LoopKind::Forward, 0, 0, 0, 10);
        assert!(r.is_empty() && Playhead::new(r).finished());
        let r = PlayRegion::new(0, 1000, LoopKind::Off, 0, 0, 0, 10);
        assert_eq!(r.end(), 10);
        let r = PlayRegion::new(2, 8, LoopKind::Forward, 100, 3, 99, 10);
        assert_eq!((r.loop_start(), r.loop_end(), r.crossfade()), (7, 8, 1));
        let r = PlayRegion::new(2, 8, LoopKind::Forward, 4, 8, 99, 10);
        assert_eq!(r.crossfade(), 2, "at most loop_start − start");
        let r = PlayRegion::new(0, 8, LoopKind::PingPong, 4, 8, 3, 10);
        assert_eq!(r.crossfade(), 0, "ping-pong ignores the crossfade");
        assert!(PlayRegion::new(0, 5, LoopKind::Off, 0, 0, 0, 0).is_empty());
    }

    #[test]
    fn reads_clamp_at_the_buffer_edges() {
        let d = [1.0f32, 2.0, 3.0];
        assert!(read_hermite(&d, 0, 0.5).is_finite());
        assert_eq!(read_hermite(&d, 1_000, 0.5), 3.0);
        assert_eq!(read_hermite(&d, u64::MAX, 0.9), 3.0);
        assert_eq!(read_hermite(&[], 0, 0.5), 0.0);
        assert_eq!(read_hermite(&d, 2, 0.0), 3.0);
    }

    #[test]
    fn crossfade_gains_are_equal_power() {
        for i in 0..=1000 {
            let t = f64::from(i) / 1000.0;
            let (a, b) = crossfade_gains(t);
            let sum = f64::from(a) * f64::from(a) + f64::from(b) * f64::from(b);
            assert!((sum - 1.0).abs() < 1e-6, "t {t}: {sum}");
        }
        assert_eq!(crossfade_gains(0.0), (1.0, 0.0));
        assert_eq!(crossfade_gains(1.0), (0.0, 1.0));
    }

    #[test]
    fn pan_gains_are_constant_power_and_balance_is_unity_at_centre() {
        for i in -10..=10 {
            let (l, r) = pan_gains_mono(i as f32 / 10.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-6);
        }
        let (l, r) = pan_gains_mono(0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && l.to_bits() == r.to_bits());
        assert_eq!(balance_gains(0.0), (1.0, 1.0));
        assert_eq!(balance_gains(-1.0), (1.0, 0.0));
        assert_eq!(balance_gains(1.0), (0.0, 1.0));
        let (l, r) = balance_gains(0.5);
        assert_eq!(l.to_bits(), (sin_quarter(0.5) as f32).to_bits());
        assert_eq!(r, 1.0);
    }

    #[test]
    fn sin_quarter_matches_libm() {
        for i in 0..=100 {
            let t = f64::from(i) / 100.0;
            assert!((sin_quarter(t) - (t * std::f64::consts::FRAC_PI_2).sin()).abs() < 1e-15);
        }
    }

    /// A sine whose period (37 frames) does not divide the loop (500 frames).
    fn sine_data() -> Vec<f32> {
        (0..2000)
            .map(|i| (std::f64::consts::TAU * f64::from(i) / 37.0).sin() as f32)
            .collect()
    }

    fn render_loop(xf: u64, kind: LoopKind, n: usize) -> Vec<f32> {
        let data = sine_data();
        let r = PlayRegion::new(0, 2000, kind, 600, 1100, xf, 2000);
        let mut h = Playhead::new(r);
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(h.read(&data));
            h.advance(1.0);
        }
        out
    }

    #[test]
    fn crossfade_reduces_the_step_at_the_loop_point() {
        let n = 1700; // run-in 600, then a bit more than two passes through the loop
        let plain = render_loop(0, LoopKind::Forward, n);
        let faded = render_loop(200, LoopKind::Forward, n);
        // The wrap happens when the position reaches 1100, i.e. at output index 1100.
        let step = |x: &[f32]| (x[1100] - x[1099]).abs();
        assert!(step(&plain) > 0.1, "premise: a hard loop clicks");
        assert!(
            step(&faded) < 0.5 * step(&plain),
            "{} vs {}",
            step(&faded),
            step(&plain)
        );
        let max_step = |x: &[f32]| {
            x.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max)
        };
        assert!(max_step(&faded[600..]) < max_step(&plain[600..]));
    }

    #[test]
    fn ping_pong_ignores_the_crossfade() {
        assert_eq!(
            render_loop(200, LoopKind::PingPong, 1700),
            render_loop(0, LoopKind::PingPong, 1700)
        );
    }

    #[test]
    fn at_moves_a_playhead_onto_a_changed_region() {
        let r = region(LoopKind::Forward, 10, 20, 0);
        assert_eq!(Playhead::at(r, 55, 0.25).position().0, 15);
        let r = region(LoopKind::Off, 0, 0, 0);
        assert!(Playhead::at(r, 150, 0.0).finished());
        let r = PlayRegion::new(10, 50, LoopKind::Off, 0, 0, 0, 100);
        assert_eq!(Playhead::at(r, 3, f64::NAN).position(), (10, 0.0));
    }

    #[test]
    fn reverse_visits_end_minus_one_down_to_start_bit_exactly() {
        let data: Vec<f32> = (0..64)
            .map(|i| ((i * 37 % 11) as f32 - 5.0) / 3.0)
            .collect();
        let r = PlayRegion::reversed(8, 60, 64);
        assert!(r.reverse() && !r.is_empty());
        let mut h = Playhead::new(r);
        let mut got = Vec::new();
        while !h.finished() {
            got.push(h.read(&data).to_bits());
            h.advance(1.0);
        }
        let want: Vec<u32> = data[8..60].iter().rev().map(|x| x.to_bits()).collect();
        assert_eq!(got, want);
        assert_eq!(h.read(&data), 0.0);
    }

    #[test]
    fn reverse_fractional_ratio_and_edges() {
        let r = PlayRegion::reversed(2, 6, 10);
        let mut h = Playhead::new(r);
        let mut got = Vec::new();
        while !h.finished() {
            let (p, f) = h.position();
            got.push(p as f64 + f);
            h.advance(0.5);
        }
        assert_eq!(got, vec![5.0, 4.5, 4.0, 3.5, 3.0, 2.5, 2.0]);
        // Ratio 2 from end-1: 5, 3, then 1 is below start.
        let mut h = Playhead::new(r);
        let mut v = Vec::new();
        while !h.finished() {
            v.push(h.position().0);
            h.advance(2.0);
        }
        assert_eq!(v, vec![5, 3]);
        // `at` clamps into the region; below start is finished.
        assert_eq!(Playhead::at(r, 100, 0.25).position(), (5, 0.25));
        assert!(Playhead::at(r, 1, 0.0).finished());
        assert!(PlayRegion::reversed(5, 5, 10).is_empty());
        assert!(Playhead::new(PlayRegion::reversed(5, 5, 10)).finished());
        // Huge, negative and NaN ratios stay safe.
        let mut h = Playhead::new(r);
        h.advance(f64::NAN);
        h.advance(-3.0);
        assert_eq!(h.position(), (5, 0.0));
        h.advance(1e12);
        assert!(h.finished());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn reversed_playhead_stays_in_bounds(
            frames in 0u64..80,
            start in 0u64..120,
            end in 0u64..120,
            ratios in prop::collection::vec(
                prop_oneof![0.0f64..4.0, 0.0f64..1e6, Just(f64::NAN), Just(f64::INFINITY), -5.0f64..0.0],
                1..40,
            ),
        ) {
            let data: Vec<f32> = (0..frames).map(|i| (i as f32).sin()).collect();
            let r = PlayRegion::reversed(start, end, frames);
            let mut h = Playhead::new(r);
            for ratio in ratios.iter().cycle().take(200) {
                let (p, f) = h.position();
                prop_assert!((0.0..1.0).contains(&f));
                if !h.finished() {
                    prop_assert!(p >= r.start() && p < r.end());
                }
                prop_assert!(h.read(&data).is_finite());
                h.advance(*ratio);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn any_zone_and_ratio_reads_in_bounds_and_stays_finite(
            frames in 0u64..80,
            start in 0u64..120,
            end in 0u64..120,
            kind in 0u8..3,
            ls in 0u64..120,
            le in 0u64..120,
            xf in 0u64..200,
            ratios in prop::collection::vec(
                prop_oneof![0.0f64..4.0, 0.0f64..1e6, Just(f64::NAN), Just(f64::INFINITY), -5.0f64..0.0],
                1..40,
            ),
        ) {
            let kind = [LoopKind::Off, LoopKind::Forward, LoopKind::PingPong][kind as usize];
            let data: Vec<f32> = (0..frames).map(|i| (i as f32).sin()).collect();
            let r = PlayRegion::new(start, end, kind, ls, le, xf, frames);
            prop_assert!(r.end() <= frames);
            if !r.is_empty() && kind != LoopKind::Off {
                prop_assert!(r.start() <= r.loop_start() && r.loop_start() < r.loop_end());
                prop_assert!(r.loop_end() <= r.end());
                prop_assert!(r.crossfade() <= r.loop_end() - r.loop_start());
                prop_assert!(r.crossfade() <= r.loop_start() - r.start());
            }
            let mut h = Playhead::new(r);
            for ratio in ratios.iter().cycle().take(200) {
                let (p, f) = h.position();
                prop_assert!((0.0..1.0).contains(&f), "frac {f}");
                if !h.finished() {
                    prop_assert!(p >= r.start() && p < r.end(), "pos {p} outside {r:?}");
                    if let Some((t, partner)) = h.crossfade_state() {
                        prop_assert!((0.0..1.0).contains(&t));
                        prop_assert!(partner >= r.start() && partner < r.end());
                    }
                }
                prop_assert!(h.read(&data).is_finite());
                h.advance(*ratio);
            }
        }
    }
}
