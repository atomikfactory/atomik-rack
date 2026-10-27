//! The FM synth's rack panel: layout, display-list goldens (front with the default params, front
//! with a bell-like patch, back), the algorithm diagram and the envelope shapes following the
//! params, the tooltips and the controls.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test fm_panel`.

mod support;

use atomik_dsp::fm::ALGORITHMS;
use atomik_model::RackDoc;
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::fm_panel::{
    EnvShape, FM_TYPE, diagram_boxes, envelope_points, fm_help, fm_label, fm_layout, ranks,
};
use atomik_ui::kurbo::{Rect, Vec2};
use atomik_ui::{DisplayList, Flip, PointerButton, RackView, UiAction};
use std::path::PathBuf;
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

fn fm_only(params: &[(&str, f32)]) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("fm", FM_TYPE, "FM Synth");
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc.devices = vec![d];
    doc.cables.clear();
    doc
}

fn setup(params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let doc = fm_only(params);
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

/// A bell-like patch: the twin-stack algorithm, inharmonic ratios and long decays.
const BELL: [(&str, f32); 9] = [
    ("algorithm", 1.0),
    ("feedback", 0.4),
    ("op2.coarse", 3.0),
    ("op2.fine", 0.5),
    ("op2.level", 0.55),
    ("op1.decay", 4500.0),
    ("op1.sustain", 0.0),
    ("op3.level", 0.4),
    ("op4.level", 0.3),
];

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    check("fm_panel_front_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_front_panel_with_a_bell_patch_matches_golden() {
    let (doc, mut v) = setup(&BELL);
    check("fm_panel_bell_1x.json", &json_of(&mut v, &doc));
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup(&[]);
    v.set_flip(Flip::Back);
    check("fm_panel_back_1x.json", &json_of(&mut v, &doc));
}

// ------------------------------------------------------------------ the displays

#[test]
fn the_diagram_names_the_algorithm_and_follows_it() {
    let (doc, mut v) = setup(&[]);
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "Stack"), "{t:?}");
    assert!(t.iter().any(|x| x == "ALGORITHM") && t.iter().any(|x| x == "ENVELOPES"));
    for n in 1..=4 {
        assert!(t.iter().any(|x| *x == format!("OP {n}")), "OP {n}");
    }
    let base = json_of(&mut v, &doc);
    for (a, alg) in ALGORITHMS.iter().enumerate().skip(1) {
        let (doc, mut v) = setup(&[("algorithm", a as f32)]);
        let t = texts_of(&mut v, &doc);
        assert!(t.iter().any(|x| x == alg.name), "{a}");
        assert_ne!(json_of(&mut v, &doc), base, "algorithm {a}");
    }
}

#[test]
fn the_envelopes_and_the_boxes_follow_the_params() {
    let (doc, mut v) = setup(&[]);
    let base = json_of(&mut v, &doc);
    for change in [
        ("op1.attack", 300.0),
        ("op2.decay", 50.0),
        ("op3.sustain", 0.1),
        ("op4.release", 3000.0),
        ("op2.delay", 400.0),
        ("op3.level", 0.5),
        ("op1.level", 0.0),
    ] {
        let (doc, mut v) = setup(&[change]);
        assert_ne!(json_of(&mut v, &doc), base, "{change:?}");
    }
}

#[test]
fn the_ranks_put_modulators_above_their_targets() {
    assert_eq!(ranks(&ALGORITHMS[0]), [0, 1, 2, 3], "stack");
    assert_eq!(ranks(&ALGORITHMS[11]), [0; 4], "additive");
    assert_eq!(ranks(&ALGORITHMS[3]), [0, 0, 0, 1], "fork");
    assert_eq!(ranks(&ALGORITHMS[10]), [0, 1, 2, 3], "tapped stack");
    for a in &ALGORITHMS {
        let r = ranks(a);
        for from in 0..4 {
            for to in 0..4 {
                if a.modulates(from, to) {
                    assert!(r[from] > r[to], "{}", a.name);
                }
            }
        }
    }
}

#[test]
fn the_diagram_boxes_stay_inside_and_apart() {
    let area = Rect::new(0.0, 0.0, 150.0, 90.0);
    for a in &ALGORITHMS {
        let b = diagram_boxes(a, area);
        for (i, r) in b.iter().enumerate() {
            assert!(
                r.x0 >= 0.0 && r.x1 <= 150.0 && r.y0 >= 14.0 && r.y1 <= 90.0,
                "{} {r:?}",
                a.name
            );
            for q in &b[..i] {
                let hit = r.x0 < q.x1 && q.x0 < r.x1 && r.y0 < q.y1 && q.y0 < r.y1;
                assert!(!hit, "{}: {r:?} {q:?}", a.name);
            }
        }
    }
}

