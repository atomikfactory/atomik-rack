//! Renders a PNG preview of the audio arrangement with vello_cpu.
//! Run: `cargo test -p atomik-ui --test audio_preview -- --ignored`. Writes
//! `target/preview/audio_arrangement.png` (not committed).

mod support;

use atomik_model::ClipId;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{TextEngine, UiEvent};
use std::path::PathBuf;
use support::audio::*;
use support::cpu::CpuCanvas;
use support::song::*;
use support::*;

const SCALE: f64 = 1.5;

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_audio_png() {
    let (doc, reg) = (audio_doc(), registry());
    let mut text = TextEngine::new();
    let mut arr = audio_arrangement(&doc, &reg);
    arr.set_scale(SCALE);
    arr.set_snap(Snap::Beat);
    arr.set_playhead(2 * BAR + PPQ + PPQ / 2);
    arr.set_selected_clips(&[ClipId(10)]);
    let lane = arr.lane_rect(&key("audio-track-2"), 0).unwrap();
    let pos = Point::new(arr.tick_to_x(2 * BAR), lane.center().y);
    let _ = arr.handle(
        &UiEvent::FileHover {
            pos,
            paths: vec![PathBuf::from("a.wav")],
        },
        &doc,
        &reg,
    );
    let b = ARR_BOUNDS;
    let mut canvas = CpuCanvas::new(
        (b.width() * SCALE) as u16,
        (b.height() * SCALE) as u16,
        &mut text,
    );
    arr.paint(&mut canvas, &doc, &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join("audio_arrangement.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}
