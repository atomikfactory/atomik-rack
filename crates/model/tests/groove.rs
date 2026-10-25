//! Quantize and groove: `ApplyQuantize` and `QuantizeNotes` (one undo step over every clip), the
//! groove library (`SetGroove`, `DeleteGroove`, `ExtractGroove`) and project format 21.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::groove::{GrooveSlot, builtin_groove};
use atomik_model::song::flatten_notes;
use atomik_model::{
    ClipContent, ClipId, Command, Document, Groove, ModelError, Note, PROJECT_VERSION,
    QuantizeParams, SongCommand, StepResolution, SyncOp, load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

const S: i64 = PPQ / 4;

/// Two tracks, `lead` (clip 1 at bar 1, loose notes) and `synth` (clip 2 at bar 2).
fn two_clips(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(
                0,
                BAR,
                vec![note(40, S, 60, 0.8), note(S + 300, S, 62, 0.8)],
            ),
        ),
        reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(BAR, BAR, vec![note(2 * S - 200, S, 64, 0.5)]),
        ),
        reg,
    )
    .unwrap();
    d
}

fn notes_of(d: &Document, id: u64) -> Vec<Note> {
    let (_, _, c) = d.doc().song.clip(ClipId(id)).unwrap();
    match &c.content {
        ClipContent::Notes(n) => n.clone(),
        _ => panic!("not notes"),
    }
}

fn quantize(d: &mut Document, reg: &Registry, clips: &[u64], q: QuantizeParams) -> Vec<SyncOp> {
    d.apply(
        sc(SongCommand::ApplyQuantize {
            clips: clips.iter().map(|&i| ClipId(i)).collect(),
            quantize: q,
        }),
        reg,
    )
    .unwrap()
}

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = save_project(d.doc());
    let undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(
        save_project(d.doc()),
        before,
        "a rejected command changes nothing"
    );
    assert_eq!(d.can_undo(), undo);
    e
}

fn apply_q(q: QuantizeParams, clips: Vec<u64>) -> Command {
    sc(SongCommand::ApplyQuantize {
        clips: clips.into_iter().map(ClipId).collect(),
        quantize: q,
    })
}

#[test]
fn quantize_moves_every_clip_in_one_undo_step() {
    let reg = registry();
    let mut d = two_clips(&reg);
    let before = save_project(d.doc());
    let ops = quantize(&mut d, &reg, &[1, 2], QuantizeParams::default());
    assert_eq!(ops, [SyncOp::SongChanged]);
    let lead: Vec<i64> = notes_of(&d, 1).iter().map(|n| n.tick).collect();
    assert_eq!(lead, [0, S]);
    // The second clip starts on a bar, so its grid is the song's: 2S - 200 goes to 2S.
    assert_eq!(notes_of(&d, 2)[0].tick, 2 * S);
    assert!(d.undo(&reg).is_some());
    assert_eq!(
        save_project(d.doc()),
        before,
        "one undo restores both clips"
    );
    d.redo(&reg).unwrap();
    assert_eq!(notes_of(&d, 1)[0].tick, 0);
    assert_eq!(notes_of(&d, 2)[0].tick, 2 * S);
}

#[test]
fn quantize_uses_the_song_grid_when_a_clip_starts_off_the_grid() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    // The clip starts 1000 ticks into the song and its note 50 ticks after the line one step
    // in (song tick S + 50): the line is clip tick S - 1000, not a multiple of the grid.
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(1000, BAR, vec![note(S - 1000 + 50, S, 60, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    quantize(&mut d, &reg, &[1], QuantizeParams::default());
    assert_eq!(notes_of(&d, 1)[0].tick, S - 1000);
}

#[test]
fn swing_and_a_groove_by_name_change_what_the_track_plays() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let notes = (0..8).map(|i| note(i * S, S, 60, 0.8)).collect();
    d.apply(add_clip("lead", 0, notes_clip(0, 2 * PPQ, notes)), &reg)
        .unwrap();
    quantize(
        &mut d,
        &reg,
        &[1],
        QuantizeParams {
            groove: Some("Swing 66%".into()),
            ..QuantizeParams::default()
        },
    );
    let played: Vec<i64> = flatten_notes(&d.doc().song.tracks[0])
        .into_iter()
        .map(|(t, ..)| t)
        .collect();
    let late = (S as f64 * 0.32).round() as i64;
    let want: Vec<i64> = (0..8)
        .map(|i| i * S + if i % 2 == 1 { late } else { 0 })
        .collect();
    assert_eq!(played, want, "the engine's flatten sees the baked times");
}

#[test]
fn quantize_notes_changes_only_the_listed_notes() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(
        sc(SongCommand::QuantizeNotes {
            clip: ClipId(1),
            notes: vec![1],
            quantize: QuantizeParams::default(),
        }),
        &reg,
    )
    .unwrap();
    let n = notes_of(&d, 1);
    assert_eq!((n[0].tick, n[1].tick), (40, S));
    rejects(
        &mut d,
        sc(SongCommand::QuantizeNotes {
            clip: ClipId(1),
            notes: vec![9],
            quantize: QuantizeParams::default(),
        }),
        &reg,
    );
}

