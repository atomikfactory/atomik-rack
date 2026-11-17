//! The PDF printouts: pure layout, the file structure and golden files.
//!
//! The fixtures are written here (made-up music and lyrics). The golden files in `tests/golden`
//! keep the plain (uncompressed) form of three small printouts; regenerate them with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-tab --test pdf` after a layout change that is meant.

use atomik_tab::chord::ChordSymbol;
use atomik_tab::instrument::{Capo, find_tuning};
use atomik_tab::pdf::draw::{Anchor, Ink, Op};
use atomik_tab::pdf::{
    Layout, PageSize, PdfError, SheetDiagrams, SheetPdfOptions, TabPdfOptions, layout_sheet,
    layout_tab, write_sheet_pdf, write_tab_pdf,
};
use atomik_tab::score::{
    Bend, Harmonic, HarmonicKind, PPQ, SlideKind, TabNote, TabScore, TabTrack, TechFlags,
    VibratoKind,
};
use atomik_tab::sheet::{ChordSheet, SheetBlock, parse_sheet};
use std::io::Read;
use std::path::PathBuf;

fn track(instrument: &str, tuning: &str, name: &str) -> TabTrack {
    TabTrack::new(
        name,
        instrument,
        find_tuning(instrument, tuning)
            .expect("a tuning")
            .tuning
            .clone(),
    )
}

fn note(tick: i64, len: i64, string: u8, fret: u8) -> TabNote {
    TabNote::new(tick, len, string, fret)
}

/// A guitar tab of four bars that uses most techniques.
fn guitar_score() -> TabScore {
    let mut s = TabScore::new();
    s.title = "Evening Practice".into();
    s.subtitle = "Four bars for the left hand".into();
    s.artist = "Test Ensemble".into();
    s.music_by = "A. Writer".into();
    s.tempo = vec![(0, 96.0)];
    s.markers = vec![(0, "Intro".into()), (8 * PPQ, "Verse".into())];
    let mut t = track("guitar6", "standard", "Guitar");
    t.capo = Capo::at(2);
    let e = PPQ / 2;
    // Bar 1: palm-muted eighths.
    let strings = [4u8, 3, 2, 1, 2, 3, 4, 3];
    let frets = [0u8, 2, 2, 1, 2, 2, 0, 2];
    for k in 0..8 {
        let mut n = note(k as i64 * e, e, strings[k], frets[k]);
        if k < 4 {
            n.tech.flags.insert(TechFlags::PALM_MUTE);
        }
        t.notes.push(n);
    }
    // Bar 2: a bend, a slide, a hammer-on and a pull-off.
    let mut bend = note(4 * PPQ, PPQ, 1, 8);
    bend.tech.bend = Some(Bend::up(200));
    t.notes.push(bend);
    let mut slide = note(5 * PPQ, PPQ, 2, 5);
    slide.tech.slide = Some(SlideKind::Legato);
    t.notes.push(slide);
    let mut hammer = note(6 * PPQ, PPQ, 2, 7);
    hammer.tech.flags.insert(TechFlags::HAMMER_ON);
    t.notes.push(hammer);
    let mut pull = note(7 * PPQ, PPQ, 2, 5);
    pull.tech.flags.insert(TechFlags::PULL_OFF);
    t.notes.push(pull);
    // Bar 3: harmonic, dead, ghost, vibrato, let ring.
    let mut harm = note(8 * PPQ, 2 * PPQ, 0, 12);
    harm.tech.harmonic = Some(Harmonic {
        kind: HarmonicKind::Natural,
        fret: None,
    });
    t.notes.push(harm);
    let mut dead = note(10 * PPQ, PPQ / 2, 3, 0);
    dead.tech.flags.insert(TechFlags::DEAD);
    t.notes.push(dead);
    let mut ghost = note(10 * PPQ + e, PPQ / 2, 2, 4);
    ghost.tech.flags.insert(TechFlags::GHOST);
    t.notes.push(ghost);
    let mut vib = note(11 * PPQ, PPQ, 1, 7);
    vib.tech.vibrato = Some(VibratoKind::Normal);
    vib.tech.flags.insert(TechFlags::LET_RING);
    t.notes.push(vib);
    // Bar 4: triplets and a tapped note.
    for k in 0..3 {
        t.notes
            .push(note(12 * PPQ + k * PPQ / 3, PPQ / 3, (k as u8) + 1, 3));
    }
    let mut tap = note(13 * PPQ, PPQ, 0, 15);
    tap.tech.flags.insert(TechFlags::TAP);
    t.notes.push(tap);
    t.notes.push(note(14 * PPQ, 2 * PPQ, 5, 3));
    t.chords = vec![
        (0, ChordSymbol::parse("Am").unwrap()),
        (4 * PPQ, ChordSymbol::parse("F").unwrap()),
        (8 * PPQ, ChordSymbol::parse("C").unwrap()),
        (12 * PPQ, ChordSymbol::parse("G7").unwrap()),
    ];
    t.lyrics = vec![
        (0, "Qui".into()),
        (PPQ, "et".into()),
        (2 * PPQ, "lights".into()),
        (4 * PPQ, "glow".into()),
    ];
    s.tracks.push(t);
    s
}

