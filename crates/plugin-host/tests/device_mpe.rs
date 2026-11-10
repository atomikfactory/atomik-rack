//! (events) The per-plugin MPE option: with `mpe.send` on, a plugin that takes MIDI gets the
//! notes as a lower MPE zone (raw messages on member channels 2 to 16), the expression of each
//! note as that channel's bend, pressure and CC 74, and the configuration message when the option
//! is switched on and when playback starts; with it off the plugin gets note events as always.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{ExprKind, NoteEvent, NoteKind};
use atomik_plugin_host::{PluginEvent, PluginEventKind as K};
use support::*;

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

fn on(offset: u16, key: u8) -> NoteEvent {
    ev(offset, NoteKind::On { key, velocity: 1.0 })
}

fn off(offset: u16, key: u8) -> NoteEvent {
    ev(offset, NoteKind::Off { key })
}

fn expr(offset: u16, key: u8, kind: ExprKind, value: f32) -> NoteEvent {
    ev(offset, NoteKind::NoteExpr { key, kind, value })
}

fn synth(
    id: &str,
) -> (
    atomik_testkit::harness::DeviceHarness,
    Rest,
    std::sync::Arc<ProbeShared>,
) {
    let (probe, shared) = Probe::new();
    let (h, r) = harness(rig(id, &synth_layout(), Box::new(probe)));
    (h, r, shared)
}

fn events(shared: &ProbeShared, call: usize) -> Vec<PluginEvent> {
    shared.log.lock().unwrap().events_of_call(call).to_vec()
}

/// The raw MIDI messages of a call, in order, as `(offset, bytes)`; other events are `None`.
fn midi(e: &[PluginEvent]) -> Vec<(u16, Vec<u8>)> {
    e.iter()
        .filter_map(|e| match e.kind {
            K::Midi { bytes, len } => Some((e.offset, bytes[..usize::from(len)].to_vec())),
            _ => None,
        })
        .collect()
}

const MCM_ON: [[u8; 3]; 5] = [
    [0xB0, 101, 0],
    [0xB0, 100, 6],
    [0xB0, 6, 15],
    [0xB0, 101, 127],
    [0xB0, 100, 127],
];

#[test]
fn the_option_is_the_last_param_and_is_off_by_default() {
    let (h, r, _s) = synth("mpe.param");
    let params = r.descriptor.params;
    let last = params.last().unwrap();
    assert_eq!(last.key, "mpe.send");
    assert_eq!(last.default, 0.0);
    assert_eq!(last.name, "Send MPE (channels 2-16)");
    drop(h);
}

#[test]
fn with_the_option_off_notes_stay_note_events() {
    let (mut h, _r, shared) = synth("mpe.off");
    h.push_note("notes_in", on(3, 60));
    h.push_note("notes_in", expr(3, 60, ExprKind::Tuning, 2.0));
    h.process();
    let e = events(&shared, 0);
    assert!(midi(&e).is_empty());
    assert!(matches!(e[0].kind, K::NoteOn { key: 60, .. }));
    assert!(matches!(e[1].kind, K::NoteExpr { key: 60, .. }));
}

#[test]
fn switching_it_on_announces_the_zone_and_the_notes_take_member_channels() {
    let (mut h, _r, shared) = synth("mpe.on");
    h.set_param("mpe.send", 1.0);
    h.push_note("notes_in", on(5, 60));
    h.push_note("notes_in", on(6, 64));
    h.process();
    let m = midi(&events(&shared, 0));
    let mcm: Vec<Vec<u8>> = m[..5].iter().map(|(_, b)| b.clone()).collect();
    assert_eq!(mcm, MCM_ON.iter().map(|b| b.to_vec()).collect::<Vec<_>>());
    assert!(m[..5].iter().all(|(offset, _)| *offset == 0));
    assert_eq!(m[5], (5, vec![0x91, 60, 127]));
    assert_eq!(m[6], (6, vec![0x92, 64, 127]));
    // No note event goes out in MPE mode.
    assert!(
        events(&shared, 0)
            .iter()
            .all(|e| matches!(e.kind, K::Midi { .. }))
    );
    // Releases come on the same channels.
    h.push_note("notes_in", off(1, 64));
    h.push_note("notes_in", off(2, 60));
    h.process();
    assert_eq!(
        midi(&events(&shared, 1)),
        vec![(1, vec![0x82, 64, 0]), (2, vec![0x81, 60, 0])]
    );
}

