//! Console model (model part): summing-input cable rules,
//! every `ConsoleCommand` with its rejections, automatic channels for instruments,
//! the solo matrix through `RackDoc::solo_silence`, and `internal.*` params.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Cable, Command, ConsoleChannel, ConsoleCommand, Document, FX_SLOTS, ModelError, RackDoc,
    SongCommand, SyncOp, empty_doc,
};
use common::song::add_track;
use common::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn add_channel(source: &str) -> Command {
    cc(ConsoleCommand::AddChannel {
        source: key(source),
    })
}

fn set_return(slot: usize, effect: Option<&str>) -> Command {
    cc(ConsoleCommand::SetReturn {
        slot,
        effect: effect.map(key),
    })
}

fn param(k: &str, p: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: p.into(),
        value,
        merge: None,
    }
}

fn console_op() -> SyncOp {
    SyncOp::ConsoleChanged
}

/// Applies `cmd`, expects a rejection and checks that nothing changed.
fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before);
    assert_eq!(d.can_undo(), can_undo);
    e
}

/// Undoes everything and checks the document is back to `before`.
fn undo_all_restores(d: &mut Document, before: &RackDoc, reg: &Registry) {
    while d.undo(reg).is_some() {}
    assert_eq!(d.doc(), before);
}

fn channel(ch: &str, source: Option<&str>) -> ConsoleChannel {
    ConsoleChannel {
        channel: key(ch),
        source: source.map(key),
    }
}

/// The 20 cables auto-wiring adds between a channel and the master (outs and 8 sends).
fn bus_cables(ch: &str, master: &str) -> Vec<Cable> {
    let mut v = vec![
        cable(&format!("{ch}:out_l"), &format!("{master}:bus_l")),
        cable(&format!("{ch}:out_r"), &format!("{master}:bus_r")),
    ];
    for k in 1..=FX_SLOTS {
        for s in ["l", "r"] {
            v.push(cable(
                &format!("{ch}:send{k}_{s}"),
                &format!("{master}:send_bus{k}_{s}"),
            ));
        }
    }
    v
}

fn sorted(mut v: Vec<Cable>) -> Vec<Cable> {
    v.sort();
    v
}

/// A document with `synth`, `delay`, `mixer` and no console, built without console devices.
fn rack() -> Document {
    Document::new(three_devices(&registry()).doc().clone())
}

// ---------------------------------------------------------------------------------------
// Summing inputs.

#[test]
fn summing_input_accepts_many_cables_but_not_the_same_cable_twice() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("delay"), &reg).unwrap();
    // The master bus already sums the channel; a direct cable from the synth joins it.
    d.apply(connect("synth:out_l", "master:bus_l"), &reg)
        .unwrap();
    d.apply(connect("mixer:master_l", "master:bus_l"), &reg)
        .unwrap();
    let into_bus: Vec<_> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.to == sock("master:bus_l"))
        .map(|c| c.from.to_string())
        .collect();
    assert_eq!(into_bus, ["channel:out_l", "mixer:master_l", "synth:out_l"]);
    let e = rejects(&mut d, connect("synth:out_l", "master:bus_l"), &reg);
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("already connected")),
        "{e:?}"
    );
    // A summing input is still an input: a cable cannot start there; kinds must match.
    let e = rejects(&mut d, connect("master:bus_l", "delay:in_l"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)));
    let e = rejects(&mut d, connect("synth:lfo_cv_out", "master:bus_r"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)));
    // Ordinary inputs keep one cable.
    let e = rejects(&mut d, connect("synth:out_l", "channel:in_l"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)));
}

