//! The floating meter panels: the spectrum panel draws the trace and a second curve, the phase
//! panel the goniometer and the correlation; a press on the buttons, the source list and the title
//! does what it says; the pictures are goldens in English and in French.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    CurveSource, DisplayList, Lang, MeterKind, MeterPanelAction, MeterPanelState, MeterPanelsView,
    Modifiers, PANEL_HEIGHT, PANEL_WIDTH, PointerButton, Theme, UiAction, UiEvent,
};
use std::path::PathBuf;
use support::*;

const AREA: (f64, f64) = (900.0, 640.0);

/// A smooth trace in dB with a bump at column `peak`.
fn trace(peak: usize, base: f32) -> Vec<f32> {
    (0..240)
        .map(|i| {
            let d = (i as f32 - peak as f32) / 18.0;
            base - 0.25 * i as f32 + 38.0 * (-d * d).exp()
        })
        .collect()
}

fn sources() -> Vec<CurveSource> {
    vec![
        CurveSource::Off,
        CurveSource::Strip("Drums".to_owned()),
        CurveSource::Strip("Bass".to_owned()),
        CurveSource::Reference,
    ]
}

fn spectrum(id: u32, source: usize) -> MeterPanelState {
    MeterPanelState {
        id,
        kind: MeterKind::Spectrum,
        name: "Lead".to_owned(),
        trace: trace(120, -22.0),
        peak_hz: 1250.0,
        peak_db: -9.4,
        size: 4096,
        pre: false,
        second: if source == 0 {
            Vec::new()
        } else {
            trace(90, -28.0)
        },
        sources: sources(),
        source,
        scope_l: Vec::new(),
        scope_r: Vec::new(),
        correlation: 0.0,
        left_db: -120.0,
        right_db: -120.0,
    }
}

fn phase(id: u32) -> MeterPanelState {
    // A mostly mono signal with a little side: an ellipse leaning to the mid axis.
    let frames: Vec<(f32, f32)> = (0..1024)
        .map(|i| {
            let t = i as f32 * 0.07;
            let (m, s) = (0.6 * t.sin(), 0.12 * (t * 1.7).sin());
            (m + s, m - s)
        })
        .collect();
    MeterPanelState {
        id,
        kind: MeterKind::Phase,
        name: "Bass".to_owned(),
        trace: Vec::new(),
        peak_hz: 0.0,
        peak_db: -120.0,
        size: 4096,
        pre: false,
        second: Vec::new(),
        sources: sources(),
        source: 0,
        scope_l: frames.iter().map(|f| f.0).collect(),
        scope_r: frames.iter().map(|f| f.1).collect(),
        correlation: 0.92,
        left_db: -4.3,
        right_db: -5.1,
    }
}

fn view_of(states: Vec<MeterPanelState>) -> MeterPanelsView {
    let mut v = MeterPanelsView::new();
    v.set_area(AREA.0, AREA.1);
    v.set_panels(states);
    v
}

fn paint(v: &mut MeterPanelsView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    dl
}

fn click(v: &mut MeterPanelsView, p: Point) -> Vec<MeterPanelAction> {
    let mut out = v.handle(&mv(p)).1;
    let (_, down) = v.handle(&down(p));
    out.extend(down);
    out.extend(v.handle(&up(p)).1);
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::MeterPanel(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn centre(r: Rect) -> Point {
    r.center()
}

fn french_golden(name: &str, list: &DisplayList) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/fr")
        .join(name);
    let actual = list.to_json();
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}", path.display()))
        .replace("\r\n", "\n");
    assert!(expected == actual, "{name} differs from its golden");
}

