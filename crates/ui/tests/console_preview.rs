//! Renders a PNG preview of the console (3 channels and the master, meters moving) with
//! vello_cpu. Run: `cargo test -p atomik-ui --test console_preview -- --ignored`. Writes
//! `target/preview/console_preview.png` (760 px pane, maximised) and
//! `console_preview_short.png` (380 px pane), not committed.

mod support;

use atomik_ui::kurbo::Rect;
use atomik_ui::{ChannelMeters, TextEngine};
use support::console::*;
use support::cpu::CpuCanvas;
use support::*;

fn render(pane_h: f64, maximised: bool, file: &str) {
    let (doc, reg) = (console_doc(), console_registry());
    let scale = 1.5;
    let bounds = Rect::new(0.0, 0.0, 1100.0, pane_h);
    let mut v = console_view(&doc, &reg);
    v.set_bounds(bounds);
    v.set_scale(scale);
    v.set_focused(true);
    v.set_maximised(maximised);
    v.set_latency_readouts(&[
        (key("ch-bass"), 0),
        (key("ch-lead"), 480),
        (key("ch-keys"), 64),
    ]);
    let m = |p: f32, r: f32, gr: f32| ChannelMeters {
        out_peak_l: p,
        out_peak_r: p * 0.8,
        out_rms_l: r,
        out_rms_r: r * 0.8,
        gr,
    };
    v.set_meters(&[
        (key("ch-bass"), m(0.7, 0.3, 0.6)),
        (key("ch-lead"), m(0.2, 0.08, 1.0)),
        (key("ch-keys"), m(1.0, 0.5, 0.9)),
        (key("master"), m(0.9, 0.45, 0.8)),
    ]);
    v.set_frame_time(0.0);
    v.set_meters(&[
        (key("ch-bass"), m(0.5, 0.25, 0.7)),
        (key("ch-lead"), m(0.15, 0.06, 1.0)),
        (key("ch-keys"), m(0.6, 0.35, 0.9)),
        (key("master"), m(0.7, 0.4, 0.8)),
    ]);
    v.set_frame_time(0.2);
    let mut text = TextEngine::new();
    let (w, h) = (
        (bounds.width() * scale) as u16,
        (bounds.height() * scale) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn console_preview_png() {
    render(760.0, true, "console_preview.png");
    render(380.0, false, "console_preview_short.png");
}
