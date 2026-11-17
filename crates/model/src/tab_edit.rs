//! Editing a tab clip: pure operations the tab editor calls.
//!
//! Every operation takes the clip's notes and its [`MidiData`] (the tab sidecar rides in it) and
//! returns a [`TabEdit`]: the new note list and the new data, with the note and the sidecar
//! changed together so they cannot disagree. [`TabEdit::into_command`] turns it into one
//! `Batch` of `SetClipContent` and `SetClipMidi`, so the edit is one undo step. After every edit
//! the techniques of the notes it touched are rendered again ([`crate::tab_render`]), so the
//! clip's velocities, lengths and expression curves always match the techniques.
//!
//! Notes are named by `(tick, key)`; strings are in tab order (0 = top line), frets are relative
//! to the capo. A note without a sidecar entry is placed automatically (the same search the
//! editor uses to show it) when an operation needs its string, and a note that cannot be placed
//! on the neck is listed in [`TabEdit::unplayable`] and left as it is, never dropped.

use std::collections::BTreeMap;

use atomik_tab::chord::ChordSymbol;
use atomik_tab::fretting::{
    FretEvent, FretOptions, NoteSlot, Placement, fret_notes, fret_sequence,
};
use atomik_tab::instrument::MAX_CAPO;
use atomik_tab::score::Techniques;
use atomik_tab::voicing::{VoicingOptions, generate};
use atomik_tab::{Capo, Tuning};

use crate::midi_data::MidiData;
use crate::song::{ClipContent, ClipId, MAX_TICK, Note, NoteList, SongCommand};
use crate::tab_data::{MAX_TAB_NOTES, TabData, TabDiagram, TabLyric, TabNoteRef};
use crate::tab_render::{render_notes, unrender};
use crate::{Command, NoteExpression};

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TabEditError {
    /// The clip carries no tab data (start one with [`start_tab`]).
    #[error("this clip has no tab data")]
    NoTab,
    /// The instrument is not in the catalogue.
    #[error("unknown instrument {0:?}")]
    UnknownInstrument(String),
    /// A string the tuning does not have.
    #[error("the tuning has no string {0}")]
    NoSuchString(u8),
    /// A fret the string cannot play.
    #[error("string {string} cannot play fret {fret}")]
    BadFret {
        /// The string.
        string: u8,
        /// The fret.
        fret: u8,
    },
    /// Another note already sits there.
    #[error("a note at tick {tick} on key {key} is in the way")]
    Occupied {
        /// Start tick of the note in the way.
        tick: i64,
        /// Its key.
        key: u8,
    },
    /// A note the clip does not have.
    #[error("the clip has no note at tick {tick} on key {key}")]
    NoSuchNote {
        /// Start tick.
        tick: i64,
        /// Key.
        key: u8,
    },
    /// The note cannot be played on the string asked for.
    #[error("the note at tick {tick} on key {key} cannot be played there")]
    NotPlayable {
        /// Start tick.
        tick: i64,
        /// Key.
        key: u8,
    },
    /// A note would leave the MIDI range.
    #[error("a note would leave the keys 0 to 127")]
    KeyRange,
    /// The note, its tick, its length or its velocity is out of range.
    #[error("{0}")]
    BadNote(String),
    /// A tuning or capo the model refuses.
    #[error("{0}")]
    BadTuning(String),
    /// The clip would hold more tab entries than it may.
    #[error("a clip keeps tab data for at most {MAX_TAB_NOTES} notes")]
    TooMany,
}

/// The result of an operation: the clip after it.
#[derive(Debug, Clone, PartialEq)]
pub struct TabEdit {
    /// The clip's notes after the edit, sorted by `(tick, key)`.
    pub notes: Vec<Note>,
    /// The clip's data (tab sidecar and expression) after the edit, in canonical form.
    pub midi: MidiData,
    /// The notes that could not be placed on the neck: they stay in the clip as they are.
    pub unplayable: Vec<(i64, u8)>,
}

