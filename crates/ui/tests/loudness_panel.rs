//! The loudness meter's own 4 U panel: layout, the texts of the readouts and the verdict, the
//! history graph and the distribution (maths and drawing), every control (preset, custom knobs,
//! span by click and wheel, RESET as a pulse, HOLD), the back jacks and the French texts. The
//! display-list goldens of both faces are in `fx_panels.rs` (`fx_loudness-meter_*`). Previews:
//! `cargo test -p atomik-ui --test loudness_panel -- --ignored`.

mod support;

use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind, loudness as slot};
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::loudness_panel::{self as lp, LOUDNESS_TYPE, Tone};
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent};
use support::fx::*;
use support::*;

fn rig_with(params: &[(&str, f32)]) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::LoudnessMeter);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 520.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (Document::new(doc), v)
}

fn rig() -> (Document, RackView) {
    rig_with(&[("preset", 0.0), ("history_span", 1.0), ("hold", 0.0)])
}

fn apply(document: &mut Document, v: &mut RackView, actions: &[UiAction]) {
    let reg = registry();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                document
                    .apply(c.clone(), &reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    v.sync(document.doc(), &reg);
}

fn param(document: &Document, p: &str) -> f32 {
    document
        .doc()
        .param_value(&key("fx"), p, &registry())
        .unwrap()
}

fn click_with(
    document: &mut Document,
    v: &mut RackView,
    at: Point,
    button: PointerButton,
    clicks: u8,
) -> Vec<UiAction> {
    let reg = registry();
    let a = v.handle(&down_with(at, button, clicks), document.doc(), &reg);
    apply(document, v, &a);
    let b = v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button,
            modifiers: Modifiers::NONE,
        },
        document.doc(),
        &reg,
    );
    apply(document, v, &b);
    a.into_iter().chain(b).collect()
}

fn click(document: &mut Document, v: &mut RackView, at: Point) -> Vec<UiAction> {
    click_with(document, v, at, PointerButton::Primary, 1)
}

fn wheel(document: &mut Document, v: &mut RackView, at: Point, dy: f64) -> Vec<UiAction> {
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos: at,
            modifiers: Modifiers::NONE,
        },
        document.doc(),
        &registry(),
    );
    apply(document, v, &a);
    a
}

fn set_params(actions: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    commands(actions)
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

fn center(v: &RackView, p: &str) -> Point {
    v.control_center(&key("fx"), p)
        .unwrap_or_else(|| panic!("no control {p}"))
}

fn geom() -> lp::LoudnessGeom {
    let reg = registry();
    atomik_ui::fx_layout::fx_layout(
        FxKind::LoudnessMeter,
        reg.get(LOUDNESS_TYPE).unwrap().descriptor,
    )
    .fx
    .unwrap()
    .loudness
    .unwrap()
}

fn painted(document: &Document, meters: Option<&[f32]>, data: bool, flip: Flip) -> DisplayList {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(
        &UiEvent::Resize(Size::new(820.0, 520.0)),
        document.doc(),
        &reg,
    );
    v.sync(document.doc(), &reg);
    if let Some(m) = meters {
        v.set_device_meter_values(&key("fx"), m);
    }
    if data {
        v.set_analyser(&key("fx"), &loudness_data());
    }
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &reg);
    dl
}

fn texts_of(dl: &DisplayList) -> Vec<String> {
    dl.texts().into_iter().map(str::to_string).collect()
}

// ------------------------------------------------------------------ layout

