//! The six effect panels: layouts, display-list goldens of both
//! faces with representative meter values, the meter feed, the EQ handles (drag, merge ids,
//! clamping, wheel Q, double-click), and a proptest that every control only emits `SetParam`s
//! the model accepts. The sidechain helper has its own file (`sidechain_helper.rs`).
//! Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test fx_panels`.

mod support;

use atomik_model::{Command, Document};
use atomik_ui::autolayout::{CONTENT_X, auto_layout, has_strip_bypass};
use atomik_ui::fx::{EQ_METERS, FxKind};
use atomik_ui::fx_layout::fx_layout;
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::{DisplayList, Flip, RackView, Theme, UiAction, UiEvent};
use proptest::prelude::*;
use std::path::PathBuf;
use support::fx::*;
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

fn slug(k: FxKind) -> &'static str {
    &k.type_id()["atomik.".len()..]
}

fn record(kind: FxKind, flip: Flip, meters: bool) -> String {
    let (doc, mut v) = rack(kind);
    if meters {
        v.set_device_meter_values(&key("fx"), &raw_meters(kind));
    }
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.to_json()
}

fn texts(kind: FxKind, meters: bool) -> Vec<String> {
    let (doc, mut v) = rack(kind);
    if meters {
        v.set_device_meter_values(&key("fx"), &raw_meters(kind));
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn desc(kind: FxKind) -> &'static atomik_device_sdk::DeviceDescriptor {
    registry().get(kind.type_id()).unwrap().descriptor
}

// ------------------------------------------------------------------ layouts

#[test]
fn heights_in_rack_units_are_pinned() {
    let units = |k| fx_layout(k, desc(k)).height_units;
    assert_eq!(units(FxKind::Reverb), 2);
    assert_eq!(units(FxKind::Compressor), 3);
    assert_eq!(units(FxKind::Eq), 6);
    assert_eq!(units(FxKind::Distortion), 3);
    assert_eq!(units(FxKind::GuitarAmp), 5);
    assert_eq!(units(FxKind::Modulation), 2);
    assert_eq!(units(FxKind::Mastering), 5);
    assert_eq!(units(FxKind::Character), 3);
    assert_eq!(units(FxKind::Spectrum), 4);
    assert_eq!(units(FxKind::PhaseMeter), 3);
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

#[test]
fn every_param_and_socket_is_placed_once_and_nothing_overlaps() {
    for kind in FxKind::EFFECTS {
        let d = desc(kind);
        let spec = fx_layout(kind, d);
        let mut params: Vec<usize> = spec.front.iter().map(|c| c.param).collect();
        if kind == FxKind::Character {
            // The preset choice is the 4 x 4 matrix, not a placed control.
            let preset = d.params.iter().position(|p| p.key == "preset").unwrap();
            params.push(preset);
        }
        // The `bypass` param is placed by the switch of the name strip, never as a knob.
        assert!(has_strip_bypass(d), "{kind:?}");
        let bypass = d.params.iter().position(|p| p.key == "bypass").unwrap();
        assert!(
            !params.contains(&bypass),
            "{kind:?} places bypass as a control"
        );
        params.push(bypass);
        params.sort_unstable();
        assert_eq!(params, (0..d.params.len()).collect::<Vec<_>>(), "{kind:?}");
        let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
        sockets.sort_unstable();
        assert_eq!(
            sockets,
            (0..d.sockets.len()).collect::<Vec<_>>(),
            "{kind:?}"
        );
        let fx = spec.fx.expect("fx panel");
        let panel = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
        for r in [fx.visual, fx.plot, fx.meters] {
            if r == Rect::ZERO {
                continue;
            }
            assert!(
                r.x0 >= panel.x0 && r.x1 <= panel.x1 && r.y0 >= 0.0 && r.y1 <= panel.y1,
                "{kind:?} {r:?}"
            );
        }
        // Controls of the back face (the Character CV trims) are placed in back coordinates.
        let rects: Vec<Rect> = spec
            .front
            .iter()
            .filter(|c| !spec.on_back(c))
            .map(|c| c.rect)
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.x0 >= panel.x0 && a.x1 <= panel.x1 && a.y0 >= 0.0 && a.y1 <= panel.y1,
                "{kind:?} control {a:?}"
            );
            assert!(!overlaps(a, &fx.visual), "{kind:?} control over the visual");
            for b in &rects[i + 1..] {
                assert!(!overlaps(a, b), "{kind:?}: {a:?} {b:?}");
            }
        }
        for s in &spec.back {
            assert!(s.rect.y1 <= spec.height() && s.rect.x1 <= panel.x1);
        }
        for c in spec.front.iter().filter(|c| spec.on_back(c)) {
            assert!(c.rect.y1 <= spec.height() && c.rect.x1 <= panel.x1);
            for s in &spec.back {
                assert!(
                    !overlaps(&c.rect, &s.rect),
                    "{kind:?}: back control over a jack"
                );
            }
        }
        assert!(fx.plot.width() > 20.0 && fx.plot.height() > 20.0);
    }
}

