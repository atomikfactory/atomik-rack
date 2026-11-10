//! The clipboard texts: what Copy puts on the system clipboard for clips, notes, automation points
//! and devices reads back to the same snapshot, and hostile text is refused with a path.

mod common;

use atomik_model::{
    AudioClipContent, ClipContent, ClipSnapshot, Curve, DeviceSnapshot, ModelError, NoteSnapshot,
    PointSnapshot, SongCommand, clips_from_text, clips_to_text, devices_from_text, devices_to_text,
    notes_from_text, notes_to_text, points_from_text, points_to_text,
};
use common::audio::*;
use common::plugin::*;
use common::song::*;
use common::*;

const FILTER: &str = "filter.cutoff";

fn load_error(r: Result<impl std::fmt::Debug, ModelError>) -> String {
    match r.expect_err("the text must be refused") {
        ModelError::Load { path, message } => format!("{path}: {message}"),
        other => panic!("expected a load error, got {other:?}"),
    }
}

#[test]
fn notes_round_trip_with_their_span() {
    let snapshot = NoteSnapshot {
        notes: vec![
            note(0, 3_840, 60, 0.8),
            note(3_840, 1_920, 64, 0.5),
            note(7_680, 3_840, 67, 1.0),
        ],
        span: 11_520,
    };
    let text = notes_to_text(&snapshot).unwrap();
    assert_eq!(notes_from_text(&text).unwrap(), snapshot);
}

#[test]
fn points_round_trip_with_every_curve() {
    let snapshot = PointSnapshot {
        points: vec![
            point(0, 0.25, Curve::Linear),
            point(960, 0.5, Curve::Step),
            point(1_920, 0.75, Curve::Bend(-30)),
        ],
        span: 1_920,
    };
    let text = points_to_text(&snapshot);
    assert_eq!(points_from_text(&text).unwrap(), snapshot);
}

#[test]
fn points_must_be_sorted_and_finite() {
    let unsorted = r#"{"span": 10, "points": [[5, 0.5, "linear"], [5, 0.6, "linear"]]}"#;
    assert!(load_error(points_from_text(unsorted)).contains("sorted"));
    let junk = r#"{"span": 10, "points": [[5, "x", "linear"]]}"#;
    assert!(load_error(points_from_text(junk)).contains("points[0]"));
}

/// A synth track with a note clip and an automation clip, an audio track with a clip.
fn clip_doc() -> (atomik_model::Document, atomik_device_sdk::Registry) {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(register(1, 96_000), &reg).ok();
    for c in [
        add(9, SYNTH, Some("lead")),
        add_track("lead"),
        add_clip("lead", 0, notes_clip(BAR, BAR, vec![note(0, 960, 60, 0.7)])),
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: FILTER.into(),
        }),
        add_clip(
            "lead",
            1,
            auto_clip(
                BAR,
                BAR,
                vec![point(0, 0.2, Curve::Linear), point(960, 0.9, Curve::Step)],
            ),
        ),
        add_audio_clip_with(
            "audio-track",
            2 * BAR,
            BAR,
            AudioClipContent {
                source_offset: 480,
                gain_db: -2.5,
                fade_in: 240,
                ..audio(1)
            },
        ),
    ] {
        d.apply(c, &reg).unwrap();
    }
    (d, reg)
}

#[test]
fn clips_of_every_kind_round_trip() {
    let (d, _) = clip_doc();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 3);
    let snapshot = ClipSnapshot::capture(d.doc(), &ids).unwrap();
    assert!(
        !snapshot.assets.is_empty(),
        "the audio clip needs its asset"
    );
    let back = clips_from_text(&clips_to_text(&snapshot)).unwrap();
    assert_eq!(back, snapshot);
    assert!(
        back.entries
            .iter()
            .any(|e| matches!(e.clip.content, ClipContent::Audio(_)))
    );
}

#[test]
fn a_clip_text_needs_its_asset_in_the_pool() {
    let (d, _) = clip_doc();
    let ids = clip_ids(d.doc());
    let mut snapshot = ClipSnapshot::capture(d.doc(), &ids).unwrap();
    snapshot.assets.clear();
    let message = load_error(clips_from_text(&clips_to_text(&snapshot)));
    assert!(message.contains("assets pool"), "{message}");
}

#[test]
fn garbage_and_unknown_fields_are_refused() {
    assert!(load_error(clips_from_text("not json")).contains("invalid JSON"));
    assert!(load_error(clips_from_text("{}")).contains("origin"));
    assert!(load_error(notes_from_text("{\"format\": \"other\"}")).contains("atomik-clip"));
    assert!(load_error(points_from_text("[1, 2]")).contains("expected"));
    let (d, _) = clip_doc();
    let text = clips_to_text(&ClipSnapshot::capture(d.doc(), &clip_ids(d.doc())).unwrap());
    let bad_lane = text.replace("\"lane\": \"notes\"", "\"lane\": \"video\"");
    assert!(load_error(clips_from_text(&bad_lane)).contains("lane"));
}

#[test]
fn devices_round_trip_with_cables_tracks_and_assets() {
    let reg = registry();
    let mut d = chain(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, 960, 60, 0.7)])),
        &reg,
    )
    .unwrap();
    let keys = vec![key("synth"), key("delay")];
    let snapshot = DeviceSnapshot::capture(d.doc(), &keys, true).unwrap();
    assert_eq!(snapshot.cables.len(), 2);
    assert_eq!(snapshot.tracks.len(), 1);
    let text = devices_to_text(&snapshot, d.doc().sample_rate, d.doc().tempo_bpm);
    let back = devices_from_text(&text, &reg).unwrap();
    assert_eq!(back.cables, snapshot.cables);
    assert_eq!(back.tracks, snapshot.tracks);
    assert_eq!(
        back.devices
            .iter()
            .map(|d| (&d.key, &d.type_id))
            .collect::<Vec<_>>(),
        snapshot
            .devices
            .iter()
            .map(|d| (&d.key, &d.type_id))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_newer_project_text_is_refused_not_misread() {
    let reg = registry();
    let d = chain(&reg);
    let keys = vec![key("synth")];
    let snapshot = DeviceSnapshot::capture(d.doc(), &keys, false).unwrap();
    let text = devices_to_text(&snapshot, 48_000, 120.0);
    let newer = text
        .replacen("\"version\": 45", "\"version\": 9999", 1)
        .replacen("\"compat\": 44", "\"compat\": 9999", 1);
    let message = load_error(devices_from_text(&newer, &reg));
    assert!(message.contains("version"), "{message}");
}

#[test]
fn a_plugin_device_keeps_its_id_and_loads_as_a_stand_in_where_it_is_missing() {
    let installed = plugin_registry();
    let mut d = atomik_model::Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &installed,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("lead")),
            sync_state("lead", b"chunk-of-state"),
            set_pinned("lead", &[10]),
        ],
    );
    let snapshot = DeviceSnapshot::capture(d.doc(), &[key("lead")], false).unwrap();
    let text = devices_to_text(&snapshot, 48_000, 120.0);
    let back = devices_from_text(&text, &registry()).expect("a missing plugin is a stand-in");
    assert_eq!(
        back.devices[0].plugin.as_ref().unwrap().id,
        snapshot.devices[0].plugin.as_ref().unwrap().id
    );
    let (got, want) = (
        back.devices[0].plugin.as_ref().unwrap(),
        snapshot.devices[0].plugin.as_ref().unwrap(),
    );
    assert!(want.state.is_some(), "the fixture has a state chunk");
    assert_eq!(got.state, want.state, "the state chunk travels");
    assert_eq!(got.pinned, want.pinned);
}