#[test]
fn summing_input_cables_count_in_the_cycle_check() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    // master main → delay → master bus would loop.
    d.apply(connect("master:main_l", "delay:in_l"), &reg)
        .unwrap();
    let e = rejects(&mut d, connect("delay:out_l", "master:bus_l"), &reg);
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle")),
        "{e:?}"
    );
    // So would master FX send → mixer → master send bus, or a send bus fed by its own send.
    d.apply(connect("master:fx_send1_l", "mixer:ch1_in_l"), &reg)
        .unwrap();
    let e = rejects(
        &mut d,
        connect("mixer:master_l", "master:send_bus3_r"),
        &reg,
    );
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    let e = rejects(
        &mut d,
        connect("master:fx_send2_l", "master:send_bus1_l"),
        &reg,
    );
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // The master is two nodes: FX sends (fed by the send buses only) and the rest. An effect
    // between an FX send and the master's returns or main bus is not a loop.
    d.apply(connect("mixer:master_r", "master:return2_r"), &reg)
        .unwrap();
    d.apply(connect("mixer:master_l", "master:bus_r"), &reg)
        .unwrap();
    // But an effect fed by the master's main side cannot feed the send buses... through a
    // channel either.
    d.apply(
        Command::Disconnect {
            to: sock("delay:in_l"),
        },
        &reg,
    )
    .unwrap();
    d.apply(add_channel("delay"), &reg).unwrap();
    let e = rejects(&mut d, connect("master:fx_send4_l", "delay:in_r"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn disconnect_of_a_summing_input_removes_every_cable_into_it_in_one_step() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    d.apply(connect("mixer:master_l", "master:bus_l"), &reg)
        .unwrap();
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::Disconnect {
                to: sock("master:bus_l"),
            },
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [routing()]);
    assert!(d.doc().cables.iter().all(|c| c.to != sock("master:bus_l")));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---------------------------------------------------------------------------------------
// AddChannel.

#[test]
fn add_channel_creates_channel_and_master_and_auto_wires() {
    let reg = console_registry();
    let mut d = Document::new(chain(&registry()).doc().clone());
    let before = d.doc().clone();
    let ops = d.apply(add_channel("delay"), &reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("master")),
            SyncOp::DeviceAdded(key("channel")),
            routing(),
            console_op(),
        ]
    );
    let doc = d.doc();
    assert_eq!(keys(&d), ["synth", "delay", "channel", "mixer", "master"]);
    assert_eq!(doc.device(&key("channel")).unwrap().type_id, CHANNEL);
    assert_eq!(doc.device(&key("channel")).unwrap().name, "Channel");
    assert_eq!(doc.device(&key("master")).unwrap().type_id, MASTER);
    assert_eq!(doc.console.channels, [channel("channel", Some("delay"))]);
    assert_eq!(doc.console.master, Some(key("master")));
    assert_eq!(
        doc.console.returns,
        [None, None, None, None, None, None, None, None]
    );
    assert!(doc.console.compensation);
    let mut want = vec![
        cable("synth:out_l", "delay:in_l"),
        cable("synth:out_r", "delay:in_r"),
        cable("delay:out_l", "mixer:ch1_in_l"),
        cable("delay:out_r", "mixer:ch1_in_r"),
        cable("delay:out_l", "channel:in_l"),
        cable("delay:out_r", "channel:in_r"),
        // The master replaces the mixer at the interface.
        cable("master:main_l", "interface:out_1"),
        cable("master:main_r", "interface:out_2"),
    ];
    want.extend(bus_cables("channel", "master"));
    assert_eq!(doc.cables, sorted(want));
    // One undo step restores everything (the mixer's interface cables too).
    let ops = d.undo(&reg).unwrap();
    assert_eq!(
        ops,
        [
            console_op(),
            routing(),
            SyncOp::DeviceRemoved(key("channel")),
            SyncOp::DeviceRemoved(key("master")),
        ]
    );
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().console.channels.len(), 1);
}

#[test]
fn second_channel_reuses_the_master_and_gets_a_generated_key() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    let ops = d.apply(add_channel("delay"), &reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("channel-2")),
            routing(),
            console_op()
        ]
    );
    assert_eq!(
        keys(&d),
        ["synth", "channel", "delay", "channel-2", "mixer", "master"]
    );
    assert_eq!(d.doc().device(&key("channel-2")).unwrap().name, "Channel 2");
    assert_eq!(
        d.doc().console.channels,
        [
            channel("channel", Some("synth")),
            channel("channel-2", Some("delay"))
        ]
    );
    let mut want = vec![
        cable("synth:out_l", "channel:in_l"),
        cable("synth:out_r", "channel:in_r"),
        cable("delay:out_l", "channel-2:in_l"),
        cable("delay:out_r", "channel-2:in_r"),
        cable("master:main_l", "interface:out_1"),
        cable("master:main_r", "interface:out_2"),
    ];
    want.extend(bus_cables("channel", "master"));
    want.extend(bus_cables("channel-2", "master"));
    assert_eq!(d.doc().cables, sorted(want));
}

