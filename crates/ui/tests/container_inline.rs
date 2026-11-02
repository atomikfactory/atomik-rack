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
