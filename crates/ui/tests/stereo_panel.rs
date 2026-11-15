//! The stereo tool's own panel: layout, the three big knobs (one undo step per drag, Shift, Esc,
//! wheel, double-click), the SOLO radio buttons, the other controls, the lamps, correlation
//! bars and readouts, and the back with its jacks and trims. The display-list goldens of both
//! faces are in `fx_panels.rs` (`fx_stereo-tool_*`).

mod support;

use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::stereo_panel::{self as sp, STEREO_TOOL_TYPE};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use support::fx::*;
use support::*;

fn rig_with(params: &[(&str, f32)]) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::StereoTool);
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
        ("lo_width", 1.0),
        ("width", 1.0),
        ("low_bypass", 20.0),
        ("solo", 0.0),
        ("invert_l", 0.0),
        ("rotate", 0.0),
        ("rotate_hz", 100.0),
        ("balance", 0.0),
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

fn click_with(
    document: &mut Document,
    v: &mut RackView,
    at: Point,
    clicks: u8,
    button: PointerButton,
) -> Vec<UiAction> {
    let reg = registry();
    let a = v.handle(&down_with(at, button, clicks), document.doc(), &reg);
    apply(document, v, &a);
    let b = v.handle(&up(at), document.doc(), &reg);
    apply(document, v, &b);
    a.into_iter().chain(b).collect()
}

