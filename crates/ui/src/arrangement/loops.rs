//! Looped clips and clip names and colours in the [`ArrangementView`].
//!
//! * The **loop handle** sits in the upper right corner of a clip's body (it shows while the
//!   clip is looped, selected or under the pointer). Dragging it to the right repeats what the
//!   clip shows: the clip grows to the pointer and, when it did not loop, starts to loop over
//!   what it showed. Dragging it back shortens the clip. A looped audio clip that plays
//!   backwards has none (a reversed clip cannot loop), nor has a frozen track's render.
//! * The **loop separators** are drawn where each repetition begins, with a bevel.
//! * `sequencer.convert_loop` turns the loops of the selected clips into one clip each
//!   (`single`) or one clip per repetition (`separate`); `sequencer.loop_instance` makes the
//!   repetition under the playhead of the selected looped clip a clip of its own. The model
//!   plans both (see `atomik_model::WorkflowCommand::ConvertLoop`).
//! * A clip's own **name** is typed over its label (a double-click on the header strip, or
//!   `sequencer.rename_clip`); its own **colour** is `sequencer.clip_color` (the palette of the
//!   markers, or `track` for the colour of its track). A blank name goes back to the generated
//!   label. Both are `SetClipMeta`.

use super::{ArrangementView, CLIP_HEADER, Drag, Hit, MUTE_BUTTON, song_cmd, workflow_cmd};
use crate::action::UiAction;
use crate::keymap::CommandOutcome;
use crate::marker_edit::palette_color;
use crate::seq_edit;
use crate::timeline::Quant;
use atomik_model::song::MAX_LOOP_REPEATS;
use atomik_model::{
    Clip, ClipContent, ClipId, LoopConvert, MetaChange, SongCommand, WorkflowCommand,
};
use vello::kurbo::Rect;

/// Width of the loop handle.
pub(super) const LOOP_HANDLE_W: f64 = 11.0;
/// The shortest clip body that has room for the handle under the header.
const LOOP_HANDLE_MIN_BODY: f64 = MUTE_BUTTON + 4.0;

/// The loop handle of a clip drawn in `r` (visible part `tl`), `None` when the clip is too small
/// for it, or cannot loop.
pub(super) fn loop_handle_rect(r: Rect, tl: Rect, clip: &Clip) -> Option<Rect> {
    if let ClipContent::Audio(a) = &clip.content
        && a.reverse
    {
        return None;
    }
    let right = r.x1.min(tl.x1);
    if r.width() < 3.0 * MUTE_BUTTON
        || right - r.x0.max(tl.x0) < 3.0 * MUTE_BUTTON
        || r.height() - CLIP_HEADER < LOOP_HANDLE_MIN_BODY
    {
        return None;
    }
    let top = r.y0 + CLIP_HEADER + 2.0;
    Some(Rect::new(
        right - 2.0 - LOOP_HANDLE_W,
        top,
        right - 2.0,
        top + MUTE_BUTTON,
    ))
}

/// The command that sets a clip's geometry and loop in one go, for the loop handle: every clip
/// kind goes through `SetClipGeometry` (an audio clip has no offset, and its fades and content
/// stay as they are).
pub(super) fn loop_geometry_cmd(
    c: &Clip,
    length: i64,
    loop_length: Option<i64>,
    merge: u64,
) -> UiAction {
    song_cmd(SongCommand::SetClipGeometry {
        id: c.id,
        start: c.start,
        length,
        offset: if c.content.is_audio() { 0 } else { c.offset },
        loop_length,
        muted: c.muted,
        merge: Some(merge),
    })
}

/// What the loop handle's drag asks for when the pointer moved `delta` ticks: the clip's length
/// and loop. A clip that did not loop starts to loop over what it showed (its offset and
/// length) once it grows past its length; a looped clip keeps its loop. The length stays inside
/// the model's repetition limit.
pub(super) fn loop_drag_wanted(
    clip: &Clip,
    delta: i64,
    step: impl Quant,
    min: i64,
) -> (i64, Option<i64>) {
    let n = clip.loop_length.unwrap_or(clip.offset + clip.length);
    let mut length = seq_edit::drag_length(clip.start, clip.end(), delta, step, min);
    length = length.min(n.saturating_mul(MAX_LOOP_REPEATS));
    match clip.loop_length {
        Some(n) => (length, Some(n)),
        None if length > clip.length => (length, Some(n)),
        None => (length, None),
    }
}

