//! Effect panel fixtures: a rack with one effect `fx` with representative params, and
//! representative raw meter values of each effect.
#![allow(dead_code, reason = "each test binary uses a subset")]

use super::*;
use atomik_model::RackDoc;
use atomik_ui::analyser::AnalyserData;
use atomik_ui::fx::FxKind;
use atomik_ui::kurbo::Size;
use atomik_ui::{RackView, Theme, UiEvent};

/// A rack with one effect `fx`, params set to something representative.
pub fn fx_doc(kind: FxKind) -> RackDoc {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = device("fx", kind.type_id(), "FX");
    let set = |d: &mut atomik_model::RackDevice, k: &str, v: f32| {
        d.params.insert(k.to_string(), v);
    };
    match kind {
        FxKind::Reverb => {
            set(&mut d, "algorithm", 2.0);
            set(&mut d, "predelay", 60.0);
            set(&mut d, "decay", 4.0);
        }
        FxKind::Compressor => {
            set(&mut d, "threshold", -24.0);
            set(&mut d, "ratio", 6.0);
            set(&mut d, "knee", 10.0);
        }
        FxKind::Eq => {
            set(&mut d, "band1.on", 1.0);
            set(&mut d, "band2.gain", 5.0);
            set(&mut d, "band3.gain", -7.0);
            set(&mut d, "band3.q", 2.5);
            set(&mut d, "band4.gain", 9.0);
            set(&mut d, "band5.gain", 4.0);
            set(&mut d, "band6.on", 1.0);
            set(&mut d, "highcut.slope", 1.0);
            set(&mut d, "band7.gain", 3.5);
            set(&mut d, "band7.shape", 1.0);
            set(&mut d, "band2.shape", 3.0);
            set(&mut d, "lowcut.slope", 3.0);
            set(&mut d, "stereo.mode", 1.0);
        }
        FxKind::Distortion => {
            set(&mut d, "curve", 2.0);
            set(&mut d, "drive", 18.0);
            set(&mut d, "bias", 0.2);
        }
        FxKind::GuitarAmp => {
            set(&mut d, "setup", 2.0);
            set(&mut d, "gain", 8.5);
            set(&mut d, "cab", 2.0);
            set(&mut d, "mic", 0.6);
            set(&mut d, "delay.on", 1.0);
        }
        FxKind::Modulation => {
            set(&mut d, "mode", 2.0);
            set(&mut d, "depth", 0.8);
            set(&mut d, "spread", 120.0);
        }
        FxKind::Mastering => {
            set(&mut d, "band1.threshold", -24.0);
            set(&mut d, "xover.low", 150.0);
        }
        FxKind::Character => {
            set(&mut d, "gain", 0.75);
            set(&mut d, "transform", 0.7);
            set(&mut d, "volume", 0.4);
        }
        FxKind::Spectrum => {
            set(&mut d, "size", 3.0);
            set(&mut d, "range", 80.0);
            set(&mut d, "tilt", 2.0);
        }
        FxKind::LoudnessMeter => {
            set(&mut d, "preset", 0.0);
            set(&mut d, "history_span", 1.0);
            set(&mut d, "target_lufs", -18.0);
            set(&mut d, "max_tp", -2.0);
        }
        FxKind::PhaseMeter => {
            set(&mut d, "hold", 2.0);
            set(&mut d, "scope_gain", 6.0);
        }
        FxKind::AudioTrack => {
            set(&mut d, "tuner_ref", 440.0);
        }
        FxKind::Tuner => {
            set(&mut d, "ref", 442.0);
        }
        FxKind::Clipper => {
            set(&mut d, "gain", 9.0);
            set(&mut d, "ceiling", -6.0);
            set(&mut d, "curve", 0.4);
            set(&mut d, "symmetry", 0.3);
        }
        FxKind::StereoTool => {
            set(&mut d, "lo_width", 0.55);
            set(&mut d, "width", 1.4);
            set(&mut d, "low_bypass", 220.0);
            set(&mut d, "solo", 1.0);
            set(&mut d, "invert_l", 1.0);
            set(&mut d, "rotate", 45.0);
            set(&mut d, "rotate_hz", 200.0);
            set(&mut d, "balance", 0.2);
        }
        FxKind::Duck => {
            set(&mut d, "mode", 3.0);
            set(&mut d, "depth", 0.9);
            set(&mut d, "length", 3.0);
            set(&mut d, "band", 1.0);
            set(&mut d, "band_freq", 90.0);
        }
        FxKind::Saturation => {
            set(&mut d, "saturation", 0.6);
            set(&mut d, "type", 0.0);
            set(&mut d, "mix", 0.8);
            set(&mut d, "output", -2.0);
            set(&mut d, "oversampling", 2.0);
        }
        FxKind::HeadphoneCorrection => {
            set(&mut d, "amount", 1.0);
            set(&mut d, "target", 1.0);
            set(&mut d, "file_kind", 0.0);
            d.headphone = Some(sample_headphones());
        }
        FxKind::DynamicEq => {
            set(&mut d, "band1.on", 1.0);
            set(&mut d, "band1.gain", 2.0);
            set(&mut d, "band1.range", -4.0);
            set(&mut d, "band2.on", 1.0);
            set(&mut d, "band2.freq", 320.0);
            set(&mut d, "band2.range", -9.0);
            set(&mut d, "band3.on", 1.0);
            set(&mut d, "band3.gain", -3.0);
            set(&mut d, "band3.freq", 1800.0);
            set(&mut d, "band3.range", 6.0);
            set(&mut d, "band4.on", 1.0);
            set(&mut d, "band4.freq", 5200.0);
            set(&mut d, "band4.q", 3.0);
            set(&mut d, "band4.source", 1.0);
            set(&mut d, "band5.gain", 3.0);
            set(&mut d, "band5.range", 0.0);
        }
        FxKind::TonalBalance => {
            set(&mut d, "response", 1.0);
            set(&mut d, "view", 2.0);
            set(&mut d, "channel", 0.0);
        }
        FxKind::GainTool => {
            set(&mut d, "mode", 1.0);
            set(&mut d, "gain", -6.0);
            set(&mut d, "fade", 3000.0);
            set(&mut d, "invert_l", 1.0);
            set(&mut d, "pan_l", -0.6);
            set(&mut d, "pan_law", 2.0);
            set(&mut d, "width", 1.4);
        }
    }
    doc.devices.push(d);
    doc
}

