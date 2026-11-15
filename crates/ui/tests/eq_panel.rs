//! The EQ panel: the band model and the curve against the device, the layout, picking a handle,
//! every gesture of the graph (drag, Alt-drag, wheel, double-clicks, the right-click menu, Esc)
//! and the controls of the strips and the right column.

mod support;

use atomik_devices::eq as dev;
use atomik_model::{Command, Document};
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::eq_panel::{self, BANDS, Band, KEYS};
use atomik_ui::fx::FxKind;
use atomik_ui::fx_layout::fx_layout;
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent};
use support::fx::*;
use support::*;

const SR: f64 = 48_000.0;

fn desc() -> &'static atomik_device_sdk::DeviceDescriptor {
    registry().get("atomik.eq").unwrap().descriptor
}

// ------------------------------------------------------------------ the model and the curve

#[test]
fn every_key_of_the_band_table_is_a_param_or_socket_of_the_device() {
    let d = desc();
    for k in KEYS {
        for key in [
            Some(k.on),
            Some(k.freq),
            k.gain,
            Some(k.q),
            k.shape,
            k.slope,
            Some(k.freq_trim),
            Some(k.q_trim),
            k.gain_trim,
        ]
        .into_iter()
        .flatten()
        {
            assert!(d.param_by_key(key).is_some(), "param {key}");
        }
        for key in [Some(k.freq_cv), Some(k.q_cv), k.gain_cv]
            .into_iter()
            .flatten()
        {
            assert!(d.socket_by_key(key).is_some(), "socket {key}");
        }
    }
    assert_eq!(
        eq_panel::BAND_NUMBER.map(|n| format!("band{n}.on")),
        KEYS.map(|k| k.on.to_string())
    );
    // The constants the panel mirrors.
    assert_eq!(eq_panel::RANGE_DB, dev::DISPLAY_RANGE_DB);
    assert_eq!(eq_panel::SLOPE_ORDER, dev::SLOPE_ORDER);
    assert_eq!(eq_panel::STEREO_NAMES, dev::STEREO_MODE_NAMES);
}

#[test]
fn the_middle_curve_is_the_curve_of_the_device_for_every_shape() {
    for shape in 0..4 {
        for q in [0.1, 0.5, 0.707, 1.0, 4.0, 18.0, 40.0] {
            for gain in [-24.0, -6.0, 0.0, 3.0, 24.0] {
                for f in [20.0, 80.0, 300.0, 1000.0, 5000.0, 16_000.0] {
                    let want = dev::middle_response_db(shape, 700.0, q, gain, f, SR);
                    let got = eq_panel::middle_response_db(shape, 700.0, q, gain, f, SR);
                    assert!(
                        (want - got).abs() < 1e-9,
                        "shape {shape} q {q} g {gain} f {f}"
                    );
                }
            }
        }
        for g in [-5.0f32, 0.0, 5.0] {
            assert_eq!(
                dev::middle_active(shape, g),
                eq_panel::middle_active(shape, g)
            );
        }
        for q in [0.05, 0.3, 1.0, 5.0, 30.0] {
            assert_eq!(dev::effective_q(shape, q), eq_panel::effective_q(shape, q));
        }
    }
}

#[test]
fn the_cut_curve_is_the_curve_of_the_device_for_every_slope() {
    for low in [true, false] {
        for slope in 0..5 {
            for q in [0.3, 0.707, 1.5, 4.0] {
                for f in [20.0, 60.0, 100.0, 150.0, 1000.0, 9000.0, 20_000.0] {
                    let want = dev::cut_response_db(low, slope, 120.0, q, f, SR);
                    let got = eq_panel::cut_response_db(low, slope, 120.0, q, f, SR);
                    assert!(
                        (want - got).abs() < 1e-9,
                        "low {low} slope {slope} q {q} f {f}"
                    );
                }
            }
        }
    }
    // -3 dB at the corner of a default 12 dB/oct cut, whatever the slope's steepness at 6 dB/oct.
    let corner =
        eq_panel::cut_response_db(true, 0, 100.0, std::f64::consts::FRAC_1_SQRT_2, 100.0, SR);
    assert!((corner + 3.01).abs() < 0.01, "{corner}");
    let corner6 =
        eq_panel::cut_response_db(false, 3, 100.0, std::f64::consts::FRAC_1_SQRT_2, 100.0, SR);
    assert!((corner6 + 3.01).abs() < 0.05, "{corner6}");
}