#[test]
fn the_envelope_shape_runs_from_zero_up_to_one_down_to_sustain_and_out() {
    let e = EnvShape {
        delay: 0.0,
        attack: 100.0,
        decay: 400.0,
        sustain: 0.5,
        release: 900.0,
    };
    let p = envelope_points(&e);
    assert_eq!(p[0], (0.0, 0.0));
    assert_eq!(p[1], (0.0, 0.0), "no delay: the attack starts at once");
    assert_eq!(p[2].1, 1.0);
    assert_eq!(p[3].1, 0.5);
    assert_eq!(p[4].1, 0.5);
    assert_eq!(p[5].1, 0.0);
    assert!((p[5].0 - 1.0).abs() < 1e-9);
    assert!(p.windows(2).all(|w| w[1].0 >= w[0].0));
    // A delay pushes the attack later; a longer attack is a longer rise (square-root scale).
    let late = envelope_points(&EnvShape { delay: 500.0, ..e });
    assert!(late[1].0 > 0.2 && late[2].0 > p[2].0);
    let slow = envelope_points(&EnvShape {
        attack: 2500.0,
        ..e
    });
    assert!(slow[2].0 - slow[1].0 > p[2].0 - p[1].0);
    // Nonsense settings still give points in range.
    let odd = envelope_points(&EnvShape {
        delay: -5.0,
        attack: 0.0,
        decay: 0.0,
        sustain: 7.0,
        release: 0.0,
    });
    assert!(
        odd.iter()
            .all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
    );
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_below_the_band() {
    let reg = registry();
    let desc = reg.get(FM_TYPE).unwrap().descriptor;
    let spec = fm_layout(desc);
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

    let panel = spec.fm.expect("fm panel");
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    let displays = std::iter::once(panel.diagram).chain(panel.envelopes);
    for d in displays {
        assert!(
            d.x0 >= content.x0 && d.x1 <= content.x1 + 1e-6 && d.y0 >= 0.0 && d.y1 <= content.y1,
            "{d:?} outside {content:?}"
        );
        assert!(d.width() >= 90.0 && d.height() >= 60.0, "{d:?} too small");
        for c in &spec.front {
            let r = c.rect;
            let hit = d.x0 < r.x1 && r.x0 < d.x1 && d.y0 < r.y1 && r.y0 < d.y1;
            assert!(!hit, "{d:?} overlaps {r:?}");
        }
    }
    // One row per operator: the controls of an operator share a y range.
    for n in 1..=4 {
        let rows: Vec<(f64, f64)> = spec
            .front
            .iter()
            .filter(|c| desc.params[c.param].key.starts_with(&format!("op{n}.")))
            .map(|c| (c.rect.y0, c.rect.y1))
            .collect();
        assert_eq!(rows.len(), 12);
        let top = rows.iter().fold(f64::MAX, |m, r| m.min(r.0));
        let bottom = rows.iter().fold(0.0f64, |m, r| m.max(r.1));
        assert!(bottom - top <= 62.0, "op{n} is one row: {top} {bottom}");
    }
    assert!(spec.wavetable.is_none() && spec.seq.is_none() && spec.samples.is_none());
    for c in &spec.front {
        assert!(c.rect.y0 >= 0.0 && c.rect.y1 <= spec.height(), "{c:?}");
        assert!(c.rect.x1 <= content.x1 + 1e-6, "{c:?}");
    }
}

#[test]
fn every_label_is_short_enough_for_a_knob() {
    let reg = registry();
    let desc = reg.get(FM_TYPE).unwrap().descriptor;
    for p in desc.params {
        if let Some(l) = fm_label(p.key) {
            assert!(l.len() <= 9, "{}: {l}", p.key);
        }
    }
    assert_eq!(fm_label("op1.coarse"), None);
}

// ------------------------------------------------------------------ tooltips and controls

#[test]
fn every_control_has_a_short_tooltip() {
    let (_, v) = setup(&[]);
    let reg = registry();
    let desc = reg.get(FM_TYPE).unwrap().descriptor;
    for p in desc.params {
        let text = fm_help(p.key).unwrap_or_else(|| panic!("{} has no help", p.key));
        assert!(
            (10..=110).contains(&text.len()),
            "{}: a tooltip is one short line ({})",
            p.key,
            text.len()
        );
        let at = v.control_center(&key("fm"), p.key).expect(p.key);
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{} shows no tooltip", p.key));
        assert_eq!(help.text, text, "{}", p.key);
    }
    assert_eq!(fm_help("nope"), None);
    assert_eq!(fm_help("op9.nope"), None);
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
fn the_algorithm_and_sound_choices_cycle() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    for (param, want) in [("algorithm", 1.0), ("sound", 1.0)] {
        let at = v.control_center(&key("fm"), param).unwrap();
        let a = v.handle(&down(at), &doc, &reg);
        assert_eq!(set_param(&a), vec![(param.to_string(), want)]);
        let _ = v.handle(&up(at), &doc, &reg);
    }
}

#[test]
fn a_knob_drags_with_a_tooltip_and_double_click_resets_it() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    let c = v.control_center(&key("fm"), "op2.level").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    let s = set_param(&a);
    assert_eq!(s[0].0, "op2.level");
    assert!((s[0].1 - 0.4).abs() < 1e-3, "{s:?}");
    assert_eq!(v.tooltip().as_deref(), Some("Level  40 %"));
    let _ = v.handle(&up(c), &doc, &reg);

    let (doc, mut v) = setup(&[("op3.detune", 30.0)]);
    let c = v.control_center(&key("fm"), "op3.detune").unwrap();
    let a = v.handle(&down_with(c, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(set_param(&a), vec![("op3.detune".to_string(), 0.0)]);
}

#[test]
fn the_fixed_toggle_switches() {
    let (doc, mut v) = setup(&[]);
    let reg = registry();
    let c = v.control_center(&key("fm"), "op1.fixed").unwrap();
    let a = v.handle(&down(c), &doc, &reg);
    assert_eq!(set_param(&a), vec![("op1.fixed".to_string(), 1.0)]);
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::kurbo::Size;
    use atomik_ui::{TextEngine, Theme, UiEvent};
    use support::cpu::CpuCanvas;
    let reg = registry();
    let doc = fm_only(&BELL);
    let mut text = TextEngine::new();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 1000.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(1.5), &doc, &reg);
    for (name, flip) in [("fm_panel", Flip::Front), ("fm_panel_back", Flip::Back)] {
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
