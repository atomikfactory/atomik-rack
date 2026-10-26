//! Step sequencer patterns in the document: the model types and
//! their validation, the four pattern commands, `Batch` acceptance, undo/redo and drag
//! merging, `SyncOp::PatternsChanged`, the cable rules for note players and the engine
//! hand-over data derived from the model.

mod common;

use atomik_device_sdk::{
    Direction, PatternData, PlayerData, Registry, SeqResolution as SdkResolution, SocketKind,
};
use atomik_model::{
    Command, DeviceKey, Document, ModelError, PatternDoc, RackDoc, SeqResolution, StepDoc,
    StepSeqDoc, SyncOp, empty_doc, player_data_for,
};
use common::*;
use proptest::prelude::*;

fn reg() -> Registry {
    registry()
}

/// A document with a sequencer `q`.
fn doc_with_seq(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SEQ, Some("q")), r).unwrap();
    // A fresh history: the tests count undo steps from here.
    Document::new(d.doc().clone())
}

fn pats(d: &Document) -> Vec<PatternDoc> {
    d.doc()
        .device(&key("q"))
        .unwrap()
        .steps
        .clone()
        .unwrap()
        .patterns
}

fn set(index: u8, pattern: PatternDoc, merge: Option<u64>) -> Command {
    Command::SetPattern {
        device: key("q"),
        index,
        pattern,
        merge,
    }
}

fn dev(c: fn(DeviceKey) -> Command) -> Command {
    c(key("q"))
}

fn add_pat(d: DeviceKey) -> Command {
    Command::AddPattern { device: d }
}

fn remove(index: u8) -> Command {
    Command::RemovePattern {
        device: key("q"),
        index,
    }
}

fn dup(index: u8) -> Command {
    Command::DuplicatePattern {
        device: key("q"),
        index,
    }
}

fn changed() -> SyncOp {
    SyncOp::PatternsChanged { device: key("q") }
}

/// A pattern with a recognisable content.
fn busy() -> PatternDoc {
    let mut p = PatternDoc::new("Bass");
    p.length = 12;
    p.resolution = SeqResolution::Eighth;
    p.swing = 30;
    p.steps[0] = StepDoc {
        on: true,
        key: 36,
        velocity: 127,
        gate: 50,
        prob: 75,
        tie: true,
    };
    p.steps[1].on = true;
    p.steps[63].key = 127;
    p
}

fn assert_bad(r: Result<Vec<SyncOp>, ModelError>, needle: &str) {
    match r {
        Err(ModelError::BadPatterns(m)) => assert!(m.contains(needle), "{m:?} lacks {needle:?}"),
        other => panic!("expected BadPatterns({needle}), got {other:?}"),
    }
}

// ---- defaults ------------------------------------------------------------------------

#[test]
fn a_new_sequencer_carries_one_empty_16_step_pattern() {
    let r = reg();
    let d = doc_with_seq(&r);
    let steps = d.doc().device(&key("q")).unwrap().steps.clone().unwrap();
    assert_eq!(steps.patterns.len(), 1);
    let p = &steps.patterns[0];
    assert_eq!(
        (p.length, p.resolution, p.swing, p.steps.len()),
        (16, SeqResolution::Sixteenth, 0, 64)
    );
    assert!(p.steps.iter().all(|s| !s.on && s.prob == 100));
    assert_eq!(p.steps[0], StepDoc::default());
    assert_eq!(steps, StepSeqDoc::default());
    assert_eq!(p, &PatternDoc::default());
}

#[test]
fn only_devices_that_take_pattern_data_get_steps() {
    let r = reg();
    let mut d = three_devices(&r);
    d.apply(add(3, ARP, Some("a")), &r).unwrap();
    assert!(d.doc().devices.iter().all(|x| x.steps.is_none()));
    // Adding the sequencer reports no pattern change: the app builds the data on insertion.
    let mut d = Document::new(empty_doc());
    let ops = d.apply(add(0, SEQ, Some("q")), &r).unwrap();
    assert!(!ops.contains(&changed()), "{ops:?}");
}

