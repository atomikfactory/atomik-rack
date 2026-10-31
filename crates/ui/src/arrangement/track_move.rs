//! Dragging a track header: the selected tracks (or the pressed one) move to another place of
//! the track list, into or out of a folder, in one undo step; with the copy role held they are
//! copied there instead.
//!
//! The pointer picks an insertion place among the rows. The upper half of a track's first row
//! is before the track, the rest of its rows after it, in the folder the track is in. The upper
//! half of a folder row is before the folder; its lower half is the start of the folder (the end
//! of it when it is collapsed). Below the last row is the end of the list. A place the model
//! would refuse (the middle of another folder, a frozen track, no change) shows no line and the
//! release does nothing. Escape cancels.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, RowKind, TRACK_INDENT, song_cmd, workflow_cmd};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::keymap::{GestureClass, Role};
use atomik_model::{DeviceKey, OutlineKind, SongCommand, TrackPlace, WorkflowCommand};
use std::collections::BTreeMap;
use vello::kurbo::{Circle, Line, Point};

/// Where a track drag would drop, and where its line is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackDrop {
    /// The place the command takes.
    pub place: TrackPlace,
    /// The y of the insertion line.
    pub y: f64,
    /// How many folders deep the place is (the line starts further right).
    pub depth: usize,
}

/// A track header drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TrackMoveDrag {
    start: Point,
    /// The track that was pressed.
    press: DeviceKey,
    /// The tracks that go: the selection when the pressed track was part of several, else it.
    devices: Vec<DeviceKey>,
    /// The pointer has gone past the drag threshold.
    moved: bool,
    /// The copy role is held: the tracks are copied.
    copy: bool,
    drop: Option<TrackDrop>,
}

/// An insertion place before it is turned into a command: the index among all tracks, the
/// folder, the line.
type Insertion = (usize, Option<u32>, f64);

impl ArrangementView {
    /// Starts a drag of the tracks `devices` from a press on the header of `press`.
    pub(super) fn begin_track_move(
        &mut self,
        press: DeviceKey,
        devices: Vec<DeviceKey>,
        at: Point,
    ) {
        self.og.mv = Some(TrackMoveDrag {
            start: at,
            press,
            devices,
            moved: false,
            copy: false,
            drop: None,
        });
        self.drag = Drag::TrackMove;
    }

    /// The place a running track drag would drop at (`None` outside a drag, or over a place
    /// that takes no tracks).
    pub fn track_drop(&self) -> Option<TrackDrop> {
        self.og.mv.as_ref().and_then(|d| d.drop)
    }

    /// Pointer motion during a track drag.
    pub(super) fn track_move_move(&mut self, pos: Point, modifiers: Modifiers) {
        let copy = self
            .vp
            .roles()
            .is_active(Role::CopyDrag, modifiers, GestureClass::DragItem);
        let Some(d) = &mut self.og.mv else {
            return;
        };
        if !d.moved && pos.distance(d.start) <= DRAG_THRESHOLD {
            return;
        }
        d.moved = true;
        d.copy = copy;
        let devices = d.devices.clone();
        let drop = self.track_drop_at(&devices, copy, pos.y);
        if let Some(d) = &mut self.og.mv {
            d.drop = drop;
        }
        self.repaint = true;
    }

    /// The release of a track drag: the move (or copy) is one command.
    pub(super) fn track_move_up(&mut self, out: &mut Vec<UiAction>) {
        self.drag = Drag::None;
        let Some(d) = self.og.mv.take() else {
            return;
        };
        self.repaint = true;
        if !d.moved {
            // A press that went nowhere on one of several selected tracks picks that one.
            if d.devices.len() > 1 {
                self.set_selected_track(Some(d.press));
            }
            return;
        }
        let Some(drop) = d.drop else {
            return;
        };
        out.push(if d.copy {
            workflow_cmd(WorkflowCommand::DuplicateTracks {
                devices: d.devices,
                place: Some(drop.place),
            })
        } else {
            song_cmd(SongCommand::MoveTracks {
                devices: d.devices,
                place: drop.place,
            })
        });
    }

    /// Escape during a track drag.
    pub(super) fn track_move_cancel(&mut self) {
        self.drag = Drag::None;
        self.og.mv = None;
        self.repaint = true;
    }

