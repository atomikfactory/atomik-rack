//! The translation lens: what the master would sound like on a small speaker, on a club system and
//! in mono. A listening aid only: it sits after everything else on the live output and is never
//! part of a bounce or an export.
//!
//! A [`LensSettings`] is a [`Voicing`] and a mono switch. The voicings, all on `f64` biquads
//! ([`BiquadCoeffs::design`]):
//!
//! * [`Voicing::Headphones`]: nothing. With mono off too the lens is neutral: [`Lens::process`] is
//!   the identity, bit for bit.
//! * [`Voicing::SmallSpeaker`]: a phone, a laptop or a small bluetooth speaker. A second-order
//!   high-pass at [`SMALL_HIGHPASS_HZ`] (140 Hz, Butterworth: 12 dB per octave, -3 dB at the corner,
//!   about -7 dB at 100 Hz and -18 dB at 50 Hz) and a gentle presence curve, a bell of
//!   [`SMALL_PRESENCE_DB`] (+3 dB) at [`SMALL_PRESENCE_HZ`] (3 kHz, Q 0.7).
//! * [`Voicing::ClubSub`]: a club or festival system that is flat down to its sub and rolls off
//!   under it. A second-order Butterworth high-pass at [`CLUB_HIGHPASS_HZ`] (32 Hz).
//!
//! Mono sums the two channels to `(L + R) / 2` on both sides, before the filters (the filters are
//! linear, so the order does not change the result, and one chain is cheaper than two). Anything
//! that is the same in both channels keeps its level.
//!
//! [`response_db`] is the magnitude of a voicing in dB at a frequency, from the same coefficients
//! the filters run, so the bass-read meter ([`crate::bass_read`]) judges the sound that is played.
//! [`Lens`] is allocation-free after construction.

use crate::{Biquad, BiquadCoeffs, BiquadShape};

/// The corner of the small speaker's high-pass in Hz.
pub const SMALL_HIGHPASS_HZ: f64 = 140.0;
/// The small speaker's presence bell in dB.
pub const SMALL_PRESENCE_DB: f64 = 3.0;
/// The small speaker's presence bell centre in Hz.
pub const SMALL_PRESENCE_HZ: f64 = 3_000.0;
/// The Q of the presence bell.
pub const SMALL_PRESENCE_Q: f64 = 0.7;
/// The corner of the club system's high-pass in Hz.
pub const CLUB_HIGHPASS_HZ: f64 = 32.0;
/// Butterworth Q for the high-passes.
const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// What the lens makes of the sound, besides mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Voicing {
    /// As it is.
    #[default]
    Headphones,
    /// A phone or a small speaker: no low end, a little presence.
    SmallSpeaker,
    /// A club system: full down to its sub, nothing below it.
    ClubSub,
}

impl Voicing {
    /// Every voicing, in the order of a list.
    pub const ALL: [Self; 3] = [Self::Headphones, Self::SmallSpeaker, Self::ClubSub];

    /// The name the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Headphones => "Headphones",
            Self::SmallSpeaker => "Small Speaker",
            Self::ClubSub => "Club Sub",
        }
    }

    /// The filter sections of the voicing: a shape, its frequency, Q and gain.
    fn sections(self) -> ([(BiquadShape, f64, f64, f64); 2], usize) {
        let none = (BiquadShape::Peaking, 1_000.0, 1.0, 0.0);
        match self {
            Self::Headphones => ([none, none], 0),
            Self::SmallSpeaker => (
                [
                    (BiquadShape::Highpass, SMALL_HIGHPASS_HZ, BUTTERWORTH_Q, 0.0),
                    (
                        BiquadShape::Peaking,
                        SMALL_PRESENCE_HZ,
                        SMALL_PRESENCE_Q,
                        SMALL_PRESENCE_DB,
                    ),
                ],
                2,
            ),
            Self::ClubSub => (
                [
                    (BiquadShape::Highpass, CLUB_HIGHPASS_HZ, BUTTERWORTH_Q, 0.0),
                    none,
                ],
                1,
            ),
        }
    }
}

/// The lens: a voicing and the mono switch. Copied into the engine by one control message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LensSettings {
    /// The speaker.
    pub voicing: Voicing,
    /// Both channels play `(L + R) / 2`.
    pub mono: bool,
}

impl LensSettings {
    /// The lens that changes nothing.
    pub const OFF: Self = Self {
        voicing: Voicing::Headphones,
        mono: false,
    };

    /// True when the lens changes nothing.
    pub fn is_neutral(self) -> bool {
        self == Self::OFF
    }
}

/// The magnitude in dB of `voicing` at `hz` for a stream at `rate` Hz (0 dB for headphones).
pub fn response_db(voicing: Voicing, hz: f64, rate: f64) -> f64 {
    let (sections, n) = voicing.sections();
    sections[..n]
        .iter()
        .map(|&(shape, f, q, g)| BiquadCoeffs::design(shape, f, q, g, rate).response_db(hz, rate))
        .sum()
}

/// The lens on the audio thread: one filter chain, run on the mono sum or on each channel.
#[derive(Debug, Clone)]
pub struct Lens {
    settings: LensSettings,
    coeffs: [BiquadCoeffs; 2],
    sections: usize,
    state: [[Biquad; 2]; 2],
}

impl Lens {
    /// A lens with `settings` for a stream at `rate` Hz, with clear filters.
    pub fn new(settings: LensSettings, rate: f64) -> Self {
        let (defs, sections) = settings.voicing.sections();
        let mut coeffs = [BiquadCoeffs::IDENTITY; 2];
        for (c, &(shape, f, q, g)) in coeffs.iter_mut().zip(&defs).take(sections) {
            *c = BiquadCoeffs::design(shape, f, q, g, rate);
        }
        Self {
            settings,
            coeffs,
            sections,
            state: [[Biquad::default(); 2]; 2],
        }
    }

    /// The settings the lens runs with.
    pub fn settings(&self) -> LensSettings {
        self.settings
    }

    fn run(&mut self, channel: usize, x: f32) -> f32 {
        let mut v = f64::from(x);
        for s in 0..self.sections {
            v = self.state[channel][s].process(&self.coeffs[s], v);
        }
        v as f32
    }

    /// One stereo frame through the lens. A neutral lens returns its input unchanged.
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.settings.is_neutral() {
            return (l, r);
        }
        if self.settings.mono {
            let m = self.run(0, 0.5 * (l + r));
            (m, m)
        } else {
            (self.run(0, l), self.run(1, r))
        }
    }
}

#[cfg(test)]
mod tests;
