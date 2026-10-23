//! Waveforms below the finest peak level: the arrangement's audio clips, the sample editor and
//! the slice editor read the samples of the asset when the view holds them, and draw a line
//! through them (with a dot on every sample) below one frame per pixel. The work of a paint
//! stays bounded by the width of the view.

mod support;

use atomik_model::Document;
use atomik_ui::canvas::Geom;
use atomik_ui::display_list::Op;
use atomik_ui::kurbo::{PathEl, Point, Vec2};
use atomik_ui::{ArrangementView, DisplayList, Modifiers, UiEvent};
use std::sync::Arc;
use support::audio::*;
use support::song::{ARR_BOUNDS, BAR};
use support::*;

type Samples = Arc<[Vec<f32>]>;

fn samples(frames: u64, channels: usize) -> Samples {
    (0..channels)
        .map(|c| wave(frames as usize, c))
        .collect::<Vec<_>>()
        .into()
}

/// The points of every sample line (the strokes of width 1.25) and the dot count.
fn lines_and_dots(dl: &DisplayList) -> (Vec<usize>, usize) {
    let mut lines = Vec::new();
    let mut dots = 0;
    for op in dl.ops() {
        match op {
            Op::Stroke {
                shape: Geom::Path(p),
                stroke,
                ..
            } if (stroke.width - 1.25).abs() < 1e-9 => lines.push(p.elements().len()),
            Op::Fill {
                shape: Geom::Circle(_),
                ..
            } => dots += 1,
            _ => {}
        }
    }
    (lines, dots)
}

fn deep(v: &mut ArrangementView, ppt: f64) {
    v.set_px_per_tick(ppt);
    v.set_scroll_ticks((BAR + 2_000) as f64);
}

fn paint(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl
}

#[test]
fn the_arrangement_draws_a_line_with_dots_at_the_deepest_zoom() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    deep(&mut v, 16.0);
    let (lines, dots) = lines_and_dots(&paint(&mut v, &doc, &reg));
    assert_eq!(lines.len(), 2, "one line per channel");
    let width = ARR_BOUNDS.width();
    for n in &lines {
        // One point per visible frame (10 px each) plus the margin: bounded by the width.
        assert!(*n > 40 && (*n as f64) < width / 5.0, "{n} points");
    }
    assert!((80..400).contains(&dots), "{dots} dots");
}

#[test]
fn a_frame_narrower_than_six_pixels_has_the_line_but_no_dots() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    // 1.5625 frames per tick: 5 px per tick is 3.2 px per frame, under the dot size.
    deep(&mut v, 5.0);
    let (lines, dots) = lines_and_dots(&paint(&mut v, &doc, &reg));
    assert_eq!(lines.len(), 2);
    assert_eq!(dots, 0);
}

#[test]
fn between_one_frame_and_the_finest_bucket_the_columns_read_the_samples() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    // 100 frames per pixel: 1 px is 64 ticks.
    deep(&mut v, 1.0 / 64.0);
    let without = paint(&mut v, &doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    let with = paint(&mut v, &doc, &reg);
    assert_ne!(with, without, "finer than the 256-frame buckets");
    assert_eq!(lines_and_dots(&with), (Vec::new(), 0), "no line yet");
    assert_eq!(with.len(), without.len(), "the same shapes, finer numbers");
    // The coarse zoom does not read the samples at all.
    deep(&mut v, 0.002);
    let a = paint(&mut v, &doc, &reg);
    v.clear_peaks(HASH_A);
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    let b = paint(&mut v, &doc, &reg);
    assert_eq!(a, b, "above the finest level the pyramid draws");
}

#[test]
fn without_samples_the_deepest_zoom_keeps_the_finest_peak_level() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    deep(&mut v, 16.0);
    let dl = paint(&mut v, &doc, &reg);
    assert_eq!(lines_and_dots(&dl), (Vec::new(), 0));
    let fills = dl
        .ops()
        .iter()
        .filter(|o| {
            matches!(
                o,
                Op::Fill {
                    shape: Geom::Path(_),
                    ..
                }
            )
        })
        .count();
    assert!(fills > 0, "the blocky min/max polygons are still drawn");
}

