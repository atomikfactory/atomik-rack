//! The granular sampler's rack panel: layout, display-list goldens (front on the built-in
//! source, front with a sample and live grains, back), the sample strip, the waveform and the
//! position, spray and grain markers following the published values, the tooltips, the controls
//! and the Edit button.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test granular_panel`.

mod support;

use atomik_model::{RackDoc, ZoneDoc};
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::granular_panel::{
    GRANULAR_TYPE, METER_COUNT, SampleInfo, TRAIL_GRAINS, granular_help, granular_label,
    granular_layout, live_grains, markers, sample_info, shown_zone, strip_lines,
};
use atomik_ui::kurbo::{Rect, Vec2};
use atomik_ui::{DisplayList, Flip, PointerButton, RackView, UiAction};
use std::path::PathBuf;
use support::audio::pyramid;
use support::sampler::*;
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

/// One granular sampler `grain` (zones and params as given) with the three sampler fixture
/// assets in the pool.
fn granular_only(zones: Vec<ZoneDoc>, params: &[(&str, f32)]) -> RackDoc {
    let mut doc = sampler_doc(zones);
    doc.devices.retain(|d| d.key == key("sampler"));
    doc.cables.clear();
    let d = &mut doc.devices[0];
    d.key = key("grain");
    d.type_id = GRANULAR_TYPE.to_string();
    d.name = "Grain Sampler".to_string();
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc
}

