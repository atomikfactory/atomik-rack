//! The song's harmony as the audio thread sees it: the key and the chord at a tick.
//!
//! The song's key, key changes and chord lane are compiled with the song into a
//! [`SongHarmony`] (sorted `Arc` slices, built off the audio thread) that the engine swaps with
//! the song. Every batch the player looks up what holds over the batch and hands devices a small
//! copyable [`HarmonyNow`] inside [`crate::TimeInfo`]: the key and the chord at the first frame
//! and at every frame inside the batch where one of them changes (a key change, a chord start or
//! end, a loop wrap). A device asks it by frame offset ([`HarmonyNow::key_at`],
//! [`HarmonyNow::chord_at`]); nothing allocates or locks.
//!
//! A change lands on the frame where a note at the same tick would be emitted, so a note and the
//! chord starting under it agree. With the transport stopped the batch holds the harmony at the
//! playhead. A song without key and chords gives [`HarmonyNow::NONE`].

use std::sync::Arc;

use crate::theory::{ChordKind, MAX_KEY, Scale};

/// A chord of the song's chord lane: a root, a quality and an optional bass note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SongChord {
    /// The root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The quality.
    pub kind: ChordKind,
    /// The bass pitch class when it is not the root (a slash chord).
    pub bass: Option<u8>,
}

impl SongChord {
    /// A chord on `root` (taken modulo 12) with no bass of its own.
    pub fn new(root: u8, kind: ChordKind) -> Self {
        Self {
            root: root % 12,
            kind,
            bass: None,
        }
    }

    /// The pitch classes of the chord tones and the bass as a 12-bit mask (bit `pc`).
    pub fn pitch_mask(&self) -> u16 {
        let mut mask = 0u16;
        for i in self.kind.intervals() {
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

    /// The chord tone nearest to `key` (`key` itself when it is one), ties down, inside
    /// `0..=127` (a chord-tone lock).
    pub fn nearest(&self, key: u8) -> u8 {
        let key = key.min(MAX_KEY);
        for d in 0..12u8 {
            if let Some(k) = key.checked_sub(d)
                && self.contains(k)
            {
                return k;
            }
            let up = key.saturating_add(d);
            if up <= MAX_KEY && self.contains(up) {
                return up;
            }
        }
        key
    }

    /// The key of the chord root at or below `key` (the root of a chord voiced from `key` up);
    /// the root at or above when no root lies below.
    pub fn root_at_or_below(&self, key: u8) -> u8 {
        let key = key.min(MAX_KEY);
        let down = (key % 12 + 12 - self.root) % 12;
        key.checked_sub(down).unwrap_or(self.root)
    }
}

/// The key and the chord from one frame of a batch on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyMark {
    /// Frame offset inside the batch, `0..BATCH`.
    pub offset: u16,
    /// The song tick the values hold at (the change's own tick, or the tick at the frame).
    pub tick: i64,
    /// The song key (`None`: the song has no key there).
    pub key: Option<Scale>,
    /// The chord of the lane (`None`: a gap in the lane).
    pub chord: Option<SongChord>,
}

/// The most marks one batch holds; changes beyond it in one batch are dropped (a batch is
/// 64 frames, a chord this short is not music).
pub const MAX_HARMONY_MARKS: usize = 8;

const EMPTY_MARK: HarmonyMark = HarmonyMark {
    offset: 0,
    tick: 0,
    key: None,
    chord: None,
};

/// The harmony over one batch (see the module docs). Copyable, fixed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyNow {
    marks: [HarmonyMark; MAX_HARMONY_MARKS],
    len: u8,
}

impl Default for HarmonyNow {
    fn default() -> Self {
        Self::NONE
    }
}

impl HarmonyNow {
    /// No harmony: no song, or a song without key and chords.
    pub const NONE: HarmonyNow = HarmonyNow {
        marks: [EMPTY_MARK; MAX_HARMONY_MARKS],
        len: 0,
    };

    /// One mark for the whole batch.
    pub fn single(tick: i64, key: Option<Scale>, chord: Option<SongChord>) -> Self {
        let mut h = Self::NONE;
        h.push(HarmonyMark {
            offset: 0,
            tick,
            key,
            chord,
        });
        h
    }

    /// Adds a mark. Offsets must not decrease; a mark at the offset of the last one replaces
    /// it, a mark with the last one's values is skipped, and a full batch drops it. Returns
    /// whether it was kept.
    pub fn push(&mut self, mark: HarmonyMark) -> bool {
        let n = usize::from(self.len);
        if n > 0 {
            let last = self.marks[n - 1];
            if mark.offset < last.offset {
                return false;
            }
            if mark.offset == last.offset {
                self.marks[n - 1] = mark;
                return true;
            }
            if last.key == mark.key && last.chord == mark.chord {
                return false;
            }
        }
        if n >= MAX_HARMONY_MARKS {
            return false;
        }
        self.marks[n] = mark;
        self.len += 1;
        true
    }

