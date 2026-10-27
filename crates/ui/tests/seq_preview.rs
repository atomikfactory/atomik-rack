//! Renders PNG previews of the sequencer views with vello_cpu.
//! Run: `cargo test -p atomik-ui --test seq_preview -- --ignored`. Writes
//! `target/preview/{arrangement,piano_roll,transport}.png` (not committed).

mod support;

use atomik_model::ClipId;
use atomik_model::theory::{ChordKind, Scale, ScaleKind};
use atomik_ui::chord_edit::Stamp;
use atomik_ui::key_rows::Fold;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::timeline::Snap;
use atomik_ui::{TextEngine, TransportState};
use support::cpu::CpuCanvas;
use support::song::*;
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

fn size(r: Rect) -> (u16, u16) {
    ((r.width() * SCALE) as u16, (r.height() * SCALE) as u16)
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    let (doc, reg) = (song_doc(), registry());
    let mut text = TextEngine::new();

    let mut arr = arrangement(&doc, &reg);
    arr.set_scale(SCALE);
    arr.set_snap(Snap::Beat);
    arr.set_playhead(2 * BAR + PPQ + PPQ / 2);
    arr.set_selected_clips(&[ClipId(2)]);
    arr.set_focused(true);
    let hover = arr.clip_rect(ClipId(5)).unwrap().center();
    let _ = arr.handle(&mv(hover), &doc, &reg);
    let (w, h) = size(ARR_BOUNDS);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    arr.paint(&mut canvas, &doc, &reg);
    write("arrangement.png", canvas);

    let mut roll = piano_roll(&doc, &reg);
    roll.set_scale(SCALE);
    roll.set_playhead(2 * BAR + PPQ);
    roll.set_selected_notes(&[4, 5, 6]);
    let (w, h) = size(ROLL_BOUNDS);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    roll.paint(&mut canvas, &doc, &reg);
    write("piano_roll.png", canvas);

    // The scale bar, the scale tint and the folds.
    let mut scaled = doc.clone();
    scaled.song.tracks[0].lanes[0].clips[0].scale = Some(Scale::new(0, ScaleKind::NaturalMinor));
    for (name, fold) in [
        ("piano_roll_scale.png", Fold::Off),
        ("piano_roll_scale_fold.png", Fold::Scale),
        ("piano_roll_used_fold.png", Fold::Used),
    ] {
        let mut roll = piano_roll(&scaled, &reg);
        roll.set_scale(SCALE);
        roll.set_fold(fold);
        roll.set_snap_to_scale(true);
        roll.set_stamp(Stamp::Chord(ChordKind::Minor));
        roll.set_selected_notes(&[4, 5]);
        let (w, h) = size(ROLL_BOUNDS);
        let mut canvas = CpuCanvas::new(w, h, &mut text);
        roll.paint(&mut canvas, &scaled, &reg);
        write(name, canvas);
    }

    let mut bar = transport_bar(&doc, &reg);
    bar.set_scale(SCALE);
    bar.set_state(TransportState {
        playing: true,
        recording: false,
        metronome: true,
        position: 2 * BAR + PPQ + PPQ / 2,
    });
    let _ = bar.handle(&mv(Point::new(0.0, 0.0)), &doc, &reg);
    let (w, h) = size(BAR_BOUNDS);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    bar.paint(&mut canvas, &doc, &reg);
    write("transport.png", canvas);

    // The recording input of an armed audio track, a peak held near full scale and the clip lamp
    // latched.
    let device = key("bass");
    bar.set_rec_input(Some(atomik_ui::RecInput {
        device: device.clone(),
        name: "Guitar".into(),
        gain_db: 6.5,
        clip_safe: true,
    }));
    for (peak, t) in [(0.9, 0.0), (1.2, 0.1), (0.35, 0.2)] {
        bar.feed_rec_peaks(&[(device.clone(), peak)], t);
    }
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    bar.paint(&mut canvas, &doc, &reg);
    write("transport_rec_input.png", canvas);
}