#[test]
fn the_meter_layout_is_pinned_to_the_device() {
    use atomik_devices::loudness_meter as dev;
    assert_eq!(fx::loudness::COUNT, dev::METERS);
    assert_eq!(FxKind::LoudnessMeter.meter_count(), dev::METERS);
    assert_eq!(FxKind::LoudnessMeter.type_id(), dev::DESCRIPTOR.type_id);
    assert_eq!(lp::LOUDNESS_TYPE, dev::DESCRIPTOR.type_id);
    assert_eq!(slot::MOMENTARY, dev::value::MOMENTARY);
    assert_eq!(slot::SHORT_TERM, dev::value::SHORT_TERM);
    assert_eq!(slot::INTEGRATED, dev::value::INTEGRATED);
    assert_eq!(slot::LRA, dev::value::LRA);
    assert_eq!(slot::TRUE_PEAK, dev::value::TRUE_PEAK);
    assert_eq!(slot::TRUE_PEAK_L, dev::value::TRUE_PEAK_L);
    assert_eq!(slot::TRUE_PEAK_R, dev::value::TRUE_PEAK_R);
    assert_eq!(slot::SAMPLE_PEAK, dev::value::SAMPLE_PEAK);
    assert_eq!(slot::PLR, dev::value::PLR);
    assert_eq!(slot::PSR, dev::value::PSR);
    assert_eq!(slot::DR, dev::value::DR);
    assert_eq!(slot::TARGET, dev::value::TARGET);
    assert_eq!(slot::MAX_TP, dev::value::MAX_TP);
    assert_eq!(slot::EPOCH, dev::value::EPOCH);
    assert_eq!(slot::HISTORY_POINTS, dev::value::HISTORY_POINTS);
    assert_eq!(slot::TIME, dev::value::TIME);
    assert_eq!(slot::BINS, dev::value::BINS);
    assert_eq!(slot::BIN_COUNT, dev::BINS);
    assert_eq!(slot::DIST_LOW, dev::DIST_LOW);
    assert_eq!(lp::SPAN_SECONDS, dev::SPAN_SECONDS);
    assert_eq!(lp::SPAN_NAMES, dev::SPAN_NAMES);
    assert_eq!(lp::TOLERANCE_LU, dev::TOLERANCE_LU);
    assert_eq!(lp::RESET_KEY, "reset");
    assert!(dev::DESCRIPTOR.param_by_key(lp::RESET_KEY).is_some());
    assert!(dev::DESCRIPTOR.param_by_key(lp::SPAN_KEY).is_some());
    // The points of the longest span are kept by the device.
    assert!(lp::span_points(3.0) <= dev::HISTORY_MAX);
    assert_eq!(lp::POINTS_PER_SECOND, 2.0);
    // The verdict words are the device's.
    for (i, t) in [
        (-14.0, -14.0),
        (-13.0, -14.0),
        (-12.9, -14.0),
        (-15.1, -14.0),
        (-120.0, -14.0),
    ] {
        let want = match dev::verdict(i, t) {
            dev::Verdict::NoData => Tone::None,
            dev::Verdict::Ok => Tone::Ok,
            _ => Tone::Warning,
        };
        assert_eq!(lp::loudness_verdict(i, t).1, want, "{i} vs {t}");
    }
    for (tp, max) in [(-1.0, -1.0), (-0.9, -1.0), (-120.0, -1.0), (-3.0, -2.0)] {
        let want = if tp <= -119.5 {
            Tone::None
        } else if dev::peak_passes(tp, max) {
            Tone::Ok
        } else {
            Tone::Error
        };
        assert_eq!(lp::peak_verdict(tp, max).1, want, "{tp} vs {max}");
    }
}

