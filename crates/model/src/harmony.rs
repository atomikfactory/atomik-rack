//! The song's harmony: a song key, key changes along the song and a chord lane.
//!
//! The harmony lives in [`Song::harmony`](crate::song::Song::harmony). The note editors, the
//! browser and a few tools read it (the piano roll of a clip without a scale of its own follows
//! [`Harmony::key_at`], the chord stamp can drop the chord of the lane, "Fit Notes to Chords"
//! moves notes onto the chord tones), and it is compiled with the song for the engine, where the
//! Key and Chord device, the Arpeggiator and the CV Quantizer can follow it (see
//! `atomik_core::harmony`).
//!
//! * The **song key** is a root and a scale kind (never chromatic). It holds from the start of
//!   the song until the first key change.
//! * A **key change** is a key from a tick on, like a meter change. [`Harmony::key_at`] is the
//!   key of the last change at or before the tick, or the song key before the first change.
//! * A **chord** is a root, a quality (the chord kinds of the chord stamp), an optional bass note
//!   and a span in ticks. The chords are sorted by start and never overlap; a gap is silence in
//!   the lane, not a chord.
//!
//! Every edit is one [`SongCommand::SetHarmony`](crate::song::SongCommand::SetHarmony) (or a
//! [`DragHarmony`](crate::song::SongCommand::DragHarmony) for a drag) carrying the whole
//! harmony, built with the pure helpers here. The project file stores the three parts as optional
//! song fields written only when set (format 45, ignorable).

use crate::chord_symbol::format_chord;
use crate::song::MAX_TICK;
use crate::theory::{ChordKind, Scale, ScaleKind, pitch_name};

/// The most key changes a song has.
pub const MAX_KEY_CHANGES: usize = 256;
/// The most chords a song has.
pub const MAX_CHORDS: usize = 4096;

/// The names of the pitch classes with flats, C first.
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

/// A key: a root pitch class and a scale kind. The scale is never chromatic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// The root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The scale kind (not [`ScaleKind::Chromatic`]).
    pub scale: ScaleKind,
}

impl Key {
    /// A key on `root` (taken modulo 12).
    pub fn new(root: u8, scale: ScaleKind) -> Self {
        Self {
            root: root % 12,
            scale,
        }
    }

    /// The same key as a [`Scale`], what the note editors highlight and snap to.
    pub fn as_scale(self) -> Scale {
        Scale::new(self.root, self.scale)
    }

    /// Whether the key can be stored: any scale but the chromatic one.
    pub fn is_valid(self) -> bool {
        self.root < 12 && self.scale != ScaleKind::Chromatic
    }

    /// The scale word of the name: `major`, `minor`, `dorian`, `harmonic minor`.
    fn word(self) -> String {
        match self.scale {
            ScaleKind::Major => "major".to_owned(),
            ScaleKind::NaturalMinor => "minor".to_owned(),
            other => other.label().to_lowercase(),
        }
    }

    /// The name shown to people and the tag of a loop in that key: `A minor`, `D dorian`.
    pub fn name(self) -> String {
        format!("{} {}", pitch_name(self.root), self.word())
    }

    /// Every spelling of [`Key::name`] a tag may use: sharps first, then the flat spelling when it
    /// differs (`A# minor`, `Bb minor`).
    pub fn tags(self) -> Vec<String> {
        let mut tags = vec![self.name()];
        let flat = format!(
            "{} {}",
            FLAT_NAMES[usize::from(self.root % 12)],
            self.word()
        );
        if flat != tags[0] {
            tags.push(flat);
        }
        tags
    }

    /// A short name for a narrow field: `C`, `Am`, `D dor`.
    pub fn short(self) -> String {
        let root = pitch_name(self.root);
        match self.scale {
            ScaleKind::Major => root.to_owned(),
            ScaleKind::NaturalMinor => format!("{root}m"),
            other => {
                let word: String = other.label().chars().take(3).collect();
                format!("{root} {}", word.to_lowercase())
            }
        }
    }
}

/// A key from a tick on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChange {
    /// Song tick, `0..=MAX_TICK`.
    pub tick: i64,
    /// The key from there on.
    pub key: Key,
}

/// A chord of the chord lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordEvent {
    /// Song tick the chord starts at, `0..=MAX_TICK`.
    pub tick: i64,
    /// Length in ticks (> 0).
    pub length: i64,
    /// The root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The quality.
    pub quality: ChordKind,
    /// The bass note when it is not the root (a slash chord).
    pub bass: Option<u8>,
}