#[test]
fn add_channel_removes_the_sources_direct_interface_cables() {
    let reg = console_registry();
    let mut doc = three_devices(&registry()).doc().clone();
    doc.output_channels = 4;
    let mut d = Document::new(doc);
    for (f, t) in [
        ("synth:out_l", "interface:out_1"),
        ("synth:out_r", "interface:out_2"),
        ("synth:out_r", "interface:out_4"),
        ("delay:out_l", "interface:out_3"),
        ("synth:out_l", "delay:in_l"),
    ] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    d.apply(add_channel("synth"), &reg).unwrap();
    let to_interface: Vec<_> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.to.device.0 == "interface")
        .map(|c| format!("{} -> {}", c.from, c.to))
        .collect();
    assert_eq!(
        to_interface,
        [
            "delay:out_l -> interface:out_3",
            "master:main_l -> interface:out_1",
            "master:main_r -> interface:out_2",
        ]
    );
    // Its other cables stay.
    assert!(d.doc().cables.contains(&cable("synth:out_l", "delay:in_l")));
}

#[test]
fn add_channel_on_a_mono_interface_wires_main_left_only() {
    let reg = console_registry();
    let mut doc = three_devices(&registry()).doc().clone();
    doc.output_channels = 1;
    let mut d = Document::new(doc);
    d.apply(add_channel("synth"), &reg).unwrap();
    let to_interface: Vec<_> = d
        .doc()
        .cables
        .iter()
        .filter(|c| c.to.device.0 == "interface")
        .cloned()
        .collect();
    assert_eq!(to_interface, [cable("master:main_l", "interface:out_1")]);
}

#[test]
fn add_channel_picks_the_first_two_audio_outputs_without_out_l_out_r() {
    let reg = console_registry();
    let mut d = Document::new(empty_doc());
    // Built without console devices, so no automatic channels.
    d.apply(
        add(0, "test.odd-synth", Some("odd")),
        &reg_without_console(),
    )
    .unwrap();
    d.apply(
        add(1, "test.mono-synth", Some("mono")),
        &reg_without_console(),
    )
    .unwrap();
    d.apply(add_channel("odd"), &reg).unwrap();
    d.apply(add_channel("mono"), &reg).unwrap();
    let cables = &d.doc().cables;
    for c in [
        cable("odd:a", "channel:in_l"),
        cable("odd:b", "channel:in_r"),
        cable("mono:out", "channel-2:in_l"),
        cable("mono:out", "channel-2:in_r"),
    ] {
        assert!(cables.contains(&c), "{c:?} missing");
    }
    assert!(!cables.iter().any(|c| c.from == sock("odd:c")));
}

/// The test instruments without the console devices.
fn reg_without_console() -> Registry {
    let all = console_registry();
    let mut r = Registry::new();
    for t in all.type_ids().filter(|t| ![CHANNEL, MASTER].contains(t)) {
        r.register(*all.get(t).unwrap()).unwrap();
    }
    r
}