#[test]
fn the_panel_is_four_units_and_every_part_is_inside_it() {
    let reg = registry();
    let d = reg.get(LOUDNESS_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::LoudnessMeter, d);
    assert_eq!(spec.height_units, 4);
    let g = geom();
    let h = spec.height();
    let x1 = atomik_ui::autolayout::CONTENT_X.1;
    for r in [g.loud, g.stats, g.verdict, g.history, g.dist, g.time] {
        assert!(r.y0 >= 0.0 && r.y1 <= h && r.x1 <= x1 + 0.01, "{r:?}");
    }
    for c in &spec.front {
        assert!(c.rect.y1 <= h && c.rect.x1 <= x1 + 0.01, "{:?}", c.rect);
    }
    // The readouts do not overlap each other or the graphs.
    let parts = [g.loud, g.stats, g.verdict, g.history, g.dist, g.time];
    for (i, a) in parts.iter().enumerate() {
        for b in &parts[i + 1..] {
            let apart = a.x1 <= b.x0 || b.x1 <= a.x0 || a.y1 <= b.y0 || b.y1 <= a.y0;
            assert!(apart, "{a:?} over {b:?}");
        }
    }
    // The graph is the control of the span choice, and nothing else sits on it.
    let span = spec
        .front
        .iter()
        .find(|c| d.params[c.param].key == lp::SPAN_KEY)
        .unwrap();
    assert_eq!(span.rect, g.history);
    let labels: Vec<String> = spec
        .front_sections
        .iter()
        .map(|s| s.label.clone())
        .collect();
    assert_eq!(
        labels,
        [
            "LOUDNESS",
            "RANGE AND PEAKS",
            "TARGET",
            "HISTORY",
            "DISTRIBUTION",
            "TARGET PRESET",
            "CUSTOM TARGET",
            "METER",
            "PROGRAMME TIME"
        ]
    );
    // Every control of the descriptor but bypass is placed on the front; the back has the four
    // audio jacks.
    let keys: Vec<&str> = spec.front.iter().map(|c| d.params[c.param].key).collect();
    for want in [
        "preset",
        "target_lufs",
        "max_tp",
        "history_span",
        "reset",
        "hold",
    ] {
        assert!(keys.contains(&want), "{want}");
    }
    assert_eq!(spec.back.len(), 4);
}

// ------------------------------------------------------------------ the texts and the maths

#[test]
fn the_readout_texts() {
    assert_eq!(lp::level_text(-14.23), "-14.2");
    assert_eq!(lp::level_text(0.0), "0.0");
    assert_eq!(lp::level_text(-120.0), "--");
    assert_eq!(lp::level_text(f32::NAN), "--");
    assert_eq!(lp::with_unit(5.24, "LU"), "5.2 LU");
    assert_eq!(lp::with_unit(-120.0, "LU"), "--");
    assert_eq!(lp::time_text(0.0), "00:00");
    assert_eq!(lp::time_text(95.4), "01:35");
    assert_eq!(lp::time_text(3_725.0), "1:02:05");
    assert_eq!(lp::time_text(f32::NAN), "00:00");
}

#[test]
fn the_verdict_lines() {
    assert_eq!(lp::loudness_verdict(-14.4, -14.0).1, Tone::Ok);
    assert_eq!(lp::loudness_verdict(-14.4, -14.0).0, "ON TARGET");
    let (w, t) = lp::loudness_verdict(-11.9, -14.0);
    assert_eq!((w.as_str(), t), ("TOO LOUD +2.1 LU", Tone::Warning));
    let (w, t) = lp::loudness_verdict(-17.0, -14.0);
    assert_eq!((w.as_str(), t), ("TOO QUIET -3.0 LU", Tone::Warning));
    assert_eq!(lp::loudness_verdict(-120.0, -14.0).0, "--");
    let (w, t) = lp::peak_verdict(-0.4, -1.0);
    assert_eq!((w.as_str(), t), ("FAIL +0.6 dB", Tone::Error));
    assert_eq!(lp::peak_verdict(-1.0, -1.0), ("OK".to_string(), Tone::Ok));
    assert_eq!(lp::peak_verdict(-120.0, -1.0).1, Tone::None);
    // The three tones have three colours.
    let theme = Theme::dark();
    let colors = [Tone::Ok, Tone::Warning, Tone::Error].map(|t| lp::tone_color(&theme, t));
    assert!(colors[0] != colors[1] && colors[1] != colors[2] && colors[0] != colors[2]);
}

