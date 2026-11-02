//! Macro banks (model half): the mapping maths (range, invert, several targets), the commands and
//! their undo, what they refuse, assignments that outlive their devices, the expansion that sets
//! the driven params and the lanes, and project format 29 (round trip, a stable text, version 28
//! files loading unchanged, refusals with their JSON path).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, Command, Curve, DeviceKey, Document, MACRO_BANK_TYPE, MacroCommand,
    ModelError, PROJECT_VERSION, SyncOp, assign_position, assign_value, expand_bank_lane,
    load_project, macro_index_of, macro_key, save_project,
};
use common::song::{add_clip, add_track, auto_clip, point, sc};
use common::*;
use serde_json::Value;

const CUTOFF: &str = "filter.cutoff";
const WAVE: &str = "osc1.wave";
const FADER: &str = "fader";

fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    run(
        &mut d,
        &r,
        ["a", "b"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    (d, r)
}

fn assign(bank: Option<&str>, index: u8, device: &str, param: &str) -> MacroCommand {
    MacroCommand::Assign {
        bank: bank.map(key),
        index,
        device: key(device),
        param: param.into(),
        min: None,
        max: None,
        invert: false,
    }
}

fn run_macro(d: &mut Document, r: &Registry, c: MacroCommand) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(Command::Macro(c), r)
}

fn bank_key(d: &Document) -> DeviceKey {
    d.doc().banks().next().expect("a bank").key.clone()
}

fn param_of(d: &Document, device: &str, param: &str) -> Option<f32> {
    d.doc()
        .device(&key(device))
        .unwrap()
        .params
        .get(param)
        .copied()
}

fn set(d: &mut Document, r: &Registry, device: &str, param: &str, value: f32) {
    d.apply(
        Command::SetParam {
            key: key(device),
            param: param.into(),
            value,
            merge: None,
        },
        r,
    )
    .unwrap();
}

fn desc(r: &Registry, type_id: &str, param: &str) -> atomik_core::ParamDescriptor {
    *r.get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
}

/// A bank with macro 1 driving `a` cutoff and macro 2 driving `a` wave and `b` cutoff.
fn mapped() -> (Document, Registry) {
    let (mut d, r) = project();
    run_macro(&mut d, &r, assign(None, 0, "a", CUTOFF)).unwrap();
    let bank = bank_key(&d);
    run_macro(&mut d, &r, assign(Some(bank.as_str()), 1, "a", WAVE)).unwrap();
    run_macro(
        &mut d,
        &r,
        MacroCommand::Assign {
            bank: Some(bank.clone()),
            index: 1,
            device: key("b"),
            param: CUTOFF.into(),
            min: Some(200.0),
            max: Some(2000.0),
            invert: true,
        },
    )
    .unwrap();
    run_macro(
        &mut d,
        &r,
        MacroCommand::Rename {
            bank,
            index: 0,
            name: " Brightness ".into(),
        },
    )
    .unwrap();
    (d, r)
}

// ------------------------------------------------------------------------------ the maths

#[test]
fn the_value_is_linear_between_min_and_max_and_inverts() {
    let r = registry();
    let p = desc(&r, SYNTH, CUTOFF);
    let mut a = atomik_model::MacroAssign {
        id: 1,
        bank: key("bank"),
        index: 0,
        device: key("a"),
        param: CUTOFF.into(),
        min: 400.0,
        max: 1400.0,
        invert: false,
    };
    assert_eq!(assign_value(&a, 0.0, &p), 400.0);
    assert_eq!(assign_value(&a, 0.5, &p), 900.0);
    assert_eq!(assign_value(&a, 1.0, &p), 1400.0);
    a.invert = true;
    assert_eq!(assign_value(&a, 0.0, &p), 1400.0);
    assert_eq!(assign_value(&a, 1.0, &p), 400.0);
    // A range the other way round runs backwards too, and the two flags cancel.
    a.invert = false;
    (a.min, a.max) = (1400.0, 400.0);
    assert_eq!(assign_value(&a, 0.25, &p), 1150.0);
    a.invert = true;
    assert_eq!(assign_value(&a, 0.25, &p), 650.0);
    // Never outside the param's own range.
    (a.min, a.max, a.invert) = (p.min, p.max, false);
    assert_eq!(assign_value(&a, 2.0, &p), p.max);
    assert_eq!(assign_value(&a, -1.0, &p), p.min);
    assert_eq!(assign_value(&a, f32::NAN, &p), p.default);
}

