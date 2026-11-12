//! The real-time stretcher: a streaming, allocation-free WSOLA that keeps the pitch while it
//! follows a time map, for the preview of a warped clip whose finished buffer is still being
//! rendered.
//!
//! It is built once with the highest sample rate it will see (that allocates), then
//! [`RtStretcher::process`] and [`RtStretcher::reset`] run without allocating, locking or
//! calling the system. The source is random access (a whole memory asset) and is read through a
//! time map given as `(output frame, source frame)` knots, so the stretch ratio can change from
//! one block to the next (warp markers, tempo ramps).
//!
//! **WSOLA** as in the offline beats engine: Hann frames of about 30 ms at a hop of half a
//! frame. Each frame is centred on the source position the map asks for, moved by up to a
//! quarter frame (about 8 ms) to the offset whose start best continues the previous frame. The
//! similarity search runs on the sum of both channels at a stride of 8 samples over candidates
//! 4 frames apart, then refines the best one sample at a time at a stride of 2, so its cost is
//! bounded per frame. Both channels use the chosen offset (stereo-linked). Overlapping frames
//! add up in `f64` and are rounded to `f32` once, so where every frame continues the previous
//! one exactly (a ratio of 1, no transposition) the output is the source, bit for bit.
//!
//! **Pitch** is the offline identity: the frames build a signal `r` times as long as the map
//! asks (`r` the pitch ratio), and that signal is read `r` times as fast with four-point
//! Hermite interpolation from a small ring. Output frame `t` reads the stretched signal at
//! `(t - t0) * r`, `t0` being the frame of the last reset.
//!
//! **Locates.** The stretcher remembers the frame after the last block. A block that starts
//! elsewhere, a new pitch ratio, a new sample rate or an explicit [`RtStretcher::reset`]
//! restart it there: one frame is placed straight at the map's position and the next ones
//! continue from it, so the first output frame is already a full overlap of real source (no
//! fade from silence and nothing left of the old state).

use crate::det_trig::cos_cycles;

/// The highest sample rate the engine builds its stretchers for.
pub const RT_MAX_SAMPLE_RATE: u32 = 192_000;

/// Frame length in milliseconds (the offline beats engine's).
const FRAME_MS: f64 = 30.0;
/// Similarity search: candidate step, sample stride of the coarse pass and of the refinement.
const STEP: i64 = 4;
const COARSE_STRIDE: usize = 8;
const FINE_STRIDE: usize = 2;
/// Below this energy (sum of squares of the target's samples read) the search is skipped.
const QUIET: f64 = 1.0e-12;

/// Frame length for a sample rate: even, at least 64.
fn frame_len(sample_rate: u32) -> usize {
    ((FRAME_MS * f64::from(sample_rate) / 1000.0).round() as usize & !1).max(64)
}

/// Four-point Hermite between `y1` and `y2`; exactly `y1` at `t == 0`.
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    if t == 0.0 {
        return y1;
    }
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Source frame `i` of a channel (silence outside it).
fn at(ch: &[f32], i: i64) -> f64 {
    if i < 0 {
        return 0.0;
    }
    ch.get(i as usize).map_or(0.0, |v| f64::from(*v))
}

/// The source frame a map reads at output frame `t`: linear between knots, continuing the end
/// segments outside them. `seg` is a cursor (it moves either way).
fn map_at(knots: &[(f64, f64)], t: f64, seg: &mut usize) -> f64 {
    let last = knots.len() - 2;
    *seg = (*seg).min(last);
    while *seg > 0 && t < knots[*seg].0 {
        *seg -= 1;
    }
    while *seg < last && t >= knots[*seg + 1].0 {
        *seg += 1;
    }
    let (a, b) = (knots[*seg], knots[*seg + 1]);
    a.1 + (t - a.0) * (b.1 - a.1) / (b.0 - a.0)
}