#[test]
fn the_spectrum_panel_with_a_reference_curve_is_a_golden_in_english_and_french() {
    let mut v = view_of(vec![spectrum(1, 3)]);
    let english = paint(&mut v);
    support::starter::check_golden("meter_panel_spectrum_ref_1x.json", &english.to_json());
    assert!(english.texts().contains(&"SPECTRUM"));
    let (french, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            let mut v = view_of(vec![spectrum(1, 3)]);
            paint(&mut v)
        })
    });
    french_golden("meter_panel_spectrum_ref_1x.json", &french);
    // The words of the program all have a French entry; the strip name is the user's.
    let missing: Vec<&String> = misses.iter().filter(|m| m.as_str() != "Lead").collect();
    assert!(missing.is_empty(), "no French entry: {missing:?}");
    assert!(french.texts().contains(&"TAILLE"));
}

#[test]
fn the_source_list_is_a_golden_in_english_and_french() {
    let mut v = view_of(vec![spectrum(1, 1)]);
    let source = centre(v.source_rect(1).unwrap());
    assert!(click(&mut v, source).is_empty(), "opening asks for nothing");
    let english = paint(&mut v);
    support::starter::check_golden("meter_panel_sources_1x.json", &english.to_json());
    let (french, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            let mut v = view_of(vec![spectrum(1, 1)]);
            let source = centre(v.source_rect(1).unwrap());
            click(&mut v, source);
            paint(&mut v)
        })
    });
    french_golden("meter_panel_sources_1x.json", &french);
    let missing: Vec<&String> = misses
        .iter()
        .filter(|m| !matches!(m.as_str(), "Lead" | "Drums" | "Bass"))
        .collect();
    assert!(missing.is_empty(), "no French entry: {missing:?}");
    assert!(french.texts().contains(&"Piste de référence"));
}

#[test]
fn the_phase_panel_is_a_golden_in_english_and_french() {
    let mut v = view_of(vec![phase(2)]);
    let english = paint(&mut v);
    support::starter::check_golden("meter_panel_phase_1x.json", &english.to_json());
    assert!(english.texts().contains(&"PHASE"));
    let (french, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            let mut v = view_of(vec![phase(2)]);
            paint(&mut v)
        })
    });
    french_golden("meter_panel_phase_1x.json", &french);
    // The one-letter axis labels of the goniometer (L, R, M, S) are the same in French.
    let missing: Vec<&String> = misses
        .iter()
        .filter(|m| m.as_str() != "Bass" && m.chars().count() > 1)
        .collect();
    assert!(missing.is_empty(), "no French entry: {missing:?}");
}

#[test]
fn the_close_cross_and_the_size_arrows_emit_their_actions() {
    let mut v = view_of(vec![spectrum(7, 0), phase(8)]);
    let close = centre(v.close_rect(7).unwrap());
    assert_eq!(click(&mut v, close), [MeterPanelAction::Close(7)]);
    let up = centre(v.size_rect(7, true).unwrap());
    assert_eq!(
        click(&mut v, up),
        [MeterPanelAction::Size { id: 7, up: true }]
    );
    let down = centre(v.size_rect(7, false).unwrap());
    assert_eq!(
        click(&mut v, down),
        [MeterPanelAction::Size { id: 7, up: false }]
    );
    // The phase panel has no size arrows: the same place is just the panel.
    let there = centre(v.size_rect(8, true).unwrap());
    assert!(click(&mut v, there).is_empty());
    let close = centre(v.close_rect(8).unwrap());
    assert_eq!(click(&mut v, close), [MeterPanelAction::Close(8)]);
}

