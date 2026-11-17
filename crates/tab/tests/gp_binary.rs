//! The binary Guitar Pro readers (versions 3.00, 4.00, 5.00 and 5.10), fed by the test encoder in
//! `gp_fixtures`. The encoder writes the layout the reader expects, so these tests check the
//! reader's logic and its consistency, not agreement with files of the original program.

mod gp_fixtures;

use atomik_tab::gp::read_gp;
use atomik_tab::score::{
    BendKind, HarmonicKind, PPQ, SlideKind, StrumDirection, TabNote, TabScore, TechFlags,
    VibratoKind,
};
use gp_fixtures::binary::*;

const VERSIONS: [u16; 4] = [300, 400, 500, 510];
const MODERN: [u16; 3] = [400, 500, 510];

fn read(song: &Song) -> TabScore {
    let bytes = song.encode();
    match read_gp(&bytes) {
        Ok((s, _)) => s,
        Err(e) => panic!("version {}: {e}", song.ver),
    }
}

fn read_with_warnings(song: &Song) -> (TabScore, Vec<String>) {
    let (s, w) = read_gp(&song.encode()).unwrap();
    (s, w.iter().map(|w| w.to_string()).collect())
}

/// A one-bar song whose beats are the given ones.
fn bar_of(ver: u16, beats: Vec<BeatDef>) -> Song {
    let mut s = Song::new(ver, 1);
    s.beats[0][0] = beats;
    s
}

/// The only note of a one-note song.
fn single(ver: u16, note: NoteDef) -> TabNote {
    let s = read(&bar_of(ver, vec![BeatDef::quarter(vec![note])]));
    assert_eq!(s.tracks[0].notes.len(), 1);
    s.tracks[0].notes[0].clone()
}

#[test]
fn reads_titles_tempo_meter_and_notes() {
    for ver in VERSIONS {
        let mut song = Song::new(ver, 2);
        song.notices = vec!["a note".into(), "another".into()];
        song.tempo = 96;
        song.programs = vec![(1, 27)];
        song.beats[0][0] = vec![
            BeatDef::quarter(vec![NoteDef::new(0, 3)]),
            BeatDef::quarter(vec![NoteDef::new(1, 5)]),
            BeatDef::quarter(vec![NoteDef::new(5, 0), NoteDef::new(4, 2)]),
            BeatDef::rest(0),
        ];
        song.beats[1][0] = vec![
            BeatDef::of(-1, vec![NoteDef::new(2, 12)]),
            BeatDef::rest(-1),
        ];
        let s = read(&song);
        assert_eq!(s.title, "Test Piece", "{ver}");
        assert_eq!(s.artist, "Nobody");
        assert_eq!(s.words_by, "W");
        assert_eq!(s.music_by, if ver >= 500 { "M" } else { "W" });
        assert_eq!(s.copyright, "(c) nobody");
        assert_eq!(s.tab_by, "tabber");
        assert_eq!(s.notices, "a note\nanother");
        assert_eq!(s.tempo, vec![(0, 96.0)]);
        assert_eq!(s.meter, vec![(0, 4, 4)]);
        assert_eq!(s.bars.len(), 2);
        assert_eq!(s.bars[1].tick, 4 * PPQ);
        let t = &s.tracks[0];
        assert_eq!(t.instrument_id, "guitar6");
        assert_eq!(t.midi_program, 27);
        assert_eq!(t.tuning.opens(), vec![64, 59, 55, 50, 45, 40]);
        assert!(!t.is_percussion);
        let n: Vec<(i64, i64, u8, u8)> = t
            .notes
            .iter()
            .map(|n| (n.tick, n.len, n.string, n.fret))
            .collect();
        assert_eq!(
            n,
            vec![
                (0, PPQ, 0, 3),
                (PPQ, PPQ, 1, 5),
                (2 * PPQ, PPQ, 4, 2),
                (2 * PPQ, PPQ, 5, 0),
                (4 * PPQ, 2 * PPQ, 2, 12),
            ],
            "{ver}"
        );
        // The default dynamic is forte.
        assert!(t.notes.iter().all(|n| n.velocity == 95));
        assert!(s.problems().is_empty());
    }
}

#[test]
fn dynamics_carry_to_the_next_notes() {
    for ver in VERSIONS {
        let mut n = NoteDef::new(0, 1);
        n.dynamic = Some(3);
        let s = read(&bar_of(
            ver,
            vec![
                BeatDef::quarter(vec![NoteDef::new(0, 0)]),
                BeatDef::quarter(vec![n]),
                BeatDef::quarter(vec![NoteDef::new(0, 2)]),
            ],
        ));
        let v: Vec<u8> = s.tracks[0].notes.iter().map(|n| n.velocity).collect();
        assert_eq!(v, [95, 47, 47], "{ver}");
    }
}

