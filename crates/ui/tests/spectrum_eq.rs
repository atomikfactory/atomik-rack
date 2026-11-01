//! The Spectrum EQ window: where the points sit, what a drag emits (one undo step), the
//! modifiers, the checkboxes, the master (analyser only) and a golden of the painted window.
//! Regenerate the golden with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test spectrum_eq`.

mod support;

use atomik_model::{Command, Document, RackDoc};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::spectrum_eq::{PointId, SpectrumEqView, StripChoice, StripEqState};
use atomik_ui::{DisplayList, Modifiers, PointerButton, Theme, UiAction, UiEvent};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const CTRL: bool = !cfg!(target_os = "macos");

fn doc() -> RackDoc {
    let mut d = reference_doc();
    let mut ch = device("ch", "atomik.channel", "Channel");
    ch.params.insert("eq.on".into(), 1.0);
    ch.params.insert("eq.hmf.gain".into(), 6.0);
    ch.params.insert("eq.hmf.freq".into(), 2_500.0);
    ch.params.insert("eq.hmf.q".into(), 1.5);
    ch.params.insert("hpf.on".into(), 1.0);
    d.devices.push(ch);
    d.devices.push(device("master", "atomik.master", "Master"));
    d
}

fn choice(key_: &str, master: bool) -> StripChoice {
    StripChoice {
        key: key(key_),
        name: if master { "MASTER" } else { "Drums" }.into(),
        master,
    }
}

fn state(d: &RackDoc, k: &str, master: bool) -> StripEqState {
    let reg = registry();
    let dev = d.devices.iter().find(|x| x.key.0 == k).unwrap();
    let desc = reg.descriptor(&dev.type_id).unwrap();
    StripEqState::read(&choice(k, master), desc, dev, 48_000.0)
}

fn view(d: &RackDoc) -> SpectrumEqView {
    let mut v = SpectrumEqView::new(Theme::dark());
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 760.0, 440.0));
    v.set_strips(vec![choice("ch", false), choice("master", true)]);
    v.set_state(Some(state(d, "ch", false)));
    v
}

fn mods(shift: bool, alt: bool, command: bool) -> Modifiers {
    Modifiers {
        shift,
        alt,
        ctrl: command && CTRL,
        meta: command && !CTRL,
    }
}

fn down(v: &mut SpectrumEqView, pos: Point, m: Modifiers) -> Vec<UiAction> {
    v.handle(&UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: m,
        clicks: 1,
    })
}

fn mv(v: &mut SpectrumEqView, pos: Point, m: Modifiers) -> Vec<UiAction> {
    v.handle(&UiEvent::PointerMove { pos, modifiers: m })
}

fn up(v: &mut SpectrumEqView, pos: Point) -> Vec<UiAction> {
    v.handle(&UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::default(),
    })
}

fn commands(actions: &[UiAction]) -> Vec<&Command> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// The `SetParam`s inside a command (a batch is flattened).
fn set_params(c: &Command) -> Vec<(String, f32, Option<u64>)> {
    match c {
        Command::SetParam {
            param,
            value,
            merge,
            ..
        } => vec![(param.clone(), *value, *merge)],
        Command::Batch(list) => list.iter().flat_map(set_params).collect(),
        other => panic!("not a param command: {other:?}"),
    }
}

#[test]
fn the_points_sit_at_their_frequency_and_gain() {
    let d = doc();
    let v = view(&d);
    let plot = v.plot();
    let p = v.point(PointId::band(1)).unwrap();
    let x = plot.x0 + plot.width() * ((2_500.0f64 / 20.0).ln() / (1000.0f64).ln());
    assert!((p.x - x).abs() < 0.01, "{} vs {x}", p.x);
    // +6 dB of a +-24 dB axis: a quarter of the half height above the middle.
    let y = plot.center().y - 6.0 / 24.0 * plot.height() / 2.0;
    assert!((p.y - y).abs() < 0.01);
}

