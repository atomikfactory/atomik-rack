//! Note lanes in the [`ArrangementView`]: a track holds one or more note lanes that feed its
//! device together (`atomik_model::Lane`, `Track::note_lanes`).
//!
//! * **Rows.** Note lane 0 is the track's header row, followed by its take rows. Each further
//!   note lane has a row of the same height with its own take rows, and stays when the track
//!   collapses (only the automation lanes fold away). The automation lanes come last.
//! * **Headers.** A lane from 1 on has a header with its label and an M button (`SetLaneMuted`).
//!   With more than one lane, the header of lane 0 gets a small chip with the same two controls
//!   on the second line, hidden when the row is [`COMPACT_BELOW`] short. A muted lane's clips are
//!   painted like the clips of a muted track.
//! * **The selected lane** ([`ArrangementView::record_lane`]) is the lane a click on its header,
//!   on its empty part or on one of its clips picks. It is where a recording, a paste and an
//!   import land, and what the lane commands act on. It is view state, not saved, and it is
//!   clamped when lanes go away.
//! * **Rename.** A double-click on a lane's label, or `sequencer.rename_note_lane`, types the
//!   name in place with the editor of the folder names; Enter commits (`RenameLane`, a blank
//!   name puts the default back) and Escape cancels.

use super::{ArrangementView, COMPACT_BELOW, Hit, LaneButton, TRACK_LIST_WIDTH, song_cmd};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::keymap::CommandOutcome;
use crate::paint::fit_label;
use crate::seq_paint as sp;
use atomik_model::{DeviceKey, LaneKind, MAX_LANE_NAME_CHARS, MAX_NOTE_LANES, SongCommand};
use vello::kurbo::{Point, Rect, RoundedRect};

/// What the track header menu needs to know about the selected lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneMenu {
    /// A note lane can be added to the selected track.
    pub can_add: bool,
    /// The selected lane can be renamed.
    pub can_rename: bool,
    /// The selected lane can be deleted (the track has more than one note lane).
    pub can_delete: bool,
    /// The selected lane is muted (the Mute item is checked).
    pub muted: bool,
    /// An automation lane header was pressed last: the lane items below act on it.
    pub automation: bool,
    /// That automation lane is muted (the Mute Automation Lane item is checked).
    pub automation_muted: bool,
    /// That automation lane has a static value (Clear Static Value is on).
    pub automation_static: bool,
}

impl ArrangementView {
    /// The note lane of `device`'s track that recordings, pastes and imports land on: the lane
    /// last picked, kept within the lanes that exist, 0 without a pick.
    pub fn record_lane(&self, device: &DeviceKey) -> usize {
        let Some(t) = self.song.track(device) else {
            return 0;
        };
        let last = t.note_lane_count().saturating_sub(1);
        self.note_lane_sel.get(device).map_or(0, |l| (*l).min(last))
    }

    /// The selected track and its selected note lane, if a track is selected.
    pub fn selected_lane(&self) -> Option<(&DeviceKey, usize)> {
        let d = self.selected_track.as_ref()?;
        self.song.track(d)?;
        Some((d, self.record_lane(d)))
    }

    /// Picks note lane `lane` of `device`'s track as its selected lane (view state).
    pub fn set_selected_lane(&mut self, device: &DeviceKey, lane: usize) {
        let ok = self
            .song
            .track(device)
            .is_some_and(|t| t.is_note_lane(lane));
        if ok && self.note_lane_sel.get(device) != Some(&lane) {
            self.note_lane_sel.insert(device.clone(), lane);
            self.repaint = true;
        }
    }

    /// Picks the lane of a row of track `track` when it is a note lane.
    pub(super) fn pick_note_lane(&mut self, track: usize, lane: usize) {
        if let Some(d) = self.song.tracks.get(track).map(|t| t.device.clone()) {
            self.set_selected_lane(&d, lane);
        }
    }

    /// Drops the picks of tracks that are gone and keeps the others inside their lanes.
    pub(super) fn lanes_sync(&mut self) {
        let song = &self.song;
        self.note_lane_sel.retain(|d, _| song.track(d).is_some());
        for (d, lane) in &mut self.note_lane_sel {
            let last = song
                .track(d)
                .map_or(0, |t| t.note_lane_count().saturating_sub(1));
            *lane = (*lane).min(last);
        }
    }

    /// Whether row `lane` of track `track` is a note lane after the first (it has a header of
    /// its own).
    pub(super) fn is_extra_note_lane(&self, track: usize, lane: usize) -> bool {
        lane > 0 && self.is_note_lane(track, lane)
    }

