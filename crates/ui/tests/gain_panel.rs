//! The gain tool's own panel: layout, the fader (scale per mode, one undo step per drag, reset,
//! wheel), the controls, RESET as a momentary button, the lamps, meters and readout, and the
//! BLEND knob that only shows while the mode blends. The display-list goldens of both faces are
//! in `fx_panels.rs` (`fx_gain-tool_*`).

mod support;

use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::gain_panel::{self as gp, GAIN_TOOL_TYPE};
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use support::fx::*;
use support::*;

fn rig_with(params: &[(&str, f32)]) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::GainTool);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (Document::new(doc), v)
}

fn rig() -> (Document, RackView) {
    // The defaults of the device (the golden document sets a few params to show them).
    rig_with(&[
        ("mode", 0.0),
        ("gain", 0.0),
        ("fader_mode", 0.0),
        ("fade", 0.0),
        ("invert_l", 0.0),
        ("pan_l", -1.0),
        ("pan_law", 1.0),
        ("width", 1.0),
    ])
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

/// A press, the moves (with `modifiers` held) and the release; everything is applied as it goes.
fn drag_with(
    document: &mut Document,
    v: &mut RackView,
    from: Point,
    path: &[Point],
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let reg = registry();
    let mut all = Vec::new();
    let a = v.handle(&down(from), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    for p in path {
        let a = v.handle(&mv_with(*p, modifiers), document.doc(), &reg);
        apply(document, v, &a);
        all.extend(a);
    }
    let end = path.last().copied().unwrap_or(from);
    let a = v.handle(&up(end), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    all
}

fn click(document: &mut Document, v: &mut RackView, at: Point, clicks: u8) -> Vec<UiAction> {
    let reg = registry();
    let a = v.handle(
        &down_with(at, PointerButton::Primary, clicks),
        document.doc(),
        &reg,
    );
    apply(document, v, &a);
    let b = v.handle(&up(at), document.doc(), &reg);
    apply(document, v, &b);
    a.into_iter().chain(b).collect()
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

fn painted(document: &Document, meters: Option<&[f32]>) -> DisplayList {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(
        &UiEvent::Resize(Size::new(820.0, 420.0)),
        document.doc(),
        &reg,
    );
    v.sync(document.doc(), &reg);
    if let Some(m) = meters {
        v.set_device_meter_values(&key("fx"), m);
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &reg);
    dl
}

fn texts(document: &Document, meters: Option<&[f32]>) -> Vec<String> {
    painted(document, meters)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The centre of the fader and the length of its track.
fn fader_track(v: &RackView) -> (Point, f64) {
    let c = v.control_center(&key("fx"), "gain").unwrap();
    let reg = registry();
    let spec = atomik_ui::fx_layout::fx_layout(
        FxKind::GainTool,
        reg.get(GAIN_TOOL_TYPE).unwrap().descriptor,
    );
    let (x0, x1, _) = spec.fx.unwrap().gain.unwrap().travel();
    (c, x1 - x0)
}

fn center(v: &RackView, p: &str) -> Point {
    v.control_center(&key("fx"), p)
        .unwrap_or_else(|| panic!("no control {p}"))
}

// ------------------------------------------------------------------ the maths

#[test]
fn the_meter_layout_is_pinned_to_the_device() {
    use atomik_devices::gain_tool as dev;
    assert_eq!(fx::GAIN_METERS, dev::METER_COUNT);
    assert_eq!(FxKind::GainTool.meter_count(), dev::METER_COUNT);
    assert_eq!(FxKind::GainTool.type_id(), dev::DESCRIPTOR.type_id);
    assert_eq!(gp::GAIN_TOOL_TYPE, dev::DESCRIPTOR.type_id);
    assert_eq!(gp::METER_IN_L, dev::METER_IN_PEAK_L);
    assert_eq!(gp::METER_IN_R, dev::METER_IN_PEAK_R);
    assert_eq!(gp::METER_OUT_L, dev::METER_OUT_PEAK_L);
    assert_eq!(gp::METER_OUT_R, dev::METER_OUT_PEAK_R);
    assert_eq!(gp::METER_HOLD, dev::METER_HOLD_DB);
    assert_eq!(gp::METER_SIGNAL, dev::METER_SIGNAL);
    assert_eq!(gp::METER_CV, dev::METER_CV_ACTIVE);
    assert_eq!(gp::METER_FADING, dev::METER_FADING);
    let gain = dev::DESCRIPTOR.param_by_key("gain").unwrap();
    assert_eq!((gain.min, gain.max), (gp::GAIN_MIN_DB, gp::GAIN_MAX_DB));
}

#[test]
fn the_scale_follows_the_fader_mode() {
    // Trim: the whole stored range, 0 dB at 60/84 of the track.
    assert_eq!(gp::fader_pos(-60.0, false), 0.0);
    assert_eq!(gp::fader_pos(24.0, false), 1.0);
    assert!((gp::fader_pos(0.0, false) - 60.0 / 84.0).abs() < 1e-6);
    // VCA: -inf at the left end, 0 dB at the right one, a stored boost shows at 0 dB.
    assert_eq!(gp::fader_pos(-60.0, true), 0.0);
    assert_eq!(gp::fader_pos(0.0, true), 1.0);
    assert_eq!(gp::fader_pos(12.0, true), 1.0);
    assert!((gp::fader_pos(-30.0, true) - 0.5).abs() < 1e-6);
    for vca in [false, true] {
        for db in [-60.0, -41.5, -12.0, -0.5] {
            let back = gp::gain_at(gp::fader_pos(db, vca), vca);
            assert!((back - db).abs() < 1e-4, "{vca} {db} {back}");
        }
    }
    let caps = |vca| -> Vec<String> { gp::scale_marks(vca).into_iter().map(|m| m.1).collect() };
    assert_eq!(
        caps(false),
        ["-60", "-48", "-36", "-24", "-12", "0", "+12", "+24"]
    );
    assert_eq!(caps(true), ["-inf", "-48", "-36", "-24", "-12", "0"]);
    assert_eq!(gp::fader_text(-60.0, true), "-inf dB");
    assert_eq!(gp::fader_text(-60.0, false), "-60.0 dB");
    assert_eq!(gp::fader_text(6.0, false), "+6.0 dB");
    assert_eq!(gp::fader_text(6.0, true), "0.0 dB");
    assert_eq!(gp::hold_text(-120.0), "--");
    assert_eq!(gp::hold_text(-3.24), "-3.2 dB");
    assert_eq!(gp::hold_text(1.0), "1.0 dB");
}

// ------------------------------------------------------------------ the fader

#[test]
fn a_drag_of_the_trim_fader_is_one_undo_step_and_moves_linearly_in_db() {
    let (mut document, mut v) = rig();
    let (c, w) = fader_track(&v);
    let depth = document.undo_depth();
    let a = drag_with(
        &mut document,
        &mut v,
        c,
        &[
            Point::new(c.x + 20.0, c.y),
            Point::new(c.x + 40.0, c.y + 30.0),
        ],
        Modifiers::NONE,
    );
    let sets = set_params(&a);
    assert!(sets.len() >= 2, "{sets:?}");
    assert!(sets.iter().all(|(p, _, _)| p == "gain"));
    let merge = sets[0].2;
    assert!(
        merge.is_some() && sets.iter().all(|s| s.2 == merge),
        "{sets:?}"
    );
    assert!(a.contains(&UiAction::EndMerge));
    let want = 40.0 / w * 84.0;
    assert!(
        (f64::from(param(&document, "gain")) - want).abs() < 0.05,
        "{} vs {want}",
        param(&document, "gain")
    );
    assert_eq!(document.undo_depth(), depth + 1, "one step for the drag");
    assert!(document.undo().is_some());
    assert_eq!(param(&document, "gain"), 0.0, "one undo undoes it all");
}

#[test]
fn the_whole_stored_range_is_reachable_in_trim() {
    let (mut document, mut v) = rig();
    let (c, _) = fader_track(&v);
    drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x + 900.0, c.y)],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "gain"), 24.0);
    let (c, _) = fader_track(&v);
    drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x - 1500.0, c.y)],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "gain"), -60.0);
}

