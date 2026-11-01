//! Mix channels, parallel channels and the follow links of the sends (model half): creation,
//! deletion and undo, the names and the cascade of the parallel channels, the solo rule, the
//! follow links the routing commands keep, and the saved file with and without the new fields.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::{
    FOLLOW_IN, FOLLOW_OUT, LinkKind, OutputDest, PARALLEL_OUT, link_kind, output_dest,
};
use atomik_model::{
    Command, ConsoleCommand, Document, RackDoc, load_project, new_project_doc, save_project,
};
use common::console_ops::assert_console_invariants;
use common::*;

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn mix(name: &str) -> Command {
    con(ConsoleCommand::AddMixChannel { name: name.into() })
}

fn parallel(strip: &str) -> Command {
    con(ConsoleCommand::CreateParallel { strip: key(strip) })
}

fn create_bus(name: &str, dest: OutputDest) -> Command {
    con(ConsoleCommand::CreateBus {
        name: name.into(),
        dest,
    })
}

fn set_output(strip: &str, dest: OutputDest) -> Command {
    con(ConsoleCommand::SetOutput {
        strip: key(strip),
        dest,
    })
}

fn bus(name: &str) -> OutputDest {
    OutputDest::Bus(key(name))
}

/// The starter with two synths and their channels `channel` and `channel-2`.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, SYNTH, Some("b")),
        ],
    );
    (d, r)
}

/// Applies `c` and checks it is exactly one undo step.
fn one_step(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    assert_console_invariants(&after);
    d.undo(r).expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

fn name_of(doc: &RackDoc, k: &str) -> String {
    doc.device(&key(k)).expect("device").name.clone()
}

fn follow_links(doc: &RackDoc) -> Vec<String> {
    doc.cables
        .iter()
        .filter(|c| link_kind(doc, c) == Some(LinkKind::FollowLink))
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect()
}

fn into_socket(doc: &RackDoc, device: &str, socket: &str) -> Vec<String> {
    doc.cables
        .iter()
        .filter(|c| c.to.device.as_str() == device && c.to.socket == socket)
        .map(|c| c.from.to_string())
        .collect()
}

fn out_of(doc: &RackDoc, device: &str, socket: &str) -> Vec<String> {
    doc.cables
        .iter()
        .filter(|c| c.from.device.as_str() == device && c.from.socket == socket)
        .map(|c| c.to.to_string())
        .collect()
}

// ---- Mix channel ---------------------------------------------------------------------------

#[test]
fn a_mix_channel_has_no_source_and_the_wiring_of_any_channel() {
    let (mut d, r) = project();
    one_step(&mut d, &r, mix("Drums bus in"));
    let doc = d.doc();
    let channel = doc.console.channels.last().expect("the new channel");
    assert_eq!(channel.source, None);
    let k = channel.channel.as_str().to_owned();
    assert_eq!(name_of(doc, &k), "Drums bus in");
    assert_eq!(out_of(doc, &k, "out_l"), ["master:bus_l"]);
    assert_eq!(out_of(doc, &k, "out_r"), ["master:bus_r"]);
    for n in 1..=8 {
        assert_eq!(
            out_of(doc, &k, &format!("send{n}_l")),
            [format!("master:send_bus{n}_l")]
        );
    }
    // The Direct In is a plain cable target.
    assert!(into_socket(doc, &k, "in_l").is_empty());
    assert_eq!(doc.console.channels.len(), 3);
    assert_console_invariants(doc);
}

#[test]
fn an_empty_name_gets_the_default_and_names_do_not_collide() {
    let (mut d, r) = project();
    run(&mut d, &r, [mix(""), mix("  ")]);
    let names: Vec<String> = d
        .doc()
        .console
        .channels
        .iter()
        .filter(|c| c.source.is_none())
        .map(|c| name_of(d.doc(), c.channel.as_str()))
        .collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1]);
    assert!(
        names.iter().all(|n| n.starts_with("Mix Channel")),
        "{names:?}"
    );
}

