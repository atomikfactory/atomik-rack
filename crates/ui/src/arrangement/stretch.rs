//! Stretching clips by their edges in the [`ArrangementView`].
//!
//! With the Select tool, the alternate-tool role (Alt) held over a clip's start or end edge
//! grabs the edge as a stretch handle: dragging it changes the clip's length by a ratio and
//! everything in it follows (note starts and lengths, automation points, an audio clip's warp),
//! the opposite edge staying where it is. Every selected clip stretches by the same ratio about
//! its own fixed edge. The edge snaps to the grid like a resize does (the disable-snap role
//! works), the preview is the host's trial (`UiAction::StretchStep`, no undo step), the release
//! accepts it as one step and Esc puts everything back. The model plans the stretch (see
//! `atomik_model::WorkflowCommand::StretchClips`). The same gesture on a clip's body keeps the
//! alternate tool's own meaning.
//!
//! With the scale-selection role held (Ctrl+Alt+Shift by default) a drag of a clip's end edge
//! scales the whole selection instead: every selected clip keeps its place relative to the
//! earliest start of the selection, which stays where it is, and the grabbed edge follows the
//! pointer (`atomik_model::WorkflowCommand::ScaleTempo`, the command of the Scale Tempo panel).
//! The preview, the grid and the release work as for the stretch.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, MIN_TICKS};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::keymap::{GestureClass, Role};
use crate::seq_edit;
use crate::viewport::Tool;
use atomik_model::{Clip, ClipId};
use atomik_model::{SCALE_PERCENT_MAX, SCALE_PERCENT_MIN};
use vello::kurbo::Point;

impl ArrangementView {
    /// Whether a press with `modifiers` is the stretch gesture: the Select tool with the
    /// alternate-tool role held (which turns it into the pencil).
    pub(super) fn stretch_role(&self, modifiers: Modifiers) -> bool {
        self.vp.tool() == Tool::Select && self.vp.effective_tool(modifiers) == Tool::Pencil
    }

    /// Whether the pointer is over a clip edge while the stretch role is held (the cursor).
    pub(super) fn stretch_hover(&self) -> bool {
        self.vp.tool() == Tool::Select && self.vp.shown_tool() == Tool::Pencil
    }

    /// Whether a press with `modifiers` is the scale-selection gesture: the Select tool with the
    /// scale-selection role held.
    pub(super) fn scale_role(&self, modifiers: Modifiers) -> bool {
        self.vp.tool() == Tool::Select
            && self
                .vp
                .roles()
                .is_active(Role::ScaleSelection, modifiers, GestureClass::DragItem)
    }

    /// Whether the pointer is over a clip's end edge while the scale-selection role is held
    /// (the cursor).
    pub(super) fn scale_hover(&self) -> bool {
        self.scale_role(self.vp.held())
    }

    /// Starts a scale drag on the end edge of `clip`: the clips of the selection (and `clip`)
    /// scale together about the earliest start among them.
    pub(super) fn begin_scale(&mut self, clip: Clip, pos: Point) {
        self.begin_stretch(clip, false, pos);
        if let Drag::Stretch { whole, .. } = &mut self.drag {
            *whole = true;
        }
    }

    /// Starts a stretch drag on the edge of `clip` (`from_start`: its start edge). The selection
    /// has the clip already; the clips it holds are stretched together.
    pub(super) fn begin_stretch(&mut self, clip: Clip, from_start: bool, pos: Point) {
        let id = clip.id;
        let originals: Vec<Clip> = self
            .song_clips()
            .filter(|c| c.id == id || self.selection.contains(&c.id))
            .cloned()
            .collect();
        self.drag = Drag::Stretch {
            start: pos,
            clip,
            originals,
            from_start,
            whole: false,
            moved: false,
            open: false,
            last: None,
        };
    }

