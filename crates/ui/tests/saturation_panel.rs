//! The saturation effect's own 1 U panel: layout, the big knob (merged drag, Shift, Esc, wheel,
//! double-click, MIDI menu), the vertical type switch (click on a label or on a position, drag,
//! wheel, arrows), the small group, the drive lamp, the back jacks with the CV trim and the
//! French texts. The display-list goldens of both faces are in `fx_panels.rs`
//! (`fx_saturation_*`). Previews: `cargo test -p atomik-ui --test saturation_panel -- --ignored`.

mod support;

use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::saturation_panel::{self as sp, SATURATION_TYPE};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use support::fx::*;
use support::*;

fn rig_with(params: &[(&str, f32)]) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::Saturation);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (Document::new(doc), v)
}

/// The device's defaults (the golden document sets a few params to show them).
fn rig() -> (Document, RackView) {
    rig_with(&[
        ("saturation", 0.0),
        ("type", 1.0),
        ("mix", 1.0),
        ("output", 0.0),
        ("oversampling", 1.0),
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

fn click(document: &mut Document, v: &mut RackView, at: Point, clicks: u8) -> Vec<UiAction> {
    click_with(document, v, at, PointerButton::Primary, clicks)
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

fn geom() -> sp::SaturationGeom {
    let reg = registry();
    atomik_ui::fx_layout::fx_layout(
        FxKind::Saturation,
        reg.get(SATURATION_TYPE).unwrap().descriptor,
    )
    .fx
    .unwrap()
    .saturation
    .unwrap()
}

/// The point of the type switch's position `i` (0 = top), in view coordinates at zoom 1.
fn switch_at(v: &RackView, i: usize) -> Point {
    let c = center(v, "type");
    let g = geom();
    Point::new(c.x, c.y + g.switch_y(i) - g.switch.center().y)
}

fn key_event(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn escape(document: &mut Document, v: &mut RackView) {
    let a = v.handle(&key_event(Key::Escape), document.doc(), &registry());
    apply(document, v, &a);
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

fn painted(document: &Document, meters: Option<&[f32]>, flip: Flip) -> DisplayList {
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
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &reg);
    dl
}

// ------------------------------------------------------------------ layout

#[test]
fn the_meter_layout_is_pinned_to_the_device() {
    use atomik_devices::saturation as dev;
    assert_eq!(fx::SATURATION_METERS, dev::METERS);
    assert_eq!(FxKind::Saturation.meter_count(), dev::METERS);
    assert_eq!(FxKind::Saturation.type_id(), dev::DESCRIPTOR.type_id);
    assert_eq!(sp::SATURATION_TYPE, dev::DESCRIPTOR.type_id);
    assert_eq!(sp::METER_DRIVE, dev::meter::DRIVE);
    assert_eq!(sp::METER_INPUT, dev::meter::INPUT);
    assert_eq!(sp::METER_OUTPUT, dev::meter::OUTPUT);
    let t = dev::DESCRIPTOR.param_by_key("type").unwrap();
    assert_eq!((t.min, t.max), (0.0, 2.0), "three positions");
}

#[test]
fn the_panel_is_one_unit_and_every_part_is_inside_it() {
    let reg = registry();
    let d = reg.get(SATURATION_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::Saturation, d);
    assert_eq!(spec.height_units, 1);
    let g = geom();
    let h = spec.height();
    for r in [g.knob, g.lamp, g.switch] {
        assert!(r.y0 >= 0.0 && r.y1 <= h && r.x1 <= atomik_ui::autolayout::CONTENT_X.1);
    }
    // The big knob's hit rect is its own size (about 58 px), not the 110 px slot.
    let mix = spec
        .front
        .iter()
        .find(|c| d.params[c.param].key == "mix")
        .unwrap();
    assert!(g.knob.width() > mix.rect.width() && g.knob.width() <= 60.0);
    assert!(g.knob.height() <= 60.0);
    assert!(
        g.knob.x1 <= g.lamp.x0 && g.lamp.x1 <= g.switch.x0,
        "left to right"
    );
    assert!(g.switch.x1 <= mix.rect.x0);
    let labels: Vec<String> = spec
        .front_sections
        .iter()
        .map(|s| s.label.clone())
        .collect();
    assert_eq!(
        labels,
        [
            "SATURATION",
            "SATURATION TYPE",
            "MIX",
            "OUTPUT",
            "OVERSAMPLING"
        ]
    );
}

#[test]
fn the_switch_bands_and_steps_are_pure_maths() {
    let g = geom();
    let at = |i: usize| sp::type_at(g.switch, g.switch_y(i));
    assert_eq!([at(0), at(1), at(2)], [0, 1, 2]);
    assert_eq!(sp::type_at(g.switch, g.switch.y0 - 40.0), 0);
    assert_eq!(sp::type_at(g.switch, g.switch.y1 + 40.0), 2);
    // Up is up: towards keep high (0), and it stops there.
    assert_eq!(sp::type_step(1.0, 1.0), 0.0);
    assert_eq!(sp::type_step(0.0, 1.0), 0.0);
    assert_eq!(sp::type_step(1.0, -1.0), 2.0);
    assert_eq!(sp::type_step(2.0, -1.0), 2.0);
    assert_eq!(sp::knob_text(0.0), "OFF");
    assert_eq!(sp::knob_text(0.43), "43 %");
    assert_eq!(sp::lamp_level(f32::NAN), 0.0);
    assert_eq!(sp::lamp_level(3.0), 1.0);
}

// ------------------------------------------------------------------ the big knob

#[test]
fn a_drag_of_the_big_knob_is_one_undo_step() {
    let (mut document, mut v) = rig();
    let c = center(&v, "saturation");
    let depth = document.undo_depth();
    let a = drag_with(
        &mut document,
        &mut v,
        c,
        &[Point::new(c.x, c.y - 20.0), Point::new(c.x, c.y - 40.0)],
        Modifiers::NONE,
    );
    let sets = set_params(&a);
    assert!(
        sets.len() >= 2 && sets.iter().all(|s| s.0 == "saturation"),
        "{sets:?}"
    );
    let merge = sets[0].2;
    assert!(merge.is_some() && sets.iter().all(|s| s.2 == merge));
    assert!(a.contains(&UiAction::EndMerge));
    assert!(param(&document, "saturation") > 0.1);
    assert_eq!(document.undo_depth(), depth + 1);
    assert!(document.undo().is_some());
    assert_eq!(param(&document, "saturation"), 0.0);
}

#[test]
fn shift_makes_the_big_knob_finer_and_escape_restores_it() {
    let travel = |m: Modifiers| {
        let (mut document, mut v) = rig();
        let c = center(&v, "saturation");
        drag_with(&mut document, &mut v, c, &[Point::new(c.x, c.y - 40.0)], m);
        param(&document, "saturation")
    };
    let (coarse, fine) = (travel(Modifiers::NONE), travel(Modifiers::SHIFT));
    assert!(
        coarse > 0.05 && (coarse / fine - 10.0).abs() < 0.5,
        "{coarse} {fine}"
    );

    let (mut document, mut v) = rig_with(&[("saturation", 0.3)]);
    let c = center(&v, "saturation");
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x, c.y - 50.0)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert!(param(&document, "saturation") > 0.35);
    escape(&mut document, &mut v);
    assert_eq!(param(&document, "saturation"), 0.3);
}

#[test]
fn the_wheel_and_double_click_and_right_click_work_on_the_big_knob() {
    let (mut document, mut v) = rig_with(&[("saturation", 0.5)]);
    let c = center(&v, "saturation");
    let a = wheel(&mut document, &mut v, c, 1.0);
    assert_eq!(set_params(&a).len(), 1);
    assert!(param(&document, "saturation") > 0.5);
    wheel(&mut document, &mut v, c, -3.0);
    assert!(param(&document, "saturation") < 0.5);
    // A double click puts it back to OFF.
    let a = click(&mut document, &mut v, c, 2);
    assert_eq!(set_params(&a), vec![("saturation".to_string(), 0.0, None)]);
    // A right click asks for the MIDI menu.
    let a = click_with(&mut document, &mut v, c, PointerButton::Secondary, 1);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::ControlMenu { param, .. } if param == "saturation")),
        "{a:?}"
    );
    assert!(set_params(&a).is_empty());
}

#[test]
fn the_tooltip_of_a_knob_drag_says_off_at_the_bottom() {
    let (mut document, mut v) = rig_with(&[("saturation", 0.2)]);
    let c = center(&v, "saturation");
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x, c.y + 400.0)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "saturation"), 0.0);
    assert!(
        v.tooltip().is_some_and(|t| t.contains("OFF")),
        "{:?}",
        v.tooltip()
    );
}

