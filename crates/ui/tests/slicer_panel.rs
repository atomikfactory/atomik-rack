//! The loop slicer's rack panel: overview strip with the slice count, the
//! sensitivity knob (view state), the Slice, To track and Edit buttons, opening the slice editor,
//! drops, the device knobs, the back sockets, and display-list goldens of both faces. Other
//! devices' panels are covered by their own goldens (they must not change).
//! Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test slicer_panel`.

mod support;

use atomik_model::{RackDoc, ZoneDoc};
use atomik_ui::autolayout::SLICER_PARAMS;
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{DisplayList, Flip, Key, Modifiers, RackView, SliceMode, Theme, UiAction, UiEvent};
use std::path::PathBuf;
use support::slicer::*;
use support::song::{down_mods, press};
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

fn slicer_only(zones: Vec<ZoneDoc>) -> RackDoc {
    let mut doc = slicer_doc(zones);
    doc.devices.retain(|d| d.key == key("slicer"));
    doc.cables.clear();
    doc
}

fn rack(zones: Vec<ZoneDoc>) -> (RackDoc, RackView) {
    let (doc, reg) = (slicer_only(zones), registry());
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 700.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (doc, v)
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point, clicks: u8) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down_mods(p, Modifiers::NONE, clicks), doc, &reg);
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

fn without_select(a: Vec<UiAction>) -> Vec<UiAction> {
    a.into_iter()
        .filter(|x| !matches!(x, UiAction::Select(_)))
        .collect()
}

// ------------------------------------------------------------------ front

#[test]
fn the_panel_shows_the_overview_and_the_buttons() {
    let (doc, mut v) = rack(Vec::new());
    let t = texts_of(&mut v, &doc);
    for s in [
        "LOOP",
        "No loop",
        "drop a loop here",
        "SLICE",
        "TO TRACK",
        "EDIT",
        "SENS 50%",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(!t.iter().any(|x| x == "SAMPLES"), "no sample strip: {t:?}");
    let (doc, mut v) = rack(sliced_zones(&MARKERS));
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "4 slices"), "{t:?}");
    let (doc, mut v) = rack(vec![ZoneDoc::new(LOOP, LOOP_FRAMES)]);
    // A single zone on a non-slice key is not a slice: still "No loop".
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "No loop"), "{t:?}");
}

#[test]
fn every_device_knob_is_on_the_panel() {
    let (_, v) = rack(sliced_zones(&MARKERS));
    let panel = v.device_rect(&key("slicer")).unwrap();
    for p in SLICER_PARAMS {
        let c = v
            .control_center(&key("slicer"), p)
            .unwrap_or_else(|| panic!("no control for {p}"));
        assert!(panel.contains(v.view_to_rack(c)), "{p} outside the panel");
    }
    assert_eq!(SLICER_PARAMS.len(), 12);
}

#[test]
fn the_edit_button_and_a_double_click_on_the_strip_open_the_slice_editor() {
    let (doc, mut v) = rack(sliced_zones(&MARKERS));
    let edit = v.edit_button_center(&key("slicer")).unwrap();
    let a = without_select(click(&mut v, &doc, edit, 1));
    assert_eq!(a, vec![UiAction::OpenSliceEditor(key("slicer"))]);
    let strip = v.slice_strip_center(&key("slicer")).unwrap();
    let a = without_select(click(&mut v, &doc, strip, 2));
    assert_eq!(a, vec![UiAction::OpenSliceEditor(key("slicer"))]);
    // A single click on the strip opens nothing.
    assert!(without_select(click(&mut v, &doc, strip, 1)).is_empty());
}

#[test]
fn a_double_click_on_the_panel_body_does_not_open_the_sample_editor() {
    let (doc, mut v) = rack(sliced_zones(&MARKERS));
    let r = v.device_rect(&key("slicer")).unwrap();
    let body = v.rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
    let a = click(&mut v, &doc, body, 2);
    assert!(
        !a.iter().any(|x| matches!(
            x,
            UiAction::OpenSampleEditor(_) | UiAction::OpenSliceEditor(_)
        )),
        "{a:?}"
    );
}

#[test]
fn the_slice_button_emits_transients_at_the_panel_sensitivity() {
    let (doc, mut v) = rack(sliced_zones(&MARKERS));
    let k = key("slicer");
    assert_eq!(v.slice_sensitivity(&k), Some(50));
    let b = v.slice_button_center(&k).unwrap();
    let a = without_select(click(&mut v, &doc, b, 1));
    assert_eq!(
        a,
        vec![UiAction::SliceLoop {
            device: k.clone(),
            mode: SliceMode::Transients { sensitivity: 50 }
        }]
    );
    // Drag the knob up 25 px: +20 %, no command.
    let knob = v.sensitivity_knob_center(&k).unwrap();
    let reg = registry();
    let mut a = v.handle(&down(knob), &doc, &reg);
    a.extend(v.handle(&mv(knob + Vec2::new(0.0, -10.0)), &doc, &reg));
    a.extend(v.handle(&mv(knob + Vec2::new(0.0, -25.0)), &doc, &reg));
    a.extend(v.handle(&up(knob + Vec2::new(0.0, -25.0)), &doc, &reg));
    assert!(without_select(a).is_empty());
    assert_eq!(v.slice_sensitivity(&k), Some(70));
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "SENS 70%"), "{t:?}");
    let a = without_select(click(&mut v, &doc, b, 1));
    assert_eq!(
        a,
        vec![UiAction::SliceLoop {
            device: k.clone(),
            mode: SliceMode::Transients { sensitivity: 70 }
        }]
    );
    // Double-click resets, Esc cancels a drag, the setter clamps.
    let _ = click(&mut v, &doc, knob, 2);
    assert_eq!(v.slice_sensitivity(&k), Some(50));
    let _ = v.handle(&down(knob), &doc, &reg);
    let _ = v.handle(&mv(knob + Vec2::new(0.0, -30.0)), &doc, &reg);
    assert_ne!(v.slice_sensitivity(&k), Some(50));
    let _ = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(v.slice_sensitivity(&k), Some(50));
    v.set_slice_sensitivity(&k, 250);
    assert_eq!(v.slice_sensitivity(&k), Some(100));
}