#[test]
fn the_total_is_the_sum_of_the_active_bands_and_the_device_curves() {
    let (doc, _) = rack(FxKind::Eq);
    let bands = eq_panel::bands_of(desc(), doc.devices.first());
    for f in [30.0, 100.0, 500.0, 2500.0, 7000.0, 15_000.0] {
        let mut want = 0.0;
        for (i, b) in bands.iter().enumerate() {
            if !b.on {
                continue;
            }
            want += match i {
                0 => dev::cut_response_db(true, b.slope, b.freq, b.q, f, SR),
                6 => dev::cut_response_db(false, b.slope, b.freq, b.q, f, SR),
                _ if dev::middle_active(b.shape, b.gain as f32) => {
                    dev::middle_response_db(b.shape, b.freq, b.q, b.gain, f, SR)
                }
                _ => 0.0,
            };
        }
        let got = eq_panel::total_response_db(&bands, f, SR);
        assert!((want - got).abs() < 1e-9, "{f}: {want} {got}");
    }
    // The fixture: LCF on at 6 dB/oct, a notch, three more bands, HCF on.
    assert_eq!(bands.map(|b| b.on), [true; BANDS]);
    assert_eq!(bands[0].slope, 3);
    assert_eq!(bands[1].shape, 3);
    assert_eq!(bands[4].shape, 1);
}

#[test]
fn off_bands_and_flat_bells_add_nothing() {
    let (doc, _) = rack(FxKind::Eq);
    let mut bands = eq_panel::bands_of(desc(), doc.devices.first());
    for b in &mut bands {
        b.on = false;
    }
    assert_eq!(eq_panel::total_response_db(&bands, 1000.0, SR), 0.0);
    bands[2].on = true;
    bands[2].gain = 0.0;
    assert!(!eq_panel::active(2, &bands[2]), "a bell at 0 dB is off");
    assert_eq!(eq_panel::total_response_db(&bands, 500.0, SR), 0.0);
    bands[1].on = true;
    assert!(
        eq_panel::active(1, &bands[1]),
        "a notch is active at any gain"
    );
    assert!(eq_panel::total_response_db(&bands, 120.0, SR) < -6.0);
}

#[test]
fn the_defaults_of_an_unstored_device_read_as_the_descriptors() {
    let bands = eq_panel::bands_of(desc(), None);
    assert_eq!(
        bands.map(|b| b.on),
        [false, true, true, true, true, true, false]
    );
    assert_eq!(bands.map(|b| b.shape), [0, 1, 0, 0, 0, 2, 0]);
    assert!(!eq_panel::active(4, &bands[4]), "band 4 starts flat");
    assert_eq!(eq_panel::range_of(desc(), None), 12.0);
}

// ------------------------------------------------------------------ the layout

