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
//!
//! A track dragged over the VCA bar at the right edge of another track's header (the bar of a
//! track whose channel is in a group) joins that group when released there: the bar's group lights
//! up on every member and no insertion line shows. A group that follows a folder refuses (the
//! model says why).
//!
//! A press on the header of a folder row drags the folder the same way, with its tracks and
//! sub-folders: the pointer picks the same places, and the release moves the folder into or out
//! of another folder (`MoveGroupTo`). A folder is not copied.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, RowKind, TRACK_INDENT, song_cmd, workflow_cmd};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::keymap::{GestureClass, Role};
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, OutlineKind, SongCommand, TrackPlace, WorkflowCommand,
};
use std::collections::BTreeMap;
use vello::kurbo::{Circle, Line, Point};

/// How far left of the header's right edge a drop counts as on the VCA bar.
const VCA_BAR_HIT: f64 = 22.0;

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
    /// The folder that is dragged, with its tracks (`devices` is empty then).
    group: Option<u32>,
    drop: Option<TrackDrop>,
    /// The VCA group whose bar the pointer is over (the release joins it).
    vca: Option<u32>,
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
            group: None,
            drop: None,
            vca: None,
        });
        self.drag = Drag::TrackMove;
    }

    /// Starts a drag of the folder `group` from a press on its header.
    pub(super) fn begin_group_move(&mut self, group: u32, at: Point) {
        self.og.mv = Some(TrackMoveDrag {
            start: at,
            press: DeviceKey(String::new()),
            devices: Vec::new(),
            moved: false,
            copy: false,
            group: Some(group),
            drop: None,
            vca: None,
        });
        self.drag = Drag::TrackMove;
    }

    /// The place a running track drag would drop at (`None` outside a drag, or over a place
    /// that takes no tracks).
    pub fn track_drop(&self) -> Option<TrackDrop> {
        self.og.mv.as_ref().and_then(|d| d.drop)
    }

    /// The VCA group whose bar a running track drag is over (`None` outside a drag, or when the
    /// pointer is not on a bar).
    pub fn vca_drop(&self) -> Option<u32> {
        self.og.mv.as_ref().and_then(|d| d.vca)
    }

    /// The group of the track header under `pos` when `pos` is on the group's bar (the right edge
    /// of the header, wider than the bar it shows).
    fn vca_bar_at(&self, pos: Point) -> Option<u32> {
        if pos.x >= self.axis.x0 || pos.x < self.axis.x0 - VCA_BAR_HIT {
            return None;
        }
        let row = self.row_at(pos.y)?;
        let RowKind::Lane { track, .. } = row.kind else {
            return None;
        };
        let device = &self.song.tracks.get(track)?.device;
        self.vca_chips.get(device).map(|chip| chip.group)
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
        d.copy = copy && d.group.is_none();
        let (devices, group) = (d.devices.clone(), d.group);
        let vca = if group.is_none() {
            self.vca_bar_at(pos)
        } else {
            None
        };
        let drop = match group {
            _ if vca.is_some() => None,
            Some(g) => self.group_drop_at(g, pos.y),
            None => self.track_drop_at(&devices, copy, pos.y),
        };
        if let Some(d) = &mut self.og.mv {
            d.drop = drop;
            d.vca = vca;
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
        if let Some(vca) = d.vca {
            let mut strips: Vec<DeviceKey> = Vec::new();
            for device in &d.devices {
                if let Some(channel) = self.track_channels.get(device)
                    && !strips.contains(channel)
                {
                    strips.push(channel.clone());
                }
            }
            if !strips.is_empty() {
                out.push(UiAction::Command(Command::Console(
                    ConsoleCommand::AssignToVca {
                        vca: Some(vca),
                        strips,
                    },
                )));
            }
            return;
        }
        let Some(drop) = d.drop else {
            return;
        };
        if let Some(group) = d.group {
            out.push(song_cmd(SongCommand::MoveGroupTo {
                group,
                place: drop.place,
            }));
            return;
        }
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

    /// The drop of the folder `group` with the pointer at `y`, `None` where the model would
    /// refuse it (into itself, into the middle of another folder, no change).
    fn group_drop_at(&self, group: u32, y: f64) -> Option<TrackDrop> {
        let song = &self.song;
        let (at, into, line) = self.insertion_at(y)?;
        let inside = song.group_tracks(group);
        // The folder's own tracks leave before the place do not count.
        let before = inside.iter().filter(|i| **i < at).count();
        let place = TrackPlace {
            to_index: at - before,
            into,
        };
        song.moves_group(group, place).then(|| TrackDrop {
            place,
            y: line,
            depth: into.map_or(0, |g| self.og.folder_depth.get(&g).map_or(0, |d| d + 1)),
        })
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
        if let Some(vca) = self.vca_drop() {
            // The bars of the group light up: the tracks would join these.
            for row in &self.rows {
                let RowKind::Lane { track, lane: 0 } = row.kind else {
                    continue;
                };
                let lit = self
                    .song
                    .tracks
                    .get(track)
                    .and_then(|t| self.vca_chips.get(&t.device))
                    .is_some_and(|chip| chip.group == vca);
                if lit {
                    let r = self.row_rect(row);
                    let x = self.axis.x0;
                    c.stroke_with(
                        vello::kurbo::RoundedRect::from_rect(
                            vello::kurbo::Rect::new(x - 7.0, r.y0 + 1.0, x, r.y1 - 1.0),
                            1.5,
                        ),
                        1.5,
                        self.st.selection,
                    );
                }
            }
            return;
        }
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