#[test]
fn a_discrete_param_rounds_half_up_and_the_position_is_the_inverse() {
    let r = registry();
    let p = desc(&r, SYNTH, WAVE);
    let a = atomik_model::MacroAssign {
        id: 1,
        bank: key("bank"),
        index: 0,
        device: key("a"),
        param: WAVE.into(),
        min: p.min,
        max: p.max,
        invert: false,
    };
    // Wave runs 0..3: thirds of the macro.
    let seen: Vec<f32> = [0.0, 0.16, 0.17, 0.5, 0.83, 0.84, 1.0]
        .iter()
        .map(|m| assign_value(&a, *m, &p))
        .collect();
    assert_eq!(seen, [0.0, 0.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
    for v in [0.0, 1.0, 2.0, 3.0] {
        assert_eq!(assign_value(&a, assign_position(&a, v), &p), v);
    }
    let inv = atomik_model::MacroAssign { invert: true, ..a };
    for v in [0.0, 1.0, 2.0, 3.0] {
        assert_eq!(assign_value(&inv, assign_position(&inv, v), &p), v);
    }
}

#[test]
fn the_macro_keys_are_macro1_to_macro8() {
    assert_eq!(macro_key(0), "macro1");
    assert_eq!(macro_key(7), "macro8");
    assert_eq!(macro_index_of("macro3"), Some(2));
    for bad in ["macro0", "macro9", "macro01", "macro", "volume"] {
        assert_eq!(macro_index_of(bad), None, "{bad}");
    }
}

// ----------------------------------------------------------------------------- the commands

#[test]
fn assigning_to_a_new_bank_is_one_undo_step() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    let ops = run_macro(&mut d, &r, assign(None, 2, "a", CUTOFF)).unwrap();
    let bank = bank_key(&d);
    assert!(ops.contains(&SyncOp::MacrosChanged {
        container: bank.clone()
    }));
    assert_eq!(d.doc().device(&bank).unwrap().type_id, MACRO_BANK_TYPE);
    let assigns = &d.doc().macros.assigns;
    assert_eq!(assigns.len(), 1);
    assert_eq!((assigns[0].id, assigns[0].index), (1, 2));
    let p = desc(&r, SYNTH, CUTOFF);
    assert_eq!((assigns[0].min, assigns[0].max), (p.min, p.max));
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    assert_eq!(d.doc().macros.assigns.len(), 1);
    assert_eq!(d.doc().banks().count(), 1);
}

#[test]
fn the_first_assignment_keeps_the_value_of_its_target() {
    let (mut d, r) = project();
    set(&mut d, &r, "a", CUTOFF, 1000.0);
    run_macro(&mut d, &r, assign(None, 0, "a", CUTOFF)).unwrap();
    let bank = bank_key(&d);
    let a = d.doc().macros.assigns[0].clone();
    let p = desc(&r, SYNTH, CUTOFF);
    // The macro moved to where the target already is: nothing jumps.
    let m = d.doc().bank_macro_value(&bank, 0);
    assert!(m > 0.0 && m < 1.0, "{m}");
    assert_eq!(assign_value(&a, m, &p), 1000.0);
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(1000.0));
    // One undo step took both.
    d.undo(&r).unwrap();
    assert!(d.doc().macros.assigns.is_empty());
    assert_eq!(d.doc().banks().count(), 0);
}

