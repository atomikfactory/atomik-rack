//! `Combine`, `Uncombine`, `MoveIntoContainer` and `MoveOutOfContainer` (model side): the cable
//! splice through the container's sockets, the song track and console channel that follow a
//! combined instrument, every refusal, undo and redo, and the bypass switch of the expansion.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Cable, Command, ContainerCommand, DeviceKey, Document, ModelError, NoteSplit, RackDoc,
    SongCommand, SyncOp, empty_doc,
};
use common::container_ops::assert_invariants;
use common::song::{add_clip, add_track, note, notes_clip};
use common::*;
use std::collections::BTreeSet;

const DIST: &str = "atomik.distortion";
const COMPRESSOR: &str = "atomik.compressor";
const CONTAINER_KEY: &str = "container";

fn combine(keys: &[&str]) -> Command {
    Command::Container(ContainerCommand::Combine {
        keys: keys.iter().map(|k| key(k)).collect(),
        name: String::new(),
    })
}

fn uncombine(container: &str) -> Command {
    Command::Container(ContainerCommand::Uncombine {
        container: key(container),
    })
}

fn move_into(device: &str, container: &str, index: usize) -> Command {
    Command::Container(ContainerCommand::MoveIntoContainer {
        key: key(device),
        container: key(container),
        index,
    })
}

fn move_out(device: &str, index: usize) -> Command {
    Command::Container(ContainerCommand::MoveOutOfContainer {
        key: key(device),
        index,
    })
}

/// Applies `c`, which must be refused; the document is the same afterwards.
fn refused(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = d.doc().clone();
    let e = d.apply(c, r).expect_err("the command must be refused");
    assert_eq!(d.doc(), &before, "a refused command changes nothing");
    e
}

/// The flat graph of `doc` with the `~split` devices and cables left out and the path keys
/// below `prefix` read as top-level keys: the graph the devices had in the rack.
fn flat(doc: &RackDoc, r: &Registry, prefix: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let x = doc.expand(r);
    let strip = |k: &DeviceKey| k.0.strip_prefix(prefix).unwrap_or(&k.0).to_owned();
    let devices = x
        .devices
        .iter()
        .filter(|d| !d.key.0.ends_with("~split"))
        .map(|d| format!("{} {}", strip(&d.key), d.type_id))
        .collect();
    let cables = x
        .cables
        .iter()
        .filter(|c| !c.from.device.0.contains('~') && !c.to.device.0.contains('~'))
        .map(|c| {
            format!(
                "{}:{} -> {}:{}",
                strip(&c.from.device),
                c.from.socket,
                strip(&c.to.device),
                c.to.socket
            )
        })
        .collect();
    (devices, cables)
}

/// Undo gives back the document before and redo the one after, and both validate.
fn check_undo_redo(d: &mut Document, r: &Registry, before: &RackDoc) {
    let after = d.doc().clone();
    d.doc().validate().expect("the combined document is valid");
    assert_invariants(d.doc(), r);
    d.undo(r).expect("one undo step");
    assert_eq!(d.doc(), before, "undo restores the document exactly");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "redo restores the combined document");
}

fn inner<'d>(d: &'d Document, path: &str) -> &'d atomik_model::ContainerDoc {
    d.doc().inner(&key(path)).expect("a container")
}

// ------------------------------------------------------------------------- the splice