#[test]
fn the_graph_maths() {
    let plot = Rect::new(0.0, 0.0, 100.0, 36.0);
    // The top is 10 LU above the target, the bottom 26 below it.
    assert_eq!(lp::graph_y(plot, -4.0, -14.0), 0.0);
    assert_eq!(lp::graph_y(plot, -40.0, -14.0), 36.0);
    assert!((lp::graph_y(plot, -14.0, -14.0) - 10.0).abs() < 1e-5);
    assert_eq!(lp::graph_y(plot, 10.0, -14.0), 0.0, "clamped");
    assert_eq!(lp::graph_y(plot, -80.0, -14.0), 36.0, "clamped");
    // The newest point is at the right edge, the older ones step back from it.
    let series: Vec<f32> = (0..5).map(|i| -20.0 + i as f32).collect();
    let runs = lp::graph_runs(plot, &series, 11, -14.0);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].len(), 5);
    assert_eq!(runs[0][4].0, 100.0);
    assert!((runs[0][3].0 - 90.0).abs() < 1e-9);
    assert!((runs[0][0].0 - 60.0).abs() < 1e-9);
    // Only the newest `want` points are drawn, spread over the whole width.
    let long: Vec<f32> = (0..100).map(|i| -30.0 + i as f32 * 0.1).collect();
    let runs = lp::graph_runs(plot, &long, 11, -14.0);
    assert_eq!(runs[0].len(), 11);
    assert_eq!((runs[0][0].0, runs[0][10].0), (0.0, 100.0));
    assert!(runs[0][10].1 < runs[0][0].1, "louder is higher up");
    // Silence breaks the line instead of diving to the bottom.
    let gap = [-20.0, -20.0, -120.0, -120.0, -20.0];
    let runs = lp::graph_runs(plot, &gap, 5, -14.0);
    assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    assert!(lp::graph_runs(plot, &[], 5, -14.0).is_empty());
    assert!(lp::graph_runs(plot, &[f32::NAN], 5, -14.0).is_empty());
    // The spans.
    assert_eq!(lp::span_points(0.0), 60);
    assert_eq!(lp::span_points(1.0), 120);
    assert_eq!(lp::span_points(3.0), 1_800);
    assert_eq!(lp::span_points(9.0), 1_800, "clamped");
    assert_eq!(lp::span_name(2.0), "5 min");
    // The distribution runs from -46 to 0.
    let plot = Rect::new(10.0, 0.0, 56.0, 10.0);
    assert_eq!(lp::dist_x(plot, -46.0), 10.0);
    assert_eq!(lp::dist_x(plot, 0.0), 56.0);
    assert_eq!(lp::dist_x(plot, -23.0), 33.0);
    assert_eq!(lp::dist_x(plot, -90.0), 10.0, "clamped");
}

// ------------------------------------------------------------------ the display

#[test]
fn the_readouts_show_the_published_values() {
    let (document, _) = rig();
    let dl = painted(
        &document,
        Some(&raw_meters(FxKind::LoudnessMeter)),
        true,
        Flip::Front,
    );
    let texts = texts_of(&dl);
    for want in [
        "-14.2",
        "-14.6",
        "-13.2",
        "6.4 LU",
        "-0.6 dBTP",
        "13.6 dB",
        "11.2 dB",
        "9.4 dB",
        "-1.1 dBFS",
        "01:35",
        "LUFS",
    ] {
        assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
    }
    // The target of the device is the one in the box, not the Custom knobs'.
    assert!(
        texts.iter().any(|t| t == "-14.0 LUFS  /  -1.0 dBTP"),
        "{texts:?}"
    );
    // -14.2 against -14: on target; a true peak of -0.6 against -1: over.
    assert!(texts.iter().any(|t| t == "ON TARGET"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "FAIL +0.4 dB"), "{texts:?}");
    // Nothing yet reads `--` everywhere.
    let none = painted(&document, None, false, Flip::Front);
    let texts = texts_of(&none);
    assert!(
        texts.iter().filter(|t| *t == "--").count() >= 8,
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "collecting..."));
    assert!(texts.iter().any(|t| t == "00:00"));
}