#[test]
fn quantize_refuses_what_it_cannot_do() {
    let reg = registry();
    let mut d = two_clips(&reg);
    let swing = QuantizeParams {
        swing: 90,
        ..QuantizeParams::default()
    };
    let e = rejects(&mut d, apply_q(swing, vec![1]), &reg);
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e}");
    let e = rejects(&mut d, apply_q(QuantizeParams::default(), vec![99]), &reg);
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
    let nope = QuantizeParams {
        groove: Some("nope".into()),
        ..QuantizeParams::default()
    };
    let e = rejects(&mut d, apply_q(nope, vec![1]), &reg);
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
    // Nothing to change: a strength of zero.
    let zero = QuantizeParams {
        strength: 0.0,
        ..QuantizeParams::default()
    };
    rejects(&mut d, apply_q(zero, vec![1]), &reg);
    // Already on the grid.
    quantize(&mut d, &reg, &[1, 2], QuantizeParams::default());
    rejects(&mut d, apply_q(QuantizeParams::default(), vec![1, 2]), &reg);
}

#[test]
fn humanize_is_deterministic_across_a_save_and_load() {
    let reg = registry();
    let q = QuantizeParams {
        strength: 0.0,
        humanize_time: 0.4,
        humanize_velocity: 0.3,
        seed: 11,
        ..QuantizeParams::default()
    };
    let mut a = two_clips(&reg);
    quantize(&mut a, &reg, &[1, 2], q.clone());
    // The same song, loaded from its text, gives the same notes: nothing depends on the session.
    let text = save_project(two_clips(&reg).doc());
    let mut b = Document::new(load_project(&text, &reg).unwrap());
    quantize(&mut b, &reg, &[1, 2], q.clone());
    assert_eq!(save_project(a.doc()), save_project(b.doc()));
    assert_ne!(notes_of(&a, 1)[0].tick, 40, "the note moved");
    // Another seed is another take.
    let mut c = two_clips(&reg);
    quantize(&mut c, &reg, &[1, 2], QuantizeParams { seed: 12, ..q });
    assert_ne!(save_project(a.doc()), save_project(c.doc()));
}

fn custom() -> Groove {
    Groove {
        name: "Late 2nd".into(),
        step: StepResolution::Sixteenth,
        slots: vec![
            GrooveSlot::STRAIGHT,
            GrooveSlot {
                offset: 0.125,
                velocity: 0.5,
            },
        ],
    }
}

#[test]
fn the_groove_library_is_undoable_and_shared_by_name() {
    let reg = registry();
    let mut d = two_clips(&reg);
    let ops = d.apply(sc(SongCommand::SetGroove(custom())), &reg).unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert_eq!(d.doc().song.grooves, [custom()]);
    // Two tracks use the same template by name.
    let q = QuantizeParams {
        groove: Some("Late 2nd".into()),
        ..QuantizeParams::default()
    };
    quantize(&mut d, &reg, &[1, 2], q.clone());
    assert_eq!(notes_of(&d, 1)[1].tick, S + S / 8);
    assert!((notes_of(&d, 1)[1].velocity - 0.4).abs() < 1e-6);
    // Changing the template and applying again moves the notes to the new offset.
    let mut g = custom();
    g.slots[1].offset = 0.25;
    d.apply(sc(SongCommand::SetGroove(g)), &reg).unwrap();
    assert_eq!(d.doc().song.grooves.len(), 1, "same name replaces");
    quantize(&mut d, &reg, &[1], q);
    assert_eq!(notes_of(&d, 1)[1].tick, S + S / 4);
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.grooves, [custom()]);
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert!(d.doc().song.grooves.is_empty());
}

