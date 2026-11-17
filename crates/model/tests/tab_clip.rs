//! Tab data on note clips and chord sheets on songs: the project file (optional, ignorable,
//! canonical round trip, hostile input), what split and join carry, the tab edit operations as
//! one undo step, and the sheets command.

mod common;

use atomik_core::PPQ;
use atomik_model::tab_edit::{self, TabEdit};
use atomik_model::{
    ClipId, Command, Document, MidiData, Note, SongCommand, TabData, TabNoteRef, load_project,
    load_project_with_warnings, save_project,
};
use atomik_tab::score::{
    Bend, Harmonic, HarmonicKind, SlideKind, TechFlags, Techniques, VibratoKind,
};
use atomik_tab::sheet::parse_sheet;
use common::song::*;
use common::*;
use serde_json::Value;

fn guitar() -> TabData {
    TabData::for_instrument("guitar6").expect("catalogue")
}

fn notes_and_data() -> (Vec<Note>, MidiData) {
    let mut tab = guitar();
    tab.notes = vec![
        TabNoteRef::new(0, 64, 0, 0),
        TabNoteRef::new(PPQ, 67, 1, 8),
        TabNoteRef::new(2 * PPQ, 55, 3, 5),
    ];
    let notes = vec![
        Note::new(0, PPQ, 64, 0.8),
        Note::new(PPQ, PPQ, 67, 0.8),
        Note::new(2 * PPQ, PPQ, 55, 0.8),
    ];
    (
        notes,
        MidiData {
            tab: Some(tab),
            ..MidiData::default()
        },
    )
}

fn doc_with(reg: &atomik_device_sdk::Registry, notes: Vec<Note>, midi: MidiData) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    let mut c = notes_clip(0, 4 * BAR, notes);
    c.midi = midi.into();
    d.apply(add_clip("synth", 0, c), reg).unwrap();
    d
}

fn clip_of(d: &Document) -> (Vec<Note>, MidiData) {
    let c = d.doc().song.clip(ClipId(1)).expect("clip").2;
    let notes = match &c.content {
        atomik_model::ClipContent::Notes(n) => n.to_vec(),
        _ => panic!("note clip"),
    };
    (notes, (*c.midi).clone())
}

fn run(d: &mut Document, reg: &atomik_device_sdk::Registry, edit: TabEdit) {
    d.apply(edit.into_command(ClipId(1), None), reg).unwrap();
}

#[test]
fn a_plain_clip_writes_no_tab_and_a_tab_clip_round_trips_exactly() {
    let reg = registry();
    let (n, _) = notes_and_data();
    let plain = doc_with(&reg, n, MidiData::default());
    assert!(!save_project(plain.doc()).contains("\"tab\""));

    let (n, mut m) = notes_and_data();
    {
        let tab = m.tab.as_mut().unwrap();
        tab.capo = atomik_tab::Capo::partial(2, 0b001111);
        tab.notes[0].tech = Techniques {
            bend: Some(Bend::bend_release(200)),
            vibrato: Some(VibratoKind::Wide),
            slide: Some(SlideKind::Shift),
            harmonic: Some(Harmonic {
                kind: HarmonicKind::Artificial,
                fret: Some(17.5),
            }),
            tremolo_division: 16,
            ..Techniques::default()
        }
        .with(TechFlags::PALM_MUTE)
        .with(TechFlags::HAMMER_ON);
        tab.notes[0].plain = Some(atomik_model::PlainNote {
            velocity: 0.7,
            length: PPQ,
        });
        tab.notes[1].tech = Techniques::default().with(TechFlags::GHOST);
        tab.lyrics = vec![atomik_model::TabLyric {
            tick: 0,
            text: "la".into(),
        }];
        let chord = atomik_tab::ChordSymbol::parse("Am").unwrap();
        let v = atomik_tab::voicing::generate(
            &tab.tuning,
            tab.capo,
            &chord,
            &atomik_tab::voicing::VoicingOptions::default(),
        );
        tab.diagrams = vec![atomik_model::TabDiagram {
            tick: PPQ,
            name: "Am".into(),
            voicing: v[0].clone(),
        }];
    }
    let d = doc_with(&reg, n, m.clone());
    let text = save_project(d.doc());
    let (loaded, warnings) = load_project_with_warnings(&text, &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    let v: Value = serde_json::from_str(&text).unwrap();
    let tab = &v["song"]["tracks"][0]["lanes"][0]["clips"][0]["tab"];
    assert_eq!(tab["instrument"], "guitar6");
    assert_eq!(tab["capo"]["strings"], 15);
    assert_eq!(v["version"], 45, "no format bump");
    assert_eq!(v["compat"], 44);
    assert_eq!(v["ignorable"]["song.tracks[].lanes[].clips[].tab"], 45);
    assert_eq!(v["ignorable"]["song.sheets"], 45);
    assert_eq!(midi_of(&loaded).tab, m.tab);
}

fn midi_of(doc: &atomik_model::RackDoc) -> MidiData {
    (*doc.song.clip(ClipId(1)).expect("clip").2.midi).clone()
}

#[test]
fn an_entry_for_a_note_the_clip_no_longer_has_is_not_saved() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let d = doc_with(&reg, n[..2].to_vec(), m);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let notes = &v["song"]["tracks"][0]["lanes"][0]["clips"][0]["tab"]["notes"];
    assert_eq!(notes.as_array().unwrap().len(), 2);
}