#[test]
fn a_macro_drives_several_params_and_a_param_follows_one_macro() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    assert_eq!(d.doc().macros.of_macro(&bank, 1).count(), 2);
    assert_eq!(d.doc().macros.of_macro(&bank, 0).count(), 1);
    // Moving the cutoff of `a` to macro 2 takes it from macro 1 in the same step.
    run_macro(&mut d, &r, assign(Some(bank.as_str()), 1, "a", CUTOFF)).unwrap();
    assert_eq!(d.doc().macros.of_macro(&bank, 0).count(), 0);
    assert_eq!(d.doc().macros.of_macro(&bank, 1).count(), 3);
    assert_eq!(d.doc().macros.assigns.len(), 3);
    d.undo(&r).unwrap();
    assert_eq!(d.doc().macros.of_macro(&bank, 0).count(), 1);
}

#[test]
fn a_driven_param_refuses_a_direct_edit_and_follows_the_macro() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    let e = d
        .apply(
            Command::SetParam {
                key: key("a"),
                param: CUTOFF.into(),
                value: 900.0,
                merge: None,
            },
            &r,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    assert!(e.to_string().contains("Brightness"), "{e}");
    // The macro is an ordinary param: its change moves every target.
    set(&mut d, &r, bank.as_str(), "macro2", 1.0);
    let p = desc(&r, SYNTH, CUTOFF);
    assert_eq!(d.doc().driven_value(&r, &key("a"), WAVE), Some(3.0));
    // Inverted with its own range: macro 1 gives the minimum, 200.
    assert_eq!(d.doc().driven_value(&r, &key("b"), CUTOFF), Some(200.0));
    assert!(p.max > 2000.0);
    let updates = d.doc().bank_updates(&r, &bank);
    assert_eq!(updates.len(), 3);
    assert!(updates.contains(&(key("a"), WAVE.into(), 3.0)));
    // And the effective value is the macro's, not the stored one.
    assert_eq!(d.doc().effective_param(&r, &key("b"), CUTOFF), Some(200.0));
    assert_ne!(param_of(&d, "b", CUTOFF), Some(200.0));
}

#[test]
fn set_range_edits_the_range_and_refuses_a_value_outside_the_param() {
    let (mut d, r) = mapped();
    let id = d
        .doc()
        .macros
        .assigns
        .iter()
        .find(|a| a.device == key("b"))
        .unwrap()
        .id;
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetRange {
            id,
            min: 300.0,
            max: 900.0,
            invert: false,
        },
    )
    .unwrap();
    let a = d
        .doc()
        .macros
        .assigns
        .iter()
        .find(|a| a.id == id)
        .unwrap()
        .clone();
    assert_eq!((a.min, a.max, a.invert), (300.0, 900.0, false));
    let same = MacroCommand::SetRange {
        id,
        min: 300.0,
        max: 900.0,
        invert: false,
    };
    assert!(run_macro(&mut d, &r, same).is_err(), "no change is refused");
    let p = desc(&r, SYNTH, CUTOFF);
    for (min, max) in [(p.min - 1.0, 900.0), (300.0, p.max * 2.0), (f32::NAN, 1.0)] {
        let c = MacroCommand::SetRange {
            id,
            min,
            max,
            invert: false,
        };
        assert!(
            matches!(run_macro(&mut d, &r, c), Err(ModelError::OutOfRange(_))),
            "{min} {max}"
        );
    }
    assert!(
        run_macro(
            &mut d,
            &r,
            MacroCommand::SetRange {
                id: 99,
                min: 0.0,
                max: 1.0,
                invert: false
            }
        )
        .is_err()
    );
    d.undo(&r).unwrap();
    let a = d.doc().macros.assigns.iter().find(|a| a.id == id).unwrap();
    assert_eq!((a.min, a.max, a.invert), (200.0, 2000.0, true));
}

