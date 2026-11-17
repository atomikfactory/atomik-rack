//! The GPIF reader and the two containers around it (`.gp` zip, `.gpx` file system), fed by the
//! builders in `gp_fixtures`.

mod gp_fixtures;

use atomik_tab::gp::{GpError, GpFormat, read_gp, sniff};
use atomik_tab::score::{
    BendKind, HarmonicKind, PPQ, SlideKind, StrumDirection, TabNote, TabScore, TechFlags,
    VibratoKind,
};
use gp_fixtures::containers::{bcfs, bcfz, gp, gpx, zip};
use gp_fixtures::gpif::{GUITAR, Gpif, enabled, property};

fn read_xml(g: &Gpif) -> (TabScore, Vec<String>) {
    let (s, w) = read_gp(g.render().as_bytes()).unwrap();
    (s, w.iter().map(|w| w.to_string()).collect())
}

/// A one-bar, one-track score whose beats are `(rhythm, notes)`; returns the notes.
fn one_bar(build: impl FnOnce(&mut Gpif) -> Vec<usize>) -> (TabScore, Vec<String>) {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let beats = build(&mut g);
    let v = g.voice(&beats);
    let b = g.bar(&[v]);
    g.master("4/4", &[b], "");
    read_xml(&g)
}

fn single_note(props: &[String], children: &str) -> TabNote {
    let (s, _) = one_bar(|g| {
        let q = g.rhythm("Quarter", 0, None);
        let n = g.note(3, 5, props, children);
        vec![g.beat(q, &[n], &[], "")]
    });
    assert_eq!(s.tracks[0].notes.len(), 1);
    s.tracks[0].notes[0].clone()
}

#[test]
fn reads_a_small_score() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 2, "");
    g.tempo(0, 0.0, "100 2");
    let q = g.rhythm("Quarter", 0, None);
    let e = g.rhythm("Eighth", 0, None);
    let h = g.rhythm("Half", 0, None);
    let n1 = g.note(5, 3, &[], "");
    let n2 = g.note(0, 0, &[], "");
    let n3 = g.note(2, 7, &[], "");
    let b1 = g.beat(q, &[n1], &[], "");
    let b2 = g.beat(e, &[n2, n3], &[], "");
    let b3 = g.beat(e, &[], &[], "");
    let b4 = g.beat(h, &[], &[], "");
    let v = g.voice(&[b1, b2, b3, b4]);
    let bar = g.bar(&[v]);
    g.master(
        "4/4",
        &[bar],
        "<Section><Letter>A</Letter><Text>Intro</Text></Section>",
    );
    let (s, w) = read_xml(&g);
    assert_eq!(
        (s.title.as_str(), s.artist.as_str()),
        ("Test Piece", "Nobody")
    );
    assert_eq!(
        (s.words_by.as_str(), s.music_by.as_str(), s.tab_by.as_str()),
        ("W", "M", "T")
    );
    assert_eq!(s.tempo, vec![(0, 100.0)]);
    assert_eq!(s.meter, vec![(0, 4, 4)]);
    assert_eq!(s.markers, vec![(0, "Intro".to_string())]);
    let t = &s.tracks[0];
    assert_eq!(t.name, "Guitar");
    assert_eq!(t.capo.fret, 2);
    assert_eq!(t.midi_program, 29);
    assert_eq!(t.instrument_id, "guitar6");
    // GPIF counts strings from the lowest: string 5 is the top line.
    let n: Vec<(i64, i64, u8, u8)> = t
        .notes
        .iter()
        .map(|n| (n.tick, n.len, n.string, n.fret))
        .collect();
    assert_eq!(
        n,
        vec![(0, PPQ, 0, 3), (PPQ, PPQ / 2, 3, 7), (PPQ, PPQ / 2, 5, 0)]
    );
    assert!(w.is_empty(), "{w:?}");
    assert!(s.problems().is_empty());
}

