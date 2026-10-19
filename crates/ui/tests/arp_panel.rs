//! The arpeggiator's rack panel: layout, display-list goldens of both
//! faces, the six mode buttons, the seven rate buttons, the octave stepper, the generic
//! controls (hold, run, velocity mode, gate, swing), and the live display of the held keys and
//! the next notes from the published meter values.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test arp_panel`.

mod support;

use atomik_model::{Command, Document, RackDoc};
use atomik_ui::arp_panel::{ARP_MODES, ARP_RATES, arp_layout};
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{DisplayList, Flip, RackView, UiAction};
use proptest::prelude::*;
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
    let doc = arp_only();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), doc, &reg);
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

fn set_param(param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key("arp"),
        param: param.into(),
        value,
        merge: None,
    }
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("arp_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_with_held_keys_and_next_notes_matches_golden() {
    let (doc, mut v) = setup();
    // Three held keys, the next eight notes: C3 E3 G3 C4 E4 G4 C3 E3 (up-down, two octaves).
    v.set_device_meter_values(
        &key("arp"),
        &[3.0, 48.0, 52.0, 55.0, 60.0, 64.0, 67.0, 48.0, 52.0],
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("arp_panel_live_1x.json", &dl.to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup();
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("arp_panel_back_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_and_the_extras_do_not_overlap() {
    let reg = registry();
    let desc = reg.get(ARP_TYPE).unwrap().descriptor;
    let spec = arp_layout(desc);
    assert_eq!(spec.height_units, 2);
    // mode, rate and octaves are drawn by the panel itself, the other seven are controls.
    let mut keys: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "gate",
            "hold",
            "random.seed",
            "run",
            "swing",
            "velocity.fixed",
            "velocity.mode"
        ]
    );
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    let panel = spec.arp.expect("arp panel");
    assert_eq!(panel.modes.len(), ARP_MODES.len());
    assert_eq!(panel.rates.len(), ARP_RATES.len());
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    let mut rects: Vec<Rect> = panel.modes.to_vec();
    rects.extend(panel.rates);
    rects.extend([
        panel.octave_dec,
        panel.octave_value,
        panel.octave_inc,
        panel.display,
    ]);
    rects.extend(spec.front.iter().map(|c| c.rect));
    for (i, a) in rects.iter().enumerate() {
        assert!(
            a.x0 >= content.x0 && a.x1 <= content.x1 && a.y0 >= 0.0 && a.y1 <= content.y1,
            "{a:?} outside {content:?}"
        );
        for b in &rects[i + 1..] {
            let overlap = a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;
            assert!(!overlap, "{a:?} overlaps {b:?}");
        }
    }
    assert!(spec.seq.is_none() && spec.drum.is_none());
}

#[test]
fn the_panel_shows_the_modes_the_rates_the_octaves_and_the_controls() {
    let (doc, mut v) = setup();
    let t = texts_of(&mut v, &doc);
    for s in [
        "MODE",
        "RATE",
        "UP",
        "DOWN",
        "UP-DN",
        "RANDOM",
        "AS PLAYED",
        "CHORD",
        "1/4",
        "1/8",
        "1/8T",
        "1/16",
        "1/16T",
        "1/32",
        "1/32T",
        "Octaves",
        "Gate",
        "Swing",
        "Hold",
        "Run",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    let r = v.device_rect(&key("arp")).unwrap();
    assert!((r.height() - 2.0 * 76.0).abs() < 1e-9, "{r:?}");
}

// ------------------------------------------------------------------ mode, rate, octaves

#[test]
fn each_mode_button_sets_the_mode_param() {
    let (doc, mut v) = setup();
    let d = key("arp");
    for (i, _) in ARP_MODES.iter().enumerate() {
        let p = v.arp_mode_center(&d, i as u8).unwrap();
        let a = click(&mut v, &doc, p);
        // The panel shows mode 2 (up-down): clicking it again is no change.
        if i == 2 {
            assert!(commands(&a).is_empty(), "{a:?}");
        } else {
            assert_eq!(commands(&a), vec![set_param("mode", i as f32)], "mode {i}");
        }
    }
    assert!(v.arp_mode_center(&d, 6).is_none());
}

#[test]
fn each_rate_button_sets_the_rate_param() {
    let (doc, mut v) = setup();
    let d = key("arp");
    for (i, _) in ARP_RATES.iter().enumerate() {
        let p = v.arp_rate_center(&d, i as u8).unwrap();
        let a = click(&mut v, &doc, p);
        if i == 3 {
            assert!(commands(&a).is_empty(), "the current rate: {a:?}");
        } else {
            assert_eq!(commands(&a), vec![set_param("rate", i as f32)], "rate {i}");
        }
    }
    assert!(v.arp_rate_center(&d, 7).is_none());
}

#[test]
fn the_octave_stepper_steps_between_one_and_four() {
    let (mut doc, mut v) = setup();
    let reg = registry();
    let d = key("arp");
    let inc = v.arp_octave_center(&d, true).unwrap();
    let dec = v.arp_octave_center(&d, false).unwrap();
    assert!(dec.x < inc.x);
    assert_eq!(
        commands(&click(&mut v, &doc, inc)),
        vec![set_param("octaves", 3.0)]
    );
    assert_eq!(
        commands(&click(&mut v, &doc, dec)),
        vec![set_param("octaves", 1.0)]
    );
    doc.devices[0].params.insert("octaves".into(), 4.0);
    v.sync(&doc, &reg);
    assert!(commands(&click(&mut v, &doc, inc)).is_empty(), "stops at 4");
    doc.devices[0].params.insert("octaves".into(), 1.0);
    v.sync(&doc, &reg);
    assert!(commands(&click(&mut v, &doc, dec)).is_empty(), "stops at 1");
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "1"), "the value is shown: {t:?}");
}

