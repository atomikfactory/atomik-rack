//! The rack view's side of Combine, Uncombine and the moves in and out of containers: the
//! reasons the menus show, the drag of a device onto a container's panel and onto the
//! breadcrumb, and the container's BYPASS switch. Goldens `container_bypassed_front_1x.json`
//! and `container_drop_target_1x.json`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_combine`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ContainerCommand, DeviceKey, Document, RackDoc};
use atomik_ui::breadcrumb::CrumbHit;
use atomik_ui::kurbo::Point;
use atomik_ui::{RackView, UiAction};
use support::container::*;
use support::*;

const REVERB: &str = "atomik.reverb";

/// [`layered`] with a loose effect `tail` between `lead` and `layer`.
fn with_tail() -> Document {
    let mut d = layered();
    apply(
        &mut d,
        Command::AddDevice {
            index: 1,
            type_id: REVERB.into(),
            key: Some(key("tail")),
        },
    );
    d
}

fn drag(v: &mut RackView, doc: &RackDoc, reg: &Registry, from: Point, to: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(from), doc, reg);
    out.extend(v.handle(&mv(to), doc, reg));
    out.extend(v.handle(&up(to), doc, reg));
    out
}

/// The view point `dy` below the top of the panel of `k`.
fn on_panel(v: &RackView, k: &str, dy: f64) -> Point {
    let r = v.device_rect(&key(k)).expect("a panel");
    v.rack_to_view(Point::new(r.x0 + 600.0, r.y0 + dy))
}

fn select(v: &mut RackView, keys: &[&str]) {
    let keys: Vec<DeviceKey> = keys.iter().map(|k| key(k)).collect();
    v.set_selection(&keys, keys.last());
}

/// The device keys of the rack without the console's own devices.
fn keys_of(doc: &RackDoc) -> Vec<String> {
    doc.devices
        .iter()
        .map(|d| d.key.0.clone())
        .filter(|k| k != "channel" && k != "master")
        .collect()
}

// ------------------------------------------------------------------ reasons

#[test]
fn the_combine_reason_is_the_models_or_none() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    assert!(
        v.combine_reason(d.doc(), &reg)
            .is_some_and(|r| r.contains("Select")),
        "nothing selected"
    );
    select(&mut v, &["tail"]);
    assert_eq!(v.combine_reason(d.doc(), &reg), None);
    select(&mut v, &["tail", "layer"]);
    assert_eq!(v.combine_reason(d.doc(), &reg), None, "a container nests");
    let v = scoped_view(d.doc(), &["layer"]);
    assert!(
        v.combine_reason(d.doc(), &reg)
            .is_some_and(|r| r.contains("top level")),
        "inside a container"
    );
}

#[test]
fn the_combine_reason_names_what_the_model_refuses() {
    // The master section is not a device a container can take.
    let d = layered();
    let reg = registry();
    let mut v = view(d.doc(), &reg);
    let master = d.doc().console.master.clone().expect("a master");
    v.set_selection(std::slice::from_ref(&master), Some(&master));
    let reason = v.combine_reason(d.doc(), &reg).expect("refused");
    assert!(!reason.is_empty(), "{reason}");
    let before = d.doc().clone();
    assert!(
        Document::new(before.clone())
            .apply(
                Command::Container(ContainerCommand::Combine {
                    keys: vec![master],
                    name: String::new(),
                }),
                &reg
            )
            .is_err()
    );
    assert_eq!(&before, d.doc(), "the check left the document alone");
}

#[test]
fn the_uncombine_reason_wants_a_container() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    assert!(v.uncombine_reason(d.doc(), &reg).is_some());
    select(&mut v, &["tail"]);
    assert!(
        v.uncombine_reason(d.doc(), &reg)
            .is_some_and(|r| r.contains("container")),
        "an effect is not a container"
    );
    select(&mut v, &["layer"]);
    assert_eq!(v.uncombine_reason(d.doc(), &reg), None);
    let v = scoped_view(d.doc(), &["layer"]);
    assert!(v.uncombine_reason(d.doc(), &reg).is_some());
}

