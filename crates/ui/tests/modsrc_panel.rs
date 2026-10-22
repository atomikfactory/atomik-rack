//! The Mod Source rack panel: layout, display-list goldens of both faces and of a live frame,
//! the generic controls, the shape curves against the DSP, and the scope ring buffer fed with
//! synthetic meter values.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test modsrc_panel`.

mod support;

use atomik_dsp::mod_source::{ModShape, mod_shape};
use atomik_model::RackDoc;
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::kurbo::Rect;
use atomik_ui::modsrc_panel::{
    COUNTER_MODULO, HISTORY, METER_COUNT, SCOPE_CAPACITY, Scope, modsrc_label, modsrc_layout,
    modsrc_socket_label, shape_value,
};
use atomik_ui::{DisplayList, Flip, RackView};
use std::path::PathBuf;
use support::seq::*;
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

fn setup() -> (RackDoc, RackView) {
    let doc = modsrc_only();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

/// A synthetic meter frame: counter `c`, LFO 1 a ramp, LFO 2 its negative, env a scaled ramp.
fn frame(c: u32, base: f32) -> Vec<f32> {
    let mut v = vec![0.0f32; METER_COUNT];
    v[0] = 0.25;
    v[1] = -0.25;
    v[2] = 0.5;
    v[3] = 0.25;
    v[4] = 0.75;
    v[5] = 3.0;
    v[6] = c as f32;
    for i in 0..HISTORY {
        let x = base + i as f32 * 0.01;
        v[7 + i] = x;
        v[15 + i] = -x;
        v[23 + i] = 0.5 + x * 0.25;
    }
    v[31] = 2.5;
    v
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("modsrc_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup();
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("modsrc_panel_back_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_with_a_live_scope_matches_golden() {
    let (doc, mut v) = setup();
    let d = key("mod");
    for c in 0..80u32 {
        let base = (c as f32 * 0.05).sin() * 0.5;
        v.set_device_meter_values(&d, &frame((c + 1) * HISTORY as u32 % COUNTER_MODULO, base));
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("modsrc_panel_live_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ layout

fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

#[test]
fn the_layout_places_every_param_and_socket_once_without_overlap() {
    let reg = registry();
    let desc = reg.get(MODSRC_TYPE).unwrap().descriptor;
    let spec = modsrc_layout(desc);
    assert_eq!(spec.height_units, 4);
    let mut params: Vec<usize> = spec.front.iter().map(|c| c.param).collect();
    params.sort_unstable();
    assert_eq!(params, (0..desc.params.len()).collect::<Vec<_>>());
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    let panel = spec.modsrc.expect("panel");
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    let mut rects: Vec<Rect> = panel.shapes.to_vec();
    rects.push(panel.scope);
    rects.extend(spec.front.iter().map(|c| c.rect));
    for (i, a) in rects.iter().enumerate() {
        assert!(
            a.x0 >= content.x0 - 1e-9
                && a.x1 <= content.x1 + 1e-9
                && a.y0 >= 0.0
                && a.y1 <= content.y1,
            "{a:?} outside {content:?}"
        );
        for b in &rects[i + 1..] {
            assert!(!overlap(a, b), "{a:?} overlaps {b:?}");
        }
    }
    let backs: Vec<Rect> = spec.back.iter().map(|s| s.rect).collect();
    for (i, a) in backs.iter().enumerate() {
        assert!(a.x1 <= content.x1 && a.y1 <= content.y1, "{a:?}");
        for b in &backs[i + 1..] {
            assert!(!overlap(a, b), "{a:?} overlaps {b:?}");
        }
    }
    assert!(spec.arp.is_none() && spec.seq.is_none() && spec.fx.is_none());
}

#[test]
fn every_control_and_socket_has_a_label() {
    let reg = registry();
    let desc = reg.get(MODSRC_TYPE).unwrap().descriptor;
    for p in desc.params {
        assert!(modsrc_label(p.key).is_some(), "{}", p.key);
    }
    for s in desc.sockets {
        assert!(modsrc_socket_label(s.key).is_some(), "{}", s.key);
    }
    assert_eq!(modsrc_label("filter.cutoff"), None);
}

#[test]
fn the_panel_shows_its_sections_labels_and_the_back_sockets() {
    let (doc, mut v) = setup();
    let t = texts_of(&mut v, &doc);
    for s in [
        "LFO 1", "LFO 2", "ENVELOPE", "SHAPES", "SCOPE", "Shape", "Morph", "Rate", "Division",
        "Key sync", "Rate CV", "Attack", "Release", "Mode",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    v.set_flip(Flip::Back);
    let t = texts_of(&mut v, &doc);
    for s in [
        "Notes in",
        "Gate in",
        "Rate CV",
        "Amount CV",
        "CV out",
        "ENVELOPE",
    ] {
        assert!(t.iter().any(|x| x == s), "back missing {s:?}: {t:?}");
    }
    let r = v.device_rect(&key("mod")).unwrap();
    assert!((r.height() - 4.0 * 76.0).abs() < 1e-9, "{r:?}");
}

#[test]
fn the_controls_are_on_the_panel() {
    let (_, v) = setup();
    for k in ["lfo1_shape", "lfo2_rate_cv", "env_attack", "env_mode"] {
        assert!(v.control_center(&key("mod"), k).is_some(), "{k}");
    }
}

// ------------------------------------------------------------------ shapes

#[test]
fn the_display_curves_match_the_dsp() {
    let rand = [-0.45f32, 0.6, -0.35];
    let randd = rand.map(f64::from);
    for shape in 0..7usize {
        for m in [-1.0f32, -0.4, 0.0, 0.5, 1.0] {
            for i in 0..200 {
                let p = f64::from(i) / 200.0;
                let want = mod_shape(ModShape::from_index(shape), p, m, rand);
                let got = shape_value(shape, p, f64::from(m), randd);
                assert!(
                    (f64::from(want) - got).abs() < 1e-5,
                    "shape {shape} morph {m} p {p}: {want} vs {got}"
                );
            }
        }
    }
}

// ------------------------------------------------------------------ scope

#[test]
fn the_scope_is_empty_until_fed_and_ignores_short_or_bad_frames() {
    let mut s = Scope::new();
    assert!(s.is_empty() && s.point(0, 0).is_none());
    assert!(!s.feed(&[0.0; 5]));
    let mut bad = frame(1, 0.0);
    bad[7] = f32::NAN;
    assert!(!s.feed(&bad));
    assert!(s.is_empty());
}

#[test]
fn the_first_frame_takes_all_eight_points_oldest_first() {
    let mut s = Scope::new();
    assert!(s.feed(&frame(8, 0.1)));
    assert_eq!(s.len(), HISTORY);
    assert!((s.point(0, 0).unwrap() - 0.17).abs() < 1e-6);
    assert!((s.point(0, 7).unwrap() - 0.10).abs() < 1e-6);
    assert!((s.point(1, 0).unwrap() + 0.17).abs() < 1e-6);
    assert!((s.point(2, 0).unwrap() - (0.5 + 0.17 * 0.25)).abs() < 1e-6);
    assert!(s.point(0, 8).is_none() && s.point(3, 0).is_none());
    assert!((s.spacing_ms() - 2.5).abs() < 1e-6);
    assert_eq!(s.phases(), [0.25, 0.75]);
    assert_eq!(s.stage(), 3);
}

#[test]
fn only_the_new_points_are_appended() {
    let mut s = Scope::new();
    s.feed(&frame(8, 0.0));
    // Two pushes since: the last two history points are new.
    assert!(s.feed(&frame(10, 0.5)));
    assert_eq!(s.len(), HISTORY + 2);
    assert!((s.point(0, 0).unwrap() - 0.57).abs() < 1e-6);
    assert!((s.point(0, 1).unwrap() - 0.56).abs() < 1e-6);
    assert!(
        (s.point(0, 2).unwrap() - 0.07).abs() < 1e-6,
        "the previous newest"
    );
    // The same counter again: nothing changed.
    assert!(!s.feed(&frame(10, 0.5)));
    assert_eq!(s.len(), HISTORY + 2);
    // Only the marker moved: a change, but no new points.
    let mut f = frame(10, 0.5);
    f[3] = 0.5;
    assert!(s.feed(&f));
    assert_eq!(s.len(), HISTORY + 2);
}

#[test]
fn a_long_gap_takes_the_whole_frame_and_the_counter_wraps() {
    let mut s = Scope::new();
    s.feed(&frame(100, 0.0));
    s.feed(&frame(130, 0.5));
    assert_eq!(s.len(), 2 * HISTORY);
    // Wrap: 4094 to 3 is five pushes.
    let mut s = Scope::new();
    s.feed(&frame(COUNTER_MODULO - 2, 0.0));
    assert!(s.feed(&frame(3, 0.5)));
    assert_eq!(s.len(), HISTORY + 5);
}

#[test]
fn the_ring_keeps_the_newest_points_and_never_grows() {
    let mut s = Scope::new();
    let mut c = 0u32;
    for _ in 0..(SCOPE_CAPACITY / HISTORY + 20) {
        c = (c + HISTORY as u32) % COUNTER_MODULO;
        s.feed(&frame(c, 0.0));
    }
    assert_eq!(s.len(), SCOPE_CAPACITY);
    assert!(s.point(0, SCOPE_CAPACITY - 1).is_some());
    assert!(s.point(0, SCOPE_CAPACITY).is_none());
}

#[test]
fn feeding_the_view_keeps_the_scope_across_a_resync() {
    let (doc, mut v) = setup();
    let reg = registry();
    let d = key("mod");
    assert!(v.modsrc_scope(&d).is_some_and(Scope::is_empty));
    v.set_device_meter_values(&d, &frame(8, 0.0));
    assert_eq!(v.modsrc_scope(&d).unwrap().len(), HISTORY);
    v.sync(&doc, &reg);
    assert_eq!(
        v.modsrc_scope(&d).unwrap().len(),
        HISTORY,
        "kept by a resync"
    );
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x.ends_with(" s")), "the time span: {t:?}");
    assert!(!t.iter().any(|x| x == "waiting for audio"));
    assert!(v.modsrc_scope(&key("nope")).is_none());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::kurbo::Size;
    use atomik_ui::{TextEngine, Theme, UiEvent};
    use support::cpu::CpuCanvas;
    let reg = registry();
    let doc = modsrc_only();
    let mut text = TextEngine::new();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 500.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(1.5), &doc, &reg);
    for c in 0..80u32 {
        let base = (c as f32 * 0.05).sin() * 0.5;
        rack.set_device_meter_values(&key("mod"), &frame((c + 1) * 8 % 4096, base));
    }
    for (name, flip) in [
        ("modsrc_panel", Flip::Front),
        ("modsrc_panel_back", Flip::Back),
    ] {
        rack.set_flip(flip);
        let mut canvas = CpuCanvas::new(1230, 750, &mut text);
        rack.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