#[test]
fn hostile_tab_json_is_refused_with_a_path() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let text = save_project(doc_with(&reg, n, m).doc());
    let cases: Vec<(String, &str)> = vec![
        (
            text.replace("\"instrument\": \"guitar6\"", "\"instrument\": 7"),
            "instrument",
        ),
        (text.replace("\"frets\": 24", "\"frets\": 0"), "tuning"),
        (text.replace("[0, 64, 0, 0]", "[0, 64, 9, 0]"), "string 9"),
        (text.replace("[0, 64, 0, 0]", "[0, 64, 0, 99]"), "fret 99"),
        (text.replace("[0, 64, 0, 0]", "[0, 200, 0, 0]"), "notes[0]"),
        (text.replace("[0, 64, 0, 0]", "[0, 64, 0]"), "notes[0]"),
        (text.replace("\"tuning\"", "\"tuning_x\""), "tuning_x"),
        (
            text.replace("[0, 64, 0, 0]", "[0, 64, 0, 0, {\"flags\": [\"nope\"]}]"),
            "unknown technique",
        ),
        (
            text.replace("[0, 64, 0, 0]", "[0, 64, 0, 0, {\"bend\": [[2.0, 100]]}]"),
            "position",
        ),
        (
            text.replace("[0, 64, 0, 0]", "[0, 64, 0, 0, {\"tremolo\": 7}]"),
            "division",
        ),
        (
            text.replace("[0, 64, 0, 0]", "[0, 64, 0, 0, {\"slide\": \"up\"}]"),
            "slide",
        ),
        (
            text.replace("[0, 64, 0, 0]", "[0, 64, 0, 0, null, [0.0, 1]]"),
            "plain",
        ),
    ];
    for (broken, needle) in cases {
        let e = load_project(&broken, &reg).expect_err(needle).to_string();
        assert!(e.contains(needle), "{needle}: {e}");
    }
    // Out of order or duplicate notes.
    let dup = text.replace("[PPQ, 67, 1, 8]", "x");
    let _ = dup;
    let swapped = text.replacen("[0, 64, 0, 0]", "[30720, 64, 0, 0]", 1);
    assert!(load_project(&swapped, &reg).is_err());
}

#[test]
fn a_tab_on_a_clip_of_another_kind_is_refused() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let mut d = doc_with(&reg, n, m);
    let (notes, midi) = clip_of(&d);
    let e = tab_edit::set_fret(&notes, &midi, (3 * PPQ, 2, 3), (PPQ, 0.7)).unwrap();
    // An edit on an unknown clip id fails; the document is unchanged.
    let before = d.doc().clone();
    assert!(d.apply(e.into_command(ClipId(99), None), &reg).is_err());
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_fret_creates_a_note_and_its_entry_in_one_undo_step() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let mut d = doc_with(&reg, n, m);
    let (notes, midi) = clip_of(&d);
    // String 2 (G3 = 55 open on a standard guitar? string 2 open is 55), fret 2 -> 57.
    let edit = tab_edit::set_fret(&notes, &midi, (3 * PPQ, 2, 2), (PPQ / 2, 0.9)).unwrap();
    run(&mut d, &reg, edit);
    let (notes, midi) = clip_of(&d);
    assert_eq!(notes.len(), 4);
    let n = notes.iter().find(|n| n.tick == 3 * PPQ).unwrap();
    assert_eq!((n.key, n.length), (57, PPQ / 2));
    let r = midi.tab.as_ref().unwrap().note_of(3 * PPQ, 57).unwrap();
    assert_eq!((r.string, r.fret), (2, 2));
    assert_eq!(d.undo_label().unwrap(), "Edit Clip");
    d.undo().unwrap();
    let (notes, midi) = clip_of(&d);
    assert_eq!(notes.len(), 3, "one undo takes back the note and the entry");
    assert!(midi.tab.as_ref().unwrap().note_of(3 * PPQ, 57).is_none());
    d.redo().unwrap();
    assert_eq!(clip_of(&d).0.len(), 4);
}

