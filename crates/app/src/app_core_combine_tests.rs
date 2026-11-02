//! Combine and Uncombine in the app core: the selection goes into a container in one undo step,
//! the engine follows (the live render equals the bounce before and after, through undo, redo,
//! and the container's bypass switch), and what a combine drops is told.

use super::*;
use crate::offline_sink::OfflineSink;
use atomik_model::{
    Clip, ClipContent, ClipId, ConsoleCommand, InsertEffect, Note, SongCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const LATENCY: usize = atomik_core::BATCH;
const BAR: i64 = 4 * atomik_core::PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply<S: EngineSink>(c: &mut AppCore<S>, command: Command) {
    c.apply_command(command.clone())
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

fn set<S: EngineSink>(c: &mut AppCore<S>, k: &str, param: &str, value: f32) {
    apply(
        c,
        Command::SetParam {
            key: key(k),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

/// A synth with its console channel and song track, and a delay on the channel's first insert.
fn core() -> AppCore<OfflineSink> {
    let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    for (type_id, k) in [(SYNTH, "synth"), (DELAY, "delay")] {
        apply(
            &mut c,
            Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.into(),
                key: Some(key(k)),
            },
        );
    }
    let channel = c.doc().console.channels[0].channel.clone();
    apply(
        &mut c,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::Existing(key("delay"))),
        }),
    );
    for (p, v) in [
        ("osc1.wave", 1.0),
        ("osc.mix", 0.0),
        ("env.amp.attack", 8.0),
        ("env.amp.release", 180.0),
    ] {
        set(&mut c, "synth", p, v);
    }
    set(&mut c, "delay", "mix", 0.35);
    apply(
        &mut c,
        Command::Song(SongCommand::AddTrack {
            device: key("synth"),
        }),
    );
    let notes = [(0, 2, 48), (2, 2, 55), (4, 2, 52)]
        .iter()
        .map(|&(beat, beats, k)| {
            Note::new(beat * atomik_core::PPQ, beats * atomik_core::PPQ, k, 0.8)
        })
        .collect();
    apply(
        &mut c,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
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
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }),
    );
    apply(&mut c, Command::Song(SongCommand::SetSongEnd(BAR)));
    c
}

fn select<S: EngineSink>(c: &mut AppCore<S>, keys: &[&str]) {
    let keys: Vec<DeviceKey> = keys.iter().map(|k| key(k)).collect();
    c.view_mut().set_selection(&keys, keys.last());
}

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
    )
    .unwrap()
    .channels
}

fn play(c: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
    c.transport_action(atomik_ui::TransportAction::Play);
    c.sink_mut().render_blocks(frames + LATENCY, block)
}

fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
    live.iter().zip(bounce).all(|(a, b)| {
        a[LATENCY..]
            .iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits())
            && a[..LATENCY].iter().all(|x| *x == 0.0)
    })
}

#[test]
fn the_reference_project_is_audible() {
    let c = core();
    let reference = bounce_of(c.doc());
    assert!(reference.iter().flatten().any(|s| s.abs() > 1e-3));
}

#[test]
fn combine_wraps_the_selection_in_one_undo_step_and_selects_the_container() {
    let mut c = core();
    let before = c.doc().clone();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let container = key("container");
    assert!(c.doc().inner(&container).is_some());
    assert_eq!(c.view().selection(), std::slice::from_ref(&container));
    assert!(c.undo());
    assert_eq!(c.doc(), &before, "one undo step");
    assert!(c.redo());
    assert!(c.doc().inner(&container).is_some());
}

#[test]
fn the_live_render_equals_the_bounce_before_and_after_a_combine() {
    let reference = bounce_of(core().doc());
    for (selection, block) in [
        (vec!["delay"], 512),
        (vec!["synth"], 480),
        (vec!["delay"], 64),
    ] {
        let mut c = core();
        select(&mut c, &selection);
        assert!(c.combine_selection(), "{selection:?}");
        assert!(bounce_of(c.doc()) == reference, "{selection:?}: bounce");
        let live = play(&mut c, reference[0].len(), block);
        assert!(same_bits(&live, &reference), "{selection:?}, block {block}");
    }
}