#[test]
fn the_vca_fader_runs_from_minus_infinity_to_zero_db() {
    let (mut document, mut v) = rig_with(&[("fader_mode", 1.0), ("gain", -30.0)]);
    let (c, w) = fader_track(&v);
    // The same pixels move the value by a different amount: the track is 60 dB long here.
    drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x + w / 4.0, c.y)],
        Modifiers::NONE,
    );
    assert!(
        (param(&document, "gain") - (-15.0)).abs() < 0.2,
        "{}",
        param(&document, "gain")
    );
    // All the way right stops at 0 dB, never the +24 of the stored range.
    let (c, _) = fader_track(&v);
    drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x + 900.0, c.y)],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "gain"), 0.0);
    // All the way left is the bottom: -inf.
    let (c, _) = fader_track(&v);
    drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x - 1500.0, c.y)],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "gain"), -60.0);
    let t = texts(&document, None);
    assert!(t.iter().any(|t| t == "-inf dB"), "{t:?}");
    assert!(t.iter().any(|t| t == "-inf") && !t.iter().any(|t| t == "+24"));
}

#[test]
fn the_wheel_steps_the_gain_and_a_vca_fader_stops_at_zero() {
    let reg = registry();
    let wheel = |v: &mut RackView, document: &Document, at: Point, m: Modifiers| {
        v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, 1.0),
                pos: at,
                modifiers: m,
            },
            document.doc(),
            &reg,
        )
    };
    let (mut document, mut v) = rig();
    let (c, _) = fader_track(&v);
    let a = wheel(&mut v, &document, c, Modifiers::NONE);
    apply(&mut document, &mut v, &a);
    let coarse = param(&document, "gain");
    assert!((coarse - 84.0 * 0.02).abs() < 0.01, "{coarse}");
    let a = wheel(&mut v, &document, c, Modifiers::SHIFT);
    apply(&mut document, &mut v, &a);
    let fine = param(&document, "gain") - coarse;
    assert!((fine - 84.0 * 0.002).abs() < 0.01, "{fine}");

    let (mut document, mut v) = rig_with(&[("fader_mode", 1.0), ("gain", -0.5)]);
    let (c, _) = fader_track(&v);
    let a = wheel(&mut v, &document, c, Modifiers::NONE);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "gain"), 0.0);
}

