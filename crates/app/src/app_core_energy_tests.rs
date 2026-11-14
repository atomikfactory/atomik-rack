//! The energy map in the app core: the measurement runs on a worker over the master of the song and
//! gives one value per section, nothing in the song changes, the result goes stale when the song
//! changes, the lane gets what it draws, and a song without a master or a project change is
//! handled.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::energy_map::{EnergyMap, Flag, Measure, Role, Section, SectionEnergy};
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Marker, Note, WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn sustained(start: i64, length: i64, keys: &[u8]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            keys.iter()
                .map(|&k| Note::new(0, length, k, 0.9))
                .collect::<Vec<_>>()
                .into(),
        ),
    }
}

/// A quiet pad for four bars, a lead on top of it from bar 3, and markers `Verse` at bar 1 and
/// `Drop` at bar 3.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let tracks: [(&str, f32, i64, &[u8]); 2] = [
        ("pad", -24.0, 0, &[48, 55]),
        ("lead", -6.0, 2 * BAR, &[72, 79, 84]),
    ];
    for (k, level, start, keys) in tracks {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 0.0),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
        ] {
            c.apply_command(Command::SetParam {
                key: key(k),
                param: p.into(),
                value: v,
                merge: None,
            })
            .unwrap();
        }
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: sustained(start, 4 * BAR - start, keys),
        }))
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::SetSongEnd(4 * BAR)))
        .unwrap();
    let marker = |id, bar: i64, name: &str| Marker {
        id,
        tick: bar * BAR,
        name: name.to_owned(),
        color: None,
    };
    c.apply_command(Command::Song(SongCommand::SetMarkers(vec![
        marker(1, 0, "Verse"),
        marker(2, 2, "Drop"),
    ])))
    .unwrap();
    c
}

#[test]
fn the_measurement_gives_one_value_per_section_and_changes_nothing() {
    let mut c = core();
    let before = c.doc().clone();
    assert!(c.energy().is_none());
    c.start_energy().expect("starts");
    assert!(c.energy_running());
    assert!(c.start_energy().is_err(), "one at a time");
    c.wait_background();
    assert!(!c.energy_running());
    let (map, stale) = c.energy().expect("a result");
    assert!(!stale);
    let names: Vec<_> = map
        .sections
        .iter()
        .map(|s| s.section.name.as_str())
        .collect();
    assert_eq!(names, ["Verse", "Drop"]);
    assert_eq!(map.sections[1].section.role, Role::Drop);
    // More plays in the second section, so it has more energy, and the lead counts as a track.
    assert!(map.sections[1].energy > map.sections[0].energy, "{map:?}");
    assert_eq!(
        (
            map.sections[0].measure.tracks,
            map.sections[1].measure.tracks
        ),
        (1, 2)
    );
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Energy map:")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(*c.doc(), before, "a measurement never changes the song");
}

#[test]
fn the_result_goes_stale_when_the_song_changes() {
    let mut c = core();
    c.start_energy().unwrap();
    c.wait_background();
    assert!(!c.energy().unwrap().1);
    c.apply_command(Command::Song(SongCommand::SetSongEnd(5 * BAR)))
        .unwrap();
    assert!(
        c.energy().unwrap().1,
        "the song changed after it was measured"
    );
    // Measuring again makes it current.
    c.start_energy().unwrap();
    c.wait_background();
    assert!(!c.energy().unwrap().1);
}

#[test]
fn an_edit_during_the_measurement_leaves_the_result_stale() {
    let mut c = core();
    c.start_energy().unwrap();
    c.apply_command(Command::Song(SongCommand::SetSongEnd(5 * BAR)))
        .unwrap();
    c.wait_background();
    assert!(
        c.energy().unwrap().1,
        "it measured the song as it was when it started"
    );
}

#[test]
fn the_lane_gets_the_map_the_progress_and_the_note() {
    let mut c = core();
    c.arrangement_mut().set_energy_lane(true);
    c.frame();
    assert!(c.arrangement().energy_map().is_none());
    c.start_energy().unwrap();
    c.wait_background();
    c.frame();
    let (map, stale) = c.arrangement().energy_map().expect("fed");
    assert_eq!((map.sections.len(), stale), (2, false));
    c.apply_command(Command::Song(SongCommand::SetSongEnd(5 * BAR)))
        .unwrap();
    c.frame();
    assert!(c.arrangement().energy_map().unwrap().1);
}

#[test]
fn a_song_without_a_master_is_refused_with_a_reason() {
    let mut doc = new_project_doc();
    doc.console.master = None;
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    let why = c.start_energy().unwrap_err();
    assert!(why.contains("master"), "{why}");
    assert_eq!(c.last_rejection(), Some(why.as_str()));
    assert!(!c.energy_running());
}

#[test]
fn a_new_project_drops_the_result() {
    let mut c = core();
    c.start_energy().unwrap();
    c.wait_background();
    assert!(c.energy().is_some());
    c.new_document();
    assert!(c.energy().is_none());
}

#[test]
fn the_flags_are_listed_as_sentences() {
    let mut c = core();
    assert!(c.energy_flags().is_empty());
    let m = |level: f32| Measure {
        loudness_db: level,
        brightness_hz: 1000.0,
        width: 0.4,
        tracks: 2,
    };
    let section = |name: &str, role, from: i64, to: i64, energy| SectionEnergy {
        section: Section {
            name: name.into(),
            role,
            start: from * BAR,
            end: to * BAR,
        },
        measure: m(-20.0),
        energy,
    };
    c.set_energy_for_test(EnergyMap {
        sections: vec![
            section("Build", Role::Build, 0, 2, 0.8),
            section("Drop", Role::Drop, 2, 4, 0.5),
        ],
        flags: vec![Flag::BuildLouderThanDrop { build: 0, drop: 1 }],
    });
    let flags = c.energy_flags();
    assert_eq!(flags.len(), 1);
    assert!(
        flags[0].contains("Build") && flags[0].contains("Drop"),
        "{flags:?}"
    );
}
