//! The Key lane and the Chord lane of the [`ArrangementView`]: two rows above the track lanes, under
//! the tempo and energy lanes, that show the song's key changes and chords (see
//! [`atomik_model::harmony`]) and edit them.
//!
//! The lanes are view options, off by default. They scroll with the timeline and take the whole
//! width; the track lanes sit under them.
//!
//! * **Key lane.** Every key change is a tag at its tick with the key's name; the song key is the
//!   tag at the start of the song. A click on the empty lane adds a key change at the snapped tick
//!   (the key already in effect there, so nothing sounds different until you change it), a drag of
//!   a tag moves it, a double click opens the picker, the right button opens a menu with Delete and
//!   Copy (and Paste on the empty lane).
//! * **Chord lane.** Every chord is a box from its start to its end with its symbol. A click on
//!   the empty lane adds the tonic chord of the key there, a bar long (shorter when the next chord
//!   is nearer), and a drag from that press sets its length. A drag of a box moves it, a drag of
//!   its right end resizes it (neither lets a chord run into the next one), a double click opens
//!   the picker, the right button opens the menu.
//!
//! Every step of a drag is one `DragHarmony` with one merge id and the drag ends with
//! [`UiAction::EndMerge`], so a gesture is one undo step; Escape puts the lane back. Delete
//! removes the selected item.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, Hit, MIN_TICKS, Menu, MenuKind, song_cmd};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Modifiers, PointerButton};
use crate::harmony_picker::HarmonyTarget;
use crate::keymap::CommandOutcome;
use crate::paint::{estimate_width, fit_label};
use crate::seq_edit;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::snap_round;
use atomik_model::harmony::{ChordEvent, Harmony, Key};
use atomik_model::theory::ScaleKind;
use atomik_model::{DeviceKey, SongCommand};
use vello::kurbo::{Point, Rect, RoundedRect};

/// Height of the key lane.
pub const KEY_LANE_HEIGHT: f64 = 20.0;
/// Height of the chord lane.
pub const CHORD_LANE_HEIGHT: f64 = 24.0;
/// The width of the grab zone at the right end of a chord.
const EDGE: f64 = 6.0;
/// The font of the labels in the lanes.
const FONT: f32 = 9.0;

/// What of the harmony lanes lies under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum HarmonyHit {
    /// The song key's tag at the start of the song.
    SongKey,
    /// The tag of the key change at this tick.
    KeyTag(i64),
    /// The key lane away from its tags.
    KeyEmpty,
    /// The chord that starts at this tick, and the part of it under the pointer.
    Chord(i64, ChordPart),
    /// The chord lane away from its chords.
    ChordEmpty,
}

/// A part of a chord box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChordPart {
    /// The middle: a drag moves the chord.
    Body,
    /// The right end: a drag resizes the chord.
    End,
}

/// What is selected in the lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sel {
    SongKey,
    Change(i64),
    Chord(i64),
}

/// What Copy took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Copied {
    Key(Key),
    Chord(ChordEvent),
}

/// What a row of a lane menu does.
#[derive(Debug, Clone)]
pub(super) enum HarmonyRow {
    /// Send this action.
    Do(Box<UiAction>),
    /// Keep a key or chord to paste later.
    Copy(Copied),
    /// Turn "Play Chords on Click" on or off.
    TogglePlay,
}

/// What a drag moves.
#[derive(Debug, Clone, Copy)]
enum Target {
    Change {
        from: i64,
    },
    Chord {
        from: i64,
        original: ChordEvent,
        part: ChordPart,
    },
}

/// A running drag.
#[derive(Debug, Clone)]
struct HarmonyDrag {
    target: Target,
    /// The harmony before the press (Escape puts it back).
    origin: Harmony,
    /// The harmony with the dragged item at its starting place (every step starts from it).
    base: Harmony,
    press_tick: i64,
    press: Point,
    merge: u64,
    moved: bool,
    emitted: bool,
    last: Harmony,
}

/// The state of the lanes.
#[derive(Debug, Clone, Default)]
pub(super) struct HarmonyUi {
    pub key_lane: bool,
    pub chord_lane: bool,
    sel: Option<Sel>,
    drag: Option<HarmonyDrag>,
    copied: Option<Copied>,
    /// What the first press of a double click added (the second press opens its picker).
    added: Option<Sel>,
    /// "Play Chords on Click" is off (it is on by default).
    silent_clicks: bool,
}