#[test]
fn shift_makes_the_fader_ten_times_finer() {
    let travel = |m: Modifiers| {
        let (mut document, mut v) = rig();
        let (c, _) = fader_track(&v);
        drag_with(&mut document, &mut v, c, &[Point::new(c.x + 50.0, c.y)], m);
        param(&document, "gain")
    };
    let (coarse, fine) = (travel(Modifiers::NONE), travel(Modifiers::SHIFT));
    assert!(
        coarse > 5.0 && (coarse / fine - 10.0).abs() < 0.2,
        "{coarse} {fine}"
    );
}

#[test]
fn a_double_click_puts_the_fader_back_to_zero_db() {
    for mode in [0.0, 1.0] {
        let (mut document, mut v) = rig_with(&[("fader_mode", mode), ("gain", -18.0)]);
        let (c, _) = fader_track(&v);
        let a = click(&mut document, &mut v, c, 2);
        let sets = set_params(&a);
        assert_eq!(sets, vec![("gain".to_string(), 0.0, None)], "{sets:?}");
        assert_eq!(param(&document, "gain"), 0.0);
    }
}

#[test]
fn escape_puts_the_fader_back() {
    let (mut document, mut v) = rig_with(&[("gain", -10.0)]);
    let (c, _) = fader_track(&v);
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x + 60.0, c.y)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_ne!(param(&document, "gain"), -10.0);
    let a = v.handle(
        &UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        document.doc(),
        &reg,
    );
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "gain"), -10.0);
}

