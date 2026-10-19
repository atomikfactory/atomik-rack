//! The plugin parameter pane: module grouping, search, the virtual
//! list over 2 048 params, the per-param widgets (slider, stepper, toggle), gesture-merged
//! edits from `PLUGIN_GESTURE_MERGE_BASE`, display texts from the app, pins, the automate
//! shortcut, MAP TO MACRO inside a scope and locked driven params, display-list goldens
//! `plugin_params_*`, the GUI gesture merger (one gesture, one undo step), and a property test
//! that no gesture sequence makes the model refuse a command.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plugin_params`.

mod support;

use atomik_model::{
    Command, ContainerCommand, Document, MacroTarget, PLUGIN_GESTURE_MERGE_BASE, PluginCommand,
    RackDoc,
};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::plugin_params::Hit;
use atomik_ui::{
    DisplayList, Key, Modifiers, PluginGestureMerger, PluginParamsView, PointerButton, Theme,
    UiAction, UiEvent,
};
use proptest::prelude::*;
use support::container::*;
use support::plugin::*;
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);

fn pane_on(doc: &RackDoc, device: &str) -> PluginParamsView {
    let reg = registry();
    let mut v = PluginParamsView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(key(device));
    v.sync(doc, &reg);
    v
}

fn feed(v: &mut PluginParamsView, doc: &RackDoc, e: UiEvent) -> Vec<UiAction> {
    v.handle(&e, doc, &registry())
}

fn click_at(v: &mut PluginParamsView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let mut out = feed(v, doc, down(p));
    out.extend(feed(v, doc, up(p)));
    out
}

fn double_click_at(v: &mut PluginParamsView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let mut out = feed(
        v,
        doc,
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 2,
        },
    );
    out.extend(feed(v, doc, up(p)));
    out
}

fn press(v: &mut PluginParamsView, doc: &RackDoc, k: Key) -> Vec<UiAction> {
    feed(
        v,
        doc,
        UiEvent::Key {
            key: k,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
    )
}

fn type_text(v: &mut PluginParamsView, doc: &RackDoc, text: &str) {
    for ch in text.chars() {
        let _ = press(v, doc, Key::Char(ch));
    }
}

fn apply_all(d: &mut Document, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), &registry())
                    .unwrap_or_else(|e| panic!("{c:?}: {e}"));
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
}

fn set_params(actions: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                param,
                value,
                merge,
                ..
            } => Some((param, value, merge)),
            _ => None,
        })
        .collect()
}

fn value_of(d: &Document, device: &str, param: &str) -> Option<f32> {
    d.doc().resolve(&key(device))?.params.get(param).copied()
}

fn center(r: Rect) -> Point {
    r.center()
}

// ------------------------------------------------------------------ list, groups, search

#[test]
fn opens_on_the_device_and_groups_the_params_by_module_in_plugin_order() {
    let d = plugin_rack();
    let v = pane_on(d.doc(), "synth");
    assert!(v.is_present());
    assert_eq!(v.device(), Some(&key("synth")));
    assert_eq!(v.total_count(), 12);
    assert_eq!(v.shown_count(), 12);
    assert_eq!(v.visible_headers(), ["Osc", "Filter", "Env"]);
    assert_eq!(
        v.visible_ids(),
        (10..=21).collect::<Vec<u32>>(),
        "rows keep the plugin's own order inside the groups"
    );
    // Rows are in the list rectangle and in order.
    let list = v.list_rect();
    let mut last = f64::MIN;
    for id in v.visible_ids() {
        let r = v.row_rect(id).unwrap();
        assert!(
            r.y0 >= list.y0 && r.y1 <= list.y1,
            "row {id} is inside the list"
        );
        assert!(r.y0 > last);
        last = r.y0;
    }
}

#[test]
fn the_search_field_owns_the_keyboard_while_the_pane_shows_a_plugin() {
    // every letter, digit and Space is typed into the search, none is a command.
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    assert!(v.is_text_owner());
    type_text(&mut v, d.doc(), "o 1");
    assert_eq!(v.query(), "o 1");
    assert!(
        !PluginParamsView::new(Theme::dark()).is_text_owner(),
        "nothing open, nothing to type into"
    );
}