fn click(document: &mut Document, v: &mut RackView, at: Point, clicks: u8) -> Vec<UiAction> {
    click_with(document, v, at, clicks, PointerButton::Primary)
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

fn center(v: &RackView, p: &str) -> Point {
    v.control_center(&key("fx"), p)
        .unwrap_or_else(|| panic!("no control {p}"))
}

fn geom() -> sp::StereoGeom {
    let reg = registry();
    atomik_ui::fx_layout::fx_layout(
        FxKind::StereoTool,
        reg.get(STEREO_TOOL_TYPE).unwrap().descriptor,
    )
    .fx
    .unwrap()
    .stereo
    .unwrap()
}

/// The view position of the centre of SOLO button `n` (0 = HI, 1 = LO, 2 = NORMAL).
fn solo_button(v: &RackView, n: usize) -> Point {
    let r = geom().solo[n];
    let top = v.device_rect(&key("fx")).unwrap().y0;
    v.rack_to_view(Point::new(r.center().x, top + r.center().y))
}

// ------------------------------------------------------------------ the pins

#[test]
fn the_meter_layout_is_pinned_to_the_device() {
    use atomik_devices::stereo_tool as dev;
    assert_eq!(fx::STEREO_METERS, dev::METER_COUNT);
    assert_eq!(FxKind::StereoTool.meter_count(), dev::METER_COUNT);
    assert_eq!(FxKind::StereoTool.type_id(), dev::DESCRIPTOR.type_id);
    assert_eq!(sp::STEREO_TOOL_TYPE, dev::DESCRIPTOR.type_id);
    assert_eq!(FxKind::of("atomik.stereo-tool"), Some(FxKind::StereoTool));
    assert!(FxKind::EFFECTS.contains(&FxKind::StereoTool));
    assert_eq!(sp::METER_LO_CORR, dev::METER_LO_CORR);
    assert_eq!(sp::METER_HI_CORR, dev::METER_HI_CORR);
    assert_eq!(sp::METER_LO_ACTIVE, dev::METER_LO_ACTIVE);
    assert_eq!(sp::METER_HI_ACTIVE, dev::METER_HI_ACTIVE);
    assert_eq!(sp::METER_SPLIT, dev::METER_SPLIT);
    // The solo choice and the crossover's off point.
    let solo = dev::DESCRIPTOR.param_by_key("solo").unwrap();
    assert_eq!((solo.min, solo.max), (0.0, 2.0));
    let x = dev::DESCRIPTOR.param_by_key("low_bypass").unwrap();
    assert_eq!((x.min, x.default), (20.0, 20.0));
    assert!(sp::XOVER_OFF_HZ > x.min && sp::XOVER_OFF_HZ < 21.0);
}

#[test]
fn the_texts_of_the_panel() {
    assert_eq!(sp::width_text(1.0), "1.00");
    assert_eq!(sp::width_text(0.555), "0.56");
    assert_eq!(sp::xover_text(20.0), "off");
    assert_eq!(sp::xover_text(20.5), "off");
    assert_eq!(sp::xover_text(21.0), "21 Hz");
    assert_eq!(sp::xover_text(220.0), "220 Hz");
    assert_eq!(sp::xover_text(1200.0), "1.20 kHz");
    assert_eq!(sp::xover_text(6000.0), "6.00 kHz");
    assert_eq!(sp::corr_pos(-1.0), 0.0);
    assert_eq!(sp::corr_pos(0.0), 0.5);
    assert_eq!(sp::corr_pos(1.0), 1.0);
    assert_eq!(sp::corr_pos(7.0), 1.0);
    assert_eq!(sp::solo_caption(2.0), "SOLO HI BAND");
    assert_eq!(sp::solo_caption(1.0), "SOLO LO BAND");
    assert_eq!(sp::solo_caption(0.0), "NORMAL");
}

#[test]
fn the_scales_and_captions_are_painted() {
    let (document, _) = rig();
    let t = texts(&document, None);
    for want in [
        "LO BAND",
        "HI BAND",
        "X-OVER",
        "SOLO",
        "MONO",
        "ORIGINAL",
        "WIDE",
        "OFF",
        "ACTIVE",
        "SPLIT",
        "CORRELATION",
        "SOLO HI BAND",
        "SOLO LO BAND",
        "NORMAL",
        "ROTATE",
        "BALANCE",
        "M/S MODE",
        "Ø L",
        "Ø R",
        "Rotate",
        "Rotate at",
        "Balance",
        "off",
        "1.00",
    ] {
        assert!(t.iter().any(|x| x == want), "no {want:?} in {t:?}");
    }
    // The two bands read their own width.
    let (document, _) = rig_with(&[("lo_width", 0.5), ("width", 1.75), ("low_bypass", 1200.0)]);
    let t = texts(&document, None);
    for want in ["0.50", "1.75", "1.20 kHz"] {
        assert!(t.iter().any(|x| x == want), "no {want:?} in {t:?}");
    }
}

#[test]
fn the_layout_is_pinned_and_nothing_overlaps() {
    let reg = registry();
    let desc = reg.get(STEREO_TOOL_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::StereoTool, desc);
    assert_eq!(spec.height_units, 3);
    let g = spec.fx.unwrap().stereo.unwrap();
    // Every param but the bypass and the three trims has a front cell; the trims are on the back.
    for k in [
        "mode",
        "invert_l",
        "invert_r",
        "width",
        "low_bypass",
        "balance",
        "mono",
        "rotate",
        "rotate_hz",
        "lo_width",
        "solo",
    ] {
        let i = desc.params.iter().position(|p| p.key == k).unwrap();
        let cell = spec.front.iter().find(|c| c.param == i);
        assert!(cell.is_some_and(|c| !spec.on_back(c)), "{k}");
    }
    for k in ["lo_width_cv_trim", "hi_width_cv_trim", "xover_cv_trim"] {
        let i = desc.params.iter().position(|p| p.key == k).unwrap();
        assert!(g.back_params.contains(&i), "{k}");
        assert!(spec.on_back(spec.front.iter().find(|c| c.param == i).unwrap()));
    }
    let height = spec.height();
    let width = f64::from(atomik_device_sdk::RACK_WIDTH);
    let front: Vec<Rect> = spec
        .front
        .iter()
        .filter(|c| !spec.on_back(c))
        .map(|c| c.rect)
        .collect();
    for (n, a) in front.iter().enumerate() {
        assert!(
            a.x0 >= 0.0 && a.x1 <= width - 16.0 && a.y0 >= 0.0 && a.y1 <= height,
            "{a:?}"
        );
        for b in &front[n + 1..] {
            assert!(a.intersect(*b).area() <= 0.0, "{a:?} overlaps {b:?}");
        }
    }
    let back: Vec<Rect> = spec
        .back
        .iter()
        .map(|s| s.rect)
        .chain(
            spec.front
                .iter()
                .filter(|c| spec.on_back(c))
                .map(|c| c.rect),
        )
        .collect();
    for (n, a) in back.iter().enumerate() {
        assert!(a.x1 <= width - 16.0 && a.y1 <= height, "{a:?}");
        for b in &back[n + 1..] {
            assert!(a.intersect(*b).area() <= 0.0, "{a:?} overlaps {b:?}");
        }
    }
    // The three solo buttons do not touch.
    for w in g.solo.windows(2) {
        assert!(w[0].y1 < w[1].y0);
    }
    assert_eq!(g.solo_at(g.solo[0].center()), 2.0);
    assert_eq!(g.solo_at(g.solo[1].center()), 1.0);
    assert_eq!(g.solo_at(g.solo[2].center()), 0.0);
    // Between two buttons the nearer one wins.
    let gap = Point::new(g.solo[0].center().x, g.solo[0].y1 + 1.0);
    assert_eq!(g.solo_at(gap), 2.0);
}

// ------------------------------------------------------------------ the big knobs

#[test]
fn every_knob_sets_its_param_with_a_merged_drag() {
    let (mut document, mut v) = rig();
    for p in [
        "lo_width",
        "width",
        "low_bypass",
        "rotate",
        "rotate_hz",
        "balance",
    ] {
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
fn a_drag_of_a_big_knob_is_one_undo_step() {
    for (p, rest) in [("lo_width", 1.0), ("width", 1.0), ("low_bypass", 20.0)] {
        let (mut document, mut v) = rig();
        let c = center(&v, p);
        let depth = document.undo_depth();
        let a = drag_with(
            &mut document,
            &mut v,
            c,
            &[
                Point::new(c.x, c.y - 10.0),
                Point::new(c.x, c.y - 30.0),
                Point::new(c.x + 4.0, c.y - 45.0),
            ],
            Modifiers::NONE,
        );
        let sets = set_params(&a);
        assert!(sets.len() >= 2, "{p}: {sets:?}");
        let merge = sets[0].2;
        assert!(merge.is_some() && sets.iter().all(|s| s.2 == merge), "{p}");
        assert!(a.contains(&UiAction::EndMerge));
        assert_ne!(param(&document, p), rest, "{p} moved");
        assert_eq!(document.undo_depth(), depth + 1, "{p}: one step");
        assert!(document.undo().is_some());
        assert_eq!(param(&document, p), rest, "{p}: one undo undoes it all");
    }
}

#[test]
fn shift_makes_a_big_knob_ten_times_finer() {
    let travel = |m: Modifiers| {
        let (mut document, mut v) = rig();
        let c = center(&v, "lo_width");
        // Down from the default 1.0 so both drags stay inside the range.
        drag_with(&mut document, &mut v, c, &[Point::new(c.x, c.y + 10.0)], m);
        1.0 - param(&document, "lo_width")
    };
    let (coarse, fine) = (travel(Modifiers::NONE), travel(Modifiers::SHIFT));
    assert!(
        coarse > 0.05 && (coarse / fine - 10.0).abs() < 0.3,
        "{coarse} {fine}"
    );
}

#[test]
fn escape_puts_a_big_knob_back() {
    let (mut document, mut v) = rig_with(&[("width", 1.5)]);
    let c = center(&v, "width");
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x, c.y - 30.0)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_ne!(param(&document, "width"), 1.5);
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
    assert_eq!(param(&document, "width"), 1.5);
}

#[test]
fn a_double_click_resets_each_big_knob_to_its_default() {
    let (mut document, mut v) =
        rig_with(&[("lo_width", 0.2), ("width", 1.8), ("low_bypass", 900.0)]);
    for (p, want) in [("lo_width", 1.0), ("width", 1.0), ("low_bypass", 20.0)] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 2);
        let sets = set_params(&a);
        assert_eq!(sets, vec![(p.to_string(), want, None)], "{p}: {sets:?}");
        assert_eq!(param(&document, p), want);
    }
}