#[test]
fn groove_names_are_checked() {
    let reg = registry();
    let mut d = two_clips(&reg);
    let named = |name: &str| Groove {
        name: name.into(),
        ..custom()
    };
    for bad in ["", " x", "Straight", "Swing 66%", &"x".repeat(33)] {
        rejects(&mut d, sc(SongCommand::SetGroove(named(bad))), &reg);
    }
    let mut g = custom();
    g.slots.clear();
    rejects(&mut d, sc(SongCommand::SetGroove(g)), &reg);
    let missing = SongCommand::DeleteGroove {
        name: "nope".into(),
    };
    rejects(&mut d, sc(missing), &reg);
    d.apply(sc(SongCommand::SetGroove(custom())), &reg).unwrap();
    let delete = SongCommand::DeleteGroove {
        name: "Late 2nd".into(),
    };
    d.apply(sc(delete), &reg).unwrap();
    assert!(d.doc().song.grooves.is_empty());
}

#[test]
fn a_song_holds_at_most_sixty_four_grooves() {
    let reg = registry();
    let mut d = song_start(&reg);
    for i in 0..64 {
        let g = Groove {
            name: format!("g{i}"),
            ..custom()
        };
        d.apply(sc(SongCommand::SetGroove(g)), &reg).unwrap();
    }
    let g = Groove {
        name: "one more".into(),
        ..custom()
    };
    rejects(&mut d, sc(SongCommand::SetGroove(g)), &reg);
}

#[test]
fn extract_groove_stores_the_clips_feel_and_it_can_be_applied_elsewhere() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    let swung = (0..8)
        .map(|i| {
            let (late, v) = if i % 2 == 1 { (800, 0.4) } else { (0, 0.8) };
            note(i * S + late, S, 60, v)
        })
        .collect();
    d.apply(add_clip("lead", 0, notes_clip(0, 2 * PPQ, swung)), &reg)
        .unwrap();
    let straight = (0..8).map(|i| note(i * S, S, 64, 0.8)).collect();
    d.apply(add_clip("synth", 0, notes_clip(0, 2 * PPQ, straight)), &reg)
        .unwrap();
    d.apply(
        sc(SongCommand::ExtractGroove {
            clip: ClipId(1),
            name: "Mine".into(),
            resolution: StepResolution::Sixteenth,
            slots: 2,
        }),
        &reg,
    )
    .unwrap();
    let g = d.doc().song.groove("Mine").unwrap();
    assert_eq!(g.slots[0].offset, 0.0);
    assert!((g.slots[1].offset - 800.0 / S as f32).abs() < 1e-3);
    // Applied to the other track, it gets the same feel.
    quantize(
        &mut d,
        &reg,
        &[2],
        QuantizeParams {
            groove: Some("Mine".into()),
            ..QuantizeParams::default()
        },
    );
    let a: Vec<i64> = notes_of(&d, 1).iter().map(|n| n.tick).collect();
    let b: Vec<i64> = notes_of(&d, 2).iter().map(|n| n.tick).collect();
    for (x, y) in a.iter().zip(&b) {
        assert!((x - y).abs() <= 4, "{a:?} vs {b:?}");
    }
    // Refusals: a built-in name, and a clip that does not exist.
    for (name, clip) in [("Straight", 1), ("x", 77)] {
        rejects(
            &mut d,
            sc(SongCommand::ExtractGroove {
                clip: ClipId(clip),
                name: name.into(),
                resolution: StepResolution::Sixteenth,
                slots: 2,
            }),
            &reg,
        );
    }
}