#[test]
fn combining_an_effect_chain_splices_its_cables_through_the_container() {
    let r = registry();
    let mut d = chain(&r);
    let before = d.doc().clone();
    let (devices, cables) = flat(&before, &r, "");
    d.apply(combine(&["synth", "delay"]), &r).unwrap();
    assert_eq!(keys(&d), [CONTAINER_KEY, "mixer"]);
    let c = d.doc().device(&key(CONTAINER_KEY)).unwrap();
    assert_eq!(c.type_id, CONTAINER);
    assert_eq!(c.name, "Container");
    let inside = inner(&d, CONTAINER_KEY);
    let local: Vec<&str> = inside.devices.iter().map(|x| x.key.0.as_str()).collect();
    assert_eq!(local, ["synth", "delay"]);
    // Nothing outside names a moved device any more; the container feeds the mixer.
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert!(
        outer.contains(&"container:out_l -> mixer:ch1_in_l".to_owned()),
        "{outer:?}"
    );
    assert!(
        outer.contains(&"container:out_r -> mixer:ch1_in_r".to_owned()),
        "{outer:?}"
    );
    assert!(
        outer
            .iter()
            .all(|c| !c.contains("synth:") && !c.contains("delay:"))
    );
    let inner_cables: Vec<String> = inside.cables.iter().map(ToString::to_string).collect();
    for expected in [
        "synth:out_l -> delay:in_l",
        "delay:out_l -> boundary:out_l",
        "delay:out_r -> boundary:out_r",
    ] {
        assert!(
            inner_cables.contains(&expected.to_owned()),
            "{expected}: {inner_cables:?}"
        );
    }
    // The expansion theorem at graph level: the flat graph is the one the rack had.
    assert_eq!(flat(d.doc(), &r, "container/"), (devices, cables));
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn a_cable_into_the_group_takes_the_first_container_input() {
    let r = registry();
    let mut d = chain(&r);
    let before = d.doc().clone();
    let (devices, cables) = flat(&before, &r, "");
    // The delay and the mixer move; the synth stays and feeds the container.
    d.apply(combine(&["delay", "mixer"]), &r).unwrap();
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert!(
        outer.contains(&"synth:out_l -> container:in_l".to_owned()),
        "{outer:?}"
    );
    assert!(
        outer.contains(&"synth:out_r -> container:in_r".to_owned()),
        "{outer:?}"
    );
    // The mixer's master goes to the interface through the container's outputs.
    assert!(
        outer.contains(&"container:out_l -> interface:out_1".to_owned()),
        "{outer:?}"
    );
    assert_eq!(flat(d.doc(), &r, "container/"), (devices, cables));
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn one_source_feeding_two_inputs_uses_one_container_input() {
    let r = registry();
    let mut d = three_devices(&r);
    run(
        &mut d,
        &r,
        [
            connect("synth:out_l", "delay:in_l"),
            connect("synth:out_l", "mixer:ch1_in_l"),
        ],
    );
    let before = d.doc().clone();
    d.apply(combine(&["delay", "mixer"]), &r).unwrap();
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert_eq!(outer, ["synth:out_l -> container:in_l"]);
    let inside: Vec<String> = inner(&d, CONTAINER_KEY)
        .cables
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        inside,
        [
            "boundary:in_l -> delay:in_l",
            "boundary:in_l -> mixer:ch1_in_l"
        ]
    );
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn a_notes_cable_into_the_group_becomes_the_notes_input_and_a_split() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, ARP, Some("arp")),
            add(1, SYNTH, Some("synth")),
            add(2, DELAY, Some("delay")),
            connect("arp:notes_out", "synth:notes_in"),
            connect("synth:out_l", "delay:in_l"),
        ],
    );
    let before = d.doc().clone();
    let (devices, cables) = flat(&before, &r, "");
    d.apply(combine(&["synth", "delay"]), &r).unwrap();
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert_eq!(outer, ["arp:notes_out -> container:notes_in"]);
    assert_eq!(
        inner(&d, CONTAINER_KEY).splits,
        [NoteSplit::full(key("synth"))]
    );
    let x = d.doc().expand(&r);
    let flat_cables: Vec<String> = x.cables.iter().map(ToString::to_string).collect();
    assert!(flat_cables.contains(&"arp:notes_out -> container/~split:notes_in".to_owned()));
    assert!(flat_cables.contains(&"container/~split:out_1 -> container/synth:notes_in".to_owned()));
    // Apart from the split, the flat graph is the rack's.
    let (d2, c2) = flat(d.doc(), &r, "container/");
    assert_eq!(d2, devices);
    let own: BTreeSet<String> = cables
        .iter()
        .filter(|c| !c.contains("notes"))
        .cloned()
        .collect();
    assert!(own.is_subset(&c2));
    check_undo_redo(&mut d, &r, &before);
}