#[test]
fn rhythms_with_dots_and_tuplets() {
    let (s, _) = one_bar(|g| {
        let dq = g.rhythm("Quarter", 1, None);
        let ddq = g.rhythm("Eighth", 2, None);
        let trip = g.rhythm("Eighth", 0, Some((3, 2)));
        let sixty = g.rhythm("64th", 0, None);
        let mut out = Vec::new();
        for (r, fret) in [
            (dq, 1),
            (ddq, 2),
            (trip, 3),
            (trip, 4),
            (trip, 5),
            (sixty, 6),
        ] {
            let n = g.note(0, fret, &[], "");
            out.push(g.beat(r, &[n], &[], ""));
        }
        out
    });
    let n: Vec<(i64, i64)> = s.tracks[0].notes.iter().map(|n| (n.tick, n.len)).collect();
    let third = PPQ / 3;
    let ddq = PPQ / 2 * 7 / 4;
    assert_eq!(
        n,
        vec![
            (0, PPQ * 3 / 2),
            (PPQ * 3 / 2, ddq),
            (PPQ * 3 / 2 + ddq, third),
            (PPQ * 3 / 2 + ddq + third, third),
            (PPQ * 3 / 2 + ddq + 2 * third, third),
            (PPQ * 3 / 2 + ddq + PPQ, PPQ / 16),
        ]
    );
}