#[test]
fn the_wheel_steps_the_knobs_and_the_solo_choice() {
    let reg = registry();
    let wheel = |v: &mut RackView, document: &Document, at: Point, dy: f64, m: Modifiers| {
        v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, dy),
                pos: at,
                modifiers: m,
            },
            document.doc(),
            &reg,
        )
    };
    let (mut document, mut v) = rig();
    for p in ["lo_width", "width", "low_bypass", "rotate", "balance"] {
        let at = center(&v, p);
        let before = param(&document, p);
        let a = wheel(&mut v, &document, at, 1.0, Modifiers::NONE);
        apply(&mut document, &mut v, &a);
        assert_ne!(param(&document, p), before, "{p}");
    }
    // Shift steps ten times finer.
    let at = center(&v, "balance");
    let b0 = param(&document, "balance");
    let a = wheel(&mut v, &document, at, 1.0, Modifiers::NONE);
    apply(&mut document, &mut v, &a);
    let b1 = param(&document, "balance");
    let a = wheel(&mut v, &document, at, 1.0, Modifiers::SHIFT);
    apply(&mut document, &mut v, &a);
    let (coarse, fine) = (b1 - b0, param(&document, "balance") - b1);
    assert!((coarse / fine - 10.0).abs() < 0.5, "{coarse} {fine}");
    // The wheel over the solo group steps through the three values.
    let at = solo_button(&v, 2);
    let a = wheel(&mut v, &document, at, 1.0, Modifiers::NONE);
    apply(&mut document, &mut v, &a);
    assert_ne!(param(&document, "solo"), 0.0);
}

