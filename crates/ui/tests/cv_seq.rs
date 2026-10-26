//! The CV sequencer in the UI: its rack panel (golden), the grid editor's two curve lanes
//! (shown only for the CV sequencer; press, drag, Alt-click slide, Esc cancel) and display-list
//! goldens. Every emitted command is applied to a real model `Document`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test cv_seq`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, CurveDoc, CurvePatternDoc, CurveStepDoc, Document, RackDoc};
use atomik_ui::kurbo::Point;
use atomik_ui::{
    DisplayList, Key, Modifiers, PointerButton, RackView, SeqEditorView, UiAction, UiEvent,
};
use std::path::PathBuf;
use support::seq::*;
use support::song::press;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const CV_TYPE: &str = "atomik.cv-sequencer";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn curve(value: i8, slide: bool) -> CurveStepDoc {
    CurveStepDoc { value, slide }
}

/// The reference rack plus a CV sequencer `seq` (3 patterns, curves on pattern 1) and a step
/// sequencer `step`.
fn cv_doc() -> RackDoc {
    let mut doc = reference_doc();
    let mut c = device("seq", CV_TYPE, "CV Sequencer");
    c.steps = Some(seq_patterns());
    let mut p = CurvePatternDoc::default();
    for i in 0..16usize {
        p.rows[0][i] = curve((i as i8) * 6 - 45, i % 4 == 1);
        p.rows[1][i] = curve(if i % 2 == 0 { 60 } else { -40 }, false);
    }
    c.curves = Some(CurveDoc { patterns: vec![p] });
    let mut s = device("step", SEQ_TYPE, "Step Sequencer");
    s.steps = Some(seq_patterns());
    doc.devices.insert(0, c);
    doc.devices.insert(1, s);
    doc
}

fn only(keep: &str) -> RackDoc {
    let mut doc = cv_doc();
    doc.devices.retain(|d| d.key == key(keep));
    doc.cables.clear();
    doc
}

fn editor_of(doc: &RackDoc, reg: &Registry, dev: &str) -> SeqEditorView {
    let mut v = SeqEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    v.open(key(dev));
    v.sync(doc, reg);
    v
}

fn paint(v: &mut SeqEditorView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn pointer(
    v: &mut SeqEditorView,
    d: &Document,
    reg: &Registry,
    p: Point,
    alt: bool,
) -> Vec<UiAction> {
    let m = Modifiers {
        alt,
        ..Modifiers::NONE
    };
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: m,
            clicks: 1,
        },
        d.doc(),
        reg,
    );
    out.extend(v.handle(&up(p), d.doc(), reg));
    out
}

