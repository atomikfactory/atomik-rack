//! The wavetable editor panel: the frame list, drawing, the harmonic bars, the processes, the
//! phase-safe toggle and its warnings, the cleanup row, one commit per gesture, Escape, tooltips,
//! and the table displays of the synth panel that open it.

mod support;

use atomik_dsp::wavetable::edit::{self, BINS, Bin, FRAME_LEN, Frame, MAX_KEYS};
use atomik_model::RackDoc;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::wavetable_panel::WAVETABLE_TYPE;
use atomik_ui::{
    DisplayList, Key, Modifiers, RackView, Theme, UiAction, UiEvent, WavetableAction,
    WavetableData, WavetableView,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 960.0, 700.0);

fn bins(f: impl Fn(usize) -> f32) -> Vec<Bin> {
    (1..=BINS)
        .map(|h| Bin {
            level: f(h),
            phase: 0.0,
        })
        .collect()
}

fn saw() -> Frame {
    edit::rebuild(&bins(|h| 1.0 / h as f32))
}

fn sine() -> Frame {
    edit::rebuild(&bins(|h| if h == 1 { 1.0 } else { 0.0 }))
}

fn data(keys: Vec<Frame>) -> WavetableData {
    WavetableData {
        device: "Wavetable Synth".into(),
        keys,
        phase_safe: true,
        stored: true,
        imported: false,
        cycle: None,
        plays: true,
    }
}

fn view(keys: Vec<Frame>) -> WavetableView {
    let mut v = WavetableView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(data(keys));
    v
}

fn only(actions: Vec<UiAction>) -> Vec<WavetableAction> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            UiAction::Wavetable(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn click(v: &mut WavetableView, p: Point) -> Vec<WavetableAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    only(out)
}

fn tap(v: &mut WavetableView, rect: fn(&WavetableView) -> Rect) -> Vec<WavetableAction> {
    let at = rect(v).center();
    click(v, at)
}

fn commit_keys(a: &[WavetableAction]) -> (Vec<Frame>, bool) {
    match a {
        [WavetableAction::Commit { keys, phase_safe }] => (keys.clone(), *phase_safe),
        other => panic!("one commit expected, got {} actions", other.len()),
    }
}

fn texts(v: &WavetableView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn key_press(v: &mut WavetableView, key: Key) -> Vec<WavetableAction> {
    only(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn the_panel_shows_the_frames_the_controls_and_the_morph_line() {
    let v = view(vec![saw(), sine(), saw()]);
    let t = texts(&v);
    for s in [
        "WAVETABLE EDITOR",
        "Wavetable Synth",
        "PHASE-SAFE",
        "ADD",
        "DUP",
        "DEL",
        "NORM",
        "SMOOTH",
        "ALL",
        "CUT",
        "DEDUPE",
        "EDGE",
        "128",
        "FRAME 1 OF 3",
        "Drawn table",
        "OSC 1 PLAYS IT",
    ] {
        assert!(t.iter().any(|x| x == s), "{s} in {t:?}");
    }
    assert!(t.iter().any(|x| x.starts_with("Morph level dip")), "{t:?}");
    assert_eq!(v.keys().len(), 3);
    assert_eq!(v.bins().len(), BINS);
    // Nothing is drawn outside the panel.
    let p = v.panel();
    assert!(
        p.x0 >= 0.0 && p.x1 <= BOUNDS.x1 && p.y1 <= BOUNDS.y1,
        "{p:?}"
    );
}

#[test]
fn an_empty_table_says_it_is_loading_and_takes_no_edit() {
    let mut v = view(Vec::new());
    assert!(texts(&v).iter().any(|s| s == "Loading the sample"));
    assert!(tap(&mut v, WavetableView::add_rect).is_empty());
    assert!(tap(&mut v, WavetableView::canvas_rect).is_empty());
    assert!(tap(&mut v, WavetableView::norm_rect).is_empty());
    let mut fresh = data(vec![sine()]);
    fresh.stored = false;
    v.set_data(fresh);
    assert!(texts(&v).iter().any(|s| s.starts_with("Not stored yet")));
}

#[test]
fn the_frame_list_adds_duplicates_deletes_and_reorders() {
    let a = saw();
    let b = sine();
    let mut v = view(vec![a.clone(), b.clone()]);
    // ADD puts the blend of the selected frame and the next after it and selects it.
    let (keys, safe) = commit_keys(&tap(&mut v, WavetableView::add_rect));
    assert!(safe);
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0], a);
    assert_eq!(keys[2], b);
    assert!(close(keys[1][100], 0.5 * (a[100] + b[100]), 1.0e-6));
    assert_eq!(v.selected(), 1);
    // DUP copies the selected frame after it.
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::dup_rect));
    assert_eq!(keys.len(), 4);
    assert_eq!(keys[1], keys[2]);
    assert_eq!(v.selected(), 2);
    // UP and DOWN swap with the neighbour.
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::down_rect));
    assert_eq!(
        keys[3], keys[1],
        "the copy went last, the sine moved before it"
    );
    assert_eq!(v.selected(), 3);
    assert!(
        tap(&mut v, WavetableView::down_rect).is_empty(),
        "already last"
    );
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::up_rect));
    assert_eq!(v.selected(), 2);
    assert_eq!(keys.len(), 4);
    // DEL removes the selected frame; the last one stays.
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::del_rect));
    assert_eq!(keys.len(), 3);
    assert!(v.selected() < 3);
    let mut one = view(vec![a]);
    assert!(tap(&mut one, WavetableView::del_rect).is_empty());
}