impl ChordEvent {
    /// A chord with no bass note of its own. The root is taken modulo 12.
    pub fn new(tick: i64, length: i64, root: u8, quality: ChordKind) -> Self {
        Self {
            tick,
            length,
            root: root % 12,
            quality,
            bass: None,
        }
    }

    /// The first tick after the chord.
    pub fn end(&self) -> i64 {
        self.tick.saturating_add(self.length)
    }

    /// The symbol the lane shows: `Am7`, `C/G`.
    pub fn symbol(&self) -> String {
        format_chord(self.root, self.quality, self.bass)
    }

    /// The pitch classes of the chord as a 12-bit mask (bit `pc`): the chord tones and the bass.
    pub fn pitch_mask(&self) -> u16 {
        let mut mask = 0u16;
        for i in self.quality.intervals() {
            mask |= 1 << ((u16::from(self.root) + u16::from(*i)) % 12);
        }
        if let Some(b) = self.bass {
            mask |= 1 << (b % 12);
        }
        mask
    }

    /// Whether `key` is a tone of the chord (or its bass), in any octave.
    pub fn contains(&self, key: u8) -> bool {
        self.pitch_mask() & (1 << (key % 12)) != 0
    }
}

/// The song key, the key changes and the chord lane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Harmony {
    /// The song key (`None`: the song has no key).
    pub key: Option<Key>,
    /// Key changes, sorted by tick, at most one per tick.
    pub changes: Vec<KeyChange>,
    /// Chords, sorted by start, never overlapping.
    pub chords: Vec<ChordEvent>,
}

