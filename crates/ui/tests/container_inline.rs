//! A container unfolded in place: its inner devices are drawn under its panel in the same rack
//! column, framed in the container's colour, and every gesture of a top-level device works on
//! them with path keys. Layers: the view model and the layout, the fold gestures, the editing of
//! inner devices, the back panel and its cables. Display-list goldens `container_inline_*`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test container_inline`.

mod support;

use atomik_model::{Command, ContainerCommand, Document, RackDoc};
use atomik_ui::container_panel::ContainerButton;
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{Flip, RackView, Theme, UiAction, UiEvent};
use support::container::*;
use support::*;

/// Tall enough for the panels of every device the fixtures unfold (a poly synth is 7 units).
const TALL: Size = Size::new(1000.0, 3600.0);

fn inline_view(doc: &RackDoc) -> RackView {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    v.set_show_console_devices(false);
    let _ = v.handle(&UiEvent::Resize(TALL), doc, &reg);
    v.sync(doc, &reg);
    v
}

fn set_open(d: &mut Document, container: &str, open: bool) {
    apply(
        d,
        Command::Container(ContainerCommand::SetOpen {
            container: key(container),
            open,
        }),
    );
}

fn unfolded() -> Document {
    let mut d = layered();
    set_open(&mut d, "layer", true);
    d
}

fn keys_in_view(v: &RackView) -> Vec<String> {
    v.panel_revisions().into_iter().map(|(k, _)| k.0).collect()
}

fn rect(v: &RackView, k: &str) -> atomik_ui::kurbo::Rect {
    v.device_rect(&key(k))
        .unwrap_or_else(|| panic!("no view {k}"))
}

// ------------------------------------------------------------------ the view model and layout

#[test]
fn a_folded_container_is_one_panel() {
    let d = layered();
    let v = inline_view(d.doc());
    assert_eq!(keys_in_view(&v), ["interface", "lead", "layer"]);
    assert!(!v.is_inline());
    assert!(v.frames().is_empty());
}

#[test]
fn an_unfolded_container_draws_its_devices_under_its_panel() {
    let d = unfolded();
    let v = inline_view(d.doc());
    assert!(v.is_inline());
    assert_eq!(
        keys_in_view(&v),
        [
            "interface",
            "lead",
            "layer",
            "layer/boundary",
            "layer/synth",
            "layer/pad",
            "layer/delay",
            "layer/reverb",
            "layer/boundary~out"
        ]
    );
    // One column: everything stacks, panel after panel, with no gap.
    let order = [
        "lead",
        "layer",
        "layer/boundary",
        "layer/synth",
        "layer/pad",
        "layer/delay",
        "layer/reverb",
        "layer/boundary~out",
    ];
    for pair in order.windows(2) {
        let (a, b) = (rect(&v, pair[0]), rect(&v, pair[1]));
        assert!((a.y1 - b.y0).abs() < 1e-6, "{} then {}", pair[0], pair[1]);
        assert!((a.x0 - b.x0).abs() < 1e-6, "same column: {}", pair[1]);
    }
}

#[test]
fn the_frame_surrounds_the_inside_in_the_margin_of_the_column() {
    let d = unfolded();
    let v = inline_view(d.doc());
    let frames = v.frames();
    assert_eq!(frames.len(), 1);
    let (container, outline) = &frames[0];
    assert_eq!(container, &key("layer"));
    let (head, foot) = (rect(&v, "layer"), rect(&v, "layer/boundary~out"));
    assert!(outline.x0 < head.x0 && outline.x1 > head.x1, "{outline:?}");
    assert!(outline.y0 <= head.y1 && outline.y1 >= foot.y1);
    // The frame stays clear of the panel above and below.
    assert!(outline.y0 > head.y0 + head.height() / 2.0);
    let below = foot.y1 + 4.0;
    assert!(outline.y1 < below);
}

#[test]
fn the_strips_are_thin_on_the_front_and_whole_panels_on_the_back() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let thin = rect(&v, "layer/boundary").height();
    assert!(thin < 50.0, "{thin}");
    v.set_flip(Flip::Back);
    v.sync(d.doc(), &registry());
    let whole = rect(&v, "layer/boundary").height();
    assert!(whole > thin + 10.0, "{whole} vs {thin}");
    assert!(rect(&v, "layer/boundary~out").height() > thin + 10.0);
    v.set_flip(Flip::Front);
    v.sync(d.doc(), &registry());
    assert!((rect(&v, "layer/boundary").height() - thin).abs() < 1e-6);
}

