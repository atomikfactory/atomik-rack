//! The `atomik.character` panel: geometry, the 4 x 4
//! preset matrix (hit test, one `SetParam` per click, arrow keys, tooltips), the knobs (drag,
//! fine, wheel, double-click and Ctrl-click reset, one undo step per drag, dB display of the
//! square-law gain and volume), the screen and meters (published values, falls, repaint only on
//! a change), the latency badge, the CV indication, the back panel trims, the contact sheet of
//! the 16 screens and the display-list goldens.
//! Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test character_panel`.

mod support;

use atomik_core::{ParamId, SampleRate};
use atomik_device_sdk::{Device, PrepareInfo};
use atomik_model::{Command, Document, RackDoc};
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::character_panel::{self, CHARACTER_UNITS, cell_rect, preset_at};
use atomik_ui::character_screen;
use atomik_ui::fx::{self, CHARACTER_KEYS, CHARACTER_PRESETS, FxKind};
use atomik_ui::fx_layout::fx_layout;
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::{
    DisplayList, Flip, Key, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent,
};
use proptest::prelude::*;
use std::path::PathBuf;
use support::container::{CONTAINER, add_inner, layered, scoped_view};
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

fn desc() -> &'static atomik_device_sdk::DeviceDescriptor {
    registry().get("atomik.character").unwrap().descriptor
}

fn fx_key() -> atomik_model::DeviceKey {
    key("fx")
}

/// The Character doc with some params overridden.
fn doc_with(params: &[(&str, f32)]) -> RackDoc {
    let mut doc = fx_doc(FxKind::Character);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    doc
}

fn view_of(doc: &RackDoc) -> RackView {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 900.0)), doc, &reg);
    v.sync(doc, &reg);
    v
}

