//! Whether the shell needs the platform loop to run frames at the full rate.
//!
//! [`Shell::wants_fast_frames`] is the one question the loop asks about a song: it joins what the
//! core answers ([`AppCore::wants_fast_frames`]) with what lives in the shell and the views: a
//! background job, a menu or tooltip that waits for the pointer, a scroll, a glide or a morph, a
//! plugin that loads, a wizard that polls the MIDI ports. The platform adds what it owns (a held
//! pointer button, an open plugin editor, a graphics rebuild) and maps the answer to a wake time
//! in `platform::window_logic`.

use super::*;

impl<B: AudioBackend> Shell<B> {
    /// Whether this song has anything that moves or is polled at the frame rate at `now_seconds`
    /// (the clock of [`Self::frame_at`]). False means the platform may wake only now and then.
    #[must_use]
    pub fn wants_fast_frames(&self, now_seconds: f64) -> bool {
        self.is_busy()
            || self.context_menu_pending()
            || self.tooltip_pending()
            || self.any_bar_open()
            || self.card_pictures_pending()
            || self.controller_wizard.is_some()
            || self.core.view().is_auto_scrolling()
            || self.core.view().unfold_pending()
            || self.core.view().pad_lights_active()
            || self.core.chord_trainer_active()
            || self.core.snapshot_morphing()
            || self.core.macro_gliding()
            || self.core.wants_fast_frames(now_seconds)
    }
}