// ------------------------------------------------------------------ solo

#[test]
fn the_solo_radio_buttons_pick_their_value() {
    let (mut document, mut v) = rig();
    // Top to bottom: SOLO HI BAND (2), SOLO LO BAND (1), NORMAL (0).
    let mut current = 0.0;
    for (n, want) in [(0, 2.0), (1, 1.0), (2, 0.0), (0, 2.0), (2, 0.0), (1, 1.0)] {
        let at = solo_button(&v, n);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "button {n}: {sets:?}");
        assert_eq!(
            (sets[0].0.as_str(), sets[0].1),
            ("solo", want),
            "button {n}"
        );
        assert_eq!(param(&document, "solo"), want);
        assert_ne!(want, current);
        current = want;
    }
}

#[test]
fn a_press_on_the_selected_solo_button_changes_nothing() {
    let (mut document, mut v) = rig_with(&[("solo", 1.0)]);
    let depth = document.undo_depth();
    let at = solo_button(&v, 1);
    let a = click(&mut document, &mut v, at, 1);
    assert!(set_params(&a).is_empty(), "{a:?}");
    assert_eq!(document.undo_depth(), depth);
}

#[test]
fn the_selected_solo_button_is_the_one_lit() {
    let json = |solo: f32| {
        let (document, _) = rig_with(&[("solo", solo)]);
        painted(&document, None).to_json()
    };
    let (n, lo, hi) = (json(0.0), json(1.0), json(2.0));
    assert!(n != lo && lo != hi && n != hi);
}

#[test]
fn a_right_click_on_solo_opens_the_midi_menu_and_changes_nothing() {
    let (mut document, mut v) = rig();
    let at = solo_button(&v, 0);
    let a = click_with(&mut document, &mut v, at, 1, PointerButton::Secondary);
    assert!(
        a.iter()
            .any(|x| matches!(x, UiAction::ControlMenu { param, .. } if param == "solo")),
        "{a:?}"
    );
    assert!(set_params(&a).is_empty());
    assert_eq!(param(&document, "solo"), 0.0);
}

#[test]
fn a_right_click_on_a_big_knob_opens_its_midi_menu() {
    let (mut document, mut v) = rig();
    for p in ["lo_width", "width", "low_bypass"] {
        let at = center(&v, p);
        let a = click_with(&mut document, &mut v, at, 1, PointerButton::Secondary);
        assert!(
            a.iter()
                .any(|x| matches!(x, UiAction::ControlMenu { param, .. } if param == p)),
            "{p}: {a:?}"
        );
    }
}