#[test]
fn the_panel_is_seven_units_and_places_the_back_grid() {
    let d = desc();
    let spec = fx_layout(FxKind::Eq, d);
    assert_eq!(spec.height_units, 7);
    let g = spec.fx.unwrap().eq.unwrap();
    // 19 CV jacks and 4 audio sockets, each once.
    assert_eq!(spec.back.len(), 23);
    assert!(g.back_params.iter().all(|p| *p != usize::MAX));
    assert_eq!(spec.front.iter().filter(|c| spec.on_back(c)).count(), 19);
    // The cuts have no gain cell: rows are FREQ, GAIN, Q, five middle bands have all three.
    let jacks_of = |k: &str| {
        spec.back
            .iter()
            .filter(|s| d.sockets[s.socket].key.contains(k))
            .count()
    };
    assert_eq!(jacks_of("_freq_cv_in"), 7);
    assert_eq!(jacks_of("_gain_cv_in"), 5);
    assert_eq!(jacks_of("_q_cv_in"), 7);
    // A jack sits in the column of its band and the row of its kind.
    for ps in &spec.back {
        let key = d.sockets[ps.socket].key;
        for (i, k) in KEYS.iter().enumerate() {
            for (row, name) in [(0, Some(k.freq_cv)), (1, k.gain_cv), (2, Some(k.q_cv))] {
                if name == Some(key) {
                    assert_eq!(ps.rect, eq_panel::back_cell(i, row), "{key}");
                }
            }
        }
    }
    // The strips run in the UI order: LCF has a slope control, band 4 (band7) a shape.
    let placed = |key: &str| {
        let p = d.params.iter().position(|p| p.key == key).unwrap();
        spec.front.iter().find(|c| c.param == p).unwrap().rect
    };
    let xs: Vec<f64> = KEYS.iter().map(|k| placed(k.freq).center().x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    assert_eq!(
        placed("lowcut.slope").center().x,
        placed("band1.freq").center().x
    );
    assert_eq!(
        placed("band7.shape").center().x,
        placed("band7.freq").center().x
    );
    // Everything fits the panel.
    let panel = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    for c in &spec.front {
        let r = c.rect;
        assert!(r.x0 >= panel.x0 && r.x1 <= panel.x1 && r.y0 >= 0.0 && r.y1 <= panel.y1);
    }
}

// ------------------------------------------------------------------ picking

fn plot() -> Rect {
    fx_layout(FxKind::Eq, desc()).fx.unwrap().plot
}

#[test]
fn a_handle_sits_where_the_band_is_and_picking_takes_the_nearest() {
    let (doc, _) = rack(FxKind::Eq);
    let bands = eq_panel::bands_of(desc(), doc.devices.first());
    let (plot, range) = (plot(), 12.0);
    for (i, b) in bands.iter().enumerate() {
        let at = eq_panel::handle_pos(plot, range, i, b);
        assert!((at.x - eq_panel::x_of(plot, b.freq)).abs() < 1e-9);
        assert_eq!(eq_panel::pick(plot, range, &bands, at), Some(i), "band {i}");
        // A little off still picks it; far away picks nothing.
        assert_eq!(
            eq_panel::pick(plot, range, &bands, at + Vec2::new(4.0, -4.0)),
            Some(i)
        );
    }
    assert_eq!(
        eq_panel::pick(
            plot,
            range,
            &bands,
            Point::new(plot.x0 + 1.0, plot.y0 + 1.0)
        ),
        None
    );
    // A notch sits on the 0 dB line, a middle band at its gain, a cut on its own curve.
    let y0 = eq_panel::y_of(plot, range, 0.0);
    assert_eq!(eq_panel::handle_pos(plot, range, 1, &bands[1]).y, y0);
    let y3 = eq_panel::y_of(plot, range, bands[3].gain);
    assert_eq!(eq_panel::handle_pos(plot, range, 3, &bands[3]).y, y3);
    let corner = eq_panel::band_response_db(0, &bands[0], bands[0].freq, eq_panel::DRAW_RATE);
    assert_eq!(
        eq_panel::handle_pos(plot, range, 0, &bands[0]).y,
        eq_panel::y_of(plot, range, corner)
    );
    // The mappings invert.
    for f in [20.0, 440.0, 20_000.0] {
        let x = eq_panel::x_of(plot, f);
        assert!((eq_panel::freq_of_x(plot, x) - f).abs() / f < 1e-9);
    }
    for db in [-12.0, -3.5, 0.0, 7.0, 12.0] {
        let y = eq_panel::y_of(plot, range, db);
        assert!((eq_panel::gain_of_y(plot, range, y) - db).abs() < 1e-9);
    }
}

#[test]
fn the_nearest_free_band_is_the_closest_one_that_is_not_in_use() {
    let flat = |freq: f64, on: bool, gain: f64| Band {
        on,
        freq,
        gain,
        q: 1.0,
        shape: 0,
        slope: 0,
    };
    let mut bands = [
        flat(80.0, true, 0.0),
        flat(120.0, true, 5.0),
        flat(500.0, false, 0.0),
        flat(2500.0, true, 0.0),
        flat(6000.0, true, 3.0),
        flat(8000.0, true, 3.0),
        flat(16_000.0, false, 0.0),
    ];
    // Band 2 (off) and band 3 (a flat bell) are free; a cut never is.
    assert_eq!(eq_panel::nearest_free_band(&bands, 400.0), Some(2));
    assert_eq!(eq_panel::nearest_free_band(&bands, 3000.0), Some(3));
    assert_eq!(eq_panel::nearest_free_band(&bands, 20.0), Some(2));
    bands[2].on = true;
    bands[2].gain = 1.0;
    bands[3].gain = 1.0;
    assert_eq!(eq_panel::nearest_free_band(&bands, 400.0), None);
}

#[test]
fn the_slope_click_goes_up_the_steepness() {
    // Stored 0 12, 1 24, 2 48, 3 6, 4 36: the click runs 6, 12, 24, 36, 48.
    let mut v = 3.0;
    let mut seen = vec![v];
    for _ in 0..5 {
        v = eq_panel::cycle_slope(v, true);
        seen.push(v);
    }
    assert_eq!(seen, vec![3.0, 0.0, 1.0, 4.0, 2.0, 3.0]);
    assert_eq!(eq_panel::cycle_slope(3.0, false), 2.0);
}

// ------------------------------------------------------------------ gestures

fn rig_with(extra: &[(&str, f32)]) -> (Document, RackView) {
    let mut doc = fx_doc(FxKind::Eq);
    for (k, v) in extra {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let mut v = RackView::new(Theme::dark());
    let reg = registry();
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 900.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (Document::new(doc), v)
}

fn rig() -> (Document, RackView) {
    rig_with(&[])
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

fn send(document: &mut Document, v: &mut RackView, e: &UiEvent) -> Vec<UiAction> {
    let a = v.handle(e, document.doc(), &registry());
    apply(document, v, &a);
    a
}

fn drag_with(
    document: &mut Document,
    v: &mut RackView,
    from: Point,
    path: &[Point],
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut all = send(
        document,
        v,
        &UiEvent::PointerDown {
            pos: from,
            button: PointerButton::Primary,
            modifiers,
            clicks: 1,
        },
    );
    for p in path {
        all.extend(send(document, v, &mv(*p)));
    }
    let end = path.last().copied().unwrap_or(from);
    all.extend(send(document, v, &up(end)));
    all
}

fn drag(document: &mut Document, v: &mut RackView, from: Point, path: &[Point]) -> Vec<UiAction> {
    drag_with(document, v, from, path, Modifiers::NONE)
}

/// Every `SetParam` among the actions, a `Batch` flattened.
fn sets(actions: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    fn walk(c: &Command, out: &mut Vec<(String, f32, Option<u64>)>) {
        match c {
            Command::SetParam {
                param,
                value,
                merge,
                ..
            } => out.push((param.clone(), *value, *merge)),
            Command::Batch(cs) => cs.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in commands(actions) {
        walk(&c, &mut out);
    }
    out
}

fn handle(v: &RackView, band: usize) -> Point {
    v.eq_handle_center(&key("fx"), band).unwrap()
}

#[test]
fn dragging_a_handle_sets_frequency_and_gain_together_in_one_undo_step() {
    let (mut document, mut v) = rig();
    // Band 2 (UI index 2, band3): a bell at 500 Hz, -7 dB.
    let h = handle(&v, 2);
    let (f0, g0) = (
        param(&document, "band3.freq"),
        param(&document, "band3.gain"),
    );
    let path: Vec<Point> = (1..=8)
        .map(|i| Point::new(h.x + 5.0 * f64::from(i), h.y - 2.0 * f64::from(i)))
        .collect();
    let a = drag(&mut document, &mut v, h, &path);
    let s = sets(&a);
    assert!(s.len() >= 10, "{s:?}");
    assert!(s.iter().any(|(p, _, _)| p == "band3.freq"));
    assert!(s.iter().any(|(p, _, _)| p == "band3.gain"));
    assert!(
        s.iter()
            .all(|(p, _, m)| (p == "band3.freq" || p == "band3.gain") && m.is_some())
    );
    let id = s[0].2;
    assert!(s.iter().all(|(_, _, m)| *m == id), "one merge id");
    assert!(matches!(a.last(), Some(UiAction::EndMerge)));
    // Every step moved both as one command.
    let batches = commands(&a)
        .into_iter()
        .filter(|c| matches!(c, Command::Batch(b) if b.len() == 2))
        .count();
    assert!(batches >= 5, "{batches}");
    let (f1, g1) = (
        param(&document, "band3.freq"),
        param(&document, "band3.gain"),
    );
    assert!(f1 > f0 * 1.3 && g1 > g0 + 1.5, "{f0}->{f1} {g0}->{g1}");
    // The handle followed the pointer.
    let moved = handle(&v, 2);
    assert!((moved.x - (h.x + 40.0)).abs() < 3.0, "{moved:?}");
    // One undo step puts both back.
    document.undo();
    assert!((param(&document, "band3.freq") - f0).abs() < 1e-3);
    assert!((param(&document, "band3.gain") - g0).abs() < 1e-3);
}

#[test]
fn a_drag_is_clamped_to_the_param_ranges() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y - 900.0)]);
    assert_eq!(
        param(&document, "band4.gain"),
        12.0,
        "the top of a +-12 dB graph"
    );
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y + 900.0)]);
    assert_eq!(param(&document, "band4.gain"), -12.0);
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x - 900.0, h.y)]);
    assert_eq!(param(&document, "band4.freq"), 20.0);
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x + 900.0, h.y)]);
    assert_eq!(param(&document, "band4.freq"), 20_000.0);
    // The shelf of band 1 stops at 5 kHz.
    let h = handle(&v, 1);
    drag(&mut document, &mut v, h, &[Point::new(h.x + 900.0, h.y)]);
    assert_eq!(param(&document, "band2.freq"), 5000.0);
}

