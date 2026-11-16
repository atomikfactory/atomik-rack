//! The Headphone Correction effect's own 2 U panel: the meter layout, the layout, every control
//! (merged drag, wheel, double-click, MIDI menu, selectors, toggles), the IMPORT and REMOVE
//! buttons and the clickable empty graph, the dimmed selectors for parametric data, the curves
//! drawn being the device's own snapshot, one undo step per drag, the back jacks and cables and
//! the French texts. The display-list goldens are `fx_headphone-correction_*` in `fx_panels.rs`
//! (with data) and `headphone_*` here (empty). Previews: `cargo test -p atomik-ui --test
//! headphone_panel -- --ignored`.

mod support;

use atomik_device_sdk::headphone::{Filter, FilterData, FilterKind, HeadphoneBody, HeadphoneData};
use atomik_devices::headphone_correction::curve::{self, DataKind, Settings, Target};
use atomik_model::{Command, Document, RackDoc};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::headphone_panel::{self as hp, HEADPHONE_TYPE};
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use support::fx::*;
use support::starter::check_golden as check;
use support::*;

fn doc_with(params: &[(&str, f32)], data: Option<HeadphoneData>) -> RackDoc {
    let mut doc = fx_doc(FxKind::HeadphoneCorrection);
    doc.devices[0].headphone = data;
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    doc
}

fn view_of(doc: &RackDoc) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), doc, &reg);
    v.sync(doc, &reg);
    v
}

fn rig_with(params: &[(&str, f32)], data: Option<HeadphoneData>) -> (Document, RackView) {
    let doc = doc_with(params, data);
    let v = view_of(&doc);
    (Document::new(doc), v)
}

/// The device's defaults, with the sample data.
fn rig() -> (Document, RackView) {
    rig_with(
        &[
            ("amount", 1.0),
            ("target", 0.0),
            ("file_kind", 0.0),
            ("simulate", 0.0),
            ("auto_gain", 1.0),
            ("output", 0.0),
        ],
        Some(sample_headphones()),
    )
}

fn empty_rig() -> (Document, RackView) {
    rig_with(&[], None)
}