impl TabEdit {
    /// The edit as one command (one undo step): the notes and the data of clip `clip`. `merge`
    /// groups a drag into one step, as for the other clip edits.
    pub fn into_command(self, clip: ClipId, merge: Option<u64>) -> Command {
        Command::Batch(vec![
            Command::Song(SongCommand::SetClipContent {
                id: clip,
                content: ClipContent::Notes(NoteList::from(self.notes)),
                merge,
            }),
            Command::Song(SongCommand::SetClipMidi {
                clip,
                midi: self.midi,
                merge,
            }),
        ])
    }
}

/// The clip being edited: notes and data, plus the tab data as it was when the edit began.
struct Work {
    notes: Vec<Note>,
    midi: MidiData,
    before: TabData,
}

impl Work {
    fn open(notes: &[Note], midi: &MidiData) -> Result<Work, TabEditError> {
        let before = midi.tab.clone().ok_or(TabEditError::NoTab)?;
        let mut notes = notes.to_vec();
        notes.sort_by_key(|n| (n.tick, n.key));
        Ok(Work {
            notes,
            midi: midi.clone(),
            before,
        })
    }

    fn tab(&self) -> &TabData {
        // `open` made sure the data is there and no operation removes it.
        self.midi.tab.as_ref().unwrap_or(&self.before)
    }

    fn tab_mut(&mut self) -> &mut TabData {
        let before = &self.before;
        self.midi.tab.get_or_insert_with(|| before.clone())
    }

    fn note_index(&self, tick: i64, key: u8) -> Option<usize> {
        self.notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
            .ok()
    }

    /// The selected notes (each must exist), sorted and without repeats.
    fn selection(&self, sel: &[(i64, u8)]) -> Result<Vec<(i64, u8)>, TabEditError> {
        let mut out: Vec<(i64, u8)> = sel.to_vec();
        out.sort_unstable();
        out.dedup();
        for &(tick, key) in &out {
            if self.note_index(tick, key).is_none() {
                return Err(TabEditError::NoSuchNote { tick, key });
            }
        }
        Ok(out)
    }

    /// Gives the unplaced notes of `sel` a position, the notes that already have one staying
    /// where they are. Returns the notes that cannot be placed.
    fn place_missing(&mut self, sel: &[(i64, u8)]) -> Vec<(i64, u8)> {
        let missing: Vec<(i64, u8)> = sel
            .iter()
            .copied()
            .filter(|(t, k)| self.tab().note_of(*t, *k).is_none())
            .collect();
        if missing.is_empty() {
            return Vec::new();
        }
        let free: Vec<(i64, u8)> = missing;
        self.fret_notes(&free, &FretOptions::default(), false)
    }

