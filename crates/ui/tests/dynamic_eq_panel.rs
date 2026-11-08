//! The dynamic EQ panel: the published meter layout, what the display shows of it, and the band
//! handles (drag, clamping, merge ids, wheel Q, double-click, Esc).

mod support;

use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::fx::*;
use support::*;

fn rig() -> (Document, RackView) {
    let (doc, mut v) = rack(FxKind::DynamicEq);
    let document = Document::new(doc);
    v.sync(document.doc(), &registry());
    (document, v)
}

fn apply(document: &mut Document, v: &mut RackView, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                document
                    .apply(c.clone(), &registry())
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    v.sync(document.doc(), &registry());
}

fn param(document: &Document, p: &str) -> f32 {
    document
        .doc()
        .param_value(&key("fx"), p, &registry())
        .unwrap()
}

fn drag(document: &mut Document, v: &mut RackView, from: Point, path: &[Point]) -> Vec<UiAction> {
    let reg = registry();
    let mut all = Vec::new();
    let a = v.handle(&down(from), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    for p in path {
        let a = v.handle(&mv(*p), document.doc(), &reg);
        apply(document, v, &a);
        all.extend(a);
    }
    let end = path.last().copied().unwrap_or(from);
    let a = v.handle(&up(end), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    all
}

fn set_params(a: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    commands(a)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                param,
                value,
                merge,
                ..
            } => Some((param, value, merge)),
            _ => None,
        })
        .collect()
}

fn texts(v: &mut RackView, doc: &Document) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn json(v: &mut RackView, doc: &Document) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    dl.to_json()
}

#[test]
fn the_published_layout_is_the_one_the_panel_reads() {
    use atomik_devices::dynamic_eq as dev;
    assert_eq!(fx::DYNEQ_METERS, dev::METER_COUNT);
    assert_eq!(fx::DYNEQ_GAINS, dev::METER_GAINS);
    assert_eq!(fx::DYNEQ_INPUT, dev::METER_INPUT);
    assert_eq!(fx::DYNEQ_KEY, dev::METER_KEY);
    assert_eq!(fx::DYNEQ_PEAKS, dev::METER_PEAKS);
    assert_eq!(fx::DYNEQ_KEY_CABLED, dev::METER_KEY_CONNECTED);
    assert_eq!(fx::DYNEQ_BANDS, dev::BANDS);
    assert_eq!(FxKind::DynamicEq.meter_count(), dev::METER_COUNT);
    assert_eq!(FxKind::of("atomik.dynamic-eq"), Some(FxKind::DynamicEq));
}

#[test]
fn the_band_gains_are_shown_as_published_and_the_energies_in_db() {
    let mut raw = vec![0.0f32; fx::DYNEQ_METERS];
    raw[fx::DYNEQ_GAINS + 1] = -4.46;
    raw[fx::DYNEQ_GAINS + 2] = 3.04;
    raw[fx::DYNEQ_INPUT + 3] = 0.1;
    raw[fx::DYNEQ_KEY_CABLED] = 1.0;
    let mut shown = Vec::new();
    assert!(fx::update_display(FxKind::DynamicEq, &raw, &mut shown));
    assert_eq!(shown.len(), fx::DYNEQ_METERS);
    assert!((shown[fx::DYNEQ_GAINS + 1] + 4.5).abs() < 1e-4, "{shown:?}");
    assert!((shown[fx::DYNEQ_GAINS + 2] - 3.0).abs() < 1e-4);
    assert!(
        (shown[fx::DYNEQ_INPUT + 3] + 10.0).abs() < 0.3,
        "10 log10 of 0.1"
    );
    assert_eq!(shown[fx::DYNEQ_INPUT], fx::LEVEL_FLOOR_DB);
    assert_eq!(shown[fx::DYNEQ_KEY_CABLED], 1.0);
    // A gain that returns to zero is shown at once (the device already smooths it).
    raw[fx::DYNEQ_GAINS + 1] = 0.0;
    assert!(fx::update_display(FxKind::DynamicEq, &raw, &mut shown));
    assert_eq!(shown[fx::DYNEQ_GAINS + 1], 0.0);
    // The same values again change nothing.
    assert!(!fx::update_display(FxKind::DynamicEq, &raw, &mut shown));
}

#[test]
fn the_live_curve_follows_the_published_gains() {
    let (document, mut v) = rig();
    let mut raw = raw_meters(FxKind::DynamicEq);
    v.set_device_meter_values(&key("fx"), &raw);
    let a = json(&mut v, &document);
    raw[fx::DYNEQ_GAINS + 1] = -9.0;
    v.set_device_meter_values(&key("fx"), &raw);
    let b = json(&mut v, &document);
    assert_ne!(a, b, "a band that moved changes the picture");
    // With no feed the live curve is the static one: still a picture.
    let (document, mut v) = rig();
    assert!(!json(&mut v, &document).is_empty());
}