fn setup(zones: Vec<ZoneDoc>, params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let doc = granular_only(zones, params);
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn json_of(v: &mut RackView, doc: &RackDoc) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

/// The values a device publishes: the source, the position and spray, the counts, the grains.
fn meters(source: f32, position: f32, spray: f32, grains: &[(f32, f32)]) -> Vec<f32> {
    let mut v = vec![-1.0; METER_COUNT];
    v[0] = source;
    v[1] = position;
    v[2] = spray;
    v[3] = grains.len() as f32;
    v[4] = 1.0;
    for (i, (p, l)) in grains.iter().enumerate() {
        v[5 + 2 * i] = *p;
        v[6 + 2 * i] = *l;
    }
    v
}

const GRAINS: [(f32, f32); 5] = [
    (0.31, 0.05),
    (0.36, 0.3),
    (0.28, 0.55),
    (0.4, 0.8),
    (0.33, 0.95),
];

fn one_zone() -> Vec<ZoneDoc> {
    vec![ZoneDoc::new(S_A, FR_A)]
}

/// A view with the sample's peaks and the live values fed.
fn live(zones: Vec<ZoneDoc>, values: &[f32]) -> (RackDoc, RackView) {
    let (doc, mut v) = setup(zones, &[("pos.position", 0.3), ("pos.spray", 0.2)]);
    v.set_sample_peaks(S_A, pyramid(FR_A, 2));
    v.set_device_meter_values(&key("grain"), values);
    (doc, v)
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_on_the_built_in_source_matches_golden() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    check("granular_panel_front_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_front_panel_with_a_sample_and_live_grains_matches_golden() {
    let (doc, mut v) = live(one_zone(), &meters(1.0, 0.3, 0.2, &GRAINS));
    check("granular_panel_live_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    v.set_flip(Flip::Back);
    check("granular_panel_back_1x.json", &json_of(&mut v, &doc));
}

// ------------------------------------------------------------------ the strip

#[test]
fn the_strip_names_the_built_in_source_and_then_the_sample() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let t = texts_of(&mut v, &doc);
    for s in [
        "SAMPLE",
        "Built-in",
        "drop a sample here",
        "EDIT",
        "WAVEFORM",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(
        t.iter().any(|x| x == "Built-in source"),
        "under the waveform: {t:?}"
    );
    assert!(!t.iter().any(|x| x == "SAMPLES"), "one sample: {t:?}");

    let (doc, mut v) = setup(one_zone(), &[]);
    let t = texts_of(&mut v, &doc);
    // The window is two seconds of the 96000-frame asset at 48 kHz.
    for s in ["piano_C3.wav", "2.00 s"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(
        !t.iter()
            .any(|x| x == "Built-in" || x == "drop a sample here")
    );
}

#[test]
fn a_zone_window_sets_the_length_and_more_zones_are_counted() {
    let mut z = ZoneDoc::new(S_A, FR_A);
    z.start = 24_000;
    z.end = 72_000;
    let (doc, mut v) = setup(vec![z.clone(), z], &[]);
    let t = texts_of(&mut v, &doc);
    assert!(
        t.iter().any(|x| x == "1.00 s \u{b7} 2 zones"),
        "window length and zone count: {t:?}"
    );
}

#[test]
fn strip_lines_shorten_a_long_name_and_format_the_time() {
    assert_eq!(
        strip_lines(None),
        ("Built-in".to_string(), "drop a sample here".to_string())
    );
    let long = SampleInfo {
        name: "a_very_long_sample_name_indeed.wav".into(),
        seconds: 12.34,
        zones: 1,
    };
    let (l1, l2) = strip_lines(Some(&long));
    assert_eq!(l1.chars().count(), 14);
    assert!(l1.ends_with('\u{2026}'));
    assert_eq!(l2, "12.3 s");
    let short = SampleInfo {
        name: "pad.wav".into(),
        seconds: 0.5,
        zones: 3,
    };
    assert_eq!(
        strip_lines(Some(&short)),
        ("pad.wav".to_string(), "0.50 s \u{b7} 3 zones".to_string())
    );
}

#[test]
fn sample_info_reads_the_first_zone_and_the_pool() {
    let doc = granular_only(vec![ZoneDoc::new(S_B, FR_B), ZoneDoc::new(S_A, FR_A)], &[]);
    let info = sample_info(&doc.devices[0], &doc).unwrap();
    assert_eq!(
        info,
        SampleInfo {
            name: "strings_C5.flac".into(),
            seconds: 1.0,
            zones: 2
        }
    );
    let none = granular_only(Vec::new(), &[]);
    assert_eq!(sample_info(&none.devices[0], &none), None);
    // A zone whose asset is not in the pool says nothing.
    let mut z = ZoneDoc::new(S_A, FR_A);
    z.asset = "d4".repeat(32);
    let lost = granular_only(vec![z], &[]);
    assert_eq!(sample_info(&lost.devices[0], &lost), None);
}

// ------------------------------------------------------------------ the waveform and markers

#[test]
fn the_waveform_appears_when_the_peaks_arrive_and_follow_the_window() {
    let (doc, mut v) = setup(one_zone(), &[]);
    let without = json_of(&mut v, &doc);
    v.set_sample_peaks(S_A, pyramid(FR_A, 2));
    let with = json_of(&mut v, &doc);
    assert_ne!(without, with, "the peaks draw the waveform");
    // A shorter window draws another picture.
    let mut z = ZoneDoc::new(S_A, FR_A);
    z.start = 10_000;
    z.end = 30_000;
    let doc2 = granular_only(vec![z], &[]);
    v.sync(&doc2, &registry());
    assert_ne!(json_of(&mut v, &doc2), with);
    // Clearing the peaks takes it away again.
    v.clear_sample_peaks(S_A);
    let cleared = json_of(&mut v, &doc2);
    let mut fresh = view(&doc2, &registry());
    let fresh = json_of(&mut fresh, &doc2);
    if cleared != fresh {
        let (a, b): (Vec<&str>, Vec<&str>) = (cleared.lines().collect(), fresh.lines().collect());
        let i = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!(
            "line {i}: {:?} vs {:?} ({} vs {} lines)",
            a.get(i),
            b.get(i),
            a.len(),
            b.len()
        );
    }
}

#[test]
fn peaks_of_another_asset_leave_the_panel_alone() {
    let (doc, mut v) = setup(one_zone(), &[]);
    let before = json_of(&mut v, &doc);
    v.set_sample_peaks(S_B, pyramid(FR_B, 1));
    assert_eq!(json_of(&mut v, &doc), before);
    assert!(!v.needs_repaint() || json_of(&mut v, &doc) == before);
}

#[test]
fn the_built_in_source_draws_its_own_waveform() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let without = json_of(&mut v, &doc);
    // A sample's peaks make no difference while the device plays its built-in pad.
    v.set_sample_peaks(S_A, pyramid(FR_A, 2));
    assert_eq!(json_of(&mut v, &doc), without);
    // The pad is a drawn polygon: more than a bare LCD.
    assert!(without.len() > 20_000, "{}", without.len());
}

#[test]
fn the_published_source_picks_the_waveform() {
    // Zones present but the device says it plays its built-in pad (the sample is not loaded).
    let (doc, mut v) = live(one_zone(), &meters(0.0, 0.3, 0.2, &[]));
    let builtin = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &meters(1.0, 0.3, 0.2, &[]));
    let sample = json_of(&mut v, &doc);
    assert_ne!(builtin, sample);
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "piano_C3.wav"), "{t:?}");
    v.set_device_meter_values(&key("grain"), &meters(0.0, 0.3, 0.2, &[]));
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "Built-in source"), "{t:?}");
}

