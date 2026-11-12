//! Vocal mode of the audio clip editor: the notes of a monophonic clip on a pitch grid, and their
//! pitch edits (see the parent module for the interaction).
//!
//! The notes come from the app's analysis ([`AudioEditorView::set_vocal_notes`]), in asset frames;
//! the edits are the clip's [`PitchEdit`]s, one per note, matched by their span. The drawn result
//! curve is what the render makes of the edit ([`note_offsets`], the same function the render
//! uses), so the picture is the sound.

use super::{AudioEditorView, Drag};
use crate::action::UiAction;
use atomik_dsp::stretch::psola::{NoteShift, VocalNote, note_offsets};
use atomik_model::pitch_edit::{with_pitch_edit, without_pitch_edits};
use atomik_model::theory::Scale;
use atomik_model::{
    AudioClipContent, ClipId, Command, PITCH_EDIT_MAX_CENTS, PitchEdit, SongCommand, WarpSettings,
};
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

/// Width of the piano keys left of the pitch grid.
pub(super) const KEYS_W: f64 = 36.0;
/// The fewest semitone rows the grid shows.
const MIN_ROWS: f32 = 14.0;
/// Semitones of room above and below the notes.
const ROOM: f32 = 2.0;
/// The flatten amount at start, in percent.
const DEFAULT_FLATTEN: u8 = 100;
/// Pixels of vertical drag per percent of the flatten field.
const FLATTEN_PX: f64 = 1.5;

/// The analysis of the open clip as the host reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Analysis {
    /// Nothing asked yet (or the clip changed).
    None,
    /// The worker runs: the fraction done.
    Running(f32),
    /// The notes are in.
    Ready,
}

/// The Vocal mode state of the editor.
#[derive(Debug)]
pub(super) struct Vocal {
    /// The notes of the open clip, in asset frames.
    pub(super) notes: Vec<VocalNote>,
    pub(super) analysis: Analysis,
    /// Selected notes, by index in `notes`.
    pub(super) selected: BTreeSet<usize>,
    /// Correct snaps to the song key's scale.
    pub(super) scale: bool,
    /// The flatten amount Flatten applies, in percent.
    pub(super) flatten: u8,
    /// Per note: the semitones the edit adds at each entry of its pitch curve.
    pub(super) offsets: Vec<Vec<f32>>,
    /// The song key at the clip start, as a scale.
    pub(super) key: Option<Scale>,
    /// The keys the grid shows, bottom and top (fractional, a row per semitone).
    pub(super) range: (f32, f32),
}

impl Default for Vocal {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            analysis: Analysis::None,
            selected: BTreeSet::new(),
            scale: false,
            flatten: DEFAULT_FLATTEN,
            offsets: Vec::new(),
            key: None,
            range: (48.0, 72.0),
        }
    }
}

/// A gesture on a note.
#[derive(Debug, Clone)]
pub(super) struct NoteDrag {
    /// The note pressed.
    pub(super) index: usize,
    pub(super) start: Point,
    /// Which way it moves, once it has moved far enough.
    pub(super) axis: Option<Axis>,
    /// The clip's content at the press: every move is computed from it.
    pub(super) orig: AudioClipContent,
    /// Drag in cents instead of semitones.
    pub(super) free: bool,
    pub(super) merge: u64,
    pub(super) emitted: bool,
    /// The last command sent, to send only changes.
    pub(super) last: Option<Command>,
}

/// The way a note drag goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Pitch,
    Time,
}

/// A rubber band on the grid.
#[derive(Debug, Clone)]
pub(super) struct Band {
    pub(super) start: Point,
    pub(super) now: Point,
    /// The selection the band adds to (Shift held), else empty.
    pub(super) base: BTreeSet<usize>,
}

/// The name of a key, `C#4` (middle C is C3, like the rest of the program).
pub(super) fn key_name(key: f32) -> String {
    atomik_model::note_name(key.round().clamp(0.0, 127.0) as u8)
}

