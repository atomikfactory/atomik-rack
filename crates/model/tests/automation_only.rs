//! Automation-only tracks (project format 34): a device without a Notes input (a console
//! channel, an effect) gets a track that holds automation lanes alone.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Curve, Document, LaneKind, PROJECT_VERSION, SongCommand, SyncOp,
    load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

const FADER: &str = "fader";
const MIX: &str = "mix";

fn lane(device: &str, param: &str) -> Command {
    sc(SongCommand::AddAutomationLane {
        device: key(device),
        param: param.into(),
    })
}

/// The chain (synth, delay, mixer) with a console channel `channel` on the delay.
fn start(reg: &Registry) -> Document {
    let mut d = Document::new(chain(&registry()).doc().clone());
    d.apply(
        Command::Console(ConsoleCommand::AddChannel {
            source: key("delay"),
        }),
        reg,
    )
    .unwrap();
    d
}

fn kinds(d: &Document, device: &str) -> Vec<LaneKind> {
    d.doc()
        .song
        .track(&key(device))
        .unwrap()
        .lanes
        .iter()
        .map(|l| l.kind.clone())
        .collect()
}

fn flags(device: &str, muted: bool, solo: bool, armed: bool) -> Command {
    sc(SongCommand::SetTrackFlags {
        device: key(device),
        muted,
        solo,
        armed,
    })
}

#[test]
fn a_lane_on_a_channel_or_an_effect_makes_a_track_with_automation_lanes_only() {
    let reg = console_registry();
    let mut d = start(&reg);
    let before = d.doc().clone();
    let ops = d.apply(lane("channel", FADER), &reg).unwrap();
    assert_eq!(ops, [SyncOp::SongChanged]);
    d.apply(lane("delay", MIX), &reg).unwrap();
    assert_eq!(kinds(&d, "channel"), [LaneKind::Automation(FADER.into())]);
    assert_eq!(kinds(&d, "delay"), [LaneKind::Automation(MIX.into())]);
    let t = d.doc().song.track(&key("channel")).unwrap();
    assert!(t.is_automation_only());
    assert!(!t.is_audio());
    assert_eq!(t.note_lane_count(), 0);
    assert!(!t.armed && !t.solo && !t.muted);
    // A second param joins the track; the same param twice is refused.
    d.apply(lane("channel", "pan"), &reg).unwrap();
    assert_eq!(d.doc().song.track(&key("channel")).unwrap().lanes.len(), 2);
    assert!(d.apply(lane("channel", "pan"), &reg).is_err());
    // One undo per lane; everything undone is the document from before.
    for _ in 0..3 {
        assert!(d.undo(&reg).is_some());
    }
    assert_eq!(d.doc(), &before);
    assert!(d.redo(&reg).is_some());
    assert!(d.doc().song.track(&key("channel")).is_some());
}

#[test]
fn an_instrument_lane_still_makes_a_note_track() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("synth", "filter.cutoff"), &reg).unwrap();
    let t = d.doc().song.track(&key("synth")).unwrap();
    assert!(!t.is_automation_only());
    assert_eq!(t.note_lane_count(), 1);
}

#[test]
fn internal_and_unknown_params_are_still_refused() {
    let reg = console_registry();
    let mut d = start(&reg);
    assert!(
        d.apply(lane("channel", "internal.solo_silence"), &reg)
            .is_err()
    );
    assert!(d.apply(lane("channel", "nope"), &reg).is_err());
    assert!(d.doc().song.tracks.is_empty());
}

#[test]
fn an_automation_only_track_has_no_note_lane_arm_solo_or_new_track() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    let mut rejects = |c| {
        let before = d.doc().clone();
        assert!(d.apply(c, &reg).is_err());
        assert_eq!(d.doc(), &before);
    };
    rejects(sc(SongCommand::AddNoteLane {
        device: key("channel"),
        name: None,
    }));
    rejects(sc(SongCommand::RemoveNoteLane {
        device: key("channel"),
        lane: 0,
    }));
    rejects(flags("channel", false, false, true));
    rejects(flags("channel", false, true, false));
    rejects(add_track("channel"));
    // Mute is allowed: it stops the track's automation.
    d.apply(flags("channel", true, false, false), &reg).unwrap();
}

#[test]
fn automation_clips_go_on_the_lane_and_note_clips_do_not() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    d.apply(
        add_clip(
            "channel",
            0,
            auto_clip(
                0,
                BAR,
                vec![point(0, 0.2, Curve::Linear), point(PPQ, 0.8, Curve::Linear)],
            ),
        ),
        &reg,
    )
    .unwrap();
    let t = d.doc().song.track(&key("channel")).unwrap();
    let flat = atomik_model::song::flatten_automation(t, 0);
    assert!(flat.iter().any(|f| f.0 == 0 && (f.1 - 0.2).abs() < 1e-6));
    assert!(
        d.apply(
            add_clip(
                "channel",
                0,
                notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])
            ),
            &reg
        )
        .is_err()
    );
}

#[test]
fn removing_the_track_or_the_device_takes_the_lanes_with_it_and_undoes() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    let with = d.doc().clone();
    d.apply(
        sc(SongCommand::RemoveTrack {
            device: key("channel"),
        }),
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.tracks.is_empty());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &with);
    d.apply(
        Command::Console(ConsoleCommand::RemoveChannel {
            channel: key("channel"),
        }),
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.track(&key("channel")).is_none());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &with);
}