#[test]
fn clearing_the_peaks_forgets_the_samples_too() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    deep(&mut v, 16.0);
    assert!(!lines_and_dots(&paint(&mut v, &doc, &reg)).0.is_empty());
    v.clear_peaks(HASH_A);
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    assert!(lines_and_dots(&paint(&mut v, &doc, &reg)).0.is_empty());
}

#[test]
fn the_paint_cost_does_not_grow_with_the_zoom_or_the_clip() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    let mut most = 0;
    for ppt in [16.0, 8.0, 2.0, 0.5, 0.1, 0.02, 0.004, 0.001] {
        deep(&mut v, ppt);
        let n = paint(&mut v, &doc, &reg).len();
        most = most.max(n);
    }
    // A fixed number of ops per element, not per frame of the 8 s asset.
    assert!(most < 3_000, "{most} ops");
}

#[test]
fn the_line_places_every_sample_at_its_real_time() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_samples(HASH_A, samples(FRAMES_A, 2));
    deep(&mut v, 16.0);
    let path = |dl: &DisplayList| -> Vec<Point> {
        dl.ops()
            .iter()
            .find_map(|o| match o {
                Op::Stroke {
                    shape: Geom::Path(p),
                    stroke,
                    ..
                } if (stroke.width - 1.25).abs() < 1e-9 => Some(
                    p.elements()
                        .iter()
                        .filter_map(|e| match e {
                            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap()
    };
    let pts = path(&paint(&mut v, &doc, &reg));
    // The spacing of the points is the distance of two source frames at this zoom.
    let dx: Vec<f64> = pts.windows(2).map(|w| w[1].x - w[0].x).collect();
    let mean = dx.iter().sum::<f64>() / dx.len() as f64;
    assert!((mean - 16.0 / 1.5625).abs() < 0.05, "{mean}");
    assert!(dx.iter().all(|d| (d - mean).abs() < 0.05));
}

// ---------------------------------------------------------------- the editors

#[test]
fn the_sample_editor_zooms_to_a_frame_of_32_pixels_and_draws_the_line() {
    use support::sampler::*;
    let (doc, reg) = (
        sampler_doc(vec![atomik_model::ZoneDoc::new(S_A, FR_A)]),
        registry(),
    );
    let d = Document::new(doc);
    let mut v = editor(d.doc(), &reg);
    v.set_samples(S_A, samples(FR_A, 2));
    v.sync(d.doc(), &reg);
    v.set_selected_zone(Some(0));
    let w = v.waveform_rect();
    let pos = Point::new(w.center().x, w.center().y);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    assert_eq!(lines_and_dots(&dl).0.len(), 0, "fitted: the pyramid draws");
    for _ in 0..40 {
        let _ = v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, 5.0),
                pos,
                modifiers: Modifiers::CTRL,
            },
            d.doc(),
            &reg,
        );
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let (lines, dots) = lines_and_dots(&dl);
    assert_eq!(lines.len(), 2);
    assert!(dots > 0, "a frame is 32 px wide at the deepest zoom");
    assert!(
        lines.iter().all(|n| (*n as f64) < w.width() / 20.0),
        "{lines:?}"
    );
}

#[test]
fn the_slice_editor_draws_the_line_at_the_deepest_zoom() {
    use support::slicer::*;
    let (doc, reg) = (slicer_doc(sliced_zones(&MARKERS)), registry());
    let mut v = editor(&doc, &reg);
    v.set_samples(LOOP, samples(LOOP_FRAMES, 2));
    let w = v.waveform_rect();
    let pos = Point::new(w.center().x, w.center().y);
    for _ in 0..40 {
        let _ = v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, 5.0),
                pos,
                modifiers: Modifiers::CTRL,
            },
            &doc,
            &reg,
        );
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let (lines, dots) = lines_and_dots(&dl);
    assert_eq!(lines.len(), 2);
    assert!(dots > 0);
}
