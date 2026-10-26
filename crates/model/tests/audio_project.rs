//! Project format v3: input channels, the sample pool, audio lanes and audio
//! clips; v1 and v2 migrate; round trips are bit-exact and canonical; errors name their JSON
//! paths; every audio clip's asset must be in the pool.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AudioClipContent, ClipContent, Command, FadeCurve, ModelError, RackDoc, SongCommand, empty_doc,
    load_patch, load_project, save_project,
};
use common::audio::*;
use common::song::*;
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

fn assert_rejects(text: &str, path: &str, needle: &str) {
    let e = load_project(text, &audio_registry()).expect_err("project must be rejected");
    let ModelError::Patch(msg) = &e else {
        panic!("expected a patch error, got {e:?}")
    };
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} (wanted path {path})"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

/// One audio track with three clips of two assets, and a pool entry nobody uses.
fn audio_doc(reg: &Registry) -> RackDoc {
    let mut d = with_audio_track(reg);
    d.apply(register(3, 77), reg).unwrap();
    let a = AudioClipContent {
        source_offset: 1234,
        gain_db: -3.5,
        fade_in: 480,
        fade_out: 4800,
        fade_curve: FadeCurve::EqualPower,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 2 * BAR, a), reg)
        .unwrap();
    d.apply(add_audio_clip("audio-track", 2 * BAR, BAR, 2), reg)
        .unwrap();
    let b = AudioClipContent {
        gain_db: 0.1,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", BAR, BAR, b), reg)
        .unwrap();
    d.doc().clone()
}

#[test]
fn v3_header_pool_lane_and_clip_schema() {
    let reg = audio_registry();
    let doc = audio_doc(&reg);
    let text = save_project(&doc);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 31);
    assert_eq!(v["input_channels"], 2);
    let h1 = hash(1);
    assert_eq!(
        v["assets"][&h1],
        json!({"channels": 2, "sample_rate": 48000, "frames": 480_000, "name": "take-480000.wav"})
    );
    assert_eq!(v["assets"].as_object().unwrap().len(), 3);
    let track = &v["song"]["tracks"][0];
    assert_eq!(track["device"], "audio-track");
    assert_eq!(track["lanes"].as_array().unwrap().len(), 1);
    assert_eq!(track["lanes"][0]["kind"], "audio");
    let clips = track["lanes"][0]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 3);
    // Clips are sorted by (start, id).
    assert_eq!(clips[0]["start"], 0);
    assert_eq!(clips[1]["start"], BAR);
    assert_eq!(clips[0]["offset"], 0);
    assert_eq!(clips[0]["loop_length"], Value::Null);
    assert_eq!(
        clips[0]["audio"],
        json!({"asset": h1, "offset": 1234, "gain_db": -3.5, "fade_in": 480,
               "fade_out": 4800, "fade_curve": "equal_power"})
    );
    assert_eq!(clips[2]["audio"]["fade_curve"], "linear");
    // The audio track's input cables are in the file.
    assert!(v["cables"].to_string().contains("interface:in_1"));
    // The excerpt the docs quote.
    assert!(text.contains("\"audio\": {\"asset\": \""));
}

#[test]
fn audio_documents_round_trip_bit_exactly_and_canonically() {
    let reg = audio_registry();
    let doc = audio_doc(&reg);
    let text = save_project(&doc);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded, doc);
    assert_eq!(save_project(&loaded), text, "canonical");
    assert_eq!(
        loaded.assets.len(),
        3,
        "unreferenced assets are kept as they are"
    );
    assert_eq!(loaded.input_channels, 2);
}

