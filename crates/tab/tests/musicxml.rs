//! MusicXML: what is written reads back as the same score, and hand-written documents read the way
//! the format says (string 1 is the highest, pitch-only parts are fretted, repeats are expanded).

use atomik_tab::ChordSymbol;
use atomik_tab::instrument::{Capo, find_tuning};
use atomik_tab::musicxml::{
    MusicXmlError, MusicXmlFormat, read_musicxml, sniff, write_musicxml, write_mxl,
};
use atomik_tab::score::{
    Bend, DiagramRef, Grace, Harmonic, HarmonicKind, KeySignature, PPQ, SlideKind, StrumDirection,
    TabNote, TabScore, TabTrack, TechFlags, Techniques, VibratoKind,
};
use atomik_tab::voicing::{Voicing, assign_fingers};

const BAR: i64 = 4 * PPQ;

fn tuning(inst: &str, id: &str) -> atomik_tab::Tuning {
    find_tuning(inst, id).unwrap().tuning.clone()
}

/// A fret that is valid on `string` (a short string starts higher up the neck).
fn fret_on(t: &atomik_tab::Tuning, string: usize, plus: u8) -> u8 {
    let start = t.strings[string].start_fret;
    if start > 0 { start + plus } else { 2 + plus }
}

fn note(
    t: &atomik_tab::Tuning,
    tick: i64,
    len: i64,
    string: usize,
    plus: u8,
    tech: Techniques,
) -> TabNote {
    let mut n = TabNote::new(tick, len, string as u8, fret_on(t, string, plus));
    n.tech = tech;
    n
}

fn tech(f: impl FnOnce(&mut Techniques)) -> Techniques {
    let mut t = Techniques::default();
    f(&mut t);
    t
}

fn diagram(t: &atomik_tab::Tuning, tick: i64, name: &str) -> DiagramRef {
    let n = t.len();
    let frets: Vec<Option<u8>> = (0..n)
        .map(|i| {
            if i + 1 == n {
                None
            } else {
                Some((i % 3) as u8)
            }
        })
        .collect();
    let (fingers, barres) = assign_fingers(&frets).unwrap_or_else(|| (vec![None; n], Vec::new()));
    DiagramRef {
        tick,
        name: name.to_owned(),
        voicing: Voicing {
            frets,
            fingers,
            barres,
            base_fret: 1,
        },
    }
}

