//! Stereo-pair patching, the cable drag that ends only at the release, and the jack menu.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{Flip, JackOp, Key, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::*;

fn back_view() -> (RackDoc, Registry, RackView) {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    (doc, reg, v)
}

fn alt_up(pos: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    }
}

/// Drags from jack `from` to jack `to` and returns what the release emitted.
fn drag(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    from: (&str, &str),
    to: (&str, &str),
    release: impl Fn(Point) -> UiEvent,
) -> Vec<Command> {
    let a = v.socket_center(&sref(from.0, from.1)).unwrap();
    let b = v.socket_center(&sref(to.0, to.1)).unwrap();
    let _ = v.handle(&down(a), doc, reg);
    let _ = v.handle(&mv(b), doc, reg);
    commands(&v.handle(&release(b), doc, reg))
}

#[test]
fn left_to_left_cables_the_pair_in_one_step() {
    let (doc, reg, mut v) = back_view();
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "out_l"),
        ("mixer", "ch2_in_l"),
        up,
    );
    assert_eq!(
        got,
        vec![Command::Batch(vec![
            Command::Connect(cable(("synth", "out_l"), ("mixer", "ch2_in_l"))),
            Command::Connect(cable(("synth", "out_r"), ("mixer", "ch2_in_r"))),
        ])]
    );
}

#[test]
fn right_to_right_cables_the_left_pair_too() {
    let (doc, reg, mut v) = back_view();
    // Dragging from the input side gives the same pair.
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("mixer", "ch2_in_r"),
        ("synth", "out_r"),
        up,
    );
    assert_eq!(
        got,
        vec![Command::Batch(vec![
            Command::Connect(cable(("synth", "out_r"), ("mixer", "ch2_in_r"))),
            Command::Connect(cable(("synth", "out_l"), ("mixer", "ch2_in_l"))),
        ])]
    );
}

#[test]
fn alt_connects_one_cable() {
    let (doc, reg, mut v) = back_view();
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "out_l"),
        ("mixer", "ch2_in_l"),
        alt_up,
    );
    assert_eq!(
        got,
        vec![Command::Connect(cable(
            ("synth", "out_l"),
            ("mixer", "ch2_in_l")
        ))]
    );
}

#[test]
fn mismatched_sides_and_other_sockets_connect_one() {
    let (doc, reg, mut v) = back_view();
    // Left to right is not a pair.
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "out_l"),
        ("mixer", "ch2_in_r"),
        up,
    );
    assert_eq!(
        got,
        vec![Command::Connect(cable(
            ("synth", "out_l"),
            ("mixer", "ch2_in_r")
        ))]
    );
    // A CV cable has no pair.
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "lfo_cv_out"),
        ("mixer", "ch2_level_cv_in"),
        up,
    );
    assert_eq!(got.len(), 1);
    assert!(matches!(got[0], Command::Connect(_)));
}

#[test]
fn a_taken_partner_input_leaves_one_cable() {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.cables
        .push(cable(("delay", "out_r"), ("mixer", "ch2_in_r")));
    doc.cables.sort();
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "out_l"),
        ("mixer", "ch2_in_l"),
        up,
    );
    assert_eq!(
        got,
        vec![Command::Connect(cable(
            ("synth", "out_l"),
            ("mixer", "ch2_in_l")
        ))]
    );
}

#[test]
fn replacing_an_occupied_input_keeps_the_pair_in_the_same_step() {
    let (doc, reg, mut v) = back_view();
    // mixer.ch1_in_l has the delay's cable; ch1_in_r is free.
    let got = drag(
        &mut v,
        &doc,
        &reg,
        ("synth", "out_l"),
        ("mixer", "ch1_in_l"),
        up,
    );
    assert_eq!(
        got,
        vec![Command::Batch(vec![
            Command::Disconnect {
                to: sref("mixer", "ch1_in_l")
            },
            Command::Connect(cable(("synth", "out_l"), ("mixer", "ch1_in_l"))),
            Command::Connect(cable(("synth", "out_r"), ("mixer", "ch1_in_r"))),
        ])]
    );
}

#[test]
fn escape_after_pressing_a_connected_input_leaves_the_cable() {
    let (doc, reg, mut v) = back_view();
    let input = v.socket_center(&sref("delay", "in_l")).unwrap();
    assert!(commands(&v.handle(&down(input), &doc, &reg)).is_empty());
    // The picked-up cable is not drawn while it is held.
    assert!(
        !v.visible_cables(&doc)
            .contains(&cable(("synth", "out_l"), ("delay", "in_l")))
    );
    let esc = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    assert!(commands(&v.handle(&esc, &doc, &reg)).is_empty());
    assert!(commands(&v.handle(&up(input), &doc, &reg)).is_empty());
    // Back in place, drawn again.
    assert!(
        v.visible_cables(&doc)
            .contains(&cable(("synth", "out_l"), ("delay", "in_l")))
    );
}