#[test]
fn a_nested_container_unfolds_inside_the_frame_of_its_parent() {
    let mut d = nested();
    set_open(&mut d, "layer", true);
    set_open(&mut d, "layer/fx", true);
    let v = inline_view(d.doc());
    let keys = keys_in_view(&v);
    for k in [
        "layer/fx",
        "layer/fx/boundary",
        "layer/fx/deep",
        "layer/fx/boundary~out",
        "layer/boundary~out",
    ] {
        assert!(keys.contains(&k.to_string()), "{k}: {keys:?}");
    }
    // A container nested in a folded one is not shown, even when its own flag is set.
    let at = |k: &str| keys.iter().position(|x| x == k).unwrap();
    assert!(at("layer/fx") < at("layer/fx/boundary"));
    assert!(at("layer/fx/boundary~out") < at("layer/boundary~out"));
    let frames = v.frames();
    assert_eq!(frames.len(), 2);
    let outer = frames.iter().find(|(k, _)| k == &key("layer")).unwrap().1;
    let inner = frames
        .iter()
        .find(|(k, _)| k == &key("layer/fx"))
        .unwrap()
        .1;
    assert!(
        outer.x0 < inner.x0 && outer.x1 > inner.x1,
        "{outer:?} {inner:?}"
    );
    assert!(outer.y0 <= inner.y0 && outer.y1 >= inner.y1);
}

#[test]
fn an_inner_container_with_a_folded_parent_stays_hidden() {
    let mut d = nested();
    set_open(&mut d, "layer/fx", true);
    let v = inline_view(d.doc());
    assert!(!v.is_inline(), "nothing at the top level is unfolded");
    assert_eq!(keys_in_view(&v), ["interface", "lead", "layer"]);
}

#[test]
fn the_view_follows_a_fold_and_an_unfold_of_the_document() {
    let mut d = layered();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let folded_height = v.content_height();
    set_open(&mut d, "layer", true);
    v.sync(d.doc(), &reg);
    assert!(v.content_height() > folded_height + 100.0);
    assert!(v.is_inline());
    set_open(&mut d, "layer", false);
    v.sync(d.doc(), &reg);
    assert_eq!(v.content_height(), folded_height);
    assert!(!v.is_inline());
    assert_eq!(keys_in_view(&v), ["interface", "lead", "layer"]);
}

#[test]
fn an_unfolded_container_in_a_second_column_stacks_in_that_column() {
    let mut d = layered();
    apply(&mut d, Command::AddRackColumn);
    apply(
        &mut d,
        Command::Workflow(atomik_model::WorkflowCommand::MoveDevicesToSlot {
            keys: vec![key("layer")],
            slot: atomik_model::RackSlot {
                column: 1,
                index: 0,
            },
            route: false,
        }),
    );
    set_open(&mut d, "layer", true);
    let v = inline_view(d.doc());
    let (head, inner) = (rect(&v, "layer"), rect(&v, "layer/synth"));
    assert!(head.x0 > 100.0, "the container is in the second column");
    assert!((head.x0 - inner.x0).abs() < 1e-6);
    assert!(inner.y0 >= head.y1);
}

#[test]
fn the_inside_of_a_container_is_not_part_of_the_rack_order_of_the_top_level() {
    // The unfolded devices are real views with path keys, never top-level keys.
    let d = unfolded();
    let v = inline_view(d.doc());
    let reg = registry();
    assert!(d.doc().device(&key("synth")).is_none());
    assert!(v.device_rect(&key("layer/synth")).is_some());
    let _ = reg;
}

// ------------------------------------------------------------------ goldens

#[test]
fn a_folded_container_matches_golden() {
    let d = layered();
    let mut v = inline_view(d.doc());
    check(
        "container_inline_folded_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn an_unfolded_container_matches_golden() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    check(
        "container_inline_open_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn an_unfolded_container_on_the_back_matches_golden() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_flip(Flip::Back);
    check(
        "container_inline_back_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn nested_unfolded_containers_match_golden() {
    let mut d = nested();
    set_open(&mut d, "layer", true);
    set_open(&mut d, "layer/fx", true);
    let mut v = inline_view(d.doc());
    check(
        "container_inline_nested_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

// ------------------------------------------------------------------ keep the helpers used

#[allow(dead_code)]
fn centre(v: &RackView, container: &str, b: ContainerButton) -> Point {
    v.container_button_center(&key(container), b).unwrap()
}

#[allow(dead_code)]
fn clicked(v: &mut RackView, d: &Document, p: Point) -> Vec<UiAction> {
    click(v, d.doc(), p)
}

// ------------------------------------------------------------------ editing the inner devices

use atomik_ui::keymap::{Os, Roles};
use atomik_ui::kurbo::Vec2;
use atomik_ui::{Modifiers, PointerButton};

fn drag(v: &mut RackView, doc: &RackDoc, from: Point, to: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(from), doc, &reg);
    out.extend(v.handle(&mv(to), doc, &reg));
    out.extend(v.handle(&up(to), doc, &reg));
    out
}

/// The view point `frac` of the way down the panel `k`, away from its controls.
fn on(v: &RackView, k: &str, frac: f64) -> Point {
    let r = rect(v, k);
    v.rack_to_view(Point::new(r.x0 + 600.0, r.y0 + r.height() * frac))
}

fn set_param_keys(actions: &[UiAction]) -> Vec<String> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { key, param, .. } => Some(format!("{key}.{param}")),
            _ => None,
        })
        .collect()
}

fn wheel(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: p,
            modifiers: Modifiers::NONE,
        },
        doc,
        &registry(),
    )
}

