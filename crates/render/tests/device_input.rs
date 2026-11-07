//! The audio at a device's input, rendered offline: it is what the devices in front of the
//! device send it, whatever the device itself does, a span of it is a slice of the same samples,
//! and a device with nothing cabled to its input is refused.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, ConsoleCommand, CreateOptions, CreateSource, DeviceKey, Document,
    InsertEffect, Note, RackDoc, SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::device_input::{input_sources, render_device_input};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};

const BAR: i64 = 4 * PPQ;
const EQ: &str = "atomik.dynamic-eq";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A synth playing a phrase on a channel whose insert slot holds a dynamic EQ.
fn project(eq_gain_db: f32) -> (RackDoc, DeviceKey) {
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
    let notes = [(0, PPQ, 64), (PPQ, PPQ, 67), (BAR, 2 * PPQ, 71)];
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
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(
                    notes
                        .iter()
                        .map(|&(t, l, k)| Note::new(t, l, k, 0.8))
                        .collect(),
                ),
            },
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(3 * BAR)));
    let channel = d
        .doc()
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(&key("lead")))
        .unwrap()
        .channel
        .clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::New { type_id: EQ.into() }),
        }),
    );
    let eq = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == EQ)
        .unwrap()
        .key
        .clone();
    for (p, v) in [
        ("band2.on", 1.0),
        ("band2.gain", eq_gain_db),
        ("band2.range", 0.0),
    ] {
        apply(
            &mut d,
            Command::SetParam {
                key: eq.clone(),
                param: p.into(),
                value: v,
                merge: None,
            },
        );
    }
    (d.doc().clone(), eq)
}

fn capture(
    doc: &RackDoc,
    eq: &DeviceKey,
    span: (u64, u64),
) -> atomik_render::device_input::DeviceInput {
    render_device_input(
        doc,
        &default_registry(),
        (&ProjectOptions::default(), None),
        &mut PluginBounce::default(),
        eq,
        span,
        RenderControl::default(),
    )
    .expect("render")
    .expect("not cancelled")
}

#[test]
fn the_input_is_the_sound_in_front_of_the_device_and_never_its_own_output() {
    let (flat, eq) = project(0.0);
    let (cut, _) = project(-18.0);
    let frames = 48_000u64 * 3;
    let a = capture(&flat, &eq, (0, frames));
    let b = capture(&cut, &eq, (0, frames));
    assert_eq!(a.sample_rate, 48_000);
    assert_eq!(a.left.len() as u64, frames);
    assert!(a.left.iter().any(|s| s.abs() > 0.01), "the synth is heard");
    // The EQ's own settings cannot change what it hears.
    assert!(
        a.left
            .iter()
            .zip(&b.left)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
    assert!(
        a.right
            .iter()
            .zip(&b.right)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}

#[test]
fn a_span_is_a_slice_of_the_same_samples() {
    let (doc, eq) = project(0.0);
    let all = capture(&doc, &eq, (0, 48_000 * 2));
    let part = capture(&doc, &eq, (24_000, 60_000));
    assert_eq!(part.left, all.left[24_000..60_000]);
    assert_eq!(part.right, all.right[24_000..60_000]);
}

#[test]
fn the_device_changes_the_bounce_but_not_its_own_input() {
    let (flat, eq) = project(0.0);
    let (cut, _) = project(-18.0);
    let bounce = |doc: &RackDoc| {
        common::render_guarded(doc, &default_registry(), &ProjectOptions::default()).unwrap()
    };
    let (x, y) = (bounce(&flat), bounce(&cut));
    assert_ne!(
        x.channels[0], y.channels[0],
        "the cut is audible in the bounce"
    );
    let a = capture(&flat, &eq, (0, 48_000));
    assert!(a.left.iter().any(|s| s.abs() > 0.01));
}

#[test]
fn a_device_with_nothing_at_its_input_is_refused() {
    let (doc, _) = project(0.0);
    let registry = default_registry();
    assert!(input_sources(&doc, &registry, &key("lead")).is_none());
    let err = render_device_input(
        &doc,
        &registry,
        (&ProjectOptions::default(), None),
        &mut PluginBounce::default(),
        &key("lead"),
        (0, 1_000),
        RenderControl::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("nothing is cabled"), "{err}");
    let eq = doc
        .devices
        .iter()
        .find(|d| d.type_id == EQ)
        .unwrap()
        .key
        .clone();
    assert!(input_sources(&doc, &registry, &eq).is_some());
    assert!(
        render_device_input(
            &doc,
            &registry,
            (&ProjectOptions::default(), None),
            &mut PluginBounce::default(),
            &eq,
            (10, 10),
            RenderControl::default(),
        )
        .is_err()
    );
}
