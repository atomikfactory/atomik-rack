//! Track organisation in the [`ArrangementView`]: folders, track colours and comments, folded
//! automation lanes and the track height presets. The data is the song's ([`Song::org`], see
//! `atomik_model::organise`); this module lays it out, draws it and turns gestures and commands
//! into song commands.
//!
//! * A **folder row** ([`FOLDER_ROW_HEIGHT`] tall) stands above the first track of a folder,
//!   indented by its depth. Its header has a chevron (collapse or expand: the tracks below it are
//!   not laid out while it is collapsed), the name (double-click renames it) and M, S and R
//!   buttons that act on every track of the folder in one batch: a button is lit when all of
//!   them have the flag, a press sets the flag on all of them unless all have it already, and
//!   the audio is the tracks' own mute and solo, nothing more. Its timeline part shows the
//!   clips of the folder's tracks as thin bars, whether it is collapsed or not.
//! * The header of a track in a folder is **indented**: its content moves right by
//!   [`TRACK_INDENT`] per level of nesting (three at most), the clips stay where they are. The
//!   folder guides stay at the left edge.
//! * A **track's name** is typed in its header: a double-click on the name, or `sequencer.rename_track`
//!   on the selected track. The edit is the one of the folder names (Enter or a press elsewhere
//!   commits, Escape cancels) and commits the mixer's `Command::Rename`, so a bus or a channel is
//!   renamed in the console too; a blank or unchanged name does nothing. A note lane's label has its
//!   own double-click (`RenameLane`).
//! * A **selected track** is outlined; shift-click or ctrl-click on a header adds a track to the
//!   selection and a click on a folder row selects the folder (its tracks count as selected).
//!   Group Tracks takes the selected tracks, or the tracks of the selected clips.
//! * A track's **colour** replaces its device's colour in the header's accent bar and in its
//!   clips. Its **comment** is typed in the header (`sequencer.track_comment`, Enter commits,
//!   Escape cancels, the text owner pattern of the marker names), and a header with one shows a
//!   note mark that opens a tooltip with the first line while the pointer is on it.
//! * An automation lane folds to a thin strip ([`FOLDED_LANE_HEIGHT`]) with the chevron at the
//!   left of its header; its clips are drawn as bars and cannot be edited until it is unfolded.
//!   Folding is saved with the project, like the take rows that are open.
//! * The **height presets** (`sequencer.track_height`) set the base height of the selected
//!   tracks (every track when none is selected): Small, Medium (the default), Large, or Fit,
//!   which divides what the view shows among the tracks.

use super::{
    ArrangementView, Drag, HSCROLL_HEIGHT, Hit, LaneButton, Row, RowKind, TRACK_LIST_WIDTH,
    TrackButton, song_cmd,
};
use crate::action::{ContextTarget, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton};
use crate::keymap::CommandOutcome;
use crate::marker_edit::{auto_name, palette_color};
use crate::paint::{fit_label, tooltip};
use crate::seq_paint as sp;
use crate::theme::mix;
use atomik_model::{
    Command, DeviceKey, MAX_GROUP_NAME, MAX_LANE_NAME_CHARS, MAX_TRACK_COMMENT, MAX_TRACK_HEIGHT,
    MIN_TRACK_HEIGHT, RackDoc, SongCommand,
};
use std::collections::BTreeSet;
use vello::kurbo::{BezPath, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of a folder row.
pub const FOLDER_ROW_HEIGHT: f64 = 26.0;
/// Height of a folded automation lane.
pub const FOLDED_LANE_HEIGHT: f64 = 16.0;
/// Base height of a track after the Small preset.
pub const HEIGHT_SMALL: u16 = 32;
/// Base height of a track after the Large preset.
pub const HEIGHT_LARGE: u16 = 120;
/// How far each level of nesting moves a folder's controls to the right.
const DEPTH_STEP: f64 = 8.0;
/// How far each level of nesting moves the content of a track header to the right.
pub const TRACK_INDENT: f64 = 10.0;
/// The deepest level the header indent follows: the content of a track nested deeper sits where
/// this one does, so the controls still fit in the track list.
const MAX_INDENT_LEVELS: usize = 3;
/// Longest a track's name can be typed in the header (the console's limit for a bus name).
const MAX_TRACK_NAME_CHARS: usize = atomik_model::console::MAX_BUS_NAME_CHARS;
/// Longest first line of a comment the tooltip shows, in characters.
const TIP_CHARS: usize = 80;

/// A control in the header of a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderButton {
    /// Mute every track of the folder.
    Mute,
    /// Solo every track of the folder.
    Solo,
    /// Arm every track of the folder.
    Arm,
}

/// What `sequencer.track_height` sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightPreset {
    /// [`HEIGHT_SMALL`].
    Small,
    /// The default height (the entry is removed).
    Medium,
    /// [`HEIGHT_LARGE`].
    Large,
    /// As tall as the view allows, the tracks sharing it.
    Fit,
}

impl HeightPreset {
    /// The preset a command argument names.
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            "fit" => Some(Self::Fit),
            _ => None,
        }
    }

    /// The command argument.
    pub fn arg(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::Fit => "fit",
        }
    }
}

/// What the Track menu and the context menus need to know about the selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrgMenu {
    /// Tracks, or clips whose tracks can be grouped, are selected.
    pub can_group: bool,
    /// A folder is selected, or the selected track is in one: Ungroup and Collapse work on it.
    pub in_folder: bool,
    /// That folder is collapsed (the item reads Expand).
    pub collapsed: bool,
    /// A folder row is selected: Rename Folder works.
    pub folder_selected: bool,
    /// A track is selected: Track Comment works.
    pub track_selected: bool,
    /// The selected track or folder can move up.
    pub can_move_up: bool,
    /// The selected track or folder can move down.
    pub can_move_down: bool,
}

/// What is being typed in the track list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditTarget {
    /// The name of a folder.
    Folder(u32),
    /// The comment of a track.
    Comment(DeviceKey),
    /// The name of a note lane of a track.
    Lane(DeviceKey, usize),
    /// The name of a track's device (a channel or a bus: the console's rename).
    Track(DeviceKey),
}