#[test]
fn tunings_for_other_instruments() {
    // A four-string bass, lowest string first.
    let mut g = Gpif::new();
    g.track("Bass", &[28, 33, 38, 43], 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let low = g.note(0, 0, &[], "");
    let high = g.note(3, 5, &[], "");
    let b = g.beat(q, &[low, high], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let (s, _) = read_xml(&g);
    let t = &s.tracks[0];
    assert_eq!(t.instrument_id, "bass4");
    assert_eq!(t.tuning.opens(), vec![43, 38, 33, 28]);
    assert_eq!(
        t.notes
            .iter()
            .map(|n| (n.string, n.fret))
            .collect::<Vec<_>>(),
        vec![(0, 5), (3, 0)]
    );

    // A five-string banjo, the drone listed first (the bottom line).
    let mut g = Gpif::new();
    g.raw_track(
        "<Track id=\"{id}\"><Name>Banjo</Name><Instrument ref=\"banjo5\"/><Staves><Staff><Properties>\
         <Property name=\"Tuning\"><Pitches>67 50 55 59 62</Pitches></Property></Properties></Staff></Staves></Track>",
    );
    let q = g.rhythm("Quarter", 0, None);
    let drone = g.note(0, 0, &[], "");
    let top = g.note(4, 2, &[], "");
    let b = g.beat(q, &[drone, top], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let (s, _) = read_xml(&g);
    let t = &s.tracks[0];
    assert_eq!(t.instrument_id, "banjo5");
    assert_eq!(t.tuning.strings[4].open, 67);
    assert_eq!(t.tuning.strings[4].start_fret, 5);
    assert_eq!(
        t.notes
            .iter()
            .map(|n| (n.string, n.fret))
            .collect::<Vec<_>>(),
        vec![(0, 2), (4, 0)]
    );

    // Seven strings.
    let mut g = Gpif::new();
    g.track("Seven", &[35, 40, 45, 50, 55, 59, 64], 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(0, 0, &[], "");
    let b = g.beat(q, &[n], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let (s, _) = read_xml(&g);
    assert_eq!(s.tracks[0].instrument_id, "guitar7");
    assert_eq!(s.tracks[0].notes[0].string, 6);
}

#[test]
fn note_techniques() {
    let t = single_note(
        &[enabled("PalmMuted"), enabled("Tapped")],
        "<LetRing/><Vibrato>Wide</Vibrato><Accent>5</Accent><AntiAccent>Normal</AntiAccent>",
    );
    let f = t.tech.flags;
    assert!(f.contains(
        TechFlags::PALM_MUTE
            | TechFlags::TAP
            | TechFlags::LET_RING
            | TechFlags::STACCATO
            | TechFlags::ACCENT
            | TechFlags::GHOST
    ));
    assert_eq!(t.tech.vibrato, Some(VibratoKind::Wide));
    assert!(
        single_note(&[enabled("Muted")], "")
            .tech
            .flags
            .contains(TechFlags::DEAD)
    );
    assert!(
        single_note(&[], "<Accent>8</Accent>")
            .tech
            .flags
            .contains(TechFlags::HEAVY_ACCENT)
    );
    assert!(
        single_note(&[enabled("Slapped")], "")
            .tech
            .flags
            .contains(TechFlags::SLAP)
    );
    assert!(
        single_note(&[enabled("Popped")], "")
            .tech
            .flags
            .contains(TechFlags::POP)
    );
    assert_eq!(
        single_note(&[], "<Vibrato>Slight</Vibrato>").tech.vibrato,
        Some(VibratoKind::Normal)
    );
}

#[test]
fn slides_and_harmonics() {
    for (flags, kind) in [
        (1, SlideKind::Shift),
        (2, SlideKind::Legato),
        (4, SlideKind::OutDown),
        (8, SlideKind::OutUp),
        (16, SlideKind::InFromBelow),
        (32, SlideKind::InFromAbove),
    ] {
        let t = single_note(&[property("Slide", &format!("<Flags>{flags}</Flags>"))], "");
        assert_eq!(t.tech.slide, Some(kind), "{flags}");
    }
    for (name, kind) in [
        ("Natural", HarmonicKind::Natural),
        ("Artificial", HarmonicKind::Artificial),
        ("Pinch", HarmonicKind::Pinch),
        ("Tap", HarmonicKind::Tapped),
        ("Semi", HarmonicKind::Semi),
    ] {
        let t = single_note(
            &[
                property("HarmonicType", &format!("<HType>{name}</HType>")),
                property("HarmonicFret", "<HFret>12</HFret>"),
            ],
            "",
        );
        let h = t.tech.harmonic.unwrap();
        assert_eq!(h.kind, kind, "{name}");
        assert_eq!(
            h.fret,
            matches!(kind, HarmonicKind::Artificial | HarmonicKind::Tapped).then_some(12.0)
        );
    }
}

#[test]
fn bends_use_offsets_in_percent() {
    let t = single_note(
        &[
            enabled("Bended"),
            property("BendOriginValue", "<Float>0</Float>"),
            property("BendMiddleValue", "<Float>50</Float>"),
            property("BendDestinationValue", "<Float>0</Float>"),
            property("BendOriginOffset", "<Float>0</Float>"),
            property("BendMiddleOffset1", "<Float>25</Float>"),
            property("BendMiddleOffset2", "<Float>50</Float>"),
            property("BendDestinationOffset", "<Float>100</Float>"),
        ],
        "",
    );
    let b = t.tech.bend.unwrap();
    assert_eq!(b.kind(), BendKind::BendRelease);
    assert_eq!(b.peak(), 100);
    assert!((b.at(0.25) - 100.0).abs() < 1e-3);
    assert!(b.at(1.0).abs() < 1e-3);
    // A plain bend of a whole step.
    let t = single_note(
        &[
            enabled("Bended"),
            property("BendOriginValue", "<Float>0</Float>"),
            property("BendDestinationValue", "<Float>100</Float>"),
        ],
        "",
    );
    let b = t.tech.bend.unwrap();
    assert_eq!(b.kind(), BendKind::Bend);
    assert_eq!(b.peak(), 200);
    // Not enabled: no bend.
    assert!(
        single_note(
            &[property("BendDestinationValue", "<Float>100</Float>")],
            ""
        )
        .tech
        .bend
        .is_none()
    );
}

#[test]
fn hammer_ons_are_marked_on_the_destination() {
    let (s, _) = one_bar(|g| {
        let q = g.rhythm("Quarter", 0, None);
        let a = g.note(3, 5, &[enabled("HopoOrigin")], "");
        let b = g.note(3, 7, &[enabled("HopoDestination")], "");
        let c = g.note(3, 7, &[enabled("HopoOrigin")], "");
        let d = g.note(3, 4, &[enabled("HopoDestination")], "");
        let e = g.note(2, 4, &[enabled("HopoDestination")], "");
        vec![
            g.beat(q, &[a], &[], ""),
            g.beat(q, &[b], &[], ""),
            g.beat(q, &[c], &[], ""),
            g.beat(q, &[d, e], &[], ""),
        ]
    });
    let n = &s.tracks[0].notes;
    assert!(n[0].tech.flags.is_empty());
    assert!(n[1].tech.flags.contains(TechFlags::HAMMER_ON));
    assert!(n[3].tech.flags.contains(TechFlags::PULL_OFF));
    // A destination with no note before it on its string gets nothing.
    assert!(n[4].tech.flags.is_empty());
}

#[test]
fn ties_keep_the_fret() {
    let (s, _) = one_bar(|g| {
        let h = g.rhythm("Half", 0, None);
        let a = g.note(3, 5, &[], "<Tie origin=\"true\" destination=\"false\"/>");
        let b = g.note(3, 0, &[], "<Tie origin=\"false\" destination=\"true\"/>");
        vec![g.beat(h, &[a], &[], ""), g.beat(h, &[b], &[], "")]
    });
    let n = &s.tracks[0].notes;
    assert!(!n[0].tie);
    assert_eq!((n[1].tie, n[1].fret), (true, 5));
}

#[test]
fn beat_effects_dynamics_and_chords() {
    let mut g = Gpif::new();
    g.raw_track(
        "<Track id=\"{id}\"><Name>G</Name><Staves><Staff><Properties>\
         <Property name=\"Tuning\"><Pitches>40 45 50 55 59 64</Pitches></Property>\
         <Property name=\"DiagramCollection\"><Items><Item id=\"0\" name=\"Am\"><Diagram stringCount=\"6\" fretCount=\"5\" baseFret=\"0\">\
         <Fret string=\"0\" fret=\"-1\"/><Fret string=\"1\" fret=\"0\"/><Fret string=\"2\" fret=\"2\"/><Fret string=\"3\" fret=\"2\"/>\
         <Fret string=\"4\" fret=\"1\"/><Fret string=\"5\" fret=\"0\"/></Diagram></Item></Items></Property>\
         </Properties></Staff></Staves></Track>",
    );
    let q = g.rhythm("Quarter", 0, None);
    let n1 = g.note(4, 1, &[], "");
    let n2 = g.note(3, 2, &[], "");
    let strum = g.beat(
        q,
        &[n1, n2],
        &[
            property("Brush", "<Direction>Up</Direction>"),
            enabled("Slapped"),
        ],
        "<Dynamic>PP</Dynamic><Chord>0</Chord><Tremolo>1/2</Tremolo><Fadding>FadeIn</Fadding>",
    );
    let n3 = g.note(3, 2, &[], "");
    let plain = g.beat(q, &[n3], &[], "");
    let v = g.voice(&[strum, plain]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let (s, _) = read_xml(&g);
    let t = &s.tracks[0];
    for n in &t.notes[..2] {
        assert_eq!(n.tech.strum, Some(StrumDirection::Up));
        assert!(n.tech.flags.contains(TechFlags::SLAP | TechFlags::FADE_IN));
        assert_eq!(n.tech.tremolo_division, 16);
        assert_eq!(n.velocity, 31);
    }
    // The dynamic holds until the next one.
    assert_eq!(t.notes[2].velocity, 31);
    assert!(t.notes[2].tech.strum.is_none());
    assert_eq!(t.diagrams.len(), 1);
    assert_eq!(t.diagrams[0].name, "Am");
    assert_eq!(
        t.diagrams[0].voicing.frets,
        vec![Some(0), Some(1), Some(2), Some(2), Some(0), None]
    );
    assert_eq!(t.chords[0].1.name(), "Am");
}

#[test]
fn the_whammy_bar_is_a_beat_curve() {
    let (s, _) = one_bar(|g| {
        let q = g.rhythm("Quarter", 0, None);
        let n = g.note(3, 5, &[], "");
        vec![g.beat(
            q,
            &[n],
            &[
                enabled("WhammyBar"),
                property("WhammyBarOriginValue", "<Float>0</Float>"),
                property("WhammyBarMiddleValue", "<Float>-50</Float>"),
                property("WhammyBarDestinationValue", "<Float>0</Float>"),
            ],
            "",
        )]
    });
    let w = s.tracks[0].notes[0].tech.whammy.clone().unwrap();
    assert_eq!(w.points.iter().map(|p| p.cents).min(), Some(-100));
}

#[test]
fn grace_beats_take_no_time() {
    let (s, _) = one_bar(|g| {
        let q = g.rhythm("Quarter", 0, None);
        let s32 = g.rhythm("32nd", 0, None);
        let grace_note = g.note(3, 3, &[], "");
        let main = g.note(3, 5, &[], "");
        let other = g.note(1, 2, &[], "");
        vec![
            g.beat(
                s32,
                &[grace_note],
                &[],
                "<GraceNotes>BeforeBeat</GraceNotes>",
            ),
            g.beat(q, &[main], &[], ""),
            g.beat(q, &[other], &[], ""),
        ]
    });
    let n = &s.tracks[0].notes;
    assert_eq!(n.len(), 2);
    assert_eq!((n[0].tick, n[0].fret), (0, 5));
    let gr = n[0].tech.grace.unwrap();
    assert_eq!((gr.fret, gr.on_beat, gr.ticks), (3, false, PPQ / 8));
    assert_eq!(n[1].tick, PPQ);
    assert!(n[1].tech.grace.is_none());
}

#[test]
fn repeats_and_alternative_endings_unroll() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let extras = [
        "<Repeat start=\"true\" end=\"false\" count=\"0\"/>",
        "",
        "<Repeat start=\"false\" end=\"true\" count=\"2\"/><AlternateEndings>1</AlternateEndings>",
        "<AlternateEndings>2</AlternateEndings>",
        "",
    ];
    for (i, extra) in extras.iter().enumerate() {
        let n = g.note(3, i as i32, &[], "");
        let b = g.beat(q, &[n], &[], "");
        let v = g.voice(&[b]);
        let bar = g.bar(&[v]);
        g.master(if i == 4 { "3/4" } else { "4/4" }, &[bar], extra);
    }
    let (s, _) = read_xml(&g);
    let frets: Vec<u8> = s.tracks[0].notes.iter().map(|n| n.fret).collect();
    assert_eq!(frets, [0, 1, 2, 0, 1, 3, 4]);
    assert_eq!(s.bars.len(), 5);
    assert_eq!(s.bars[2].volta, Some(1));
    assert_eq!(s.bars[3].volta, Some(2));
    assert_eq!(s.bars[2].repeat_close, Some(2));
    // The meter change is at the played bar number.
    assert_eq!(s.meter, vec![(0, 4, 4), (6, 3, 4)]);
}

#[test]
fn tempo_automations_and_units() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    g.tempo(0, 0.0, "120 2");
    g.tempo(1, 0.5, "60 3");
    g.tempo(2, 0.0, "50 4");
    let q = g.rhythm("Quarter", 0, None);
    for _ in 0..3 {
        let n = g.note(3, 0, &[], "");
        let b = g.beat(q, &[n], &[], "");
        let v = g.voice(&[b]);
        let bar = g.bar(&[v]);
        g.master("4/4", &[bar], "");
    }
    let (s, _) = read_xml(&g);
    assert_eq!(
        s.tempo,
        vec![(0, 120.0), (4 * PPQ + 2 * PPQ, 90.0), (8 * PPQ, 100.0)]
    );
}

#[test]
fn two_tracks_and_voices() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    g.track("Bass", &[28, 33, 38, 43], 0, "");
    let w = g.rhythm("Whole", 0, None);
    let h = g.rhythm("Half", 0, None);
    let n1 = g.note(3, 1, &[], "");
    let n2 = g.note(0, 0, &[], "");
    let n3 = g.note(5, 3, &[], "");
    let b1 = g.beat(h, &[n1], &[], "");
    let b1b = g.beat(h, &[n1], &[], "");
    let b2 = g.beat(w, &[n2], &[], "");
    let b3 = g.beat(w, &[n3], &[], "");
    let v1 = g.voice(&[b1, b1b]);
    let v2 = g.voice(&[b3]);
    let v3 = g.voice(&[b2]);
    let bar_a = g.bar(&[v1, v2]);
    let bar_b = g.bar(&[v3]);
    g.master("4/4", &[bar_a, bar_b], "");
    let (s, _) = read_xml(&g);
    assert_eq!(s.tracks.len(), 2);
    assert_eq!(s.tracks[0].notes.len(), 3);
    assert_eq!(s.tracks[1].notes.len(), 1);
    assert_eq!(s.tracks[1].tuning.len(), 4);
}

#[test]
fn drum_kits_map_elements_to_keys() {
    let mut g = Gpif::new();
    g.raw_track(
        "<Track id=\"{id}\"><Name>Drums</Name><InstrumentSet><Type>drumKit</Type><Elements>\
         <Element><Name>Kick</Name><Articulations><Articulation><OutputMidiNumber>36</OutputMidiNumber></Articulation></Articulations></Element>\
         <Element><Name>Hat</Name><Articulations><Articulation><OutputMidiNumber>42</OutputMidiNumber></Articulation>\
         <Articulation><OutputMidiNumber>46</OutputMidiNumber></Articulation></Articulations></Element>\
         </Elements></InstrumentSet><Staves><Staff></Staff></Staves></Track>",
    );
    let q = g.rhythm("Quarter", 0, None);
    let kick = g.raw_note(
        "<Note id=\"{id}\"><Properties><Property name=\"Element\"><Element>0</Element></Property>\
         <Property name=\"Variation\"><Variation>0</Variation></Property></Properties></Note>",
    );
    let hat = g.raw_note(
        "<Note id=\"{id}\"><Properties><Property name=\"Element\"><Element>1</Element></Property>\
         <Property name=\"Variation\"><Variation>1</Variation></Property></Properties></Note>",
    );
    let b1 = g.beat(q, &[kick], &[], "");
    let b2 = g.beat(q, &[hat], &[], "");
    let v = g.voice(&[b1, b2]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let (s, _) = read_xml(&g);
    let t = &s.tracks[0];
    assert!(t.is_percussion);
    assert_eq!(
        t.notes.iter().map(|n| n.fret).collect::<Vec<_>>(),
        vec![36, 46]
    );
}

#[test]
fn lyrics_and_key_signatures() {
    let mut g = Gpif::new();
    g.raw_track(
        "<Track id=\"{id}\"><Name>G</Name><Lyrics dispatched=\"true\"><Line><Text>La-la di</Text><Offset>0</Offset></Line></Lyrics>\
         <Staves><Staff><Properties><Property name=\"Tuning\"><Pitches>40 45 50 55 59 64</Pitches></Property></Properties></Staff></Staves></Track>",
    );
    let q = g.rhythm("Quarter", 0, None);
    let mut beats = Vec::new();
    for i in 0..3 {
        let n = g.note(3, i, &[], "");
        beats.push(g.beat(q, &[n], &[], ""));
    }
    let v = g.voice(&beats);
    let bar = g.bar(&[v]);
    g.master(
        "4/4",
        &[bar],
        "<Key><AccidentalCount>-3</AccidentalCount><Mode>Minor</Mode></Key>",
    );
    let (s, _) = read_xml(&g);
    let l: Vec<&str> = s.tracks[0].lyrics.iter().map(|l| l.1.as_str()).collect();
    assert_eq!(l, ["La", "la", "di"]);
    assert_eq!((s.key[0].1.fifths, s.key[0].1.minor), (-3, true));
}

#[test]
fn escaped_text_is_unescaped() {
    let mut g = Gpif::new();
    g.title = "Rock &amp; Roll &lt;1&gt;".into();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(0, 0, &[], "");
    let b = g.beat(q, &[n], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    assert_eq!(read_xml(&g).0.title, "Rock & Roll <1>");
}

#[test]
fn a_zip_holds_the_score_stored_or_deflated() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(3, 5, &[], "");
    let b = g.beat(q, &[n], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let xml = g.render();
    for deflate in [false, true] {
        let bytes = gp(&xml, deflate);
        assert_eq!(sniff(&bytes), Some(GpFormat::Zip));
        let (s, _) = read_gp(&bytes).unwrap();
        assert_eq!(s.title, "Test Piece");
        assert_eq!(s.tracks[0].notes[0].fret, 5, "deflate {deflate}");
    }
    // No score inside.
    let bytes = zip(&[("Content/other.xml", b"<a/>")], true);
    assert_eq!(read_gp(&bytes).unwrap_err(), GpError::NoScore);
    // An unknown compression method is refused politely.
    let mut bytes = zip(&[("Content/score.gpif", xml.as_bytes())], false);
    let at = bytes
        .windows(4)
        .rposition(|w| w == [b'P', b'K', 1, 2])
        .unwrap();
    bytes[at + 10] = 99; // the method of the only directory entry
    assert!(matches!(read_gp(&bytes), Err(GpError::Container(_))));
}

#[test]
fn a_gpx_holds_the_score_in_every_form() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let mut beats = Vec::new();
    for i in 0..64 {
        let n = g.note(i % 6, (i % 12) as i32, &[], "");
        beats.push(g.beat(q, &[n], &[], ""));
    }
    let v = g.voice(&beats[..4]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    let xml = g.render();
    for (compressed, lz) in [(false, false), (true, false), (true, true)] {
        let bytes = gpx(&xml, compressed, lz);
        assert_eq!(sniff(&bytes), Some(GpFormat::Gpx));
        let (s, _) =
            read_gp(&bytes).unwrap_or_else(|e| panic!("compressed {compressed} lz {lz}: {e}"));
        assert_eq!(s.tracks[0].notes.len(), 4, "{compressed} {lz}");
    }
    // The compressor really found repeats: the XML repeats a lot.
    assert!(gpx(&xml, true, true).len() < gpx(&xml, true, false).len());
}

#[test]
fn a_big_score_spans_many_sectors() {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let mut notes = Vec::new();
    for i in 0..400 {
        let n = g.note(i % 6, (i % 20) as i32, &[], "");
        notes.push(g.beat(q, &[n], &[], ""));
    }
    for chunk in notes.chunks(4) {
        let v = g.voice(chunk);
        let bar = g.bar(&[v]);
        g.master("4/4", &[bar], "");
    }
    let xml = g.render();
    assert!(xml.len() > 3 * 4096, "{}", xml.len());
    let (s, _) = read_gp(&gpx(&xml, true, true)).unwrap();
    assert_eq!(s.tracks[0].notes.len(), 400);
    assert_eq!(s.bars.len(), 100);
}

#[test]
fn damaged_containers_are_refused_with_messages() {
    assert!(matches!(read_gp(b"BCFS"), Err(GpError::NoScore)));
    let image = bcfs(&[("other.bin", b"abc")]);
    assert_eq!(read_gp(&image).unwrap_err(), GpError::NoScore);
    assert!(matches!(
        read_gp(b"BCFZ\x10\0\0\0"),
        Err(GpError::Container(_))
    ));
    let mut z = bcfz(&bcfs(&[("score.gpif", b"<GPIF/>")]), false);
    z.truncate(z.len() - 40);
    assert!(read_gp(&z).is_err());
    // The entry says it is longer than the sectors it lists.
    let mut image = bcfs(&[("score.gpif", b"<GPIF></GPIF>")]);
    image[4096 + 0x8C..4096 + 0x90].copy_from_slice(&100_000i32.to_le_bytes());
    assert!(matches!(
        read_gp(&image),
        Err(GpError::Container(_)) | Err(GpError::TooBig(_))
    ));
    // A block number far outside the file.
    let mut image = bcfs(&[("score.gpif", b"<GPIF></GPIF>")]);
    image[4096 + 0x94..4096 + 0x98].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(matches!(read_gp(&image), Err(GpError::Container(_))));
}

#[test]
fn bad_xml_and_wrong_roots() {
    assert!(matches!(
        read_gp(b"<?xml version=\"1.0\"?><other/>"),
        Err(GpError::Corrupt(_))
    ));
    assert!(matches!(
        read_gp(b"<?xml version=\"1.0\"?><GPIF><a></GPIF>"),
        Err(GpError::Xml(_))
    ));
    // A score with no tracks.
    let g = Gpif::new();
    assert!(matches!(
        read_gp(g.render().as_bytes()),
        Err(GpError::Corrupt(_))
    ));
}

#[test]
fn notes_on_missing_strings_are_dropped() {
    let (s, w) = one_bar(|g| {
        let q = g.rhythm("Quarter", 0, None);
        let ok = g.note(1, 1, &[], "");
        let bad = g.note(9, 1, &[], "");
        vec![g.beat(q, &[ok, bad], &[], "")]
    });
    assert_eq!(s.tracks[0].notes.len(), 1);
    assert!(w.iter().any(|w| w.contains("string")));
}