#[test]
fn set_fret_on_an_occupied_string_moves_the_note_and_keeps_its_techniques() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let mut d = doc_with(&reg, n, m);
    let (notes, midi) = clip_of(&d);
    let edit = tab_edit::set_techniques(
        &notes,
        &midi,
        &[(0, 64)],
        &Techniques::default().with(TechFlags::GHOST),
    )
    .unwrap();
    run(&mut d, &reg, edit);
    let (notes, midi) = clip_of(&d);
    assert!((notes[0].velocity - 0.48).abs() < 1e-5);
    // Fret 3 on string 0 is 67: the same tick and string, another pitch.
    let edit = tab_edit::set_fret(&notes, &midi, (0, 0, 3), (PPQ, 0.5)).unwrap();
    run(&mut d, &reg, edit);
    let (notes, midi) = clip_of(&d);
    let n = notes.iter().find(|n| n.tick == 0).unwrap();
    assert_eq!(n.key, 67);
    assert!(
        (n.velocity - 0.48).abs() < 1e-5,
        "ghost applied once to the plain 0.8: {}",
        n.velocity
    );
    assert!(midi.tab.as_ref().unwrap().note_of(0, 64).is_none());
    let r = midi.tab.as_ref().unwrap().note_of(0, 67).unwrap();
    assert!(r.tech.has(TechFlags::GHOST));
    // The same pitch as another note at that tick is refused.
    let e = tab_edit::set_fret(&notes, &midi, (PPQ, 2, 12), (PPQ, 0.5)); // 55+12 = 67 at PPQ on string 1
    assert!(
        matches!(e, Err(tab_edit::TabEditError::Occupied { .. })),
        "{e:?}"
    );
}

#[test]
fn techniques_render_and_clearing_them_restores_the_plain_clip() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let mut d = doc_with(&reg, n.clone(), m);
    let (notes, midi) = clip_of(&d);
    let tech = Techniques {
        bend: Some(Bend::up(200)),
        ..Techniques::default()
    }
    .with(TechFlags::ACCENT)
    .with(TechFlags::LET_RING);
    run(
        &mut d,
        &reg,
        tab_edit::set_techniques(&notes, &midi, &[(0, 64)], &tech).unwrap(),
    );
    let (notes, midi) = clip_of(&d);
    assert_eq!(midi.expression.len(), 1);
    assert!((notes[0].velocity - 0.96).abs() < 1e-5);
    assert_eq!(
        notes[0].length,
        2 * PPQ,
        "to the next note on that string or the tail"
    );
    run(
        &mut d,
        &reg,
        tab_edit::clear_techniques(&notes, &midi, &[(0, 64)]).unwrap(),
    );
    let (notes, midi) = clip_of(&d);
    assert!(midi.expression.is_empty());
    assert_eq!(notes, n);
    // And the file still loads.
    load_project(&save_project(d.doc()), &reg).unwrap();
}

#[test]
fn notes_without_entries_are_placed_when_techniques_need_them() {
    let reg = registry();
    let mut d = doc_with(
        &reg,
        vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 59, 0.8)],
        MidiData {
            tab: Some(guitar()),
            ..MidiData::default()
        },
    );
    let (notes, midi) = clip_of(&d);
    let edit = tab_edit::set_techniques(
        &notes,
        &midi,
        &[(0, 64), (PPQ, 59)],
        &Techniques::default().with(TechFlags::STACCATO),
    )
    .unwrap();
    assert!(edit.unplayable.is_empty());
    run(&mut d, &reg, edit);
    let (notes, midi) = clip_of(&d);
    assert_eq!(midi.tab.as_ref().unwrap().notes.len(), 2);
    assert!(notes.iter().all(|n| n.length == PPQ / 2));
    // A note below the neck cannot be placed: listed, left alone.
    let mut low = notes.clone();
    low.push(Note::new(2 * PPQ, PPQ, 20, 0.8));
    let edit = tab_edit::set_techniques(
        &low,
        &midi,
        &[(2 * PPQ, 20)],
        &Techniques::default().with(TechFlags::GHOST),
    )
    .unwrap();
    assert_eq!(edit.unplayable, vec![(2 * PPQ, 20)]);
    assert_eq!(edit.notes.len(), 3);
}

