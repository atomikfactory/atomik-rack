//! Hostile and damaged MusicXML input: truncation, flipped bytes, garbage, absurd numbers,
//! decompression bombs and size caps. Nothing here may panic, run for long or allocate without
//! bound; the reader returns an error or a score that holds the invariants the app relies on.

mod gp_fixtures;

use atomik_tab::instrument::find_tuning;
use atomik_tab::musicxml::{MusicXmlError, limits, read_musicxml, write_musicxml, write_mxl};
use atomik_tab::score::{Bend, PPQ, SlideKind, TabNote, TabScore, TabTrack, TechFlags};
use gp_fixtures::Rng;
use gp_fixtures::containers::zip;

fn small_score() -> TabScore {
    let t = find_tuning("guitar6", "standard").unwrap().tuning.clone();
    let mut s = TabScore::new();
    s.title = "Made Up".into();
    s.tempo = vec![(0, 110.0)];
    s.meter = vec![(0, 3, 4)];
    let mut tr = TabTrack::new("Guitar", "guitar6", t);
    for i in 0..6 {
        let mut n = TabNote::new(i * PPQ / 2, PPQ / 2, (i % 6) as u8, (i % 4) as u8);
        if i == 1 {
            n.tech.bend = Some(Bend::up(200));
        }
        if i == 2 {
            n.tech.flags = TechFlags::HAMMER_ON | TechFlags::PALM_MUTE;
        }
        if i == 3 {
            n.tech.slide = Some(SlideKind::Legato);
        }
        tr.notes.push(n);
    }
    tr.lyrics = vec![(0, "la-".into()), (PPQ, "la".into())];
    s.tracks.push(tr);
    s.sanitize();
    s
}

/// What the app relies on after any successful read.
fn check(score: &TabScore) {
    assert!(score.problems().is_empty(), "{:?}", score.problems());
    let mut again = score.clone();
    assert!(again.sanitize().is_empty(), "sanitize is not a no-op");
    assert!(score.tracks.len() <= atomik_tab::score::limits::MAX_TRACKS);
    for t in &score.tracks {
        for n in &t.notes {
            assert!(n.tick >= 0 && n.len >= 1 && (1..=127).contains(&n.velocity));
            if !t.is_percussion {
                assert!(usize::from(n.string) < t.tuning.len());
            }
        }
    }
    for (tick, bpm) in &score.tempo {
        assert!(*tick >= 0 && bpm.is_finite() && *bpm > 0.0);
    }
}

fn try_read(bytes: &[u8]) {
    if let Ok((score, _)) = read_musicxml(bytes) {
        check(&score);
        // What reads also writes and reads again.
        if let Ok((xml, _)) = write_musicxml(&score) {
            let (again, _) = read_musicxml(xml.as_bytes()).expect("our own output reads");
            check(&again);
        }
    }
}

#[test]
fn the_small_score_is_fine() {
    let s = small_score();
    let (xml, w) = write_musicxml(&s).unwrap();
    assert!(w.is_empty());
    let (got, _) = read_musicxml(xml.as_bytes()).unwrap();
    check(&got);
    assert_eq!(got.tracks[0].notes.len(), 6);
}

#[test]
fn truncation_at_every_byte_never_panics() {
    let (xml, _) = write_musicxml(&small_score()).unwrap();
    let bytes = xml.as_bytes();
    for cut in 0..bytes.len() {
        // A cut that lands inside a character is still bytes to the reader.
        try_read(&bytes[..cut]);
    }
}

#[test]
fn flipped_and_deleted_bytes_never_panic() {
    let (xml, _) = write_musicxml(&small_score()).unwrap();
    let base = xml.into_bytes();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..1500 {
        let mut b = base.clone();
        match rng.below(3) {
            0 => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            1 => {
                let i = rng.below(b.len());
                let n = rng.below(40).min(b.len() - i);
                b.drain(i..i + n);
            }
            _ => {
                let i = rng.below(b.len());
                let digits = rng.next().to_string().into_bytes();
                b.splice(i..i, digits);
            }
        }
        try_read(&b);
    }
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for round in 0..400 {
        let n = rng.below(600);
        let mut b: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        if round % 2 == 0 {
            let mut head =
                b"<?xml version=\"1.0\"?><score-partwise><part-list/><part id=\"P1\"><measure>"
                    .to_vec();
            head.extend_from_slice(&b);
            b = head;
        }
        try_read(&b);
    }
}

