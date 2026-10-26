//! The quantize panel: every value steps with its buttons and the wheel, the groove row cycles
//! through the offered grooves, a groove dims the grid, APPLY and EXTRACT need a target, the
//! library lists, deletes and scrolls, and the settings become the model's parameters.

mod support;

use atomik_core::PPQ;
use atomik_model::StepResolution;
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::quantize_panel::GRIDS;
use atomik_ui::{
    DisplayList, GrooveEntry, QuantizeAction, QuantizeRow, QuantizeValues, QuantizeView, Theme,
    UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn groove(name: &str, builtin: bool) -> GrooveEntry {
    GrooveEntry {
        name: name.to_owned(),
        step: StepResolution::Sixteenth,
        slots: 2,
        builtin,
    }
}

fn view() -> QuantizeView {
    let mut v = QuantizeView::new(Theme::dark(), QuantizeValues::default());
    v.set_bounds(BOUNDS);
    v.set_grooves(vec![
        groove("Straight", true),
        groove("Swing 60%", true),
        groove("Mine", false),
    ]);
    v.set_target("2 selected clips".into(), true, true);
    v
}

fn click(v: &mut QuantizeView, p: Point) -> Vec<QuantizeAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Quantize(q) => Some(q),
            _ => None,
        })
        .collect()
}

/// A click at a point worked out from the view itself.
macro_rules! clk {
    ($v:ident, $e:expr) => {{
        let p = $e;
        click(&mut $v, p)
    }};
}