#[test]
fn a_press_on_the_source_button_opens_the_list_and_a_row_picks_a_source() {
    let mut v = view_of(vec![spectrum(3, 0)]);
    let button = centre(v.source_rect(3).unwrap());
    assert!(click(&mut v, button).is_empty());
    let list = v.list_rect(3).expect("open");
    assert!(list.height() > 4.0 * 18.0, "four rows");
    // The third row is the strip Bass.
    let row = centre(v.list_row_rect(3, 2).unwrap());
    assert_eq!(
        click(&mut v, row),
        [MeterPanelAction::Source { id: 3, index: 2 }]
    );
    assert!(v.list_rect(3).is_none(), "a pick closes the list");
    // A press elsewhere closes it without asking for anything.
    click(&mut v, button);
    assert!(v.list_rect(3).is_some());
    let elsewhere = Point::new(AREA.0 - 6.0, AREA.1 - 6.0);
    assert!(click(&mut v, elsewhere).is_empty());
    assert!(v.list_rect(3).is_none());
    // The button again closes it (the press that closed it does not reopen it).
    click(&mut v, button);
    click(&mut v, button);
    assert!(v.list_rect(3).is_none());
}

#[test]
fn a_long_list_scrolls_with_the_wheel_and_keeps_its_rows_addressable() {
    let mut s = spectrum(4, 0);
    s.sources = std::iter::once(CurveSource::Off)
        .chain((0..24).map(|i| CurveSource::Strip(format!("Channel {i}"))))
        .collect();
    let mut v = view_of(vec![s]);
    let button = centre(v.source_rect(4).unwrap());
    click(&mut v, button);
    assert!(v.list_row_rect(4, 0).is_some());
    assert!(v.list_row_rect(4, 12).is_none(), "ten rows show");
    let at = centre(v.list_rect(4).unwrap());
    let wheel = |dy: f64| UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
        pos: at,
        modifiers: Modifiers::NONE,
    };
    for _ in 0..5 {
        assert!(v.handle(&wheel(-1.0)).0, "taken");
    }
    assert!(v.list_row_rect(4, 0).is_none(), "scrolled");
    let row = centre(v.list_row_rect(4, 12).unwrap());
    assert_eq!(
        click(&mut v, row),
        [MeterPanelAction::Source { id: 4, index: 12 }]
    );
}

#[test]
fn a_panel_is_dragged_by_its_title_kept_inside_the_window_and_raised_by_a_press() {
    let mut v = view_of(vec![spectrum(1, 0), phase(2)]);
    assert_eq!(v.ids(), [1, 2]);
    let first = v.bounds(1).unwrap();
    assert_ne!(v.bounds(1).unwrap().origin(), v.bounds(2).unwrap().origin());
    assert!(
        (first.width() - PANEL_WIDTH).abs() < 1e-9 && (first.height() - PANEL_HEIGHT).abs() < 1e-9
    );
    // A press on the title of panel 1 raises it above panel 2 and a drag moves it.
    let grab = Point::new(first.x0 + 120.0, first.y0 + 8.0);
    assert!(v.handle(&down(grab)).0);
    assert_eq!(v.ids(), [2, 1], "raised");
    assert!(v.is_dragging());
    v.handle(&mv(Point::new(grab.x + 50.0, grab.y + 30.0)));
    let moved = v.bounds(1).unwrap();
    assert!((moved.x0 - first.x0 - 50.0).abs() < 1e-9 && (moved.y0 - first.y0 - 30.0).abs() < 1e-9);
    // Far outside the window: kept inside.
    v.handle(&mv(Point::new(5000.0, 5000.0)));
    let edge = v.bounds(1).unwrap();
    assert!(edge.x1 <= AREA.0 && edge.y1 <= AREA.1, "{edge:?}");
    v.handle(&up(Point::new(5000.0, 5000.0)));
    assert!(!v.is_dragging());
    // A press on the plot does not drag.
    let b = v.bounds(2).unwrap();
    assert!(v.handle(&down(Point::new(b.x0 + 100.0, b.y0 + 120.0))).0);
    assert!(!v.is_dragging());
    // A press away from every panel is not taken.
    assert!(!v.handle(&down(Point::new(2.0, 2.0))).0);
}

