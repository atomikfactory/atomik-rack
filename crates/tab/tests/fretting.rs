//! Fretting properties over every catalogue tuning.

use atomik_tab::fretting::{
    FretEvent, FretOptions, NoteSlot, Placement, candidates, fret_notes, fret_sequence,
};
use atomik_tab::instrument::{Capo, catalogue};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn placements_sound_the_requested_pitch(
        inst in 0usize..24,
        keys in proptest::collection::vec(proptest::collection::vec(30u8..90, 0..5), 1..8),
        capo in 0u8..5,
    ) {
        let cat = catalogue();
        let inst = &cat[inst % cat.len()];
        let tuning = &inst.default_tuning().tuning;
        let capo = Capo::at(capo);
        let events: Vec<FretEvent> = keys
            .iter()
            .map(|k| FretEvent { notes: k.iter().map(|x| NoteSlot::new(*x)).collect() })
            .collect();
        let r = fret_sequence(tuning, capo, &events, None, &FretOptions::default());
        prop_assert_eq!(r.placements.len(), events.len());
        let mut unplayable = 0;
        for (ev, pl) in events.iter().zip(&r.placements) {
            prop_assert_eq!(pl.len(), ev.notes.len());
            let mut used = Vec::new();
            for (note, p) in ev.notes.iter().zip(pl) {
                match p {
                    Some(p) => {
                        prop_assert_eq!(tuning.pitch(p.string, p.fret, capo), Some(note.key));
                        prop_assert!(!used.contains(&p.string), "one note per string");
                        used.push(p.string);
                    }
                    None => unplayable += 1,
                }
            }
        }
        prop_assert_eq!(unplayable, r.unplayable.len());
    }

    #[test]
    fn a_lone_note_is_placed_exactly_when_a_string_can_sound_it(
        inst in 0usize..24,
        key in 30u8..90,
    ) {
        let cat = catalogue();
        let tuning = &cat[inst % cat.len()].default_tuning().tuning;
        let cands = candidates(tuning, Capo::NONE, key);
        let p = fret_notes(tuning, Capo::NONE, &[key], None, &FretOptions::default());
        prop_assert_eq!(p[0].is_some(), !cands.is_empty());
    }

    #[test]
    fn fixed_placements_never_move(
        key in 40u8..80,
        string in 0usize..6,
        fret in 0u8..20,
    ) {
        let cat = catalogue();
        let tuning = &cat[0].default_tuning().tuning;
        let ev = FretEvent { notes: vec![NoteSlot { key, fixed: Some(Placement { string, fret }) }] };
        let r = fret_sequence(tuning, Capo::NONE, &[ev], None, &FretOptions::default());
        prop_assert_eq!(r.placements[0][0], Some(Placement { string, fret }));
    }
}

#[test]
fn deterministic() {
    let tuning = &catalogue()[0].default_tuning().tuning;
    let ev: Vec<FretEvent> = [52, 55, 59, 64, 67, 71]
        .iter()
        .map(|k| FretEvent {
            notes: vec![NoteSlot::new(*k), NoteSlot::new(*k + 5)],
        })
        .collect();
    let a = fret_sequence(tuning, Capo::NONE, &ev, None, &FretOptions::default());
    let b = fret_sequence(tuning, Capo::NONE, &ev, None, &FretOptions::default());
    assert_eq!(a, b);
}

#[test]
fn open_strings_are_preferred_by_default() {
    let tuning = &catalogue()[0].default_tuning().tuning;
    // B3 is the open B string, or the fourth fret of the G string.
    let open = fret_notes(tuning, Capo::NONE, &[59], None, &FretOptions::default());
    assert_eq!(open[0], Some(Placement { string: 1, fret: 0 }));
    let none = FretOptions {
        prefer_open: 0.0,
        ..FretOptions::default()
    };
    let p = fret_notes(tuning, Capo::NONE, &[59], Some(4.0), &none)[0].unwrap();
    assert_eq!(tuning.pitch(p.string, p.fret, Capo::NONE), Some(59));
}