#[test]
fn tunings_of_other_instruments() {
    for ver in VERSIONS {
        // A four-string bass.
        let mut s = Song::new(ver, 1);
        s.tracks = vec![TrackDef::guitar("Bass").with_strings(&[43, 38, 33, 28])];
        s.beats[0][0] = vec![BeatDef::quarter(vec![
            NoteDef::new(3, 0),
            NoteDef::new(0, 5),
        ])];
        let sc = read(&s);
        let t = &sc.tracks[0];
        assert_eq!(t.instrument_id, "bass4", "{ver}");
        assert_eq!(t.tuning.len(), 4);
        assert_eq!(t.notes.iter().map(|n| n.string).collect::<Vec<_>>(), [0, 3]);
        assert_eq!(t.key_of(&t.notes[0]), Some(48));

        // A five-string banjo (open G, the drone is the last line) keeps its short string.
        let mut s = Song::new(ver, 1);
        let mut banjo = TrackDef::guitar("Banjo").with_strings(&[62, 59, 55, 50, 67]);
        banjo.banjo = true;
        s.tracks = vec![banjo];
        s.beats[0][0] = vec![BeatDef::quarter(vec![
            NoteDef::new(4, 0),
            NoteDef::new(0, 0),
        ])];
        let sc = read(&s);
        let t = &sc.tracks[0];
        assert_eq!(t.instrument_id, "banjo5", "{ver}");
        assert_eq!(t.tuning.strings[4].start_fret, 5);
        assert_eq!(t.tuning.strings[4].open, 67);
        assert_eq!(t.notes.iter().map(|n| n.string).collect::<Vec<_>>(), [0, 4]);

        // A seven-string guitar.
        let mut s = Song::new(ver, 1);
        s.tracks = vec![TrackDef::guitar("Seven").with_strings(&[64, 59, 55, 50, 45, 40, 35])];
        s.beats[0][0] = vec![BeatDef::quarter(vec![NoteDef::new(6, 0)])];
        let sc = read(&s);
        assert_eq!(sc.tracks[0].instrument_id, "guitar7");
        assert_eq!(sc.tracks[0].notes[0].string, 6);

        // Drop D is in the catalogue; an odd tuning is custom, with the family it looks like.
        let mut s = Song::new(ver, 1);
        s.tracks = vec![TrackDef::guitar("Odd").with_strings(&[65, 60, 56, 51, 46, 38])];
        s.beats[0][0] = vec![BeatDef::quarter(vec![NoteDef::new(5, 2)])];
        let sc = read(&s);
        assert_eq!(sc.tracks[0].instrument_id, "guitar6");
        assert_eq!(sc.tracks[0].tuning.opens(), vec![65, 60, 56, 51, 46, 38]);
        let mut s = Song::new(ver, 1);
        s.tracks = vec![TrackDef::guitar("Drop D").with_strings(&[64, 59, 55, 50, 45, 38])];
        let sc = read(&s);
        assert_eq!(sc.tracks[0].instrument_id, "guitar6");

        // A twelve-string flag picks the 12-string instrument.
        let mut s = Song::new(ver, 1);
        let mut g = TrackDef::guitar("Twelve");
        g.twelve = true;
        s.tracks = vec![g];
        assert_eq!(read(&s).tracks[0].instrument_id, "guitar12");
    }
}

#[test]
fn the_capo_and_fret_count_are_read() {
    for ver in VERSIONS {
        let mut s = Song::new(ver, 1);
        s.tracks[0].capo = 3;
        s.tracks[0].frets = 22;
        s.beats[0][0] = vec![BeatDef::quarter(vec![NoteDef::new(0, 2)])];
        let sc = read(&s);
        let t = &sc.tracks[0];
        assert_eq!(t.capo.fret, 3);
        assert_eq!(t.tuning.frets, 22);
        // Frets are relative to the capo: fret 2 on the top string with the capo on 3 is G.
        assert_eq!(t.key_of(&t.notes[0]), Some(64 + 3 + 2));
    }
}

