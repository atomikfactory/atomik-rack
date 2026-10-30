//! MIDI mappings (model half): the commands and their undo, what they refuse, mappings that
//! outlive their target, the preset by role (applied to a second project), and project format 28
//! (round trip, a stable text, version 27 files loading unchanged, refusals with their JSON path).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, MAX_MIDI_MAPS, MapMode, MapOptions, MidiMapCommand,
    MidiMapping, MidiSource, MidiTarget, ModelError, PROJECT_VERSION, PresetTarget, RelativeKind,
    SourceKind, SyncOp, TransportTarget, load_midi_preset, load_project, new_project_doc,
    save_midi_preset, save_project,
};
use common::*;
use serde_json::Value;

/// The starter with three synths (channels `channel`, `channel-2`, `channel-3`) and a VCA.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    run(
        &mut d,
        &r,
        ["a", "b", "c"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel")],
        })],
    );
    (d, r)
}

fn param(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

fn cc(channel: u8, n: u8) -> MidiSource {
    MidiSource::any_port(Some(channel), SourceKind::Cc(n))
}

fn mapping(source: MidiSource, target: MidiTarget) -> MidiMapping {
    let options = MapOptions::for_source(source.kind);
    MidiMapping {
        id: 0,
        source,
        target,
        options,
    }
}

fn run_map(d: &mut Document, r: &Registry, c: MidiMapCommand) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(Command::MidiMap(c), r)
}

fn add_map(d: &mut Document, r: &Registry, m: MidiMapping) {
    run_map(d, r, MidiMapCommand::Add(m)).unwrap();
}

fn ids(d: &Document) -> Vec<u32> {
    d.doc().midi_maps.iter().map(|m| m.id).collect()
}

fn mapped() -> (Document, Registry) {
    let (mut d, r) = project();
    add_map(&mut d, &r, mapping(cc(0, 7), param("channel", "fader")));
    add_map(&mut d, &r, mapping(cc(0, 10), param("channel", "pan")));
    add_map(
        &mut d,
        &r,
        MidiMapping {
            options: MapOptions {
                min: 0.25,
                max: 0.75,
                invert: true,
                pickup: false,
                consume: false,
                mode: MapMode::Relative(RelativeKind::BinaryOffset),
                sensitivity: 0.02,
            },
            ..mapping(cc(1, 20), param("a", "filter.cutoff"))
        },
    );
    add_map(
        &mut d,
        &r,
        mapping(
            MidiSource {
                port: Some("Pad Controller".into()),
                channel: Some(9),
                kind: SourceKind::Note(36),
            },
            MidiTarget::Transport(TransportTarget::Play),
        ),
    );
    add_map(
        &mut d,
        &r,
        mapping(
            MidiSource::any_port(None, SourceKind::Cc14(1)),
            MidiTarget::Tempo,
        ),
    );
    add_map(
        &mut d,
        &r,
        mapping(
            MidiSource::any_port(Some(2), SourceKind::PitchBend),
            MidiTarget::Vca { id: 1 },
        ),
    );
    add_map(
        &mut d,
        &r,
        mapping(
            cc(3, 74),
            MidiTarget::SelectedStrip {
                param: "fader".into(),
            },
        ),
    );
    (d, r)
}

// ---- Commands ----------------------------------------------------------------------------------

#[test]
fn a_mapping_is_added_with_the_next_id_in_one_undo_step() {
    let (mut d, r) = project();
    assert!(d.doc().midi_maps.is_empty());
    let before = d.doc().clone();
    let ops = run_map(
        &mut d,
        &r,
        MidiMapCommand::Add(mapping(cc(0, 7), param("channel", "fader"))),
    )
    .unwrap();
    assert_eq!(ops, vec![SyncOp::MidiMapsChanged]);
    assert_eq!(ids(&d), [1]);
    add_map(&mut d, &r, mapping(cc(0, 8), param("channel", "pan")));
    assert_eq!(ids(&d), [1, 2]);
    // Ids are max + 1: a removed middle id is not reused while a higher one lives.
    run_map(&mut d, &r, MidiMapCommand::Remove { id: 1 }).unwrap();
    add_map(&mut d, &r, mapping(cc(0, 9), param("channel", "width")));
    assert_eq!(ids(&d), [2, 3]);
    for _ in 0..4 {
        assert!(d.undo(&r).is_some());
    }
    assert_eq!(d.doc(), &before, "four commands, four steps");
    assert!(d.redo(&r).is_some());
    assert_eq!(ids(&d), [1]);
}