fn doc(attrs: &str, notes: &str) -> String {
    format!(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>g</part-name></score-part></part-list>\
         <part id=\"P1\"><measure number=\"1\"><attributes>{attrs}</attributes>{notes}</measure></part></score-partwise>"
    )
}

#[test]
fn absurd_numbers_are_bounded() {
    let hostile_attrs = [
        "<divisions>0</divisions>",
        "<divisions>-4</divisions>",
        "<divisions>99999999999999999999</divisions>",
        "<divisions>1000000</divisions>",
        "<divisions>1</divisions><time><beats>0</beats><beat-type>0</beat-type></time>",
        "<divisions>1</divisions><time><beats>999</beats><beat-type>3</beat-type></time>",
        "<divisions>1</divisions><key><fifths>99999</fifths></key>",
        "<divisions>1</divisions><staff-details><staff-lines>-3</staff-lines><capo>999</capo>\
         <staff-tuning line=\"1\"><tuning-step>E</tuning-step><tuning-octave>99999</tuning-octave></staff-tuning></staff-details>",
        "<divisions>1</divisions><staff-details><staff-tuning line=\"9\"><tuning-step>X</tuning-step></staff-tuning></staff-details>",
        "<divisions>1</divisions><transpose><chromatic>99999</chromatic><octave-change>-99999</octave-change></transpose>",
    ];
    let hostile_notes = [
        "<note><pitch><step>E</step><octave>4</octave></pitch><duration>99999999999999999</duration></note>",
        "<note><pitch><step>E</step><octave>4</octave></pitch><duration>-5</duration></note>",
        "<backup><duration>999999999999</duration></backup><note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>",
        "<forward><duration>999999999999</duration></forward><note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>",
        "<note><pitch><step>E</step><octave>99999</octave></pitch><duration>1</duration></note>",
        "<note><pitch><step>E</step><alter>1e308</alter><octave>4</octave></pitch><duration>1</duration></note>",
        "<note dynamics=\"1e308\"><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration>\
         <notations><technical><string>9999</string><fret>99999</fret></technical></notations></note>",
        "<note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration>\
         <notations><technical><bend><bend-alter>1e308</bend-alter></bend><bend><bend-alter>NaN</bend-alter><release/></bend></technical></notations></note>",
        "<direction><sound tempo=\"1e308\"/></direction><direction><sound tempo=\"-5\"/></direction><direction><sound tempo=\"NaN\"/></direction>\
         <note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>",
        "<harmony><root><root-step>H</root-step></root><kind>none</kind><frame><frame-note><string>0</string><fret>-1</fret></frame-note></frame></harmony>",
        "<note><grace/><pitch><step>E</step><octave>4</octave></pitch></note><note><grace/><pitch><step>E</step><octave>4</octave></pitch></note>",
        "<note><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>",
        "<note><unpitched/><duration>1</duration></note><note><rest/><duration>1</duration></note>",
    ];
    for a in hostile_attrs {
        for n in hostile_notes {
            try_read(doc(a, n).as_bytes());
        }
    }
}

#[test]
fn too_many_bars_are_refused() {
    let mut s = String::from(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>g</part-name></score-part></part-list><part id=\"P1\">",
    );
    for i in 0..(atomik_tab::score::limits::MAX_BARS + 5) {
        s.push_str(&format!("<measure number=\"{i}\"/>"));
    }
    s.push_str("</part></score-partwise>");
    assert_eq!(
        read_musicxml(s.as_bytes()),
        Err(MusicXmlError::TooMany("bars"))
    );
}

#[test]
fn too_many_parts_are_refused() {
    let mut s = String::from("<score-partwise><part-list/>");
    for _ in 0..(atomik_tab::score::limits::MAX_TRACKS + 5) {
        s.push_str("<part id=\"P\"><measure number=\"1\"/></part>");
    }
    s.push_str("</score-partwise>");
    assert_eq!(
        read_musicxml(s.as_bytes()),
        Err(MusicXmlError::TooMany("parts"))
    );
}