#[test]
fn the_tooltip_of_a_drag_reads_the_scale_of_the_mode() {
    let (mut document, mut v) = rig_with(&[("fader_mode", 1.0), ("gain", -30.0)]);
    let (c, _) = fader_track(&v);
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x - 900.0, c.y)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert!(
        v.tooltip().is_some_and(|t| t.contains("-inf dB")),
        "{:?}",
        v.tooltip()
    );
}

// ------------------------------------------------------------------ the other controls

#[test]
fn every_switch_and_selector_sets_its_param() {
    let (mut document, mut v) = rig();
    for (p, want) in [
        ("mute", 1.0),
        ("invert_l", 1.0),
        ("invert_r", 1.0),
        ("mono", 1.0),
        ("peak_auto", 1.0),
        ("fader_mode", 1.0),
        // The default law is -3 dB (1): a click steps to -4.5 dB.
        ("pan_law", 2.0),
        ("mode", 1.0),
    ] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {sets:?}");
        assert_eq!((sets[0].0.as_str(), sets[0].1), (p, want), "{p}");
    }
}

#[test]
fn every_knob_sets_its_param_with_a_merged_drag() {
    let (mut document, mut v) = rig_with(&[("mode", 1.0)]);
    for p in ["fade", "blend", "pan_l", "pan_r", "pan", "width"] {
        let at = center(&v, p);
        let a = drag_with(
            &mut document,
            &mut v,
            at,
            // Up and then well down: a knob at either end of its range moves.
            &[Point::new(at.x, at.y - 25.0), Point::new(at.x, at.y + 35.0)],
            Modifiers::NONE,
        );
        let sets = set_params(&a);
        assert!(
            !sets.is_empty() && sets.iter().all(|s| s.0 == p),
            "{p}: {sets:?}"
        );
        assert!(sets.iter().all(|s| s.2.is_some()), "{p} merges");
        assert!(a.contains(&UiAction::EndMerge), "{p}");
    }
}

#[test]
fn reset_is_momentary_and_stores_nothing() {
    let (mut document, mut v) = rig();
    let depth = document.undo_depth();
    let at = center(&v, "peak_reset");
    let a = click(&mut document, &mut v, at, 1);
    let pulse = UiAction::PulseParam {
        device: key("fx"),
        param: "peak_reset".into(),
    };
    assert_eq!(a.iter().filter(|x| **x == pulse).count(), 1, "{a:?}");
    assert!(commands(&a).is_empty(), "no SetParam: {a:?}");
    assert_eq!(param(&document, "peak_reset"), 0.0);
    assert_eq!(document.undo_depth(), depth, "no undo step for the reset");
}

#[test]
fn blend_shows_only_while_the_mode_blends() {
    let blend = |mode: f32| {
        let (document, mut v) = rig_with(&[("mode", mode)]);
        let label = texts(&document, None).iter().any(|x| x == "Blend");
        let reg = registry();
        let desc = reg.get(GAIN_TOOL_TYPE).unwrap().descriptor;
        // The cell is placed whether it shows or not: press where it would be.
        let spec = atomik_ui::fx_layout::fx_layout(FxKind::GainTool, desc);
        let cell = spec
            .front
            .iter()
            .find(|c| desc.params[c.param].key == "blend")
            .unwrap()
            .rect;
        let top = v.device_rect(&key("fx")).unwrap().y0;
        let at = v.rack_to_view(Point::new(cell.center().x, top + cell.y0 + 21.0));
        let a = v.handle(&down(at), document.doc(), &reg);
        let b = v.handle(&mv(Point::new(at.x, at.y - 30.0)), document.doc(), &reg);
        let hit = set_params(&[a, b].concat()).iter().any(|s| s.0 == "blend");
        (label, hit)
    };
    assert_eq!(blend(0.0), (false, false), "gain: no knob, no hit");
    assert_eq!(blend(1.0), (true, true), "mix");
    assert_eq!(blend(2.0), (true, true), "xfade");
}