#[test]
fn dotted_and_tuplet_values_add_up() {
    for ver in VERSIONS {
        let triplet = |fret| {
            let mut b = BeatDef::of(1, vec![NoteDef::new(0, fret)]);
            b.tuplet = Some(3);
            b
        };
        let mut dotted = BeatDef::quarter(vec![NoteDef::new(0, 1)]);
        dotted.dotted = true;
        let s = read(&bar_of(
            ver,
            vec![
                dotted,
                BeatDef::of(1, vec![NoteDef::new(0, 2)]),
                triplet(3),
                triplet(4),
                triplet(5),
                BeatDef::rest(0),
            ],
        ));
        let n: Vec<(i64, i64)> = s.tracks[0].notes.iter().map(|n| (n.tick, n.len)).collect();
        let third = PPQ / 3;
        assert_eq!(
            n,
            vec![
                (0, PPQ * 3 / 2),
                (PPQ * 3 / 2, PPQ / 2),
                (2 * PPQ, third),
                (2 * PPQ + third, third),
                (2 * PPQ + 2 * third, third),
            ],
            "{ver}"
        );
    }
}

#[test]
fn the_odd_tuplets_fill_their_time() {
    for ver in VERSIONS {
        // Seven sixteenths in the time of four (a quarter), then a three-quarter rest.
        let beats: Vec<BeatDef> = (0..7)
            .map(|i| {
                let mut b = BeatDef::of(2, vec![NoteDef::new(0, i)]);
                b.tuplet = Some(7);
                b
            })
            .chain([BeatDef::rest(-1), BeatDef::rest(0)])
            .collect();
        let s = read(&bar_of(ver, beats));
        let notes = &s.tracks[0].notes;
        assert_eq!(notes.len(), 7);
        // The last one ends exactly on the beat.
        assert_eq!(notes[6].tick + notes[6].len, PPQ, "{ver}");
        assert_eq!(notes[0].len, (PPQ as f64 / 7.0).round() as i64);
    }
}

/// Bars 0..n with one quarter note each whose fret is the bar number.
fn numbered(ver: u16, n: usize) -> Song {
    let mut s = Song::new(ver, n);
    for i in 0..n {
        s.beats[i][0] = vec![
            BeatDef::quarter(vec![NoteDef::new(0, i as i8)]),
            BeatDef::rest(-1),
            BeatDef::rest(0),
        ];
    }
    s
}

fn frets(s: &TabScore) -> Vec<u8> {
    s.tracks[0].notes.iter().map(|n| n.fret).collect()
}

#[test]
fn a_repeat_is_played_the_number_of_times() {
    for ver in VERSIONS {
        let mut s = numbered(ver, 4);
        s.bars[1].repeat_open = true;
        s.bars[2].repeat_close = Some(2);
        let sc = read(&s);
        assert_eq!(frets(&sc), [0, 1, 2, 1, 2, 3], "{ver}");
        let ticks: Vec<i64> = sc.tracks[0].notes.iter().map(|n| n.tick).collect();
        assert_eq!(ticks, [0, 4, 8, 12, 16, 20].map(|b| b * PPQ));
        // The bars of the file keep their repeat marks and where they first play.
        assert_eq!(sc.bars.len(), 4);
        assert!(sc.bars[1].repeat_open);
        assert_eq!(sc.bars[2].repeat_close, Some(2));
        assert_eq!(sc.bars[3].tick, 5 * 4 * PPQ);

        let mut s = numbered(ver, 3);
        s.bars[1].repeat_open = true;
        s.bars[2].repeat_close = Some(3);
        assert_eq!(frets(&read(&s)), [0, 1, 2, 1, 2, 1, 2], "{ver}");
    }
}

#[test]
fn alternative_endings_follow_the_pass() {
    for ver in VERSIONS {
        // |: A B | 1. C :| 2. D | E
        let mut s = numbered(ver, 5);
        s.bars[0].repeat_open = true;
        s.bars[2].alt = 0b01;
        s.bars[2].repeat_close = Some(2);
        s.bars[3].alt = 0b10;
        let sc = read(&s);
        assert_eq!(frets(&sc), [0, 1, 2, 0, 1, 3, 4], "{ver}");
        assert_eq!(sc.bars[2].volta, Some(0b01));
        assert_eq!(sc.bars[3].volta, Some(0b10));
        assert_eq!(sc.bars[4].volta, None);
    }
}

#[test]
fn meter_key_and_markers_follow_the_bars() {
    for ver in VERSIONS {
        let mut s = numbered(ver, 4);
        s.key = 1;
        s.bars[1].num = Some(3);
        s.bars[1].den = Some(4);
        s.bars[1].marker = Some("Verse".into());
        s.bars[2].den = Some(8);
        s.bars[2].num = Some(6);
        s.bars[3].key = Some((-2, true));
        let sc = read(&s);
        assert_eq!(sc.meter, vec![(0, 4, 4), (1, 3, 4), (2, 6, 8)], "{ver}");
        assert_eq!(sc.markers, vec![(4 * PPQ, "Verse".to_string())]);
        let keys: Vec<(i64, i8, bool)> = sc
            .key
            .iter()
            .map(|(t, k)| (*t, k.fifths, k.minor))
            .collect();
        assert_eq!(keys[0], (0, 1, false));
        assert_eq!(keys[1], (4 * PPQ + 3 * PPQ + 3 * PPQ, -2, true));
        // The bar lengths follow the meter.
        assert_eq!(sc.bars[1].len, 3 * PPQ);
        assert_eq!(sc.bars[2].len, 3 * PPQ);
    }
}