#[test]
fn dragging_the_hmf_point_is_one_undo_step_and_both_params_move() {
    let d = doc();
    let reg = registry();
    let mut v = view(&d);
    let mut document = Document::new(d.clone());
    let start = v.point(PointId::band(1)).unwrap();
    let target = Point::new(start.x + 80.0, start.y - 30.0);
    let mut all = down(&mut v, start, mods(false, false, false));
    assert!(all.is_empty(), "a press alone changes nothing");
    for i in 1..=8 {
        let t = i as f64 / 8.0;
        let p = Point::new(
            start.x + (target.x - start.x) * t,
            start.y + (target.y - start.y) * t,
        );
        all.extend(mv(&mut v, p, mods(false, false, false)));
    }
    all.extend(up(&mut v, target));
    assert_eq!(all.last(), Some(&UiAction::EndMerge));
    let cmds = commands(&all);
    assert!(!cmds.is_empty());
    let mut merge = None;
    for c in &cmds {
        let params = set_params(c);
        let keys: Vec<&str> = params.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(keys, ["eq.hmf.freq", "eq.hmf.gain"], "always both");
        for (_, _, m) in &params {
            assert!(m.is_some());
            assert!(merge.is_none_or(|x| Some(x) == *m), "one merge id");
            merge = *m;
        }
        document.apply((*c).clone(), &reg).unwrap();
    }
    document.end_merge();
    let hmf = |doc: &RackDoc, k: &str| {
        *doc.devices
            .iter()
            .find(|x| x.key.0 == "ch")
            .unwrap()
            .params
            .get(k)
            .unwrap()
    };
    assert!(hmf(document.doc(), "eq.hmf.freq") > 2_500.0);
    assert!(hmf(document.doc(), "eq.hmf.gain") > 6.0);
    assert_eq!(document.undo_depth(), 1, "one undo step");
    document.undo(&reg);
    assert_eq!(hmf(document.doc(), "eq.hmf.freq"), 2_500.0);
    assert_eq!(hmf(document.doc(), "eq.hmf.gain"), 6.0);
}

#[test]
fn shift_locks_a_drag_to_one_axis() {
    let d = doc();
    let mut v = view(&d);
    let start = v.point(PointId::band(1)).unwrap();
    down(&mut v, start, Modifiers::default());
    let m = mods(true, false, false);
    mv(&mut v, Point::new(start.x + 1.0, start.y), m);
    let out = mv(&mut v, Point::new(start.x + 60.0, start.y - 40.0), m);
    let params = set_params(commands(&out)[0]);
    let gain = params.iter().find(|p| p.0 == "eq.hmf.gain").unwrap().1;
    let freq = params.iter().find(|p| p.0 == "eq.hmf.freq").unwrap().1;
    assert_eq!(gain, 6.0, "the vertical axis is locked out");
    assert!(freq > 2_500.0);
}

#[test]
fn alt_drag_changes_only_the_q_of_a_bell() {
    let d = doc();
    let mut v = view(&d);
    let start = v.point(PointId::band(1)).unwrap();
    down(&mut v, start, mods(false, true, false));
    let out = mv(
        &mut v,
        Point::new(start.x + 20.0, start.y - 60.0),
        mods(false, true, false),
    );
    let params = set_params(commands(&out)[0]);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].0, "eq.hmf.q");
    assert!(params[0].1 > 1.5, "dragging up raises the Q");
    // A shelf has no Q to drag: the Alt press is an ordinary drag.
    let mut v = view(&d);
    let lf = v.point(PointId::band(3)).unwrap();
    down(&mut v, lf, mods(false, true, false));
    let out = mv(
        &mut v,
        Point::new(lf.x + 40.0, lf.y),
        mods(false, true, false),
    );
    assert_eq!(set_params(commands(&out)[0]).len(), 2);
}

#[test]
fn a_ctrl_click_resets_the_gain_and_a_filter_corner_drag_moves_the_frequency() {
    let d = doc();
    let mut v = view(&d);
    let p = v.point(PointId::band(1)).unwrap();
    let out = down(&mut v, p, mods(false, false, true));
    let params = set_params(commands(&out)[0]);
    assert_eq!(params, [("eq.hmf.gain".to_string(), 0.0, None)]);
    assert!(!v.dragging(), "a reset is a click, not a drag");

    let hpf = v.point(PointId::HPF).unwrap();
    down(&mut v, hpf, Modifiers::default());
    let out = mv(
        &mut v,
        Point::new(hpf.x + 50.0, hpf.y + 20.0),
        Modifiers::default(),
    );
    let params = set_params(commands(&out)[0]);
    assert_eq!(params.len(), 1, "a filter has no gain");
    assert_eq!(params[0].0, "hpf.freq");
    assert!(params[0].1 > 80.0);
}