#[test]
fn dropping_a_picked_up_cable_on_nothing_cuts_it() {
    let (doc, reg, mut v) = back_view();
    let input = v.socket_center(&sref("delay", "in_l")).unwrap();
    let _ = v.handle(&down(input), &doc, &reg);
    let nowhere = Point::new(5.0, 5.0);
    let _ = v.handle(&mv(nowhere), &doc, &reg);
    assert_eq!(
        commands(&v.handle(&up(nowhere), &doc, &reg)),
        vec![Command::Disconnect {
            to: sref("delay", "in_l")
        }]
    );
}

#[test]
fn dropping_a_picked_up_cable_on_its_own_input_changes_nothing() {
    let (doc, reg, mut v) = back_view();
    let input = v.socket_center(&sref("delay", "in_l")).unwrap();
    let _ = v.handle(&down(input), &doc, &reg);
    let _ = v.handle(
        &mv(input + atomik_ui::kurbo::Vec2::new(40.0, 0.0)),
        &doc,
        &reg,
    );
    let _ = v.handle(&mv(input), &doc, &reg);
    assert!(commands(&v.handle(&up(input), &doc, &reg)).is_empty());
}

#[test]
fn the_tooltip_of_a_jack_names_its_peer() {
    let (doc, reg, mut v) = back_view();
    let input = v.socket_center(&sref("delay", "in_l")).unwrap();
    let _ = v.handle(&mv(input), &doc, &reg);
    let tip = v.tooltip().unwrap();
    assert!(tip.contains("from Poly Synth > Out L"), "{tip}");
    // The other end names the delay.
    let out = v.socket_center(&sref("synth", "out_l")).unwrap();
    let _ = v.handle(&mv(out), &doc, &reg);
    let tip = v.tooltip().unwrap();
    assert!(tip.contains("to Stereo Delay >"), "{tip}");
    // A free jack shows only its name.
    let free = v.socket_center(&sref("synth", "out_r")).unwrap();
    let _ = v.handle(&mv(free), &doc, &reg);
    assert_eq!(v.tooltip().as_deref(), Some("Out R"));
}

// ---------------------------------------------------------------- the jack menu

#[test]
fn a_right_click_on_a_jack_asks_for_its_menu() {
    let (doc, reg, mut v) = back_view();
    let p = v.socket_center(&sref("synth", "out_l")).unwrap();
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert!(
        a.iter()
            .any(|x| matches!(x, UiAction::JackMenu { socket, .. }
            if *socket == sref("synth", "out_l")))
    );
}

#[test]
fn the_menu_lists_compatible_jacks_per_device_and_stars_occupied_ones() {
    let (doc, reg, v) = back_view();
    let m = v.jack_menu(&sref("synth", "out_l"), &doc, &reg).unwrap();
    let devices: Vec<&str> = m.groups.iter().map(|g| g.device.as_str()).collect();
    assert!(!devices.contains(&"Poly Synth"));
    assert!(devices.contains(&"Stereo Delay") && devices.contains(&"Line Mixer"));
    // Rack order: delay before mixer.
    let pos = |n: &str| devices.iter().position(|d| *d == n).unwrap();
    assert!(pos("Stereo Delay") < pos("Line Mixer"));
    let mixer = m.groups.iter().find(|g| g.device == "Line Mixer").unwrap();
    let free = mixer
        .jacks
        .iter()
        .find(|j| j.op == JackOp::Connect(cable(("synth", "out_l"), ("mixer", "ch2_in_l"))));
    let taken = mixer
        .jacks
        .iter()
        .find(|j| j.op == JackOp::Replace(cable(("synth", "out_l"), ("mixer", "ch1_in_l"))))
        .expect("the occupied input is offered as a replace");
    assert!(taken.occupied);
    assert!(free.is_some_and(|j| !j.occupied));
    // The cable that exists is not offered again.
    let delay = m
        .groups
        .iter()
        .find(|g| g.device == "Stereo Delay")
        .unwrap();
    assert!(
        !delay
            .jacks
            .iter()
            .any(|j| j.op == JackOp::Connect(cable(("synth", "out_l"), ("delay", "in_l"))))
    );
    // Only audio inputs: no CV jack of the mixer.
    assert!(mixer.jacks.iter().all(|j| !j.label.contains("CV")));
    assert_eq!(m.peers.len(), 1);
    assert_eq!(m.peers[0].name, "Stereo Delay");
}