// ------------------------------------------------------------------------- the refusals

#[test]
fn a_cable_the_container_has_no_socket_for_refuses_the_combine_and_names_it() {
    let r = registry();
    // CV out of a moving device.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, COMPRESSOR, Some("comp")),
            add(1, DIST, Some("dist")),
            connect("comp:gr_cv_out", "dist:drive_cv_in"),
        ],
    );
    let e = refused(&mut d, &r, combine(&["comp"]));
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    let text = e.to_string();
    assert!(
        text.contains("comp:gr_cv_out -> dist:drive_cv_in"),
        "{text}"
    );
    assert!(text.contains("no CV output"), "{text}");
    // CV into a moving device.
    let e = refused(&mut d, &r, combine(&["dist"]));
    assert!(e.to_string().contains("no CV input"), "{e}");
    // Together they have no CV cable crossing.
    d.apply(combine(&["comp", "dist"]), &r).unwrap();
    assert_eq!(inner(&d, CONTAINER_KEY).cables.len(), 1);
}

#[test]
fn a_third_audio_source_or_destination_refuses_the_combine_naming_each_cable() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add(2, SYNTH, Some("c")),
            add(3, MIXER, Some("mixer")),
            connect("a:out_l", "mixer:ch1_in_l"),
            connect("b:out_l", "mixer:ch1_in_r"),
            connect("c:out_l", "mixer:ch2_in_l"),
        ],
    );
    let e = refused(&mut d, &r, combine(&["mixer"]));
    let text = e.to_string();
    assert!(
        text.contains("c:out_l -> mixer:ch2_in_l needs a third audio input"),
        "{text}"
    );
    assert!(!text.contains("a:out_l"), "{text}");
    // Outputs: three moving outputs feed three outside inputs.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add(2, SYNTH, Some("c")),
            add(3, DELAY, Some("x")),
            add(4, DELAY, Some("y")),
            add(5, DELAY, Some("z")),
            connect("a:out_l", "x:in_l"),
            connect("b:out_l", "y:in_l"),
            connect("c:out_l", "z:in_l"),
        ],
    );
    let e = refused(&mut d, &r, combine(&["a", "b", "c"]));
    let text = e.to_string();
    assert!(
        text.contains("c:out_l -> z:in_l needs a third audio output"),
        "{text}"
    );
    // Two of them are fine, and the first goes out on the left.
    d.apply(combine(&["a", "b"]), &r).unwrap();
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert!(
        outer.contains(&"container:out_l -> x:in_l".to_owned()),
        "{outer:?}"
    );
    assert!(
        outer.contains(&"container:out_r -> y:in_l".to_owned()),
        "{outer:?}"
    );
}

#[test]
fn two_notes_sources_or_a_notes_output_that_leaves_refuse_the_combine() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, ARP, Some("arp")),
            add(1, ARP, Some("arp2")),
            add(2, SYNTH, Some("a")),
            add(3, SYNTH, Some("b")),
            connect("arp:notes_out", "a:notes_in"),
            connect("arp2:notes_out", "b:notes_in"),
        ],
    );
    let e = refused(&mut d, &r, combine(&["a", "b"]));
    assert!(
        e.to_string().contains("a container has one notes input"),
        "{e}"
    );
    // The player moves, the synth stays: a notes cable would have to leave.
    let e = refused(&mut d, &r, combine(&["arp"]));
    assert!(e.to_string().contains("no notes output"), "{e}");
    // Both players and both synths: the two cables stay inside.
    d.apply(combine(&["arp", "arp2", "a", "b"]), &r).unwrap();
    assert!(d.doc().cables.is_empty());
}

#[test]
fn what_a_container_cannot_hold_refuses_the_combine() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("synth"))]);
    let channel = d.doc().console.channels[0].channel.0.clone();
    let master = d.doc().console.master.clone().unwrap().0;
    for k in [channel.as_str(), master.as_str()] {
        let e = refused(&mut d, &r, combine(&[k]));
        assert!(matches!(e, ModelError::NotAllowedInContainer(_)), "{e:?}");
        assert!(
            e.to_string()
                .contains("cannot be placed inside a container")
        );
    }
    let e = refused(&mut d, &r, combine(&[]));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    let e = refused(&mut d, &r, combine(&["missing"]));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
    let e = refused(&mut d, &r, combine(&["c/inner"]));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
}