    /// Frets the notes of `free` (one sequence over the clip's time order); notes of the same
    /// ticks that already have an entry and are not in `free` are kept where they are and avoided.
    /// `replace` lets a note in `free` that has an entry move (its techniques stay). Returns the
    /// notes that cannot be placed (an existing entry of such a note is left alone).
    fn fret_notes(
        &mut self,
        free: &[(i64, u8)],
        opts: &FretOptions,
        replace: bool,
    ) -> Vec<(i64, u8)> {
        let tuning = self.tab().tuning.clone();
        let capo = self.tab().capo;
        let mut by_tick: BTreeMap<i64, Vec<(u8, bool)>> = BTreeMap::new();
        for &(tick, key) in free {
            by_tick.entry(tick).or_default().push((key, true));
        }
        for (tick, list) in &mut by_tick {
            for r in self.tab().notes.iter().filter(|r| r.tick == *tick) {
                if !free.contains(&(r.tick, r.key)) {
                    list.push((r.key, false));
                }
            }
        }
        let ticks: Vec<i64> = by_tick.keys().copied().collect();
        let events: Vec<FretEvent> = ticks
            .iter()
            .map(|t| FretEvent {
                notes: by_tick[t]
                    .iter()
                    .map(|&(key, is_free)| NoteSlot {
                        key,
                        fixed: (!is_free)
                            .then(|| {
                                self.tab().note_of(*t, key).map(|r| Placement {
                                    string: usize::from(r.string),
                                    fret: r.fret,
                                })
                            })
                            .flatten(),
                    })
                    .collect(),
            })
            .collect();
        let result = fret_sequence(&tuning, capo, &events, None, opts);
        let mut unplayable = Vec::new();
        for (e, tick) in ticks.iter().enumerate() {
            for (n, &(key, is_free)) in by_tick[tick].iter().enumerate() {
                if !is_free {
                    continue;
                }
                match result.placements[e].get(n).copied().flatten() {
                    Some(p) => {
                        let mut entry = self
                            .tab()
                            .note_of(*tick, key)
                            .cloned()
                            .unwrap_or_else(|| TabNoteRef::new(*tick, key, 0, 0));
                        if !replace && self.tab().note_of(*tick, key).is_some() {
                            continue;
                        }
                        entry.string = p.string as u8;
                        entry.fret = p.fret;
                        self.set_entry(entry);
                    }
                    None => unplayable.push((*tick, key)),
                }
            }
        }
        unplayable
    }

    fn set_entry(&mut self, entry: TabNoteRef) {
        let tab = self.tab_mut();
        match tab
            .notes
            .binary_search_by_key(&(entry.tick, entry.key), |n| (n.tick, n.key))
        {
            Ok(i) => tab.notes[i] = entry,
            Err(i) => tab.notes.insert(i, entry),
        }
    }

    fn remove_entry(&mut self, tick: i64, key: u8) {
        let tab = self.tab_mut();
        if let Ok(i) = tab
            .notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
        {
            tab.notes.remove(i);
        }
    }

    /// Un-renders `sel` and makes what is left the state the next render starts from.
    fn unrender(&mut self, sel: &[(i64, u8)]) {
        unrender(&mut self.notes, &mut self.midi, sel);
        self.before = self.tab().clone();
    }

    fn check_size(&self) -> Result<(), TabEditError> {
        if self.tab().notes.len() > MAX_TAB_NOTES {
            Err(TabEditError::TooMany)
        } else {
            Ok(())
        }
    }

    /// Renders the techniques of `touched` and returns the edit.
    fn finish(
        mut self,
        touched: &[(i64, u8)],
        unplayable: Vec<(i64, u8)>,
    ) -> Result<TabEdit, TabEditError> {
        self.check_size()?;
        self.notes.sort_by_key(|n| (n.tick, n.key));
        self.midi = std::mem::take(&mut self.midi).canonical();
        render_notes(&mut self.notes, &mut self.midi, touched, Some(&self.before));
        self.midi = std::mem::take(&mut self.midi).canonical();
        Ok(TabEdit {
            notes: self.notes,
            midi: self.midi,
            unplayable,
        })
    }
}

fn check_new_note(tick: i64, length: i64, velocity: f32) -> Result<(), TabEditError> {
    if !(0..=MAX_TICK).contains(&tick) {
        return Err(TabEditError::BadNote(format!(
            "tick {tick} is outside 0..={MAX_TICK}"
        )));
    }
    if !(1..=MAX_TICK).contains(&length) {
        return Err(TabEditError::BadNote(format!(
            "length {length} must be 1..={MAX_TICK}"
        )));
    }
    if !(velocity > 0.0 && velocity <= 1.0) {
        return Err(TabEditError::BadNote(format!(
            "velocity {velocity} is outside (0, 1]"
        )));
    }
    Ok(())
}