/// A banjo with the drone string, rolls over two bars.
fn banjo_score() -> TabScore {
    let mut s = TabScore::new();
    s.title = "Porch Roll".into();
    s.tempo = vec![(0, 110.0)];
    let mut t = track("banjo5", "open_g", "Banjo");
    let e = PPQ / 2;
    for bar in 0..2i64 {
        for k in 0..8i64 {
            let string = [4u8, 1, 0, 4, 1, 0, 4, 1][k as usize];
            let fret = if string == 4 {
                if bar == 1 && k == 0 { 7 } else { 0 }
            } else if k % 3 == 0 {
                2
            } else {
                0
            };
            t.notes.push(note((bar * 8 + k) * e, e, string, fret));
        }
    }
    s.tracks.push(t);
    s
}

fn many_bars(bars: i64, instrument: &str, tuning: &str) -> TabScore {
    let mut s = TabScore::new();
    s.title = "Long".into();
    let mut t = track(instrument, tuning, "Part");
    let strings = t.tuning.strings.len() as u8;
    for b in 0..bars {
        for k in 0..8i64 {
            t.notes.push(note(
                b * 4 * PPQ + k * PPQ / 2,
                PPQ / 2,
                (k as u8) % strings,
                (k as u8) % 5,
            ));
        }
    }
    s.tracks.push(t);
    s
}

const SHEET: &str = "title: Lantern Song
artist: Test Ensemble
key: G
capo: 2

[Verse 1]
G        D
Lanterns glow along the river
Em          C
Quiet steps and morning silver
# repeat twice
[Chorus]
C       G
Carry on, carry on, carry me home along the winding path tonight
";

fn sheet() -> ChordSheet {
    parse_sheet(SHEET).expect("a sheet").sheet
}

fn count_staff_lines(l: &Layout, page: usize) -> usize {
    l.pages[page]
        .ops
        .iter()
        .filter(|o| {
            matches!(o, Op::Line { a, b, width, ink: Ink::Black, .. }
                if (a.1 - b.1).abs() < 1e-3 && (*width - 0.5).abs() < 1e-3 && b.0 - a.0 > 60.0)
        })
        .count()
}