#[test]
fn the_track_moves_and_takes_a_colour_like_any_track() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("synth", "filter.cutoff"), &reg).unwrap();
    d.apply(lane("channel", FADER), &reg).unwrap();
    d.apply(
        sc(SongCommand::MoveTrack {
            device: key("channel"),
            down: false,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.tracks[0].device, key("channel"));
    d.apply(
        sc(SongCommand::SetTrackColor {
            device: key("channel"),
            color: Some([255, 0, 0]),
        }),
        &reg,
    )
    .unwrap();
}

// ---- project format 34 -----------------------------------------------------------------

fn saved(d: &Document) -> String {
    save_project(d.doc())
}

fn edited(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v["song"]["tracks"][0]);
    serde_json::to_string_pretty(&v).unwrap()
}

#[test]
fn the_project_version_is_thirty_four_and_an_automation_only_track_round_trips() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    d.apply(lane("delay", MIX), &reg).unwrap();
    d.apply(
        add_clip(
            "delay",
            0,
            auto_clip(0, BAR, vec![point(0, 0.1, Curve::Step)]),
        ),
        &reg,
    )
    .unwrap();
    let text = saved(&d);
    assert_eq!(PROJECT_VERSION, 38);
    assert!(text.contains("\"version\": 38"));
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_version_33_file_loads_unchanged_and_resaves_as_34() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("synth", "filter.cutoff"), &reg).unwrap();
    let text = saved(&d);
    let v33 = text.replacen("\"version\": 38", "\"version\": 33", 1);
    let loaded = load_project(&v33, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn before_version_34_a_track_on_a_device_without_notes_is_refused() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    let v33 = saved(&d).replacen("\"version\": 38", "\"version\": 33", 1);
    let e = load_project(&v33, &reg).unwrap_err().to_string();
    assert!(e.contains("no Notes input"), "{e}");
}

#[test]
fn an_automation_only_track_on_an_instrument_or_a_note_lane_on_an_effect_is_refused() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    let text = saved(&d);
    // On a device with a Notes input the first lane must be a notes lane.
    let on_synth = edited(&text, |t| t["device"] = Value::String("synth".into()));
    let e = load_project(&on_synth, &reg).unwrap_err().to_string();
    assert!(e.contains("first lane"), "{e}");
    // A notes lane on a device without a Notes input.
    let notes = edited(&text, |t| {
        t["lanes"][0] = serde_json::json!({"kind": "notes", "clips": []});
    });
    let e = load_project(&notes, &reg).unwrap_err().to_string();
    assert!(e.contains("automation"), "{e}");
    // No lane at all.
    let none = edited(&text, |t| t["lanes"] = serde_json::json!([]));
    assert!(load_project(&none, &reg).is_err());
    // Armed and solo have no meaning on it.
    for field in ["armed", "solo"] {
        let bad = edited(&text, |t| t[field] = Value::Bool(true));
        let e = load_project(&bad, &reg).unwrap_err().to_string();
        assert!(e.contains(field), "{e}");
    }
}

// ---- mute, solo and copies ---------------------------------------------------------------

#[test]
fn a_solo_elsewhere_does_not_stop_the_automation_but_a_mute_does() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(lane("channel", FADER), &reg).unwrap();
    let playing = |d: &Document| -> Vec<String> {
        atomik_model::song::effective_tracks(&d.doc().song)
            .iter()
            .map(|t| t.device.0.clone())
            .collect()
    };
    assert_eq!(playing(&d), ["synth", "channel"]);
    d.apply(flags("synth", false, true, false), &reg).unwrap();
    assert_eq!(
        playing(&d),
        ["synth", "channel"],
        "a solo keeps the automation"
    );
    d.apply(flags("channel", true, false, false), &reg).unwrap();
    assert_eq!(playing(&d), ["synth"], "a mute stops it");
}

#[test]
fn duplicating_an_effect_track_copies_its_automation_track() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("delay", MIX), &reg).unwrap();
    d.apply(
        add_clip(
            "delay",
            0,
            auto_clip(0, BAR, vec![point(0, 0.3, Curve::Linear)]),
        ),
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Workflow(atomik_model::WorkflowCommand::DuplicateTrack {
            device: key("delay"),
        }),
        &reg,
    )
    .unwrap();
    let tracks = &d.doc().song.tracks;
    assert_eq!(tracks.len(), 2);
    assert!(tracks.iter().all(|t| t.is_automation_only()));
    assert_ne!(tracks[0].device, tracks[1].device);
    assert_ne!(
        tracks[0].lanes[0].clips[0].id,
        tracks[1].lanes[0].clips[0].id
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks.len(), 1);
}

#[test]
fn duplicating_a_channel_track_is_one_undo_step_or_a_clean_refusal() {
    let reg = console_registry();
    let mut d = start(&reg);
    d.apply(lane("channel", FADER), &reg).unwrap();
    let before = d.doc().clone();
    let r = d.apply(
        Command::Workflow(atomik_model::WorkflowCommand::DuplicateTrack {
            device: key("channel"),
        }),
        &reg,
    );
    match r {
        Ok(_) => {
            assert!(d.doc().song.tracks.iter().all(|t| t.is_automation_only()));
            d.undo(&reg).unwrap();
            assert_eq!(d.doc(), &before);
        }
        Err(_) => assert_eq!(d.doc(), &before, "a refusal changes nothing"),
    }
}
