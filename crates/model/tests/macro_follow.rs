//! Macro follow-ups (model half): a curve and a smoothing time per assignment, and a direct edit
//! of a driven param that moves the range of its assignment (with the drag merge into one undo
//! step); the file round trip of the two new fields.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, Command, Curve, DeviceKey, Document, MAX_MACRO_SMOOTH_MS, MacroAssign,
    MacroCommand, MacroCurve, ModelError, assign_position, assign_value, expand_bank_lane,
    load_project, save_project,
};
use common::song::point;
use common::*;
use serde_json::Value;

const CUTOFF: &str = "filter.cutoff";
const WAVE: &str = "osc1.wave";

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

fn run_macro(d: &mut Document, r: &Registry, c: MacroCommand) -> Result<(), ModelError> {
    d.apply(Command::Macro(c), r).map(|_| ())
}

fn assign(d: &mut Document, r: &Registry, bank: Option<DeviceKey>, device: &str, param: &str) {
    run_macro(
        d,
        r,
        MacroCommand::Assign {
            bank,
            index: 0,
            device: key(device),
            param: param.into(),
            min: Some(400.0),
            max: Some(1400.0),
            invert: false,
        },
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

fn bank_key(d: &Document) -> DeviceKey {
    d.doc().banks().next().expect("a bank").key.clone()
}

fn set_macro(d: &mut Document, r: &Registry, value: f32) {
    let bank = bank_key(d);
    d.apply(
        Command::SetParam {
            key: bank,
            param: "macro1".into(),
            value,
            merge: None,
        },
        r,
    )
    .unwrap();
}

fn first(d: &Document) -> MacroAssign {
    d.doc().macros.assigns[0].clone()
}

fn linear(min: f32, max: f32) -> MacroAssign {
    MacroAssign {
        id: 1,
        bank: key("bank"),
        index: 0,
        device: key("a"),
        param: CUTOFF.into(),
        min,
        max,
        invert: false,
        curve: MacroCurve::Linear,
        smooth_ms: 0,
    }
}

// ------------------------------------------------------------------------------ curves

#[test]
fn the_curves_bend_the_travel_but_keep_both_ends() {
    let r = registry();
    let p = desc(&r, SYNTH, CUTOFF);
    let mut a = linear(400.0, 1400.0);
    for curve in MacroCurve::ALL {
        a.curve = curve;
        assert_eq!(assign_value(&a, 0.0, &p), 400.0, "{curve:?}");
        assert_eq!(assign_value(&a, 1.0, &p), 1400.0, "{curve:?}");
    }
    a.curve = MacroCurve::Linear;
    assert_eq!(assign_value(&a, 0.5, &p), 900.0);
    // Slow start: half the macro covers a quarter of the range.
    a.curve = MacroCurve::Exp;
    assert_eq!(assign_value(&a, 0.5, &p), 650.0);
    // Fast start: half the macro covers three quarters.
    a.curve = MacroCurve::Log;
    assert_eq!(assign_value(&a, 0.5, &p), 1150.0);
    // Inverting runs the curve from the other end.
    a.curve = MacroCurve::Exp;
    a.invert = true;
    assert_eq!(assign_value(&a, 0.5, &p), 650.0);
    assert_eq!(assign_value(&a, 0.0, &p), 1400.0);
    assert_eq!(assign_value(&a, 0.25, &p), 400.0 + 1000.0 * 0.75 * 0.75);
}

#[test]
fn a_curve_never_leaves_the_range_and_its_position_is_the_inverse() {
    let r = registry();
    let p = desc(&r, SYNTH, CUTOFF);
    for curve in MacroCurve::ALL {
        for invert in [false, true] {
            let a = MacroAssign {
                curve,
                invert,
                ..linear(400.0, 1400.0)
            };
            for m in [0.0, 0.1, 0.5, 0.9, 1.0] {
                let v = assign_value(&a, m, &p);
                assert!((400.0..=1400.0).contains(&v), "{curve:?} {invert} {m}: {v}");
            }
            for m in [-1.0, 2.0] {
                let v = assign_value(&a, m, &p);
                assert!((p.min..=p.max).contains(&v), "{curve:?} {invert} {m}: {v}");
            }
            for v in [400.0_f32, 500.0, 900.0, 1300.0, 1400.0] {
                let back = assign_value(&a, assign_position(&a, v), &p);
                assert!((back - v).abs() < 0.05, "{curve:?} {invert} {v}: {back}");
            }
        }
    }
}

#[test]
fn curve_names_round_trip_and_next_cycles() {
    for c in MacroCurve::ALL {
        assert_eq!(MacroCurve::from_name(c.name()), Some(c));
    }
    assert_eq!(MacroCurve::from_name("sine"), None);
    assert_eq!(MacroCurve::Linear.next(), MacroCurve::Exp);
    assert_eq!(MacroCurve::Exp.next(), MacroCurve::Log);
    assert_eq!(MacroCurve::Log.next(), MacroCurve::Linear);
}

#[test]
fn set_shape_is_one_undo_step_and_the_expansion_and_lanes_follow_the_curve() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let bank = bank_key(&d);
    let id = first(&d).id;
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetShape {
            id,
            curve: MacroCurve::Exp,
            smooth_ms: 250,
        },
    )
    .unwrap();
    let a = first(&d);
    assert_eq!((a.curve, a.smooth_ms), (MacroCurve::Exp, 250));
    set_macro(&mut d, &r, 0.5);
    let x = d.doc().expand(&r);
    let e = x.devices.iter().find(|e| e.key == key("a")).unwrap();
    assert_eq!(e.params[CUTOFF], 650.0);
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(650.0));
    // A lane on the macro maps every point through the same curve.
    let pts = [
        point(0, 0.0, Curve::Linear),
        point(480, 0.5, Curve::Linear),
        point(960, 1.0, Curve::Linear),
    ];
    let lanes = expand_bank_lane(d.doc(), &r, &bank, "macro1", &pts).unwrap();
    let values: Vec<f32> = lanes[0]
        .2
        .iter()
        .map(|p: &AutomationPoint| p.value)
        .collect();
    assert_eq!(values, [400.0, 650.0, 1400.0]);
    // One undo step for the shape, and no change is refused.
    let same = MacroCommand::SetShape {
        id,
        curve: MacroCurve::Exp,
        smooth_ms: 250,
    };
    assert!(run_macro(&mut d, &r, same).is_err());
    set_macro(&mut d, &r, 0.25);
    d.undo().unwrap();
    d.undo().unwrap();
    d.undo().unwrap();
    let a = first(&d);
    assert_eq!((a.curve, a.smooth_ms), (MacroCurve::Linear, 0));
    d.redo().unwrap();
    assert_eq!(first(&d).curve, MacroCurve::Exp);
}

