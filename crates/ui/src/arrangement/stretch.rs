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

use super::{ArrangementView, DRAG_THRESHOLD, Drag, MIN_TICKS};
use crate::action::UiAction;
use crate::event::Modifiers;
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
