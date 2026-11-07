//! The Delivery panel: the Targets page lists the targets with what each does to the master and a
//! warning for a quiet master with hot peaks, the Peaks page lists the highest peaks with the
//! threshold, step and width controls, every button and key emits its action, the pages switch,
//! and every control has a short tooltip.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DeliveryAction, DeliveryData, DeliveryPage, DeliveryView, DisplayList, Key, Modifiers, PeakRow,
    PeaksData, TargetMark, TargetRow, TargetsData, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 960.0, 700.0);

fn targets() -> TargetsData {
    let row = |name: &str, effect: &str, mark| TargetRow {
        name: name.into(),
        effect: effect.into(),
        mark,
        current: false,
    };
    TargetsData {
        running: None,
        master: Some("-18.0 LUFS   -0.2 dBTP   range 5.0 LU".into()),
        stale: false,
        rows: vec![
            row(
                "Streaming -14 LUFS / -1 dBTP",
                "turned up 4.0 dB into its limiter",
                TargetMark::Limiter,
            ),
            row(
                "Broadcast -23 LUFS / -1 dBTP",
                "turned down 5.0 dB",
                TargetMark::Fine,
            ),
            row(
                "Film mix -27 LUFS / -2 dBTP",
                "turned down 9.0 dB",
                TargetMark::Fine,
            ),
        ],
        chosen: 0,
        warning: "A quiet master with peaks near 0 dBFS is turned up into a limiter.".into(),
        ceiling_note: "Set Ceiling sets the Mastering ceiling to -1.0 dBTP".into(),
    }
}

fn peaks() -> PeaksData {
    let row = |time: &str, db: f32, headroom: f32| PeakRow {
        time: time.into(),
        db,
        headroom,
    };
    PeaksData {
        clip: "Drum bus".into(),
        message: String::new(),
        rows: vec![
            row("0:01.250", -0.2, 0.0),
            row("0:03.000", -0.6, 0.4),
            row("0:07.500", -1.4, 1.2),
        ],
        total: 40,
        chosen: 0,
        threshold_db: -1.0,
        step_db: 0.3,
        width: 8,
        preview: "Above -1.0 dBFS: 2 peaks, 0.8 dB of headroom".into(),
        can_apply: true,
    }
}

fn data(page: DeliveryPage) -> DeliveryData {
    DeliveryData {
        page,
        targets: targets(),
        peaks: peaks(),
    }
}

fn view(page: DeliveryPage) -> DeliveryView {
    let mut v = DeliveryView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(data(page));
    v
}

fn actions(out: Vec<UiAction>) -> Vec<DeliveryAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Delivery(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn click(v: &mut DeliveryView, p: Point) -> Vec<DeliveryAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    actions(out)
}