#[test]
fn the_list_never_grows_past_the_synths_frame_count() {
    let keys: Vec<Frame> = (0..MAX_KEYS).map(|_| saw()).collect();
    let mut v = view(keys);
    assert!(tap(&mut v, WavetableView::add_rect).is_empty());
    assert!(tap(&mut v, WavetableView::dup_rect).is_empty());
    assert_eq!(v.keys().len(), MAX_KEYS);
    // Every row is inside the panel.
    let last = v.row_rect(MAX_KEYS - 1);
    assert!(last.y1 <= v.panel().y1, "{last:?}");
    let at = v.row_rect(9).center();
    assert!(click(&mut v, at).is_empty(), "selecting commits nothing");
    assert_eq!(v.selected(), 9);
    assert!(key_press(&mut v, Key::Up).is_empty());
    assert_eq!(v.selected(), 8);
    let _ = key_press(&mut v, Key::Down);
    assert_eq!(v.selected(), 9);
}

#[test]
fn a_stroke_in_the_box_draws_and_commits_once_on_release() {
    let mut v = view(vec![sine(), saw()]);
    let c = v.canvas_rect();
    let y = |value: f64| c.y0 + (1.0 - value) / 2.0 * c.height();
    let x = |share: f64| c.x0 + share * c.width();
    assert!(v.handle(&down(Point::new(x(0.25), y(0.0)))).is_empty());
    assert!(v.dragging());
    assert!(v.handle(&mv(Point::new(x(0.5), y(1.0)))).is_empty());
    assert!(v.handle(&mv(Point::new(x(0.75), y(-1.0)))).is_empty());
    let a = only(v.handle(&up(Point::new(x(0.75), y(-1.0)))));
    assert!(!v.dragging());
    let (keys, _) = commit_keys(&a);
    let frame = &keys[0];
    let at = |share: f64| ((share * (FRAME_LEN - 1) as f64).round() as usize).min(FRAME_LEN - 1);
    assert!(close(frame[at(0.25)], 0.0, 0.02));
    assert!(close(frame[at(0.5)], 1.0, 0.02));
    assert!(close(frame[at(0.75)], -1.0, 0.02));
    assert!(
        close(frame[at(0.375)], 0.5, 0.05),
        "a line between the points"
    );
    assert_eq!(keys[1], saw(), "the other frame is untouched");
    // The bars follow the drawn frame.
    assert!(v.bins()[0].level > 0.0);
}