// ------------------------------------------------------------------ the other controls

#[test]
fn every_switch_and_selector_sets_its_param() {
    let (mut document, mut v) = rig();
    for (p, want) in [
        ("invert_l", 1.0),
        ("invert_r", 1.0),
        ("mono", 1.0),
        ("mode", 1.0),
    ] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {sets:?}");
        assert_eq!((sets[0].0.as_str(), sets[0].1), (p, want), "{p}");
    }
}

// ------------------------------------------------------------------ meters, lamps, bars

fn with_meters(meters: &[f32]) -> String {
    let (document, _) = rig();
    painted(&document, Some(meters)).to_json()
}

fn shown(meters: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    fx::update_display(FxKind::StereoTool, meters, &mut out);
    out
}

#[test]
fn the_published_values_become_what_the_panel_draws() {
    let s = shown(&[-0.347, 0.5, 1.0, 0.0, 1.0]);
    assert_eq!(s, vec![-0.35, 0.5, 1.0, 0.0, 1.0]);
    // Correlation keeps its sign and is held within -1..1.
    let s = shown(&[-3.0, 9.0, 0.0, 0.0, 0.0]);
    assert_eq!((s[0], s[1]), (-1.0, 1.0));
}

#[test]
fn each_meter_value_changes_its_own_part_of_the_panel() {
    let base = [0.0, 0.0, 0.0, 0.0, 0.0];
    let still = with_meters(&base);
    let drawn = |i: usize, v: f32| {
        let mut m = base;
        m[i] = v;
        with_meters(&m)
    };
    let all = [
        drawn(0, 0.6),
        drawn(1, 0.6),
        drawn(2, 1.0),
        drawn(3, 1.0),
        drawn(4, 1.0),
    ];
    for (i, a) in all.iter().enumerate() {
        assert_ne!(*a, still, "meter {i} draws nothing");
        for (j, b) in all.iter().enumerate().skip(i + 1) {
            assert_ne!(a, b, "meters {i} and {j} draw the same");
        }
    }
    // A correlation below zero is drawn the other way round.
    assert_ne!(drawn(0, -0.6), drawn(0, 0.6));
    assert_ne!(drawn(0, -0.6), still);
}

#[test]
fn the_lamps_and_bars_sit_in_their_own_band() {
    let g = geom();
    // The lamps are in the order low band, split, high band from left to right, and so are
    // the correlation bars.
    let mid = g.xover_knob.center().x;
    assert!(g.lamps[0].x1 < g.lamps[1].x0 && g.lamps[1].x1 < g.lamps[2].x0);
    assert!(g.lamps[0].x0 < mid && mid < g.lamps[2].x0);
    assert!(g.corr[0].x1 < mid && mid < g.corr[1].x0);
    assert!(g.lo_knob.x1 <= g.xover_knob.x0 && g.xover_knob.x1 <= g.hi_knob.x0);
}

// ------------------------------------------------------------------ the back

#[test]
fn the_back_has_every_jack_hit_testable_and_the_three_trims() {
    let (mut document, mut v) = rig();
    v.set_flip(Flip::Back);
    let reg = registry();
    let desc = reg.get(STEREO_TOOL_TYPE).unwrap().descriptor;
    assert_eq!(desc.sockets.len(), 11);
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
    for p in ["lo_width_cv_trim", "hi_width_cv_trim", "xover_cv_trim"] {
        let at = center(&v, p);
        let a = drag_with(
            &mut document,
            &mut v,
            at,
            &[Point::new(at.x, at.y - 30.0), Point::new(at.x, at.y + 40.0)],
            Modifiers::NONE,
        );
        let sets = set_params(&a);
        assert!(sets.iter().any(|s| s.0 == p), "{p}");
        assert!(sets.iter().all(|s| s.2.is_some()), "{p} merges");
        assert!(a.contains(&UiAction::EndMerge));
    }
    // The front controls are not on the back and the trims are not on the front.
    assert!(v.control_center(&key("fx"), "lo_width").is_none());
    assert!(v.control_center(&key("fx"), "solo").is_none());
    v.set_flip(Flip::Front);
    for p in ["lo_width_cv_trim", "hi_width_cv_trim", "xover_cv_trim"] {
        assert!(v.control_center(&key("fx"), p).is_none(), "{p}");
    }
}