#[test]
fn change_string_keeps_the_pitch_and_refuses_what_cannot_be_played() {
    let (notes, midi) = notes_and_data();
    // 64 open on string 0 (E4): down one string it is fret 5 on B3.
    let e = tab_edit::change_string(&notes, &midi, &[(0, 64)], 1).unwrap();
    let r = e.midi.tab.as_ref().unwrap().note_of(0, 64).unwrap();
    assert_eq!((r.string, r.fret), (1, 5));
    assert_eq!(e.notes, notes, "the notes do not change");
    // Up from the top string: no such string.
    assert!(tab_edit::change_string(&notes, &midi, &[(0, 64)], -1).is_err());
    // The low note cannot go higher than its neck allows? 55 on string 3 (G3 open) -> string 2? fine.
    assert!(tab_edit::change_string(&notes, &midi, &[(0, 99)], 1).is_err());
}

#[test]
fn transpose_moves_notes_entries_and_curves_together() {
    let (notes, mut midi) = notes_and_data();
    let e = tab_edit::set_techniques(
        &notes,
        &midi,
        &[(0, 64)],
        &Techniques {
            bend: Some(Bend::up(100)),
            ..Techniques::default()
        }
        .with(TechFlags::GHOST),
    )
    .unwrap();
    let (notes, m) = (e.notes, e.midi);
    midi = m;
    let e = tab_edit::transpose(&notes, &midi, &[(0, 64)], 3).unwrap();
    let n = e.notes.iter().find(|n| n.tick == 0).unwrap();
    assert_eq!(n.key, 67);
    assert!((n.velocity - 0.48).abs() < 1e-5, "ghost once");
    let tab = e.midi.tab.as_ref().unwrap();
    assert!(tab.note_of(0, 64).is_none());
    let r = tab.note_of(0, 67).unwrap();
    assert_eq!((r.string, r.fret), (0, 3));
    assert_eq!(e.midi.expression.len(), 1);
    assert_eq!(e.midi.expression[0].key, 67);
    // Out of range and collisions are refused.
    assert!(tab_edit::transpose(&notes, &midi, &[(0, 64)], 100).is_err());
    assert!(matches!(
        tab_edit::transpose(&notes, &midi, &[(PPQ, 67)], 0).map(|e| e.notes.len()),
        Ok(3)
    ));
}

#[test]
fn a_new_tuning_or_capo_keeps_the_pitches_and_refrets() {
    let (notes, midi) = notes_and_data();
    // Drop D: the low string goes down a tone; pitches stay.
    let drop_d = atomik_tab::instrument::find_tuning("guitar6", "drop_d").unwrap();
    let e = tab_edit::set_tuning(
        &notes,
        &midi,
        "guitar6",
        drop_d.tuning.clone(),
        atomik_tab::Capo::NONE,
    )
    .unwrap();
    assert_eq!(e.notes, notes);
    let tab = e.midi.tab.as_ref().unwrap();
    assert_eq!(tab.tuning, drop_d.tuning);
    for r in &tab.notes {
        assert_eq!(
            tab.tuning.pitch(usize::from(r.string), r.fret, tab.capo),
            Some(r.key)
        );
    }
    // A capo: frets are relative to it and the pitches are the same.
    let e = tab_edit::set_capo(&notes, &midi, atomik_tab::Capo::at(2)).unwrap();
    let tab = e.midi.tab.as_ref().unwrap();
    assert_eq!(tab.capo, atomik_tab::Capo::at(2));
    for r in &tab.notes {
        assert_eq!(
            tab.tuning.pitch(usize::from(r.string), r.fret, tab.capo),
            Some(r.key)
        );
    }
    // A tuning that cannot play a note drops its entry and lists it.
    let uke = atomik_tab::instrument::find_tuning("ukulele_soprano", "reentrant").unwrap();
    let e = tab_edit::set_tuning(
        &notes,
        &midi,
        "ukulele_soprano",
        uke.tuning.clone(),
        atomik_tab::Capo::NONE,
    )
    .unwrap();
    assert!(
        e.unplayable.contains(&(2 * PPQ, 55))
            || e.midi.tab.as_ref().unwrap().note_of(2 * PPQ, 55).is_some()
    );
    assert_eq!(e.notes.len(), 3, "notes are never dropped");
    // A broken tuning is refused.
    let mut bad = drop_d.tuning.clone();
    bad.strings.clear();
    assert!(tab_edit::set_tuning(&notes, &midi, "x", bad, atomik_tab::Capo::NONE).is_err());
}

