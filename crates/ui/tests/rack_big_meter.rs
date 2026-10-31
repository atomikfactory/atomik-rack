//! The Big Meter overlay of the rack view: shown from the View switch, fed by the per-socket
//! peaks the app sets, moved by the frame clock, with its pair, mode, hold and clip buttons.
//! The golden regenerates with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test
//! rack_big_meter`.

mod support;

use atomik_ui::big_meter::{BigHit, HoldMode, Layout, MeterMode, pairs};
use atomik_ui::{RackView, UiAction};
use support::starter::*;
use support::*;

fn setup() -> (atomik_model::Document, atomik_device_sdk::Registry) {
    let reg = registry();
    (starter(), reg)
}

fn press(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: atomik_ui::kurbo::Point,
) -> Vec<UiAction> {
    let a = v.handle(&down(p), doc, reg);
    let _ = v.handle(&up(p), doc, reg);
    a
}

/// Ticks the view `frames` times at 60 Hz from `start`, feeding `peaks` each frame.
fn feed(v: &mut RackView, peaks: &[f32], start: f64, frames: usize) -> f64 {
    let mut t = start;
    for _ in 0..frames {
        v.set_io_peaks(peaks);
        t += 1.0 / 60.0;
        v.tick(t);
    }
    t
}

#[test]
fn the_overlay_is_off_until_switched_on_and_leaves_the_rack_alone() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    assert!(!v.big_meter_visible() && v.big_meter_rect().is_none());
    let off = paint_json(&mut v, doc, &reg);
    v.set_big_meter_visible(true);
    assert!(v.big_meter_rect().is_some());
    let on = paint_json(&mut v, doc, &reg);
    assert_ne!(on, off);
    check_golden("rack_big_meter_1x.json", &on);
    v.set_big_meter_visible(false);
    assert_eq!(paint_json(&mut v, doc, &reg), off);
}

#[test]
fn the_peaks_of_the_shown_pair_move_the_levels_with_the_frame_clock() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_big_meter_visible(true);
    let n = v.io_socket_count();
    assert!(n >= 4, "the starter has two inputs and two outputs: {n}");
    // Input pair 1-2 is shown first. Left at -6 dBFS, right silent.
    let mut peaks = vec![0.0; n];
    peaks[0] = 0.5;
    let t = feed(&mut v, &peaks, 0.0, 120);
    let m = v.big_meter();
    assert_eq!(m.mode, MeterMode::Vu);
    assert!((m.channels[0].db + 6.0).abs() < 0.3, "{}", m.channels[0].db);
    assert!(m.channels[1].db < -50.0);
    // A hot peak on the other pair does not touch this one.
    let mut other = vec![0.0; n];
    other[2] = 1.5;
    other[3] = 1.5;
    feed(&mut v, &other, t, 10);
    assert!(!v.big_meter().channels[0].clip);
}

#[test]
fn the_buttons_change_the_pair_the_mode_the_hold_and_the_reference() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_big_meter_visible(true);
    let origin = v.big_meter_rect().unwrap().origin();
    let l = Layout::at(origin);
    let (i, o) = (
        usize::from(doc.input_channels),
        usize::from(doc.output_channels),
    );
    let count = pairs(i, o).len();
    press(&mut v, doc, &reg, l.next().center());
    assert_eq!(v.big_meter().pair, 1 % count);
    press(&mut v, doc, &reg, l.prev().center());
    assert_eq!(v.big_meter().pair, 0);
    press(&mut v, doc, &reg, l.prev().center());
    assert_eq!(v.big_meter().pair, count - 1, "wraps around");
    press(&mut v, doc, &reg, l.mode(MeterMode::Ppm).center());
    assert_eq!(v.big_meter().mode, MeterMode::Ppm);
    press(&mut v, doc, &reg, l.hold(HoldMode::Infinite).center());
    assert_eq!(v.big_meter().hold, HoldMode::Infinite);
    // The reference buttons only exist on the VU scale.
    let before = v.big_meter().vu_reference_db;
    press(&mut v, doc, &reg, l.reference_up().center());
    assert_eq!(
        v.big_meter().vu_reference_db,
        before,
        "PPM has no reference"
    );
    press(&mut v, doc, &reg, l.mode(MeterMode::Vu).center());
    press(&mut v, doc, &reg, l.reference_up().center());
    assert_eq!(v.big_meter().vu_reference_db, before + 1.0);
    press(&mut v, doc, &reg, l.reference_down().center());
    press(&mut v, doc, &reg, l.reference_down().center());
    assert_eq!(v.big_meter().vu_reference_db, before - 1.0);
}

#[test]
fn a_clip_lamp_latches_and_a_press_on_it_resets_it() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_big_meter_visible(true);
    let n = v.io_socket_count();
    let mut peaks = vec![0.0; n];
    peaks[0] = 1.2;
    peaks[1] = 1.2;
    let t = feed(&mut v, &peaks, 0.0, 3);
    feed(&mut v, &vec![0.0; n], t, 30);
    assert!(v.big_meter().channels[0].clip && v.big_meter().channels[1].clip);
    let l = Layout::at(v.big_meter_rect().unwrap().origin());
    press(&mut v, doc, &reg, l.clip(1).center());
    assert!(v.big_meter().channels[0].clip && !v.big_meter().channels[1].clip);
}

#[test]
fn a_press_on_the_overlay_never_reaches_the_rack_and_a_hidden_one_takes_none() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_big_meter_visible(true);
    let rect = v.big_meter_rect().unwrap();
    let probe = atomik_ui::kurbo::Point::new(rect.x0 + 3.0, rect.y0 + 3.0);
    assert_eq!(v.big_meter().hit(rect.origin(), probe), Some(BigHit::Body));
    assert!(press(&mut v, doc, &reg, probe).is_empty());
    v.set_big_meter_visible(false);
    assert_eq!(v.big_meter_rect(), None);
}

#[test]
fn a_hidden_overlay_costs_the_feed_nothing_and_a_new_pair_starts_clean() {
    let (d, reg) = setup();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    let n = v.io_socket_count();
    let mut peaks = vec![0.0; n];
    peaks[0] = 0.9;
    feed(&mut v, &peaks, 0.0, 30);
    assert_eq!(v.big_meter().channels[0].db, -60.0, "hidden: not fed");
    v.set_big_meter_visible(true);
    let t = feed(&mut v, &peaks, 1.0, 120);
    assert!(v.big_meter().channels[0].db > -3.0);
    let l = Layout::at(v.big_meter_rect().unwrap().origin());
    press(&mut v, doc, &reg, l.next().center());
    assert_eq!(v.big_meter().channels[0].db, -60.0);
    let _ = t;
}