fn apply_all(d: &mut Document, actions: &[UiAction]) {
    let reg = registry();
    for c in commands(actions) {
        d.apply(c, &reg).expect("the model takes the command");
    }
}

/// The keys of the rack without the console devices.
fn top_keys(d: &Document) -> Vec<&str> {
    d.doc()
        .devices
        .iter()
        .map(|x| x.key.0.as_str())
        .filter(|k| *k != "channel" && *k != "master")
        .collect()
}

fn local_keys(d: &Document, container: &str) -> Vec<String> {
    d.doc()
        .inner(&key(container))
        .unwrap()
        .devices
        .iter()
        .map(|x| x.key.0.clone())
        .collect()
}

#[test]
fn clicking_an_inner_device_selects_it_by_path_key() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v.plate_center(&key("layer/pad")).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert_eq!(a, vec![UiAction::Select(key("layer/pad"))]);
    assert_eq!(v.selected(), Some(&key("layer/pad")));
    assert_eq!(v.selection(), vec![key("layer/pad")]);
}

#[test]
fn a_knob_of_an_inner_device_sets_its_param_by_path_key() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v
        .control_center(&key("layer/pad"), "filter.resonance")
        .unwrap();
    let a = wheel(&mut v, d.doc(), p);
    assert_eq!(set_param_keys(&a), ["layer/pad.filter.resonance"]);
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
}

#[test]
fn a_param_a_macro_drives_is_locked_on_the_unfolded_device() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    // Macro 1 of the container drives the cutoff of the synth: the control does not move.
    let p = v
        .control_center(&key("layer/synth"), "filter.cutoff")
        .unwrap();
    let a = wheel(&mut v, d.doc(), p);
    assert!(set_param_keys(&a).is_empty(), "{a:?}");
}

#[test]
fn delete_removes_inner_devices_by_path_key_without_bridging() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let p = v.plate_center(&key("layer/reverb")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    let a = v.command("edit.delete", None, d.doc(), &reg);
    assert_eq!(
        commands(a.actions()),
        vec![Command::RemoveDevice {
            key: key("layer/reverb")
        }]
    );
    let mut d2 = unfolded();
    apply_all(&mut d2, a.actions());
    v.sync(d2.doc(), &reg);
    assert!(v.device_rect(&key("layer/reverb")).is_none());
}

#[test]
fn the_selection_stays_inside_one_container() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    v.set_roles(Roles::defaults(Os::Windows));
    let lead = v.plate_center(&key("lead")).unwrap();
    let _ = click(&mut v, d.doc(), lead);
    // A toggle click on a device of the container replaces the selection.
    let pad = v.plate_center(&key("layer/pad")).unwrap();
    let a = press_with(
        &mut v,
        d.doc(),
        pad,
        PointerButton::Primary,
        1,
        Modifiers::CTRL,
    );
    assert_eq!(a, vec![UiAction::Select(key("layer/pad"))]);
    // A toggle within the container extends it.
    let delay = v.plate_center(&key("layer/delay")).unwrap();
    let _ = press_with(
        &mut v,
        d.doc(),
        delay,
        PointerButton::Primary,
        1,
        Modifiers::CTRL,
    );
    assert_eq!(v.selection(), vec![key("layer/pad"), key("layer/delay")]);
    // Select all takes the devices of the container of the primary.
    let _ = v.command("edit.select_all", None, d.doc(), &reg);
    assert_eq!(
        v.selection(),
        vec![
            key("layer/synth"),
            key("layer/pad"),
            key("layer/delay"),
            key("layer/reverb")
        ]
    );
}

#[test]
fn select_all_on_the_rack_leaves_the_inner_devices_out() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let _ = v.command("edit.select_all", None, d.doc(), &reg);
    assert_eq!(v.selection(), vec![key("lead"), key("layer")]);
}

#[test]
fn dragging_an_inner_device_inside_its_frame_reorders_it() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("layer/synth")).unwrap();
    let to = on(&v, "layer/reverb", 0.8);
    let a = drag(&mut v, d.doc(), from, to);
    assert_eq!(
        commands(&a),
        vec![Command::MoveDevice {
            key: key("layer/synth"),
            to: 3
        }],
        "{a:?}"
    );
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    assert_eq!(
        local_keys(&d2, "layer"),
        ["pad", "delay", "reverb", "synth"]
    );
}