    /// Whether lane `lane` of track `track` is a note lane.
    pub(super) fn is_note_lane(&self, track: usize, lane: usize) -> bool {
        self.song
            .tracks
            .get(track)
            .is_some_and(|t| t.is_note_lane(lane))
    }

    /// Whether the header of lane 0 of `track` shows the lane chip: the track has several note
    /// lanes and the row has room for a second line.
    pub(super) fn has_lane_chip(&self, hr: Rect, track: usize) -> bool {
        hr.height() >= COMPACT_BELOW
            && self
                .song
                .tracks
                .get(track)
                .is_some_and(|t| t.note_lane_count() > 1)
    }

    /// The rectangle of the M button of lane `lane` in its header row `hr`.
    pub(super) fn lane_mute_rect(&self, hr: Rect, lane: usize) -> Rect {
        if lane == 0 {
            let x = hr.x0 + TRACK_LIST_WIDTH;
            Rect::new(x - 22.0, hr.y0 + 33.0, x - 6.0, hr.y0 + 49.0)
        } else {
            let (x, cy) = (hr.x0 + TRACK_LIST_WIDTH, hr.y0 + hr.height() / 2.0);
            Rect::new(x - 34.0, cy - 8.0, x - 14.0, cy + 8.0)
        }
    }

    /// The label area of lane `lane`'s header (a double-click renames, a click picks the lane).
    pub(super) fn lane_label_rect(&self, hr: Rect, track: usize, lane: usize) -> Rect {
        if lane == 0 {
            let x = hr.x0 + TRACK_LIST_WIDTH;
            Rect::new(x - 46.0, hr.y0 + 33.0, x - 24.0, hr.y0 + 49.0)
        } else {
            Rect::new(
                hr.x0 + 18.0 + self.track_indent(track),
                hr.y0,
                self.lane_mute_rect(hr, lane).x0 - 2.0,
                hr.y1,
            )
        }
    }

    /// What a press at `pos` in the header of note lane `lane` of `track` is, for the controls
    /// that belong to the lane: its M button and its label.
    pub(super) fn note_lane_header_hit(
        &self,
        hr: Rect,
        track: usize,
        lane: usize,
        pos: Point,
    ) -> Option<Hit> {
        if lane == 0 && !self.has_lane_chip(hr, track) {
            return None;
        }
        if self.lane_mute_rect(hr, lane).contains(pos) {
            return Some(Hit::LaneButton(track, lane, LaneButton::Mute));
        }
        self.lane_label_rect(hr, track, lane)
            .contains(pos)
            .then_some(Hit::NoteLaneHeader(track, lane))
    }

    /// A press on the label of a note lane header: picks the lane and the track; a double-click
    /// types the lane's name, a right click opens the track header menu.
    pub(super) fn note_lane_header_down(
        &mut self,
        track: usize,
        lane: usize,
        secondary: bool,
        clicks: u8,
        pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        let device = self.song.tracks[track].device.clone();
        self.set_selected_track(Some(device.clone()));
        self.set_selected_lane(&device, lane);
        if secondary {
            out.push(UiAction::ContextMenu {
                target: crate::action::ContextTarget::TrackHeader,
                at: pos,
            });
        } else if clicks >= 2 {
            self.begin_lane_rename(&device, lane);
        }
    }

