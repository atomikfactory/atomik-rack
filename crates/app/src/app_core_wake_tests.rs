//! The core's answer to "do you want fast frames": each group of causes turns it on and, once it
//! is over, off again.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::ChannelMessage;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{TransportAction, UiAction};

fn core() -> AppCore<RecordingSink> {
    AppCore::new(RecordingSink::new())
}

#[test]
fn a_core_at_rest_wants_no_fast_frames() {
    let mut c = core();
    c.frame();
    assert!(!c.wants_fast_frames(1.0));
}

#[test]
fn the_transport_wants_frames_while_it_plays_and_lets_go_when_it_stops() {
    let mut c = core();
    c.handle_action(UiAction::Transport(TransportAction::Play));
    assert!(c.wants_fast_frames(1.0));
    c.handle_action(UiAction::Transport(TransportAction::Stop));
    for _ in 0..8 {
        c.frame();
    }
    assert!(!c.wants_fast_frames(2.0));
}

#[test]
fn a_momentary_pulse_wants_frames_until_it_is_let_go() {
    let mut c = core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.gain-tool".into(),
        key: Some(DeviceKey("g".into())),
    })
    .expect("a gain tool");
    c.handle_action(UiAction::PulseParam {
        device: DeviceKey("g".into()),
        param: "peak_reset".into(),
    });
    assert!(c.wants_fast_frames(1.0), "the param is up for two frames");
    for _ in 0..4 {
        c.frame();
    }
    assert!(!c.wants_fast_frames(2.0));
}

#[test]
fn live_midi_wants_frames_for_a_second_after_the_last_message() {
    let mut c = core();
    c.inject_midi("Pad", ChannelMessage::cc(0, 1, 64));
    assert!(c.wants_fast_frames(10.0), "waiting in the hub");
    c.midi_control_frame(10.0);
    assert!(c.wants_fast_frames(10.0));
    assert!(c.wants_fast_frames(10.9), "still inside the second");
    assert!(!c.wants_fast_frames(11.1), "aged out");
    assert!(!c.wants_fast_frames(60.0));
    // Another message starts the second again.
    c.inject_midi("Pad", ChannelMessage::cc(0, 1, 65));
    c.midi_control_frame(60.0);
    assert!(c.wants_fast_frames(60.5));
    assert!(!c.wants_fast_frames(61.5));
}

#[test]
fn a_waker_on_the_hub_is_called_for_every_message() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let c = core();
    let woken = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&woken);
    c.control_hub().set_waker(Arc::new(move || {
        seen.fetch_add(1, Ordering::Relaxed);
    }));
    c.inject_midi("Pad", ChannelMessage::cc(0, 1, 64));
    c.inject_midi("Pad", ChannelMessage::note_on(0, 60, 90));
    assert_eq!(woken.load(Ordering::Relaxed), 2);
}

#[test]
fn midi_learn_wants_frames_until_it_is_cancelled() {
    let mut c = core();
    c.set_midi_learn(true);
    assert!(c.wants_fast_frames(1.0));
    c.set_midi_learn(false);
    assert!(!c.wants_fast_frames(1.0));
}