#[test]
fn learning_adds_a_mapping_with_the_options_of_its_source() {
    let (mut d, r) = project();
    let target = param("channel", "fader");
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Learn {
            source: cc(2, 7),
            target: target.clone(),
        },
    )
    .unwrap();
    let m = &d.doc().midi_maps[0];
    assert_eq!(m.source, cc(2, 7));
    assert_eq!(m.options, MapOptions::default());
    assert!(m.options.pickup && m.options.consume);
    // A note is a button: it toggles.
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Learn {
            source: MidiSource::any_port(Some(0), SourceKind::Note(40)),
            target: param("channel", "mute"),
        },
    )
    .unwrap();
    assert_eq!(d.doc().midi_maps[1].options.mode, MapMode::Toggle);
    // Learning a target again moves it to the new control and keeps the options.
    let first = d.doc().midi_maps[0].clone();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Set(MidiMapping {
            options: MapOptions {
                min: 0.1,
                ..MapOptions::default()
            },
            ..first
        }),
    )
    .unwrap();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Learn {
            source: cc(5, 11),
            target,
        },
    )
    .unwrap();
    assert_eq!(
        d.doc().midi_maps.len(),
        2,
        "no second mapping for the target"
    );
    assert_eq!(d.doc().midi_maps[0].source, cc(5, 11));
    assert!((d.doc().midi_maps[0].options.min - 0.1).abs() < 1e-6);
    // Learning a button over an encoder starts the options over (an encoder needs a controller).
    let first = d.doc().midi_maps[0].clone();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Set(MidiMapping {
            options: MapOptions {
                mode: MapMode::Relative(RelativeKind::TwosComplement),
                ..MapOptions::default()
            },
            ..first
        }),
    )
    .unwrap();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Learn {
            source: MidiSource::any_port(None, SourceKind::Note(1)),
            target: param("channel", "fader"),
        },
    )
    .unwrap();
    assert_eq!(d.doc().midi_maps[0].options.mode, MapMode::Toggle);
}

#[test]
fn set_edits_one_mapping_and_forget_removes_every_mapping_of_a_target() {
    let (mut d, r) = mapped();
    let mut m = d.doc().midi_map(1).unwrap().clone();
    m.options.invert = true;
    m.options.min = 0.2;
    run_map(&mut d, &r, MidiMapCommand::Set(m.clone())).unwrap();
    assert_eq!(d.doc().midi_map(1), Some(&m));
    // Two mappings on one target (a second control through the file or the panel).
    add_map(&mut d, &r, mapping(cc(4, 1), param("channel", "fader")));
    assert_eq!(d.doc().midi_maps_for(&param("channel", "fader")).count(), 2);
    let before = d.doc().midi_maps.len();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::Forget {
            target: param("channel", "fader"),
        },
    )
    .unwrap();
    assert_eq!(d.doc().midi_maps.len(), before - 2);
    assert!(d.undo(&r).is_some());
    assert_eq!(d.doc().midi_maps.len(), before);
    // A mapping that does not exist cannot be edited or removed.
    assert!(run_map(&mut d, &r, MidiMapCommand::Remove { id: 99 }).is_err());
    let mut ghost = m;
    ghost.id = 99;
    assert!(run_map(&mut d, &r, MidiMapCommand::Set(ghost)).is_err());
    assert!(
        run_map(
            &mut d,
            &r,
            MidiMapCommand::Forget {
                target: param("c", "volume")
            }
        )
        .is_err()
    );
}