#[test]
fn panels_come_and_go_with_the_states_the_app_feeds() {
    let mut v = view_of(vec![spectrum(1, 0), spectrum(2, 0)]);
    let place = v.bounds(1).unwrap().origin();
    v.set_panels(vec![spectrum(1, 2)]);
    assert_eq!(v.ids(), [1]);
    assert_eq!(v.bounds(1).unwrap().origin(), place, "an open panel stays");
    v.set_panels(vec![spectrum(1, 2), phase(9)]);
    assert_eq!(v.ids(), [1, 9]);
    v.set_panels(Vec::new());
    assert!(v.is_empty());
    // The same state again asks for no repaint.
    let mut v = view_of(vec![spectrum(1, 0)]);
    paint(&mut v);
    v.set_panels(vec![spectrum(1, 0)]);
    assert!(!v.needs_repaint());
    v.set_panels(vec![spectrum(1, 1)]);
    assert!(v.needs_repaint());
}

#[test]
fn every_part_has_a_short_tooltip() {
    let mut v = view_of(vec![spectrum(1, 0)]);
    for r in [
        v.close_rect(1).unwrap(),
        v.size_rect(1, true).unwrap(),
        v.size_rect(1, false).unwrap(),
        v.source_rect(1).unwrap(),
        v.point_rect(1).unwrap(),
    ] {
        let help = v.help_at(centre(r)).expect("a tooltip");
        assert!(help.text.len() <= 90, "{}", help.text);
        let french = atomik_ui::i18n::with_language(Lang::French, || {
            atomik_ui::i18n::tr(&help.text).into_owned()
        });
        assert!(french.chars().count() <= 90, "{french}");
        assert_ne!(french, help.text, "{} has a French line", help.text);
    }
    let b = v.bounds(1).unwrap();
    assert!(v.help_at(Point::new(b.x0 + 100.0, b.y0 + 6.0)).is_some());
    assert!(v.help_at(Point::new(-4.0, -4.0)).is_none());
    let _ = paint(&mut v);
    let _ = PointerButton::Primary;
}

/// A spectrum panel that taps before the fader at the largest size.
fn spectrum_pre_large(id: u32) -> MeterPanelState {
    MeterPanelState {
        pre: true,
        size: 65_536,
        ..spectrum(id, 1)
    }
}

#[test]
fn a_panel_with_pre_selected_and_a_large_size_is_a_golden_in_english_and_french() {
    let mut v = view_of(vec![spectrum_pre_large(1)]);
    let english = paint(&mut v);
    support::starter::check_golden("meter_panel_pre_large_1x.json", &english.to_json());
    assert!(english.texts().contains(&"PRE") && english.texts().contains(&"65536"));
    let (french, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(Lang::French, || {
            let mut v = view_of(vec![spectrum_pre_large(1)]);
            paint(&mut v)
        })
    });
    french_golden("meter_panel_pre_large_1x.json", &french);
    let missing: Vec<&String> = misses
        .iter()
        .filter(|m| m.as_str() != "Lead" && m.as_str() != "Drums")
        .collect();
    assert!(missing.is_empty(), "no French entry: {missing:?}");
    assert!(french.texts().contains(&"PR\u{c9}"));
    // POST is the default and reads POST; the phase panel has the same chip.
    let post = paint(&mut view_of(vec![spectrum(1, 0)]));
    assert!(post.texts().contains(&"POST") && !post.texts().contains(&"PRE"));
    let phase = paint(&mut view_of(vec![phase(2)]));
    assert!(phase.texts().contains(&"POST"));
}

#[test]
fn the_pre_post_chip_emits_a_point_action_on_both_panels() {
    for (id, state) in [(3, spectrum(3, 0)), (4, phase(4))] {
        let mut alone = view_of(vec![state]);
        let chip = centre(alone.point_rect(id).unwrap());
        assert_eq!(click(&mut alone, chip), [MeterPanelAction::Point { id }]);
    }
    let v = view_of(vec![spectrum(3, 0), phase(4)]);
    // The chip sits clear of the size arrows and the source button.
    let p = v.point_rect(3).unwrap();
    for r in [
        v.size_rect(3, true).unwrap(),
        v.size_rect(3, false).unwrap(),
        v.source_rect(3).unwrap(),
    ] {
        assert!(p.intersect(r).area() == 0.0, "{p:?} {r:?}");
    }
    // The phase chip is inside its panel.
    let (b, chip) = (v.bounds(4).unwrap(), v.point_rect(4).unwrap());
    assert!(b.contains(Point::new(chip.x1 - 1.0, chip.y1 - 1.0)));
}