#[test]
fn add_channel_rejections() {
    let reg = console_registry();
    let mut d = rack();
    assert!(matches!(
        rejects(&mut d, add_channel("nope"), &reg),
        ModelError::UnknownDevice(_)
    ));
    // Without the console devices in the registry.
    assert!(matches!(
        rejects(&mut d, add_channel("synth"), &registry()),
        ModelError::UnknownType(t) if t == CHANNEL
    ));
    d.apply(add_channel("synth"), &reg).unwrap();
    for source in ["synth", "channel", "master", "interface"] {
        let e = rejects(&mut d, add_channel(source), &reg);
        assert!(
            matches!(e, ModelError::BadConsole(_) | ModelError::UnknownDevice(_)),
            "{source}: {e:?}"
        );
    }
    // No audio output to feed a channel.
    d.apply(
        add(0, "test.silent-synth", Some("silent")),
        &reg_without_console(),
    )
    .unwrap();
    let e = rejects(&mut d, add_channel("silent"), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    // A return effect cannot also feed a channel (it would loop through the master).
    d.apply(set_return(0, Some("delay")), &reg).unwrap();
    let e = rejects(&mut d, add_channel("delay"), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    // A loop through cables the user made is still a cable rejection.
    d.apply(connect("master:monitor_l", "mixer:ch1_in_l"), &reg)
        .unwrap();
    let e = rejects(&mut d, add_channel("mixer"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn a_new_master_is_wired_to_every_existing_channel() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    // The commands never remove the master, a file can lack it: build that document.
    let mut doc = d.doc().clone();
    let master = doc.console.master.take().expect("a master");
    doc.devices.retain(|x| x.key != master);
    doc.cables
        .retain(|c| c.from.device != master && c.to.device != master);
    let mut d = Document::new(doc);
    assert_eq!(d.doc().console.master, None);
    assert_eq!(d.doc().console.channels.len(), 1);
    d.apply(add_channel("delay"), &reg).unwrap();
    assert_eq!(d.doc().console.master, Some(key("master")));
    for c in bus_cables("channel", "master")
        .into_iter()
        .chain(bus_cables("channel-2", "master"))
    {
        assert!(d.doc().cables.contains(&c), "{c:?} missing");
    }
}

// ---------------------------------------------------------------------------------------
// RemoveChannel, MoveChannel.

#[test]
fn remove_channel_removes_device_and_cables_and_keeps_the_master() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    let before = d.doc().clone();
    let ops = d
        .apply(
            cc(ConsoleCommand::RemoveChannel {
                channel: key("channel"),
            }),
            &reg,
        )
        .unwrap();
    assert_eq!(
        ops,
        [
            console_op(),
            routing(),
            SyncOp::DeviceRemoved(key("channel"))
        ]
    );
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(d.doc().console.master, Some(key("master")));
    assert_eq!(keys(&d), ["synth", "delay", "mixer", "master"]);
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device.0 != "channel" && c.to.device.0 != "channel")
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    for bad in ["synth", "nope"] {
        let e = rejects(
            &mut d,
            cc(ConsoleCommand::RemoveChannel { channel: key(bad) }),
            &reg,
        );
        assert!(matches!(e, ModelError::BadConsole(_)), "{bad}: {e:?}");
    }
    // The master is fixed, not merely "not a channel".
    let e = rejects(
        &mut d,
        cc(ConsoleCommand::RemoveChannel {
            channel: key("master"),
        }),
        &reg,
    );
    assert!(
        matches!(e, ModelError::Fixed { what: "master", .. }),
        "{e:?}"
    );
}

#[test]
fn move_channel_reorders_the_console_only() {
    let reg = console_registry();
    let mut d = rack();
    for s in ["synth", "delay", "mixer"] {
        d.apply(add_channel(s), &reg).unwrap();
    }
    let mut d = Document::new(d.doc().clone());
    let before = d.doc().clone();
    let mv = |ch: &str, to: usize| {
        cc(ConsoleCommand::MoveChannel {
            channel: key(ch),
            to,
        })
    };
    let ops = d.apply(mv("channel", 2), &reg).unwrap();
    assert_eq!(ops, [console_op()]);
    let order = |d: &Document| -> Vec<String> {
        d.doc()
            .console
            .channels
            .iter()
            .map(|c| c.channel.0.clone())
            .collect()
    };
    assert_eq!(order(&d), ["channel-2", "channel-3", "channel"]);
    assert_eq!(d.doc().devices, before.devices, "rack order is unchanged");
    assert_eq!(d.doc().cables, before.cables);
    // Clamped to the last position.
    d.apply(mv("channel-2", 99), &reg).unwrap();
    assert_eq!(order(&d), ["channel-3", "channel", "channel-2"]);
    let e = rejects(&mut d, mv("synth", 0), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)));
    undo_all_restores(&mut d, &before, &reg);
}

// ---------------------------------------------------------------------------------------
// SetReturn, SetCompensation.

