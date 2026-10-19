//! Renders PNG previews of the step editor and the drum panel with vello_cpu.
//! Run: `cargo test -p atomik-ui --test drum_preview -- --ignored`. Writes
//! `target/preview/{step_editor,step_editor_swing,drum_panel,drum_back}.png` (not committed).

mod support;

use atomik_model::StepResolution;
use atomik_ui::kurbo::Size;
use atomik_ui::{Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::drum::*;
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
    let doc = drum_doc();
    let mut v = editor(&doc, &reg, Some(CLIP));
    v.set_scale(SCALE);
    v.set_focused(true);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("step_editor.png", canvas);

    let mut clip = beat_clip();
    clip.grid = Some(grid(StepResolution::ThirtySecond, 64, 30));
    clip.length = 64 * S32;
    let doc = drum_doc_with(Some(clip));
    let mut v = editor(&doc, &reg, Some(CLIP));
    v.set_scale(SCALE);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("step_editor_swing.png", canvas);

    let mut doc = drum_doc();
    doc.devices.retain(|d| d.key == key("drums"));
    doc.cables.clear();
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(Size::new(820.0, 270.0)), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
    rack.set_selected_pad(&key("drums"), 2);
    let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (270.0 * SCALE) as u16, &mut text);
    rack.paint(&mut canvas, &doc, &reg);
    write("drum_panel.png", canvas);
    rack.set_flip(Flip::Back);
    let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (270.0 * SCALE) as u16, &mut text);
    rack.paint(&mut canvas, &doc, &reg);
    write("drum_back.png", canvas);
}
