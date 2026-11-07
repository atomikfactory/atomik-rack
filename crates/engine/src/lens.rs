//! The translation lens on the live output: the master heard as a small speaker, a club system or
//! in mono ([`atomik_dsp::lens`] has the filters), applied to interface outputs 1 and 2 after the
//! master chain and the reference, before the click and the browser preview.
//!
//! It is a listening aid: the settings arrive in a control message
//! ([`EngineHandle::lens_set`](crate::EngineHandle)) and nothing but the live engine ever receives
//! one. An offline render, a bounce or an export runs the same engine with the lens never set, so
//! it is neutral and the output is the mix bit for bit, as it is in the live engine until a
//! setting is chosen and again once the lens is switched off.
//!
//! A change of the settings ducks the output 5 ms down, swaps the filters at silence (clear state)
//! and brings it back 5 ms up, so it never clicks. No allocation on the audio thread.

use atomik_core::Batch;
use atomik_dsp::lens::{Lens, LensSettings};

/// The duck around a change, one way, in seconds.
const DUCK_SECONDS: f64 = 0.005;

/// The voice on the audio thread (owned by the engine).
pub(crate) struct LensVoice {
    rate: f64,
    duck_step: f32,
    /// What the user asked for.
    target: LensSettings,
    /// What the filters run now.
    lens: Lens,
    /// The duck around a change: 1 is open.
    duck: f32,
}

impl LensVoice {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            rate,
            duck_step: (1.0 / (DUCK_SECONDS * rate)).min(1.0) as f32,
            target: LensSettings::OFF,
            lens: Lens::new(LensSettings::OFF, rate),
            duck: 1.0,
        }
    }

    /// Takes new settings; the change is made with a duck, in the next batches.
    pub(crate) fn set(&mut self, settings: LensSettings) {
        self.target = settings;
    }

    /// The lens neither runs nor is asked to.
    fn idle(&self) -> bool {
        self.target.is_neutral() && self.lens.settings().is_neutral() && self.duck >= 1.0
    }

    /// Runs the lens over the first two channels of `out`. Neutral: nothing is touched.
    pub(crate) fn process(&mut self, out: &mut [Batch]) {
        if out.len() < 2 || self.idle() {
            return;
        }
        let (first, rest) = out.split_at_mut(1);
        let (left, right) = (&mut first[0], &mut rest[0]);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.target != self.lens.settings() {
                self.duck -= self.duck_step;
                if self.duck <= 0.0 {
                    self.duck = 0.0;
                    self.lens = Lens::new(self.target, self.rate);
                }
            } else if self.duck < 1.0 {
                self.duck = (self.duck + self.duck_step).min(1.0);
            }
            if self.lens.settings().is_neutral() && self.duck >= 1.0 {
                continue;
            }
            let (a, b) = self.lens.process(*l, *r);
            *l = a * self.duck;
            *r = b * self.duck;
        }
    }
}

#[cfg(test)]
mod tests;