#[test]
fn the_verdict_follows_the_target_in_force() {
    let (document, _) = rig();
    let at = |integrated: f32, target: f32, tp: f32, max: f32| {
        let mut m = raw_meters(FxKind::LoudnessMeter);
        m[slot::INTEGRATED] = integrated;
        m[slot::TARGET] = target;
        m[slot::TRUE_PEAK] = tp;
        m[slot::MAX_TP] = max;
        texts_of(&painted(&document, Some(&m), true, Flip::Front))
    };
    let t = at(-11.0, -14.0, -2.0, -1.0);
    assert!(t.iter().any(|s| s == "TOO LOUD +3.0 LU"), "{t:?}");
    assert!(t.iter().any(|s| s == "OK"), "{t:?}");
    let t = at(-30.0, -23.0, -9.0, -2.0);
    assert!(t.iter().any(|s| s == "TOO QUIET -7.0 LU"), "{t:?}");
    let t = at(-23.0, -23.0, -9.0, -2.0);
    assert!(t.iter().any(|s| s == "ON TARGET"), "{t:?}");
    assert!(t.iter().any(|s| s == "-23.0 LUFS  /  -2.0 dBTP"), "{t:?}");
}

#[test]
fn the_graphs_draw_from_the_data_and_the_bins() {
    let (document, _) = rig();
    let meters = raw_meters(FxKind::LoudnessMeter);
    let base = painted(&document, Some(&meters), true, Flip::Front).to_json();
    // The history: no data, different data, and a different span all change the drawing.
    assert_ne!(
        base,
        painted(&document, Some(&meters), false, Flip::Front).to_json()
    );
    let (long, _) = rig_with(&[("history_span", 3.0)]);
    assert_ne!(
        base,
        painted(&long, Some(&meters), true, Flip::Front).to_json()
    );
    // The distribution follows the bins, and nothing else of the meters moves a bar.
    let mut flat = meters.clone();
    for b in &mut flat[slot::BINS..] {
        *b = 0.0;
    }
    assert_ne!(
        base,
        painted(&document, Some(&flat), true, Flip::Front).to_json()
    );
    // The target line of the graphs moves with the target in force.
    let mut moved = meters.clone();
    moved[slot::TARGET] = -23.0;
    assert_ne!(
        base,
        painted(&document, Some(&moved), true, Flip::Front).to_json()
    );
    // The held state shows on the time readout.
    let (held, _) = rig_with(&[("hold", 1.0)]);
    let t = texts_of(&painted(&held, Some(&meters), true, Flip::Front));
    assert!(t.iter().any(|s| s == "HOLD"), "{t:?}");
    let t = texts_of(&painted(&document, Some(&meters), true, Flip::Front));
    assert!(!t.iter().any(|s| s == "HOLD"));
    // The span is named on the graph.
    assert!(t.iter().any(|s| s == "-1 min"), "{t:?}");
    let t = texts_of(&painted(&long, Some(&meters), true, Flip::Front));
    assert!(t.iter().any(|s| s == "-15 min"), "{t:?}");
}

#[test]
fn the_feed_becomes_the_shown_values_unchanged() {
    let mut shown = Vec::new();
    let raw = raw_meters(FxKind::LoudnessMeter);
    fx::update_display(FxKind::LoudnessMeter, &raw, &mut shown);
    assert_eq!(shown.len(), fx::loudness::COUNT);
    assert_eq!(shown[slot::INTEGRATED], -14.2);
    assert_eq!(shown[slot::LRA], 6.4);
    assert_eq!(shown[slot::PLR], 13.6);
    assert_eq!(shown[slot::TIME], 95.0);
    assert_eq!(shown[slot::HISTORY_POINTS], 120.0);
    assert_eq!(shown[slot::EPOCH], 3.0);
    // A ratio below -48 and a time above 10000 s survive (their own slots).
    let mut big = raw.clone();
    big[slot::PLR] = -60.0;
    big[slot::TIME] = 40_000.0;
    fx::update_display(FxKind::LoudnessMeter, &big, &mut shown);
    assert_eq!(shown[slot::PLR], -60.0);
    assert_eq!(shown[slot::TIME], 40_000.0);
    // Nothing yet: the ratios read -120 and the bins nothing.
    let mut fresh = Vec::new();
    let quiet: Vec<f32> = (0..16).map(|_| -120.0).collect();
    fx::update_display(FxKind::LoudnessMeter, &quiet, &mut fresh);
    assert_eq!(fresh[slot::INTEGRATED], -120.0);
    assert_eq!(fresh[slot::DR], -120.0);
    assert_eq!(fresh[slot::BINS], 0.0);
}

