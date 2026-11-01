//! The Spectrum EQ window in the app core.
//!
//! The window shows one console strip (a channel, a bus or the master): its spectrum, and for
//! all but the master its filters and EQ as a response curve with draggable points
//! ([`atomik_ui::spectrum_eq`]). The core feeds it once per frame from [`AppCore::frame`]:
//!
//! * **The strip** is the one the window follows (the primary selected strip of the mixer, else
//!   the strip of the selected track: Follow Selection) or the one picked with its arrows.
//! * **The tap is switched on only while it is looked at.** The strip's `internal.tap_live`
//!   param (the engine only, like `internal.tuner_live`: never a model command, not stored or
//!   undoable) is 1 for the shown strip while the window is open, and 0 for every other strip, so
//!   a closed window, or a strip nobody looks at, costs the audio thread nothing and renders the
//!   same bits. The value is sent again after an engine restart or when the strip is added again
//!   (a new engine id), and only when it changed.
//! * **The spectrum** is an FFT of the newest 4096 frames of the strip's tap (after the EQ, before
//!   the fader), averaged and mapped to 240 log-frequency columns like the spectrum device's
//!   (`atomik_dsp::spectrum`), run here and never on the audio thread. A tap that did not move
//!   since the last feed (a paused engine) is skipped.
//! * **The curve and the points** come from the strip's params in the document, so a drag shows
//!   at once and undo moves the points back.

use super::*;
use atomik_dsp::spectrum::{Settings, Spectrum};
use atomik_model::console::TAP_LIVE_PARAM;
use atomik_ui::analyser::TRACE_COLUMNS;
use atomik_ui::spectrum_eq::{StripChoice, StripEqState};

