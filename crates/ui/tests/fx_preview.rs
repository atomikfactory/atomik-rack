//! Renders PNG previews of the six effect panels with vello_cpu.
//! Run: `cargo test -p atomik-ui --test fx_preview -- --ignored`. Writes
//! `target/preview/fx_<effect>[_back|_menu].png` (not committed).

mod support;

use atomik_ui::fx::FxKind;
use atomik_ui::kurbo::Size;
use atomik_ui::{Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::fx::*;
use support::*;

const SCALE: f64 = 1.5;

fn write(name: &str, canvas: CpuCanvas<'_>) {
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    let reg = registry();
    let mut text = TextEngine::new();
    for kind in FxKind::ALL {
        let doc = fx_doc(kind);
        let h = if kind == FxKind::Eq { 560.0 } else { 420.0 };
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, h)), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        v.set_device_meter_values(&key("fx"), &raw_meters(kind));
        let slug = &kind.type_id()["atomik.".len()..];
        for (flip, suffix) in [(Flip::Front, ""), (Flip::Back, "_back")] {
            v.set_flip(flip);
            let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (h * SCALE) as u16, &mut text);
            v.paint(&mut canvas, &doc, &reg);
            write(&format!("fx_{slug}{suffix}.png"), canvas);
        }
    }

    // The compressor with a sidechain source and the "SC from" list open.
    let mut doc = fx_doc(FxKind::Compressor);
    doc.devices
        .push(device("kick", "atomik.poly-synth", "Kick"));
    doc.devices.push(device("pad", "atomik.poly-synth", "Pad"));
    doc.devices[0].params.insert("sc_enable".into(), 1.0);
    doc.cables.push(cable(("kick", "out_l"), ("fx", "sc_in_l")));
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
    v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::Compressor));
    let b = v.sc_button_center(&key("fx")).unwrap();
    let _ = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("fx_compressor_menu.png", canvas);
}