fn key_press(v: &mut DeliveryView, key: Key) -> Vec<DeliveryAction> {
    actions(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn texts(v: &DeliveryView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_targets_page_lists_each_target_with_its_effect_and_the_warning() {
    let t = texts(&view(DeliveryPage::Targets));
    for s in [
        "DELIVERY",
        "TARGETS",
        "PEAKS",
        "MEASURE",
        "SET CEILING",
        "CLOSE",
    ] {
        assert!(t.iter().any(|x| x == s), "{s} in {t:?}");
    }
    assert!(
        t.iter().any(|s| s == "Streaming -14 LUFS / -1 dBTP"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "turned down 5.0 dB"));
    assert!(
        t.iter().any(|s| s.starts_with("Master  -18.0 LUFS")),
        "{t:?}"
    );
    assert!(
        t.iter().any(|s| s.contains("turned up into a limiter")),
        "{t:?}"
    );
}

#[test]
fn a_target_that_is_the_projects_says_so() {
    let mut v = view(DeliveryPage::Targets);
    let mut d = data(DeliveryPage::Targets);
    d.targets.rows[1].current = true;
    v.set_data(d);
    assert!(
        texts(&v)
            .iter()
            .any(|s| s == "Broadcast -23 LUFS / -1 dBTP  (project target)")
    );
}

#[test]
fn the_buttons_rows_and_keys_of_the_targets_page_emit_their_actions() {
    let mut v = view(DeliveryPage::Targets);
    let measure = v.main_rect().center();
    assert_eq!(click(&mut v, measure), vec![DeliveryAction::Measure]);
    let set = v.action_rect().center();
    assert_eq!(click(&mut v, set), vec![DeliveryAction::SetCeiling]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![DeliveryAction::Close]);
    let row = v.row_rect(2).center();
    assert_eq!(click(&mut v, row), vec![DeliveryAction::Choose(2)]);
    // A click below the last target is nothing.
    let empty = v.row_rect(5).center();
    assert!(click(&mut v, empty).is_empty());
    let mut chosen = data(DeliveryPage::Targets);
    chosen.targets.chosen = 1;
    v.set_data(chosen);
    assert_eq!(
        key_press(&mut v, Key::Down),
        vec![DeliveryAction::Choose(2)]
    );
    assert_eq!(key_press(&mut v, Key::Up), vec![DeliveryAction::Choose(0)]);
    assert_eq!(key_press(&mut v, Key::Enter), vec![DeliveryAction::Measure]);
    assert_eq!(
        key_press(&mut v, Key::Right),
        vec![DeliveryAction::Page(DeliveryPage::Peaks)]
    );
}

#[test]
fn while_it_measures_the_button_stops_and_the_progress_shows() {
    let mut v = view(DeliveryPage::Targets);
    let mut d = data(DeliveryPage::Targets);
    d.targets.running = Some(0.4);
    v.set_data(d);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "STOP"), "{t:?}");
    assert!(t.iter().any(|s| s == "Rendering the master: 40 %"), "{t:?}");
    let button = v.main_rect().center();
    assert_eq!(click(&mut v, button), vec![DeliveryAction::Stop]);
    assert!(key_press(&mut v, Key::Enter).is_empty());
    assert!(v.help_at(button).unwrap().text.contains("Stop"));
}

#[test]
fn a_stale_measurement_says_so() {
    let mut v = view(DeliveryPage::Targets);
    let mut d = data(DeliveryPage::Targets);
    d.targets.stale = true;
    v.set_data(d);
    assert!(
        texts(&v)
            .iter()
            .any(|s| s.starts_with("The song changed since this measurement"))
    );
}

#[test]
fn the_pages_switch_with_the_tabs_and_the_arrows() {
    let mut v = view(DeliveryPage::Targets);
    let peaks_tab = v.tab_rect(DeliveryPage::Peaks).center();
    assert_eq!(
        click(&mut v, peaks_tab),
        vec![DeliveryAction::Page(DeliveryPage::Peaks)]
    );
    let targets_tab = v.tab_rect(DeliveryPage::Targets).center();
    assert_eq!(
        click(&mut v, targets_tab),
        vec![DeliveryAction::Page(DeliveryPage::Targets)]
    );
    v.set_data(data(DeliveryPage::Peaks));
    assert_eq!(
        key_press(&mut v, Key::Left),
        vec![DeliveryAction::Page(DeliveryPage::Targets)]
    );
}

#[test]
fn the_peaks_page_lists_the_peaks_and_the_controls() {
    let t = texts(&view(DeliveryPage::Peaks));
    for s in [
        "TRIM",
        "APPLY ALL",
        "RESCAN",
        "STEP 0.3 dB",
        "WIDTH 8",
        "-1.0 dBFS",
        "Trim above",
    ] {
        assert!(t.iter().any(|x| x == s), "{s} in {t:?}");
    }
    assert!(
        t.iter().any(|s| s == "Clip  Drum bus  (40 peaks kept)"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "0:03.000"));
    assert!(t.iter().any(|s| s == "-0.60 dBFS"));
    assert!(t.iter().any(|s| s == "1.20 dB if trimmed to here"));
    assert!(
        t.iter().any(|s| s.starts_with("Above -1.0 dBFS: 2 peaks")),
        "{t:?}"
    );
}

#[test]
fn the_buttons_rows_and_keys_of_the_peaks_page_emit_their_actions() {
    let mut v = view(DeliveryPage::Peaks);
    let cases = [
        (v.main_rect().center(), DeliveryAction::Trim),
        (v.action_rect().center(), DeliveryAction::ApplyAll),
        (
            v.threshold_down_rect().center(),
            DeliveryAction::Threshold(-0.1),
        ),
        (
            v.threshold_up_rect().center(),
            DeliveryAction::Threshold(0.1),
        ),
        (v.step_rect().center(), DeliveryAction::CycleStep),
        (v.width_rect().center(), DeliveryAction::CycleWidth),
        (v.rescan_rect().center(), DeliveryAction::Rescan),
        (v.row_rect(1).center(), DeliveryAction::PeakChoose(1)),
    ];
    for (p, want) in cases {
        assert_eq!(click(&mut v, p), vec![want]);
    }
    assert_eq!(key_press(&mut v, Key::Enter), vec![DeliveryAction::Trim]);
    assert_eq!(
        key_press(&mut v, Key::Down),
        vec![DeliveryAction::PeakChoose(1)]
    );
}

#[test]
fn without_a_clip_the_page_says_why() {
    let mut v = view(DeliveryPage::Peaks);
    let mut d = data(DeliveryPage::Peaks);
    d.peaks = PeaksData {
        message: "Select an audio clip to trim its peaks".into(),
        threshold_db: -1.0,
        step_db: 0.3,
        width: 8,
        ..PeaksData::default()
    };
    v.set_data(d);
    let t = texts(&v);
    assert!(
        t.iter()
            .any(|s| s == "Select an audio clip to trim its peaks"),
        "{t:?}"
    );
}

#[test]
fn every_control_has_a_short_tooltip() {
    for page in [DeliveryPage::Targets, DeliveryPage::Peaks] {
        let v = view(page);
        let mut points = vec![
            v.close_rect().center(),
            v.tab_rect(DeliveryPage::Targets).center(),
            v.tab_rect(DeliveryPage::Peaks).center(),
            v.main_rect().center(),
            v.action_rect().center(),
            v.row_rect(0).center(),
        ];
        if page == DeliveryPage::Peaks {
            points.extend([
                v.threshold_down_rect().center(),
                v.threshold_up_rect().center(),
                v.step_rect().center(),
                v.width_rect().center(),
                v.rescan_rect().center(),
            ]);
        }
        for p in points {
            let help = v.help_at(p).expect("a tooltip");
            assert!(help.text.len() <= 80, "{}", help.text);
        }
    }
}

#[test]
fn the_panel_stays_inside_a_small_window_and_its_buttons_do_not_overlap() {
    for page in [DeliveryPage::Targets, DeliveryPage::Peaks] {
        let mut v = view(page);
        v.set_bounds(Rect::new(0.0, 0.0, 600.0, 520.0));
        let p = v.panel();
        assert!(p.x0 >= 0.0 && p.x1 <= 600.0 && p.y0 >= 0.0 && p.y1 <= 520.0);
        let header = [
            v.tab_rect(DeliveryPage::Targets),
            v.tab_rect(DeliveryPage::Peaks),
            v.main_rect(),
            v.action_rect(),
            v.close_rect(),
        ];
        for (i, a) in header.iter().enumerate() {
            assert!(a.x0 >= p.x0 && a.x1 <= p.x1, "{a:?} in {p:?}");
            for b in &header[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{a:?} overlaps {b:?}");
            }
        }
        let row = [
            v.threshold_down_rect(),
            v.threshold_up_rect(),
            v.step_rect(),
            v.width_rect(),
            v.rescan_rect(),
        ];
        assert!(
            row.iter().all(|r| r.x1 <= p.x1 && r.y1 <= p.y1),
            "{row:?} in {p:?}"
        );
        assert!(v.list_rect().y1 <= p.y1);
    }
}