#[test]
fn the_range_of_the_graph_scales_the_gain() {
    let (mut document, mut v) = rig_with(&[("display.range", 2.0)]);
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y - 900.0)]);
    assert_eq!(param(&document, "band4.gain"), 24.0, "+-24 dB");
    let (mut document, mut v) = rig_with(&[("display.range", 0.0), ("band4.gain", 3.0)]);
    let h = handle(&v, 3);
    drag(&mut document, &mut v, h, &[Point::new(h.x, h.y - 900.0)]);
    assert_eq!(param(&document, "band4.gain"), 6.0, "+-6 dB");
}

#[test]
fn a_cut_and_a_notch_only_move_the_frequency() {
    let (mut document, mut v) = rig();
    // The low cut (index 0) and the notch of band 1 (index 1, `band2.shape` = 3).
    for (band, key_freq) in [(0, "band1.freq"), (1, "band2.freq")] {
        let h = handle(&v, band);
        let a = drag(
            &mut document,
            &mut v,
            h,
            &[
                Point::new(h.x + 4.0, h.y - 40.0),
                Point::new(h.x + 9.0, h.y - 60.0),
            ],
        );
        let s = sets(&a);
        assert!(!s.is_empty());
        assert!(
            s.iter().all(|(p, _, m)| p == key_freq && m.is_some()),
            "{s:?}"
        );
        assert_eq!(
            param(&document, "band2.gain"),
            5.0,
            "the notch keeps its gain"
        );
    }
}

