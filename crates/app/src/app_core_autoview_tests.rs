//! Controls that follow automation, through the core: the recording sink with a scripted
//! transport, a lane on a synth and on its channel strip. The views show the lane's value at
//! the transport position, the document keeps its own, and what the user holds stays theirs.

use super::tests::rack_only_core;
use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_engine::TransportState;
use atomik_model::{AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey};
use atomik_model::{SongCommand, WriteMode};
use atomik_ui::{TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";
const CUTOFF: &str = "filter.cutoff";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

/// A clip of linear automation points `(tick, value)` from tick 0 for `bars` bars.
fn clip(bars: i64, points: &[(i64, f32)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: bars * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Automation(
            points
                .iter()
                .map(|&(tick, value)| AutomationPoint {
                    tick,
                    value,
                    curve: Curve::Linear,
                })
                .collect(),
        ),
    }
}

/// Synth `a` with a track and a cutoff lane that ramps from 300 at bar 0 to 3000 at bar 1 and
/// holds; the start-up calls taken.
fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for cmd in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        },
        song(SongCommand::AddTrack { device: key("a") }),
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: CUTOFF.into(),
        }),
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip: clip(4, &[(0, 300.0), (BAR, 3000.0)]),
        }),
    ] {
        c.apply_command(cmd).unwrap();
    }
    c.sink_mut().take();
    c
}

/// The engine is playing at song tick `tick`; one UI frame runs.
fn playing_at(c: &mut AppCore<RecordingSink>, tick: i64) {
    c.sink_mut().state = TransportState {
        playing: true,
        tick,
        engine_frame: 1000 + tick as u64,
        ..TransportState::default()
    };
    c.playing = true;
    c.frame();
}