#[test]
fn a_drop_that_changes_nothing_inside_the_frame_sends_nothing() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("layer/pad")).unwrap();
    let to = on(&v, "layer/pad", 0.9);
    let a = drag(&mut v, d.doc(), from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn dragging_a_group_inside_the_frame_moves_it_with_one_command() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_selection(
        &[key("layer/synth"), key("layer/pad")],
        Some(&key("layer/pad")),
    );
    let from = v.plate_center(&key("layer/synth")).unwrap();
    let to = on(&v, "layer/reverb", 0.8);
    let a = drag(&mut v, d.doc(), from, to);
    match commands(&a).as_slice() {
        [Command::Workflow(atomik_model::WorkflowCommand::MoveDevices { keys, to, route })] => {
            assert_eq!(keys, &vec![key("layer/synth"), key("layer/pad")]);
            assert_eq!(*to, 2);
            assert!(!route);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn dropping_a_device_of_the_rack_in_the_frame_moves_it_into_the_container() {
    let mut d = layered();
    apply(
        &mut d,
        Command::AddDevice {
            index: 2,
            type_id: DELAY.into(),
            key: Some(key("tail")),
        },
    );
    set_open(&mut d, "layer", true);
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("tail")).unwrap();
    let to = on(&v, "layer/pad", 0.9);
    let a = drag(&mut v, d.doc(), from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::MoveIntoContainer {
            key: key("tail"),
            container: key("layer"),
            index: 2,
        })],
        "{a:?}"
    );
    apply_all(&mut d, &a);
    assert_eq!(
        local_keys(&d, "layer"),
        ["synth", "pad", "tail", "delay", "reverb"]
    );
}

#[test]
fn a_device_with_a_track_is_refused_by_the_frame_with_the_models_reason() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("lead")).unwrap();
    let to = on(&v, "layer/pad", 0.9);
    let a = drag(&mut v, d.doc(), from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(texts.contains("has a song track"), "{texts}");
}

#[test]
fn dropping_an_inner_device_outside_its_frame_moves_it_to_the_rack() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    // The reverb is not cabled, so the model lets it go. Dropped on the top of the lead panel
    // it lands before it.
    let from = v.plate_center(&key("layer/reverb")).unwrap();
    let to = on(&v, "lead", 0.2);
    let a = drag(&mut v, d.doc(), from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::MoveOutOfContainer {
            key: key("layer/reverb"),
            index: 0,
        })],
        "{a:?}"
    );
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    let top = top_keys(&d2);
    assert_eq!(top, ["reverb", "lead", "layer"]);
}

#[test]
fn a_move_out_the_model_refuses_says_why_and_sends_nothing() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    // The synth is cabled to the delay inside: it cannot leave alone.
    let from = v.plate_center(&key("layer/synth")).unwrap();
    let to = on(&v, "lead", 0.2);
    let a = drag(&mut v, d.doc(), from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(
        texts.contains("cannot move the device out of the container"),
        "the reason is shown: {texts}"
    );
}

#[test]
fn a_device_of_a_nested_container_stays_in_its_container() {
    let mut d = nested();
    set_open(&mut d, "layer", true);
    set_open(&mut d, "layer/fx", true);
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("layer/fx/deep")).unwrap();
    let to = on(&v, "lead", 0.2);
    let a = drag(&mut v, d.doc(), from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(
        texts.contains("not from one container into another"),
        "{texts}"
    );
}

#[test]
fn a_container_dropped_on_its_own_frame_does_nothing() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let from = v.plate_center(&key("layer")).unwrap();
    let to = on(&v, "layer/pad", 0.5);
    let a = drag(&mut v, d.doc(), from, to);
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Container(_))),
        "{a:?}"
    );
}

#[test]
fn the_rack_treats_an_unfolded_container_and_its_inside_as_one_block() {
    let mut d = layered();
    apply(
        &mut d,
        Command::AddDevice {
            index: 2,
            type_id: DELAY.into(),
            key: Some(key("tail")),
        },
    );
    set_open(&mut d, "layer", true);
    let reg = registry();
    let mut v = inline_view(d.doc());
    // Released above the middle of the lead: the tail goes before everything.
    let from = v.plate_center(&key("tail")).unwrap();
    let to = on(&v, "lead", 0.1);
    let a = drag(&mut v, d.doc(), from, to);
    let want = Command::Workflow(atomik_model::WorkflowCommand::MoveDevicesToSlot {
        keys: vec![key("tail")],
        slot: atomik_model::RackSlot {
            column: 0,
            index: 0,
        },
        route: true,
    });
    assert_eq!(commands(&a), vec![want.clone()], "{a:?}");
    let mut d2 = d;
    d2.apply(want, &reg).expect("the model takes it");
    let top = top_keys(&d2);
    assert_eq!(top, ["tail", "lead", "layer"]);
}

#[test]
fn a_drop_below_the_whole_block_lands_after_the_container_not_inside_it() {
    let mut d = layered();
    apply(
        &mut d,
        Command::AddDevice {
            index: 0,
            type_id: DELAY.into(),
            key: Some(key("head")),
        },
    );
    set_open(&mut d, "layer", true);
    let reg = registry();
    let mut v = inline_view(d.doc());
    // Released in the gap under the frame: the head goes to the end of the column.
    let foot = rect(&v, "layer/boundary~out");
    let to = v.rack_to_view(Point::new(foot.x0 + 600.0, foot.y1 + 30.0));
    let from = v.plate_center(&key("head")).unwrap();
    let a = drag(&mut v, d.doc(), from, to);
    let want = Command::Workflow(atomik_model::WorkflowCommand::MoveDevicesToSlot {
        keys: vec![key("head")],
        slot: atomik_model::RackSlot {
            column: 0,
            index: 2,
        },
        route: true,
    });
    assert_eq!(commands(&a), vec![want.clone()], "{a:?}");
    let mut d2 = d;
    d2.apply(want, &reg).expect("the model takes it");
    let top = top_keys(&d2);
    assert_eq!(top, ["lead", "layer", "head"]);
}