#[test]
fn set_shape_refuses_a_long_smoothing_time_and_an_unknown_assignment() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let id = first(&d).id;
    let long = MacroCommand::SetShape {
        id,
        curve: MacroCurve::Linear,
        smooth_ms: MAX_MACRO_SMOOTH_MS + 1,
    };
    assert!(matches!(
        run_macro(&mut d, &r, long),
        Err(ModelError::OutOfRange(_))
    ));
    let none = MacroCommand::SetShape {
        id: 99,
        curve: MacroCurve::Log,
        smooth_ms: 0,
    };
    assert!(run_macro(&mut d, &r, none).is_err());
    let top = MacroCommand::SetShape {
        id,
        curve: MacroCurve::Log,
        smooth_ms: MAX_MACRO_SMOOTH_MS,
    };
    run_macro(&mut d, &r, top).unwrap();
}

// ------------------------------------------------------------------------------ the file

#[test]
fn the_curve_and_the_smoothing_are_written_only_when_set_and_round_trip() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let a = &v["macros"]["assigns"][0];
    assert!(a.get("curve").is_none() && a.get("smooth_ms").is_none());
    let id = first(&d).id;
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetShape {
            id,
            curve: MacroCurve::Log,
            smooth_ms: 120,
        },
    )
    .unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let a = &v["macros"]["assigns"][0];
    assert_eq!(a["curve"], "log");
    assert_eq!(a["smooth_ms"], 120);
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    // Only the smoothing set: the curve stays out of the file.
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetShape {
            id,
            curve: MacroCurve::Linear,
            smooth_ms: 40,
        },
    )
    .unwrap();
    let v: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    let a = &v["macros"]["assigns"][0];
    assert!(a.get("curve").is_none());
    assert_eq!(a["smooth_ms"], 40);
}

#[test]
fn bad_curve_and_smoothing_values_are_refused_on_load_with_their_json_path() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let id = first(&d).id;
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetShape {
            id,
            curve: MacroCurve::Exp,
            smooth_ms: 90,
        },
    )
    .unwrap();
    let text = save_project(d.doc());
    let err = |t: String| load_project(&t, &r).unwrap_err().to_string();
    let e = err(text.replacen("\"curve\": \"exp\"", "\"curve\": \"sine\"", 1));
    assert!(e.contains("macros.assigns[0].curve"), "{e}");
    let e = err(text.replacen("\"smooth_ms\": 90", "\"smooth_ms\": 5001", 1));
    assert!(e.contains("macros.assigns[0].smooth_ms"), "{e}");
    let e = err(text.replacen("\"smooth_ms\": 90", "\"smooth_ms\": -1", 1));
    assert!(e.contains("macros.assigns[0].smooth_ms"), "{e}");
}

// ------------------------------------------------------------------------ editing a driven param

fn move_range(d: &mut Document, r: &Registry, param: &str, value: f32, merge: Option<u64>) {
    run_macro(
        d,
        r,
        MacroCommand::MoveRange {
            device: key("a"),
            param: param.into(),
            value,
            merge,
        },
    )
    .unwrap();
}