impl Vocal {
    /// The note's centre pitch, or its key when the analysis found none.
    pub(super) fn centre(&self, i: usize) -> f32 {
        let n = &self.notes[i];
        n.pitch.centre.unwrap_or(f32::from(n.key))
    }
}

/// The edit of the note `n` in `edits` (the one on its span), or a fresh one.
pub(super) fn edit_of(edits: &[PitchEdit], n: &VocalNote) -> PitchEdit {
    edits
        .iter()
        .find(|e| e.src_start == n.start && e.src_end == n.end)
        .copied()
        .unwrap_or_else(|| PitchEdit::new(n.start, n.end))
}

/// What the render does with `e` on a curve.
pub(super) fn shift_of(e: &PitchEdit) -> NoteShift {
    NoteShift {
        start: e.src_start,
        end: e.src_end,
        shift_cents: e.shift_cents,
        drift: f32::from(e.drift) / 100.0,
        vibrato_keep: e.vibrato_keep,
    }
}

/// The nearest key to `pitch` in `scale` (any key without one).
pub(super) fn nearest_key(pitch: f32, scale: Option<&Scale>) -> f32 {
    let base = pitch.round();
    let Some(scale) = scale else {
        return base;
    };
    let mut best = base;
    let mut dist = f32::MAX;
    for d in -6..=6 {
        let k = base + d as f32;
        if (0.0..=127.0).contains(&k) && scale.contains(k as u8) && (k - pitch).abs() < dist {
            dist = (k - pitch).abs();
            best = k;
        }
    }
    best
}

impl AudioEditorView {
    // ------------------------------------------------------------------ host API

    /// Supplies the notes the analysis found in the open clip `clip` (asset frames). An empty
    /// list says the clip has no pitched note. A result for another clip is ignored.
    pub fn set_vocal_notes(&mut self, clip: ClipId, notes: Vec<VocalNote>) {
        if self.clip != Some(clip) {
            return;
        }
        self.vocal.notes = notes;
        self.vocal.analysis = Analysis::Ready;
        self.vocal.selected.clear();
        self.refresh_vocal(true);
        self.repaint = true;
    }

    /// Tells how far the analysis is (`Some(0..=1)`), or that it stopped without a result
    /// (`None` before any notes arrived).
    pub fn set_vocal_progress(&mut self, progress: Option<f32>) {
        let next = match progress {
            Some(p) => Analysis::Running(p.clamp(0.0, 1.0)),
            None if self.vocal.analysis == Analysis::Ready => Analysis::Ready,
            None => Analysis::None,
        };
        if next != self.vocal.analysis {
            self.vocal.analysis = next;
            self.repaint = true;
        }
    }

    /// The notes of the open clip the analysis found (asset frames).
    pub fn vocal_notes(&self) -> &[VocalNote] {
        &self.vocal.notes
    }

    /// True once the notes of the open clip are in (an empty list included).
    pub fn vocal_ready(&self) -> bool {
        self.vocal.analysis == Analysis::Ready
    }

    /// The selected notes, by index in [`Self::vocal_notes`].
    pub fn vocal_selection(&self) -> Vec<usize> {
        self.vocal.selected.iter().copied().collect()
    }

    /// Selects notes by index (others are ignored).
    pub fn set_vocal_selection(&mut self, notes: &[usize]) {
        let n = self.vocal.notes.len();
        self.vocal.selected = notes.iter().copied().filter(|i| *i < n).collect();
        self.repaint = true;
    }

    /// Whether Correct snaps to the song key's scale.
    pub fn vocal_scale(&self) -> bool {
        self.vocal.scale
    }

    /// The flatten amount Flatten applies, in percent.
    pub fn vocal_flatten(&self) -> u8 {
        self.vocal.flatten
    }

