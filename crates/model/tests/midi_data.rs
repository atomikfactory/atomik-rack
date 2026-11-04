//! Program changes and SysEx messages carried by note clips: adding a clip with them, what the
//! checks refuse, the project file (optional, ignorable fields, canonical round trip), the
//! flattening into song time for a MIDI file export, and what a join and a loop conversion do
//! with them.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_midi_data;
use atomik_model::{
    ClipId, Command, Document, LoopConvert, MidiData, ProgramChange, SongCommand, SysexEvent,
    WorkflowCommand, load_project, load_project_with_warnings, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

fn prog(tick: i64, program: u8) -> ProgramChange {
    ProgramChange::new(tick, program)
}

fn pairs(programs: &[ProgramChange]) -> Vec<(i64, u8)> {
    programs.iter().map(|p| (p.tick, p.program)).collect()
}

fn sysex(tick: i64, data: &[u8]) -> SysexEvent {
    SysexEvent {
        tick,
        data: data.to_vec(),
    }
}

fn data() -> MidiData {
    MidiData {
        programs: vec![prog(0, 5), prog(PPQ, 9)],
        sysex: vec![sysex(PPQ / 2, &[0x7E, 0x7F, 0x09, 0x01])],
    }
}

fn with_data(reg: &Registry, length: i64, midi: MidiData) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    let mut c = notes_clip(0, length, vec![note(0, PPQ, 60, 0.8)]);
    c.midi = midi.into();
    d.apply(add_clip("synth", 0, c), reg).unwrap();
    d
}

fn midi_of(d: &Document, id: u64) -> MidiData {
    (*d.doc().song.clip(ClipId(id)).expect("clip").2.midi).clone()
}

#[test]
fn a_clip_keeps_its_data_sorted_and_a_plain_clip_writes_nothing() {
    let reg = registry();
    let shuffled = MidiData {
        programs: vec![prog(PPQ, 9), prog(0, 5), prog(PPQ, 11)],
        sysex: vec![sysex(PPQ, &[2]), sysex(0, &[1])],
    };
    let d = with_data(&reg, 2 * BAR, shuffled);
    let m = midi_of(&d, 1);
    assert_eq!(m.programs, [prog(0, 5), prog(PPQ, 11)]);
    assert_eq!(m.sysex, [sysex(0, &[1]), sysex(PPQ, &[2])]);
    let plain = with_data(&reg, 2 * BAR, MidiData::default());
    let text = save_project(plain.doc());
    assert!(
        !text.contains("programs") && !text.contains("sysex"),
        "{text}"
    );
}