#[test]
fn set_return_wires_the_effect_between_fx_send_and_return() {
    let reg = console_registry();
    let mut d = Document::new(chain(&registry()).doc().clone());
    d.apply(add_channel("synth"), &reg).unwrap();
    let before = d.doc().clone();
    let ops = d.apply(set_return(0, Some("delay")), &reg).unwrap();
    assert_eq!(ops, [routing(), console_op()]);
    let doc = d.doc();
    assert_eq!(doc.console.returns[0], Some(key("delay")));
    let delay_cables: Vec<_> = doc
        .cables
        .iter()
        .filter(|c| c.from.device.0 == "delay" || c.to.device.0 == "delay")
        .cloned()
        .collect();
    // The insert-chain cables (synth → delay → mixer) are replaced.
    assert_eq!(
        delay_cables,
        sorted(vec![
            cable("master:fx_send1_l", "delay:in_l"),
            cable("master:fx_send1_r", "delay:in_r"),
            cable("delay:out_l", "master:return1_l"),
            cable("delay:out_r", "master:return1_r"),
        ])
    );
    // Moving it to slot 4 rewires; clearing unwires.
    d.apply(set_return(3, None), &reg).unwrap();
    let e = rejects(&mut d, set_return(3, Some("delay")), &reg);
    assert!(
        matches!(e, ModelError::BadConsole(_)),
        "already on slot 1: {e:?}"
    );
    d.apply(set_return(0, None), &reg).unwrap();
    assert!(d.doc().console.returns.iter().all(Option::is_none));
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device.0 != "delay" && c.to.device.0 != "delay")
    );
    d.apply(set_return(7, Some("delay")), &reg).unwrap();
    assert!(
        d.doc()
            .cables
            .contains(&cable("master:fx_send8_r", "delay:in_r"))
    );
    assert!(
        d.doc()
            .cables
            .contains(&cable("delay:out_l", "master:return8_l"))
    );
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_return_rejections() {
    let reg = console_registry();
    let mut d = rack();
    let e = rejects(&mut d, set_return(0, Some("delay")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "no master: {e:?}");
    d.apply(add_channel("synth"), &reg).unwrap();
    let e = rejects(&mut d, set_return(FX_SLOTS, Some("delay")), &reg);
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    let e = rejects(&mut d, set_return(0, Some("nope")), &reg);
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e:?}");
    for console_dev in ["channel", "master"] {
        let e = rejects(&mut d, set_return(0, Some(console_dev)), &reg);
        assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    }
    // A synth has no audio inputs.
    d.apply(add(9, SYNTH, Some("lone")), &registry()).unwrap();
    let e = rejects(&mut d, set_return(0, Some("lone")), &reg);
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    // A channel source cannot also be a return effect.
    d.apply(add_channel("delay"), &reg).unwrap();
    let e = rejects(&mut d, set_return(0, Some("delay")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    // A loop through cables the user made is still a cable rejection.
    d.apply(add(9, DELAY, Some("echo")), &reg).unwrap();
    d.apply(connect("master:gr_cv_out", "echo:time_cv_in"), &reg)
        .unwrap();
    let e = rejects(&mut d, set_return(5, Some("echo")), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn set_compensation_toggles_in_one_undo_step() {
    let reg = console_registry();
    let mut d = rack();
    assert!(d.doc().console.compensation, "on by default");
    let ops = d
        .apply(cc(ConsoleCommand::SetCompensation(false)), &reg)
        .unwrap();
    assert_eq!(ops, [console_op()]);
    assert!(!d.doc().console.compensation);
    d.undo(&reg).unwrap();
    assert!(d.doc().console.compensation);
}

// ---------------------------------------------------------------------------------------
// Solo and mute, internal params.

/// A rack with three channels (`channel`, `channel-2`, `channel-3`) and a return.
fn three_channels(reg: &Registry) -> Document {
    let mut d = rack();
    for s in ["synth", "mixer"] {
        d.apply(add_channel(s), reg).unwrap();
    }
    d.apply(add(9, DELAY, Some("verb")), reg).unwrap();
    d.apply(add_channel("verb"), reg).unwrap();
    d.apply(set_return(0, Some("delay")), reg).unwrap();
    Document::new(d.doc().clone())
}

fn silence(d: &Document, reg: &Registry) -> Vec<(String, bool)> {
    d.doc()
        .solo_silence(reg)
        .into_iter()
        .map(|(k, s)| (k.0, s))
        .collect()
}

fn s(v: &[(&str, bool)]) -> Vec<(String, bool)> {
    v.iter().map(|&(k, b)| (k.to_owned(), b)).collect()
}

#[test]
fn ac_x2_solo_matrix() {
    let reg = console_registry();
    let mut d = three_channels(&reg);
    let none = s(&[
        ("channel", false),
        ("channel-2", false),
        ("channel-3", false),
    ]);
    assert_eq!(silence(&d, &reg), none);
    // Mute does not silence other channels and is not part of solo silence.
    d.apply(param("channel", "mute", 1.0), &reg).unwrap();
    assert_eq!(silence(&d, &reg), none);
    // A solo silences every other channel (returns are not channels: solo-safe).
    let ops = d.apply(param("channel-2", "solo", 1.0), &reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::ParamChanged {
                key: key("channel-2"),
                param: "solo".into(),
                value: 1.0
            },
            console_op()
        ]
    );
    assert_eq!(
        silence(&d, &reg),
        s(&[("channel", true), ("channel-2", false), ("channel-3", true)])
    );
    // Additive: a second solo joins the first. A soloed muted channel is not silenced.
    d.apply(param("channel", "solo", 1.0), &reg).unwrap();
    assert_eq!(
        silence(&d, &reg),
        s(&[
            ("channel", false),
            ("channel-2", false),
            ("channel-3", true)
        ])
    );
    // Clear all solos: one undo step.
    let ops = d.apply(cc(ConsoleCommand::ClearSolos), &reg).unwrap();
    assert_eq!(ops.last(), Some(&console_op()));
    assert_eq!(
        ops.iter()
            .filter(|o| matches!(o, SyncOp::ParamChanged { value, .. } if *value == 0.0))
            .count(),
        2
    );
    assert_eq!(silence(&d, &reg), none);
    assert_eq!(
        d.doc().param_value(&key("channel"), "solo", &reg),
        Some(0.0)
    );
    d.undo(&reg).unwrap();
    assert_eq!(
        silence(&d, &reg),
        s(&[
            ("channel", false),
            ("channel-2", false),
            ("channel-3", true)
        ])
    );
    d.redo(&reg).unwrap();
    // Clear all mutes.
    d.apply(param("channel-3", "mute", 1.0), &reg).unwrap();
    let before = d.doc().clone();
    let ops = d.apply(cc(ConsoleCommand::ClearMutes), &reg).unwrap();
    assert_eq!(ops.len(), 2, "{ops:?}");
    for ch in ["channel", "channel-2", "channel-3"] {
        assert_eq!(d.doc().param_value(&key(ch), "mute", &reg), Some(0.0));
    }
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    // Nothing to clear is still accepted.
    d.apply(cc(ConsoleCommand::ClearSolos), &reg).unwrap();
}

#[test]
fn solo_silence_is_empty_without_channels_and_ignores_strays() {
    let reg = console_registry();
    let mut d = rack();
    assert!(d.doc().solo_silence(&reg).is_empty());
    // A channel device outside the console (e.g. from a patch) is not a console channel.
    d.apply(add(0, CHANNEL, Some("stray")), &reg).unwrap();
    d.apply(param("stray", "solo", 1.0), &reg).unwrap();
    d.apply(add_channel("synth"), &reg).unwrap();
    assert_eq!(silence(&d, &reg), s(&[("channel", false)]));
}

#[test]
fn internal_params_are_never_stored() {
    let reg = console_registry();
    let mut d = three_channels(&reg);
    let e = rejects(&mut d, param("channel", "internal.solo_silence", 1.0), &reg);
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    d.apply(add_track("synth"), &reg).unwrap();
    let e = rejects(
        &mut d,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("channel"),
            param: "internal.solo_silence".into(),
        }),
        &reg,
    );
    assert!(
        matches!(e, ModelError::UnknownKey(_) | ModelError::BadSong(_)),
        "{e:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Automatic channels.

#[test]
fn ac_x6_adding_an_instrument_adds_its_channel_in_the_same_step() {
    let reg = console_registry();
    let mut d = Document::new(empty_doc());
    let ops = d.apply(add(0, SYNTH, Some("bass")), &reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("bass")),
            SyncOp::DeviceAdded(key("master")),
            SyncOp::DeviceAdded(key("channel")),
            routing(),
            console_op(),
        ]
    );
    assert_eq!(keys(&d), ["bass", "channel", "master"]);
    assert_eq!(d.doc().console.channels, [channel("channel", Some("bass"))]);
    let ops = d.apply(add(0, SYNTH, None), &reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("poly-synth")),
            SyncOp::DeviceAdded(key("channel-2")),
            routing(),
            console_op(),
        ]
    );
    assert_eq!(
        keys(&d),
        ["poly-synth", "channel-2", "bass", "channel", "master"]
    );
    // Effects, mixers and instruments without audio outputs get no channel.
    d.apply(add(9, DELAY, None), &reg).unwrap();
    d.apply(add(9, MIXER, None), &reg).unwrap();
    d.apply(add(9, "test.silent-synth", None), &reg).unwrap();
    assert_eq!(d.doc().console.channels.len(), 2);
    // Undo removes the instrument and its channel (and the master it created) together.
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    let ops = d.undo(&reg).unwrap();
    assert_eq!(
        ops,
        [
            console_op(),
            routing(),
            SyncOp::DeviceRemoved(key("channel-2")),
            SyncOp::DeviceRemoved(key("poly-synth")),
        ]
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &empty_doc());
}