/// The key `string` at `fret` sounds, when the neck has that fret there.
fn key_at(tab: &TabData, string: u8, fret: u8) -> Result<u8, TabEditError> {
    let s = usize::from(string);
    if s >= tab.tuning.len() {
        return Err(TabEditError::NoSuchString(string));
    }
    let playable = fret == 0
        || (fret >= tab.tuning.min_fret(s, tab.capo) && fret <= tab.tuning.max_fret(s, tab.capo));
    if !playable {
        return Err(TabEditError::BadFret { string, fret });
    }
    tab.tuning
        .pitch(s, fret, tab.capo)
        .ok_or(TabEditError::BadFret { string, fret })
}

/// Starts tab data on a clip: the catalogue instrument `instrument_id` in its default tuning.
/// With `auto_fret` every note is placed on the neck (notes that cannot be are listed). A clip
/// that already has tab data is returned unchanged.
pub fn start_tab(
    notes: &[Note],
    midi: &MidiData,
    instrument_id: &str,
    auto_fret: bool,
) -> Result<TabEdit, TabEditError> {
    let mut midi = midi.clone();
    if midi.tab.is_none() {
        midi.tab = Some(
            TabData::for_instrument(instrument_id)
                .ok_or_else(|| TabEditError::UnknownInstrument(instrument_id.to_owned()))?,
        );
    }
    if auto_fret {
        return auto_fret_clip(notes, &midi, &FretOptions::default());
    }
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|n| (n.tick, n.key));
    Ok(TabEdit {
        notes: sorted,
        midi,
        unplayable: Vec::new(),
    })
}

/// Sets the note at `tick` on `string` to `fret`. A note already on that string at that tick
/// moves to the new pitch and keeps its length, velocity and techniques; otherwise a note of
/// `length` ticks and `velocity` is created. Refused when another note already has that pitch at
/// that tick.
pub fn set_fret(
    notes: &[Note],
    midi: &MidiData,
    (tick, string, fret): (i64, u8, u8),
    (length, velocity): (i64, f32),
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let key = key_at(w.tab(), string, fret)?;
    let occupant = w.tab().note_on_string(tick, string).cloned();
    if let Some(o) = &occupant
        && o.key == key
    {
        // Same pitch on the same string: only the entry may change (the fret is the same).
        return w.finish(&[(tick, key)], Vec::new());
    }
    if w.note_index(tick, key).is_some() {
        let other = w
            .tab()
            .note_of(tick, key)
            .is_some_and(|r| r.string != string);
        if other {
            return Err(TabEditError::Occupied { tick, key });
        }
        // A note of that pitch without a position: the entry adopts it.
    }
    let mut touched = vec![(tick, key)];
    let (len, vel) = match &occupant {
        Some(o) => {
            w.unrender(&[(o.tick, o.key)]);
            let i = w
                .note_index(o.tick, o.key)
                .ok_or(TabEditError::NoSuchNote {
                    tick: o.tick,
                    key: o.key,
                })?;
            let old = w.notes.remove(i);
            w.remove_entry(o.tick, o.key);
            touched.push((o.tick, o.key));
            (old.length, old.velocity)
        }
        None => {
            check_new_note(tick, length, velocity)?;
            (length, velocity)
        }
    };
    if w.note_index(tick, key).is_none() {
        w.notes.push(Note::new(tick, len, key, vel));
        w.notes.sort_by_key(|n| (n.tick, n.key));
    }
    let mut entry = TabNoteRef::new(tick, key, string, fret);
    if let Some(o) = occupant {
        entry.tech = o.tech;
    }
    w.set_entry(entry);
    w.finish(&touched, Vec::new())
}

/// Removes the note at `tick` on `string` and its entry.
pub fn clear_fret(
    notes: &[Note],
    midi: &MidiData,
    tick: i64,
    string: u8,
) -> Result<TabEdit, TabEditError> {
    let w = Work::open(notes, midi)?;
    let Some(o) = w.tab().note_on_string(tick, string).cloned() else {
        return Err(TabEditError::NoSuchNote { tick, key: 0 });
    };
    delete_notes(notes, midi, &[(o.tick, o.key)])
}