#[test]
fn xml_bombs_are_refused() {
    let mut deep = String::from("<?xml version=\"1.0\"?><score-partwise>");
    deep.push_str(&"<a>".repeat(20_000));
    assert!(matches!(
        read_musicxml(deep.as_bytes()),
        Err(MusicXmlError::Xml(_))
    ));
    // Entities are never expanded, so a billion-laughs document is only text.
    let laughs = "<?xml version=\"1.0\"?><!DOCTYPE score-partwise [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;\">\
        <!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;\">]><score-partwise><work><work-title>&c;&c;&c;</work-title></work>\
        <part-list/><part id=\"P\"><measure number=\"1\"/></part></score-partwise>";
    if let Ok((s, _)) = read_musicxml(laughs.as_bytes()) {
        assert!(s.title.len() < 1000);
    }
    // Too many elements for the cap, in a file within the size cap.
    let mut wide = String::from("<score-partwise>");
    wide.push_str(&"<a/>".repeat(atomik_tab::gp::limits::MAX_XML_NODES + 10));
    wide.push_str("</score-partwise>");
    assert!(wide.len() < limits::MAX_FILE_BYTES);
    assert!(matches!(
        read_musicxml(wide.as_bytes()),
        Err(MusicXmlError::TooMany(_))
    ));
}

#[test]
fn a_file_over_the_size_cap_is_refused_unread() {
    let big = vec![b'<'; limits::MAX_FILE_BYTES + 1];
    assert!(matches!(
        read_musicxml(&big),
        Err(MusicXmlError::TooLarge(_, _))
    ));
}

fn container() -> &'static [u8] {
    b"<container><rootfiles><rootfile full-path=\"score.xml\"/></rootfiles></container>"
}

#[test]
fn a_compressed_bomb_is_cut_at_the_cap() {
    // The score claims to be small and inflates past the cap.
    let mut doc = b"<score-partwise>".to_vec();
    doc.resize(limits::MAX_DECOMPRESSED + (2 << 20), b' ');
    let z = zip(
        &[("META-INF/container.xml", container()), ("score.xml", &doc)],
        true,
    );
    assert!(z.len() < limits::MAX_FILE_BYTES);
    assert!(matches!(read_musicxml(&z), Err(MusicXmlError::TooBig(_))));
}

#[test]
fn damaged_archives_never_panic() {
    let s = small_score();
    let (z, _) = write_mxl(&s).unwrap();
    for cut in (0..z.len()).step_by(3) {
        try_read(&z[..cut]);
    }
    let mut rng = Rng(77);
    for _ in 0..600 {
        let mut b = z.clone();
        let i = rng.below(b.len());
        b[i] = rng.next() as u8;
        try_read(&b);
    }
    // A zip without a container description, and one whose rootfile is missing.
    let plain = zip(&[("META-INF/container.xml", container())], false);
    assert!(read_musicxml(&plain).is_err());
    let none = zip(&[("a.txt", b"x")], true);
    assert!(read_musicxml(&none).is_err());
}

#[test]
fn an_archive_without_a_rootfile_uses_the_first_xml_entry() {
    let (xml, _) = write_musicxml(&small_score()).unwrap();
    let z = zip(
        &[
            ("META-INF/container.xml", b"<container/>"),
            ("song.musicxml", xml.as_bytes()),
        ],
        true,
    );
    let (got, _) = read_musicxml(&z).unwrap();
    assert_eq!(got.tracks[0].notes.len(), 6);
}

/// Writes the files `scripts/fuzz-seed.sh` seeds the fuzz corpus with
/// (it does nothing without the variable).
#[test]
#[ignore = "writes the fuzz seed corpus"]
fn write_fuzz_seeds() {
    let Some(dir) = std::env::var_os("ATOMIK_FUZZ_SEED_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = small_score();
    let (xml, _) = write_musicxml(&s).unwrap();
    std::fs::write(dir.join("score.musicxml"), &xml).unwrap();
    let (z, _) = write_mxl(&s).unwrap();
    std::fs::write(dir.join("score.mxl"), z).unwrap();
    std::fs::write(
        dir.join("pitch-only.xml"),
        doc(
            "<divisions>2</divisions>",
            "<note><pitch><step>E</step><octave>3</octave></pitch><duration>2</duration></note>\
             <harmony><root><root-step>E</root-step></root><kind>minor</kind></harmony>",
        ),
    )
    .unwrap();
}