#[test]
fn a_plugin_without_modules_has_no_headers() {
    let d = plugin_rack();
    let v = pane_on(d.doc(), "fx");
    assert!(v.visible_headers().is_empty());
    assert_eq!(v.visible_ids(), [1, 2, 3]);
}

#[test]
fn search_filters_on_name_module_and_key_and_escape_clears_then_closes() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    type_text(&mut v, d.doc(), "FILTER");
    assert_eq!(v.query(), "FILTER");
    assert_eq!(v.visible_ids(), [14, 15, 16, 17]);
    assert_eq!(v.visible_headers(), ["Filter"]);
    assert_eq!(v.shown_count(), 4);
    // The key `p19` matches too.
    for _ in 0..6 {
        let _ = press(&mut v, d.doc(), Key::Backspace);
    }
    type_text(&mut v, d.doc(), "p19");
    assert_eq!(v.visible_ids(), [19]);
    // A name part.
    for _ in 0..3 {
        let _ = press(&mut v, d.doc(), Key::Backspace);
    }
    type_text(&mut v, d.doc(), "mode");
    assert_eq!(v.visible_ids(), [17]);
    // No match: an empty list says so.
    type_text(&mut v, d.doc(), "zzz");
    assert!(v.visible_ids().is_empty());
    assert_eq!(v.shown_count(), 0);
    let dl = {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &registry());
        dl
    };
    assert!(dl.texts().iter().any(|t| t.contains("No parameters match")));
    // Escape clears the query first, then closes the pane.
    let acts = press(&mut v, d.doc(), Key::Escape);
    assert!(!acts.contains(&UiAction::ClosePluginParams));
    assert_eq!(v.query(), "");
    assert_eq!(v.shown_count(), 12);
    let acts = press(&mut v, d.doc(), Key::Escape);
    assert!(acts.contains(&UiAction::ClosePluginParams));
}

#[test]
fn the_pane_states_what_undo_covers() {
    let d = plugin_rack();
    let v = pane_on(d.doc(), "synth");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    assert!(
        dl.texts().iter().any(|t| t.contains("not undoable")),
        "{:?}",
        dl.texts()
    );
}

#[test]
fn the_close_button_closes() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let acts = {
        let p = center(v.close_rect());
        click_at(&mut v, d.doc(), p)
    };
    assert!(acts.contains(&UiAction::ClosePluginParams));
    assert_eq!(v.hit(center(v.close_rect())), Hit::Close);
}

#[test]
fn a_vanished_device_makes_the_pane_not_present() {
    let mut d = plugin_rack();
    let reg = registry();
    let mut v = pane_on(d.doc(), "synth");
    apply(&mut d, Command::RemoveDevice { key: key("synth") });
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.is_present());
}

// ------------------------------------------------------------------ the virtual list