#[test]
fn add_many_and_clear_are_one_step_each() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    run_map(
        &mut d,
        &r,
        MidiMapCommand::AddMany(vec![
            mapping(cc(0, 1), param("a", "filter.cutoff")),
            mapping(cc(0, 2), param("b", "filter.cutoff")),
            mapping(cc(0, 3), param("c", "filter.cutoff")),
        ]),
    )
    .unwrap();
    assert_eq!(ids(&d), [1, 2, 3]);
    run_map(&mut d, &r, MidiMapCommand::Clear).unwrap();
    assert!(d.doc().midi_maps.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(ids(&d), [1, 2, 3]);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    assert!(run_map(&mut d, &r, MidiMapCommand::AddMany(vec![])).is_err());
    assert!(run_map(&mut d, &r, MidiMapCommand::Clear).is_err());
    // One bad mapping refuses the lot.
    let bad = MidiMapCommand::AddMany(vec![
        mapping(cc(0, 1), param("a", "filter.cutoff")),
        mapping(cc(0, 2), param("a", "no.such.param")),
    ]);
    assert!(run_map(&mut d, &r, bad).is_err());
    assert!(d.doc().midi_maps.is_empty());
}

#[test]
fn bad_mappings_are_refused_with_a_reason() {
    let (mut d, r) = project();
    let refused = |d: &mut Document, m: MidiMapping| -> String {
        run_map(d, &r, MidiMapCommand::Add(m))
            .unwrap_err()
            .to_string()
    };
    let e = refused(&mut d, mapping(cc(0, 7), param("nope", "fader")));
    assert!(e.contains("no device `nope`"), "{e}");
    let e = refused(&mut d, mapping(cc(0, 7), param("channel", "no.such")));
    assert!(e.contains("no param"), "{e}");
    let e = refused(
        &mut d,
        mapping(cc(0, 7), param("channel", "internal.solo_silence")),
    );
    assert!(e.contains("not a parameter you can map"), "{e}");
    let e = refused(&mut d, mapping(cc(0, 7), MidiTarget::Vca { id: 9 }));
    assert!(e.contains("no VCA group 9"), "{e}");
    let e = refused(
        &mut d,
        mapping(
            cc(0, 7),
            MidiTarget::SelectedStrip {
                param: "nope".into(),
            },
        ),
    );
    assert!(e.contains("channel strip has no param"), "{e}");
    let e = refused(&mut d, mapping(cc(16, 7), param("channel", "fader")));
    assert!(e.contains("channel 17"), "{e}");
    let e = refused(&mut d, mapping(cc(0, 128), param("channel", "fader")));
    assert!(e.contains("controller 128"), "{e}");
    let e = refused(
        &mut d,
        mapping(
            MidiSource::any_port(None, SourceKind::Cc14(32)),
            param("channel", "fader"),
        ),
    );
    assert!(e.contains("14 bit"), "{e}");
    let e = refused(
        &mut d,
        MidiMapping {
            options: MapOptions {
                mode: MapMode::Relative(RelativeKind::SignMagnitude),
                ..MapOptions::default()
            },
            ..mapping(
                MidiSource::any_port(None, SourceKind::PitchBend),
                param("channel", "fader"),
            )
        },
    );
    assert!(e.contains("relative encoder"), "{e}");
    let e = refused(
        &mut d,
        MidiMapping {
            options: MapOptions {
                max: 1.5,
                ..MapOptions::default()
            },
            ..mapping(cc(0, 7), param("channel", "fader"))
        },
    );
    assert!(e.contains("between 0 and 1"), "{e}");
    assert!(d.doc().midi_maps.is_empty(), "a refusal changes nothing");
}

#[test]
fn a_command_that_changes_nothing_is_refused_and_the_limit_holds() {
    let (mut d, r) = project();
    add_map(&mut d, &r, mapping(cc(0, 7), param("channel", "fader")));
    let same = d.doc().midi_maps[0].clone();
    assert!(run_map(&mut d, &r, MidiMapCommand::Set(same)).is_err());
    // The cap is checked with the whole list.
    let many: Vec<MidiMapping> = (0..MAX_MIDI_MAPS)
        .map(|i| {
            mapping(
                cc((i % 16) as u8, (i / 16) as u8),
                param("a", "filter.cutoff"),
            )
        })
        .collect();
    let e = run_map(&mut d, &r, MidiMapCommand::AddMany(many))
        .unwrap_err()
        .to_string();
    assert!(e.contains("at most 512"), "{e}");
    assert_eq!(d.doc().midi_maps.len(), 1);
}