/// The key a new key change starts with when the song has none in effect.
fn default_key() -> Key {
    Key::new(0, ScaleKind::Major)
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the key lane is shown above the track lanes.
    pub fn key_lane_visible(&self) -> bool {
        self.hl.key_lane
    }

    /// Shows or hides the key lane.
    pub fn set_key_lane(&mut self, on: bool) {
        if on != self.hl.key_lane {
            self.hl.key_lane = on;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Whether the chord lane is shown above the track lanes.
    pub fn chord_lane_visible(&self) -> bool {
        self.hl.chord_lane
    }

    /// Shows or hides the chord lane.
    pub fn set_chord_lane(&mut self, on: bool) {
        if on != self.hl.chord_lane {
            self.hl.chord_lane = on;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The key lane's rectangle (empty when it is hidden).
    pub fn key_lane_rect(&self) -> Rect {
        self.key_rect()
    }

    /// The chord lane's rectangle (empty when it is hidden).
    pub fn chord_lane_rect(&self) -> Rect {
        self.chord_rect()
    }

    /// The centre of the tag of the key change at `tick` (the song key's tag for tick 0 when the
    /// song has no change there).
    pub fn key_tag_center(&self, tick: i64) -> Option<Point> {
        self.key_tag_rect(tick).map(|r| r.center())
    }

    /// The centre of the chord that starts at `tick`.
    pub fn chord_center(&self, tick: i64) -> Option<Point> {
        self.chord_box(self.song.harmony.chord_index(tick)?)
            .map(|r| r.center())
    }

    /// The right end of the chord that starts at `tick`, as a point a press resizes it from.
    pub fn chord_end_handle(&self, tick: i64) -> Option<Point> {
        let r = self.chord_box(self.song.harmony.chord_index(tick)?)?;
        Some(Point::new(r.x1 - EDGE / 2.0, r.center().y))
    }

    /// The tick of the selected key change or chord.
    pub fn selected_harmony_tick(&self) -> Option<i64> {
        match self.hl.sel? {
            Sel::SongKey => None,
            Sel::Change(t) | Sel::Chord(t) => Some(t),
        }
    }

    // ------------------------------------------------------------------ geometry

    /// The bottom of the harmony lanes (the energy lane's bottom when both are hidden).
    pub(super) fn harmony_bottom(&self) -> f64 {
        self.chord_bottom()
    }

    fn key_bottom(&self) -> f64 {
        self.energy_bottom()
            + if self.hl.key_lane {
                KEY_LANE_HEIGHT
            } else {
                0.0
            }
    }

    fn chord_bottom(&self) -> f64 {
        self.key_bottom()
            + if self.hl.chord_lane {
                CHORD_LANE_HEIGHT
            } else {
                0.0
            }
    }

    /// The key lane over the timeline (empty height when hidden).
    pub(super) fn key_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.energy_bottom(),
            self.bounds.x1.max(self.axis.x0),
            self.key_bottom(),
        )
    }

    /// The chord lane over the timeline (empty height when hidden).
    pub(super) fn chord_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.key_bottom(),
            self.bounds.x1.max(self.axis.x0),
            self.chord_bottom(),
        )
    }

    /// The name a key tag shows.
    fn key_text(key: Key) -> String {
        key.name()
    }

    /// The tag of the key change at `tick`; for tick 0 without a change there, the song key's tag.
    fn key_tag_rect(&self, tick: i64) -> Option<Rect> {
        let lane = self.key_rect();
        if lane.height() <= 0.0 {
            return None;
        }
        let h = &self.song.harmony;
        let key = match h.change_index(tick) {
            Some(i) => h.changes[i].key,
            None if tick == 0 => h.key?,
            None => return None,
        };
        let x = self.axis.tick_to_x(tick);
        let w = estimate_width(&Self::key_text(key), FONT) + 12.0;
        let r = Rect::new(x + 1.0, lane.y0 + 2.0, x + 1.0 + w, lane.y1 - 2.0);
        (r.x1 > lane.x0 && r.x0 < lane.x1).then_some(r)
    }

    /// The box of chord `i`, cut to the timeline.
    fn chord_box(&self, i: usize) -> Option<Rect> {
        let lane = self.chord_rect();
        if lane.height() <= 0.0 {
            return None;
        }
        let c = self.song.harmony.chords.get(i)?;
        let (x0, x1) = (self.axis.tick_to_x(c.tick), self.axis.tick_to_x(c.end()));
        let r = Rect::new(
            x0 + 0.5,
            lane.y0 + 2.0,
            (x1 - 0.5).max(x0 + 1.5),
            lane.y1 - 2.0,
        );
        (r.x1 > lane.x0 && r.x0 < lane.x1).then_some(r)
    }

    /// What lies under `pos`, which is inside the key or chord lane.
    pub(super) fn harmony_hit(&self, pos: Point) -> Hit {
        let (key, chord) = (self.key_rect(), self.chord_rect());
        if pos.x < self.axis.x0 {
            return Hit::Empty;
        }
        let h = &self.song.harmony;
        if key.height() > 0.0 && pos.y >= key.y0 && pos.y < key.y1 {
            for c in h.changes.iter().rev() {
                if self.key_tag_rect(c.tick).is_some_and(|r| r.contains(pos)) {
                    return Hit::Harmony(HarmonyHit::KeyTag(c.tick));
                }
            }
            if self.key_tag_rect(0).is_some_and(|r| r.contains(pos)) {
                return Hit::Harmony(HarmonyHit::SongKey);
            }
            return Hit::Harmony(HarmonyHit::KeyEmpty);
        }
        if chord.height() > 0.0 && pos.y >= chord.y0 && pos.y < chord.y1 {
            for (i, c) in h.chords.iter().enumerate() {
                let Some(r) = self.chord_box(i) else { continue };
                if !r.contains(pos) {
                    continue;
                }
                let zone = EDGE.min(r.width() / 3.0);
                let end_in_view = r.x1 <= chord.x1;
                let part = if pos.x >= r.x1 - zone && end_in_view {
                    ChordPart::End
                } else {
                    ChordPart::Body
                };
                return Hit::Harmony(HarmonyHit::Chord(c.tick, part));
            }
            return Hit::Harmony(HarmonyHit::ChordEmpty);
        }
        Hit::Empty
    }

    // ------------------------------------------------------------------ gestures

    /// The snapped tick under `pos`, never before the start of the song.
    fn harmony_tick(&self, pos: Point, modifiers: Modifiers) -> i64 {
        snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0)
    }

    fn harmony_select(&mut self, sel: Option<Sel>) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.tempo_sel = None;
        self.meter_sel = None;
        self.mk.sel = None;
        self.og.group = None;
        self.hl.sel = sel;
    }

    /// A press on a lane.
    pub(super) fn harmony_down(
        &mut self,
        part: HarmonyHit,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let tick = self.harmony_tick(pos, modifiers);
        let h = self.song.harmony.clone();
        if button == PointerButton::Secondary {
            self.harmony_menu(&part, tick, pos);
            return;
        }
        if button != PointerButton::Primary {
            return;
        }
        let double = clicks >= 2;
        if !double {
            self.hl.added = None;
        }
        match part {
            HarmonyHit::SongKey => {
                self.harmony_select(Some(Sel::SongKey));
                if double {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::SongKey));
                }
            }
            HarmonyHit::KeyTag(at) => {
                self.harmony_select(Some(Sel::Change(at)));
                if double {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::KeyChange {
                        tick: at,
                    }));
                } else {
                    self.harmony_start(Target::Change { from: at }, h.clone(), h, pos, tick);
                }
            }
            HarmonyHit::KeyEmpty => {
                if let (true, Some(Sel::Change(at))) = (double, self.hl.added) {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::KeyChange {
                        tick: at,
                    }));
                    return;
                }
                let key = h.key_at(tick).unwrap_or_else(default_key);
                let Ok(next) = h.with_key_change(tick, Some(key)) else {
                    return;
                };
                self.harmony_select(Some(Sel::Change(tick)));
                self.hl.added = Some(Sel::Change(tick));
                self.harmony_start(Target::Change { from: tick }, h, next, pos, tick);
                if double {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::KeyChange { tick }));
                }
            }
            HarmonyHit::Chord(at, chord_part) => {
                let Some(i) = h.chord_index(at) else { return };
                let original = h.chords[i];
                self.harmony_select(Some(Sel::Chord(at)));
                if double {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::Chord {
                        tick: at,
                        length: original.length,
                    }));
                } else {
                    if !self.hl.silent_clicks {
                        out.push(UiAction::AuditionChord(original));
                    }
                    let target = Target::Chord {
                        from: at,
                        original,
                        part: chord_part,
                    };
                    self.harmony_start(target, h.clone(), h, pos, self.axis.x_to_tick(pos.x));
                }
            }
            HarmonyHit::ChordEmpty => {
                if let (true, Some(Sel::Chord(at))) = (double, self.hl.added) {
                    let length = h
                        .chord_index(at)
                        .map_or(self.default_chord_length(at), |i| h.chords[i].length);
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::Chord {
                        tick: at,
                        length,
                    }));
                    return;
                }
                let want = self.default_chord_length(tick);
                let Some(length) = h.room_for_chord(tick, want, None) else {
                    return;
                };
                let chord = h.suggested_chord(tick, length);
                let Ok(next) = h.with_chord(chord) else {
                    return;
                };
                self.harmony_select(Some(Sel::Chord(tick)));
                self.hl.added = Some(Sel::Chord(tick));
                if !self.hl.silent_clicks && !double {
                    out.push(UiAction::AuditionChord(chord));
                }
                let target = Target::Chord {
                    from: tick,
                    original: chord,
                    part: ChordPart::End,
                };
                // The drag sets the end: it is measured from the end the chord starts with.
                self.harmony_start(target, h, next.clone(), pos, tick + length);
                if double {
                    out.push(UiAction::HarmonyPicker(HarmonyTarget::Chord {
                        tick,
                        length,
                    }));
                }
            }
        }
        // A pending add is sent as the first step of its drag.
        self.harmony_flush_add(out);
    }

    /// A chord added by a press is one bar long: up to the next bar line.
    fn default_chord_length(&self, tick: i64) -> i64 {
        (self.bars.next_bar_line(tick) - tick).max(MIN_TICKS)
    }

    /// Starts a drag. `base` is the harmony the drag works from (the item at its starting place),
    /// `origin` what Escape goes back to. A press that added an item has `base != origin`: the add
    /// is sent as the drag's first step.
    fn harmony_start(
        &mut self,
        target: Target,
        origin: Harmony,
        base: Harmony,
        pos: Point,
        press_tick: i64,
    ) {
        let merge = self.merge_id();
        self.hl.drag = Some(HarmonyDrag {
            target,
            origin,
            last: base.clone(),
            base,
            press_tick,
            press: pos,
            merge,
            moved: false,
            emitted: false,
        });
        self.drag = Drag::Harmony;
    }

    /// Sends the harmony an add produced, as the first step of the drag it started.
    fn harmony_flush_add(&mut self, out: &mut Vec<UiAction>) {
        let Some(d) = &mut self.hl.drag else { return };
        if d.emitted || d.base == d.origin {
            return;
        }
        d.emitted = true;
        out.push(song_cmd(SongCommand::DragHarmony {
            harmony: d.base.clone(),
            merge: d.merge,
        }));
    }

    /// Pointer motion during a drag: one command per step that changes something.
    pub(super) fn harmony_move(
        &mut self,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some(d) = self.hl.drag.clone() else {
            return;
        };
        if !d.moved && pos.distance(d.press) <= DRAG_THRESHOLD {
            return;
        }
        let grid = self.step(modifiers);
        let min = seq_edit::min_length(self.step(Modifiers::NONE), MIN_TICKS);
        let delta = self.axis.x_to_tick(pos.x) - d.press_tick;
        let (next, selected) = match d.target {
            Target::Change { from } => {
                let to = snap_round(from + delta, grid).max(0);
                (d.base.with_key_change_moved(from, to), Sel::Change(to))
            }
            Target::Chord {
                from,
                original,
                part: ChordPart::Body,
            } => {
                let tick = snap_round(original.tick + delta, grid).max(0);
                let chord = ChordEvent { tick, ..original };
                (d.base.with_chord_replaced(from, chord), Sel::Chord(tick))
            }
            Target::Chord {
                from,
                original,
                part: ChordPart::End,
            } => {
                let end = snap_round(original.end() + delta, grid);
                let want = (end - original.tick).max(min);
                let length = d
                    .base
                    .room_for_chord(original.tick, want, Some(original.tick))
                    .unwrap_or(original.length);
                let chord = ChordEvent { length, ..original };
                (
                    d.base.with_chord_replaced(from, chord),
                    Sel::Chord(original.tick),
                )
            }
        };
        let Some(live) = &mut self.hl.drag else {
            return;
        };
        live.moved = true;
        self.repaint = true;
        let Ok(next) = next else { return };
        if next == live.last {
            return;
        }
        live.last = next.clone();
        live.emitted = true;
        let merge = live.merge;
        self.hl.sel = Some(selected);
        out.push(song_cmd(SongCommand::DragHarmony {
            harmony: next,
            merge,
        }));
    }

    /// The release: the steps of the drag become one undo step.
    pub(super) fn harmony_up(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.hl.drag.take()
            && d.emitted
        {
            out.push(UiAction::EndMerge);
        }
    }

    /// Escape during a drag: the lanes go back to what they were before the press.
    pub(super) fn harmony_cancel(&mut self, out: &mut Vec<UiAction>) {
        self.drag = Drag::None;
        let Some(d) = self.hl.drag.take() else { return };
        if !d.emitted {
            return;
        }
        self.hl.sel = None;
        out.push(song_cmd(SongCommand::DragHarmony {
            harmony: d.origin,
            merge: d.merge,
        }));
        out.push(UiAction::EndMerge);
    }

    /// The right button: the menu of the item (or of the empty lane) under `pos`.
    fn harmony_menu(&mut self, part: &HarmonyHit, tick: i64, pos: Point) {
        let h = &self.song.harmony;
        let set = |h: Result<Harmony, String>| {
            h.ok()
                .map(|h| HarmonyRow::Do(Box::new(song_cmd(SongCommand::SetHarmony(h)))))
        };
        let mut rows: Vec<(String, HarmonyRow)> = Vec::new();
        let mut push = |label: &str, row: Option<HarmonyRow>| {
            if let Some(row) = row {
                rows.push((label.to_owned(), row));
            }
        };
        match part {
            HarmonyHit::SongKey => {
                self.hl.sel = Some(Sel::SongKey);
                push("Clear Song Key", set(h.with_key(None)));
                push("Copy Key", h.key.map(|k| HarmonyRow::Copy(Copied::Key(k))));
            }
            HarmonyHit::KeyTag(at) => {
                self.hl.sel = Some(Sel::Change(*at));
                push("Delete Key Change", set(h.with_key_change(*at, None)));
                let key = h.change_index(*at).map(|i| h.changes[i].key);
                push("Copy Key", key.map(|k| HarmonyRow::Copy(Copied::Key(k))));
            }
            HarmonyHit::KeyEmpty => {
                let key = h.key_at(tick).unwrap_or_else(default_key);
                push(
                    "Add Key Change Here",
                    set(h.with_key_change(tick, Some(key))),
                );
                if let Some(Copied::Key(k)) = self.hl.copied {
                    push("Paste Key Change", set(h.with_key_change(tick, Some(k))));
                }
            }
            HarmonyHit::Chord(at, _) => {
                self.hl.sel = Some(Sel::Chord(*at));
                push("Delete Chord", set(Ok(h.without_chord(*at))));
                let chord = h.chord_index(*at).map(|i| h.chords[i]);
                push(
                    "Copy Chord",
                    chord.map(|c| HarmonyRow::Copy(Copied::Chord(c))),
                );
            }
            HarmonyHit::ChordEmpty => {
                let want = self.default_chord_length(tick);
                if let Some(length) = h.room_for_chord(tick, want, None) {
                    push(
                        "Add Chord Here",
                        set(h.with_chord(h.suggested_chord(tick, length))),
                    );
                }
                if let Some(Copied::Chord(c)) = self.hl.copied
                    && let Some(length) = h.room_for_chord(tick, c.length, None)
                {
                    push(
                        "Paste Chord",
                        set(h.with_chord(ChordEvent { tick, length, ..c })),
                    );
                }
            }
        }
        // The chord lane's own switch, last: whether a click on a chord plays it.
        let chord_lane = matches!(part, HarmonyHit::Chord(..) | HarmonyHit::ChordEmpty);
        if chord_lane {
            rows.push(("Play Chords on Click".to_owned(), HarmonyRow::TogglePlay));
        }
        self.repaint = true;
        if rows.is_empty() {
            return;
        }
        let checked = rows
            .iter()
            .map(|(_, r)| matches!(r, HarmonyRow::TogglePlay) && !self.hl.silent_clicks)
            .collect();
        let items = rows
            .iter()
            .enumerate()
            .map(|(i, (label, _))| (i.to_string(), label.clone()))
            .collect();
        let rows = rows.into_iter().map(|(_, r)| r).collect();
        let mut menu = Menu::new(
            DeviceKey(String::new()),
            MenuKind::Harmony(rows),
            items,
            pos,
            0,
        );
        menu.checked = checked;
        self.open_menu(menu);
    }

    /// A row of a lane menu was picked.
    pub(super) fn harmony_row(&mut self, row: &HarmonyRow, out: &mut Vec<UiAction>) {
        match row {
            HarmonyRow::Do(action) => out.push((**action).clone()),
            HarmonyRow::Copy(item) => self.hl.copied = Some(*item),
            HarmonyRow::TogglePlay => self.hl.silent_clicks = !self.hl.silent_clicks,
        }
    }

    /// Whether a click on a chord of the lane plays it ("Play Chords on Click", on by default).
    pub fn plays_chords_on_click(&self) -> bool {
        !self.hl.silent_clicks
    }

    /// `edit.delete` removes the selected key change or chord (the song key: clears it); `edit.copy`
    /// keeps it to paste from the lane's menu.
    pub(super) fn harmony_command(&mut self, id: &str) -> Option<CommandOutcome> {
        let sel = self.hl.sel?;
        let h = &self.song.harmony;
        match id {
            "edit.delete" => {
                let next = match sel {
                    Sel::SongKey => h.with_key(None),
                    Sel::Change(t) => h.with_key_change(t, None),
                    Sel::Chord(t) => Ok(h.without_chord(t)),
                }
                .ok()?;
                self.hl.sel = None;
                Some(CommandOutcome::Handled(vec![song_cmd(
                    SongCommand::SetHarmony(next),
                )]))
            }
            "edit.copy" => {
                self.hl.copied = match sel {
                    Sel::SongKey => h.key.map(Copied::Key),
                    Sel::Change(t) => h.change_index(t).map(|i| Copied::Key(h.changes[i].key)),
                    Sel::Chord(t) => h.chord_index(t).map(|i| Copied::Chord(h.chords[i])),
                };
                Some(CommandOutcome::Handled(Vec::new()))
            }
            _ => None,
        }
    }

    /// A press anywhere but on a lane lets go of the selection.
    pub(super) fn harmony_deselect(&mut self) {
        self.hl.sel = None;
    }

    /// Forgets a selection whose item is gone.
    pub(super) fn harmony_synced(&mut self) {
        let h = &self.song.harmony;
        let alive = match self.hl.sel {
            None => true,
            Some(Sel::SongKey) => h.key.is_some(),
            Some(Sel::Change(t)) => h.change_index(t).is_some(),
            Some(Sel::Chord(t)) => h.chord_index(t).is_some(),
        };
        if !alive {
            self.hl.sel = None;
        }
        if self.hl.drag.is_none() && matches!(self.drag, Drag::Harmony) {
            self.drag = Drag::None;
        }
    }

    // ------------------------------------------------------------------ painting

    /// The lanes and their header cells.
    pub(super) fn paint_harmony_lanes(&self, c: &mut dyn Canvas) {
        self.paint_key_lane(c);
        self.paint_chord_lane(c);
    }

    fn paint_lane_frame(&self, c: &mut dyn Canvas, rect: Rect, name: &str, line: &str) {
        let st = &self.st;
        let t = &st.base;
        let header = Rect::new(self.bounds.x0, rect.y0, rect.x0, rect.y1);
        sp::rect(c, header, st.header_lane);
        sp::label(
            c,
            name,
            header.x0 + 12.0,
            header.y0 + 14.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        sp::label(
            c,
            &fit_label(line, header.width() - 60.0, 10.0),
            header.x1 - 8.0,
            header.y0 + 14.0,
            10.0,
            t.highlight,
            Align::End,
        );
        sp::rect(c, rect, st.lane_auto);
        sp::line(
            c,
            header.x0,
            rect.y1 - 0.5,
            rect.x1,
            rect.y1 - 0.5,
            st.separator,
        );
        sp::line(
            c,
            rect.x0 - 0.5,
            rect.y0,
            rect.x0 - 0.5,
            rect.y1,
            st.separator,
        );
    }

    fn paint_key_lane(&self, c: &mut dyn Canvas) {
        let rect = self.key_rect();
        if rect.height() <= 0.0 {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let h = &self.song.harmony;
        let line = h
            .key_at(self.playhead)
            .map_or_else(|| "none".to_owned(), Key::short);
        self.paint_lane_frame(c, rect, "KEY", &line);
        c.push_clip(&rect.into());
        let mut tags: Vec<(i64, Key, bool)> = Vec::new();
        if let (Some(k), None) = (h.key, h.change_index(0)) {
            tags.push((0, k, true));
        }
        tags.extend(h.changes.iter().map(|ch| (ch.tick, ch.key, false)));
        for (tick, key, song) in tags {
            let Some(r) = self.key_tag_rect(tick) else {
                continue;
            };
            let selected = self.hl.sel
                == Some(if song {
                    Sel::SongKey
                } else {
                    Sel::Change(tick)
                });
            let hovered = self.hover
                == Some(Hit::Harmony(if song {
                    HarmonyHit::SongKey
                } else {
                    HarmonyHit::KeyTag(tick)
                }));
            let fill = if selected {
                mix(t.highlight, t.plate, 0.55)
            } else if hovered {
                mix(t.focus, t.plate, 0.7)
            } else if song {
                mix(t.text_muted, t.plate, 0.85)
            } else {
                mix(t.text_muted, t.plate, 0.7)
            };
            if !song {
                let x = self.axis.tick_to_x(tick);
                sp::line(c, x + 0.5, rect.y0, x + 0.5, rect.y1, t.text_muted);
            }
            c.fill_with(RoundedRect::from_rect(r, 3.0), fill);
            sp::label(
                c,
                &Self::key_text(key),
                r.x0 + 6.0,
                r.center().y + 3.0,
                FONT,
                t.text,
                Align::Start,
            );
        }
        c.pop_clip();
    }

    fn paint_chord_lane(&self, c: &mut dyn Canvas) {
        let rect = self.chord_rect();
        if rect.height() <= 0.0 {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let h = &self.song.harmony;
        let line = h
            .chord_at(self.playhead)
            .map_or_else(|| "-".to_owned(), ChordEvent::symbol);
        self.paint_lane_frame(c, rect, "CHORDS", &line);
        c.push_clip(&rect.into());
        for (i, chord) in h.chords.iter().enumerate() {
            let Some(r) = self.chord_box(i) else { continue };
            let selected = self.hl.sel == Some(Sel::Chord(chord.tick));
            let hovered = matches!(&self.hover, Some(Hit::Harmony(HarmonyHit::Chord(t0, _))) if *t0 == chord.tick);
            let fill = if selected {
                mix(t.highlight, t.plate, 0.5)
            } else if hovered {
                mix(t.focus, t.plate, 0.65)
            } else {
                mix(t.highlight, t.plate, 0.78)
            };
            c.fill_with(RoundedRect::from_rect(r, 3.0), fill);
            if selected {
                c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.0, t.highlight);
            }
            c.push_clip(&r.inset(-1.0).into());
            sp::label(
                c,
                &fit_label(&chord.symbol(), r.width() - 8.0, FONT + 1.0),
                r.x0 + 5.0,
                r.center().y + 3.5,
                FONT + 1.0,
                t.text,
                Align::Start,
            );
            c.pop_clip();
        }
        c.pop_clip();
    }
}