fn double_click(v: &mut MeterPanelsView, p: Point) {
    v.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 2,
    });
    v.handle(&up(p));
}

#[test]
fn the_grip_resizes_a_panel_never_smaller_than_it_opens_nor_out_of_the_window() {
    let mut v = view_of(vec![spectrum(1, 0)]);
    let start = v.bounds(1).unwrap();
    let grip = v.grip_rect(1).expect("a grip");
    assert!(
        (grip.x1 - start.x1).abs() < 1e-9 && (grip.y1 - start.y1).abs() < 1e-9,
        "in the bottom right corner"
    );
    let at = centre(grip);
    assert!(v.handle(&down(at)).0);
    assert!(v.is_dragging());
    v.handle(&mv(Point::new(at.x + 200.0, at.y + 100.0)));
    let big = v.bounds(1).unwrap();
    assert!(
        (big.width() - start.width() - 200.0).abs() < 1e-9,
        "{big:?}"
    );
    assert!(
        (big.height() - start.height() - 100.0).abs() < 1e-9,
        "{big:?}"
    );
    assert_eq!(big.origin(), start.origin(), "the top left stays");
    assert!(
        v.trace_columns(1) > 240,
        "a wider plot asks for a finer trace"
    );
    // Smaller than it opens: no.
    v.handle(&mv(Point::new(at.x - 300.0, at.y - 300.0)));
    let small = v.bounds(1).unwrap();
    assert!(
        (small.width() - PANEL_WIDTH).abs() < 1e-9 && (small.height() - PANEL_HEIGHT).abs() < 1e-9
    );
    assert_eq!(v.trace_columns(1), 240);
    // Past the window: stops at its edge.
    v.handle(&mv(Point::new(5000.0, 5000.0)));
    let edge = v.bounds(1).unwrap();
    assert!(edge.x1 <= AREA.0 && edge.y1 <= AREA.1, "{edge:?}");
    v.handle(&up(Point::new(5000.0, 5000.0)));
    assert!(!v.is_dragging());
}

#[test]
fn maximize_fills_the_window_and_puts_the_panel_back_where_it_was() {
    let mut v = view_of(vec![spectrum(1, 0), phase(2)]);
    let start = v.bounds(1).unwrap();
    let at = centre(v.maximize_rect(1).unwrap());
    click(&mut v, at);
    assert!(v.is_maximized(1));
    let full = v.bounds(1).unwrap();
    assert!(
        full.width() > AREA.0 - 20.0 && full.height() > AREA.1 - 20.0,
        "{full:?}"
    );
    assert!(v.grip_rect(1).is_none(), "no grip while maximized");
    // A maximized panel is not dragged by its title.
    let title = Point::new(full.x0 + 120.0, full.y0 + 8.0);
    v.handle(&down(title));
    assert!(!v.is_dragging());
    v.handle(&up(title));
    // The window grows: it still fills it.
    v.set_area(1200.0, 800.0);
    assert!(v.bounds(1).unwrap().width() > 1180.0);
    v.set_area(AREA.0, AREA.1);
    // Back where it was.
    let at = centre(v.maximize_rect(1).unwrap());
    click(&mut v, at);
    assert!(!v.is_maximized(1));
    assert_eq!(v.bounds(1).unwrap(), start);
    // A double click on the title does the same, on the phase panel too (moved clear of the
    // spectrum panel, which lies over its title).
    v.move_to(2, Point::new(480.0, 360.0));
    let p = v.bounds(2).unwrap();
    double_click(&mut v, Point::new(p.x0 + 120.0, p.y0 + 8.0));
    assert!(v.is_maximized(2));
    let r = v.plot_rect(2).unwrap();
    assert!(
        (r.width() - r.height()).abs() < 1e-9,
        "the goniometer stays square"
    );
    assert!(r.height() > 300.0, "and grows: {r:?}");
    let full = v.bounds(2).unwrap();
    double_click(&mut v, Point::new(full.x0 + 120.0, full.y0 + 8.0));
    assert_eq!(v.bounds(2).unwrap(), p);
}

