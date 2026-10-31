//! A device that took a patch bounces bit for bit like a device created from the same patch,
//! and a reset device bounces like a new one.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, DeviceSnapshot, Document, Note, RackDoc,
    SongCommand, WorkflowCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
}

fn set(d: &mut Document, k: &str, param: &str, value: f32) {
    apply(
        d,
        Command::SetParam {
            key: key(k),
            param: param.to_string(),
            value,
            merge: None,
        },
    );
}

/// A track with a two-note clip on the first non-console device of the document.
fn with_track(d: &mut Document) -> DeviceKey {
    let synth = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == SYNTH)
        .expect("a synth")
        .key
        .clone();
    apply(
        d,
        Command::Song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
    );
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: synth.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(
                    vec![Note::new(0, PPQ, 60, 0.8), Note::new(2 * PPQ, PPQ, 67, 0.6)].into(),
                ),
            },
        }),
    );
    synth
}

fn bounce(doc: &RackDoc) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(256).unwrap(),
        workers: 1,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, None).unwrap();
    assert!(r.channels.iter().flatten().any(|x| *x != 0.0), "not silent");
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

/// A synth whose sound is far from the defaults.
fn source() -> Document {
    let mut d = Document::new(empty_doc());
    apply(
        &mut d,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: Some(key("src")),
        },
    );
    set(&mut d, "src", "filter.cutoff", 900.0);
    set(&mut d, "src", "filter.resonance", 0.6);
    d
}

#[test]
fn a_device_that_took_a_patch_bounces_like_one_created_from_it() {
    let patch = DeviceSnapshot::capture(source().doc(), &[key("src")], false).unwrap();

    // A new synth takes the patch.
    let mut applied = Document::new(empty_doc());
    apply(
        &mut applied,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: None,
        },
    );
    let target = applied.doc().devices[0].key.clone();
    apply(
        &mut applied,
        Command::Workflow(WorkflowCommand::ApplyPatch {
            device: target,
            snapshot: patch.clone(),
        }),
    );
    with_track(&mut applied);

    // A synth created from the patch.
    let mut created = Document::new(empty_doc());
    apply(
        &mut created,
        Command::Workflow(WorkflowCommand::PasteDevices {
            snapshot: patch,
            container: None,
            at: 0,
        }),
    );
    with_track(&mut created);

    assert_eq!(bounce(applied.doc()), bounce(created.doc()));
}

#[test]
fn a_reset_device_bounces_like_a_new_one() {
    let patch = DeviceSnapshot::capture(source().doc(), &[key("src")], false).unwrap();
    let mut reset = Document::new(empty_doc());
    apply(
        &mut reset,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: None,
        },
    );
    let target = reset.doc().devices[0].key.clone();
    apply(
        &mut reset,
        Command::Workflow(WorkflowCommand::ApplyPatch {
            device: target.clone(),
            snapshot: patch,
        }),
    );
    apply(
        &mut reset,
        Command::Workflow(WorkflowCommand::ResetDevice { device: target }),
    );
    with_track(&mut reset);

    let mut fresh = Document::new(empty_doc());
    apply(
        &mut fresh,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: None,
        },
    );
    with_track(&mut fresh);
    assert_eq!(bounce(reset.doc()), bounce(fresh.doc()));
}

#[test]
fn applying_the_patch_changes_the_sound() {
    let patch = DeviceSnapshot::capture(source().doc(), &[key("src")], false).unwrap();
    let mut applied = Document::new(empty_doc());
    apply(
        &mut applied,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: None,
        },
    );
    with_track(&mut applied);
    let before = bounce(applied.doc());
    let target = applied.doc().devices[0].key.clone();
    apply(
        &mut applied,
        Command::Workflow(WorkflowCommand::ApplyPatch {
            device: target,
            snapshot: patch,
        }),
    );
    assert_ne!(before, bounce(applied.doc()));
}