/// Presses on the BLEND cell and drags up: whether that sets `blend`.
fn blend_hit(document: &Document, v: &mut RackView) -> bool {
    let reg = registry();
    let desc = reg.get(GAIN_TOOL_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::GainTool, desc);
    let cell = spec
        .front
        .iter()
        .find(|c| desc.params[c.param].key == "blend")
        .unwrap()
        .rect;
    let top = v.device_rect(&key("fx")).unwrap().y0;
    let at = v.rack_to_view(Point::new(cell.center().x, top + cell.y0 + 21.0));
    let a = v.handle(&down(at), document.doc(), &reg);
    let b = v.handle(&mv(Point::new(at.x, at.y - 30.0)), document.doc(), &reg);
    let c = v.handle(&up(at), document.doc(), &reg);
    set_params(&[a, b, c].concat())
        .iter()
        .any(|s| s.0 == "blend")
}

fn set_mode(document: &mut Document, v: &mut RackView, mode: f32) {
    let reg = registry();
    document
        .apply(
            Command::SetParam {
                key: key("fx"),
                param: "mode".into(),
                value: mode,
                merge: None,
            },
            &reg,
        )
        .unwrap();
    v.sync(document.doc(), &reg);
}

#[test]
fn the_blend_knob_follows_a_mode_change_on_the_same_view() {
    let (mut document, mut v) = rig_with(&[("mode", 0.0)]);
    assert!(!blend_hit(&document, &mut v), "gain: no hit");
    set_mode(&mut document, &mut v, 1.0);
    assert!(
        blend_hit(&document, &mut v),
        "gain -> mix: the knob is live"
    );
    set_mode(&mut document, &mut v, 0.0);
    assert!(
        !blend_hit(&document, &mut v),
        "mix -> gain: no invisible cell"
    );
}

#[test]
fn only_the_primary_click_pulses_reset_wheel_arrows_and_enter_are_safe() {
    let reg = registry();
    let (mut document, mut v) = rig();
    let depth = document.undo_depth();
    let at = center(&v, "peak_reset");
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: at,
            modifiers: Modifiers::NONE,
        },
        document.doc(),
        &reg,
    );
    assert!(a.is_empty(), "wheel: {a:?}");
    // Focus it with a click, then the keys.
    let _ = click(&mut document, &mut v, at, 1);
    let key_ev = |k: Key| UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    for k in [Key::Up, Key::Down, Key::PageUp, Key::PageDown] {
        let a = v.handle(&key_ev(k), document.doc(), &reg);
        assert!(commands(&a).is_empty(), "{a:?}");
    }
    for k in [Key::Enter, Key::Space] {
        let a = v.handle(&key_ev(k), document.doc(), &reg);
        assert!(commands(&a).is_empty(), "no SetParam: {a:?}");
        assert!(
            a.contains(&UiAction::PulseParam {
                device: key("fx"),
                param: "peak_reset".into()
            }),
            "{a:?}"
        );
    }
    assert_eq!(param(&document, "peak_reset"), 0.0);
    assert_eq!(document.undo_depth(), depth);
}