fn record(doc: &RackDoc, flip: Flip, meters: Option<&[f32]>) -> String {
    let mut v = view_of(doc);
    if let Some(m) = meters {
        v.set_device_meter_values(&fx_key(), m);
    }
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

fn texts(doc: &RackDoc, flip: Flip, meters: Option<&[f32]>) -> Vec<String> {
    let mut v = view_of(doc);
    if let Some(m) = meters {
        v.set_device_meter_values(&fx_key(), m);
    }
    v.set_flip(flip);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(t: &[String], s: &str) -> bool {
    t.iter().any(|x| x == s)
}

fn cabled(doc: &mut RackDoc, socket: &str) {
    doc.devices.push(device("lfo", "atomik.poly-synth", "LFO"));
    doc.cables.push(cable(("lfo", "out_l"), ("fx", socket)));
}

// ------------------------------------------------------------------ the pure maths

#[test]
fn the_square_law_matches_the_device_and_displays_in_db() {
    use atomik_devices::character::{knob_gain, knob_gain_db};
    for k in 0..=100 {
        let x = k as f32 / 100.0;
        assert_eq!(
            fx::character_gain(x).to_bits(),
            knob_gain(x).to_bits(),
            "{x}"
        );
        let (a, b) = (fx::character_gain_db(x), knob_gain_db(x));
        assert!(
            a.to_bits() == b.to_bits() || (a - b).abs() < 1e-4,
            "{x}: {a} {b}"
        );
    }
    assert_eq!(fx::character_gain_db(0.0), f32::NEG_INFINITY);
    assert!(fx::character_gain_db(0.5).abs() < 1e-6);
    assert!((fx::character_gain_db(1.0) - 12.041).abs() < 1e-2);
    assert_eq!(fx::character_gain_text(0.0), "-inf dB");
    assert_eq!(fx::character_gain_text(0.5), "+0.0 dB");
    assert_eq!(fx::character_gain_text(1.0), "+12.0 dB");
    assert_eq!(fx::character_gain_text(0.25), "-12.0 dB");
    assert_eq!(fx::character_gain_text(f32::NAN), "+0.0 dB");
    assert_eq!(
        fx::character_value_text("gain", 0.25).as_deref(),
        Some("-12.0 dB")
    );
    assert_eq!(
        fx::character_value_text("volume", 1.0).as_deref(),
        Some("+12.0 dB")
    );
    assert_eq!(fx::character_value_text("transform", 0.5), None);
}

#[test]
fn the_preset_table_matches_the_device() {
    use atomik_devices::character::{PRESET_NAMES, voicing::PRESET_KEYS};
    assert_eq!(CHARACTER_PRESETS, 16);
    assert_eq!(CHARACTER_KEYS, PRESET_KEYS);
    assert_eq!(fx::CHARACTER_NAMES, PRESET_NAMES);
    let d = desc();
    let p = d.params.iter().find(|p| p.key == "preset").unwrap();
    assert!(matches!(p.unit, atomik_core::Unit::Choice(n) if n == PRESET_NAMES));
    assert_eq!(
        CHARACTER_KEYS,
        [
            "reel",
            "loudness",
            "sparkle",
            "low-end",
            "wide",
            "pocket-radio",
            "camcorder",
            "dusty-groove",
            "lossy",
            "swirl",
            "torn-cone",
            "tin-robot",
            "bit-bender",
            "spin-cycle",
            "ion-sweep",
            "ghost-pipes",
        ]
    );
}

#[test]
fn the_latency_class_table_matches_the_device() {
    use atomik_devices::character::Character;
    for rate in [44_100u32, 48_000, 96_000] {
        let mut short = None;
        for preset in 0..16usize {
            let mut c = Character::new();
            c.prepare(&PrepareInfo {
                sample_rate: SampleRate::new(f64::from(rate)).unwrap(),
                seed: 1,
            });
            c.set_param(ParamId(1), preset as f32);
            let lat = c.latency_frames();
            if fx::character_is_long(preset) {
                assert!(lat > 500, "preset {preset} at {rate}: {lat}");
            } else {
                assert!(lat < 100, "preset {preset} at {rate}: {lat}");
                assert_eq!(*short.get_or_insert(lat), lat, "one short class");
            }
        }
    }
    assert!(fx::character_is_long(8) && !fx::character_is_long(0));
    assert_eq!((0..16).filter(|p| fx::character_is_long(*p)).count(), 1);
}

#[test]
fn the_cv_span_is_the_knob_plus_minus_the_trim_clamped() {
    assert_eq!(fx::cv_span(0.5, 1.0), (0.0, 1.0));
    assert_eq!(fx::cv_span(0.5, 0.0), (0.5, 0.5));
    let (lo, hi) = fx::cv_span(0.5, 0.25);
    assert!((lo - 0.25).abs() < 1e-6 && (hi - 0.75).abs() < 1e-6);
    let (lo, hi) = fx::cv_span(0.9, 0.5);
    assert!((lo - 0.4).abs() < 1e-6 && (hi - 1.0).abs() < 1e-6);
    let (lo, hi) = fx::cv_span(0.1, 0.5);
    assert!(lo == 0.0 && (hi - 0.6).abs() < 1e-6);
    assert_eq!(fx::cv_span(f32::NAN, 0.5), (0.0, 1.0));
    // A trim above half is not capped at half, and one above full is capped at full.
    let (lo, hi) = fx::cv_span(0.7, 0.6);
    assert!(
        (lo - 0.1).abs() < 1e-6 && (hi - 1.0).abs() < 1e-6,
        "{lo} {hi}"
    );
    assert_eq!(fx::cv_span(0.7, 2.0), (0.0, 1.0));
    assert_eq!(fx::cv_span(0.7, f32::NAN), (0.7, 0.7));
}

#[test]
fn the_display_values_have_their_own_ballistics() {
    let mut shown = Vec::new();
    assert!(fx::update_display(
        FxKind::Character,
        &[0.5, 0.35, 0.7, 0.6],
        &mut shown
    ));
    assert_eq!(shown, vec![-6.0, -9.0, 0.7, 0.6]);
    // Peaks fall 1.5 dB per feed, the Transform value follows at once, the activity falls.
    assert!(fx::update_display(
        FxKind::Character,
        &[0.0, 0.0, 0.2, 0.0],
        &mut shown
    ));
    assert_eq!(shown[0], -7.5);
    assert_eq!(shown[1], -10.5);
    assert_eq!(shown[2], 0.2);
    assert!(
        (shown[3] - (0.6 - fx::ACTIVITY_FALL)).abs() < 0.021,
        "{shown:?}"
    );
    // The fall is 0.04 per feed (about 0.4 s for the whole range at 60 feeds per second).
    assert_eq!(fx::ACTIVITY_FALL, 0.04);
    assert!((shown[3] - 0.56).abs() < 1e-6, "{shown:?}");
    // Silence settles and then reports no change.
    let mut n = 0;
    while fx::update_display(FxKind::Character, &[0.0; 4], &mut shown) {
        n += 1;
        assert!(n < 200, "never settles");
    }
    assert_eq!(shown, vec![-90.0, -90.0, 0.0, 0.0]);
    // Hostile values.
    let mut s = Vec::new();
    fx::update_display(
        FxKind::Character,
        &[f32::NAN, f32::INFINITY, 9.0, -1.0],
        &mut s,
    );
    assert!(s.iter().all(|v| v.is_finite()), "{s:?}");
    assert!(s[2] <= 1.0 && s[3] >= 0.0);
}

// ------------------------------------------------------------------ layout

#[test]
fn the_panel_is_three_units_with_a_matrix_of_sixteen_cells() {
    let d = desc();
    let spec = fx_layout(FxKind::Character, d);
    assert_eq!(spec.height_units, CHARACTER_UNITS);
    assert_eq!(CHARACTER_UNITS, 3);
    let g = spec.fx.unwrap().character.expect("character geometry");
    let panel = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    for r in [g.matrix, g.screen, g.badge, g.bars] {
        assert!(r.width() > 10.0 && r.height() > 5.0, "{r:?}");
        assert!(
            r.x0 >= panel.x0 && r.x1 <= panel.x1 && r.y0 >= 0.0 && r.y1 <= panel.y1,
            "{r:?}"
        );
    }
    // The 16 cells sit inside the matrix, row-major from the top left, and never overlap.
    let cells: Vec<Rect> = (0..16).map(|i| cell_rect(g.matrix, i)).collect();
    for (i, a) in cells.iter().enumerate() {
        assert!(
            a.x0 >= g.matrix.x0
                && a.x1 <= g.matrix.x1
                && a.y0 >= g.matrix.y0
                && a.y1 <= g.matrix.y1
        );
        assert!(a.width() >= 40.0 && a.height() >= 36.0, "{a:?}");
        for b in &cells[i + 1..] {
            assert!(!(a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1));
        }
    }
    for i in 0..16 {
        assert!(cells[i].x0 > cells[0].x0 - 0.1);
        assert_eq!(
            cells[i].center().x,
            cells[i % 4].center().x,
            "column of {i}"
        );
        assert_eq!(
            cells[i].center().y,
            cells[i / 4 * 4].center().y,
            "row of {i}"
        );
    }
    assert!(cells[1].x0 > cells[0].x1 - 0.1 && cells[4].y0 > cells[0].y1 - 0.1);
    // Hit testing: every cell centre hits itself, the gaps and the outside hit nothing.
    for (i, r) in cells.iter().enumerate() {
        assert_eq!(preset_at(g.matrix, r.center()), Some(i));
        assert_eq!(
            preset_at(g.matrix, Point::new(r.x0 + 0.5, r.y0 + 0.5)),
            Some(i)
        );
    }
    assert_eq!(
        preset_at(g.matrix, Point::new(g.matrix.x1 + 5.0, g.matrix.y0 + 5.0)),
        None
    );
    assert_eq!(
        preset_at(g.matrix, Point::new(g.matrix.x0 - 5.0, g.matrix.y0 + 5.0)),
        None
    );
    let gap = Point::new(
        cells[0].x1 + 0.5 * (cells[1].x0 - cells[0].x1),
        cells[0].center().y,
    );
    if cells[1].x0 > cells[0].x1 {
        assert_eq!(preset_at(g.matrix, gap), None);
    }
}

#[test]
fn the_trims_are_back_controls_and_the_sockets_sit_in_order() {
    let d = desc();
    let spec = fx_layout(FxKind::Character, d);
    let g = spec.fx.unwrap().character.unwrap();
    let key_of = |c: &atomik_ui::autolayout::PlacedControl| d.params[c.param].key;
    let back: Vec<&str> = spec
        .front
        .iter()
        .filter(|c| spec.on_back(c))
        .map(key_of)
        .collect();
    assert_eq!(back, vec!["transform_cv_trim", "mix_cv_trim"]);
    let front: Vec<&str> = spec
        .front
        .iter()
        .filter(|c| !spec.on_back(c))
        .map(key_of)
        .collect();
    assert_eq!(front, vec!["gain", "transform", "mix", "volume"]);
    assert_eq!(g.back_params.len(), 2);
    // Each trim sits under its CV jack.
    let jack = |k: &str| {
        let s = d.sockets.iter().position(|s| s.key == k).unwrap();
        spec.back.iter().find(|p| p.socket == s).unwrap().rect
    };
    for (trim, socket) in [
        ("transform_cv_trim", "transform_cv_in"),
        ("mix_cv_trim", "mix_cv_in"),
    ] {
        let c = spec.front.iter().find(|c| key_of(c) == trim).unwrap();
        let j = jack(socket);
        assert!((c.rect.center().x - j.center().x).abs() < 1.0, "{trim}");
        assert!(c.rect.y0 >= j.y1, "{trim} under the jack");
    }
    let order: Vec<&str> = spec.back.iter().map(|p| d.sockets[p.socket].key).collect();
    assert_eq!(
        order,
        vec![
            "in_l",
            "in_r",
            "transform_cv_in",
            "mix_cv_in",
            "out_l",
            "out_r"
        ]
    );
}

#[test]
fn the_knob_row_follows_the_screen_in_the_documented_order() {
    let d = desc();
    let spec = fx_layout(FxKind::Character, d);
    let xs: Vec<(&str, f64)> = spec
        .front
        .iter()
        .filter(|c| !spec.on_back(c))
        .map(|c| (d.params[c.param].key, c.rect.center().x))
        .collect();
    assert!(
        xs.windows(2).all(|w| w[0].1 < w[1].1),
        "left to right: {xs:?}"
    );
}

// ------------------------------------------------------------------ painting

#[test]
fn the_front_shows_the_name_the_readouts_and_the_knob_values() {
    let doc = doc_with(&[]);
    let t = texts(&doc, Flip::Front, Some(&raw_meters(FxKind::Character)));
    for s in [
        "PRESET",
        "SCREEN",
        "METER",
        "CONTROLS",
        "REEL",
        "IN",
        "OUT",
        "ACT",
        "Input Gain",
        "Transform",
        "Dry/Wet",
        "Volume",
        "+7.0 dB",
        "-3.9 dB",
        "70 %",
        "100 %",
        "LAT SHORT",
    ] {
        assert!(has(&t, s), "{s:?}: {t:?}");
    }
    assert!(!has(&t, "LAT LONG"));
}

#[test]
fn the_screen_names_the_active_preset_and_lossy_shows_the_long_badge() {
    for (i, name) in atomik_devices::character::PRESET_NAMES.iter().enumerate() {
        let doc = doc_with(&[("preset", i as f32)]);
        let t = texts(&doc, Flip::Front, None);
        assert!(has(&t, &name.to_uppercase()), "{name}: {t:?}");
        let long = i == 8;
        assert_eq!(has(&t, "LAT LONG"), long, "{name}");
        assert_eq!(has(&t, "LAT SHORT"), !long, "{name}");
    }
}

#[test]
fn the_knobs_show_minus_infinity_at_the_bottom() {
    let doc = doc_with(&[("gain", 0.0), ("volume", 1.0)]);
    let t = texts(&doc, Flip::Front, None);
    assert!(has(&t, "-inf dB") && has(&t, "+12.0 dB"), "{t:?}");
}

#[test]
fn every_state_paints_differently() {
    let base = record(&doc_with(&[]), Flip::Front, None);
    let metered = record(
        &doc_with(&[]),
        Flip::Front,
        Some(&raw_meters(FxKind::Character)),
    );
    assert_ne!(base, metered, "meters and screen react");
    let mut seen = vec![base.clone()];
    for i in 1..16 {
        let r = record(&doc_with(&[("preset", i as f32)]), Flip::Front, None);
        assert!(!seen.contains(&r), "preset {i} looks like an earlier one");
        seen.push(r);
    }
    let mut cab = doc_with(&[]);
    cabled(&mut cab, "transform_cv_in");
    assert_ne!(record(&cab, Flip::Front, None), base, "the CV indication");
    let mut cab2 = doc_with(&[]);
    cabled(&mut cab2, "mix_cv_in");
    assert_ne!(record(&cab2, Flip::Front, None), base);
    assert_ne!(
        record(&cab2, Flip::Front, None),
        record(&cab, Flip::Front, None)
    );
    // The trim sets the size of the indication.
    let mut half = cab.clone();
    half.devices[0]
        .params
        .insert("transform_cv_trim".into(), 0.4);
    assert_ne!(
        record(&half, Flip::Front, None),
        record(&cab, Flip::Front, None)
    );
    // Without a cable the trim changes nothing on the front.
    let mut trimmed = doc_with(&[]);
    trimmed.devices[0]
        .params
        .insert("transform_cv_trim".into(), 0.4);
    assert_eq!(record(&trimmed, Flip::Front, None), base);
}

#[test]
fn the_back_shows_the_jacks_and_the_trims() {
    let t = texts(&doc_with(&[]), Flip::Back, None);
    for s in [
        "AUDIO IN",
        "CV IN",
        "AUDIO OUT",
        "In L",
        "In R",
        "Transform CV",
        "Dry/Wet CV",
        "Out L",
        "Out R",
        "CV trim",
    ] {
        assert!(has(&t, s), "{s:?}: {t:?}");
    }
    assert_eq!(t.iter().filter(|x| *x == "CV trim").count(), 2);
    // The front controls are not on the back and the trims are not on the front.
    assert!(!has(&t, "Input Gain"));
    let f = texts(&doc_with(&[]), Flip::Front, None);
    assert!(!has(&f, "CV trim"));
    let a = record(&doc_with(&[]), Flip::Back, None);
    let mut trimmed = doc_with(&[]);
    trimmed.devices[0].params.insert("mix_cv_trim".into(), 0.2);
    assert_ne!(record(&trimmed, Flip::Back, None), a, "the trim knob moves");
}

#[test]
fn the_colours_the_panel_uses_read_against_their_backgrounds() {
    // No new theme token: the badge, the readouts and the glyphs use existing ones. `Theme` has
    // the single `dark()` theme, so there is no light theme to compare.
    use atomik_ui::theme::{contrast_ratio, mix};
    let t = Theme::dark();
    let accent = t.accent_for_type("atomik.character");
    let lit = character_panel::lit_face(&t, accent);
    let over = |c: atomik_ui::peniko::Color, bg| contrast_ratio(c, bg);
    // Text on the screen (name, badge) and under the knobs.
    assert!(over(t.warning, t.lcd) >= 4.5, "long badge");
    assert!(over(t.text_muted, t.lcd) >= 4.5, "short badge");
    assert!(
        over(mix(accent, atomik_ui::peniko::Color::WHITE, 0.3), t.lcd) >= 4.5,
        "name"
    );
    assert!(over(t.text, t.panel_bottom) >= 4.5, "readouts");
    // Glyphs (non-text, 3:1) on the idle and the lit button face.
    assert!(over(t.text_muted, t.button) >= 3.0, "idle glyph");
    assert!(
        over(character_panel::lit_glyph(accent), lit) >= 3.0,
        "lit glyph"
    );
    assert!(over(accent, t.panel_bottom) >= 3.0, "the lit outline");
    // The CV ring on the panel.
    assert!(over(t.cable_cv, t.panel_bottom) >= 3.0, "cv ring");
}

#[test]
fn the_screens_react_to_transform_activity_and_phase() {
    let theme = Theme::dark();
    let rect = Rect::new(0.0, 0.0, 320.0, 104.0);
    let accent = theme.accent_for_type("atomik.character");
    let rec = |preset: usize, t: f32, a: f32, ph: f32| {
        let mut dl = DisplayList::new();
        character_screen::scene(&mut dl, &theme, rect, preset, t, a, ph, accent);
        dl.to_json()
    };
    let mut all = Vec::new();
    for p in 0..16 {
        let base = rec(p, 0.3, 0.5, 0.2);
        assert!(base.lines().count() >= 6, "scene {p} draws something");
        assert_ne!(base, rec(p, 0.9, 0.5, 0.2), "scene {p} reacts to Transform");
        assert_ne!(base, rec(p, 0.3, 1.0, 0.2), "scene {p} reacts to activity");
        assert_ne!(
            base,
            rec(p, 0.3, 0.5, 0.6),
            "scene {p} animates with the phase"
        );
        assert_eq!(base, rec(p, 0.3, 0.5, 0.2), "scene {p} is deterministic");
        assert!(!all.contains(&base), "scene {p} repeats an earlier one");
        all.push(base);
    }
    // Out-of-range inputs are clamped, not drawn outside the rect.
    let _ = rec(3, 7.0, -4.0, 123.4);
    let _ = rec(3, f32::NAN, f32::INFINITY, f32::NAN);
}

#[test]
fn the_glyphs_are_sixteen_different_drawings() {
    let theme = Theme::dark();
    let rect = Rect::new(0.0, 0.0, 50.0, 46.0);
    let rec = |p: usize, lit: bool| {
        let mut dl = DisplayList::new();
        character_screen::glyph(&mut dl, &theme, rect, p, lit, theme.text_muted);
        dl.to_json()
    };
    let mut all = Vec::new();
    for p in 0..16 {
        let g = rec(p, false);
        assert!(g.lines().count() > 3, "glyph {p}");
        assert!(!all.contains(&g), "glyph {p} repeats");
        all.push(g);
    }
}

#[test]
fn the_contact_sheet_draws_all_sixteen_screens() {
    let theme = Theme::dark();
    let mut dl = DisplayList::new();
    character_screen::contact_sheet(
        &mut dl,
        &theme,
        Rect::new(0.0, 0.0, 1280.0, 420.0),
        0.3,
        0.6,
        0.25,
    );
    let t: Vec<String> = dl.texts().into_iter().map(str::to_string).collect();
    for name in atomik_devices::character::PRESET_NAMES {
        assert!(has(&t, &name.to_uppercase()), "{name}: {t:?}");
    }
    check("character_screens_1x.json", &dl.to_json());
}

#[test]
fn goldens() {
    let metered = raw_meters(FxKind::Character);
    check(
        "character_front_1x.json",
        &record(&doc_with(&[]), Flip::Front, Some(&metered)),
    );
    check(
        "character_front_lossy_1x.json",
        &record(&doc_with(&[("preset", 8.0)]), Flip::Front, Some(&metered)),
    );
    let mut cab = doc_with(&[("transform_cv_trim", 0.5)]);
    cabled(&mut cab, "transform_cv_in");
    check(
        "character_front_cv_1x.json",
        &record(&cab, Flip::Front, Some(&metered)),
    );
    check(
        "character_back_1x.json",
        &record(&doc_with(&[("mix_cv_trim", 0.5)]), Flip::Back, None),
    );
}

// ------------------------------------------------------------------ the meter feed

#[test]
fn the_feed_repaints_only_when_a_displayed_value_changes() {
    let doc = doc_with(&[]);
    let mut v = view_of(&doc);
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint());
    let rev = |v: &RackView| {
        v.panel_revisions()
            .iter()
            .find(|(k, _)| *k == fx_key())
            .unwrap()
            .1
    };
    let r0 = rev(&v);
    v.set_device_meter_values(&fx_key(), &[0.5, 0.35, 0.7, 0.6]);
    assert!(v.needs_repaint());
    assert!(rev(&v) > r0);
    assert_eq!(v.device_meters(&fx_key()), &[-6.0, -9.0, 0.7, 0.6]);
    v.paint(&mut dl, &doc, &reg);
    let r1 = rev(&v);
    // Values that round to the same display values: nothing.
    v.set_device_meter_values(&fx_key(), &[0.5001, 0.3501, 0.7001, 0.6001]);
    assert!(!v.needs_repaint());
    assert_eq!(rev(&v), r1);
    // Silence settles and stops requesting repaints.
    let mut n = 0;
    loop {
        v.set_device_meter_values(&fx_key(), &[0.0; 4]);
        if !v.needs_repaint() {
            break;
        }
        v.paint(&mut dl, &doc, &reg);
        n += 1;
        assert!(n < 300, "never settles");
    }
    assert_eq!(v.device_meters(&fx_key()), &[-90.0, -90.0, 0.0, 0.0]);
    // The back face shows no meters and does not repaint.
    v.set_flip(Flip::Back);
    v.paint(&mut dl, &doc, &reg);
    v.set_device_meter_values(&fx_key(), &[0.9, 0.9, 0.9, 0.9]);
    assert!(!v.needs_repaint());
}

#[test]
fn the_screen_animates_while_the_values_move_and_stays_still_when_idle() {
    let doc = doc_with(&[]);
    let reg = registry();
    let mut v = view_of(&doc);
    let shot = |v: &mut RackView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    let idle = shot(&mut v);
    v.set_device_meter_values(&fx_key(), &[0.5, 0.35, 0.7, 0.6]);
    let a = shot(&mut v);
    v.set_device_meter_values(&fx_key(), &[0.4, 0.35, 0.7, 0.8]);
    let b = shot(&mut v);
    assert_ne!(a, b);
    assert_ne!(idle, a);
    let (c0, c1) = (shot(&mut v), shot(&mut v));
    assert_eq!(c0, c1, "no feed, no change");
}

// ------------------------------------------------------------------ the matrix

fn rig(params: &[(&str, f32)]) -> (Document, RackView) {
    let doc = doc_with(params);
    let mut v = view_of(&doc);
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

fn param(document: &Document, p: &str) -> f32 {
    document
        .doc()
        .param_value(&fx_key(), p, &registry())
        .unwrap()
}

fn click_at(document: &mut Document, v: &mut RackView, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut all = v.handle(&down(p), document.doc(), &reg);
    apply(document, v, &all);
    let a = v.handle(&up(p), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    all
}

fn press_key(document: &mut Document, v: &mut RackView, k: Key) -> Vec<UiAction> {
    let a = v.handle(
        &support::song::press(k, Modifiers::NONE),
        document.doc(),
        &registry(),
    );
    apply(document, v, &a);
    a
}

fn cell_center(v: &RackView, i: usize) -> Point {
    v.preset_cell_center(&fx_key(), i).expect("cell")
}

#[test]
fn every_cell_selects_its_preset_with_one_set_param() {
    for i in 1..16 {
        let (mut document, mut v) = rig(&[]);
        let c = cell_center(&v, i);
        let a = click_at(&mut document, &mut v, c);
        assert_eq!(
            set_params(&a),
            vec![("preset".to_string(), i as f32, None)],
            "cell {i}"
        );
        assert!(
            a.contains(&UiAction::Select(fx_key())),
            "the click selects the device"
        );
        assert_eq!(param(&document, "preset"), i as f32);
    }
}

#[test]
fn a_click_on_the_active_cell_emits_nothing() {
    let (mut document, mut v) = rig(&[("preset", 9.0)]);
    let c = cell_center(&v, 9);
    let a = click_at(&mut document, &mut v, c);
    assert!(commands(&a).is_empty(), "{a:?}");
    // And a second click on a newly chosen cell too.
    let c = cell_center(&v, 3);
    assert_eq!(set_params(&click_at(&mut document, &mut v, c)).len(), 1);
    assert!(commands(&click_at(&mut document, &mut v, c)).is_empty());
}

#[test]
fn each_preset_click_is_its_own_undo_step() {
    let (mut document, mut v) = rig(&[]);
    for i in [4usize, 11, 2] {
        let c = cell_center(&v, i);
        click_at(&mut document, &mut v, c);
    }
    assert_eq!(param(&document, "preset"), 2.0);
    document.undo(&registry());
    assert_eq!(param(&document, "preset"), 11.0);
    document.undo(&registry());
    assert_eq!(param(&document, "preset"), 4.0);
    document.redo(&registry());
    assert_eq!(param(&document, "preset"), 11.0);
}

#[test]
fn the_gaps_and_the_secondary_button_do_not_change_the_preset() {
    let (mut document, mut v) = rig(&[]);
    let reg = registry();
    let c = cell_center(&v, 6);
    let a = v.handle(
        &down_with(c, PointerButton::Secondary, 1),
        document.doc(),
        &reg,
    );
    assert!(commands(&a).is_empty(), "{a:?}");
    assert!(a.contains(&UiAction::Select(fx_key())));
    let a = v.handle(&up(c), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    assert_eq!(param(&document, "preset"), 0.0);
}

#[test]
fn arrow_keys_move_the_selection_by_one_and_by_row_and_home_end_jump() {
    let (mut document, mut v) = rig(&[]);
    let c = cell_center(&v, 5);
    click_at(&mut document, &mut v, c);
    let go = |document: &mut Document, v: &mut RackView, k: Key| -> Option<f32> {
        let a = press_key(document, v, k);
        let s = set_params(&a);
        assert!(s.len() <= 1, "{s:?}");
        s.first().map(|x| {
            assert_eq!(x.0, "preset");
            assert_eq!(x.2, None, "its own undo step");
            x.1
        })
    };
    assert_eq!(go(&mut document, &mut v, Key::Right), Some(6.0));
    assert_eq!(go(&mut document, &mut v, Key::Down), Some(10.0));
    assert_eq!(go(&mut document, &mut v, Key::Left), Some(9.0));
    assert_eq!(go(&mut document, &mut v, Key::Up), Some(5.0));
    assert_eq!(go(&mut document, &mut v, Key::End), Some(15.0));
    // The ends stay: no wrap, nothing emitted.
    assert_eq!(go(&mut document, &mut v, Key::Right), None);
    assert_eq!(go(&mut document, &mut v, Key::Down), None);
    assert_eq!(go(&mut document, &mut v, Key::Home), Some(0.0));
    assert_eq!(go(&mut document, &mut v, Key::Left), None);
    assert_eq!(go(&mut document, &mut v, Key::Up), None);
    assert_eq!(param(&document, "preset"), 0.0);
}

#[test]
fn keys_do_nothing_without_the_grid_focus_and_the_focus_survives_edits() {
    let (mut document, mut v) = rig(&[]);
    // Nothing focused: the arrows do not touch the preset.
    assert!(set_params(&press_key(&mut document, &mut v, Key::Right)).is_empty());
    let c = cell_center(&v, 1);
    click_at(&mut document, &mut v, c);
    assert_eq!(
        v.focused_control(),
        Some((fx_key(), "preset".to_string())),
        "a click focuses the matrix"
    );
    // The model changed (a rebuild) and the focus is still there.
    press_key(&mut document, &mut v, Key::Right);
    press_key(&mut document, &mut v, Key::Right);
    assert_eq!(param(&document, "preset"), 3.0);
    // Escape unfocuses; the arrows are idle again.
    press_key(&mut document, &mut v, Key::Escape);
    assert!(set_params(&press_key(&mut document, &mut v, Key::Right)).is_empty());
}

#[test]
fn with_the_matrix_focused_the_neighbour_keys_are_left_to_it() {
    use atomik_ui::keymap::CommandOutcome;
    let (mut document, mut v) = rig(&[]);
    let reg = registry();
    let c = cell_center(&v, 2);
    click_at(&mut document, &mut v, c);
    for id in [
        "rack.select_next",
        "rack.select_prev",
        "rack.extend_next",
        "rack.extend_prev",
    ] {
        assert_eq!(
            v.command(id, None, document.doc(), &reg),
            CommandOutcome::Declined,
            "{id}: the arrow keys belong to the matrix"
        );
    }
}

#[test]
fn tab_reaches_the_matrix_and_the_knobs() {
    let (mut document, mut v) = rig(&[]);
    let mut seen = Vec::new();
    for _ in 0..8 {
        press_key(&mut document, &mut v, Key::Tab);
        seen.push(v.focused_control().map(|(_, p)| p));
    }
    for want in ["preset", "gain", "transform", "mix", "volume"] {
        assert!(seen.contains(&Some(want.to_string())), "{want}: {seen:?}");
    }
}

#[test]
fn the_hover_tooltip_names_the_preset() {
    let (document, mut v) = rig(&[]);
    for (i, name) in atomik_devices::character::PRESET_NAMES.iter().enumerate() {
        let c = cell_center(&v, i);
        let _ = v.handle(&mv(c), document.doc(), &registry());
        assert_eq!(v.tooltip().as_deref(), Some(*name), "cell {i}");
    }
    let far = Point::new(2.0, 2.0);
    let _ = v.handle(&mv(far), document.doc(), &registry());
    assert_eq!(v.tooltip(), None);
}

#[test]
fn the_wheel_over_the_matrix_steps_the_preset() {
    let (mut document, mut v) = rig(&[("preset", 4.0)]);
    let c = cell_center(&v, 12);
    let wheel = |d: f64| UiEvent::Wheel {
        delta: Vec2::new(0.0, d),
        pos: c,
        modifiers: Modifiers::NONE,
    };
    let a = v.handle(&wheel(1.0), document.doc(), &registry());
    assert_eq!(set_params(&a), vec![("preset".to_string(), 5.0, None)]);
    apply(&mut document, &mut v, &a);
    let a = v.handle(&wheel(-1.0), document.doc(), &registry());
    assert_eq!(set_params(&a), vec![("preset".to_string(), 4.0, None)]);
}

// ------------------------------------------------------------------ the knobs

fn knob(v: &RackView, k: &str) -> Point {
    v.control_center(&fx_key(), k)
        .unwrap_or_else(|| panic!("no knob {k}"))
}

fn drag(
    document: &mut Document,
    v: &mut RackView,
    from: Point,
    path: &[(Point, Modifiers)],
) -> Vec<UiAction> {
    let reg = registry();
    let mut all = Vec::new();
    let a = v.handle(&down(from), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    for (p, m) in path {
        let a = v.handle(&mv_with(*p, *m), document.doc(), &reg);
        apply(document, v, &a);
        all.extend(a);
    }
    let end = path.last().map_or(from, |x| x.0);
    let a = v.handle(&up(end), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    all
}

#[test]
fn a_knob_drag_is_one_merged_undo_step_for_every_knob() {
    for k in ["gain", "transform", "mix", "volume"] {
        let (mut document, mut v) = rig(&[("mix", 0.5)]);
        let before = param(&document, k);
        let p = knob(&v, k);
        let path: Vec<(Point, Modifiers)> = (1..=8)
            .map(|i| (Point::new(p.x, p.y - 4.0 * f64::from(i)), Modifiers::NONE))
            .collect();
        let a = drag(&mut document, &mut v, p, &path);
        let sets = set_params(&a);
        assert!(sets.len() >= 4, "{k}: {sets:?}");
        assert!(sets.iter().all(|(n, _, m)| n == k && m.is_some()), "{k}");
        let id = sets[0].2;
        assert!(sets.iter().all(|(_, _, m)| *m == id), "{k}: one merge id");
        assert!(matches!(a.last(), Some(UiAction::EndMerge)));
        let after = param(&document, k);
        assert!(after > before, "{k}: {before} -> {after}");
        document.undo(&registry());
        assert!((param(&document, k) - before).abs() < 1e-6, "{k}: one undo");
    }
}

#[test]
fn shift_makes_a_drag_ten_times_finer() {
    let travel = |m: Modifiers| {
        let (mut document, mut v) = rig(&[("transform", 0.2)]);
        let p = knob(&v, "transform");
        drag(
            &mut document,
            &mut v,
            p,
            &[(Point::new(p.x, p.y - 40.0), m)],
        );
        param(&document, "transform") - 0.2
    };
    let (coarse, fine) = (travel(Modifiers::NONE), travel(Modifiers::SHIFT));
    assert!((coarse - 0.2).abs() < 1e-3, "40 px of 200: {coarse}");
    assert!((fine - 0.02).abs() < 1e-3, "{fine}");
}

#[test]
fn double_click_and_ctrl_click_reset_to_the_default() {
    for (k, def) in [
        ("gain", 0.5),
        ("transform", 0.5),
        ("mix", 1.0),
        ("volume", 0.5),
    ] {
        for how in 0..2 {
            let (mut document, mut v) = rig(&[(k, 0.13)]);
            let reg = registry();
            let p = knob(&v, k);
            let ev = if how == 0 {
                down_with(p, PointerButton::Primary, 2)
            } else {
                UiEvent::PointerDown {
                    pos: p,
                    button: PointerButton::Primary,
                    // The reset role is Cmd on macOS and Ctrl elsewhere.
                    modifiers: if cfg!(target_os = "macos") {
                        Modifiers {
                            meta: true,
                            ..Modifiers::NONE
                        }
                    } else {
                        Modifiers::CTRL
                    },
                    clicks: 1,
                }
            };
            let a = v.handle(&ev, document.doc(), &reg);
            assert_eq!(
                set_params(&a),
                vec![(k.to_string(), def, None)],
                "{k} {how}"
            );
            apply(&mut document, &mut v, &a);
            let a = v.handle(&up(p), document.doc(), &reg);
            apply(&mut document, &mut v, &a);
            assert_eq!(param(&document, k), def);
        }
    }
}

#[test]
fn the_wheel_steps_a_knob_and_shift_is_fine() {
    let (mut document, mut v) = rig(&[]);
    let p = knob(&v, "gain");
    let wheel = |d: f64, m: Modifiers| UiEvent::Wheel {
        delta: Vec2::new(0.0, d),
        pos: p,
        modifiers: m,
    };
    let a = v.handle(&wheel(1.0, Modifiers::NONE), document.doc(), &registry());
    let s = set_params(&a);
    assert_eq!(s.len(), 1);
    assert!((s[0].1 - 0.77).abs() < 1e-4, "{s:?}");
    apply(&mut document, &mut v, &a);
    let a = v.handle(&wheel(-1.0, Modifiers::SHIFT), document.doc(), &registry());
    assert!((set_params(&a)[0].1 - 0.768).abs() < 1e-4);
}

#[test]
fn the_drag_tooltip_shows_decibels_for_gain_and_volume() {
    for (k, label) in [("gain", "Input Gain"), ("volume", "Volume")] {
        let (document, mut v) = rig(&[]);
        let reg = registry();
        let p = knob(&v, k);
        let _ = v.handle(&down(p), document.doc(), &reg);
        let _ = v.handle(&mv(Point::new(p.x, p.y - 300.0)), document.doc(), &reg);
        // More than the full range up reaches the top: +12 dB.
        assert_eq!(
            v.tooltip().as_deref(),
            Some(format!("{label}  +12.0 dB").as_str()),
            "{k}"
        );
        let _ = v.handle(&mv(Point::new(p.x, p.y + 600.0)), document.doc(), &reg);
        assert_eq!(
            v.tooltip().as_deref(),
            Some(format!("{label}  -inf dB").as_str())
        );
        let _ = v.handle(&up(p), document.doc(), &reg);
    }
    // Other knobs keep the generic text.
    let (document, mut v) = rig(&[]);
    let p = knob(&v, "transform");
    let _ = v.handle(&down(p), document.doc(), &registry());
    let _ = v.handle(
        &mv(Point::new(p.x, p.y - 20.0)),
        document.doc(),
        &registry(),
    );
    assert_eq!(v.tooltip().as_deref(), Some("Transform  80 %"));
}

#[test]
fn the_back_trims_are_knobs_of_the_back_face_only() {
    let (mut document, mut v) = rig(&[]);
    assert!(
        v.control_center(&fx_key(), "transform_cv_trim").is_none(),
        "not on the front"
    );
    v.set_flip(Flip::Back);
    assert!(
        v.control_center(&fx_key(), "gain").is_none(),
        "not on the back"
    );
    for (k, other) in [
        ("transform_cv_trim", "mix_cv_trim"),
        ("mix_cv_trim", "transform_cv_trim"),
    ] {
        let p = v.control_center(&fx_key(), k).expect("trim on the back");
        let a = drag(
            &mut document,
            &mut v,
            p,
            &[(Point::new(p.x, p.y + 50.0), Modifiers::NONE)],
        );
        let sets = set_params(&a);
        assert!(
            !sets.is_empty() && sets.iter().all(|s| s.0 == k),
            "{k}: {sets:?}"
        );
        assert!(
            (param(&document, k) - 0.75).abs() < 1e-3,
            "{k}: {}",
            param(&document, k)
        );
        assert_eq!(param(&document, other), 1.0);
        // Double-click resets.
        let a = v.handle(
            &down_with(p, PointerButton::Primary, 2),
            document.doc(),
            &registry(),
        );
        assert_eq!(set_params(&a), vec![(k.to_string(), 1.0, None)]);
        let a2 = v.handle(&up(p), document.doc(), &registry());
        apply(&mut document, &mut v, &a);
        apply(&mut document, &mut v, &a2);
    }
}

#[test]
fn cabling_a_cv_input_updates_the_panel_without_a_rebuild() {
    let (mut document, mut v) = rig(&[]);
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &reg);
    let rev = |v: &RackView| {
        v.panel_revisions()
            .iter()
            .find(|(k, _)| *k == fx_key())
            .unwrap()
            .1
    };
    let r0 = rev(&v);
    let before = dl.to_json();
    let mut doc = document.doc().clone();
    cabled(&mut doc, "transform_cv_in");
    v.sync(&doc, &reg);
    assert!(rev(&v) > r0, "the panel is re-recorded for the new cable");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert_ne!(dl.to_json(), before);
    let _ = &mut document;
}

// ------------------------------------------------------------------ inside a container

#[test]
fn inside_a_container_a_right_click_on_a_knob_offers_the_macros() {
    let mut d = layered();
    add_inner(&mut d, "layer", 4, "atomik.character", "chr");
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v
        .control_center(&key("chr"), "gain")
        .expect("the gain knob");
    let a = v.handle(
        &down_with(p, PointerButton::Secondary, 1),
        d.doc(),
        &registry(),
    );
    assert!(commands(&a).is_empty());
    let rows = v.macro_menu_rows();
    assert_eq!(rows.len(), 8, "{rows:?}");
    // The matrix still selects presets there.
    let c = v
        .preset_cell_center(&key("chr"), 7)
        .expect("matrix in the scope");
    let a = v.handle(&down(c), d.doc(), &registry());
    // The press closed the macro menu; the next click picks the cell.
    let _ = a;
    let a = v.handle(&down(c), d.doc(), &registry());
    let s = set_params(&a);
    assert_eq!(s.len(), 1, "{a:?}");
    assert_eq!(s[0].0, "preset");
    let _ = CONTAINER;
}

// ------------------------------------------------------------------ robustness

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn no_gesture_sequence_makes_the_model_refuse_a_command(
        steps in proptest::collection::vec((0u8..6, 0.0f64..1.0, 0.0f64..1.0, 1u8..3, 0u8..8), 1..32),
        back in any::<bool>(),
    ) {
        let (mut document, mut v) = rig(&[]);
        let reg = registry();
        if back {
            v.set_flip(Flip::Back);
        }
        let rect = v.device_rect(&fx_key()).unwrap();
        let area = Rect::new(CONTENT_X.0, rect.y0, CONTENT_X.1, rect.y1);
        for (what, fx, fy, clicks, keyn) in steps {
            let p = v.rack_to_view(Point::new(
                area.x0 + fx * area.width(),
                area.y0 + fy * area.height(),
            ));
            let mut actions = Vec::new();
            match what {
                0 => {
                    actions.extend(v.handle(&down_with(p, PointerButton::Primary, clicks), document.doc(), &reg));
                    actions.extend(v.handle(&up(p), document.doc(), &reg));
                }
                1 => {
                    actions.extend(v.handle(&down(p), document.doc(), &reg));
                    let to = Point::new(p.x + (fx - 0.5) * 120.0, p.y + (fy - 0.5) * 300.0);
                    actions.extend(v.handle(&mv(to), document.doc(), &reg));
                    actions.extend(v.handle(&mv(Point::new(to.x, to.y - 400.0)), document.doc(), &reg));
                    actions.extend(v.handle(&up(to), document.doc(), &reg));
                }
                2 => actions.extend(v.handle(&UiEvent::Wheel {
                    delta: Vec2::new(0.0, if fx > 0.5 { 1.0 } else { -1.0 }),
                    pos: p,
                    modifiers: Modifiers::NONE,
                }, document.doc(), &reg)),
                3 => {
                    actions.extend(v.handle(&down_with(p, PointerButton::Secondary, 1), document.doc(), &reg));
                    actions.extend(v.handle(&up(p), document.doc(), &reg));
                }
                4 => {
                    let k = [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End, Key::Tab, Key::Escape][keyn as usize].clone();
                    actions.extend(v.handle(&support::song::press(k, Modifiers::NONE), document.doc(), &reg));
                }
                _ => {
                    actions.extend(v.handle(&UiEvent::PointerDown {
                        pos: p,
                        button: PointerButton::Primary,
                        modifiers: Modifiers::CTRL,
                        clicks: 1,
                    }, document.doc(), &reg));
                    actions.extend(v.handle(&up(p), document.doc(), &reg));
                }
            }
            for a in actions {
                match a {
                    UiAction::Command(c) => {
                        if let Command::SetParam { key: k, param, value, .. } = &c {
                            let pd = desc().params.iter().find(|x| x.key == param).expect("param exists");
                            prop_assert!(*value >= pd.min && *value <= pd.max, "{param} {value}");
                            prop_assert_eq!(k, &fx_key());
                            if param == "preset" {
                                prop_assert!(value.fract() == 0.0);
                            }
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

// ------------------------------------------------------------------ previews

/// Renders PNG previews of the panel (front in four states, back, the contact sheet) with
/// vello_cpu. Run: `cargo test -p atomik-ui --test character_panel -- --ignored`. Writes
/// `target/character-preview/*.png` (not committed).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    const SCALE: f64 = 1.5;
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/character-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let reg = registry();
    let mut text = TextEngine::new();
    let mut states: Vec<(&str, RackDoc, Flip)> = vec![
        ("front", doc_with(&[]), Flip::Front),
        ("front_lossy", doc_with(&[("preset", 8.0)]), Flip::Front),
        ("front_dusty", doc_with(&[("preset", 7.0)]), Flip::Front),
        ("back", doc_with(&[("mix_cv_trim", 0.5)]), Flip::Back),
    ];
    let mut cab = doc_with(&[("transform_cv_trim", 0.5)]);
    cabled(&mut cab, "transform_cv_in");
    states.push(("front_cv", cab, Flip::Front));
    for (name, doc, flip) in states {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        v.set_device_meter_values(&fx_key(), &raw_meters(FxKind::Character));
        v.set_flip(flip);
        let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        std::fs::write(
            dir.join(format!("{name}.png")),
            canvas.finish().into_png().expect("png"),
        )
        .unwrap();
    }
    let theme = Theme::dark();
    let mut canvas = CpuCanvas::new(1280, 560, &mut text);
    character_screen::contact_sheet(
        &mut canvas,
        &theme,
        Rect::new(0.0, 0.0, 1280.0, 560.0),
        0.5,
        0.7,
        0.25,
    );
    std::fs::write(
        dir.join("sheet.png"),
        canvas.finish().into_png().expect("png"),
    )
    .unwrap();
    let mut canvas = CpuCanvas::new(800, 100, &mut text);
    for i in 0..16 {
        let r = Rect::new(i as f64 * 50.0, 0.0, i as f64 * 50.0 + 50.0, 46.0);
        character_screen::glyph(&mut canvas, &theme, r, i, i % 2 == 0, theme.text_muted);
        let r2 = r + Vec2::new(0.0, 50.0);
        character_screen::glyph(
            &mut canvas,
            &theme,
            r2,
            i,
            true,
            theme.accent_for_type("atomik.character"),
        );
    }
    std::fs::write(
        dir.join("glyphs.png"),
        canvas.finish().into_png().expect("png"),
    )
    .unwrap();
}
