//! The reference voice: a finished track the mix is compared against, played after the master
//! chain.
//!
//! The decoder fills a [`PreviewFeed`](crate::PreviewFeed); the matching stream travels to the
//! audio thread in a control message ([`EngineHandle::reference_load`](crate::EngineHandle)) and
//! is read, resampled and level-shaped by this voice. Everything happens on the interface
//! outputs 1 and 2 at the end of a batch, after the routing and the master meters, before the
//! click and the browser preview. Live only: an offline render, a bounce or an export never
//! receives a reference.
//!
//! # Listening
//!
//! [`ReferenceSettings::listening`] crossfades between the mix and the reference over 30 ms
//! (linear, the two gains sum to 1). The reference is multiplied by [`ReferenceSettings::gain`]
//! (linear, smoothed with a 150 ms time constant so a level-match update never steps). With
//! nothing sounding and no band or side solo the voice leaves the outputs untouched, bit for
//! bit. A reference stream that ended or is no longer listened to is dropped after the
//! crossfade and handed back through the retire queue.
//!
//! # Band and side solo
//!
//! [`ReferenceBand`] and [`ReferenceSolo`] are applied to the mix and to the reference by two
//! identical paths, so the comparison stays like for like. A band is a Linkwitz-Riley (24 dB
//! per octave) high-pass and/or low-pass: sub below 80 Hz, low 80 Hz to 300 Hz, mid 300 Hz to
//! 4 kHz, high above 4 kHz. Mid solo plays `(L + R) / 2` on both sides and side solo plays
//! `(L - R) / 2` on both sides. A change of either is made with a 5 ms duck of the whole output
//! down, the swap at silence and 5 ms back up, so it never clicks.
//!
//! # Measuring the mix
//!
//! While [`ReferenceSettings::measure`] is on, the voice measures the mix as it arrives (before
//! anything of the reference) with the K-weighting of ITU-R BS.1770 over 3 s windows in steps of
//! 100 ms and publishes the short-term loudness through
//! [`ReferenceShared::short_term_lufs`]. The app compares it with the reference's loudest
//! section to set the level match.

use crate::preview::{PreviewStream, Slot, Step};
use atomik_core::Batch;
use atomik_dsp::k_weighting::k_weighting;
use atomik_dsp::{Biquad, BiquadCoeffs, BiquadShape};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The mix/reference crossfade in seconds.
const CROSSFADE_SECONDS: f64 = 0.030;
/// The duck around a band or side change, one way, in seconds.
const DUCK_SECONDS: f64 = 0.005;
/// Time constant of the reference gain smoothing in seconds.
const GAIN_SECONDS: f64 = 0.150;
/// Sub-blocks of the short-term window (100 ms each).
const SHORT_SUBS: usize = 30;
/// The loudness published for silence.
pub const REFERENCE_FLOOR_LUFS: f32 = -100.0;
/// Butterworth Q: two such sections in series make a Linkwitz-Riley section.
const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The band both signals are limited to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferenceBand {
    /// The whole spectrum.
    #[default]
    Full,
    /// Below 80 Hz.
    Sub,
    /// 80 Hz to 300 Hz.
    Low,
    /// 300 Hz to 4 kHz.
    Mid,
    /// Above 4 kHz.
    High,
}

impl ReferenceBand {
    /// Every band, in the order of a list.
    pub const ALL: [Self; 5] = [Self::Full, Self::Sub, Self::Low, Self::Mid, Self::High];

    /// The filter stages of the band: a shape and its corner frequency in Hz.
    fn stages(self) -> ([(BiquadShape, f64); 2], usize) {
        let none = (BiquadShape::Lowpass, 0.0);
        match self {
            Self::Full => ([none, none], 0),
            Self::Sub => ([(BiquadShape::Lowpass, 80.0), none], 1),
            Self::Low => (
                [(BiquadShape::Highpass, 80.0), (BiquadShape::Lowpass, 300.0)],
                2,
            ),
            Self::Mid => (
                [
                    (BiquadShape::Highpass, 300.0),
                    (BiquadShape::Lowpass, 4_000.0),
                ],
                2,
            ),
            Self::High => ([(BiquadShape::Highpass, 4_000.0), none], 1),
        }
    }
}