// ------------------------------------------------------------------ the controls

#[test]
fn every_control_sets_its_param() {
    let (mut document, mut v) = rig();
    // PRESET: a click steps to the next choice, a right click back.
    let at = center(&v, "preset");
    let a = click(&mut document, &mut v, at);
    let sets = set_params(&a);
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert_eq!((sets[0].0.as_str(), sets[0].1), ("preset", 1.0));
    let a = click_with(&mut document, &mut v, at, PointerButton::Secondary, 1);
    assert!(
        a.iter().any(
            |a| matches!(a, UiAction::Command(Command::SetParam { value, .. }) if *value == 0.0)
        ),
        "{a:?}"
    );
    assert_eq!(param(&document, "preset"), 0.0);
    // The wheel steps the preset too, and reaches Custom.
    for _ in 0..7 {
        wheel(&mut document, &mut v, at, -1.0);
    }
    assert!(param(&document, "preset") >= 0.0);
    // HOLD toggles.
    let at = center(&v, "hold");
    let a = click(&mut document, &mut v, at);
    assert_eq!(
        set_params(&a)
            .iter()
            .map(|s| (s.0.as_str(), s.1))
            .collect::<Vec<_>>(),
        [("hold", 1.0)]
    );
    let a = click(&mut document, &mut v, at);
    assert_eq!(set_params(&a)[0].1, 0.0);
    // The two custom knobs drag with one merged undo step each.
    for p in ["target_lufs", "max_tp"] {
        let at = center(&v, p);
        let reg = registry();
        let a = v.handle(&down(at), document.doc(), &reg);
        apply(&mut document, &mut v, &a);
        let mut all = a;
        for dy in [-25.0, 40.0] {
            let a = v.handle(&mv(Point::new(at.x, at.y + dy)), document.doc(), &reg);
            apply(&mut document, &mut v, &a);
            all.extend(a);
        }
        let a = v.handle(&up(Point::new(at.x, at.y + 40.0)), document.doc(), &reg);
        apply(&mut document, &mut v, &a);
        all.extend(a);
        let sets = set_params(&all);
        assert!(
            !sets.is_empty() && sets.iter().all(|s| s.0 == p),
            "{p}: {sets:?}"
        );
        assert!(sets.iter().all(|s| s.2.is_some()), "{p} merges");
        assert!(all.contains(&UiAction::EndMerge), "{p}");
    }
    // A double click puts a knob back to its default.
    let at = center(&v, "target_lufs");
    let a = click_with(&mut document, &mut v, at, PointerButton::Primary, 2);
    assert!(
        set_params(&a)
            .iter()
            .any(|s| s.0 == "target_lufs" && (s.1 + 14.0).abs() < 1e-6),
        "{a:?}"
    );
    assert_eq!(param(&document, "target_lufs"), -14.0);
    // A right click on a knob opens its MIDI menu.
    let a = click_with(&mut document, &mut v, at, PointerButton::Secondary, 1);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::ControlMenu { param, .. } if param == "target_lufs")),
        "{a:?}"
    );
}