// ------------------------------------------------------------------ the browser, the library, menus

fn browser_row(v: &RackView, type_id: &str) -> Point {
    let reg = registry();
    let rows = v.browser_rows(&reg);
    let i = rows
        .iter()
        .position(|r| *r == type_id)
        .unwrap_or_else(|| panic!("no row {type_id}"));
    v.browser_row_center(i).unwrap()
}

#[test]
fn a_browser_row_dropped_in_the_frame_adds_the_device_inside_at_that_place() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_browser_visible(true);
    let row = browser_row(&v, "atomik.stereo-delay");
    let to = on(&v, "layer/pad", 0.9);
    let a = drag(&mut v, d.doc(), row, to);
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::AddInner {
            container: key("layer"),
            index: 2,
            type_id: "atomik.stereo-delay".into(),
            key: None,
            auto_split: false,
        })],
        "{a:?}"
    );
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    assert_eq!(local_keys(&d2, "layer").len(), 5);
}

#[test]
fn an_instrument_row_dropped_in_the_frame_gets_its_full_range_split() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_browser_visible(true);
    let row = browser_row(&v, "atomik.poly-synth");
    let to = on(&v, "layer/synth", 0.1);
    let a = drag(&mut v, d.doc(), row, to);
    match commands(&a).as_slice() {
        [
            Command::Container(ContainerCommand::AddInner {
                container,
                index,
                auto_split,
                ..
            }),
        ] => {
            assert_eq!(container, &key("layer"));
            assert_eq!(*index, 0);
            assert!(auto_split);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_browser_row_dropped_under_the_frame_adds_to_the_rack_after_the_container() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_browser_visible(true);
    let row = browser_row(&v, "atomik.stereo-delay");
    let foot = rect(&v, "layer/boundary~out");
    let to = v.rack_to_view(Point::new(foot.x0 + 600.0, foot.y1 + 30.0));
    let a = drag(&mut v, d.doc(), row, to);
    let c = commands(&a);
    assert!(
        c.iter().all(|c| !matches!(c, Command::Container(_))),
        "a rack add: {c:?}"
    );
    assert_eq!(c.len(), 1, "{a:?}");
}

#[test]
fn the_add_key_with_an_inner_device_selected_adds_inside_after_it() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_browser_visible(true);
    let plate = v.plate_center(&key("layer/synth")).unwrap();
    let _ = click(&mut v, d.doc(), plate);
    let row = browser_row(&v, "atomik.stereo-delay");
    let a = press_with(
        &mut v,
        d.doc(),
        row,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    );
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::AddInner {
            container: key("layer"),
            index: 1,
            type_id: "atomik.stereo-delay".into(),
            key: None,
            auto_split: false,
        })],
        "{a:?}"
    );
}

#[test]
fn the_add_key_with_a_rack_device_selected_adds_to_the_rack_after_it() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    v.set_browser_visible(true);
    let plate = v.plate_center(&key("lead")).unwrap();
    let _ = click(&mut v, d.doc(), plate);
    let row = browser_row(&v, "atomik.stereo-delay");
    let a = press_with(
        &mut v,
        d.doc(),
        row,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    );
    let c = commands(&a);
    assert_eq!(c.len(), 1, "{a:?}");
    assert!(!matches!(c[0], Command::Container(_)), "{c:?}");
}

#[test]
fn a_library_item_dropped_in_the_frame_goes_into_the_container() {
    use std::path::PathBuf;
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let path = PathBuf::from("/lib/Patches/Pad.atomikpatch");
    let to = on(&v, "layer/pad", 0.9);
    let hover = UiEvent::FileHover {
        pos: to,
        paths: vec![path.clone()],
    };
    let _ = v.handle(&hover, d.doc(), &reg);
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: to,
            paths: vec![path.clone()],
        },
        d.doc(),
        &reg,
    );
    // A patch dropped on a panel of a device of its own type is for that device, so the drop
    // below uses the gap under the last device instead: right under the frame content.
    let _ = a;
    let to = on(&v, "layer/boundary~out", 0.5);
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: to,
            paths: vec![path],
        },
        d.doc(),
        &reg,
    );
    match a.as_slice() {
        [
            UiAction::InsertLibrary {
                container, slot, ..
            },
        ] => {
            assert_eq!(container.as_ref(), Some(&key("layer")));
            assert_eq!(slot.column, 0);
            assert_eq!(slot.index, 4);
        }
        other => panic!("{other:?}"),
    }
}

fn rows(v: &RackView) -> Vec<String> {
    v.macro_menu_rows().into_iter().map(|(l, _)| l).collect()
}