/// Which part of the stereo image both signals are limited to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferenceSolo {
    /// Left and right as they are.
    #[default]
    Off,
    /// `(L + R) / 2` on both sides.
    Mid,
    /// `(L - R) / 2` on both sides.
    Side,
}

/// What the voice is told to do (copied into the engine by one control message).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceSettings {
    /// True when the reference is what the user hears.
    pub listening: bool,
    /// Linear gain of the reference (the level match).
    pub gain: f32,
    /// The band solo.
    pub band: ReferenceBand,
    /// The stereo solo.
    pub solo: ReferenceSolo,
    /// Whether the mix is measured (the short-term loudness is published).
    pub measure: bool,
}

impl Default for ReferenceSettings {
    fn default() -> Self {
        Self {
            listening: false,
            gain: 1.0,
            band: ReferenceBand::Full,
            solo: ReferenceSolo::Off,
            measure: false,
        }
    }
}

/// Numbers the voice publishes, shared with the handle.
#[derive(Debug, Default)]
pub struct ReferenceShared {
    /// Short-term loudness of the mix in LUFS as `f32` bits (0 until first published).
    short_term: AtomicU32,
    /// True once `short_term` holds a reading.
    measured: AtomicBool,
    /// True while a reference stream is loaded.
    loaded: AtomicBool,
}

impl ReferenceShared {
    /// The mix's short-term loudness in LUFS ([`REFERENCE_FLOOR_LUFS`] for silence), or `None`
    /// while it is not measured (nothing published since the measurement was switched on).
    pub fn short_term_lufs(&self) -> Option<f32> {
        self.measured
            .load(Ordering::Acquire)
            .then(|| f32::from_bits(self.short_term.load(Ordering::Relaxed)))
    }

    /// True while the voice holds a reference stream.
    pub fn loaded(&self) -> bool {
        self.loaded.load(Ordering::Relaxed)
    }
}

/// The two paths' shared filter settings.
#[derive(Clone, Copy)]
struct Shape {
    band: ReferenceBand,
    solo: ReferenceSolo,
    coeffs: [BiquadCoeffs; 2],
    stages: usize,
}

impl Shape {
    fn new(band: ReferenceBand, solo: ReferenceSolo, rate: f64) -> Self {
        let (defs, stages) = band.stages();
        let mut coeffs = [BiquadCoeffs::IDENTITY; 2];
        for (c, (shape, hz)) in coeffs.iter_mut().zip(defs).take(stages) {
            *c = BiquadCoeffs::design(shape, hz, BUTTERWORTH_Q, 0.0, rate);
        }
        Self {
            band,
            solo,
            coeffs,
            stages,
        }
    }

    fn is_neutral(&self) -> bool {
        self.band == ReferenceBand::Full && self.solo == ReferenceSolo::Off
    }
}

/// One signal's filters: `[channel][section]`, two sections per stage.
#[derive(Clone, Copy, Default)]
struct Path {
    bq: [[Biquad; 4]; 2],
}

impl Path {
    fn run(&mut self, shape: &Shape, l: f32, r: f32) -> (f32, f32) {
        let mut out = [f64::from(l), f64::from(r)];
        for (ch, x) in out.iter_mut().enumerate() {
            for stage in 0..shape.stages {
                for section in 0..2 {
                    *x = self.bq[ch][stage * 2 + section].process(&shape.coeffs[stage], *x);
                }
            }
        }
        let (l, r) = (out[0] as f32, out[1] as f32);
        match shape.solo {
            ReferenceSolo::Off => (l, r),
            ReferenceSolo::Mid => {
                let m = 0.5 * (l + r);
                (m, m)
            }
            ReferenceSolo::Side => {
                let s = 0.5 * (l - r);
                (s, s)
            }
        }
    }
}

/// The K-weighted short-term meter of the mix.
struct Meter {
    shelf: BiquadCoeffs,
    rlb: BiquadCoeffs,
    filt: [[Biquad; 2]; 2],
    sub_len: usize,
    sub_n: usize,
    acc: f64,
    ring: [f64; SHORT_SUBS],
    pos: usize,
    filled: usize,
}