fn shown(c: &AppCore<RecordingSink>) -> f32 {
    c.view.shown_value(&key("a"), CUTOFF).unwrap()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

fn undo_depth(c: &mut AppCore<RecordingSink>) -> usize {
    let mut n = 0;
    while c.undo() {
        n += 1;
    }
    while c.redo() {}
    n
}

#[test]
fn the_control_follows_the_lane_while_the_song_plays_and_the_document_stays() {
    let mut c = core();
    c.set_saved(c.doc().clone());
    let doc = c.doc().clone();
    let stored = c
        .doc()
        .param_value(&key("a"), CUTOFF, c.registry())
        .unwrap();
    for (tick, want) in [
        (0, 300.0),
        (BAR / 2, 1650.0),
        (BAR, 3000.0),
        (3 * BAR, 3000.0),
    ] {
        playing_at(&mut c, tick);
        assert!(
            near(shown(&c), want),
            "tick {tick}: {} vs {want}",
            shown(&c)
        );
    }
    assert_eq!(c.doc(), &doc, "playback does not change the document");
    assert_eq!(
        c.doc().param_value(&key("a"), CUTOFF, c.registry()),
        Some(stored)
    );
    assert!(!c.is_dirty(), "playback does not dirty the project");
}

#[test]
fn playback_adds_no_undo_step() {
    let mut c = core();
    let before = undo_depth(&mut c);
    for t in [0, BAR / 2, BAR] {
        playing_at(&mut c, t);
    }
    assert_eq!(undo_depth(&mut c), before);
}

#[test]
fn a_stopped_transport_shows_the_value_at_the_playhead() {
    let mut c = core();
    c.transport_action(TransportAction::Locate(BAR / 2));
    c.frame();
    assert!(near(shown(&c), 1650.0), "{}", shown(&c));
    c.transport_action(TransportAction::Locate(BAR / 4));
    c.frame();
    assert!(near(shown(&c), 975.0), "{}", shown(&c));
    c.transport_action(TransportAction::ReturnToZero);
    c.frame();
    assert!(near(shown(&c), 300.0));
}

#[test]
fn a_param_edited_with_the_song_stopped_shows_the_edit_until_the_playhead_moves() {
    let mut c = core();
    c.transport_action(TransportAction::Locate(BAR / 2));
    c.frame();
    c.apply_command(Command::SetParam {
        key: key("a"),
        param: CUTOFF.into(),
        value: 5000.0,
        merge: None,
    })
    .unwrap();
    assert!(near(shown(&c), 5000.0), "the knob shows the user's value");
    c.frame();
    assert!(near(shown(&c), 5000.0), "and keeps it frame after frame");
    c.transport_action(TransportAction::Locate(BAR / 4));
    c.frame();
    assert!(near(shown(&c), 975.0), "a locate shows the lane again");
}

#[test]
fn a_muted_track_plays_no_automation_so_the_control_shows_its_own_value() {
    let mut c = core();
    c.apply_command(song(SongCommand::SetTrackFlags {
        device: key("a"),
        muted: true,
        solo: false,
        armed: false,
    }))
    .unwrap();
    let stored = c
        .doc()
        .param_value(&key("a"), CUTOFF, c.registry())
        .unwrap();
    playing_at(&mut c, BAR / 2);
    assert!(near(shown(&c), stored));
    // The lane is still marked: it exists.
    assert!(c.view.is_automated(&key("a"), CUTOFF));
}

#[test]
fn a_lane_a_take_is_writing_shows_the_users_value_and_follows_the_lane_again_on_release() {
    let mut c = core();
    c.apply_command(song(SongCommand::SetLaneWriteMode {
        device: key("a"),
        param: CUTOFF.into(),
        mode: WriteMode::Touch,
    }))
    .unwrap();
    c.handle_action(UiAction::ToggleLaneArm {
        device: key("a"),
        param: CUTOFF.into(),
    });
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    playing_at(&mut c, BAR / 4);
    assert!(near(shown(&c), 975.0), "before the touch the lane shows");
    c.apply_command(Command::SetParam {
        key: key("a"),
        param: CUTOFF.into(),
        value: 6000.0,
        merge: Some(7),
    })
    .unwrap();
    playing_at(&mut c, BAR / 2);
    assert!(near(shown(&c), 6000.0), "held: the user's value");
    playing_at(&mut c, BAR / 2 + 100);
    assert!(near(shown(&c), 6000.0));
    c.handle_action(UiAction::EndMerge);
    let tick = BAR / 2 + 200;
    playing_at(&mut c, tick);
    let want = 300.0 + 2700.0 * (tick as f32 / BAR as f32);
    assert!(
        near(shown(&c), want),
        "released: the lane again, {}",
        shown(&c)
    );
}

#[test]
fn an_unchanged_value_re_records_no_panel_and_a_changed_one_only_its_own() {
    let mut c = core();
    playing_at(&mut c, BAR / 2);
    let revs = c.view.panel_revisions();
    playing_at(&mut c, BAR / 2);
    assert_eq!(c.view.panel_revisions(), revs, "same tick, same values");
    playing_at(&mut c, BAR / 2 + 480);
    let now = c.view.panel_revisions();
    for ((k, a), (_, b)) in revs.iter().zip(&now) {
        assert_eq!(a != b, *k == key("a"), "{k:?}");
    }
}

// ---- a lane on a channel strip and on the master ---------------------------------------------

/// Synth `a` on its console channel, a lane on the channel's fader (a ramp from 0.2 at bar 0 to
/// 0.8 at bar 1) and one on the master's first return level, each on a track of its own.
fn console_core() -> (AppCore<RecordingSink>, DeviceKey, DeviceKey) {
    // The full registry: an instrument gets its console channel and the master.
    let mut c = AppCore::with_registry(
        RecordingSink::new(),
        atomik_render::default_registry(),
        atomik_model::empty_doc(),
        None,
    );
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("a")),
    })
    .unwrap();
    let channel = c.doc().console.channels[0].channel.clone();
    let master = c.doc().console.master.clone().expect("a master");
    for (device, param, points) in [
        (&channel, "fader", [(0, 0.2), (BAR, 0.8)]),
        (&master, "return1.level", [(0, -40.0), (BAR, -10.0)]),
    ] {
        for cmd in [
            song(SongCommand::AddAutomationLane {
                device: device.clone(),
                param: param.into(),
            }),
            song(SongCommand::AddClip {
                device: device.clone(),
                lane: 0,
                clip: clip(4, &points),
            }),
        ] {
            c.apply_command(cmd).unwrap();
        }
    }
    c.sink_mut().take();
    (c, channel, master)
}