#[test]
fn negative_zero_gain_and_huge_frames_survive() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(
        Command::RegisterAsset {
            hash: hash(5),
            meta: atomik_model::AssetMeta {
                channels: 6,
                sample_rate: 96_000,
                frames: 1 << 40,
                original_name: "q\"uo\\te é 🎸.wav".into(),
            },
        },
        &reg,
    )
    .unwrap();
    let a = AudioClipContent {
        gain_db: -0.0,
        source_offset: 1 << 40,
        fade_in: 1 << 40,
        ..audio(5)
    };
    d.apply(add_audio_clip_with("audio-track", 0, BAR, a), &reg)
        .unwrap();
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    let ClipContent::Audio(a) = &loaded.song.tracks[0].lanes[0].clips[0].content else {
        panic!()
    };
    assert_eq!(a.gain_db.to_bits(), (-0.0f32).to_bits());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn v2_projects_load_with_no_inputs_and_an_empty_pool() {
    let reg = audio_registry();
    let mut doc = empty_doc();
    doc.input_channels = 0;
    let v3 = save_project(&doc);
    let v2 = edited(&v3, |v| {
        v["version"] = json!(2);
        let o = v.as_object_mut().unwrap();
        o.remove("assets");
        o.remove("input_channels");
    });
    let loaded = load_project(&v2, &reg).unwrap();
    assert_eq!(loaded, doc);
    assert_eq!(loaded.input_channels, 0);
    assert!(loaded.assets.is_empty());
    // And saves as v3.
    assert!(save_project(&loaded).contains("\"version\": 31,"));
    // A v2 file that already says "input_channels" or "assets" is malformed.
    assert_rejects(
        &edited(&v3, |v| v["version"] = json!(2)),
        "assets",
        "unknown field",
    );
}

#[test]
fn v1_projects_migrate_to_v3_with_no_inputs_and_an_empty_pool() {
    let reg = console_registry();
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-v1.atomik"),
    )
    .unwrap();
    let doc = load_project(&text, &reg).unwrap();
    assert_eq!(doc.input_channels, 0);
    assert!(doc.assets.is_empty());
    let again = save_project(&doc);
    assert!(again.contains("\"version\": 31,"));
    assert_eq!(load_project(&again, &reg).unwrap(), doc);
}

#[test]
fn patches_load_with_no_inputs_and_may_not_declare_them() {
    let reg = registry();
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/patches/slice-demo.v2.json"),
    )
    .unwrap();
    let doc = load_patch(&text, &reg).unwrap();
    assert_eq!(doc.input_channels, 0);
    assert!(doc.assets.is_empty());
    let with_inputs = edited(&text, |v| v["input_channels"] = json!(2));
    let e = load_patch(&with_inputs, &reg).unwrap_err();
    assert!(
        matches!(&e, ModelError::Patch(m) if m.starts_with("input_channels: ")),
        "{e:?}"
    );
}

#[test]
fn optional_fields_have_defaults() {
    let reg = audio_registry();
    let doc = audio_doc(&reg);
    let text = save_project(&doc);
    // No input_channels, no assets in a v3 file: 0 and empty (then the clips cannot load).
    let bare = edited(&save_project(&empty_doc()), |v| {
        let o = v.as_object_mut().unwrap();
        o.remove("assets");
        o.remove("input_channels");
    });
    let loaded = load_project(&bare, &reg).unwrap();
    assert_eq!((loaded.input_channels, loaded.assets.len()), (0, 0));
    // Only asset and offset are required in "audio".
    let minimal = edited(&text, |v| {
        let a = &mut v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"];
        *a = json!({"asset": hash(1), "offset": 0});
    });
    let loaded = load_project(&minimal, &reg).unwrap();
    let ClipContent::Audio(a) = &loaded.song.tracks[0].lanes[0].clips[0].content else {
        panic!()
    };
    assert_eq!(*a, audio(1));
}