#[test]
fn the_markers_follow_the_published_values_and_the_knobs_when_idle() {
    let reg = registry();
    let desc = reg.get(GRANULAR_TYPE).unwrap().descriptor;
    let doc = granular_only(one_zone(), &[("pos.position", 0.4), ("pos.spray", 0.1)]);
    let dev = &doc.devices[0];
    assert_eq!(markers(dev, desc, &[]), (0.4, 0.1), "no feed: the knobs");
    assert_eq!(
        markers(dev, desc, &meters(1.0, -1.0, -1.0, &[])),
        (0.4, 0.1),
        "idle: the knobs"
    );
    assert_eq!(
        markers(dev, desc, &meters(1.0, 0.75, 0.3, &[])),
        (0.75, 0.3)
    );
    assert_eq!(
        markers(dev, desc, &[1.0, f32::NAN, 7.0]),
        (0.4, 1.0),
        "bad values are ignored or limited"
    );
}

#[test]
fn the_live_grains_are_the_published_pairs_inside_the_window() {
    assert_eq!(live_grains(&[]), Vec::new());
    assert_eq!(live_grains(&meters(1.0, 0.3, 0.2, &[])), Vec::new());
    assert_eq!(
        live_grains(&meters(1.0, 0.3, 0.2, &GRAINS)),
        GRAINS.to_vec()
    );
    let mut v = meters(1.0, 0.3, 0.2, &GRAINS[..2]);
    v[5 + 2 * 4] = 2.0;
    v[6 + 2 * 4] = 0.1;
    v[5 + 2 * 5] = 0.5;
    v[6 + 2 * 5] = f32::NAN;
    assert_eq!(
        live_grains(&v),
        GRAINS[..2].to_vec(),
        "out of range and NaN are dropped"
    );
    let full: Vec<(f32, f32)> = (0..TRAIL_GRAINS).map(|i| (i as f32 / 20.0, 0.5)).collect();
    assert_eq!(
        live_grains(&meters(1.0, 0.3, 0.2, &full)).len(),
        TRAIL_GRAINS
    );
}

#[test]
fn shown_zone_is_the_published_source_else_the_first() {
    let doc = granular_only(vec![ZoneDoc::new(S_A, FR_A), ZoneDoc::new(S_B, FR_B)], &[]);
    let d = &doc.devices[0];
    assert_eq!(shown_zone(d, &[]).unwrap().asset, S_A, "no feed");
    assert_eq!(shown_zone(d, &[1.0]).unwrap().asset, S_A);
    assert_eq!(shown_zone(d, &[2.0]).unwrap().asset, S_B);
    assert!(shown_zone(d, &[3.0]).is_none(), "no such zone");
    assert!(shown_zone(d, &[0.0]).is_none(), "the built-in source");
    assert_eq!(shown_zone(d, &[f32::NAN]).unwrap().asset, S_A);
}

#[test]
fn position_spray_and_grains_each_change_the_picture() {
    let (doc, mut v) = live(one_zone(), &meters(1.0, 0.3, 0.2, &GRAINS));
    let base = json_of(&mut v, &doc);
    for (what, values) in [
        ("position", meters(1.0, 0.6, 0.2, &GRAINS)),
        ("spray", meters(1.0, 0.3, 0.5, &GRAINS)),
        ("a grain", meters(1.0, 0.3, 0.2, &GRAINS[..4])),
        (
            "a grain's life",
            meters(
                1.0,
                0.3,
                0.2,
                &[
                    (0.31, 0.5),
                    (0.36, 0.3),
                    (0.28, 0.55),
                    (0.4, 0.8),
                    (0.33, 0.95),
                ],
            ),
        ),
    ] {
        v.set_device_meter_values(&key("grain"), &values);
        assert_ne!(json_of(&mut v, &doc), base, "{what}");
    }
    // The same values again change nothing and ask for no repaint.
    v.set_device_meter_values(&key("grain"), &meters(1.0, 0.3, 0.2, &GRAINS));
    let _ = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &meters(1.0, 0.3, 0.2, &GRAINS));
    assert!(!v.needs_repaint());
}