#[test]
fn the_key_spectrum_is_labelled_only_while_a_cable_is_in_the_key_sockets() {
    let (document, mut v) = rig();
    let mut raw = raw_meters(FxKind::DynamicEq);
    v.set_device_meter_values(&key("fx"), &raw);
    assert!(texts(&mut v, &document).iter().any(|t| t == "KEY"));
    raw[fx::DYNEQ_KEY_CABLED] = 0.0;
    v.set_device_meter_values(&key("fx"), &raw);
    assert!(!texts(&mut v, &document).iter().any(|t| t == "KEY"));
}

#[test]
fn dragging_a_handle_sideways_sets_the_frequency_in_one_undo_step() {
    let (mut document, mut v) = rig();
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    let before = param(&document, "band2.freq");
    let path: Vec<Point> = (1..=10)
        .map(|i| Point::new(h.x + 6.0 * f64::from(i), h.y))
        .collect();
    let a = drag(&mut document, &mut v, h, &path);
    let sets = set_params(&a);
    assert!(sets.len() >= 5, "{sets:?}");
    assert!(
        sets.iter()
            .all(|(p, _, m)| p == "band2.freq" && m.is_some())
    );
    let id = sets[0].2;
    assert!(sets.iter().all(|(_, _, m)| *m == id), "one merge id");
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));
    assert!(param(&document, "band2.freq") > before * 1.3);
    document.undo();
    assert!((param(&document, "band2.freq") - before).abs() < 1e-3);
}

#[test]
fn dragging_a_handle_up_sets_the_static_gain_and_is_clamped() {
    let (mut document, mut v) = rig();
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    let path = [Point::new(h.x, h.y - 6.0), Point::new(h.x, h.y - 20.0)];
    let a = drag(&mut document, &mut v, h, &path);
    let sets = set_params(&a);
    assert!(sets.iter().all(|(p, _, _)| p == "band2.gain"), "{sets:?}");
    assert!(param(&document, "band2.gain") > 0.0);
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y - 900.0)]);
    assert_eq!(param(&document, "band2.gain"), 24.0);
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y + 900.0)]);
    assert_eq!(param(&document, "band2.gain"), -24.0);
}

#[test]
fn the_shelf_handles_stop_at_their_frequency_range() {
    let (mut document, mut v) = rig();
    let h = v.eq_handle_center(&key("fx"), 0).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x + 900.0, h.y)]);
    assert_eq!(param(&document, "band1.freq"), 5000.0);
    let h = v.eq_handle_center(&key("fx"), 4).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x - 900.0, h.y)]);
    assert_eq!(param(&document, "band5.freq"), 500.0);
}

#[test]
fn the_wheel_over_a_handle_steps_q_and_a_double_click_toggles_the_band() {
    let (mut document, mut v) = rig();
    let reg = registry();
    let h = v.eq_handle_center(&key("fx"), 2).unwrap();
    let q0 = param(&document, "band3.q");
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: h,
            modifiers: Modifiers::NONE,
        },
        document.doc(),
        &reg,
    );
    let sets = set_params(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    assert_eq!(sets[0].0, "band3.q");
    assert!(sets[0].1 > q0);
    let a = v.handle(
        &down_with(h, PointerButton::Primary, 2),
        document.doc(),
        &reg,
    );
    assert_eq!(
        set_params(&a),
        vec![("band3.on".to_string(), 0.0, None)],
        "band 3 was on"
    );
    apply(&mut document, &mut v, &a);
    let a = v.handle(&up(h), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "band3.on"), 0.0);
}