#[test]
fn a_2048_param_list_shows_only_the_rows_in_view_and_scrolls_to_the_end() {
    let d = big_rack();
    let mut v = pane_on(d.doc(), "big");
    assert_eq!(v.total_count(), 2048);
    let first = v.visible_ids();
    assert!(
        (10..=40).contains(&first.len()),
        "a viewport of rows, not 2048: {}",
        first.len()
    );
    assert_eq!(first[0], 0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    assert!(
        dl.len() < 3_000,
        "painting is bounded by the viewport: {}",
        dl.len()
    );
    // Scroll to the end with the keyboard and the wheel.
    let _ = press(&mut v, d.doc(), Key::End);
    let last = v.visible_ids();
    assert_eq!(*last.last().unwrap(), 2047);
    let _ = press(&mut v, d.doc(), Key::Home);
    assert_eq!(v.visible_ids()[0], 0);
    let acts = feed(
        &mut v,
        d.doc(),
        UiEvent::Wheel {
            delta: Vec2::new(0.0, -3.0),
            pos: Point::new(20.0, 300.0),
            modifiers: Modifiers::NONE,
        },
    );
    assert!(commands(&acts).is_empty());
    assert!(v.scroll() > 0.0);
    assert!(v.visible_ids()[0] > 0);
}

#[test]
fn searching_a_big_table_finds_a_module_and_a_key() {
    let d = big_rack();
    let mut v = pane_on(d.doc(), "big");
    type_text(&mut v, d.doc(), "group 3");
    // Group 3 holds params 192..=255 (and nothing else matches "group 3" except group 30..39).
    assert_eq!(v.visible_headers()[0], "Group 3");
    assert_eq!(v.visible_ids()[0], 192);
    for _ in 0..7 {
        let _ = press(&mut v, d.doc(), Key::Backspace);
    }
    type_text(&mut v, d.doc(), "p2047");
    assert_eq!(v.visible_ids(), [2047]);
    assert_eq!(v.shown_count(), 1);
}

// ------------------------------------------------------------------ the widgets

fn slider_drag(v: &mut PluginParamsView, d: &Document, id: u32, dx: f64) -> Vec<UiAction> {
    let s = v.slider_rect(id).unwrap();
    let from = center(s);
    let mut out = feed(v, d.doc(), down(from));
    out.extend(feed(v, d.doc(), mv(Point::new(from.x + dx / 2.0, from.y))));
    out.extend(feed(v, d.doc(), mv(Point::new(from.x + dx, from.y))));
    out.extend(feed(v, d.doc(), up(Point::new(from.x + dx, from.y))));
    out
}

#[test]
fn a_slider_drag_is_one_merged_gesture_of_set_params_on_the_plugins_own_key() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let w = v.slider_rect(10).unwrap().width();
    let acts = slider_drag(&mut v, &d, 10, w / 4.0);
    let sets = set_params(&acts);
    assert!(!sets.is_empty());
    let merge = sets[0].2.expect("a drag merges");
    assert!(merge >= PLUGIN_GESTURE_MERGE_BASE);
    for (param, value, m) in &sets {
        assert_eq!(param, "p10");
        assert_eq!(*m, Some(merge), "one merge id for the whole drag");
        assert!((0.0..=1.0).contains(value));
    }
    let last = sets.last().unwrap().1;
    assert!(
        (last - 0.25).abs() < 0.02,
        "a quarter of the width is a quarter of 0..1: {last}"
    );
    assert_eq!(
        acts.iter()
            .filter(|a| matches!(a, UiAction::EndMerge))
            .count(),
        1,
        "ends with EndMerge"
    );
    apply_all(&mut d, &acts);
    assert!(value_of(&d, "synth", "p10").is_some());
    // One undo step undoes the whole drag.
    d.undo(&registry()).unwrap();
    assert_eq!(value_of(&d, "synth", "p10"), None);
    // A second drag gets a new merge id.
    let acts2 = slider_drag(&mut v, &d, 10, w / 4.0);
    assert_ne!(set_params(&acts2)[0].2, Some(merge));
}

#[test]
fn shift_makes_a_slider_drag_fine_and_the_range_is_the_params_own() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    // p14 ranges 20..20000.
    let s = v.slider_rect(14).unwrap();
    let from = center(s);
    let mut out = feed(&mut v, d.doc(), down(from));
    out.extend(feed(
        &mut v,
        d.doc(),
        mv_with(
            Point::new(from.x + s.width() / 2.0, from.y),
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        ),
    ));
    out.extend(feed(&mut v, d.doc(), up(from)));
    let last = set_params(&out).last().unwrap().1;
    // Half the width with Shift is a twentieth of the range (fine divisor), from the default 20.
    assert!(last > 20.0 && last < 20.0 + 0.1 * 19980.0, "{last}");
}

