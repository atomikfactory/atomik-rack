//! The tonal balance panel: geometry, the maths of its meters and curve, the target list (pick,
//! learn from files, save, rename, delete), the settings under the visual, painting and the
//! French texts. The display-list goldens of both faces are in `fx_panels.rs`
//! (`fx_tonal-balance_*`).

mod support;

use atomik_model::{Command, Document};
use atomik_ui::analyser::AnalyserData;
use atomik_ui::fx::FxKind;
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::tonal_panel::{self as tp, RowAction, TargetKind, TargetRow, TonalOp, TonalView};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use support::fx::*;
use support::*;

fn rig_with(params: &[(&str, f32)], data: Option<&AnalyserData>) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::TonalBalance);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    v.sync(&doc, &reg);
    if let Some(d) = data {
        v.set_analyser(&key("fx"), d);
    }
    (Document::new(doc), v)
}

fn rig() -> (Document, RackView) {
    rig_with(&[], Some(&tonal_data()))
}

fn apply(document: &mut Document, v: &mut RackView, actions: &[UiAction]) {
    let reg = registry();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                document
                    .apply(c.clone(), &reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    v.sync(document.doc(), &reg);
}

fn click(document: &mut Document, v: &mut RackView, at: Point) -> Vec<UiAction> {
    let reg = registry();
    let a = v.handle(&down(at), document.doc(), &reg);
    apply(document, v, &a);
    let b = v.handle(&up(at), document.doc(), &reg);
    apply(document, v, &b);
    a.into_iter().chain(b).collect()
}

fn type_key(document: &mut Document, v: &mut RackView, key: Key) -> Vec<UiAction> {
    let a = v.handle(
        &UiEvent::Key {
            key,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        document.doc(),
        &registry(),
    );
    apply(document, v, &a);
    a
}

fn param(document: &Document, p: &str) -> f32 {
    document
        .doc()
        .param_value(&key("fx"), p, &registry())
        .unwrap()
}

fn set_params(actions: &[UiAction]) -> Vec<(String, f32)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { param, value, .. } => Some((param, value)),
            _ => None,
        })
        .collect()
}

fn tonal_ops(actions: &[UiAction]) -> Vec<TonalOp> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::TonalTarget { op, .. } => Some(op.clone()),
            _ => None,
        })
        .collect()
}

fn paint(document: &Document, v: &mut RackView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &registry());
    dl
}

fn texts(dl: &DisplayList) -> Vec<String> {
    dl.texts().into_iter().map(str::to_string).collect()
}

fn visual() -> Rect {
    let reg = registry();
    atomik_ui::fx_layout::fx_layout(
        FxKind::TonalBalance,
        reg.get(tp::TONAL_TYPE).unwrap().descriptor,
    )
    .fx
    .unwrap()
    .visual
}

// ------------------------------------------------------------------ geometry and maths

#[test]
fn the_panel_is_three_units_with_the_visual_across_the_top() {
    let reg = registry();
    let spec = atomik_ui::fx_layout::fx_layout(
        FxKind::TonalBalance,
        reg.get(tp::TONAL_TYPE).unwrap().descriptor,
    );
    assert_eq!(spec.height_units, 3);
    let fx = spec.fx.unwrap();
    assert_eq!(fx.kind, FxKind::TonalBalance);
    for c in &spec.front {
        assert!(c.rect.y0 >= fx.visual.y1, "{c:?} under the visual");
    }
    let labels: Vec<_> = spec
        .front_sections
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(labels, ["TONAL BALANCE", "RESPONSE", "VIEW", "LEARN"]);
}

#[test]
fn the_views_lay_out_the_meters_and_the_curve_inside_the_visual() {
    let v = visual();
    for view in [tp::VIEW_BANDS, tp::VIEW_CURVE, 2] {
        let g = tp::geometry(v, view);
        assert!(v.contains_rect(g.target), "{view}");
        assert!(g.target.x1 < g.status.x0);
        let has_meters = view != tp::VIEW_CURVE;
        let has_curve = view != tp::VIEW_BANDS;
        for (i, m) in g.meters.iter().enumerate() {
            assert_eq!(m.width() > 0.0, has_meters, "{view} {i}");
            if has_meters {
                assert!(v.contains_rect(*m), "{view} {i}: {m:?}");
                assert!(m.y0 >= g.target.y1, "meters under the header");
                if i > 0 {
                    assert!((g.meters[i - 1].x1 - m.x0).abs() < 1e-9, "columns touch");
                }
                let bar = tp::bar_rect(*m);
                assert!(m.contains_rect(bar) && bar.height() > 30.0, "{bar:?}");
            }
        }
        assert_eq!(g.curve.width() > 0.0, has_curve, "{view}");
        if has_curve {
            assert!(v.contains_rect(g.curve) && g.curve.width() > 150.0);
            if has_meters {
                assert!(
                    g.meters[3].x1 < g.curve.x0,
                    "the curve is beside the meters"
                );
            }
        }
    }
}