#[test]
fn escape_restores_the_start_value_of_a_handle_drag() {
    let (mut document, mut v) = rig();
    let reg = registry();
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    let before = param(&document, "band2.freq");
    let a = v.handle(&down(h), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    for i in 1..6 {
        let a = v.handle(
            &mv(Point::new(h.x + 8.0 * f64::from(i), h.y)),
            document.doc(),
            &reg,
        );
        apply(&mut document, &mut v, &a);
    }
    assert!(param(&document, "band2.freq") > before);
    let a = v.handle(
        &support::song::press(atomik_ui::Key::Escape, Modifiers::NONE),
        document.doc(),
        &reg,
    );
    apply(&mut document, &mut v, &a);
    assert!((param(&document, "band2.freq") - before).abs() < 1e-3);
    assert!(a.contains(&UiAction::EndMerge));
}

#[test]
fn the_back_of_the_panel_has_no_handles() {
    let (_, mut v) = rig();
    v.set_flip(Flip::Back);
    assert!(v.eq_handle_center(&key("fx"), 1).is_none());
}

#[test]
fn every_control_of_the_panel_explains_itself_in_one_short_line() {
    let (document, v) = rig();
    let desc = registry().get("atomik.dynamic-eq").unwrap().descriptor;
    for p in desc.params {
        if p.key == "bypass" || p.key.starts_with("internal.") {
            continue;
        }
        let text = fx::dyneq_help(p.key).unwrap_or_else(|| panic!("{} has no help", p.key));
        assert!(text.len() <= 90, "{}: {} characters", p.key, text.len());
        let at = v
            .control_center(&key("fx"), p.key)
            .unwrap_or_else(|| panic!("{} is not on the panel", p.key));
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{} has no tooltip", p.key));
        assert_eq!(help.text, text, "{}", p.key);
    }
    let _ = document;
}

fn set_band_on(document: &mut Document, band: usize, value: f32) {
    document
        .apply(
            Command::SetParam {
                key: key("fx"),
                param: format!("band{band}.on"),
                value,
                merge: None,
            },
            &registry(),
        )
        .unwrap();
}

#[test]
fn find_and_cut_are_buttons_that_ask_the_app_and_change_nothing_themselves() {
    use atomik_ui::resonance_marks::ResonanceBtn;
    let (document, mut v) = rig();
    let reg = registry();
    for (btn, want_find) in [(ResonanceBtn::Find, true), (ResonanceBtn::Cut, false)] {
        let at = v.resonance_button_center(&key("fx"), btn).unwrap();
        let a = v.handle(&down(at), document.doc(), &reg);
        let a2 = v.handle(&up(at), document.doc(), &reg);
        let all: Vec<UiAction> = a.into_iter().chain(a2).collect();
        let asked = all.iter().any(|x| match x {
            UiAction::FindResonances(k) => want_find && *k == key("fx"),
            UiAction::CutResonances(k) => !want_find && *k == key("fx"),
            _ => false,
        });
        assert!(asked, "{btn:?}: {all:?}");
        assert!(commands(&all).is_empty(), "the buttons never edit the song");
    }
    v.set_flip(Flip::Back);
    assert!(
        v.resonance_button_center(&key("fx"), ResonanceBtn::Find)
            .is_none()
    );
}

#[test]
fn the_markers_and_the_busy_state_are_drawn_and_shown_again_after_a_rebuild() {
    use atomik_ui::resonance_marks::ResonanceMark;
    let (mut document, mut v) = rig();
    // The three bells are off: free for the cuts.
    for band in 2..=4 {
        set_band_on(&mut document, band, 0.0);
    }
    v.sync(document.doc(), &registry());
    let plain = json(&mut v, &document);
    assert!(texts(&mut v, &document).iter().any(|t| t == "FIND"));
    assert!(
        !texts(&mut v, &document)
            .iter()
            .any(|t| t.starts_with("CUT "))
    );
    let marks = [
        ResonanceMark {
            freq_hz: 1_200.0,
            q: 8.0,
            prominence_db: 14.0,
        },
        ResonanceMark {
            freq_hz: 320.0,
            q: 3.0,
            prominence_db: 9.0,
        },
    ];
    v.set_resonances(&key("fx"), &marks, false);
    assert_eq!(v.resonance_marks(&key("fx")), &marks);
    let shown = json(&mut v, &document);
    assert_ne!(plain, shown, "the markers change the picture");
    let t = texts(&mut v, &document);
    // The three bells are free: CUT says how many of the two resonances they can take.
    assert!(t.iter().any(|x| x == "CUT 2"), "{t:?}");
    assert!(t.iter().any(|x| x == "1") && t.iter().any(|x| x == "2"));
    // A rebuild of the view (a document change) keeps them.
    v.sync(document.doc(), &registry());
    assert_eq!(v.resonance_marks(&key("fx")), &marks);
    // Busy: FIND shows it is working.
    v.set_resonances(&key("fx"), &[], true);
    assert!(texts(&mut v, &document).iter().any(|t| t == "..."));
    assert!(v.resonance_marks(&key("fx")).is_empty());
    // Other devices and unknown keys are ignored.
    v.set_resonances(&key("nothing"), &marks, false);
}

#[test]
fn a_bell_that_is_on_is_not_counted_as_free_on_the_cut_button() {
    use atomik_ui::resonance_marks::ResonanceMark;
    let (mut document, mut v) = rig();
    for band in 2..=4 {
        set_band_on(&mut document, band, 1.0);
    }
    v.sync(document.doc(), &registry());
    v.set_resonances(
        &key("fx"),
        &[ResonanceMark {
            freq_hz: 900.0,
            q: 5.0,
            prominence_db: 10.0,
        }],
        false,
    );
    let t = texts(&mut v, &document);
    assert!(t.iter().any(|x| x == "CUT"), "no free bell: {t:?}");
    assert!(!t.iter().any(|x| x.starts_with("CUT ")));
}

#[test]
fn the_finder_buttons_and_delta_explain_themselves() {
    use atomik_ui::resonance_marks::ResonanceBtn;
    let (_, v) = rig();
    for btn in [ResonanceBtn::Find, ResonanceBtn::Cut] {
        let at = v.resonance_button_center(&key("fx"), btn).unwrap();
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{btn:?} has no tooltip"));
        assert!(help.text.len() <= 100, "{}", help.text);
    }
}