#[test]
fn a_tempo_change_in_a_mix_table_is_kept() {
    for ver in VERSIONS {
        let mut s = numbered(ver, 2);
        s.beats[1][0][1].tempo = Some(150);
        let sc = read(&s);
        // The second beat of bar 1 starts a quarter and a half... the first beat is a quarter.
        assert_eq!(sc.tempo, vec![(0, 120.0), (4 * PPQ + PPQ, 150.0)], "{ver}");
        // A volume change alone adds nothing.
        let mut s = numbered(ver, 1);
        s.beats[0][0][0].mix_volume = Some(10);
        assert_eq!(read(&s).tempo, vec![(0, 120.0)]);
    }
}

#[test]
fn the_tempo_after_a_repeat_is_restated() {
    for ver in VERSIONS {
        // |: A | B (tempo 90) :| C: the second pass of A plays at the starting tempo.
        let mut s = numbered(ver, 3);
        s.bars[0].repeat_open = true;
        s.bars[1].repeat_close = Some(2);
        s.beats[1][0][0].tempo = Some(90);
        let sc = read(&s);
        assert_eq!(
            sc.tempo,
            vec![
                (0, 120.0),
                (4 * PPQ, 90.0),
                (8 * PPQ, 120.0),
                (12 * PPQ, 90.0)
            ],
            "{ver}"
        );
    }
}

fn flags(n: &TabNote) -> TechFlags {
    n.tech.flags
}

#[test]
fn accents_ghost_dead_and_ties() {
    for ver in VERSIONS {
        let mut n = NoteDef::new(0, 4);
        n.ghost = true;
        n.accent = true;
        n.heavy = true;
        n.own_duration = true;
        n.fingering = true;
        let t = single(ver, n);
        assert!(
            flags(&t).contains(TechFlags::GHOST | TechFlags::ACCENT | TechFlags::HEAVY_ACCENT),
            "{ver}"
        );
        let mut dead = NoteDef::new(0, 4);
        dead.kind = 3;
        assert!(flags(&single(ver, dead)).contains(TechFlags::DEAD));

        // A tied note takes the fret of the note before it.
        let mut tied = NoteDef::new(0, 0);
        tied.kind = 2;
        let s = read(&bar_of(
            ver,
            vec![
                BeatDef::quarter(vec![NoteDef::new(0, 7)]),
                BeatDef::quarter(vec![tied]),
            ],
        ));
        let n = &s.tracks[0].notes;
        assert_eq!((n[1].fret, n[1].tie), (7, true), "{ver}");
        assert!(!n[0].tie);

        // A tie with nothing before it is a plain note.
        let mut tied = NoteDef::new(0, 3);
        tied.kind = 2;
        let n = single(ver, tied);
        assert!(!n.tie);
    }
}

#[test]
fn hammer_ons_and_pull_offs_land_on_the_destination() {
    for ver in VERSIONS {
        let hammer = NoteFx {
            hammer: true,
            ..NoteFx::default()
        };
        let s = read(&bar_of(
            ver,
            vec![
                BeatDef::quarter(vec![NoteDef::new(1, 5).fx(hammer.clone())]),
                BeatDef::quarter(vec![NoteDef::new(1, 7)]),
                BeatDef::quarter(vec![NoteDef::new(1, 5).fx(hammer.clone())]),
                BeatDef::quarter(vec![NoteDef::new(1, 3)]),
            ],
        ));
        let n = &s.tracks[0].notes;
        assert!(flags(&n[0]).is_empty(), "{ver}");
        assert!(flags(&n[1]).contains(TechFlags::HAMMER_ON));
        assert!(flags(&n[2]).is_empty());
        assert!(flags(&n[3]).contains(TechFlags::PULL_OFF));
        assert!(!flags(&n[3]).contains(TechFlags::HAMMER_ON));
    }
}

