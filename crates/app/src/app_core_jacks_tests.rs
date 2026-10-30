//! The jack menu's choices in the app core: each is one command and one undo step.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{Cable, SocketRef};
use atomik_ui::JackOp;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sock(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

fn cable(from: (&str, &str), to: (&str, &str)) -> Cable {
    Cable {
        from: sock(from.0, from.1),
        to: sock(to.0, to.1),
    }
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for (t, k) in [
        ("atomik.poly-synth", "synth"),
        ("atomik.stereo-delay", "delay"),
        ("atomik.stereo-delay", "delay2"),
    ] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: t.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    c
}

fn op(c: &mut AppCore<RecordingSink>, op: JackOp) -> bool {
    c.jack_op(&op.encode())
}

#[test]
fn a_connect_is_one_undo_step() {
    let mut c = core();
    let before = c.doc().cables.len();
    assert!(op(
        &mut c,
        JackOp::Connect(cable(("synth", "out_l"), ("delay", "in_l")))
    ));
    assert_eq!(c.doc().cables.len(), before + 1);
    assert!(c.undo());
    assert_eq!(c.doc().cables.len(), before);
}

#[test]
fn a_replace_swaps_the_cable_in_one_undo_step() {
    let mut c = core();
    let first = cable(("synth", "out_l"), ("delay", "in_l"));
    let second = cable(("delay2", "out_l"), ("delay", "in_l"));
    assert!(op(&mut c, JackOp::Connect(first.clone())));
    assert!(op(&mut c, JackOp::Replace(second.clone())));
    assert!(c.doc().cables.contains(&second) && !c.doc().cables.contains(&first));
    assert!(c.undo());
    assert!(c.doc().cables.contains(&first) && !c.doc().cables.contains(&second));
}

#[test]
fn a_disconnect_cuts_every_cable_of_the_jack_in_one_step() {
    let mut c = core();
    for to in [("delay", "in_l"), ("delay2", "in_l")] {
        assert!(op(&mut c, JackOp::Connect(cable(("synth", "out_l"), to))));
    }
    let both = c.doc().cables.len();
    let of_jack = c
        .doc()
        .cables
        .iter()
        .filter(|x| x.from == sock("synth", "out_l"))
        .count();
    assert!(of_jack >= 2);
    assert!(op(&mut c, JackOp::Disconnect(sock("synth", "out_l"))));
    assert_eq!(c.doc().cables.len(), both - of_jack);
    assert!(c.undo());
    assert_eq!(c.doc().cables.len(), both);
    // Nothing to cut is not a step.
    assert!(!op(&mut c, JackOp::Disconnect(sock("delay", "out_r"))));
}

#[test]
fn a_refused_cable_changes_nothing() {
    let mut c = core();
    let before = c.doc().cables.len();
    // An output to an output.
    assert!(!op(
        &mut c,
        JackOp::Connect(cable(("synth", "out_l"), ("delay", "out_l")))
    ));
    assert_eq!(c.doc().cables.len(), before);
    assert!(!c.jack_op("garbage"));
}

#[test]
fn a_new_channel_is_one_undo_step() {
    let mut c = core();
    let devices = c.doc().devices.len();
    assert!(op(
        &mut c,
        JackOp::NewChannel {
            device: key("delay"),
            out_l: "out_l".into(),
            out_r: "out_r".into(),
        }
    ));
    assert!(c.doc().devices.len() > devices);
    assert!(c.undo());
    assert_eq!(c.doc().devices.len(), devices);
}

#[test]
fn revealing_a_device_only_moves_the_view() {
    let mut c = core();
    let rev = c.doc().cables.len();
    assert!(op(&mut c, JackOp::Reveal(key("delay"))));
    assert!(!op(&mut c, JackOp::Reveal(key("missing"))));
    assert_eq!(c.doc().cables.len(), rev);
}