#[test]
fn the_move_out_reason_needs_one_inner_device() {
    let d = layered();
    let reg = registry();
    let v = plain_view(d.doc(), &reg);
    assert!(
        v.move_out_reason(d.doc(), &reg)
            .is_some_and(|r| r.contains("Open")),
        "the top level has nothing to move out of"
    );
    let mut v = scoped_view(d.doc(), &["layer"]);
    assert!(
        v.move_out_reason(d.doc(), &reg).is_some(),
        "nothing selected"
    );
    select(&mut v, &["reverb"]);
    assert_eq!(v.move_out_reason(d.doc(), &reg), None);
    select(&mut v, &["reverb", "delay"]);
    assert!(
        v.move_out_reason(d.doc(), &reg)
            .is_some_and(|r| r.contains("one at a time"))
    );
}

// ------------------------------------------------------------------ the drag in

#[test]
fn dropping_a_device_on_a_containers_panel_moves_it_inside() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let from = v.plate_center(&key("tail")).unwrap();
    let to = on_panel(&v, "layer", 60.0);
    let a = drag(&mut v, d.doc(), &reg, from, to);
    let want = Command::Container(ContainerCommand::MoveIntoContainer {
        key: key("tail"),
        container: key("layer"),
        index: usize::MAX,
    });
    assert_eq!(commands(&a), vec![want.clone()]);
    let mut d = d;
    d.apply(want, &reg).expect("the model takes it");
    assert_eq!(keys_of(d.doc()), ["lead", "layer"]);
    assert!(d.doc().inner(&key("layer")).unwrap().devices.len() == 5);
}

#[test]
fn the_edge_of_a_container_places_the_device_next_to_it() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let from = v.plate_center(&key("lead")).unwrap();
    // The top few units of the panel are still the place above it.
    let to = on_panel(&v, "layer", 3.0);
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
}

#[test]
fn a_group_over_a_container_is_placed_not_moved_inside() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    select(&mut v, &["lead", "tail"]);
    let from = v.plate_center(&key("tail")).unwrap();
    let to = on_panel(&v, "layer", 60.0);
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
}

#[test]
fn a_container_is_not_its_own_target() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let from = v.plate_center(&key("layer")).unwrap();
    let to = on_panel(&v, "layer", 60.0);
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
}

// ------------------------------------------------------------------ the drag out

#[test]
fn dropping_an_inner_device_on_the_project_crumb_moves_it_out_after_the_container() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let from = v.plate_center(&key("reverb")).unwrap();
    let to = v.breadcrumb_center(CrumbHit::Segment(0)).unwrap();
    let a = drag(&mut v, d.doc(), &reg, from, to);
    let want = Command::Container(ContainerCommand::MoveOutOfContainer {
        key: key("layer/reverb"),
        index: 2,
    });
    assert_eq!(commands(&a), vec![want.clone()]);
    assert!(
        a.iter()
            .all(|x| !matches!(x, UiAction::ExitContainer { .. })),
        "the drop does not leave the container"
    );
    let mut d = d;
    d.apply(want, &reg).expect("the model takes it");
    assert_eq!(keys_of(d.doc()), ["lead", "layer", "reverb"]);
}

#[test]
fn the_back_button_takes_a_dropped_device_out_too() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let from = v.plate_center(&key("reverb")).unwrap();
    let to = v.breadcrumb_center(CrumbHit::Back).unwrap();
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::Container(
            ContainerCommand::MoveOutOfContainer { .. }
        )]
    ));
}

#[test]
fn a_nested_container_gives_nothing_back_by_the_crumb() {
    let d = nested();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer", "fx"]);
    let from = v.plate_center(&key("deep")).unwrap();
    let to = v.breadcrumb_center(CrumbHit::Segment(0)).unwrap();
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
}

