//! The sampler's rack panel extras: the sample strip (zone and sample
//! counts, Edit button), opening the editor by button or double-click on the panel body, and
//! file drops onto the panel with a hover highlight. Other devices ignore all of it.

mod support;

use atomik_model::Command;
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{DisplayList, Flip, PointerButton, RackView, Theme, UiAction, UiEvent};
use std::path::PathBuf;
use support::sampler::*;
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

/// The reference rack plus the sampler (zones as given), as a synced view.
fn rack(zones: Vec<atomik_model::ZoneDoc>) -> (atomik_model::RackDoc, RackView) {
    let (doc, reg) = (sampler_doc(zones), registry());
    let v = view(&doc, &reg);
    (doc, v)
}

fn sampler_only(zones: Vec<atomik_model::ZoneDoc>) -> atomik_model::RackDoc {
    let mut doc = sampler_doc(zones);
    doc.devices.retain(|d| d.key == key("sampler"));
    doc.cables.clear();
    doc
}

fn texts_of(v: &mut RackView, doc: &atomik_model::RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

/// A point on a device's panel that is neither a control, the plate nor the Edit button: the
/// left rack ear.
fn body_point(v: &RackView, device: &str) -> Point {
    let r = v.device_rect(&key(device)).unwrap();
    v.rack_to_view(Point::new(r.x0 + 6.0, r.center().y))
}

#[test]
fn the_panel_shows_the_sample_strip_with_counts() {
    let (doc, mut v) = rack(Vec::new());
    let t = texts_of(&mut v, &doc);
    for s in ["SAMPLES", "No samples", "drop files here", "EDIT"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    let (doc, mut v) = rack(layered_zones());
    let t = texts_of(&mut v, &doc);
    for s in ["4 zones", "3 samples"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    let (doc, mut v) = rack(vec![atomik_model::ZoneDoc::new(S_A, FR_A)]);
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "1 zone") && t.iter().any(|x| x == "1 sample"));
}

#[test]
fn other_devices_have_no_sample_strip() {
    let doc = reference_doc();
    let reg = registry();
    let mut v = view(&doc, &reg);
    let t = texts_of(&mut v, &doc);
    assert!(!t.iter().any(|x| x == "SAMPLES" || x == "EDIT"), "{t:?}");
    assert!(v.edit_button_center(&key("synth")).is_none());
}

#[test]
fn the_generic_controls_stay_and_do_not_overlap_the_strip() {
    let (doc, v) = rack(Vec::new());
    for param in ["master.level", "env.amp.attack", "filter.cutoff", "voices"] {
        let c = v.control_center(&key("sampler"), param);
        assert!(c.is_some(), "{param} is still on the panel");
    }
    let edit = v.edit_button_center(&key("sampler")).unwrap();
    for param in ["master.level", "env.amp.attack", "filter.cutoff", "voices"] {
        let c = v.control_center(&key("sampler"), param).unwrap();
        assert!(
            (c.x - edit.x).abs() > 30.0,
            "{param} sits right of the sample strip"
        );
    }
    let _ = doc;
}

#[test]
fn the_edit_button_opens_the_editor() {
    let (doc, mut v) = rack(layered_zones());
    let reg = registry();
    let p = v.edit_button_center(&key("sampler")).unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    assert!(
        a.contains(&UiAction::OpenSampleEditor(key("sampler"))),
        "{a:?}"
    );
    assert!(commands(&a).is_empty());
    let _ = v.handle(&up(p), &doc, &reg);
    // The button is on the front only.
    v.set_flip(Flip::Back);
    assert!(v.edit_button_center(&key("sampler")).is_none());
}

#[test]
fn double_click_on_the_panel_body_opens_the_editor_but_a_single_click_only_selects() {
    let (doc, mut v) = rack(layered_zones());
    let reg = registry();
    let p = body_point(&v, "sampler");
    let a = v.handle(&down(p), &doc, &reg);
    assert!(!a.iter().any(|x| matches!(x, UiAction::OpenSampleEditor(_))));
    let _ = v.handle(&up(p), &doc, &reg);
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert!(
        a.contains(&UiAction::OpenSampleEditor(key("sampler"))),
        "{a:?}"
    );
    let _ = v.handle(&up(p), &doc, &reg);
    // Other devices: a double-click on the body does nothing special.
    let p = body_point(&v, "synth");
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert!(!a.iter().any(|x| matches!(x, UiAction::OpenSampleEditor(_))));
}

#[test]
fn double_click_on_a_control_still_resets_it_and_does_not_open_the_editor() {
    let (doc, mut v) = rack(layered_zones());
    let reg = registry();
    let p = v.control_center(&key("sampler"), "master.level").unwrap();
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert!(!a.iter().any(|x| matches!(x, UiAction::OpenSampleEditor(_))));
    assert!(
        commands(&a)
            .iter()
            .any(|c| matches!(c, Command::SetParam { param, .. } if param == "master.level"))
    );
}

#[test]
fn files_dropped_on_the_sampler_panel_become_an_import_and_the_panel_lights_up() {
    let (doc, mut v) = rack(Vec::new());
    let reg = registry();
    let paths = vec![PathBuf::from("kick.wav"), PathBuf::from("snare.wav")];
    let on = body_point(&v, "sampler");
    assert!(
        v.handle(
            &UiEvent::FileHover {
                pos: on,
                paths: paths.clone()
            },
            &doc,
            &reg
        )
        .is_empty()
    );
    assert_eq!(v.drop_hover(), Some(&key("sampler")));
    assert!(v.needs_repaint());
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
            paths: paths.clone(),
            device: key("sampler"),
            at_key: None
        }]
    );
    assert_eq!(v.drop_hover(), None);
    // Over a control of the sampler, too (the whole panel is the target).
    let c = v.control_center(&key("sampler"), "voices").unwrap();
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: c,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert_eq!(a.len(), 1);
}