#[test]
fn escape_during_a_drag_restores_the_press_value_and_ends_the_merge() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let s = v.slider_rect(12).unwrap();
    let from = center(s);
    let mut out = feed(&mut v, d.doc(), down(from));
    out.extend(feed(
        &mut v,
        d.doc(),
        mv(Point::new(from.x + s.width() / 3.0, from.y)),
    ));
    assert!(v.is_dragging());
    out.extend(press(&mut v, d.doc(), Key::Escape));
    assert!(!v.is_dragging());
    assert!(out.contains(&UiAction::EndMerge));
    assert!(
        !out.contains(&UiAction::ClosePluginParams),
        "Esc cancelled the drag only"
    );
    apply_all(&mut d, &out);
    let v12 = value_of(&d, "synth", "p12").unwrap_or(0.0);
    assert!(v12.abs() < 1e-6, "back to the start value: {v12}");
    // The release after the cancel does nothing.
    assert!(commands(&feed(&mut v, d.doc(), up(from))).is_empty());
}

#[test]
fn a_double_click_resets_a_slider_to_the_default() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    apply(
        &mut d,
        Command::SetParam {
            key: key("synth"),
            param: "p14".into(),
            value: 5000.0,
            merge: None,
        },
    );
    v.sync(d.doc(), &registry());
    let acts = {
        let p = center(v.slider_rect(14).unwrap());
        double_click_at(&mut v, d.doc(), p)
    };
    let sets = set_params(&acts);
    assert_eq!(
        sets.last().map(|s| (s.0.as_str(), s.1, s.2)),
        Some(("p14", 20.0, None))
    );
}

#[test]
fn a_stepped_param_has_a_stepper_that_clamps_and_a_zero_one_param_a_toggle() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    // p17: 0..=3 stepped.
    assert!(v.slider_rect(17).is_none(), "no slider on a stepped param");
    let inc = center(v.inc_rect(17).unwrap());
    let dec = center(v.dec_rect(17).unwrap());
    let acts = click_at(&mut v, d.doc(), inc);
    assert_eq!(
        set_params(&acts),
        vec![("p17".to_string(), 1.0, None)],
        "one step up, one command"
    );
    apply_all(&mut d, &acts);
    v.sync(d.doc(), &registry());
    // Down twice clamps at the minimum.
    let acts = click_at(&mut v, d.doc(), dec);
    assert_eq!(set_params(&acts), vec![("p17".to_string(), 0.0, None)]);
    apply_all(&mut d, &acts);
    v.sync(d.doc(), &registry());
    let acts = click_at(&mut v, d.doc(), dec);
    assert!(set_params(&acts).is_empty(), "already at the minimum");
    // Up beyond the maximum is clamped.
    for _ in 0..5 {
        let acts = click_at(&mut v, d.doc(), inc);
        apply_all(&mut d, &acts);
        v.sync(d.doc(), &registry());
    }
    assert_eq!(value_of(&d, "synth", "p17"), Some(3.0));
    // p18 is 0/1: a toggle.
    assert!(v.toggle_rect(18).is_some() && v.inc_rect(18).is_none());
    let acts = {
        let p = center(v.toggle_rect(18).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert_eq!(set_params(&acts), vec![("p18".to_string(), 1.0, None)]);
}

#[test]
fn the_wheel_over_a_widget_steps_it() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let wheel = |v: &mut PluginParamsView, p: Point, y: f64| {
        feed(
            v,
            d.doc(),
            UiEvent::Wheel {
                delta: Vec2::new(0.0, y),
                pos: p,
                modifiers: Modifiers::NONE,
            },
        )
    };
    let p = center(v.slider_rect(10).unwrap());
    let acts = wheel(&mut v, p, 1.0);
    let sets = set_params(&acts);
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].0, "p10");
    assert!(
        sets[0].1 > 0.0 && sets[0].1 <= 0.05,
        "a wheel notch is a small step: {}",
        sets[0].1
    );
    let p = center(v.inc_rect(17).unwrap());
    let acts = wheel(&mut v, p, 1.0);
    assert_eq!(set_params(&acts), vec![("p17".to_string(), 1.0, None)]);
    // Over the list background the wheel scrolls (nothing to scroll with 12 rows: no command).
    let acts = wheel(&mut v, Point::new(5.0, 300.0), 1.0);
    assert!(commands(&acts).is_empty());
}