#[test]
fn a_hammer_looks_for_the_next_note_on_its_string() {
    for ver in VERSIONS {
        let hammer = NoteFx {
            hammer: true,
            ..NoteFx::default()
        };
        // A note on another string sits between the two.
        let s = read(&bar_of(
            ver,
            vec![
                BeatDef::quarter(vec![NoteDef::new(1, 5).fx(hammer.clone())]),
                BeatDef::quarter(vec![NoteDef::new(3, 2)]),
                BeatDef::quarter(vec![NoteDef::new(1, 8)]),
            ],
        ));
        let n = &s.tracks[0].notes;
        assert!(flags(&n[2]).contains(TechFlags::HAMMER_ON), "{ver}");
        assert!(flags(&n[1]).is_empty());
    }
}

#[test]
fn slides_of_every_kind() {
    // Version 3 has one kind; version 4 numbers them; version 5 uses a bit mask.
    let t = single(
        300,
        NoteDef::new(0, 5).fx(NoteFx {
            slide: Some(1),
            ..NoteFx::default()
        }),
    );
    assert_eq!(t.tech.slide, Some(SlideKind::Shift));
    let v4 = [
        (1, SlideKind::Shift),
        (2, SlideKind::Legato),
        (3, SlideKind::OutDown),
        (4, SlideKind::OutUp),
        (-1, SlideKind::InFromBelow),
        (-2, SlideKind::InFromAbove),
    ];
    for (raw, kind) in v4 {
        let t = single(
            400,
            NoteDef::new(0, 5).fx(NoteFx {
                slide: Some(raw),
                ..NoteFx::default()
            }),
        );
        assert_eq!(t.tech.slide, Some(kind), "v4 {raw}");
    }
    let v5 = [
        (1, SlideKind::Shift),
        (2, SlideKind::Legato),
        (4, SlideKind::OutDown),
        (8, SlideKind::OutUp),
        (16, SlideKind::InFromBelow),
        (32, SlideKind::InFromAbove),
    ];
    for ver in [500, 510] {
        for (raw, kind) in v5 {
            let t = single(
                ver,
                NoteDef::new(0, 5).fx(NoteFx {
                    slide: Some(raw),
                    ..NoteFx::default()
                }),
            );
            assert_eq!(t.tech.slide, Some(kind), "v{ver} {raw}");
        }
    }
}

#[test]
fn bends_become_cents() {
    for ver in VERSIONS {
        let bend = NoteFx {
            bend: Some(vec![(0, 0), (30, 100), (60, 100)]),
            ..NoteFx::default()
        };
        let t = single(ver, NoteDef::new(0, 7).fx(bend));
        let b = t.tech.bend.expect("a bend");
        assert_eq!(b.kind(), BendKind::Bend, "{ver}");
        assert_eq!(b.points.len(), 3);
        assert!((b.at(0.5) - 200.0).abs() < 1e-3);
        assert_eq!(b.peak(), 200);
        // A bend and release.
        let br = NoteFx {
            bend: Some(vec![(0, 0), (20, 50), (40, 50), (60, 0)]),
            ..NoteFx::default()
        };
        let b = single(ver, NoteDef::new(0, 7).fx(br)).tech.bend.unwrap();
        assert_eq!(b.kind(), BendKind::BendRelease);
        assert_eq!(b.peak(), 100);
        // A pre-bend.
        let pre = NoteFx {
            bend: Some(vec![(0, 100), (60, 100)]),
            ..NoteFx::default()
        };
        assert_eq!(
            single(ver, NoteDef::new(0, 7).fx(pre))
                .tech
                .bend
                .unwrap()
                .kind(),
            BendKind::PreBend
        );
    }
}

#[test]
fn let_ring_and_grace_notes() {
    for ver in VERSIONS {
        let t = single(
            ver,
            NoteDef::new(0, 7).fx(NoteFx {
                let_ring: true,
                grace: Some((9, 2, if ver >= 500 { 2 } else { 0 })),
                ..NoteFx::default()
            }),
        );
        assert!(flags(&t).contains(TechFlags::LET_RING), "{ver}");
        let g = t.tech.grace.expect("a grace note");
        assert_eq!(g.fret, 9);
        assert_eq!(g.ticks, PPQ / 8);
        assert_eq!(g.on_beat, ver >= 500);
    }
}

#[test]
fn modern_note_effects() {
    for ver in MODERN {
        let t = single(
            ver,
            NoteDef::new(0, 7).fx(NoteFx {
                staccato: true,
                palm_mute: true,
                vibrato: true,
                tremolo_picking: Some(2),
                ..NoteFx::default()
            }),
        );
        assert!(
            flags(&t).contains(TechFlags::STACCATO | TechFlags::PALM_MUTE),
            "{ver}"
        );
        assert_eq!(t.tech.vibrato, Some(VibratoKind::Normal));
        assert_eq!(t.tech.tremolo_division, 16);
        for (raw, div) in [(1, 8), (3, 32)] {
            let t = single(
                ver,
                NoteDef::new(0, 7).fx(NoteFx {
                    tremolo_picking: Some(raw),
                    ..NoteFx::default()
                }),
            );
            assert_eq!(t.tech.tremolo_division, div);
        }
    }
}