#[test]
fn escape_puts_the_values_back_and_ends_the_step() {
    let d = doc();
    let mut v = view(&d);
    let start = v.point(PointId::band(1)).unwrap();
    down(&mut v, start, Modifiers::default());
    mv(
        &mut v,
        Point::new(start.x + 40.0, start.y + 25.0),
        Modifiers::default(),
    );
    let out = v.handle(&UiEvent::Key {
        key: atomik_ui::Key::Escape,
        modifiers: Modifiers::default(),
        pressed: true,
    });
    let params = set_params(commands(&out)[0]);
    assert_eq!(params[0], ("eq.hmf.freq".to_string(), 2_500.0, params[0].2));
    assert_eq!(params[1], ("eq.hmf.gain".to_string(), 6.0, params[1].2));
    assert_eq!(out.last(), Some(&UiAction::EndMerge));
    assert!(!v.dragging());
}

#[test]
fn the_checkboxes_the_arrows_and_follow_emit_their_actions() {
    let d = doc();
    let mut v = view(&d);
    // Click the middle of the bar's controls by scanning for what they do.
    let mut seen = Vec::new();
    for x in (0..760).step_by(3) {
        let mut probe = view(&d);
        for a in down(
            &mut probe,
            Point::new(f64::from(x), 16.0),
            Modifiers::default(),
        ) {
            if !seen.contains(&a) {
                seen.push(a);
            }
        }
    }
    let names: Vec<String> = seen
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(c) => Some(set_params(c)[0].0.clone()),
            _ => None,
        })
        .collect();
    for want in ["eq.on", "hpf.on", "lpf.on", "eq.hf.bell", "eq.lf.bell"] {
        assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
    }
    assert!(seen.contains(&UiAction::SpectrumEqStep(-1)));
    assert!(seen.contains(&UiAction::SpectrumEqStep(1)));
    assert!(
        seen.contains(&UiAction::SpectrumEqFollow(false)),
        "follow starts on"
    );
    // The EQ box is a toggle: on now, so the click switches it off.
    let eq_off = seen.iter().any(|a| match a {
        UiAction::Command(c) => set_params(c)[0] == ("eq.on".to_string(), 0.0, None),
        _ => false,
    });
    assert!(eq_off);
    v.set_follow(false);
    assert!(!v.follow());
}

#[test]
fn the_master_shows_the_analyser_only() {
    let d = doc();
    let mut v = view(&d);
    v.set_state(Some(state(&d, "master", true)));
    assert!(v.point(PointId::band(1)).is_none());
    // No point and no checkbox answers a click anywhere on the plot or the bar's right side.
    for x in (300..760).step_by(5) {
        let out = down(&mut v, Point::new(f64::from(x), 16.0), Modifiers::default());
        assert!(commands(&out).is_empty(), "x = {x}");
    }
    let plot = v.plot();
    let out = down(&mut v, plot.center(), Modifiers::default());
    assert!(out.is_empty());
}

#[test]
fn a_wheel_over_a_bell_changes_its_q() {
    let d = doc();
    let mut v = view(&d);
    let p = v.point(PointId::band(1)).unwrap();
    let out = v.handle(&UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
        pos: p,
        modifiers: Modifiers::default(),
    });
    let params = set_params(commands(&out)[0]);
    assert_eq!(params[0].0, "eq.hmf.q");
    assert!(params[0].1 > 1.5);
}

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

#[test]
fn the_window_paints_the_trace_the_curve_and_the_points() {
    let d = doc();
    let mut v = view(&d);
    let trace: Vec<f32> = (0..240)
        .map(|i| -30.0 - 40.0 * (i as f32 / 239.0) - 8.0 * ((i as f32) * 0.3).sin())
        .collect();
    v.set_trace(&trace, &[]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let texts: Vec<&str> = dl.texts();
    for want in [
        "HF",
        "HMF",
        "LMF",
        "LF",
        "HPF",
        "LPF",
        "Drums",
        "EQ",
        "FOLLOW SELECTION",
    ] {
        assert!(texts.contains(&want), "{want} in {texts:?}");
    }
    golden("spectrum_eq_channel.json", &dl.to_json());
    // The master: no EQ texts, the same trace.
    v.set_state(Some(state(&d, "master", true)));
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let texts: Vec<&str> = dl.texts();
    assert!(
        !texts.contains(&"HMF") && !texts.contains(&"EQ"),
        "{texts:?}"
    );
    golden("spectrum_eq_master.json", &dl.to_json());
}

#[test]
fn an_empty_window_says_so_and_a_resize_moves_the_plot() {
    let mut v = SpectrumEqView::new(Theme::dark());
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().contains(&"No strip to show"));
    let before = v.plot();
    v.handle(&UiEvent::Resize(Size::new(1000.0, 600.0)));
    assert!(v.plot().width() > before.width() && v.plot().height() > before.height());
}
