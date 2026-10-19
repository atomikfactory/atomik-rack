//! Console links and the output display (model half): the exact
//! classifier `console::link_kind` over every cable class of a project, and
//! `console::output_dest` for the "Output: Master" display.

mod common;

use atomik_model::console::{LinkKind, OutputDest, link_kind, output_dest};
use atomik_model::{
    Cable, Command, ConsoleCommand, Document, FX_SLOTS, RackDoc, SocketRef, empty_doc,
    new_project_doc,
};
use common::audio::{add_audio_track, audio_registry};
use common::*;

/// The starter with two synths (and their channels), a delay in the insert of the first
/// channel, a reverb on return 1, a mixer fed straight by the second channel (a direct out) and
/// an audio track (with its channel and `interface` input cables).
fn project() -> (Document, atomik_device_sdk::Registry) {
    let r = audio_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("synth")),
            add(usize::MAX, SYNTH, Some("bass")),
            add(usize::MAX, DELAY, Some("insert")),
            add(usize::MAX, "atomik.reverb", Some("verb")),
            add(usize::MAX, MIXER, Some("mixer")),
            add_audio_track(),
            Command::Console(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot: 0,
                effect: Some(atomik_model::InsertEffect::Existing(key("insert"))),
            }),
            Command::Console(ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("verb")),
            }),
            connect("channel-2:out_l", "mixer:ch1_in_l"),
            connect("channel-2:out_r", "mixer:ch1_in_r"),
        ],
    );
    (d, r)
}

fn sref(text: &str) -> SocketRef {
    sock(text)
}

fn kind_of(doc: &RackDoc, from: &str, to: &str) -> Option<LinkKind> {
    link_kind(doc, &cable(from, to))
}

#[test]
fn exactly_the_eighteen_cables_per_channel_are_console_links() {
    let (d, _) = project();
    let doc = d.doc();
    assert_eq!(doc.console.channels.len(), 3, "two synths and the track");
    for ch in &doc.console.channels {
        let links: Vec<&Cable> = doc
            .cables
            .iter()
            .filter(|c| c.from.device == ch.channel && link_kind(doc, c).is_some())
            .collect();
        assert_eq!(
            links.len(),
            2 + 2 * FX_SLOTS,
            "{} has out_l/r and 8 sends in l and r",
            ch.channel
        );
    }
    let total = doc
        .cables
        .iter()
        .filter(|c| link_kind(doc, c).is_some())
        .count();
    assert_eq!(total, 3 * 18);
}

#[test]
fn the_kinds_name_the_output_and_the_send() {
    let (d, _) = project();
    let doc = d.doc();
    assert_eq!(
        kind_of(doc, "channel:out_l", "master:bus_l"),
        Some(LinkKind::ChannelOut)
    );
    assert_eq!(
        kind_of(doc, "channel:out_r", "master:bus_r"),
        Some(LinkKind::ChannelOut)
    );
    for k in 1..=FX_SLOTS {
        for side in ["l", "r"] {
            assert_eq!(
                kind_of(
                    doc,
                    &format!("channel-2:send{k}_{side}"),
                    &format!("master:send_bus{k}_{side}")
                ),
                Some(LinkKind::ChannelSend(k as u8)),
                "send {k} {side}"
            );
        }
    }
}

#[test]
fn every_other_cable_of_the_project_is_ordinary() {
    let (d, _) = project();
    let doc = d.doc();
    let ordinary: Vec<&Cable> = doc
        .cables
        .iter()
        .filter(|c| link_kind(doc, c).is_none())
        .collect();
    let has = |from: &str, to: &str| ordinary.contains(&&cable(from, to));
    // Source to channel.
    assert!(has("synth:out_l", "channel:in_l"));
    // Insert loop.
    assert!(has("channel:insert_send_l", "insert:in_l"));
    assert!(has("insert:out_l", "channel:insert_return_l"));
    // Master send/return loop.
    assert!(has("master:fx_send1_l", "verb:in_l"));
    assert!(has("verb:out_l", "master:return1_l"));
    // Main to the interface.
    assert!(has("master:main_l", "interface:out_1"));
    assert!(has("master:main_r", "interface:out_2"));
    // A direct out of a channel.
    assert!(has("channel-2:out_l", "mixer:ch1_in_l"));
    // The audio track's input cables.
    assert!(
        ordinary
            .iter()
            .any(|c| c.from.device.as_str() == "interface" && c.from.socket.starts_with("in_"))
    );
    // Nothing else is hidden: the counts add up.
    assert_eq!(ordinary.len() + 3 * 18, doc.cables.len());
}