#[test]
fn sections_are_labelled_as_documented() {
    let labels = |k: FxKind| -> Vec<String> {
        fx_layout(k, desc(k))
            .front_sections
            .into_iter()
            .map(|s| s.label)
            .collect()
    };
    for want in ["DETECTOR", "GAIN", "SIDECHAIN", "OUTPUT"] {
        assert!(
            labels(FxKind::Compressor).iter().any(|l| l == want),
            "{want}"
        );
    }
    for want in ["SPACE", "TONE", "BALANCE", "OUTPUT"] {
        assert!(labels(FxKind::Reverb).iter().any(|l| l == want), "{want}");
    }
    for want in [
        "LOW CUT",
        "LOW SHELF",
        "PEAK 1",
        "PEAK 2",
        "HIGH SHELF",
        "HIGH CUT",
    ] {
        assert!(labels(FxKind::Eq).iter().any(|l| l == want), "{want}");
    }
    for want in ["DRIVE", "TONE", "CRUSH", "OUTPUT"] {
        assert!(
            labels(FxKind::Distortion).iter().any(|l| l == want),
            "{want}"
        );
    }
    for want in [
        "AMP", "SETUP", "INPUT", "PREAMP", "TONE", "POWER", "CABINET", "CHORUS", "DELAY", "REVERB",
    ] {
        assert!(
            labels(FxKind::GuitarAmp).iter().any(|l| l == want),
            "{want}"
        );
    }
    for want in ["RATE", "VOICE", "OUTPUT"] {
        assert!(
            labels(FxKind::Modulation).iter().any(|l| l == want),
            "{want}"
        );
    }
    for want in [
        "INPUT",
        "MULTIBAND",
        "BAND 1",
        "BAND 2",
        "BAND 3",
        "STEREO",
        "LIMITER",
        "OUTPUT",
    ] {
        assert!(
            labels(FxKind::Mastering).iter().any(|l| l == want),
            "{want}"
        );
    }
    // Section titles never repeat on one panel.
    for want in ["PRESET", "SCREEN", "METER", "CONTROLS"] {
        assert!(
            labels(FxKind::Character).iter().any(|l| l == want),
            "{want}"
        );
    }
    for kind in FxKind::EFFECTS {
        let l = labels(kind);
        let mut u = l.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), l.len(), "{kind:?}: {l:?}");
    }
}

#[test]
fn other_devices_keep_their_generated_layout() {
    // The effects use their own layout; a device without a visual stays on auto_layout.
    let d = registry().get("atomik.stereo-delay").unwrap().descriptor;
    assert!(auto_layout(d).fx.is_none());
}

// ------------------------------------------------------------------ painting

#[test]
fn the_visuals_draw_their_readouts() {
    let t = texts(FxKind::Compressor, true);
    for s in ["TRANSFER", "SC: off", "IN", "OUT", "GR", "6.0:1"] {
        assert!(t.iter().any(|x| x == s), "compressor {s:?}: {t:?}");
    }
    let t = texts(FxKind::Reverb, true);
    for s in ["DECAY", "PLATE", "RT60 4.0 s", "PRE 60 ms", "TAIL"] {
        assert!(t.iter().any(|x| x == s), "reverb {s:?}: {t:?}");
    }
    let t = texts(FxKind::Eq, true);
    for s in ["RESPONSE", "100", "1k", "10k", "1", "6", "0"] {
        assert!(t.iter().any(|x| x == s), "eq {s:?}: {t:?}");
    }
    let t = texts(FxKind::Distortion, true);
    for s in ["CURVE", "TUBE", "+18.0 dB"] {
        assert!(t.iter().any(|x| x == s), "distortion {s:?}: {t:?}");
    }
    let t = texts(FxKind::Modulation, false);
    for s in ["LFO", "PHASER", "0.50 Hz", "4 STAGES"] {
        assert!(t.iter().any(|x| x == s), "modulation {s:?}: {t:?}");
    }
    let t = texts(FxKind::Mastering, true);
    for s in [
        "MOMENTARY",
        "-20.5",
        "-21.2",
        "-23.4",
        "-1.8",
        "B1",
        "LIM",
        "150",
        "3.0k",
    ] {
        assert!(t.iter().any(|x| x == s), "mastering {s:?}: {t:?}");
    }
}