/// What the analyser feed of the window remembers between frames.
#[derive(Default)]
pub(super) struct SpectrumEqState {
    /// The window is open.
    shown: bool,
    /// The window follows the selected strip.
    follow: bool,
    /// The strip picked with the arrows (while not following).
    pinned: Option<DeviceKey>,
    /// The strip shown now.
    current: Option<DeviceKey>,
    /// The strip whose tap the engine was told to write, with its engine id.
    sent: Option<(DeviceKey, DeviceInstanceId)>,
    spectrum: Spectrum,
    /// The tap head the trace was computed at.
    head: u64,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl SpectrumEqState {
    /// A new engine: every tap starts again from frame 0 and the engine knows nothing of what
    /// was sent.
    pub(super) fn restart(&mut self) {
        self.sent = None;
        self.head = 0;
    }

    fn reset_analysis(&mut self) {
        self.spectrum.reset();
        self.head = 0;
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The Spectrum EQ view.
    pub fn spectrum_eq(&self) -> &atomik_ui::spectrum_eq::SpectrumEqView {
        &self.spectrum_eq
    }

    /// The Spectrum EQ view, mutably (bounds, scale, focus).
    pub fn spectrum_eq_mut(&mut self) -> &mut atomik_ui::spectrum_eq::SpectrumEqView {
        &mut self.spectrum_eq
    }

    /// The window opened or closed (the shell). Closing switches the strip's tap off.
    pub fn set_spectrum_eq_shown(&mut self, shown: bool) {
        if self.speq.shown == shown {
            return;
        }
        self.speq.shown = shown;
        if shown {
            // A first look follows the selection (the state of the box is kept while closed).
            self.speq.follow = self.speq.follow || self.speq.current.is_none();
            self.speq.reset_analysis();
            self.feed_spectrum_eq();
        } else {
            self.spectrum_eq.clear_trace();
            self.push_tap_live(None);
        }
    }

    /// Whether the window is open.
    pub fn spectrum_eq_shown(&self) -> bool {
        self.speq.shown
    }

    /// The strip the window shows now.
    pub fn spectrum_eq_strip(&self) -> Option<&DeviceKey> {
        self.speq.current.as_ref()
    }

    /// Whether the window follows the selected strip.
    pub fn spectrum_eq_follows(&self) -> bool {
        self.speq.follow
    }

    /// An arrow of the window ([`UiAction::SpectrumEqStep`]): the strip `delta` places along,
    /// wrapping; the window stops following the selection.
    pub(super) fn spectrum_eq_step(&mut self, delta: i8) {
        let keys: Vec<DeviceKey> = self
            .console
            .strip_names()
            .into_iter()
            .map(|(k, _, _)| k)
            .collect();
        if keys.is_empty() {
            return;
        }
        let at = self
            .speq
            .current
            .as_ref()
            .and_then(|c| keys.iter().position(|k| k == c))
            .unwrap_or(0) as i64;
        let next = (at + i64::from(delta)).rem_euclid(keys.len() as i64) as usize;
        self.speq.pinned = Some(keys[next].clone());
        self.speq.follow = false;
        self.feed_spectrum_eq();
    }

    /// The Follow Selection box ([`UiAction::SpectrumEqFollow`]).
    pub(super) fn spectrum_eq_follow(&mut self, follow: bool) {
        self.speq.follow = follow;
        if !follow {
            self.speq.pinned = self.speq.current.clone();
        }
        self.feed_spectrum_eq();
    }

    /// The strip the window should show: the selected one while following, else the picked one.
    fn spectrum_eq_target(&self, choices: &[StripChoice]) -> Option<DeviceKey> {
        let valid = |k: &DeviceKey| choices.iter().any(|c| &c.key == k);
        if self.speq.follow {
            let selected = self
                .console
                .primary_strip()
                .or_else(|| self.console.selected_strips().into_iter().next())
                .or_else(|| self.strips_to_route(false).into_iter().next());
            if let Some(k) = selected.filter(valid) {
                return Some(k);
            }
        }
        self.speq
            .pinned
            .clone()
            .filter(valid)
            .or_else(|| self.speq.current.clone().filter(valid))
            .or_else(|| choices.first().map(|c| c.key.clone()))
    }

    /// Switches the analyser tap on for `want` and off for the strip that had it (module docs).
    fn push_tap_live(&mut self, want: Option<&DeviceKey>) {
        let want_id = want.and_then(|k| self.ids.get(k).copied().map(|id| (k.clone(), id)));
        if self.speq.sent == want_id {
            return;
        }
        if let Some((key, id)) = self.speq.sent.take()
            && let Some(pid) = self.tap_param(&key)
        {
            // The strip may be gone: a refusal is nothing to show.
            let _ = self.sink.set_param(id, pid, 0.0);
        }
        if let Some((key, id)) = want_id
            && let Some(pid) = self.tap_param(&key)
        {
            let result = self.sink.set_param(id, pid, 1.0);
            self.engine_result(result);
            self.speq.sent = Some((key, id));
        }
        self.speq.head = 0;
    }

    fn tap_param(&self, key: &DeviceKey) -> Option<ParamId> {
        let type_id = self.doc().device(key)?.type_id.clone();
        self.registry
            .get(&type_id)?
            .descriptor
            .param_by_key(TAP_LIVE_PARAM)
            .map(|p| p.id)
    }

    /// Feeds the window (module docs); once per frame from [`Self::frame`].
    pub(super) fn feed_spectrum_eq(&mut self) {
        if !self.speq.shown {
            if self.speq.sent.is_some() {
                self.push_tap_live(None);
            }
            return;
        }
        let choices: Vec<StripChoice> = self
            .console
            .strip_names()
            .into_iter()
            .map(|(key, name, master)| StripChoice { key, name, master })
            .collect();
        self.spectrum_eq.set_strips(choices.clone());
        self.spectrum_eq.set_follow(self.speq.follow);
        let target = self.spectrum_eq_target(&choices);
        if target != self.speq.current {
            self.speq.current.clone_from(&target);
            self.speq.reset_analysis();
            self.spectrum_eq.clear_trace();
        }
        let sample_rate = f64::from(self.doc().sample_rate);
        let state = target.as_ref().and_then(|key| {
            let choice = choices.iter().find(|c| &c.key == key)?;
            let dev = self.doc().device(key)?;
            let desc = self.registry.descriptor(&dev.type_id)?;
            Some(StripEqState::read(choice, desc, dev, sample_rate))
        });
        self.spectrum_eq.set_state(state);
        self.push_tap_live(target.as_ref());
        let Some(id) = target.as_ref().and_then(|k| self.ids.get(k).copied()) else {
            return;
        };
        let settings = Settings::default();
        let mut a = std::mem::take(&mut self.speq);
        a.left.resize(settings.size, 0.0);
        a.right.resize(settings.size, 0.0);
        if let Some((n, head)) = self.sink.read_device_scope(id, &mut a.left, &mut a.right)
            && head != 0
            && head != a.head
        {
            let elapsed = head.saturating_sub(a.head);
            a.head = head;
            a.spectrum.update(
                &a.left[..n],
                &a.right[..n],
                elapsed,
                sample_rate as f32,
                &settings,
                TRACE_COLUMNS,
            );
            let peaks = if settings.hold_ms > 0.0 {
                a.spectrum.peaks()
            } else {
                &[]
            };
            self.spectrum_eq.set_trace(a.spectrum.trace(), peaks);
        }
        self.speq = a;
    }
}