fn texts(v: &QuantizeView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

fn wheel(v: &mut QuantizeView, p: Point, dy: f64) {
    v.handle(&UiEvent::Wheel {
        pos: p,
        delta: Vec2::new(0.0, dy),
        modifiers: Default::default(),
    });
}

#[test]
fn the_panel_paints_its_rows_values_and_buttons() {
    let v = view();
    let t = texts(&v);
    for want in [
        "QUANTIZE",
        "Applies to: 2 selected clips",
        "Grid",
        "1/16",
        "Strength",
        "100 %",
        "Swing",
        "50 %",
        "Groove",
        "None",
        "Humanize timing",
        "Humanize velocity",
        "Seed",
        "Snap note ends",
        "OFF",
        "APPLY",
        "GROOVES IN THIS SONG",
        "Mine",
        "DELETE",
        "EXTRACT GROOVE FROM CLIP",
        "CLOSE",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
}

#[test]
fn an_empty_library_and_no_target_say_so() {
    let mut v = QuantizeView::new(Theme::dark(), QuantizeValues::default());
    v.set_bounds(BOUNDS);
    let t = texts(&v);
    assert!(has(&t, "Applies to: nothing selected"));
    assert!(has(&t, "None yet: extract one from a clip."));
}

#[test]
fn plus_and_minus_step_each_value_within_its_range() {
    let mut v = view();
    let press = |v: &mut QuantizeView, row, plus: bool| {
        let r = if plus {
            v.plus_rect(row)
        } else {
            v.minus_rect(row)
        };
        click(v, r.center());
    };
    press(&mut v, QuantizeRow::Strength, false);
    assert_eq!(v.values().strength, 95);
    for _ in 0..30 {
        press(&mut v, QuantizeRow::Strength, false);
    }
    assert_eq!(v.values().strength, 0, "stops at 0");
    press(&mut v, QuantizeRow::Strength, true);
    assert_eq!(v.values().strength, 5);
    for _ in 0..40 {
        press(&mut v, QuantizeRow::Swing, true);
    }
    assert_eq!(v.values().swing, 75, "swing stops at 75");
    for _ in 0..40 {
        press(&mut v, QuantizeRow::Swing, false);
    }
    assert_eq!(v.values().swing, 50, "and at 50");
    press(&mut v, QuantizeRow::Grid, true);
    assert_eq!(v.value_text(QuantizeRow::Grid), "1/16 triplet");
    press(&mut v, QuantizeRow::HumanizeTime, true);
    press(&mut v, QuantizeRow::HumanizeVelocity, true);
    press(&mut v, QuantizeRow::Seed, true);
    assert_eq!(
        (
            v.values().humanize_time,
            v.values().humanize_velocity,
            v.values().seed
        ),
        (5, 5, 1)
    );
    // The snap row toggles from either button or its value window.
    press(&mut v, QuantizeRow::SnapEnds, true);
    assert!(v.values().snap_ends);
    clk!(v, v.value_rect(QuantizeRow::SnapEnds).center());
    assert!(!v.values().snap_ends);
}

#[test]
fn the_wheel_steps_the_row_under_the_pointer() {
    let mut v = view();
    let at = v.value_rect(QuantizeRow::Strength).center();
    wheel(&mut v, at, -1.0);
    assert_eq!(v.values().strength, 95);
    wheel(&mut v, at, 1.0);
    wheel(&mut v, at, 1.0);
    assert_eq!(v.values().strength, 100, "clamped");
    let at = v.value_rect(QuantizeRow::Swing).center();
    wheel(&mut v, at, 1.0);
    assert_eq!(v.values().swing, 51);
}

#[test]
fn the_groove_row_cycles_none_builtins_and_the_songs_grooves() {
    let mut v = view();
    let plus = v.plus_rect(QuantizeRow::Groove).center();
    let minus = v.minus_rect(QuantizeRow::Groove).center();
    let names: Vec<_> = (0..4)
        .map(|_| {
            click(&mut v, plus);
            v.value_text(QuantizeRow::Groove)
        })
        .collect();
    assert_eq!(names, ["Straight", "Swing 60%", "Mine", "None"]);
    click(&mut v, minus);
    assert_eq!(
        v.values().groove.as_deref(),
        Some("Mine"),
        "wraps backwards"
    );
}

#[test]
fn a_groove_sets_the_grid_and_enables_its_velocity() {
    let mut v = view();
    assert!(v.row_enabled(QuantizeRow::Grid));
    assert!(!v.row_enabled(QuantizeRow::GrooveVelocity));
    let before = v.values().clone();
    clk!(v, v.plus_rect(QuantizeRow::GrooveVelocity).center());
    assert_eq!(*v.values(), before, "a disabled row does not step");
    clk!(v, v.plus_rect(QuantizeRow::Groove).center());
    assert!(!v.row_enabled(QuantizeRow::Grid));
    assert!(v.row_enabled(QuantizeRow::GrooveVelocity));
    assert_eq!(v.value_text(QuantizeRow::Grid), "1/16 (groove)");
    clk!(v, v.minus_rect(QuantizeRow::Grid).center());
    assert_eq!(
        v.values().grid,
        before.grid,
        "the grid does not move while a groove sets it"
    );
    clk!(v, v.minus_rect(QuantizeRow::GrooveVelocity).center());
    assert_eq!(v.values().groove_velocity, 95);
}

#[test]
fn a_chosen_groove_that_goes_away_is_dropped() {
    let mut v = view();
    v.select_groove("Mine");
    assert_eq!(v.values().groove.as_deref(), Some("Mine"));
    v.set_grooves(vec![groove("Straight", true)]);
    assert_eq!(v.values().groove, None);
    v.select_groove("nope");
    assert_eq!(v.values().groove, None, "an unknown name is ignored");
}

#[test]
fn the_settings_become_the_models_parameters() {
    let mut v = view();
    for (row, n) in [
        (QuantizeRow::Strength, -2i32),
        (QuantizeRow::Swing, 16),
        (QuantizeRow::HumanizeTime, 4),
        (QuantizeRow::HumanizeVelocity, 2),
        (QuantizeRow::Seed, 7),
        (QuantizeRow::Grid, -1),
    ] {
        for _ in 0..n.abs() {
            v.step(row, n.signum());
        }
    }
    v.step(QuantizeRow::SnapEnds, 1);
    let p = v.params();
    assert_eq!(p.grid, GRIDS[2].1);
    assert_eq!(p.grid, PPQ / 3, "a triplet eighth");
    assert!((p.strength - 0.9).abs() < 1e-6);
    assert_eq!(p.swing, 66);
    assert!((p.humanize_time - 0.2).abs() < 1e-6);
    assert!((p.humanize_velocity - 0.1).abs() < 1e-6);
    assert_eq!((p.seed, p.snap_ends, &p.groove), (7, true, &None));
    p.validate().unwrap();
    // Every combination the panel can make is a valid parameter set.
    for g in 0..GRIDS.len() {
        let values = QuantizeValues {
            grid: g,
            ..QuantizeValues::default()
        };
        values.params().validate().unwrap();
    }
}

#[test]
fn apply_and_extract_need_a_target_and_close_always_works() {
    let mut v = view();
    assert_eq!(clk!(v, v.apply_rect().center()), [QuantizeAction::Apply]);
    assert_eq!(
        clk!(v, v.extract_rect().center()),
        [QuantizeAction::Extract]
    );
    assert_eq!(clk!(v, v.close_rect().center()), [QuantizeAction::Close]);
    v.set_target(String::new(), false, false);
    assert!(clk!(v, v.apply_rect().center()).is_empty());
    assert!(clk!(v, v.extract_rect().center()).is_empty());
    assert_eq!(clk!(v, v.close_rect().center()), [QuantizeAction::Close]);
    v.set_target("4 selected notes".into(), true, false);
    assert_eq!(clk!(v, v.apply_rect().center()), [QuantizeAction::Apply]);
    assert!(clk!(v, v.extract_rect().center()).is_empty());
}

#[test]
fn the_extract_row_steps_the_measure_and_the_slot_count() {
    let mut v = view();
    clk!(v, v.plus_rect(QuantizeRow::ExtractStep).center());
    assert_eq!(v.values().extract_step, StepResolution::ThirtySecond);
    clk!(v, v.plus_rect(QuantizeRow::ExtractStep).center());
    assert_eq!(
        v.values().extract_step,
        StepResolution::ThirtySecond,
        "stops"
    );
    for _ in 0..3 {
        clk!(v, v.plus_rect(QuantizeRow::ExtractSlots).center());
    }
    assert_eq!(v.values().extract_slots, 5);
    for _ in 0..40 {
        clk!(v, v.minus_rect(QuantizeRow::ExtractSlots).center());
    }
    assert_eq!(v.values().extract_slots, 1);
    for _ in 0..80 {
        v.step(QuantizeRow::ExtractSlots, 1);
    }
    assert_eq!(v.values().extract_slots, 32);
}

#[test]
fn delete_names_the_groove_of_its_row_and_the_list_scrolls() {
    let mut v = view();
    let del = v.delete_rect(0).expect("one row");
    assert_eq!(
        click(&mut v, del.center()),
        [QuantizeAction::Delete("Mine".into())]
    );
    // Six grooves: four rows show, the wheel over the list scrolls.
    let mut grooves = vec![groove("Straight", true)];
    grooves.extend((1..=6).map(|i| groove(&format!("g{i}"), false)));
    v.set_grooves(grooves);
    assert!(v.delete_rect(3).is_some() && v.delete_rect(4).is_none());
    let in_list = v.library_row_rect(0).unwrap().center();
    wheel(&mut v, in_list, -1.0);
    assert!(v.delete_rect(0).is_none(), "the first row scrolled out");
    let del = v.delete_rect(4).expect("the fifth row scrolled in");
    assert_eq!(
        click(&mut v, del.center()),
        [QuantizeAction::Delete("g5".into())]
    );
}

#[test]
fn the_panel_is_centred_and_keeps_inside_a_small_window() {
    let mut v = view();
    let p = v.panel();
    assert!((p.center().x - BOUNDS.center().x).abs() < 1.0);
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 300.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 300.0 && p.y1 <= 300.0, "{p:?}");
}

#[test]
fn the_panel_matches_its_golden() {
    let v = view();
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    starter::check_golden("quantize_panel_1x.json", &dl.to_json());
}

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    use support::cpu::CpuCanvas;
    let mut v = view();
    v.set_scale(1.5);
    v.select_groove("Mine");
    v.step(QuantizeRow::HumanizeTime, 1);
    let mut text = atomik_ui::TextEngine::new();
    let mut canvas = CpuCanvas::new(1350, 1050, &mut text);
    v.paint(&mut canvas);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/quantize_panel.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
fn dragging_a_value_up_or_down_steps_it_and_a_click_changes_nothing() {
    let mut v = view();
    let at = v.value_rect(QuantizeRow::Swing).center();
    let to = |dy: f64| Point::new(at.x, at.y + dy);
    v.handle(&down(at));
    assert!(v.is_dragging());
    v.handle(&mv(to(-15.0)));
    assert_eq!(v.values().swing, 53, "5 px per step, up increases");
    v.handle(&mv(to(-5.0)));
    assert_eq!(v.values().swing, 51, "and down decreases");
    v.handle(&up(to(-5.0)));
    assert!(!v.is_dragging());
    v.handle(&mv(to(-100.0)));
    assert_eq!(v.values().swing, 51, "a released pointer drags nothing");
    // A grid drag steps the list in order; a click on the value box changes nothing.
    let grid = v.value_rect(QuantizeRow::Grid).center();
    let before = v.values().grid;
    v.handle(&down(grid));
    v.handle(&up(grid));
    assert_eq!(v.values().grid, before);
    v.handle(&down(grid));
    v.handle(&mv(Point::new(grid.x, grid.y - 10.0)));
    assert_eq!(v.values().grid, before + 2);
}
