//! The sound analyst's patch: from the measurements of a sound ([`SoundProfile`]) to a starting
//! patch on a built-in synth, and a way to hear how close it is.
//!
//! [`propose`] picks the synth by rules whose numbers are the constants of this module, then sets
//! its envelopes, filter movement and pitch envelope from the measurements:
//!
//! | The sound is | The patch is |
//! |---|---|
//! | pitched, a pure tone, all harmonics or odd harmonics only | the poly synth ([`POLY`]) on one oscillator: a sine, a saw, a square, or a triangle when the third harmonic is under [`TRIANGLE_BELOW_DB`] |
//! | inharmonic (a bell, metal) | the FM synth ([`FM`]) on the "twin stack" routing: operator 1 at the fundamental, operator 2 at the ratio of the strongest partial that does not line up |
//! | noise, or unpitched | the poly synth as a noise voice: its noise source at full level, the oscillators a sine four octaves under the note so low that the filter removes it, a high-pass filter from [`NOISE_HIGHPASS_FROM_HZ`] of centroid and a band-pass below |
//!
//! * The **amplitude envelope** sets the amp attack, decay, sustain and release of the synth.
//!   The measured times are converted with the exact shape of the synth's envelope
//!   ([`atomik_dsp::Adsr`]: one-pole segments that reach their target in the stated time): the
//!   attack from the rise between a tenth and nine tenths of the peak (the measuring window's own
//!   blur taken off), the decay from the time to come within a tenth of the way to the sustain,
//!   the release from the time to fall to a tenth of the sustain. A sound without a sustain
//!   gets a sustain of 0 and is held to its end, so it decays on its own as measured.
//! * The **filter movement** is a filter envelope: the cutoff at the end of the held note is
//!   [`CUTOFF_PER_CENTROID`] times the measured end centroid and the envelope amount opens it to
//!   the start centroid (six octaves is an amount of 1), with a decay that takes the measured
//!   fall time. A brightness that moves less than [`atomik_dsp::sound_analysis::STATIC_OCTAVES`]
//!   gets no filter envelope.
//! * The **pitch envelope** is the pitch offset at the start (in the poly synth the mod envelope
//!   on the pitch, 24 semitones at an amount of 1; in the FM synth `pitch.env_amount`), decaying
//!   in the time that takes the offset down to the measured settling threshold.
//! * The **noise share** becomes the poly synth's noise level (the noise adds to the oscillator
//!   in power).
//!
//! [`fit`] then renders the proposal on the note it will play (the key of the fundamental, held
//! for the measured note length, 120 BPM, 48 kHz, one voice), measures that with the same
//! analysis and, when the brightness is off, moves the cutoff (and the opening of the filter
//! envelope) by the octaves it is off, divided by the slope the renders so far show, and tries
//! again (up to [`REFINE_PASSES`] times; the best of the tries is kept). The result is the
//! proposal, the profile of its render and the [`Distances`] to the target: the match meter.
//!
//! The render is the patch render of the offline renderer, deterministic like a bounce.

use crate::{RenderError, render_json};
use atomik_device_sdk::Registry;
use atomik_dsp::exp2_det;
use atomik_dsp::sound_analysis::{
    Distances, GAP_FRACTION, HarmonicKind, SoundProfile, WINDOW_S, analyse, compare,
};
use serde_json::{Map, json};
use std::num::NonZeroUsize;