#[test]
fn a_mapping_outlives_its_target_and_can_still_be_edited() {
    let (mut d, r) = mapped();
    run(&mut d, &r, [Command::RemoveDevice { key: key("a") }]);
    let m = d
        .doc()
        .midi_maps
        .iter()
        .find(|m| m.target == param("a", "filter.cutoff"))
        .expect("the mapping stays")
        .clone();
    // The target is gone, but the options of the mapping can still be changed.
    let mut edited = m.clone();
    edited.options.max = 0.9;
    run_map(&mut d, &r, MidiMapCommand::Set(edited)).unwrap();
    // Pointing it at a missing device is refused.
    let mut moved = m;
    moved.target = param("a", "filter.resonance");
    assert!(run_map(&mut d, &r, MidiMapCommand::Set(moved)).is_err());
    // And the removal is one step that undo brings back with the device.
    assert!(d.undo(&r).is_some());
    assert!(d.undo(&r).is_some());
    assert!(d.doc().device(&key("a")).is_some());
}

// ---- The preset, by role --------------------------------------------------------------------

#[test]
fn a_preset_addresses_devices_by_type_and_position_and_drops_the_port() {
    let (d, _r) = mapped();
    let (preset, notes) = d.doc().midi_preset("Desk", &[]);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(preset.name, "Desk");
    assert_eq!(preset.maps.len(), 7);
    // `channel` is the first atomik.channel, `a` the first poly synth.
    assert_eq!(
        preset.maps[0].target,
        PresetTarget::Device {
            type_id: CHANNEL.into(),
            nth: 0,
            param: "fader".into()
        }
    );
    assert_eq!(
        preset.maps[2].target,
        PresetTarget::Device {
            type_id: SYNTH.into(),
            nth: 0,
            param: "filter.cutoff".into()
        }
    );
    assert_eq!(
        preset.maps[3].target,
        PresetTarget::Transport(TransportTarget::Play)
    );
    assert!(
        preset.maps.iter().all(|m| m.source.port.is_none()),
        "a preset has no port"
    );
    assert_eq!(preset.maps[3].source.channel, Some(9));
    // Only the chosen mappings are exported.
    let (some, _) = d.doc().midi_preset("Two", &[2, 5]);
    assert_eq!(some.maps.len(), 2);
}

#[test]
fn a_preset_applies_to_a_second_project_through_its_own_devices() {
    let (d, r) = mapped();
    let (preset, _) = d.doc().midi_preset("Desk", &[]);
    // A different project: the poly synths come in another order and under other keys, there is
    // no VCA, and only two channels.
    let mut other = Document::new(new_project_doc());
    run(
        &mut other,
        &r,
        ["x", "y"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    let (maps, notes) = other.doc().resolve_preset(&r, &preset);
    // The VCA has no group 1 here, and nothing else is missing.
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("no VCA group 1"), "{notes:?}");
    assert_eq!(maps.len(), 6);
    let cutoff = maps
        .iter()
        .find(|m| matches!(&m.target, MidiTarget::Param { param, .. } if param == "filter.cutoff"))
        .unwrap();
    assert_eq!(
        cutoff.target,
        param("x", "filter.cutoff"),
        "the first poly synth of the other project"
    );
    let fader = maps
        .iter()
        .find(|m| matches!(&m.target, MidiTarget::Param { param, .. } if param == "fader"))
        .unwrap();
    assert_eq!(
        fader.target,
        param("channel", "fader"),
        "the first channel strip"
    );
    assert_eq!(cutoff.options, d.doc().midi_map(3).unwrap().options);
    // Applied as one undo step.
    let mut o = other;
    run_map(&mut o, &r, MidiMapCommand::AddMany(maps)).unwrap();
    assert_eq!(o.doc().midi_maps.len(), 6);
    assert_eq!(ids(&o), [1, 2, 3, 4, 5, 6]);
    o.undo(&r).unwrap();
    assert!(o.doc().midi_maps.is_empty());
}

#[test]
fn a_preset_that_names_a_missing_instance_says_so() {
    let (d, r) = mapped();
    // The third poly synth does not exist in a project of one.
    let (mut preset, _) = d.doc().midi_preset("Desk", &[3]);
    preset.maps[0].target = PresetTarget::Device {
        type_id: SYNTH.into(),
        nth: 2,
        param: "filter.cutoff".into(),
    };
    let mut one = Document::new(new_project_doc());
    run(&mut one, &r, [add(usize::MAX, SYNTH, Some("only"))]);
    let (maps, notes) = one.doc().resolve_preset(&r, &preset);
    assert!(maps.is_empty());
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("number 3"), "{notes:?}");
}

