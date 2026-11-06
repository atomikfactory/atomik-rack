//! The level lane of a VCA group through the core: recording a fader move into the lane, the
//! hold while the lane is written, the song that follows the members, and the mute that holds
//! the group's level.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_engine::TransportState;
use atomik_model::console::{CHANNEL_TYPE, VCA_DB_PARAM};
use atomik_model::{ClipContent, Command, ConsoleCommand, DeviceKey, SongCommand, vca_lane_key};
use atomik_ui::{TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn lane_key() -> String {
    vca_lane_key(1)
}

/// Synths `a` and `b` with their channels, a group over both channels and its level lane on the
/// master, the start-up calls taken.
fn core() -> (AppCore<RecordingSink>, DeviceKey, Vec<DeviceKey>) {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    let channels: Vec<DeviceKey> = ["a", "b"]
        .iter()
        .map(|k| c.doc().console.channel_of(&key(k)).unwrap().channel.clone())
        .collect();
    c.apply_command(Command::Console(ConsoleCommand::CreateVca {
        name: "Group".into(),
        members: channels.clone(),
    }))
    .unwrap();
    let master = c.doc().console.master.clone().unwrap();
    c.apply_command(Command::Song(SongCommand::AddAutomationLane {
        device: master.clone(),
        param: lane_key(),
    }))
    .unwrap();
    c.sink_mut().take();
    (c, master, channels)
}

fn at(c: &mut AppCore<RecordingSink>, tick: i64) {
    c.sink_mut().state = TransportState {
        playing: true,
        tick,
        engine_frame: 1000 + tick as u64,
        ..TransportState::default()
    };
}

fn level(c: &mut AppCore<RecordingSink>, db: f32, merge: Option<u64>) {
    c.apply_command(Command::Console(ConsoleCommand::SetVcaLevel {
        vca: 1,
        db,
        merge,
    }))
    .unwrap();
}

fn holds(calls: &[SinkCall]) -> Vec<Vec<(DeviceInstanceId, ParamId)>> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Hold(h) => Some(h.clone()),
            _ => None,
        })
        .collect()
}

fn vca_db(c: &AppCore<RecordingSink>) -> ParamId {
    c.registry()
        .get(CHANNEL_TYPE)
        .unwrap()
        .descriptor
        .param_by_key(VCA_DB_PARAM)
        .unwrap()
        .id
}

#[test]
fn a_fader_move_is_recorded_into_the_armed_level_lane() {
    let (mut c, master, channels) = core();
    c.handle_action(UiAction::ToggleLaneArm {
        device: master.clone(),
        param: lane_key(),
    });
    assert!(c.automation_armed(&master, &lane_key()));
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    assert!(c.is_recording(), "{:?}", c.last_rejection());
    c.sink_mut().take();
    at(&mut c, 1000);
    level(&mut c, -3.0, Some(5));
    // The lane is held on both members' `internal.vca_db` while it is written.
    let held: Vec<_> = channels
        .iter()
        .map(|ch| (c.device_id(ch).unwrap(), vca_db(&c)))
        .collect();
    let mut want = held.clone();
    want.sort();
    assert_eq!(holds(&c.sink_mut().take()), [want]);
    at(&mut c, 2000);
    level(&mut c, -9.0, Some(5));
    at(&mut c, 3000);
    level(&mut c, -15.0, Some(5));
    at(&mut c, 4000);
    c.handle_action(UiAction::EndMerge);
    at(&mut c, 6000);
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    assert!(!c.is_recording());
    let lane = &c.doc().song.track(&master).unwrap().lanes[0];
    assert_eq!(lane.clips.len(), 1);
    let ClipContent::Automation(points) = &lane.clips[0].content else {
        panic!("automation");
    };
    assert_eq!(points.first().map(|p| p.value), Some(-3.0));
    assert_eq!(points.last().map(|p| p.value), Some(-15.0));
    // The recorded take is one undo step.
    assert!(c.undo());
    assert!(
        c.doc().song.track(&master).unwrap().lanes[0]
            .clips
            .is_empty()
    );
}

#[test]
fn a_level_lane_plays_on_the_members_and_follows_them() {
    let (mut c, master, channels) = core();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: master.clone(),
        lane: 0,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: 15360,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Automation(vec![
                atomik_model::AutomationPoint {
                    tick: 0,
                    value: 0.0,
                    curve: atomik_model::Curve::Linear,
                },
                atomik_model::AutomationPoint {
                    tick: 7680,
                    value: -12.0,
                    curve: atomik_model::Curve::Linear,
                },
            ]),
        },
    }))
    .unwrap();
    let db = vca_db(&c);
    let lanes_of = |calls: &[SinkCall]| -> Option<usize> {
        calls.iter().rev().find_map(|call| match call {
            SinkCall::Song(input) => Some(
                input
                    .tracks
                    .iter()
                    .filter(|t| t.automation.iter().any(|l| l.param == db))
                    .count(),
            ),
            _ => None,
        })
    };
    let calls = c.sink_mut().take();
    assert_eq!(lanes_of(&calls), Some(2), "one engine lane per member");
    // A member leaves the group: the song is built again with one lane.
    c.apply_command(Command::Console(ConsoleCommand::AssignToVca {
        vca: None,
        strips: vec![channels[1].clone()],
    }))
    .unwrap();
    let calls = c.sink_mut().take();
    assert_eq!(lanes_of(&calls), Some(1));
    // A plain level drag does not rebuild the song.
    level(&mut c, -2.0, Some(1));
    assert_eq!(lanes_of(&c.sink_mut().take()), None);
}

#[test]
fn muting_the_lane_holds_the_group_level_it_gave() {
    let (mut c, master, _) = core();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: master.clone(),
        lane: 0,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: 15360,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Automation(vec![atomik_model::AutomationPoint {
                tick: 0,
                value: -8.0,
                curve: atomik_model::Curve::Linear,
            }]),
        },
    }))
    .unwrap();
    assert_eq!(c.doc().console.vca(1).unwrap().level_db, 0.0);
    c.toggle_automation_lane_mute(&master, &lane_key());
    assert!(c.doc().song.track(&master).unwrap().lanes[0].muted);
    assert_eq!(c.doc().console.vca(1).unwrap().level_db, -8.0);
    // One undo step puts both back.
    assert!(c.undo());
    assert!(!c.doc().song.track(&master).unwrap().lanes[0].muted);
    assert_eq!(c.doc().console.vca(1).unwrap().level_db, 0.0);
}

#[test]
fn the_level_lane_can_be_shown_removed_and_given_a_static_value() {
    let (mut c, master, _) = core();
    assert!(c.has_lane_for(&master, &lane_key()));
    assert!(c.has_lane_to_clear(&master, &lane_key()));
    level(&mut c, -4.5, None);
    assert!(c.set_static_from_control(&master, &lane_key()));
    assert_eq!(
        c.doc().song.track(&master).unwrap().lanes[0].static_value,
        Some(-4.5)
    );
    assert!(c.clear_automation_of(&master, &lane_key()));
    assert!(c.doc().song.track(&master).is_none());
}