// ------------------------------------------------------------------ display text

#[test]
fn display_texts_come_from_the_app_and_stale_ones_are_asked_for_again() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let wanted = v.take_text_requests();
    assert_eq!(
        wanted.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        (10..=21).collect::<Vec<u32>>()
    );
    assert!(v.take_text_requests().is_empty(), "asked once");
    v.set_texts(&[(10, 0.0, "0.00 dB".into()), (14, 20.0, "20 Hz".into())]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    let texts = dl.texts();
    assert!(
        texts.contains(&"0.00 dB") && texts.contains(&"20 Hz"),
        "{texts:?}"
    );
    // A change makes the text stale: the number is shown and the text is requested again.
    apply(
        &mut d,
        Command::SetParam {
            key: key("synth"),
            param: "p10".into(),
            value: 0.5,
            merge: None,
        },
    );
    v.sync(d.doc(), &registry());
    let wanted = v.take_text_requests();
    assert_eq!(wanted, vec![(10, 0.5)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    assert!(!dl.texts().contains(&"0.00 dB"));
    assert!(dl.texts().contains(&"0.500"), "{:?}", dl.texts());
}

#[test]
fn a_scroll_requests_only_the_newly_visible_params() {
    let d = big_rack();
    let mut v = pane_on(d.doc(), "big");
    let first = v.take_text_requests();
    assert!(first.len() < 60 && !first.is_empty());
    let acts = feed(
        &mut v,
        d.doc(),
        UiEvent::Wheel {
            delta: Vec2::new(0.0, -10.0),
            pos: Point::new(20.0, 300.0),
            modifiers: Modifiers::NONE,
        },
    );
    let asks: Vec<&UiAction> = acts
        .iter()
        .filter(|a| matches!(a, UiAction::PluginParamTexts { .. }))
        .collect();
    assert_eq!(
        asks.len(),
        1,
        "the scroll tells the app which texts it needs"
    );
    if let UiAction::PluginParamTexts { device, params } = asks[0] {
        assert_eq!(device, &key("big"));
        assert!(
            params
                .iter()
                .all(|(id, _)| !first.iter().any(|(f, _)| f == id))
        );
        assert!(!params.is_empty());
    }
}

// ------------------------------------------------------------------ pins and automate

#[test]
fn the_pin_toggle_edits_the_pins_one_command_each() {
    let mut d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    // Default pins are the first eight (10..=17): p18 is not pinned, p10 is.
    assert!(v.is_pinned(10) && !v.is_pinned(18));
    let acts = {
        let p = center(v.pin_rect(18).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPinned {
            device: key("synth"),
            pinned: vec![10, 11, 12, 13, 14, 15, 16, 17, 18]
        })]
    );
    apply_all(&mut d, &acts);
    v.sync(d.doc(), &registry());
    assert!(v.is_pinned(18));
    let acts = {
        let p = center(v.pin_rect(10).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert_eq!(
        commands(&acts),
        vec![Command::Plugin(PluginCommand::SetPinned {
            device: key("synth"),
            pinned: vec![11, 12, 13, 14, 15, 16, 17, 18]
        })]
    );
}

#[test]
fn pins_stop_at_sixteen_and_keep_at_least_one() {
    let mut b = big_rack();
    let sixteen: Vec<u32> = (0..16).collect();
    set_pins(&mut b, "big", &sixteen);
    let mut v = pane_on(b.doc(), "big");
    let id = *v.visible_ids().iter().find(|id| **id >= 16).unwrap();
    let acts = {
        let p = center(v.pin_rect(id).unwrap());
        click_at(&mut v, b.doc(), p)
    };
    assert!(
        commands(&acts).is_empty(),
        "a seventeenth pin is refused: {acts:?}"
    );
    assert!(v.toast().is_some_and(|t| t.contains("16")));
    // One pin left: it cannot be removed.
    let mut d = plugin_rack();
    set_pins(&mut d, "fx", &[2]);
    let mut v = pane_on(d.doc(), "fx");
    let acts = {
        let p = center(v.pin_rect(2).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert!(commands(&acts).is_empty());
    assert!(v.toast().is_some());
}

#[test]
fn the_automate_shortcut_names_the_device_and_the_param_id() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let acts = {
        let p = center(v.auto_rect(19).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert!(acts.contains(&UiAction::AutomatePluginParam {
        device: key("synth"),
        param: 19
    }));
    assert!(commands(&acts).is_empty());
}

// ------------------------------------------------------------------ macros in a scope

#[test]
fn map_to_macro_exists_only_inside_a_container_and_maps_through_a_menu() {
    let mut d = plugin_in_container();
    // At the top level there is no MAP button.
    let v = pane_on(d.doc(), "fx");
    assert!(v.map_rect(1).is_none());
    // Inside the container: MAP opens the macro menu.
    let mut v = pane_on(d.doc(), "layer/inner");
    let acts = {
        let p = center(v.map_rect(1).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert!(commands(&acts).is_empty());
    let rows = v.menu_rows();
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].0, "Map to macro 1");
    let acts = click_at(&mut v, d.doc(), rows[2].1);
    match commands(&acts).as_slice() {
        [
            Command::Container(ContainerCommand::SetMacro {
                container,
                index,
                target,
                merge,
            }),
        ] => {
            assert_eq!(container, &key("layer"));
            assert_eq!(*index, 2);
            assert_eq!(
                target,
                &MacroTarget {
                    device: key("inner"),
                    param: "p1".into(),
                    min: -24.0,
                    max: 24.0
                }
            );
            assert_eq!(*merge, None);
        }
        other => panic!("{other:?}"),
    }
    assert!(v.menu_rows().is_empty(), "the menu closed");
    apply_all(&mut d, &acts);
    v.sync(d.doc(), &registry());
    // Now it is driven here: the button reads M3, the slider is locked, the menu offers unmap.
    assert_eq!(v.driver_of(1), Some(2));
    let s = v.slider_rect(1).unwrap();
    let c = center(s);
    let mut out = feed(&mut v, d.doc(), down(c));
    out.extend(feed(&mut v, d.doc(), mv(Point::new(c.x + 40.0, c.y))));
    out.extend(feed(&mut v, d.doc(), up(c)));
    assert!(
        commands(&out).is_empty(),
        "a driven param is locked: {out:?}"
    );
    assert!(v.toast().is_some_and(|t| t.contains("macro 3")));
    let _ = {
        let p = center(v.map_rect(1).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    let rows = v.menu_rows();
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["Edit range", "Unmap"]
    );
    let acts = click_at(&mut v, d.doc(), rows[1].1);
    assert!(matches!(
        commands(&acts).as_slice(),
        [Command::Container(ContainerCommand::RemoveMacroTarget { container, index: 2, device, param })]
            if container == &key("layer") && device == &key("inner") && param == "p1"
    ));
    let _ = {
        let p = center(v.map_rect(1).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    let rows = v.menu_rows();
    let acts = click_at(&mut v, d.doc(), rows[0].1);
    assert!(acts.contains(&UiAction::OpenContainerEditor {
        device: key("layer"),
        macros: Some(2)
    }));
}

#[test]
fn a_press_elsewhere_closes_the_macro_menu_without_a_command() {
    let d = plugin_in_container();
    let mut v = pane_on(d.doc(), "layer/inner");
    let _ = {
        let p = center(v.map_rect(1).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    assert!(!v.menu_rows().is_empty());
    let acts = click_at(&mut v, d.doc(), Point::new(3.0, 3.0));
    assert!(commands(&acts).is_empty());
    assert!(v.menu_rows().is_empty());
}

// ------------------------------------------------------------------ the gesture merger

#[test]
fn a_gui_gesture_is_one_merged_undo_step_flagged_by_its_ids() {
    let mut d = plugin_rack();
    let mut m = PluginGestureMerger::new();
    let k = key("synth");
    assert!(m.begin(&k, 10).is_empty());
    let mut acts = Vec::new();
    for v in [0.1, 0.2, 0.4] {
        acts.extend(m.change(&k, 10, v));
    }
    acts.extend(m.end(&k, 10));
    let sets = set_params(&acts);
    assert_eq!(sets.len(), 3);
    let id = sets[0].2.unwrap();
    assert!(id >= PLUGIN_GESTURE_MERGE_BASE);
    assert!(sets.iter().all(|s| s.0 == "p10" && s.2 == Some(id)));
    assert!(acts.contains(&UiAction::EndMerge));
    apply_all(&mut d, &acts);
    assert_eq!(value_of(&d, "synth", "p10"), Some(0.4));
    d.undo(&registry()).unwrap();
    assert_eq!(
        value_of(&d, "synth", "p10"),
        None,
        "the whole gesture undone at once"
    );
}

#[test]
fn gestures_on_different_params_get_different_ids_and_a_change_outside_a_gesture_does_not_merge() {
    let mut m = PluginGestureMerger::new();
    let k = key("synth");
    m.begin(&k, 10);
    m.begin(&k, 11);
    let a = set_params(&m.change(&k, 10, 0.1))[0].2;
    let b = set_params(&m.change(&k, 11, 0.1))[0].2;
    assert!(a.is_some() && b.is_some() && a != b);
    assert!(m.end(&k, 10).contains(&UiAction::EndMerge));
    assert!(m.end(&k, 11).contains(&UiAction::EndMerge));
    assert!(m.end(&k, 11).is_empty(), "a second end is ignored");
    // A value without a gesture (a patch change) is a plain command, not merged.
    let c = set_params(&m.change(&k, 12, 0.5));
    assert_eq!(c[0].2, None);
    // The next gesture gets a fresh id.
    m.begin(&k, 10);
    assert_ne!(set_params(&m.change(&k, 10, 0.2))[0].2, a);
}

#[test]
fn the_pane_and_the_merger_never_share_a_merge_id() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    let w = v.slider_rect(10).unwrap().width();
    let pane_id = set_params(&slider_drag(&mut v, &d, 10, w / 4.0))[0]
        .2
        .unwrap();
    let mut m = PluginGestureMerger::new();
    let k = key("synth");
    m.begin(&k, 10);
    let gui_id = set_params(&m.change(&k, 10, 0.3))[0].2.unwrap();
    assert_ne!(pane_id, gui_id);
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_pane_matches_golden() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    v.set_texts(&[(10, 0.0, "0.0 %".into()), (19, 0.0, "0.00 s".into())]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("plugin_params_1x.json", &dl.to_json());
}

#[test]
fn the_search_result_matches_golden() {
    let d = plugin_rack();
    let mut v = pane_on(d.doc(), "synth");
    type_text(&mut v, d.doc(), "filter");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("plugin_params_search_1x.json", &dl.to_json());
}

#[test]
fn the_pane_inside_a_scope_matches_golden() {
    let mut d = plugin_in_container();
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: MacroTarget {
                device: key("inner"),
                param: "p1".into(),
                min: -24.0,
                max: 24.0,
            },
            merge: None,
        }),
    );
    let mut v = pane_on(d.doc(), "layer/inner");
    let _ = {
        let p = center(v.map_rect(2).unwrap());
        click_at(&mut v, d.doc(), p)
    };
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("plugin_params_scope_1x.json", &dl.to_json());
    assert!(dl.texts().contains(&"M1"), "{:?}", dl.texts());
}

#[test]
fn a_large_table_matches_golden() {
    let d = big_rack();
    let mut v = pane_on(d.doc(), "big");
    let _ = press(&mut v, d.doc(), Key::PageDown);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("plugin_params_big_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ property test

#[derive(Debug, Clone)]
enum Step {
    Down(f64, f64, u8, bool),
    Move(f64, f64, bool),
    Up(f64, f64),
    Wheel(f64, f64, f64),
    Char(char),
    Backspace,
    Escape,
    Enter,
    Home,
    End,
    PageDown,
    Undo,
    Redo,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0.0..1000.0f64, 0.0..560.0f64, 1u8..=2, any::<bool>())
            .prop_map(|(x, y, c, s)| Step::Down(x, y, c, s)),
        (0.0..1000.0f64, 0.0..560.0f64, any::<bool>()).prop_map(|(x, y, s)| Step::Move(x, y, s)),
        (0.0..1000.0f64, 0.0..560.0f64).prop_map(|(x, y)| Step::Up(x, y)),
        (0.0..1000.0f64, 0.0..560.0f64, -4.0..4.0f64).prop_map(|(x, y, d)| Step::Wheel(x, y, d)),
        proptest::char::range('a', 'z').prop_map(Step::Char),
        Just(Step::Backspace),
        Just(Step::Escape),
        Just(Step::Enter),
        Just(Step::Home),
        Just(Step::End),
        Just(Step::PageDown),
        Just(Step::Undo),
        Just(Step::Redo),
    ]
}

fn run(steps: &[Step], doc: Document, device: &str) {
    let reg = registry();
    let mut d = doc;
    let mut v = pane_on(d.doc(), device);
    for s in steps {
        let event = match s {
            Step::Down(x, y, c, shift) => UiEvent::PointerDown {
                pos: Point::new(*x, *y),
                button: PointerButton::Primary,
                modifiers: Modifiers {
                    shift: *shift,
                    ..Modifiers::NONE
                },
                clicks: *c,
            },
            Step::Move(x, y, shift) => mv_with(
                Point::new(*x, *y),
                Modifiers {
                    shift: *shift,
                    ..Modifiers::NONE
                },
            ),
            Step::Up(x, y) => up(Point::new(*x, *y)),
            Step::Wheel(x, y, dy) => UiEvent::Wheel {
                delta: Vec2::new(0.0, *dy),
                pos: Point::new(*x, *y),
                modifiers: Modifiers::NONE,
            },
            Step::Char(c) => key_ev(Key::Char(*c)),
            Step::Backspace => key_ev(Key::Backspace),
            Step::Escape => key_ev(Key::Escape),
            Step::Enter => key_ev(Key::Enter),
            Step::Home => key_ev(Key::Home),
            Step::End => key_ev(Key::End),
            Step::PageDown => key_ev(Key::PageDown),
            Step::Undo => key_ev(Key::Char('z')).with_ctrl(),
            Step::Redo => key_ev(Key::Char('y')).with_ctrl(),
        };
        let actions = v.handle(&event, d.doc(), &reg);
        for a in actions {
            match a {
                UiAction::Command(c) => {
                    d.apply(c.clone(), &reg)
                        .unwrap_or_else(|e| panic!("the model refused {c:?}: {e}"));
                }
                UiAction::EndMerge => d.end_merge(),
                UiAction::Undo => {
                    let _ = d.undo(&reg);
                }
                UiAction::Redo => {
                    let _ = d.redo(&reg);
                }
                _ => {}
            }
        }
        v.sync(d.doc(), &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
    }
}

fn key_ev(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

trait WithCtrl {
    fn with_ctrl(self) -> Self;
}

impl WithCtrl for UiEvent {
    fn with_ctrl(self) -> Self {
        match self {
            UiEvent::Key { key, pressed, .. } => UiEvent::Key {
                key,
                pressed,
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
            },
            other => other,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn no_gesture_sequence_makes_the_model_refuse_a_command(steps in proptest::collection::vec(step(), 1..60)) {
        run(&steps, plugin_rack(), "synth");
        run(&steps, plugin_in_container(), "layer/inner");
        run(&steps, big_rack(), "big");
    }
}