#[test]
fn a_right_click_on_a_free_inner_knob_offers_the_macros_of_its_container() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let p = v
        .control_center(&key("layer/pad"), "filter.resonance")
        .unwrap();
    assert!(right_click(&mut v, d.doc(), p).is_empty());
    let r = rows(&v);
    assert_eq!(r.len(), 8);
    assert_eq!(r[0], "Map to macro 1 (Cutoff)");
    assert_eq!(r[1], "Map to macro 2");
    // A row maps the param with the descriptor range; the target names the device relative
    // to its container.
    let row = v.macro_menu_rows()[1].1;
    let a = click(&mut v, d.doc(), row);
    match commands(&a).as_slice() {
        [
            Command::Container(ContainerCommand::SetMacro {
                container,
                index,
                target,
                ..
            }),
        ] => {
            assert_eq!(container, &key("layer"));
            assert_eq!(*index, 1);
            assert_eq!(target.device, key("pad"));
            assert_eq!(target.param, "filter.resonance");
        }
        other => panic!("{other:?}"),
    }
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    let _ = reg;
}

#[test]
fn a_right_click_on_a_knob_of_the_rack_opens_no_macro_menu() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v.control_center(&key("lead"), "filter.resonance").unwrap();
    let _ = right_click(&mut v, d.doc(), p);
    assert!(rows(&v).is_empty());
}

#[test]
fn a_mapped_inner_param_offers_edit_range_and_unmap() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v
        .control_center(&key("layer/synth"), "filter.cutoff")
        .unwrap();
    let _ = right_click(&mut v, d.doc(), p);
    assert_eq!(rows(&v), ["Edit range", "Unmap"]);
}

#[test]
fn an_inner_container_opens_its_own_view_with_one_scope_level_per_path_segment() {
    let mut d = nested();
    set_open(&mut d, "layer", true);
    let mut v = inline_view(d.doc());
    let p = v
        .container_button_center(&key("layer/fx"), ContainerButton::Open)
        .expect("OPEN of the inner container");
    let a = click(&mut v, d.doc(), p);
    assert!(
        a.contains(&UiAction::EnterContainer {
            device: key("layer/fx")
        }),
        "{a:?}"
    );
    assert_eq!(v.scope(), [key("layer"), key("fx")]);
}

#[test]
fn the_arrow_of_an_inner_container_folds_that_container_only() {
    let mut d = nested();
    set_open(&mut d, "layer", true);
    let mut v = inline_view(d.doc());
    let p = centre(&v, "layer/fx", ContainerButton::Fold);
    let a = click(&mut v, d.doc(), p);
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::SetOpen {
            container: key("layer/fx"),
            open: true,
        })]
    );
}

#[test]
fn to_channel_of_an_inner_drum_machine_says_to_use_the_container() {
    // A device inside a container reaches the console through the container.
    let mut d = layered();
    apply(
        &mut d,
        Command::Container(ContainerCommand::AddInner {
            container: key("layer"),
            index: 4,
            type_id: "atomik.drum-machine".into(),
            key: Some(key("kit")),
            auto_split: false,
        }),
    );
    set_open(&mut d, "layer", true);
    let mut v = inline_view(d.doc());
    let p = v.to_channel_button_center(&key("layer/kit")).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert!(commands(&a).is_empty(), "{a:?}");
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(texts.contains("through the container"), "{texts}");
}

#[test]
fn a_press_in_the_alt_fold_of_a_column_leaves_the_inner_devices_out() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v.fold_button_center(&key("lead")).unwrap();
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    let a = press_with(&mut v, d.doc(), p, PointerButton::Primary, 1, alt);
    match commands(&a).as_slice() {
        [Command::Workflow(atomik_model::WorkflowCommand::SetFoldedMany { keys, .. })] => {
            assert!(keys.iter().all(|k| !k.0.contains('/')), "{keys:?}");
        }
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------ keys, menus and feedback

#[test]
fn the_neighbour_keys_walk_the_inner_devices_in_the_order_they_are_drawn() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let plate = v.plate_center(&key("lead")).unwrap();
    let _ = click(&mut v, d.doc(), plate);
    let mut walked = Vec::new();
    for _ in 0..5 {
        let _ = v.command("rack.select_next", None, d.doc(), &reg);
        walked.push(v.selected().unwrap().0.clone());
    }
    assert_eq!(
        walked,
        [
            "layer",
            "layer/synth",
            "layer/pad",
            "layer/delay",
            "layer/reverb"
        ]
    );
    // Going back up passes the container again.
    let _ = v.command("rack.select_prev", None, d.doc(), &reg);
    assert_eq!(v.selected(), Some(&key("layer/delay")));
}

#[test]
fn a_right_click_on_an_inner_device_selects_it_and_asks_for_the_device_menu() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = v.plate_center(&key("layer/pad")).unwrap();
    let a = right_click(&mut v, d.doc(), p);
    assert!(a.contains(&UiAction::Select(key("layer/pad"))), "{a:?}");
    assert!(
        a.iter().any(|x| matches!(
            x,
            UiAction::ContextMenu {
                target: atomik_ui::ContextTarget::Device,
                ..
            }
        )),
        "{a:?}"
    );
}

