//! Transposition properties.

use atomik_tab::ChordSymbol;
use atomik_tab::transpose::{Key, Spelling, capo_shape, transpose_chord, transpose_key};
use proptest::prelude::*;

const NAMES: [&str; 14] = [
    "C", "Dm", "Em7", "F", "G7", "Am", "Bdim", "Bb", "Eb/G", "F#m7b5", "Abmaj7", "Db", "C#sus4",
    "G/B",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn pitch_classes_move_by_the_interval(i in 0usize..14, semis in -30i32..30, sharps in any::<bool>()) {
        let c = ChordSymbol::parse(NAMES[i]).unwrap();
        let sp = if sharps { Spelling::Sharps } else { Spelling::Flats };
        let t = transpose_chord(&c, semis, sp);
        prop_assert_eq!(i32::from(t.root_pc()), (i32::from(c.root_pc()) + semis).rem_euclid(12));
        prop_assert_eq!(t.quality, c.quality);
        prop_assert_eq!(t.extension, c.extension);
        prop_assert_eq!(t.bass.is_some(), c.bass.is_some());
        let back = transpose_chord(&t, -semis, sp);
        prop_assert_eq!(back.root_pc(), c.root_pc());
        prop_assert_eq!(back.bass_pc(), c.bass_pc());
    }

    #[test]
    fn keyless_spelling_avoids_exotic_notes(i in 0usize..14, semis in 0i32..12, sharps in any::<bool>()) {
        let c = ChordSymbol::parse(NAMES[i]).unwrap();
        let sp = if sharps { Spelling::Sharps } else { Spelling::Flats };
        let n = transpose_chord(&c, semis, sp).name();
        for bad in ["E#", "B#", "Cb", "Fb"] {
            prop_assert!(!n.contains(bad), "{n}");
        }
    }

    #[test]
    fn capo_shapes_are_the_inverse_of_capo(i in 0usize..14, capo in 0u8..8) {
        let c = ChordSymbol::parse(NAMES[i]).unwrap();
        let shape = capo_shape(&c, capo, None);
        let sounding = transpose_chord(&shape, i32::from(capo), Spelling::Sharps);
        prop_assert_eq!(sounding.root_pc(), c.root_pc());
    }
}

#[test]
fn every_key_transposes_to_a_parseable_key() {
    for tonic in [
        "C", "C#", "Db", "D", "Eb", "E", "F", "F#", "Gb", "G", "Ab", "A", "Bb", "B",
    ] {
        for minor in [false, true] {
            let k = Key::parse(&format!("{tonic}{}", if minor { "m" } else { "" })).unwrap();
            for semis in -12..=12 {
                let t = transpose_key(k, semis);
                assert_eq!(Key::parse(&t.name()), Some(t));
                assert_eq!(t.minor, minor);
                assert!(t.scale().is_some(), "{} has a clean scale", t.name());
            }
        }
    }
}