#[test]
fn the_menu_of_an_input_lists_outputs_and_offers_to_replace_its_own_cable() {
    let (doc, reg, v) = back_view();
    let m = v.jack_menu(&sref("delay", "in_l"), &doc, &reg).unwrap();
    // The jack is occupied, so every output replaces its cable.
    let synth = m.groups.iter().find(|g| g.device == "Poly Synth").unwrap();
    assert!(
        synth
            .jacks
            .iter()
            .any(|j| j.op == JackOp::Replace(cable(("synth", "out_r"), ("delay", "in_l"))))
    );
    assert_eq!(m.peers[0].name, "Poly Synth");
}

#[test]
fn the_menu_refuses_a_cable_that_would_close_a_loop() {
    let (doc, reg, v) = back_view();
    // delay -> mixer -> interface: the mixer's outputs cannot feed the delay's inputs.
    let m = v.jack_menu(&sref("mixer", "master_r"), &doc, &reg).unwrap();
    assert!(m.groups.iter().all(|g| g.device != "Stereo Delay"));
    let m = v.jack_menu(&sref("delay", "out_r"), &doc, &reg).unwrap();
    assert!(m.groups.iter().all(|g| g.device != "Poly Synth"));
}

#[test]
fn a_new_channel_is_offered_for_a_free_output_pair() {
    let (mut doc, reg) = (reference_doc(), registry());
    // Free the synth's outputs.
    doc.cables
        .retain(|c| c.from.device != key("synth") || c.from.socket == "lfo_cv_out");
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    let m = v.jack_menu(&sref("synth", "out_r"), &doc, &reg).unwrap();
    assert_eq!(
        m.channel,
        Some(JackOp::NewChannel {
            device: key("synth"),
            out_l: "out_l".into(),
            out_r: "out_r".into(),
        })
    );
    // An input, a cabled output and a CV output offer none.
    let m = v.jack_menu(&sref("delay", "in_l"), &doc, &reg).unwrap();
    assert!(m.channel.is_none());
    let m = v.jack_menu(&sref("delay", "out_l"), &doc, &reg).unwrap();
    assert!(m.channel.is_none());
    let m = v
        .jack_menu(&sref("synth", "lfo_cv_out"), &doc, &reg)
        .unwrap();
    assert!(m.channel.is_none());
}

#[test]
fn ops_round_trip_and_build_one_undo_step() {
    let doc = reference_doc();
    let c = cable(("synth", "out_r"), ("delay", "in_r"));
    for op in [
        JackOp::Connect(c.clone()),
        JackOp::Replace(c.clone()),
        JackOp::Disconnect(sref("delay", "in_l")),
        JackOp::Reveal(key("mixer")),
        JackOp::NewChannel {
            device: key("synth"),
            out_l: "out_l".into(),
            out_r: "out_r".into(),
        },
    ] {
        assert_eq!(JackOp::parse(&op.encode()), Some(op));
    }
    assert_eq!(JackOp::parse("nonsense"), None);
    assert!(matches!(
        JackOp::Replace(c).command(&doc),
        Some(Command::Batch(v)) if v.len() == 2
    ));
    // An input has one cable to cut, an output with two cuts them in one batch.
    assert_eq!(
        JackOp::Disconnect(sref("delay", "in_l")).command(&doc),
        Some(Command::Disconnect {
            to: sref("delay", "in_l")
        })
    );
    let mut busy = doc.clone();
    busy.cables
        .push(cable(("synth", "out_l"), ("mixer", "ch2_in_l")));
    assert!(matches!(
        JackOp::Disconnect(sref("synth", "out_l")).command(&busy),
        Some(Command::Batch(v)) if v.len() == 2
    ));
    assert_eq!(JackOp::Reveal(key("mixer")).command(&doc), None);
}

#[test]
fn scrolling_to_a_device_reaches_another_column() {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.columns = 2;
    doc.devices[2].column = 1;
    let mut v = RackView::new(atomik_ui::Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(700.0, 1200.0)), &doc, &reg);
    v.sync(&doc, &reg);
    let seen = |v: &RackView| {
        let p = v.plate_center(&key("mixer")).unwrap();
        p.x > 0.0 && p.x < 700.0
    };
    assert!(!seen(&v), "the second column starts off screen");
    assert!(v.reveal_key(&key("mixer")));
    assert!(seen(&v));
    assert!(!v.reveal_key(&key("nothing")));
}