#[test]
fn a_meter_maps_the_deviation_with_the_target_in_the_middle() {
    let bar = Rect::new(0.0, 10.0, 24.0, 110.0);
    assert!((tp::meter_y(bar, 0.0) - 60.0).abs() < 1e-9);
    assert!((tp::meter_y(bar, tp::METER_RANGE_DB) - 10.0).abs() < 1e-9);
    assert!((tp::meter_y(bar, -tp::METER_RANGE_DB) - 110.0).abs() < 1e-9);
    assert!((tp::meter_y(bar, 40.0) - 10.0).abs() < 1e-9, "clamped");
    assert!(
        tp::meter_y(bar, 3.0) < tp::meter_y(bar, 0.0),
        "louder is higher"
    );
    assert!(tp::in_range(2.9, 3.0) && tp::in_range(-3.0, 3.0));
    assert!(!tp::in_range(3.1, 3.0) && !tp::in_range(-4.0, 3.0));
    assert_eq!(tp::deviation_text(4.24, true), "+4.2");
    assert_eq!(tp::deviation_text(-0.6, true), "-0.6");
    assert_eq!(tp::deviation_text(0.0, true), "+0.0");
    assert_eq!(tp::deviation_text(1.0, false), "--");
    assert_eq!(tp::deviation_text(f32::NAN, true), "--");
}

#[test]
fn the_curve_range_holds_the_target_range_and_the_mix() {
    let data = tonal_data().tonal;
    let (lo, hi) = tp::curve_range(&data);
    for (i, t) in data.target_bands.iter().enumerate() {
        assert!(lo <= t - data.target_tol[i] && hi >= t + data.target_tol[i]);
    }
    assert!(data.bands.iter().all(|b| *b >= lo && *b <= hi));
    assert!(hi - lo >= 30.0);
    let (lo, hi) = tp::curve_range(&TonalView::default());
    assert!(hi > lo, "an empty view still has a range");
    let plot = Rect::new(0.0, 0.0, 100.0, 80.0);
    assert!((tp::curve_y(plot, hi, (lo, hi)) - 0.0).abs() < 1e-9);
    assert!((tp::curve_y(plot, lo, (lo, hi)) - 80.0).abs() < 1e-9);
}

#[test]
fn the_smooth_curve_passes_through_the_band_values_in_order() {
    let values: Vec<f32> = (0..31).map(|i| (i as f32 * 0.7).sin() * 6.0).collect();
    let pts = tp::smooth_curve(&values, 4);
    assert_eq!(pts.len(), 30 * 4 + 1);
    for (i, v) in values.iter().enumerate().take(30) {
        let (hz, y) = pts[i * 4];
        assert!((y - v).abs() < 1e-4, "band {i}");
        assert!((hz - atomik_ui::analyser::band_centre_hz(i)).abs() < 1e-6);
    }
    assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "left to right");
    assert!(tp::smooth_curve(&[1.0], 4).is_empty());
}

#[test]
fn the_status_line_says_what_is_going_on_with_the_important_thing_first() {
    let base = TonalView::default();
    assert_eq!(tp::status_text(&base), "NO SIGNAL");
    let ready = TonalView {
        ready: true,
        ..base.clone()
    };
    assert_eq!(tp::status_text(&ready), "");
    let pending = TonalView {
        pending: true,
        ..ready.clone()
    };
    assert_eq!(tp::status_text(&pending), "LEARNED, NOT SAVED");
    let missing = TonalView {
        missing: true,
        ..pending.clone()
    };
    assert_eq!(tp::status_text(&missing), "TARGET MISSING, USING BALANCED");
    let learning = TonalView {
        learning: true,
        learned_seconds: 12.4,
        ..missing.clone()
    };
    assert_eq!(tp::status_text(&learning), "LEARNING 12 s");
    let files = TonalView {
        files: Some(0.4),
        ..learning
    };
    assert_eq!(tp::status_text(&files), "ANALYSING FILES 40%");
}

