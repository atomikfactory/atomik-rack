//! Renders PNG previews of the container panel, the rack inside a container and the container
//! editor with vello_cpu.
//! Run: `cargo test -p atomik-ui --test container_preview -- --ignored`. Writes
//! `target/preview/*.png` (not committed).

mod support;

use atomik_ui::kurbo::Rect;
use atomik_ui::kurbo::Size;
use atomik_ui::{ContainerEditorView, ContainerTab, Flip, RackView, TextEngine, Theme, UiEvent};
use support::container::*;
use support::cpu::CpuCanvas;
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

fn rack_view(doc: &atomik_model::RackDoc, size: Size) -> RackView {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(size), doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(SCALE), doc, &reg);
    v
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    let mut text = TextEngine::new();
    let d = layered();
    let size = Size::new(820.0, 640.0);
    let (pw, ph) = ((size.width * SCALE) as u16, (size.height * SCALE) as u16);

    let mut v = rack_view(d.doc(), size);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("panel_front.png", canvas);
    v.set_flip(Flip::Back);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("panel_back.png", canvas);

    let mut v = rack_view(d.doc(), Size::new(820.0, 900.0));
    v.set_scope(&path(&["layer"]));
    let (pw, ph) = ((820.0 * SCALE) as u16, (900.0 * SCALE) as u16);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("scope_front.png", canvas);
    v.set_flip(Flip::Back);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("scope_back.png", canvas);

    // The editor, both tabs.
    let mut d = layered();
    add_inner(&mut d, "layer", 4, SYNTH, "lead2");
    apply(
        &mut d,
        atomik_model::Command::Container(atomik_model::ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: target("pad", "filter.cutoff", 8000.0, 200.0),
            merge: None,
        }),
    );
    let reg = registry();
    let bounds = Rect::new(0.0, 0.0, 1000.0, 560.0);
    let mut ed = ContainerEditorView::new(Theme::dark());
    ed.set_bounds(bounds);
    ed.set_scale(SCALE);
    ed.set_focused(true);
    ed.open(key("layer"));
    let (ew, eh) = ((1000.0 * SCALE) as u16, (560.0 * SCALE) as u16);
    let mut canvas = CpuCanvas::new(ew, eh, &mut text);
    ed.paint(&mut canvas, d.doc(), &reg);
    write("editor_splits.png", canvas);
    ed.set_tab(ContainerTab::Macros);
    let mut canvas = CpuCanvas::new(ew, eh, &mut text);
    ed.paint(&mut canvas, d.doc(), &reg);
    write("editor_macros.png", canvas);
}