/// A text being typed: one line, Enter commits, Escape cancels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TextEdit {
    target: EditTarget,
    text: String,
    /// The text the edit started with: committing it unchanged does nothing.
    original: String,
}

/// What the view remembers about the organisation of the tracks.
#[derive(Debug, Default)]
pub(super) struct OrgUi {
    /// The selected folder.
    pub group: Option<u32>,
    /// The tracks selected besides `selected_track`.
    pub extra: BTreeSet<DeviceKey>,
    /// The text being typed.
    pub edit: Option<TextEdit>,
    /// Per track (index into `Song::tracks`): nesting depth, and whether it is hidden in a
    /// collapsed folder.
    pub shape: Vec<(usize, bool)>,
    /// The nesting depth of each folder.
    pub folder_depth: std::collections::BTreeMap<u32, usize>,
}

fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb8(r, g, b)
}

impl ArrangementView {
    // ------------------------------------------------------------------ selection

    /// The selected folder, if one is.
    pub fn selected_group(&self) -> Option<u32> {
        self.og.group
    }

    /// Selects a folder (or none); a folder that does not exist selects nothing. The clip, point
    /// and track selections are cleared.
    pub fn set_selected_group(&mut self, group: Option<u32>) {
        let group = group.filter(|g| self.song.org.group(*g).is_some());
        if group.is_some() {
            self.selection.clear();
            self.point_sel = None;
            self.selected_track = None;
            self.og.extra.clear();
        }
        self.og.group = group;
        self.repaint = true;
    }

    /// Adds a track to the selected tracks, or takes it out again (the first selected track,
    /// the one a plain click selected, stays). Selects the track alone when none is selected.
    pub fn toggle_selected_track(&mut self, device: &DeviceKey) {
        if self.selected_track.is_none() || self.song.track(device).is_none() {
            self.set_selected_track(Some(device.clone()));
            return;
        }
        if self.selected_track.as_ref() == Some(device) {
            return;
        }
        if !self.og.extra.remove(device) {
            self.og.extra.insert(device.clone());
        }
        self.og.group = None;
        self.repaint = true;
    }

    /// The selected tracks in track order: the selected folder's tracks, or the selected track
    /// and the ones added to it.
    pub fn selected_tracks(&self) -> Vec<DeviceKey> {
        if let Some(g) = self.og.group {
            return self
                .song
                .group_tracks(g)
                .into_iter()
                .map(|i| self.song.tracks[i].device.clone())
                .collect();
        }
        let Some(first) = &self.selected_track else {
            return Vec::new();
        };
        self.song
            .tracks
            .iter()
            .map(|t| &t.device)
            .filter(|d| *d == first || self.og.extra.contains(*d))
            .cloned()
            .collect()
    }

    /// The tracks a command such as Group Tracks or Freeze Track works on: the selected tracks,
    /// else the tracks of the selected clips, in track order.
    pub fn command_tracks(&self) -> Vec<DeviceKey> {
        self.group_candidates()
    }

    /// What Group Tracks works on: the selected tracks, else the tracks of the selected clips.
    fn group_candidates(&self) -> Vec<DeviceKey> {
        let tracks = self.selected_tracks();
        if !tracks.is_empty() && self.og.group.is_none() {
            return tracks;
        }
        let of_clips: BTreeSet<usize> = self
            .selection
            .items()
            .iter()
            .filter_map(|id| self.find_clip(*id).map(|(t, _, _)| t))
            .collect();
        of_clips
            .into_iter()
            .map(|i| self.song.tracks[i].device.clone())
            .collect()
    }

    /// The folder Ungroup and Collapse work on: the selected folder, else the innermost folder
    /// of the selected track.
    fn target_folder(&self) -> Option<u32> {
        self.og.group.or_else(|| {
            self.selected_track
                .as_ref()
                .and_then(|d| self.song.track_group(d))
        })
    }

    /// How far the content of a track's header (and of its lanes' headers) is moved right:
    /// [`TRACK_INDENT`] per level of folder it is in, up to [`MAX_INDENT_LEVELS`]. The clips do
    /// not move.
    pub(super) fn track_indent(&self, track: usize) -> f64 {
        let depth = self.og.shape.get(track).map_or(0, |s| s.0);
        depth.min(MAX_INDENT_LEVELS) as f64 * TRACK_INDENT
    }

    /// The nesting depth of a track (0 at the top level).
    pub fn track_depth(&self, device: &DeviceKey) -> usize {
        self.song
            .tracks
            .iter()
            .position(|t| &t.device == device)
            .and_then(|i| self.og.shape.get(i))
            .map_or(0, |s| s.0)
    }

    /// Whether the arrangement hides a track because a folder above it is collapsed.
    pub fn track_hidden(&self, device: &DeviceKey) -> bool {
        self.song
            .tracks
            .iter()
            .position(|t| &t.device == device)
            .and_then(|i| self.og.shape.get(i))
            .is_some_and(|s| s.1)
    }

    /// What the Track menu and the context menus enable for the selection.
    pub fn org_menu(&self) -> OrgMenu {
        let folder = self.target_folder();
        let (up, down) = match (self.og.group, &self.selected_track) {
            (Some(g), _) => (
                self.song.can_move_group(g, false),
                self.song.can_move_group(g, true),
            ),
            (None, Some(d)) => (
                self.song.can_move_track(d, false),
                self.song.can_move_track(d, true),
            ),
            (None, None) => (false, false),
        };
        OrgMenu {
            can_group: !self.group_candidates().is_empty(),
            in_folder: folder.is_some(),
            collapsed: folder.is_some_and(|g| self.song.org.collapsed(g)),
            folder_selected: self.og.group.is_some(),
            track_selected: self.selected_track.is_some(),
            can_move_up: up,
            can_move_down: down,
        }
    }