#[test]
fn to_track_emits_slices_to_track() {
    let (doc, mut v) = rack(sliced_zones(&MARKERS));
    let k = key("slicer");
    let b = v.to_track_button_center(&k).unwrap();
    let a = without_select(click(&mut v, &doc, b, 1));
    assert_eq!(a, vec![UiAction::SlicesToTrack { device: k }]);
}

#[test]
fn slice_and_to_track_do_nothing_without_a_loop() {
    let (doc, mut v) = rack(Vec::new());
    let k = key("slicer");
    let b = v.slice_button_center(&k).unwrap();
    assert!(without_select(click(&mut v, &doc, b, 1)).is_empty());
    let b = v.to_track_button_center(&k).unwrap();
    assert!(without_select(click(&mut v, &doc, b, 1)).is_empty());
}

#[test]
fn files_dropped_on_the_panel_import_into_the_slicer_without_a_key() {
    let (doc, mut v) = rack(Vec::new());
    let reg = registry();
    let k = key("slicer");
    let on = v.slice_strip_center(&k).unwrap();
    let paths = vec![PathBuf::from("loop.wav")];
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert_eq!(v.drop_hover(), Some(&k));
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: on,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths,
            device: k,
            at_key: None
        }]
    );
    assert_eq!(v.drop_hover(), None);
}

#[test]
fn other_devices_have_no_slicer_extras() {
    let doc = reference_doc();
    let reg = registry();
    let v = view(&doc, &reg);
    let synth = key("synth");
    assert!(v.slice_button_center(&synth).is_none());
    assert!(v.to_track_button_center(&synth).is_none());
    assert!(v.slice_strip_center(&synth).is_none());
    assert!(v.slice_sensitivity(&synth).is_none());
    // The sampler keeps its own strip and editor.
    let doc = {
        let mut d = slicer_doc(Vec::new());
        d.devices
            .push(device("sampler", "atomik.sampler", "Sampler"));
        d
    };
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 2000.0)), &doc, &reg);
    v.sync(&doc, &reg);
    let edit = v.edit_button_center(&key("sampler")).unwrap();
    let a = without_select(click(&mut v, &doc, edit, 1));
    assert_eq!(a, vec![UiAction::OpenSampleEditor(key("sampler"))]);
    assert!(v.slice_button_center(&key("sampler")).is_none());
}

// ------------------------------------------------------------------ back

#[test]
fn the_back_sockets_are_reachable_and_inside_the_panel() {
    let (_, mut v) = rack(sliced_zones(&MARKERS));
    v.set_flip(Flip::Back);
    let panel = v.device_rect(&key("slicer")).unwrap();
    let mut centres = Vec::new();
    for n in ["notes_in", "pitch_cv_in", "out_l", "out_r"] {
        let c = v
            .socket_center(&sref("slicer", n))
            .unwrap_or_else(|| panic!("socket {n} has no jack on the back"));
        assert!(panel.contains(v.view_to_rack(c)), "{n} outside the panel");
        centres.push(c);
    }
    for (i, a) in centres.iter().enumerate() {
        for b in &centres[i + 1..] {
            assert!(a.distance(*b) > 30.0, "jacks overlap: {a:?} {b:?}");
        }
    }
    // The front-only extras are not on the back.
    assert!(v.slice_button_center(&key("slicer")).is_none());
}

// ------------------------------------------------------------------ goldens

fn record(zones: Vec<ZoneDoc>, sens: Option<u8>, flip: Flip, hover: bool) -> String {
    let (doc, mut v) = rack(zones);
    if let Some(s) = sens {
        v.set_slice_sensitivity(&key("slicer"), s);
    }
    v.set_flip(flip);
    if hover {
        let pos = v.slice_strip_center(&key("slicer")).unwrap();
        let _ = v.handle(
            &UiEvent::FileHover {
                pos,
                paths: vec![PathBuf::from("a.wav")],
            },
            &doc,
            &registry(),
        );
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.to_json()
}

#[test]
fn the_empty_front_panel_matches_golden() {
    check(
        "slicer_panel_empty_1x.json",
        &record(Vec::new(), None, Flip::Front, false),
    );
}

#[test]
fn the_sliced_front_panel_matches_golden() {
    check(
        "slicer_panel_front_1x.json",
        &record(sliced_zones(&MARKERS), Some(70), Flip::Front, false),
    );
}

#[test]
fn the_front_panel_with_a_drop_hover_matches_golden() {
    check(
        "slicer_panel_drop_1x.json",
        &record(Vec::new(), None, Flip::Front, true),
    );
}

#[test]
fn the_back_panel_matches_golden() {
    check(
        "slicer_panel_back_1x.json",
        &record(sliced_zones(&MARKERS), None, Flip::Back, false),
    );
}