/// Representative raw meter values of each effect (the device layouts of `docs/DEVICES.md`).
pub fn raw_meters(kind: FxKind) -> Vec<f32> {
    match kind {
        FxKind::Reverb => vec![0.3, 0.28, 0.05],
        FxKind::Compressor => vec![0.5, 0.35, 6.0],
        FxKind::Eq => {
            let mut v: Vec<f32> = (0..16)
                .map(|k| 0.5 * 10f32.powf(-(k as f32 - 5.0).abs() * 0.35))
                .collect();
            v.extend([0.4, 0.38]);
            v
        }
        FxKind::Distortion => vec![0.7, 0.5],
        // in peak, out peak, gate reduction dB
        FxKind::GuitarAmp => vec![0.6, 0.35, 18.0],
        FxKind::Modulation => vec![],
        FxKind::Mastering => vec![-20.5, -21.2, -23.4, -1.8, 2.0, 3.5, 1.2, 4.0],
        // in peak, out peak, transform_eff, activity
        FxKind::Character => vec![0.5, 0.35, 0.7, 0.6],
        // peak L, R (linear), correlation, long correlation, sample peak L, R, true peak L, R
        // (dBFS), clipped samples
        FxKind::Spectrum => vec![0.5, 0.4, 0.62, 0.7, -1.2, -3.0, -0.4, -2.1, 3.0],
        // recent peak L R, held peak L R, RMS L R, sample peak, true peak, correlation, balance,
        // width, mid RMS, side RMS, momentary, short-term, integrated
        FxKind::PhaseMeter => vec![
            0.5, 0.4, 0.6, 0.45, 0.2, 0.16, -4.4, -3.9, 0.62, -0.1, 0.35, 0.17, 0.09, -18.4, -19.2,
            -20.1,
        ],
        // driven peak, output peak, clipped share (percent), longest run (ms)
        FxKind::Clipper => vec![0.9, 0.5, 18.5, 1.4],
        // The dip and the position along the drawn curve.
        FxKind::Duck => vec![0.6, 0.3],
        // Low and high correlation, the two ACTIVE lamps and the split lamp.
        FxKind::StereoTool => vec![-0.35, 0.62, 1.0, 0.0, 1.0],
        // Band gains (dB), 16 input energies, 16 key energies, output peaks, key cabled.
        FxKind::DynamicEq => {
            let mut v = vec![-1.5, -4.5, 3.0, -2.0, 3.0];
            v.extend((0..16).map(|k| 0.4 * 10f32.powf(-(k as f32 - 6.0).abs() * 0.3)));
            v.extend((0..16).map(|k| 0.3 * 10f32.powf(-(k as f32 - 11.0).abs() * 0.4)));
            v.extend([0.4, 0.38, 1.0]);
            v
        }
        // Input peaks L and R, output peaks L and R (linear), the held peak (dB), then the signal,
        // CV and fading flags.
        FxKind::GainTool => vec![0.5, 0.4, 0.35, 0.28, -3.2, 1.0, 1.0, 1.0],
        // The drive lamp (0..1), the input and the output peak.
        FxKind::Saturation => vec![0.6, 0.5, 0.45],
        // Peak L, R (linear).
        FxKind::TonalBalance => vec![0.5, 0.4],
        // The 16 scalars (momentary, short-term, integrated, LRA, true peak, true peak L and R,
        // sample peak, PLR, PSR, DR, target, max true peak, reset count, history points, seconds)
        // and the 46 distribution bins.
        FxKind::LoudnessMeter => {
            let mut v = vec![
                -13.2, -14.6, -14.2, 6.4, -0.6, -0.8, -0.6, -1.1, 13.6, 11.2, 9.4, -14.0, -1.0,
                3.0, 120.0, 95.0,
            ];
            v.extend((0..46).map(|i| {
                let t = (i as f32 - 31.0) / 4.0;
                (-t * t).exp() * 0.9 + if i == 31 { 0.1 } else { 0.0 }
            }));
            v
        }
        // Loaded, the preamp (dB), the largest boost (dB), not busy.
        FxKind::HeadphoneCorrection => vec![1.0, -4.2, 4.2, 0.0],
        // The audio track publishes no meter values.
        FxKind::AudioTrack | FxKind::Tuner => vec![],
    }
}