    /// Sets the flatten amount (0..=100).
    pub fn set_vocal_flatten(&mut self, percent: u8) {
        self.vocal.flatten = percent.min(100);
        self.repaint = true;
    }

    /// The centre of the note's blob (its edited centre pitch, at the middle of its span).
    pub fn vocal_note_center(&self, index: usize) -> Option<Point> {
        self.blob(index).map(|r| r.center())
    }

    /// Centre of the Correct button.
    pub fn correct_button_center(&self) -> Point {
        self.layout().correct.center()
    }

    /// Centre of the Flatten button.
    pub fn flatten_button_center(&self) -> Point {
        self.layout().flatten.center()
    }

    /// Centre of the Reset button.
    pub fn reset_button_center(&self) -> Point {
        self.layout().reset.center()
    }

    /// Centre of the SCALE toggle.
    pub fn scale_button_center(&self) -> Point {
        self.layout().scale.center()
    }

    /// The height of a semitone row of the pitch grid.
    pub fn vocal_row_height(&self) -> f64 {
        let (lo, hi) = self.vocal.range;
        self.layout().wave.height() / f64::from((hi - lo).max(1.0))
    }

    // ------------------------------------------------------------------ geometry

    /// Logical y of a fractional key on the grid.
    pub(super) fn y_of_key(&self, key: f32) -> f64 {
        let w = self.layout().wave;
        let (_, hi) = self.vocal.range;
        w.y0 + f64::from(hi - key) * self.vocal_row_height()
    }

    /// Logical x of an asset frame, through the clip's time curve.
    pub(super) fn x_of_frame(&self, frame: u64) -> Option<f64> {
        let offset = self.info.as_ref()?.content.source_offset as f64;
        let tick = self.tick_of_source(frame as f64 - offset)?;
        Some(self.x_of(tick))
    }

    /// The x span of an asset span, cut to what the clip plays.
    fn x_span(&self, start: u64, end: u64) -> Option<(f64, f64)> {
        let info = self.info.as_ref()?;
        let offset = info.content.source_offset as f64;
        let (s0, s1) = self
            .curve
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let a = (start as f64 - offset).max(s0);
        let b = (end as f64 - offset).min(s1);
        if b <= a {
            return None;
        }
        let xa = self.x_of(self.tick_of_source(a)?);
        let xb = self.x_of(self.tick_of_source(b)?);
        Some((xa.min(xb), xa.max(xb)))
    }

    /// The blob of note `i`: its span at its edited centre.
    pub(super) fn blob(&self, i: usize) -> Option<Rect> {
        let n = self.vocal.notes.get(i)?;
        let (x0, x1) = self.x_span(n.start, n.end)?;
        let e = edit_of(self.edits(), n);
        let y = self.y_of_key(self.vocal.centre(i) + e.shift_cents as f32 / 100.0);
        let half = (self.vocal_row_height() * 0.42).clamp(3.0, 9.0);
        Some(Rect::new(x0, y - half, x1.max(x0 + 2.0), y + half))
    }

    /// The note under `pos`, the topmost (last) first.
    pub(super) fn note_at(&self, pos: Point) -> Option<usize> {
        (0..self.vocal.notes.len()).rev().find(|i| {
            self.blob(*i)
                .is_some_and(|r| r.inflate(0.0, 2.0).contains(pos))
        })
    }

    /// The clip's pitch edits.
    pub(super) fn edits(&self) -> &[PitchEdit] {
        self.info.as_ref().map_or(&[], |i| &i.content.pitch_edits)
    }

    /// Whether the pitch of the clip can be edited.
    pub(super) fn voicing(&self) -> bool {
        self.mode == super::AudioMode::Vocal
            && self.present
            && self.meta.is_some()
            && self.length() >= 2
    }

    // ------------------------------------------------------------------ state