/// A score that uses every technique, a tempo, meter and key change, markers, lyrics, chord
/// symbols and diagrams, on the given instrument.
fn fixture(inst: &str, tuning_id: &str, capo: u8) -> TabScore {
    let t = tuning(inst, tuning_id);
    let mut s = TabScore::new();
    s.title = "Made Up Etude".into();
    s.subtitle = "for testing".into();
    s.artist = "Nobody".into();
    s.album = "No Album".into();
    s.words_by = "Lyricist".into();
    s.music_by = "Composer".into();
    s.tab_by = "Transcriber".into();
    s.copyright = "(c) nobody".into();
    s.notices = "Made up & <not> real".into();
    s.tempo = vec![(0, 96.0), (2 * BAR, 120.5)];
    s.meter = vec![(0, 4, 4), (4, 6, 8)];
    s.key = vec![
        (
            0,
            KeySignature {
                fifths: 2,
                minor: false,
            },
        ),
        (
            2 * BAR,
            KeySignature {
                fifths: -1,
                minor: true,
            },
        ),
    ];
    s.markers = vec![
        (0, "Intro".into()),
        (BAR + PPQ + PPQ / 2, "Mid".into()),
        (BAR, "Verse".into()),
    ];
    s.markers.sort_by_key(|m| m.0);

    let mut tr = TabTrack::new("Test Part", inst, t.clone());
    tr.capo = Capo::at(capo);
    tr.midi_program = 26;
    let n = t.len();
    let mut notes: Vec<TabNote> = Vec::new();

    // Bar 0: palm mute + let ring, a bend with vibrato, hammer-on, pull-off, a legato slide.
    let mut a = note(
        &t,
        0,
        PPQ,
        0,
        0,
        tech(|x| x.flags = TechFlags::PALM_MUTE | TechFlags::LET_RING),
    );
    a.velocity = 100;
    notes.push(a);
    notes.push(note(
        &t,
        PPQ,
        PPQ,
        1,
        0,
        tech(|x| {
            x.bend = Some(Bend::up(200));
            x.vibrato = Some(VibratoKind::Normal);
        }),
    ));
    notes.push(note(
        &t,
        2 * PPQ,
        PPQ / 2,
        1,
        2,
        tech(|x| x.flags = TechFlags::HAMMER_ON),
    ));
    notes.push(note(
        &t,
        2 * PPQ + PPQ / 2,
        PPQ / 2,
        1,
        0,
        tech(|x| x.flags = TechFlags::PULL_OFF),
    ));
    notes.push(note(
        &t,
        3 * PPQ,
        PPQ,
        2,
        0,
        tech(|x| x.slide = Some(SlideKind::Legato)),
    ));

    // Bar 1: a harmonic, a bend and release, a strummed chord, a triplet, a tied note.
    notes.push(note(
        &t,
        BAR,
        PPQ,
        2,
        2,
        tech(|x| {
            x.harmonic = Some(Harmonic {
                kind: HarmonicKind::Natural,
                fret: None,
            })
        }),
    ));
    notes.push(note(
        &t,
        BAR + PPQ,
        PPQ,
        0,
        1,
        tech(|x| x.bend = Some(Bend::bend_release(100))),
    ));
    for s_i in 0..3 {
        notes.push(note(
            &t,
            BAR + 2 * PPQ,
            PPQ,
            s_i,
            1,
            tech(|x| {
                x.strum = Some(StrumDirection::Down);
                x.flags = TechFlags::ACCENT;
            }),
        ));
    }
    for k in 0..3 {
        notes.push(note(
            &t,
            BAR + 3 * PPQ + k * (PPQ / 3),
            PPQ / 3,
            1,
            k as u8,
            Techniques::default(),
        ));
    }

    // Bars 2 and 3: one technique per eighth note.
    let list: Vec<Techniques> = vec![
        tech(|x| x.flags = TechFlags::DEAD),
        tech(|x| x.flags = TechFlags::GHOST),
        tech(|x| x.flags = TechFlags::STACCATO | TechFlags::ACCENT),
        tech(|x| x.flags = TechFlags::HEAVY_ACCENT | TechFlags::TAP),
        tech(|x| x.flags = TechFlags::SLAP),
        tech(|x| x.flags = TechFlags::POP),
        tech(|x| x.flags = TechFlags::FADE_IN | TechFlags::THUMB),
        tech(|x| x.vibrato = Some(VibratoKind::Wide)),
        tech(|x| x.bend = Some(Bend::pre_bend(100))),
        tech(|x| x.bend = Some(Bend::pre_bend_release(200))),
        tech(|x| x.whammy = Some(Bend::bend_release(-200))),
        tech(|x| {
            x.harmonic = Some(Harmonic {
                kind: HarmonicKind::Tapped,
                fret: Some(12.0),
            })
        }),
        tech(|x| {
            x.harmonic = Some(Harmonic {
                kind: HarmonicKind::Pinch,
                fret: None,
            })
        }),
        tech(|x| {
            x.harmonic = Some(Harmonic {
                kind: HarmonicKind::Semi,
                fret: None,
            })
        }),
        tech(|x| {
            x.harmonic = Some(Harmonic {
                kind: HarmonicKind::Artificial,
                fret: Some(7.0),
            })
        }),
        tech(|x| {
            x.tremolo_division = 16;
            x.grace = Some(Grace {
                fret: 3,
                ticks: PPQ / 8,
                on_beat: false,
            });
        }),
    ];
    for (i, tc) in list.into_iter().enumerate() {
        let tick = 2 * BAR + (i as i64) * (PPQ / 2);
        notes.push(note(&t, tick, PPQ / 2, i % n, (i % 3) as u8, tc));
    }

    // Bar 4 is 6/8: the slides that belong to one note, a shift slide and a tied pair.
    let base = 4 * BAR;
    let kinds = [
        SlideKind::InFromBelow,
        SlideKind::InFromAbove,
        SlideKind::OutUp,
        SlideKind::OutDown,
        SlideKind::Shift,
    ];
    for (i, k) in kinds.iter().enumerate() {
        notes.push(note(
            &t,
            base + (i as i64) * (PPQ / 2),
            PPQ / 2,
            i % n,
            0,
            tech(|x| x.slide = Some(*k)),
        ));
    }
    notes.push(note(
        &t,
        base + 5 * (PPQ / 2),
        PPQ / 2,
        3 % n,
        0,
        Techniques::default(),
    ));
    // Bar 5 (6/8): a note and its tied continuation.
    let base5 = base + 3 * PPQ;
    notes.push(note(&t, base5, PPQ, 3 % n, 1, Techniques::default()));
    let mut tied = note(&t, base5 + PPQ, PPQ, 3 % n, 1, Techniques::default());
    tied.tie = true;
    notes.push(tied);
    notes.push(note(&t, base5 + 2 * PPQ, PPQ, 0, 0, Techniques::default()));
    tr.notes = notes;

    // Lyrics on note starts.
    tr.lyrics = vec![
        (0, "hel-".into()),
        (PPQ, "lo".into()),
        (BAR, "made-".into()),
        (BAR + PPQ, "up-".into()),
        (BAR + 2 * PPQ, "words".into()),
    ];
    // Chords and diagrams.
    let am = ChordSymbol::parse("Am").unwrap();
    let gb = ChordSymbol::parse("G/B").unwrap();
    let c7 = ChordSymbol::parse("C7#9").unwrap();
    tr.chords = vec![(0, am.clone()), (2 * PPQ, gb), (BAR + PPQ, c7)];
    tr.diagrams = vec![
        diagram(&t, 0, "Am"),
        DiagramRef {
            tick: BAR + 3 * PPQ,
            name: "Odd Shape".into(),
            voicing: diagram(&t, 0, "x").voicing,
        },
    ];
    s.tracks.push(tr);
    s
}