/// A measured response of imaginary headphones: a bass bump, a presence peak and a treble dip,
/// 41 points from 20 Hz to 20 kHz, rounded to a tenth of a dB.
pub fn sample_headphones() -> atomik_device_sdk::headphone::HeadphoneData {
    use atomik_device_sdk::headphone::{HeadphoneBody, HeadphoneData, ResponseData};
    let bump = |f: f32, at: f32, width: f32, db: f32| {
        let x = (f / at).log2() / width;
        db * (-x * x).exp()
    };
    let freq: Vec<f32> = (0..41)
        .map(|k| (20.0 * (k as f32 * 9.965_784 / 40.0).exp2() * 10.0).round() / 10.0)
        .collect();
    let left: Vec<f32> = freq
        .iter()
        .map(|f| {
            let db =
                bump(*f, 60.0, 1.2, 4.0) + bump(*f, 3000.0, 0.6, 6.0) - bump(*f, 8000.0, 0.4, 5.0);
            (db * 10.0).round() / 10.0
        })
        .collect();
    HeadphoneData {
        name: "Studio cans.csv".to_owned(),
        body: HeadphoneBody::Response(ResponseData {
            freq,
            left,
            right: None,
        }),
    }
}

pub fn rack(kind: FxKind) -> (RackDoc, RackView) {
    let (doc, reg) = (fx_doc(kind), registry());
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 900.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (doc, v)
}

/// A spectrum with a tone at 1 kHz: a bump on a floor, peaks a little above.
pub fn spectrum_data(bands: bool) -> AnalyserData {
    let n = if bands { 31 } else { 240 };
    let trace: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            -80.0 + 60.0 * (-((t - 0.55) * 14.0).powi(2)).exp()
        })
        .collect();
    let peaks = trace.iter().map(|v| v + 4.0).collect();
    AnalyserData {
        trace,
        peaks,
        bands,
        peak_hz: 1000.0,
        peak_db: -20.06,
        ..AnalyserData::default()
    }
}

/// The loudness meter's history: the short-term and the momentary loudness every half second, a
/// quiet passage, a loud one and a gap of silence.
pub fn loudness_data() -> AnalyserData {
    let short: Vec<f32> = (0..120)
        .map(|i| {
            let t = i as f32;
            if (60..66).contains(&i) {
                -120.0
            } else {
                -17.0 + 4.0 * (t * 0.07).sin() - if i < 30 { 6.0 } else { 0.0 }
            }
        })
        .collect();
    let momentary = short
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if *v < -100.0 {
                *v
            } else {
                v + 2.5 * (i as f32 * 0.9).sin()
            }
        })
        .collect();
    AnalyserData {
        loud_short: short,
        loud_momentary: momentary,
        ..AnalyserData::default()
    }
}

/// Scope frames: a Lissajous of a slightly detuned pair, so the figure is an ellipse.
pub fn scope_data() -> AnalyserData {
    let (l, r): (Vec<f32>, Vec<f32>) = (0..1024)
        .map(|i| {
            let t = i as f32 * 0.04;
            (0.6 * t.sin(), 0.5 * (t + 0.7).sin())
        })
        .unzip();
    AnalyserData {
        scope_l: l,
        scope_r: r,
        ..AnalyserData::default()
    }
}

/// A tonal balance reading against Balanced: too much low end, the highs a little quiet.
pub fn tonal_data() -> AnalyserData {
    use atomik_ui::tonal_panel::{TargetKind, TargetRow, TonalView};
    let target = atomik_dsp::tonal::builtin(0);
    let mut bands = target.bands.to_vec();
    for (i, v) in bands.iter_mut().enumerate() {
        if i < 9 {
            *v += 4.0;
        } else if i >= 24 {
            *v -= 3.5;
        }
    }
    let row = |value: f32, name: &str, kind| TargetRow {
        value,
        name: name.to_string(),
        kind,
    };
    AnalyserData {
        tonal: TonalView {
            ready: true,
            deviation: [4.2, -0.6, 1.1, -5.0],
            tolerance: [3.0; 4],
            bands,
            target_bands: target.bands.to_vec(),
            target_tol: target.tol.to_vec(),
            target_name: "Balanced".to_string(),
            targets: vec![
                row(0.0, "Balanced", TargetKind::Builtin),
                row(1.0, "Bright", TargetKind::Builtin),
                row(2.0, "Warm", TargetKind::Builtin),
                row(3.0, "Bass-heavy", TargetKind::Builtin),
                row(11.0, "My reference", TargetKind::User),
            ],
            ..TonalView::default()
        },
        ..AnalyserData::default()
    }
}