#[test]
fn a_mix_channel_takes_a_cable_in_and_is_deleted_with_its_cables() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [add(usize::MAX, SYNTH, Some("lead")), mix("In")],
    );
    let m = d.doc().console.channels.last().unwrap().channel.clone();
    // The synth made its own channel; free its outputs for the mix channel's Direct In.
    run(
        &mut d,
        &r,
        [Command::Disconnect {
            to: sock("channel-3:in_l"),
        }],
    );
    run(
        &mut d,
        &r,
        [connect("lead:out_l", &format!("{}:in_l", m.as_str()))],
    );
    assert_eq!(
        into_socket(d.doc(), m.as_str(), "in_l"),
        ["lead:out_l"].map(String::from)
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RemoveChannel { channel: m.clone() }),
    );
    assert!(d.doc().device(&m).is_none());
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device != m && c.to.device != m)
    );
}

#[test]
fn the_first_mix_channel_creates_the_master() {
    let r = console_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    assert!(d.doc().console.master.is_none());
    one_step(&mut d, &r, mix("Only"));
    assert!(d.doc().console.master.is_some());
    assert_eq!(d.doc().console.channels.len(), 1);
}

// ---- Parallel channels ---------------------------------------------------------------------

#[test]
fn a_parallel_channel_hangs_off_the_parallel_out_and_is_named_after_its_strip() {
    let (mut d, r) = project();
    one_step(&mut d, &r, parallel("channel"));
    let doc = d.doc();
    let (parent, child) = doc.console.parallels[0].clone();
    assert_eq!(parent, key("channel"));
    assert_eq!(
        name_of(doc, child.as_str()),
        format!("P1: {}", name_of(doc, "channel"))
    );
    assert_eq!(
        doc.console.channel_of(&key("a")).map(|c| &c.channel),
        Some(&key("channel"))
    );
    assert_eq!(doc.console.channels.last().unwrap().source, None);
    for (side, out) in ["l", "r"].into_iter().zip(PARALLEL_OUT) {
        assert_eq!(
            out_of(doc, "channel", out),
            [format!("{}:in_{side}", child.as_str())]
        );
    }
    assert_eq!(output_dest(doc, &child), OutputDest::Master);
    // The main output of the strip is untouched.
    assert_eq!(out_of(doc, "channel", "out_l"), ["master:bus_l"]);
}

#[test]
fn a_second_copy_counts_on_and_a_copy_can_have_copies() {
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel"), parallel("channel")]);
    let kids: Vec<_> = d
        .doc()
        .console
        .parallels_of(&key("channel"))
        .cloned()
        .collect();
    assert_eq!(kids.len(), 2);
    let base = name_of(d.doc(), "channel");
    assert_eq!(name_of(d.doc(), kids[0].as_str()), format!("P1: {base}"));
    assert_eq!(name_of(d.doc(), kids[1].as_str()), format!("P2: {base}"));
    run(&mut d, &r, [parallel(kids[0].as_str())]);
    let grand = d
        .doc()
        .console
        .parallels_of(&kids[0])
        .next()
        .cloned()
        .expect("chain");
    assert_eq!(name_of(d.doc(), grand.as_str()), format!("P1: P1: {base}"));
    assert_console_invariants(d.doc());
}

#[test]
fn a_parallel_channel_plays_into_the_bus_of_its_strip() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create_bus("Drums", OutputDest::Master),
            set_output("channel", bus("bus")),
        ],
    );
    one_step(&mut d, &r, parallel("channel"));
    let child = d.doc().console.parallels[0].1.clone();
    assert_eq!(output_dest(d.doc(), &child), bus("bus"));
    // ...and follows that bus like any strip of it.
    assert_eq!(
        into_socket(d.doc(), child.as_str(), FOLLOW_IN),
        [format!("bus:{FOLLOW_OUT}")]
    );
}

#[test]
fn a_strip_without_a_parallel_output_is_refused() {
    let (mut d, r) = project();
    let e = d
        .apply(parallel("a"), &r)
        .expect_err("a synth is no strip")
        .to_string();
    assert!(e.contains("not a console channel or bus"), "{e}");
}

#[test]
fn renaming_a_strip_renames_its_copies_unless_the_user_did() {
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel"), parallel("channel")]);
    let kids: Vec<_> = d
        .doc()
        .console
        .parallels_of(&key("channel"))
        .cloned()
        .collect();
    // The user renamed the second copy.
    run(
        &mut d,
        &r,
        [Command::Rename {
            key: kids[1].clone(),
            name: "Crunch".into(),
        }],
    );
    one_step(
        &mut d,
        &r,
        Command::Rename {
            key: key("channel"),
            name: "Lead".into(),
        },
    );
    assert_eq!(name_of(d.doc(), kids[0].as_str()), "P1: Lead");
    assert_eq!(name_of(d.doc(), kids[1].as_str()), "Crunch");
}

