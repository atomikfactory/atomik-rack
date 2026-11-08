//! The translation lens in the app core: **Mixer > Translation Lens** and the transport bar's more
//! menu switch the master's monitoring between headphones (as it is), a small speaker, a club
//! sub system and mono ([`atomik_dsp::lens`] has the filters).
//!
//! Like the loudness match of an effect it is monitoring state: kept here, sent to the live
//! engine when it changes ([`atomik_engine::EngineHandle::lens_set`]) and again after an engine
//! restart, never stored in the project, not an undo step, and so never part of a bounce or an
//! export (they render the document, not the live output). Choosing a voicing that is on already
//! goes back to headphones; Mono is a switch of its own; Headphones switches everything off. Every
//! change says in the status line what the lens is now, and the status line shows the lens while
//! it is on.

use super::*;
use atomik_engine::{LensSettings, LensVoicing};

/// The lens and what the engine has of it.
#[derive(Debug, Default)]
pub(super) struct LensState {
    /// What the user chose.
    settings: LensSettings,
    /// What the engine was told last (a new engine starts with the lens off).
    sent: LensSettings,
}

/// The words for a setting, as the status line says them.
pub fn lens_label(settings: LensSettings) -> String {
    match (settings.voicing, settings.mono) {
        (LensVoicing::Headphones, false) => "Headphones".to_owned(),
        (LensVoicing::Headphones, true) => "Mono".to_owned(),
        (voicing, false) => voicing.label().to_owned(),
        (voicing, true) => format!("{} + Mono", voicing.label()),
    }
}

impl<S: EngineSink> AppCore<S> {
    /// What the lens is set to.
    pub fn lens(&self) -> LensSettings {
        self.lens.settings
    }

    /// The words for the status line while the lens is on (`LENS SMALL SPEAKER + MONO`); `None`
    /// while the master is heard as it is.
    pub fn lens_status(&self) -> Option<String> {
        let settings = self.lens.settings;
        (!settings.is_neutral()).then(|| format!("LENS {}", lens_label(settings).to_uppercase()))
    }

    /// Chooses `voicing`; the one that is on goes back to headphones. Says what it did.
    pub fn set_lens_voicing(&mut self, voicing: LensVoicing) {
        let now = self.lens.settings.voicing;
        self.lens.settings.voicing = if now == voicing {
            LensVoicing::Headphones
        } else {
            voicing
        };
        self.lens_changed();
    }

    /// Switches mono on or off. Says what it did.
    pub fn toggle_lens_mono(&mut self) {
        self.lens.settings.mono = !self.lens.settings.mono;
        self.lens_changed();
    }

    /// Switches the whole lens off: the master as it is. Says what it did.
    pub fn lens_off(&mut self) {
        self.lens.settings = LensSettings::OFF;
        self.lens_changed();
    }

    fn lens_changed(&mut self) {
        self.push_lens();
        let settings = self.lens.settings;
        self.reject(if settings.is_neutral() {
            "Translation lens off: the master as it is".to_owned()
        } else {
            format!("Translation lens: {}", lens_label(settings))
        });
    }

    /// Sends the lens to the engine when it has another one.
    pub(super) fn push_lens(&mut self) {
        if self.lens.sent == self.lens.settings {
            return;
        }
        let result = self.sink.lens_set(self.lens.settings);
        self.engine_result(result);
        self.lens.sent = self.lens.settings;
    }

    /// A new engine has the lens off: it gets the setting again.
    pub(super) fn lens_after_reset(&mut self) {
        self.lens.sent = LensSettings::OFF;
        self.push_lens();
    }
}