#[test]
fn without_console_devices_instruments_get_no_channel() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    let ops = d.apply(add(0, SYNTH, Some("bass")), &reg).unwrap();
    assert_eq!(ops, [SyncOp::DeviceAdded(key("bass")), routing()]);
    assert_eq!(d.doc().console, empty_doc().console);
}

#[test]
fn ac_x6_removing_an_instrument_removes_its_channel_and_undo_restores_both() {
    let reg = console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("bass")), &reg).unwrap();
    d.apply(add(9, SYNTH, Some("lead")), &reg).unwrap();
    d.apply(add_track("bass"), &reg).unwrap();
    d.apply(param("channel", "fader", 0.5), &reg).unwrap();
    let before = d.doc().clone();
    let ops = d
        .apply(Command::RemoveDevice { key: key("bass") }, &reg)
        .unwrap();
    assert_eq!(
        ops,
        [
            console_op(),
            SyncOp::SongChanged,
            routing(),
            SyncOp::DeviceRemoved(key("channel")),
            SyncOp::DeviceRemoved(key("bass")),
        ]
    );
    assert_eq!(keys(&d), ["master", "lead", "channel-2"]);
    assert_eq!(
        d.doc().console.channels,
        [channel("channel-2", Some("lead"))]
    );
    let ops = d.undo(&reg).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("bass")),
            SyncOp::DeviceAdded(key("channel")),
            SyncOp::ParamChanged {
                key: key("channel"),
                param: "fader".into(),
                value: 0.5
            },
            routing(),
            console_op(),
            SyncOp::SongChanged,
        ]
    );
    assert_eq!(d.doc(), &before);
    // Removing the last channel keeps the master.
    d.apply(Command::RemoveDevice { key: key("bass") }, &reg)
        .unwrap();
    d.apply(Command::RemoveDevice { key: key("lead") }, &reg)
        .unwrap();
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(keys(&d), ["master"]);
    assert_eq!(d.doc().console.master, Some(key("master")));
}

