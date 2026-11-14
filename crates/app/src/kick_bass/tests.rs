use super::*;
use atomik_model::{Clip, ClipContent, ClipId, Note, SongCommand, new_project_doc};
use atomik_render::default_registry;
use std::sync::atomic::AtomicBool;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn clip(notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 8 * PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    }
}

fn apply(doc: &mut Document, registry: &Registry, command: Command) {
    doc.apply(command.clone(), registry)
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

fn set(doc: &mut Document, registry: &Registry, device: &str, param: &str, value: f32) {
    apply(
        doc,
        registry,
        Command::SetParam {
            key: key(device),
            param: param.to_owned(),
            value,
            merge: None,
        },
    );
}

/// A song with a short kick and a steady bass on every beat, both a sine at A1 (55 Hz) from a
/// poly synth that restarts each note at its start angle: the kick at 0, the bass at
/// `bass_start` degrees.
pub(crate) fn scene(bass_start: f32) -> RackDoc {
    let registry = default_registry();
    let mut doc = Document::new(new_project_doc());
    for name in ["kick", "bass"] {
        apply(
            &mut doc,
            &registry,
            Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.to_owned(),
                key: Some(key(name)),
            },
        );
        apply(
            &mut doc,
            &registry,
            Command::Rename {
                key: key(name),
                name: if name == "kick" {
                    "Kick Drum".to_owned()
                } else {
                    "Sub Bass".to_owned()
                },
            },
        );
        apply(
            &mut doc,
            &registry,
            Command::Song(SongCommand::AddTrack { device: key(name) }),
        );
        for (param, value) in [
            ("osc1.wave", 3.0),
            ("osc.mix", 0.0),
            ("filter.cutoff", 20_000.0),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("voices", 1.0),
            ("env.amp.attack", 1.0),
            ("env.amp.release", 5.0),
            ("osc.phase", 0.0),
        ] {
            set(&mut doc, &registry, name, param, value);
        }
    }
    set(&mut doc, &registry, "kick", "master.level", 6.0);
    set(&mut doc, &registry, "bass", "master.level", -9.0);
    set(&mut doc, &registry, "kick", "env.amp.decay", 150.0);
    set(&mut doc, &registry, "kick", "env.amp.sustain", 0.0);
    set(&mut doc, &registry, "bass", "env.amp.sustain", 1.0);
    set(&mut doc, &registry, "bass", "osc.phase_start", bass_start);
    let beat = |b: i64, len: i64| Note::new(b * PPQ, len, 33, 0.9);
    for (name, length) in [("kick", PPQ / 4), ("bass", PPQ * 3 / 4)] {
        let notes = (0..8).map(|b| beat(b, length)).collect();
        apply(
            &mut doc,
            &registry,
            Command::Song(SongCommand::AddClip {
                device: key(name),
                lane: 0,
                clip: clip(notes),
            }),
        );
    }
    doc.doc().clone()
}

fn setup() -> Setup {
    Setup {
        kick: key("kick"),
        bass: key("bass"),
    }
}

fn analysis_of(doc: &RackDoc) -> Analysis {
    let registry = registry_for(doc).unwrap();
    let cancel = AtomicBool::new(false);
    analyse(doc, &registry, None, &setup(), (&cancel, &|_| {}))
        .unwrap()
        .unwrap()
}

#[test]
fn a_bass_that_starts_against_the_kick_cancels_and_one_that_starts_with_it_builds_up() {
    let against = analysis_of(&scene(180.0));
    assert!(
        against.base.hits.len() >= 6,
        "{:?}",
        against.base.hits.len()
    );
    assert!(against.base.cancelling() >= 6, "{:?}", against.base);
    assert!(
        against.base.overall_db < -1.5,
        "{}",
        against.base.overall_db
    );
    let with = analysis_of(&scene(0.0));
    assert!(
        with.base.cancelling() == 0 && with.base.overall_db > 1.0,
        "{:?}",
        with.base
    );
    let hz = with.base.kick_hz.expect("the kick pitch");
    assert!((hz - 55.0).abs() < 3.0, "{hz}");
    assert!(with.trials.is_empty(), "nothing to fix: {:?}", with.trials);
}

#[test]
fn the_fixes_offered_are_proven_by_a_render_and_the_start_phase_is_among_them() {
    let doc = scene(180.0);
    let a = analysis_of(&doc);
    assert!(!a.trials.is_empty(), "{a:?}");
    for t in &a.trials {
        assert!(t.check.score() >= a.base.score() + MIN_GAIN, "{t:?}");
    }
    let best = &a.trials[0];
    assert!(best.check.overall_db > a.base.overall_db + 2.0, "{best:?}");
    assert!(
        a.trials
            .iter()
            .any(|t| matches!(t.fix, Fix::StartPhase { .. })),
        "{:?}",
        a.trials
    );
    // The analysis never touches the song it was given.
    assert_eq!(doc, scene(180.0));
}

#[test]
fn the_analysis_is_the_same_every_time() {
    let doc = scene(180.0);
    assert_eq!(analysis_of(&doc), analysis_of(&doc));
}

#[test]
fn applying_a_fix_is_one_undo_step_and_the_check_then_agrees_with_the_trial() {
    let registry = default_registry();
    let doc = scene(180.0);
    let a = analysis_of(&doc);
    for trial in &a.trials {
        let mut scratch = Scratch {
            document: Document::new(doc.clone()),
            registry: &registry,
        };
        let text = apply_fix(&mut scratch, &a.setup, &trial.fix, 48_000).unwrap();
        assert!(text.starts_with("Kick and Bass: "), "{text}");
        let fixed = analysis_of(scratch.document.doc());
        assert_eq!(fixed.base, trial.check, "{:?}", trial.fix);
        // One undo puts the song back.
        scratch.document.undo().unwrap();
        assert_eq!(scratch.document.doc(), &doc, "{:?}", trial.fix);
    }
}

