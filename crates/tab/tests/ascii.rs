//! ASCII tab, end to end and against hostile input.

use atomik_tab::Tuning;
use atomik_tab::ascii::{AsciiOptions, AsciiWriteOptions, parse_ascii, write_ascii};
use atomik_tab::instrument::find_tuning;
use atomik_tab::score::{PPQ, TabNote, TabScore, TabTrack, limits};
use proptest::prelude::*;

fn guitar() -> Tuning {
    find_tuning("guitar6", "standard").unwrap().tuning.clone()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn random_text_never_panics(s in "(.|\n){0,500}") {
        let _ = parse_ascii(&s, &AsciiOptions::default());
    }

    #[test]
    fn tab_like_text_never_panics(s in "([eBGDAE]\\|[-0-9hpbrx/\\\\~()<>|]{0,60}\n){0,10}") {
        if let Ok(p) = parse_ascii(&s, &AsciiOptions::default()) {
            prop_assert!(p.score.note_count() <= limits::MAX_NOTES);
            prop_assert!(p.score.problems().is_empty() || p.score.tracks.iter().all(|t| t.notes.iter().all(|n| n.fret <= 36)));
        }
    }

    #[test]
    fn plain_notes_on_a_grid_round_trip(
        notes in proptest::collection::vec((0i64..32, 0u8..6, 0u8..22), 1..40)
    ) {
        let mut score = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        let mut seen = Vec::new();
        for (slot, string, fret) in notes {
            if seen.contains(&(slot, string)) {
                continue;
            }
            seen.push((slot, string));
            // Sixteenth-note slots over two bars.
            t.notes.push(TabNote::new(slot * PPQ / 4, PPQ / 4, string, fret));
        }
        score.tracks.push(t);
        score.sanitize();
        let text = write_ascii(&score, &AsciiWriteOptions::default());
        let back = parse_ascii(&text, &AsciiOptions::default()).unwrap();
        let key = |s: &TabScore| {
            let mut v: Vec<(i64, u8, u8)> = s.tracks[0].notes.iter().map(|n| (n.tick, n.string, n.fret)).collect();
            v.sort_unstable();
            v
        };
        prop_assert_eq!(key(&back.score), key(&score), "{}", text);
    }
}

#[test]
fn a_banjo_tab_with_its_drone_line() {
    // Five lines: the drone string is the last one, as banjo tab is written.
    let src = "D|-0---0---0---0-|
B|-1---1---1---1-|
G|-0---0---0---0-|
D|-2---2---2---2-|
g|-0---0---0---0-|
";
    let opts = AsciiOptions {
        tuning: Some(find_tuning("banjo5", "open_g").unwrap().tuning.clone()),
        ..AsciiOptions::default()
    };
    let p = parse_ascii(src, &opts).unwrap();
    let t = &p.score.tracks[0];
    assert_eq!(t.instrument_id, "banjo5");
    assert_eq!(t.notes.len(), 20);
    // Fret 0 on the drone string sounds G above the staff.
    let drone = t.notes.iter().find(|n| n.string == 4).unwrap();
    assert_eq!(t.key_of(drone), Some(67));
}

#[test]
fn meter_and_tempo_options_are_used() {
    let src = "e|-0-0-0-|
B|-------|
";
    let opts = AsciiOptions {
        numerator: 3,
        denominator: 4,
        bpm: 90.0,
        ..AsciiOptions::default()
    };
    let p = parse_ascii(src, &opts).unwrap();
    assert_eq!(p.score.tempo, vec![(0, 90.0)]);
    assert_eq!(p.score.meter, vec![(0, 3, 4)]);
    let last = p.score.tracks[0].notes.last().unwrap();
    assert!(last.tick + last.len <= 3 * PPQ);
}

#[test]
fn legend_lists_what_was_used() {
    let mut score = TabScore::new();
    let mut t = TabTrack::new("g", "guitar6", guitar());
    let mut n = TabNote::new(0, PPQ, 1, 7);
    n.tech.flags.insert(atomik_tab::score::TechFlags::DEAD);
    t.notes.push(n);
    score.tracks.push(t);
    let text = write_ascii(&score, &AsciiWriteOptions::default());
    assert!(text.contains("x dead note"), "{text}");
    let none = write_ascii(
        &score,
        &AsciiWriteOptions {
            legend: false,
            ..AsciiWriteOptions::default()
        },
    );
    assert!(!none.contains("Legend"));
}