    /// Forgets every mark.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// The marks, offsets ascending; the first is at offset 0 when there is any.
    pub fn marks(&self) -> &[HarmonyMark] {
        &self.marks[..usize::from(self.len)]
    }

    /// Whether no key and no chord hold anywhere in the batch.
    pub fn is_none(&self) -> bool {
        self.marks()
            .iter()
            .all(|m| m.key.is_none() && m.chord.is_none())
    }

    /// The mark that holds at frame `offset` of the batch.
    pub fn at(&self, offset: u16) -> Option<&HarmonyMark> {
        let marks = self.marks();
        let after = marks.partition_point(|m| m.offset <= offset);
        marks.get(after.checked_sub(1)?)
    }

    /// The song key at frame `offset` of the batch.
    pub fn key_at(&self, offset: u16) -> Option<Scale> {
        self.at(offset).and_then(|m| m.key)
    }

    /// The chord of the lane at frame `offset` of the batch.
    pub fn chord_at(&self, offset: u16) -> Option<SongChord> {
        self.at(offset).and_then(|m| m.chord)
    }
}

/// A key from a tick on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMark {
    /// The first tick of the key.
    pub tick: i64,
    /// The key.
    pub key: Scale,
}

/// A chord over `[tick, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordMark {
    /// The first tick of the chord.
    pub tick: i64,
    /// The first tick after it.
    pub end: i64,
    /// The chord.
    pub chord: SongChord,
}

/// The compiled harmony of a song: the song key, the key changes and the chords, sorted by
/// tick in shared slices, so a clone is cheap and a lookup is a binary search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SongHarmony {
    key: Option<Scale>,
    changes: Option<Arc<[KeyMark]>>,
    chords: Option<Arc<[ChordMark]>>,
}

impl SongHarmony {
    /// Compiles a harmony (allocates: off the audio thread). Key changes are sorted by tick (the
    /// last of one tick wins); chords are sorted by start, empty ones dropped and each one cut
    /// where the next starts, so they never overlap.
    pub fn new(key: Option<Scale>, changes: &[KeyMark], chords: &[ChordMark]) -> Self {
        let mut ch: Vec<KeyMark> = changes.to_vec();
        ch.sort_by_key(|c| c.tick);
        let mut keep: Vec<KeyMark> = Vec::with_capacity(ch.len());
        for c in ch {
            match keep.last_mut() {
                Some(last) if last.tick == c.tick => *last = c,
                _ => keep.push(c),
            }
        }
        let mut cs: Vec<ChordMark> = chords.iter().filter(|c| c.end > c.tick).copied().collect();
        cs.sort_by_key(|c| c.tick);
        for i in 1..cs.len() {
            let start = cs[i].tick;
            if cs[i - 1].end > start {
                cs[i - 1].end = start;
            }
        }
        cs.retain(|c| c.end > c.tick);
        Self {
            key,
            changes: (!keep.is_empty()).then(|| keep.into()),
            chords: (!cs.is_empty()).then(|| cs.into()),
        }
    }