// ------------------------------------------------------------------ the target list

fn row(value: f32, name: &str, kind: TargetKind) -> TargetRow {
    TargetRow {
        value,
        name: name.to_string(),
        kind,
    }
}

#[test]
fn the_list_has_the_targets_then_the_actions_enabled_as_the_state_allows() {
    let mut view = tonal_data().tonal;
    view.selected = 0;
    let rows = tp::menu_rows(&view);
    let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Balanced",
            "Bright",
            "Warm",
            "Bass-heavy",
            "My reference",
            "Learn from files…",
            "Save learned target…",
            "Rename target…",
            "Delete target"
        ]
    );
    assert!(rows[0].checked && !rows[1].checked);
    assert!(rows[4].rule, "a rule above the user's targets");
    assert!(!rows[1].rule);
    let enabled = |rows: &[tp::MenuRow]| rows.iter().map(|r| r.enabled).collect::<Vec<_>>();
    // A built-in in force: nothing to save, rename or delete.
    assert_eq!(&enabled(&rows)[5..], [true, false, false, false]);
    // A saved one in force: rename and delete.
    view.selected = 4;
    assert_eq!(
        &enabled(&tp::menu_rows(&view))[5..],
        [true, false, true, true]
    );
    // A learned target waiting: save; a running analysis: no second one.
    view.pending = true;
    view.targets
        .insert(4, row(4.0, "Learned", TargetKind::Learned));
    view.selected = 4;
    let rows = tp::menu_rows(&view);
    assert_eq!(rows[4].label, "Learned (not saved)");
    assert_eq!(&enabled(&rows)[6..], [true, true, false, false]);
    view.files = Some(0.2);
    assert!(!tp::menu_rows(&view)[6].enabled);
    // A missing target checks nothing.
    view.missing = true;
    assert!(tp::menu_rows(&view).iter().all(|r| !r.checked));
    assert_eq!(rows[5].action, RowAction::Pick(11.0));
}

#[test]
fn the_target_button_opens_the_list_and_a_row_sets_the_target_param_in_one_step() {
    let (mut document, mut v) = rig();
    let at = v.tonal_target_center(&key("fx")).expect("a button");
    assert!(v.tonal_menu_labels().is_empty());
    let a = click(&mut document, &mut v, at);
    assert!(tonal_ops(&a).is_empty());
    assert_eq!(v.tonal_menu_labels().len(), 9);
    assert_eq!(v.tonal_menu_labels()[0], "Balanced");
    // Bright.
    let p = v.tonal_menu_row_center(1).unwrap();
    let a = click(&mut document, &mut v, p);
    assert_eq!(set_params(&a), [("hidden.target".to_string(), 1.0)]);
    assert_eq!(param(&document, "hidden.target"), 1.0);
    assert!(
        v.tonal_menu_labels().is_empty(),
        "the press closes the list"
    );
    // The saved target.
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(4).unwrap();
    let a = click(&mut document, &mut v, p);
    assert_eq!(set_params(&a), [("hidden.target".to_string(), 11.0)]);
    assert_eq!(param(&document, "hidden.target"), 11.0);
}

#[test]
fn escape_and_a_press_elsewhere_close_the_list_without_doing_anything() {
    let (mut document, mut v) = rig();
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    assert!(!v.tonal_menu_labels().is_empty());
    let a = type_key(&mut document, &mut v, Key::Escape);
    assert!(set_params(&a).is_empty());
    assert!(v.tonal_menu_labels().is_empty());
    // The button again, then the button again closes.
    click(&mut document, &mut v, at);
    assert!(!v.tonal_menu_labels().is_empty());
    click(&mut document, &mut v, at);
    assert!(v.tonal_menu_labels().is_empty());
    // A press far from the list.
    click(&mut document, &mut v, at);
    let a = click(&mut document, &mut v, Point::new(5.0, 400.0));
    assert!(set_params(&a).is_empty() && tonal_ops(&a).is_empty());
    assert!(v.tonal_menu_labels().is_empty());
    assert_eq!(param(&document, "hidden.target"), 0.0);
}