#[test]
fn a_group_over_the_device_limit_or_too_deep_is_refused() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    for i in 0..=atomik_model::MAX_INNER_DEVICES {
        d.apply(add(i, DELAY, Some(&format!("fx{i}"))), &r).unwrap();
    }
    let all: Vec<String> = keys(&d);
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let e = refused(&mut d, &r, combine(&all));
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e:?}");
    // Three levels of containers inside one more would be four.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("a")),
            add_inner("a", 0, CONTAINER, "b"),
            add_inner("a/b", 0, CONTAINER, "c"),
        ],
    );
    let e = refused(&mut d, &r, combine(&["a"]));
    assert!(matches!(e, ModelError::ContainerTooDeep(_)), "{e:?}");
}

#[test]
fn a_device_a_macro_bank_drives_cannot_be_combined() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, DELAY, Some("delay"))]);
    d.apply(
        Command::Macro(atomik_model::MacroCommand::Assign {
            bank: None,
            index: 0,
            device: key("delay"),
            param: "mix".into(),
            min: None,
            max: None,
            invert: false,
        }),
        &r,
    )
    .unwrap();
    let e = refused(&mut d, &r, combine(&["delay"]));
    assert!(matches!(e, ModelError::BadMacro(_)), "{e:?}");
    assert!(e.to_string().contains("remove the assignment first"), "{e}");
}

// ------------------------------------------------------------------------- song and console

/// A synth with a console channel and a song track, and a delay: the rack of the roles tests.
fn instrument(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, DELAY, Some("delay")),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(
                    0,
                    1920,
                    vec![note(0, 480, 60, 0.8), note(480, 480, 64, 0.7)],
                ),
            ),
        ],
    );
    d
}

#[test]
fn the_track_and_the_channel_of_a_combined_instrument_follow_the_container() {
    let r = console_registry();
    let mut d = instrument(&r);
    let before = d.doc().clone();
    assert_eq!(d.doc().console.channels.len(), 1);
    d.apply(combine(&["synth", "delay"]), &r).unwrap();
    let c = key(CONTAINER_KEY);
    assert_eq!(d.doc().console.channels[0].source.as_ref(), Some(&c));
    let track = &d.doc().song.tracks[0];
    assert_eq!(track.device, c);
    assert_eq!(track.lanes.len(), 1, "the note lane stays");
    assert_eq!(track.lanes[0].clips, before.song.tracks[0].lanes[0].clips);
    // The song's notes enter through the container's split to the synth.
    assert_eq!(
        inner(&d, CONTAINER_KEY).splits,
        [NoteSplit::full(key("synth"))]
    );
    check_undo_redo(&mut d, &r, &before);
}