#[test]
fn stepping_up_from_a_vca_boost_leaves_it_and_down_starts_from_zero() {
    let reg = registry();
    let step = |dir: f32| {
        let (mut document, mut v) = rig_with(&[("fader_mode", 1.0), ("gain", 12.0)]);
        let (c, _) = fader_track(&v);
        let a = v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, f64::from(dir)),
                pos: c,
                modifiers: Modifiers::NONE,
            },
            document.doc(),
            &reg,
        );
        apply(&mut document, &mut v, &a);
        (set_params(&a).len(), param(&document, "gain"))
    };
    assert_eq!(step(1.0), (0, 12.0), "up leaves the boost");
    let (n, g) = step(-1.0);
    assert_eq!(n, 1);
    assert!(g < 0.0 && g > -3.0, "down starts from 0 dB: {g}");
}

// ------------------------------------------------------------------ meters, lamps, readout

fn with_meters(meters: &[f32], params: &[(&str, f32)]) -> Vec<String> {
    let (document, _) = rig_with(params);
    texts(&document, Some(meters))
}

fn shown(meters: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    fx::update_display(FxKind::GainTool, meters, &mut out);
    out
}

#[test]
fn the_readout_reads_the_held_peak_meter() {
    let none = with_meters(&[0.0, 0.0, 0.0, 0.0, -120.0, 0.0, 0.0, 0.0], &[]);
    assert!(none.iter().any(|t| t == "--"), "{none:?}");
    let some = with_meters(&[0.0, 0.0, 0.0, 0.0, -7.3, 0.0, 0.0, 0.0], &[]);
    assert!(some.iter().any(|t| t == "-7.3 dB"), "{some:?}");
    // The output peaks only move the bars.
    let loud = with_meters(&[0.9, 0.9, 0.9, 0.9, -120.0, 0.0, 0.0, 0.0], &[]);
    assert!(loud.iter().any(|t| t == "--"));
}

#[test]
fn the_published_values_become_what_the_panel_draws() {
    let s = shown(&[0.5, 0.25, 0.125, 0.0, -3.24, 1.0, 0.0, 1.0]);
    assert!((s[0] + 6.0).abs() < 0.6 && (s[1] + 12.0).abs() < 0.6 && (s[2] + 18.0).abs() < 0.6);
    assert_eq!(s[3], fx::LEVEL_FLOOR_DB);
    assert!(
        (s[4] + 3.2).abs() < 0.06,
        "held peak as published: {}",
        s[4]
    );
    assert_eq!((s[5], s[6], s[7]), (1.0, 0.0, 1.0));
}

#[test]
fn each_meter_value_changes_its_own_part_of_the_panel() {
    let json = |m: &[f32]| {
        let (document, _) = rig();
        painted(&document, Some(m)).to_json()
    };
    let base = [0.0, 0.0, 0.0, 0.0, -120.0, 0.0, 0.0, 0.0];
    let still = json(&base);
    for i in 0..8 {
        let mut m = base;
        m[i] = match i {
            4 => -12.0,
            5..=7 => 1.0,
            _ => 0.5,
        };
        assert_ne!(json(&m), still, "meter {i} draws nothing");
    }
    // The signal, CV and fading lamps are independent of each other.
    let lit = |i: usize| {
        let mut m = base;
        m[i] = 1.0;
        json(&m)
    };
    assert_ne!(lit(5), lit(6));
    assert_ne!(lit(6), lit(7));
    assert_ne!(lit(5), lit(7));
}

// ------------------------------------------------------------------ the back