#[test]
fn bad_data_is_refused_without_a_change() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    let before = d.doc().clone();
    let mut try_add = |midi: MidiData| {
        let mut c = notes_clip(0, BAR, vec![]);
        c.midi = midi.into();
        d.apply(add_clip("synth", 0, c), &reg)
    };
    assert!(
        try_add(MidiData {
            sysex: vec![sysex(0, &[0x90])],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        try_add(MidiData {
            programs: vec![prog(-1, 1)],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        try_add(MidiData {
            programs: vec![prog(0, 200)],
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_project_file_round_trips_canonically_with_optional_fields() {
    let reg = registry();
    let d = with_data(&reg, 2 * BAR, data());
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    assert_eq!(clip["programs"], serde_json::json!([[0, 5], [PPQ, 9]]));
    assert_eq!(clip["sysex"], serde_json::json!([[PPQ / 2, "7E 7F 09 01"]]));
    let (loaded, warnings) = load_project_with_warnings(&text, &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn bad_data_in_a_file_names_its_json_path() {
    let reg = registry();
    let d = with_data(&reg, 2 * BAR, data());
    let text = save_project(d.doc());
    let broken = text.replace("7E 7F 09 01", "7E ZZ");
    let e = load_project(&broken, &reg).unwrap_err().to_string();
    assert!(e.contains("sysex[0][1]"), "{e}");
    let broken = text.replace("[0, 5]", "[0, 500]");
    let e = load_project(&broken, &reg).unwrap_err().to_string();
    assert!(e.contains("programs[0][1]"), "{e}");
}

#[test]
fn flattening_follows_offset_loop_and_hiding() {
    let reg = registry();
    let mut d = with_data(&reg, 4 * PPQ, data());
    let flat = flatten_midi_data(&d.doc().song.tracks[0]);
    assert_eq!(pairs(&flat.programs), [(0, 5), (PPQ, 9)]);
    assert_eq!(flat.sysex, [(PPQ / 2, vec![0x7E, 0x7F, 0x09, 0x01])]);
    // An offset past the first program: the program in force starts the clip.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 3 * PPQ,
            offset: PPQ / 2 + 10,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let flat = flatten_midi_data(&d.doc().song.tracks[0]);
    assert_eq!(pairs(&flat.programs), [(0, 5), (PPQ - PPQ / 2 - 10, 9)]);
    assert!(flat.sysex.is_empty(), "the message is before the window");
    // A loop repeats the window, with the program in force at each start.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 4 * PPQ,
            offset: 0,
            loop_length: Some(2 * PPQ),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let flat = flatten_midi_data(&d.doc().song.tracks[0]);
    assert_eq!(
        pairs(&flat.programs),
        [(0, 5), (PPQ, 9), (2 * PPQ, 5), (3 * PPQ, 9)]
    );
    assert_eq!(
        flat.sysex.iter().map(|s| s.0).collect::<Vec<_>>(),
        [PPQ / 2, 2 * PPQ + PPQ / 2]
    );
    // A later clip over the middle hides it.
    d.apply(
        add_clip("synth", 0, notes_clip(PPQ / 4, 3 * PPQ, vec![])),
        &reg,
    )
    .unwrap();
    let flat = flatten_midi_data(&d.doc().song.tracks[0]);
    // The visible tail of the looped clip starts with the program held there.
    assert_eq!(pairs(&flat.programs), [(0, 5), (3 * PPQ + PPQ / 4, 9)]);
    assert!(flat.sysex.is_empty());
}

#[test]
fn a_join_and_a_loop_conversion_carry_the_data() {
    let reg = registry();
    let mut d = with_data(&reg, 2 * PPQ, data());
    let mut second = notes_clip(2 * PPQ, 2 * PPQ, vec![note(0, PPQ, 62, 0.8)]);
    second.midi = MidiData {
        programs: vec![prog(PPQ / 4, 40)],
        sysex: vec![sysex(1, &[3])],
    }
    .into();
    d.apply(add_clip("synth", 0, second), &reg).unwrap();
    d.apply(
        Command::Workflow(WorkflowCommand::JoinClips {
            ids: vec![ClipId(1), ClipId(2)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    let m = midi_of(&d, ids[0].0);
    assert_eq!(
        m.programs,
        [prog(0, 5), prog(PPQ, 9), prog(2 * PPQ + PPQ / 4, 40)]
    );
    assert_eq!(m.sysex.len(), 2);
    assert_eq!(m.sysex[1].tick, 2 * PPQ + 1);

    // A loop unrolls: the second repetition repeats the window.
    let mut d = with_data(&reg, 4 * PPQ, data());
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 4 * PPQ,
            offset: 0,
            loop_length: Some(2 * PPQ),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let before = flatten_midi_data(&d.doc().song.tracks[0]);
    d.apply(
        Command::Workflow(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Single,
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 1);
    assert_eq!(
        flatten_midi_data(&d.doc().song.tracks[0]),
        before,
        "the converted clip plays what the loop played"
    );
}

#[test]
fn a_bank_select_travels_with_its_program_change_and_saves_as_four_values() {
    let reg = registry();
    let banked = ProgramChange {
        bank_msb: Some(3),
        bank_lsb: Some(0),
        ..prog(PPQ, 9)
    };
    let half = ProgramChange {
        bank_lsb: Some(5),
        ..prog(2 * PPQ, 4)
    };
    let midi = MidiData {
        programs: vec![prog(0, 5), banked, half],
        sysex: Vec::new(),
    };
    let d = with_data(&reg, 4 * BAR, midi);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    assert_eq!(
        clip["programs"],
        serde_json::json!([[0, 5], [PPQ, 9, 3, 0], [2 * PPQ, 4, null, 5]])
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    // The flattened data keeps the bank, and a cut window carries the bank in force.
    let flat = flatten_midi_data(&d.doc().song.tracks[0]);
    assert_eq!(flat.programs[1], banked);
    let m = midi_of(&d, 1);
    assert_eq!(m.window(PPQ + 10, 3 * PPQ, 0).programs[0].bank_msb, Some(3));
    let bad = text.replace("[15360, 9, 3, 0]", "[15360, 9, 300, 0]");
    let e = load_project(&bad, &reg).unwrap_err().to_string();
    assert!(e.contains("programs[1][2]"), "{e}");
}

#[test]
fn set_clip_midi_replaces_the_data_in_one_undo_step() {
    let reg = registry();
    let mut d = with_data(&reg, 2 * BAR, data());
    let before = d.doc().clone();
    let next = MidiData {
        programs: vec![
            ProgramChange {
                bank_msb: Some(1),
                ..prog(PPQ, 7)
            },
            prog(0, 2),
        ],
        sysex: vec![sysex(5, &[1, 2, 3])],
    };
    let set = |midi: MidiData, merge| {
        sc(SongCommand::SetClipMidi {
            clip: ClipId(1),
            midi,
            merge,
        })
    };
    d.apply(set(next, None), &reg).unwrap();
    let m = midi_of(&d, 1);
    assert_eq!(m.programs[0], prog(0, 2), "the data is put in order");
    assert_eq!(m.programs[1].bank_msb, Some(1));
    assert_eq!(m.sysex, [sysex(5, &[1, 2, 3])]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    // Clearing is one step too.
    d.apply(set(MidiData::default(), None), &reg).unwrap();
    assert!(midi_of(&d, 1).is_empty());
    d.undo().unwrap();
    assert_eq!(midi_of(&d, 1).programs.len(), 2);
    // A drag shares one merge id: one undo step.
    let n = d.undo_depth();
    for t in [10, 20, 30] {
        d.apply(
            set(
                MidiData {
                    programs: vec![prog(t, 1)],
                    sysex: Vec::new(),
                },
                Some(77),
            ),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.undo_depth(), n + 1);
    // Refused: bad data, an unknown clip, audio.
    let bad = MidiData {
        programs: vec![prog(0, 200)],
        sysex: Vec::new(),
    };
    assert!(d.apply(set(bad, None), &reg).is_err());
    let unknown = sc(SongCommand::SetClipMidi {
        clip: ClipId(99),
        midi: MidiData::default(),
        merge: None,
    });
    assert!(d.apply(unknown, &reg).is_err());
}