#[test]
fn a_channel_strip_knob_follows_its_lane_and_shows_the_mark() {
    let (mut c, channel, _) = console_core();
    assert!(c.console.is_automated(&channel, "fader"));
    let stored = c
        .doc()
        .param_value(&channel, "fader", c.registry())
        .unwrap();
    let doc = c.doc().clone();
    for (tick, want) in [(0, 0.2), (BAR / 2, 0.5), (BAR, 0.8), (3 * BAR, 0.8)] {
        playing_at(&mut c, tick);
        let got = c.console.shown_value(&channel, "fader").unwrap();
        assert!((got - want).abs() < 1e-3, "tick {tick}: {got} vs {want}");
    }
    assert_eq!(c.doc(), &doc, "playback does not change the document");
    // A muted automation track plays nothing: the knob shows its own value, and keeps its mark.
    c.apply_command(song(SongCommand::SetTrackFlags {
        device: channel.clone(),
        muted: true,
        solo: false,
        armed: false,
    }))
    .unwrap();
    playing_at(&mut c, BAR / 2);
    let got = c.console.shown_value(&channel, "fader").unwrap();
    assert!((got - stored).abs() < 1e-3, "{got} vs {stored}");
    assert!(c.console.is_automated(&channel, "fader"));
}

#[test]
fn the_master_follows_its_lane_and_the_rack_panel_of_a_channel_too() {
    let (mut c, channel, master) = console_core();
    assert!(c.console.is_automated(&master, "return1.level"));
    playing_at(&mut c, BAR / 2);
    let got = c.console.shown_value(&master, "return1.level").unwrap();
    assert!((got + 25.0).abs() < 0.01, "{got}");
    // The rack shows the channel device the same way when its console devices are visible.
    let rack = c.view.shown_value(&channel, "fader");
    assert!(rack.is_none_or(|v| (v - 0.5).abs() < 1e-3), "{rack:?}");
}

// ---- the gates equal an always-sync reference ------------------------------------------------

/// The value the control must show: the uncached model answer for the lane, else the stored one.
fn reference(c: &AppCore<RecordingSink>, tick: i64) -> f32 {
    c.doc()
        .automation_values_at(tick as f64)
        .get(&(key("a"), CUTOFF.to_string()))
        .copied()
        .unwrap_or_else(|| {
            c.doc()
                .param_value(&key("a"), CUTOFF, c.registry())
                .unwrap()
        })
}

#[test]
fn the_gated_values_equal_the_uncached_ones_on_a_random_event_stream() {
    let mut c = core();
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n
    };
    let mut muted = false;
    let mut tick = 0;
    for step in 0..300 {
        match next(5) {
            0 => {
                muted = !muted;
                c.apply_command(song(SongCommand::SetTrackFlags {
                    device: key("a"),
                    muted,
                    solo: false,
                    armed: false,
                }))
                .unwrap();
            }
            1 => {
                c.undo();
            }
            2 => {
                c.redo();
            }
            _ => tick = next(3 * BAR as u64) as i64,
        }
        if next(4) == 0 {
            // A frame at the same position and revision: the gate holds, the answer is the same.
            playing_at(&mut c, tick);
        }
        playing_at(&mut c, tick);
        let want = reference(&c, tick);
        assert!(
            near(shown(&c), want),
            "step {step} tick {tick}: {} vs {want}",
            shown(&c)
        );
    }
}

#[test]
fn an_unchanged_revision_and_position_asks_the_views_for_nothing() {
    let mut c = core();
    playing_at(&mut c, BAR / 2);
    assert!(!c.update_shown_values(), "nothing moved");
    c.position = BAR / 2 + 480;
    assert!(
        c.update_shown_values(),
        "a moved playhead changes the values"
    );
}