#[test]
fn the_back_is_grouped_and_labelled() {
    let doc = fx_doc(FxKind::StereoTool);
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    v.sync(&doc, &reg);
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let t: Vec<&str> = dl.texts();
    for want in [
        "AUDIO IN",
        "AUDIO OUT",
        "CV IN",
        "BAND OUTPUTS",
        "In L",
        "In R",
        "Out L",
        "Out R",
        "Lo width",
        "Hi width",
        "X-over",
        "Lo L",
        "Lo R",
        "Hi L",
        "Hi R",
        "CV trim",
    ] {
        assert!(t.contains(&want), "no {want:?} in {t:?}");
    }
    assert_eq!(t.iter().filter(|x| **x == "CV trim").count(), 3);
}

/// Whether the actions connect something (a cable onto a left jack brings the right one along
/// as one batch).
fn connects(cmds: &[Command]) -> bool {
    cmds.iter().any(|c| match c {
        Command::Connect(_) => true,
        Command::Batch(b) => b.iter().any(|c| matches!(c, Command::Connect(_))),
        _ => false,
    })
}

#[test]
fn a_cable_from_another_device_reaches_every_audio_and_cv_jack() {
    let (document, _) = rig();
    let mut doc = document.doc().clone();
    doc.devices.push(device("src", "atomik.poly-synth", "Src"));
    doc.devices.push(device("mod", "atomik.mod-source", "Mod"));
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
    // Audio from the synth to the audio jacks, a control signal from the Mod Source to the CV ones.
    for (src, out, to_socket) in [
        ("src", "out_l", "in_l"),
        ("src", "out_l", "in_r"),
        ("mod", "lfo1_cv_out", "lo_width_cv"),
        ("mod", "lfo1_cv_out", "hi_width_cv"),
        ("mod", "env_cv_out", "xover_cv"),
    ] {
        let from = v
            .socket_center(&sref(src, out))
            .unwrap_or_else(|| panic!("{src} {out}"));
        let to = v.socket_center(&sref("fx", to_socket)).unwrap();
        let a = v.handle(&down(from), document.doc(), &reg);
        let b = v.handle(&mv(to), document.doc(), &reg);
        let c = v.handle(&up(to), document.doc(), &reg);
        let cmds = commands(&[a, b, c].concat());
        assert!(connects(&cmds), "{to_socket}: {cmds:?}");
    }
}

#[test]
fn a_cable_from_a_band_output_to_another_device_connects() {
    let (document, _) = rig();
    let mut doc = document.doc().clone();
    doc.devices.push(device("dst", "atomik.stereo-tool", "Dst"));
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
    for from_socket in ["lo_out_l", "lo_out_r", "hi_out_l", "hi_out_r"] {
        let from = v.socket_center(&sref("fx", from_socket)).unwrap();
        let to = v.socket_center(&sref("dst", "in_l")).unwrap();
        let a = v.handle(&down(from), document.doc(), &reg);
        let b = v.handle(&mv(to), document.doc(), &reg);
        let c = v.handle(&up(to), document.doc(), &reg);
        let cmds = commands(&[a, b, c].concat());
        assert!(connects(&cmds), "{from_socket}: {cmds:?}");
    }
}

// ------------------------------------------------------------------ French

