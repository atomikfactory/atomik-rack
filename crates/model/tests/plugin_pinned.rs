//! The layout rows a project uses (a stored value, a lane of a track or of a note clip, a macro
//! target or bank assignment, a pin, a learned MIDI control) are the ones a plugin has to keep
//! listing even when its own table leaves them out, and the ones a save keeps: every way a
//! document can name a plugin param is counted, so none is lost when the table shrinks.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, Curve, Document, MapOptions, MidiMapCommand, MidiMapping, MidiSource, MidiTarget,
    ParamLane, RackDoc, SongCommand, SourceKind, load_project, save_project,
};
use common::plugin::*;
use common::song::{add_clip, add_track, notes_clip, point};
use common::*;

fn doc_of(r: &Registry, commands: impl IntoIterator<Item = Command>) -> RackDoc {
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        r,
        [add_plugin(0, &synth_ref(), &synth_layout(), Some("lead"))],
    );
    run(&mut d, r, commands);
    d.doc().clone()
}

fn pinned_ids(doc: &RackDoc) -> Vec<u32> {
    doc.plugin_pinned_rows()
        .get(&synth_ref())
        .map(|rows| rows.iter().map(|r| r.id).collect())
        .unwrap_or_default()
}

fn saved_ids(doc: &RackDoc) -> Vec<u32> {
    plugin_of(&doc.compacted(), "lead")
        .layout
        .params
        .iter()
        .map(|r| r.id)
        .collect()
}

fn map_command(param: &str) -> Command {
    Command::MidiMap(MidiMapCommand::Add(MidiMapping {
        id: 0,
        source: MidiSource::any_port(Some(0), SourceKind::Cc(7)),
        target: MidiTarget::Param {
            device: key("lead"),
            param: param.into(),
        },
        options: MapOptions::default(),
    }))
}

#[test]
fn a_project_that_uses_nothing_pins_nothing() {
    let r = plugin_registry();
    let doc = doc_of(&r, []);
    assert!(doc.plugin_pinned_rows().is_empty());
    assert!(saved_ids(&doc).is_empty());
}

#[test]
fn a_stored_value_pins_its_row() {
    let r = plugin_registry();
    let doc = doc_of(&r, [set_param("lead", "p10", 0.25)]);
    assert_eq!(pinned_ids(&doc), [10]);
    assert_eq!(saved_ids(&doc), [10]);
}

#[test]
fn a_track_lane_pins_its_row() {
    let r = plugin_registry();
    let doc = doc_of(
        &r,
        [
            add_track("lead"),
            Command::Song(SongCommand::AddAutomationLane {
                device: key("lead"),
                param: "p11".into(),
            }),
        ],
    );
    assert_eq!(pinned_ids(&doc), [11]);
    assert_eq!(saved_ids(&doc), [11]);
}

#[test]
fn a_pin_keeps_its_row() {
    let r = plugin_registry();
    let doc = doc_of(&r, [set_pinned("lead", &[42])]);
    assert_eq!(pinned_ids(&doc), [42]);
    assert_eq!(saved_ids(&doc), [42]);
}

#[test]
fn a_lane_of_a_note_clip_pins_its_row() {
    let r = plugin_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("lead")),
            add_track("lead"),
            add_clip("lead", 0, notes_clip(0, 960, Vec::new())),
        ],
    );
    let clip = d.doc().song.tracks[0].lanes[0].clips[0].id;
    run(
        &mut d,
        &r,
        [Command::Song(SongCommand::SetClipParamLanes {
            clip,
            lanes: vec![ParamLane {
                device: key("lead"),
                param: "p42".into(),
                points: vec![point(0, 0.5, Curve::Linear)],
            }],
            merge: None,
        })],
    );
    assert_eq!(pinned_ids(d.doc()), [42]);
    assert_eq!(saved_ids(d.doc()), [42]);
}

#[test]
fn a_learned_midi_control_pins_its_row() {
    let r = plugin_registry();
    let doc = doc_of(&r, [map_command("p11")]);
    assert_eq!(
        saved_ids(&doc),
        [11],
        "the save keeps the row the mapping names"
    );
    assert_eq!(pinned_ids(&doc), [11]);
}

#[test]
fn a_macro_target_pins_its_row() {
    let r = plugin_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("layer")),
            add_plugin_inner("layer", 0, &fx_ref(), &fx_layout(), Some("comp"), false),
            set_macro("layer", 0, target("comp", "p1", -60.0, 12.0)),
        ],
    );
    let rows = d.doc().plugin_pinned_rows();
    let ids: Vec<u32> = rows[&fx_ref()].iter().map(|r| r.id).collect();
    assert_eq!(ids, [1]);
}

#[test]
fn the_rows_of_two_devices_of_one_plugin_are_one_list_in_id_order() {
    let r = plugin_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("one")),
            add_plugin(1, &synth_ref(), &synth_layout(), Some("two")),
            set_param("one", "p42", 4.0),
            set_param("two", "p10", 0.25),
            set_param("two", "p42", 8.0),
        ],
    );
    let ids: Vec<u32> = d.doc().plugin_pinned_rows()[&synth_ref()]
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [10, 42]);
}

#[test]
fn a_dead_key_has_no_row_to_pin_and_stays_in_the_document() {
    let r = plugin_registry();
    let doc = doc_of(&r, [set_param("lead", "p10", 0.25)]);
    // The saved text names a param the layout has no row for (a plugin that dropped it): the
    // value stays, nothing is pinned for it.
    let mut v: serde_json::Value = serde_json::from_str(&save_project(&doc)).unwrap();
    let params = v["devices"][0]["params"].as_object_mut().unwrap();
    params.insert("p9999".into(), 0.5.into());
    let loaded = load_project(&v.to_string(), &r).unwrap();
    assert_eq!(pinned_ids(&loaded), [10]);
    assert!(
        loaded
            .device(&key("lead"))
            .unwrap()
            .params
            .contains_key("p9999")
    );
}

#[test]
fn the_rows_survive_a_save_and_a_load_with_the_plugin_installed() {
    let r = plugin_registry();
    let doc = doc_of(
        &r,
        [
            set_param("lead", "p10", 0.25),
            add_track("lead"),
            Command::Song(SongCommand::AddAutomationLane {
                device: key("lead"),
                param: "p11".into(),
            }),
            map_command("p42"),
        ],
    );
    let loaded = load_project(&save_project(&doc), &r).unwrap();
    assert_eq!(pinned_ids(&loaded), [10, 11, 42]);
    assert_eq!(loaded.plugin_pinned_rows(), doc.plugin_pinned_rows());
}