#[test]
fn a_direct_plain_edit_stays_refused_but_move_range_shifts_the_whole_range() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    set_macro(&mut d, &r, 0.5);
    let refused = d.apply(
        Command::SetParam {
            key: key("a"),
            param: CUTOFF.into(),
            value: 1000.0,
            merge: None,
        },
        &r,
    );
    assert!(matches!(refused, Err(ModelError::MacroDriven(_))));
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(900.0));
    move_range(&mut d, &r, CUTOFF, 1000.0, None);
    let a = first(&d);
    assert_eq!((a.min, a.max), (500.0, 1500.0));
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(1000.0));
    // The macro keeps its position, and the other end of the travel moved with the range.
    assert_eq!(d.doc().bank_macro_value(&bank_key(&d), 0), 0.5);
    set_macro(&mut d, &r, 1.0);
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(1500.0));
    // Undo puts the range back.
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!((first(&d).min, first(&d).max), (400.0, 1400.0));
}

#[test]
fn move_range_stops_at_the_param_range_and_keeps_the_span() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    set_macro(&mut d, &r, 0.5);
    let p = desc(&r, SYNTH, CUTOFF);
    move_range(&mut d, &r, CUTOFF, p.max, None);
    let a = first(&d);
    assert_eq!(a.max, p.max);
    assert_eq!(a.max - a.min, 1000.0);
    // Already at the limit: nothing to move.
    let again = MacroCommand::MoveRange {
        device: key("a"),
        param: CUTOFF.into(),
        value: p.max,
        merge: None,
    };
    assert!(run_macro(&mut d, &r, again).is_err());
    move_range(&mut d, &r, CUTOFF, p.min, None);
    let a = first(&d);
    assert_eq!(a.min, p.min);
    assert_eq!(a.max - a.min, 1000.0);
}

#[test]
fn move_range_follows_the_curve_and_the_direction() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let id = first(&d).id;
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetShape {
            id,
            curve: MacroCurve::Exp,
            smooth_ms: 0,
        },
    )
    .unwrap();
    run_macro(
        &mut d,
        &r,
        MacroCommand::SetRange {
            id,
            min: 400.0,
            max: 1400.0,
            invert: true,
            merge: None,
        },
    )
    .unwrap();
    set_macro(&mut d, &r, 0.5);
    let before = d.doc().driven_value(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(before, 650.0);
    move_range(&mut d, &r, CUTOFF, 800.0, None);
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(800.0));
    let a = first(&d);
    assert_eq!((a.curve, a.invert), (MacroCurve::Exp, true));
    assert_eq!((a.min, a.max), (550.0, 1550.0));
}

#[test]
fn move_range_on_a_discrete_param_lands_on_a_step() {
    let (mut d, r) = project();
    run_macro(
        &mut d,
        &r,
        MacroCommand::Assign {
            bank: None,
            index: 0,
            device: key("a"),
            param: WAVE.into(),
            min: Some(0.0),
            max: Some(2.0),
            invert: false,
        },
    )
    .unwrap();
    set_macro(&mut d, &r, 0.5);
    assert_eq!(d.doc().driven_value(&r, &key("a"), WAVE), Some(1.0));
    move_range(&mut d, &r, WAVE, 2.0, None);
    assert_eq!(d.doc().driven_value(&r, &key("a"), WAVE), Some(2.0));
    let a = first(&d);
    assert_eq!((a.min, a.max), (1.0, 3.0));
}

#[test]
fn move_range_refuses_what_a_macro_does_not_drive_and_a_bad_value() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    for (device, param, value) in [
        ("a", "filter.resonance", 0.5),
        ("b", CUTOFF, 900.0),
        ("a", CUTOFF, f32::NAN),
        ("a", CUTOFF, f32::INFINITY),
    ] {
        let c = MacroCommand::MoveRange {
            device: key(device),
            param: param.into(),
            value,
            merge: None,
        };
        assert!(
            run_macro(&mut d, &r, c).is_err(),
            "{device} {param} {value}"
        );
    }
}

#[test]
fn a_drag_of_move_range_is_one_undo_step() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    set_macro(&mut d, &r, 0.5);
    let steps = |d: &Document| {
        let mut probe = d.clone();
        let mut n = 0;
        while probe.undo().is_some() {
            n += 1;
        }
        n
    };
    let base = steps(&d);
    for v in [950.0, 1000.0, 1100.0, 1200.0] {
        move_range(&mut d, &r, CUTOFF, v, Some(7));
    }
    assert_eq!(steps(&d), base + 1);
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(1200.0));
    d.undo().unwrap();
    assert_eq!((first(&d).min, first(&d).max), (400.0, 1400.0));
    d.redo().unwrap();
    assert_eq!(d.doc().driven_value(&r, &key("a"), CUTOFF), Some(1200.0));
}

#[test]
fn a_drag_of_set_range_is_one_undo_step() {
    let (mut d, r) = project();
    assign(&mut d, &r, None, "a", CUTOFF);
    let id = first(&d).id;
    for max in [1500.0, 1600.0, 1700.0] {
        run_macro(
            &mut d,
            &r,
            MacroCommand::SetRange {
                id,
                min: 400.0,
                max,
                invert: false,
                merge: Some(9),
            },
        )
        .unwrap();
    }
    assert_eq!(first(&d).max, 1700.0);
    d.undo().unwrap();
    assert_eq!(first(&d).max, 1400.0);
}