#[test]
fn alt_drag_changes_the_q_and_the_resonance_in_one_undo_step() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    let q0 = param(&document, "band3.q");
    let a = drag_with(
        &mut document,
        &mut v,
        h,
        &[Point::new(h.x, h.y - 20.0), Point::new(h.x, h.y - 40.0)],
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    let s = sets(&a);
    assert!(
        s.iter().all(|(p, _, m)| p == "band3.q" && m.is_some()),
        "{s:?}"
    );
    let id = s[0].2;
    assert!(s.iter().all(|(_, _, m)| *m == id));
    assert!(param(&document, "band3.q") > q0 * 1.5);
    assert_eq!(param(&document, "band3.gain"), -7.0, "the gain stays");
    document.undo();
    assert!((param(&document, "band3.q") - q0).abs() < 1e-3);
    // The resonance of the high cut.
    let h = handle(&v, 6);
    let r0 = param(&document, "band6.q");
    let a = drag_with(
        &mut document,
        &mut v,
        h,
        &[Point::new(h.x, h.y - 30.0)],
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    assert!(sets(&a).iter().all(|(p, _, _)| p == "band6.q"));
    assert!(param(&document, "band6.q") > r0);
}

fn wheel(document: &mut Document, v: &mut RackView, at: Point, dy: f64) -> Vec<UiAction> {
    send(
        document,
        v,
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos: at,
            modifiers: Modifiers::NONE,
        },
    )
}

#[test]
fn the_wheel_over_a_handle_steps_its_q() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    let q0 = param(&document, "band3.q");
    let a = wheel(&mut document, &mut v, h, 1.0);
    let s = sets(&a);
    assert_eq!(s.len(), 1, "{a:?}");
    assert_eq!(s[0].0, "band3.q");
    assert_eq!(s[0].2, None, "a wheel step is its own undo step");
    assert!(s[0].1 > q0);
    let a = wheel(&mut document, &mut v, h, -1.0);
    assert!(sets(&a)[0].1 < s[0].1);
    // The resonance of the low cut.
    let h = handle(&v, 0);
    let a = wheel(&mut document, &mut v, h, 1.0);
    assert_eq!(sets(&a)[0].0, "band1.q");
}

#[test]
fn a_double_click_on_a_handle_resets_the_band() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    let a = send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Primary, 2),
    );
    let s = sets(&a);
    let keys: Vec<&str> = s.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(keys, ["band3.freq", "band3.gain", "band3.q"]);
    assert!(s.iter().all(|(_, _, m)| m.is_none()));
    send(&mut document, &mut v, &up(h));
    assert_eq!(param(&document, "band3.freq"), 500.0);
    assert_eq!(param(&document, "band3.gain"), 0.0);
    assert_eq!(param(&document, "band3.q"), 1.0);
    assert_eq!(param(&document, "band3.on"), 1.0, "the band stays on");
}

#[test]
fn a_double_click_on_the_empty_graph_switches_on_the_nearest_free_band() {
    // Band 4 (UI index 4, band7) is flat: the free one. Bands 1..3 and 5 are in use.
    let (mut document, mut v) = rig_with(&[("band7.gain", 0.0), ("band7.on", 0.0)]);
    let plot = v.eq_plot_rect(&key("fx")).unwrap();
    // Near 3 kHz, 6 dB up.
    let f = 3000.0;
    let x = plot.x0 + eq_panel::x_of(Rect::new(0.0, 0.0, plot.width(), 1.0), f);
    let y = plot.y0 + (plot.height() * 0.5) - plot.height() * (6.0 / 24.0);
    let p = Point::new(x, y);
    let a = send(
        &mut document,
        &mut v,
        &down_with(p, PointerButton::Primary, 2),
    );
    let s = sets(&a);
    let get = |k: &str| s.iter().find(|(p, _, _)| p == k).map(|(_, v, _)| *v);
    assert_eq!(get("band7.on"), Some(1.0), "{s:?}");
    assert!((get("band7.freq").unwrap() - 3000.0).abs() < 60.0, "{s:?}");
    assert!((get("band7.gain").unwrap() - 6.0).abs() < 0.6, "{s:?}");
    send(&mut document, &mut v, &up(p));
    assert_eq!(param(&document, "band7.on"), 1.0);
    // With no free band the double-click does nothing but select.
    let (mut document, mut v) = rig();
    let plot = v.eq_plot_rect(&key("fx")).unwrap();
    let p = Point::new(plot.x0 + 3.0, plot.y0 + 3.0);
    let a = send(
        &mut document,
        &mut v,
        &down_with(p, PointerButton::Primary, 2),
    );
    assert!(sets(&a).is_empty(), "{a:?}");
    assert!(a.contains(&UiAction::Select(key("fx"))));
}

