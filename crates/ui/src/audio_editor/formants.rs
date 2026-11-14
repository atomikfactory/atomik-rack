//! The Keep formants controls of a Tonal clip, in the Overview toolbar:
//!
//! ```text
//! [KEEP FORMANTS]  SHIFT [-] +2 st [+]  [STEREO LINK]
//! ```
//!
//! KEEP FORMANTS keeps the spectral envelope of the sound where it was while the clip is
//! transposed (a transposed chord or a whole clip keeps its timbre); SHIFT moves the formants
//! on their own by up to 12 semitones (it works with KEEP FORMANTS on); STEREO LINK keeps the
//! phase between left and right while the clip is stretched, so it keeps its image. They are
//! the clip's own settings ([`atomik_model::WarpSettings`]) and each click is one
//! `SetAudioClip`, one undo step. They apply in the rendered buffer; the live preview of a clip
//! that is still being rendered keeps the formants moving with the pitch.

use super::{AudioEditorView, Hit};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::seq_paint as sp;
use atomik_model::{WARP_FORMANT_MAX_CENTS, WarpMode};
use vello::kurbo::Point;

/// The shift of one click, in cents.
const SHIFT_STEP: i32 = 100;

impl AudioEditorView {
    /// Whether the controls apply: the clip is open and warped (or transposed) in Tonal mode.
    pub(super) fn formants_shown(&self) -> bool {
        self.present
            && self
                .info
                .as_ref()
                .is_some_and(|i| i.content.warp.mode == WarpMode::Tonal)
    }

    /// A press on one of the controls: one `SetAudioClip`.
    pub(super) fn formant_press(&mut self, hit: Hit, out: &mut Vec<UiAction>) {
        let (Some(id), Some(info)) = (self.clip, &self.info) else {
            return;
        };
        if !self.formants_shown() {
            return;
        }
        let mut content = info.content.clone();
        let w = &mut content.warp;
        match hit {
            Hit::KeepFormants => w.keep_formants = !w.keep_formants,
            Hit::LinkStereo => w.link_stereo = !w.link_stereo,
            Hit::FormantDown if w.keep_formants => {
                w.formant_cents = (w.formant_cents - SHIFT_STEP).max(-WARP_FORMANT_MAX_CENTS);
            }
            Hit::FormantUp if w.keep_formants => {
                w.formant_cents = (w.formant_cents + SHIFT_STEP).min(WARP_FORMANT_MAX_CENTS);
            }
            _ => return,
        }
        if content != info.content {
            out.push(UiAction::Command(self.set_content(id, info, content)));
        }
    }

    /// The formant shift in whole semitones.
    pub fn formant_semitones(&self) -> i32 {
        self.info
            .as_ref()
            .map_or(0, |i| i.content.warp.formant_cents / 100)
    }

    /// Centre of the KEEP FORMANTS button.
    pub fn keep_formants_button_center(&self) -> Point {
        self.layout().keep.center()
    }

    /// Centre of the formant shift "down" button.
    pub fn formant_down_button_center(&self) -> Point {
        self.layout().formant_down.center()
    }

    /// Centre of the formant shift "up" button.
    pub fn formant_up_button_center(&self) -> Point {
        self.layout().formant_up.center()
    }

    /// Centre of the STEREO LINK button.
    pub fn stereo_link_button_center(&self) -> Point {
        self.layout().link.center()
    }

    /// Paints the controls (Overview mode).
    pub(super) fn paint_formants_toolbar(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let l = self.layout();
        let shown = self.formants_shown();
        let w = self
            .info
            .as_ref()
            .map(|i| i.content.warp.clone())
            .unwrap_or_default();
        let ty = l.bar.center().y + 2.5;
        sp::button(
            c,
            st,
            l.keep,
            "KEEP FORMANTS",
            w.keep_formants.then_some(self.accent),
            shown && self.hover == Hit::KeepFormants,
        );
        super::paint::muted_label(self, c, "SHIFT", l.formant_down.x0 - 38.0, ty);
        sp::button(
            c,
            st,
            l.formant_down,
            "-",
            None,
            shown && self.hover == Hit::FormantDown,
        );
        sp::label(
            c,
            &format!("{:+} st", w.formant_cents / 100),
            (l.formant_down.x1 + l.formant_up.x0) / 2.0,
            l.bar.center().y + 4.0,
            10.0,
            st.base.text,
            Align::Center,
        );
        sp::button(
            c,
            st,
            l.formant_up,
            "+",
            None,
            shown && self.hover == Hit::FormantUp,
        );
        sp::button(
            c,
            st,
            l.link,
            "STEREO LINK",
            w.link_stereo.then_some(self.accent),
            shown && self.hover == Hit::LinkStereo,
        );
        for (r, on) in [
            (l.keep, shown),
            (l.formant_down, shown && w.keep_formants),
            (l.formant_up, shown && w.keep_formants),
            (l.link, shown),
        ] {
            if !on {
                sp::rect(c, r, st.base.background.with_alpha(0.55));
            }
        }
    }
}