#[test]
fn the_expression_of_a_note_is_its_channels_bend_pressure_and_slide() {
    let (mut h, _r, shared) = synth("mpe.expr");
    h.set_param("mpe.send", 1.0);
    h.process();
    h.push_note("notes_in", on(0, 60));
    h.push_note("notes_in", on(0, 62));
    h.push_note("notes_in", expr(10, 60, ExprKind::Tuning, 12.0));
    h.push_note("notes_in", expr(20, 62, ExprKind::Pressure, 1.0));
    h.push_note("notes_in", expr(30, 62, ExprKind::Slide, 0.5));
    h.push_note("notes_in", expr(40, 60, ExprKind::Tuning, -48.0));
    h.process();
    assert_eq!(
        midi(&events(&shared, 1)),
        vec![
            (0, vec![0x91, 60, 127]),
            (0, vec![0x92, 62, 127]),
            (10, vec![0xE1, 0x00, 0x50]),
            (20, vec![0xD2, 127]),
            (30, vec![0xB2, 74, 64]),
            (40, vec![0xE1, 0x00, 0x00]),
        ]
    );
}

#[test]
fn global_messages_go_to_the_master_channel() {
    let (mut h, _r, shared) = synth("mpe.master");
    h.set_param("mpe.send", 1.0);
    h.process();
    h.push_note(
        "notes_in",
        ev(
            0,
            NoteKind::Cc {
                controller: 1,
                value: 1.0,
            },
        ),
    );
    h.push_note("notes_in", ev(1, NoteKind::PitchBend { value: 0.0 }));
    h.push_note("notes_in", ev(2, NoteKind::ChannelPressure { value: 1.0 }));
    h.process();
    assert_eq!(
        midi(&events(&shared, 1)),
        vec![
            (0, vec![0xB0, 1, 127]),
            (1, vec![0xE0, 0, 64]),
            (2, vec![0xD0, 127])
        ]
    );
}

#[test]
fn the_zone_is_announced_again_when_playback_starts() {
    let (mut h, _r, shared) = synth("mpe.play");
    h.time_mut().playing = false;
    h.set_param("mpe.send", 1.0);
    h.process();
    assert_eq!(midi(&events(&shared, 0)).len(), 5);
    h.process();
    assert!(midi(&events(&shared, 1)).is_empty());
    h.time_mut().playing = true;
    h.process();
    assert_eq!(midi(&events(&shared, 2)).len(), 5, "playback started");
}

#[test]
fn all_off_releases_the_member_notes_and_the_zone() {
    let (mut h, _r, shared) = synth("mpe.alloff");
    h.set_param("mpe.send", 1.0);
    h.push_note("notes_in", on(0, 60));
    h.push_note("notes_in", on(0, 64));
    h.process();
    h.push_note("notes_in", ev(4, NoteKind::AllOff));
    h.process();
    assert_eq!(
        midi(&events(&shared, 1)),
        vec![
            (4, vec![0x81, 60, 0]),
            (4, vec![0x82, 64, 0]),
            (4, vec![0xB0, 123, 0]),
        ]
    );
}

#[test]
fn switching_it_on_ends_plain_notes_and_switching_it_off_ends_the_zone() {
    let (mut h, _r, shared) = synth("mpe.switch");
    h.push_note("notes_in", on(0, 60));
    h.process();
    h.set_param("mpe.send", 1.0);
    h.push_note("notes_in", on(2, 62));
    h.process();
    let e = events(&shared, 1);
    assert!(
        matches!(e[0].kind, K::NoteOff { key: 60, .. }),
        "the plain note ends as a note event: {e:?}"
    );
    assert_eq!(midi(&e).len(), 6, "the zone, then the member note");
    h.set_param("mpe.send", 0.0);
    h.process();
    let m = midi(&events(&shared, 2));
    assert_eq!(m[0], (0, vec![0x81, 62, 0]), "the member note is released");
    assert!(
        m.contains(&(0, vec![0xB0, 6, 0])),
        "the zone is switched off"
    );
    // Plain note events again.
    h.push_note("notes_in", on(0, 65));
    h.process();
    assert!(matches!(
        events(&shared, 3)[0].kind,
        K::NoteOn { key: 65, .. }
    ));
}