impl Meter {
    fn new(rate: f64) -> Self {
        let (shelf, rlb) = k_weighting(rate);
        Self {
            shelf,
            rlb,
            filt: [[Biquad::default(); 2]; 2],
            sub_len: ((rate * 0.1).round() as usize).max(1),
            sub_n: 0,
            acc: 0.0,
            ring: [0.0; SHORT_SUBS],
            pos: 0,
            filled: 0,
        }
    }

    fn reset(&mut self) {
        self.filt = [[Biquad::default(); 2]; 2];
        self.sub_n = 0;
        self.acc = 0.0;
        self.ring = [0.0; SHORT_SUBS];
        self.pos = 0;
        self.filled = 0;
    }

    /// Feeds one frame; returns the short-term loudness when a sub-block just completed.
    fn push(&mut self, l: f32, r: f32) -> Option<f32> {
        let mut z = 0.0;
        for (ch, x) in [l, r].into_iter().enumerate() {
            let y = self.filt[ch][0].process(&self.shelf, f64::from(x));
            let y = self.filt[ch][1].process(&self.rlb, y);
            z += y * y;
        }
        self.acc += z;
        self.sub_n += 1;
        if self.sub_n < self.sub_len {
            return None;
        }
        self.ring[self.pos] = self.acc / self.sub_n as f64;
        self.pos = (self.pos + 1) % SHORT_SUBS;
        self.filled = (self.filled + 1).min(SHORT_SUBS);
        self.sub_n = 0;
        self.acc = 0.0;
        // Fewer than a whole window: the missing sub-blocks count as silence.
        let mean = self.ring.iter().sum::<f64>() / SHORT_SUBS as f64;
        Some(if mean > 1e-12 {
            (-0.691 + 10.0 * mean.log10()).max(f64::from(REFERENCE_FLOOR_LUFS)) as f32
        } else {
            REFERENCE_FLOOR_LUFS
        })
    }
}

/// The voice on the audio thread (owned by the engine).
pub(crate) struct ReferenceVoice {
    rate: f64,
    fade: u32,
    crossfade_step: f32,
    duck_step: f32,
    gain_coef: f32,
    settings: ReferenceSettings,
    slot: Option<Slot>,
    /// Ended streams waiting to be retired (at most one is cut per message, one ends per batch).
    dead: [Option<PreviewStream>; 2],
    /// Crossfade position: 0 is the mix, 1 the reference.
    x: f32,
    /// The smoothed reference gain.
    gain: f32,
    /// The duck around a shape change: 1 is open.
    duck: f32,
    applied: Shape,
    mix: Path,
    reference: Path,
    meter: Meter,
    shared: Arc<ReferenceShared>,
}

impl ReferenceVoice {
    pub(crate) fn new(rate: f64, shared: Arc<ReferenceShared>) -> Self {
        let step = |seconds: f64| (1.0 / (seconds * rate)).min(1.0) as f32;
        Self {
            rate,
            fade: ((0.005 * rate).round() as u32).max(1),
            crossfade_step: step(CROSSFADE_SECONDS),
            duck_step: step(DUCK_SECONDS),
            gain_coef: (1.0 - (-1.0 / (GAIN_SECONDS * rate)).exp()) as f32,
            settings: ReferenceSettings::default(),
            slot: None,
            dead: [None, None],
            x: 0.0,
            gain: 1.0,
            duck: 1.0,
            applied: Shape::new(ReferenceBand::Full, ReferenceSolo::Off, rate),
            mix: Path::default(),
            reference: Path::default(),
            meter: Meter::new(rate),
            shared,
        }
    }

    /// Loads `stream` (the reference file read from the position the user chose). A stream that
    /// is loaded already is cut; it is returned so the caller retires it.
    pub(crate) fn load(&mut self, stream: PreviewStream) -> Option<PreviewStream> {
        let cut = self.slot.take().map(Slot::into_stream);
        self.slot = Some(Slot::steady(stream, self.rate));
        self.reference = Path::default();
        self.shared.loaded.store(true, Ordering::Relaxed);
        cut
    }

    /// Drops the loaded stream, if any (the output returns to the mix at once; the caller
    /// sets `listening` off first for a crossfade).
    pub(crate) fn clear(&mut self) -> Option<PreviewStream> {
        self.shared.loaded.store(false, Ordering::Relaxed);
        self.slot.take().map(Slot::into_stream)
    }