#[test]
fn resolutions_map_onto_the_sdk_ones() {
    let pairs = [
        (SeqResolution::Quarter, SdkResolution::Quarter),
        (SeqResolution::Eighth, SdkResolution::Eighth),
        (SeqResolution::EighthTriplet, SdkResolution::EighthTriplet),
        (SeqResolution::Sixteenth, SdkResolution::Sixteenth),
        (
            SeqResolution::SixteenthTriplet,
            SdkResolution::SixteenthTriplet,
        ),
        (SeqResolution::ThirtySecond, SdkResolution::ThirtySecond),
        (
            SeqResolution::ThirtySecondTriplet,
            SdkResolution::ThirtySecondTriplet,
        ),
    ];
    assert_eq!(SeqResolution::ALL.len(), 7);
    for (i, (m, s)) in pairs.into_iter().enumerate() {
        assert_eq!(SeqResolution::ALL[i], m);
        assert_eq!(SdkResolution::from(m), s);
        assert_eq!(m.step_ticks(), s.step_ticks());
        assert_eq!(SeqResolution::from(s), m);
    }
}

// ---- SetPattern ----------------------------------------------------------------------

#[test]
fn set_pattern_replaces_one_pattern_and_undoes() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let before = d.doc().clone();
    let ops = d.apply(set(0, busy(), None), &r).unwrap();
    assert_eq!(ops, vec![changed()]);
    assert_eq!(pats(&d), vec![busy()]);
    let after = d.doc().clone();
    assert_eq!(d.undo(&r).unwrap(), vec![changed()]);
    assert_eq!(d.doc(), &before);
    assert_eq!(d.redo(&r).unwrap(), vec![changed()]);
    assert_eq!(d.doc(), &after);
    assert!(!d.can_redo());
}

#[test]
fn set_pattern_accepts_the_range_limits() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let mut p = busy();
    p.length = 64;
    p.swing = 75;
    p.steps[2] = StepDoc {
        on: true,
        key: 0,
        velocity: 1,
        gate: 1,
        prob: 0,
        tie: false,
    };
    p.steps[3].key = 127;
    p.steps[3].velocity = 127;
    p.steps[3].gate = 100;
    p.steps[3].prob = 100;
    for res in SeqResolution::ALL {
        p.resolution = res;
        d.apply(set(0, p.clone(), None), &r).unwrap();
    }
    p.length = 1;
    d.apply(set(0, p.clone(), None), &r).unwrap();
    assert_eq!(pats(&d), vec![p]);
}

#[test]
fn set_pattern_rejects_invalid_patterns_and_changes_nothing() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let before = d.doc().clone();
    type Edit = fn(&mut PatternDoc);
    let cases: Vec<(&str, Edit)> = vec![
        ("length", |p| p.length = 0),
        ("length", |p| p.length = 65),
        ("swing", |p| p.swing = 76),
        ("steps", |p| {
            p.steps.pop();
        }),
        ("steps", |p| p.steps.push(StepDoc::default())),
        ("steps", |p| p.steps.clear()),
        ("key", |p| p.steps[5].key = 128),
        ("velocity", |p| p.steps[5].velocity = 0),
        ("velocity", |p| p.steps[63].velocity = 128),
        ("gate", |p| p.steps[0].gate = 0),
        ("gate", |p| p.steps[0].gate = 101),
        ("prob", |p| p.steps[7].prob = 101),
        // A step beyond the length is kept, so it is validated too.
        ("key", |p| {
            p.length = 4;
            p.steps[40].key = 200;
        }),
        ("name", |p| p.name = "x".repeat(65)),
    ];
    for (needle, edit) in cases {
        let mut p = PatternDoc::default();
        edit(&mut p);
        assert_bad(d.apply(set(0, p, None), &r), needle);
        assert_eq!(d.doc(), &before);
        assert!(!d.can_undo());
    }
}