#[test]
fn polarity_adds_one_stereo_tool_on_the_bass_channel_and_a_second_fix_reuses_it() {
    let registry = default_registry();
    let doc = scene(180.0);
    let mut scratch = Scratch {
        document: Document::new(doc),
        registry: &registry,
    };
    apply_fix(&mut scratch, &setup(), &Fix::Polarity, 48_000).unwrap();
    apply_fix(
        &mut scratch,
        &setup(),
        &Fix::Rotate {
            degrees: 40.0,
            hz: 55.0,
        },
        48_000,
    )
    .unwrap();
    let tools: Vec<_> = scratch
        .document
        .doc()
        .devices
        .iter()
        .filter(|d| d.type_id == STEREO_TOOL)
        .collect();
    assert_eq!(tools.len(), 1);
    let bass_channel = channel_of(scratch.document.doc(), &key("bass")).unwrap();
    assert_eq!(
        insert_chain(scratch.document.doc(), &registry, &bass_channel),
        vec![tools[0].key.clone()]
    );
    let value = |p: &str| {
        scratch
            .document
            .doc()
            .param_value(&tools[0].key, p, &registry)
            .unwrap()
    };
    assert!((value("invert_l") - 1.0).abs() < 1e-6 && (value("invert_r") - 1.0).abs() < 1e-6);
    assert!((value("rotate") - 40.0).abs() < 1e-6 && (value("rotate_hz") - 55.0).abs() < 1e-6);
}

#[test]
fn a_kick_nudge_needs_a_drum_machine_and_a_start_phase_needs_a_synth() {
    let registry = default_registry();
    let mut scratch = Scratch {
        document: Document::new(scene(0.0)),
        registry: &registry,
    };
    let nudge = Fix::KickPitch {
        pad: 1,
        cents: 50.0,
    };
    assert!(apply_fix(&mut scratch, &setup(), &nudge, 48_000).is_err());
    let swapped = Setup {
        kick: key("kick"),
        bass: key("nothing"),
    };
    let phase = Fix::StartPhase { degrees: 90.0 };
    assert!(apply_fix(&mut scratch, &swapped, &phase, 48_000).is_err());
    assert!(apply_fix(&mut scratch, &swapped, &Fix::Polarity, 48_000).is_err());
}

#[test]
fn a_silent_or_missing_track_is_said_in_words() {
    let doc = scene(0.0);
    let registry = registry_for(&doc).unwrap();
    let cancel = AtomicBool::new(false);
    let missing = Setup {
        kick: key("kick"),
        bass: key("ghost"),
    };
    let err = analyse(&doc, &registry, None, &missing, (&cancel, &|_| {})).unwrap_err();
    assert!(err.contains("no mixer channel"), "{err}");
    let mut muted = Document::new(doc);
    let channel = channel_of(muted.doc(), &key("bass")).unwrap();
    set(&mut muted, &registry, &channel.0, "mute", 1.0);
    let err = analyse(muted.doc(), &registry, None, &setup(), (&cancel, &|_| {})).unwrap_err();
    assert!(err.contains("makes no sound"), "{err}");
}

#[test]
fn a_cancelled_analysis_gives_nothing() {
    let doc = scene(180.0);
    let registry = registry_for(&doc).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        analyse(&doc, &registry, None, &setup(), (&cancel, &|_| {})).unwrap(),
        None
    );
}

#[test]
fn the_labels_say_what_each_fix_does() {
    let rate = 48_000;
    assert!(Fix::Polarity.label(rate).contains("polarity"));
    assert!(Fix::Delay { frames: 480 }.label(rate).contains("10.00 ms"));
    assert!(
        Fix::StartPhase { degrees: 90.0 }
            .label(rate)
            .contains("90 degrees")
    );
    let nudge = Fix::KickPitch {
        pad: 3,
        cents: -50.0,
    };
    assert!(nudge.label(rate).contains("-50 cents"));
    let rotate = Fix::Rotate {
        degrees: 40.0,
        hz: 55.0,
    };
    assert!(rotate.label(rate).contains("40 degrees at 55 Hz"));
}

#[test]
fn an_uneven_bass_gets_the_peak_evening_rotation_proven_by_a_render() {
    let registry = default_registry();
    let mut doc = Document::new(scene(0.0));
    for (param, value) in [
        ("osc2.wave", 3.0),
        ("osc2.octave", 1.0),
        ("osc.mix", 0.4),
        ("osc.phase_start", 60.0),
    ] {
        set(&mut doc, &registry, "bass", param, value);
    }
    let a = analysis_of(doc.doc());
    let h = a.headroom.expect("the bass is uneven");
    assert!(h.found.angle_deg > 0.0, "{h:?}");
    assert!(h.found.headroom_db > 0.3, "{h:?}");
    assert!(
        matches!(h.trial.fix, Fix::Rotate { degrees, .. } if (degrees - h.found.angle_deg).abs() < 1e-6)
    );
    // The trial was rendered: it has hits of its own.
    assert!(!h.trial.check.hits.is_empty());
    // A plain sine has nothing to even out.
    assert!(analysis_of(&scene(0.0)).headroom.is_none());
}