#[test]
fn harmonics() {
    // Version 4 numbers the kinds.
    for (raw, kind) in [
        (1, HarmonicKind::Natural),
        (3, HarmonicKind::Tapped),
        (4, HarmonicKind::Pinch),
        (5, HarmonicKind::Semi),
        (15, HarmonicKind::Artificial),
        (17, HarmonicKind::Artificial),
        (22, HarmonicKind::Artificial),
    ] {
        let t = single(
            400,
            NoteDef::new(0, 12).fx(NoteFx {
                harmonic: Some((raw, vec![])),
                ..NoteFx::default()
            }),
        );
        assert_eq!(t.tech.harmonic.map(|h| h.kind), Some(kind), "v4 {raw}");
    }
    // Version 5 has data after some kinds.
    for ver in [500, 510] {
        for (raw, data, kind, fret) in [
            (1, vec![], HarmonicKind::Natural, None),
            (2, vec![5, 0, 2], HarmonicKind::Artificial, None),
            (3, vec![17], HarmonicKind::Tapped, Some(17.0)),
            (4, vec![], HarmonicKind::Pinch, None),
            (5, vec![], HarmonicKind::Semi, None),
        ] {
            let t = single(
                ver,
                NoteDef::new(0, 12).fx(NoteFx {
                    harmonic: Some((raw, data)),
                    vibrato: true,
                    ..NoteFx::default()
                }),
            );
            let h = t.tech.harmonic.expect("a harmonic");
            assert_eq!((h.kind, h.fret), (kind, fret), "v{ver} {raw}");
            // The bytes after the harmonic were consumed: the flag after it is still read.
            assert_eq!(t.tech.vibrato, Some(VibratoKind::Normal));
        }
    }
    // Version 3 puts the harmonic on the beat.
    let mut b = BeatDef::quarter(vec![NoteDef::new(0, 12)]);
    b.fx = Some(BeatFx {
        v3_harmonic: 1,
        ..BeatFx::default()
    });
    let s = read(&bar_of(300, vec![b]));
    assert_eq!(
        s.tracks[0].notes[0].tech.harmonic.map(|h| h.kind),
        Some(HarmonicKind::Natural)
    );
}

#[test]
fn trills_are_reported_as_ignored() {
    for ver in MODERN {
        let song = bar_of(
            ver,
            vec![BeatDef::quarter(vec![NoteDef::new(0, 7).fx(NoteFx {
                trill: Some((9, 2)),
                vibrato: true,
                ..NoteFx::default()
            })])],
        );
        let (s, w) = read_with_warnings(&song);
        assert_eq!(s.tracks[0].notes.len(), 1);
        assert!(w.iter().any(|w| w.contains("trill")), "{ver}: {w:?}");
        assert_eq!(s.tracks[0].notes[0].tech.vibrato, Some(VibratoKind::Normal));
    }
}

#[test]
fn beat_effects_apply_to_every_note_of_the_beat() {
    for ver in VERSIONS {
        let two = |fx: BeatFx| {
            let mut b = BeatDef::quarter(vec![NoteDef::new(0, 3), NoteDef::new(1, 3)]);
            b.fx = Some(fx);
            let s = read(&bar_of(ver, vec![b]));
            s.tracks[0].notes.clone()
        };
        for n in two(BeatFx {
            stroke_up: Some(true),
            slap: 3,
            fade_in: true,
            vibrato: true,
            ..BeatFx::default()
        }) {
            assert_eq!(n.tech.strum, Some(StrumDirection::Up), "{ver}");
            assert!(flags(&n).contains(TechFlags::POP | TechFlags::FADE_IN));
            assert_eq!(n.tech.vibrato, Some(VibratoKind::Normal));
        }
        for n in two(BeatFx {
            stroke_up: Some(false),
            slap: 2,
            wide_vibrato: true,
            ..BeatFx::default()
        }) {
            assert_eq!(n.tech.strum, Some(StrumDirection::Down));
            assert!(flags(&n).contains(TechFlags::SLAP));
            assert_eq!(n.tech.vibrato, Some(VibratoKind::Wide));
        }
        for n in two(BeatFx {
            slap: 1,
            ..BeatFx::default()
        }) {
            assert!(flags(&n).contains(TechFlags::TAP));
        }
    }
}