#[test]
fn a_device_inside_a_container_cannot_be_exported_by_role() {
    let r = console_registry();
    let mut d = container_doc(&r);
    add_map(
        &mut d,
        &r,
        mapping(cc(0, 1), param("c/synth", "filter.cutoff")),
    );
    let (preset, notes) = d.doc().midi_preset("x", &[]);
    assert!(preset.maps.is_empty());
    assert_eq!(notes.len(), 1, "{notes:?}");
}

#[test]
fn a_preset_file_round_trips_in_a_stable_text() {
    let (d, _r) = mapped();
    let (preset, _) = d.doc().midi_preset("Desk \"1\"", &[]);
    let text = save_midi_preset(&preset);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["format"], "atomik-midi-preset");
    assert_eq!(v["version"], 1);
    assert_eq!(v["maps"][0]["target"]["type"], CHANNEL);
    assert_eq!(v["maps"][0]["target"]["nth"], 0);
    assert!(v["maps"][0]["source"].get("port").is_none());
    let loaded = load_midi_preset(&text).unwrap();
    assert_eq!(loaded, preset);
    assert_eq!(save_midi_preset(&loaded), text);
}

#[test]
fn bad_preset_files_are_refused_with_their_json_path() {
    let (d, _r) = mapped();
    let (preset, _) = d.doc().midi_preset("Desk", &[]);
    let text = save_midi_preset(&preset);
    let err = |t: String| load_midi_preset(&t).unwrap_err().to_string();
    assert!(err("not json".into()).contains("invalid JSON"));
    assert!(err(text.replace("atomik-midi-preset", "other")).contains("format"));
    assert!(
        err(text.replace("\"version\": 1", "\"version\": 2")).contains("unsupported version 2")
    );
    assert!(err(text.replace("\"device\"", "\"param\"")).contains("maps[0].target.kind"));
    let with_port = text.replacen("\"kind\": \"cc\"", "\"port\": \"X\", \"kind\": \"cc\"", 1);
    assert!(err(with_port).contains("a preset has no input port"));
    assert!(err(text.replacen("\"nth\": 0", "\"nth\": -1", 1)).contains("maps[0].target.nth"));
}

// ---- The file ---------------------------------------------------------------------------------

#[test]
fn mappings_round_trip_in_format_28_in_a_stable_text() {
    let (d, r) = mapped();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 35);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 35);
    let maps = &v["midi_maps"];
    assert_eq!(maps.as_array().unwrap().len(), 7);
    assert_eq!(maps[0]["id"], 1);
    assert_eq!(maps[0]["source"]["kind"], "cc");
    assert_eq!(maps[0]["source"]["number"], 7);
    assert_eq!(maps[0]["source"]["channel"], 0);
    assert_eq!(maps[0]["target"]["kind"], "param");
    assert_eq!(maps[0]["target"]["device"], "channel");
    // Defaults are not written.
    assert!(maps[0].get("min").is_none());
    assert!(maps[0].get("mode").is_none());
    assert!(maps[0].get("pickup").is_none());
    assert!(maps[0].get("consume").is_none());
    // Everything that differs is.
    assert_eq!(maps[2]["min"], 0.25);
    assert_eq!(maps[2]["max"], 0.75);
    assert_eq!(maps[2]["invert"], true);
    assert_eq!(maps[2]["mode"], "relative_offset");
    assert_eq!(maps[2]["step"], 0.02);
    assert_eq!(maps[2]["pickup"], false);
    assert_eq!(maps[2]["consume"], false);
    assert_eq!(maps[3]["source"]["port"], "Pad Controller");
    assert_eq!(maps[3]["source"]["kind"], "note");
    assert_eq!(maps[3]["mode"], "toggle");
    assert_eq!(maps[3]["target"]["action"], "play");
    assert_eq!(maps[4]["source"]["kind"], "cc14");
    assert!(maps[4]["source"].get("channel").is_none());
    assert_eq!(maps[4]["target"]["kind"], "tempo");
    assert_eq!(maps[5]["source"]["kind"], "pitch_bend");
    assert!(maps[5]["source"].get("number").is_none());
    assert_eq!(maps[5]["target"]["id"], 1);
    assert_eq!(maps[6]["target"]["kind"], "selected");
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.midi_maps, d.doc().midi_maps);
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_project_without_mappings_differs_from_its_version_27_file_in_the_version_line() {
    let (d, r) = project();
    let text = save_project(d.doc());
    assert!(!text.contains("midi_maps"));
    let v27 = text.replace("\"version\": 35", "\"version\": 27");
    let loaded = load_project(&v27, &r).unwrap();
    assert!(loaded.midi_maps.is_empty());
    assert_eq!(save_project(&loaded), text);
    // Before the format a file has no field for them.
    let with = save_project(mapped().0.doc()).replace("\"version\": 35", "\"version\": 27");
    let e = load_project(&with, &r).unwrap_err().to_string();
    assert!(e.contains("midi_maps"), "{e}");
}

