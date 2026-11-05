//! Pixel goldens of the main panes, rasterised by the CPU canvas at 1x, 1.25x (hairlines) and 2x
//! and compared perceptually with the PNGs in `tests/pixel/` (see `support/pixel.rs` for the
//! tolerance). Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test pixel_goldens`; a failure leaves the
//! actual and diff images in `target/pixel-diff/`.
//!
//! The negative tests prove the comparison: a one pixel layout shift and a clipped label fail it.

mod support;

use atomik_ui::context_menu::{ContextMenu, Entry, MenuItemDef};
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::paint::{fit_label_in, measure};
use atomik_ui::{Align, Canvas, CanvasExt, ChannelMeters, Flip, TextRun, Theme, UiEvent};
use support::console::*;
use support::pixel::*;
use support::song::*;
use support::*;

/// 1x, 1.25x and 2x: the scales the goldens exist at.
const SCALES: [(f64, &str); 3] = [(1.0, "1x"), (1.25, "125x"), (2.0, "2x")];
/// The panes with thin lines are also checked at 1.25x.
const HAIRLINE: [f64; 3] = [1.0, 1.25, 2.0];
const PLAIN: [f64; 2] = [1.0, 2.0];

fn suffix(scale: f64) -> &'static str {
    SCALES
        .iter()
        .find(|(s, _)| (*s - scale).abs() < 1e-9)
        .expect("scale")
        .1
}

/// Checks every scale before failing, so one run writes the diff images of all of them.
fn each(scales: &[f64], name: &str, size: Size, paint: impl Fn(&mut dyn Canvas, f64)) {
    let mut failed = Vec::new();
    for &scale in scales {
        let image = render(size, scale, |c| paint(c, scale));
        let case = format!("{name}_{}", suffix(scale));
        if std::panic::catch_unwind(|| check(&case, &image)).is_err() {
            failed.push(case);
        }
    }
    assert!(failed.is_empty(), "pixel goldens differ: {failed:?}");
}

#[test]
fn the_transport_bar_matches_its_goldens() {
    let (doc, reg) = (song_doc(), registry());
    each(
        &HAIRLINE,
        "transport",
        Size::new(620.0, 44.0),
        |c, scale| {
            let mut v = transport_bar(&doc, &reg);
            v.set_scale(scale);
            v.paint(c, &doc, &reg);
        },
    );
}

#[test]
fn the_arrangement_matches_its_goldens() {
    let (doc, reg) = (song_doc(), registry());
    each(
        &PLAIN,
        "arrangement",
        Size::new(640.0, 260.0),
        |c, scale| {
            let mut v = arrangement(&doc, &reg);
            v.set_scale(scale);
            v.paint(c, &doc, &reg);
        },
    );
}

#[test]
fn the_piano_roll_matches_its_goldens() {
    let (doc, reg) = (song_doc(), registry());
    each(
        &HAIRLINE,
        "piano_roll",
        Size::new(520.0, 300.0),
        |c, scale| {
            let mut v = piano_roll(&doc, &reg);
            v.set_scale(scale);
            v.paint(c, &doc, &reg);
        },
    );
}

#[test]
fn the_console_matches_its_goldens() {
    let (doc, reg) = (console_doc(), console_registry());
    each(&PLAIN, "console", Size::new(560.0, 400.0), |c, scale| {
        let mut v = console_view(&doc, &reg);
        v.set_scale(scale);
        let m = ChannelMeters {
            out_peak_l: 0.5,
            out_peak_r: 0.8,
            out_rms_l: 0.25,
            out_rms_r: 0.4,
            gr: 0.0,
        };
        v.set_meters(&[(key("ch-bass"), m), (key("master"), m)]);
        v.set_frame_time(1.0);
        v.paint(c, &doc, &reg);
    });
}

#[test]
fn the_rack_matches_its_goldens() {
    let (doc, reg) = (reference_doc(), registry());
    each(&PLAIN, "rack", Size::new(560.0, 400.0), |c, scale| {
        let mut v = plain_view(&doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), &doc, &reg);
        v.set_flip(Flip::Front);
        v.set_selected(Some(key("delay")));
        v.paint(c, &doc, &reg);
    });
}

fn item(label: &str, shortcut: Option<&str>, enabled: bool, checked: Option<bool>) -> Entry {
    Entry::Item(MenuItemDef {
        command: "edit.noop".into(),
        arg: None,
        label: label.into(),
        shortcut: shortcut.map(str::to_owned),
        enabled,
        checked,
    })
}

#[test]
fn a_context_menu_matches_its_goldens() {
    each(&PLAIN, "context_menu", Size::new(300.0, 190.0), |c, _| {
        let entries = vec![
            item("Cut", Some("Ctrl+X"), true, None),
            item("Copy", Some("Ctrl+C"), true, None),
            item("Paste", Some("Ctrl+V"), false, None),
            Entry::Separator,
            item("Snap to Grid", None, true, Some(true)),
            item(
                "A very long label that has to be cut to fit the panel",
                None,
                true,
                None,
            ),
        ];
        let menu =
            ContextMenu::from_entries(entries, Point::new(12.0, 10.0), Size::new(300.0, 190.0));
        menu.paint(c, &Theme::dark());
    });
}