#[test]
fn a_single_press_on_the_empty_graph_selects_the_device() {
    let (mut document, mut v) = rig();
    let plot = v.eq_plot_rect(&key("fx")).unwrap();
    let p = Point::new(plot.x0 + 3.0, plot.y0 + 3.0);
    let a = send(&mut document, &mut v, &down(p));
    assert!(a.contains(&UiAction::Select(key("fx"))), "{a:?}");
    assert!(commands(&a).is_empty());
}

#[test]
fn the_right_click_menu_of_a_middle_band_picks_the_shape() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    let a = send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    assert!(commands(&a).is_empty());
    assert_eq!(
        v.eq_menu_labels(),
        [
            "Bell",
            "Low shelf",
            "High shelf",
            "Notch",
            "Band on",
            "Reset band"
        ]
    );
    // Choose Low shelf.
    let row = v.eq_menu_row_center(1).unwrap();
    let a = send(&mut document, &mut v, &down(row));
    assert_eq!(sets(&a), vec![("band3.shape".to_string(), 1.0, None)]);
    assert_eq!(param(&document, "band3.shape"), 1.0);
    assert!(v.eq_menu_labels().is_empty(), "the menu closes");
    send(&mut document, &mut v, &up(row));
}

#[test]
fn the_right_click_menu_of_a_cut_picks_the_slope_and_switches_the_band() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 6);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    assert_eq!(
        v.eq_menu_labels(),
        [
            "6 dB/oct",
            "12 dB/oct",
            "24 dB/oct",
            "36 dB/oct",
            "48 dB/oct",
            "Band on",
            "Reset band"
        ]
    );
    // 48 dB/oct is the fifth row and the stored value 2.
    let row = v.eq_menu_row_center(4).unwrap();
    let a = send(&mut document, &mut v, &down(row));
    assert_eq!(sets(&a), vec![("highcut.slope".to_string(), 2.0, None)]);
    send(&mut document, &mut v, &up(row));
    // On/Off: the band is on, the row switches it off.
    let h = handle(&v, 6);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    let row = v.eq_menu_row_center(5).unwrap();
    let a = send(&mut document, &mut v, &down(row));
    assert_eq!(sets(&a), vec![("band6.on".to_string(), 0.0, None)]);
    send(&mut document, &mut v, &up(row));
    assert_eq!(param(&document, "band6.on"), 0.0);
}

#[test]
fn reset_band_in_the_menu_puts_back_frequency_gain_q_and_shape() {
    let (mut document, mut v) = rig_with(&[("band4.shape", 2.0), ("band4.q", 7.0)]);
    let h = handle(&v, 3);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    let row = v.eq_menu_row_center(5).unwrap();
    send(&mut document, &mut v, &down(row));
    send(&mut document, &mut v, &up(row));
    assert_eq!(param(&document, "band4.freq"), 2500.0);
    assert_eq!(param(&document, "band4.gain"), 0.0);
    assert_eq!(param(&document, "band4.q"), 1.0);
    assert_eq!(param(&document, "band4.shape"), 0.0);
}

#[test]
fn a_press_elsewhere_closes_the_menu_and_escape_cancels_a_drag() {
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    assert!(!v.eq_menu_labels().is_empty());
    let away = Point::new(h.x + 200.0, h.y + 40.0);
    let a = send(&mut document, &mut v, &down(away));
    assert!(v.eq_menu_labels().is_empty());
    assert!(commands(&a).is_empty());
    send(&mut document, &mut v, &up(away));
    // Escape restores the values of the press in the same undo step.
    let h = handle(&v, 2);
    let (f0, g0) = (
        param(&document, "band3.freq"),
        param(&document, "band3.gain"),
    );
    send(&mut document, &mut v, &down(h));
    for i in 1..6 {
        send(
            &mut document,
            &mut v,
            &mv(Point::new(
                h.x + 10.0 * f64::from(i),
                h.y - 6.0 * f64::from(i),
            )),
        );
    }
    assert_ne!(param(&document, "band3.freq"), f0);
    let a = send(
        &mut document,
        &mut v,
        &support::song::press(atomik_ui::Key::Escape, Modifiers::NONE),
    );
    assert!((param(&document, "band3.freq") - f0).abs() < 1e-3);
    assert!((param(&document, "band3.gain") - g0).abs() < 1e-3);
    assert!(a.contains(&UiAction::EndMerge));
}