#[test]
fn the_knobs_show_when_no_voice_sounds() {
    let (doc, mut v) = setup(one_zone(), &[("pos.position", 0.7), ("pos.spray", 0.3)]);
    v.set_sample_peaks(S_A, pyramid(FR_A, 2));
    v.set_device_meter_values(&key("grain"), &meters(1.0, -1.0, -1.0, &[]));
    let idle = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &meters(1.0, 0.7, 0.3, &[]));
    assert_eq!(
        json_of(&mut v, &doc),
        idle,
        "the same as the published knob values"
    );
    let (doc2, mut v2) = setup(one_zone(), &[("pos.position", 0.2), ("pos.spray", 0.3)]);
    v2.set_sample_peaks(S_A, pyramid(FR_A, 2));
    v2.set_device_meter_values(&key("grain"), &meters(1.0, -1.0, -1.0, &[]));
    assert_ne!(
        json_of(&mut v2, &doc2),
        idle,
        "moving the knob moves the marker"
    );
}

#[test]
fn a_short_or_long_feed_is_accepted() {
    let (doc, mut v) = live(one_zone(), &[]);
    let base = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &[1.0]);
    let _ = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &vec![0.5; 100]);
    let _ = json_of(&mut v, &doc);
    v.set_device_meter_values(&key("grain"), &[]);
    assert_eq!(json_of(&mut v, &doc), base);
}

#[test]
fn feeding_another_device_type_is_ignored() {
    let doc = reference_doc();
    let reg = registry();
    let mut v = view(&doc, &reg);
    let before = json_of(&mut v, &doc);
    v.set_sample_peaks(S_A, pyramid(FR_A, 2));
    v.clear_sample_peaks(S_B);
    assert_eq!(json_of(&mut v, &doc), before);
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_below_the_band() {
    let reg = registry();
    let desc = reg.get(GRANULAR_TYPE).unwrap().descriptor;
    let spec = granular_layout(desc);
    let mut placed: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    placed.sort_unstable();
    let mut want: Vec<&str> = desc.params.iter().map(|p| p.key).collect();
    want.sort_unstable();
    assert_eq!(placed, want, "every param is a control, once");
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());

    let panel = spec.granular.expect("granular panel");
    let strip = spec.samples.expect("sample strip");
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    for (name, d) in [("strip", strip), ("wave", panel.wave)] {
        assert!(
            d.x0 >= content.x0 - 1e-6
                && d.x1 <= content.x1 + 1e-6
                && d.y0 >= 0.0
                && d.y1 <= content.y1,
            "{name} {d:?} outside {content:?}"
        );
        assert!(d.height() >= 80.0, "{name} {d:?} too small");
        for c in &spec.front {
            let r = c.rect;
            let hit = d.x0 < r.x1 && r.x0 < d.x1 && d.y0 < r.y1 && r.y0 < d.y1;
            assert!(!hit, "{name} {d:?} overlaps {r:?}");
        }
    }
    assert!(panel.wave.width() >= 400.0, "{:?}", panel.wave);
    assert!(
        strip.x1 < panel.wave.x0,
        "the strip is left of the waveform"
    );
    assert!(spec.fm.is_none() && spec.wavetable.is_none() && spec.drum.is_none());
    for c in &spec.front {
        assert!(c.rect.y0 >= 0.0 && c.rect.y1 <= spec.height(), "{c:?}");
        assert!(c.rect.x1 <= content.x1 + 1e-6, "{c:?}");
    }
    // The sections come in the documented order and keep the key families together.
    let titles: Vec<&str> = spec
        .front_sections
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    for t in [
        "WAVEFORM", "MAIN", "GRAIN", "POSITION", "PITCH", "AMP ENV", "FILTER", "MOD ENV", "LFO",
        "CV",
    ] {
        assert!(titles.contains(&t), "{t} missing: {titles:?}");
    }
}

#[test]
fn every_label_is_short_enough_for_a_knob() {
    let reg = registry();
    let desc = reg.get(GRANULAR_TYPE).unwrap().descriptor;
    for p in desc.params {
        if let Some(l) = granular_label(p.key) {
            assert!(l.len() <= 11, "{}: {l}", p.key);
        }
    }
    assert_eq!(granular_label("voices"), None);
    assert_eq!(granular_label("env.amp.attack"), None);
}

// ------------------------------------------------------------------ tooltips and controls