/// The poly synth.
pub const POLY: &str = "atomik.poly-synth";
/// The FM synth.
pub const FM: &str = "atomik.fm-synth";
/// An odd-harmonic sound whose third harmonic is weaker than this against the first, in dB, is a
/// triangle (a square's is -9.5 dB, a triangle's -19 dB).
pub const TRIANGLE_BELOW_DB: f32 = -14.0;
/// A noise sound whose centroid is at or above this, in Hz, is made with a high-pass filter
/// (below it, a band-pass).
pub const NOISE_HIGHPASS_FROM_HZ: f32 = 2_500.0;
/// The cutoff of the low-pass filter as a multiple of the centroid it should produce.
pub const CUTOFF_PER_CENTROID: f32 = 1.0;
/// The share of a high-pass noise filter's cutoff of its centroid.
pub const NOISE_HIGHPASS_PER_CENTROID: f32 = 0.6;
/// How many times a proposal is corrected after its first render.
pub const REFINE_PASSES: usize = 3;
/// A correction is made while the brightness is further than this off, in octaves.
pub const REFINE_WITHIN: f32 = 0.05;
/// The octaves the centroid moves per octave of cutoff before a render has shown it.
const FIRST_SLOPE: f32 = 0.75;
/// The shortest attack, decay, release the proposal sets, in ms.
pub const MIN_MS: f32 = 1.0;
/// The longest, in ms.
pub const MAX_MS: f32 = 10_000.0;
/// The release of a sound that has none, in ms.
pub const DEFAULT_RELEASE_MS: f32 = 120.0;
/// The lowest cutoff the proposal sets, in Hz (a noise voice keeps its oscillator out of it).
pub const MIN_CUTOFF_HZ: f32 = 120.0;
/// A low-pass cutoff stays this many times above the fundamental of a pitched sound (under it the
/// filter would take the note itself away).
pub const FUNDAMENTAL_CLEARANCE: f32 = 1.5;
/// The lowest cutoff of a noise voice, in Hz.
pub const MIN_NOISE_CUTOFF_HZ: f32 = 700.0;
/// The octaves of cutoff an envelope amount of 1 moves (the poly synth's filter envelope).
pub const OCTAVES_PER_AMOUNT: f32 = 6.0;
/// The semitones of pitch a mod envelope amount of 1 moves.
pub const SEMITONES_PER_AMOUNT: f32 = 24.0;
/// The key a sound without a pitch plays.
pub const UNPITCHED_KEY: u8 = 60;
/// The highest ratio of a modulator of the FM proposal.
const MAX_RATIO: f32 = 31.9;
/// The rate of the render, in Hz.
pub const RENDER_RATE: u32 = 48_000;
/// Seconds the render runs past the end of the sound.
const TAIL_S: f32 = 0.3;
/// The tempo of the render: two beats a second.
const BEATS_PER_SECOND: f64 = 2.0;
/// The rise of the synth's attack from a tenth to nine tenths of its time.
const ATTACK_RISE: f32 = 0.748;
/// The one-pole undershoot of the synth's decay and release segments.
const UNDERSHOOT: f32 = 0.001;
/// The seed of the render's random sources.
const SEED: u64 = 1;

/// Which synth a proposal is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// The poly synth with one oscillator.
    Poly,
    /// The FM synth.
    Fm,
    /// The poly synth as a noise voice.
    Noise,
}

impl Family {
    /// The device type.
    pub fn type_id(self) -> &'static str {
        match self {
            Self::Fm => FM,
            Self::Poly | Self::Noise => POLY,
        }
    }
}

/// A starting patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// The family of the synth.
    pub family: Family,
    /// The param keys and plain values that differ from a new device, in the order they are set.
    pub params: Vec<(String, f32)>,
    /// The key of the note the patch plays (the nearest key of the fundamental).
    pub key: u8,
    /// How long that note is held, in seconds.
    pub hold_s: f32,
    /// How long the sound lasts, in seconds.
    pub length_s: f32,
    /// The lowest cutoff a correction may set, in Hz.
    pub cutoff_floor: f32,
    /// A line on each choice that was made, for the panel.
    pub reasons: Vec<String>,
}