#[test]
fn combine_then_uncombine_gives_the_rack_back() {
    let r = console_registry();
    let mut d = instrument(&r);
    let before = d.doc().clone();
    d.apply(combine(&["synth", "delay"]), &r).unwrap();
    d.apply(uncombine(CONTAINER_KEY), &r).unwrap();
    assert_eq!(d.doc(), &before);
    assert!(d.doc().validate().is_ok());
    // Both steps undo one by one.
    d.undo(&r).unwrap();
    assert!(d.doc().has_containers());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_automation_lanes_of_the_combined_instrument_are_dropped_with_a_note() {
    let r = console_registry();
    let mut d = instrument(&r);
    run(
        &mut d,
        &r,
        [Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        })],
    );
    let before = d.doc().clone();
    let command = combine(&["synth"]);
    let notes = d
        .doc()
        .container_move_notes(
            &r,
            match &command {
                Command::Container(c) => c,
                _ => unreachable!(),
            },
        )
        .unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("Dropped 1 automation lane of \"Poly Synth\""),
        "{notes:?}"
    );
    d.apply(command, &r).unwrap();
    assert_eq!(d.doc().song.tracks[0].lanes.len(), 1);
    // The lane is back with the undo.
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_player_stack_above_the_instrument_becomes_the_splits_and_comes_back() {
    let r = console_registry();
    let mut d = instrument(&r);
    let player = player_above_synth(&mut d, &r);
    let before = d.doc().clone();
    assert_eq!(before.song.tracks[0].notes_to.as_ref(), Some(&player));
    d.apply(combine(&[player.0.as_str(), "synth"]), &r).unwrap();
    let track = &d.doc().song.tracks[0];
    assert_eq!(
        track.notes_to, None,
        "the container's splits are the entry now"
    );
    assert_eq!(
        inner(&d, CONTAINER_KEY).splits,
        [NoteSplit::full(player.clone())]
    );
    d.doc().validate().unwrap();
    d.apply(uncombine(CONTAINER_KEY), &r).unwrap();
    assert_eq!(d.doc().song.tracks[0].device, key("synth"));
    assert_eq!(d.doc().song.tracks[0].notes_to.as_ref(), Some(&player));
    assert_eq!(
        d.doc().console.channels[0].source.as_ref(),
        Some(&key("synth"))
    );
}

/// Puts an arpeggiator above the synth of [`instrument`] (the track enters at it) and returns
/// its key.
fn player_above_synth(d: &mut Document, r: &Registry) -> DeviceKey {
    d.apply(
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: ARP.into(),
            target: atomik_model::CreateTarget::Selected(key("synth")),
            options: atomik_model::CreateOptions::default(),
        }),
        r,
    )
    .unwrap();
    d.doc().song.tracks[0]
        .notes_to
        .clone()
        .expect("the track enters at the player")
}

#[test]
fn two_devices_that_each_have_a_track_or_channel_cannot_share_a_container() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("one")), add(1, SYNTH, Some("two"))],
    );
    let e = refused(&mut d, &r, combine(&["one", "two"]));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    assert!(e.to_string().contains("Combine keeps one track"), "{e}");
    assert!(
        e.to_string()
            .contains("\"Poly Synth\" and \"Poly Synth 2\""),
        "{e}"
    );
}

#[test]
fn a_track_that_sends_its_notes_into_the_group_from_outside_is_refused() {
    let r = console_registry();
    let mut d = instrument(&r);
    let player = player_above_synth(&mut d, &r);
    // The player moves, the track's instrument stays: the track would lose its entry.
    let e = refused(&mut d, &r, combine(&[player.0.as_str(), "delay"]));
    assert!(e.to_string().contains("sends its notes to"), "{e}");
}

// ------------------------------------------------------------------------- uncombine

#[test]
fn uncombine_joins_the_container_cables_to_the_devices_and_refuses_a_narrow_split() {
    let r = registry();
    let mut d = chain(&r);
    run(
        &mut d,
        &r,
        [
            add(0, ARP, Some("arp")),
            connect("arp:notes_out", "synth:notes_in"),
        ],
    );
    let before = d.doc().clone();
    d.apply(combine(&["synth", "delay"]), &r).unwrap();
    // A narrowed split cannot be dissolved into a plain cable.
    d.apply(
        set_splits(
            CONTAINER_KEY,
            vec![NoteSplit {
                key_hi: 59,
                ..NoteSplit::full(key("synth"))
            }],
        ),
        &r,
    )
    .unwrap();
    let e = refused(&mut d, &r, uncombine(CONTAINER_KEY));
    assert!(e.to_string().contains("narrows or transposes"), "{e}");
    d.undo(&r).unwrap();
    d.apply(uncombine(CONTAINER_KEY), &r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn uncombine_refuses_a_track_that_would_play_several_devices() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "low"),
            add_inner("c", 1, SYNTH, "high"),
            set_splits("c", vec![split("low", 0, 127, 0), split("high", 0, 127, 0)]),
            add_track("c"),
        ],
    );
    // Two instruments, neither stands out as the one the track plays.
    let e = refused(&mut d, &r, uncombine("c"));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    assert!(e.to_string().contains("stands out"), "{e}");
    // With one instrument the track plays it, and a second split on an effect is a second
    // device to play.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, "atomik.note-filter", "filter"),
            set_splits(
                "c",
                vec![split("synth", 0, 127, 0), split("filter", 0, 127, 0)],
            ),
            add_track("c"),
        ],
    );
    let e = refused(&mut d, &r, uncombine("c"));
    assert!(
        e.to_string().contains("plays 2 devices through splits"),
        "{e}"
    );
}

