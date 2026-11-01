//! The reference rack (3 devices, 4 cables) recorded as a display list at 1× and 2×,
//! front then back, compared with checked-in goldens. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test snapshot`.
//!
//! The recorder stores text as strings and anchor positions (no glyphs) and rounds numbers to
//! 0.01, so the goldens do not depend on installed fonts or the platform.

mod support;

use atomik_ui::kurbo::Size;
use atomik_ui::{DisplayList, Flip, RackView, Theme, UiEvent};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn record(scale: f64) -> String {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 1000.0)), &doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(scale), &doc, &reg);
    v.set_selected(Some(key("delay")));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    v.set_flip(Flip::Back);
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

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

#[test]
fn reference_rack_1x_matches_golden() {
    check("reference_rack_1x.json", &record(1.0));
}

#[test]
fn reference_rack_2x_matches_golden() {
    check("reference_rack_2x.json", &record(2.0));
}

#[test]
fn recording_is_deterministic_and_complete() {
    let a = record(1.0);
    assert_eq!(a, record(1.0));
    for text in [
        "Poly",
        "Synth",
        "Stereo Delay",
        "Line",
        "Cutoff",
        "Feedback",
        "AUDIO IN",
        "CV OUT",
    ] {
        assert!(
            a.contains(&format!("\"text\":\"{text}\"")),
            "missing {text}"
        );
    }
    // Four cables on the back: each draws one shadow, one body and one gloss cubic.
    assert_eq!(a.matches("\"cubic\"").count(), 4 * 3);
}