// ------------------------------------------------------------------ the controls

fn click(document: &mut Document, v: &mut RackView, at: Point) -> Vec<UiAction> {
    let mut a = send(document, v, &down(at));
    a.extend(send(document, v, &up(at)));
    a
}

#[test]
fn the_stereo_mode_the_range_and_the_spectrum_switch_set_their_params() {
    let (mut document, mut v) = rig();
    let at = v.choice_center(&key("fx"), "stereo.mode").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("stereo.mode".to_string(), 2.0, None)]);
    let a = send(
        &mut document,
        &mut v,
        &down_with(at, PointerButton::Secondary, 1),
    );
    assert_eq!(sets(&a), vec![("stereo.mode".to_string(), 1.0, None)]);
    send(&mut document, &mut v, &up(at));
    let at = v.choice_center(&key("fx"), "display.range").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("display.range".to_string(), 2.0, None)]);
    let at = v.control_center(&key("fx"), "spectrum.on").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("spectrum.on".to_string(), 0.0, None)]);
    assert_eq!(param(&document, "spectrum.on"), 0.0);
}

#[test]
fn the_strip_controls_set_the_band_params() {
    let (mut document, mut v) = rig();
    // The shape of band 4 (band7): Low shelf -> High shelf.
    let at = v.choice_center(&key("fx"), "band7.shape").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("band7.shape".to_string(), 2.0, None)]);
    // The slope of the low cut runs up the steepness: 6 -> 12 dB/oct (stored 3 -> 0).
    let at = v.choice_center(&key("fx"), "lowcut.slope").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("lowcut.slope".to_string(), 0.0, None)]);
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("lowcut.slope".to_string(), 1.0, None)]);
    // The ON switch of the high cut.
    let at = v.control_center(&key("fx"), "band6.on").unwrap();
    let a = click(&mut document, &mut v, at);
    assert_eq!(sets(&a), vec![("band6.on".to_string(), 0.0, None)]);
    // The output knob answers a wheel notch.
    let at = v.control_center(&key("fx"), "output").unwrap();
    let a = wheel(&mut document, &mut v, at, 1.0);
    assert_eq!(sets(&a)[0].0, "output");
}

#[test]
fn the_cv_trims_are_on_the_back_only() {
    let (_, mut v) = rig();
    assert!(v.control_center(&key("fx"), "band3.freq_cv_trim").is_none());
    assert!(v.control_center(&key("fx"), "band3.freq").is_some());
    v.set_flip(Flip::Back);
    assert!(v.control_center(&key("fx"), "band3.freq_cv_trim").is_some());
    assert!(v.control_center(&key("fx"), "band3.freq").is_none());
    assert!(
        v.control_center(&key("fx"), "band1.gain_cv_trim").is_none(),
        "no gain trim on a cut"
    );
    // The jacks answer for cables.
    for s in [
        "band1_freq_cv_in",
        "band7_gain_cv_in",
        "band6_q_cv_in",
        "in_l",
        "out_r",
    ] {
        let at = atomik_model::SocketRef {
            device: key("fx"),
            socket: s.to_string(),
        };
        assert!(v.socket_center(&at).is_some(), "{s}");
    }
}

// ------------------------------------------------------------------ painting

fn texts_of(v: &mut RackView, doc: &Document) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_graph_is_labelled_and_shows_the_stereo_mode() {
    let (document, mut v) = rig_with(&[("display.range", 2.0)]);
    let t = texts_of(&mut v, &document);
    for want in [
        "RESPONSE", "20", "100", "1k", "20k", "+24", "+18", "-24", "0", "LCF", "HCF", "MID",
        "Output", "Spectrum", "Stereo", "Range",
    ] {
        assert!(t.iter().any(|x| x == want), "{want}: {t:?}");
    }
    let (document, mut v) = rig_with(&[("stereo.mode", 0.0), ("display.range", 0.0)]);
    let t = texts_of(&mut v, &document);
    assert!(!t.iter().any(|x| x == "MID"), "Stereo mode has no tag");
    assert!(
        t.iter().any(|x| x == "+2") && t.iter().any(|x| x == "+6"),
        "{t:?}"
    );
}

fn json_with(extra: &[(&str, f32)], fed: bool) -> String {
    let (document, mut v) = rig_with(extra);
    if fed {
        v.set_analyser(&key("fx"), &spectrum_data(false));
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &registry());
    dl.to_json()
}