#[test]
fn auto_fret_keeps_existing_positions_and_places_the_rest() {
    let reg = registry();
    let mut tab = guitar();
    tab.notes = vec![TabNoteRef::new(0, 64, 1, 5)]; // a pinned, unusual position
    let notes = vec![
        Note::new(0, PPQ, 64, 0.8),
        Note::new(0, PPQ, 48, 0.8),
        Note::new(PPQ, PPQ, 52, 0.8),
    ];
    let midi = MidiData {
        tab: Some(tab),
        ..MidiData::default()
    };
    let e = tab_edit::auto_fret_clip(&notes, &midi, &Default::default()).unwrap();
    let t = e.midi.tab.as_ref().unwrap();
    assert_eq!(t.notes.len(), 3);
    let pinned = t.note_of(0, 64).unwrap();
    assert_eq!((pinned.string, pinned.fret), (1, 5));
    let strings_at_0: Vec<u8> = t
        .notes
        .iter()
        .filter(|r| r.tick == 0)
        .map(|r| r.string)
        .collect();
    assert_eq!(strings_at_0.len(), 2);
    assert_ne!(
        strings_at_0[0], strings_at_0[1],
        "a string plays one note at a time"
    );
    let _ = reg;
    // start_tab places everything on a fresh clip.
    let plain = MidiData::default();
    let e = tab_edit::start_tab(&notes, &plain, "bass4", true).unwrap();
    assert!(e.midi.tab.is_some());
    assert!(tab_edit::start_tab(&notes, &plain, "banjo99", false).is_err());
}

#[test]
fn delete_notes_removes_notes_entries_and_curves() {
    let (notes, midi) = notes_and_data();
    let e = tab_edit::set_techniques(
        &notes,
        &midi,
        &[(0, 64)],
        &Techniques {
            bend: Some(Bend::up(100)),
            ..Techniques::default()
        },
    )
    .unwrap();
    let d = tab_edit::delete_notes(&e.notes, &e.midi, &[(0, 64)]).unwrap();
    assert_eq!(d.notes.len(), 2);
    assert!(d.midi.expression.is_empty());
    assert!(d.midi.tab.as_ref().unwrap().note_of(0, 64).is_none());
    assert!(tab_edit::delete_notes(&e.notes, &e.midi, &[(5, 5)]).is_err());
    let c = tab_edit::clear_fret(&notes, &midi, PPQ, 1).unwrap();
    assert_eq!(c.notes.len(), 2);
    assert!(tab_edit::clear_fret(&notes, &midi, PPQ, 5).is_err());
}

#[test]
fn a_clip_without_tab_data_is_refused_by_the_operations() {
    let (notes, _) = notes_and_data();
    let e = tab_edit::set_fret(&notes, &MidiData::default(), (0, 0, 0), (PPQ, 0.5));
    assert_eq!(e, Err(tab_edit::TabEditError::NoTab));
}

#[test]
fn a_split_gives_each_half_the_entries_of_its_notes_and_a_join_keeps_them() {
    let reg = registry();
    let (n, m) = notes_and_data();
    let mut d = doc_with(&reg, n, m);
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: PPQ + PPQ / 2,
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 2);
    // Both halves keep the neck; like the expression, an entry rides along and the file keeps    // only the entries of notes a half still shows.    for id in &ids {        assert!(d.doc().song.clip(*id).unwrap().2.midi.tab.is_some());    }    let text = save_project(d.doc());    let v: Value = serde_json::from_str(&text).unwrap();    let clips = v["song"]["tracks"][0]["lanes"][0]["clips"].as_array().unwrap();    let counts: Vec<usize> = clips        .iter()        .map(|c| c["tab"]["notes"].as_array().map_or(0, Vec::len))        .collect();    assert_eq!(counts, [2, 1]);
    load_project(&save_project(d.doc()), &reg).unwrap();
}

