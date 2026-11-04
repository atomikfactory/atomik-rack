//! The allocation guard around a project bounce: `render_guarded` wraps every engine render
//! call, the wrapper is called once per batch, and an allocation inside it is caught.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::plugin_pass::with_render_guard;
use atomik_render::project::{ProjectOptions, render_doc};
use std::cell::Cell;

fn project() -> atomik_model::RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let synth = DeviceKey("synth".into());
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
        Command::Song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
        Command::Song(SongCommand::AddClip {
            device: synth,
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, PPQ, 69, 1.0)].into()),
            },
        }),
    ] {
        d.apply(c, &registry).expect("a valid command");
    }
    d.doc().clone()
}

thread_local! {
    /// The guard calls of this test's thread (the bounce runs on it).
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn the_guard_is_called_around_every_batch_of_a_bounce() {
    let doc = project();
    CALLS.set(0);
    let rendered = with_render_guard(
        |render| {
            CALLS.set(CALLS.get() + 1);
            render();
        },
        || render_doc(&doc, &default_registry(), &ProjectOptions::default()).unwrap(),
    );
    let batches = CALLS.get();
    assert!(batches > 10, "{batches} batches");
    // Every output frame came from a guarded batch (the pre-roll is not counted).
    assert!(batches * atomik_core::BATCH >= rendered.channels[0].len());
}

#[test]
fn a_guarded_bounce_matches_an_unguarded_one() {
    let doc = project();
    let (options, registry) = (ProjectOptions::default(), default_registry());
    let plain = render_doc(&doc, &registry, &options).unwrap();
    let guarded = common::render_guarded(&doc, &registry, &options).unwrap();
    assert_eq!(plain.channels, guarded.channels);
}

#[test]
#[should_panic(expected = "HI-1 violated")]
fn an_allocation_inside_a_guarded_render_call_is_caught() {
    let doc = project();
    let _ = with_render_guard(
        |render| {
            atomik_testkit::no_alloc::assert_no_alloc(|| {
                render();
                drop(std::hint::black_box(Box::new(7_u64)));
            });
        },
        || render_doc(&doc, &default_registry(), &ProjectOptions::default()),
    );
}

#[test]
fn without_a_guard_the_loop_renders_directly() {
    let doc = project();
    CALLS.set(0);
    let _ = render_doc(&doc, &default_registry(), &ProjectOptions::default()).unwrap();
    assert_eq!(CALLS.get(), 0);
}