    /// Forgets what no longer exists (called after the rows are rebuilt).
    pub(super) fn org_sync(&mut self) {
        if self.og.group.is_some_and(|g| {
            self.song.org.group(g).is_none() || self.song.group_tracks(g).is_empty()
        }) {
            self.og.group = None;
        }
        let song = &self.song;
        self.og.extra.retain(|d| song.track(d).is_some());
        if self.selected_track.is_none() {
            self.og.extra.clear();
        }
        let gone = match self.og.edit.as_ref().map(|e| &e.target) {
            Some(EditTarget::Folder(g)) => song.org.group(*g).is_none(),
            Some(EditTarget::Comment(d) | EditTarget::Track(d)) => song.track(d).is_none(),
            Some(EditTarget::Lane(d, lane)) => song.track(d).is_none_or(|t| !t.is_note_lane(*lane)),
            None => false,
        };
        if gone {
            self.og.edit = None;
        }
    }

    // ------------------------------------------------------------------ layout

    /// The nesting depth and the hidden flag of every track, and the depth of every folder, from
    /// the song's outline.
    pub(super) fn org_shape(&mut self) {
        use atomik_model::OutlineKind;
        let outline = self.song.outline();
        self.og.shape = vec![(0, false); self.song.tracks.len()];
        self.og.folder_depth.clear();
        for e in outline {
            match e.kind {
                OutlineKind::Track(i) => self.og.shape[i] = (e.depth, e.hidden),
                OutlineKind::Folder(g) => {
                    self.og.folder_depth.insert(g, e.depth);
                }
            }
        }
    }

    /// The row height of lane `lane`: lane 0 and the note lanes have their track's height, an
    /// automation lane the lane height, a thin strip when it is folded.
    pub(super) fn lane_height_of(&self, track: usize, lane: usize) -> f64 {
        if let Some(t) = self
            .song
            .tracks
            .get(track)
            .filter(|t| lane == 0 || t.is_note_lane(lane))
        {
            self.track_row_height(&t.device)
        } else if self.is_lane_folded(track, lane) {
            FOLDED_LANE_HEIGHT
        } else {
            self.lane_row_height()
        }
    }

    /// Whether lane `lane` of track `track` is a folded automation lane.
    pub(super) fn is_lane_folded(&self, track: usize, lane: usize) -> bool {
        // The lane that follows the selected parameter stays open, whatever the project says.
        !self.is_followed(track, lane)
            && self
                .lane_id(track, lane)
                .is_some_and(|(d, p)| self.song.lane_folded(&d, &p))
    }

    /// The accent colour of a track: its own colour, else its device's.
    pub(super) fn track_accent(&self, doc: &RackDoc, track: usize) -> Color {
        let device = &self.song.tracks[track].device;
        self.song
            .track_color(device)
            .map_or_else(|| sp::device_accent(&self.st.base, doc, device), rgb)
    }

    /// The colour of a folder (its own, else a neutral).
    fn folder_accent(&self, group: u32) -> Color {
        self.song
            .org
            .group(group)
            .and_then(|g| g.color)
            .map_or(self.st.base.text_muted, rgb)
    }

    fn folder_depth(&self, group: u32) -> usize {
        self.og.folder_depth.get(&group).copied().unwrap_or(0)
    }

    /// The chevron of a folder header.
    pub(super) fn folder_chevron_rect(&self, hr: Rect, group: u32) -> Rect {
        let x = hr.x0 + 8.0 + self.folder_depth(group) as f64 * DEPTH_STEP;
        let cy = hr.y0 + hr.height() / 2.0;
        Rect::new(x, cy - 7.0, x + 14.0, cy + 7.0)
    }

    /// Where a folder's name starts and how wide it may be before the buttons.
    fn folder_name_box(&self, hr: Rect, group: u32) -> (f64, f64) {
        let x = self.folder_chevron_rect(hr, group).x1 + 5.0;
        (x, (hr.x0 + 108.0 - x).max(16.0))
    }

    /// The note mark of a track header with a comment, at the right end of its first line.
    fn comment_rect(&self, hr: Rect, track: usize) -> Option<Rect> {
        let device = &self.song.tracks.get(track)?.device;
        if self.song.track_comment(device).is_empty() {
            return None;
        }
        let cy = if hr.height() < super::COMPACT_BELOW {
            hr.y0 + hr.height() / 2.0
        } else {
            hr.y0 + 16.0
        };
        let x = hr.x0 + TRACK_LIST_WIDTH - 20.0;
        Some(Rect::new(x, cy - 7.0, x + 12.0, cy + 7.0))
    }

    /// The name of a track in its header row `hr`: where a double-click renames it. It is on
    /// the first line of the header, or the only one of a compact row.
    pub(super) fn track_name_rect(&self, hr: Rect, track: usize) -> Rect {
        let x = hr.x0 + 28.0 + self.track_indent(track);
        let cy = if hr.height() < super::COMPACT_BELOW {
            hr.y0 + hr.height() / 2.0
        } else {
            hr.y0 + 17.0
        };
        let w = self.track_name_width(hr, track) + 4.0;
        Rect::new(x, cy - 9.0, x + w, cy + 9.0)
    }

    /// How wide the name of a track may be in its header before the buttons and marks.
    pub(super) fn track_name_width(&self, hr: Rect, track: usize) -> f64 {
        let Some(tr) = self.song.tracks.get(track) else {
            return 0.0;
        };
        let base = if hr.height() < super::COMPACT_BELOW {
            76.0
        } else if self.song.track_comment(&tr.device).is_empty() {
            TRACK_LIST_WIDTH - 44.0
        } else {
            TRACK_LIST_WIDTH - 62.0
        };
        let frozen = if tr.frozen.is_some() { 16.0 } else { 0.0 };
        (base - frozen - self.track_indent(track)).max(16.0)
    }

    /// What a press on the name of a track header is, when it is on it.
    pub(super) fn track_name_hit(&self, hr: Rect, track: usize, pos: Point) -> Option<Hit> {
        self.track_name_rect(hr, track)
            .contains(pos)
            .then_some(Hit::TrackName(track))
    }