#[test]
fn uncombine_drops_the_macros_with_a_note() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "delay"),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
        ],
    );
    let command = ContainerCommand::Uncombine {
        container: key("c"),
    };
    let notes = d.doc().container_move_notes(&r, &command).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("macros"), "{notes:?}");
}

// ------------------------------------------------------------------------- moving in and out

#[test]
fn a_device_moves_into_an_existing_container_and_out_again() {
    let r = registry();
    let mut d = three_devices(&r);
    // The delay and the synth are separate chains into the mixer.
    run(
        &mut d,
        &r,
        [
            connect("synth:out_l", "mixer:ch1_in_l"),
            connect("delay:out_l", "mixer:ch2_in_l"),
        ],
    );
    d.apply(combine(&["synth"]), &r).unwrap();
    let before = d.doc().clone();
    let (devices, cables) = flat(&before, &r, "container/");
    // The delay joins the synth inside: its output leaves on the container's free output.
    d.apply(move_into("delay", CONTAINER_KEY, 1), &r).unwrap();
    assert_eq!(keys(&d), [CONTAINER_KEY, "mixer"]);
    let local: Vec<&str> = inner(&d, CONTAINER_KEY)
        .devices
        .iter()
        .map(|x| x.key.0.as_str())
        .collect();
    assert_eq!(local, ["synth", "delay"]);
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert!(
        outer.contains(&"container:out_r -> mixer:ch2_in_l".to_owned()),
        "{outer:?}"
    );
    assert_invariants(d.doc(), &r);
    d.doc().validate().unwrap();
    assert_eq!(flat(d.doc(), &r, "container/"), (devices, cables));
    // Out again: the delay is back in the rack and its cable reconnects.
    d.apply(move_out("container/delay", 1), &r).unwrap();
    assert_eq!(keys(&d), [CONTAINER_KEY, "delay", "mixer"]);
    let outer: Vec<String> = d.doc().cables.iter().map(ToString::to_string).collect();
    assert!(
        outer.contains(&"delay:out_l -> mixer:ch2_in_l".to_owned()),
        "{outer:?}"
    );
    d.doc().validate().unwrap();
    assert_invariants(d.doc(), &r);
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_device_with_a_track_is_refused_by_move_into() {
    let r = console_registry();
    let mut d = instrument(&r);
    run(&mut d, &r, [add(0, CONTAINER, Some("box"))]);
    let e = refused(&mut d, &r, move_into("synth", "box", 0));
    assert!(matches!(e, ModelError::BadScope(_)), "{e:?}");
    assert!(
        e.to_string().contains("combine it into a new container"),
        "{e}"
    );
}

#[test]
fn moving_out_a_device_cabled_to_one_that_stays_is_refused() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, DELAY, "delay"),
            connect("c/synth:out_l", "c/delay:in_l"),
        ],
    );
    let e = refused(&mut d, &r, move_out("c/synth", 0));
    assert!(e.to_string().contains("leaves with one that stays"), "{e}");
}

#[test]
fn moving_out_reconnects_the_boundary_cables_directly() {
    let r = registry();
    let mut d = three_devices(&r);
    run(
        &mut d,
        &r,
        [
            connect("synth:out_l", "delay:in_l"),
            connect("delay:out_l", "mixer:ch1_in_l"),
        ],
    );
    let before = d.doc().clone();
    let (devices, cables) = flat(&before, &r, "");
    d.apply(combine(&["delay"]), &r).unwrap();
    d.apply(move_out("container/delay", 1), &r).unwrap();
    // The delay is a top-level device again, cabled as it was; the container keeps the cable
    // into its (now unused) input.
    let mut outer = d.doc().cables.clone();
    outer.retain(|c| c.to != sock("container:in_l"));
    assert_eq!(outer, before.cables);
    assert!(d.doc().device(&key("delay")).is_some());
    let (devices2, cables2) = flat(d.doc(), &r, "container/");
    assert_eq!((devices2, cables2), (devices, cables));
    d.doc().validate().unwrap();
}

