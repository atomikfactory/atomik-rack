//! Macro follow-ups, second round (model half): a bank drives params inside a container, and an
//! automation lane on a driven param sets the base the macro adds its range to.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, Command, ContainerCommand, Curve, DeviceKey, Document, MacroAssign,
    MacroCommand, ModelError, compose_driven_lane, expand_bank_lane, lane_value, load_project,
    save_project,
};
use common::song::{add_clip, add_track, auto_clip, point, sc};
use common::*;

const CUTOFF: &str = "filter.cutoff";
const MIX: &str = "mix";

fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
        ],
    );
    (d, r)
}

fn assign_cmd(
    bank: Option<DeviceKey>,
    index: u8,
    device: &str,
    param: &str,
    min: f32,
    max: f32,
) -> Command {
    Command::Macro(MacroCommand::Assign {
        bank,
        index,
        device: key(device),
        param: param.into(),
        min: Some(min),
        max: Some(max),
        invert: false,
    })
}

fn bank_key(d: &Document) -> DeviceKey {
    d.doc().banks().next().expect("a bank").key.clone()
}

fn move_macro(d: &mut Document, r: &Registry, value: f32) {
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

fn cutoff(r: &Registry) -> atomik_core::ParamDescriptor {
    *r.get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key(CUTOFF)
        .unwrap()
}

// ---------------------------------------------------------------- inside a container

#[test]
fn a_bank_drives_a_param_inside_a_container() {
    let (mut d, r) = project();
    d.apply(assign_cmd(None, 0, "c/fx", MIX, 0.2, 0.8), &r)
        .unwrap();
    move_macro(&mut d, &r, 0.5);
    let doc = d.doc();
    assert_eq!(doc.driven_value(&r, &key("c/fx"), MIX), Some(0.5));
    // The expansion sets the inner device, and lists the target.
    let flat = doc.expand(&r);
    let fx = flat.devices.iter().find(|x| x.key == key("c/fx")).unwrap();
    assert_eq!(fx.params.get(MIX), Some(&0.5));
    assert!(
        flat.macro_targets
            .iter()
            .any(|(bank, _, t)| bank == &bank_key(&d) && t[0].device == key("c/fx"))
    );
    // A direct edit is refused like any driven param, the views show the driven value.
    let e = d.apply(set_param("c/fx", MIX, 0.9), &r).unwrap_err();
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    let view = d.doc().view_with_driven_values(&r).expect("a view");
    assert_eq!(
        view.resolve(&key("c/fx")).unwrap().params.get(MIX),
        Some(&0.5)
    );
    // Removing the assignment leaves the value, and undo brings the assignment back.
    let id = d.doc().macros.assigns[0].id;
    d.apply(Command::Macro(MacroCommand::Remove { id }), &r)
        .unwrap();
    assert_eq!(
        d.doc().resolve(&key("c/fx")).unwrap().params.get(MIX),
        Some(&0.5)
    );
    d.undo().unwrap();
    assert!(d.doc().bank_driver(&key("c/fx"), MIX).is_some());
}

#[test]
fn a_range_edit_and_a_direct_edit_work_on_an_inner_target() {
    let (mut d, r) = project();
    d.apply(assign_cmd(None, 0, "c/fx", MIX, 0.2, 0.8), &r)
        .unwrap();
    move_macro(&mut d, &r, 0.5);
    d.apply(
        Command::Macro(MacroCommand::MoveRange {
            device: key("c/fx"),
            param: MIX.into(),
            value: 0.7,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let a = d.doc().macros.assigns[0].clone();
    assert!(
        (a.min - 0.4).abs() < 1e-6 && (a.max - 1.0).abs() < 1e-6,
        "{a:?}"
    );
    let v = d.doc().driven_value(&r, &key("c/fx"), MIX).unwrap();
    assert!((v - 0.7).abs() < 1e-6, "{v}");
}

#[test]
fn a_container_macro_and_a_bank_never_drive_the_same_param() {
    let (mut d, r) = project();
    // A bank first: the container macro that wants the param is refused.
    d.apply(assign_cmd(None, 0, "c/fx", MIX, 0.0, 1.0), &r)
        .unwrap();
    let e = d
        .apply(set_macro("c", 0, target("fx", MIX, 0.0, 1.0)), &r)
        .unwrap_err();
    assert!(e.to_string().contains("already driven"), "{e}");
    // A container macro first: the bank is refused.
    let (mut d, r) = project();
    d.apply(set_macro("c", 0, target("fx", MIX, 0.0, 1.0)), &r)
        .unwrap();
    let e = d
        .apply(assign_cmd(None, 0, "c/fx", MIX, 0.0, 1.0), &r)
        .unwrap_err();
    assert!(e.to_string().contains("driven by macro 1 of c"), "{e}");
}

#[test]
fn a_device_with_driven_params_stays_where_its_key_is() {
    let (mut d, r) = project();
    d.apply(assign_cmd(None, 0, "c/fx", MIX, 0.0, 1.0), &r)
        .unwrap();
    let out = Command::Container(ContainerCommand::MoveOutOfContainer {
        key: key("c/fx"),
        index: 0,
    });
    let e = d.apply(out, &r).unwrap_err();
    assert!(e.to_string().contains("driven by"), "{e}");
    let un = Command::Container(ContainerCommand::Uncombine {
        container: key("c"),
    });
    let e = d.apply(un, &r).unwrap_err();
    assert!(e.to_string().contains("driven by"), "{e}");
    // A top-level device that is driven cannot enter a container either.
    d.apply(assign_cmd(None, 1, "a", CUTOFF, 400.0, 1400.0), &r)
        .unwrap();
    let into = Command::Container(ContainerCommand::MoveIntoContainer {
        key: key("a"),
        container: key("c"),
        index: 0,
    });
    assert!(d.apply(into, &r).is_err());
}

#[test]
fn an_assignment_to_an_inner_device_survives_a_save_and_a_load() {
    let (mut d, r) = project();
    d.apply(assign_cmd(None, 0, "c/fx", MIX, 0.2, 0.8), &r)
        .unwrap();
    move_macro(&mut d, &r, 0.25);
    let text = save_project(d.doc());
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(loaded.macros, d.doc().macros);
    assert_eq!(
        loaded.driven_value(&r, &key("c/fx"), MIX),
        d.doc().driven_value(&r, &key("c/fx"), MIX)
    );
}

// ---------------------------------------------------------------- a lane on a driven param

fn lane_project(points: Vec<AutomationPoint>) -> (Document, Registry) {
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
            add_clip("a", 1, auto_clip(0, 3840, points)),
        ],
    );
    d.apply(assign_cmd(None, 0, "a", CUTOFF, 20.0, 1020.0), &r)
        .unwrap();
    (d, r)
}

fn assign_of(d: &Document) -> MacroAssign {
    d.doc().macros.assigns[0].clone()
}

#[test]
fn the_lane_sets_the_base_and_the_macro_adds_its_range() {
    let (mut d, r) = lane_project(vec![
        point(0, 400.0, Curve::Linear),
        point(1920, 800.0, Curve::Step),
    ]);
    move_macro(&mut d, &r, 0.5);
    let base = d.doc().own_lane_points(&key("a"), CUTOFF).unwrap();
    let played = d.doc().driven_lane_points(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(played.len(), base.len());
    for (b, p) in base.iter().zip(&played) {
        assert_eq!((b.0, b.2), (p.0, p.2));
        assert_eq!(p.1, b.1 + 500.0, "the macro at 0.5 adds half of 1000");
    }
    // At macro 0 the lane sounds as drawn.
    move_macro(&mut d, &r, 0.0);
    let played = d.doc().driven_lane_points(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(played[0].1, base[0].1);
}

#[test]
fn the_sum_is_clamped_to_the_param_range_and_an_inverted_macro_adds_at_zero() {
    let p = cutoff(&console_registry());
    let (mut d, r) = lane_project(vec![point(0, p.max - 10.0, Curve::Step)]);
    move_macro(&mut d, &r, 1.0);
    let played = d.doc().driven_lane_points(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(played[0].1, p.max);
    let id = assign_of(&d).id;
    d.apply(
        Command::Macro(MacroCommand::SetRange {
            id,
            min: 20.0,
            max: 1020.0,
            invert: true,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let played = d.doc().driven_lane_points(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(played[0].1, p.max - 10.0, "inverted: macro 1 adds nothing");
}

#[test]
fn a_param_without_points_has_no_composed_lane_and_the_macro_lane_folds_into_one() {
    // No clips on the lane: the macro plays as before, its own lane expands to the target.
    let (mut d, r) = project();
    d.apply(assign_cmd(None, 0, "a", CUTOFF, 400.0, 1400.0), &r)
        .unwrap();
    assert_eq!(d.doc().driven_lane_points(&r, &key("a"), CUTOFF), None);
    let bank = bank_key(&d);
    let line = [AutomationPoint {
        tick: 0,
        value: 0.5,
        curve: Curve::Step,
    }];
    let out = expand_bank_lane(d.doc(), &r, &bank, "macro1", &line).unwrap();
    assert_eq!(out.len(), 1);
    // With points on the target's own lane the macro's lane is folded into it instead.
    let (mut d, r) = lane_project(vec![point(0, 400.0, Curve::Step)]);
    let bank = bank_key(&d);
    let out = expand_bank_lane(d.doc(), &r, &bank, "macro1", &line).unwrap();
    assert!(out.is_empty());
    run(
        &mut d,
        &r,
        [
            add_track(bank.as_str()),
            sc(atomik_model::SongCommand::AddAutomationLane {
                device: bank.clone(),
                param: "macro1".into(),
            }),
            add_clip(
                bank.as_str(),
                1,
                auto_clip(0, 3840, vec![point(0, 1.0, Curve::Step)]),
            ),
        ],
    );
    let played = d.doc().driven_lane_points(&r, &key("a"), CUTOFF).unwrap();
    assert_eq!(
        played[0].1, 1400.0,
        "400 from the lane plus 1000 from the macro lane"
    );
}

#[test]
fn two_moving_lanes_are_joined_at_the_points_of_either() {
    let (d, r) = lane_project(vec![point(0, 0.0, Curve::Linear)]);
    let a = assign_of(&d);
    let p = cutoff(&r);
    let base = [(0, 400.0, Curve::Linear), (960, 800.0, Curve::Step)];
    let m = [(0, 0.0, Curve::Linear), (480, 1.0, Curve::Step)];
    let out = compose_driven_lane(&a, &p, &base, 0.0, &m);
    let ticks: Vec<i64> = out.iter().map(|x| x.0).collect();
    assert_eq!(ticks[..3], [0, 480, 960]);
    // Straight between the points: at 480 the base is 600 and the macro adds 1000.
    assert_eq!(out[1].1, lane_value(&a, 600.0, 1.0, &p));
    // The lane stays on its last values after the end.
    assert_eq!(out.last().unwrap().2, Curve::Step);
}

#[test]
fn a_jump_stays_a_jump() {
    let (d, r) = lane_project(vec![point(0, 0.0, Curve::Linear)]);
    let a = assign_of(&d);
    let p = cutoff(&r);
    // The macro ramps for a bar; the base holds 400, then jumps to 2000 at tick 960.
    let base = [(0, 400.0, Curve::Step), (960, 2000.0, Curve::Step)];
    let m = [(0, 0.0, Curve::Linear), (1920, 1.0, Curve::Step)];
    let out = compose_driven_lane(&a, &p, &base, 0.0, &m);
    let before = out
        .iter()
        .find(|x| x.0 == 959)
        .expect("a point before the jump");
    assert_eq!(before.2, Curve::Step);
    let at = out.iter().find(|x| x.0 == 960).unwrap();
    assert!(at.1 > before.1, "the lane jumps up at 960");
}

#[test]
fn a_bent_segment_is_followed_by_straight_pieces() {
    let (d, r) = lane_project(vec![point(0, 0.0, Curve::Linear)]);
    let a = assign_of(&d);
    let p = cutoff(&r);
    let base = [(0, 400.0, Curve::Bend(40)), (960, 4000.0, Curve::Step)];
    let m = [(0, 0.0, Curve::Step), (960, 0.0, Curve::Step)];
    let out = compose_driven_lane(&a, &p, &base, 0.0, &m);
    assert!(out.len() >= atomik_model::BEND_PARTS as usize);
    // Every cut sits on the bent curve (the macro adds nothing here).
    for x in &out[..out.len() - 1] {
        let want = Curve::Bend(40).blend(400.0, 4000.0, x.0 as f64 / 960.0) as f32;
        assert!((x.1 - want).abs() < 1.0, "{x:?} vs {want}");
    }
}