/// A streaming WSOLA stretcher for one stereo source (see the module docs).
pub struct RtStretcher {
    /// Largest frame length (sizes the buffers).
    max_len: usize,
    /// Sample rate in effect (0 before the first block).
    sample_rate: u32,
    /// Frame length and hop in effect.
    len: usize,
    hop: usize,
    /// Hann window of `len` (the first `len` entries).
    window: Vec<f64>,
    /// Start a fresh stream at the next block.
    fresh: bool,
    /// Output frame expected next.
    next: u64,
    /// Pitch ratio in effect.
    ratio: f64,
    /// Output frame of the last reset (stretched-signal index 0).
    base: u64,
    /// Map cursor.
    seg: usize,
    /// Source frame where the last placed frame started.
    prev: i64,
    /// Stretched-signal index where the next frame starts; everything before it is complete.
    done: i64,
    /// The second halves of the last frame, waiting for the next one (per channel).
    tail: [Vec<f64>; 2],
    /// The completed stretched signal, a ring indexed by `index & mask` (per channel).
    ring: [Vec<f32>; 2],
    mask: usize,
    /// The mono target of the similarity search.
    target: Vec<f64>,
}

impl RtStretcher {
    /// A stretcher for sample rates up to `max_sample_rate` (allocates its buffers once; a
    /// higher rate later plays with this rate's frame length).
    pub fn new(max_sample_rate: u32) -> Self {
        let max_len = frame_len(max_sample_rate.max(1));
        let hop = max_len / 2;
        let ring = (hop + 8).next_power_of_two();
        Self {
            max_len,
            sample_rate: 0,
            len: max_len,
            hop,
            window: vec![0.0; max_len],
            fresh: true,
            next: 0,
            ratio: 1.0,
            base: 0,
            seg: 0,
            prev: 0,
            done: 0,
            tail: [vec![0.0; hop], vec![0.0; hop]],
            ring: [vec![0.0; ring], vec![0.0; ring]],
            mask: ring - 1,
            target: vec![0.0; hop],
        }
    }

    /// Makes the next block start a fresh stream at output frame `at` (a locate, a loop wrap or
    /// a new clip in this stretcher). Allocation-free.
    pub fn reset(&mut self, at: u64) {
        self.fresh = true;
        self.next = at;
    }

    /// The output frame the next block continues from, if a stream is running.
    pub fn next_frame(&self) -> Option<u64> {
        (!self.fresh).then_some(self.next)
    }

    /// Renders output frames `start .. start + n` (`n` the shorter output's length) of the
    /// stereo `source` (both slices the same length; pass one channel twice for mono) read
    /// through `knots` (`(output frame, source frame)`, at least two, strictly increasing in
    /// the first value; fewer give silence), transposed by `pitch_ratio` (clamped to
    /// [`super::PITCH_RATIO_MIN`]`..=`[`super::PITCH_RATIO_MAX`]; 1 for none). Source frames
    /// outside the source are silent. Allocation-free.
    pub fn process(
        &mut self,
        source: [&[f32]; 2],
        knots: &[(f64, f64)],
        pitch_ratio: f64,
        sample_rate: u32,
        start: u64,
        out: [&mut [f32]; 2],
    ) {
        let [ol, or] = out;
        let n = ol.len().min(or.len());
        if knots.len() < 2 {
            ol[..n].fill(0.0);
            or[..n].fill(0.0);
            self.fresh = true;
            return;
        }
        let ratio = if pitch_ratio.is_finite() {
            pitch_ratio.clamp(super::PITCH_RATIO_MIN, super::PITCH_RATIO_MAX)
        } else {
            1.0
        };
        if sample_rate != self.sample_rate {
            self.configure(sample_rate);
        }
        if self.fresh || start != self.next || ratio.to_bits() != self.ratio.to_bits() {
            self.restart(source, knots, ratio, start);
        }
        for k in 0..n {
            let p = (start + k as u64 - self.base) as f64 * self.ratio;
            let whole = p.floor();
            let i = whole as i64;
            while self.done <= i + 2 {
                self.place(source, knots);
            }
            let t = (p - whole) as f32;
            for (ring, o) in self.ring.iter().zip([&mut *ol, &mut *or]) {
                let y = |j: i64| ring[(j as usize) & self.mask];
                o[k] = hermite(y(i - 1), y(i), y(i + 1), y(i + 2), t);
            }
        }
        self.next = start + n as u64;
    }

