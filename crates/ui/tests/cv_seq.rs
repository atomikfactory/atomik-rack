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
    d.undo().unwrap();
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

// ------------------------------------------------------------------ copy and paste with curves

use atomik_ui::SeqButton;
use atomik_ui::seq_editor::PatternButton;

/// The curves of pattern `index` of `seq` in `doc` (flat when none are stored).
fn curves_of(doc: &RackDoc, index: usize) -> CurvePatternDoc {
    doc.device(&key("seq"))
        .unwrap()
        .curves
        .as_ref()
        .map_or_else(CurvePatternDoc::default, |c| c.pattern(index))
}

fn click_at(v: &mut SeqEditorView, d: &Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    pointer(v, d, reg, p, false)
}

/// The command a paste emits: exactly one, applied to `d`.
fn applied_paste(actions: &[UiAction], d: &mut Document, reg: &Registry) -> Command {
    let commands: Vec<&Command> = actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 1, "one command, one undo step: {actions:?}");
    d.apply(commands[0].clone(), reg).unwrap();
    commands[0].clone()
}

#[test]
fn the_editor_copies_and_pastes_the_curves_with_the_pattern_as_one_step() {
    let doc = cv_doc();
    let reg = registry();
    let mut d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    let copy = v.pattern_button_center(PatternButton::Copy);
    assert!(click_at(&mut v, &d, &reg, copy).is_empty());
    assert_eq!(v.curve_clipboard(), Some(&curves_of(&doc, 0)));
    v.set_pattern_index(2);
    v.sync(d.doc(), &reg);
    let paste = v.pattern_button_center(PatternButton::Paste);
    let acts = click_at(&mut v, &d, &reg, paste);
    let command = applied_paste(&acts, &mut d, &reg);
    let Command::Batch(parts) = command else {
        panic!("a pattern and its curves go in one batch");
    };
    assert!(matches!(parts[0], Command::SetPattern { index: 2, .. }));
    assert!(matches!(parts[1], Command::SetCurves { index: 2, .. }));
    assert_eq!(curves_of(d.doc(), 2), curves_of(&doc, 0));
    assert_eq!(
        curves_of(d.doc(), 0),
        curves_of(&doc, 0),
        "the source stays"
    );
    d.undo().unwrap();
    assert_eq!(curves_of(d.doc(), 2), CurvePatternDoc::default());
    assert_eq!(
        d.doc().device(&key("seq")).unwrap().steps,
        doc.device(&key("seq")).unwrap().steps,
        "the steps come back with the curves"
    );
}

#[test]
fn pasting_a_copy_without_curves_leaves_the_curves_alone() {
    let doc = cv_doc();
    let reg = registry();
    let mut d = Document::new(doc.clone());
    // Copy from the step sequencer: no curves in the clipboard.
    let mut step = editor_of(&doc, &reg, "step");
    let copy = step.pattern_button_center(PatternButton::Copy);
    let _ = click_at(&mut step, &d, &reg, copy);
    assert_eq!(step.curve_clipboard(), None);
    // The same clipboard pasted in the CV sequencer (as the app mirrors it).
    let mut v = editor_of(&doc, &reg, "seq");
    v.set_clipboard(step.clipboard().cloned());
    v.set_curve_clipboard(step.curve_clipboard().cloned());
    v.set_pattern_index(1);
    v.sync(d.doc(), &reg);
    let paste = v.pattern_button_center(PatternButton::Paste);
    let acts = click_at(&mut v, &d, &reg, paste);
    let command = applied_paste(&acts, &mut d, &reg);
    assert!(matches!(command, Command::SetPattern { index: 1, .. }));
    assert_eq!(
        d.doc().device(&key("seq")).unwrap().curves,
        doc.device(&key("seq")).unwrap().curves
    );
}

#[test]
fn a_step_sequencer_does_not_copy_or_paste_curves() {
    let doc = cv_doc();
    let reg = registry();
    let d = Document::new(doc.clone());
    let mut cv = editor_of(&doc, &reg, "seq");
    let copy = cv.pattern_button_center(PatternButton::Copy);
    let _ = click_at(&mut cv, &d, &reg, copy);
    let mut step = editor_of(&doc, &reg, "step");
    step.set_clipboard(cv.clipboard().cloned());
    step.set_curve_clipboard(cv.curve_clipboard().cloned());
    step.set_pattern_index(2);
    step.sync(d.doc(), &reg);
    let paste = step.pattern_button_center(PatternButton::Paste);
    let acts = click_at(&mut step, &d, &reg, paste);
    assert!(set_curves(&acts).is_empty());
    assert!(
        acts.iter()
            .all(|a| matches!(a, UiAction::Command(Command::SetPattern { .. })))
    );
}

#[test]
fn pasting_onto_identical_steps_and_curves_does_nothing() {
    let doc = cv_doc();
    let reg = registry();
    let d = Document::new(doc.clone());
    let mut v = editor_of(&doc, &reg, "seq");
    let copy = v.pattern_button_center(PatternButton::Copy);
    let _ = click_at(&mut v, &d, &reg, copy);
    let paste = v.pattern_button_center(PatternButton::Paste);
    assert!(click_at(&mut v, &d, &reg, paste).is_empty());
    // Only the curves differ: only the curves are set.
    let mut other = doc.clone();
    let c = other
        .devices
        .iter_mut()
        .find(|d| d.key == key("seq"))
        .unwrap();
    c.curves.as_mut().unwrap().patterns[0].rows[0][0].value = 7;
    let d2 = Document::new(other.clone());
    v.sync(d2.doc(), &reg);
    let acts = click_at(&mut v, &d2, &reg, paste);
    assert!(matches!(
        acts.as_slice(),
        [UiAction::Command(Command::SetCurves { index: 0, .. })]
    ));
}

#[test]
fn the_rack_panel_copies_and_pastes_the_curves_with_the_pattern() {
    let doc = only("seq");
    let reg = registry();
    let mut d = Document::new(doc.clone());
    let mut v: RackView = view(&doc, &reg);
    let button = |v: &RackView, b: SeqButton| v.seq_button_center(&key("seq"), b).unwrap();
    let click = |v: &mut RackView, d: &Document, p: Point| {
        let mut a = v.handle(&down(p), d.doc(), &reg);
        a.extend(v.handle(&up(p), d.doc(), &reg));
        a
    };
    let p = button(&v, SeqButton::Copy);
    assert!(commands(&click(&mut v, &d, p)).is_empty());
    assert_eq!(v.pattern_curve_clipboard(), Some(&curves_of(&doc, 0)));
    let p = button(&v, SeqButton::Slot(2));
    let _ = click(&mut v, &d, p);
    let p = button(&v, SeqButton::Paste);
    let acts = click(&mut v, &d, p);
    let command = applied_paste(&acts, &mut d, &reg);
    assert!(
        matches!(command, Command::Batch(ref c) if c.len() == 2),
        "{command:?}"
    );
    assert_eq!(curves_of(d.doc(), 2), curves_of(&doc, 0));
    v.sync(d.doc(), &reg);
    // Copy on a pattern without curves of its own copies flat rows.
    let p = button(&v, SeqButton::Copy);
    let _ = click(&mut v, &d, p);
    assert_eq!(v.pattern_curve_clipboard(), Some(&curves_of(d.doc(), 2)));
}
