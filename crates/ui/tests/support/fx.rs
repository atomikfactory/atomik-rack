//! Effect panel fixtures: a rack with one effect `fx` with representative params, and
//! representative raw meter values of each effect.
#![allow(dead_code, reason = "each test binary uses a subset")]

use super::*;
use atomik_model::RackDoc;
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
        }
        FxKind::Distortion => {
            set(&mut d, "curve", 2.0);
            set(&mut d, "drive", 18.0);
            set(&mut d, "bias", 0.2);
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
        FxKind::Modulation => vec![],
        FxKind::Mastering => vec![-20.5, -21.2, -23.4, -1.8, 2.0, 3.5, 1.2, 4.0],
        // in peak, out peak, transform_eff, activity
        FxKind::Character => vec![0.5, 0.35, 0.7, 0.6],
    }
}

pub fn rack(kind: FxKind) -> (RackDoc, RackView) {
    let (doc, reg) = (fx_doc(kind), registry());
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 900.0)), &doc, &reg);
    v.sync(&doc, &reg);
    (doc, v)
}