#[test]
fn removing_console_devices_and_returns_keeps_the_console_valid() {
    let reg = console_registry();
    let mut d = three_channels(&reg);
    let before = d.doc().clone();
    // A channel removed as a device leaves the console.
    d.apply(
        Command::RemoveDevice {
            key: key("channel-2"),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(
        d.doc().console.channels,
        [
            channel("channel", Some("synth")),
            channel("channel-3", Some("verb"))
        ]
    );
    // A non-instrument source keeps its channel, without a source.
    d.apply(Command::RemoveDevice { key: key("verb") }, &reg)
        .unwrap();
    assert_eq!(
        d.doc().console.channels,
        [
            channel("channel", Some("synth")),
            channel("channel-3", None)
        ]
    );
    // A return effect leaves its slot.
    d.apply(Command::RemoveDevice { key: key("delay") }, &reg)
        .unwrap();
    assert_eq!(d.doc().console.returns[0], None);
    // The master cannot be removed: rejected, the returns stay.
    d.apply(add(9, DELAY, Some("echo")), &reg).unwrap();
    d.apply(set_return(1, Some("echo")), &reg).unwrap();
    let e = rejects(&mut d, Command::RemoveDevice { key: key("master") }, &reg);
    assert!(
        matches!(e, ModelError::Fixed { what: "master", .. }),
        "{e:?}"
    );
    assert_eq!(d.doc().console.returns[1], Some(key("echo")));
    undo_all_restores(&mut d, &before, &reg);
}

// ---------------------------------------------------------------------------------------
// Processing stages: loops are checked per (device, stage).

#[test]
fn channel_insert_loops_are_accepted_and_true_loops_rejected() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    // insert_send (stage 0) → delay → insert_return (stage 1): an insert, not a loop.
    for (f, t) in [
        ("channel:insert_send_l", "delay:in_l"),
        ("channel:insert_send_r", "delay:in_r"),
        ("delay:out_l", "channel:insert_return_l"),
        ("delay:out_r", "channel:insert_return_r"),
    ] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    // Back into stage 0 (the dynamics key) is a loop: stage 0 → delay → stage 0.
    let e = rejects(&mut d, connect("delay:out_l", "channel:key_in"), &reg);
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle")),
        "{e:?}"
    );
    // So is a stage-1 output feeding the insert effect (stage 1 → delay → stage 1).
    d.apply(
        Command::Disconnect {
            to: sock("delay:in_r"),
        },
        &reg,
    )
    .unwrap();
    let e = rejects(&mut d, connect("channel:out_l", "delay:in_r"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // A direct insert patch on the device itself is fine; the reverse is a loop.
    d.apply(
        Command::Disconnect {
            to: sock("channel:insert_return_l"),
        },
        &reg,
    )
    .unwrap();
    d.apply(
        connect("channel:insert_send_l", "channel:insert_return_l"),
        &reg,
    )
    .unwrap();
    let e = rejects(&mut d, connect("channel:send3_l", "channel:key_in"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // Single-stage devices keep the device-level rule.
    let e = rejects(&mut d, connect("delay:out_r", "delay:in_r"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn master_stages_allow_returns_and_inserts_but_not_loops() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    d.apply(add(9, DELAY, Some("echo")), &reg).unwrap();
    // FX return: fx_send (stage 0) → delay → return (stage 1).
    d.apply(set_return(0, Some("delay")), &reg).unwrap();
    // Master insert: insert_send (stage 1) → echo → insert_return (stage 2).
    d.apply(connect("master:insert_send_l", "echo:in_l"), &reg)
        .unwrap();
    d.apply(connect("echo:out_l", "master:insert_return_l"), &reg)
        .unwrap();
    // The insert effect cannot feed an earlier stage: stage 1 → echo → stage 1 / 0.
    let e = rejects(&mut d, connect("echo:out_r", "master:return2_l"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    let e = rejects(&mut d, connect("echo:out_r", "master:send_bus1_l"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // key_in is stage 1 (no sidechain lag), so the insert effect cannot key the master.
    let e = rejects(&mut d, connect("echo:out_r", "master:key_in"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    // The return effect may feed the main bus or the master insert (later stages)...
    d.apply(connect("delay:out_l", "master:bus_l"), &reg)
        .unwrap();
    d.apply(connect("delay:out_r", "master:key_in"), &reg)
        .unwrap();
    // ...but main (stage 2) back into the return effect is a loop.
    d.apply(
        Command::Disconnect {
            to: sock("delay:in_l"),
        },
        &reg,
    )
    .unwrap();
    let e = rejects(&mut d, connect("master:main_l", "delay:in_l"), &reg);
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle")),
        "{e:?}"
    );
    // Through a channel: the return effect feeding a channel loops via the send buses.
    let e = rejects(&mut d, connect("delay:out_l", "channel:key_in"), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn stage_rules_apply_to_patch_and_project_loading() {
    let reg = console_registry();
    let mut d = rack();
    d.apply(add_channel("synth"), &reg).unwrap();
    d.apply(connect("channel:insert_send_l", "delay:in_l"), &reg)
        .unwrap();
    d.apply(connect("delay:out_l", "channel:insert_return_l"), &reg)
        .unwrap();
    let text = atomik_model::save_project(d.doc());
    assert_eq!(&atomik_model::load_project(&text, &reg).unwrap(), d.doc());
    let patch = atomik_model::save_patch(d.doc());
    assert!(atomik_model::load_patch(&patch, &reg).is_ok());
    // A real loop in a file is rejected with the cable's path.
    let looped = text.replace(
        "\"to\": \"channel:insert_return_l\"",
        "\"to\": \"channel:key_in\"",
    );
    let e = atomik_model::load_project(&looped, &reg).unwrap_err();
    assert!(
        matches!(&e, ModelError::Patch(m) if m.starts_with("cables[") && m.contains("cycle")),
        "{e:?}"
    );
}