    /// Centre of the label of note lane `lane` of `device`'s track (`None` without that lane or
    /// without a label on screen: lane 0 of a track with one lane).
    pub fn note_lane_header_center(&self, device: &DeviceKey, lane: usize) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let hr = self.row_rect(self.row_of(track, lane)?);
        let hr = Rect::new(self.bounds.x0, hr.y0, self.axis.x0, hr.y1);
        if lane == 0 && !self.has_lane_chip(hr, track) {
            return None;
        }
        self.is_note_lane(track, lane)
            .then(|| self.lane_label_rect(hr, track, lane).center())
    }

    /// Centre of a control in the header of note lane `lane` of `device`'s track: the M button
    /// (`None` when the lane has none: lane 0 of a track with one lane, or in a compact row).
    pub fn note_lane_button_center(
        &self,
        device: &DeviceKey,
        lane: usize,
        button: LaneButton,
    ) -> Option<Point> {
        if button != LaneButton::Mute {
            return None;
        }
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let hr = self.row_rect(self.row_of(track, lane)?);
        let hr = Rect::new(self.bounds.x0, hr.y0, self.axis.x0, hr.y1);
        if !self.is_note_lane(track, lane) || (lane == 0 && !self.has_lane_chip(hr, track)) {
            return None;
        }
        Some(self.lane_mute_rect(hr, lane).center())
    }

    /// The M button of a lane header: mutes the lane, or unmutes it.
    pub(super) fn lane_mute_button(&self, track: usize, lane: usize, out: &mut Vec<UiAction>) {
        let Some(t) = self.song.tracks.get(track) else {
            return;
        };
        let Some(l) = t
            .lanes
            .get(lane)
            .filter(|l| t.is_note_lane(lane) || matches!(l.kind, LaneKind::Automation(_)))
        else {
            return;
        };
        out.push(song_cmd(SongCommand::SetLaneMuted {
            device: t.device.clone(),
            lane,
            muted: !l.muted,
        }));
    }

    // ------------------------------------------------------------------ rename

    /// Starts typing the name of note lane `lane` of `device`'s track. False without such a
    /// lane (an automation lane has no name to type) or on a frozen track.
    pub fn begin_lane_rename(&mut self, device: &DeviceKey, lane: usize) -> bool {
        let Some(t) = self.song.track(device) else {
            return false;
        };
        if !t.is_note_lane(lane) || t.frozen.is_some() {
            return false;
        }
        let label = t.lane_label(lane);
        self.begin_lane_edit(device.clone(), lane, label);
        true
    }

    /// The commit of a typed lane name: `RenameLane` when it changes the name, a blank text
    /// puts the default back.
    pub(super) fn lane_rename_command(
        &self,
        device: &DeviceKey,
        lane: usize,
        text: &str,
    ) -> Option<UiAction> {
        let t = self.song.track(device)?;
        let current = t.lanes.get(lane).filter(|_| t.is_note_lane(lane))?;
        let name: String = text.trim().chars().take(MAX_LANE_NAME_CHARS).collect();
        let name = name.trim_end();
        let name = (!name.is_empty()).then(|| name.to_owned());
        if current.name == name {
            return None;
        }
        Some(song_cmd(SongCommand::RenameLane {
            device: device.clone(),
            lane,
            name,
        }))
    }

    // ------------------------------------------------------------------ menu and commands

    /// What the "Add Note Lane", "Rename Note Lane...", "Delete Note Lane" and "Mute Lane"
    /// items of the track header menu need: the state of the selected track and lane.
    pub fn lane_menu(&self) -> LaneMenu {
        let Some((d, lane)) = self.selected_lane() else {
            return self.automation_menu();
        };
        let Some(t) = self.song.track(d) else {
            return self.automation_menu();
        };
        if t.is_audio() || t.is_automation_only() || t.frozen.is_some() {
            return self.automation_menu();
        }
        LaneMenu {
            can_add: t.note_lane_count() < MAX_NOTE_LANES,
            can_rename: true,
            can_delete: t.note_lane_count() > 1,
            muted: t.lanes.get(lane).is_some_and(|l| l.muted),
            ..self.automation_menu()
        }
    }

    /// The automation part of [`LaneMenu`].
    fn automation_menu(&self) -> LaneMenu {
        let (automation, automation_muted, automation_static) = self.automation_lane_menu();
        LaneMenu {
            automation,
            automation_muted,
            automation_static,
            ..LaneMenu::default()
        }
    }

    /// The automation lane whose header was pressed last, if it still exists: what the lane items
    /// of the track header menu act on (`sequencer.delete_automation_lane` and the like).
    pub fn picked_automation_lane(&self) -> Option<&(DeviceKey, String)> {
        let lane = self.picked_lane.as_ref()?;
        self.song.track(&lane.0)?.automation_lane(&lane.1)?;
        Some(lane)
    }

    /// The state of the picked automation lane for the track header menu (all false without
    /// one).
    pub fn automation_lane_menu(&self) -> (bool, bool, bool) {
        let Some((d, p)) = self.picked_automation_lane() else {
            return (false, false, false);
        };
        let lane = self
            .song
            .track(d)
            .and_then(|t| t.automation_lane(p).and_then(|i| t.lanes.get(i)));
        (
            true,
            lane.is_some_and(|l| l.muted),
            lane.is_some_and(|l| l.static_value.is_some()),
        )
    }

    /// The lane commands of the registry. `None` for a command that is not one of them.
    pub(super) fn lane_command(&mut self, id: &str) -> Option<CommandOutcome> {
        if !matches!(
            id,
            "sequencer.add_note_lane"
                | "sequencer.rename_note_lane"
                | "sequencer.delete_note_lane"
                | "sequencer.toggle_lane_mute"
        ) {
            return None;
        }
        let Some((device, lane)) = self.selected_lane().map(|(d, l)| (d.clone(), l)) else {
            return Some(CommandOutcome::Declined);
        };
        let menu = self.lane_menu();
        let allowed = match id {
            "sequencer.add_note_lane" => menu.can_add,
            "sequencer.rename_note_lane" => menu.can_rename,
            "sequencer.delete_note_lane" => menu.can_delete,
            _ => menu.can_rename,
        };
        if !allowed {
            return Some(CommandOutcome::Declined);
        }
        let command = match id {
            "sequencer.add_note_lane" => {
                // The new lane comes last among the note lanes: it becomes the selected one.
                let at = self.song.track(&device).map_or(0, |t| t.note_lane_count());
                self.note_lane_sel.insert(device.clone(), at);
                SongCommand::AddNoteLane { device, name: None }
            }
            "sequencer.rename_note_lane" => {
                self.begin_lane_rename(&device, lane);
                return Some(CommandOutcome::Handled(Vec::new()));
            }
            "sequencer.delete_note_lane" => SongCommand::RemoveNoteLane { device, lane },
            _ => SongCommand::SetLaneMuted {
                device,
                lane,
                muted: !menu.muted,
            },
        };
        Some(CommandOutcome::Handled(vec![song_cmd(command)]))
    }

    // ------------------------------------------------------------------ painting

    /// The header of a note lane after the first: accent bar, label, M button.
    pub(super) fn paint_note_lane_header(
        &self,
        c: &mut dyn Canvas,
        hr: Rect,
        track: usize,
        lane: usize,
        accent: vello::peniko::Color,
    ) {
        let st = &self.st;
        let t = &st.base;
        let tr = &self.song.tracks[track];
        let x = hr.x0 + self.track_indent(track);
        sp::rect(c, hr, st.header_lane);
        sp::rect(
            c,
            Rect::new(x + 16.0, hr.y0 + 6.0, x + 18.0, hr.y1 - 6.0),
            accent.with_alpha(0.7),
        );
        let label = fit_label(
            &tr.lane_label(lane),
            self.lane_label_rect(hr, track, lane).width(),
            10.0,
        );
        let cy = hr.y0 + hr.height() / 2.0 + 3.5;
        sp::label(c, &label, x + 26.0, cy, 10.0, t.text, Align::Start);
        self.paint_lane_mute(c, hr, track, lane);
        if self.selected_note_lane(track) == Some(lane) {
            c.stroke_with(
                RoundedRect::from_rect(hr.inset(-1.5), 2.0),
                1.5,
                st.selection,
            );
        }
    }

    /// The chip on the header of lane 0 of a track with several note lanes: its label and M.
    pub(super) fn paint_lane_chip(&self, c: &mut dyn Canvas, hr: Rect, track: usize) {
        if !self.has_lane_chip(hr, track) {
            return;
        }
        let st = &self.st;
        let label = self.lane_label_rect(hr, track, 0);
        let selected = self.selected_note_lane(track) == Some(0);
        let hot = self.hover == Some(Hit::NoteLaneHeader(track, 0));
        let shape = RoundedRect::from_rect(label, 2.0);
        let col = if selected {
            st.selection
        } else {
            st.base.text_muted
        };
        c.stroke_with(shape, 1.0, if hot { st.base.text } else { col });
        let text = fit_label(
            &self.song.tracks[track].lane_label(0),
            label.width() - 4.0,
            7.5,
        );
        sp::label(
            c,
            &text,
            label.center().x,
            label.center().y + 3.0,
            7.5,
            if selected {
                st.base.text
            } else {
                st.base.text_muted
            },
            Align::Center,
        );
        self.paint_lane_mute(c, hr, track, 0);
    }

    fn paint_lane_mute(&self, c: &mut dyn Canvas, hr: Rect, track: usize, lane: usize) {
        let st = &self.st;
        let muted = self.song.tracks[track].lanes[lane].muted;
        let hovered = self.hover == Some(Hit::LaneButton(track, lane, LaneButton::Mute));
        sp::button(
            c,
            st,
            self.lane_mute_rect(hr, lane),
            "M",
            muted.then_some(st.mute),
            hovered,
        );
    }

    /// The selected note lane of a track when the lane picker is meaningful (several lanes).
    fn selected_note_lane(&self, track: usize) -> Option<usize> {
        let t = self.song.tracks.get(track)?;
        (t.note_lane_count() > 1).then(|| self.record_lane(&t.device))
    }
}
