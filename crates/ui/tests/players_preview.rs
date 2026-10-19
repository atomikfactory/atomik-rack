//! Renders PNG previews of the two player panels and the sequencer grid editor with vello_cpu.
//! Run: `cargo test -p atomik-ui --test players_preview -- --ignored`. Writes
//! `target/preview/{seq_panel,seq_panel_back,arp_panel,arp_panel_back,seq_editor,seq_editor_fill}.png`
//! (not committed).

mod support;

use atomik_ui::kurbo::Size;
use atomik_ui::{Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::seq::*;
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
    let doc = players_doc();
    let mut v = editor(&doc, &reg);
    v.set_scale(SCALE);
    v.set_focused(true);
    v.set_meter_values(&[0.0, 5.0, 1.0]);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("seq_editor.png", canvas);
    v.set_pattern_index(1);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    write("seq_editor_fill.png", canvas);

    let mut doc = players_doc();
    doc.cables.clear();
    doc.devices
        .retain(|d| d.key == key("seq") || d.key == key("arp"));
    let size = Size::new(820.0, 460.0);
    let mut rack = RackView::new(Theme::dark());
    let _ = rack.handle(&UiEvent::Resize(size), &doc, &reg);
    let _ = rack.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
    rack.set_device_meter_values(&key("seq"), &[0.0, 4.0, 1.0]);
    rack.set_device_meter_values(
        &key("arp"),
        &[3.0, 48.0, 52.0, 55.0, 60.0, 64.0, 67.0, 48.0, 52.0],
    );
    let (pw, ph) = ((size.width * SCALE) as u16, (size.height * SCALE) as u16);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    rack.paint(&mut canvas, &doc, &reg);
    write("players_front.png", canvas);
    rack.set_flip(Flip::Back);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    rack.paint(&mut canvas, &doc, &reg);
    write("players_back.png", canvas);
}
