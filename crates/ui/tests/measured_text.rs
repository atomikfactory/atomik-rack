//! Labels are cut by what the text engine measures, so Cyrillic, CJK and wide Latin names are
//! neither clipped nor truncated early.

mod support;

use atomik_ui::paint::{fit_label_in, measure, tail_fit};
use atomik_ui::{Canvas, TextEngine};
use support::cpu::CpuCanvas;

const NAMES: [&str; 5] = [
    "Kick drum heavy 01",
    "WWWWWWWWWWWWWWWW",
    "Тяжёлый бас-барабан",
    "日本語のサンプル名前です",
    "混合 mixed Имя 名前",
];

fn canvas(text: &mut TextEngine) -> CpuCanvas<'_> {
    CpuCanvas::new(64, 32, text)
}

#[test]
fn a_cut_label_fits_and_one_more_character_would_not() {
    let mut text = TextEngine::new();
    let mut c = canvas(&mut text);
    for name in NAMES {
        let full = measure(&mut c, name, 11.0, 400);
        for share in [0.3, 0.5, 0.8] {
            let width = full * share;
            let cut = fit_label_in(&mut c, name, width, 11.0);
            assert!(
                measure(&mut c, &cut, 11.0, 400) <= width + 1e-6,
                "{name:?} at {width}: {cut:?} overflows"
            );
            assert!(cut.ends_with('.'), "{cut:?}");
            // Keeping one more character (and the closing dot) must not fit.
            let kept = cut.chars().count() - 1;
            if kept < name.chars().count() {
                let longer: String = name.chars().take(kept + 1).collect::<String>() + ".";
                assert!(
                    kept == 0 || measure(&mut c, &longer, 11.0, 400) > width,
                    "{name:?} at {width}: {cut:?} is cut too early"
                );
            }
        }
    }
}

#[test]
fn a_label_that_fits_is_left_alone() {
    let mut text = TextEngine::new();
    let mut c = canvas(&mut text);
    for name in NAMES {
        let full = measure(&mut c, name, 11.0, 400);
        assert_eq!(fit_label_in(&mut c, name, full + 0.5, 11.0), name);
    }
}

#[test]
fn a_very_narrow_room_keeps_one_character() {
    let mut text = TextEngine::new();
    let mut c = canvas(&mut text);
    assert_eq!(fit_label_in(&mut c, "日本語", 0.0, 11.0), "日.");
    assert_eq!(fit_label_in(&mut c, "", 0.0, 11.0), "");
}

#[test]
fn a_field_shows_the_end_of_its_text_that_fits() {
    let mut text = TextEngine::new();
    let mut c = canvas(&mut text);
    for name in NAMES {
        let full = measure(&mut c, name, 10.0, 700);
        let room = full * 0.5;
        let shown = tail_fit(&mut c, name, room, 10.0, 700);
        assert!(name.ends_with(shown));
        assert!(measure(&mut c, shown, 10.0, 700) <= room + 1e-6);
        assert!(!shown.is_empty(), "{name:?}");
    }
}

#[test]
fn a_canvas_without_a_text_engine_falls_back_to_its_estimate() {
    let mut list = atomik_ui::DisplayList::new();
    let w = measure(&mut list as &mut dyn Canvas, "abcd", 10.0, 400);
    assert!((w - 4.0 * 10.0 * 0.58).abs() < 1e-9);
}
