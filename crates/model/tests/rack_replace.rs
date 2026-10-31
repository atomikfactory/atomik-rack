//! `WorkflowCommand::ReplaceDevice`: a device becomes another of the same kind and keeps its
//! track, channel, cables and place.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document, SongCommand,
    WorkflowCommand, empty_doc, replace_lost_lanes, replace_reason,
};
use common::plugin::*;
use common::song::*;
use common::workflow::*;
use common::*;

const WAVETABLE: &str = "atomik.wavetable-synth";
const FM: &str = "atomik.fm-synth";

fn reg() -> Registry {
    plugin_console_registry()
}

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

fn replace(target: &str, t: &str) -> Command {
    wf(WorkflowCommand::ReplaceDevice {
        target: key(target),
        source: typ(t),
    })
}

/// A synth with channel and armed track, a clip with a note, and two automation lanes.
fn song_doc(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            wf(WorkflowCommand::CreateInstrument {
                index: 0,
                source: typ(SYNTH),
                key: Some(key("synth")),
                options: CreateOptions::default(),
            }),
            add_clip("synth", 0, notes_clip(0, 960, vec![note(0, 480, 60, 0.8)])),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "osc1.wave".into(),
            }),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
        ],
    );
    d
}

#[test]
fn a_replaced_instrument_keeps_its_track_channel_and_cables_in_one_undo() {
    let r = reg();
    let mut d = song_doc(&r);
    let before = d.doc().clone();
    let lost = replace_lost_lanes(d.doc(), &r, &key("synth"), &typ(WAVETABLE));
    assert_eq!(lost, vec!["osc1.wave".to_owned()]);
    assert_eq!(
        replace_reason(d.doc(), &r, &key("synth"), &typ(WAVETABLE)),
        None
    );
    one_step(&mut d, &r, replace("synth", WAVETABLE));
    let doc = d.doc();
    let new = doc.device(&key("synth")).expect("the key stays");
    assert_eq!(new.type_id, WAVETABLE);
    assert_eq!(new.name, "Wavetable Synth");
    assert_eq!(doc.devices.len(), before.devices.len());
    assert_eq!(
        doc.cables, before.cables,
        "out_l/out_r fit the new instrument"
    );
    let track = doc.song.track(&key("synth")).expect("the track stays");
    let old_track = before.song.track(&key("synth")).unwrap();
    assert_eq!(track.armed, old_track.armed);
    assert_eq!(
        track.lanes[0].clips, old_track.lanes[0].clips,
        "the clips stay"
    );
    assert_eq!(
        track.lanes.len(),
        old_track.lanes.len() - 1,
        "one lane lost its param"
    );
    assert_eq!(doc.console.channels, before.console.channels);
    // The strip keeps its name.
    let channel = &doc.console.channels[0].channel;
    assert_eq!(doc.device(channel), before.device(channel));
    // Undo brings back the old device with its params and the lane.
    d.undo(&r).unwrap();
    assert_eq!(d.doc().devices, before.devices);
    assert_eq!(d.doc().song, before.song);
}

#[test]
fn a_renamed_device_and_channel_keep_their_names() {
    let r = reg();
    let mut d = song_doc(&r);
    let channel = d.doc().console.channels[0].channel.clone();
    run(
        &mut d,
        &r,
        [
            Command::Rename {
                key: key("synth"),
                name: "Lead".into(),
            },
            Command::Rename {
                key: channel.clone(),
                name: "Lead bus".into(),
            },
        ],
    );
    one_step(&mut d, &r, replace("synth", FM));
    assert_eq!(d.doc().device(&key("synth")).unwrap().name, "Lead");
    assert_eq!(d.doc().device(&channel).unwrap().name, "Lead bus");
}