// ------------------------------------------------------------------ the type switch

#[test]
fn a_click_on_a_label_or_a_position_picks_it() {
    for i in 0..3usize {
        // From each start, a click on position i gives i, whatever it was before.
        for start in [0.0f32, 1.0, 2.0] {
            let (mut document, mut v) = rig_with(&[("type", start)]);
            let at = switch_at(&v, i);
            let a = click(&mut document, &mut v, at, 1);
            assert_eq!(param(&document, "type"), i as f32, "{start} -> {i}");
            let sets = set_params(&a);
            if (start - i as f32).abs() < 0.5 {
                assert!(sets.is_empty(), "nothing to change: {sets:?}");
            } else {
                assert_eq!(sets.len(), 1);
                assert_eq!(sets[0].0, "type");
            }
        }
    }
    // A click on the text of a label (right of the slot) is the same band.
    let (mut document, mut v) = rig();
    let g = geom();
    let top = switch_at(&v, 0);
    let text_x = top.x - g.switch.center().x + g.switch.x0 + 50.0 - (top.x - g.switch.center().x);
    click(&mut document, &mut v, Point::new(text_x, top.y), 1);
    assert_eq!(param(&document, "type"), 0.0);
}

#[test]
fn a_drag_on_the_switch_steps_with_up_being_up_in_one_undo_step() {
    let (mut document, mut v) = rig();
    let depth = document.undo_depth();
    let (a, b) = (switch_at(&v, 1), switch_at(&v, 0));
    let act = drag_with(
        &mut document,
        &mut v,
        a,
        &[Point::new(a.x, b.y)],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "type"), 0.0, "dragged up: keep high");
    assert!(
        set_params(&act)
            .iter()
            .all(|s| s.0 == "type" && s.2.is_some())
    );
    assert_eq!(document.undo_depth(), depth + 1);
    // From the top down past the bottom: the three positions in turn, one step.
    let depth = document.undo_depth();
    let (a, z) = (switch_at(&v, 0), switch_at(&v, 2));
    let act = drag_with(
        &mut document,
        &mut v,
        a,
        &[
            Point::new(a.x, (a.y + z.y) / 2.0),
            Point::new(a.x, z.y + 30.0),
        ],
        Modifiers::NONE,
    );
    assert_eq!(param(&document, "type"), 2.0);
    let values: Vec<f32> = set_params(&act).iter().map(|s| s.1).collect();
    assert!(
        values.contains(&1.0),
        "the middle position on the way: {values:?}"
    );
    assert_eq!(document.undo_depth(), depth + 1);
    assert!(document.undo().is_some());
    assert_eq!(param(&document, "type"), 0.0, "one undo for the whole drag");
}