#[test]
fn set_pattern_rejects_bad_targets() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(add(1, SYNTH, Some("s")), &r).unwrap();
    d.apply(add(2, ARP, Some("a")), &r).unwrap();
    let before = d.doc().clone();
    assert_bad(d.apply(set(1, PatternDoc::default(), None), &r), "index");
    assert_bad(d.apply(set(255, PatternDoc::default(), None), &r), "index");
    for other in ["s", "a"] {
        let c = Command::SetPattern {
            device: key(other),
            index: 0,
            pattern: PatternDoc::default(),
            merge: None,
        };
        assert_bad(d.apply(c, &r), "pattern data");
    }
    let c = Command::SetPattern {
        device: key("nope"),
        index: 0,
        pattern: PatternDoc::default(),
        merge: None,
    };
    assert!(matches!(d.apply(c, &r), Err(ModelError::UnknownDevice(_))));
    for c in [add_pat(key("s")), remove_of("s"), dup_of("a")] {
        assert_bad(d.apply(c, &r), "pattern data");
    }
    assert_eq!(d.doc(), &before);
}

fn remove_of(k: &str) -> Command {
    Command::RemovePattern {
        device: key(k),
        index: 0,
    }
}

fn dup_of(k: &str) -> Command {
    Command::DuplicatePattern {
        device: key(k),
        index: 0,
    }
}

#[test]
fn drags_merge_into_one_undo_step() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let original = d.doc().clone();
    let mut p = PatternDoc::default();
    for v in [10, 20, 30] {
        p.steps[0].velocity = v;
        d.apply(set(0, p.clone(), Some(6 << 40)), &r).unwrap();
    }
    assert_eq!(pats(&d)[0].steps[0].velocity, 30);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &original, "the whole drag is one step");
    assert!(!d.can_undo());
    d.redo(&r).unwrap();
    assert_eq!(pats(&d)[0].steps[0].velocity, 30);
}

#[test]
fn different_merge_ids_patterns_or_no_id_do_not_merge() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(add_pat(key("q")), &r).unwrap();
    let mut p = PatternDoc::default();
    let mut step = |d: &mut Document, i: u8, v: u8, m: Option<u64>| {
        p.steps[0].velocity = v;
        d.apply(set(i, p.clone(), m), &r).unwrap();
    };
    step(&mut d, 0, 10, Some(1));
    step(&mut d, 0, 11, Some(2)); // another drag
    step(&mut d, 1, 12, Some(2)); // another pattern
    step(&mut d, 1, 13, None); // standalone
    step(&mut d, 1, 14, None);
    let mut undone = 0;
    while d.undo(&r).is_some() {
        undone += 1;
    }
    assert_eq!(undone, 1 + 5, "AddPattern plus five separate steps");
}

#[test]
fn end_merge_and_other_commands_close_a_drag() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let mut p = PatternDoc::default();
    p.steps[0].velocity = 5;
    d.apply(set(0, p.clone(), Some(1)), &r).unwrap();
    d.end_merge();
    p.steps[0].velocity = 6;
    d.apply(set(0, p.clone(), Some(1)), &r).unwrap();
    d.apply(
        Command::SetParam {
            key: key("q"),
            param: "transpose".into(),
            value: 3.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    p.steps[0].velocity = 7;
    d.apply(set(0, p, Some(1)), &r).unwrap();
    let mut undone = 0;
    while d.undo(&r).is_some() {
        undone += 1;
    }
    assert_eq!(undone, 4);
}

// ---- Add, Remove, Duplicate ----------------------------------------------------------

#[test]
fn add_pattern_appends_empty_patterns_up_to_eight() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    for n in 2..=8usize {
        let ops = d.apply(dev(add_pat), &r).unwrap();
        assert_eq!(ops, vec![changed()]);
        let all = pats(&d);
        assert_eq!(all.len(), n);
        let last = all.last().unwrap();
        assert_eq!(
            (last.length, last.resolution, last.swing),
            (16, SeqResolution::Sixteenth, 0)
        );
        assert!(last.steps.iter().all(|s| !s.on));
        assert!(!last.name.is_empty());
    }
    let before = d.doc().clone();
    assert_bad(d.apply(dev(add_pat), &r), "8");
    assert_eq!(d.doc(), &before);
    // Each add was one undo step.
    for n in (1..8).rev() {
        assert_eq!(d.undo(&r).unwrap(), vec![changed()]);
        assert_eq!(pats(&d).len(), n);
    }
    assert!(!d.can_undo());
}