#[test]
fn the_classifier_is_exact_on_hand_made_cables() {
    let (d, _) = project();
    let doc = d.doc();
    // Wrong side, wrong socket, wrong bus number, wrong direction of travel.
    for (from, to) in [
        ("channel:out_l", "master:bus_r"),
        ("channel:out_r", "master:bus_l"),
        ("channel:out_l", "master:send_bus1_l"),
        ("channel:send1_l", "master:bus_l"),
        ("channel:send1_l", "master:send_bus2_l"),
        ("channel:send1_l", "master:send_bus1_r"),
        ("channel:send1_r", "master:send_bus1_l"),
        ("channel:send9_l", "master:send_bus9_l"),
        ("channel:send0_l", "master:send_bus0_l"),
        ("channel:send01_l", "master:send_bus01_l"),
        // A non-canonical number on the channel side never counts, even into a canonical bus.
        ("channel:send01_l", "master:send_bus1_l"),
        ("channel:send+1_l", "master:send_bus1_l"),
        ("channel:send001_r", "master:send_bus1_r"),
        ("channel:send+1_l", "master:send_bus+1_l"),
        ("channel:send1", "master:send_bus1"),
        ("channel:send_l", "master:send_bus_l"),
        ("channel:out_l", "master:return1_l"),
        ("channel:out_l", "master:key_in"),
        ("channel:out_l", "master:insert_return_l"),
        ("channel:insert_send_l", "master:bus_l"),
        ("master:main_l", "channel:in_l"),
        ("master:fx_send1_l", "master:bus_l"),
        ("synth:out_l", "master:bus_l"),
        ("channel:out_l", "interface:out_1"),
        ("channel:out_l", "mixer:ch1_in_l"),
        ("channel:out_l", "channel-2:in_l"),
    ] {
        assert_eq!(kind_of(doc, from, to), None, "{from} -> {to}");
    }
}

#[test]
fn a_cable_into_a_device_that_is_not_the_master_is_never_a_link() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("synth")),
            add(usize::MAX, MASTER, Some("stray")),
        ],
    );
    let doc = d.doc();
    assert_eq!(doc.console.master, Some(key("master")));
    assert_eq!(kind_of(doc, "channel:out_l", "stray:bus_l"), None);
    assert_eq!(
        kind_of(doc, "channel:out_l", "master:bus_l"),
        Some(LinkKind::ChannelOut)
    );
}

#[test]
fn a_channel_device_outside_the_console_counts_as_a_channel() {
    // The classifier looks at the device type too: a channel from a foreign file is a channel.
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(&mut d, &r, [add(usize::MAX, CHANNEL, Some("loose"))]);
    let doc = d.doc();
    assert!(doc.console.channels.is_empty());
    assert_eq!(
        kind_of(doc, "loose:out_l", "master:bus_l"),
        Some(LinkKind::ChannelOut)
    );
    assert_eq!(
        kind_of(doc, "loose:send3_r", "master:send_bus3_r"),
        Some(LinkKind::ChannelSend(3))
    );
}

#[test]
fn a_document_without_a_master_has_no_links() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("synth"))]);
    let mut doc = d.doc().clone();
    doc.console.master = None;
    assert!(doc.cables.iter().all(|c| link_kind(&doc, c).is_none()));
    // An unknown source device is no channel either.
    assert_eq!(kind_of(&doc, "ghost:out_l", "master:bus_l"), None);
}