/// Removes the notes in `selection` with their entries and expression.
pub fn delete_notes(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let sel = w.selection(selection)?;
    w.unrender(&sel);
    for &(tick, key) in &sel {
        if let Some(i) = w.note_index(tick, key) {
            w.notes.remove(i);
        }
        w.remove_entry(tick, key);
        if let Ok(i) = w
            .midi
            .expression
            .binary_search_by_key(&(tick, key), |e| (e.tick, e.key))
        {
            w.midi.expression.remove(i);
        }
    }
    w.finish(&sel, Vec::new())
}

/// Moves the notes in `selection` `delta` strings (down is positive) keeping their pitch. All
/// or nothing: refused when a note cannot be played on its new string or two notes would meet.
pub fn change_string(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
    delta: i32,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let sel = w.selection(selection)?;
    let unplayable = w.place_missing(&sel);
    if let Some(&(tick, key)) = unplayable.first() {
        return Err(TabEditError::NotPlayable { tick, key });
    }
    let tab = w.tab().clone();
    let mut moved: Vec<TabNoteRef> = Vec::new();
    for &(tick, key) in &sel {
        let Some(r) = tab.note_of(tick, key) else {
            continue;
        };
        let target = i32::from(r.string) + delta;
        let target = u8::try_from(target)
            .ok()
            .filter(|s| usize::from(*s) < tab.tuning.len())
            .ok_or(TabEditError::NotPlayable { tick, key })?;
        let fret = tab
            .tuning
            .fret_for_pitch(usize::from(target), key, tab.capo)
            .ok_or(TabEditError::NotPlayable { tick, key })?;
        let mut m = r.clone();
        m.string = target;
        m.fret = fret;
        moved.push(m);
    }
    // No two notes of one tick on one string, among the moved and the ones that stay.
    for (i, m) in moved.iter().enumerate() {
        let clash_moved = moved[..i]
            .iter()
            .any(|o| o.tick == m.tick && o.string == m.string);
        let clash_stay = tab
            .notes
            .iter()
            .any(|o| o.tick == m.tick && o.string == m.string && !sel.contains(&(o.tick, o.key)));
        if clash_moved || clash_stay {
            return Err(TabEditError::NotPlayable {
                tick: m.tick,
                key: m.key,
            });
        }
    }
    for m in moved {
        w.set_entry(m);
    }
    w.finish(&sel, Vec::new())
}

/// Places the notes in `selection` on the neck again, whatever position they had (their
/// techniques stay). Notes at the same tick that are not selected keep their strings and are
/// avoided. Notes that cannot be placed keep their old entry and are listed.
pub fn refret(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
    opts: &FretOptions,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let sel = w.selection(selection)?;
    let unplayable = w.fret_notes(&sel, opts, true);
    w.finish(&sel, unplayable)
}

/// Places every note of the clip that has no position yet, keeping the positions the clip
/// already has (they are avoided and guide the hand). Notes that cannot be placed are listed.
pub fn auto_fret_clip(
    notes: &[Note],
    midi: &MidiData,
    opts: &FretOptions,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let missing: Vec<(i64, u8)> = w
        .notes
        .iter()
        .map(|n| (n.tick, n.key))
        .filter(|(t, k)| w.tab().note_of(*t, *k).is_none())
        .collect();
    let unplayable = w.fret_notes(&missing, opts, false);
    w.finish(&missing, unplayable)
}

/// Changes the techniques of the notes in `selection` with `f`. A note without a position is
/// placed first; one that cannot be placed is listed and left alone.
pub fn update_techniques(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
    f: impl Fn(&mut Techniques),
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let sel = w.selection(selection)?;
    let unplayable = w.place_missing(&sel);
    for &(tick, key) in &sel {
        let Some(mut r) = w.tab().note_of(tick, key).cloned() else {
            continue;
        };
        f(&mut r.tech);
        w.set_entry(r);
    }
    w.finish(&sel, unplayable)
}