impl Proposal {
    /// The device type of the synth.
    pub fn type_id(&self) -> &'static str {
        self.family.type_id()
    }

    /// The value set for `key`.
    pub fn param(&self, key: &str) -> Option<f32> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    fn set(&mut self, key: &str, value: f32) {
        match self.params.iter_mut().find(|(k, _)| k == key) {
            Some(p) => p.1 = value,
            None => self.params.push((key.to_owned(), value)),
        }
    }

    /// Moves the cutoff of the filter by `octaves` (a poly or noise proposal; nothing for FM).
    pub fn shift_cutoff(&mut self, octaves: f32) {
        if let Some(c) = self.param("filter.cutoff") {
            let floor = self.cutoff_floor;
            self.set(
                "filter.cutoff",
                (c * exp2_det(octaves)).clamp(floor, 18_000.0),
            );
        }
    }
}

/// A proposal, the profile of its render and how close that is to the target.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// The patch.
    pub proposal: Proposal,
    /// What the patch sounds like on its note.
    pub rendered: SoundProfile,
    /// The distances of the three curves.
    pub distances: Distances,
    /// How many corrections were made.
    pub passes: usize,
}

/// Why a proposal could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    /// The renderer failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The render holds no sound that can be analysed.
    #[error("the patch renders no sound that can be measured: {0}")]
    Unmeasurable(atomik_dsp::sound_analysis::AnalysisError),
    /// A param key of the proposal is not one of the device (a bug of this module).
    #[error("the proposal sets a param the device does not have: {0}")]
    Param(String),
}

/// The time of a decay of the synth's envelope, from 1 to `sustain`, in which the level comes
/// within `GAP_FRACTION` of the way to `sustain`, as a share of the decay time.
fn decay_share(sustain: f32) -> f32 {
    let gap = 1.0 - sustain;
    let target = (GAP_FRACTION * gap + UNDERSHOOT) / (gap + UNDERSHOOT);
    target.ln() / (UNDERSHOOT / (gap + UNDERSHOOT)).ln()
}

/// The share of a release time in which the level falls from `sustain` to a tenth of it.
fn release_share(sustain: f32) -> f32 {
    let target = (0.1 * sustain + UNDERSHOOT) / (sustain + UNDERSHOOT);
    target.ln() / (UNDERSHOOT / (1.0 + UNDERSHOOT)).ln()
}

/// The attack time in ms for a measured rise of `rise_s` (the blur of the window taken off).
fn attack_ms(rise_s: f32) -> f32 {
    let blur = 0.8 * WINDOW_S;
    if rise_s < 1.15 * blur {
        return MIN_MS;
    }
    let rise = (rise_s * rise_s - blur * blur).sqrt();
    ms(rise / ATTACK_RISE * 1000.0)
}

/// The amplitude envelope as a set of param values: `(attack, decay, sustain, release)` in ms
/// and level.
fn amp_envelope(p: &SoundProfile) -> (f32, f32, f32, f32) {
    let e = &p.envelope;
    let attack = attack_ms(e.attack_s);
    if e.sustain <= 0.0 {
        return (attack, fall_ms(e.decay_s), 0.0, DEFAULT_RELEASE_MS);
    }
    let sustain = if e.sustain >= 0.9 { 1.0 } else { e.sustain };
    let decay = if sustain >= 1.0 || e.decay_s <= 0.0 {
        50.0
    } else {
        ms(e.decay_s / decay_share(sustain) * 1000.0)
    };
    let release = ms(e.release_s / release_share(sustain) * 1000.0);
    (attack, decay, sustain, release)
}

/// The decay in ms of a segment that falls to 0 and comes within a tenth of the way down in
/// `seconds`.
fn fall_ms(seconds: f32) -> f32 {
    ms(seconds / decay_share(0.0) * 1000.0)
}

/// A time in ms, within the range of the synths' envelope params.
fn ms(v: f32) -> f32 {
    v.clamp(MIN_MS, MAX_MS)
}