#[test]
fn escape_during_a_switch_drag_puts_it_back() {
    let (mut document, mut v) = rig();
    let reg = registry();
    let (a, z) = (switch_at(&v, 1), switch_at(&v, 2));
    let r = v.handle(&down(a), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(&mv(Point::new(a.x, z.y + 20.0)), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    assert_eq!(param(&document, "type"), 2.0);
    escape(&mut document, &mut v);
    assert_eq!(param(&document, "type"), 1.0);
}

#[test]
fn the_wheel_and_the_arrows_step_the_switch_without_wrapping() {
    let (mut document, mut v) = rig();
    let at = center(&v, "type");
    // Up (positive) is towards keep high, and stops there.
    wheel(&mut document, &mut v, at, 1.0);
    assert_eq!(param(&document, "type"), 0.0);
    let a = wheel(&mut document, &mut v, at, 1.0);
    assert!(set_params(&a).is_empty(), "no wrap past the top");
    wheel(&mut document, &mut v, at, -1.0);
    wheel(&mut document, &mut v, at, -1.0);
    assert_eq!(param(&document, "type"), 2.0);
    let a = wheel(&mut document, &mut v, at, -1.0);
    assert!(set_params(&a).is_empty(), "no wrap past the bottom");
    // The arrow keys on the focused switch do the same.
    let reg = registry();
    let r = v.handle(&down(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(&up(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let before = param(&document, "type");
    let r = v.handle(&key_event(Key::Up), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    assert!(param(&document, "type") <= before);
    for _ in 0..3 {
        let r = v.handle(&key_event(Key::Up), document.doc(), &reg);
        apply(&mut document, &mut v, &r);
    }
    assert_eq!(param(&document, "type"), 0.0, "stops at the top");
    let r = v.handle(&key_event(Key::Down), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    assert_eq!(param(&document, "type"), 1.0);
}

#[test]
fn the_painted_switch_follows_the_value() {
    let lever = |t: f32| {
        let (document, _) = rig_with(&[("type", t)]);
        painted(&document, None, Flip::Front).to_json()
    };
    let (a, b, c) = (lever(0.0), lever(1.0), lever(2.0));
    assert!(a != b && b != c && a != c);
    let (document, _) = rig();
    let t: Vec<String> = painted(&document, None, Flip::Front)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    for s in [
        "KEEP HIGH",
        "NEUTRAL",
        "KEEP LOW",
        "OFF",
        "DRIVE",
        "SATURATION TYPE",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
}

// ------------------------------------------------------------------ the small group

#[test]
fn every_other_control_sets_its_param() {
    let (mut document, mut v) = rig();
    for (p, want) in [("auto_level", 0.0), ("oversampling", 2.0)] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {sets:?}");
        assert_eq!((sets[0].0.as_str(), sets[0].1), (p, want), "{p}");
    }
    // The wheel steps the selector (wrapping like the other selectors).
    let at = center(&v, "oversampling");
    wheel(&mut document, &mut v, at, 1.0);
    assert_eq!(param(&document, "oversampling"), 0.0);
    for p in ["mix", "output"] {
        let at = center(&v, p);
        let a = drag_with(
            &mut document,
            &mut v,
            at,
            &[Point::new(at.x, at.y - 25.0), Point::new(at.x, at.y + 40.0)],
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

// ------------------------------------------------------------------ the lamp

#[test]
fn the_lamp_reads_the_drive_meter_and_nothing_else() {
    let (document, _) = rig();
    let at = |m: [f32; 3]| painted(&document, Some(&m), Flip::Front).to_json();
    let off = at([0.0, 0.0, 0.0]);
    let half = at([0.5, 0.0, 0.0]);
    let full = at([1.0, 0.0, 0.0]);
    assert!(
        off != half && half != full && off != full,
        "brightness changes the lamp"
    );
    // The peaks are not shown by this panel.
    assert_eq!(off, at([0.0, 0.9, 0.9]));
    assert_eq!(half, at([0.5, 0.2, 0.7]));
    // The feed becomes the shown value unchanged.
    let mut shown = Vec::new();
    fx::update_display(FxKind::Saturation, &[0.5, 0.5, 0.5], &mut shown);
    assert_eq!(shown[sp::METER_DRIVE], 0.5);
}

// ------------------------------------------------------------------ the back

#[test]
fn the_back_has_every_jack_hit_testable_and_the_trim() {
    let (mut document, mut v) = rig();
    v.set_flip(Flip::Back);
    let reg = registry();
    let desc = reg.get(SATURATION_TYPE).unwrap().descriptor;
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
    let at = center(&v, "saturation_cv_trim");
    let a = drag_with(
        &mut document,
        &mut v,
        at,
        &[Point::new(at.x, at.y - 30.0), Point::new(at.x, at.y + 40.0)],
        Modifiers::NONE,
    );
    assert!(set_params(&a).iter().any(|s| s.0 == "saturation_cv_trim"));
    // The front controls are not on the back and the trim is not on the front.
    assert!(v.control_center(&key("fx"), "saturation").is_none());
    v.set_flip(Flip::Front);
    assert!(v.control_center(&key("fx"), "saturation_cv_trim").is_none());
    // The trim sits beside the CV jack, not over it.
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::Saturation, desc);
    for c in spec.front.iter().filter(|c| spec.on_back(c)) {
        for s in &spec.back {
            let apart = c.rect.x0 >= s.rect.x1
                || c.rect.x1 <= s.rect.x0
                || c.rect.y0 >= s.rect.y1
                || c.rect.y1 <= s.rect.y0;
            assert!(apart, "{:?} over {:?}", c.rect, s.rect);
        }
    }
}

#[test]
fn a_cable_from_another_device_reaches_the_audio_and_cv_jacks() {
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
    for (from_socket, to_socket) in [("out_l", "in_l"), ("lfo_cv_out", "saturation_cv_in")] {
        let from = v
            .socket_center(&sref("src", from_socket))
            .expect("the synth's out jack");
        let to = v.socket_center(&sref("fx", to_socket)).unwrap();
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
            "{to_socket}: {cmds:?}"
        );
    }
}

// ------------------------------------------------------------------ French

/// Both faces painted in French: every fixed word has an entry.
#[test]
fn the_panel_in_french() {
    use atomik_ui::i18n::{self, Lang};
    let doc = fx_doc(FxKind::Saturation);
    let reg = registry();
    for (name, flip) in [("front", Flip::Front), ("back", Flip::Back)] {
        let (list, misses) = i18n::with_misses(|| {
            i18n::with_language(Lang::French, || {
                let mut v = RackView::new(Theme::dark());
                let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
                v.sync(&doc, &reg);
                v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::Saturation));
                v.set_flip(flip);
                let mut dl = DisplayList::new();
                v.paint(&mut dl, &doc, &reg);
                dl
            })
        });
        let missing: Vec<&String> = misses
            .iter()
            .filter(|m| {
                !matches!(m.as_str(), "FX" | "Saturation")
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
            for s in ["GARDER LES AIGUS", "NEUTRE", "GARDER LES GRAVES"] {
                assert!(french.contains(&s), "{s}: {french:?}");
            }
        }
    }
}

// ------------------------------------------------------------------ previews

/// Writes previews of both faces to `target/preview`: `cargo test -p atomik-ui --test
/// saturation_panel -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let reg = registry();
    type Case = (&'static str, Flip, Vec<(&'static str, f32)>, [f32; 3]);
    let cases: Vec<Case> = vec![
        (
            "front_off",
            Flip::Front,
            vec![("saturation", 0.0), ("type", 1.0)],
            [0.0, 0.0, 0.0],
        ),
        (
            "front_mid",
            Flip::Front,
            vec![("saturation", 0.5), ("type", 0.0)],
            [0.4, 0.5, 0.5],
        ),
        (
            "front_full",
            Flip::Front,
            vec![("saturation", 1.0), ("type", 2.0)],
            [1.0, 0.5, 0.5],
        ),
        ("back", Flip::Back, vec![], [0.0, 0.0, 0.0]),
    ];
    for (name, flip, params, meters) in cases {
        let mut doc = fx_doc(FxKind::Saturation);
        for (k, v) in &params {
            doc.devices[0].params.insert((*k).to_string(), *v);
        }
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 180.0)), &doc, &reg);
        v.sync(&doc, &reg);
        v.set_device_meter_values(&key("fx"), &meters);
        v.set_flip(flip);
        let _ = v.handle(&UiEvent::ScaleChanged(2.0), &doc, &reg);
        let mut text = TextEngine::new();
        let mut canvas = CpuCanvas::new(1640, 360, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/preview/saturation_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}

#[test]
fn every_french_section_title_fits_its_section() {
    use atomik_ui::i18n::{self, Lang};
    let reg = registry();
    let d = reg.get(SATURATION_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::Saturation, d);
    let theme = Theme::dark();
    i18n::with_language(Lang::French, || {
        for s in spec.front_sections.iter().chain(&spec.back_sections) {
            let text = i18n::tr(&s.label);
            // Painted with 1 px of letter spacing; keep the rule's 6 px gap too.
            let w = atomik_ui::paint::estimate_width(&text, theme.section_size)
                + text.chars().count() as f64
                + 6.0;
            assert!(w <= s.rect.width(), "{text}: {w} > {}", s.rect.width());
        }
    });
}

#[test]
fn enter_on_the_type_switch_wraps_and_goes_through_the_shared_step() {
    use atomik_model::MacroAssign;
    let reg = registry();
    // Free: Enter cycles down and wraps from keep low back to keep high.
    let (mut document, mut v) = rig_with(&[("type", 1.0)]);
    let at = center(&v, "type");
    let r = v.handle(&down(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(&up(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    // The click picked the middle position; Enter goes down to keep low, then wraps to the top.
    for want in [2.0, 0.0] {
        let r = v.handle(&key_event(Key::Enter), document.doc(), &reg);
        apply(&mut document, &mut v, &r);
        assert_eq!(param(&document, "type"), want);
    }
    // Driven by a macro.
    let mut doc = fx_doc(FxKind::Saturation);
    doc.devices[0].params.insert("type".into(), 1.0);
    let mut bank = device("bank", "atomik.macro-bank", "Macro Bank");
    bank.params.insert("macro1".into(), 0.5);
    doc.devices.push(bank);
    doc.macros.assigns.push(MacroAssign {
        id: 1,
        bank: key("bank"),
        index: 0,
        device: key("fx"),
        param: "type".into(),
        min: 0.0,
        max: 2.0,
        invert: false,
        curve: Default::default(),
        smooth_ms: 0,
    });
    let mut v = plain_view(&doc, &reg);
    let at = v.control_center(&key("fx"), "type").unwrap();
    let _ = v.handle(&down(at), &doc, &reg);
    let _ = v.handle(&up(at), &doc, &reg);
    for (n, k) in [Key::Enter, Key::Space].into_iter().enumerate() {
        let a = v.handle(&key_event(k), &doc, &reg);
        // In the rack proper a driven control still edits through its macro, like every other
        // control: exactly one command, and it comes from the shared step (which blocks a driven
        // param inside a container).
        assert_eq!(set_params(&a).len(), 1, "key {n}: {a:?}");
    }
}