#[test]
fn removing_an_assignment_leaves_the_param_where_the_macro_put_it() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    set(&mut d, &r, bank.as_str(), "macro2", 1.0);
    let id = d
        .doc()
        .macros
        .assigns
        .iter()
        .find(|a| a.device == key("a") && a.param == WAVE)
        .unwrap()
        .id;
    run_macro(&mut d, &r, MacroCommand::Remove { id }).unwrap();
    // Free again: it holds the value it had (3) and can be edited.
    assert_eq!(param_of(&d, "a", WAVE), Some(3.0));
    assert_eq!(d.doc().bank_driver(&key("a"), WAVE), None);
    set(&mut d, &r, "a", WAVE, 1.0);
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc().driven_value(&r, &key("a"), WAVE), Some(3.0));
    assert_eq!(param_of(&d, "a", WAVE), None);
    assert!(run_macro(&mut d, &r, MacroCommand::Remove { id: 99 }).is_err());
}

#[test]
fn clearing_a_macro_frees_all_its_targets_in_one_step() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    set(&mut d, &r, bank.as_str(), "macro2", 0.5);
    run_macro(
        &mut d,
        &r,
        MacroCommand::Clear {
            bank: bank.clone(),
            index: 1,
        },
    )
    .unwrap();
    assert_eq!(d.doc().macros.assigns.len(), 1);
    assert!(param_of(&d, "a", WAVE).is_some() && param_of(&d, "b", CUTOFF).is_some());
    assert!(
        run_macro(
            &mut d,
            &r,
            MacroCommand::Clear {
                bank: bank.clone(),
                index: 1
            }
        )
        .is_err()
    );
    d.undo(&r).unwrap();
    assert_eq!(d.doc().macros.assigns.len(), 3);
    assert_eq!(param_of(&d, "b", CUTOFF), None);
}

#[test]
fn names_are_trimmed_limited_and_can_be_cleared() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    assert_eq!(d.doc().macro_name(&bank, 0), "Brightness");
    assert_eq!(d.doc().macro_name(&bank, 4), "Macro 5");
    let name = |n: &str| MacroCommand::Rename {
        bank: bank.clone(),
        index: 0,
        name: n.into(),
    };
    assert!(run_macro(&mut d, &r, name(&"x".repeat(25))).is_err());
    assert!(run_macro(&mut d, &r, name("Brightness")).is_err());
    run_macro(&mut d, &r, name("")).unwrap();
    assert_eq!(d.doc().macro_name(&bank, 0), "Macro 1");
    d.undo(&r).unwrap();
    assert_eq!(d.doc().macro_name(&bank, 0), "Brightness");
}

#[test]
fn what_cannot_be_driven_is_refused() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    let refused = |d: &mut Document, c: MacroCommand| {
        let e = run_macro(d, &r, c).unwrap_err();
        assert!(
            matches!(
                e,
                ModelError::BadMacro(_) | ModelError::UnknownKey(_) | ModelError::UnknownDevice(_)
            ),
            "{e}"
        );
    };
    // A macro 9, a bank that is not one, a param that does not exist, a bank as a target.
    refused(&mut d, assign(Some(bank.as_str()), 8, "a", WAVE));
    refused(&mut d, assign(Some("a"), 0, "b", WAVE));
    refused(&mut d, assign(Some("nope"), 0, "b", WAVE));
    refused(&mut d, assign(Some(bank.as_str()), 0, "b", "no.such"));
    refused(
        &mut d,
        assign(Some(bank.as_str()), 0, bank.as_str(), "macro1"),
    );
    refused(&mut d, assign(Some(bank.as_str()), 0, "ghost", WAVE));
    // Outside the param's range.
    let c = MacroCommand::Assign {
        bank: Some(bank.clone()),
        index: 0,
        device: key("b"),
        param: WAVE.into(),
        min: Some(-1.0),
        max: None,
        invert: false,
    };
    assert!(matches!(
        run_macro(&mut d, &r, c),
        Err(ModelError::OutOfRange(_))
    ));
}

#[test]
fn containers_and_their_insides_have_their_own_macros() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
        ],
    );
    run_macro(&mut d, &r, assign(None, 0, "a", WAVE)).unwrap();
    let bank = bank_key(&d);
    for target in ["c", "c/fx"] {
        let e = run_macro(&mut d, &r, assign(Some(bank.as_str()), 0, target, "macro1"))
            .or_else(|_| run_macro(&mut d, &r, assign(Some(bank.as_str()), 0, target, "mix")))
            .unwrap_err();
        assert!(e.to_string().contains("container"), "{target}: {e}");
    }
}