/// The decay in ms in which a pitch offset of `semitones` falls to the settled threshold in
/// `settle_s` (an exponential fall).
fn pitch_decay_ms(semitones: f32, settle_s: f32) -> f32 {
    let steps = (semitones.abs() / atomik_dsp::sound_analysis::SETTLED_SEMITONES)
        .ln()
        .max(0.3);
    // A time constant: the offset is `start * exp(-t / tau)`.
    ms(settle_s / steps * 1000.0)
}

/// The oscillator wave (the `osc1.wave` choice), its name and its RMS against a saw's (0.577)
/// for a profile of harmonic sound.
fn waveform_of(profile: &SoundProfile) -> (f32, &'static str, f32) {
    let h = &profile.harmonics;
    match h.kind {
        HarmonicKind::Pure => (3.0, "a sine", 0.707),
        HarmonicKind::OddOnly if h.third_db.is_some_and(|d| d < TRIANGLE_BELOW_DB) => {
            (2.0, "a triangle", 0.577)
        }
        HarmonicKind::OddOnly => (1.0, "a square", 1.0),
        _ => (0.0, "a saw", 0.577),
    }
}

/// The family for a profile (the table of the module docs).
pub fn family_of(profile: &SoundProfile) -> Family {
    match profile.harmonics.kind {
        HarmonicKind::Noise => Family::Noise,
        HarmonicKind::Inharmonic => Family::Fm,
        _ if !profile.pitched() => Family::Noise,
        _ => Family::Poly,
    }
}

fn key_of(profile: &SoundProfile) -> (u8, f32) {
    match profile.pitch {
        Some(p) => {
            let key = p.key.round().clamp(12.0, 108.0);
            (key as u8, ((p.key - key) * 100.0).clamp(-50.0, 50.0))
        }
        None => match profile.harmonics.reference_hz {
            Some(hz) if profile.harmonics.kind == HarmonicKind::Inharmonic => {
                let key = (69.0 + 12.0 * (hz / 440.0).log2())
                    .round()
                    .clamp(12.0, 108.0);
                (key as u8, 0.0)
            }
            _ => (UNPITCHED_KEY, 0.0),
        },
    }
}

/// The starting patch for `profile` (module docs).
pub fn propose(profile: &SoundProfile) -> Proposal {
    let family = family_of(profile);
    let (key, fine) = key_of(profile);
    let e = &profile.envelope;
    let length_s = profile.duration_s;
    let hold_s = if e.sustain > 0.0 {
        e.hold_s.max(0.05)
    } else {
        length_s
    };
    let mut p = Proposal {
        family,
        params: Vec::new(),
        key,
        hold_s,
        length_s,
        cutoff_floor: match (family, profile.pitch) {
            (Family::Noise, _) => MIN_NOISE_CUTOFF_HZ,
            (_, Some(pitch)) => (pitch.fundamental_hz * FUNDAMENTAL_CLEARANCE).max(MIN_CUTOFF_HZ),
            _ => MIN_CUTOFF_HZ,
        },
        reasons: Vec::new(),
    };
    match family {
        Family::Poly | Family::Noise => poly_patch(&mut p, profile, fine),
        Family::Fm => fm_patch(&mut p, profile),
    }
    p
}

/// The envelope reasons shared by every family.
fn envelope_reason(p: &SoundProfile) -> String {
    let e = &p.envelope;
    if e.sustain > 0.0 {
        format!(
            "Envelope: attack {:.0} ms, decay {:.0} ms to a sustain of {:.0} %, release {:.0} ms",
            e.attack_s * 1000.0,
            e.decay_s * 1000.0,
            e.sustain * 100.0,
            e.release_s * 1000.0
        )
    } else {
        format!(
            "Envelope: attack {:.0} ms, then it dies away in {:.0} ms",
            e.attack_s * 1000.0,
            e.decay_s * 1000.0
        )
    }
}