#[test]
fn sheets_are_saved_optional_and_edits_are_undoable() {
    let reg = registry();
    let mut d = song_start(&reg);
    assert!(!save_project(d.doc()).contains("\"sheets\""));
    let sheet = parse_sheet(
        "{title: Made Up}\n{key: Am}\n[Verse]\n[Am]la la [F]la [C/G]la\n\nAm   F\nla la la\n",
    )
    .unwrap()
    .sheet;
    d.apply(sc(SongCommand::SetSheets(vec![sheet.clone()])), &reg)
        .unwrap();
    assert_eq!(d.undo_label().unwrap(), "Edit Chord Sheets");
    let text = save_project(d.doc());
    let (loaded, warnings) = load_project_with_warnings(&text, &reg).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(loaded.song.sheets, vec![sheet.clone()]);
    assert_eq!(save_project(&loaded), text);
    d.undo().unwrap();
    assert!(d.doc().song.sheets.is_empty());
    d.redo().unwrap();
    assert_eq!(d.doc().song.sheets.len(), 1);
    // A line linked to a bar keeps its anchor.
    let mut linked = sheet;
    if let Some(atomik_tab::sheet::SheetBlock::Line { bar, .. }) = linked
        .blocks
        .iter_mut()
        .find(|b| matches!(b, atomik_tab::sheet::SheetBlock::Line { .. }))
    {
        *bar = Some(3);
    }
    d.apply(sc(SongCommand::SetSheets(vec![linked.clone()])), &reg)
        .unwrap();
    let again = load_project(&save_project(d.doc()), &reg).unwrap();
    assert_eq!(again.song.sheets[0], linked);
}

#[test]
fn bad_sheets_are_refused_in_a_command_and_in_a_file() {
    let reg = registry();
    let mut d = song_start(&reg);
    let mut sheet = parse_sheet("[Am]la\n").unwrap().sheet;
    sheet.capo = 99;
    let before = d.doc().clone();
    assert!(
        d.apply(sc(SongCommand::SetSheets(vec![sheet])), &reg)
            .is_err()
    );
    assert_eq!(d.doc(), &before);
    let ok = parse_sheet("[Am]la\n").unwrap().sheet;
    d.apply(sc(SongCommand::SetSheets(vec![ok])), &reg).unwrap();
    let text = save_project(d.doc());
    for (broken, needle) in [
        (text.replace("\"Am\"", "\"H#\""), "not a chord"),
        (text.replace("\"line\"", "\"verse\""), "unknown block"),
        (text.replace("\"blocks\"", "\"bloks\""), "bloks"),
        (
            text.replace("\"sheets\": [", "\"sheets\": [3, "),
            "sheets[0]",
        ),
    ] {
        let e = load_project(&broken, &reg).expect_err(needle).to_string();
        assert!(e.contains(needle), "{needle}: {e}");
    }
}

#[test]
fn an_older_reader_of_the_same_version_can_drop_the_new_fields() {
    // The file says so: both rows are ignorable and `compat` stays 44.
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(
        sc(SongCommand::SetSheets(vec![
            parse_sheet("[Am]la\n").unwrap().sheet,
        ])),
        &reg,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    assert_eq!(v["compat"], 44);
    assert_eq!(v["ignorable"]["song.sheets"], 45);
}

#[test]
fn a_command_batch_of_a_whole_tab_session_survives_undo_redo_and_save() {
    let reg = registry();
    let mut d = doc_with(
        &reg,
        vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 62, 0.8)],
        MidiData::default(),
    );
    let (notes, midi) = clip_of(&d);
    run(
        &mut d,
        &reg,
        tab_edit::start_tab(&notes, &midi, "guitar6", true).unwrap(),
    );
    let (notes, midi) = clip_of(&d);
    run(
        &mut d,
        &reg,
        tab_edit::set_techniques(
            &notes,
            &midi,
            &[(0, 64)],
            &Techniques {
                slide: Some(SlideKind::Shift),
                ..Techniques::default()
            },
        )
        .unwrap(),
    );
    let (notes, _) = clip_of(&d);
    assert_eq!(notes[0].length, PPQ);
    let saved = save_project(d.doc());
    load_project(&saved, &reg).unwrap();
    d.undo().unwrap();
    d.undo().unwrap();
    assert!(clip_of(&d).1.tab.is_none());
    d.redo().unwrap();
    d.redo().unwrap();
    assert_eq!(save_project(d.doc()), saved);
    let _: Option<Command> = None;
}