#[test]
fn a_press_without_a_move_that_changes_nothing_commits_nothing() {
    let mut v = view(vec![sine()]);
    let c = v.canvas_rect();
    // A press on the line the sine already follows at that point still sets that sample; but a
    // press that leaves the frame as it was (clicking exactly on it) is not an edit.
    let at = Point::new(c.x0, c.y0 + c.height() / 2.0);
    let _ = v.handle(&down(at));
    let a = only(v.handle(&up(at)));
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn escape_during_a_stroke_puts_the_frame_back() {
    let before = sine();
    let mut v = view(vec![before.clone()]);
    let c = v.canvas_rect();
    let _ = v.handle(&down(Point::new(c.x0 + 100.0, c.y0 + 10.0)));
    let _ = v.handle(&mv(Point::new(c.x0 + 300.0, c.y0 + 150.0)));
    assert_ne!(v.keys()[0], before);
    assert!(key_press(&mut v, Key::Escape).is_empty());
    assert!(!v.dragging());
    assert_eq!(v.keys()[0], before);
    assert!(only(v.handle(&up(Point::new(c.x0 + 300.0, c.y0 + 150.0)))).is_empty());
}

#[test]
fn an_edit_the_host_refused_is_put_back_by_the_next_data() {
    let stored = data(vec![sine(), saw()]);
    let mut v = WavetableView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(stored.clone());
    let _ = tap(&mut v, WavetableView::invert_rect);
    assert_ne!(
        v.keys()[0],
        stored.keys[0],
        "the panel is ahead of the stored table"
    );
    // The host stored nothing: it hands the same data again.
    v.set_data(stored.clone());
    assert_eq!(v.keys(), &stored.keys[..]);
    assert_eq!(v.selected(), 0);
}

#[test]
fn the_host_cannot_replace_the_frames_under_a_gesture() {
    let mut v = view(vec![sine()]);
    let c = v.canvas_rect();
    let _ = v.handle(&down(Point::new(c.x0 + 50.0, c.y0 + 20.0)));
    let drawn = v.keys()[0].clone();
    v.set_data(data(vec![saw(), saw()]));
    assert_eq!(v.keys().len(), 1);
    assert_eq!(v.keys()[0], drawn);
    let _ = v.handle(&up(Point::new(c.x0 + 50.0, c.y0 + 20.0)));
    v.set_data(data(vec![saw(), saw()]));
    assert_eq!(v.keys().len(), 2);
}

#[test]
fn dragging_the_bars_sets_levels_and_leaves_the_other_harmonics_alone() {
    let mut v = view(vec![saw()]);
    let before = v.bins().to_vec();
    let l = v.level_rect();
    let pitch = l.width() / BINS as f64;
    let bar = |h: usize| l.x0 + (h as f64 - 0.5) * pitch;
    // Harmonic 3 to half the scale, then across to 4 at the top.
    let _ = v.handle(&down(Point::new(bar(3), l.y1 - 0.5 * l.height())));
    assert!(close(v.bins()[2].level, 0.75, 0.01), "{:?}", v.bins()[2]);
    let _ = v.handle(&mv(Point::new(bar(4), l.y0)));
    assert!(close(v.bins()[3].level, 1.5, 0.01));
    let a = only(v.handle(&up(Point::new(bar(4), l.y0))));
    let (keys, _) = commit_keys(&a);
    let after = edit::analyse(&keys[0], BINS);
    for h in 0..BINS {
        if h == 2 || h == 3 {
            continue;
        }
        assert!(
            close(before[h].level, after[h].level, 1.0e-3),
            "harmonic {}",
            h + 1
        );
    }
    assert!(close(after[2].level, 0.75, 0.01) && close(after[3].level, 1.5, 0.01));
    // A level bar keeps the harmonic's phase.
    assert!(close(after[2].phase, before[2].phase, 0.01));
}

#[test]
fn dragging_the_dots_sets_phases() {
    let mut v = view(vec![saw()]);
    let p = v.phase_rect();
    let pitch = p.width() / BINS as f64;
    let at = Point::new(p.x0 + 1.5 * pitch, p.y0 + 0.25 * p.height());
    let _ = v.handle(&down(at));
    let a = only(v.handle(&up(at)));
    let (keys, _) = commit_keys(&a);
    let bins = edit::analyse(&keys[0], BINS);
    assert!(close(bins[1].phase, 0.25, 0.01), "{:?}", bins[1]);
    assert!(close(bins[1].level, 0.5, 0.01), "the level stays");
}

#[test]
fn the_processes_act_on_the_selected_frame_or_on_all() {
    let mut loud = sine();
    for x in &mut loud {
        *x *= 0.25;
    }
    let mut v = view(vec![loud.clone(), loud.clone()]);
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::norm_rect));
    let peak = |f: &Frame| f.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(close(peak(&keys[0]), 1.0, 1.0e-5));
    assert!(
        close(peak(&keys[1]), 0.25, 1.0e-5),
        "only the selected frame"
    );
    // ALL toggles without a commit and then every frame is processed.
    assert!(tap(&mut v, WavetableView::all_rect).is_empty());
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::invert_rect));
    assert!(close(
        keys[0][50],
        -edit::rebuild(&bins(|h| if h == 1 { 1.0 } else { 0.0 }))[50],
        1.0e-4
    ));
    assert!(close(keys[1][50], -loud[50], 1.0e-6));
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::reverse_rect));
    assert!(close(keys[1][0], -loud[FRAME_LEN - 1], 1.0e-6));
    let _ = commit_keys(&tap(&mut v, WavetableView::smooth_rect));
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::dc_rect));
    for f in &keys {
        assert!(f.iter().map(|&x| f64::from(x)).sum::<f64>().abs() < 1.0e-2);
    }
}