    /// The drop of `devices` with the pointer at `y`, `None` where nothing can be dropped.
    fn track_drop_at(&self, devices: &[DeviceKey], copy: bool, y: f64) -> Option<TrackDrop> {
        let song = &self.song;
        if devices
            .iter()
            .any(|d| song.track(d).is_none_or(|t| t.frozen.is_some()))
        {
            return None;
        }
        let (at, into, line) = self.insertion_at(y)?;
        let place = if copy {
            TrackPlace { to_index: at, into }
        } else {
            // The tracks that leave before the place do not count.
            let before = song
                .tracks
                .iter()
                .take(at)
                .filter(|t| devices.contains(&t.device))
                .count();
            TrackPlace {
                to_index: at - before,
                into,
            }
        };
        let ok = if copy {
            song.accepts_copies_at(place)
        } else {
            song.moves_tracks(devices, place)
        };
        ok.then(|| TrackDrop {
            place,
            y: line,
            depth: into.map_or(0, |g| self.og.folder_depth.get(&g).map_or(0, |d| d + 1)),
        })
    }

    /// The insertion place at `y`: the index among all tracks it goes before, the folder it
    /// joins and the y of its line.
    fn insertion_at(&self, y: f64) -> Option<Insertion> {
        let song = &self.song;
        let outline = song.outline();
        let mut track_parent: BTreeMap<usize, Option<u32>> = BTreeMap::new();
        let mut folder_parent: BTreeMap<u32, Option<u32>> = BTreeMap::new();
        for e in &outline {
            match e.kind {
                OutlineKind::Track(i) => {
                    track_parent.insert(i, e.parent);
                }
                OutlineKind::Folder(g) => {
                    folder_parent.insert(g, e.parent);
                }
            }
        }
        let rows: Vec<_> = self
            .rows
            .iter()
            .map(|r| (r.kind.clone(), self.row_rect(r)))
            .collect();
        // The bottom of the last row of the list (the rows of the add buttons come after it).
        let list_bottom = rows
            .iter()
            .rev()
            .find(|(k, _)| !matches!(k, RowKind::AddTrack(_) | RowKind::AddAudio))
            .map(|(_, r)| r.y1)?;
        let end = (song.tracks.len(), None, list_bottom);
        let (first_y, _) = rows.first().map(|(_, r)| (r.y0, ()))?;
        if y >= list_bottom {
            return Some(end);
        }
        let y = y.max(first_y);
        let (kind, rect) = rows.iter().find(|(_, r)| y >= r.y0 && y < r.y1)?;
        let upper = y < rect.y0 + rect.height() / 2.0;
        let block_bottom = |track: usize| {
            rows.iter()
                .filter(|(k, _)| match k {
                    RowKind::Lane { track: t, .. }
                    | RowKind::TakeGroup { track: t, .. }
                    | RowKind::Take { track: t, .. } => *t == track,
                    _ => false,
                })
                .map(|(_, r)| r.y1)
                .fold(rect.y1, f64::max)
        };
        Some(match kind {
            RowKind::Folder { group } => {
                let tracks = song.group_tracks(*group);
                let (first, last) = (*tracks.first()?, *tracks.last()?);
                if upper {
                    (first, folder_parent.get(group).copied().flatten(), rect.y0)
                } else if song.org.collapsed(*group) {
                    (last + 1, Some(*group), rect.y1)
                } else {
                    (first, Some(*group), rect.y1)
                }
            }
            RowKind::Lane { track, lane } => {
                let parent = track_parent.get(track).copied().flatten();
                if *lane == 0 && upper {
                    (*track, parent, rect.y0)
                } else {
                    (*track + 1, parent, block_bottom(*track))
                }
            }
            RowKind::TakeGroup { track, .. } | RowKind::Take { track, .. } => {
                let parent = track_parent.get(track).copied().flatten();
                (*track + 1, parent, block_bottom(*track))
            }
            RowKind::AddTrack(_) | RowKind::AddAudio => end,
        })
    }

    /// The insertion line of a running track drag.
    pub(super) fn paint_track_drop(&self, c: &mut dyn crate::canvas::Canvas) {
        use crate::canvas::CanvasExt;
        let Some(drop) = self.track_drop() else {
            return;
        };
        let color = self.st.selection;
        let x0 = self.bounds.x0 + drop.depth.min(3) as f64 * TRACK_INDENT;
        let y = drop.y.round();
        c.stroke_with(Line::new((x0, y), (self.bounds.x1, y)), 2.0, color);
        c.fill_with(Circle::new((x0 + 3.0, y), 3.0), color);
    }
}