fn parametric() -> HeadphoneData {
    HeadphoneData {
        name: "bands.txt".to_owned(),
        body: HeadphoneBody::Filters(FilterData {
            preamp_db: -3.0,
            filters: vec![
                Filter {
                    kind: FilterKind::Peak,
                    freq: 1000.0,
                    gain_db: 4.0,
                    q: 1.0,
                },
                Filter {
                    kind: FilterKind::Peak,
                    freq: 6000.0,
                    gain_db: -3.0,
                    q: 2.0,
                },
            ],
        }),
    }
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

fn geom() -> hp::HeadphoneGeom {
    let reg = registry();
    atomik_ui::fx_layout::fx_layout(
        FxKind::HeadphoneCorrection,
        reg.get(HEADPHONE_TYPE).unwrap().descriptor,
    )
    .fx
    .unwrap()
    .headphone
    .unwrap()
}

/// The view point of a panel-coordinate point of the (only) device, at zoom 1.
fn at_panel(v: &RackView, p: Point) -> Point {
    // The generic control cell and its panel rect give the offset between the two.
    let reg = registry();
    let spec = atomik_ui::fx_layout::fx_layout(
        FxKind::HeadphoneCorrection,
        reg.get(HEADPHONE_TYPE).unwrap().descriptor,
    );
    let amount = spec
        .front
        .iter()
        .find(|c| reg.get(HEADPHONE_TYPE).unwrap().descriptor.params[c.param].key == "amount")
        .unwrap();
    let c = center(v, "amount");
    // `control_center` of a knob is the middle of its dial, not of its cell.
    let panel_center = Point::new(
        amount.rect.center().x,
        amount.rect.y0 + atomik_ui::paint::KNOB_DY,
    );
    Point::new(c.x + p.x - panel_center.x, c.y + p.y - panel_center.y)
}

fn click_panel(document: &mut Document, v: &mut RackView, p: Point) -> Vec<UiAction> {
    let at = at_panel(v, p);
    click(document, v, at, 1)
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

fn painted(doc: &RackDoc, meters: Option<&[f32]>, flip: Flip) -> DisplayList {
    let reg = registry();
    let mut v = view_of(doc);
    if let Some(m) = meters {
        v.set_device_meter_values(&key("fx"), m);
    }
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    dl
}

fn texts_of(dl: &DisplayList) -> Vec<String> {
    dl.texts().into_iter().map(str::to_string).collect()
}

// ------------------------------------------------------------------ layout

#[test]
fn the_meter_layout_is_pinned_to_the_device() {
    use atomik_devices::headphone_correction as dev;
    assert_eq!(fx::HEADPHONE_METERS, dev::METERS);
    assert_eq!(hp::HEADPHONE_METERS, dev::METERS);
    assert_eq!(FxKind::HeadphoneCorrection.meter_count(), dev::METERS);
    assert_eq!(FxKind::HeadphoneCorrection.type_id(), HEADPHONE_TYPE);
    assert_eq!(hp::METER_LOADED, dev::meter::LOADED);
    assert_eq!(hp::METER_PREAMP, dev::meter::PREAMP);
    assert_eq!(hp::METER_BOOST, dev::meter::BOOST);
    assert_eq!(hp::METER_BUSY, dev::meter::BUSY);
    let reg = registry();
    let d = reg.get(HEADPHONE_TYPE).unwrap().descriptor;
    for k in [
        "amount",
        "target",
        "file_kind",
        "simulate",
        "auto_gain",
        "output",
    ] {
        assert!(d.param_by_key(k).is_some(), "{k}");
    }
    let amount = d.param_by_key("amount").unwrap();
    assert_eq!((amount.min, amount.max, amount.default), (0.0, 2.0, 1.0));
}

#[test]
fn the_meters_become_the_values_the_panel_shows() {
    let mut shown = Vec::new();
    fx::update_display(
        FxKind::HeadphoneCorrection,
        &[1.0, -4.2, 4.2, 1.0],
        &mut shown,
    );
    assert_eq!(shown[hp::METER_LOADED], 1.0);
    assert!((shown[hp::METER_PREAMP] + 4.2).abs() < 0.051);
    assert!((shown[hp::METER_BOOST] - 4.2).abs() < 0.051);
    assert_eq!(shown[hp::METER_BUSY], 1.0);
}

#[test]
fn the_panel_is_two_units_and_every_part_is_inside_it() {
    let reg = registry();
    let d = reg.get(HEADPHONE_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::HeadphoneCorrection, d);
    assert_eq!(spec.height_units, 2);
    let g = geom();
    let h = spec.height();
    let x1 = atomik_ui::autolayout::CONTENT_X.1;
    for r in [
        g.name,
        g.import,
        g.remove,
        g.plot,
        g.caption,
        g.readout,
        g.selectors[0],
        g.selectors[1],
        g.simulate,
        g.amount,
    ] {
        assert!(r.y0 >= 0.0 && r.y1 <= h && r.x1 <= x1, "{r:?}");
    }
    // The buttons are on the name line, the graph and the caption under it, left of the controls.
    assert!(g.name.x1 <= g.import.x0 && g.import.x1 <= g.remove.x0);
    assert!(g.name.y1 <= g.plot.y0 && g.plot.y1 <= g.caption.y0);
    for c in &spec.front {
        assert!(c.rect.x0 >= g.plot.x1, "{:?} over the graph", c.rect);
    }
    let labels: Vec<String> = spec
        .front_sections
        .iter()
        .map(|s| s.label.clone())
        .collect();
    assert_eq!(
        labels,
        [
            "HEADPHONES",
            "AMOUNT",
            "OUTPUT",
            "AUTO GAIN",
            "TARGET",
            "DATA IS",
            "SIMULATE"
        ]
    );
    // No placed controls overlap each other.
    for (i, a) in spec.front.iter().enumerate() {
        for b in &spec.front[i + 1..] {
            let apart = a.rect.x1 <= b.rect.x0
                || b.rect.x1 <= a.rect.x0
                || a.rect.y1 <= b.rect.y0
                || b.rect.y1 <= a.rect.y0;
            assert!(apart, "{:?} over {:?}", a.rect, b.rect);
        }
    }
}

#[test]
fn the_pure_helpers_read_the_way_the_panel_draws() {
    let g = geom();
    assert_eq!(hp::x_of(g.plot, 20.0), g.plot.x0);
    assert!((hp::x_of(g.plot, 20_000.0) - g.plot.x1).abs() < 1e-9);
    let mid = hp::x_of(g.plot, 632.455_532);
    assert!((mid - g.plot.center().x).abs() < 1e-3, "log axis");
    assert_eq!(hp::y_of(g.plot, 12.0, 0.0), g.plot.center().y);
    assert!(
        hp::y_of(g.plot, 12.0, 6.0) < g.plot.center().y,
        "boost is up"
    );
    assert_eq!(hp::preamp_text(true, -4.24), "-4.2 dB");
    assert_eq!(hp::preamp_text(true, 0.0), "+0.0 dB");
    assert_eq!(hp::preamp_text(false, -4.0), "-");
}

// ------------------------------------------------------------------ the controls

#[test]
fn a_drag_of_a_knob_is_one_undo_step_and_the_gestures_work() {
    for p in ["amount", "output"] {
        let (mut document, mut v) = rig();
        let c = center(&v, p);
        let before = param(&document, p);
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
            sets.len() >= 2 && sets.iter().all(|s| s.0 == p),
            "{p}: {sets:?}"
        );
        let merge = sets[0].2;
        assert!(merge.is_some() && sets.iter().all(|s| s.2 == merge), "{p}");
        assert!(a.contains(&UiAction::EndMerge), "{p}");
        assert!((param(&document, p) - before).abs() > 1e-6, "{p}");
        assert_eq!(document.undo_depth(), depth + 1, "{p}");
        assert!(document.undo().is_some());
        assert_eq!(param(&document, p), before, "{p}: one undo for the drag");
    }
}

#[test]
fn shift_makes_the_amount_finer_and_escape_restores_it() {
    let travel = |m: Modifiers| {
        let (mut document, mut v) = rig_with(&[("amount", 1.0)], Some(sample_headphones()));
        let c = center(&v, "amount");
        drag_with(&mut document, &mut v, c, &[Point::new(c.x, c.y - 30.0)], m);
        (param(&document, "amount") - 1.0).abs()
    };
    let (coarse, fine) = (travel(Modifiers::NONE), travel(Modifiers::SHIFT));
    assert!(
        coarse > 0.05 && (coarse / fine - 10.0).abs() < 0.8,
        "{coarse} {fine}"
    );

    let (mut document, mut v) = rig();
    let c = center(&v, "amount");
    let reg = registry();
    let a = v.handle(&down(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&mv(Point::new(c.x, c.y - 50.0)), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert!(param(&document, "amount") > 1.05);
    escape(&mut document, &mut v);
    assert_eq!(param(&document, "amount"), 1.0);
}

#[test]
fn the_wheel_double_click_and_right_click_work_on_the_knobs() {
    for (p, default) in [("amount", 1.0f32), ("output", 0.0)] {
        let (mut document, mut v) = rig_with(
            &[("amount", 0.5), ("output", 3.0)],
            Some(sample_headphones()),
        );
        let c = center(&v, p);
        let start = param(&document, p);
        let a = wheel(&mut document, &mut v, c, 1.0);
        assert_eq!(set_params(&a).len(), 1, "{p}");
        assert!(param(&document, p) > start, "{p}");
        wheel(&mut document, &mut v, c, -1.0);
        wheel(&mut document, &mut v, c, -1.0);
        assert!(param(&document, p) < start, "{p}");
        // A double click puts it back to its default.
        let a = click(&mut document, &mut v, c, 2);
        assert_eq!(set_params(&a), vec![(p.to_string(), default, None)], "{p}");
        // A right click asks for the MIDI menu.
        let a = click_with(&mut document, &mut v, c, PointerButton::Secondary, 1);
        assert!(
            a.iter()
                .any(|a| matches!(a, UiAction::ControlMenu { param, .. } if param == p)),
            "{p}: {a:?}"
        );
        assert!(set_params(&a).is_empty(), "{p}");
    }
}

#[test]
fn every_other_control_sets_its_param() {
    let (mut document, mut v) = rig();
    // The toggles flip on a click.
    for (p, want) in [("simulate", 1.0), ("auto_gain", 0.0)] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {sets:?}");
        assert_eq!((sets[0].0.as_str(), sets[0].1), (p, want), "{p}");
        assert_eq!(param(&document, p), want, "{p}");
    }
    // The selectors step with a click, with the wheel and with the arrows.
    for p in ["target", "file_kind"] {
        let at = center(&v, p);
        let a = click(&mut document, &mut v, at, 1);
        let sets = set_params(&a);
        assert_eq!(sets.len(), 1, "{p}: {sets:?}");
        assert_eq!(sets[0].0, p);
        let now = param(&document, p);
        wheel(&mut document, &mut v, at, 1.0);
        assert!(
            (param(&document, p) - now).abs() > 1e-6,
            "{p}: the wheel steps it"
        );
    }
    // The arrow keys step the focused selector too.
    let at = center(&v, "target");
    let reg = registry();
    let r = v.handle(&down(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let r = v.handle(&up(at), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    let before = param(&document, "target");
    let r = v.handle(&key_event(Key::Up), document.doc(), &reg);
    apply(&mut document, &mut v, &r);
    assert!(
        (param(&document, "target") - before).abs() > 1e-6,
        "an arrow steps it"
    );
}

// ------------------------------------------------------------------ the buttons

fn import_of(actions: &[UiAction]) -> Vec<atomik_model::DeviceKey> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::HeadphoneImport(k) => Some(k.clone()),
            _ => None,
        })
        .collect()
}

fn remove_of(actions: &[UiAction]) -> Vec<atomik_model::DeviceKey> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::HeadphoneRemove(k) => Some(k.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_empty_graph_says_so_and_a_click_on_it_imports() {
    let (mut document, mut v) = empty_rig();
    let doc = document.doc().clone();
    let t = texts_of(&painted(&doc, None, Flip::Front));
    for s in [
        "Import headphone data...",
        "No headphone data",
        "IMPORT",
        "HEADPHONES",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    let g = geom();
    let a = click_panel(&mut document, &mut v, g.plot.center());
    assert_eq!(import_of(&a), vec![key("fx")], "{a:?}");
    assert!(set_params(&a).is_empty());
    // A press near a corner of the graph counts too.
    let corner = Point::new(g.plot.x1 - 4.0, g.plot.y0 + 4.0);
    let a = click_panel(&mut document, &mut v, corner);
    assert_eq!(import_of(&a), vec![key("fx")], "corner: {a:?}");
}

#[test]
fn the_import_and_remove_buttons_follow_the_data() {
    let g = geom();
    // With data: IMPORT imports (to pick another file), REMOVE removes, the graph does nothing.
    let (mut document, mut v) = rig();
    let a = click_panel(&mut document, &mut v, g.import.center());
    assert_eq!(import_of(&a), vec![key("fx")], "{a:?}");
    let a = click_panel(&mut document, &mut v, g.plot.center());
    assert!(
        import_of(&a).is_empty() && remove_of(&a).is_empty(),
        "{a:?}"
    );
    let a = click_panel(&mut document, &mut v, g.remove.center());
    assert_eq!(remove_of(&a), vec![key("fx")], "{a:?}");
    // Without data: REMOVE is dim and does nothing, IMPORT still works.
    let (mut document, mut v) = empty_rig();
    let a = click_panel(&mut document, &mut v, g.remove.center());
    assert!(
        remove_of(&a).is_empty() && import_of(&a).is_empty(),
        "{a:?}"
    );
    let a = click_panel(&mut document, &mut v, g.import.center());
    assert_eq!(import_of(&a), vec![key("fx")]);
    // The hit regions follow the document: the same view, data arriving and going.
    let (mut document, mut v) = empty_rig();
    let mut doc = document.doc().clone();
    doc.devices[0].headphone = Some(sample_headphones());
    document = Document::new(doc);
    v.sync(document.doc(), &registry());
    let a = click_panel(&mut document, &mut v, g.remove.center());
    assert_eq!(remove_of(&a), vec![key("fx")], "data arrived: {a:?}");
    let a = click_panel(&mut document, &mut v, g.plot.center());
    assert!(import_of(&a).is_empty(), "the graph no longer imports");
    let mut doc = document.doc().clone();
    doc.devices[0].headphone = None;
    document = Document::new(doc);
    v.sync(document.doc(), &registry());
    let a = click_panel(&mut document, &mut v, g.plot.center());
    assert_eq!(import_of(&a), vec![key("fx")], "data gone: {a:?}");
}

#[test]
fn the_buttons_have_tooltips_and_ask_for_the_commands() {
    let (_, v) = rig();
    let g = geom();
    let import = v.help_at(at_panel(&v, g.import.center()));
    assert_eq!(
        import.and_then(|h| h.id),
        Some("rack.headphone_import".into())
    );
    let remove = v.help_at(at_panel(&v, g.remove.center()));
    assert_eq!(
        remove.and_then(|h| h.id),
        Some("rack.headphone_remove".into())
    );
}

// ------------------------------------------------------------------ the picture

fn dimming_veils(dl: &DisplayList) -> usize {
    use atomik_ui::Op;
    let g = geom();
    let sizes: Vec<(f64, f64)> = g
        .selectors
        .iter()
        .map(|r| (r.width() + 2.0, r.height() + 2.0))
        .collect();
    dl.ops()
        .iter()
        .filter(|op| match op {
            Op::Fill {
                shape: atomik_ui::Geom::RoundedRect(r),
                brush: atomik_ui::peniko::Brush::Solid(c),
            } => {
                let b = r.rect();
                let a = c.to_rgba8().a;
                sizes
                    .iter()
                    .any(|(w, h)| (b.width() - w).abs() < 1e-6 && (b.height() - h).abs() < 1e-6)
                    && (110..=120).contains(&a)
            }
            _ => false,
        })
        .count()
}

#[test]
fn the_selectors_are_greyed_for_parametric_data_only() {
    let measured = painted(
        &doc_with(&[], Some(sample_headphones())),
        Some(&raw_meters(FxKind::HeadphoneCorrection)),
        Flip::Front,
    );
    assert_eq!(dimming_veils(&measured), 0, "a response uses both");
    let empty = painted(&doc_with(&[], None), None, Flip::Front);
    assert_eq!(dimming_veils(&empty), 0, "no data: nothing is dimmed");
    let bands = painted(
        &doc_with(&[], Some(parametric())),
        Some(&raw_meters(FxKind::HeadphoneCorrection)),
        Flip::Front,
    );
    assert_eq!(dimming_veils(&bands), 2, "TARGET and DATA IS are dimmed");
    // A dimmed selector still takes a value (it is kept for the next response).
    let (mut document, mut v) = rig_with(&[], Some(parametric()));
    let at = center(&v, "target");
    let a = click(&mut document, &mut v, at, 1);
    assert_eq!(set_params(&a).len(), 1);
}

/// The points of the stroked path of width `width` (the highlighted correction is 2 wide).
fn stroked_path(dl: &DisplayList, width: f64) -> Vec<Point> {
    use atomik_ui::{Geom, Op};
    for op in dl.ops() {
        if let Op::Stroke {
            shape: Geom::Path(p),
            stroke,
            ..
        } = op
            && (stroke.width - width).abs() < 1e-9
            && stroke.dash_pattern.is_empty()
        {
            return p
                .elements()
                .iter()
                .filter_map(|e| match e {
                    atomik_ui::kurbo::PathEl::MoveTo(p) | atomik_ui::kurbo::PathEl::LineTo(p) => {
                        Some(*p)
                    }
                    _ => None,
                })
                .collect();
        }
    }
    Vec::new()
}

/// The curve of `data` and `settings` as the panel should draw it, relative to its first point.
fn expected_correction(data: &HeadphoneData, s: &Settings) -> Vec<Vec2> {
    let snap = curve::snapshot(data, 0, s, hp::DRAW_RATE, hp::GRAPH_POINTS);
    let g = geom();
    let g2 = hp::HpGraph::build(data, s);
    assert_eq!(g2.snap, snap, "the panel's graph is the device's snapshot");
    let pts: Vec<Point> = snap
        .correction
        .iter()
        .enumerate()
        .map(|(i, db)| {
            Point::new(
                g.plot.x0 + i as f64 / (snap.correction.len() - 1) as f64 * g.plot.width(),
                hp::y_of(g.plot, g2.range_db, *db),
            )
        })
        .collect();
    pts.iter().map(|p| *p - pts[0]).collect()
}

fn assert_correction_drawn(doc: &RackDoc, settings: &Settings) {
    let dl = painted(
        doc,
        Some(&raw_meters(FxKind::HeadphoneCorrection)),
        Flip::Front,
    );
    let got = stroked_path(&dl, 2.0);
    let data = doc.devices[0].headphone.as_ref().unwrap();
    let want = expected_correction(data, settings);
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        let d = (*g - got[0]) - *w;
        assert!(d.hypot() < 0.01, "{d:?}");
    }
}

#[test]
fn the_curve_drawn_is_the_snapshot_of_the_device() {
    let settings = Settings {
        kind: DataKind::Response,
        target: Target::Studio,
        amount: 1.0,
        simulate: false,
        auto_gain: true,
    };
    let doc = doc_with(
        &[("target", 1.0), ("amount", 1.0), ("file_kind", 0.0)],
        Some(sample_headphones()),
    );
    assert_correction_drawn(&doc, &settings);
    // Amount, simulate, target and the data kind change what is drawn, exactly as they change
    // what the device applies.
    let doc = doc_with(
        &[("target", 0.0), ("amount", 1.6), ("simulate", 1.0)],
        Some(sample_headphones()),
    );
    assert_correction_drawn(
        &doc,
        &Settings {
            target: Target::Flat,
            amount: 1.6,
            simulate: true,
            ..settings
        },
    );
    let doc = doc_with(&[("file_kind", 1.0)], Some(sample_headphones()));
    assert_correction_drawn(
        &doc,
        &Settings {
            kind: DataKind::Correction,
            target: Target::Flat,
            ..settings
        },
    );
    let doc = doc_with(&[("amount", 0.5)], Some(parametric()));
    assert_correction_drawn(
        &doc,
        &Settings {
            kind: DataKind::Response,
            target: Target::Flat,
            amount: 0.5,
            ..settings
        },
    );
}

#[test]
fn amount_zero_draws_a_flat_correction_and_simulate_says_so() {
    let doc = doc_with(&[("amount", 0.0)], Some(sample_headphones()));
    let dl = painted(&doc, None, Flip::Front);
    let ys: Vec<f64> = stroked_path(&dl, 2.0).iter().map(|p| p.y).collect();
    assert!(ys.len() > 10 && ys.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-6));
    let t = texts_of(&dl);
    assert!(t.iter().any(|x| x == "CORRECTION"), "{t:?}");
    let doc = doc_with(&[("simulate", 1.0)], Some(sample_headphones()));
    let t = texts_of(&painted(&doc, None, Flip::Front));
    assert!(t.iter().any(|x| x == "SIMULATION"), "{t:?}");
    assert!(!t.iter().any(|x| x == "CORRECTION"), "{t:?}");
}

#[test]
fn the_legend_follows_what_is_drawn() {
    let t = texts_of(&painted(
        &doc_with(&[], Some(sample_headphones())),
        None,
        Flip::Front,
    ));
    for s in ["RESPONSE", "TARGET", "CORRECTION"] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    // A parametric file is a correction already: no response, no target curve.
    let t = texts_of(&painted(
        &doc_with(&[], Some(parametric())),
        None,
        Flip::Front,
    ));
    assert!(t.iter().any(|x| x == "CORRECTION"));
    assert!(!t.iter().any(|x| x == "RESPONSE"), "{t:?}");
    // Nor for a file that is a correction curve.
    let t = texts_of(&painted(
        &doc_with(&[("file_kind", 1.0)], Some(sample_headphones())),
        None,
        Flip::Front,
    ));
    assert!(!t.iter().any(|x| x == "RESPONSE"), "{t:?}");
}

#[test]
fn the_status_line_names_the_data_and_the_format_and_the_build() {
    let doc = doc_with(&[], Some(sample_headphones()));
    let t = texts_of(&painted(&doc, Some(&[1.0, -4.2, 4.2, 0.0]), Flip::Front));
    assert!(t.iter().any(|x| x == "Studio cans.csv"), "{t:?}");
    assert!(
        t.iter().any(|x| x == "Frequency response, 41 points"),
        "{t:?}"
    );
    assert!(!t.iter().any(|x| x == "building..."), "{t:?}");
    assert!(
        t.iter().any(|x| x == "-4.2 dB"),
        "the applied preamp: {t:?}"
    );
    for s in ["PREAMP", "MONITOR ONLY \u{2014} bypass before export"] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    let t = texts_of(&painted(&doc, Some(&[1.0, -4.2, 4.2, 1.0]), Flip::Front));
    assert!(t.iter().any(|x| x == "building..."), "{t:?}");
    let t = texts_of(&painted(&doc, Some(&[0.0, 0.0, 0.0, 0.0]), Flip::Front));
    assert!(t.iter().any(|x| x == "-"), "nothing applied yet: {t:?}");
    let t = texts_of(&painted(
        &doc_with(&[], Some(parametric())),
        None,
        Flip::Front,
    ));
    assert!(t.iter().any(|x| x == "bands.txt"), "{t:?}");
    assert!(t.iter().any(|x| x == "Parametric EQ, 2 bands"), "{t:?}");
    let mut stereo = sample_headphones();
    if let HeadphoneBody::Response(r) = &mut stereo.body {
        r.right = Some(r.left.clone());
    }
    let t = texts_of(&painted(&doc_with(&[], Some(stereo)), None, Flip::Front));
    assert!(
        t.iter()
            .any(|x| x == "Frequency response, 41 points, left and right"),
        "{t:?}"
    );
}

#[test]
fn the_simulate_lamp_is_lit_in_the_warning_colour() {
    use atomik_ui::Op;
    let lit = |on: f32| {
        let dl = painted(
            &doc_with(&[("simulate", on)], Some(sample_headphones())),
            None,
            Flip::Front,
        );
        let w = hp::warning().to_rgba8();
        dl.ops()
            .iter()
            .filter(|op| match op {
                Op::Fill {
                    brush: atomik_ui::peniko::Brush::Solid(c),
                    ..
                } => {
                    let c = c.to_rgba8();
                    (c.r, c.g, c.b) == (w.r, w.g, w.b)
                }
                _ => false,
            })
            .count()
    };
    assert!(
        lit(1.0) > lit(0.0),
        "the lamp and the curve use the warning colour"
    );
}

#[test]
fn the_graph_is_kept_while_nothing_changes() {
    use std::sync::Arc;
    let reg = registry();
    let desc = reg.get(HEADPHONE_TYPE).unwrap().descriptor;
    let doc = doc_with(&[], Some(sample_headphones()));
    let dev = &doc.devices[0];
    let first = hp::graph_of(Some(desc), dev, None).unwrap();
    let again = hp::graph_of(Some(desc), dev, Some(Arc::clone(&first))).unwrap();
    assert!(
        Arc::ptr_eq(&first, &again),
        "same data and settings: reused"
    );
    let mut louder = dev.clone();
    louder.params.insert("amount".into(), 1.5);
    let third = hp::graph_of(Some(desc), &louder, Some(Arc::clone(&first))).unwrap();
    assert!(!Arc::ptr_eq(&first, &third), "a setting changed: rebuilt");
    let mut other = dev.clone();
    if let Some(HeadphoneData {
        body: HeadphoneBody::Response(r),
        ..
    }) = &mut other.headphone
    {
        r.left[3] += 1.0;
    }
    let fourth = hp::graph_of(Some(desc), &other, Some(Arc::clone(&first))).unwrap();
    assert!(!Arc::ptr_eq(&first, &fourth), "the data changed: rebuilt");
    let mut none = dev.clone();
    none.headphone = None;
    assert!(hp::graph_of(Some(desc), &none, Some(first)).is_none());
}

// ------------------------------------------------------------------ the back

#[test]
fn the_back_has_every_jack_hit_testable() {
    let (mut document, mut v) = rig();
    v.set_flip(Flip::Back);
    let reg = registry();
    let desc = reg.get(HEADPHONE_TYPE).unwrap().descriptor;
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
    // The front controls are not on the back.
    assert!(v.control_center(&key("fx"), "amount").is_none());
    let t = texts_of(&painted(document.doc(), None, Flip::Back));
    for s in ["AUDIO IN", "AUDIO OUT"] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
}

#[test]
fn a_cable_from_another_device_reaches_the_audio_jacks() {
    let mut doc = doc_with(&[], Some(sample_headphones()));
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
    let connects = |cmds: &[Command]| {
        cmds.iter()
            .flat_map(|c| match c {
                Command::Batch(b) => b.clone(),
                other => vec![other.clone()],
            })
            .filter(|c| matches!(c, Command::Connect(_)))
            .count()
    };
    for (from_socket, to_socket) in [("out_l", "in_l"), ("out_r", "in_r")] {
        let from = v
            .socket_center(&sref("src", from_socket))
            .expect("the synth's out jack");
        let to = v.socket_center(&sref("fx", to_socket)).unwrap();
        let a = v.handle(&down(from), document.doc(), &reg);
        let b = v.handle(&mv(to), document.doc(), &reg);
        let c = v.handle(&up(to), document.doc(), &reg);
        let cmds = commands(&[a, b, c].concat());
        assert!(connects(&cmds) >= 1, "{to_socket}: {cmds:?}");
    }
    // And from the device's own out jacks to another device.
    let mut doc = doc_with(&[], Some(sample_headphones()));
    doc.devices.push(device("dst", "atomik.gain-tool", "Dst"));
    let document = Document::new(doc);
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(
        &UiEvent::Resize(Size::new(1000.0, 3000.0)),
        document.doc(),
        &reg,
    );
    v.sync(document.doc(), &reg);
    v.set_flip(Flip::Back);
    let from = v.socket_center(&sref("fx", "out_l")).unwrap();
    let to = v.socket_center(&sref("dst", "in_l")).unwrap();
    let a = v.handle(&down(from), document.doc(), &reg);
    let b = v.handle(&mv(to), document.doc(), &reg);
    let c = v.handle(&up(to), document.doc(), &reg);
    let cmds = commands(&[a, b, c].concat());
    assert!(connects(&cmds) >= 1, "{cmds:?}");
}

// ------------------------------------------------------------------ goldens

#[test]
fn goldens_of_the_empty_panel() {
    let doc = doc_with(&[], None);
    check(
        "headphone_empty_front_1x.json",
        &painted(&doc, Some(&[0.0, 0.0, 0.0, 0.0]), Flip::Front).to_json(),
    );
    let doc = doc_with(&[], Some(parametric()));
    check(
        "headphone_parametric_front_1x.json",
        &painted(&doc, Some(&[1.0, -3.0, 0.0, 0.0]), Flip::Front).to_json(),
    );
}

// ------------------------------------------------------------------ French

/// Every face and state painted in French: every fixed word has an entry.
#[test]
fn the_panel_in_french() {
    use atomik_ui::i18n::{self, Lang};
    let reg = registry();
    let mut stereo = sample_headphones();
    if let HeadphoneBody::Response(r) = &mut stereo.body {
        r.right = Some(r.left.clone());
    }
    let cases = [
        (
            "data",
            doc_with(&[], Some(sample_headphones())),
            Flip::Front,
        ),
        (
            "simulate",
            doc_with(&[("simulate", 1.0)], Some(stereo)),
            Flip::Front,
        ),
        ("bands", doc_with(&[], Some(parametric())), Flip::Front),
        ("empty", doc_with(&[], None), Flip::Front),
        ("back", doc_with(&[], Some(sample_headphones())), Flip::Back),
    ];
    for (name, doc, flip) in cases {
        for busy in [0.0f32, 1.0] {
            let (list, misses) = i18n::with_misses(|| {
                i18n::with_language(Lang::French, || {
                    let mut v = RackView::new(Theme::dark());
                    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
                    v.sync(&doc, &reg);
                    v.set_device_meter_values(&key("fx"), &[1.0, -4.2, 4.2, busy]);
                    v.set_flip(flip);
                    let mut dl = DisplayList::new();
                    v.paint(&mut dl, &doc, &reg);
                    dl
                })
            });
            let missing: Vec<&String> = misses
                .iter()
                .filter(|m| {
                    // The name of the device and of the imported file stay as they are.
                    !matches!(
                        m.as_str(),
                        "FX" | "Headphone Correction" | "Studio cans.csv" | "bands.txt"
                    ) && m.chars().filter(|c| c.is_alphabetic()).count() > 2
                        && !m.chars().any(|c| c.is_ascii_digit())
                })
                .collect();
            assert!(
                missing.is_empty(),
                "{name}: no French entry for {missing:?}"
            );
            let french = list.texts();
            if flip == Flip::Front && name == "empty" {
                for s in ["Importer les données du casque…", "Aucune donnée de casque"] {
                    let s = s.replace('…', "...");
                    assert!(french.contains(&s.as_str()), "{s}: {french:?}");
                }
            }
            if flip == Flip::Front && name == "data" {
                for s in ["Réponse en fréquence, 41 points", "IMPORTER"] {
                    assert!(french.contains(&s), "{s}: {french:?}");
                }
            }
        }
    }
}

#[test]
fn every_french_section_title_fits_its_section() {
    use atomik_ui::i18n::{self, Lang};
    let reg = registry();
    let d = reg.get(HEADPHONE_TYPE).unwrap().descriptor;
    let spec = atomik_ui::fx_layout::fx_layout(FxKind::HeadphoneCorrection, d);
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
/// headphone_panel -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let reg = registry();
    let mut tall = sample_headphones();
    if let HeadphoneBody::Response(r) = &mut tall.body {
        for v in &mut r.left {
            *v *= 3.0;
        }
    }
    type Case = (
        &'static str,
        Flip,
        Vec<(&'static str, f32)>,
        Option<HeadphoneData>,
        [f32; 4],
    );
    let cases: Vec<Case> = vec![
        (
            "front_data",
            Flip::Front,
            vec![("target", 1.0)],
            Some(sample_headphones()),
            [1.0, -4.2, 4.2, 0.0],
        ),
        (
            "front_simulate",
            Flip::Front,
            vec![("simulate", 1.0), ("amount", 1.4)],
            Some(tall),
            [1.0, -9.0, 9.0, 1.0],
        ),
        (
            "front_bands",
            Flip::Front,
            vec![],
            Some(parametric()),
            [1.0, -3.0, 0.0, 0.0],
        ),
        ("front_empty", Flip::Front, vec![], None, [0.0; 4]),
        (
            "back",
            Flip::Back,
            vec![],
            Some(sample_headphones()),
            [0.0; 4],
        ),
    ];
    for lang in [
        atomik_ui::i18n::Lang::English,
        atomik_ui::i18n::Lang::French,
    ] {
        for (name, flip, params, data, meters) in &cases {
            let doc = doc_with(params, data.clone());
            atomik_ui::i18n::with_language(lang, || {
                let mut v = RackView::new(Theme::dark());
                let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 340.0)), &doc, &reg);
                v.sync(&doc, &reg);
                v.set_device_meter_values(&key("fx"), meters);
                v.set_flip(*flip);
                let _ = v.handle(&UiEvent::ScaleChanged(2.0), &doc, &reg);
                let mut text = TextEngine::new();
                let mut canvas = CpuCanvas::new(1640, 680, &mut text);
                v.paint(&mut canvas, &doc, &reg);
                let png = canvas.finish().into_png().expect("png");
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                    "../../target/preview/headphone_{name}_{lang:?}.png"
                ));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, png).unwrap();
            });
        }
    }
}
