//! The guitar tab feature end to end in a headless session: a hand-written MusicXML tab comes in,
//! its track plays Plucked Strings, the Tab Editor shows the frets, the tab goes out as PDF and
//! MusicXML, the MusicXML reads back, and a bounce of the result is not silent.

use atomik_mcp::{Args, Paths, Server, ToolOutput};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn server() -> Server {
    Server::new(Paths::new(repo(), &[]))
}

fn call(s: &mut Server, name: &str, args: Value) -> ToolOutput {
    s.call_tool(name, &Args::new(Some(&args)))
}

fn ok(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, name, args);
    assert!(!r.is_error, "{name}: {}", r.first_text());
    r.parsed()
}

/// A path inside the allowed folders, fresh for this test.
fn tmp(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-session-tab");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

/// One quarter note of a guitar tab: the sounding pitch, the string (1 is the highest) and the fret.
fn note(step: &str, octave: u8, string: u8, fret: u8) -> String {
    format!(
        "<note><pitch><step>{step}</step><octave>{octave}</octave></pitch><duration>1</duration>\
         <type>quarter</type><notations><technical><string>{string}</string><fret>{fret}</fret>\
         </technical></notations></note>"
    )
}

/// A made-up two-bar melody in standard tuning, written by hand.
fn melody() -> String {
    let tuning: String = [("E", 2), ("A", 2), ("D", 3), ("G", 3), ("B", 3), ("E", 4)]
        .iter()
        .enumerate()
        .map(|(i, (step, octave))| {
            format!(
                "<staff-tuning line=\"{}\"><tuning-step>{step}</tuning-step>\
                 <tuning-octave>{octave}</tuning-octave></staff-tuning>",
                i + 1
            )
        })
        .collect();
    let bar1 = [
        note("B", 3, 2, 0),
        note("D", 4, 2, 3),
        note("E", 4, 1, 0),
        note("G", 4, 1, 3),
    ]
    .concat();
    let bar2 = [
        note("A", 3, 3, 2),
        note("C", 4, 2, 1),
        note("E", 4, 1, 0),
        note("D", 4, 2, 3),
    ]
    .concat();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<score-partwise version=\"3.1\"><work><work-title>Porch Light</work-title></work>\
<part-list><score-part id=\"P1\"><part-name>Guitar</part-name></score-part></part-list>\
<part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions>\
<key><fifths>0</fifths></key><time><beats>4</beats><beat-type>4</beat-type></time>\
<clef><sign>TAB</sign><line>5</line></clef>\
<staff-details><staff-lines>6</staff-lines>{tuning}</staff-details></attributes>\
<direction placement=\"above\"><direction-type><words>Easy</words></direction-type>\
<sound tempo=\"120\"/></direction>{bar1}</measure>\
<measure number=\"2\">{bar2}</measure></part></score-partwise>"
    )
}

/// The texts the session draws, in drawing order.
fn texts(s: &mut Server) -> Vec<String> {
    let t = ok(s, "session_texts", json!({"limit": 600}));
    t["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["text"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn a_musicxml_tab_goes_through_the_whole_feature() {
    let mut s = server();
    ok(&mut s, "session_open", json!({}));
    let xml = tmp("porch-light.musicxml");
    std::fs::write(&xml, melody()).unwrap();

    // Import: one track of 8 notes, with the file's tempo.
    let imported = ok(
        &mut s,
        "session_tab",
        json!({"action": "import", "file": xml, "as_song": true}),
    );
    assert_eq!(imported["tracks"], 1, "{imported}");
    assert_eq!(imported["can_undo"], true);
    assert!(
        imported["message"].as_str().unwrap().contains("8 notes"),
        "{imported}"
    );
    // The track plays Plucked Strings. (A headless session has no factory folder, so the
    // instrument's preset is not loaded here: the device keeps its default patch.)
    let status = ok(&mut s, "session_status", json!({}));
    let devices = status["document"]["devices"].as_array().unwrap();
    let guitar = devices
        .iter()
        .find(|d| d["type_id"] == "atomik.plucked")
        .unwrap_or_else(|| panic!("no Plucked Strings device: {}", status["document"]));
    assert_eq!(guitar["name"], "Guitar");
    assert_eq!(status["document"]["song"]["tempo_events"][0]["bpm"], 120.0);

    // The Tab Editor shows the strings and the fret numbers of the melody.
    ok(
        &mut s,
        "session_command",
        json!({"id": "view.toggle_tab_editor"}),
    );
    let drawn = texts(&mut s);
    // The staff draws the fret numbers of the eight notes, then the instrument label.
    let label = drawn
        .iter()
        .position(|t| t.contains("Guitar (6 strings)"))
        .unwrap_or_else(|| panic!("no instrument label: {drawn:?}"));
    let mut frets: Vec<&str> = drawn[label - 8..label].iter().map(String::as_str).collect();
    frets.sort_unstable();
    assert_eq!(
        frets,
        ["0", "0", "0", "1", "2", "3", "3", "3"],
        "fret numbers drawn: {drawn:?}"
    );

    // Exports: a PDF and MusicXML.
    let pdf = tmp("porch-light.pdf");
    let printed = ok(
        &mut s,
        "session_tab",
        json!({"action": "export", "file": pdf, "format": "pdf", "page": "a4"}),
    );
    assert!(printed["bytes"].as_u64().unwrap() > 500, "{printed}");
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
    let back = tmp("porch-light-out.musicxml");
    ok(
        &mut s,
        "session_tab",
        json!({"action": "export", "file": back, "format": "musicxml"}),
    );
    let written = std::fs::read_to_string(&back).unwrap();
    assert!(written.contains("<staff-tuning"), "the tuning is written");
    assert!(written.contains("<fret>3</fret>"));

    // The exported MusicXML reads again, into a fresh session: the same notes.
    let mut again = server();
    ok(&mut again, "session_open", json!({}));
    let reread = ok(
        &mut again,
        "session_tab",
        json!({"action": "import", "file": back, "as_song": true}),
    );
    assert_eq!(reread["tracks"], 1, "{reread}");
    assert!(
        reread["message"].as_str().unwrap().contains("8 notes"),
        "{reread}"
    );

    // A bounce of the imported song is not silent.
    let wav = tmp("porch-light.wav");
    let bounce = ok(&mut s, "session_bounce", json!({"output": wav}));
    assert_eq!(bounce["audio"]["silent"], false, "{bounce}");
    let heard = ok(
        &mut again,
        "session_bounce",
        json!({"output": tmp("again.wav")}),
    );
    assert_eq!(heard["audio"]["silent"], false, "{heard}");
    // The practice speed is a setting of the live engine: a bounce does not change with it.
    ok(
        &mut s,
        "session_command",
        json!({"id": "practice.speed_down"}),
    );
    assert!(
        texts(&mut s).iter().any(|t| t == "95%"),
        "the bar shows the speed"
    );
    let slowed = ok(
        &mut s,
        "session_bounce",
        json!({"output": tmp("porch-light-50.wav"), "reference": wav, "tolerance_db": 0.0}),
    );
    assert_eq!(slowed["comparison"]["bit_identical"], true, "{slowed}");
}