#[test]
fn meter_values_change_the_painting() {
    for kind in FxKind::EFFECTS {
        if kind == FxKind::Modulation {
            // Nothing to feed: the panel is identical.
            assert_eq!(
                record(kind, Flip::Front, false),
                record(kind, Flip::Front, true)
            );
            continue;
        }
        assert_ne!(
            record(kind, Flip::Front, false),
            record(kind, Flip::Front, true),
            "{kind:?}"
        );
    }
}

#[test]
fn the_meter_feed_repaints_only_when_a_displayed_value_changes() {
    let (doc, mut v) = rack(FxKind::Compressor);
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint());
    let rev = |v: &RackView| {
        v.panel_revisions()
            .iter()
            .find(|(k, _)| *k == key("fx"))
            .unwrap()
            .1
    };
    let r0 = rev(&v);
    v.set_device_meter_values(&key("fx"), &[0.5, 0.35, 6.0]);
    assert!(v.needs_repaint());
    assert!(rev(&v) > r0);
    v.paint(&mut dl, &doc, &reg);
    // The same values again: no new revision and no repaint request.
    let r1 = rev(&v);
    v.set_device_meter_values(&key("fx"), &[0.5, 0.35, 6.0]);
    assert!(!v.needs_repaint());
    assert_eq!(rev(&v), r1);
    // Silence falls back to the floor and then settles.
    let mut n = 0;
    loop {
        v.set_device_meter_values(&key("fx"), &[0.0, 0.0, 0.0]);
        if !v.needs_repaint() {
            break;
        }
        v.paint(&mut dl, &doc, &reg);
        n += 1;
        assert!(n < 200, "meters never settle");
    }
    assert!(
        v.device_meters(&key("fx"))
            .iter()
            .take(2)
            .all(|d| *d <= -90.0)
    );
    assert_eq!(v.device_meters(&key("fx"))[2], 0.0);
    // Unknown devices and the interface are ignored.
    v.set_device_meter_values(&key("nope"), &[1.0]);
    assert!(!v.needs_repaint());
}

#[test]
fn a_hidden_panel_does_not_request_repaints() {
    let (doc, mut v) = rack(FxKind::Distortion);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    v.set_flip(Flip::Back);
    v.paint(&mut dl, &doc, &registry());
    v.set_device_meter_values(&key("fx"), &[0.7, 0.5]);
    assert!(!v.needs_repaint(), "the back face shows no meters");
    v.set_flip(Flip::Front);
    v.paint(&mut dl, &doc, &registry());
    // The list recorded for the front now shows the fed values.
    let t = v.device_meters(&key("fx")).to_vec();
    assert_eq!(t.len(), 2);
    assert!(t[0] > -10.0);
}

#[test]
fn meter_buffers_have_the_documented_sizes() {
    for kind in FxKind::EFFECTS {
        let (_, mut v) = rack(kind);
        v.set_device_meter_values(&key("fx"), &raw_meters(kind));
        assert_eq!(
            v.device_meters(&key("fx")).len(),
            kind.meter_count(),
            "{kind:?}"
        );
    }
    assert_eq!(FxKind::Eq.meter_count(), EQ_METERS);
}

#[test]
fn goldens_of_every_panel() {
    for kind in FxKind::EFFECTS {
        if kind == FxKind::Character {
            // Its goldens (front states, back, contact sheet) live in `character_panel.rs`.
            continue;
        }
        let name = slug(kind);
        check(
            &format!("fx_{name}_front_1x.json"),
            &record(kind, Flip::Front, true),
        );
        check(
            &format!("fx_{name}_back_1x.json"),
            &record(kind, Flip::Back, false),
        );
    }
}

