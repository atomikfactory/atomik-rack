//! The transport while a MIDI clock is followed: the buttons do nothing and say why, and the
//! playing state is the engine's.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use crate::status::SyncStatus;
use atomik_model::empty_doc;

fn core(following: bool) -> AppCore<RecordingSink> {
    let sink = RecordingSink {
        sync: SyncStatus {
            following,
            ..SyncStatus::default()
        },
        ..RecordingSink::default()
    };
    AppCore::with_registry(sink, default_registry(), empty_doc(), None)
}

fn transports(c: &mut AppCore<RecordingSink>) -> Vec<TransportCommand> {
    c.sink_mut()
        .take()
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::Transport(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn the_transport_buttons_do_nothing_while_a_clock_is_followed_and_say_why() {
    let mut c = core(true);
    transports(&mut c);
    for action in [
        TransportAction::Play,
        TransportAction::Stop,
        TransportAction::TogglePlay,
        TransportAction::ReturnToZero,
        TransportAction::Locate(960),
        TransportAction::ToggleRecord,
    ] {
        c.transport_action(action);
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.contains("follows the MIDI clock")),
            "{action:?}"
        );
        assert!(
            transports(&mut c).is_empty(),
            "{action:?} reached the engine"
        );
    }
    assert!(!c.is_playing());
}

#[test]
fn without_a_followed_clock_the_transport_is_the_apps() {
    let mut c = core(false);
    transports(&mut c);
    c.transport_action(TransportAction::Play);
    assert!(c.is_playing());
    assert!(
        transports(&mut c)
            .iter()
            .any(|t| matches!(t, TransportCommand::Play))
    );
    // The settings that are not the transport still work while following.
    let mut c = core(true);
    c.transport_action(TransportAction::ToggleMetronome);
    assert!(
        transports(&mut c)
            .iter()
            .any(|t| matches!(t, TransportCommand::Metronome { .. }))
    );
}

#[test]
fn the_playing_state_follows_the_engine_while_a_clock_is_followed() {
    let mut c = core(true);
    assert!(!c.is_playing());
    c.sink_mut().state.playing = true;
    c.frame();
    assert!(c.is_playing());
    c.sink_mut().state.playing = false;
    c.frame();
    assert!(!c.is_playing());
}
