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
//!
//! The **length handle** sits on the line where the zone ends, inside the later clip. Dragging it
//! makes the crossfade longer or shorter: the later clip stays and the earlier clip's end follows
//! the pointer (snapped like a resize), previewed by `UiAction::CrossfadeLengthStep` and planned by
//! `atomik_model::WorkflowCommand::ResizeCrossfade`. It is offered only where the model accepts
//! it: the earlier clip ends exactly where the crossfade does and neither clip plays backwards
//! or loops.

use super::{ArrangementView, DRAG_THRESHOLD, Drag};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::seq_edit;
use atomik_model::{Clip, ClipContent, ClipId};
use vello::kurbo::{Point, Rect};

/// Half the width of the length handle's grab area, in logical pixels.
const GRAB_PX: f64 = 4.0;
/// Narrowest zone, in logical pixels, that shows a length handle (it would cover the clip's
/// start edge otherwise).
pub(super) const MIN_ZONE_PX: f64 = 10.0;

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
            anchor: clip.start,
            length: false,
            clip,
            min,
            max,
            moved: false,
            open: false,
            last: None,
        };
        true
    }

    /// Whether the crossfade of audio clip `later` can change length: the clip before it ends
    /// where the zone does and neither clip plays backwards or loops. Returns the earlier clip.
    fn resizable_crossfade(&self, later: ClipId) -> Option<(Clip, Clip)> {
        let zone = *self.au.zones.get(&later)?;
        let (_, _, clip) = self.find_clip(later)?;
        let (_, _, earlier) = self.find_clip(zone.earlier)?;
        let reversed = |c: &Clip| matches!(&c.content, ClipContent::Audio(a) if a.reverse);
        let plain = zone.end == earlier.end()
            && !reversed(earlier)
            && clip.loop_length.is_none()
            && earlier.loop_length.is_none();
        plain.then(|| (clip.clone(), earlier.clone()))
    }

    /// Whether `pos` is on the length handle of the crossfade at the start of `clip` (inside the
    /// clip's rectangle `r`, below its header).
    pub(super) fn crossfade_handle_at(&self, pos: Point, r: Rect, clip: &Clip) -> bool {
        let Some(zone) = self.au.zones.get(&clip.id) else {
            return false;
        };
        let (x0, x1) = (
            self.x_of_tick(zone.start as f64),
            self.x_of_tick(zone.end as f64),
        );
        x1 - x0 >= MIN_ZONE_PX
            && (pos.x - x1).abs() <= GRAB_PX
            && pos.y >= r.y0 + super::CLIP_HEADER
            && self.resizable_crossfade(clip.id).is_some()
    }

    /// Whether a plain press at `pos` on audio clip `id` grabs the length handle of its
    /// crossfade. Starts the drag when it does (the clip is selected like a click on it would).
    pub(super) fn crossfade_length_press(
        &mut self,
        id: ClipId,
        pos: Point,
        modifiers: Modifiers,
    ) -> bool {
        if modifiers.shift || modifiers.command() {
            return false;
        }
        let Some((clip, _)) = self.resizable_crossfade(id) else {
            return false;
        };
        let Some(&zone) = self.au.zones.get(&id) else {
            return false;
        };
        // The overlap stays at least a tick long and inside the later clip.
        let (min, max) = (clip.start + 1 - zone.end, clip.end() - 1 - zone.end);
        if min > max {
            return false;
        }
        self.selection.click(id, false, false);
        self.point_sel = None;
        self.selected_track = None;
        self.drag = Drag::Crossfade {
            start: pos,
            anchor: zone.end,
            length: true,
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
            anchor,
            length,
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
        let wanted = seq_edit::drag_start_by(snap_mode, *anchor, raw, step);
        let delta = (wanted - *anchor).clamp(*min, *max);
        if delta == 0 {
            // Back where it started: the preview is dropped, the drag goes on.
            if std::mem::take(open) {
                out.push(UiAction::CrossfadeEnd { commit: false });
            }
            *last = None;
        } else if *last != Some(delta) {
            let later = clip.id;
            out.push(if *length {
                UiAction::CrossfadeLengthStep { later, delta }
            } else {
                UiAction::CrossfadeStep { later, delta }
            });
            *open = true;
            *last = Some(delta);
        }
        self.repaint = true;
    }
}