    pub(crate) fn set(&mut self, settings: ReferenceSettings) {
        let gain = if settings.gain.is_finite() {
            settings.gain.clamp(0.0, 16.0)
        } else {
            1.0
        };
        // A fresh start (not listening before) takes its level at once: nothing to glide from.
        if settings.listening && !self.settings.listening && self.x == 0.0 {
            self.gain = gain;
        }
        if settings.measure != self.settings.measure {
            self.meter.reset();
            self.shared.measured.store(false, Ordering::Release);
        }
        self.settings = ReferenceSettings { gain, ..settings };
    }

    /// Hands out one ended stream, if any.
    pub(crate) fn take_dead(&mut self) -> Option<PreviewStream> {
        self.dead.iter_mut().find_map(Option::take)
    }

    fn park(&mut self, stream: PreviewStream) {
        if let Some(free) = self.dead.iter_mut().find(|d| d.is_none()) {
            *free = Some(stream);
        }
    }

    fn wanted(&self) -> (ReferenceBand, ReferenceSolo) {
        (self.settings.band, self.settings.solo)
    }

    /// True when the outputs need no work this batch.
    fn idle(&self) -> bool {
        !self.settings.measure
            && !self.settings.listening
            && self.slot.is_none()
            && self.x == 0.0
            && self.applied.is_neutral()
            && self.wanted() == (self.applied.band, self.applied.solo)
    }

    /// Processes one batch of interface outputs 1 and 2.
    pub(crate) fn process(&mut self, out: &mut [Batch]) {
        if out.len() < 2 {
            return;
        }
        if self.idle() {
            return;
        }
        let target_x = if self.settings.listening { 1.0 } else { 0.0 };
        let mut ended = false;
        let (first, rest) = out.split_at_mut(1);
        let (left, right) = (&mut first[0], &mut rest[0]);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (ml, mr) = (*l, *r);
            if self.settings.measure
                && let Some(lufs) = self.meter.push(ml, mr)
            {
                self.shared
                    .short_term
                    .store(lufs.to_bits(), Ordering::Relaxed);
                self.shared.measured.store(true, Ordering::Release);
            }
            let mut rf = [0.0f32; 2];
            if let Some(slot) = &mut self.slot {
                match slot.step(self.fade) {
                    Step::Sample(f) => rf = f,
                    Step::Silence | Step::Pending => {}
                    Step::End => ended = true,
                }
            }
            // The crossfade and the gain glide.
            if self.x < target_x {
                self.x = (self.x + self.crossfade_step).min(target_x);
            } else if self.x > target_x {
                self.x = (self.x - self.crossfade_step).max(target_x);
            }
            self.gain += (self.settings.gain - self.gain) * self.gain_coef;
            // A shape change ducks the output, swaps the filters at silence and opens again.
            if self.wanted() != (self.applied.band, self.applied.solo) {
                self.duck -= self.duck_step;
                if self.duck <= 0.0 {
                    self.duck = 0.0;
                    let (band, solo) = self.wanted();
                    self.applied = Shape::new(band, solo, self.rate);
                    self.mix = Path::default();
                    self.reference = Path::default();
                }
            } else if self.duck < 1.0 {
                self.duck = (self.duck + self.duck_step).min(1.0);
            }
            if self.x == 0.0 && target_x == 0.0 && self.applied.is_neutral() && self.duck >= 1.0 {
                continue;
            }
            let (pl, pr) = if self.applied.is_neutral() {
                (ml, mr)
            } else {
                self.mix.run(&self.applied, ml, mr)
            };
            let (ql, qr) = if self.applied.is_neutral() {
                (rf[0], rf[1])
            } else {
                self.reference.run(&self.applied, rf[0], rf[1])
            };
            let (a, b) = (1.0 - self.x, self.x * self.gain);
            *l = (pl * a + ql * b) * self.duck;
            *r = (pr * a + qr * b) * self.duck;
        }
        if let Some(slot) = &self.slot {
            slot.publish_position();
        }
        // The stream is let go of when it ended, or once the reference is faded out.
        let done = ended || (!self.settings.listening && self.x == 0.0);
        if done && let Some(slot) = self.slot.take() {
            self.shared.loaded.store(false, Ordering::Relaxed);
            let stream = slot.into_stream();
            self.park(stream);
        }
    }
}

#[cfg(test)]
mod tests;