#[test]
fn the_tremolo_bar_becomes_a_whammy_curve() {
    let mut b = BeatDef::quarter(vec![NoteDef::new(0, 3)]);
    b.fx = Some(BeatFx {
        tremolo_v3: Some(100),
        ..BeatFx::default()
    });
    let w = read(&bar_of(300, vec![b])).tracks[0].notes[0]
        .tech
        .whammy
        .clone()
        .expect("a dive");
    assert_eq!(w.peak(), 0);
    assert!((w.at(0.5) + 200.0).abs() < 1e-3);
    for ver in [400, 500, 510] {
        let mut b = BeatDef::quarter(vec![NoteDef::new(0, 3)]);
        b.fx = Some(BeatFx {
            tremolo_points: Some(vec![(0, 0), (30, -100), (60, 0)]),
            ..BeatFx::default()
        });
        let w = read(&bar_of(ver, vec![b])).tracks[0].notes[0]
            .tech
            .whammy
            .clone()
            .expect("a dive");
        assert!((w.at(0.5) + 200.0).abs() < 1e-3, "{ver}");
        assert!(w.at(1.0).abs() < 1e-3);
    }
}

#[test]
fn the_picking_stroke_gives_a_direction_when_there_is_no_stroke() {
    for ver in MODERN {
        let mut b = BeatDef::quarter(vec![NoteDef::new(0, 3)]);
        b.fx = Some(BeatFx {
            pick: Some(2),
            ..BeatFx::default()
        });
        assert_eq!(
            read(&bar_of(ver, vec![b])).tracks[0].notes[0].tech.strum,
            Some(StrumDirection::Down)
        );
    }
}

#[test]
fn chord_diagrams_in_both_layouts() {
    for ver in VERSIONS {
        for new_format in [false, true] {
            let mut b = BeatDef::quarter(vec![NoteDef::new(1, 1)]);
            b.chord = Some(ChordDef {
                name: "Am".into(),
                frets: vec![0, 1, 2, 2, 0, -1],
                new_format,
            });
            b.text = Some("a made up remark".into());
            let (s, w) = read_with_warnings(&bar_of(ver, vec![b]));
            let t = &s.tracks[0];
            assert_eq!(t.diagrams.len(), 1, "{ver} {new_format}");
            let d = &t.diagrams[0];
            assert_eq!(d.name, "Am");
            assert_eq!(
                d.voicing.frets,
                vec![Some(0), Some(1), Some(2), Some(2), Some(0), None]
            );
            assert_eq!(t.chords.len(), 1);
            assert_eq!(t.chords[0].1.name(), "Am");
            assert!(w.iter().any(|w| w.contains("text")));
            assert_eq!(t.notes.len(), 1);
        }
    }
}

#[test]
fn lyrics_are_laid_on_the_notes() {
    for ver in MODERN {
        let mut s = Song::new(ver, 2);
        s.lyrics = Some((1, vec![(1, "Hel-lo big".into()), (2, "world".into())]));
        s.beats[0][0] = vec![
            BeatDef::quarter(vec![NoteDef::new(0, 0)]),
            BeatDef::rest(0),
            BeatDef::quarter(vec![NoteDef::new(0, 1)]),
            BeatDef::quarter(vec![NoteDef::new(0, 2)]),
        ];
        s.beats[1][0] = vec![BeatDef::quarter(vec![NoteDef::new(0, 3)])];
        let sc = read(&s);
        let l: Vec<(i64, &str)> = sc.tracks[0]
            .lyrics
            .iter()
            .map(|(t, s)| (*t, s.as_str()))
            .collect();
        assert_eq!(
            l,
            vec![
                (0, "Hel"),
                (2 * PPQ, "lo"),
                (3 * PPQ, "big"),
                (4 * PPQ, "world")
            ],
            "{ver}"
        );
    }
}

#[test]
fn drum_tracks_keep_the_key_in_the_fret() {
    for ver in VERSIONS {
        let mut s = Song::new(ver, 1);
        let mut drums = TrackDef::guitar("Drums");
        drums.drums = true;
        drums.channel = 10;
        s.tracks = vec![drums];
        s.beats[0][0] = vec![
            BeatDef::quarter(vec![NoteDef::new(0, 36), NoteDef::new(2, 42)]),
            BeatDef::quarter(vec![NoteDef::new(1, 38)]),
        ];
        let sc = read(&s);
        let t = &sc.tracks[0];
        assert!(t.is_percussion, "{ver}");
        let keys: Vec<u8> = t.notes.iter().map(|n| n.fret).collect();
        assert_eq!(keys, [36, 42, 38]);
        assert!(sc.problems().is_empty());
    }
}

