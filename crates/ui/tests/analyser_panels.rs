//! The spectrum and phase meter panels: the layouts, what the feed changes, the texts the panels
//! draw, bypass freezing the display, the goldens of populated panels, and that the UI's copies
//! of the device layouts (the phase meter's value indices, the band edges) match the devices
//! and the DSP they mirror. Regenerate goldens with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test analyser_panels`.

mod support;

use atomik_ui::analyser::{AnalyserData, band_edges};
use atomik_ui::fx::{self, FxKind, update_display};
use atomik_ui::fx_layout::fx_layout;
use atomik_ui::{DisplayList, Flip, RackView, UiEvent};
use std::path::PathBuf;
use support::fx::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
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

fn paint(doc: &atomik_model::RackDoc, v: &mut RackView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn texts(dl: &DisplayList) -> Vec<String> {
    dl.texts().into_iter().map(str::to_string).collect()
}

fn fed(kind: FxKind, data: &AnalyserData) -> (atomik_model::RackDoc, RackView) {
    let (doc, mut v) = rack(kind);
    v.set_device_meter_values(&key("fx"), &raw_meters(kind));
    v.set_analyser(&key("fx"), data);
    (doc, v)
}

#[test]
fn both_panels_are_effect_panels_with_their_own_visual_and_meter_counts() {
    assert_eq!(FxKind::of("atomik.spectrum"), Some(FxKind::Spectrum));
    assert_eq!(FxKind::of("atomik.phase-meter"), Some(FxKind::PhaseMeter));
    assert_eq!(FxKind::Spectrum.meter_count(), 2);
    assert_eq!(FxKind::PhaseMeter.meter_count(), 16);
    for kind in [FxKind::Spectrum, FxKind::PhaseMeter] {
        let spec = fx_layout(kind, registry().get(kind.type_id()).unwrap().descriptor);
        let panel = spec.fx.expect("a visual");
        assert_eq!(panel.kind, kind);
        for r in [panel.plot, panel.meters, panel.readout] {
            assert!(r.width() > 10.0 && r.height() > 8.0, "{kind:?}: {r:?}");
            assert!(panel.visual.contains_rect(r), "{kind:?}: {r:?}");
        }
    }
    // The phase meter's four areas sit side by side without touching.
    let spec = fx_layout(
        FxKind::PhaseMeter,
        registry().get("atomik.phase-meter").unwrap().descriptor,
    );
    let p = spec.fx.unwrap();
    let mut areas = [p.plot, p.readout, p.meters, p.aux];
    areas.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    for w in areas.windows(2) {
        assert!(w[0].x1 + 8.0 <= w[1].x0, "{areas:?}");
    }
    assert!(p.visual.contains_rect(p.aux));
    // The goniometer is square.
    assert!((p.plot.width() - p.plot.height()).abs() < 1e-9);
}

#[test]
fn the_view_ignores_the_feed_of_other_devices_and_repaints_only_on_news() {
    let (doc, mut v) = rack(FxKind::Spectrum);
    let data = spectrum_data(false);
    assert!(
        v.analyser_data(&key("fx"))
            .is_some_and(|d| d.trace.is_empty())
    );
    v.set_analyser(&key("fx"), &data);
    assert_eq!(v.analyser_data(&key("fx")), Some(&data));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    // The same data again changes nothing: no repaint is asked for.
    v.set_analyser(&key("fx"), &data);
    assert!(!v.needs_repaint(), "no news, no repaint");
    let mut other = data.clone();
    other.peak_hz = 2000.0;
    v.set_analyser(&key("fx"), &other);
    assert!(v.needs_repaint(), "new data repaints");
    v.paint(&mut dl, &doc, &registry());
    // A hidden (back) panel does not ask for repaints.
    v.set_flip(Flip::Back);
    v.paint(&mut dl, &doc, &registry());
    v.set_analyser(&key("fx"), &data);
    assert!(!v.needs_repaint(), "the back face shows no analysis");
    // Unknown keys and other effects are ignored.
    v.set_analyser(&key("nope"), &data);
    let (_, mut comp) = rack(FxKind::Compressor);
    comp.set_analyser(&key("fx"), &data);
    assert!(comp.analyser_data(&key("fx")).is_none());
}

#[test]
fn the_spectrum_draws_its_axes_and_the_loudest_bin() {
    let (doc, mut v) = fed(FxKind::Spectrum, &spectrum_data(false));
    let t = texts(&paint(&doc, &mut v));
    // `range` is 80 in the fixture: a grid every 10 dB.
    for s in [
        "SPECTRUM",
        "20",
        "100",
        "1k",
        "20k",
        "0",
        "-10",
        "-80",
        "PEAK 1.00 kHz  -20.1 dB",
        "L",
        "R",
    ] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
    // No feed: the readout says so.
    let (doc, mut v) = rack(FxKind::Spectrum);
    let t = texts(&paint(&doc, &mut v));
    assert!(t.iter().any(|x| x == "PEAK --"), "{t:?}");
}

#[test]
fn the_phase_meter_draws_its_readouts() {
    let (doc, mut v) = fed(FxKind::PhaseMeter, &scope_data());
    let t = texts(&paint(&doc, &mut v));
    for s in [
        "STEREO IMAGE AND LEVELS",
        "CORRELATION",
        "+0.62",
        "BALANCE",
        "L 10%",
        "WIDTH",
        "35%",
        "MOMENTARY",
        "-18.4",
        "-19.2",
        "-20.1",
        "PEAK",
        "-4.4",
        "TRUE PEAK",
        "-3.9",
        "LUFS",
        "dBTP",
        "M",
        "S",
    ] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
    // Loudness off shows dashes where the loudness would be.
    let (mut doc, mut v) = rack(FxKind::PhaseMeter);
    doc.devices[0].params.insert("loudness".into(), 0.0);
    v.sync(&doc, &registry());
    v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::PhaseMeter));
    let t = texts(&paint(&doc, &mut v));
    assert!(t.iter().any(|x| x == "--"), "{t:?}");
    assert!(!t.iter().any(|x| x == "-18.4"), "{t:?}");
}