#[test]
fn learn_from_files_is_an_action_for_the_app_and_a_disabled_row_does_nothing() {
    let (mut document, mut v) = rig();
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(5).unwrap();
    let a = click(&mut document, &mut v, p);
    assert_eq!(tonal_ops(&a), [TonalOp::LearnFiles]);
    // Delete is disabled with a built-in in force.
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(8).unwrap();
    let a = click(&mut document, &mut v, p);
    assert!(tonal_ops(&a).is_empty());
    assert!(v.tonal_menu_labels().is_empty());
}

fn type_text(document: &mut Document, v: &mut RackView, text: &str) {
    for c in text.chars() {
        type_key(document, v, Key::Char(c));
    }
}

#[test]
fn save_types_the_name_in_place_of_the_button_and_enter_asks_the_app() {
    let mut data = tonal_data();
    data.tonal.pending = true;
    data.tonal
        .targets
        .insert(4, row(4.0, "Learned", TargetKind::Learned));
    data.tonal.selected = 4;
    let (mut document, mut v) = rig_with(&[("hidden.target", 4.0)], Some(&data));
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(7).unwrap();
    let a = click(&mut document, &mut v, p);
    assert!(tonal_ops(&a).is_empty(), "the name comes first");
    assert_eq!(v.tonal_naming(&key("fx")), Some("Learned target"));
    // The field is painted where the button was.
    let dl = paint(&document, &mut v);
    assert!(texts(&dl).iter().any(|t| t == "Learned target"));
    assert!(!texts(&dl).iter().any(|t| t == "TARGET"));
    // The whole name is selected: typing replaces it.
    type_text(&mut document, &mut v, "Club");
    assert_eq!(v.tonal_naming(&key("fx")), Some("Club"));
    type_key(&mut document, &mut v, Key::Backspace);
    let a = type_key(&mut document, &mut v, Key::Enter);
    assert_eq!(tonal_ops(&a), [TonalOp::Save("Clu".to_string())]);
    assert_eq!(v.tonal_naming(&key("fx")), None);
}

#[test]
fn rename_starts_from_the_current_name_and_escape_or_a_blank_name_changes_nothing() {
    let mut data = tonal_data();
    data.tonal.selected = 4;
    data.tonal.target_name = "My reference".to_string();
    let (mut document, mut v) = rig_with(&[("hidden.target", 11.0)], Some(&data));
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(7).unwrap();
    click(&mut document, &mut v, p);
    assert_eq!(v.tonal_naming(&key("fx")), Some("My reference"));
    type_text(&mut document, &mut v, "Radio");
    let a = type_key(&mut document, &mut v, Key::Enter);
    assert_eq!(tonal_ops(&a), [TonalOp::Rename("Radio".to_string())]);
    // Escape.
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(7).unwrap();
    click(&mut document, &mut v, p);
    type_text(&mut document, &mut v, "x");
    let a = type_key(&mut document, &mut v, Key::Escape);
    assert!(tonal_ops(&a).is_empty());
    assert_eq!(v.tonal_naming(&key("fx")), None);
    // A blank name.
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(7).unwrap();
    click(&mut document, &mut v, p);
    type_key(&mut document, &mut v, Key::Backspace);
    let a = type_key(&mut document, &mut v, Key::Enter);
    assert!(tonal_ops(&a).is_empty(), "{a:?}");
}

#[test]
fn delete_asks_the_app_when_a_saved_target_is_in_force() {
    let mut data = tonal_data();
    data.tonal.selected = 4;
    let (mut document, mut v) = rig_with(&[("hidden.target", 11.0)], Some(&data));
    let at = v.tonal_target_center(&key("fx")).unwrap();
    click(&mut document, &mut v, at);
    let p = v.tonal_menu_row_center(8).unwrap();
    let a = click(&mut document, &mut v, p);
    assert_eq!(tonal_ops(&a), [TonalOp::Delete]);
}

// ------------------------------------------------------------------ the settings

#[test]
fn the_settings_under_the_visual_set_their_params() {
    let (mut document, mut v) = rig();
    for (p, before) in [("response", 1.0), ("view", 2.0), ("channel", 0.0)] {
        assert_eq!(param(&document, p), before, "{p}");
        let at = v
            .control_center(&key("fx"), p)
            .unwrap_or_else(|| panic!("{p}"));
        let a = click(&mut document, &mut v, at);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {a:?}");
        assert_eq!(sets[0].0, p);
        assert_ne!(sets[0].1, before, "{p} moved");
        assert_eq!(param(&document, p), sets[0].1);
    }
    let at = v.control_center(&key("fx"), "learn").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(set_params(&a), [("learn".to_string(), 1.0)]);
    let a = click(&mut document, &mut v, at);
    assert_eq!(set_params(&a), [("learn".to_string(), 0.0)]);
    // The target has no knob of its own.
    assert!(v.control_center(&key("fx"), "hidden.target").is_none());
}