    /// The text field of the edit in progress: over the folder's name, or over the header of the
    /// track whose comment is typed.
    fn edit_rect(&self) -> Option<Rect> {
        let e = self.og.edit.as_ref()?;
        match &e.target {
            EditTarget::Folder(g) => {
                let row = self
                    .rows
                    .iter()
                    .find(|r| r.kind == RowKind::Folder { group: *g })?;
                let hr = self.row_rect(row);
                let (x, _) = self.folder_name_box(hr, *g);
                Some(Rect::new(
                    x - 3.0,
                    hr.y0 + 4.0,
                    hr.x0 + TRACK_LIST_WIDTH - 6.0,
                    hr.y1 - 4.0,
                ))
            }
            EditTarget::Lane(d, lane) => {
                let track = self.song.tracks.iter().position(|t| &t.device == d)?;
                let hr = self.row_rect(self.row_of(track, *lane)?);
                let (top, bottom) = if *lane == 0 {
                    (hr.y0 + 7.0, hr.y0 + 27.0)
                } else {
                    let cy = hr.y0 + hr.height() / 2.0;
                    (cy - 10.0, cy + 10.0)
                };
                Some(Rect::new(
                    hr.x0 + 24.0,
                    top,
                    hr.x0 + TRACK_LIST_WIDTH - 40.0,
                    bottom,
                ))
            }
            EditTarget::Track(d) => {
                let track = self.song.tracks.iter().position(|t| &t.device == d)?;
                let hr = self.row_rect(self.row_of(track, 0)?);
                let name = self.track_name_rect(hr, track);
                Some(Rect::new(
                    name.x0 - 3.0,
                    name.y0 - 1.0,
                    hr.x0 + TRACK_LIST_WIDTH - 6.0,
                    name.y1 + 1.0,
                ))
            }
            EditTarget::Comment(d) => {
                let track = self.song.tracks.iter().position(|t| &t.device == d)?;
                let hr = self.row_rect(self.row_of(track, 0)?);
                let cy = if hr.height() < super::COMPACT_BELOW {
                    hr.y0 + hr.height() / 2.0
                } else {
                    hr.y0 + 17.0
                };
                Some(Rect::new(
                    hr.x0 + 28.0,
                    cy - 10.0,
                    hr.x0 + TRACK_LIST_WIDTH - 6.0,
                    cy + 10.0,
                ))
            }
        }
    }

    // ------------------------------------------------------------------ points for hosts

