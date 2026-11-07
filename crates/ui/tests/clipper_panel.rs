//! The clipper panel: its transfer curve (the maths of the device), the clip activity readout
//! and its audibility colour, the meter feed and the pinned layout.

mod support;

use atomik_ui::fx::{
    CLIPPER_METERS, CLIPPER_RUN_SAFE_MS, FxKind, clipper_transfer, update_display,
};
use atomik_ui::{DisplayList, Flip};
use support::fx::*;
use support::*;

fn texts_with(meters: Option<&[f32]>) -> Vec<String> {
    let (doc, mut v) = rack(FxKind::Clipper);
    if let Some(m) = meters {
        v.set_device_meter_values(&key("fx"), m);
    }
    v.set_flip(Flip::Front);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn json_with(meters: &[f32]) -> String {
    let (doc, mut v) = rack(FxKind::Clipper);
    v.set_device_meter_values(&key("fx"), meters);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.to_json()
}

#[test]
fn the_curve_is_the_one_the_device_applies() {
    // Hard: a plain clamp in units of the ceiling.
    for x in [-2.0f32, -1.0, -0.4, 0.0, 0.7, 1.0, 1.6] {
        assert_eq!(clipper_transfer(0.0, 0.0, x), x.clamp(-1.0, 1.0), "x = {x}");
    }
    // Soft: untouched under the knee, below the line above it, never over the ceiling.
    assert_eq!(clipper_transfer(0.5, 0.0, 0.4), 0.4);
    let y = clipper_transfer(0.5, 0.0, 0.8);
    assert!(y < 0.8 && y > 0.5, "{y}");
    assert!(clipper_transfer(1.0, 0.0, 2.0) < 1.0);
    // Asymmetric: the positive half is lowered.
    assert_eq!(clipper_transfer(0.0, 1.0, 2.0), 0.5);
    assert_eq!(clipper_transfer(0.0, 1.0, -2.0), -1.0);
    // The device's own shaper gives the same numbers.
    let device = atomik_dsp::clipper::ClipCurve::new(1.0, 0.3, -0.4);
    for i in -20..=20 {
        let x = i as f32 * 0.1;
        assert_eq!(clipper_transfer(0.3, -0.4, x), device.apply(x));
    }
}

#[test]
fn the_meter_layout_matches_the_device() {
    assert_eq!(CLIPPER_METERS, atomik_devices::clipper::METERS);
    assert_eq!(FxKind::Clipper.meter_count(), CLIPPER_METERS);
    assert_eq!(atomik_devices::clipper::meter::DRIVEN, 0);
    assert_eq!(atomik_devices::clipper::meter::OUTPUT, 1);
    assert_eq!(atomik_devices::clipper::meter::SHARE, 2);
    assert_eq!(atomik_devices::clipper::meter::LONGEST, 3);
}

#[test]
fn the_share_and_the_run_are_shown_as_published_to_a_tenth() {
    let mut shown = Vec::new();
    assert!(update_display(
        FxKind::Clipper,
        &[0.9, 0.5, 18.46, 1.437],
        &mut shown
    ));
    assert_eq!(shown.len(), CLIPPER_METERS);
    assert_eq!(shown[2], 18.5);
    assert_eq!(shown[3], 1.4);
    // They fall at once: no release to mask a run that has gone.
    update_display(FxKind::Clipper, &[0.9, 0.5, 0.0, 0.0], &mut shown);
    assert_eq!((shown[2], shown[3]), (0.0, 0.0));
}

#[test]
fn the_panel_names_the_curve_and_reads_the_activity() {
    let t = texts_with(Some(&[0.9, 0.5, 18.5, 1.4]));
    for s in [
        "CURVE",
        "SOFT 40 %, ASYM",
        "-6.0 dB",
        "CLIPPED 18.5 %",
        "RUN 1.40 ms",
        "IN",
        "OUT",
    ] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
    // Without a feed the readout starts at nothing.
    let t = texts_with(None);
    assert!(t.iter().any(|x| x == "CLIPPED 0.0 %"), "{t:?}");
    assert!(t.iter().any(|x| x == "RUN 0.00 ms"), "{t:?}");
}

#[test]
fn a_run_past_the_safe_length_is_drawn_in_a_warning_colour() {
    let safe = json_with(&[0.9, 0.5, 5.0, CLIPPER_RUN_SAFE_MS * 0.5]);
    let long = json_with(&[0.9, 0.5, 5.0, CLIPPER_RUN_SAFE_MS * 3.0]);
    assert_ne!(safe, long);
    // The warning colour is on the run readout only: the share text keeps its colour.
    let colour_of = |json: &str, text: &str| {
        let at = json.find(&format!("\"text\":\"{text}")).expect(text);
        let line = &json[at..json[at..].find('\n').map_or(json.len(), |n| at + n)];
        let c = line.find("\"color\":").expect("colour");
        line[c..].to_string()
    };
    assert_ne!(
        colour_of(&safe, "RUN 0.50 ms"),
        colour_of(&long, "RUN 3.00 ms")
    );
    assert_eq!(
        colour_of(&safe, "CLIPPED 5.0 %"),
        colour_of(&long, "CLIPPED 5.0 %")
    );
}

#[test]
fn the_curve_follows_its_params() {
    let (doc, mut v) = rack(FxKind::Clipper);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let before = dl.to_json();
    let (mut doc2, mut v2) = rack(FxKind::Clipper);
    doc2.devices[0].params.insert("curve".into(), 0.0);
    doc2.devices[0].params.insert("symmetry".into(), 0.0);
    v2.sync(&doc2, &registry());
    let mut dl2 = DisplayList::new();
    v2.paint(&mut dl2, &doc2, &registry());
    assert_ne!(before, dl2.to_json());
    assert!(
        dl2.texts().into_iter().any(|t| t == "HARD CLIP"),
        "{:?}",
        dl2.texts()
    );
}