/// Labels cut to a width, drawn one under the other: the overflow cases.
fn overflow_rows(c: &mut dyn Canvas, names: &[&str], width: f64) {
    let theme = Theme::dark();
    c.fill_with(Rect::new(0.0, 0.0, 400.0, 400.0), theme.panel_top);
    for (i, name) in names.iter().enumerate() {
        let y = 22.0 + 26.0 * i as f64;
        c.stroke_with(
            Rect::new(10.5, y - 14.5, 10.5 + width, y + 6.5),
            1.0,
            theme.text_muted,
        );
        let cut = fit_label_in(c, name, width - 6.0, 12.0);
        c.text(&TextRun::new(cut, Point::new(13.0, y), 12.0, theme.text));
    }
}

#[test]
fn text_overflow_cases_match_their_goldens() {
    let names = [
        "Kick drum heavy 01",
        "WWWWWWWWWWWWWWWWWWWW",
        "iiiiiiiiiiiiiiiiiiiiiiiiiiii",
        "A fairly long track name here",
    ];
    each(&PLAIN, "text_overflow", Size::new(200.0, 110.0), |c, _| {
        overflow_rows(c, &names, 150.0);
    });
}

/// CJK glyphs come from whichever fallback font the machine has, so only the machine the
/// reference was made on (or an OS-pinned PNG) can compare them; others only render.
#[test]
fn a_cjk_string_matches_its_goldens() {
    let names = [
        "日本語のサンプル名前です",
        "混合 mixed 名前 test",
        "ステレオ・ベース",
    ];
    for scale in PLAIN {
        let image = render(Size::new(200.0, 84.0), scale, |c| {
            overflow_rows(c, &names, 150.0)
        });
        let pinned = std::env::consts::OS == REFERENCE_OS;
        let has_pin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(
                "tests/pixel/cjk_{}.{}.png",
                suffix(scale),
                std::env::consts::OS
            ))
            .exists();
        if pinned || has_pin {
            check(&format!("cjk_{}", suffix(scale)), &image);
        } else {
            assert!(image.rgba.iter().any(|b| *b != 0), "rendered something");
        }
    }
}

#[test]
fn the_goldens_are_small() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pixel");
    for e in std::fs::read_dir(dir).expect("tests/pixel").flatten() {
        let len = e.metadata().expect("metadata").len();
        assert!(len < 300_000, "{:?} is {len} bytes", e.file_name());
    }
}

// ------------------------------------------------------------------ the comparison itself

fn pane() -> Image {
    let (doc, reg) = (song_doc(), registry());
    render(Size::new(300.0, 120.0), 1.0, |c| {
        let mut v = arrangement(&doc, &reg);
        v.paint(c, &doc, &reg);
    })
}

#[test]
fn an_unchanged_image_passes_and_a_one_pixel_layout_shift_fails() {
    let golden = pane();
    assert!(compare(&golden, &pane(), Tolerance::SAME_OS).is_ok());
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0)] {
        let shifted = golden.shifted(dx, dy);
        let why = compare(&shifted, &golden, Tolerance::SAME_OS)
            .expect_err(&format!("a shift of ({dx}, {dy}) px passed"));
        println!("shift ({dx}, {dy}): {why}");
        // Even the tolerance for another OS's fonts refuses a shifted pane.
        assert!(compare(&shifted, &golden, Tolerance::OTHER_OS).is_err());
    }
}

#[test]
fn a_clipped_label_fails() {
    let golden = render(Size::new(200.0, 40.0), 1.0, |c| {
        overflow_rows(c, &["Kick drum heavy 01"], 150.0);
    });
    // The same row with the end of the label cut off (what a clip or a too early truncation does).
    let clipped = golden.painted((95, 8, 160, 30), [0x28, 0x2c, 0x34, 255]);
    let why = compare(&clipped, &golden, Tolerance::SAME_OS).expect_err("a clipped label passed");
    println!("clipped label: {why}");
    // A label cut by the width it is given draws differently from the full one.
    let full = render(Size::new(200.0, 40.0), 1.0, |c| {
        overflow_rows(c, &["Kick drum"], 150.0)
    });
    let cut = render(Size::new(200.0, 40.0), 1.0, |c| {
        overflow_rows(c, &["Kick drum"], 40.0)
    });
    assert!(compare(&cut, &full, Tolerance::SAME_OS).is_err());
}

#[test]
fn text_that_only_antialiases_differently_passes_the_other_os_tolerance() {
    let golden = pane();
    // Rounding noise: every channel of every pixel moves by 3.
    let mut noisy = golden.clone();
    for (i, b) in noisy.rgba.iter_mut().enumerate() {
        if i % 4 != 3 {
            *b = b.saturating_add(3);
        }
    }
    assert!(compare(&noisy, &golden, Tolerance::SAME_OS).is_ok());
    assert!(compare(&noisy, &golden, Tolerance::OTHER_OS).is_ok());
}

#[test]
fn an_image_of_another_size_fails() {
    let a = pane();
    let b = render(Size::new(10.0, 10.0), 1.0, |_| {});
    assert!(compare(&b, &a, Tolerance::OTHER_OS).is_err());
}

#[test]
fn png_round_trips() {
    let a = pane();
    assert_eq!(decode(&encode(&a)).unwrap(), a);
}

#[test]
fn the_measured_width_decides_the_cut() {
    // A guard on the helper the overflow goldens use: a cut label is never wider than asked.
    let mut text = atomik_ui::TextEngine::new();
    let mut c = support::cpu::CpuCanvas::new(8, 8, &mut text);
    let cut = fit_label_in(&mut c, "Kick drum heavy 01", 60.0, 12.0);
    assert!(measure(&mut c, &cut, 12.0, 400) <= 60.0);
    let _ = Align::Start;
}
