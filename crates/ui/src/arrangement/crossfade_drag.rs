//! Dragging a crossfade zone in the [`ArrangementView`].
//!
//! Where an audio clip starts over the end of an earlier one the overlap is a crossfade zone
//! (see `atomik_model::song::crossfades`), shaded at the start of the later clip. Pressing in
//! the zone with the Select tool and dragging moves it along the timeline: the later clip
//! starts later or earlier and the earlier one ends later or earlier by the same distance, so
//! the cut and the crossfade move together and the overlap keeps its length. The later clip's
//! start snaps to the grid like a resize does (the disable-snap role works), the preview is the
//! host's trial (`UiAction::CrossfadeStep`, no undo step), the release accepts it as one step
//! and Esc puts everything back. The model plans the move (see
//! `atomik_model::WorkflowCommand::MoveCrossfade`); the drag keeps the zone inside both clips.
//! A press that does not start a drag only selects the clip.

use super::{ArrangementView, DRAG_THRESHOLD, Drag};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::seq_edit;
use atomik_model::ClipId;
use vello::kurbo::Point;

impl ArrangementView {
    /// Whether a plain press at `pos` on the body of audio clip `id` grabs its crossfade zone:
    /// the pointer is inside the zone and no selection modifier is held. Starts the drag when
    /// it is (the clip is selected like a click on it would).
    pub(super) fn crossfade_press(&mut self, id: ClipId, pos: Point, modifiers: Modifiers) -> bool {
        if modifiers.shift || modifiers.command() {
            return false;
        }
        let Some(&zone) = self.au.zones.get(&id) else {
            return false;
        };
        let tick = self.axis.x_to_tick(pos.x);
        if !(zone.start..zone.end).contains(&tick) {
            return false;
        }
        let (Some((_, _, clip)), Some((_, _, earlier))) =
            (self.find_clip(id), self.find_clip(zone.earlier))
        else {
            return false;
        };
        // The zone stays inside both clips: the later one starts after the earlier one's start,
        // the earlier one ends before the later one's end.
        let min = earlier.start + 1 - clip.start;
        let max = clip.end() - 1 - zone.end;
        if min > max {
            return false;
        }
        let clip = clip.clone();
        self.selection.click(id, false, false);
        self.point_sel = None;
        self.selected_track = None;
        self.drag = Drag::Crossfade {
            start: pos,
            clip,
            min,
            max,
            moved: false,
            open: false,
            last: None,
        };
        true
    }

    /// The pointer moved during a crossfade drag.
    pub(super) fn crossfade_move(
        &mut self,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let step = self.step(modifiers);
        let axis = self.axis;
        let snap_mode = self.vp.snap_mode();
        let Drag::Crossfade {
            start,
            clip,
            min,
            max,
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
        let raw = axis.dx_to_ticks(pos.x - start.x);
        let wanted = seq_edit::drag_start_by(snap_mode, clip.start, raw, step);
        let delta = (wanted - clip.start).clamp(*min, *max);
        if delta == 0 {
            // Back where it started: the preview is dropped, the drag goes on.
            if std::mem::take(open) {
                out.push(UiAction::CrossfadeEnd { commit: false });
            }
            *last = None;
        } else if *last != Some(delta) {
            out.push(UiAction::CrossfadeStep {
                later: clip.id,
                delta,
            });
            *open = true;
            *last = Some(delta);
        }
        self.repaint = true;
    }
}
