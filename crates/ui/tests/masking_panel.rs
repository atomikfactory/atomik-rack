//! The masking map panel: it lists the pairs it was given, draws the spectra of the chosen pair,
//! a band click chooses the band, a row click the pair, and the buttons emit their actions.

mod support;

use atomik_dsp::masking::{BANDS, band_label};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, MaskingAction, MaskingData, MaskingPair, MaskingView, Modifiers, Theme,
    UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn pair(masker: &str, masked: &str, worst: usize, score: f32) -> MaskingPair {
    let mut zone = [false; BANDS];
    zone[worst] = true;
    let mut masker_db = [-70.0; BANDS];
    let mut masked_db = [-70.0; BANDS];
    masker_db[worst] = -10.0;
    masked_db[worst] = -30.0;
    MaskingPair {
        masker: masker.into(),
        masked: masked.into(),
        worst,
        score,
        zone,
        masker_db,
        masked_db,
        ranges: vec![(1.0, 3.5), (62.0, 70.0)],
    }
}

fn data() -> MaskingData {
    MaskingData {
        tracks: 3,
        pairs: vec![
            pair("Kick", "Bass", 10, 82.0),
            pair("Vox", "Keys", 14, 41.0),
        ],
    }
}

fn view() -> MaskingView {
    let mut v = MaskingView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(Some(data()));
    v
}

fn click(v: &mut MaskingView, p: Point) -> Vec<MaskingAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Masking(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &MaskingView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_lists_the_pairs_with_their_band_and_score() {
    let t = texts(&view());
    assert!(t.iter().any(|s| s == "MASKING MAP"));
    assert!(t.iter().any(|s| s.contains("Kick > Bass")), "{t:?}");
    assert!(t.iter().any(|s| s.contains("Vox > Keys")));
    assert!(t.iter().any(|s| s == "82%"));
    assert!(t.iter().any(|s| *s == band_label(10)));
    assert!(t.iter().any(|s| s.starts_with("DE-MASK")));
    assert!(t.iter().any(|s| s.contains("0:01.0 - 0:03.5")), "{t:?}");
}

#[test]
fn an_empty_panel_says_what_to_do() {
    let mut v = MaskingView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "No analysis yet"));
    assert!(t.iter().any(|s| s.contains("Analyse renders")));
    // Nothing to de-mask or locate.
    let p = v.demask_rect().center();
    assert!(click(&mut v, p).is_empty());
    v.set_data(Some(MaskingData::default()));
    assert!(texts(&v).iter().any(|s| s.contains("No pair")));
}

#[test]
fn rows_and_arrow_keys_choose_the_pair_and_bands_choose_the_band() {
    let mut v = view();
    assert_eq!((v.pair(), v.band()), (0, Some(10)));
    let row = v.row_rect(1).center();
    assert!(click(&mut v, row).is_empty());
    assert_eq!((v.pair(), v.band()), (1, Some(14)));
    let slot = v.band_rect(5).center();
    click(&mut v, slot);
    assert_eq!(v.band(), Some(5));
    // Another pair goes back to its worst band.
    v.handle(&UiEvent::Key {
        key: Key::Up,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!((v.pair(), v.band()), (0, Some(10)));
    // A click below the last row does not choose a pair that is not there.
    let empty = v.row_rect(5).center();
    click(&mut v, empty);
    assert_eq!(v.pair(), 0);
}

#[test]
fn the_buttons_emit_their_actions() {
    let mut v = view();
    let analyse = v.analyse_rect().center();
    assert_eq!(click(&mut v, analyse), vec![MaskingAction::Analyse]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![MaskingAction::Close]);
    let demask = v.demask_rect().center();
    assert_eq!(
        click(&mut v, demask),
        vec![MaskingAction::Demask { pair: 0, band: 10 }]
    );
    let slot = v.band_rect(12).center();
    click(&mut v, slot);
    let demask = v.demask_rect().center();
    assert_eq!(
        click(&mut v, demask),
        vec![MaskingAction::Demask { pair: 0, band: 12 }]
    );
    let range = v.range_rect(1).center();
    assert_eq!(
        click(&mut v, range),
        vec![MaskingAction::Locate { pair: 0, range: 1 }]
    );
}

#[test]
fn while_an_analysis_runs_the_button_cancels_and_shows_the_progress() {
    let mut v = view();
    v.set_progress(Some(0.42));
    assert!(v.busy());
    assert!(texts(&v).iter().any(|s| s == "CANCEL 42%"));
    let button = v.analyse_rect().center();
    assert_eq!(click(&mut v, button), vec![MaskingAction::Cancel]);
    v.set_progress(None);
    assert!(texts(&v).iter().any(|s| s == "ANALYSE"));
}

#[test]
fn the_note_replaces_the_hint_and_every_control_has_a_short_tooltip() {
    let mut v = view();
    v.set_note("De-masked Bass under Kick");
    assert!(texts(&v).iter().any(|s| s == "De-masked Bass under Kick"));
    for p in [
        v.close_rect().center(),
        v.analyse_rect().center(),
        v.demask_rect().center(),
        v.chart_rect().center(),
        v.range_rect(0).center(),
    ] {
        let help = v.help_at(p).expect("a tooltip");
        assert!(help.text.len() <= 80, "{}", help.text);
    }
}

#[test]
fn the_panel_stays_inside_a_small_window() {
    let mut v = view();
    v.set_bounds(Rect::new(0.0, 0.0, 700.0, 520.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 700.0 && p.y0 >= 0.0);
    let chart = v.chart_rect();
    assert!(chart.x1 <= p.x1 && chart.width() > 200.0);
    assert!(v.demask_rect().y1 <= p.y1);
}