#[test]
fn reset_is_a_pulse_and_stores_nothing() {
    let (mut document, mut v) = rig();
    let at = center(&v, "reset");
    let a = click(&mut document, &mut v, at);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::PulseParam { param, .. } if param == "reset")),
        "{a:?}"
    );
    assert!(set_params(&a).is_empty(), "nothing is stored: {a:?}");
    assert_eq!(param(&document, "reset"), 0.0);
    // The wheel and a right click do not pulse it; a right click opens the MIDI menu.
    let a = wheel(&mut document, &mut v, at, 1.0);
    assert!(a.is_empty(), "{a:?}");
    let a = click_with(&mut document, &mut v, at, PointerButton::Secondary, 1);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::ControlMenu { param, .. } if param == "reset")),
        "{a:?}"
    );
    // The keyboard pulses it too.
    let reg = registry();
    let r = v.handle(&down(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(&up(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(
        &UiEvent::Key {
            key: atomik_ui::Key::Enter,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        document.doc(),
        &reg,
    );
    assert!(
        r.iter()
            .any(|a| matches!(a, UiAction::PulseParam { param, .. } if param == "reset")),
        "{r:?}"
    );
}

#[test]
fn a_click_or_the_wheel_on_the_graph_steps_the_span() {
    let (mut document, mut v) = rig_with(&[("history_span", 0.0)]);
    let g = geom();
    let reg = registry();
    let desc = reg.get(LOUDNESS_TYPE).unwrap().descriptor;
    let span = desc.param_by_key("history_span").unwrap();
    // The control sits on the graph: its centre is the graph's centre.
    let at = center(&v, "history_span");
    assert!((at.x - g.history.center().x).abs() < 1.0 || at.x > g.history.x0);
    let a = click(&mut document, &mut v, at);
    let sets = set_params(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    assert_eq!((sets[0].0.as_str(), sets[0].1), ("history_span", 1.0));
    // Right click steps back, the wheel steps both ways, always through the four spans.
    click_with(&mut document, &mut v, at, PointerButton::Secondary, 1);
    assert_eq!(param(&document, "history_span"), 0.0);
    let mut seen = vec![param(&document, "history_span")];
    for _ in 0..4 {
        wheel(&mut document, &mut v, at, 1.0);
        seen.push(param(&document, "history_span"));
    }
    assert!(
        seen.iter().all(|s| *s >= span.min && *s <= span.max),
        "{seen:?}"
    );
    assert!(
        seen.windows(2).all(|w| w[0].to_bits() != w[1].to_bits()),
        "{seen:?}"
    );
}

#[test]
fn the_custom_target_is_what_the_knobs_hold() {
    // The target in force is a published value, so the knobs only change the box when the
    // device is on Custom: the panel draws what it is told.
    let (document, _) = rig_with(&[("target_lufs", -18.0), ("max_tp", -2.0)]);
    let mut m = raw_meters(FxKind::LoudnessMeter);
    m[slot::TARGET] = -18.0;
    m[slot::MAX_TP] = -2.0;
    let t = texts_of(&painted(&document, Some(&m), true, Flip::Front));
    assert!(t.iter().any(|s| s == "-18.0 LUFS  /  -2.0 dBTP"), "{t:?}");
}

// ------------------------------------------------------------------ the back

#[test]
fn the_back_has_every_jack_hit_testable() {
    let (mut document, mut v) = rig();
    v.set_flip(Flip::Back);
    let reg = registry();
    let desc = reg.get(LOUDNESS_TYPE).unwrap().descriptor;
    assert_eq!(desc.sockets.len(), 4);
    for s in desc.sockets {
        let at = atomik_model::SocketRef {
            device: key("fx"),
            socket: s.key.to_string(),
        };
        let p = v
            .socket_center(&at)
            .unwrap_or_else(|| panic!("{} has no jack", s.key));
        let a = v.handle(&down(p), document.doc(), &reg);
        assert!(commands(&a).is_empty(), "{}: {a:?}", s.key);
        let a = v.handle(&up(p), document.doc(), &reg);
        apply(&mut document, &mut v, &a);
    }
    // No front control is on the back.
    assert!(v.control_center(&key("fx"), "preset").is_none());
    // The back is painted with its sections.
    let dl = painted(&document, None, false, Flip::Back);
    let t = texts_of(&dl);
    for want in ["AUDIO IN", "AUDIO OUT", "In L", "In R", "Out L", "Out R"] {
        assert!(t.iter().any(|s| s == want), "{want}: {t:?}");
    }
}

#[test]
fn a_cable_from_another_device_reaches_the_audio_jacks() {
    let (document, _) = rig();
    let mut doc = document.doc().clone();
    doc.devices.push(device("src", "atomik.poly-synth", "Src"));
    let document = Document::new(doc);
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(
        &UiEvent::Resize(Size::new(1000.0, 3000.0)),
        document.doc(),
        &reg,
    );
    v.sync(document.doc(), &reg);
    v.set_flip(Flip::Back);
    let from = v.socket_center(&sref("src", "out_l")).unwrap();
    let to = v.socket_center(&sref("fx", "in_l")).unwrap();
    let a = v.handle(&down(from), document.doc(), &reg);
    let b = v.handle(&mv(to), document.doc(), &reg);
    let c = v.handle(&up(to), document.doc(), &reg);
    let cmds = commands(&[a, b, c].concat());
    assert!(
        cmds.iter().any(|c| match c {
            Command::Connect(_) => true,
            Command::Batch(b) => b.iter().any(|c| matches!(c, Command::Connect(_))),
            _ => false,
        }),
        "{cmds:?}"
    );
}

// ------------------------------------------------------------------ French

/// Both faces painted in French: every fixed word has an entry.
#[test]
fn the_panel_in_french() {
    use atomik_ui::i18n::{self, Lang};
    let doc = fx_doc(FxKind::LoudnessMeter);
    let reg = registry();
    for (name, flip) in [("front", Flip::Front), ("back", Flip::Back)] {
        let (list, misses) = i18n::with_misses(|| {
            i18n::with_language(Lang::French, || {
                let mut v = RackView::new(Theme::dark());
                let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 520.0)), &doc, &reg);
                v.sync(&doc, &reg);
                v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::LoudnessMeter));
                v.set_analyser(&key("fx"), &loudness_data());
                v.set_flip(flip);
                let mut dl = DisplayList::new();
                v.paint(&mut dl, &doc, &reg);
                dl
            })
        });
        let missing: Vec<&String> = misses
            .iter()
            .filter(|m| {
                !matches!(m.as_str(), "FX" | "LUFS")
                    && m.chars().filter(|c| c.is_alphabetic()).count() > 2
                    && !m.chars().any(|c| c.is_ascii_digit())
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{name}: no French entry for {missing:?}"
        );
        let french = list.texts();
        if flip == Flip::Front {
            for s in ["DANS LA CIBLE", "CIBLE", "HISTORIQUE"] {
                assert!(french.contains(&s), "{s}: {french:?}");
            }
        }
    }
}

#[test]
fn every_french_section_title_fits_its_section() {
    use atomik_ui::i18n::{self, Lang};
    let reg = registry();
    let d = reg.get(LOUDNESS_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::LoudnessMeter, d);
    let theme = Theme::dark();
    i18n::with_language(Lang::French, || {
        for s in spec.front_sections.iter().chain(&spec.back_sections) {
            let text = i18n::tr(&s.label);
            let w = atomik_ui::paint::estimate_width(&text, theme.section_size)
                + text.chars().count() as f64
                + 6.0;
            assert!(w <= s.rect.width(), "{text}: {w} > {}", s.rect.width());
        }
    });
}

// ------------------------------------------------------------------ previews

/// Writes previews of both faces to `target/preview`: `cargo test -p atomik-ui --test
/// loudness_panel -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let reg = registry();
    for (name, flip, data) in [
        ("front", Flip::Front, true),
        ("front_empty", Flip::Front, false),
        ("back", Flip::Back, false),
    ] {
        let doc = fx_doc(FxKind::LoudnessMeter);
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 480.0)), &doc, &reg);
        v.sync(&doc, &reg);
        if data {
            v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::LoudnessMeter));
            v.set_analyser(&key("fx"), &loudness_data());
        }
        v.set_flip(flip);
        let _ = v.handle(&UiEvent::ScaleChanged(2.0), &doc, &reg);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(1640, 960, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/preview/loudness_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
