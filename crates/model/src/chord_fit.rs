//! Fitting notes to the chord lane: a note that falls outside the chord at its position moves to the
//! nearest chord tone.
//!
//! The chord at a note is the chord that holds at the note's start in song ticks. A note on a
//! chord tone (or the bass) stays; a note with no chord under it stays; any other note moves by
//! the fewest semitones to a key that is a chord tone, a tie going down. A chord tone already
//! taken by another note at the same start is passed over for the next nearest one, and a note
//! with no free tone nearby stays, so no note is ever lost or merged.

use crate::harmony::ChordEvent;
use crate::song::Note;

/// The keys within an octave of `key` (within `0..=127`) that are tones of `chord`, nearest first,
/// a tie going down.
fn candidates(key: u8, chord: &ChordEvent) -> impl Iterator<Item = u8> + '_ {
    (1..=12i32).flat_map(move |d| {
        let down = i32::from(key) - d;
        let up = i32::from(key) + d;
        [down, up]
            .into_iter()
            .filter(|k| (0..=127).contains(k))
            .map(|k| k as u8)
            .filter(|k| chord.contains(*k))
    })
}

/// Moves the notes at the indices `selected` onto the tones of the chord at their start.
/// `origin` is the song tick of content tick 0 (a clip's start minus its offset). Returns the whole
/// note list in its own order (so the indices of a selection stay valid; the caller sorts it as
/// every note edit does), or `None` when no note moved.
pub fn fit_to_chords(
    notes: &[Note],
    selected: &[usize],
    chords: &[ChordEvent],
    origin: i64,
) -> Option<Vec<Note>> {
    let chord_at = |tick: i64| -> Option<&ChordEvent> {
        let after = chords.partition_point(|c| c.tick <= tick);
        let c = &chords[after.checked_sub(1)?];
        (tick < c.end()).then_some(c)
    };
    let mut out = notes.to_vec();
    let mut moved = false;
    let mut order: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|i| *i < out.len())
        .collect();
    order.sort_unstable();
    order.dedup();
    for i in order {
        let note = out[i];
        let Some(chord) = chord_at(origin.saturating_add(note.tick)) else {
            continue;
        };
        if chord.contains(note.key) {
            continue;
        }
        let free = |k: u8| {
            !out.iter()
                .enumerate()
                .any(|(j, n)| j != i && n.tick == note.tick && n.key == k)
        };
        if let Some(k) = candidates(note.key, chord).find(|k| free(*k)) {
            out[i].key = k;
            moved = true;
        }
    }
    if !moved {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::ChordKind;
    use atomik_core::PPQ;

    const BAR: i64 = PPQ * 4;

    fn lane() -> Vec<ChordEvent> {
        vec![
            ChordEvent::new(0, BAR, 0, ChordKind::Major),
            ChordEvent::new(BAR, BAR, 9, ChordKind::Minor),
        ]
    }

    fn keys(notes: &[Note]) -> Vec<u8> {
        notes.iter().map(|n| n.key).collect()
    }

    fn note(tick: i64, key: u8) -> Note {
        Note::new(tick, PPQ, key, 0.8)
    }

    #[test]
    fn a_note_outside_the_chord_moves_to_the_nearest_tone() {
        // C major has C, E and G.
        let notes = [note(0, 61), note(PPQ, 62), note(2 * PPQ, 63)];
        let out = fit_to_chords(&notes, &[0, 1, 2], &lane(), 0).unwrap();
        // 61 -> 60 (1 down), 62 -> 60 or 64 (2 each: ties go down), 63 -> 64 (1 up).
        assert_eq!(keys(&out), [60, 60, 64]);
    }

    #[test]
    fn notes_on_chord_tones_and_the_bass_stay() {
        let mut lane = lane();
        lane[0].bass = Some(2);
        let notes = [
            note(0, 60),
            note(0, 64),
            note(0, 67),
            note(0, 62),
            note(0, 72),
        ];
        assert_eq!(fit_to_chords(&notes, &[0, 1, 2, 3, 4], &lane, 0), None);
    }

    #[test]
    fn only_the_selected_notes_move() {
        let notes = [note(0, 61), note(PPQ, 61)];
        let out = fit_to_chords(&notes, &[1], &lane(), 0).unwrap();
        assert_eq!(keys(&out), [61, 60]);
    }

    #[test]
    fn each_note_follows_the_chord_at_its_own_start() {
        // 66 is F#: in C major it is nearest to G (67); in A minor (A C E) nearest to E(64)? 64 is 2
        // down and 69 is 3 up.
        let notes = [note(0, 66), note(BAR, 66)];
        let out = fit_to_chords(&notes, &[0, 1], &lane(), 0).unwrap();
        assert_eq!(keys(&out), [67, 64]);
    }

    #[test]
    fn a_note_with_no_chord_under_it_stays() {
        let notes = [note(5 * BAR, 61)];
        assert_eq!(fit_to_chords(&notes, &[0], &lane(), 0), None);
        assert_eq!(fit_to_chords(&notes, &[0], &[], 0), None);
    }

    #[test]
    fn the_clip_origin_places_the_notes_in_the_song() {
        // The clip starts at bar 2: its first note sits on the A minor chord.
        let notes = [note(0, 61)];
        let out = fit_to_chords(&notes, &[0], &lane(), BAR).unwrap();
        assert_eq!(keys(&out), [60], "C is the nearest A minor tone");
        let out = fit_to_chords(&notes, &[0], &lane(), BAR - 100).unwrap();
        assert_eq!(keys(&out), [60]);
    }

    #[test]
    fn a_taken_tone_is_passed_over_and_no_note_is_lost() {
        let notes = [note(0, 60), note(0, 61)];
        let out = fit_to_chords(&notes, &[1], &lane(), 0).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(keys(&out), [60, 64], "C is taken, E is the next nearest");
    }

    #[test]
    fn the_result_keeps_the_order_and_every_other_field() {
        let mut n = note(0, 61);
        n.velocity = 0.3;
        n.muted = true;
        let notes = [n, note(0, 63)];
        let out = fit_to_chords(&notes, &[0, 1], &lane(), 0).unwrap();
        assert!(
            out.windows(2)
                .all(|w| (w[0].tick, w[0].key) <= (w[1].tick, w[1].key))
        );
        let moved = out.iter().find(|x| x.key == 60).unwrap();
        assert!(moved.muted && (moved.velocity - 0.3).abs() < f32::EPSILON);
        assert_eq!(moved.length, PPQ);
    }

    #[test]
    fn the_edges_of_the_key_range_hold() {
        let lane = [ChordEvent::new(0, BAR, 0, ChordKind::Major)];
        // Key 1 is C#: C (key 0) is the nearest tone. Key 127 is G, already a tone.
        let notes = [note(0, 1), note(0, 127)];
        let out = fit_to_chords(&notes, &[0, 1], &lane, 0).unwrap();
        assert_eq!(keys(&out), [0, 127]);
    }
}
