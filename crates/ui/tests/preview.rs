//! Renders a PNG preview of the reference rack (front and back) with vello_cpu.
//! Run: `cargo test -p atomik-ui --test preview -- --ignored`. Writes
//! `target/preview/preview.png` (not committed).

mod support;

use atomik_ui::kurbo::{Affine, Size};
use atomik_ui::{Canvas, Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::*;

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    let (doc, reg) = (reference_doc(), registry());
    let scale = 1.5;
    let view_size = Size::new(820.0, 700.0);
    let mut text = TextEngine::new();
    let (w, h) = (
        (view_size.width * 2.0 * scale) as u16,
        (view_size.height * scale) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    for (i, flip) in [Flip::Front, Flip::Back].into_iter().enumerate() {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(view_size), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), &doc, &reg);
        v.set_flip(flip);
        if flip == Flip::Back {
            let c = cable(("synth", "lfo_cv_out"), ("delay", "time_cv_in"));
            if let Some(m) = v.cable_midpoint(&c) {
                let _ = v.handle(&mv(m), &doc, &reg);
            }
        } else {
            v.set_selected(Some(key("delay")));
        }
        canvas.push_transform(Affine::translate((view_size.width * scale * i as f64, 0.0)));
        v.paint(&mut canvas, &doc, &reg);
        canvas.pop_transform();
    }
    let png = canvas.finish().into_png().expect("png");
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/preview/preview.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}