#[test]
fn the_phase_safe_toggle_commits_and_the_warnings_name_the_pairs() {
    // A saw and the same saw half a cycle later: the fundamental cancels half way.
    let a = saw();
    let flipped: Frame = (0..FRAME_LEN)
        .map(|n| a[(n + FRAME_LEN / 2) % FRAME_LEN])
        .collect();
    let mut off = data(vec![a.clone(), flipped]);
    off.phase_safe = false;
    let mut v = WavetableView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(off);
    assert!(!v.phase_safe());
    assert_eq!(v.warnings().len(), 1);
    let t = texts(&v);
    assert!(
        t.iter()
            .any(|s| s.starts_with("Frames 1 and 2: the fundamental cancels")),
        "{t:?}"
    );
    assert!(v.dip_db() > 4.0, "{}", v.dip_db());
    // Turning it on commits the table with the flag and clears the warning and the dip.
    let (keys, safe) = commit_keys(&tap(&mut v, WavetableView::safe_rect));
    assert!(safe && v.phase_safe());
    assert_eq!(keys.len(), 2);
    assert!(v.warnings().is_empty());
    assert!(v.dip_db() < 1.0, "{}", v.dip_db());
    assert!(texts(&v).iter().any(|s| s.starts_with("Morph level dip")));
    // And off again.
    let (_, safe) = commit_keys(&tap(&mut v, WavetableView::safe_rect));
    assert!(!safe);
}

#[test]
fn a_frame_without_a_fundamental_is_named_when_phase_safe_is_on() {
    let hollow = edit::rebuild(&bins(|h| if h == 3 { 0.5 } else { 0.0 }));
    let v = view(vec![saw(), hollow]);
    let t = texts(&v);
    assert!(
        t.iter()
            .any(|s| s == "Frame 2 has no fundamental: phase-safe cannot align it"),
        "{t:?}"
    );
}

#[test]
fn the_cleanup_row_reads_the_cycle_and_its_buttons_work() {
    let mut d = data(vec![saw(), saw(), sine()]);
    d.imported = true;
    d.cycle = Some((441.0, 100.0));
    d.plays = false;
    let mut v = WavetableView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(d.clone());
    let t = texts(&v);
    assert!(
        t.iter().any(|s| s == "CYCLE 441.0 samples (100 Hz)"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "USE OSC 1"));
    assert_eq!(
        tap(&mut v, WavetableView::cut_rect),
        vec![WavetableAction::CutCycles]
    );
    assert_eq!(
        tap(&mut v, WavetableView::use_rect),
        vec![WavetableAction::UseOnOsc1]
    );
    // DEDUPE drops the repeated frame.
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::dedupe_rect));
    assert_eq!(keys.len(), 2);
    // Nothing to drop: no commit.
    v.set_data(WavetableData {
        keys: vec![saw(), sine()],
        ..d.clone()
    });
    assert!(tap(&mut v, WavetableView::dedupe_rect).is_empty());
    // EDGE closes the jump at the cycle ends; the amount steps through its choices.
    let ramp: Frame = (0..FRAME_LEN)
        .map(|n| (std::f64::consts::TAU * 1.25 * n as f64 / FRAME_LEN as f64).sin() as f32)
        .collect();
    v.set_data(WavetableData {
        keys: vec![ramp.clone()],
        ..d.clone()
    });
    let jump = |f: &Frame| (f[0] - f[FRAME_LEN - 1]).abs();
    assert_eq!(v.edge_overlap(), 128);
    for want in [256, 512, 32, 64, 128] {
        assert!(tap(&mut v, WavetableView::edge_amount_rect).is_empty());
        assert_eq!(v.edge_overlap(), want);
    }
    let _ = tap(&mut v, WavetableView::edge_amount_rect);
    let _ = tap(&mut v, WavetableView::edge_amount_rect);
    assert_eq!(v.edge_overlap(), 512);
    let (keys, _) = commit_keys(&tap(&mut v, WavetableView::edge_rect));
    assert!(
        jump(&keys[0]) < 0.5 * jump(&ramp),
        "{} vs {}",
        jump(&keys[0]),
        jump(&ramp)
    );
    // A table that is no import cannot be cut, and a playing table needs no USE.
    let mut v = view(vec![saw()]);
    assert!(tap(&mut v, WavetableView::cut_rect).is_empty());
    assert!(tap(&mut v, WavetableView::use_rect).is_empty());
    // No cycle found: CUT does nothing and the readout says so.
    let mut none = d;
    none.cycle = None;
    v.set_data(none);
    assert!(texts(&v).iter().any(|s| s == "CYCLE not found"));
    assert!(tap(&mut v, WavetableView::cut_rect).is_empty());
}