    /// Whether the song has no key, no key change and no chord.
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && self.changes.is_none() && self.chords.is_none()
    }

    fn change_list(&self) -> &[KeyMark] {
        self.changes.as_deref().unwrap_or(&[])
    }

    fn chord_list(&self) -> &[ChordMark] {
        self.chords.as_deref().unwrap_or(&[])
    }

    /// The key at `tick`: the last change at or before it, else the song key.
    pub fn key_at(&self, tick: i64) -> Option<Scale> {
        let ch = self.change_list();
        match ch.partition_point(|c| c.tick <= tick).checked_sub(1) {
            Some(i) => Some(ch[i].key),
            None => self.key,
        }
    }

    /// The chord that holds at `tick`.
    pub fn chord_at(&self, tick: i64) -> Option<SongChord> {
        let cs = self.chord_list();
        let c = cs[cs.partition_point(|c| c.tick <= tick).checked_sub(1)?];
        (tick < c.end).then_some(c.chord)
    }

    /// The first tick after `tick` where the key or the chord may change: a key change, a chord
    /// start or a chord end.
    pub fn next_change_after(&self, tick: i64) -> Option<i64> {
        let ch = self.change_list();
        let key = ch
            .get(ch.partition_point(|c| c.tick <= tick))
            .map(|c| c.tick);
        let cs = self.chord_list();
        let i = cs.partition_point(|c| c.tick <= tick);
        // The chord holding at `tick` ends after it; otherwise the next one starts.
        let chord = match i.checked_sub(1).map(|j| cs[j]) {
            Some(c) if c.end > tick => Some(c.end),
            _ => cs.get(i).map(|c| c.tick),
        };
        match (key, chord) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The mark of `tick` at frame `offset` of a batch.
    pub fn mark(&self, offset: u16, tick: i64) -> HarmonyMark {
        HarmonyMark {
            offset,
            tick,
            key: self.key_at(tick),
            chord: self.chord_at(tick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::ScaleKind;

    fn c_major() -> Scale {
        Scale::new(0, ScaleKind::Major)
    }

    fn harmony() -> SongHarmony {
        SongHarmony::new(
            Some(c_major()),
            &[KeyMark {
                tick: 1000,
                key: Scale::new(9, ScaleKind::NaturalMinor),
            }],
            &[
                ChordMark {
                    tick: 0,
                    end: 500,
                    chord: SongChord::new(0, ChordKind::Major),
                },
                ChordMark {
                    tick: 600,
                    end: 2000,
                    chord: SongChord::new(9, ChordKind::Minor),
                },
            ],
        )
    }

    #[test]
    fn key_and_chord_lookups_follow_the_lane() {
        let h = harmony();
        assert_eq!(h.key_at(0), Some(c_major()));
        assert_eq!(h.key_at(999), Some(c_major()));
        assert_eq!(h.key_at(1000).map(|k| k.root), Some(9));
        assert_eq!(h.chord_at(0).map(|c| c.root), Some(0));
        assert_eq!(h.chord_at(499).map(|c| c.root), Some(0));
        assert_eq!(h.chord_at(500), None, "a gap is no chord");
        assert_eq!(h.chord_at(600).map(|c| c.root), Some(9));
        assert_eq!(h.chord_at(2000), None);
        assert_eq!(h.chord_at(-5), None);
    }

    #[test]
    fn next_change_walks_every_boundary() {
        let h = harmony();
        let mut t = -1;
        let mut seen = Vec::new();
        while let Some(n) = h.next_change_after(t) {
            seen.push(n);
            t = n;
        }
        assert_eq!(seen, vec![0, 500, 600, 1000, 2000]);
        assert!(SongHarmony::default().next_change_after(0).is_none());
        assert!(SongHarmony::default().is_empty());
    }

    #[test]
    fn overlapping_chords_are_cut_and_changes_deduplicated() {
        let c = SongChord::new(0, ChordKind::Major);
        let h = SongHarmony::new(
            None,
            &[
                KeyMark {
                    tick: 5,
                    key: c_major(),
                },
                KeyMark {
                    tick: 5,
                    key: Scale::new(2, ScaleKind::Dorian),
                },
            ],
            &[
                ChordMark {
                    tick: 100,
                    end: 300,
                    chord: c,
                },
                ChordMark {
                    tick: 0,
                    end: 200,
                    chord: c,
                },
                ChordMark {
                    tick: 50,
                    end: 50,
                    chord: c,
                },
            ],
        );
        assert_eq!(h.key_at(5).map(|k| k.root), Some(2));
        assert_eq!(h.key_at(4), None);
        assert_eq!(h.next_change_after(0), Some(5));
        assert_eq!(h.next_change_after(5), Some(100));
        assert_eq!(h.chord_at(150), Some(c));
    }

    #[test]
    fn harmony_now_answers_by_offset() {
        let a = SongChord::new(0, ChordKind::Major);
        let b = SongChord::new(7, ChordKind::Seventh);
        let mut now = HarmonyNow::single(10, Some(c_major()), Some(a));
        assert!(now.push(HarmonyMark {
            offset: 20,
            tick: 40,
            key: Some(c_major()),
            chord: Some(b),
        }));
        // Same values again: skipped.
        assert!(!now.push(HarmonyMark {
            offset: 30,
            tick: 50,
            key: Some(c_major()),
            chord: Some(b),
        }));
        assert_eq!(now.marks().len(), 2);
        assert_eq!(now.chord_at(0), Some(a));
        assert_eq!(now.chord_at(19), Some(a));
        assert_eq!(now.chord_at(20), Some(b));
        assert_eq!(now.chord_at(63), Some(b));
        assert_eq!(now.key_at(63), Some(c_major()));
        assert!(!now.is_none());
        assert!(HarmonyNow::NONE.is_none());
        assert_eq!(HarmonyNow::NONE.chord_at(0), None);
    }

    #[test]
    fn harmony_now_keeps_at_most_its_capacity() {
        let mut now = HarmonyNow::NONE;
        for i in 0..20u16 {
            now.push(HarmonyMark {
                offset: i,
                tick: i64::from(i),
                key: None,
                chord: Some(SongChord::new((i % 12) as u8, ChordKind::Major)),
            });
        }
        assert_eq!(now.marks().len(), MAX_HARMONY_MARKS);
    }

    #[test]
    fn chord_tone_lock_and_root_placement() {
        let am = SongChord::new(9, ChordKind::Minor); // A C E
        assert_eq!(am.nearest(60), 60);
        assert_eq!(am.nearest(62), 60, "D is between C and E: ties down");
        assert_eq!(am.nearest(63), 64);
        assert_eq!(am.nearest(127), 124);
        assert_eq!(am.root_at_or_below(60), 57);
        assert_eq!(am.root_at_or_below(57), 57);
        assert_eq!(am.root_at_or_below(3), 9);
        let slash = SongChord {
            bass: Some(7),
            ..SongChord::new(0, ChordKind::Major)
        };
        assert!(slash.contains(55));
        assert_eq!(slash.pitch_mask().count_ones(), 3);
    }
}