impl Harmony {
    /// Whether the song has no key, no key change and no chord.
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && self.changes.is_empty() && self.chords.is_empty()
    }

    /// The key at `tick`: the last change at or before it, else the song key.
    pub fn key_at(&self, tick: i64) -> Option<Key> {
        let after = self.changes.partition_point(|c| c.tick <= tick);
        match after.checked_sub(1) {
            Some(i) => Some(self.changes[i].key),
            None => self.key,
        }
    }

    /// The chord that holds at `tick` (a chord covers `[tick, tick + length)`).
    pub fn chord_at(&self, tick: i64) -> Option<&ChordEvent> {
        let after = self.chords.partition_point(|c| c.tick <= tick);
        let chord = &self.chords[after.checked_sub(1)?];
        (tick < chord.end()).then_some(chord)
    }

    /// The index of the chord that starts exactly at `tick`.
    pub fn chord_index(&self, tick: i64) -> Option<usize> {
        self.chords.binary_search_by_key(&tick, |c| c.tick).ok()
    }

    /// The index of the key change at exactly `tick`.
    pub fn change_index(&self, tick: i64) -> Option<usize> {
        self.changes.binary_search_by_key(&tick, |c| c.tick).ok()
    }

    /// How long a chord starting at `tick` may be, at most `want` ticks: the room before the
    /// next chord. The chord that starts at `ignoring` is not counted (the one being moved).
    /// `None` when `tick` lies inside another chord.
    pub fn room_for_chord(&self, tick: i64, want: i64, ignoring: Option<i64>) -> Option<i64> {
        let mut room = want.min(MAX_TICK - tick);
        for c in self.chords.iter().filter(|c| Some(c.tick) != ignoring) {
            if c.tick <= tick && tick < c.end() {
                return None;
            }
            if c.tick > tick {
                room = room.min(c.tick - tick);
                break;
            }
        }
        (room > 0).then_some(room)
    }

    /// The chord a click on an empty lane starts with: the tonic chord of the key at `tick`
    /// (minor when the key's third is minor, major otherwise), C major without a key.
    pub fn suggested_chord(&self, tick: i64, length: i64) -> ChordEvent {
        let Some(key) = self.key_at(tick) else {
            return ChordEvent::new(tick, length, 0, ChordKind::Major);
        };
        let steps = key.scale.intervals();
        let minor = steps.contains(&3) && !steps.contains(&4);
        let quality = if minor {
            ChordKind::Minor
        } else {
            ChordKind::Major
        };
        ChordEvent::new(tick, length, key.root, quality)
    }

    /// Whether the harmony can be stored: ranges, order, no overlap, no chromatic key.
    pub fn check(&self) -> Result<(), String> {
        if let Some(k) = self.key
            && !k.is_valid()
        {
            return Err("the song key needs a root and a scale that is not chromatic".to_owned());
        }
        if self.changes.len() > MAX_KEY_CHANGES {
            return Err(format!("at most {MAX_KEY_CHANGES} key changes"));
        }
        for (i, c) in self.changes.iter().enumerate() {
            if !(0..=MAX_TICK).contains(&c.tick) {
                return Err(format!("key change {i} is outside the song"));
            }
            if !c.key.is_valid() {
                return Err(format!("key change {i} has no valid key"));
            }
            if i > 0 && self.changes[i - 1].tick >= c.tick {
                return Err(format!("key change {i} is not after the one before"));
            }
        }
        if self.chords.len() > MAX_CHORDS {
            return Err(format!("at most {MAX_CHORDS} chords"));
        }
        for (i, c) in self.chords.iter().enumerate() {
            if c.tick < 0 || c.length < 1 || c.end() > MAX_TICK {
                return Err(format!("chord {i} is outside the song or empty"));
            }
            if c.root >= 12 || c.bass.is_some_and(|b| b >= 12 || b == c.root) {
                return Err(format!("chord {i} has a bad root or bass note"));
            }
            if i > 0 && self.chords[i - 1].end() > c.tick {
                return Err(format!("chord {i} overlaps the one before"));
            }
        }
        Ok(())
    }

    /// The harmony with this song key.
    pub fn with_key(&self, key: Option<Key>) -> Result<Harmony, String> {
        let mut h = self.clone();
        h.key = key;
        h.check()?;
        Ok(h)
    }

    /// The harmony with a key change at `tick` (replacing the one there), or with it removed when
    /// `key` is `None`.
    pub fn with_key_change(&self, tick: i64, key: Option<Key>) -> Result<Harmony, String> {
        let mut h = self.clone();
        h.changes.retain(|c| c.tick != tick);
        if let Some(key) = key {
            h.changes.push(KeyChange { tick, key });
            h.changes.sort_by_key(|c| c.tick);
        }
        h.check()?;
        Ok(h)
    }

    /// The harmony with the key change at `from` moved to `to`; refused when another change is
    /// there already or none is at `from`.
    pub fn with_key_change_moved(&self, from: i64, to: i64) -> Result<Harmony, String> {
        let i = self
            .change_index(from)
            .ok_or_else(|| "there is no key change there".to_owned())?;
        if from != to && self.change_index(to).is_some() {
            return Err("there is a key change there already".to_owned());
        }
        let key = self.changes[i].key;
        self.with_key_change(from, None)?
            .with_key_change(to, Some(key))
    }

    /// The harmony with `chord` in the lane, replacing the chord that starts at the same tick.
    /// Refused when it would overlap another chord.
    pub fn with_chord(&self, chord: ChordEvent) -> Result<Harmony, String> {
        let mut h = self.clone();
        h.chords.retain(|c| c.tick != chord.tick);
        let mut chord = chord;
        chord.bass = chord.bass.filter(|b| *b != chord.root);
        h.chords.push(chord);
        h.chords.sort_by_key(|c| c.tick);
        h.check()?;
        Ok(h)
    }

    /// The harmony without the chord that starts at `tick` (unchanged when there is none).
    pub fn without_chord(&self, tick: i64) -> Harmony {
        let mut h = self.clone();
        h.chords.retain(|c| c.tick != tick);
        h
    }

    /// The harmony with the chord that starts at `from` replaced by `chord` (moved, resized or
    /// changed). Refused when there is no chord at `from`, or another chord starts where `chord`
    /// does (a move never replaces a chord), or the result would overlap.
    pub fn with_chord_replaced(&self, from: i64, chord: ChordEvent) -> Result<Harmony, String> {
        if self.chord_index(from).is_none() {
            return Err("there is no chord there".to_owned());
        }
        if chord.tick != from && self.chord_index(chord.tick).is_some() {
            return Err("there is a chord there already".to_owned());
        }
        self.without_chord(from).with_chord(chord)
    }

    /// The harmony with the `found` chords (sorted, not overlapping) put in the lane over the
    /// ticks `from..to`; a found chord is cut to the range.
    ///
    /// * **Replace** (`fill_gaps_only` false): the chords of the range go. A chord that starts
    ///   before the range ends where it begins; one that runs past its end keeps the part after.
    /// * **Merge** (`fill_gaps_only` true): the chords already there stay and the found chords
    ///   fill only the gaps between them (a found chord may become several pieces).
    ///
    /// Refused when the result could not be stored (too many chords).
    pub fn with_chords_in_range(
        &self,
        (from, to): (i64, i64),
        found: &[ChordEvent],
        fill_gaps_only: bool,
    ) -> Result<Harmony, String> {
        let (from, to) = (from.clamp(0, MAX_TICK), to.clamp(0, MAX_TICK));
        let mut kept: Vec<ChordEvent> = Vec::new();
        if fill_gaps_only {
            kept.clone_from(&self.chords);
        } else {
            for c in &self.chords {
                if c.end() <= from || c.tick >= to {
                    kept.push(*c);
                    continue;
                }
                if c.tick < from {
                    kept.push(ChordEvent {
                        length: from - c.tick,
                        ..*c
                    });
                }
                if c.end() > to {
                    kept.push(ChordEvent {
                        tick: to,
                        length: c.end() - to,
                        ..*c
                    });
                }
            }
        }
        let mut added: Vec<ChordEvent> = Vec::new();
        for f in found {
            let (a, b) = (f.tick.max(from), f.end().min(to));
            if b <= a {
                continue;
            }
            // The free stretches of `a..b` among the chords kept.
            let mut at = a;
            for c in kept.iter().filter(|c| c.end() > a && c.tick < b) {
                if c.tick > at {
                    added.push(ChordEvent {
                        tick: at,
                        length: c.tick - at,
                        ..*f
                    });
                }
                at = at.max(c.end());
            }
            if b > at {
                added.push(ChordEvent {
                    tick: at,
                    length: b - at,
                    ..*f
                });
            }
        }
        kept.extend(added);
        kept.sort_by_key(|c| c.tick);
        let out = Harmony {
            chords: kept,
            ..self.clone()
        };
        out.check()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;

    const BAR: i64 = PPQ * 4;

    fn key(root: u8, scale: ScaleKind) -> Key {
        Key::new(root, scale)
    }

    fn chord(bar: i64, bars: i64, root: u8, quality: ChordKind) -> ChordEvent {
        ChordEvent::new(bar * BAR, bars * BAR, root, quality)
    }

    fn spans(h: &Harmony) -> Vec<(i64, i64, u8)> {
        h.chords
            .iter()
            .map(|c| (c.tick / BAR, c.end() / BAR, c.root))
            .collect()
    }

    #[test]
    fn found_chords_replace_the_chords_of_their_range() {
        let h = Harmony {
            chords: vec![
                chord(0, 2, 0, ChordKind::Major),
                chord(3, 2, 2, ChordKind::Minor),
                chord(6, 4, 4, ChordKind::Minor),
                chord(12, 1, 5, ChordKind::Major),
            ],
            ..Harmony::default()
        };
        let found = [
            chord(0, 2, 7, ChordKind::Major),
            chord(2, 4, 9, ChordKind::Minor),
            chord(6, 4, 11, ChordKind::Dim),
        ];
        let out = h
            .with_chords_in_range((BAR, 8 * BAR), &found, false)
            .unwrap();
        // The first chord ends where the range starts; the third keeps its part after the range.
        assert_eq!(
            spans(&out),
            vec![
                (0, 1, 0),
                (1, 2, 7),
                (2, 6, 9),
                (6, 8, 11),
                (8, 10, 4),
                (12, 13, 5)
            ]
        );
        assert!(out.check().is_ok());
    }

    #[test]
    fn merging_found_chords_only_fills_the_gaps() {
        let h = Harmony {
            chords: vec![
                chord(2, 1, 0, ChordKind::Major),
                chord(4, 1, 5, ChordKind::Major),
            ],
            ..Harmony::default()
        };
        let found = [chord(0, 8, 9, ChordKind::Minor)];
        let out = h.with_chords_in_range((0, 6 * BAR), &found, true).unwrap();
        assert_eq!(
            spans(&out),
            vec![(0, 2, 9), (2, 3, 0), (3, 4, 9), (4, 5, 5), (5, 6, 9)]
        );
    }

    #[test]
    fn found_chords_keep_the_key_and_their_bass() {
        let h = Harmony {
            key: Some(key(0, ScaleKind::Major)),
            ..Harmony::default()
        };
        let mut slash = chord(0, 1, 0, ChordKind::Major);
        slash.bass = Some(7);
        let out = h.with_chords_in_range((0, BAR), &[slash], false).unwrap();
        assert_eq!(out.key, h.key);
        assert_eq!(out.chords, vec![slash]);
        assert_eq!(
            h.with_chords_in_range((0, BAR), &[], false).unwrap(),
            h,
            "nothing found changes nothing"
        );
    }

    #[test]
    fn key_names_follow_the_loop_tags() {
        assert_eq!(key(9, ScaleKind::NaturalMinor).name(), "A minor");
        assert_eq!(key(0, ScaleKind::Major).name(), "C major");
        assert_eq!(key(2, ScaleKind::Dorian).name(), "D dorian");
        assert_eq!(key(7, ScaleKind::HarmonicMinor).name(), "G harmonic minor");
        assert_eq!(
            key(10, ScaleKind::NaturalMinor).tags(),
            ["A# minor", "Bb minor"]
        );
        assert_eq!(key(9, ScaleKind::NaturalMinor).tags(), ["A minor"]);
        assert_eq!(key(9, ScaleKind::NaturalMinor).short(), "Am");
        assert_eq!(key(0, ScaleKind::Major).short(), "C");
        assert_eq!(key(2, ScaleKind::Dorian).short(), "D dor");
    }

    #[test]
    fn the_key_at_a_tick_follows_the_changes() {
        let h = Harmony {
            key: Some(key(0, ScaleKind::Major)),
            changes: vec![
                KeyChange {
                    tick: 8 * BAR,
                    key: key(9, ScaleKind::NaturalMinor),
                },
                KeyChange {
                    tick: 16 * BAR,
                    key: key(2, ScaleKind::Dorian),
                },
            ],
            chords: Vec::new(),
        };
        assert_eq!(h.key_at(0), Some(key(0, ScaleKind::Major)));
        assert_eq!(h.key_at(8 * BAR - 1), Some(key(0, ScaleKind::Major)));
        assert_eq!(h.key_at(8 * BAR), Some(key(9, ScaleKind::NaturalMinor)));
        assert_eq!(
            h.key_at(16 * BAR - 1),
            Some(key(9, ScaleKind::NaturalMinor))
        );
        assert_eq!(h.key_at(99 * BAR), Some(key(2, ScaleKind::Dorian)));
    }

    #[test]
    fn a_song_without_a_key_has_none_before_the_first_change() {
        let h = Harmony::default()
            .with_key_change(4 * BAR, Some(key(7, ScaleKind::Mixolydian)))
            .unwrap();
        assert_eq!(h.key_at(0), None);
        assert_eq!(h.key_at(4 * BAR), Some(key(7, ScaleKind::Mixolydian)));
        assert_eq!(Harmony::default().key_at(123), None);
    }

    #[test]
    fn a_chord_covers_its_span_and_gaps_have_none() {
        let h = Harmony::default()
            .with_chord(chord(0, 2, 0, ChordKind::Major))
            .unwrap()
            .with_chord(chord(3, 1, 9, ChordKind::Minor))
            .unwrap();
        assert_eq!(h.chord_at(0).map(|c| c.root), Some(0));
        assert_eq!(h.chord_at(2 * BAR - 1).map(|c| c.root), Some(0));
        assert_eq!(h.chord_at(2 * BAR), None, "the gap");
        assert_eq!(h.chord_at(3 * BAR).map(|c| c.root), Some(9));
        assert_eq!(h.chord_at(4 * BAR), None);
        assert_eq!(h.chord_index(3 * BAR), Some(1));
        assert_eq!(h.chord_index(3 * BAR + 1), None);
    }

    #[test]
    fn chords_never_overlap() {
        let h = Harmony::default()
            .with_chord(chord(0, 2, 0, ChordKind::Major))
            .unwrap();
        assert!(h.with_chord(chord(1, 1, 5, ChordKind::Major)).is_err());
        assert!(
            h.with_chord(chord(0, 3, 5, ChordKind::Major)).is_ok(),
            "same start replaces"
        );
        let longer = h.with_chord(chord(0, 3, 5, ChordKind::Major)).unwrap();
        assert_eq!(longer.chords.len(), 1);
        assert_eq!(longer.chords[0].root, 5);
        assert!(
            h.with_chord(chord(2, 1, 5, ChordKind::Major)).is_ok(),
            "touching is fine"
        );
        let two = h.with_chord(chord(2, 1, 5, ChordKind::Major)).unwrap();
        assert!(
            two.with_chord_replaced(2 * BAR, chord(1, 2, 5, ChordKind::Major))
                .is_err()
        );
        assert!(
            two.with_chord_replaced(2 * BAR, chord(4, 1, 5, ChordKind::Major))
                .is_ok()
        );
        assert!(
            two.with_chord_replaced(7, chord(4, 1, 5, ChordKind::Major))
                .is_err(),
            "no chord at 7"
        );
    }

    #[test]
    fn room_for_a_chord_stops_at_the_next_one() {
        let h = Harmony::default()
            .with_chord(chord(2, 1, 0, ChordKind::Major))
            .unwrap();
        assert_eq!(h.room_for_chord(0, BAR, None), Some(BAR));
        assert_eq!(h.room_for_chord(BAR + BAR / 2, BAR, None), Some(BAR / 2));
        assert_eq!(
            h.room_for_chord(2 * BAR + 1, BAR, None),
            None,
            "inside a chord"
        );
        assert_eq!(h.room_for_chord(2 * BAR + 1, BAR, Some(2 * BAR)), Some(BAR));
        assert_eq!(h.room_for_chord(3 * BAR, BAR, None), Some(BAR));
    }

    #[test]
    fn a_bass_equal_to_the_root_is_dropped_and_a_bad_one_refused() {
        let mut c = chord(0, 1, 0, ChordKind::Major);
        c.bass = Some(0);
        let h = Harmony::default().with_chord(c).unwrap();
        assert_eq!(h.chords[0].bass, None);
        let mut bad = h.clone();
        bad.chords[0].bass = Some(12);
        assert!(bad.check().is_err());
        bad.chords[0].bass = Some(0);
        assert!(bad.check().is_err(), "a stored bass differs from the root");
    }

    #[test]
    fn a_chromatic_key_is_refused() {
        assert!(
            Harmony::default()
                .with_key(Some(key(0, ScaleKind::Chromatic)))
                .is_err()
        );
        assert!(
            Harmony::default()
                .with_key_change(0, Some(key(0, ScaleKind::Chromatic)))
                .is_err()
        );
        assert!(
            Harmony::default()
                .with_key(Some(key(0, ScaleKind::Blues)))
                .is_ok()
        );
    }

    #[test]
    fn key_changes_are_replaced_moved_and_removed() {
        let h = Harmony::default()
            .with_key_change(4 * BAR, Some(key(2, ScaleKind::Major)))
            .unwrap()
            .with_key_change(8 * BAR, Some(key(7, ScaleKind::Major)))
            .unwrap();
        let replaced = h
            .with_key_change(4 * BAR, Some(key(5, ScaleKind::Major)))
            .unwrap();
        assert_eq!(replaced.changes.len(), 2);
        assert_eq!(replaced.changes[0].key.root, 5);
        let moved = h.with_key_change_moved(4 * BAR, 6 * BAR).unwrap();
        assert_eq!(moved.changes[0].tick, 6 * BAR);
        assert!(h.with_key_change_moved(4 * BAR, 8 * BAR).is_err());
        assert!(h.with_key_change_moved(5 * BAR, 6 * BAR).is_err());
        let removed = h.with_key_change(4 * BAR, None).unwrap();
        assert_eq!(removed.changes.len(), 1);
        assert_eq!(removed.changes[0].tick, 8 * BAR);
    }

    #[test]
    fn a_new_chord_is_the_tonic_of_the_key_at_its_tick() {
        let h = Harmony {
            key: Some(key(0, ScaleKind::Major)),
            changes: vec![KeyChange {
                tick: 4 * BAR,
                key: key(9, ScaleKind::NaturalMinor),
            }],
            chords: Vec::new(),
        };
        let first = h.suggested_chord(BAR, BAR);
        assert_eq!((first.root, first.quality), (0, ChordKind::Major));
        let later = h.suggested_chord(4 * BAR, BAR);
        assert_eq!((later.root, later.quality), (9, ChordKind::Minor));
        assert_eq!((later.tick, later.length), (4 * BAR, BAR));
        let none = Harmony::default().suggested_chord(0, BAR);
        assert_eq!((none.root, none.quality), (0, ChordKind::Major));
    }

    #[test]
    fn the_chord_tones_include_the_bass() {
        let mut c = chord(0, 1, 0, ChordKind::Major);
        c.bass = Some(7);
        assert!(c.contains(0) && c.contains(4) && c.contains(7) && c.contains(67));
        assert!(!c.contains(2));
        let mut slash = chord(0, 1, 0, ChordKind::Major);
        slash.bass = Some(11);
        assert!(slash.contains(11), "a bass outside the chord is a tone too");
        assert_eq!(c.symbol(), "C/G");
    }
}
