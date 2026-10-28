//! The tooltips of the arrangement: what the button under the pointer does.

use super::markers::ListHit;
use super::{
    ArrangementView, ClipPart, Drag, FolderButton, Hit, LaneButton, RowKind, TrackButton,
    mute_button_rect,
};
use crate::tooltip::Help;
use crate::viewport::Tool;
use vello::kurbo::{Point, Rect};

/// The help of a track header button.
fn track_button(b: TrackButton, rect: Rect) -> Help {
    let help = |text: &'static str| Help::new(rect, text);
    match b {
        TrackButton::Mute => help("Mute this track"),
        TrackButton::Solo => help("Solo this track: only soloed tracks are heard"),
        TrackButton::Arm => help("Arm this track for recording"),
        TrackButton::Expand => help("Show or hide the automation lanes of this track"),
        TrackButton::AddLane => help("Add an automation lane for a parameter"),
        TrackButton::Input => help("Choose the audio input this track records from"),
        TrackButton::Monitor => help("Input monitoring: click to cycle off, auto, on"),
    }
}

/// The help of an automation or note lane header button.
fn lane_button(b: LaneButton, rect: Rect) -> Help {
    let help = |text: &'static str| Help::new(rect, text);
    match b {
        LaneButton::Mute => {
            Help::new(rect, "Mute this note lane").with_command("sequencer.toggle_lane_mute")
        }
        LaneButton::Fold => help("Fold the lane to a thin strip, or unfold it"),
        LaneButton::Arm => help("Arm this lane for automation recording"),
        LaneButton::Mode => help("Write mode: click to cycle overwrite, touch, latch"),
    }
}

/// The help of a toolbar tool.
fn tool(t: Tool, rect: Rect) -> Help {
    let text = match t {
        Tool::Select => "Select tool: click to select, drag to move, resize or box select",
        Tool::Pencil => "Pencil tool: click to draw a clip or an automation point",
        Tool::Eraser => "Eraser tool: click a clip or a point to delete it",
        Tool::Razor => "Razor tool: click a clip to split it at the snapped position",
    };
    Help::new(rect, text).with_command(t.command_id())
}

impl ArrangementView {
    /// What the button under `pos` does, for its tooltip (`None` while a menu is open or a
    /// gesture runs, and away from any button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.menu.is_some() || !matches!(self.drag, Drag::None) {
            return None;
        }
        let rect_of =
            |track: usize, lane: usize| self.row_of(track, lane).map(|r| self.row_rect(r));
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit_base(pos) {
            Hit::Snap => help(
                self.snap_rect(),
                "Snap grid: click to step to the next value, drag or scroll to choose",
            ),
            Hit::Tool(t) => Some(tool(t, self.tool_rect(t))),
            Hit::Button(track, b) => {
                let rr = rect_of(track, 0)?;
                Some(track_button(b, self.track_button_rect(rr, track, b)))
            }
            Hit::TrackMeter(track) => self.meter_help(rect_of(track, 0)?, track),
            Hit::LaneButton(track, lane, b) => {
                let rr = rect_of(track, lane)?;
                let rect = if b == LaneButton::Mute {
                    let hr = Rect::new(self.bounds.x0, rr.y0, self.axis.x0, rr.y1);
                    self.lane_mute_rect(hr, lane)
                } else {
                    self.lane_button_rect(rr, track, lane, b)
                };
                Some(lane_button(b, rect))
            }
            Hit::FolderToggle(g) => {
                let rr = self.folder_row(g)?;
                help(
                    self.folder_chevron_rect(rr, g),
                    "Collapse or expand this folder",
                )
            }
            Hit::FolderButton(g, b) => {
                let rr = self.folder_row(g)?;
                let rect = self.button_rect(rr, super::organise::track_button(b));
                help(
                    rect,
                    match b {
                        FolderButton::Mute => "Mute every track of this folder",
                        FolderButton::Solo => "Solo every track of this folder",
                        FolderButton::Arm => "Arm every track of this folder for recording",
                    },
                )
            }
            Hit::AddTrack(device) => {
                let rr = self.add_track_row(&device)?;
                help(
                    Rect::new(self.bounds.x0, rr.y0, self.axis.x0, rr.y1),
                    "Add a song track for this device",
                )
            }
            Hit::AddAudio => {
                let rr = self.add_audio_row()?;
                help(
                    Rect::new(self.bounds.x0, rr.y0, self.axis.x0, rr.y1),
                    "Add an audio track",
                )
            }
            Hit::TakeToggle(g) => {
                let rr = self.take_group_row(g)?;
                help(
                    self.group_toggle_rect(rr),
                    "Show or hide the takes of this group",
                )
            }
            Hit::TakeFlatten(g) => {
                let rr = self.take_group_row(g)?;
                help(
                    self.flatten_rect(rr),
                    "Keep what plays as plain clips and drop the takes",
                )
            }
            Hit::TakePick(g, i) => {
                let rr = self.take_row_rect(g, i)?;
                help(
                    Rect::new(self.bounds.x0, rr.y0, self.axis.x0, rr.y1),
                    "Play only this take",
                )
            }
            Hit::TakeDelete(g, i) => {
                let rr = self.take_row_rect(g, i)?;
                help(self.take_delete_rect(rr), "Delete this take")
            }
            Hit::Clip {
                id,
                part: ClipPart::Mute,
                ..
            } => {
                let r = self.clip_rect(id)?;
                let rect = mute_button_rect(r, self.timeline_rect(), self.is_audio(id))?;
                help(rect, "Mute this clip")
            }
            Hit::Panel(ListHit::AddMarker) => {
                help(self.list_add_marker_rect(), "Add a marker at the playhead")
            }
            Hit::Panel(ListHit::AddRange) => {
                help(self.list_add_range_rect(), "Add a range from the loop")
            }
            Hit::Panel(ListHit::Delete(sel)) => {
                help(self.list_delete_rect(sel)?, "Delete this marker or range")
            }
            _ => None,
        }
    }
}

impl ArrangementView {
    /// The rectangle of the row of kind `want`.
    fn row_where(&self, want: impl Fn(&RowKind) -> bool) -> Option<Rect> {
        self.rows
            .iter()
            .find(|r| want(&r.kind))
            .map(|r| self.row_rect(r))
    }

    fn folder_row(&self, group: u32) -> Option<Rect> {
        self.row_where(|k| matches!(k, RowKind::Folder { group: g } if *g == group))
    }

    fn add_track_row(&self, device: &atomik_model::DeviceKey) -> Option<Rect> {
        self.row_where(|k| matches!(k, RowKind::AddTrack(d) if d == device))
    }

    fn add_audio_row(&self) -> Option<Rect> {
        self.row_where(|k| matches!(k, RowKind::AddAudio))
    }

    fn take_group_row(&self, group: u32) -> Option<Rect> {
        self.row_where(|k| matches!(k, RowKind::TakeGroup { group: g, .. } if *g == group))
    }

    fn take_row_rect(&self, group: u32, take: usize) -> Option<Rect> {
        self.row_where(
            |k| matches!(k, RowKind::Take { group: g, take: t, .. } if *g == group && *t == take),
        )
    }
}