#[test]
fn a_mapping_may_name_a_device_that_does_not_exist() {
    let (d, r) = mapped();
    let text = save_project(d.doc()).replace("\"device\": \"a\"", "\"device\": \"gone\"");
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(
        loaded.midi_map(3).unwrap().target,
        param("gone", "filter.cutoff")
    );
}

#[test]
fn bad_mappings_are_refused_on_load_with_their_json_path() {
    let (d, r) = mapped();
    let text = save_project(d.doc());
    let err = |t: String| load_project(&t, &r).unwrap_err().to_string();
    let e = err(text.replacen("{\"id\": 2, \"source\"", "{\"id\": 1, \"source\"", 1));
    assert!(e.contains("midi_maps[1].id") && e.contains("twice"), "{e}");
    let e = err(text.replacen("{\"id\": 1, \"source\"", "{\"id\": 0, \"source\"", 1));
    assert!(e.contains("midi_maps[0].id"), "{e}");
    let e = err(text.replacen("\"number\": 7", "\"number\": 200", 1));
    assert!(e.contains("midi_maps[0].source.number"), "{e}");
    let e = err(text.replacen("\"channel\": 0", "\"channel\": 16", 1));
    assert!(e.contains("midi_maps[0].source.channel"), "{e}");
    let e = err(text.replacen("\"kind\": \"cc\"", "\"kind\": \"nrpn\"", 1));
    assert!(e.contains("midi_maps[0].source.kind"), "{e}");
    let e = err(text.replacen("\"action\": \"play\"", "\"action\": \"explode\"", 1));
    assert!(e.contains("midi_maps[3].target.action"), "{e}");
    let e = err(text.replacen("\"min\": 0.25", "\"min\": 2", 1));
    assert!(e.contains("midi_maps[2].min"), "{e}");
    let e = err(text.replacen("\"mode\": \"relative_offset\"", "\"mode\": \"spin\"", 1));
    assert!(e.contains("midi_maps[2].mode"), "{e}");
    let e = err(text.replacen("\"step\": 0.02", "\"step\": 0", 1));
    assert!(e.contains("midi_maps[2].step"), "{e}");
    let e = err(text.replacen(
        "\"kind\": \"pitch_bend\"",
        "\"kind\": \"pitch_bend\", \"number\": 1",
        1,
    ));
    assert!(e.contains("midi_maps[5].source.number"), "{e}");
    // A relative encoder on a button.
    let e = err(text.replacen("\"mode\": \"toggle\"", "\"mode\": \"relative_twos\"", 1));
    assert!(e.contains("midi_maps[3]"), "{e}");
    let e = err(text.replacen("\"midi_maps\": [", "\"midi_maps\": [\n    {\"id\": 9, \"source\": {\"kind\": \"cc\"}, \"target\": {\"kind\": \"tempo\"}},", 1));
    assert!(e.contains("midi_maps[0].source.number"), "{e}");
}

#[test]
fn mapping_ids_are_not_the_device_keys_a_project_keeps_after_undo() {
    // The history carries the whole list, so an undo of a load-time-numbered list is exact.
    let (d, r) = mapped();
    let text = save_project(d.doc());
    let mut loaded = Document::new(load_project(&text, &r).unwrap());
    run_map(&mut loaded, &r, MidiMapCommand::Remove { id: 4 }).unwrap();
    loaded.undo(&r).unwrap();
    assert_eq!(loaded.doc().midi_maps, d.doc().midi_maps);
}