#[test]
fn the_spectrum_is_drawn_behind_the_curve_only_while_it_is_switched_on() {
    let none = json_with(&[("spectrum.on", 0.0)], false);
    let none_on = json_with(&[], false);
    let shown = json_with(&[], true);
    let hidden = json_with(&[("spectrum.on", 0.0)], true);
    assert_ne!(none_on, shown, "the trace is drawn");
    assert_eq!(none, hidden, "spectrum.on 0 draws nothing of it");
    assert!(!shown.contains("NaN") && !shown.contains("inf"));
}

#[test]
fn the_dynamic_eq_keeps_its_own_panel() {
    // Not the EQ's layout: its sections and height are its own (see dynamic_eq_panel).
    let spec = fx_layout(
        FxKind::DynamicEq,
        registry().get("atomik.dynamic-eq").unwrap().descriptor,
    );
    assert_eq!(spec.height_units, 9);
    assert!(spec.fx.unwrap().eq.is_none());
}

// ------------------------------------------------------------------ review fixes

#[test]
fn escape_during_a_drag_leaves_no_extra_undo_step() {
    // A bell, a shelf (gain and frequency), and a cut or notch (frequency only).
    for band in [2usize, 6, 0, 1] {
        let (mut document, mut v) = rig();
        let k = &KEYS[band];
        let names: Vec<&str> = [Some(k.freq), k.gain].into_iter().flatten().collect();
        let before: Vec<f32> = names.iter().map(|n| param(&document, n)).collect();
        let depth = document.undo_depth();
        let h = handle(&v, band);
        send(&mut document, &mut v, &down(h));
        for i in 1..6 {
            send(
                &mut document,
                &mut v,
                &mv(Point::new(h.x + 10.0 * f64::from(i), h.y)),
            );
        }
        assert_eq!(document.undo_depth(), depth + 1, "band {band}: one step");
        send(
            &mut document,
            &mut v,
            &support::song::press(atomik_ui::Key::Escape, Modifiers::NONE),
        );
        for (n, b) in names.iter().zip(&before) {
            assert!((param(&document, n) - b).abs() < 1e-3, "band {band} {n}");
        }
        assert_eq!(
            document.undo_depth(),
            depth + 1,
            "band {band}: the restore merged into the drag's step"
        );
        document.undo();
        assert_eq!(document.undo_depth(), depth, "band {band}");
    }
}

#[test]
fn alt_drag_escape_leaves_no_extra_undo_step() {
    let (mut document, mut v) = rig();
    let q0 = param(&document, "band3.q");
    let depth = document.undo_depth();
    let h = handle(&v, 2);
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    send(
        &mut document,
        &mut v,
        &UiEvent::PointerDown {
            pos: h,
            button: PointerButton::Primary,
            modifiers: alt,
            clicks: 1,
        },
    );
    for i in 1..6 {
        send(
            &mut document,
            &mut v,
            &mv(Point::new(h.x, h.y - 8.0 * f64::from(i))),
        );
    }
    assert_ne!(param(&document, "band3.q"), q0);
    send(
        &mut document,
        &mut v,
        &support::song::press(atomik_ui::Key::Escape, Modifiers::NONE),
    );
    assert!((param(&document, "band3.q") - q0).abs() < 1e-3);
    assert_eq!(document.undo_depth(), depth + 1);
}

#[test]
fn a_handle_menu_never_applies_to_another_device_after_the_list_changed() {
    let reg = registry();
    // The device list changes under the open menu: another device takes the index.
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    let row = v.eq_menu_row_center(1).unwrap();
    let mut doc = document.doc().clone();
    doc.devices.remove(0);
    doc.devices.insert(0, device("other", "atomik.eq", "Other"));
    v.sync(&doc, &reg);
    let a = v.handle(&down(row), &doc, &reg);
    assert!(
        sets(&a).is_empty(),
        "the device of the menu is gone: {:?}",
        commands(&a)
    );
    v.handle(&up(row), &doc, &reg);

    // Another device in front of it: the row still goes to the device of the menu.
    let (mut document, mut v) = rig();
    let h = handle(&v, 2);
    send(
        &mut document,
        &mut v,
        &down_with(h, PointerButton::Secondary, 1),
    );
    let row = v.eq_menu_row_center(1).unwrap();
    let mut doc = document.doc().clone();
    doc.devices.insert(0, device("other", "atomik.eq", "Other"));
    v.sync(&doc, &reg);
    let a = v.handle(&down(row), &doc, &reg);
    let keys: Vec<_> = commands(&a)
        .iter()
        .flat_map(|c| match c {
            Command::SetParam { key, .. } => vec![key.clone()],
            Command::Batch(cs) => cs
                .iter()
                .filter_map(|c| match c {
                    Command::SetParam { key, .. } => Some(key.clone()),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        })
        .collect();
    assert!(!keys.is_empty());
    assert!(keys.iter().all(|k| *k == key("fx")), "{keys:?}");
}