fn poly_patch(p: &mut Proposal, profile: &SoundProfile, fine: f32) {
    let noise_voice = p.family == Family::Noise;
    let (a, d, s, r) = amp_envelope(profile);
    p.set("voices", 2.0);
    p.set("velocity.amp", 0.0);
    p.set("osc.mix", 0.0);
    p.set("filter.type", 0.0);
    p.set("filter.key_track", 0.0);
    p.set("filter.resonance", 0.1);
    p.set("env.amp.attack", a);
    p.set("env.amp.decay", d);
    p.set("env.amp.sustain", s);
    p.set("env.amp.release", r);
    let osc_rms = if noise_voice {
        // The oscillator is a sine four octaves under the note: far under the filter.
        p.set("osc1.wave", 3.0);
        p.set("osc1.octave", -3.0);
        p.set("osc1.semi", -12.0);
        p.set("noise.level", 1.0);
        p.reasons.push(format!(
            "Noise: {:.0} % of the sound is noise, so the poly synth plays its noise source",
            profile.noise_share * 100.0
        ));
        0.577
    } else {
        let (wave, name, rms) = waveform_of(profile);
        p.set("osc1.wave", wave);
        p.set("osc1.fine", fine);
        p.reasons.push(format!(
            "Source: {} on the poly synth ({})",
            name,
            profile.harmonics.kind.text()
        ));
        rms
    };
    if !noise_voice && profile.noise_share > 0.02 {
        let share = profile.noise_share.clamp(0.0, 0.95);
        let level = ((share / (1.0 - share)).sqrt() * osc_rms / 0.577).clamp(0.0, 1.0);
        p.set("noise.level", level);
        p.reasons
            .push(format!("Noise: {:.0} % added", profile.noise_share * 100.0));
    }
    // The filter.
    let b = &profile.brightness;
    let octaves = (b.start_hz.max(1.0) / b.end_hz.max(1.0)).log2();
    let moving = octaves.abs() >= atomik_dsp::sound_analysis::STATIC_OCTAVES && b.fall_s > 0.0;
    let reference = if moving { b.end_hz } else { b.mean_hz };
    let floor = p.cutoff_floor;
    let cutoff = if noise_voice {
        if b.mean_hz >= NOISE_HIGHPASS_FROM_HZ {
            p.set("filter.mode", 2.0);
            reference * NOISE_HIGHPASS_PER_CENTROID
        } else {
            p.set("filter.mode", 1.0);
            reference
        }
    } else {
        reference * CUTOFF_PER_CENTROID
    };
    p.set("filter.cutoff", cutoff.clamp(floor, 18_000.0));
    if moving {
        let amount = (octaves / OCTAVES_PER_AMOUNT).clamp(-1.0, 1.0);
        p.set("filter.env_amount", amount);
        p.set("env.filter.attack", MIN_MS);
        p.set("env.filter.decay", fall_ms(b.fall_s));
        p.set("env.filter.sustain", 0.0);
        p.set("env.filter.release", r);
        p.reasons.push(format!(
            "Filter: {:.0} Hz at the start, {:.0} Hz at the end, in {:.0} ms",
            b.start_hz,
            b.end_hz,
            b.fall_s * 1000.0
        ));
    } else {
        p.set("filter.env_amount", 0.0);
        p.reasons
            .push(format!("Filter: steady around {:.0} Hz", b.mean_hz));
    }
    if let Some(pitch) = profile.pitch.filter(|m| m.start_semitones != 0.0) {
        p.set("env.mod.dest", 0.0);
        p.set(
            "env.mod.amount",
            (pitch.start_semitones / SEMITONES_PER_AMOUNT).clamp(-1.0, 1.0),
        );
        p.set("env.mod.attack", MIN_MS);
        // The mod envelope falls to zero like the amp decay with sustain 0: the time in which
        // the offset has fallen to the settled threshold, as a share of its decay.
        let tau = pitch_decay_ms(pitch.start_semitones, pitch.settle_s);
        p.set("env.mod.decay", ms(tau * 6.9));
        p.set("env.mod.sustain", 0.0);
        p.set("env.mod.release", r);
        p.reasons.push(format!(
            "Pitch: starts {:+.1} semitones from the note and settles in {:.0} ms",
            pitch.start_semitones,
            pitch.settle_s * 1000.0
        ));
    }
    p.reasons.push(envelope_reason(profile));
}