#[test]
fn the_compressor_shows_its_sidechain_source() {
    let (mut doc, reg) = (fx_doc(FxKind::Compressor), registry());
    doc.devices
        .push(device("kick", "atomik.poly-synth", "Kick"));
    doc.devices[0].params.insert("sc_enable".into(), 1.0);
    doc.cables.push(cable(("kick", "out_l"), ("fx", "sc_in_l")));
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 900.0)), &doc, &reg);
    v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::Compressor));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let t: Vec<String> = dl.texts().into_iter().map(str::to_string).collect();
    assert!(t.iter().any(|x| x == "SC: Kick"), "{t:?}");
    check("fx_compressor_sidechain_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ EQ handles

fn eq_rig() -> (Document, RackView) {
    let (doc, mut v) = rack(FxKind::Eq);
    let document = Document::new(doc);
    v.sync(document.doc(), &registry());
    (document, v)
}

fn apply(document: &mut Document, v: &mut RackView, actions: &[UiAction]) -> Vec<Option<u64>> {
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                if let Command::SetParam { merge, .. } = c {
                    merges.push(*merge);
                }
                document
                    .apply(c.clone(), &registry())
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    v.sync(document.doc(), &registry());
    merges
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

#[test]
fn dragging_a_handle_sideways_sets_the_frequency_in_one_undo_step() {
    let (mut document, mut v) = eq_rig();
    let h = v.eq_handle_center(&key("fx"), 2).unwrap();
    let before = param(&document, "band3.freq");
    let path: Vec<Point> = (1..=10)
        .map(|i| Point::new(h.x + 6.0 * f64::from(i), h.y))
        .collect();
    let a = drag(&mut document, &mut v, h, &path);
    let sets = set_params(&a);
    assert!(sets.len() >= 5, "{sets:?}");
    assert!(
        sets.iter()
            .all(|(p, _, m)| p == "band3.freq" && m.is_some())
    );
    let id = sets[0].2;
    assert!(sets.iter().all(|(_, _, m)| *m == id), "one merge id");
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));
    let after = param(&document, "band3.freq");
    assert!(after > before * 1.3, "{before} -> {after}");
    // The handle followed the pointer.
    let moved = v.eq_handle_center(&key("fx"), 2).unwrap();
    assert!((moved.x - (h.x + 60.0)).abs() < 3.0, "{moved:?}");
    // One undo step.
    document.undo(&registry());
    assert!((param(&document, "band3.freq") - before).abs() < 1e-3);
}

#[test]
fn dragging_a_handle_up_sets_the_gain_and_is_clamped() {
    let (mut document, mut v) = eq_rig();
    let h = v.eq_handle_center(&key("fx"), 3).unwrap();
    let path = [Point::new(h.x, h.y - 6.0), Point::new(h.x, h.y - 20.0)];
    let a = drag(&mut document, &mut v, h, &path);
    let sets = set_params(&a);
    assert!(sets.iter().all(|(p, _, _)| p == "band4.gain"), "{sets:?}");
    assert!(param(&document, "band4.gain") > 9.0);
    // Far above the plot: clamped to +24 dB.
    let h = v.eq_handle_center(&key("fx"), 3).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y - 900.0)]);
    assert_eq!(param(&document, "band4.gain"), 24.0);
    // Far below: -24 dB.
    let h = v.eq_handle_center(&key("fx"), 3).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y + 900.0)]);
    assert_eq!(param(&document, "band4.gain"), -24.0);
}

#[test]
fn frequency_drags_are_clamped_to_the_band_range() {
    let (mut document, mut v) = eq_rig();
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x - 900.0, h.y)]);
    assert_eq!(param(&document, "band2.freq"), 20.0);
    let h = v.eq_handle_center(&key("fx"), 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x + 900.0, h.y)]);
    assert_eq!(
        param(&document, "band2.freq"),
        5000.0,
        "the low shelf stops at 5 kHz"
    );
}

#[test]
fn a_cut_handle_only_moves_the_frequency() {
    let (mut document, mut v) = eq_rig();
    let h = v.eq_handle_center(&key("fx"), 0).unwrap();
    // A mostly vertical drag still only touches the frequency.
    let a = drag(
        &mut document,
        &mut v,
        h,
        &[Point::new(h.x + 4.0, h.y - 40.0)],
    );
    let sets = set_params(&a);
    assert!(sets.iter().all(|(p, _, _)| p == "band1.freq"), "{sets:?}");
}

#[test]
fn the_wheel_over_a_handle_steps_q_and_a_double_click_toggles_the_band() {
    let (mut document, mut v) = eq_rig();
    let reg = registry();
    let h = v.eq_handle_center(&key("fx"), 2).unwrap();
    let q0 = param(&document, "band3.q");
    let a = v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
            pos: h,
            modifiers: atomik_ui::Modifiers::NONE,
        },
        document.doc(),
        &reg,
    );
    let sets = set_params(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    assert_eq!(sets[0].0, "band3.q");
    assert_eq!(sets[0].2, None, "a wheel step is its own undo step");
    assert!(sets[0].1 > q0);
    apply(&mut document, &mut v, &a);
    let a = v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: h,
            modifiers: atomik_ui::Modifiers::NONE,
        },
        document.doc(),
        &reg,
    );
    assert!(set_params(&a)[0].1 < sets[0].1);
    // Double-click toggles band 3 off.
    let a = v.handle(
        &down_with(h, atomik_ui::PointerButton::Primary, 2),
        document.doc(),
        &reg,
    );
    let sets = set_params(&a);
    assert_eq!(sets, vec![("band3.on".to_string(), 0.0, None)]);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&up(h), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "band3.on"), 0.0);
}