// ------------------------------------------------------------------------- bypass

fn bypass_demo(r: &Registry) -> Document {
    let mut d = chain(r);
    d.apply(combine(&["delay"]), r).unwrap();
    d
}

#[test]
fn a_bypassed_container_expands_to_a_direct_in_to_out_path() {
    let r = registry();
    let mut d = bypass_demo(&r);
    let flat_on = d.doc().expand(&r);
    assert!(flat_on.devices.iter().any(|x| x.key.0 == "container/delay"));
    d.apply(
        Command::SetParam {
            key: key(CONTAINER_KEY),
            param: "bypass".into(),
            value: 1.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    assert!(d.doc().container_bypassed(&key(CONTAINER_KEY)));
    let off = d.doc().expand(&r);
    let names: Vec<&str> = off.devices.iter().map(|x| x.key.0.as_str()).collect();
    assert!(!names.contains(&"container/delay"), "{names:?}");
    assert!(names.contains(&"container/~split"), "{names:?}");
    let cables: Vec<String> = off.cables.iter().map(ToString::to_string).collect();
    // The synth feeds the mixer directly.
    assert!(
        cables.contains(&"synth:out_l -> mixer:ch1_in_l".to_owned()),
        "{cables:?}"
    );
    assert!(
        cables.contains(&"synth:out_r -> mixer:ch1_in_r".to_owned()),
        "{cables:?}"
    );
    assert!(
        cables.iter().all(|c| !c.contains("container/delay")),
        "{cables:?}"
    );
    // Back on: the inside returns.
    d.apply(
        Command::SetParam {
            key: key(CONTAINER_KEY),
            param: "bypass".into(),
            value: 0.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().expand(&r), flat_on);
    // The switch survives a save.
    d.apply(
        Command::SetParam {
            key: key(CONTAINER_KEY),
            param: "bypass".into(),
            value: 1.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    let text = atomik_model::save_project(d.doc());
    assert!(text.contains("\"bypass\": 1"), "{text}");
    let loaded = atomik_model::load_project(&text, &r).unwrap();
    assert_eq!(&loaded, d.doc());
}

#[test]
fn a_container_with_nothing_to_move_reports_no_notes() {
    let r = registry();
    let d = chain(&r);
    let command = ContainerCommand::Combine {
        keys: vec![key("delay")],
        name: String::new(),
    };
    assert!(
        d.doc()
            .container_move_notes(&r, &command)
            .unwrap()
            .is_empty()
    );
    let other = ContainerCommand::DuplicateContainer {
        container: key("c"),
    };
    assert!(d.doc().container_move_notes(&r, &other).unwrap().is_empty());
}

#[test]
fn the_sync_ops_of_a_combine_announce_the_container_once_and_remove_each_device() {
    let r = registry();
    let mut d = chain(&r);
    let ops = d.apply(combine(&["synth", "delay"]), &r).unwrap();
    let added: Vec<&DeviceKey> = ops
        .iter()
        .filter_map(|o| match o {
            SyncOp::DeviceAdded(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(added, [&key(CONTAINER_KEY)]);
    let removed: Vec<&DeviceKey> = ops
        .iter()
        .filter_map(|o| match o {
            SyncOp::DeviceRemoved(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(removed, [&key("synth"), &key("delay")]);
    let _ = Cable {
        from: sock("a:b"),
        to: sock("c:d"),
    };
}

// ------------------------------------------------------------------------- the property

/// A rack of nine devices with audio, notes and CV cables, with or without the console and a
/// song track on the first synth.
fn property_rack(r: &Registry, console: bool) -> Document {
    let mut d = Document::new(empty_doc());
    let devices = [
        (ARP, "arp"),
        (SYNTH, "s1"),
        (SYNTH, "s2"),
        (DELAY, "d1"),
        (DELAY, "d2"),
        (COMPRESSOR, "c1"),
        (DIST, "dist"),
        (MIXER, "mix"),
    ];
    for (i, (type_id, k)) in devices.into_iter().enumerate() {
        d.apply(add(i, type_id, Some(k)), r).unwrap();
    }
    let cables = [
        ("arp:notes_out", "s1:notes_in"),
        ("s1:out_l", "d1:in_l"),
        ("s1:out_r", "d1:in_r"),
        ("d1:out_l", "c1:in_l"),
        ("d1:out_r", "c1:in_r"),
        ("c1:out_l", "mix:ch1_in_l"),
        ("c1:out_r", "mix:ch1_in_r"),
        ("s2:out_l", "d2:in_l"),
        ("d2:out_l", "mix:ch2_in_l"),
        ("c1:gr_cv_out", "dist:drive_cv_in"),
    ];
    for (f, t) in cables {
        d.apply(connect(f, t), r).unwrap();
    }
    if console {
        d.apply(add_track("s1"), r).unwrap();
        d.apply(
            add_clip("s1", 0, notes_clip(0, 1920, vec![note(0, 480, 60, 0.8)])),
            r,
        )
        .unwrap();
    } else {
        for (f, t) in [
            ("mix:master_l", "interface:out_1"),
            ("mix:master_r", "interface:out_2"),
        ] {
            d.apply(connect(f, t), r).unwrap();
        }
    }
    Document::new(d.doc().clone())
}

fn sorted(doc: &RackDoc) -> RackDoc {
    let mut d = doc.clone();
    d.devices.sort_by(|a, b| a.key.cmp(&b.key));
    d
}

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Any selection of up to eight devices either combines, keeps the rack's flat graph and
    /// uncombines to the original rack (up to the order of the devices), or is refused with
    /// the document untouched.
    #[test]
    fn a_selection_combines_and_uncombines_to_the_original_or_is_refused(
        picks in proptest::collection::vec(0usize..8, 1..=8),
        console in any::<bool>(),
    ) {
        let r = if console { console_registry() } else { registry() };
        let mut d = property_rack(&r, console);
        let original = d.doc().clone();
        let names: Vec<String> = keys(&d);
        let mut chosen: Vec<DeviceKey> = Vec::new();
        for p in picks {
            let k = key(&names[p % names.len()]);
            if !chosen.contains(&k) {
                chosen.push(k);
            }
        }
        let selection: Vec<&str> = chosen.iter().map(|k| k.0.as_str()).collect();
        let before_graph = flat(&original, &r, "");
        match d.apply(combine(&selection), &r) {
            Err(e) => {
                prop_assert!(!matches!(e, ModelError::Invariant(_)), "{e}");
                prop_assert_eq!(d.doc(), &original);
            }
            Ok(_) => {
                prop_assert!(d.doc().validate().is_ok());
                prop_assert!(d.doc().check_containers(&r).is_ok());
                let (devices, cables) = flat(d.doc(), &r, "container/");
                prop_assert_eq!(&devices, &before_graph.0);
                // Without the notes cables (they pass the container's split) the same cables.
                let plain = |s: &BTreeSet<String>| -> BTreeSet<String> {
                    s.iter().filter(|c| !c.contains("notes")).cloned().collect()
                };
                prop_assert_eq!(plain(&cables), plain(&before_graph.1));
                let combined = d.doc().clone();
                match d.apply(uncombine(CONTAINER_KEY), &r) {
                    Ok(_) => {
                        prop_assert!(d.doc().validate().is_ok());
                        prop_assert_eq!(sorted(d.doc()), sorted(&original));
                    }
                    Err(e) => {
                        prop_assert!(!matches!(e, ModelError::Invariant(_)), "{e}");
                        prop_assert_eq!(d.doc(), &combined);
                    }
                }
                // Undo of everything is the original, exactly.
                while d.undo(&r).is_some() {}
                prop_assert_eq!(d.doc(), &original);
            }
        }
    }
}