    /// Works out the drawn offsets, the song key and the grid's range again (after the notes or
    /// the clip changed). `refit` moves the range to the notes; a drag keeps it.
    pub(super) fn refresh_vocal(&mut self, refit: bool) {
        let rate = self.rate;
        let edits = self.edits().to_vec();
        self.vocal.offsets = self
            .vocal
            .notes
            .iter()
            .map(|n| note_offsets(&n.pitch, &shift_of(&edit_of(&edits, n)), rate))
            .collect();
        self.vocal.selected.retain(|i| *i < self.vocal.notes.len());
        if !refit {
            return;
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (i, n) in self.vocal.notes.iter().enumerate() {
            let e = edit_of(&edits, n);
            let c = self.vocal.centre(i);
            for p in [c, c + e.shift_cents as f32 / 100.0] {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        if lo > hi {
            (lo, hi) = (60.0, 60.0);
        }
        let (mut lo, mut hi) = ((lo - ROOM).floor() - 0.5, (hi + ROOM).ceil() + 0.5);
        let short = MIN_ROWS - (hi - lo);
        if short > 0.0 {
            lo -= (short / 2.0).floor();
            hi += (short / 2.0).ceil();
        }
        self.vocal.range = (lo.max(-0.5), hi.min(127.5));
    }

    /// The command that gives the clip `edits` (`None` when nothing changes).
    pub(super) fn edits_command(
        &self,
        edits: Vec<PitchEdit>,
        merge: Option<u64>,
    ) -> Option<Command> {
        let id = self.clip?;
        (edits != self.edits()).then_some(Command::Song(SongCommand::SetPitchEdits {
            id,
            edits,
            merge,
        }))
    }

    /// The notes Correct, Flatten and Reset work on: the selection.
    fn chosen(&self) -> Vec<usize> {
        self.vocal.selected.iter().copied().collect()
    }

    /// The edits after `f` changed the edit of each note in `notes` (a neutral result removes
    /// it).
    fn edited(&self, notes: &[usize], f: impl Fn(usize, &mut PitchEdit)) -> Vec<PitchEdit> {
        let mut edits = self.edits().to_vec();
        for &i in notes {
            let n = &self.vocal.notes[i];
            let mut e = edit_of(&edits, n);
            f(i, &mut e);
            edits = if e.is_neutral() && e.vibrato_keep {
                without_pitch_edits(&edits, n.start, n.end)
            } else {
                with_pitch_edit(&edits, e)
            };
        }
        edits
    }

    /// Correct (`editor.vocal_correct`): each selected note's centre to the nearest semitone, or
    /// the nearest note of the song key's scale with SCALE on.
    pub(super) fn vocal_correct(&mut self, out: &mut Vec<UiAction>) {
        let chosen = self.chosen();
        if chosen.is_empty() {
            self.status = "Select the notes to correct first".into();
            return;
        }
        let scale = self
            .vocal
            .scale
            .then_some(self.vocal.key.as_ref())
            .flatten();
        let targets: Vec<(usize, i32)> = chosen
            .iter()
            .map(|&i| {
                let c = self.vocal.centre(i);
                let to = nearest_key(c, scale);
                (i, ((to - c) * 100.0).round() as i32)
            })
            .collect();
        let edits = self.edited(&chosen, |i, e| {
            if let Some((_, cents)) = targets.iter().find(|(j, _)| *j == i) {
                e.shift_cents = (*cents).clamp(-PITCH_EDIT_MAX_CENTS, PITCH_EDIT_MAX_CENTS);
            }
        });
        out.extend(self.edits_command(edits, None).map(UiAction::Command));
    }

    /// Flatten (`editor.vocal_flatten`): the flatten amount as the drift of the selected notes.
    pub(super) fn vocal_flatten_selected(&mut self, out: &mut Vec<UiAction>) {
        let chosen = self.chosen();
        if chosen.is_empty() {
            self.status = "Select the notes to flatten first".into();
            return;
        }
        let drift = self.vocal.flatten.min(100);
        let edits = self.edited(&chosen, |_, e| e.drift = drift);
        out.extend(self.edits_command(edits, None).map(UiAction::Command));
    }

    /// Reset (`editor.vocal_reset`): removes the edits of the selected notes, or every edit when
    /// none is selected.
    pub(super) fn vocal_reset(&mut self, out: &mut Vec<UiAction>) {
        let chosen = self.chosen();
        let edits = if chosen.is_empty() {
            Vec::new()
        } else {
            let mut edits = self.edits().to_vec();
            for i in chosen {
                let n = &self.vocal.notes[i];
                edits = without_pitch_edits(&edits, n.start, n.end);
            }
            edits
        };
        out.extend(self.edits_command(edits, None).map(UiAction::Command));
    }

    /// The request for the analysis of the open clip, while Vocal mode shows it.
    pub(super) fn analyse_action(&self) -> Option<UiAction> {
        let clip = self.clip.filter(|_| self.voicing())?;
        Some(UiAction::AnalyseVocal {
            clip,
            sensitivity: self.sensitivity,
        })
    }

    /// The flatten field dragged by `dy` pixels from `orig`.
    pub(super) fn drag_flatten(&mut self, orig: u8, dy: f64) {
        let v = (f64::from(orig) - dy / FLATTEN_PX)
            .round()
            .clamp(0.0, 100.0);
        self.vocal.flatten = v as u8;
    }

    /// A press on note `i`: selects it (Shift toggles it) and starts a drag.
    pub(super) fn press_note(&mut self, i: usize, pos: Point, shift: bool, free: bool) {
        if shift {
            if !self.vocal.selected.remove(&i) {
                self.vocal.selected.insert(i);
            }
            return;
        }
        if !self.vocal.selected.contains(&i) {
            self.vocal.selected.clear();
            self.vocal.selected.insert(i);
        }
        let Some(info) = &self.info else { return };
        let orig = info.content.clone();
        let merge = self.merge_id();
        self.drag = Drag::Note(Box::new(NoteDrag {
            index: i,
            start: pos,
            axis: None,
            orig,
            free,
            merge,
            emitted: false,
            last: None,
        }));
    }

    /// A note drag moved to `pos`: the selected notes move in pitch, or the pressed note in time.
    pub(super) fn drag_note(
        &mut self,
        pos: Point,
        doc_song: &atomik_model::Song,
    ) -> Option<Command> {
        let Drag::Note(d) = &self.drag else {
            return None;
        };
        let d = d.as_ref().clone();
        let axis = match d.axis {
            Some(a) => a,
            None => {
                let (dx, dy) = (pos.x - d.start.x, pos.y - d.start.y);
                if dx.hypot(dy) <= super::DRAG_THRESHOLD {
                    return None;
                }
                let a = if dy.abs() >= dx.abs() {
                    Axis::Pitch
                } else {
                    Axis::Time
                };
                if let Drag::Note(dd) = &mut self.drag {
                    dd.axis = Some(a);
                }
                a
            }
        };
        let cmd = match axis {
            Axis::Pitch => self.pitch_move(&d, pos),
            Axis::Time => self.time_move(&d, pos, doc_song),
        }?;
        if let Drag::Note(dd) = &mut self.drag {
            if dd.last.as_ref() == Some(&cmd) {
                return None;
            }
            dd.last = Some(cmd.clone());
            dd.emitted = true;
        }
        Some(cmd)
    }

    fn pitch_move(&mut self, d: &NoteDrag, pos: Point) -> Option<Command> {
        let id = self.clip?;
        let semis = (d.start.y - pos.y) / self.vocal_row_height().max(1.0);
        let step = if d.free {
            (semis * 100.0).round() as i32
        } else {
            semis.round() as i32 * 100
        };
        let mut edits = d.orig.pitch_edits.clone();
        for &i in &self.vocal.selected {
            let n = &self.vocal.notes[i];
            let mut e = edit_of(&d.orig.pitch_edits, n);
            e.shift_cents =
                (e.shift_cents + step).clamp(-PITCH_EDIT_MAX_CENTS, PITCH_EDIT_MAX_CENTS);
            edits = with_pitch_edit(&edits, e);
        }
        Some(Command::Song(SongCommand::SetPitchEdits {
            id,
            edits,
            merge: Some(d.merge),
        }))
    }

    fn time_move(
        &mut self,
        d: &NoteDrag,
        pos: Point,
        song: &atomik_model::Song,
    ) -> Option<Command> {
        let (id, info) = (self.clip?, self.info.as_ref()?);
        if d.orig.reverse {
            self.status = "A reversed clip cannot be warped: un-reverse it to move notes".into();
            return None;
        }
        let n = self.vocal.notes.get(d.index)?;
        let offset = d.orig.source_offset;
        let (Some(a), Some(b)) = (n.start.checked_sub(offset), n.end.checked_sub(offset)) else {
            self.status = "The note starts before the clip".into();
            return None;
        };
        let delta = (self.tick_of(pos.x) - self.tick_of(d.start.x)).round() as i64;
        let base = atomik_model::warped_at_song_tempo(song, info.start, &d.orig);
        let warp: WarpSettings = base.warp.clone();
        match atomik_model::slice_edit::move_span(&warp, self.rate, info.length, (a, b), delta) {
            Ok(markers) => {
                if d.orig.warp.enabled {
                    Some(Command::Song(SongCommand::SetWarpMarkers {
                        id,
                        markers,
                        merge: Some(d.merge),
                    }))
                } else {
                    let mut content = base;
                    content.warp.markers = markers;
                    Some(Command::Song(SongCommand::SetAudioClip {
                        id,
                        start: info.start,
                        length: info.length,
                        muted: info.muted,
                        content,
                        merge: Some(d.merge),
                    }))
                }
            }
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }

    /// The command that puts the clip back as it was at the press of a note drag.
    pub(super) fn restore_note_drag(&self, d: &NoteDrag) -> Option<Command> {
        let (id, info) = (self.clip?, self.info.as_ref()?);
        Some(match d.axis {
            Some(Axis::Time) if d.orig.warp.enabled => Command::Song(SongCommand::SetWarpMarkers {
                id,
                markers: d.orig.warp.markers.clone(),
                merge: Some(d.merge),
            }),
            Some(Axis::Time) => Command::Song(SongCommand::SetAudioClip {
                id,
                start: info.start,
                length: info.length,
                muted: info.muted,
                content: d.orig.clone(),
                merge: Some(d.merge),
            }),
            _ => Command::Song(SongCommand::SetPitchEdits {
                id,
                edits: d.orig.pitch_edits.clone(),
                merge: Some(d.merge),
            }),
        })
    }

    /// The rubber band moved: the notes it touches are selected (with the base selection).
    pub(super) fn band_select(&mut self) {
        let Drag::Band(b) = &self.drag else { return };
        let r = Rect::from_points(b.start, b.now);
        let mut sel = b.base.clone();
        for i in 0..self.vocal.notes.len() {
            if self.blob(i).is_some_and(|n| n.intersect(r).area() > 0.0) {
                sel.insert(i);
            }
        }
        self.vocal.selected = sel;
    }

    /// The footer's inspector text for the first selected note.
    pub(super) fn vocal_inspector(&self) -> Option<String> {
        let i = *self.vocal.selected.first()?;
        let n = self.vocal.notes.get(i)?;
        let e = edit_of(self.edits(), n);
        let c = self.vocal.centre(i);
        let after = c + e.shift_cents as f32 / 100.0;
        let cents = |p: f32| {
            let v = ((p - p.round()) * 100.0).round() as i32;
            format!("{v:+}")
        };
        Some(format!(
            "{}  ·  {} ct → {} ct  ·  drift {}%",
            key_name(after),
            cents(c),
            cents(after),
            e.drift
        ))
    }
}