#[test]
fn every_control_has_a_short_tooltip() {
    let (_, v) = setup(Vec::new(), &[]);
    let reg = registry();
    let desc = reg.get(GRANULAR_TYPE).unwrap().descriptor;
    for p in desc.params {
        let text = granular_help(p.key).unwrap_or_else(|| panic!("{} has no help", p.key));
        assert!(
            (10..=110).contains(&text.len()),
            "{}: a tooltip is one short line ({})",
            p.key,
            text.len()
        );
        let at = v.control_center(&key("grain"), p.key).expect(p.key);
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{} shows no tooltip", p.key));
        assert_eq!(help.text, text, "{}", p.key);
    }
    assert_eq!(granular_help("nope"), None);
}

fn set_param(actions: &[UiAction]) -> Vec<(String, f32)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            atomik_model::Command::SetParam { param, value, .. } => Some((param, value)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_sound_and_window_choices_cycle() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let reg = registry();
    for (param, want) in [("sound", 1.0), ("grain.window", 1.0)] {
        let at = v.control_center(&key("grain"), param).unwrap();
        let a = v.handle(&down(at), &doc, &reg);
        assert_eq!(set_param(&a), vec![(param.to_string(), want)]);
        let _ = v.handle(&up(at), &doc, &reg);
    }
}

#[test]
fn a_knob_drags_with_a_tooltip_and_double_click_resets_it() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let reg = registry();
    let c = v.control_center(&key("grain"), "pos.position").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    let s = set_param(&a);
    assert_eq!(s[0].0, "pos.position");
    assert!((s[0].1 - 0.3).abs() < 1e-3, "{s:?}");
    assert_eq!(v.tooltip().as_deref(), Some("Position  30 %"));
    let _ = v.handle(&up(c), &doc, &reg);

    let (doc, mut v) = setup(Vec::new(), &[("pitch.fine", 30.0)]);
    let c = v.control_center(&key("grain"), "pitch.fine").unwrap();
    let a = v.handle(&down_with(c, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(set_param(&a), vec![("pitch.fine".to_string(), 0.0)]);
}

#[test]
fn the_filter_toggle_switches() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let reg = registry();
    let c = v.control_center(&key("grain"), "filter.enabled").unwrap();
    let a = v.handle(&down(c), &doc, &reg);
    assert_eq!(set_param(&a), vec![("filter.enabled".to_string(), 1.0)]);
}

#[test]
fn the_edit_button_opens_the_sample_editor_and_the_body_takes_a_double_click() {
    let (doc, mut v) = setup(one_zone(), &[]);
    let reg = registry();
    let p = v.edit_button_center(&key("grain")).unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    assert!(
        a.contains(&UiAction::OpenSampleEditor(key("grain"))),
        "{a:?}"
    );
    assert!(commands(&a).is_empty());
    let _ = v.handle(&up(p), &doc, &reg);
    v.set_flip(Flip::Back);
    assert!(v.edit_button_center(&key("grain")).is_none(), "front only");
}

#[test]
fn the_panel_is_a_drop_target_like_the_sampler() {
    let (doc, mut v) = setup(Vec::new(), &[]);
    let reg = registry();
    let r = v.device_rect(&key("grain")).unwrap();
    let at = v.rack_to_view(atomik_ui::kurbo::Point::new(r.x0 + 6.0, r.center().y));
    let a = v.handle(
        &atomik_ui::UiEvent::FileDrop {
            pos: at,
            paths: vec![PathBuf::from("pad.wav")],
        },
        &doc,
        &reg,
    );
    assert!(
        a.iter().any(|x| matches!(
            x,
            UiAction::ImportSamples { device, .. } if *device == key("grain")
        )),
        "{a:?}"
    );
}

#[test]
fn the_browser_offers_the_granular_sampler_as_an_instrument() {
    let doc = granular_only(Vec::new(), &[]);
    let reg = registry();
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    let rows = v.browser_rows(&reg);
    assert!(rows.contains(&GRANULAR_TYPE), "{rows:?}");
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::kurbo::Size;
    use atomik_ui::{TextEngine, Theme, UiEvent};
    use support::cpu::CpuCanvas;
    let reg = registry();
    let doc = granular_only(one_zone(), &[("pos.position", 0.3), ("pos.spray", 0.2)]);
    let mut text = TextEngine::new();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 1000.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(1.5), &doc, &reg);
    rack.set_sample_peaks(S_A, pyramid(FR_A, 2));
    rack.set_device_meter_values(&key("grain"), &meters(1.0, 0.3, 0.2, &GRAINS));
    for (name, flip) in [
        ("granular_panel", Flip::Front),
        ("granular_panel_back", Flip::Back),
    ] {
        rack.set_flip(flip);
        let mut canvas = CpuCanvas::new(1230, 1500, &mut text);
        rack.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
