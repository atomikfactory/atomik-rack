//! Folder clips: the bars in the timeline part of a folder row.
//!
//! A folder row stands for the clips of its tracks. Clips whose spans overlap merge into one
//! **folder clip** that remembers the ids it stands for. Pressing one selects exactly those
//! clips, so the actions that work on the clip selection (mute, delete, duplicate, split,
//! reverse, quantise, copy) act on all of them. Dragging one moves them in time only: no clip
//! changes its track or lane. The razor splits every contained clip at the pointer, the
//! eraser deletes them and the mute tool toggles them.
//!
//! The folder clips are derived from the song when the view syncs; nothing about them is saved.

use super::{ArrangementView, Drag, Hit, mute_cmd, workflow_cmd};
use crate::action::UiAction;
use crate::event::{Modifiers, PointerButton};
use crate::viewport::Tool;
use atomik_model::{ClipId, Song, SongCommand, WorkflowCommand};
use vello::kurbo::Point;

/// One bar of a folder row: the union of overlapping clip spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderClip {
    /// First tick of the bar.
    pub start: i64,
    /// First tick after the bar.
    pub end: i64,
    /// The clips the bar stands for, in id order.
    pub ids: Vec<ClipId>,
}

/// The folder clips of `group`: every note lane (or the audio lane) of its tracks counts, the
/// automation lanes do not. Sorted by start; spans that overlap are one clip.
pub fn folder_clips(song: &Song, group: u32) -> Vec<FolderClip> {
    let mut spans: Vec<(i64, i64, ClipId)> = Vec::new();
    for i in song.group_tracks(group) {
        let track = &song.tracks[i];
        for lane in track.lanes.iter().take(track.note_lane_count().max(1)) {
            spans.extend(
                lane.clips
                    .iter()
                    .map(|c| (c.start, c.start + c.length, c.id)),
            );
        }
    }
    spans.sort_unstable();
    let mut out: Vec<FolderClip> = Vec::new();
    for (start, end, id) in spans {
        match out.last_mut() {
            Some(last) if start < last.end => {
                last.end = last.end.max(end);
                last.ids.push(id);
            }
            _ => out.push(FolderClip {
                start,
                end,
                ids: vec![id],
            }),
        }
    }
    for f in &mut out {
        f.ids.sort_unstable();
    }
    out
}

impl ArrangementView {
    /// Recomputes the folder clips of every folder.
    pub(super) fn sync_folder_clips(&mut self) {
        let groups: Vec<u32> = self.song.org.groups.iter().map(|g| g.id).collect();
        self.og.folder_clips = groups
            .into_iter()
            .map(|g| (g, folder_clips(&self.song, g)))
            .collect();
    }

    /// The folder clip of `group` under the x of `pos`, if any.
    pub(super) fn folder_clip_at(&self, group: u32, pos: Point) -> Option<usize> {
        let tick = self.axis.x_to_tick(pos.x);
        self.og
            .folder_clips
            .get(&group)?
            .iter()
            .position(|f| (f.start..f.end).contains(&tick))
    }

    fn folder_clip_ids(&self, group: u32, index: usize) -> Vec<ClipId> {
        self.og
            .folder_clips
            .get(&group)
            .and_then(|v| v.get(index))
            .map(|f| f.ids.clone())
            .unwrap_or_default()
    }

    /// A press on a folder clip. True when the hit was one.
    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    pub(super) fn folder_clip_down(
        &mut self,
        hit: &Hit,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Hit::FolderClip(group, index) = *hit else {
            return false;
        };
        let ids = self.folder_clip_ids(group, index);
        if ids.is_empty() {
            return true;
        }
        let primary = button == PointerButton::Primary;
        if primary && clicks >= 2 {
            // A double-click on the timeline part collapses or expands the folder, as it does
            // away from the bars.
            let collapsed = !self.song.org.collapsed(group);
            out.push(super::song_cmd(SongCommand::SetGroupCollapsed {
                group,
                collapsed,
            }));
            return true;
        }
        if primary {
            match self.vp.effective_tool(modifiers) {
                Tool::Razor => {
                    let at = crate::timeline::snap_round(
                        self.axis.x_to_tick(pos.x),
                        self.step(modifiers),
                    );
                    let clips: Vec<ClipId> = ids;
                    out.extend(self.split_all_at_in(at, Some(&clips)));
                    return true;
                }
                Tool::Mute => {
                    let all_muted = self
                        .song_clips()
                        .filter(|c| ids.contains(&c.id))
                        .all(|c| c.muted);
                    out.push(mute_cmd(ids, !all_muted));
                    return true;
                }
                Tool::Eraser => {
                    self.selection.clear();
                    out.push(workflow_cmd(WorkflowCommand::RemoveClips { ids }));
                    return true;
                }
                Tool::Hand | Tool::Magnifier | Tool::Pencil | Tool::Speaker => return true,
                _ => {}
            }
        }
        // The folder row keeps no selection of its own once its clips are picked.
        self.og.group = None;
        self.og.extra.clear();
        self.point_sel = None;
        self.selected_track = None;
        let all = ids.iter().all(|id| self.selection.contains(id));
        if modifiers.command() && primary {
            if all {
                self.selection.retain(|id| !ids.contains(id));
            } else {
                ids.iter().for_each(|id| self.selection.add(*id));
            }
        } else if modifiers.shift && primary {
            ids.iter().for_each(|id| self.selection.add(*id));
        } else if !all {
            self.selection.set(ids.iter().copied());
        }
        self.repaint = true;
        if !primary {
            if button == PointerButton::Secondary {
                out.push(UiAction::ContextMenu {
                    target: crate::action::ContextTarget::Clip,
                    at: pos,
                });
            }
            return true;
        }
        if !ids.iter().all(|id| self.selection.contains(id)) {
            return true;
        }
        let originals: Vec<_> = self
            .song_clips()
            .filter(|c| self.selection.contains(&c.id))
            .cloned()
            .collect();
        let Some(first) = originals.first() else {
            return true;
        };
        let Some((track, lane, _)) = self.find_clip(first.id) else {
            return true;
        };
        let merge = self.merge_id();
        self.drag = Drag::Move {
            start: pos,
            grabbed_start: originals.iter().map(|c| c.start).min().unwrap_or(0),
            originals,
            merge,
            moved: false,
            emitted: false,
            last_dt: 0,
            reselect: None,
            toggle_off: None,
            copy: false,
            grab: (track, lane),
            over: Some((track, lane)),
            horizontal: true,
        };
        true
    }
}