#[test]
fn the_scope_mode_changes_the_figure_and_its_labels() {
    let (mut doc, mut v) = fed(FxKind::PhaseMeter, &scope_data());
    let gonio = paint(&doc, &mut v).to_json();
    doc.devices[0].params.insert("scope".into(), 1.0);
    v.sync(&doc, &registry());
    let t = paint(&doc, &mut v);
    assert_ne!(gonio, t.to_json());
    // The Lissajous labels the axes L and R (the goniometer's M and S are gone).
    let t = texts(&t);
    assert!(!t.iter().any(|x| x == "S"), "{t:?}");
}

#[test]
fn a_bypassed_analyser_keeps_its_picture_and_says_so() {
    for (kind, data) in [
        (FxKind::Spectrum, spectrum_data(false)),
        (FxKind::PhaseMeter, scope_data()),
    ] {
        let (mut doc, mut v) = fed(kind, &data);
        let live = paint(&doc, &mut v);
        assert!(!texts(&live).iter().any(|x| x == "BYPASSED"));
        doc.devices[0].params.insert("bypass".into(), 1.0);
        v.sync(&doc, &registry());
        let frozen = paint(&doc, &mut v);
        assert!(
            texts(&frozen).iter().any(|x| x == "BYPASSED"),
            "{kind:?}: {:?}",
            texts(&frozen)
        );
        // It still draws the last data (the readout is still there).
        assert_eq!(v.analyser_data(&key("fx")), Some(&data));
    }
}

#[test]
fn the_bands_and_the_fine_trace_paint_differently_and_the_data_changes_the_painting() {
    let (doc, mut v) = fed(FxKind::Spectrum, &spectrum_data(false));
    let fine = paint(&doc, &mut v).to_json();
    let (doc2, mut v2) = fed(FxKind::Spectrum, &spectrum_data(true));
    let bands = paint(&doc2, &mut v2).to_json();
    assert_ne!(fine, bands);
    let (doc3, mut v3) = rack(FxKind::Spectrum);
    v3.set_device_meter_values(&key("fx"), &raw_meters(FxKind::Spectrum));
    assert_ne!(fine, paint(&doc3, &mut v3).to_json());
    // Peak hold off (no peaks fed) draws less.
    let mut no_peaks = spectrum_data(false);
    no_peaks.peaks.clear();
    let (doc4, mut v4) = fed(FxKind::Spectrum, &no_peaks);
    assert_ne!(fine, paint(&doc4, &mut v4).to_json());
}

#[test]
fn goldens_of_the_populated_panels() {
    for (name, kind, data, flip) in [
        (
            "analyser_spectrum_trace_1x.json",
            FxKind::Spectrum,
            spectrum_data(false),
            Flip::Front,
        ),
        (
            "analyser_spectrum_bands_1x.json",
            FxKind::Spectrum,
            spectrum_data(true),
            Flip::Front,
        ),
        (
            "analyser_phase_meter_1x.json",
            FxKind::PhaseMeter,
            scope_data(),
            Flip::Front,
        ),
    ] {
        let (doc, mut v) = fed(kind, &data);
        v.set_flip(flip);
        check(name, &paint(&doc, &mut v).to_json());
    }
}