#[test]
fn cables_from_interface_inputs_load_and_follow_input_channels() {
    let reg = audio_registry();
    let text = save_project(&audio_doc(&reg));
    let doc = load_project(&text, &reg).unwrap();
    assert!(
        doc.cables
            .contains(&cable("interface:in_2", "audio-track:in_r"))
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    let i = v["cables"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["from"] == "interface:in_2")
        .unwrap();
    assert_rejects(
        &edited(&text, |v| v["input_channels"] = json!(1)),
        &format!("cables[{i}].from"),
        "in_2",
    );
    assert_rejects(
        &edited(&text, |v| v["input_channels"] = json!(33)),
        "input_channels",
        "0..=32",
    );
    assert_rejects(
        &edited(&text, |v| v["input_channels"] = json!(-1)),
        "input_channels",
        "unsigned",
    );
}

#[test]
fn load_rejects_bad_assets_and_audio_clips_with_paths() {
    let reg = audio_registry();
    let good = save_project(&audio_doc(&reg));
    assert!(load_project(&good, &reg).is_ok());
    let h1 = hash(1);
    // Paths quote keys that are not identifiers (a hash may start with a digit).
    let asset = format!("assets.\"{h1}\"");
    let clip0 = "song.tracks[0].lanes[0].clips[0]";
    type Edit = Box<dyn Fn(&mut Value)>;
    let h1c = h1.clone();
    let cases: Vec<(Edit, String, &str)> = vec![
        // The pool.
        (
            Box::new(|v| v["assets"] = json!([])),
            "assets".into(),
            "expected an object",
        ),
        (
            Box::new(|v| {
                v["assets"]["ABC"] = json!({"channels": 1, "sample_rate": 1, "frames": 1});
            }),
            "assets.ABC".into(),
            "64 lowercase hex",
        ),
        (
            Box::new(move |v| v["assets"][&h1c]["channels"] = json!(0)),
            format!("{asset}.channels"),
            "at least 1",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h]["sample_rate"] = json!(0);
            }),
            format!("{asset}.sample_rate"),
            "above 0",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h]["frames"] = json!(0);
            }),
            format!("{asset}.frames"),
            "at least 1",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h]["frames"] = json!(1u64 << 41);
            }),
            format!("{asset}.frames"),
            "outside",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h]["extra"] = json!(1);
            }),
            format!("{asset}.extra"),
            "unknown field",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h]["name"] = json!(5);
            }),
            format!("{asset}.name"),
            "expected a string",
        ),
        (
            Box::new(|v| {
                let h = hash(1);
                v["assets"][&h].as_object_mut().unwrap().remove("frames");
            }),
            format!("{asset}.frames"),
            "missing field",
        ),
        // Clips.
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["asset"] = json!(hash(9));
            }),
            format!("{clip0}.audio.asset"),
            "not in the assets pool",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["offset"] = json!(480_001);
            }),
            format!("{clip0}.audio.offset"),
            "beyond the asset",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["gain_db"] = json!(30.0);
            }),
            format!("{clip0}.audio.gain_db"),
            "outside",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["fade_in"] = json!(-1);
            }),
            format!("{clip0}.audio.fade_in"),
            "unsigned",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["fade_curve"] =
                    json!("cubic");
            }),
            format!("{clip0}.audio.fade_curve"),
            "equal_power",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["bogus"] = json!(1);
            }),
            format!("{clip0}.audio.bogus"),
            "unknown field",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("audio");
            }),
            format!("{clip0}.audio"),
            "missing field",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"] = json!([]);
            }),
            format!("{clip0}.notes"),
            "unknown field",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["offset"] = json!(5)),
            format!("{clip0}.offset"),
            "no content offset",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["loop_length"] = json!(100);
            }),
            format!("{clip0}.loop_length"),
            "cannot loop",
        ),
        // Lanes.
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["kind"] = json!("notes")),
            "song.tracks[0].lanes[0].kind".into(),
            "must be \"audio\"",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"kind": "audio", "clips": []}));
            }),
            "song.tracks[0].lanes[1]".into(),
            "single lane",
        ),
    ];
    for (edit, path, needle) in cases {
        assert_rejects(&edited(&good, edit), &path, needle);
    }
}

#[test]
fn a_notes_track_cannot_have_an_audio_lane() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    let text = save_project(d.doc());
    let i = d
        .doc()
        .song
        .tracks
        .iter()
        .position(|t| t.device.as_str() == "synth")
        .unwrap();
    assert_rejects(
        &edited(&text, |v| {
            v["song"]["tracks"][i]["lanes"][0]["kind"] = json!("audio");
        }),
        &format!("song.tracks[{i}].lanes[0].kind"),
        "first lane must be \"notes\"",
    );
}