#[test]
fn every_button_emits_a_value_the_model_accepts() {
    let reg = registry();
    let (doc, mut v) = setup();
    let mut document = Document::new(doc.clone());
    let d = key("arp");
    let mut points: Vec<Point> = (0..6).filter_map(|i| v.arp_mode_center(&d, i)).collect();
    points.extend((0..7).filter_map(|i| v.arp_rate_center(&d, i)));
    points.extend(v.arp_octave_center(&d, true));
    points.extend(v.arp_octave_center(&d, false));
    for p in points {
        let a = click(&mut v, document.doc(), p);
        for c in commands(&a) {
            document
                .apply(c.clone(), &reg)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
        v.sync(document.doc(), &reg);
    }
}

// ------------------------------------------------------------------ generic controls

#[test]
fn the_generic_controls_are_on_the_panel_and_emit_set_param() {
    let (doc, mut v) = setup();
    let reg = registry();
    let d = key("arp");
    for p in [
        "gate",
        "swing",
        "hold",
        "run",
        "velocity.mode",
        "velocity.fixed",
        "random.seed",
    ] {
        assert!(v.control_center(&d, p).is_some(), "{p}");
    }
    for p in ["mode", "rate", "octaves"] {
        assert!(v.control_center(&d, p).is_none(), "{p} is a panel widget");
    }
    let p = v.control_center(&d, "hold").unwrap();
    assert_eq!(
        commands(&click(&mut v, &doc, p)),
        vec![set_param("hold", 1.0)]
    );
    let p = v.control_center(&d, "gate").unwrap();
    let mut cmds = Vec::new();
    for e in [
        down(p),
        mv(p + atomik_ui::kurbo::Vec2::new(0.0, -20.0)),
        up(p),
    ] {
        cmds.extend(commands(&v.handle(&e, &doc, &reg)));
    }
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::SetParam { param, .. } if param == "gate")),
        "{cmds:?}"
    );
}

// ------------------------------------------------------------------ live display

#[test]
fn the_display_shows_the_held_count_and_the_next_notes() {
    let (doc, mut v) = setup();
    let d = key("arp");
    assert_eq!(v.arp_display(&d), None, "nothing published yet");
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "No keys held"), "{t:?}");
    v.set_device_meter_values(&d, &[2.0, 60.0, 64.0, 67.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        v.arp_display(&d),
        Some((2, vec![60, 64, 67])),
        "zeros are no note"
    );
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "HELD 2"), "{t:?}");
    for n in ["C4", "E4", "G4"] {
        assert!(t.iter().any(|x| x == n), "missing {n}: {t:?}");
    }
    assert!(!t.iter().any(|x| x == "No keys held"));
    // A later feed with nothing held goes back.
    v.set_device_meter_values(&d, &[0.0; 9]);
    assert_eq!(v.arp_display(&d), Some((0, vec![])));
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "No keys held"), "{t:?}");
}

#[test]
fn a_meter_feed_re_records_the_panel_only_when_something_changed() {
    let (doc, mut v) = setup();
    let d = key("arp");
    let _ = texts_of(&mut v, &doc);
    v.set_device_meter_values(&d, &[1.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert!(v.needs_repaint());
    let _ = texts_of(&mut v, &doc);
    let revs = v.panel_revisions();
    v.set_device_meter_values(&d, &[1.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(v.panel_revisions(), revs);
    assert!(!v.needs_repaint());
    v.set_device_meter_values(&d, &[1.0, 62.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_ne!(v.panel_revisions(), revs);
    // Short or empty feeds are tolerated.
    v.set_device_meter_values(&d, &[]);
    v.set_device_meter_values(&d, &[2.0]);
}

proptest! {
    #[test]
    fn any_meter_values_paint_without_panicking(vals in proptest::collection::vec(-1.0e6f32..1.0e6f32, 0..12)) {
        let (doc, mut v) = setup();
        v.set_device_meter_values(&key("arp"), &vals);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
        prop_assert!(!dl.is_empty());
    }
}
