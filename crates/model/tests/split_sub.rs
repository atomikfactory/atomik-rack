//! `WorkflowCommand::SplitSub`: a Mono Bass track gets a mono sine sub layer and a low cut on the
//! bass, inside one container, in one undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, DEFAULT_SPLIT_HZ, DeviceKey, Document, MAX_SPLIT_HZ,
    MIN_SPLIT_HZ, ModelError, RackDoc, SongCommand, WorkflowCommand, empty_doc,
};
use common::song::*;
use common::workflow::*;
use common::*;

const BASS: &str = "atomik.mono-bass";

fn reg() -> Registry {
    console_registry()
}

fn split(device: &str, freq: f32) -> Command {
    wf(WorkflowCommand::SplitSub {
        device: key(device),
        freq,
    })
}

/// A Mono Bass with a channel and a track that has a clip and an automation lane, and a synth.
fn bass_doc(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            wf(WorkflowCommand::CreateInstrument {
                index: 0,
                source: CreateSource::Type(BASS.into()),
                key: Some(key("bass")),
                options: CreateOptions::default(),
            }),
            wf(WorkflowCommand::CreateInstrument {
                index: 1,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key("lead")),
                options: CreateOptions::default(),
            }),
            add_clip("bass", 0, notes_clip(0, 960, vec![note(0, 480, 36, 0.8)])),
        ],
    );
    d
}

fn param(doc: &RackDoc, path: &str, name: &str) -> f32 {
    let d = doc.resolve(&key(path)).unwrap_or_else(|| panic!("{path}"));
    d.params.get(name).copied().unwrap_or_else(|| {
        // Unset params read as their default; the test registry knows it.
        let r = reg();
        r.get(&d.type_id)
            .unwrap()
            .descriptor
            .param_by_key(name)
            .unwrap()
            .default
    })
}

#[test]
fn split_sub_is_one_undo_step_and_builds_the_container() {
    let r = reg();
    let mut d = bass_doc(&r);
    let before = d.doc().clone();
    one_step(&mut d, &r, split("bass", 120.0));
    let doc = d.doc();
    let container = doc
        .devices
        .iter()
        .find(|x| x.type_id == CONTAINER)
        .expect("a container");
    let inner = container.inner.as_ref().expect("its inside");
    let types: Vec<&str> = inner.devices.iter().map(|x| x.type_id.as_str()).collect();
    assert_eq!(
        types,
        [BASS, SYNTH, "atomik.eq", "atomik.line-mixer"],
        "the bass, the sub layer, the low cut and the sum"
    );
    // The bass is untouched.
    let original = &inner.devices[0];
    assert_eq!(original.params, before.device(&key("bass")).unwrap().params);
    // The track, its clip and the channel follow the bass into the container.
    let track = doc.song.track(&container.key).expect("the track moved");
    assert_eq!(track.lanes[0].clips.len(), 1);
    assert!(doc.device(&key("bass")).is_none());
    assert!(
        doc.console
            .channels
            .iter()
            .any(|c| c.source.as_ref() == Some(&container.key))
    );
    // Both layers hear the notes: two full-range splits.
    assert_eq!(inner.splits.len(), 2);
    assert_eq!(doc.devices.len(), before.devices.len());
    // The rest of the project is as it was.
    assert_eq!(doc.device(&key("lead")), before.device(&key("lead")));
    doc.validate().expect("the document stays valid");
    assert!(doc.check_containers(&r).is_ok());
}

#[test]
fn the_sub_layer_is_a_mono_phase_reset_sine_low_passed_at_the_split() {
    let r = reg();
    let mut d = bass_doc(&r);
    run(&mut d, &r, [split("bass", 100.0)]);
    let doc = d.doc();
    let c = doc.devices.iter().find(|x| x.type_id == CONTAINER).unwrap();
    let sub = format!("{}/sub", c.key);
    assert_eq!(param(doc, &sub, "voices"), 1.0);
    assert_eq!(param(doc, &sub, "osc1.wave"), 3.0, "a sine");
    assert_eq!(
        param(doc, &sub, "osc2.wave"),
        3.0,
        "both oscillators are sines"
    );
    assert_eq!(param(doc, &sub, "osc.phase"), 0.0, "reset");
    assert_eq!(param(doc, &sub, "osc.phase_start"), 0.0);
    assert_eq!(param(doc, &sub, "spread"), 0.0);
    assert_eq!(param(doc, &sub, "chorus.mix"), 0.0);
    assert_eq!(
        param(doc, &sub, "filter.type"),
        0.0,
        "the state variable filter"
    );
    assert_eq!(param(doc, &sub, "filter.mode"), 0.0, "low-pass, 12 dB");
    assert_eq!(
        param(doc, &sub, "filter.cutoff"),
        125.0,
        "1.25 times the split"
    );
    assert_eq!(param(doc, &sub, "filter.env_amount"), 0.0);
    assert_eq!(param(doc, &sub, "filter.key_track"), 0.0);
    // It follows the envelope of the bass.
    let bass = format!("{}/bass", c.key);
    for p in ["env.amp.attack", "env.amp.release", "glide", "legato"] {
        assert_eq!(param(doc, &sub, p), param(doc, &bass, p), "{p}");
    }
    // The bass goes through a low cut at the split.
    let eq = format!("{}/split-eq", c.key);
    assert_eq!(param(doc, &eq, "band1.on"), 1.0);
    assert_eq!(param(doc, &eq, "band1.freq"), 100.0);
    assert_eq!(param(doc, &eq, "lowcut.slope"), 1.0, "24 dB per octave");
}