/// `(coarse, fine)` of an operator at `ratio` times the played pitch.
fn coarse_fine(ratio: f32) -> (f32, f32) {
    let ratio = ratio.clamp(0.5, MAX_RATIO);
    let coarse = ratio.floor();
    (coarse, ratio - coarse)
}

fn fm_patch(p: &mut Proposal, profile: &SoundProfile) {
    let (a, d, s, r) = amp_envelope(profile);
    p.set("algorithm", 1.0);
    // The strongest partial that does not sit on a multiple of the reference.
    let off = profile
        .harmonics
        .partials
        .iter()
        .find(|q| {
            q.ratio > 0.0
                && (q.ratio / q.ratio.round().max(1.0) - 1.0).abs()
                    > atomik_dsp::sound_analysis::HARMONIC_TOLERANCE as f32
        })
        .map(|q| q.ratio);
    let ratio = off.unwrap_or(std::f32::consts::SQRT_2);
    let (coarse, fine) = coarse_fine(ratio);
    let strong = profile
        .harmonics
        .partials
        .iter()
        .filter(|q| q.level_db > -30.0)
        .count() as f32;
    let index = (0.15 + 0.1 * strong).clamp(0.2, 0.8);
    p.set("op1.coarse", 1.0);
    p.set("op1.fine", 0.0);
    p.set("op1.level", 0.8);
    p.set("op1.attack", a);
    p.set("op1.decay", d);
    p.set("op1.sustain", s);
    p.set("op1.release", r);
    p.set("op2.coarse", coarse);
    p.set("op2.fine", fine);
    p.set("op2.level", index);
    // The modulator carries the brightness: it follows the amplitude, and falls to the
    // share of the brightness that is left at the end.
    let b = &profile.brightness;
    let octaves = (b.start_hz.max(1.0) / b.end_hz.max(1.0)).log2();
    p.set("op2.attack", a);
    if octaves.abs() >= atomik_dsp::sound_analysis::STATIC_OCTAVES && b.fall_s > 0.0 {
        let kept = exp2_det(-octaves.abs() * 0.5).clamp(0.0, 1.0);
        p.set("op2.decay", fall_ms(b.fall_s));
        p.set("op2.sustain", if s > 0.0 { kept } else { 0.0 });
        p.reasons.push(format!(
            "Brightness: the modulator falls in {:.0} ms",
            b.fall_s * 1000.0
        ));
    } else {
        p.set("op2.decay", d);
        p.set("op2.sustain", s);
    }
    p.set("op2.release", r);
    for op in ["op3", "op4"] {
        p.set(&format!("{op}.level"), 0.0);
    }
    if let Some(pitch) = profile.pitch.filter(|m| m.start_semitones != 0.0) {
        p.set("pitch.env_amount", pitch.start_semitones);
        p.set(
            "pitch.env_decay",
            pitch_decay_ms(pitch.start_semitones, pitch.settle_s),
        );
    }
    p.reasons.push(format!(
        "Source: the FM synth, a carrier at the pitch and a modulator at {ratio:.2} times it \
         (the strongest partial that is not a harmonic)"
    ));
    p.reasons.push(envelope_reason(profile));
}