#[test]
fn duplicate_pattern_copies_to_the_end() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(set(0, busy(), None), &r).unwrap();
    d.apply(dev(add_pat), &r).unwrap();
    let ops = d.apply(dup(0), &r).unwrap();
    assert_eq!(ops, vec![changed()]);
    let all = pats(&d);
    assert_eq!(all.len(), 3);
    let copy = &all[2];
    assert_eq!(copy.steps, all[0].steps);
    assert_eq!(
        (copy.length, copy.resolution, copy.swing),
        (all[0].length, all[0].resolution, all[0].swing)
    );
    assert!(copy.name.starts_with("Bass"));
    d.undo(&r).unwrap();
    assert_eq!(pats(&d).len(), 2);
    d.redo(&r).unwrap();
    assert_eq!(pats(&d), all);
}

#[test]
fn duplicate_is_refused_at_eight_and_for_a_bad_index() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    assert_bad(d.apply(dup(1), &r), "index");
    for _ in 1..8 {
        d.apply(dup(0), &r).unwrap();
    }
    assert_eq!(pats(&d).len(), 8);
    let before = d.doc().clone();
    assert_bad(d.apply(dup(0), &r), "8");
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_long_name_is_cut_when_duplicating() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let p = PatternDoc::new("é".repeat(64));
    d.apply(set(0, p, None), &r).unwrap();
    d.apply(dup(0), &r).unwrap();
    assert!(pats(&d)[1].name.chars().count() <= 64);
}

#[test]
fn remove_pattern_shifts_the_later_ones_and_refuses_the_last() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(dev(add_pat), &r).unwrap();
    d.apply(dev(add_pat), &r).unwrap();
    for (i, n) in ["one", "two", "three"].into_iter().enumerate() {
        let mut p = PatternDoc::new(n);
        p.length = 1 + i as u8;
        d.apply(set(i as u8, p, None), &r).unwrap();
    }
    let before = pats(&d);
    assert_bad(d.apply(remove(3), &r), "index");
    let ops = d.apply(remove(1), &r).unwrap();
    assert_eq!(ops, vec![changed()]);
    let names: Vec<_> = pats(&d).iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, ["one", "three"]);
    d.apply(remove(0), &r).unwrap();
    assert_eq!(pats(&d).len(), 1);
    let one = d.doc().clone();
    assert_bad(d.apply(remove(0), &r), "last");
    assert_eq!(d.doc(), &one);
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(pats(&d), before);
}

#[test]
fn removing_a_pattern_clamps_the_pattern_param_in_the_same_step() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(dev(add_pat), &r).unwrap();
    d.apply(dev(add_pat), &r).unwrap();
    let set_param = |d: &mut Document, v: f32| {
        d.apply(
            Command::SetParam {
                key: key("q"),
                param: "pattern".into(),
                value: v,
                merge: None,
            },
            &r,
        )
        .unwrap();
    };
    set_param(&mut d, 3.0);
    let before = d.doc().clone();
    let ops = d.apply(remove(0), &r).unwrap();
    assert_eq!(
        ops,
        vec![
            SyncOp::ParamChanged {
                key: key("q"),
                param: "pattern".into(),
                value: 2.0
            },
            changed()
        ]
    );
    assert_eq!(d.doc().param_value(&key("q"), "pattern", &r), Some(2.0));
    // One undo step restores both.
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    // A param already inside the new count is left alone.
    set_param(&mut d, 1.0);
    let ops = d.apply(remove(2), &r).unwrap();
    assert_eq!(ops, vec![changed()]);
    assert_eq!(d.doc().param_value(&key("q"), "pattern", &r), Some(1.0));
}