#[test]
fn the_groove_library_round_trips_in_format_21() {
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(sc(SongCommand::SetGroove(custom())), &reg).unwrap();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 30);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 30);
    assert_eq!(v["song"]["grooves"][0]["name"], "Late 2nd");
    assert_eq!(v["song"]["grooves"][0]["step"], "sixteenth");
    assert_eq!(
        v["song"]["grooves"][0]["slots"][1],
        serde_json::json!([0.125, 0.5])
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.grooves, [custom()]);
    assert_eq!(save_project(&loaded), text, "canonical");
    // No library, no field.
    assert!(!save_project(two_clips(&reg).doc()).contains("grooves"));
}

#[test]
fn a_version_20_file_loads_without_grooves_and_refuses_the_field() {
    let reg = registry();
    let text = save_project(two_clips(&reg).doc());
    let v20 = text.replace("\"version\": 30", "\"version\": 20");
    let loaded = load_project(&v20, &reg).unwrap();
    assert!(loaded.song.grooves.is_empty());
    assert!(save_project(&loaded).contains("\"version\": 30"));
    let mut d = two_clips(&reg);
    d.apply(sc(SongCommand::SetGroove(custom())), &reg).unwrap();
    let with = save_project(d.doc()).replace("\"version\": 30", "\"version\": 20");
    let e = load_project(&with, &reg).unwrap_err().to_string();
    assert!(e.contains("grooves"), "{e}");
}

fn edited(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    v.to_string()
}

#[test]
fn bad_grooves_in_a_file_are_refused_with_their_path() {
    use serde_json::json;
    let reg = registry();
    let mut d = two_clips(&reg);
    d.apply(sc(SongCommand::SetGroove(custom())), &reg).unwrap();
    let text = save_project(d.doc());
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "offset",
            Box::new(|v| v["song"]["grooves"][0]["slots"][0][0] = json!(0.9)),
        ),
        (
            "velocity",
            Box::new(|v| v["song"]["grooves"][0]["slots"][0][1] = json!(3.0)),
        ),
        (
            "built-in",
            Box::new(|v| v["song"]["grooves"][0]["name"] = json!("Straight")),
        ),
        (
            "step",
            Box::new(|v| v["song"]["grooves"][0]["step"] = json!("quarter")),
        ),
        (
            "slot",
            Box::new(|v| v["song"]["grooves"][0]["slots"][0] = json!([0.0])),
        ),
        (
            "slots",
            Box::new(|v| v["song"]["grooves"][0]["slots"] = json!([])),
        ),
        (
            "second groove",
            Box::new(|v| {
                let g = v["song"]["grooves"][0].clone();
                v["song"]["grooves"].as_array_mut().unwrap().push(g);
            }),
        ),
        (
            "extra",
            Box::new(|v| v["song"]["grooves"][0]["extra"] = json!(1)),
        ),
    ];
    for (word, f) in cases {
        let e = load_project(&edited(&text, f), &reg)
            .unwrap_err()
            .to_string();
        assert!(e.contains("grooves"), "{word}: {e}");
        assert!(e.contains(word) || word == "extra", "{word}: {e}");
    }
}

#[test]
fn the_builtin_swing_matches_a_quantize_with_that_swing() {
    let reg = registry();
    let run = |q: QuantizeParams| {
        let mut d = song_start(&reg);
        d.apply(add_track("lead"), &reg).unwrap();
        let notes = (0..8).map(|i| note(i * S + 5, S, 60, 0.8)).collect();
        d.apply(add_clip("lead", 0, notes_clip(0, 2 * PPQ, notes)), &reg)
            .unwrap();
        quantize(&mut d, &reg, &[1], q);
        notes_of(&d, 1)
    };
    let by_swing = run(QuantizeParams {
        swing: 60,
        ..QuantizeParams::default()
    });
    let by_groove = run(QuantizeParams {
        groove: Some("Swing 60%".into()),
        ..QuantizeParams::default()
    });
    assert_eq!(by_swing, by_groove);
    assert!(builtin_groove("Swing 60%").is_some());
}

#[test]
fn the_clips_a_take_group_compiled_are_left_alone() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: vec![notes_clip(0, BAR, vec![note(40, S, 60, 0.8)])],
        }),
        &reg,
    )
    .unwrap();
    let compiled = d.doc().song.tracks[0].lanes[0].clips[0].id.0;
    let e = rejects(
        &mut d,
        apply_q(QuantizeParams::default(), vec![compiled]),
        &reg,
    );
    assert!(e.to_string().contains("no note"), "{e}");
}
