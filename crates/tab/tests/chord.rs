//! Chord-name grammar properties.

use atomik_tab::ChordSymbol;
use atomik_tab::chord::{Degree, Extension, Note, Quality};
use proptest::prelude::*;

fn arb_chord() -> impl Strategy<Value = ChordSymbol> {
    let quality = prop_oneof![
        Just(Quality::Major),
        Just(Quality::Minor),
        Just(Quality::Dim),
        Just(Quality::Aug),
        Just(Quality::Sus2),
        Just(Quality::Sus4),
    ];
    let ext = prop_oneof![
        Just(Extension::None),
        Just(Extension::Six),
        Just(Extension::SixNine),
        Just(Extension::Seventh),
        Just(Extension::MajSeventh),
        Just(Extension::Ninth),
        Just(Extension::Eleventh),
        Just(Extension::Thirteenth),
    ];
    (
        0u8..7,
        -1i8..=1,
        quality,
        ext,
        proptest::option::of((0u8..7, -1i8..=1)),
        proptest::collection::vec(0usize..4, 0..3),
    )
        .prop_map(|(l, a, q, e, bass, alts)| {
            let mut c = ChordSymbol::new(Note::new(l, a), q, e);
            for k in alts {
                let d = [(-1, 9), (1, 9), (1, 11), (-1, 13)][k];
                let deg = Degree {
                    accidental: d.0,
                    degree: d.1,
                };
                if !c.alterations.contains(&deg) {
                    c.alterations.push(deg);
                }
            }
            c.bass = bass
                .map(|(l, a)| Note::new(l, a))
                .filter(|b| b.pitch_class() != c.root.pitch_class());
            c
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn formatting_then_parsing_gives_the_same_chord(c in arb_chord()) {
        let name = c.name();
        let back = ChordSymbol::parse(&name).unwrap_or_else(|e| panic!("{name}: {e}"));
        prop_assert_eq!(back.name(), name.clone());
        prop_assert_eq!(&back, &c, "{}", name);
    }

    #[test]
    fn arbitrary_text_never_panics(s in ".{0,80}") {
        let _ = ChordSymbol::parse(&s);
    }

    #[test]
    fn every_parsed_chord_has_a_root_tone(s in "[A-G][#b]?[a-z0-9#b()/+-]{0,10}") {
        if let Ok(c) = ChordSymbol::parse(&s) {
            let tones = c.tones();
            prop_assert!(!tones.is_empty());
            prop_assert_eq!(tones[0].pc, c.root_pc());
            for t in &tones {
                prop_assert!(t.pc < 12);
            }
        }
    }
}

#[test]
fn unicode_accidentals_and_symbols() {
    assert_eq!(
        ChordSymbol::parse("B\u{266d}maj7").unwrap().name(),
        "Bbmaj7"
    );
    assert_eq!(
        ChordSymbol::parse("F\u{266f}\u{f8}").unwrap().name(),
        "F#m7b5"
    );
    assert_eq!(ChordSymbol::parse("C\u{394}7").unwrap().name(), "Cmaj7");
    assert_eq!(ChordSymbol::parse("C\u{b0}7").unwrap().name(), "Cdim7");
}