#[test]
fn drops_elsewhere_or_without_files_are_ignored() {
    let (doc, mut v) = rack(Vec::new());
    let reg = registry();
    let paths = vec![PathBuf::from("kick.wav")];
    // On another device.
    let other = body_point(&v, "synth");
    let hover = UiEvent::FileHover {
        pos: other,
        paths: paths.clone(),
    };
    assert!(v.handle(&hover, &doc, &reg).is_empty());
    assert_eq!(v.drop_hover(), None);
    let drop = UiEvent::FileDrop {
        pos: other,
        paths: paths.clone(),
    };
    assert!(v.handle(&drop, &doc, &reg).is_empty());
    // On the interface strip and outside every panel.
    for pos in [Point::new(500.0, 3.0), Point::new(-30.0, -30.0)] {
        let drop = UiEvent::FileDrop {
            pos,
            paths: paths.clone(),
        };
        assert!(v.handle(&drop, &doc, &reg).is_empty(), "{pos:?}");
    }
    // No files on the sampler.
    let on = body_point(&v, "sampler");
    let none = UiEvent::FileDrop {
        pos: on,
        paths: Vec::new(),
    };
    assert!(v.handle(&none, &doc, &reg).is_empty());
    // Cancel clears a hover; the rear view is no target.
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert!(v.drop_hover().is_some());
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    v.set_flip(Flip::Back);
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: on,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert!(a.is_empty());
}

fn record(zones: Vec<atomik_model::ZoneDoc>, hover: bool) -> String {
    let (doc, reg) = (sampler_only(zones), registry());
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 700.0)), &doc, &reg);
    v.sync(&doc, &reg);
    if hover {
        let pos = body_point(&v, "sampler");
        let _ = v.handle(
            &UiEvent::FileHover {
                pos,
                paths: vec![PathBuf::from("a.wav")],
            },
            &doc,
            &reg,
        );
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

#[test]
fn empty_sampler_panel_matches_golden() {
    check("sampler_panel_empty_1x.json", &record(Vec::new(), false));
}

#[test]
fn sampler_panel_with_zones_matches_golden() {
    check(
        "sampler_panel_zones_1x.json",
        &record(layered_zones(), false),
    );
}

#[test]
fn sampler_panel_drop_hover_matches_golden() {
    check("sampler_panel_drop_1x.json", &record(layered_zones(), true));
}

#[test]
fn panel_recordings_are_deterministic_and_the_hover_adds_the_highlight() {
    assert_eq!(
        record(layered_zones(), false),
        record(layered_zones(), false)
    );
    assert_ne!(
        record(layered_zones(), true),
        record(layered_zones(), false)
    );
}

#[test]
fn the_sample_button_sits_beside_edit_starts_and_stops_a_sampling_and_right_click_picks_the_input()
{
    let (doc, mut v) = rack(layered_zones());
    let reg = registry();
    let edit = v.edit_button_center(&key("sampler")).unwrap();
    // The SAMPLE half is the right half of the same button row, 2 px gap included.
    let sample = Point::new(edit.x + 60.0, edit.y);
    let a = v.handle(&down(sample), &doc, &reg);
    assert!(
        a.contains(&UiAction::SampleInto {
            device: key("sampler"),
            at_key: None
        }),
        "{a:?}"
    );
    assert!(!a.contains(&UiAction::OpenSampleEditor(key("sampler"))));
    let _ = v.handle(&up(sample), &doc, &reg);
    let a = v.handle(
        &down_with(sample, atomik_ui::PointerButton::Secondary, 1),
        &doc,
        &reg,
    );
    assert!(a.contains(&UiAction::CycleSamplingInput), "{a:?}");
    let _ = v.handle(&up(sample), &doc, &reg);
    // The panel reads STOP while a sampling runs (its picture changes).
    v.set_sampling(Some(key("sampler")));
    v.set_sampling(None);
}