#[test]
fn a_param_with_automation_on_its_own_lane_is_refused_and_so_is_a_lane_for_a_driven_one() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add_track("a"),
            sc(atomik_model::SongCommand::AddAutomationLane {
                device: key("a"),
                param: CUTOFF.into(),
            }),
            add_clip(
                "a",
                1,
                auto_clip(0, 960, vec![point(0, 400.0, Curve::Linear)]),
            ),
        ],
    );
    let e = run_macro(&mut d, &r, assign(None, 0, "a", CUTOFF)).unwrap_err();
    assert!(e.to_string().contains("automation"), "{e}");
    // An empty lane is no data: it may be driven (the song builder skips it).
    run(
        &mut d,
        &r,
        [sc(atomik_model::SongCommand::AddAutomationLane {
            device: key("a"),
            param: WAVE.into(),
        })],
    );
    run_macro(&mut d, &r, assign(None, 0, "a", WAVE)).unwrap();
    assert_eq!(d.doc().banks().count(), 1);
    // A new lane or clip for a driven param says to automate the macro.
    let e = d
        .apply(
            add_clip(
                "a",
                2,
                auto_clip(0, 960, vec![point(0, 1.0, Curve::Linear)]),
            ),
            &r,
        )
        .unwrap_err();
    assert!(e.to_string().contains("automate the macro"), "{e}");
    // No lane can be added for a param that is already driven.
    let bank = bank_key(&d);
    run_macro(&mut d, &r, assign(Some(bank.as_str()), 1, "b", CUTOFF)).unwrap();
    let e = d
        .apply(
            sc(atomik_model::SongCommand::AddAutomationLane {
                device: key("b"),
                param: CUTOFF.into(),
            }),
            &r,
        )
        .unwrap_err();
    assert!(e.to_string().contains("automate the macro"), "{e}");
}

#[test]
fn an_assignment_outlives_its_devices_and_undo_brings_it_back() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    let all = d.doc().macros.clone();
    // The target goes: the assignment stays but does nothing, and the param is free to name.
    d.apply(Command::RemoveDevice { key: key("b") }, &r)
        .unwrap();
    assert_eq!(d.doc().macros, all);
    assert_eq!(d.doc().bank_updates(&r, &bank).len(), 2);
    d.undo(&r).unwrap();
    assert_eq!(d.doc().bank_updates(&r, &bank).len(), 3);
    // The bank goes: nothing is driven.
    d.apply(Command::RemoveDevice { key: bank.clone() }, &r)
        .unwrap();
    assert_eq!(d.doc().macros, all);
    assert_eq!(d.doc().bank_driver(&key("a"), CUTOFF), None);
    let expanded = d.doc().expand(&r);
    assert!(expanded.macro_targets.is_empty());
    set(&mut d, &r, "a", CUTOFF, 1234.0);
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert!(d.doc().bank_driver(&key("a"), CUTOFF).is_some());
}

// ---------------------------------------------------------------------------- the expansion

#[test]
fn the_expansion_sets_the_driven_params_and_lists_the_targets() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    set(&mut d, &r, bank.as_str(), "macro2", 0.5);
    let x = d.doc().expand(&r);
    let a = x.devices.iter().find(|e| e.key == key("a")).unwrap();
    let b = x.devices.iter().find(|e| e.key == key("b")).unwrap();
    assert_eq!(a.params[WAVE], 2.0);
    // Inverted over 200..2000 at one half.
    assert_eq!(b.params[CUTOFF], 1100.0);
    let targets: Vec<_> = x
        .macro_targets
        .iter()
        .map(|(c, i, t)| (c.clone(), *i, t.len()))
        .collect();
    assert_eq!(targets, [(bank.clone(), 0, 1), (bank, 1, 2)]);
    // The document's own params are untouched.
    assert_eq!(param_of(&d, "b", CUTOFF), None);
}