// ------------------------------------------------------------------------------ output display

fn one_channel() -> (Document, atomik_device_sdk::Registry) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("synth")),
            add(usize::MAX, MIXER, Some("mixer")),
        ],
    );
    (d, r)
}

fn cut_to_master(d: &mut Document, r: &atomik_device_sdk::Registry) {
    for to in ["master:bus_l", "master:bus_r"] {
        run(d, r, [Command::Disconnect { to: sock(to) }]);
    }
}

#[test]
fn a_new_channel_outputs_to_the_master() {
    let (d, _) = one_channel();
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::Master);
}

#[test]
fn a_channel_cabled_elsewhere_is_direct() {
    let (mut d, r) = one_channel();
    cut_to_master(&mut d, &r);
    run(
        &mut d,
        &r,
        [
            connect("channel:out_l", "mixer:ch1_in_l"),
            connect("channel:out_r", "mixer:ch1_in_r"),
        ],
    );
    assert_eq!(
        output_dest(d.doc(), &key("channel")),
        OutputDest::Direct(vec![sref("mixer:ch1_in_l"), sref("mixer:ch1_in_r")])
    );
}

#[test]
fn a_channel_cabled_to_the_interface_is_direct() {
    let (mut d, r) = one_channel();
    cut_to_master(&mut d, &r);
    // The master owns out 1 and 2; free them for the channel.
    for to in ["interface:out_1", "interface:out_2"] {
        run(&mut d, &r, [Command::Disconnect { to: sock(to) }]);
    }
    run(
        &mut d,
        &r,
        [
            connect("channel:out_l", "interface:out_1"),
            connect("channel:out_r", "interface:out_2"),
        ],
    );
    assert_eq!(
        output_dest(d.doc(), &key("channel")),
        OutputDest::Direct(vec![sref("interface:out_1"), sref("interface:out_2")])
    );
}

#[test]
fn master_and_direct_together_is_mixed() {
    let (mut d, r) = one_channel();
    run(&mut d, &r, [connect("channel:out_l", "mixer:ch1_in_l")]);
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::Mixed);
}

#[test]
fn one_link_and_one_direct_output_is_mixed() {
    // One link and one other cable (two output cables, one console link) is Mixed, not Direct.
    let (mut d, r) = one_channel();
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("master:bus_r"),
            },
            connect("channel:out_r", "mixer:ch1_in_r"),
        ],
    );
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::Mixed);
    // And the other way round: only the right side stays a link.
    let (mut d, r) = one_channel();
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("master:bus_l"),
            },
            connect("channel:out_l", "mixer:ch1_in_l"),
        ],
    );
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::Mixed);
}

#[test]
fn a_channel_without_output_cables_is_none() {
    let (mut d, r) = one_channel();
    cut_to_master(&mut d, &r);
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::None);
}

#[test]
fn only_the_outputs_decide_not_the_sends() {
    let (mut d, r) = one_channel();
    // Cutting the sends changes nothing for the display.
    for k in 1..=FX_SLOTS {
        for side in ["l", "r"] {
            run(
                &mut d,
                &r,
                [Command::Disconnect {
                    to: sock(&format!("master:send_bus{k}_{side}")),
                }],
            );
        }
    }
    assert_eq!(output_dest(d.doc(), &key("channel")), OutputDest::Master);
}

#[test]
fn something_that_is_not_a_channel_has_no_output_display() {
    let (d, _) = one_channel();
    assert_eq!(output_dest(d.doc(), &key("synth")), OutputDest::None);
    assert_eq!(output_dest(d.doc(), &key("ghost")), OutputDest::None);
    assert_eq!(output_dest(d.doc(), &key("master")), OutputDest::None);
}

#[test]
fn the_display_is_deterministic_for_equal_documents() {
    let (d, _) = one_channel();
    let c = d.doc().clone();
    assert_eq!(
        output_dest(d.doc(), &key("channel")),
        output_dest(&c, &key("channel"))
    );
}