#[test]
fn renaming_a_bus_renames_its_copies() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [create_bus("Drums", OutputDest::Master), parallel("bus")],
    );
    let child = d.doc().console.parallels[0].1.clone();
    assert_eq!(name_of(d.doc(), child.as_str()), "P1: Drums");
    one_step(
        &mut d,
        &r,
        Command::Rename {
            key: key("bus"),
            name: "Kit".into(),
        },
    );
    assert_eq!(name_of(d.doc(), child.as_str()), "P1: Kit");
}

#[test]
fn deleting_the_strip_deletes_its_copies_and_undo_brings_them_back() {
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel"), parallel("channel")]);
    let kids: Vec<_> = d
        .doc()
        .console
        .parallels_of(&key("channel"))
        .cloned()
        .collect();
    run(&mut d, &r, [parallel(kids[0].as_str())]);
    let before = d.doc().clone();
    assert_eq!(before.console.channels.len(), 5);
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RemoveChannel {
            channel: key("channel"),
        }),
    );
    let doc = d.doc();
    assert_eq!(doc.console.channels.len(), 1, "only channel-2 is left");
    assert!(doc.console.parallels.is_empty());
    assert!(kids.iter().all(|k| doc.device(k).is_none()));
    assert_console_invariants(doc);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn deleting_the_instrument_or_the_device_takes_the_whole_chain() {
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel")]);
    let child = d.doc().console.parallels[0].1.clone();
    one_step(&mut d, &r, Command::RemoveDevice { key: key("a") });
    assert!(d.doc().device(&child).is_none());
    assert!(d.doc().console.parallels.is_empty());
    // A copy alone can be removed; its entry goes.
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel")]);
    let child = d.doc().console.parallels[0].1.clone();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RemoveChannel {
            channel: child.clone(),
        }),
    );
    assert!(d.doc().console.parallels.is_empty());
    assert!(d.doc().device(&key("channel")).is_some());
}

#[test]
fn deleting_a_bus_deletes_its_copies() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [create_bus("Drums", OutputDest::Master), parallel("bus")],
    );
    let child = d.doc().console.parallels[0].1.clone();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::DeleteBus { bus: key("bus") }),
    );
    assert!(d.doc().device(&child).is_none());
    assert!(d.doc().console.parallels.is_empty());
}

#[test]
fn soloing_the_strip_leaves_its_copy_audible() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [parallel("channel"), set_param("channel", "solo", 1.0)],
    );
    let child = d.doc().console.parallels[0].1.clone();
    let silent: Vec<(String, bool)> = d
        .doc()
        .solo_silence(&r)
        .into_iter()
        .map(|(k, s)| (k.0, s))
        .collect();
    let of = |k: &str| silent.iter().find(|(n, _)| n == k).map(|(_, s)| *s);
    assert_eq!(of("channel"), Some(false));
    assert_eq!(
        of(child.as_str()),
        Some(false),
        "the copy plays with its strip"
    );
    assert_eq!(of("channel-2"), Some(true), "the other strips are silenced");
}

// ---- Send follow ---------------------------------------------------------------------------

#[test]
fn a_strip_routed_to_a_bus_follows_it_and_master_strips_follow_nothing() {
    let (mut d, r) = project();
    assert!(d.doc().console.send_follow, "a new project follows");
    run(&mut d, &r, [create_bus("Drums", OutputDest::Master)]);
    assert!(follow_links(d.doc()).is_empty());
    one_step(&mut d, &r, set_output("channel", bus("bus")));
    assert_eq!(
        follow_links(d.doc()),
        ["bus:follow_out -> channel:follow_in"]
    );
    // Back to the master: the link goes.
    one_step(&mut d, &r, set_output("channel", OutputDest::Master));
    assert!(follow_links(d.doc()).is_empty());
}

#[test]
fn follow_links_cascade_through_nested_buses() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create_bus("Outer", OutputDest::Master),
            create_bus("Inner", bus("bus")),
            set_output("channel", bus("bus-2")),
        ],
    );
    let mut links = follow_links(d.doc());
    links.sort();
    assert_eq!(
        links,
        [
            "bus-2:follow_out -> channel:follow_in",
            "bus:follow_out -> bus-2:follow_in"
        ]
    );
    assert_console_invariants(d.doc());
}

