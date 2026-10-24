//! `WorkflowCommand::ImportMidi`: tracks, clips, tempo, meter and markers from a MIDI file in one
//! undo step, and the rejections that leave the document alone.

mod common;

use atomik_core::{MeterEvent, PPQ};
use atomik_device_sdk::Registry;
use atomik_model::song::TempoEvent;
use atomik_model::{
    ClipContent, Command, ControllerKind, ControllerLane, Document, ImportedTrack, MidiImport,
    ModelError, Note, SyncOp, WorkflowCommand,
};
use common::song::*;
use common::*;

const BAR: i64 = PPQ * 4;

fn track(name: &str, start: i64, length: i64, notes: Vec<Note>) -> ImportedTrack {
    ImportedTrack {
        name: name.to_owned(),
        clip: notes_clip(start, length, notes),
    }
}

fn import(tracks: Vec<ImportedTrack>) -> MidiImport {
    MidiImport {
        instrument: SYNTH.to_owned(),
        tracks,
        ..MidiImport::default()
    }
}

fn run_import(d: &mut Document, reg: &Registry, i: MidiImport) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(Command::Workflow(WorkflowCommand::ImportMidi(i)), reg)
}

#[test]
fn the_tracks_come_with_instruments_unarmed_and_named_in_one_undo_step() {
    let reg = registry();
    let mut d = song_start(&reg);
    let before = d.doc().clone();
    let controllers = vec![ControllerLane::new(
        ControllerKind::Cc(1),
        vec![point(0, 0.5, atomik_model::song::Curve::Step)],
    )];
    let mut second = track("Bass", BAR, 2 * BAR, vec![note(0, PPQ, 36, 1.0)]);
    second.clip.controllers = controllers.clone();
    let ops = run_import(
        &mut d,
        &reg,
        import(vec![
            track(
                "Lead",
                0,
                BAR,
                vec![note(0, PPQ, 60, 0.5), note(PPQ, PPQ, 62, 0.5)],
            ),
            second,
            track("  ", 0, BAR, vec![note(0, PPQ, 64, 0.5)]),
        ]),
    )
    .unwrap();
    assert!(ops.contains(&SyncOp::SongChanged));
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), before.song.tracks.len() + 3);
    let added = &doc.song.tracks[before.song.tracks.len()..];
    assert!(added.iter().all(|t| !t.armed && t.lanes.len() == 1));
    let names: Vec<_> = added
        .iter()
        .map(|t| doc.device(&t.device).unwrap().name.clone())
        .collect();
    assert_eq!(&names[..2], ["Lead", "Bass"]);
    assert_ne!(
        names[2].trim(),
        "",
        "a blank name keeps the instrument's own"
    );
    let clip = &added[1].lanes[0].clips[0];
    assert_eq!((clip.start, clip.length), (BAR, 2 * BAR));
    assert_eq!(clip.controllers, controllers);
    let ClipContent::Notes(n) = &added[0].lanes[0].clips[0].content else {
        panic!("notes")
    };
    assert_eq!(n.len(), 2);
    // The new clips got distinct ids.
    let mut ids: Vec<_> = added.iter().map(|t| t.lanes[0].clips[0].id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 3);

    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one undo removes everything");
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks.len(), before.song.tracks.len() + 3);
}