fn set_curves(actions: &[UiAction]) -> Vec<(u8, CurvePatternDoc, Option<u64>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::SetCurves {
                index,
                curves,
                merge,
                ..
            }) => Some((*index, curves.clone(), *merge)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_cv_panel_matches_golden() {
    let doc = only("seq");
    let reg = registry();
    let mut v: RackView = view(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("cv_seq_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_cv_panel_is_three_units_with_the_curve_strips_and_every_param_placed() {
    let doc = only("seq");
    let reg = registry();
    let desc = reg.descriptor(CV_TYPE).unwrap();
    let spec = atomik_ui::seq_panel::cv_seq_layout(desc);
    assert_eq!(spec.height_units, 3);
    assert!(spec.seq.unwrap().curves.is_some());
    let mut placed: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    placed.sort_unstable();
    let mut want: Vec<&str> = desc
        .params
        .iter()
        .map(|p| p.key)
        .filter(|k| *k != "bypass")
        .collect();
    want.sort_unstable();
    assert_eq!(placed, want, "every param but the strip bypass has a knob");
    let _ = doc;
}

#[test]
fn seq_editor_shows_curve_lanes_only_for_the_cv_sequencer() {
    let doc = cv_doc();
    let reg = registry();
    let cv = editor_of(&doc, &reg, "seq");
    assert!(cv.has_curve_lanes());
    let step = editor_of(&doc, &reg, "step");
    assert!(!step.has_curve_lanes());
    // The lanes' labels are painted for the CV sequencer only.
    let mut cv = cv;
    let mut step = step;
    let texts = |dl: &DisplayList| {
        dl.texts()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert!(texts(&paint(&mut cv, &doc)).iter().any(|t| t == "CURVE A"));
    assert!(
        !texts(&paint(&mut step, &doc))
            .iter()
            .any(|t| t == "CURVE A")
    );
    // A press where the lane would be does nothing on the step sequencer.
    let d = Document::new(doc.clone());
    let p = cv.curve_point(0, 3, 50.0);
    assert!(set_curves(&pointer(&mut step, &d, &reg, p, false)).is_empty());
}

#[test]
fn the_cv_editor_matches_golden() {
    let doc = cv_doc();
    let reg = registry();
    let mut v = editor_of(&doc, &reg, "seq");
    check("seq_editor_cv_1x.json", &paint(&mut v, &doc).to_json());
}

#[test]
fn a_press_in_a_curve_lane_sets_the_value_and_a_drag_paints_columns() {
    let doc = cv_doc();
    let reg = registry();
    let mut d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    // Press at the top of step 2 of row B: +100.
    let p = v.curve_point(1, 2, 100.0);
    let down_ev = UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let mut acts = v.handle(&down_ev, d.doc(), &reg);
    // Drag right to step 4 at the zero line: columns 3 and 4 get 0.
    let to = v.curve_point(1, 4, 0.0);
    acts.extend(v.handle(
        &UiEvent::PointerMove {
            pos: to,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &reg,
    ));
    acts.extend(v.handle(&up(to), d.doc(), &reg));
    let sets = set_curves(&acts);
    assert_eq!(sets.len(), 2, "one command per change");
    assert!(
        sets.iter()
            .all(|(i, _, m)| *i == 0 && m.is_some() && *m == sets[0].2)
    );
    assert!(acts.iter().any(|a| matches!(a, UiAction::EndMerge)));
    for a in &acts {
        if let UiAction::Command(c) = a {
            d.apply(c.clone(), &reg).unwrap();
        }
    }
    d.end_merge();
    let rows = &d
        .doc()
        .device(&key("seq"))
        .unwrap()
        .curves
        .as_ref()
        .unwrap()
        .patterns[0]
        .rows;
    assert_eq!(rows[1][2].value, 100);
    assert_eq!((rows[1][3].value, rows[1][4].value), (0, 0));
    assert_eq!(rows[0][2].value, 2 * 6 - 45, "row A is untouched");
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc().device(&key("seq")).unwrap().curves,
        doc.device(&key("seq")).unwrap().curves,
        "one undo step"
    );
}

#[test]
fn alt_click_toggles_the_slide_of_a_step() {
    let doc = cv_doc();
    let reg = registry();
    let mut d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    let was = doc
        .device(&key("seq"))
        .unwrap()
        .curves
        .as_ref()
        .unwrap()
        .patterns[0]
        .rows[0][5]
        .slide;
    let p = v.curve_point(0, 5, 0.0);
    let acts = pointer(&mut v, &d, &reg, p, true);
    let sets = set_curves(&acts);
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].2, None, "a toggle is a single undo step");
    assert_eq!(sets[0].1.rows[0][5].slide, !was);
    assert_eq!(sets[0].1.rows[0][5].value, 5 * 6 - 45, "the value stays");
    for a in &acts {
        if let UiAction::Command(c) = a {
            d.apply(c.clone(), &reg).unwrap();
        }
    }
}

#[test]
fn escape_during_a_curve_drag_restores_the_pattern_of_the_press() {
    let doc = cv_doc();
    let reg = registry();
    let d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    let p = v.curve_point(0, 6, -100.0);
    let mut acts = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        d.doc(),
        &reg,
    );
    acts.extend(v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg));
    let sets = set_curves(&acts);
    assert_eq!(sets.len(), 2, "the edit, then the restore");
    assert_eq!(
        sets[1].1,
        doc.device(&key("seq"))
            .unwrap()
            .curves
            .as_ref()
            .unwrap()
            .patterns[0]
    );
    assert!(acts.iter().any(|a| matches!(a, UiAction::EndMerge)));
}

#[test]
fn a_cv_sequencer_without_stored_curves_edits_from_flat() {
    let mut doc = cv_doc();
    doc.devices
        .iter_mut()
        .find(|d| d.key == key("seq"))
        .unwrap()
        .curves = None;
    let reg = registry();
    let d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    let p = v.curve_point(0, 1, 50.0);
    let acts = pointer(&mut v, &d, &reg, p, false);
    let sets = set_curves(&acts);
    assert_eq!(sets[0].1.rows[0][1].value, 50);
    assert!(sets[0].1.rows[0][0].value == 0 && sets[0].1.rows[1].iter().all(|s| s.value == 0));
}
