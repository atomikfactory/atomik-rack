//! Renders PNG previews of the slice editor and the slicer panel with vello_cpu.
//! Run: `cargo test -p atomik-ui --test slice_preview -- --ignored`. Writes
//! `target/preview/{slice_editor,slice_editor_empty,slicer_panel,slicer_panel_back}.png` (not committed).

mod support;

use atomik_ui::kurbo::Size;
use atomik_ui::{Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::slicer::*;
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
    let (w, h) = (
        (EDITOR_BOUNDS.width() * SCALE) as u16,
        (EDITOR_BOUNDS.height() * SCALE) as u16,
    );
    for (name, zones, sel) in [
        ("slice_editor.png", sliced_zones(&MARKERS), Some(1)),
        ("slice_editor_empty.png", Vec::new(), None),
    ] {
        let doc = slicer_doc(zones);
        let mut v = editor(&doc, &reg);
        v.set_scale(SCALE);
        v.set_focused(true);
        v.set_selected_slice(sel);
        let mut canvas = CpuCanvas::new(w, h, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        write(name, canvas);
    }
    for (name, flip) in [
        ("slicer_panel.png", Flip::Front),
        ("slicer_panel_back.png", Flip::Back),
    ] {
        let mut doc = slicer_doc(sliced_zones(&MARKERS));
        doc.devices.retain(|d| d.key == key("slicer"));
        doc.cables.clear();
        let mut rack = RackView::new(Theme::dark());
        let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
        let _ = rack.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        rack.set_flip(flip);
        let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
        rack.paint(&mut canvas, &doc, &reg);
        write(name, canvas);
    }
}
