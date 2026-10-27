//! The audio export panel: the options paint and change, the dither row follows the bit depth,
//! the tail row is for songs only, EXPORT and CANCEL emit the right actions with the chosen
//! values, and the progress mode shows the bar and only CANCEL.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, ExportAction, ExportDepth, ExportDither, ExportScope, ExportValues, ExportView,
    Key, Modifiers, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 600.0);

fn view(scope: ExportScope) -> ExportView {
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(scope, "my song.wav", 48_000, ExportValues::default());
    v
}

fn center(v: &ExportView, label: &str) -> Point {
    v.button_rect(label)
        .unwrap_or_else(|| panic!("no button {label}"))
        .center()
}

fn click(v: &mut ExportView, label: &str) -> Vec<ExportAction> {
    let p = center(v, label);
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Export(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn key(v: &mut ExportView, key: Key) -> Vec<ExportAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::default(),
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::Export(e) => Some(e),
        _ => None,
    })
    .collect()
}

fn texts(v: &ExportView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

#[test]
fn the_song_options_paint_every_choice_and_the_file_name() {
    let t = texts(&view(ExportScope::Song));
    for want in [
        "EXPORT SONG AS AUDIO FILE",
        "my song.wav",
        "BIT DEPTH",
        "16-BIT",
        "24-BIT",
        "32-BIT FLOAT",
        "DITHER",
        "TPDF",
        "SAMPLE RATE",
        "PROJECT",
        "44.1 kHz",
        "96 kHz",
        "TAIL",
        "KEEP EFFECT TAIL",
        "EXPORT",
        "CANCEL",
        "Project rate 48 kHz",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
}

#[test]
fn a_loop_has_no_tail_row() {
    let v = view(ExportScope::Loop);
    let t = texts(&v);
    assert!(has(&t, "EXPORT LOOP AS AUDIO FILE"));
    assert!(!has(&t, "KEEP EFFECT TAIL"));
    assert!(v.button_rect("KEEP EFFECT TAIL").is_none());
    assert!(has(&t, "Exactly from the left to the right locator"));
}

#[test]
fn the_buttons_change_the_values() {
    let mut v = view(ExportScope::Song);
    assert_eq!(v.values(), ExportValues::default());
    assert_eq!(v.values().depth, ExportDepth::Pcm24);
    assert!(click(&mut v, "16-BIT").is_empty());
    assert_eq!(v.values().depth, ExportDepth::Pcm16);
    click(&mut v, "SHAPED");
    assert_eq!(v.values().dither, ExportDither::Shaped);
    click(&mut v, "44.1 kHz");
    assert_eq!(v.values().rate, Some(44_100));
    click(&mut v, "PROJECT");
    assert_eq!(v.values().rate, None);
    click(&mut v, "KEEP EFFECT TAIL");
    assert!(!v.values().tail);
    click(&mut v, "32-BIT FLOAT");
    assert_eq!(v.values().depth, ExportDepth::Float32);
}

#[test]
fn the_dither_row_only_works_at_16_bit() {
    let mut v = view(ExportScope::Song);
    // 24-bit (the default): the dither buttons are inert.
    assert_eq!(v.values().dither, ExportDither::Tpdf);
    click(&mut v, "NONE");
    assert_eq!(v.values().dither, ExportDither::Tpdf);
    click(&mut v, "16-BIT");
    click(&mut v, "NONE");
    assert_eq!(v.values().dither, ExportDither::Off);
}

#[test]
fn export_starts_with_the_chosen_values_and_cancel_and_escape_cancel() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "16-BIT");
    click(&mut v, "96 kHz");
    let started = click(&mut v, "EXPORT");
    assert_eq!(
        started,
        vec![ExportAction::Start(ExportValues {
            depth: ExportDepth::Pcm16,
            dither: ExportDither::Tpdf,
            rate: Some(96_000),
            tail: true,
        })]
    );
    assert_eq!(click(&mut v, "CANCEL"), vec![ExportAction::Cancel]);
    assert_eq!(key(&mut v, Key::Escape), vec![ExportAction::Cancel]);
    assert_eq!(
        key(&mut v, Key::Enter),
        vec![ExportAction::Start(v.values())]
    );
}

#[test]
fn the_progress_mode_shows_the_bar_and_only_cancel_works() {
    let mut v = view(ExportScope::Loop);
    v.show_progress();
    assert!(v.in_progress());
    v.set_progress(0.37, "Rendering");
    let t = texts(&v);
    assert!(has(&t, "EXPORTING LOOP"), "{t:?}");
    assert!(has(&t, "37%"), "{t:?}");
    assert!(has(&t, "Rendering"), "{t:?}");
    assert!(v.button_rect("EXPORT").is_none());
    assert!(v.button_rect("16-BIT").is_none());
    assert_eq!(click(&mut v, "CANCEL"), vec![ExportAction::Cancel]);
    // Enter does not start anything while rendering; Escape stops it.
    assert!(key(&mut v, Key::Enter).is_empty());
    assert_eq!(key(&mut v, Key::Escape), vec![ExportAction::Cancel]);
}

#[test]
fn the_panel_is_centred_and_fits_a_small_window() {
    let mut v = view(ExportScope::Song);
    let p = v.panel();
    assert!((p.center().x - 450.0).abs() < 1.0);
    v.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
    assert!(v.panel().width() <= 400.0 - 2.0 * 16.0 + 0.5);
}

#[test]
fn every_button_has_a_tooltip() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "16-BIT");
    for label in [
        "16-BIT",
        "TPDF",
        "PROJECT",
        "44.1 kHz",
        "KEEP EFFECT TAIL",
        "EXPORT",
    ] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
}