#[test]
fn the_back_has_every_jack_hit_testable_and_the_two_trims() {
    let (mut document, mut v) = rig();
    v.set_flip(Flip::Back);
    let reg = registry();
    let desc = reg.get(GAIN_TOOL_TYPE).unwrap().descriptor;
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
    for p in ["gain_cv_trim", "pan_cv_trim"] {
        let at = center(&v, p);
        let a = drag_with(
            &mut document,
            &mut v,
            at,
            &[Point::new(at.x, at.y - 30.0), Point::new(at.x, at.y + 40.0)],
            Modifiers::NONE,
        );
        assert!(set_params(&a).iter().any(|s| s.0 == p), "{p}");
    }
    // The front controls are not on the back and the trims are not on the front.
    assert!(v.control_center(&key("fx"), "gain").is_none());
    v.set_flip(Flip::Front);
    assert!(v.control_center(&key("fx"), "gain_cv_trim").is_none());
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
    let from = v
        .socket_center(&sref("src", "out_l"))
        .expect("the synth's out jack");
    for to_socket in ["in_l", "aux_r"] {
        let to = v.socket_center(&sref("fx", to_socket)).unwrap();
        let a = v.handle(&down(from), document.doc(), &reg);
        let b = v.handle(&mv(to), document.doc(), &reg);
        let c = v.handle(&up(to), document.doc(), &reg);
        let cmds = commands(&[a, b, c].concat());
        assert!(
            // A cable onto a left jack brings the right one along (one batch).
            cmds.iter().any(|c| match c {
                Command::Connect(_) => true,
                Command::Batch(b) => b.iter().any(|c| matches!(c, Command::Connect(_))),
                _ => false,
            }),
            "{to_socket}: {cmds:?}"
        );
    }
}

// ------------------------------------------------------------------ French

/// Both faces painted in French: every fixed word has an entry and the front matches its golden.
#[test]
fn the_panel_in_french() {
    use atomik_ui::i18n::{self, Lang};
    let doc = fx_doc(FxKind::GainTool);
    let reg = registry();
    for (name, flip) in [("front", Flip::Front), ("back", Flip::Back)] {
        let (list, misses) = i18n::with_misses(|| {
            i18n::with_language(Lang::French, || {
                let mut v = RackView::new(Theme::dark());
                let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
                v.sync(&doc, &reg);
                v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::GainTool));
                v.set_flip(flip);
                let mut dl = DisplayList::new();
                v.paint(&mut dl, &doc, &reg);
                dl
            })
        });
        // Names the fixture gives and values (dB readouts, scale marks) are not words.
        let missing: Vec<&String> = misses
            .iter()
            .filter(|m| {
                !matches!(m.as_str(), "FX" | "Gain Tool" | "GAIN TOOL" | "Audio I/O")
                    && m.chars().filter(|c| c.is_alphabetic()).count() > 2
                    && !m.chars().any(|c| c.is_ascii_digit())
                    && !m.ends_with("-inf")
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{name}: no French entry for {missing:?}"
        );
        assert!(!list.texts().is_empty());
        let french = list.texts();
        if flip == Flip::Front {
            for word in ["SIGNAL", "FONDU", "ENTRÉE", "SORTIE", "POLARITÉ"] {
                assert!(
                    french.iter().any(|t| t.to_uppercase().contains(word)),
                    "{word} in {french:?}"
                );
            }
        }
    }
}

// ------------------------------------------------------------------ previews

/// Renders PNG previews of the panel with vello_cpu. Run:
/// `cargo test -p atomik-ui --test gain_panel -- --ignored`. Writes `target/gain-preview/*.png`
/// (not committed).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    const SCALE: f64 = 1.5;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/gain-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let reg = registry();
    let mut text = TextEngine::new();
    type State<'a> = (&'a str, Flip, Vec<(&'a str, f32)>);
    let states: Vec<State> = vec![
        ("front", Flip::Front, vec![]),
        (
            "front_vca",
            Flip::Front,
            vec![("fader_mode", 1.0), ("gain", -60.0)],
        ),
        ("back", Flip::Back, vec![]),
    ];
    for (name, flip, params) in states {
        let mut doc = fx_doc(FxKind::GainTool);
        for (k, v) in params {
            doc.devices[0].params.insert(k.to_string(), v);
        }
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::GainTool));
        v.set_flip(flip);
        let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        std::fs::write(
            dir.join(format!("{name}.png")),
            canvas.finish().into_png().expect("png"),
        )
        .unwrap();
    }
}