#[test]
fn a_document_without_assignments_expands_as_it_did() {
    let (d, r) = project();
    let x = d.doc().expand(&r);
    assert!(x.macro_targets.is_empty());
    for e in &x.devices {
        assert_eq!(e.params, d.doc().device(&e.key).unwrap().params);
    }
}

#[test]
fn a_lane_on_a_macro_expands_to_one_lane_per_target_with_every_point_mapped() {
    let (d, r) = mapped();
    let bank = bank_key(&d);
    let pts = [
        point(0, 0.0, Curve::Linear),
        point(480, 0.25, Curve::Step),
        point(960, 1.0, Curve::Linear),
    ];
    let lanes = expand_bank_lane(d.doc(), &r, &bank, "macro2", &pts).unwrap();
    assert_eq!(lanes.len(), 2);
    let (dev, param, mapped) = &lanes[1];
    assert_eq!((dev, param.as_str()), (&key("b"), CUTOFF));
    let values: Vec<f32> = mapped.iter().map(|p| p.value).collect();
    // Inverted over 200..2000.
    assert_eq!(values, [2000.0, 1550.0, 200.0]);
    // Ticks and curves are the lane's own.
    assert_eq!(
        mapped.iter().map(|p| (p.tick, p.curve)).collect::<Vec<_>>(),
        pts.iter()
            .map(|p: &AutomationPoint| (p.tick, p.curve))
            .collect::<Vec<_>>()
    );
    // An unassigned macro gives no lanes, and what is no macro gives none.
    assert_eq!(
        expand_bank_lane(d.doc(), &r, &bank, "macro8", &pts)
            .unwrap()
            .len(),
        0
    );
    assert!(expand_bank_lane(d.doc(), &r, &bank, "volume", &pts).is_none());
    assert!(expand_bank_lane(d.doc(), &r, &key("a"), "macro1", &pts).is_none());
}

#[test]
fn a_bank_is_an_automatable_track() {
    let (mut d, r) = mapped();
    let bank = bank_key(&d);
    run(
        &mut d,
        &r,
        [sc(atomik_model::SongCommand::AddAutomationLane {
            device: bank.clone(),
            param: "macro1".into(),
        })],
    );
    assert!(
        d.doc()
            .song
            .track(&bank)
            .unwrap()
            .automation_lane("macro1")
            .is_some()
    );
    // An effect is automated on a track of its own, unless a macro drives the param.
    let (mut e, r2) = project();
    run(&mut e, &r2, [add(usize::MAX, DELAY, Some("fx"))]);
    run_macro(&mut e, &r2, assign(None, 0, "fx", "mix")).unwrap();
    assert!(
        e.apply(
            sc(atomik_model::SongCommand::AddAutomationLane {
                device: key("fx"),
                param: "mix".into(),
            }),
            &r2,
        )
        .is_err()
    );
}

#[test]
fn channel_strip_params_can_be_driven_except_solo() {
    let (mut d, r) = project();
    run_macro(&mut d, &r, assign(None, 0, "channel", FADER)).unwrap();
    let bank = bank_key(&d);
    let e = run_macro(
        &mut d,
        &r,
        assign(Some(bank.as_str()), 0, "channel", "solo"),
    )
    .unwrap_err();
    assert!(e.to_string().contains("solo"), "{e}");
}

// -------------------------------------------------------------------------------- the file