    /// The pointer moved during a stretch drag.
    pub(super) fn stretch_move(
        &mut self,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let step = self.step(modifiers);
        let min = seq_edit::min_length(&step, MIN_TICKS);
        let axis = self.axis;
        let snap_mode = self.vp.snap_mode();
        let Drag::Stretch {
            start,
            clip,
            originals,
            from_start,
            whole,
            moved,
            open,
            last,
        } = &mut self.drag
        else {
            return;
        };
        *moved |= pos.distance(*start) > DRAG_THRESHOLD;
        if !*moved {
            return;
        }
        let delta = axis.dx_to_ticks(pos.x - start.x);
        if *whole {
            // The grabbed end edge goes where the pointer asks; the earliest start stays.
            let base = originals
                .iter()
                .map(|c| c.start)
                .min()
                .unwrap_or(clip.start);
            let end = clip.start + seq_edit::drag_length(clip.start, clip.end(), delta, step, min);
            let ratio = scale_ratio(base, clip, originals, end, min);
            let percent = 100.0 / ratio;
            if (percent - 100.0).abs() < 1e-6 {
                if std::mem::take(open) {
                    out.push(UiAction::StretchEnd { commit: false });
                }
                *last = None;
            } else if *last != Some(percent) {
                out.push(UiAction::ScaleStep {
                    ids: originals.iter().map(|c| c.id).collect::<Vec<ClipId>>(),
                    percent,
                });
                *open = true;
                *last = Some(percent);
            }
            self.repaint = true;
            return;
        }
        let wanted = if *from_start {
            clip.end() - seq_edit::drag_start_by(snap_mode, clip.start, delta, step)
        } else {
            seq_edit::drag_length(clip.start, clip.end(), delta, step, min)
        };
        let ratio = stretch_ratio(clip, originals, *from_start, wanted.max(min));
        let percent = 100.0 / ratio;
        // Back at the original length: the preview is dropped, the drag goes on.
        let unchanged = (percent - 100.0).abs() < 1e-6;
        if unchanged {
            if std::mem::take(open) {
                out.push(UiAction::StretchEnd { commit: false });
            }
            *last = None;
        } else if *last != Some(percent) {
            out.push(UiAction::StretchStep {
                ids: originals.iter().map(|c| c.id).collect::<Vec<ClipId>>(),
                percent,
                from_start: *from_start,
            });
            *open = true;
            *last = Some(percent);
        }
        self.repaint = true;
    }
}

/// The ratio the selection scales by about `base` (its earliest start) when the end of the
/// grabbed `clip` goes to `end`, held to what the model accepts: a tempo between
/// [`SCALE_PERCENT_MIN`] and [`SCALE_PERCENT_MAX`] and no clip shorter than `min` ticks.
fn scale_ratio(base: i64, clip: &Clip, originals: &[Clip], end: i64, min: i64) -> f64 {
    let span = (clip.end() - base).max(1) as f64;
    let mut ratio = (end - base).max(1) as f64 / span;
    let shortest = originals
        .iter()
        .map(|c| c.length)
        .min()
        .unwrap_or(min)
        .max(1);
    ratio = ratio.max(min as f64 / shortest as f64);
    ratio.clamp(100.0 / SCALE_PERCENT_MAX, 100.0 / SCALE_PERCENT_MIN)
}

/// The ratio of the new length to the old for the grabbed `clip` given the length the pointer
/// asks for, held to what the model accepts: a tempo between [`SCALE_PERCENT_MIN`] and
/// [`SCALE_PERCENT_MAX`], and no start edge before tick 0 for any of the clips.
fn stretch_ratio(clip: &Clip, originals: &[Clip], from_start: bool, wanted: i64) -> f64 {
    let mut ratio = wanted as f64 / clip.length.max(1) as f64;
    if from_start {
        for c in originals {
            ratio = ratio.min(c.end() as f64 / c.length.max(1) as f64);
        }
    }
    ratio.clamp(100.0 / SCALE_PERCENT_MAX, 100.0 / SCALE_PERCENT_MIN)
}