#[test]
fn hold_keeps_the_peaks_falling_slowly_and_a_click_on_the_plot_starts_again() {
    let mut v = view_of(vec![spectrum(1, 0)]);
    assert!(!v.is_holding(1));
    let at = centre(v.hold_rect(1).unwrap());
    click(&mut v, at);
    assert!(v.is_holding(1));
    let loud = spectrum(1, 0).trace;
    assert_eq!(v.held(1), loud.as_slice());
    // The trace drops by 30 dB: the hold falls by a little at each new trace.
    let mut quiet = spectrum(1, 0);
    quiet.trace = loud.iter().map(|d| d - 30.0).collect();
    v.set_panels(vec![quiet.clone()]);
    let fall = atomik_ui::meter_panels::HOLD_FALL_DB;
    assert!((v.held(1)[120] - (loud[120] - fall)).abs() < 1e-4);
    // The same trace again is no new trace: nothing falls.
    v.set_panels(vec![quiet.clone()]);
    assert!((v.held(1)[120] - (loud[120] - fall)).abs() < 1e-4);
    // A click on the plot starts it again from the trace.
    let plot = v.plot_rect(1).unwrap();
    click(&mut v, Point::new(plot.x0 + 40.0, plot.y1 - 20.0));
    assert_eq!(v.held(1), quiet.trace.as_slice());
    // Off: nothing held.
    let at = centre(v.hold_rect(1).unwrap());
    click(&mut v, at);
    assert!(!v.is_holding(1) && v.held(1).is_empty());
}

#[test]
fn the_cursor_readout_names_the_frequency_the_note_and_the_level() {
    let plot = Rect::new(0.0, 0.0, 300.0, 100.0);
    let trace = vec![-12.0f32; 300];
    // 440 Hz on the log axis from 20 Hz to 20 kHz.
    let x = (440.0f64 / 20.0).ln() / 1000.0f64.ln() * 300.0;
    let text = atomik_ui::meter_panels::cursor_text(plot, &trace, x);
    assert!(text.starts_with("440 Hz"), "{text}");
    assert!(text.contains("A4"), "{text}");
    assert!(text.ends_with("-12.0 dB"), "{text}");
    let silent = vec![-200.0f32; 300];
    assert!(atomik_ui::meter_panels::cursor_text(plot, &silent, x).ends_with("--"));
    // Over the plot the readout is drawn.
    let mut v = view_of(vec![spectrum(1, 0)]);
    let r = v.plot_rect(1).unwrap();
    v.handle(&mv(Point::new(r.x0 + 100.0, r.y0 + 60.0)));
    let dl = paint(&mut v);
    assert!(dl.to_json().contains(" dB"), "a readout");
}

#[test]
fn a_maximized_panel_and_its_cursor_are_a_golden() {
    let mut v = view_of(vec![spectrum(1, 2)]);
    let at = centre(v.hold_rect(1).unwrap());
    click(&mut v, at);
    v.toggle_maximized(1);
    let r = v.plot_rect(1).unwrap();
    v.handle(&mv(Point::new(r.x0 + r.width() * 0.6, r.y0 + 80.0)));
    let dl = paint(&mut v);
    support::starter::check_golden("meter_panel_maximized_1x.json", &dl.to_json());
}