#[test]
fn the_ui_copies_of_the_device_layouts_match_the_devices() {
    use atomik_devices::phase_meter::{PHASE_METERS, value};
    assert_eq!(fx::phase::COUNT, PHASE_METERS);
    for (ui, device) in [
        (fx::phase::PEAK_L, value::PEAK_L),
        (fx::phase::PEAK_R, value::PEAK_R),
        (fx::phase::HOLD_L, value::HOLD_L),
        (fx::phase::HOLD_R, value::HOLD_R),
        (fx::phase::RMS_L, value::RMS_L),
        (fx::phase::RMS_R, value::RMS_R),
        (fx::phase::SAMPLE_PEAK_DB, value::SAMPLE_PEAK_DB),
        (fx::phase::TRUE_PEAK_DB, value::TRUE_PEAK_DB),
        (fx::phase::CORRELATION, value::CORRELATION),
        (fx::phase::BALANCE, value::BALANCE),
        (fx::phase::WIDTH, value::WIDTH),
        (fx::phase::MID_RMS, value::MID_RMS),
        (fx::phase::SIDE_RMS, value::SIDE_RMS),
        (fx::phase::MOMENTARY, value::MOMENTARY),
        (fx::phase::SHORT_TERM, value::SHORT_TERM),
        (fx::phase::INTEGRATED, value::INTEGRATED),
    ] {
        assert_eq!(ui, device);
    }
    // The band edges are the DSP's bands: the geometric centre of each is its centre.
    for (i, centre) in atomik_dsp::spectrum::band_centres().into_iter().enumerate() {
        let (lo, hi) = band_edges(i);
        assert!(
            ((lo * hi).sqrt() / f64::from(centre) - 1.0).abs() < 1e-4,
            "band {i}"
        );
        assert!(
            (hi / lo - 10f64.powf(0.1)).abs() < 1e-9,
            "a third of an octave"
        );
    }
    assert_eq!(
        atomik_ui::analyser::TRACE_COLUMNS,
        240,
        "the columns the app asks of the analysis"
    );
}

#[test]
fn the_phase_meter_display_values_have_the_right_ballistics() {
    let kind = FxKind::PhaseMeter;
    let mut shown = Vec::new();
    let mut raw = vec![0.0f32; 16];
    raw[fx::phase::CORRELATION] = 0.5;
    raw[fx::phase::BALANCE] = -0.25;
    raw[fx::phase::WIDTH] = 0.4;
    raw[fx::phase::PEAK_L] = 0.5;
    raw[fx::phase::SAMPLE_PEAK_DB] = -6.0;
    assert!(update_display(kind, &raw, &mut shown));
    assert!((shown[fx::phase::CORRELATION] - 0.5).abs() < 1e-6);
    assert!((shown[fx::phase::BALANCE] + 0.25).abs() < 1e-6);
    assert!((shown[fx::phase::WIDTH] - 0.4).abs() < 1e-6);
    assert!(
        (shown[fx::phase::PEAK_L] + 6.0).abs() < 0.3,
        "20 log 0.5, in dB"
    );
    assert!(
        (shown[fx::phase::SAMPLE_PEAK_DB] + 6.0).abs() < 0.06,
        "shown as published"
    );
    // The signed values move at once in both directions (the device already averages them);
    // the peak falls by one step per feed.
    raw[fx::phase::CORRELATION] = -0.8;
    raw[fx::phase::PEAK_L] = 0.0;
    update_display(kind, &raw, &mut shown);
    assert!((shown[fx::phase::CORRELATION] + 0.8).abs() < 1e-6);
    assert!(shown[fx::phase::PEAK_L] > -6.0 - 2.0 && shown[fx::phase::PEAK_L] < -6.0);
    // Nothing moves, nothing changes: a quiet meter stops asking for repaints.
    for _ in 0..200 {
        update_display(kind, &raw, &mut shown);
    }
    assert!(!update_display(kind, &raw, &mut shown));
    // A silent correlation of -0.0 never flickers against 0.0.
    raw[fx::phase::CORRELATION] = -0.001;
    update_display(kind, &raw, &mut shown);
    let at_rest = shown[fx::phase::CORRELATION].to_bits();
    raw[fx::phase::CORRELATION] = 0.001;
    update_display(kind, &raw, &mut shown);
    assert_eq!(shown[fx::phase::CORRELATION].to_bits(), at_rest);
}

#[test]
fn the_panels_resize_with_the_window_without_leaving_the_rack() {
    for kind in [FxKind::Spectrum, FxKind::PhaseMeter] {
        let (doc, mut v) = rack(kind);
        for (w, h) in [(820.0, 900.0), (1400.0, 700.0), (640.0, 600.0)] {
            let _ = v.handle(
                &UiEvent::Resize(atomik_ui::kurbo::Size::new(w, h)),
                &doc,
                &registry(),
            );
            let dl = paint(&doc, &mut v);
            assert!(!dl.texts().is_empty(), "{kind:?} at {w} x {h}");
        }
    }
}