#[test]
fn an_audio_track_device_without_the_real_type_cannot_have_audio_lanes() {
    // With a registry that lacks the audio track type, the device itself is unknown.
    let reg = audio_registry();
    let text = save_project(&audio_doc(&reg));
    let e = load_project(&text, &console_registry()).unwrap_err();
    assert!(
        matches!(&e, ModelError::Patch(m) if m.contains("unknown device type")),
        "{e:?}"
    );
}

#[test]
fn load_does_not_trust_clip_ids_or_order_in_audio_lanes() {
    let reg = audio_registry();
    let text = save_project(&audio_doc(&reg));
    let clip0 = "song.tracks[0].lanes[0].clips[0]";
    assert_rejects(
        &edited(&text, |v| {
            let c = v["song"]["tracks"][0]["lanes"][0]["clips"][1]["id"].clone();
            v["song"]["tracks"][0]["lanes"][0]["clips"][0]["id"] = c;
        }),
        "song.tracks[0].lanes[0].clips[1].id",
        "duplicate clip id",
    );
    assert_rejects(
        &edited(&text, |v| {
            v["song"]["tracks"][0]["lanes"][0]["clips"][0]["length"] = json!(0);
        }),
        &format!("{clip0}.length"),
        "greater than 0",
    );
    // Unsorted clips load sorted.
    let shuffled = edited(&text, |v| {
        v["song"]["tracks"][0]["lanes"][0]["clips"]
            .as_array_mut()
            .unwrap()
            .reverse();
    });
    assert_eq!(
        load_project(&shuffled, &reg).unwrap(),
        load_project(&text, &reg).unwrap()
    );
}

// ---- proptest -------------------------------------------------------------------------------

/// Random audio documents: clips of the two registered assets with random geometry and content,
/// then random undo/redo noise does not matter: save → load → save is the identity.
fn audio_clip_spec() -> impl Strategy<Value = (i64, i64, u8, u64, f32, u64, u64, bool)> {
    (
        0i64..16 * BAR,
        1i64..4 * BAR,
        1u8..=2,
        0u64..96_000,
        -60f32..24f32,
        0u64..100_000,
        0u64..100_000,
        any::<bool>(),
    )
}

proptest! {
    #[test]
    fn audio_projects_round_trip_bit_exactly(
        clips in prop::collection::vec(audio_clip_spec(), 0..8),
        extra_tracks in 0usize..3,
        inputs in 0u16..=4,
        tempo in 20f64..=999f64,
    ) {
        let reg = audio_registry();
        let mut d = with_assets(&reg);
        d.apply(Command::SetInputChannels(inputs), &reg).unwrap();
        d.apply(Command::SetTempo(tempo), &reg).unwrap();
        for _ in 0..=extra_tracks {
            d.apply(add_audio_track(), &reg).unwrap();
        }
        for (i, (start, len, n, so, gain, fi, fo, eq)) in clips.into_iter().enumerate() {
            let a = AudioClipContent {
                asset: hash(n),
                source_offset: so,
                gain_db: gain,
                fade_in: fi,
                fade_out: fo,
                fade_curve: if eq { FadeCurve::EqualPower } else { FadeCurve::Linear },
                reverse: false,
                envelope: Vec::new(),
                xfade: None,
                warp: Default::default(),
            };
            let device = if i % 2 == 0 { "audio-track" } else { "audio-track-2" };
            if d.doc().song.track(&key(device)).is_some() {
                d.apply(add_audio_clip_with(device, start, len, a), &reg).unwrap();
            }
        }
        // Mix in a split and a duplicate.
        if let Some(id) = d.doc().song.tracks[0].lanes[0].clips.first().map(|c| c.id) {
            let c = d.doc().song.clip(id).unwrap().2.clone();
            if c.length > 1 {
                d.apply(sc(SongCommand::SplitClip { id, at: c.start + c.length / 2 }), &reg).ok();
            }
            d.apply(sc(SongCommand::DuplicateClip { id, start: 20 * BAR }), &reg).ok();
        }
        let doc = d.doc().clone();
        let text = save_project(&doc);
        let loaded = load_project(&text, &reg).unwrap();
        prop_assert_eq!(&loaded, &doc);
        prop_assert_eq!(save_project(&loaded), text);
    }
}