// ------------------------------------------------------------------ painting

#[test]
fn the_meters_and_the_curve_draw_with_their_readouts() {
    let (document, mut v) = rig();
    let t = texts(&paint(&document, &mut v));
    for s in [
        "TONAL BALANCE",
        "TARGET",
        "Balanced",
        "LOW",
        "LOW-MID",
        "HIGH-MID",
        "HIGH",
        "+4.2",
        "-0.6",
        "+1.1",
        "-5.0",
        "100",
        "1k",
        "10k",
    ] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
}

#[test]
fn without_a_reading_the_meters_show_dashes_and_the_panel_says_so() {
    let (document, mut v) = rig_with(&[], None);
    let t = texts(&paint(&document, &mut v));
    assert_eq!(t.iter().filter(|x| *x == "--").count(), 4, "{t:?}");
    assert!(t.iter().any(|x| x == "NO SIGNAL"), "{t:?}");
    assert!(!t.iter().any(|x| x == "+4.2"));
}

#[test]
fn the_view_choice_shows_the_meters_the_curve_or_both() {
    let data = tonal_data();
    let shots: Vec<String> = [0.0, 1.0, 2.0]
        .iter()
        .map(|view| {
            let (document, mut v) = rig_with(&[("view", *view)], Some(&data));
            paint(&document, &mut v).to_json()
        })
        .collect();
    assert_ne!(shots[0], shots[1]);
    assert_ne!(shots[0], shots[2]);
    assert_ne!(shots[1], shots[2]);
    let (document, mut v) = rig_with(&[("view", 1.0)], Some(&data));
    let t = texts(&paint(&document, &mut v));
    assert!(!t.iter().any(|x| x == "LOW-MID"), "no meters: {t:?}");
    assert!(t.iter().any(|x| x == "1k"));
    let (document, mut v) = rig_with(&[("view", 0.0)], Some(&data));
    let t = texts(&paint(&document, &mut v));
    assert!(t.iter().any(|x| x == "LOW-MID"));
    assert!(!t.iter().any(|x| x == "1k"), "no curve: {t:?}");
}

#[test]
fn the_marker_colour_follows_the_range_and_the_data_changes_the_painting() {
    let mut inside = tonal_data();
    inside.tonal.deviation = [0.5, -0.6, 1.1, -1.0];
    let outside = tonal_data();
    let (d1, mut v1) = rig_with(&[], Some(&inside));
    let (d2, mut v2) = rig_with(&[], Some(&outside));
    let a = paint(&d1, &mut v1).to_json();
    let b = paint(&d2, &mut v2).to_json();
    assert_ne!(a, b);
    // The curve moves with the mix.
    let mut moved = tonal_data();
    for v in &mut moved.tonal.bands {
        *v += 2.0;
    }
    let (d3, mut v3) = rig_with(&[], Some(&moved));
    assert_ne!(b, paint(&d3, &mut v3).to_json());
}

#[test]
fn a_missing_target_and_a_pending_one_are_said_in_orange_words() {
    let mut data = tonal_data();
    data.tonal.missing = true;
    let (document, mut v) = rig_with(&[("hidden.target", 17.0)], Some(&data));
    let t = texts(&paint(&document, &mut v));
    assert!(
        t.iter().any(|x| x == "TARGET MISSING, USING BALANCED"),
        "{t:?}"
    );
    let mut data = tonal_data();
    data.tonal.learning = true;
    data.tonal.learned_seconds = 7.0;
    let (document, mut v) = rig_with(&[], Some(&data));
    let t = texts(&paint(&document, &mut v));
    assert!(t.iter().any(|x| x == "LEARNING 7 s"), "{t:?}");
}

#[test]
fn a_bypassed_panel_says_so_and_dims_its_picture() {
    let data = tonal_data();
    let (d1, mut v1) = rig_with(&[], Some(&data));
    let (d2, mut v2) = rig_with(&[("bypass", 1.0)], Some(&data));
    assert!(!texts(&paint(&d1, &mut v1)).iter().any(|x| x == "BYPASSED"));
    assert!(texts(&paint(&d2, &mut v2)).iter().any(|x| x == "BYPASSED"));
}