#[test]
fn escape_restores_the_start_value_of_an_eq_drag() {
    let (mut document, mut v) = eq_rig();
    let reg = registry();
    let h = v.eq_handle_center(&key("fx"), 2).unwrap();
    let before = param(&document, "band3.freq");
    let a = v.handle(&down(h), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    for i in 1..6 {
        let a = v.handle(
            &mv(Point::new(h.x + 10.0 * f64::from(i), h.y)),
            document.doc(),
            &reg,
        );
        apply(&mut document, &mut v, &a);
    }
    assert_ne!(param(&document, "band3.freq"), before);
    let a = v.handle(
        &support::song::press(atomik_ui::Key::Escape, atomik_ui::Modifiers::NONE),
        document.doc(),
        &reg,
    );
    apply(&mut document, &mut v, &a);
    assert!((param(&document, "band3.freq") - before).abs() < 1e-3);
    assert!(a.contains(&UiAction::EndMerge));
}

#[test]
fn a_press_that_misses_every_handle_selects_the_device() {
    let (document, mut v) = eq_rig();
    let reg = registry();
    let h = v.eq_handle_center(&key("fx"), 2).unwrap();
    let a = v.handle(
        &down(Point::new(h.x + 3.0, h.y - 70.0)),
        document.doc(),
        &reg,
    );
    assert!(a.contains(&UiAction::Select(key("fx"))), "{a:?}");
    assert!(commands(&a).is_empty());
}

// ------------------------------------------------------------------ every control emits valid SetParams

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn random_clicks_and_drags_only_emit_valid_set_params(
        kind in 0usize..7,
        steps in proptest::collection::vec((0u8..4, 0.0f64..1.0, 0.0f64..1.0, 1u8..3), 1..24),
    ) {
        let kind = FxKind::EFFECTS[kind];
        let (doc, mut v) = rack(kind);
        let reg = registry();
        let mut document = Document::new(doc);
        let rect = v.device_rect(&key("fx")).unwrap();
        let area = Rect::new(CONTENT_X.0, rect.y0, CONTENT_X.1, rect.y1);
        for (what, fx, fy, clicks) in steps {
            let p = v.rack_to_view(Point::new(
                area.x0 + fx * area.width(),
                area.y0 + fy * area.height(),
            ));
            let mut actions = Vec::new();
            match what {
                0 => {
                    actions.extend(v.handle(&down_with(p, atomik_ui::PointerButton::Primary, clicks), document.doc(), &reg));
                    actions.extend(v.handle(&up(p), document.doc(), &reg));
                }
                1 => {
                    actions.extend(v.handle(&down(p), document.doc(), &reg));
                    let to = Point::new(p.x + (fx - 0.5) * 120.0, p.y + (fy - 0.5) * 120.0);
                    actions.extend(v.handle(&mv(to), document.doc(), &reg));
                    actions.extend(v.handle(&mv(Point::new(to.x * 1.5, to.y - 300.0)), document.doc(), &reg));
                    actions.extend(v.handle(&up(to), document.doc(), &reg));
                }
                2 => {
                    actions.extend(v.handle(&UiEvent::Wheel {
                        delta: atomik_ui::kurbo::Vec2::new(0.0, if fx > 0.5 { 1.0 } else { -1.0 }),
                        pos: p,
                        modifiers: atomik_ui::Modifiers::NONE,
                    }, document.doc(), &reg));
                }
                _ => {
                    actions.extend(v.handle(&down_with(p, atomik_ui::PointerButton::Secondary, 1), document.doc(), &reg));
                    actions.extend(v.handle(&up(p), document.doc(), &reg));
                }
            }
            for a in actions {
                match a {
                    UiAction::Command(c) => {
                        if let Command::SetParam { key: k, param, value, .. } = &c {
                            let d = reg.get(kind.type_id()).unwrap().descriptor;
                            let pd = d.params.iter().find(|x| x.key == param).expect("param exists");
                            prop_assert!(*value >= pd.min && *value <= pd.max, "{param} {value}");
                            prop_assert_eq!(k, &key("fx"));
                        }
                        let r = document.apply(c.clone(), &reg);
                        prop_assert!(r.is_ok(), "model rejected {:?}: {:?}", c, r);
                    }
                    UiAction::EndMerge => document.end_merge(),
                    _ => {}
                }
            }
            v.sync(document.doc(), &reg);
        }
    }
}