fn roundtrip(s: &TabScore) -> TabScore {
    let (xml, w) = write_musicxml(s).expect("writes");
    assert!(w.is_empty(), "writer warnings: {w:?}");
    let (mut got, rw) = read_musicxml(xml.as_bytes()).expect("reads");
    assert!(rw.is_empty(), "reader warnings: {rw:?}");
    got.bars.clear();
    got
}

fn assert_same(got: &TabScore, want: &TabScore) {
    assert_eq!(got.title, want.title);
    assert_eq!(got.subtitle, want.subtitle);
    assert_eq!(got.artist, want.artist);
    assert_eq!(got.album, want.album);
    assert_eq!(got.words_by, want.words_by);
    assert_eq!(got.music_by, want.music_by);
    assert_eq!(got.tab_by, want.tab_by);
    assert_eq!(got.copyright, want.copyright);
    assert_eq!(got.notices, want.notices);
    assert_eq!(got.tempo, want.tempo);
    assert_eq!(got.meter, want.meter);
    assert_eq!(got.key, want.key);
    assert_eq!(got.markers, want.markers);
    assert_eq!(got.tracks.len(), want.tracks.len());
    for (g, w) in got.tracks.iter().zip(&want.tracks) {
        assert_eq!(g.name, w.name);
        assert_eq!(g.instrument_id, w.instrument_id);
        assert_eq!(g.tuning, w.tuning);
        assert_eq!(g.capo, w.capo);
        assert_eq!(g.midi_program, w.midi_program);
        assert_eq!(g.is_percussion, w.is_percussion);
        assert_eq!(g.notes.len(), w.notes.len(), "note count");
        for (i, (a, b)) in g.notes.iter().zip(&w.notes).enumerate() {
            assert_eq!(a, b, "note {i}");
        }
        assert_eq!(g.lyrics, w.lyrics);
        assert_eq!(g.chords, w.chords);
        assert_eq!(g.diagrams, w.diagrams);
    }
}

fn sorted(mut s: TabScore) -> TabScore {
    let w = s.sanitize();
    assert!(w.is_empty(), "{w:?}");
    s
}

