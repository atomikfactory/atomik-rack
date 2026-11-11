//! The distortion panel: the plot title follows the selected curve.

mod support;

use atomik_ui::DisplayList;
use atomik_ui::fx::FxKind;
use support::fx::*;
use support::*;

fn title_for(curve: f32) -> Vec<String> {
    let (mut doc, mut v) = rack(FxKind::Distortion);
    doc.devices[0].params.insert("curve".into(), curve);
    v.sync(&doc, &registry());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_plot_title_follows_every_curve_choice() {
    let names = [
        "SOFT CLIP",
        "HARD CLIP",
        "TUBE",
        "FOLDBACK",
        "BITCRUSH",
        "TAPE",
        "FUZZ",
        "MODULATE",
        "WARP",
        "SCREAM",
    ];
    for (i, name) in names.iter().enumerate() {
        let texts = title_for(i as f32);
        assert!(texts.iter().any(|t| t == name), "curve {i}: {texts:?}");
        if i != 4 {
            assert!(!texts.iter().any(|t| t == "BITCRUSH"), "curve {i}");
        }
    }
}