    /// Takes the frame length of `sample_rate` and restarts at the next block.
    fn configure(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.len = frame_len(sample_rate).min(self.max_len);
        self.hop = self.len / 2;
        for (i, w) in self.window[..self.len].iter_mut().enumerate() {
            *w = 0.5 - 0.5 * cos_cycles(i as f64 / self.len as f64);
        }
        self.fresh = true;
    }

    /// Starts a stream at output frame `start`: one frame placed at the map's position, so the
    /// next one completes the stretched signal from just before index 0.
    fn restart(&mut self, source: [&[f32]; 2], knots: &[(f64, f64)], ratio: f64, start: u64) {
        self.fresh = false;
        self.ratio = ratio;
        self.base = start;
        self.seg = 0;
        for t in &mut self.tail {
            t[..self.hop].fill(0.0);
        }
        let hop = self.hop as i64;
        self.done = -2 * hop;
        let a = self.nominal(knots, self.done);
        self.write(source, a);
    }

    /// The source frame the frame starting at stretched index `j` starts at, as the map asks:
    /// the frame's centre on the map's position.
    fn nominal(&mut self, knots: &[(f64, f64)], j: i64) -> i64 {
        let hop = self.hop as i64;
        let t = self.base as f64 + (j + hop) as f64 / self.ratio;
        map_at(knots, t, &mut self.seg).round() as i64 - hop
    }

    /// Places the next frame at the offset that best continues the previous one.
    fn place(&mut self, source: [&[f32]; 2], knots: &[(f64, f64)]) {
        let nominal = self.nominal(knots, self.done);
        let target = self.prev + self.hop as i64;
        let a = if nominal == target {
            target
        } else {
            self.search(source, nominal, target)
        };
        self.write(source, a);
    }

    /// The candidate near `nominal` whose start is most like the source at `target`.
    fn search(&mut self, [l, r]: [&[f32]; 2], nominal: i64, target: i64) -> i64 {
        let hop = self.hop;
        for (i, t) in self.target[..hop].iter_mut().enumerate() {
            let j = target + i as i64;
            *t = at(l, j) + at(r, j);
        }
        let tgt = &self.target[..hop];
        let score = |c: i64, stride: usize| {
            let (mut dot, mut ea, mut eb) = (0.0f64, 0.0f64, 0.0f64);
            let mut i = 0;
            while i < hop {
                let j = c + i as i64;
                let x = at(l, j) + at(r, j);
                dot += x * tgt[i];
                ea += x * x;
                eb += tgt[i] * tgt[i];
                i += stride;
            }
            (dot / (ea * eb).sqrt().max(1.0e-30), eb)
        };
        let (s0, energy) = score(nominal, COARSE_STRIDE);
        if energy < QUIET {
            return nominal;
        }
        let reach = (hop as i64 / 2) / STEP * STEP;
        let mut best = (s0, nominal);
        let mut c = nominal - reach;
        while c <= nominal + reach {
            let s = score(c, COARSE_STRIDE).0;
            if s > best.0 {
                best = (s, c);
            }
            c += STEP;
        }
        let centre = best.1;
        let mut fine = (score(centre, FINE_STRIDE).0, centre);
        for c in centre - (STEP - 1)..=centre + (STEP - 1) {
            let s = score(c, FINE_STRIDE).0;
            if s > fine.0 {
                fine = (s, c);
            }
        }
        fine.1
    }

    /// Adds the frame starting at source frame `a` at stretched index `done`: its first half
    /// completes `done .. done + hop` (into the ring), its second half waits in the tail.
    fn write(&mut self, source: [&[f32]; 2], a: i64) {
        let hop = self.hop;
        let (w1, w2) = self.window[..self.len].split_at(hop);
        for ((ch, tail), ring) in source.iter().zip(&mut self.tail).zip(&mut self.ring) {
            for i in 0..hop {
                let v = tail[i] + w1[i] * at(ch, a + i as i64);
                ring[((self.done + i as i64) as usize) & self.mask] = v as f32;
                tail[i] = w2[i] * at(ch, a + (hop + i) as i64);
            }
        }
        self.prev = a;
        self.done += hop as i64;
    }
}