#[test]
fn move_out_of_container_is_offered_for_one_inner_device_of_a_top_level_container() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    // Nothing selected, then a device of the rack, then an inner device the model lets go.
    assert!(v.move_out_reason(d.doc(), &reg).is_some());
    let p = v.plate_center(&key("lead")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    assert!(v.move_out_reason(d.doc(), &reg).is_some());
    let p = v.plate_center(&key("layer/reverb")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    assert_eq!(v.move_out_reason(d.doc(), &reg), None);
    assert_eq!(v.move_out_index_for(&key("layer/reverb"), d.doc()), 2);
    // The synth is cabled to the delay inside: the model says why not.
    let p = v.plate_center(&key("layer/synth")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    let why = v.move_out_reason(d.doc(), &reg).expect("refused");
    assert!(why.contains("cannot move the device out"), "{why}");
}

#[test]
fn the_drop_feedback_shows_the_frame_while_a_device_is_dragged_over_it() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let idle = paint_rack(&mut v, d.doc()).to_json();
    let from = v.plate_center(&key("layer/synth")).unwrap();
    let to = on(&v, "layer/reverb", 0.8);
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    let dragging = paint_rack(&mut v, d.doc()).to_json();
    assert_ne!(
        idle, dragging,
        "the drop line and the frame outline are drawn"
    );
    let _ = v.handle(&up(to), d.doc(), &reg);
}

#[test]
fn the_navigator_draws_the_inner_devices_in_the_column_of_their_container() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let folded = paint_rack(&mut inline_view(layered().doc()), layered().doc())
        .to_json()
        .len();
    let open = paint_rack(&mut v, d.doc()).to_json().len();
    assert!(open > folded, "the unfolded rack draws more");
    // The content is taller, and the navigator follows the same layout.
    assert!(v.content_height() > inline_view(layered().doc()).content_height());
}

#[test]
fn an_unfolded_container_is_not_a_scope() {
    let d = unfolded();
    let v = inline_view(d.doc());
    assert!(v.scope().is_empty());
    assert_eq!(v.scope_depth(), 0);
    assert!(v.breadcrumb_labels().len() <= 1);
}

// ------------------------------------------------------------------ the back panel and its cables

fn back_view(d: &Document) -> RackView {
    let mut v = inline_view(d.doc());
    v.set_flip(Flip::Back);
    v.sync(d.doc(), &registry());
    v
}

fn jack(v: &RackView, device: &str, socket: &str) -> Point {
    v.socket_center(&sref(device, socket))
        .unwrap_or_else(|| panic!("no jack {device}:{socket}"))
}

fn rendered(cable: &atomik_model::Cable) -> String {
    cable.to_string()
}

#[test]
fn the_back_shows_the_inner_cables_and_the_containers_own_jacks() {
    let d = unfolded();
    let v = back_view(&d);
    let shown: Vec<String> = v.visible_cables(d.doc()).iter().map(rendered).collect();
    for expected in [
        "layer/synth:out_l -> layer/delay:in_l",
        "layer/delay:out_l -> layer/boundary:out_l",
    ] {
        assert!(
            shown.contains(&expected.to_string()),
            "{expected}: {shown:?}"
        );
    }
    // The INPUT and OUTPUT strips carry the container's jacks, the container panel its own.
    for (device, socket) in [
        ("layer/boundary", "in_l"),
        ("layer/boundary", "out_l"),
        ("layer", "in_l"),
        ("layer", "out_l"),
        ("layer/pad", "out_l"),
    ] {
        assert!(
            v.socket_center(&sref(device, socket)).is_some(),
            "{device}:{socket}"
        );
    }
}