/// Both faces painted in French: every fixed word has an entry and the key words are French.
#[test]
fn the_panel_in_french() {
    use atomik_ui::i18n::{self, Lang};
    let doc = fx_doc(FxKind::StereoTool);
    let reg = registry();
    for (name, flip) in [("front", Flip::Front), ("back", Flip::Back)] {
        let (list, misses) = i18n::with_misses(|| {
            i18n::with_language(Lang::French, || {
                let mut v = RackView::new(Theme::dark());
                let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
                v.sync(&doc, &reg);
                v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::StereoTool));
                v.set_flip(flip);
                let mut dl = DisplayList::new();
                v.paint(&mut dl, &doc, &reg);
                dl
            })
        });
        // Names the fixture gives and values (readouts, scale marks) are not words.
        let missing: Vec<&String> = misses
            .iter()
            .filter(|m| {
                !matches!(
                    m.as_str(),
                    "FX" | "Stereo Tool" | "STEREO TOOL" | "Audio I/O"
                ) && m.chars().filter(|c| c.is_alphabetic()).count() > 2
                    && !m.chars().any(|c| c.is_ascii_digit())
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{name}: no French entry for {missing:?}"
        );
        let french = list.texts();
        assert!(!french.is_empty());
        let words: &[&str] = if flip == Flip::Front {
            &[
                "BANDE GRAVE",
                "BANDE AIGUË",
                "CROISEMENT",
                "POLARITÉ",
                "LARGE",
            ]
        } else {
            &["SORTIES DE BANDE", "ENTRÉE CV", "GRAVE G", "CROISEMENT"]
        };
        for word in words {
            assert!(
                french.iter().any(|t| t.to_uppercase().contains(word)),
                "{word} in {french:?}"
            );
        }
    }
}

// ------------------------------------------------------------------ previews

/// Renders PNG previews of the panel with vello_cpu. Run:
/// `cargo test -p atomik-ui --test stereo_panel -- --ignored`. Writes
/// `target/stereo-preview/*.png` (not committed).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    const SCALE: f64 = 1.5;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/stereo-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let reg = registry();
    let mut text = TextEngine::new();
    type State<'a> = (&'a str, Flip, Vec<(&'a str, f32)>);
    let states: Vec<State> = vec![
        ("front", Flip::Front, vec![]),
        (
            "front_off",
            Flip::Front,
            vec![("low_bypass", 20.0), ("solo", 2.0), ("lo_width", 1.0)],
        ),
        ("back", Flip::Back, vec![]),
    ];
    for (name, flip, params) in states {
        let mut doc = fx_doc(FxKind::StereoTool);
        for (k, v) in params {
            doc.devices[0].params.insert(k.to_string(), v);
        }
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::StereoTool));
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

#[test]
fn the_solo_radio_stops_at_its_ends_for_the_wheel_and_the_arrows() {
    let reg = registry();
    let wheel = |v: &mut RackView, document: &Document, at: Point, dy: f64| {
        v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, dy),
                pos: at,
                modifiers: Modifiers::NONE,
            },
            document.doc(),
            &reg,
        )
    };
    // At HI, one more notch up does nothing (it used to jump to NORMAL).
    let (document, mut v) = rig_with(&[("solo", 2.0)]);
    let at = solo_button(&v, 0);
    for dy in [1.0, 5.0] {
        let a = wheel(&mut v, &document, at, dy);
        assert!(set_params(&a).is_empty(), "{a:?}");
    }
    // And at NORMAL, one notch down.
    let (document, mut v) = rig_with(&[("solo", 0.0)]);
    let at = solo_button(&v, 2);
    let a = wheel(&mut v, &document, at, -1.0);
    assert!(set_params(&a).is_empty(), "{a:?}");
    // In the middle it still steps.
    let (mut document, mut v) = rig_with(&[("solo", 1.0)]);
    let at = solo_button(&v, 1);
    let a = wheel(&mut v, &document, at, 1.0);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "solo"), 2.0);
}

#[test]
fn the_solo_help_does_not_promise_a_step() {
    let (_, v) = rig();
    let at = solo_button(&v, 1);
    let h = v.help_at(at);
    let text = h.map(|h| h.text.to_string()).unwrap_or_default();
    assert!(text.contains("Solo"), "{text}");
    assert!(!text.contains("step"), "{text}");
}