    /// The centre of the name of a track's header (where a double-click renames the track).
    pub fn track_name_center(&self, device: &DeviceKey) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let hr = self.row_rect(self.row_of(track, 0)?);
        Some(self.track_name_rect(hr, track).center())
    }

    /// The centre of a folder row's header, away from its buttons (the name).
    pub fn folder_header_center(&self, group: u32) -> Option<Point> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Folder { group })?;
        let hr = self.row_rect(row);
        let (x, w) = self.folder_name_box(hr, group);
        Some(Point::new(x + w / 2.0, hr.center().y))
    }

    /// The centre of the chevron of a folder row.
    pub fn folder_toggle_center(&self, group: u32) -> Option<Point> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Folder { group })?;
        Some(self.folder_chevron_rect(self.row_rect(row), group).center())
    }

    /// The centre of a button of a folder row.
    pub fn folder_button_center(&self, group: u32, button: FolderButton) -> Option<Point> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Folder { group })?;
        Some(
            self.button_rect(self.row_rect(row), track_button(button))
                .center(),
        )
    }

    /// A point in the timeline part of a folder row at a song tick.
    pub fn folder_point(&self, group: u32, tick: i64) -> Option<Point> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Folder { group })?;
        Some(Point::new(
            self.axis.tick_to_x(tick),
            self.row_rect(row).center().y,
        ))
    }

    /// The centre of the note mark of a track with a comment.
    pub fn comment_mark_center(&self, device: &DeviceKey) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let hr = self.row_rect(self.row_of(track, 0)?);
        self.comment_rect(hr, track).map(|r| r.center())
    }

    /// The comment the tooltip shows (the pointer is on a note mark), if any.
    pub fn comment_tooltip(&self) -> Option<&str> {
        match self.hover {
            Some(Hit::Comment(track)) => self
                .song
                .tracks
                .get(track)
                .map(|t| self.song.track_comment(&t.device)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ hit testing

    /// What is under `pos` in the header of a folder row.
    pub(super) fn folder_hit(&self, group: u32, hr: Rect, pos: Point) -> Hit {
        if self.folder_chevron_rect(hr, group).contains(pos) {
            return Hit::FolderToggle(group);
        }
        for b in [FolderButton::Mute, FolderButton::Solo, FolderButton::Arm] {
            if self.button_rect(hr, track_button(b)).contains(pos) {
                return Hit::FolderButton(group, b);
            }
        }
        Hit::Folder(group)
    }

    /// What a press on a track header's note mark does: the hit, when it is on one.
    pub(super) fn comment_hit(&self, hr: Rect, track: usize, pos: Point) -> Option<Hit> {
        self.comment_rect(hr, track)
            .filter(|r| r.contains(pos))
            .map(|_| Hit::Comment(track))
    }

    // ------------------------------------------------------------------ gestures

    /// A press on a folder control. True when the hit was one.
    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    pub(super) fn folder_down(
        &mut self,
        hit: &Hit,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) -> bool {
        match hit {
            Hit::FolderToggle(g) if button == PointerButton::Primary => {
                let collapsed = !self.song.org.collapsed(*g);
                out.push(song_cmd(SongCommand::SetGroupCollapsed {
                    group: *g,
                    collapsed,
                }));
            }
            Hit::FolderButton(g, b) if button == PointerButton::Primary => {
                self.folder_button(*g, *b, out);
            }
            Hit::Folder(g) => {
                self.set_selected_group(Some(*g));
                if button == PointerButton::Secondary {
                    out.push(UiAction::ContextMenu {
                        target: ContextTarget::Folder,
                        at: pos,
                    });
                } else if clicks >= 2 && pos.x < self.axis.x0 {
                    self.begin_folder_rename(*g);
                } else if clicks >= 2 {
                    let collapsed = !self.song.org.collapsed(*g);
                    out.push(song_cmd(SongCommand::SetGroupCollapsed {
                        group: *g,
                        collapsed,
                    }));
                }
            }
            Hit::TrackName(track) => {
                self.track_name_down(*track, button, modifiers, clicks, pos, out);
            }
            Hit::Comment(track) if button == PointerButton::Primary => {
                if let Some(d) = self.song.tracks.get(*track).map(|t| t.device.clone()) {
                    self.set_selected_track(Some(d.clone()));
                    self.begin_comment_edit(&d);
                }
            }
            _ => return false,
        }
        true
    }

    /// A press on the name of a track header: it picks the track like any press on the header
    /// (Shift or Ctrl adds it to the selection, a right click opens the header menu), and a
    /// double-click with the primary button types the name.
    fn track_name_down(
        &mut self,
        track: usize,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        self.header_down(track, button, modifiers, pos, out);
        let plain = !(modifiers.shift || modifiers.command());
        if button == PointerButton::Primary && plain && clicks >= 2 {
            let device = self.song.tracks[track].device.clone();
            self.begin_track_rename(&device);
        }
    }

    /// M, S or R on a folder: every track of it in one batch.
    fn folder_button(&mut self, group: u32, b: FolderButton, out: &mut Vec<UiAction>) {
        let tracks = self.song.group_tracks(group);
        let flag = |t: &atomik_model::Track| match b {
            FolderButton::Mute => t.muted,
            FolderButton::Solo => t.solo,
            FolderButton::Arm => t.armed,
        };
        let set = !tracks.iter().all(|i| flag(&self.song.tracks[*i]));
        let commands: Vec<Command> = tracks
            .iter()
            .map(|i| {
                let t = &self.song.tracks[*i];
                let (mut muted, mut solo, mut armed) = (t.muted, t.solo, t.armed);
                match b {
                    FolderButton::Mute => muted = set,
                    FolderButton::Solo => solo = set,
                    FolderButton::Arm => armed = set,
                }
                Command::Song(SongCommand::SetTrackFlags {
                    device: t.device.clone(),
                    muted,
                    solo,
                    armed,
                })
            })
            .collect();
        if !commands.is_empty() {
            out.push(UiAction::Command(Command::Batch(commands)));
        }
    }

    /// The flags of a folder's buttons: lit when every track of a non-empty folder has them.
    fn folder_flags(&self, group: u32) -> [bool; 3] {
        let tracks = self.song.group_tracks(group);
        let all = |f: fn(&atomik_model::Track) -> bool| {
            !tracks.is_empty() && tracks.iter().all(|i| f(&self.song.tracks[*i]))
        };
        [all(|t| t.muted), all(|t| t.solo), all(|t| t.armed)]
    }

    // ------------------------------------------------------------------ typing

    /// Whether a name or a comment is being typed.
    pub(super) fn org_editing(&self) -> bool {
        self.og.edit.is_some()
    }

    /// The text being typed in the track list (a folder name or a comment), if any.
    pub fn org_edit_text(&self) -> Option<&str> {
        self.og.edit.as_ref().map(|e| e.text.as_str())
    }

    /// Starts typing a folder's name. False when there is no such folder.
    pub fn begin_folder_rename(&mut self, group: u32) -> bool {
        let Some(name) = self.song.org.group(group).map(|g| g.name.clone()) else {
            return false;
        };
        self.start_edit(EditTarget::Folder(group), name);
        true
    }

    /// Starts typing a track's name: the name of the device it plays, as the console renames a
    /// channel or a bus. False when there is no such track.
    pub fn begin_track_rename(&mut self, device: &DeviceKey) -> bool {
        if self.song.track(device).is_none() {
            return false;
        }
        let name = self
            .device_sig
            .iter()
            .find(|d| &d.0 == device)
            .map_or_else(|| device.0.clone(), |d| d.2.clone());
        self.start_edit(EditTarget::Track(device.clone()), name);
        true
    }

    /// Starts typing a track's comment. False when there is no such track.
    pub fn begin_comment_edit(&mut self, device: &DeviceKey) -> bool {
        if self.song.track(device).is_none() {
            return false;
        }
        // One line is typed; a comment of several shows its line breaks as slashes.
        let text = self.song.track_comment(device).replace('\n', " / ");
        self.start_edit(EditTarget::Comment(device.clone()), text);
        true
    }

    /// Starts typing the name of a note lane (`text` is what shows now).
    pub(super) fn begin_lane_edit(&mut self, device: DeviceKey, lane: usize, text: String) {
        self.start_edit(EditTarget::Lane(device, lane), text);
    }

    fn start_edit(&mut self, target: EditTarget, text: String) {
        self.og.edit = Some(TextEdit {
            target,
            original: text.clone(),
            text,
        });
        self.drag = Drag::None;
        self.mk.rename = None;
        self.menu = None;
        self.repaint = true;
    }

    /// Ends an open edit without changing anything.
    pub fn cancel_org_edit(&mut self) {
        if self.og.edit.take().is_some() {
            self.repaint = true;
        }
    }

    /// Ends the edit; committing emits the change when the text differs from what is there.
    fn finish_edit(&mut self, commit: bool, out: &mut Vec<UiAction>) {
        let Some(e) = self.og.edit.take() else {
            return;
        };
        self.repaint = true;
        if !commit || e.text == e.original {
            return;
        }
        let text = e.text.trim();
        match e.target {
            EditTarget::Folder(group) => {
                let name: String = text.chars().take(MAX_GROUP_NAME).collect();
                let name = name.trim_end().to_owned();
                if name.is_empty() || self.song.org.group(group).is_none_or(|g| g.name == name) {
                    return;
                }
                out.push(song_cmd(SongCommand::RenameGroup { group, name }));
            }
            EditTarget::Lane(device, lane) => {
                out.extend(self.lane_rename_command(&device, lane, text));
            }
            EditTarget::Track(key) => {
                // A blank name cancels; the model checks the rest (a bus name must be unique).
                if text.is_empty() {
                    return;
                }
                out.push(UiAction::Command(Command::Rename {
                    key,
                    name: text.to_owned(),
                }));
            }
            EditTarget::Comment(device) => {
                let comment: String = text.chars().take(MAX_TRACK_COMMENT).collect();
                let comment = comment.trim_end().to_owned();
                if self.song.track_comment(&device) == comment {
                    return;
                }
                out.push(song_cmd(SongCommand::SetTrackComment { device, comment }));
            }
        }
    }

    /// A key while a text is typed.
    pub(super) fn org_edit_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        match key {
            Key::Enter => self.finish_edit(true, out),
            Key::Escape => self.finish_edit(false, out),
            _ if modifiers.command() => {}
            Key::Backspace => {
                if let Some(e) = &mut self.og.edit {
                    e.text.pop();
                }
            }
            Key::Space => self.edit_push(' '),
            Key::Char(c) if !c.is_control() => self.edit_push(*c),
            _ => {}
        }
        self.repaint = true;
    }

    fn edit_push(&mut self, c: char) {
        if let Some(e) = &mut self.og.edit {
            let max = match e.target {
                EditTarget::Folder(_) => MAX_GROUP_NAME,
                EditTarget::Comment(_) => MAX_TRACK_COMMENT,
                EditTarget::Lane(..) => MAX_LANE_NAME_CHARS,
                EditTarget::Track(_) => MAX_TRACK_NAME_CHARS,
            };
            if e.text.chars().count() < max {
                e.text.push(c);
            }
        }
    }

    /// A press while a text is typed: inside the field it keeps typing, anywhere else it
    /// commits. True when the press was inside the field (and is used up).
    pub(super) fn org_edit_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        if self.og.edit.is_none() {
            return false;
        }
        if self.edit_rect().is_some_and(|r| r.contains(pos)) {
            return true;
        }
        self.finish_edit(true, out);
        false
    }

    // ------------------------------------------------------------------ commands

    /// The organisation commands of the registry. `None` for a command that is not one of them.
    pub(super) fn org_command(&mut self, id: &str, arg: Option<&str>) -> Option<CommandOutcome> {
        let outcome = match id {
            "sequencer.group_tracks" => {
                let devices = self.group_candidates();
                if devices.is_empty() {
                    return Some(CommandOutcome::NotApplicable(
                        "Select the tracks to group first".into(),
                    ));
                }
                let name = auto_name(
                    "Folder",
                    self.song.org.groups.iter().map(|g| g.name.as_str()),
                );
                CommandOutcome::Handled(vec![song_cmd(SongCommand::GroupTracks { devices, name })])
            }
            "sequencer.ungroup_tracks" => match self.target_folder() {
                Some(group) => {
                    self.og.group = None;
                    CommandOutcome::Handled(vec![song_cmd(SongCommand::Ungroup { group })])
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.toggle_folder" => match self.target_folder() {
                Some(group) => {
                    CommandOutcome::Handled(vec![song_cmd(SongCommand::SetGroupCollapsed {
                        group,
                        collapsed: !self.song.org.collapsed(group),
                    })])
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.rename_folder" => {
                let group = self.og.group;
                match group {
                    Some(group) if self.begin_folder_rename(group) => {
                        CommandOutcome::Handled(Vec::new())
                    }
                    _ => CommandOutcome::Declined,
                }
            }
            "sequencer.rename_track" => match self.selected_track.clone() {
                Some(d) if self.begin_track_rename(&d) => CommandOutcome::Handled(Vec::new()),
                _ => CommandOutcome::Declined,
            },
            "sequencer.track_comment" => match self.selected_track.clone() {
                Some(d) if self.begin_comment_edit(&d) => CommandOutcome::Handled(Vec::new()),
                _ => CommandOutcome::Declined,
            },
            "sequencer.move_track_up" | "sequencer.move_track_down" => {
                let down = id == "sequencer.move_track_down";
                let command = match (self.og.group, self.selected_track.clone()) {
                    (Some(group), _) => self
                        .song
                        .can_move_group(group, down)
                        .then_some(SongCommand::MoveGroup { group, down }),
                    (None, Some(device)) => self
                        .song
                        .can_move_track(&device, down)
                        .then_some(SongCommand::MoveTrack { device, down }),
                    (None, None) => return Some(CommandOutcome::Declined),
                };
                match command {
                    Some(c) => CommandOutcome::Handled(vec![song_cmd(c)]),
                    None => CommandOutcome::Handled(Vec::new()),
                }
            }
            "sequencer.track_color" => {
                let Some(color) = arg.and_then(palette_color) else {
                    return Some(CommandOutcome::Declined);
                };
                if let Some(group) = self.og.group {
                    CommandOutcome::Handled(vec![song_cmd(SongCommand::SetGroupColor {
                        group,
                        color,
                    })])
                } else {
                    let tracks = self.selected_tracks();
                    if tracks.is_empty() {
                        return Some(CommandOutcome::Declined);
                    }
                    let commands = tracks
                        .into_iter()
                        .map(|device| Command::Song(SongCommand::SetTrackColor { device, color }))
                        .collect();
                    CommandOutcome::Handled(vec![UiAction::Command(Command::Batch(commands))])
                }
            }
            "sequencer.fold_lanes" => {
                let folded = match arg {
                    Some("fold") => true,
                    Some("unfold") => false,
                    _ => return Some(CommandOutcome::Declined),
                };
                let selected = self.selected_tracks();
                let commands: Vec<Command> = self
                    .song
                    .tracks
                    .iter()
                    .filter(|t| selected.is_empty() || selected.contains(&t.device))
                    .flat_map(|t| {
                        t.lanes.iter().filter_map(|l| match &l.kind {
                            atomik_model::LaneKind::Automation(p)
                                if self.song.lane_folded(&t.device, p) != folded =>
                            {
                                Some(Command::Song(SongCommand::SetLaneFolded {
                                    device: t.device.clone(),
                                    param: p.clone(),
                                    folded,
                                }))
                            }
                            _ => None,
                        })
                    })
                    .collect();
                if commands.is_empty() {
                    CommandOutcome::Handled(Vec::new())
                } else {
                    CommandOutcome::Handled(vec![UiAction::Command(Command::Batch(commands))])
                }
            }
            "sequencer.track_height" => match arg.and_then(HeightPreset::from_arg) {
                Some(preset) => CommandOutcome::Handled(self.height_preset(preset)),
                None => CommandOutcome::Declined,
            },
            _ => return None,
        };
        Some(outcome)
    }

    /// The song command that applies a height preset to the selected tracks, or to every track
    /// when none is selected (nothing when no height would change).
    pub fn height_preset(&self, preset: HeightPreset) -> Vec<UiAction> {
        let selected = self.selected_tracks();
        let targets: Vec<usize> = self
            .song
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| selected.is_empty() || selected.contains(&t.device))
            .map(|(i, _)| i)
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        let mut map = self.song.track_heights.clone();
        let fit = (preset == HeightPreset::Fit).then(|| self.fit_height(&targets));
        for i in targets {
            let device = self.song.tracks[i].device.clone();
            match preset {
                HeightPreset::Small => map.insert(device, HEIGHT_SMALL),
                HeightPreset::Medium => map.remove(&device),
                HeightPreset::Large => map.insert(device, HEIGHT_LARGE),
                HeightPreset::Fit => map.insert(device, fit.unwrap_or(HEIGHT_LARGE)),
            };
        }
        if map == self.song.track_heights {
            return Vec::new();
        }
        vec![song_cmd(SongCommand::SetTrackHeights(map))]
    }

    /// The base height that makes the shown target tracks fill the view: what is left of the
    /// view after the rows that are not their first lanes, shared among them, at zoom 1.
    fn fit_height(&self, targets: &[usize]) -> u16 {
        let shown: Vec<&Row> = targets.iter().filter_map(|t| self.row_of(*t, 0)).collect();
        if shown.is_empty() {
            return atomik_model::DEFAULT_TRACK_HEIGHT;
        }
        let avail = (self.bounds.y1 - self.lanes_top() - HSCROLL_HEIGHT).max(0.0);
        let fixed: f64 = self.content_height - shown.iter().map(|r| r.h).sum::<f64>();
        let each = (avail - fixed) / shown.len() as f64 / self.th.zoom;
        each.round()
            .clamp(f64::from(MIN_TRACK_HEIGHT), f64::from(MAX_TRACK_HEIGHT)) as u16
    }

    // ------------------------------------------------------------------ painting

    /// The header of a folder row.
    pub(super) fn paint_folder_header(&self, c: &mut dyn Canvas, hr: Rect, group: u32) {
        let st = &self.st;
        let t = &st.base;
        let Some(g) = self.song.org.group(group) else {
            return;
        };
        let accent = self.folder_accent(group);
        let depth = self.folder_depth(group);
        sp::rect(c, hr, mix(st.header_track, accent, 0.18));
        sp::line(c, hr.x0, hr.y0 + 0.5, hr.x1, hr.y0 + 0.5, t.bevel_light);
        sp::rect(
            c,
            Rect::new(
                hr.x0 + depth as f64 * DEPTH_STEP,
                hr.y0,
                hr.x0 + depth as f64 * DEPTH_STEP + 4.0,
                hr.y1,
            ),
            accent,
        );
        let ctr = self.folder_chevron_rect(hr, group).center();
        let mut chev = BezPath::new();
        if g.collapsed {
            chev.move_to(pt(ctr.x - 2.0, ctr.y - 4.0));
            chev.line_to(pt(ctr.x + 3.0, ctr.y));
            chev.line_to(pt(ctr.x - 2.0, ctr.y + 4.0));
        } else {
            chev.move_to(pt(ctr.x - 4.0, ctr.y - 2.0));
            chev.line_to(pt(ctr.x + 4.0, ctr.y - 2.0));
            chev.line_to(pt(ctr.x, ctr.y + 3.0));
        }
        chev.close_path();
        let hot = self.hover == Some(Hit::FolderToggle(group));
        c.fill_with(chev, if hot { t.text } else { t.text_muted });
        let (nx, nw) = self.folder_name_box(hr, group);
        let name = fit_label(&g.name, nw, 11.0);
        c.text(&TextRun::new(name, pt(nx, hr.center().y + 4.0), 11.0, t.text).weight(600));
        let flags = self.folder_flags(group);
        let hovered = |b| self.hover == Some(Hit::FolderButton(group, b));
        for (b, label, on, lit) in [
            (FolderButton::Mute, "M", flags[0], st.mute),
            (FolderButton::Solo, "S", flags[1], st.solo),
            (FolderButton::Arm, "R", flags[2], st.record),
        ] {
            sp::button(
                c,
                st,
                self.button_rect(hr, track_button(b)),
                label,
                on.then_some(lit),
                hovered(b),
            );
        }
        if self.og.group == Some(group) {
            c.stroke_with(
                RoundedRect::from_rect(hr.inset(-1.0), 1.0),
                1.2,
                st.selection,
            );
        }
    }

    /// The timeline part of a folder row: the clips of its tracks as thin bars.
    pub(super) fn paint_folder_lane(
        &self,
        c: &mut dyn Canvas,
        doc: &RackDoc,
        lr: Rect,
        group: u32,
    ) {
        let st = &self.st;
        sp::rect(c, lr, mix(st.lane_alt, self.folder_accent(group), 0.12));
        sp::grid(c, st, &self.axis, lr, &self.bars);
        for i in self.song.group_tracks(group) {
            let accent = self.track_accent(doc, i);
            let track = &self.song.tracks[i];
            // Every note lane (or the audio lane) counts; the automation lanes do not.
            let lanes = track.lanes.iter().take(track.note_lane_count().max(1));
            for lane in lanes {
                for clip in &lane.clips {
                    let (x0, x1) = (
                        self.axis.tick_to_x(clip.start),
                        self.axis.tick_to_x(clip.start + clip.length),
                    );
                    if x1 < lr.x0 - 2.0 || x0 > lr.x1 + 2.0 {
                        continue;
                    }
                    let r = Rect::new(x0, lr.y0 + 7.0, x1.max(x0 + 1.0), lr.y1 - 7.0);
                    c.fill_with(
                        RoundedRect::from_rect(r, 2.0),
                        accent.with_alpha(if clip.muted || lane.muted { 0.25 } else { 0.7 }),
                    );
                }
            }
        }
    }

    /// The marks a track header gets from the organisation: the folder guides at its left edge,
    /// the selection outline and the note mark of a comment.
    pub(super) fn paint_track_marks(&self, c: &mut dyn Canvas, hr: Rect, track: usize) {
        let st = &self.st;
        let t = &st.base;
        let (depth, _) = self.og.shape.get(track).copied().unwrap_or((0, false));
        let device = &self.song.tracks[track].device;
        if depth > 0 {
            let mut chain = self.song.track_group(device);
            let mut guides = Vec::new();
            while let Some(g) = chain {
                guides.push(self.folder_accent(g));
                chain = self.song.org.group(g).and_then(|g| g.parent);
            }
            for (i, col) in guides.into_iter().take(2).enumerate() {
                let x = hr.x0 + 5.0 + 3.0 * i as f64;
                sp::rect(c, Rect::new(x, hr.y0, x + 2.0, hr.y1), col);
            }
        }
        if self.selected_track.as_ref() == Some(device) || self.og.extra.contains(device) {
            c.stroke_with(
                RoundedRect::from_rect(hr.inset(-1.0), 1.0),
                1.2,
                st.selection,
            );
        }
        // A track whose channel is in a VCA group: a slim bar in the group's colour at the right
        // edge of the header.
        if let Some(color) = self.vca_chips.get(device) {
            let x = hr.x0 + super::TRACK_LIST_WIDTH;
            sp::rect(
                c,
                Rect::new(x - 5.0, hr.y0 + 2.0, x - 2.0, hr.y1 - 2.0),
                *color,
            );
        }
        if let Some(r) = self.comment_rect(hr, track) {
            let hot = self.hover == Some(Hit::Comment(track));
            let col = if hot { t.text } else { t.text_muted };
            let body = Rect::new(r.x0 + 1.0, r.y0 + 1.0, r.x1 - 1.0, r.y1 - 1.0);
            c.stroke_with(RoundedRect::from_rect(body, 1.5), 1.0, col);
            for k in 0..3 {
                let y = body.y0 + 3.5 + 3.0 * f64::from(k);
                sp::line(c, body.x0 + 2.5, y, body.x1 - 2.5, y, col);
            }
        }
    }

    /// The fold chevron at the left of an automation lane header.
    pub(super) fn paint_lane_fold(&self, c: &mut dyn Canvas, hr: Rect, track: usize, lane: usize) {
        let t = &self.st.base;
        let r = self.lane_button_rect(hr, track, LaneButton::Fold);
        let ctr = r.center();
        let folded = self.is_lane_folded(track, lane);
        let mut chev = BezPath::new();
        if folded {
            chev.move_to(pt(ctr.x - 2.0, ctr.y - 3.5));
            chev.line_to(pt(ctr.x + 2.5, ctr.y));
            chev.line_to(pt(ctr.x - 2.0, ctr.y + 3.5));
        } else {
            chev.move_to(pt(ctr.x - 3.5, ctr.y - 2.0));
            chev.line_to(pt(ctr.x + 3.5, ctr.y - 2.0));
            chev.line_to(pt(ctr.x, ctr.y + 2.5));
        }
        chev.close_path();
        let hot = self.hover == Some(Hit::LaneButton(track, lane, LaneButton::Fold));
        c.fill_with(chev, if hot { t.text } else { t.text_muted });
    }

    /// The header of a folded automation lane: the chevron, a bar in the track's colour and the
    /// parameter's name on one thin line.
    pub(super) fn paint_folded_lane_header(
        &self,
        c: &mut dyn Canvas,
        hr: Rect,
        track: usize,
        lane: usize,
        accent: Color,
    ) {
        let st = &self.st;
        let x = hr.x0 + self.track_indent(track);
        sp::rect(c, hr, st.header_lane);
        sp::rect(
            c,
            Rect::new(x + 16.0, hr.y0 + 3.0, x + 18.0, hr.y1 - 3.0),
            accent.with_alpha(0.7),
        );
        self.paint_lane_fold(c, hr, track, lane);
        let name = fit_label(self.param_of(track, lane).name, 150.0 - (x - hr.x0), 9.0);
        sp::label(
            c,
            &name,
            x + 26.0,
            hr.center().y + 3.0,
            9.0,
            st.base.text_muted,
            Align::Start,
        );
    }

    /// The timeline part of a folded lane: its clips as thin bars.
    pub(super) fn paint_folded_lane(
        &self,
        c: &mut dyn Canvas,
        rr: Rect,
        track: usize,
        lane: usize,
        accent: Color,
    ) {
        let muted = self.song.tracks[track].muted;
        for clip in &self.song.tracks[track].lanes[lane].clips {
            let x0 = self.axis.tick_to_x(clip.start);
            let x1 = self.axis.tick_to_x(clip.start + clip.length);
            let r = Rect::new(x0, rr.y0 + 5.0, x1.max(x0 + 1.0), rr.y1 - 5.0);
            c.fill_with(
                RoundedRect::from_rect(r, 2.0),
                accent.with_alpha(if muted || clip.muted { 0.2 } else { 0.5 }),
            );
        }
    }

    /// What goes over everything else: the field of the text being typed and the comment tooltip.
    pub(super) fn paint_org_overlay(&self, c: &mut dyn Canvas) {
        if let (Some(e), Some(r)) = (&self.og.edit, self.edit_rect()) {
            super::markers::name_field(c, &self.st, r, &e.text);
        } else if let Some(text) = self.comment_tooltip()
            && let Some(Hit::Comment(track)) = self.hover
            && let Some(row) = self.row_of(track, 0)
            && let Some(r) = self.comment_rect(self.row_rect(row), track)
        {
            let first = text.lines().next().unwrap_or("");
            let mut shown: String = first.chars().take(TIP_CHARS).collect();
            if shown.chars().count() < first.chars().count() || text.contains('\n') {
                shown.push('\u{2026}');
            }
            let at = Point::new(r.x1 - 4.0, r.y1 + 26.0);
            tooltip(c, &self.st.base, at, &shown);
        }
    }
}

/// The track header button a folder button sits where.
pub(super) fn track_button(b: FolderButton) -> TrackButton {
    match b {
        FolderButton::Mute => TrackButton::Mute,
        FolderButton::Solo => TrackButton::Solo,
        FolderButton::Arm => TrackButton::Arm,
    }
}