#[test]
fn guitar_round_trips_every_technique() {
    let s = sorted(fixture("guitar6", "standard", 0));
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn guitar_with_a_capo_keeps_it_and_the_relative_frets() {
    let s = sorted(fixture("guitar6", "drop_d", 3));
    let got = roundtrip(&s);
    assert_eq!(got.tracks[0].capo, Capo::at(3));
    assert_same(&got, &s);
}

#[test]
fn bass_round_trips() {
    let s = sorted(fixture("bass4", "standard", 0));
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn banjo_with_its_drone_round_trips() {
    let s = sorted(fixture("banjo5", "open_g", 0));
    let got = roundtrip(&s);
    let t = &got.tracks[0].tuning;
    assert_eq!(t.strings[4].start_fret, 5, "the drone keeps its start fret");
    assert_same(&got, &s);
}

#[test]
fn re_entrant_ukulele_round_trips() {
    let s = sorted(fixture("ukulele_soprano", "reentrant", 0));
    let got = roundtrip(&s);
    assert!(got.tracks[0].tuning.is_re_entrant());
    assert_same(&got, &s);
}

#[test]
fn seven_string_guitar_round_trips() {
    let s = sorted(fixture("guitar7", "standard", 0));
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn a_custom_tuning_with_labels_round_trips() {
    let mut s = fixture("guitar6", "standard", 0);
    let t = &mut s.tracks[0];
    t.instrument_id = String::new();
    t.tuning = atomik_tab::Tuning::from_opens(&[62, 57, 53, 48, 43, 38, 31], 21);
    t.tuning.strings[2].label = "F-ish".into();
    t.notes.retain(|n| n.string < 7);
    // The made-up diagrams are for six strings.
    t.diagrams.clear();
    let s = sorted(s);
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn without_the_private_field_the_catalogue_tuning_is_found_by_its_notes() {
    let s = sorted(fixture("banjo5", "open_g", 0));
    let (xml, _) = write_musicxml(&s).unwrap();
    let stripped: String = xml
        .lines()
        .filter(|l| !l.contains("atomik-tuning-"))
        .collect::<Vec<_>>()
        .join("\n");
    let (got, _) = read_musicxml(stripped.as_bytes()).unwrap();
    assert_eq!(got.tracks[0].tuning.strings[4].start_fret, 5);
    assert_eq!(got.tracks[0].tuning.opens(), s.tracks[0].tuning.opens());
    assert!(got.tracks[0].instrument_id.starts_with("banjo"));
}

#[test]
fn compressed_files_read_like_plain_ones() {
    let s = sorted(fixture("guitar6", "standard", 0));
    let (z, w) = write_mxl(&s).unwrap();
    assert!(w.is_empty());
    assert_eq!(sniff(&z), Some(MusicXmlFormat::Mxl));
    let (mut got, _) = read_musicxml(&z).unwrap();
    got.bars.clear();
    assert_same(&got, &s);
}

#[test]
fn percussion_round_trips() {
    let mut s = TabScore::new();
    let mut t = TabTrack::new("Kit", "", atomik_tab::Tuning::from_opens(&[0, 0, 0], 36));
    t.is_percussion = true;
    t.midi_program = 0;
    // The lane of a hit is the position of its key in the kit's list: 36, 38, 42.
    for (i, (lane, key)) in [(0u8, 36u8), (1, 38), (2, 42), (1, 38), (2, 42)]
        .iter()
        .enumerate()
    {
        t.notes
            .push(TabNote::new((i as i64) * (PPQ / 2), PPQ / 2, *lane, *key));
    }
    s.tracks.push(t);
    s.meter = vec![(0, 4, 4)];
    s.tempo = vec![(0, 100.0)];
    let s = sorted(s);
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn several_parts_stay_separate() {
    let mut s = fixture("guitar6", "standard", 0);
    let bass = fixture("bass4", "standard", 0).tracks.remove(0);
    let mut bass = bass;
    bass.name = "Bass".into();
    bass.chords.clear();
    bass.diagrams.clear();
    bass.lyrics.clear();
    s.tracks.push(bass);
    let s = sorted(s);
    let got = roundtrip(&s);
    assert_eq!(got.tracks.len(), 2);
    assert_same(&got, &s);
}

#[test]
fn a_note_across_a_bar_line_becomes_two_tied_notes() {
    let t = tuning("guitar6", "standard");
    let mut s = TabScore::new();
    let mut tr = TabTrack::new("g", "guitar6", t);
    tr.notes.push(TabNote::new(BAR - PPQ, 3 * PPQ, 2, 2));
    s.tracks.push(tr);
    s.meter = vec![(0, 4, 4)];
    let got = roundtrip(&sorted(s));
    let n = &got.tracks[0].notes;
    assert_eq!(n.len(), 2);
    assert_eq!((n[0].tick, n[0].len, n[0].tie), (BAR - PPQ, PPQ, false));
    assert_eq!((n[1].tick, n[1].len, n[1].tie), (BAR, 2 * PPQ, true));
    assert_eq!((n[1].string, n[1].fret), (2, 2));
}

#[test]
fn tuplets_and_odd_lengths_keep_their_ticks() {
    let t = tuning("guitar6", "standard");
    let mut s = TabScore::new();
    let mut tr = TabTrack::new("g", "guitar6", t);
    let mut tick = 0;
    // A quintuplet of sixteenths, a run of sixteenth triplets, a note of five sixteenths.
    for _ in 0..5 {
        tr.notes.push(TabNote::new(tick, PPQ / 5, 1, 1));
        tick += PPQ / 5;
    }
    for _ in 0..6 {
        tr.notes.push(TabNote::new(tick, PPQ / 6, 2, 2));
        tick += PPQ / 6;
    }
    let before = tick;
    tr.notes.push(TabNote::new(before, 5 * PPQ / 4, 3, 3));
    s.tracks.push(tr);
    s.meter = vec![(0, 4, 4)];
    let want = sorted(s.clone());
    let (xml, _) = write_musicxml(&want).unwrap();
    assert!(xml.contains("<time-modification><actual-notes>5</actual-notes>"));
    assert!(xml.contains("<actual-notes>3</actual-notes>"));
    let (got, _) = read_musicxml(xml.as_bytes()).unwrap();
    // The five-sixteenth note is a quarter tied to a sixteenth: join the tie before comparing.
    let mut ticks: Vec<(i64, i64)> = Vec::new();
    for n in &got.tracks[0].notes {
        match ticks.last_mut() {
            Some(last) if n.tie => last.1 += n.len,
            _ => ticks.push((n.tick, n.len)),
        }
    }
    let want_ticks: Vec<(i64, i64)> = want.tracks[0]
        .notes
        .iter()
        .map(|n| (n.tick, n.len))
        .collect();
    assert_eq!(ticks, want_ticks);
}

#[test]
fn overlapping_notes_use_a_second_voice() {
    let t = tuning("guitar6", "standard");
    let mut s = TabScore::new();
    let mut tr = TabTrack::new("g", "guitar6", t);
    tr.notes.push(TabNote::new(0, BAR, 5, 0));
    for i in 0..4 {
        tr.notes.push(TabNote::new(i * PPQ, PPQ, 1, i as u8));
    }
    s.tracks.push(tr);
    s.meter = vec![(0, 4, 4)];
    let s = sorted(s);
    let (xml, _) = write_musicxml(&s).unwrap();
    assert!(xml.contains("<voice>2</voice>"));
    assert!(xml.contains("<backup>"));
    assert_same(&roundtrip(&s), &s);
}

#[test]
fn the_time_grid_is_exact_or_it_says_so() {
    let t = tuning("guitar6", "standard");
    let mut s = TabScore::new();
    let mut tr = TabTrack::new("g", "guitar6", t);
    tr.notes.push(TabNote::new(0, PPQ, 1, 1));
    s.tracks.push(tr);
    let (xml, w) = write_musicxml(&s).unwrap();
    assert!(w.is_empty());
    assert!(
        xml.contains("<divisions>1</divisions>"),
        "a quarter-note score needs divisions 1"
    );

    // A note that starts one tick into the bar: no small grid is exact.
    let mut s2 = s.clone();
    s2.tracks[0].notes.push(TabNote::new(PPQ + 1, 7, 2, 2));
    let (_, w) = write_musicxml(&s2).unwrap();
    assert!(w.iter().any(|x| x.message.contains("rounded")), "{w:?}");
}

#[test]
fn string_one_is_the_highest_string() {
    let xml = r#"<?xml version="1.0"?>
<score-partwise version="3.1">
<part-list><score-part id="P1"><part-name>Gtr</part-name></score-part></part-list>
<part id="P1"><measure number="1">
<attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>
<note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><type>quarter</type>
<notations><technical><string>1</string><fret>0</fret></technical></notations></note>
<note><pitch><step>E</step><octave>2</octave></pitch><duration>1</duration><type>quarter</type>
<notations><technical><string>6</string><fret>0</fret></technical></notations></note>
<note><pitch><step>G</step><octave>3</octave></pitch><duration>2</duration><type>half</type>
<notations><technical><string>3</string><fret>0</fret></technical></notations></note>
</measure></part></score-partwise>"#;
    let (s, w) = read_musicxml(xml.as_bytes()).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let t = &s.tracks[0];
    assert_eq!(
        t.tuning.opens(),
        [64, 59, 55, 50, 45, 40],
        "no tuning given: a guitar"
    );
    let strings: Vec<u8> = t.notes.iter().map(|n| n.string).collect();
    assert_eq!(strings, [0, 5, 2]);
    assert!(t.notes.iter().all(|n| n.fret == 0));
    assert_eq!(t.notes[2].tick, 2 * PPQ);
    assert_eq!(t.notes[2].len, 2 * PPQ);
}

#[test]
fn staff_tuning_and_a_capo_decide_the_neck() {
    // Drop D with a capo on 2; line 1 is the bottom line. Fret 3 on string 2 (B string, capo
    // raises it to C#) sounds E, written pitch says so.
    let xml = r#"<score-partwise version="3.1">
<part-list><score-part id="P1"><part-name>Gtr</part-name></score-part></part-list>
<part id="P1"><measure number="1">
<attributes><divisions>2</divisions><time><beats>4</beats><beat-type>4</beat-type></time>
<staff-details><staff-lines>6</staff-lines>
<staff-tuning line="1"><tuning-step>D</tuning-step><tuning-octave>2</tuning-octave></staff-tuning>
<staff-tuning line="2"><tuning-step>A</tuning-step><tuning-octave>2</tuning-octave></staff-tuning>
<staff-tuning line="3"><tuning-step>D</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
<staff-tuning line="4"><tuning-step>G</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
<staff-tuning line="5"><tuning-step>B</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
<staff-tuning line="6"><tuning-step>E</tuning-step><tuning-octave>4</tuning-octave></staff-tuning>
<capo>2</capo></staff-details></attributes>
<note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration><type>quarter</type>
<notations><technical><string>2</string><fret>3</fret></technical></notations></note>
<note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration><type>quarter</type>
<notations><technical><string>2</string><fret>5</fret></technical></notations></note>
</measure></part></score-partwise>"#;
    let (s, w) = read_musicxml(xml.as_bytes()).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let t = &s.tracks[0];
    assert_eq!(t.instrument_id, "guitar6");
    assert_eq!(t.tuning.opens(), [64, 59, 55, 50, 45, 38]);
    assert_eq!(t.capo, Capo::at(2));
    // The first fret is relative to the capo (3 + 2 + B3 = E4); the second is counted from the
    // nut (5 on the B string = E4), so it is 3 above the capo too.
    assert_eq!(t.notes[0].fret, 3);
    assert_eq!(t.notes[1].fret, 3);
    assert!(t.notes.iter().all(|n| t.key_of(n) == Some(64)));
}

#[test]
fn a_part_with_only_pitches_is_fretted() {
    let mut body = String::new();
    for (step, oct) in [("E", 3), ("A", 3), ("D", 4), ("G", 4)] {
        body.push_str(&format!(
            "<note><pitch><step>{step}</step><octave>{oct}</octave></pitch><duration>1</duration><type>quarter</type></note>"
        ));
    }
    let xml = format!(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>Guitar</part-name></score-part></part-list>\
         <part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions></attributes>{body}</measure></part></score-partwise>"
    );
    let (s, w) = read_musicxml(xml.as_bytes()).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let t = &s.tracks[0];
    let keys: Vec<Option<u8>> = t.notes.iter().map(|n| t.key_of(n)).collect();
    assert_eq!(keys, [Some(52), Some(57), Some(62), Some(67)]);
    assert!(t.notes.iter().all(|n| n.fret <= 5));
}

#[test]
fn a_bass_part_is_fretted_for_a_bass() {
    let xml = "<score-partwise><part-list><score-part id=\"P1\"><part-name>Electric Bass</part-name>\
        <midi-instrument id=\"i\"><midi-program>34</midi-program></midi-instrument></score-part></part-list>\
        <part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
        <note><pitch><step>E</step><octave>1</octave></pitch><duration>1</duration></note>\
        <note><pitch><step>G</step><octave>2</octave></pitch><duration>1</duration></note>\
        </measure></part></score-partwise>";
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    assert_eq!(s.tracks[0].instrument_id, "bass4");
    assert_eq!(s.tracks[0].notes[0].fret, 0);
    assert_eq!(s.tracks[0].notes[0].string, 3);
}

fn repeat_doc(measures: &str) -> String {
    format!(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>g</part-name></score-part></part-list>\
         <part id=\"P1\">{measures}</part></score-partwise>"
    )
}

fn bar_with(fret: u8, extra: &str) -> String {
    let pitch = 64 - 5 + i32::from(fret); // string 3 (G) = 55 .. use pitch for G string
    let _ = pitch;
    format!(
        "<note><pitch><step>G</step><octave>3</octave></pitch><duration>4</duration>\
         <notations><technical><string>3</string><fret>{fret}</fret></technical></notations></note>{extra}"
    )
}

#[test]
fn repeats_and_endings_are_expanded() {
    // |: A | B 1. C :| 2. D | E   with the frets 1..5 as markers.
    let m = |n: u8, attrs: &str, barline: &str| {
        format!(
            "<measure number=\"{n}\">{attrs}{}{barline}</measure>",
            bar_with(n, "")
        )
    };
    let first = "<attributes><divisions>4</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>\
        <barline location=\"left\"><repeat direction=\"forward\"/></barline>";
    let c_attrs = "<barline location=\"left\"><ending number=\"1\" type=\"start\"/></barline>";
    let doc = repeat_doc(&format!(
        "{}{}{}{}{}",
        m(1, first, ""),
        m(2, "", ""),
        m(
            3,
            c_attrs,
            "<barline location=\"right\"><ending number=\"1\" type=\"stop\"/><repeat direction=\"backward\"/></barline>"
        ),
        m(
            4,
            "<barline location=\"left\"><ending number=\"2\" type=\"start\"/></barline>",
            "<barline location=\"right\"><ending number=\"2\" type=\"discontinue\"/></barline>"
        ),
        m(5, "", "")
    ));
    let (s, w) = read_musicxml(doc.as_bytes()).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let frets: Vec<u8> = s.tracks[0].notes.iter().map(|n| n.fret).collect();
    assert_eq!(frets, [1, 2, 3, 1, 2, 4, 5]);
    let ticks: Vec<i64> = s.tracks[0].notes.iter().map(|n| n.tick).collect();
    assert_eq!(ticks, (0..7).map(|i| i * BAR).collect::<Vec<_>>());
    // The file's own bars are kept for an exporter.
    assert_eq!(s.bars.len(), 5);
    assert!(s.bars[0].repeat_open);
    assert_eq!(s.bars[2].repeat_close, Some(2));
    assert_eq!(s.bars[2].volta, Some(0b01));
    assert_eq!(s.bars[3].volta, Some(0b10));
}

#[test]
fn tempo_comes_from_sound_or_the_metronome() {
    let xml = repeat_doc(
        "<measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
         <direction><direction-type><metronome><beat-unit>eighth</beat-unit><per-minute>200</per-minute></metronome></direction-type></direction>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>4</duration></note></measure>\
         <measure number=\"2\"><direction><direction-type><words>faster</words></direction-type><sound tempo=\"150\"/></direction>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>4</duration></note></measure>",
    );
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    assert_eq!(s.tempo, vec![(0, 100.0), (BAR, 150.0)]);
}

#[test]
fn a_notation_staff_and_a_tab_staff_in_one_part_read_once() {
    let xml = repeat_doc(
        "<measure number=\"1\"><attributes><divisions>1</divisions><staves>2</staves>\
         <clef number=\"1\"><sign>G</sign><line>2</line></clef><clef number=\"2\"><sign>TAB</sign><line>5</line></clef></attributes>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>4</duration><staff>1</staff></note>\
         <backup><duration>4</duration></backup>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>4</duration><staff>2</staff>\
         <notations><technical><string>1</string><fret>0</fret></technical></notations></note></measure>",
    );
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    assert_eq!(s.tracks[0].notes.len(), 1);
}

#[test]
fn techniques_are_read_from_hand_written_elements() {
    let xml = repeat_doc(
        "<measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
         <note><pitch><step>A</step><octave>3</octave></pitch><duration>1</duration><notehead>x</notehead>\
         <notations><technical><string>3</string><fret>2</fret><tap/></technical></notations></note>\
         <note><pitch><step>B</step><octave>3</octave></pitch><duration>1</duration>\
         <notations><technical><string>3</string><fret>4</fret><pull-off type=\"stop\">P</pull-off>\
         <bend><bend-alter>1.5</bend-alter></bend></technical></notations><play><mute>palm</mute></play></note>\
         <note><pitch><step>A</step><octave>3</octave></pitch><duration>2</duration>\
         <notations><technical><string>3</string><fret>2</fret><harmonic><natural/></harmonic></technical>\
         <ornaments><wavy-line type=\"start\"/></ornaments></notations></note></measure>",
    );
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    let n = &s.tracks[0].notes;
    assert!(n[0].tech.has(TechFlags::DEAD) && n[0].tech.has(TechFlags::TAP));
    assert!(n[1].tech.has(TechFlags::PULL_OFF) && n[1].tech.has(TechFlags::PALM_MUTE));
    assert_eq!(n[1].tech.bend.as_ref().map(Bend::peak), Some(150));
    assert!(n[2].tech.harmonic.is_some());
    assert_eq!(n[2].tech.vibrato, Some(VibratoKind::Normal));
}

#[test]
fn a_hammer_on_started_on_the_origin_lands_on_the_next_note() {
    let xml = repeat_doc(
        "<measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
         <note><pitch><step>A</step><octave>3</octave></pitch><duration>1</duration>\
         <notations><technical><string>3</string><fret>2</fret><hammer-on type=\"start\">H</hammer-on></technical></notations></note>\
         <note><pitch><step>B</step><octave>3</octave></pitch><duration>1</duration>\
         <notations><technical><string>3</string><fret>4</fret></technical></notations></note>\
         <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>\
         <notations><technical><string>3</string><fret>5</fret></technical></notations></note></measure>",
    );
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    let n = &s.tracks[0].notes;
    assert!(!n[0].tech.has(TechFlags::HAMMER_ON));
    assert!(n[1].tech.has(TechFlags::HAMMER_ON));
    assert!(!n[2].tech.has(TechFlags::HAMMER_ON));
}

#[test]
fn harmony_and_lyrics_are_read() {
    let xml = repeat_doc(
        "<measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
         <harmony><root><root-step>B</root-step><root-alter>-1</root-alter></root><kind>minor-seventh</kind><bass><bass-step>F</bass-step></bass></harmony>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration>\
         <lyric number=\"1\"><syllabic>begin</syllabic><text>fa</text></lyric></note>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration>\
         <lyric number=\"1\"><syllabic>end</syllabic><text>ke</text></lyric></note></measure>",
    );
    let (s, _) = read_musicxml(xml.as_bytes()).unwrap();
    let t = &s.tracks[0];
    assert_eq!(t.chords.len(), 1);
    assert_eq!(t.chords[0].1.name(), "Bbm7/F");
    assert_eq!(
        t.lyrics,
        vec![(0, "fa-".to_owned()), (2 * PPQ, "ke".to_owned())]
    );
}

#[test]
fn only_partwise_scores_are_read() {
    assert_eq!(
        read_musicxml(b"<score-timewise></score-timewise>"),
        Err(MusicXmlError::Timewise)
    );
    assert_eq!(read_musicxml(b"hello"), Err(MusicXmlError::UnknownFormat));
    assert_eq!(read_musicxml(b""), Err(MusicXmlError::UnknownFormat));
}

#[test]
fn sniffing_is_by_content() {
    assert_eq!(
        sniff(b"\xEF\xBB\xBF<?xml version=\"1.0\"?><!DOCTYPE x><score-partwise/>"),
        Some(MusicXmlFormat::Xml)
    );
    assert_eq!(sniff(b"<?xml version=\"1.0\"?><GPIF></GPIF>"), None);
    assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg></svg>"), None);
    assert_eq!(sniff(b"FICHIER GUITAR PRO v5.00"), None);
    assert_eq!(sniff(b"PK\x03\x04junk"), None);
}

#[test]
fn an_empty_score_cannot_be_written_and_an_empty_bar_is_a_whole_rest() {
    assert_eq!(
        write_musicxml(&TabScore::new()),
        Err(MusicXmlError::NoMusic)
    );
    let mut s = TabScore::new();
    s.tracks
        .push(TabTrack::new("g", "guitar6", tuning("guitar6", "standard")));
    let (xml, _) = write_musicxml(&s).unwrap();
    assert!(xml.contains("<rest measure=\"yes\"/>"));
}

#[test]
fn text_is_escaped() {
    let mut s = fixture("guitar6", "standard", 0);
    s.title = "A & B <c> \"d\"".into();
    let (xml, _) = write_musicxml(&sorted(s.clone())).unwrap();
    assert!(!xml.contains("<c>"));
    let (got, _) = read_musicxml(xml.as_bytes()).unwrap();
    assert_eq!(got.title, s.title);
}
