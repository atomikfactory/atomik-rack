//! Chord sheet reading, writing and transposing, end to end.

use atomik_tab::ChordSymbol;
use atomik_tab::sheet::{ChordSheet, SheetBlock, parse_inline, parse_over_lyrics, parse_sheet};
use proptest::prelude::*;

const SONG: &str = "Title: Moonlit Kitchen
Artist: Made Up Trio
Key: C

[Verse]
C            Am
Cooking soup at midnight
F            G
Stirring in the dark
[Chorus]
        C     G
Spoon, spoon, spoon
Am       F
Wooden spoon
";

#[test]
fn plain_to_inline_to_plain() {
    let a = parse_over_lyrics(SONG).unwrap().sheet;
    let inline = a.to_inline();
    assert!(inline.contains("{title: Moonlit Kitchen}"));
    assert!(inline.contains("[C]Cooking"));
    let b = parse_inline(&inline).unwrap().sheet;
    assert_eq!(a, b);
    let plain = b.to_over_lyrics();
    assert_eq!(parse_over_lyrics(&plain).unwrap().sheet, a);
}

#[test]
fn transposing_the_whole_sheet() {
    let mut s = parse_sheet(SONG).unwrap().sheet;
    s.transpose(2, None);
    assert_eq!(s.key.map(|k| k.name()), Some("D".to_owned()));
    let used: Vec<String> = s.chords_used().iter().map(ChordSymbol::name).collect();
    assert_eq!(used, vec!["D", "Bm", "G", "A"]);
    // Anchors do not move.
    match &s.blocks[2] {
        SheetBlock::Line { chords, .. } => assert_eq!(chords[1].0, 13),
        b => panic!("{b:?}"),
    }
}

#[test]
fn capo_view_round_trips_through_concert_pitch() {
    let s = parse_sheet(SONG).unwrap().sheet;
    let shapes = s.with_capo(2);
    assert_eq!(shapes.capo, 2);
    let used: Vec<String> = shapes.chords_used().iter().map(ChordSymbol::name).collect();
    assert_eq!(used, vec!["Bb", "Gm", "Eb", "F"]);
    assert_eq!(shapes.with_capo(0).chords_used(), s.chords_used());
}

#[test]
fn bar_anchors_are_kept_in_the_model() {
    let mut s = ChordSheet::default();
    s.blocks.push(SheetBlock::Line {
        text: "la la".into(),
        chords: vec![(0, ChordSymbol::parse("C").unwrap())],
        bar: Some(4),
    });
    let t = s.clone();
    assert_eq!(s, t);
    // The text forms do not carry anchors.
    let back = parse_inline(&s.to_inline()).unwrap().sheet;
    match &back.blocks[0] {
        SheetBlock::Line { bar, .. } => assert_eq!(*bar, None),
        b => panic!("{b:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_text_never_panics_or_overflows_caps(s in "(.|\n){0,600}") {
        for p in [parse_sheet(&s), parse_inline(&s), parse_over_lyrics(&s)].into_iter().flatten() {
            // Writing what was read must work and be readable again.
            let t = p.sheet.to_over_lyrics();
            prop_assert!(parse_over_lyrics(&t).is_ok());
            let t = p.sheet.to_inline();
            prop_assert!(parse_inline(&t).is_ok());
        }
    }

    #[test]
    fn bracket_text_never_panics(s in "[\\[\\]{}a-gA-G#b0-9:/ \n]{0,300}") {
        let _ = parse_sheet(&s);
    }

    #[test]
    fn generated_sheets_round_trip(
        lines in proptest::collection::vec(
            (proptest::collection::vec((0usize..4, 0usize..6), 0..4), "[a-z]{3,8}( [a-z]{3,8}){1,4}"),
            1..6,
        )
    ) {
        let names = ["C", "G", "Am", "F", "Dm7", "E7"];
        let mut sheet = ChordSheet::default();
        for (chords, text) in &lines {
            let mut placed: Vec<(usize, ChordSymbol)> = Vec::new();
            let mut at = 0usize;
            for (gap, which) in chords {
                at += gap * 5 + 4;
                if at < text.chars().count() {
                    placed.push((at, ChordSymbol::parse(names[*which]).unwrap()));
                }
            }
            sheet.blocks.push(SheetBlock::Line { text: text.clone(), chords: placed, bar: None });
        }
        let plain = sheet.to_over_lyrics();
        let again = parse_over_lyrics(&plain).unwrap().sheet;
        prop_assert_eq!(&again, &sheet, "{}", plain);
        let inline = sheet.to_inline();
        let again = parse_inline(&inline).unwrap().sheet;
        prop_assert_eq!(&again, &sheet, "{}", inline);
    }
}