/// Sets the techniques of the notes in `selection` to `tech`.
pub fn set_techniques(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
    tech: &Techniques,
) -> Result<TabEdit, TabEditError> {
    update_techniques(notes, midi, selection, |t| *t = tech.clone())
}

/// Removes every technique from the notes in `selection` (their velocity, length and rendered
/// curves go back to plain).
pub fn clear_techniques(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
) -> Result<TabEdit, TabEditError> {
    update_techniques(notes, midi, selection, |t| *t = Techniques::default())
}

/// Moves the notes in `selection` by `semitones`. A note keeps its string when the new pitch
/// is playable there, else it is placed again; a note that cannot be played anywhere keeps its
/// pitch change but loses its entry (it is listed). Refused when a note would leave 0..=127 or
/// meet another note.
pub fn transpose(
    notes: &[Note],
    midi: &MidiData,
    selection: &[(i64, u8)],
    semitones: i32,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let sel = w.selection(selection)?;
    let moved: Vec<(i64, u8, u8)> = sel
        .iter()
        .map(|&(tick, key)| {
            u8::try_from(i32::from(key) + semitones)
                .ok()
                .filter(|k| *k <= 127)
                .map(|k| (tick, key, k))
                .ok_or(TabEditError::KeyRange)
        })
        .collect::<Result<_, _>>()?;
    for &(tick, old, new) in &moved {
        let meets = w.note_index(tick, new).is_some() && !sel.contains(&(tick, new));
        if meets {
            return Err(TabEditError::Occupied { tick, key: new });
        }
        let _ = old;
    }
    w.unrender(&sel);
    let tuning = w.tab().tuning.clone();
    let capo = w.tab().capo;
    let mut touched: Vec<(i64, u8)> = sel.clone();
    let mut unplayable = Vec::new();
    // Take the notes, their entries and their expression out, then put them back moved.
    let mut taken: Vec<(Note, Option<TabNoteRef>, Option<NoteExpression>)> = Vec::new();
    for &(tick, key, _) in &moved {
        let i = w
            .note_index(tick, key)
            .ok_or(TabEditError::NoSuchNote { tick, key })?;
        let n = w.notes.remove(i);
        let r = w.tab().note_of(tick, key).cloned();
        w.remove_entry(tick, key);
        let x = w
            .midi
            .expression
            .binary_search_by_key(&(tick, key), |e| (e.tick, e.key))
            .ok()
            .map(|i| w.midi.expression.remove(i));
        taken.push((n, r, x));
    }
    for ((tick, _, new), (mut n, r, x)) in moved.into_iter().zip(taken) {
        n.key = new;
        w.notes.push(n);
        if let Some(mut x) = x {
            x.key = new;
            w.midi.expression.push(x);
        }
        if let Some(mut r) = r {
            r.key = new;
            let s = usize::from(r.string);
            match tuning.fret_for_pitch(s, new, capo) {
                Some(fret) => {
                    r.fret = fret;
                    w.set_entry(r);
                }
                None => match fret_notes(&tuning, capo, &[new], None, &FretOptions::default())
                    .first()
                    .copied()
                    .flatten()
                {
                    Some(p) => {
                        r.string = p.string as u8;
                        r.fret = p.fret;
                        w.set_entry(r);
                    }
                    None => unplayable.push((tick, new)),
                },
            }
        }
        touched.push((tick, new));
    }
    w.finish(&touched, unplayable)
}