#[test]
fn the_live_render_equals_the_bounce_after_undo_redo_and_uncombine() {
    let reference = bounce_of(core().doc());
    // Undo.
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    assert!(c.undo());
    let live = play(&mut c, reference[0].len(), 512);
    assert!(same_bits(&live, &reference), "undo");
    // Redo.
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    assert!(c.undo());
    assert!(c.redo());
    let live = play(&mut c, reference[0].len(), 512);
    assert!(same_bits(&live, &reference), "redo");
    // Uncombine: the delay is a top-level device again, selected.
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    assert!(c.uncombine_selection());
    assert!(c.doc().device(&key("delay")).is_some());
    assert!(!c.doc().has_containers());
    assert_eq!(c.view().selection(), [key("delay")]);
    let live = play(&mut c, reference[0].len(), 480);
    assert!(same_bits(&live, &reference), "uncombine");
}

#[test]
fn the_bypass_switch_follows_in_the_engine_both_ways_and_through_undo() {
    let reference = bounce_of(core().doc());
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    set(&mut c, "container", "bypass", 1.0);
    let bypassed = bounce_of(c.doc());
    assert!(bypassed != reference, "the delay is audible");
    let live = play(&mut c, reference[0].len(), 512);
    assert!(same_bits(&live, &bypassed), "bypassed");
    // Switched off again (a fresh engine run of the same edits).
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    set(&mut c, "container", "bypass", 1.0);
    set(&mut c, "container", "bypass", 0.0);
    let live = play(&mut c, reference[0].len(), 512);
    assert!(same_bits(&live, &reference), "bypass off");
    // Undo of the switch.
    let mut c = core();
    select(&mut c, &["delay"]);
    assert!(c.combine_selection());
    set(&mut c, "container", "bypass", 1.0);
    assert!(c.undo());
    let live = play(&mut c, reference[0].len(), 64);
    assert!(same_bits(&live, &reference), "undo of the bypass");
    // The same through redo.
    assert!(c.redo());
    assert!(c.doc().container_bypassed(&key("container")));
}

#[test]
fn what_a_combine_dropped_is_told_and_a_refusal_is_reported() {
    let mut c = core();
    apply(
        &mut c,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
    );
    select(&mut c, &["synth"]);
    assert!(c.combine_selection());
    let told = c.last_rejection().expect("the dropped lane is told");
    assert!(told.contains("Dropped 1 automation lane"), "{told}");
    // Nothing selected, nothing to combine; no container selected, nothing to uncombine.
    let mut c = core();
    select(&mut c, &[]);
    assert!(!c.combine_selection());
    assert!(c.last_rejection().unwrap().contains("Select the devices"));
    select(&mut c, &["delay"]);
    assert!(!c.uncombine_selection());
    assert!(c.last_rejection().unwrap().contains("Select a container"));
    // The console devices cannot go in a container: the model's reason is shown.
    let channel = c.doc().console.channels[0].channel.clone();
    select(&mut c, &[channel.0.as_str()]);
    let before = c.doc().clone();
    assert!(!c.combine_selection());
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("cannot be placed inside a container")
    );
    assert_eq!(c.doc(), &before);
}

/// [`core`] plus a second instrument with its own channel and song track.
fn core_with_two_instruments() -> AppCore<OfflineSink> {
    let mut c = core();
    apply(
        &mut c,
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key("synth2")),
        },
    );
    apply(
        &mut c,
        Command::Song(SongCommand::AddTrack {
            device: key("synth2"),
        }),
    );
    c
}

#[test]
fn the_edit_menu_dims_combine_for_two_instruments_and_a_key_says_which() {
    let mut c = core_with_two_instruments();
    select(&mut c, &["synth", "synth2"]);
    assert_eq!(c.combine_menu(), (false, false));
    let before = c.doc().clone();
    assert!(!c.combine_selection());
    let why = c.last_rejection().unwrap();
    assert!(why.contains("\"Poly Synth\" and \"Poly Synth 2\""), "{why}");
    assert!(why.contains("combine them separately"), "{why}");
    assert_eq!(c.doc(), &before);
    // One instrument alone is fine, and the answer follows the selection and the document.
    select(&mut c, &["synth"]);
    assert_eq!(c.combine_menu(), (true, false));
    select(&mut c, &["synth", "synth2"]);
    assert_eq!(c.combine_menu(), (false, false));
    select(&mut c, &[]);
    assert_eq!(c.combine_menu(), (false, false));
}
