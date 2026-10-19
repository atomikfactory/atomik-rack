//! Renders PNG previews of the sample editor and the sampler panel with vello_cpu.
//! Run: `cargo test -p atomik-ui --test sample_preview -- --ignored`. Writes
//! `target/preview/{sample_editor,sample_editor_loop,sampler_panel}.png` (not committed).

mod support;

use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::sampler::*;
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

    let doc = sampler_doc(layered_zones());
    let mut v = editor(&doc, &reg);
    v.set_scale(SCALE);
    v.set_focused(true);
    v.set_selected_zone(Some(1));
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("sample_editor.png", canvas);

    let doc = sampler_doc(vec![looped_zone()]);
    let mut v = editor(&doc, &reg);
    v.set_scale(SCALE);
    let drop = UiEvent::FileHover {
        pos: Point::new(v.key_center_x(64), v.velocity_y(90)),
        paths: vec!["a.wav".into()],
    };
    let _ = v.handle(&drop, &doc, &reg);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("sample_editor_loop.png", canvas);

    let mut doc = sampler_doc(layered_zones());
    doc.devices.retain(|d| d.key == key("sampler"));
    doc.cables.clear();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
    let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
    rack.paint(&mut canvas, &doc, &reg);
    write("sampler_panel.png", canvas);
}