/// Changes the instrument, tuning and capo of the clip. The notes keep their pitches: every note
/// with a position is placed on the new neck again (techniques stay); one the new neck cannot
/// play loses its position and techniques and is listed. Chord diagrams are drawn again for the
/// new neck, and dropped when no shape exists.
pub fn set_tuning(
    notes: &[Note],
    midi: &MidiData,
    instrument_id: &str,
    tuning: Tuning,
    capo: Capo,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    tuning
        .validate()
        .map_err(|e| TabEditError::BadTuning(e.to_string()))?;
    if capo.fret > MAX_CAPO {
        return Err(TabEditError::BadTuning(format!(
            "the capo is at most fret {MAX_CAPO}"
        )));
    }
    let entries: Vec<TabNoteRef> = w.tab().notes.clone();
    let touched: Vec<(i64, u8)> = entries.iter().map(|r| (r.tick, r.key)).collect();
    {
        let tab = w.tab_mut();
        tab.instrument_id = instrument_id.to_owned();
        tab.tuning = tuning.clone();
        tab.capo = capo;
    }
    // Keep the pitches: every entry is placed again on the new neck, one sequence in time order.
    let mut by_tick: BTreeMap<i64, Vec<&TabNoteRef>> = BTreeMap::new();
    for r in &entries {
        by_tick.entry(r.tick).or_default().push(r);
    }
    let events: Vec<FretEvent> = by_tick
        .values()
        .map(|list| FretEvent {
            notes: list.iter().map(|r| NoteSlot::new(r.key)).collect(),
        })
        .collect();
    let result = fret_sequence(&tuning, capo, &events, None, &FretOptions::default());
    let mut unplayable = Vec::new();
    for (e, list) in by_tick.values().enumerate() {
        for (n, r) in list.iter().enumerate() {
            match result.placements[e].get(n).copied().flatten() {
                Some(p) => {
                    let mut moved = (*r).clone();
                    moved.string = p.string as u8;
                    moved.fret = p.fret;
                    w.set_entry(moved);
                }
                None => {
                    w.remove_entry(r.tick, r.key);
                    unplayable.push((r.tick, r.key));
                }
            }
        }
    }
    let diagrams: Vec<TabDiagram> = w
        .tab()
        .diagrams
        .iter()
        .filter_map(|d| {
            let chord = ChordSymbol::parse(&d.name).ok()?;
            let voicing = generate(&tuning, capo, &chord, &VoicingOptions::default())
                .into_iter()
                .next()?;
            Some(TabDiagram {
                tick: d.tick,
                name: d.name.clone(),
                voicing,
            })
        })
        .collect();
    w.tab_mut().diagrams = diagrams;
    w.finish(&touched, unplayable)
}

/// Changes the capo only (see [`set_tuning`]).
pub fn set_capo(notes: &[Note], midi: &MidiData, capo: Capo) -> Result<TabEdit, TabEditError> {
    let tab = midi.tab.as_ref().ok_or(TabEditError::NoTab)?;
    set_tuning(
        notes,
        midi,
        &tab.instrument_id.clone(),
        tab.tuning.clone(),
        capo,
    )
}

/// Pins a chord diagram at `tick` (it replaces one at the same tick).
pub fn set_diagram(
    notes: &[Note],
    midi: &MidiData,
    diagram: TabDiagram,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    let tab = w.tab_mut();
    tab.diagrams.retain(|d| d.tick != diagram.tick);
    tab.diagrams.push(diagram);
    w.finish(&[], Vec::new())
}

/// Removes the chord diagram at `tick`.
pub fn remove_diagram(notes: &[Note], midi: &MidiData, tick: i64) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    w.tab_mut().diagrams.retain(|d| d.tick != tick);
    w.finish(&[], Vec::new())
}

/// Replaces the lyric syllables of the clip.
pub fn set_lyrics(
    notes: &[Note],
    midi: &MidiData,
    lyrics: Vec<TabLyric>,
) -> Result<TabEdit, TabEditError> {
    let mut w = Work::open(notes, midi)?;
    w.tab_mut().lyrics = lyrics;
    w.finish(&[], Vec::new())
}