// ---- Batch ---------------------------------------------------------------------------

#[test]
fn batch_accepts_the_pattern_commands_as_one_step() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::Batch(vec![
                dev(add_pat),
                set(1, busy(), Some(9)),
                dup(1),
                remove(0),
            ]),
            &r,
        )
        .unwrap();
    assert_eq!(ops, vec![changed()], "one report per device");
    let all = pats(&d);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], busy());
    assert_eq!(all[1].steps, busy().steps);
    let after = d.doc().clone();
    assert_eq!(d.undo(&r).unwrap(), vec![changed()]);
    assert_eq!(d.doc(), &before);
    assert!(!d.can_undo());
    d.redo(&r).unwrap();
    assert_eq!(d.doc(), &after);
}

#[test]
fn a_failing_batch_changes_nothing() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let before = d.doc().clone();
    let bad = Command::Batch(vec![dev(add_pat), remove(5)]);
    assert_bad(d.apply(bad, &r), "index");
    assert_eq!(d.doc(), &before);
    assert!(!d.can_undo());
}

#[test]
fn batch_mixes_patterns_with_params() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    let ops = d
        .apply(
            Command::Batch(vec![
                set(0, busy(), None),
                Command::SetParam {
                    key: key("q"),
                    param: "transpose".into(),
                    value: 5.0,
                    merge: None,
                },
            ]),
            &r,
        )
        .unwrap();
    assert!(ops.contains(&changed()));
    assert!(ops.iter().any(|o| matches!(o, SyncOp::ParamChanged { .. })));
    d.undo(&r).unwrap();
    assert!(!d.can_undo());
}

// ---- devices and cables --------------------------------------------------------------

#[test]
fn removing_and_restoring_a_sequencer_keeps_its_patterns() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(set(0, busy(), None), &r).unwrap();
    let before = d.doc().clone();
    d.apply(Command::RemoveDevice { key: key("q") }, &r)
        .unwrap();
    assert!(d.doc().devices.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn note_players_cable_like_any_note_device() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [(SEQ, "q"), (ARP, "a"), (SYNTH, "s")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), &r).unwrap();
    }
    d.apply(connect("q:notes_out", "a:notes_in"), &r).unwrap();
    d.apply(connect("a:notes_out", "s:notes_in"), &r).unwrap();
    // A cycle through the players is refused.
    assert!(d.apply(connect("s:notes_out", "q:notes_in"), &r).is_err());
    assert!(matches!(
        d.apply(connect("a:notes_out", "q:notes_in"), &r),
        Err(ModelError::BadCable(_))
    ));
    // CV outs go to a CV input; a CV out into a Notes input is a kind mismatch.
    d.apply(connect("q:gate_cv_out", "s:cutoff_cv_in"), &r)
        .unwrap();
    assert!(
        d.apply(connect("q:pitch_cv_out", "a:notes_in"), &r)
            .is_err()
    );
    // The sequencer can also feed the synth directly (an alternative to the arp).
    let desc = r.get(SEQ).unwrap().descriptor;
    let outs: Vec<_> = desc
        .sockets
        .iter()
        .filter(|s| s.direction == Direction::Output)
        .map(|s| (s.key, s.kind))
        .collect();
    assert_eq!(
        outs,
        [
            ("notes_out", SocketKind::Notes),
            ("gate_cv_out", SocketKind::Cv),
            ("pitch_cv_out", SocketKind::Cv),
            ("velocity_cv_out", SocketKind::Cv),
            ("trig_cv_out", SocketKind::Cv),
        ]
    );
}

// ---- engine hand-over ----------------------------------------------------------------

fn data(doc: &RackDoc, r: &Registry, k: &str) -> Option<PlayerData> {
    player_data_for(doc.device(&key(k)).unwrap(), r).map(|a| (*a).clone())
}