#[test]
fn route_to_new_bus_and_delete_bus_keep_the_links() {
    let (mut d, r) = project();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("channel"), key("channel-2")],
            name: "Group".into(),
        }),
    );
    assert_eq!(follow_links(d.doc()).len(), 2);
    // Deleting the bus moves the strips to the master: no link is left.
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::DeleteBus { bus: key("bus") }),
    );
    assert!(follow_links(d.doc()).is_empty());
    // Deleting a bus inside a bus re-links the feeders to the outer one.
    run(
        &mut d,
        &r,
        [
            create_bus("Outer", OutputDest::Master),
            create_bus("Inner", bus("bus")),
            set_output("channel", bus("bus-2")),
        ],
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::DeleteBus { bus: key("bus-2") }),
    );
    assert_eq!(
        follow_links(d.doc()),
        ["bus:follow_out -> channel:follow_in"]
    );
}

#[test]
fn switching_the_flag_makes_and_removes_every_link_in_one_step() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create_bus("Drums", OutputDest::Master),
            set_output("channel", bus("bus")),
            set_output("channel-2", bus("bus")),
        ],
    );
    assert_eq!(follow_links(d.doc()).len(), 2);
    one_step(&mut d, &r, con(ConsoleCommand::SetSendFollow(false)));
    assert!(!d.doc().console.send_follow);
    assert!(follow_links(d.doc()).is_empty());
    // With the flag off the routing commands make none.
    run(&mut d, &r, [set_output("channel", OutputDest::Master)]);
    run(&mut d, &r, [set_output("channel", bus("bus"))]);
    assert!(follow_links(d.doc()).is_empty());
    one_step(&mut d, &r, con(ConsoleCommand::SetSendFollow(true)));
    assert_eq!(follow_links(d.doc()).len(), 2);
}

#[test]
fn a_project_saved_without_the_flag_loads_with_it_off_and_makes_no_links() {
    let r = console_registry();
    let mut doc = legacy_project_doc();
    assert!(!doc.console.send_follow);
    let text = save_project(&doc);
    assert!(!text.contains("send_follow") && !text.contains("parallels"));
    doc = load_project(&text, &r).expect("loads");
    assert!(!doc.console.send_follow);
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            create_bus("Drums", OutputDest::Master),
            set_output("channel", bus("bus")),
        ],
    );
    assert!(follow_links(d.doc()).is_empty());
}

// ---- The saved file ------------------------------------------------------------------------

#[test]
fn the_flag_and_the_copies_round_trip_and_are_written_only_when_set() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create_bus("Drums", OutputDest::Master),
            set_output("channel", bus("bus")),
            parallel("channel"),
            mix("In"),
        ],
    );
    let text = save_project(d.doc());
    assert!(text.contains("\"send_follow\": true"));
    assert!(text.contains("\"parallels\": [[\"channel\", "));
    let loaded = load_project(&text, &r).expect("loads");
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    // Neither field when unset.
    let plain = save_project(&legacy_project_doc());
    assert!(!plain.contains("send_follow") && !plain.contains("parallels"));
}

#[test]
fn a_parallel_pair_that_names_a_missing_or_wrong_device_is_rejected() {
    let (mut d, r) = project();
    run(&mut d, &r, [parallel("channel")]);
    let text = save_project(d.doc());
    for (from, to, needle) in [
        (
            "\"parallels\": [[\"channel\", ",
            "\"parallels\": [[\"nope\", ",
            "no device has id",
        ),
        (
            "\"parallels\": [[\"channel\", \"channel-3\"]]",
            "\"parallels\": [[\"channel\", \"a\"]]",
            "not a console channel",
        ),
        (
            "\"parallels\": [[\"channel\", \"channel-3\"]]",
            "\"parallels\": [[\"channel\"]]",
            "expected [parent, child]",
        ),
    ] {
        let bad = text.replace(from, to);
        assert_ne!(bad, text, "the edit must apply: {from}");
        let e = load_project(&bad, &r).expect_err("rejected").to_string();
        assert!(e.contains(needle), "{needle}: {e}");
    }
}