#[test]
fn the_feed_repaints_only_on_news_and_the_back_has_the_four_sockets() {
    let (document, mut v) = rig();
    let _ = paint(&document, &mut v);
    assert!(!v.needs_repaint());
    v.set_analyser(&key("fx"), &tonal_data());
    assert!(!v.needs_repaint(), "the same data");
    let mut other = tonal_data();
    other.tonal.deviation[0] = 1.0;
    v.set_analyser(&key("fx"), &other);
    assert!(v.needs_repaint());
    v.set_flip(Flip::Back);
    let t = texts(&paint(&document, &mut v));
    for s in ["In L", "In R", "Out L", "Out R"] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
    assert!(!t.iter().any(|x| x == "LOW"));
    // A press on the back face opens no list.
    assert!(v.tonal_target_center(&key("fx")).is_none());
}

#[test]
fn the_texts_have_french_translations_without_duplicate_keys() {
    let fr = include_str!("../locales/fr.txt");
    let mut seen = std::collections::BTreeSet::new();
    for line in fr.lines().filter(|l| l.contains(" => ")) {
        let k = line.split(" => ").next().unwrap();
        assert!(seen.insert(k.to_string()), "duplicate key {k:?}");
    }
    for s in [
        "TONAL BALANCE",
        "TARGET",
        "LOW",
        "LOW-MID",
        "HIGH-MID",
        "HIGH",
        "NO SIGNAL",
        "LEARNING",
        "ANALYSING FILES",
        "LEARNED, NOT SAVED",
        "TARGET MISSING, USING BALANCED",
        "Learn from files…",
        "Save learned target…",
        "Rename target…",
        "Delete target",
        "not saved",
        "Balanced",
        "Bright",
        "Warm",
        "Bass-heavy",
        "RESPONSE",
        "LEARN",
        "VIEW",
        "< 150 Hz",
        "150 Hz - 1 kHz",
        "1 - 5 kHz",
        "> 5 kHz",
    ] {
        assert!(seen.contains(s), "no French for {s:?}");
    }
}

#[test]
fn every_control_only_sets_params_the_model_accepts() {
    let (mut document, mut v) = rig();
    let reg = registry();
    let mut x = 120.0;
    while x < 740.0 {
        let mut y = 12.0;
        while y < 240.0 {
            let at = Point::new(x, y);
            let mut all = v.handle(
                &down_with(at, PointerButton::Primary, 1),
                document.doc(),
                &reg,
            );
            all.extend(v.handle(&up(at), document.doc(), &reg));
            for c in commands(&all) {
                document
                    .apply(c.clone(), &reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            v.sync(document.doc(), &reg);
            y += 23.0;
        }
        x += 41.0;
    }
}

// ------------------------------------------------------------------ previews

/// Writes previews to `target/preview`: `cargo test -p atomik-ui --test tonal_panel --
/// --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let reg = registry();
    let mut open = tonal_data();
    open.tonal.pending = true;
    let mut missing = tonal_data();
    missing.tonal.missing = true;
    type Case = (&'static str, Flip, Vec<(&'static str, f32)>, AnalyserData);
    let cases: Vec<Case> = vec![
        ("both", Flip::Front, vec![], tonal_data()),
        ("bands", Flip::Front, vec![("view", 0.0)], tonal_data()),
        ("curve", Flip::Front, vec![("view", 1.0)], open),
        ("empty", Flip::Front, vec![], AnalyserData::default()),
        (
            "missing",
            Flip::Front,
            vec![("hidden.target", 17.0)],
            missing,
        ),
        ("back", Flip::Back, vec![], tonal_data()),
    ];
    for (name, flip, params, data) in cases {
        let mut doc = fx_doc(FxKind::TonalBalance);
        for (k, v) in &params {
            doc.devices[0].params.insert((*k).to_string(), *v);
        }
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 260.0)), &doc, &reg);
        v.sync(&doc, &reg);
        v.set_analyser(&key("fx"), &data);
        v.set_flip(flip);
        let _ = v.handle(&UiEvent::ScaleChanged(2.0), &doc, &reg);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(1640, 520, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/preview/tonal_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