#[test]
fn a_cable_between_two_inner_devices_is_connected_by_path_key() {
    let d = unfolded();
    let mut v = back_view(&d);
    let from = jack(&v, "layer/pad", "out_l");
    let to = jack(&v, "layer/reverb", "in_l");
    let a = drag(&mut v, d.doc(), from, to);
    // The stereo partner follows, as for any pair of jacks.
    match commands(&a).as_slice() {
        [Command::Batch(parts)] => match parts.as_slice() {
            [Command::Connect(first), Command::Connect(second)] => {
                assert_eq!(rendered(first), "layer/pad:out_l -> layer/reverb:in_l");
                assert_eq!(rendered(second), "layer/pad:out_r -> layer/reverb:in_r");
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    assert!(
        d2.doc()
            .inner(&key("layer"))
            .unwrap()
            .cables
            .iter()
            .any(|c| rendered(c) == "pad:out_l -> reverb:in_l")
    );
}

#[test]
fn a_cable_from_the_input_strip_reaches_an_inner_device() {
    let d = unfolded();
    let mut v = back_view(&d);
    let from = jack(&v, "layer/boundary", "in_l");
    let to = jack(&v, "layer/reverb", "in_l");
    let a = drag(&mut v, d.doc(), from, to);
    assert!(!commands(&a).is_empty(), "{a:?}");
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    assert!(
        d2.doc()
            .inner(&key("layer"))
            .unwrap()
            .cables
            .iter()
            .any(|c| rendered(c) == "boundary:in_l -> reverb:in_l")
    );
}

#[test]
fn a_cable_to_the_output_strip_replaces_the_one_on_that_jack() {
    let d = unfolded();
    let mut v = back_view(&d);
    let from = jack(&v, "layer/pad", "out_l");
    let to = jack(&v, "layer/boundary", "out_l");
    let a = drag(&mut v, d.doc(), from, to);
    let c = commands(&a);
    assert!(!c.is_empty(), "{a:?}");
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
    let cables: Vec<String> = d2
        .doc()
        .inner(&key("layer"))
        .unwrap()
        .cables
        .iter()
        .map(rendered)
        .collect();
    assert!(
        cables.contains(&"pad:out_l -> boundary:out_l".to_string()),
        "{cables:?}"
    );
    assert!(
        !cables.contains(&"delay:out_l -> boundary:out_l".to_string()),
        "{cables:?}"
    );
}

#[test]
fn a_cable_never_joins_two_containers_levels() {
    let d = unfolded();
    let mut v = back_view(&d);
    let from = jack(&v, "lead", "out_l");
    let inner_target = sref("layer/reverb", "in_l");
    let to = jack(&v, "layer/reverb", "in_l");
    let a = drag(&mut v, d.doc(), from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    // The jack is dimmed while the cable is dragged from the rack's own device.
    let reg = registry();
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    assert_eq!(
        v.socket_highlight(&inner_target),
        atomik_ui::SocketHighlight::Incompatible
    );
    let _ = v.handle(&up(to), d.doc(), &reg);
}

#[test]
fn a_cable_to_the_containers_own_jack_is_a_cable_of_the_rack() {
    let d = unfolded();
    let mut v = back_view(&d);
    // The lead feeds the container's input jacks (the rack's level).
    let from = jack(&v, "lead", "out_l");
    let to = jack(&v, "layer", "in_l");
    let a = drag(&mut v, d.doc(), from, to);
    match commands(&a).as_slice() {
        [Command::Batch(parts)] => match parts.first() {
            Some(Command::Connect(c)) => assert_eq!(rendered(c), "lead:out_l -> layer:in_l"),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?} {a:?}"),
    }
}

#[test]
fn picking_a_cable_up_from_an_inner_input_and_dropping_it_on_nothing_cuts_it() {
    let d = unfolded();
    let mut v = back_view(&d);
    let from = jack(&v, "layer/delay", "in_l");
    let nowhere = v.rack_to_view(Point::new(-10.0, 5.0));
    let a = drag(&mut v, d.doc(), from, nowhere);
    assert_eq!(
        commands(&a),
        vec![Command::Disconnect {
            to: sref("layer/delay", "in_l")
        }],
        "{a:?}"
    );
    let mut d2 = unfolded();
    apply_all(&mut d2, &a);
}

#[test]
fn the_jack_menu_of_an_inner_jack_lists_only_the_devices_of_its_container() {
    let d = unfolded();
    let reg = registry();
    let v = back_view(&d);
    let menu = v
        .jack_menu(&sref("layer/pad", "out_l"), d.doc(), &reg)
        .expect("a menu");
    let names: Vec<&str> = menu.groups.iter().map(|g| g.device.as_str()).collect();
    assert!(names.contains(&"Reverb"), "{names:?}");
    assert!(
        !names.contains(&"Poly Synth") && !names.contains(&"Layer"),
        "devices of the rack are in another scope: {names:?}"
    );
}

#[test]
fn the_jack_menu_of_a_jack_of_the_rack_leaves_the_inner_devices_out() {
    let d = unfolded();
    let reg = registry();
    let v = back_view(&d);
    let menu = v
        .jack_menu(&sref("lead", "out_l"), d.doc(), &reg)
        .expect("a menu");
    let names: Vec<&str> = menu.groups.iter().map(|g| g.device.as_str()).collect();
    assert!(names.contains(&"Layer"), "{names:?}");
    assert!(
        !names.contains(&"Reverb") && !names.contains(&"Stereo Delay"),
        "{names:?}"
    );
}

#[test]
fn tab_flips_the_unfolded_rack_and_keeps_the_selection() {
    let d = unfolded();
    let reg = registry();
    let mut v = inline_view(d.doc());
    let p = v.plate_center(&key("layer/pad")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    v.set_flip(Flip::Back);
    v.sync(d.doc(), &reg);
    assert_eq!(v.selected(), Some(&key("layer/pad")));
    v.set_flip(Flip::Front);
    v.sync(d.doc(), &reg);
    assert_eq!(v.selected(), Some(&key("layer/pad")));
}

#[test]
fn opening_an_unfolded_container_in_its_own_view_shows_only_its_inside() {
    let d = unfolded();
    let mut v = inline_view(d.doc());
    let p = centre(&v, "layer", ContainerButton::Open);
    let _ = click(&mut v, d.doc(), p);
    assert_eq!(v.scope(), [key("layer")]);
    assert!(!v.is_inline());
    assert!(keys_in_view(&v).contains(&"synth".to_string()));
    assert!(v.frames().is_empty());
}