#[test]
fn tempo_meter_and_markers_are_part_of_the_same_step() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(
        sc(atomik_model::SongCommand::SetMarkers(vec![
            atomik_model::song::Marker {
                id: 7,
                tick: 5,
                name: "Old".into(),
                color: None,
            },
        ])),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    let mut i = import(vec![track("A", 0, BAR, vec![note(0, PPQ, 60, 0.5)])]);
    i.tempo = Some(vec![
        TempoEvent::step(0, 90.0),
        TempoEvent::step(BAR, 140.0),
    ]);
    i.meter = Some(vec![MeterEvent::new(1, 3, 4), MeterEvent::new(3, 4, 4)]);
    i.markers = vec![(BAR, "Chorus".into()), (0, "Intro".into())];
    let ops = run_import(&mut d, &reg, i).unwrap();
    assert!(ops.contains(&SyncOp::TempoChanged(90.0)));
    let song = &d.doc().song;
    assert_eq!(song.tempo.len(), 2);
    assert!((d.doc().tempo_bpm - 90.0).abs() < 1e-9);
    assert_eq!(
        song.meter,
        vec![MeterEvent::new(1, 3, 4), MeterEvent::new(3, 4, 4)]
    );
    let markers: Vec<_> = song
        .markers
        .iter()
        .map(|m| (m.tick, m.name.as_str()))
        .collect();
    assert_eq!(markers, [(0, "Intro"), (5, "Old"), (BAR, "Chorus")]);
    let ids: std::collections::BTreeSet<_> = song.markers.iter().map(|m| m.id).collect();
    assert_eq!(ids.len(), 3, "the new markers get fresh ids");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_file_with_only_a_tempo_map_imports_without_tracks() {
    let reg = registry();
    let mut d = song_start(&reg);
    let mut i = import(Vec::new());
    i.tempo = Some(vec![TempoEvent::step(0, 100.0)]);
    run_import(&mut d, &reg, i).unwrap();
    assert!((d.doc().tempo_bpm - 100.0).abs() < 1e-9);
}

#[test]
fn the_song_end_grows_to_the_next_bar_line_and_never_shrinks() {
    let reg = registry();
    let mut d = song_start(&reg);
    let end = d.doc().song.end;
    run_import(
        &mut d,
        &reg,
        import(vec![track(
            "Long",
            0,
            end + PPQ,
            vec![note(0, PPQ, 60, 0.5)],
        )]),
    )
    .unwrap();
    let grown = d.doc().song.end;
    assert!(grown >= end + PPQ);
    assert_eq!(grown % BAR, 0);
    assert!(grown < end + PPQ + BAR);
    run_import(
        &mut d,
        &reg,
        import(vec![track("Short", 0, BAR, vec![note(0, 1, 60, 0.5)])]),
    )
    .unwrap();
    assert_eq!(d.doc().song.end, grown);
}

#[test]
fn rejected_imports_change_nothing() {
    let reg = registry();
    let mut d = song_start(&reg);
    let before = d.doc().clone();
    let bad: Vec<(MidiImport, &str)> = vec![
        (import(Vec::new()), "empty"),
        (
            MidiImport {
                instrument: "atomik.stereo-delay".into(),
                ..import(vec![track("A", 0, BAR, vec![note(0, 1, 60, 0.5)])])
            },
            "not an instrument",
        ),
        (
            MidiImport {
                instrument: "no.such.device".into(),
                ..import(vec![track("A", 0, BAR, vec![note(0, 1, 60, 0.5)])])
            },
            "unknown type",
        ),
        (
            // The second track's clip is invalid: the first must not stay behind.
            import(vec![
                track("A", 0, BAR, vec![note(0, 1, 60, 0.5)]),
                track("B", 0, BAR, vec![note(0, 0, 60, 0.5)]),
            ]),
            "zero-length note",
        ),
        (
            MidiImport {
                tempo: Some(vec![TempoEvent::step(5, 120.0)]),
                ..import(vec![track("A", 0, BAR, vec![note(0, 1, 60, 0.5)])])
            },
            "tempo map not at 0",
        ),
        (
            MidiImport {
                tempo: Some(vec![TempoEvent::step(0, 5000.0)]),
                ..import(Vec::new())
            },
            "tempo out of range",
        ),
        (
            MidiImport {
                meter: Some(vec![MeterEvent::new(2, 4, 4)]),
                ..import(Vec::new())
            },
            "meter map not at bar 1",
        ),
        (
            MidiImport {
                markers: vec![(0, String::new())],
                ..import(Vec::new())
            },
            "blank marker",
        ),
    ];
    for (i, why) in bad {
        assert!(run_import(&mut d, &reg, i).is_err(), "{why}");
        assert_eq!(d.doc(), &before, "{why}: the document is untouched");
    }
}
