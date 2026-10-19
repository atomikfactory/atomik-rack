//! Display-list goldens of the sample editor: an empty sampler, four layered zones with
//! one missing asset (the second zone selected), and a selected forward-looped zone with a
//! crossfade while files hover over the map. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test sample_snapshot`.

mod support;

use atomik_model::RackDoc;
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, SampleEditorView, UiEvent};
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

fn paint(v: &mut SampleEditorView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn record_empty() -> DisplayList {
    let (doc, reg) = (sampler_doc(Vec::new()), registry());
    let mut v = editor(&doc, &reg);
    paint(&mut v, &doc)
}

fn record_layered() -> DisplayList {
    let (doc, reg) = (sampler_doc(layered_zones()), registry());
    let mut v = editor(&doc, &reg);
    v.set_selected_zone(Some(1));
    paint(&mut v, &doc)
}

fn record_looped() -> DisplayList {
    let (doc, reg) = (sampler_doc(vec![looped_zone()]), registry());
    let mut v = editor(&doc, &reg);
    let hover = UiEvent::FileHover {
        pos: Point::new(v.key_center_x(64), v.velocity_y(90)),
        paths: vec!["a.wav".into()],
    };
    let _ = v.handle(&hover, &doc, &reg);
    paint(&mut v, &doc)
}

#[test]
fn empty_sampler_matches_golden() {
    check("sample_editor_empty_1x.json", &record_empty().to_json());
}

#[test]
fn layered_zones_with_a_missing_asset_match_golden() {
    check("sample_editor_layered_1x.json", &record_layered().to_json());
}

#[test]
fn selected_looped_zone_with_crossfade_matches_golden() {
    check("sample_editor_looped_1x.json", &record_looped().to_json());
}

#[test]
fn recordings_are_deterministic_and_complete() {
    assert_eq!(record_layered().to_json(), record_layered().to_json());
    assert_eq!(record_looped().to_json(), record_looped().to_json());
    let t = record_layered();
    let texts = t.texts();
    for s in [
        "SAMPLE EDITOR",
        "ZONES",
        "127",
        "64",
        "1",
        "C-1",
        "C4",
        "OFF",
        "FWD",
        "PING",
        "GATE",
        "ONE-SHOT",
        "ROOT",
        "XFADE",
        "lost.wav",
    ] {
        assert!(texts.contains(&s), "missing {s:?}: {texts:?}");
    }
    let l = record_looped();
    assert!(l.texts().contains(&"Drop 1 file at E4"));
}