#[test]
fn the_sub_plays_the_fundamentals_of_both_oscillators_at_their_level() {
    let r = reg();
    let mut d = bass_doc(&r);
    // The default Mono Bass: a saw at 60 % and a square an octave down at 40 %.
    run(&mut d, &r, [split("bass", 120.0)]);
    let doc = d.doc();
    let c = doc.devices.iter().find(|x| x.type_id == CONTAINER).unwrap();
    let sub = format!("{}/sub", c.key);
    assert_eq!(param(doc, &sub, "osc1.octave"), 0.0);
    assert_eq!(param(doc, &sub, "osc2.octave"), -1.0, "the square's octave");
    assert_eq!(param(doc, &sub, "osc2.wave"), 3.0, "both are sines");
    assert_eq!(param(doc, &sub, "osc1.fine"), 0.0);
    assert_eq!(
        param(doc, &sub, "osc2.fine"),
        0.0,
        "no detune: the bass's 7 cents are not copied"
    );
    // The fundamentals: 0.6 of a saw's 2/pi and 0.4 of a square's 4/pi.
    let pi = std::f32::consts::PI;
    let (a1, a2) = (0.6 * 2.0 / pi, 0.4 * 4.0 / pi);
    assert!((param(doc, &sub, "osc.mix") - a2 / (a1 + a2)).abs() < 1e-5);
    let bass = format!("{}/bass", c.key);
    let step = param(doc, &sub, "master.level") - param(doc, &bass, "master.level");
    assert!((step - 20.0 * (a1 + a2).log10()).abs() < 0.01, "{step}");
    // Each channel of the sum gives back the -3 dB of the centred pan.
    let mix = format!("{}/split-mix", c.key);
    assert!((param(doc, &mix, "ch1.level") - 3.0103).abs() < 1e-4);
    assert!((param(doc, &mix, "ch2.level") - 3.0103).abs() < 1e-4);
}

#[test]
fn the_default_split_is_inside_the_range() {
    assert!((MIN_SPLIT_HZ..=MAX_SPLIT_HZ).contains(&DEFAULT_SPLIT_HZ));
}

fn refused(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = d.doc().clone();
    let e = d.apply(c, r).expect_err("must be refused");
    assert_eq!(d.doc(), &before, "a refusal changes nothing");
    e
}

#[test]
fn split_sub_refuses_what_it_cannot_split() {
    let r = reg();
    let mut d = bass_doc(&r);
    // Not a Mono Bass.
    assert!(
        refused(&mut d, &r, split("lead", 120.0))
            .to_string()
            .contains("Mono Bass")
    );
    // A frequency outside the range.
    for f in [10.0, 1_000.0, f32::NAN] {
        refused(&mut d, &r, split("bass", f));
    }
    // A device that is not there.
    refused(&mut d, &r, split("nothing", 120.0));
    // Splitting twice: the bass is inside a container now.
    run(&mut d, &r, [split("bass", 120.0)]);
    refused(&mut d, &r, split("bass", 120.0));
    let container = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == CONTAINER)
        .unwrap()
        .key
        .clone();
    refused(&mut d, &r, split(&format!("{container}/bass"), 120.0));
    refused(&mut d, &r, split(&container.0, 120.0));
}

#[test]
fn a_bass_without_a_track_is_refused() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [Command::AddDevice {
            index: 0,
            type_id: BASS.into(),
            key: Some(key("bass")),
        }],
    );
    let e = refused(&mut d, &r, split("bass", 120.0));
    assert!(e.to_string().contains("no track"), "{e}");
}

#[test]
fn the_automation_of_the_track_is_dropped_and_said() {
    let r = reg();
    let mut d = bass_doc(&r);
    run(
        &mut d,
        &r,
        [sc(SongCommand::AddAutomationLane {
            device: key("bass"),
            param: "filter.cutoff".into(),
        })],
    );
    let notes = d
        .doc()
        .split_sub_notes(&r, &key("bass"), 120.0)
        .expect("it would work");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("automation"), "{notes:?}");
    one_step(&mut d, &r, split("bass", 120.0));
    let doc = d.doc();
    let c = doc.devices.iter().find(|x| x.type_id == CONTAINER).unwrap();
    assert!(
        doc.song
            .track(&c.key)
            .unwrap()
            .lanes
            .iter()
            .all(|l| { !matches!(l.kind, atomik_model::LaneKind::Automation(_)) })
    );
}

#[test]
fn a_project_with_a_split_bass_saves_and_loads_the_same() {
    let r = reg();
    let mut d = bass_doc(&r);
    run(&mut d, &r, [split("bass", 90.0)]);
    let text = atomik_model::save_project(d.doc());
    let back = atomik_model::load_project(&text, &r).expect("it loads");
    assert_eq!(back.devices, d.doc().devices);
    assert_eq!(back.cables, d.doc().cables);
    assert_eq!(back.song, d.doc().song);
}

#[allow(dead_code)]
fn unused(_: &DeviceKey) {}
