//! Atomik Rack DSP primitives: seeded RNG, smoothers, pan law, pitch conversion, PolyBLEP
//! oscillators, a ZDF state-variable filter, ADSR envelopes, LFOs and a fractional delay line.
//! It also holds the console DSP: RBJ-cookbook biquads with smoothed parameters
//! ([`SmoothedBiquad`]), 18 dB/oct HP/LP ([`PassFilter`]), a feed-forward [`Compressor`], a
//! [`Gate`]/expander, the console [`fader_gain`] taper and mid/side [`stereo_width`].
//! Sample playback maths: ([`Playhead`], [`read_hermite`], loop crossfade).
//! The stage blocks of the factory devices, each a small documented module:
//! [`dither`] (the requantiser of an audio export), [`shaper`] (drive curves), [`leveller`] (peak-linked upward leveller), [`crossover`]
//! (flat-sum 24 dB/oct split), [`phaser`], [`chorus`], [`echo`], [`comb`], [`room`] (small box
//! and large room on [`Fdn8`]), [`ringmod`], [`bitcrush`], [`noise`] (white, pink, rumble,
//! crackle, presence gate), [`spectral`] (STFT stage), [`spectrum`] (the analyser display: FFT power, 1/3-octave bands, peak hold), [`stereo_meter`] (correlation, balance and width), [`modal`], [`wavetable`] (the band-limited factory wavetables of the wavetable synth), [`wobble`] and [`smoother2`], and the offline [`stretch`] (time-stretch and pitch-shift), [`tuner`] (pitch, note and cents for the audio track tuner) and [`notes`] (audio to notes: a YIN pitch tracker for monophonic audio and a drum hit classifier).
//! They are all deterministic (see [`exp2_det`] and the module docs), allocation-free
//! in `process`, and offer a per-sample call plus a block call that is bit-identical for any
//! chunking.
//!
//! Every type is allocation-free after construction (the delay line allocates only in
//! [`DelayLine::new`] / [`DelayLine::resize`], which are non-real-time). All math is `f32`, with
//! `f64` phase accumulators. Designs are textbook public-domain DSP (see each module).

mod adsr;
mod allpass;
mod biquad;
pub mod bitcrush;
mod block_smoother;
pub mod chorus;
pub mod comb;
pub mod crossover;
mod delay;
mod det_math;
mod det_trig;
pub mod dither;
mod dynamics;
pub mod echo;
mod fader;
mod fdn;
pub mod leveller;
mod lfo;
mod mod_delay;
pub mod mod_source;
pub mod modal;
pub mod noise;
pub mod notes;
pub mod onset;
mod osc;
pub mod oversample;
mod pan;
mod parabolic_lfo;
mod pass_filter;
pub mod phaser;
mod pitch;
pub mod ringmod;
pub mod rng;
pub mod room;
mod sample_read;
pub mod shaper;
mod smoother;
pub mod smoother2;
pub mod spectral;
pub mod spectrum;
pub mod stereo_meter;
pub mod stretch;
mod svf;
pub mod tuner;
pub mod wavetable;
pub mod wobble;

pub use adsr::{Adsr, AdsrStage, STEAL_FADE_MS};
pub use allpass::Allpass;
pub use biquad::{
    Biquad, BiquadCoeffs, BiquadShape, COEFF_UPDATE_INTERVAL, EQ_SMOOTH_MS, SmoothedBiquad,
};
pub use bitcrush::Bitcrush;
pub use block_smoother::{BlockSmoother, Ramp};
pub use chorus::Chorus;
pub use comb::Comb;
pub use crossover::Crossover;
pub use delay::DelayLine;
pub use det_math::{exp2_det, exp2_det_f64, flush_denormal, hadamard8, log2_det};
pub use det_trig::sin_cycles;
pub use dither::{Dither, DitherMode};
pub use dynamics::{
    Compressor, Detector, EXPANDER_RATIO, GATE_DETECTOR_MS, Gate, KNEE_DB, RMS_WINDOW_MS,
    static_gain_reduction_db, static_gain_reduction_knee_db,
};
pub use echo::Echo;
pub use fader::{
    FADER_UNITY_POS, fader_db, fader_db_vca, fader_gain, fader_gain_vca, fader_pos_for_db, vca_gain,
};
pub use fdn::{FDN_LINES, Fdn8};
pub use leveller::Leveller;
pub use lfo::{Lfo, LfoShape};
pub use mod_delay::ModDelay;
pub use modal::{MAX_MODES, Modal};
pub use noise::{Crackle, PinkNoise, PresenceGate, Rumble, Wander, WhiteNoise};
pub use onset::detect_onsets;
pub use osc::{PolyBlepOsc, Waveform};
pub use pan::{pan_constant_power, stereo_width};
pub use parabolic_lfo::ParabolicLfo;
pub use pass_filter::{PASS_FILTER_SLOPE_DB_PER_OCT, PassFilter, PassKind};
pub use phaser::Phaser;
pub use pitch::note_to_hz;
pub use ringmod::RingMod;
pub use rng::Rng;
pub use room::{Room, RoomRange};
pub use sample_read::{
    LoopKind, MAX_RATIO, PlayRegion, Playhead, balance_gains, crossfade_gains, hermite4,
    pan_gains_mono, read_hermite, sin_quarter,
};
pub use shaper::{Shaper, ShaperKind};
pub use smoother::{OnePoleSmoother, Smoother};
pub use smoother2::{Smoother2, SmootherClass};
pub use spectral::{Spectral, SpectralAlgo, frame_for_rate};
pub use svf::{FilterMode, Svf, SvfOutputs};
pub use wobble::Wobble;

#[cfg(test)]
mod test_util;