impl ArrangementView {
    /// The loop commands and the clip name and colour commands; `None` for an id that is not one
    /// of them.
    pub(super) fn loop_command(&mut self, id: &str, arg: Option<&str>) -> Option<CommandOutcome> {
        match id {
            "sequencer.rename_clip" => {
                let clips = self.selected_in_order();
                let [clip] = clips.as_slice() else {
                    return Some(CommandOutcome::NotApplicable(
                        "Select one clip to rename".into(),
                    ));
                };
                let id = clip.id;
                self.begin_clip_rename(id);
                Some(CommandOutcome::Handled(Vec::new()))
            }
            "sequencer.clip_color" => {
                let color = match arg? {
                    "track" => MetaChange::Clear,
                    name => match palette_color(name) {
                        Some(Some(rgb)) => MetaChange::Set(rgb),
                        _ => return Some(CommandOutcome::Declined),
                    },
                };
                let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                if ids.is_empty() {
                    return Some(CommandOutcome::NotApplicable(
                        "Select the clips to colour".into(),
                    ));
                }
                Some(CommandOutcome::Handled(vec![song_cmd(
                    SongCommand::SetClipMeta {
                        ids,
                        name: MetaChange::Keep,
                        color,
                    },
                )]))
            }
            "sequencer.convert_loop" => {
                let mode = match arg? {
                    "single" => LoopConvert::Single,
                    "separate" => LoopConvert::Separate,
                    _ => return Some(CommandOutcome::Declined),
                };
                let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                if !self
                    .selected_in_order()
                    .iter()
                    .any(|c| c.loop_length.is_some())
                {
                    return Some(CommandOutcome::NotApplicable(
                        "Select a clip that loops".into(),
                    ));
                }
                Some(CommandOutcome::Handled(vec![workflow_cmd(
                    WorkflowCommand::ConvertLoop { ids, mode },
                )]))
            }
            "sequencer.loop_instance" => {
                let clips = self.selected_in_order();
                let [clip] = clips.as_slice() else {
                    return Some(CommandOutcome::NotApplicable(
                        "Select one looped clip".into(),
                    ));
                };
                if clip.loop_length.is_none() {
                    return Some(CommandOutcome::NotApplicable(
                        "The selected clip does not loop".into(),
                    ));
                }
                let at = self.playhead;
                let Some(index) = seq_edit::content_windows(clip)
                    .iter()
                    .position(|w| (w.song_start..w.song_end).contains(&at))
                else {
                    return Some(CommandOutcome::NotApplicable(
                        "Put the playhead on the repetition to make independent".into(),
                    ));
                };
                Some(CommandOutcome::Handled(vec![workflow_cmd(
                    WorkflowCommand::DuplicateLoopInstance { id: clip.id, index },
                )]))
            }
            _ => None,
        }
    }

    /// The loop handle drag begins on `clip` at `pos`.
    pub(super) fn begin_loop_drag(&mut self, clip: Clip, pos: vello::kurbo::Point) {
        let merge = self.merge_id();
        self.drag = Drag::Resize {
            start: pos,
            others: Vec::new(),
            last: (clip.length, clip.loop_length),
            clip,
            merge,
            moved: false,
            emitted: false,
            loop_mode: false,
            looping: true,
        };
    }

    /// Whether the loop handle of a clip is painted: while it loops, is selected or is under the
    /// pointer.
    pub(super) fn loop_handle_shown(&self, clip: &Clip) -> bool {
        clip.loop_length.is_some()
            || self.selection.contains(&clip.id)
            || matches!(&self.hover, Some(Hit::Clip { id, .. }) if *id == clip.id)
    }
}