/// The inline patch of `proposal` playing its note once (the patch format of the offline
/// renderer).
pub fn patch_json(proposal: &Proposal) -> String {
    let mut params = Map::new();
    for (k, v) in &proposal.params {
        params.insert(k.clone(), json!(f64::from(*v)));
    }
    let total = f64::from(proposal.length_s.max(proposal.hold_s) + TAIL_S);
    json!({
        "format": "atomik-patch", "version": 1,
        "sample_rate": RENDER_RATE, "tempo_bpm": 120, "seed": SEED,
        "length_beats": total * BEATS_PER_SECOND,
        "output_channels": 2,
        "devices": [ { "id": "s", "type": proposal.type_id(), "params": params } ],
        "cables": [
            { "from": "s:out_l", "to": "interface:out_1" },
            { "from": "s:out_r", "to": "interface:out_2" }
        ],
        "notes": [ {
            "device": "s", "socket": "notes_in", "at_beats": 0.0,
            "length_beats": f64::from(proposal.hold_s) * BEATS_PER_SECOND,
            "key": proposal.key, "velocity": 1.0
        } ]
    })
    .to_string()
}

/// Renders `proposal` on its note and measures the result.
///
/// # Errors
/// The render fails, a param key of the proposal is wrong, or the render is silent.
pub fn measure(proposal: &Proposal, registry: &Registry) -> Result<SoundProfile, SoundError> {
    let host = NonZeroUsize::new(512).unwrap_or(NonZeroUsize::MIN);
    let rendered = render_json(&patch_json(proposal), registry, host)?;
    if let Some(w) = rendered.warnings.first() {
        return Err(SoundError::Param(w.message.clone()));
    }
    let planes: Vec<&[f32]> = rendered.channels.iter().map(Vec::as_slice).collect();
    analyse(&planes, rendered.sample_rate).map_err(SoundError::Unmeasurable)
}

/// The proposal for `target`, rendered, compared and corrected (module docs).
///
/// # Errors
/// The render fails or holds no sound.
pub fn fit(target: &SoundProfile, registry: &Registry) -> Result<Fit, SoundError> {
    let mut proposal = propose(target);
    let mut rendered = measure(&proposal, registry)?;
    let mut best = Fit {
        distances: compare(target, &rendered),
        proposal: proposal.clone(),
        rendered: rendered.clone(),
        passes: 0,
    };
    // How many octaves the centroid moves per octave of cutoff; learned from the renders.
    let mut slope = FIRST_SLOPE;
    let mut last: Option<(f32, f32)> = None;
    for pass in 1..=REFINE_PASSES {
        let Some(cutoff) = proposal.param("filter.cutoff") else {
            break;
        };
        let moving = proposal
            .param("filter.env_amount")
            .is_some_and(|a| a != 0.0);
        let (want, have) = if moving {
            (target.brightness.end_hz, rendered.brightness.end_hz)
        } else {
            (target.brightness.mean_hz, rendered.brightness.mean_hz)
        };
        let error = (want.max(1.0) / have.max(1.0)).log2();
        if error.abs() < REFINE_WITHIN {
            break;
        }
        let at = cutoff.max(1.0).log2();
        if let Some((c, h)) = last
            && (at - c).abs() > 0.01
        {
            slope = ((have.max(1.0).log2() - h) / (at - c)).clamp(0.2, 1.5);
        }
        last = Some((at, have.max(1.0).log2()));
        proposal.shift_cutoff(error / slope);
        if moving {
            // What is left of the start error once the cutoff moved the whole sound.
            let start = (target.brightness.start_hz.max(1.0)
                / rendered.brightness.start_hz.max(1.0))
            .log2();
            let amount = proposal.param("filter.env_amount").unwrap_or(0.0)
                + (start - error) / (slope * OCTAVES_PER_AMOUNT);
            proposal.set("filter.env_amount", amount.clamp(-1.0, 1.0));
        }
        rendered = measure(&proposal, registry)?;
        let distances = compare(target, &rendered);
        if distances.overall() > best.distances.overall() {
            best = Fit {
                proposal: proposal.clone(),
                rendered: rendered.clone(),
                distances,
                passes: pass,
            };
        }
    }
    Ok(best)
}