#[test]
fn a_group_dropped_on_the_crumb_does_not_move_out() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    select(&mut v, &["reverb", "delay"]);
    let from = v.plate_center(&key("reverb")).unwrap();
    let to = v.breadcrumb_center(CrumbHit::Segment(0)).unwrap();
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
}

// ------------------------------------------------------------------ bypass

#[test]
fn the_container_has_a_bypass_switch_that_sets_the_param() {
    let d = layered();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    assert!(
        v.bypass_button_rect(&key("lead")).is_none(),
        "an instrument has none"
    );
    let at = v
        .bypass_button_rect(&key("layer"))
        .expect("a switch")
        .center();
    let a = v.handle(&down(at), d.doc(), &reg);
    let _ = v.handle(&up(at), d.doc(), &reg);
    let cmds = commands(&a);
    let [
        Command::SetParam {
            key: k,
            param,
            value,
            ..
        },
    ] = cmds.as_slice()
    else {
        panic!("{a:?}");
    };
    assert_eq!((k, param.as_str(), *value), (&key("layer"), "bypass", 1.0));
    let mut d = d;
    d.apply(cmds[0].clone(), &reg).unwrap();
    assert!(d.doc().container_bypassed(&key("layer")));
    // Switched back in.
    let mut v = plain_view(d.doc(), &reg);
    let at = v.bypass_button_rect(&key("layer")).unwrap().center();
    let a = v.handle(&down(at), d.doc(), &reg);
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::SetParam { value, .. }] if *value == 0.0
    ));
}

#[test]
fn an_inner_container_has_the_switch_with_a_path_key() {
    let d = nested();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let at = v.bypass_button_rect(&key("fx")).expect("a switch").center();
    let a = v.handle(&down(at), d.doc(), &reg);
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::SetParam { key: k, param, .. }] if *k == key("layer/fx") && param == "bypass"
    ));
}

// ------------------------------------------------------------------ goldens

#[test]
fn a_bypassed_container_matches_golden() {
    let mut d = layered();
    apply(
        &mut d,
        Command::SetParam {
            key: key("layer"),
            param: "bypass".into(),
            value: 1.0,
            merge: None,
        },
    );
    let mut v = plain_view(d.doc(), &registry());
    check(
        "container_bypassed_front_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_drop_target_outline_matches_golden() {
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let from = v.plate_center(&key("tail")).unwrap();
    let to = on_panel(&v, "layer", 60.0);
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    check(
        "container_drop_target_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

// ------------------------------------------------------------------ refused moves are not offered

#[test]
fn a_device_with_a_track_dropped_on_a_container_is_placed_not_moved_inside() {
    // `lead` has a song track and a console channel: the model would refuse it, so the panel is
    // no target for it (no command, no outline) and the drop places it as any other move.
    let d = with_tail();
    let reg = registry();
    let mut v = plain_view(d.doc(), &reg);
    let from = v.plate_center(&key("lead")).unwrap();
    let to = on_panel(&v, "layer", 60.0);
    let a = drag(&mut v, d.doc(), &reg, from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
    // Held over the panel it draws the ordinary drop line, not the container's outline.
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    let held = paint_rack(&mut v, d.doc()).to_json();
    assert!(!held.contains("Move into container"));
}

#[test]
fn every_move_a_drop_emits_is_one_the_model_takes() {
    let d = with_tail();
    let reg = registry();
    for from_key in ["lead", "tail", "layer"] {
        let mut v = plain_view(d.doc(), &reg);
        let from = v.plate_center(&key(from_key)).unwrap();
        let to = on_panel(&v, "layer", 60.0);
        let a = drag(&mut v, d.doc(), &reg, from, to);
        for c in commands(&a) {
            Document::new(d.doc().clone())
                .apply(c.clone(), &reg)
                .unwrap_or_else(|e| panic!("{from_key}: {c:?} refused: {e}"));
        }
    }
}