#[test]
fn an_insert_effect_keeps_its_place_in_the_chain() {
    let r = reg();
    let mut d = song_doc(&r);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateEffect {
            source: typ(DELAY),
            target: CreateTarget::Selected(key("synth")),
            key: None,
            options: CreateOptions::default(),
        })],
    );
    let channel = d.doc().console.channels[0].channel.clone();
    let chain = atomik_model::insert_chain(d.doc(), &r, &channel);
    assert_eq!(chain.len(), 1);
    one_step(&mut d, &r, replace(&chain[0].0, "atomik.reverb"));
    let after = atomik_model::insert_chain(d.doc(), &r, &channel);
    assert_eq!(after, chain, "the chain holds the same slot, now a reverb");
    assert_eq!(d.doc().device(&chain[0]).unwrap().type_id, "atomik.reverb");
}

#[test]
fn a_replaced_player_keeps_the_track_entry() {
    let r = reg();
    let mut d = song_doc(&r);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreatePlayer {
            type_id: ARP.into(),
            target: CreateTarget::Selected(key("synth")),
            options: CreateOptions::default(),
        })],
    );
    let arp: DeviceKey = d
        .doc()
        .song
        .track(&key("synth"))
        .unwrap()
        .notes_to
        .clone()
        .unwrap();
    one_step(&mut d, &r, replace(&arp.0, "atomik.note-filter"));
    assert_eq!(
        d.doc().song.track(&key("synth")).unwrap().notes_to,
        Some(arp.clone())
    );
    assert_eq!(d.doc().device(&arp).unwrap().type_id, "atomik.note-filter");
    assert!(
        d.doc()
            .cables
            .iter()
            .any(|c| c.from.device == arp && c.to.device == key("synth"))
    );
}

#[test]
fn replacements_of_another_kind_utilities_and_frozen_tracks_are_rejected() {
    let r = reg();
    let mut d = song_doc(&r);
    // An instrument by an effect.
    assert!(matches!(
        rejected(&mut d, &r, replace("synth", DELAY)),
        atomik_model::ModelError::BadScope(_)
    ));
    // The console channel and the master.
    let channel = d.doc().console.channels[0].channel.clone();
    rejected(&mut d, &r, replace(&channel.0, "atomik.channel"));
    let master = d.doc().console.master.clone().unwrap();
    rejected(&mut d, &r, replace(&master.0, "atomik.master"));
    // A mixer is a utility.
    run(&mut d, &r, [add(usize::MAX, MIXER, Some("mix"))]);
    rejected(&mut d, &r, replace("mix", MIXER));
    // Unknown key and unknown type.
    rejected(&mut d, &r, replace("nothing", WAVETABLE));
    rejected(&mut d, &r, replace("synth", "atomik.nope"));
}

#[test]
fn a_frozen_track_cannot_have_its_instrument_replaced() {
    let r = reg();
    let d = song_doc(&r);
    let mut frozen = d.doc().clone();
    // Mark the track frozen directly: the freeze data itself is not what is under test.
    let t = frozen
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("synth"))
        .unwrap();
    t.frozen = Some(atomik_model::freeze::Frozen {
        start: 0,
        length: 960,
        audio: common::audio::audio(1),
        effects: Vec::new(),
    });
    let reason = replace_reason(&frozen, &r, &key("synth"), &typ(WAVETABLE));
    assert!(reason.is_some_and(|m| m.contains("frozen")));
}

#[test]
fn an_automation_track_whose_lanes_all_go_is_removed_with_them() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("delay")),
            sc(SongCommand::AddAutomationLane {
                device: key("delay"),
                param: "feedback".into(),
            }),
        ],
    );
    assert_eq!(
        replace_lost_lanes(d.doc(), &r, &key("delay"), &typ("atomik.reverb")),
        vec!["feedback".to_owned()]
    );
    one_step(&mut d, &r, replace("delay", "atomik.reverb"));
    assert!(d.doc().song.track(&key("delay")).is_none());
    assert!(d.doc().validate().is_ok());
    let text = atomik_model::save_project(d.doc());
    atomik_model::load_project(&text, &r).expect("the saved project loads");
}