#[test]
fn a_second_voice_shares_the_bar() {
    for ver in [500, 510] {
        let mut s = Song::new(ver, 1);
        s.beats[0][0] = vec![
            BeatDef::of(-1, vec![NoteDef::new(0, 5)]),
            BeatDef::of(-1, vec![NoteDef::new(0, 7)]),
        ];
        s.voice2[0][0] = vec![BeatDef::of(-2, vec![NoteDef::new(5, 0)])];
        let sc = read(&s);
        let n: Vec<(i64, u8, u8)> = sc.tracks[0]
            .notes
            .iter()
            .map(|n| (n.tick, n.string, n.fret))
            .collect();
        assert_eq!(n, vec![(0, 0, 5), (0, 5, 0), (2 * PPQ, 0, 7)], "{ver}");
    }
}

#[test]
fn several_tracks_read_in_order() {
    for ver in VERSIONS {
        let mut s = Song::new(ver, 2);
        let mut bass = TrackDef::guitar("Bass").with_strings(&[43, 38, 33, 28]);
        bass.channel = 2;
        s.add_track(bass);
        s.programs = vec![(1, 25), (2, 33)];
        s.beats[0][0] = vec![BeatDef::quarter(vec![NoteDef::new(0, 1)])];
        s.beats[0][1] = vec![BeatDef::of(-2, vec![NoteDef::new(3, 0)])];
        s.beats[1][1] = vec![BeatDef::of(-2, vec![NoteDef::new(2, 3)])];
        let sc = read(&s);
        assert_eq!(sc.tracks.len(), 2);
        assert_eq!(sc.tracks[0].name, "Guitar");
        assert_eq!(sc.tracks[1].name, "Bass");
        assert_eq!(sc.tracks[1].midi_program, 33);
        assert_eq!(sc.tracks[0].notes.len(), 1);
        let bass: Vec<i64> = sc.tracks[1].notes.iter().map(|n| n.tick).collect();
        assert_eq!(bass, [0, 4 * PPQ]);
    }
}

#[test]
fn notes_on_missing_strings_are_dropped_with_a_warning() {
    for ver in VERSIONS {
        let mut s = Song::new(ver, 1);
        s.tracks = vec![TrackDef::guitar("Uke").with_strings(&[69, 64, 60, 67])];
        s.beats[0][0] = vec![BeatDef::quarter(vec![
            NoteDef::new(1, 2),
            NoteDef::new(5, 2),
        ])];
        let (sc, w) = read_with_warnings(&s);
        assert_eq!(sc.tracks[0].notes.len(), 1, "{ver}");
        assert!(w.iter().any(|w| w.contains("string")));
        assert!(
            sc.tracks[0].instrument_id.starts_with("ukulele"),
            "{:?}",
            sc.tracks[0].instrument_id
        );
    }
}

#[test]
fn unsupported_and_foreign_files_are_refused_with_messages() {
    let mut w = gp_fixtures::W::default();
    w.byte_size_string("FICHIER GUITAR PRO v2.20", 30);
    w.zeros(100);
    let e = read_gp(&w.0).unwrap_err().to_string();
    assert!(e.contains("2.20"), "{e}");
    assert!(read_gp(b"hello world, not a score").is_err());
    assert!(read_gp(&[]).is_err());
}

#[test]
fn a_whole_song_reads_back_in_one_piece() {
    // Many bars, two tracks, repeats, a tempo change and effects, for every version.
    for ver in VERSIONS {
        let mut s = Song::new(ver, 8);
        s.add_track(TrackDef::guitar("Rhythm"));
        s.bars[2].repeat_open = true;
        s.bars[4].repeat_close = Some(2);
        for b in 0..8usize {
            let fx = NoteFx {
                bend: (b % 3 == 0).then(|| vec![(0, 0), (60, 100)]),
                let_ring: b % 2 == 0,
                ..NoteFx::default()
            };
            s.beats[b][0] = (0..4)
                .map(|i| BeatDef::quarter(vec![NoteDef::new(i % 6, (b + i) as i8).fx(fx.clone())]))
                .collect();
            s.beats[b][1] = vec![BeatDef::of(-2, vec![NoteDef::new(b % 6, b as i8)])];
        }
        let sc = read(&s);
        assert!(sc.problems().is_empty(), "{ver}: {:?}", sc.problems());
        // Bars 2..=4 are played twice.
        assert_eq!(sc.tracks[0].notes.len(), 4 * (8 + 3), "{ver}");
        assert_eq!(sc.tracks[1].notes.len(), 8 + 3);
    }
}
