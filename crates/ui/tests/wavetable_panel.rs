//! The wavetable synth's rack panel: layout, display-list goldens (front with the default
//! params, front with two other tables and positions, back), the frame displays following the
//! params, and the generic controls (table choice, position drag and wheel, tooltip).
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test wavetable_panel`.

mod support;

use atomik_model::RackDoc;
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::kurbo::{Rect, Vec2};
use atomik_ui::wavetable_panel::{WAVETABLE_TYPE, wavetable_label, wavetable_layout};
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn wavetable_only(params: &[(&str, f32)]) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("wt", WAVETABLE_TYPE, "Wavetable Synth");
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc.devices = vec![d];
    doc.cables.clear();
    doc
}

fn setup(params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let doc = wavetable_only(params);
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn json_of(v: &mut RackView, doc: &RackDoc) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

const OTHER: [(&str, f32); 4] = [
    ("osc1.table", 5.0),
    ("osc1.position", 0.5),
    ("osc2.table", 8.0),
    ("osc2.position", 1.0),
];

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    check("wavetable_panel_front_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_front_panel_with_other_tables_matches_golden() {
    let (doc, mut v) = setup(&OTHER);
    check("wavetable_panel_tables_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    v.set_flip(Flip::Back);
    check("wavetable_panel_back_1x.json", &json_of(&mut v, &doc));
}

// ------------------------------------------------------------------ the displays

#[test]
fn the_displays_name_the_table_and_the_position() {
    let (doc, mut v) = setup(&[]);
    let t = texts_of(&mut v, &doc);
    let has = |s: &str| t.iter().any(|x| x == s);
    // Default: osc 1 on saw>square at 0 %, osc 2 on pulse width at 25 %.
    assert!(has("saw>square") && has("0 %"), "{t:?}");
    assert!(has("pulse width") && has("25 %"), "{t:?}");
    assert!(has("TABLE 1") && has("TABLE 2"), "{t:?}");
    let (doc, mut v) = setup(&OTHER);
    let t = texts_of(&mut v, &doc);
    let has = |s: &str| t.iter().any(|x| x == s);
    assert!(has("vowels") && has("50 %"), "{t:?}");
    assert!(has("fm 1:2") && has("100 %"), "{t:?}");
}

#[test]
fn the_displays_follow_the_params() {
    let (doc, mut v) = setup(&[]);
    let base = json_of(&mut v, &doc);
    for change in [
        ("osc1.table", 3.0),
        ("osc1.position", 0.7),
        ("osc2.table", 6.0),
        ("osc2.position", 0.0),
    ] {
        let (doc, mut v) = setup(&[change]);
        assert_ne!(json_of(&mut v, &doc), base, "{change:?}");
    }
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_and_the_displays_overlap_nothing() {
    let reg = registry();
    let desc = reg.get(WAVETABLE_TYPE).unwrap().descriptor;
    let spec = wavetable_layout(desc);
    let mut placed: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    placed.sort_unstable();
    let mut want: Vec<&str> = desc.params.iter().map(|p| p.key).collect();
    want.sort_unstable();
    assert_eq!(placed, want, "every param is a control, once");
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());

    let panel = spec.wavetable.expect("wavetable panel");
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    for d in panel.displays {
        assert!(
            d.x0 >= content.x0 && d.x1 <= content.x1 && d.y0 >= 0.0 && d.y1 <= content.y1,
            "{d:?} outside {content:?}"
        );
        assert!(d.width() >= 100.0 && d.height() >= 36.0, "{d:?} too small");
        for c in &spec.front {
            let r = c.rect;
            let hit = d.x0 < r.x1 && r.x0 < d.x1 && d.y0 < r.y1 && r.y0 < d.y1;
            assert!(!hit, "{d:?} overlaps {r:?}");
        }
    }
    let [a, b] = panel.displays;
    assert!(a.y1 <= b.y0, "the displays are stacked: {a:?} {b:?}");
    assert!(spec.seq.is_none() && spec.modsrc.is_none() && spec.samples.is_none());
    // No control hangs out of the panel.
    for c in &spec.front {
        assert!(c.rect.y0 >= 0.0 && c.rect.y1 <= spec.height(), "{c:?}");
        assert!(c.rect.x1 <= content.x1 + 1e-6, "{c:?}");
    }
}

#[test]
fn every_label_is_short_enough_for_a_knob() {
    let reg = registry();
    let desc = reg.get(WAVETABLE_TYPE).unwrap().descriptor;
    for p in desc.params {
        if let Some(l) = wavetable_label(p.key) {
            assert!(l.len() <= 9, "{}: {l}", p.key);
        }
    }
    assert_eq!(wavetable_label("osc1.position"), None);
}

// ------------------------------------------------------------------ the controls

fn set_param(actions: &[UiAction]) -> Vec<(String, f32)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            atomik_model::Command::SetParam { param, value, .. } => Some((param, value)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_table_choice_opens_a_pick_list_with_every_table() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    let desc = reg.get(WAVETABLE_TYPE).unwrap().descriptor;
    for (param, table) in [("osc1.table", 5usize), ("osc2.table", 12)] {
        let names = match desc.param_by_key(param).unwrap().unit {
            atomik_core::Unit::Choice(n) => n,
            _ => panic!("{param} is a choice"),
        };
        assert_eq!(names.len(), 13, "twelve tables and the imported one");
        let at = v.choice_center(&key("wt"), param).unwrap();
        // The press opens the list and sets nothing.
        let a = v.handle(&down(at), &doc, &reg);
        assert!(set_param(&a).is_empty(), "{param}");
        let _ = v.handle(&up(at), &doc, &reg);
        let labels = v.choice_menu_labels();
        assert_eq!(labels.len(), names.len(), "{param}: a row per table");
        assert_eq!(labels[table], names[table]);
        assert_eq!(labels[12], "imported");
        // A press on a row sets that table and closes the list.
        let at_row = v.choice_menu_row_center(table).unwrap();
        let a = v.handle(&down(at_row), &doc, &reg);
        assert_eq!(set_param(&a), vec![(param.to_string(), table as f32)]);
        let _ = v.handle(&up(at_row), &doc, &reg);
        assert!(v.choice_menu_labels().is_empty(), "{param}: closed");
    }
}

#[test]
fn a_right_click_still_steps_the_table_back_and_the_short_choices_step_forward() {
    let (doc, mut v) = setup(&[("osc1.table", 4.0)]);
    let reg = registry();
    let at = v.control_center(&key("wt"), "osc1.table").unwrap();
    let a = v.handle(&down_with(at, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(set_param(&a), vec![("osc1.table".to_string(), 3.0)]);
    let _ = v.handle(&up(at), &doc, &reg);
    assert!(v.choice_menu_labels().is_empty());
    // The unison mode has two entries: a click steps.
    let at = v.control_center(&key("wt"), "unison.mode").unwrap();
    let a = v.handle(&down(at), &doc, &reg);
    assert_eq!(set_param(&a), vec![("unison.mode".to_string(), 1.0)]);
    let _ = v.handle(&up(at), &doc, &reg);
    assert!(v.choice_menu_labels().is_empty());
}

#[test]
fn only_the_table_choices_of_the_wavetable_synth_open_a_pick_list() {
    use atomik_ui::master_panel::is_pick_list;
    let reg = registry();
    let desc = reg.get(WAVETABLE_TYPE).unwrap().descriptor;
    let opens = |key: &str| is_pick_list(desc.param_by_key(key).unwrap());
    assert!(opens("osc1.table") && opens("osc2.table"));
    assert!(!opens("filter.mode") && !opens("lfo.wave") && !opens("unison.mode"));
}

#[test]
fn the_position_knob_drags_and_takes_the_wheel_with_a_tooltip() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    let c = v.control_center(&key("wt"), "osc1.position").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    let s = set_param(&a);
    assert_eq!(s[0].0, "osc1.position");
    assert!((s[0].1 - 0.1).abs() < 1e-3, "{s:?}");
    assert_eq!(v.tooltip().as_deref(), Some("Position  10 %"));
    let _ = v.handle(&up(c), &doc, &reg);
    assert_eq!(v.tooltip(), None);

    let c = v.control_center(&key("wt"), "osc2.position").unwrap();
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: c,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    let s = set_param(&a);
    assert_eq!(s[0].0, "osc2.position");
    assert!(s[0].1 > 0.25, "{s:?}");
}

#[test]
fn double_click_resets_a_knob_to_its_default() {
    let (doc, mut v) = setup(&[("unison.detune", 80.0)]);
    let reg = registry();
    let c = v.control_center(&key("wt"), "unison.detune").unwrap();
    let a = v.handle(&down_with(c, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(set_param(&a), vec![("unison.detune".to_string(), 20.0)]);
}

// ------------------------------------------------------------------ the live position

#[test]
fn the_displays_follow_the_published_position_and_fall_back_to_the_knobs() {
    let (doc, mut v) = setup(&[]);
    let idle = json_of(&mut v, &doc);
    // Nothing sounds: the device publishes -1 and the display is the knob's.
    v.set_device_meter_values(&key("wt"), &[-1.0, -1.0]);
    assert_eq!(json_of(&mut v, &doc), idle);
    // A voice sounds at another frame than the knobs say.
    v.set_device_meter_values(&key("wt"), &[0.6, -1.0]);
    let live = json_of(&mut v, &doc);
    assert_ne!(live, idle);
    v.set_device_meter_values(&key("wt"), &[0.6, 0.9]);
    let both = json_of(&mut v, &doc);
    assert_ne!(both, live, "oscillator 2 follows its own value");
    // The percentage stays the knob's.
    let t = texts_of(&mut v, &doc);
    assert!(
        t.iter().any(|x| x == "0 %") && t.iter().any(|x| x == "25 %"),
        "{t:?}"
    );
    // The published value equal to the knob is the same picture as idle.
    v.set_device_meter_values(&key("wt"), &[0.0, 0.25]);
    assert_eq!(json_of(&mut v, &doc), idle);
    // Back to idle.
    v.set_device_meter_values(&key("wt"), &[0.6, 0.9]);
    let _ = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("wt"), &[-1.0, -1.0]);
    assert_eq!(json_of(&mut v, &doc), idle);
}

#[test]
fn an_unchanged_or_tiny_move_of_the_published_position_asks_for_no_repaint() {
    let (doc, mut v) = setup(&[]);
    v.set_device_meter_values(&key("wt"), &[0.5, 0.5]);
    let _ = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("wt"), &[0.5, 0.5]);
    assert!(!v.needs_repaint());
    // Below the display's resolution.
    v.set_device_meter_values(&key("wt"), &[0.5004, 0.4996]);
    assert!(!v.needs_repaint());
    v.set_device_meter_values(&key("wt"), &[0.52, 0.5]);
    assert!(v.needs_repaint());
}

#[test]
fn the_imported_table_is_named_and_asks_for_a_sample_until_one_is_dropped() {
    let (doc, mut v) = setup(&[("osc1.table", 12.0)]);
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "imported"), "{t:?}");
    assert!(t.iter().any(|x| x == "drop a sample"), "{t:?}");
    // With a sample on the device the hint is gone.
    let mut doc = wavetable_only(&[("osc1.table", 12.0)]);
    doc.devices[0]
        .zones
        .push(atomik_model::ZoneDoc::new("a".repeat(64), 4_096));
    let reg = registry();
    let mut v = view(&doc, &reg);
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "imported"), "{t:?}");
    assert!(!t.iter().any(|x| x == "drop a sample"), "{t:?}");
    // The live position does not make the missing curve appear or panic.
    v.set_device_meter_values(&key("wt"), &[0.7, 0.2]);
    let _ = json_of(&mut v, &doc);
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::kurbo::Size;
    use atomik_ui::{TextEngine, Theme};
    use support::cpu::CpuCanvas;
    let reg = registry();
    let doc = wavetable_only(&OTHER);
    let mut text = TextEngine::new();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 620.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(1.5), &doc, &reg);
    for (name, flip) in [
        ("wavetable_panel", Flip::Front),
        ("wavetable_panel_back", Flip::Back),
    ] {
        rack.set_flip(flip);
        let mut canvas = CpuCanvas::new(1230, 930, &mut text);
        rack.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
