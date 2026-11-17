//! The Plucked Strings rack panel: the six sections in order (STRING, PLUCK, COURSE, BODY,
//! OUTPUT, PLAY), every param and socket placed once inside the panel, the body choice as a
//! pick list, and the display-list goldens of the front (default and a muted bass) and the back.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plucked_panel`.

mod support;

use atomik_model::RackDoc;
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::plucked_panel::{PLUCKED_TYPE, plucked_layout};
use atomik_ui::{DisplayList, Flip, RackView};
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
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn setup(params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let mut doc = reference_doc();
    let mut d = device("pl", PLUCKED_TYPE, "Plucked Strings");
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc.devices = vec![d];
    doc.cables.clear();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn paint(v: &mut RackView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    check(
        "plucked_panel_front_1x.json",
        &paint(&mut v, &doc).to_json(),
    );
}

#[test]
fn the_front_panel_of_a_muted_bass_matches_golden() {
    let (doc, mut v) = setup(&[("body", 4.0), ("palm_mute", 0.8), ("course.mode", 2.0)]);
    check("plucked_panel_bass_1x.json", &paint(&mut v, &doc).to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    v.set_flip(Flip::Back);
    check("plucked_panel_back_1x.json", &paint(&mut v, &doc).to_json());
}

#[test]
fn the_sections_come_in_playing_order_with_their_labels() {
    let (doc, mut v) = setup(&[]);
    let list = paint(&mut v, &doc);
    let texts = list.texts();
    let at = |s: &str| {
        texts
            .iter()
            .position(|t| *t == s)
            .unwrap_or_else(|| panic!("no {s} in {texts:?}"))
    };
    let order: Vec<usize> = ["STRING", "PLUCK", "COURSE", "BODY", "OUTPUT", "PLAY"]
        .iter()
        .map(|s| at(s))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{texts:?}");
    for label in [
        "Decay",
        "Sustain",
        "Brightness",
        "Stiffness",
        "Palm mute",
        "Position",
        "Hardness",
        "Detune",
        "Body",
        "Mix",
        "Gain",
        "Width",
        "Sens",
        "Range",
    ] {
        at(label);
    }
    assert!(!texts.contains(&"MAIN"), "{texts:?}");
}

#[test]
fn every_param_and_socket_is_placed_once_inside_the_panel() {
    let reg = registry();
    let desc = reg.get(PLUCKED_TYPE).unwrap().descriptor;
    let spec = plucked_layout(desc);
    let mut placed: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    placed.sort_unstable();
    let mut want: Vec<&str> = desc.params.iter().map(|p| p.key).collect();
    want.sort_unstable();
    assert_eq!(placed, want);
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    assert_eq!(spec.front_sections.len(), 6);
    for c in &spec.front {
        assert!(
            c.rect.x0 >= CONTENT_X.0 && c.rect.x1 <= CONTENT_X.1,
            "{c:?}"
        );
        assert!(c.rect.y0 >= 0.0 && c.rect.y1 <= spec.height(), "{c:?}");
    }
    for (i, a) in spec.front.iter().enumerate() {
        for b in &spec.front[i + 1..] {
            let hit = a.rect.x0 < b.rect.x1
                && b.rect.x0 < a.rect.x1
                && a.rect.y0 < b.rect.y1
                && b.rect.y0 < a.rect.y1;
            assert!(!hit, "{a:?} overlaps {b:?}");
        }
    }
}

#[test]
fn the_body_choice_opens_a_pick_list_of_every_body() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    let at = v.choice_center(&key("pl"), "body").unwrap();
    let _ = v.handle(&down(at), &doc, &reg);
    let _ = v.handle(&up(at), &doc, &reg);
    let labels = v.choice_menu_labels();
    assert_eq!(labels.len(), atomik_dsp::pluck::body::BODY_NAMES.len());
    assert_eq!(labels[1], "Steel");
    assert_eq!(labels[9], "Resonator");
}