#[test]
fn macros_round_trip_in_format_29_in_a_stable_text() {
    let (d, r) = mapped();
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 40);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 40);
    let m = &v["macros"];
    let assigns = m["assigns"].as_array().unwrap();
    assert_eq!(assigns.len(), 3);
    assert_eq!(assigns[0]["id"], 1);
    assert_eq!(assigns[0]["macro"], 1);
    assert_eq!(assigns[0]["device"], "a");
    assert_eq!(assigns[0]["param"], CUTOFF);
    assert!(assigns[0].get("invert").is_none());
    assert_eq!(assigns[2]["min"], 200.0);
    assert_eq!(assigns[2]["max"], 2000.0);
    assert_eq!(assigns[2]["invert"], true);
    assert_eq!(m["names"][0]["name"], "Brightness");
    assert_eq!(m["names"][0]["macro"], 1);
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.macros, d.doc().macros);
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_project_without_macros_differs_from_its_version_28_file_in_the_version_line() {
    let (d, r) = project();
    let text = save_project(d.doc());
    assert!(!text.contains("\"macros\""));
    let v28 = text.replace("\"version\": 40", "\"version\": 28");
    let loaded = load_project(&v28, &r).unwrap();
    assert!(loaded.macros.is_empty());
    assert_eq!(save_project(&loaded), text);
    // Before the format a file has no field for them.
    let with = save_project(mapped().0.doc()).replace("\"version\": 40", "\"version\": 28");
    let e = load_project(&with, &r).unwrap_err().to_string();
    assert!(e.contains("macros"), "{e}");
}

#[test]
fn an_assignment_may_name_a_device_that_does_not_exist() {
    let (d, r) = mapped();
    let text = save_project(d.doc()).replace("\"device\": \"a\"", "\"device\": \"gone\"");
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.macros.assigns[0].device, key("gone"));
    assert_eq!(loaded.bank_driver(&key("gone"), CUTOFF), None);
}

#[test]
fn bad_macros_are_refused_on_load_with_their_json_path() {
    let (d, r) = mapped();
    let text = save_project(d.doc());
    let err = |t: String| load_project(&t, &r).unwrap_err().to_string();
    let e = err(text.replacen("{\"id\": 2,", "{\"id\": 1,", 1));
    assert!(
        e.contains("macros.assigns[1].id") && e.contains("twice"),
        "{e}"
    );
    let e = err(text.replacen("{\"id\": 1,", "{\"id\": 0,", 1));
    assert!(e.contains("macros.assigns[0].id"), "{e}");
    let e = err(text.replacen("\"macro\": 1,", "\"macro\": 9,", 1));
    assert!(e.contains("macros.assigns[0].macro"), "{e}");
    let e = err(text.replacen(
        "\"macro\": 2, \"device\": \"b\"",
        "\"macro\": 0, \"device\": \"b\"",
        1,
    ));
    assert!(e.contains("macros.assigns[2].macro"), "{e}");
    // The cutoff of `a` assigned a second time.
    let e = err(text.replacen(
        "\"device\": \"a\", \"param\": \"osc1.wave\"",
        "\"device\": \"a\", \"param\": \"filter.cutoff\"",
        1,
    ));
    assert!(
        e.contains("macros.assigns[1].param") && e.contains("one macro"),
        "{e}"
    );
    let e = err(text.replacen("\"min\": 200,", "\"min\": \"low\",", 1));
    assert!(e.contains("macros.assigns[2].min"), "{e}");
    let e = err(text.replacen("\"name\": \"Brightness\"", "\"name\": \" Brightness\"", 1));
    assert!(e.contains("macros.names[0].name"), "{e}");
    let e = err(text.replacen(
        "\"name\": \"Brightness\"",
        "\"name\": \"a very long name for a macro knob\"",
        1,
    ));
    assert!(e.contains("macros.names[0].name"), "{e}");
    let e = err(text.replacen("\"macros\": {", "\"macros\": {\"extra\": 1,", 1));
    assert!(e.contains("macros"), "{e}");
}

#[test]
fn undo_after_a_load_is_exact() {
    let (d, r) = mapped();
    let text = save_project(d.doc());
    let mut loaded = Document::new(load_project(&text, &r).unwrap());
    let id = loaded.doc().macros.assigns[0].id;
    run_macro(&mut loaded, &r, MacroCommand::Remove { id }).unwrap();
    loaded.undo(&r).unwrap();
    assert_eq!(loaded.doc().macros, d.doc().macros);
    assert_eq!(loaded.doc(), d.doc());
}