#[test]
fn close_and_the_controls_have_tooltips() {
    let mut v = view(vec![saw(), sine()]);
    assert_eq!(
        tap(&mut v, WavetableView::close_rect),
        vec![WavetableAction::Close]
    );
    for r in [
        v.close_rect(),
        v.safe_rect(),
        v.add_rect(),
        v.dup_rect(),
        v.del_rect(),
        v.up_rect(),
        v.down_rect(),
        v.canvas_rect(),
        v.level_rect(),
        v.phase_rect(),
        v.norm_rect(),
        v.dc_rect(),
        v.invert_rect(),
        v.reverse_rect(),
        v.smooth_rect(),
        v.all_rect(),
        v.cut_rect(),
        v.dedupe_rect(),
        v.edge_rect(),
        v.edge_amount_rect(),
        v.use_rect(),
        v.row_rect(1),
    ] {
        let help = v
            .help_at(r.center())
            .unwrap_or_else(|| panic!("no tooltip at {r:?}"));
        assert!(!help.text.is_empty());
        assert!(help.text.len() <= 100, "{}", help.text);
    }
    assert!(v.help_at(Point::new(1.0, 1.0)).is_none());
    assert!(
        v.help_at(v.row_rect(5).center()).is_none(),
        "no frame there"
    );
}

#[test]
fn every_control_fits_inside_the_panel_and_overlaps_no_other() {
    let v = view(vec![saw()]);
    let p = v.panel();
    let rects = [
        ("close", v.close_rect()),
        ("safe", v.safe_rect()),
        ("add", v.add_rect()),
        ("dup", v.dup_rect()),
        ("del", v.del_rect()),
        ("up", v.up_rect()),
        ("down", v.down_rect()),
        ("canvas", v.canvas_rect()),
        ("level", v.level_rect()),
        ("phase", v.phase_rect()),
        ("norm", v.norm_rect()),
        ("dc", v.dc_rect()),
        ("inv", v.invert_rect()),
        ("rev", v.reverse_rect()),
        ("smooth", v.smooth_rect()),
        ("all", v.all_rect()),
        ("cut", v.cut_rect()),
        ("dedupe", v.dedupe_rect()),
        ("edge", v.edge_rect()),
        ("amount", v.edge_amount_rect()),
        ("use", v.use_rect()),
        ("list", v.row_rect(0).union(v.row_rect(MAX_KEYS - 1))),
    ];
    for (name, r) in &rects {
        assert!(
            r.x0 >= p.x0 && r.x1 <= p.x1 && r.y0 >= p.y0 && r.y1 <= p.y1,
            "{name} {r:?} outside {p:?}"
        );
    }
    for (i, (a, ra)) in rects.iter().enumerate() {
        for (b, rb) in &rects[i + 1..] {
            let hit = ra.x0 < rb.x1 && rb.x0 < ra.x1 && ra.y0 < rb.y1 && rb.y0 < ra.y1;
            assert!(!hit, "{a} {ra:?} overlaps {b} {rb:?}");
        }
    }
}

#[test]
fn the_panel_fits_a_small_window_and_the_bars_have_a_pixel_each() {
    let mut v = view(vec![saw()]);
    v.set_bounds(Rect::new(0.0, 0.0, 800.0, 560.0));
    let p = v.panel();
    assert!(p.x1 <= 800.0 && p.y1 <= 560.0, "{p:?}");
    assert!(v.level_rect().width() / BINS as f64 >= 4.0);
}

// ------------------------------------------------------------------ the synth panel

fn synth_setup() -> (RackDoc, RackView) {
    let mut doc = reference_doc();
    doc.devices = vec![device("wt", WAVETABLE_TYPE, "Wavetable Synth")];
    doc.cables.clear();
    let v = support::view(&doc, &registry());
    (doc, v)
}

#[test]
fn a_click_on_a_table_display_opens_the_editor() {
    let (doc, mut v) = synth_setup();
    let reg = registry();
    for osc in 0..2 {
        let at = v.table_display_center(&key("wt"), osc).expect("a display");
        let a = v.handle(&down(at), &doc, &reg);
        let opened: Vec<_> = a
            .iter()
            .filter(|a| matches!(a, UiAction::EditWavetable(k) if *k == key("wt")))
            .collect();
        assert_eq!(opened.len(), 1, "display {osc}: {a:?}");
        let _ = v.handle(&up(at), &doc, &reg);
    }
    let help = v.help_at(v.table_display_center(&key("wt"), 0).unwrap());
    assert!(help.is_some_and(|h| h.text.contains("table editor")));
}