/// The numbers printed at the start of each system (small mid-grey texts).
fn system_starts(l: &Layout) -> Vec<usize> {
    l.pages
        .iter()
        .flat_map(|p| p.ops.iter())
        .filter_map(|o| match o {
            Op::Text {
                size,
                ink: Ink::Mid,
                anchor: Anchor::Start,
                text,
                ..
            } if (*size - 6.5).abs() < 1e-3 => text.parse().ok(),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- a tiny reader

fn streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    let find = |from: usize, pat: &[u8]| -> Option<usize> {
        pdf[from..]
            .windows(pat.len())
            .position(|w| w == pat)
            .map(|p| p + from)
    };
    while let Some(s) = find(i, b"stream\n") {
        // Skip the `stream` of `endstream`.
        if s >= 3 && &pdf[s - 3..s] == b"end" {
            i = s + 7;
            continue;
        }
        let start = s + 7;
        let Some(end) = find(start, b"\nendstream") else {
            break;
        };
        let raw = &pdf[start..end];
        let dict_start = pdf[..s].windows(2).rposition(|w| w == b"<<").unwrap_or(0);
        let compressed = pdf[dict_start..s].windows(11).any(|w| w == b"FlateDecode");
        if compressed {
            let mut buf = Vec::new();
            flate2::read::ZlibDecoder::new(raw)
                .read_to_end(&mut buf)
                .expect("a zlib stream");
            out.push(buf);
        } else {
            out.push(raw.to_vec());
        }
        i = end + 10;
    }
    out
}

/// The strings shown by `Tj`, in order, of one content stream.
fn shown(content: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < content.len() {
        if content[i] == b'(' {
            let mut depth = 1;
            let mut s = Vec::new();
            i += 1;
            while i < content.len() && depth > 0 {
                match content[i] {
                    b'\\' if i + 1 < content.len() => {
                        i += 1;
                        s.push(content[i]);
                    }
                    b'(' => {
                        depth += 1;
                        s.push(b'(');
                    }
                    b')' => {
                        depth -= 1;
                        if depth > 0 {
                            s.push(b')');
                        }
                    }
                    c => s.push(c),
                }
                i += 1;
            }
            out.push(s.iter().map(|b| *b as char).collect());
        } else if content[i] == b'<' {
            // A hex string (text with a byte outside ASCII).
            let end = content[i..]
                .iter()
                .position(|b| *b == b'>')
                .map_or(content.len(), |p| p + i);
            let hex: Vec<u8> = content[i + 1..end]
                .iter()
                .filter(|b| b.is_ascii_hexdigit())
                .copied()
                .collect();
            let bytes: Vec<char> = hex
                .chunks(2)
                .map(|p| {
                    let s: String = p.iter().map(|b| *b as char).collect();
                    u8::from_str_radix(&s, 16).unwrap_or(b'?') as char
                })
                .collect();
            out.push(bytes.into_iter().collect());
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn page_count(pdf: &[u8]) -> usize {
    let text: String = pdf.iter().map(|b| *b as char).collect();
    text.matches("/Type /Page\n").count() + text.matches("/Type /Page ").count()
}

// ---------------------------------------------------------------- tab layout

#[test]
fn a_guitar_tab_prints_its_content_and_techniques() {
    let l = layout_tab(&guitar_score(), &TabPdfOptions::default()).unwrap();
    assert_eq!(l.pages.len(), 1);
    let texts = l.texts();
    for want in [
        "Evening Practice",
        "Test Ensemble",
        "Intro",
        "Verse",
        "= 96",
        "Am",
        "G7",
        "full",
        "P.M.",
        "let ring",
        "<12>",
        "(4)",
        "x",
        "Qui",
        "1 / 1",
    ] {
        assert!(texts.contains(&want), "missing {want:?} in {texts:?}");
    }
    assert!(texts.iter().any(|t| t.contains("Capo 2")));
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Tuning, bottom line first: E A D G B E"))
    );
    // Hammer-on and pull-off letters, a time signature, the bar number.
    assert!(texts.contains(&"H") && texts.contains(&"P"));
    assert!(texts.contains(&"4"), "the time signature");
    assert_eq!(system_starts(&l), vec![1]);
    assert_eq!(count_staff_lines(&l, 0), 6);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
}

#[test]
fn bend_amounts_name_the_interval() {
    let mut s = guitar_score();
    s.tracks[0].notes[8].tech.bend = Some(Bend::bend_release(100));
    let l = layout_tab(&s, &TabPdfOptions::default()).unwrap();
    assert!(l.texts().contains(&"1/2"));
}

#[test]
fn the_staff_has_a_line_per_string() {
    for (instrument, tuning, lines) in [
        ("bass4", "standard", 4),
        ("bass5", "standard", 5),
        ("guitar7", "standard", 7),
        ("guitar8", "standard", 8),
        ("ukulele_soprano", "reentrant", 4),
    ] {
        let s = many_bars(2, instrument, tuning);
        let l = layout_tab(&s, &TabPdfOptions::default()).unwrap();
        assert_eq!(
            count_staff_lines(&l, 0),
            lines * system_starts(&l).len(),
            "{instrument}"
        );
    }
}

#[test]
fn a_banjo_prints_five_lines_and_names_the_drone() {
    let l = layout_tab(&banjo_score(), &TabPdfOptions::default()).unwrap();
    assert_eq!(count_staff_lines(&l, 0), 5);
    assert!(
        l.texts().iter().any(|t| t.contains("starts at fret 5")),
        "{:?}",
        l.texts()
    );
    assert!(l.texts().contains(&"7"), "the fretted drone");
}

#[test]
fn bars_fill_the_line_and_a_new_system_starts_when_full() {
    let l = layout_tab(
        &many_bars(24, "guitar6", "standard"),
        &TabPdfOptions::default(),
    )
    .unwrap();
    let starts = system_starts(&l);
    assert!(starts.len() >= 3, "{starts:?}");
    assert_eq!(starts[0], 1);
    assert!(starts.windows(2).all(|w| w[1] > w[0]));
    // Every system but the last ends at the right margin (the staff lines are stretched).
    let right = PageSize::A4.points().0 - 42.0;
    let widest = l.pages[0]
        .ops
        .iter()
        .filter_map(|o| match o {
            Op::Line { b, width, .. } if (*width - 0.5).abs() < 1e-3 => Some(b.0),
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!((widest - right).abs() < 1.5, "{widest} vs {right}");
}

#[test]
fn a_fixed_number_of_bars_per_line_is_honoured() {
    let opts = TabPdfOptions {
        bars_per_line: Some(2),
        ..TabPdfOptions::default()
    };
    let l = layout_tab(&many_bars(7, "guitar6", "standard"), &opts).unwrap();
    assert_eq!(system_starts(&l), vec![1, 3, 5, 7]);
    let opts = TabPdfOptions {
        bars_per_line: Some(4),
        ..TabPdfOptions::default()
    };
    let l = layout_tab(&many_bars(7, "guitar6", "standard"), &opts).unwrap();
    assert_eq!(system_starts(&l), vec![1, 5]);
}

#[test]
fn pages_break_between_systems_and_carry_numbers() {
    let opts = TabPdfOptions {
        bars_per_line: Some(2),
        ..TabPdfOptions::default()
    };
    let l = layout_tab(&many_bars(80, "guitar6", "standard"), &opts).unwrap();
    assert!(l.pages.len() >= 2, "{}", l.pages.len());
    let total = l.pages.len();
    for (i, p) in l.pages.iter().enumerate() {
        assert!(
            p.texts()
                .contains(&format!("{} / {}", i + 1, total).as_str())
        );
    }
    // No staff line sits below the bottom margin.
    let limit = PageSize::A4.points().1 - 42.0;
    for p in &l.pages {
        for o in &p.ops {
            if let Op::Line { a, b, .. } = o {
                assert!(a.1 <= limit && b.1 <= limit);
            }
        }
    }
}

#[test]
fn letter_paper_changes_the_page_size() {
    let opts = TabPdfOptions {
        page: PageSize::Letter,
        ..TabPdfOptions::default()
    };
    let l = layout_tab(&guitar_score(), &opts).unwrap();
    assert_eq!(l.size, (612.0, 792.0));
    let pdf = l.to_pdf();
    assert!(String::from_utf8_lossy(&pdf).contains("612"));
}

#[test]
fn diagrams_can_be_switched_off() {
    let on = layout_tab(&guitar_score(), &TabPdfOptions::default()).unwrap();
    let off = layout_tab(
        &guitar_score(),
        &TabPdfOptions {
            diagrams: false,
            ..TabPdfOptions::default()
        },
    )
    .unwrap();
    // The legend draws more dots and strings; the chord names stay.
    assert!(on.pages[0].ops.len() > off.pages[0].ops.len() + 40);
    assert!(off.texts().contains(&"Am"));
}

#[test]
fn a_meter_change_prints_its_signature() {
    let mut s = guitar_score();
    s.meter = vec![(0, 4, 4), (2, 3, 4)];
    let l = layout_tab(&s, &TabPdfOptions::default()).unwrap();
    let texts = l.texts();
    assert!(texts.contains(&"3"));
}

#[test]
fn percussion_tracks_are_left_out_with_a_warning_and_empty_scores_are_refused() {
    let mut s = guitar_score();
    let mut drums = track("guitar6", "standard", "Drums");
    drums.is_percussion = true;
    drums.notes.push(note(0, PPQ, 0, 36));
    s.tracks.push(drums);
    let l = layout_tab(&s, &TabPdfOptions::default()).unwrap();
    assert!(l.warnings.iter().any(|w| w.contains("percussion")));
    let empty = TabScore::new();
    assert_eq!(
        layout_tab(&empty, &TabPdfOptions::default()).unwrap_err(),
        PdfError::Empty("the score has no fretted notes")
    );
}

#[test]
fn text_the_font_cannot_print_is_counted() {
    let mut s = guitar_score();
    s.title = "Chanson \u{4e2d}\u{6587}".into();
    let l = layout_tab(&s, &TabPdfOptions::default()).unwrap();
    assert!(
        l.warnings.iter().any(|w| w.contains("2 characters")),
        "{:?}",
        l.warnings
    );
}

#[test]
fn hostile_sizes_do_not_panic() {
    let mut s = guitar_score();
    s.tracks[0].notes.push(note(i64::MAX / 4, PPQ, 0, 3));
    s.tracks[0].notes.push(note(-5, PPQ, 0, 3));
    s.sanitize();
    let _ = layout_tab(
        &s,
        &TabPdfOptions {
            margin: f32::NAN,
            bars_per_line: Some(0),
            ..TabPdfOptions::default()
        },
    );
}

// ---------------------------------------------------------------- the file

#[test]
fn the_file_has_the_pages_and_the_texts_of_the_layout() {
    let l = layout_tab(
        &many_bars(40, "guitar6", "standard"),
        &TabPdfOptions::default(),
    )
    .unwrap();
    let plain = l.to_pdf_plain();
    assert!(plain.starts_with(b"%PDF-"));
    assert!(plain.ends_with(b"%%EOF") || plain.ends_with(b"%%EOF\n"));
    assert_eq!(page_count(&plain), l.pages.len());
    let from_file: Vec<String> = streams(&plain).iter().flat_map(|c| shown(c)).collect();
    let from_layout: Vec<String> = l.texts().iter().map(|t| (*t).to_owned()).collect();
    assert_eq!(from_file, from_layout);
}

#[test]
fn the_compressed_file_inflates_to_the_plain_streams() {
    let l = layout_tab(&guitar_score(), &TabPdfOptions::default()).unwrap();
    let packed = l.to_pdf();
    let plain = l.to_pdf_plain();
    assert!(packed.len() < plain.len());
    assert_eq!(streams(&packed), streams(&plain));
    assert_eq!(packed, l.to_pdf(), "deterministic");
}

#[test]
fn the_same_input_gives_the_same_bytes_and_other_input_does_not() {
    let a = write_tab_pdf(&guitar_score(), &TabPdfOptions::default())
        .unwrap()
        .0;
    let b = write_tab_pdf(&guitar_score(), &TabPdfOptions::default())
        .unwrap()
        .0;
    assert_eq!(a, b);
    let mut other = guitar_score();
    other.title = "Another".into();
    assert_ne!(
        a,
        write_tab_pdf(&other, &TabPdfOptions::default()).unwrap().0
    );
    // No date goes in.
    let text = String::from_utf8_lossy(&a);
    assert!(!text.contains("CreationDate") && !text.contains("ModDate"));
}

// ---------------------------------------------------------------- chord sheets

#[test]
fn a_sheet_prints_chords_over_lyrics_in_sections() {
    let l = layout_sheet(&sheet(), &SheetPdfOptions::default()).unwrap();
    let texts = l.texts();
    for want in [
        "Lantern Song",
        "Test Ensemble",
        "Verse 1",
        "Chorus",
        "repeat twice",
        "G",
        "Em",
    ] {
        assert!(texts.contains(&want), "missing {want:?} in {texts:?}");
    }
    assert!(texts.iter().any(|t| t.contains("Key: G")));
    assert!(texts.iter().any(|t| t.contains("Capo: 2")));
    // The lyric words are there (wrapped into pieces that start at chords).
    let all = texts.join(" ");
    assert!(all.contains("Lanterns"));
    assert!(all.contains("river"));
}

#[test]
fn a_chord_sits_over_the_word_it_is_anchored_to() {
    let l = layout_sheet(&sheet(), &SheetPdfOptions::default()).unwrap();
    let x_of = |want: &str| {
        l.pages[0].ops.iter().find_map(|o| match o {
            Op::Text { x, text, .. } if text == want => Some(*x),
            _ => None,
        })
    };
    // `D` is anchored over `glow`: it starts where that word starts.
    let d = x_of("D").expect("the chord D");
    let word = l.pages[0]
        .ops
        .iter()
        .find_map(|o| match o {
            Op::Text { x, text, .. } if text.starts_with("glow") => Some(*x),
            _ => None,
        })
        .expect("the lyric piece");
    assert!((d - word).abs() < 0.01, "{d} vs {word}");
}

#[test]
fn a_long_line_wraps_inside_the_margins() {
    let l = layout_sheet(
        &sheet(),
        &SheetPdfOptions {
            font_size: 16.0,
            ..SheetPdfOptions::default()
        },
    )
    .unwrap();
    let right = PageSize::A4.points().0;
    for p in &l.pages {
        for o in &p.ops {
            if let Op::Text { x, text, .. } = o
                && !text.contains('/')
            {
                assert!(*x < right - 48.0 + 1.0, "{text:?} at {x}");
            }
        }
    }
    assert!(l.texts().iter().any(|t| t.contains("tonight")));
}

#[test]
fn transposing_and_the_capo_rewrite_the_chords() {
    let up = layout_sheet(
        &sheet(),
        &SheetPdfOptions {
            transpose: 2,
            capo: Some(4),
            ..SheetPdfOptions::default()
        },
    )
    .unwrap();
    // Two semitones up with the capo two frets higher: the shapes stay the same.
    assert!(up.texts().contains(&"Em"));
    let no_capo = layout_sheet(
        &sheet(),
        &SheetPdfOptions {
            capo: Some(0),
            ..SheetPdfOptions::default()
        },
    )
    .unwrap();
    // Played without the capo the G shape becomes the A chord.
    assert!(no_capo.texts().contains(&"A"));
    assert!(!no_capo.texts().iter().any(|t| t.contains("Capo")));
}

#[test]
fn the_diagram_strip_lists_the_chords_used() {
    let plain = layout_sheet(&sheet(), &SheetPdfOptions::default()).unwrap();
    let with = layout_sheet(
        &sheet(),
        &SheetPdfOptions {
            diagrams: Some(SheetDiagrams {
                instrument_id: "ukulele_soprano".into(),
            }),
            ..SheetPdfOptions::default()
        },
    )
    .unwrap();
    assert!(with.pages[0].ops.len() > plain.pages[0].ops.len() + 50);
    assert!(with.texts().iter().any(|t| t.contains("Chords for")));
}

#[test]
fn an_empty_sheet_is_refused_and_tab_blocks_are_set_in_courier() {
    let mut s = ChordSheet::default();
    assert!(matches!(
        layout_sheet(&s, &SheetPdfOptions::default()),
        Err(PdfError::Empty(_))
    ));
    s.blocks
        .push(SheetBlock::Tab(atomik_tab::sheet::TabBlockText {
            lines: vec!["e|--0--3--|".into(), "B|--1--0--|".into()],
        }));
    let l = layout_sheet(&s, &SheetPdfOptions::default()).unwrap();
    assert!(l.pages[0].ops.iter().any(|o| matches!(
        o,
        Op::Text { font: atomik_tab::pdf::font::Font::Mono, text, .. } if text.starts_with("e|")
    )));
}

#[test]
fn a_sheet_file_has_the_pages_and_is_deterministic() {
    let (a, w) = write_sheet_pdf(&sheet(), &SheetPdfOptions::default()).unwrap();
    assert!(w.is_empty());
    assert_eq!(
        a,
        write_sheet_pdf(&sheet(), &SheetPdfOptions::default())
            .unwrap()
            .0
    );
    assert!(a.starts_with(b"%PDF-"));
    assert_eq!(page_count(&a), 1);
    let l = layout_sheet(&sheet(), &SheetPdfOptions::default()).unwrap();
    let from_file: Vec<String> = streams(&a).iter().flat_map(|c| shown(c)).collect();
    assert_eq!(from_file.len(), l.texts().len());
}

// ---------------------------------------------------------------- golden files

fn golden(name: &str, bytes: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        return;
    }
    let want = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with ATOMIK_UPDATE_GOLDEN=1)", path.display()));
    assert!(
        want == bytes,
        "{name} differs from its golden file (run with ATOMIK_UPDATE_GOLDEN=1 if the change is meant)"
    );
}

#[test]
fn golden_guitar_tab_with_techniques() {
    let l = layout_tab(&guitar_score(), &TabPdfOptions::default()).unwrap();
    golden("tab_guitar.pdf", &l.to_pdf_plain());
}

#[test]
fn golden_banjo_tab() {
    let l = layout_tab(&banjo_score(), &TabPdfOptions::default()).unwrap();
    golden("tab_banjo.pdf", &l.to_pdf_plain());
}

#[test]
fn golden_chord_sheet_with_a_diagram_strip() {
    let l = layout_sheet(
        &sheet(),
        &SheetPdfOptions {
            diagrams: Some(SheetDiagrams {
                instrument_id: "guitar6".into(),
            }),
            ..SheetPdfOptions::default()
        },
    )
    .unwrap();
    golden("sheet_diagrams.pdf", &l.to_pdf_plain());
}

/// Writes the printouts to `target/preview` to look at them in a viewer.
#[test]
#[ignore = "writes files for a human to look at"]
fn write_previews() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/preview");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, bytes: Vec<u8>| std::fs::write(dir.join(name), bytes).unwrap();
    write(
        "tab_guitar.pdf",
        layout_tab(&guitar_score(), &TabPdfOptions::default())
            .unwrap()
            .to_pdf(),
    );
    write(
        "tab_banjo.pdf",
        layout_tab(&banjo_score(), &TabPdfOptions::default())
            .unwrap()
            .to_pdf(),
    );
    write(
        "tab_long.pdf",
        layout_tab(
            &many_bars(40, "guitar6", "standard"),
            &TabPdfOptions::default(),
        )
        .unwrap()
        .to_pdf(),
    );
    write(
        "sheet.pdf",
        layout_sheet(
            &sheet(),
            &SheetPdfOptions {
                diagrams: Some(SheetDiagrams {
                    instrument_id: "guitar6".into(),
                }),
                ..SheetPdfOptions::default()
            },
        )
        .unwrap()
        .to_pdf(),
    );
}

/// One page as an SVG (for looking at a layout without a PDF viewer).
fn svg(l: &Layout, page: usize) -> String {
    use atomik_tab::pdf::font::Font;
    let gray = |i: Ink| match i {
        Ink::Black => "#000",
        Ink::Dark => "#404040",
        Ink::Mid => "#737373",
        Ink::Light => "#bfbfbf",
        Ink::White => "#fff",
    };
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}' viewBox='0 0 {} {}'><rect width='100%' height='100%' fill='white'/>",
        l.size.0 * 1.6,
        l.size.1 * 1.6,
        l.size.0,
        l.size.1
    );
    for o in &l.pages[page].ops {
        match o {
            Op::Line { a, b, width, ink, dash, round } => s.push_str(&format!(
                "<line x1='{}' y1='{}' x2='{}' y2='{}' stroke='{}' stroke-width='{}' {} {}/>",
                a.0, a.1, b.0, b.1, gray(*ink), width,
                if *dash > 0.0 { format!("stroke-dasharray='{dash}'") } else { String::new() },
                if *round { "stroke-linecap='round'" } else { "" }
            )),
            Op::Rect { x, y, w, h, fill, stroke } => s.push_str(&format!(
                "<rect x='{x}' y='{y}' width='{w}' height='{h}' fill='{}' stroke='{}' stroke-width='{}'/>",
                fill.map_or("none", gray), stroke.map_or("none", |(_, i)| gray(i)), stroke.map_or(0.0, |(w, _)| w)
            )),
            Op::Circle { c, r, fill, width, ink } => s.push_str(&format!(
                "<circle cx='{}' cy='{}' r='{r}' fill='{}' stroke='{}' stroke-width='{width}'/>",
                c.0, c.1, if *fill { gray(*ink) } else { "none" }, if *fill { "none" } else { gray(*ink) }
            )),
            Op::Curve { a, ctrl, b, width, ink } => s.push_str(&format!(
                "<path d='M{} {} Q{} {} {} {}' fill='none' stroke='{}' stroke-width='{width}'/>",
                a.0, a.1, ctrl.0, ctrl.1, b.0, b.1, gray(*ink)
            )),
            Op::Poly { pts, ink } => s.push_str(&format!(
                "<polygon points='{}' fill='{}'/>",
                pts.iter().map(|p| format!("{},{}", p.0, p.1)).collect::<Vec<_>>().join(" "),
                gray(*ink)
            )),
            Op::Text { x, y, size, font, ink, anchor, text } => s.push_str(&format!(
                "<text x='{x}' y='{y}' font-size='{size}' font-family='{}' font-weight='{}' fill='{}' text-anchor='{}'>{}</text>",
                if *font == Font::Mono { "Courier New" } else { "Helvetica, Arial" },
                if *font == Font::Bold { "bold" } else { "normal" },
                gray(*ink),
                match anchor { Anchor::Start => "start", Anchor::Middle => "middle", Anchor::End => "end" },
                text.replace('&', "&amp;").replace('<', "&lt;")
            )),
        }
    }
    s.push_str("</svg>");
    s
}

/// The pages as SVG files in `target/preview`.
#[test]
#[ignore = "writes files for a human to look at"]
fn write_svg_previews() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/preview");
    std::fs::create_dir_all(&dir).unwrap();
    let sheets = [
        (
            "tab_guitar",
            layout_tab(&guitar_score(), &TabPdfOptions::default()).unwrap(),
        ),
        (
            "tab_banjo",
            layout_tab(&banjo_score(), &TabPdfOptions::default()).unwrap(),
        ),
        (
            "tab_long",
            layout_tab(
                &many_bars(40, "guitar6", "standard"),
                &TabPdfOptions::default(),
            )
            .unwrap(),
        ),
        (
            "sheet",
            layout_sheet(
                &sheet(),
                &SheetPdfOptions {
                    diagrams: Some(SheetDiagrams {
                        instrument_id: "guitar6".into(),
                    }),
                    ..SheetPdfOptions::default()
                },
            )
            .unwrap(),
        ),
    ];
    for (name, l) in &sheets {
        std::fs::write(dir.join(format!("{name}.svg")), svg(l, 0)).unwrap();
    }
}