#[test]
fn the_model_derives_the_engine_pattern_data() {
    let r = reg();
    let mut d = doc_with_seq(&r);
    d.apply(set(0, busy(), None), &r).unwrap();
    d.apply(dev(add_pat), &r).unwrap();
    let mut second = PatternDoc::new("Hats");
    second.resolution = SeqResolution::ThirtySecondTriplet;
    second.length = 64;
    second.swing = 75;
    d.apply(set(1, second, None), &r).unwrap();
    let PlayerData::StepSeq(seq) = data(d.doc(), &r, "q").unwrap() else {
        panic!("a step sequencer gets StepSeq data")
    };
    assert_eq!(seq.count, 2);
    let p0 = seq.pattern(0).unwrap();
    assert_eq!(
        (p0.length, p0.resolution, p0.swing),
        (12, SdkResolution::Eighth, 30)
    );
    let s0 = p0.steps[0];
    assert_eq!(
        (s0.on, s0.key, s0.velocity, s0.gate, s0.prob, s0.tie),
        (true, 36, 127, 50, 75, true)
    );
    assert_eq!(p0.steps[63].key, 127);
    let p1 = seq.pattern(1).unwrap();
    assert_eq!(
        (p1.length, p1.resolution, p1.swing),
        (64, SdkResolution::ThirtySecondTriplet, 75)
    );
    assert!(seq.pattern(2).is_none());
    // Unused slots are the SDK default.
    assert_eq!(seq.patterns[2], PatternData::default());
}

#[test]
fn only_player_devices_have_pattern_data() {
    let r = reg();
    let mut d = three_devices(&r);
    d.apply(add(3, SEQ, Some("q")), &r).unwrap();
    d.apply(add(4, ARP, Some("a")), &r).unwrap();
    assert!(data(d.doc(), &r, "synth").is_none());
    assert!(data(d.doc(), &r, "a").is_none());
    assert!(data(d.doc(), &r, "q").is_some());
    // A sequencer loaded without `steps` still gets the default data (an empty pattern).
    let mut doc = d.doc().clone();
    doc.devices[3].steps = None;
    let PlayerData::StepSeq(s) = data(&doc, &r, "q").unwrap() else {
        panic!("a step sequencer gets StepSeq data")
    };
    assert_eq!(s, atomik_device_sdk::StepSeqData::default());
}

// ---- properties ----------------------------------------------------------------------

fn pattern_cmd() -> impl Strategy<Value = Command> {
    let step = (
        any::<bool>(),
        0u8..=127,
        1u8..=127,
        1u8..=100,
        0u8..=100,
        any::<bool>(),
    );
    prop_oneof![
        Just(dev(add_pat)),
        (0u8..9).prop_map(remove),
        (0u8..9).prop_map(dup),
        (
            0u8..9,
            1u8..=64,
            0u8..=75,
            0usize..7,
            prop::collection::vec(step, 64),
            prop::option::of(0u64..3)
        )
            .prop_map(|(i, len, swing, res, steps, merge)| {
                let mut p = PatternDoc::new("p");
                p.length = len;
                p.swing = swing;
                p.resolution = SeqResolution::ALL[res];
                p.steps = steps
                    .into_iter()
                    .map(|(on, key, velocity, gate, prob, tie)| StepDoc {
                        on,
                        key,
                        velocity,
                        gate,
                        prob,
                        tie,
                    })
                    .collect();
                set(i, p, merge)
            }),
    ]
}

proptest! {
    #[test]
    fn any_pattern_edit_sequence_undoes_and_redoes_exactly(
        cmds in prop::collection::vec(pattern_cmd(), 0..40)
    ) {
        let r = reg();
        let mut d = doc_with_seq(&r);
        let start = d.doc().clone();
        for c in cmds {
            let before = d.doc().clone();
            if d.apply(c, &r).is_err() {
                prop_assert_eq!(d.doc(), &before);
            }
            let n = pats(&d).len();
            prop_assert!((1..=8).contains(&n));
        }
        let end = d.doc().clone();
        while d.undo(&r).is_some() {}
        prop_assert_eq!(d.doc(), &start);
        while d.redo(&r).is_some() {}
        prop_assert_eq!(d.doc(), &end);
    }
}
