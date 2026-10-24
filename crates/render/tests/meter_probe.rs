//! `probe_devices`: an offline render that stops early and reports what the metering devices
//! (spectrum, phase meter) showed: their published values and the newest scope frames. The
//! probe changes nothing in the audio, and a project with the meters bounces bit for bit like
//! the project without them.

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, ConsoleCommand, CreateOptions, CreateSource, DeviceKey, Document,
    InsertEffect, Note, RackDoc, SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, probe_devices, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A synth playing a long note, with `inserts` on its channel.
fn project(inserts: &[&str]) -> Document {
    let mut d = Document::new(new_project_doc());
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 2 * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 2 * BAR, 57, 0.8)]),
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    let channel = d
        .doc()
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(&key("lead")))
        .unwrap()
        .channel
        .clone();
    for (slot, t) in inserts.iter().enumerate() {
        apply(
            &mut d,
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel.clone(),
                slot,
                effect: Some(InsertEffect::New {
                    type_id: (*t).into(),
                }),
            }),
        );
    }
    d
}

fn device_of(doc: &RackDoc, type_id: &str) -> DeviceKey {
    doc.devices
        .iter()
        .find(|d| d.type_id == type_id)
        .unwrap_or_else(|| panic!("no {type_id}"))
        .key
        .clone()
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn frame_bits(frames: &[f32]) -> Vec<u32> {
    frames.iter().map(|s| s.to_bits()).collect()
}

fn bits(channels: &[Vec<f32>]) -> Vec<Vec<u32>> {
    channels
        .iter()
        .map(|c| c.iter().map(|s| s.to_bits()).collect())
        .collect()
}

#[test]
fn a_probe_reads_the_values_and_the_newest_frames_of_the_meters() {
    let d = project(&["atomik.phase-meter", "atomik.spectrum"]);
    let doc = d.doc();
    let (phase, spectrum) = (
        device_of(doc, "atomik.phase-meter"),
        device_of(doc, "atomik.spectrum"),
    );
    let registry = default_registry();
    let frames = 48_000;
    let probes = probe_devices(
        doc,
        &registry,
        &options(512, 0),
        None,
        frames,
        &[phase.clone(), spectrum.clone(), key("lead")],
    )
    .unwrap();
    // The synth is neither metered nor tapped: left out.
    assert_eq!(probes.len(), 2, "{probes:?}");
    let p = probes.iter().find(|p| p.key == phase).unwrap();
    assert_eq!(p.values.len(), 16);
    assert!(p.values[0] > 0.01, "the synth is audible: {:?}", p.values);
    assert!(
        p.values[8] > 0.0 && p.values[8] <= 1.0,
        "a correlation: {}",
        p.values[8]
    );
    assert_eq!((p.left.len(), p.right.len()), (8192, 8192));
    assert!(
        p.head >= frames as u64,
        "the device ran for the whole render: {}",
        p.head
    );
    assert!(p.left.iter().any(|v| v.abs() > 0.001));
    let s = probes.iter().find(|p| p.key == spectrum).unwrap();
    assert_eq!(s.values.len(), 2);
    // The spectrum sits after the phase meter, which does not change the audio: they saw the
    // same frames.
    assert_eq!(frame_bits(&s.left), frame_bits(&p.left));
}

#[test]
fn a_probe_stops_where_it_is_told_and_changes_nothing() {
    let d = project(&["atomik.phase-meter"]);
    let doc = d.doc();
    let phase = device_of(doc, "atomik.phase-meter");
    let registry = default_registry();
    let early = probe_devices(
        doc,
        &registry,
        &options(512, 0),
        None,
        4_800,
        std::slice::from_ref(&phase),
    )
    .unwrap();
    let late = probe_devices(
        doc,
        &registry,
        &options(512, 0),
        None,
        48_000,
        std::slice::from_ref(&phase),
    )
    .unwrap();
    assert!(late[0].head > early[0].head + 40_000);
    // The same probe for any block size and worker count.
    let again = probe_devices(
        doc,
        &registry,
        &options(333, 2),
        None,
        48_000,
        std::slice::from_ref(&phase),
    )
    .unwrap();
    assert_eq!(frame_bits(&late[0].left), frame_bits(&again[0].left));
    let values = |p: &[atomik_render::project::DeviceProbe]| {
        p[0].values.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    };
    assert_eq!(values(&late), values(&again));
    // No devices to probe: nothing comes back.
    assert!(
        probe_devices(doc, &registry, &options(512, 0), None, 4_800, &[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_meters_do_not_change_the_bounce_by_a_single_bit() {
    let registry = default_registry();
    let plain = render_doc_in(project(&[]).doc(), &registry, &options(512, 0), None).unwrap();
    let metered = render_doc_in(
        project(&["atomik.phase-meter", "atomik.spectrum"]).doc(),
        &registry,
        &options(512, 0),
        None,
    )
    .unwrap();
    assert_eq!(bits(&plain.channels), bits(&metered.channels));
}
