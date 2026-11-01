//! Benchmark-style test: playback frames of a 64-track, 2,000-clip song at 1080p, recorded and
//! encoded with and without the cached lane and track list layers.
//! Run: `cargo test -p atomik-ui --release --test bench_arrangement -- --ignored --nocapture`.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

mod support;

use atomik_model::stress::large_song_doc;
use atomik_ui::kurbo::Rect;
use atomik_ui::{
    ArrangementView, DisplayList, SceneCache, SceneCanvas, TextEngine, Theme, timeline::Snap,
};
use std::time::Instant;
use support::*;

#[test]
#[ignore = "benchmark; run explicitly in release"]
fn playback_frames_of_a_dense_song() {
    let reg = registry();
    let doc = large_song_doc(&reg, 64, 2_000, 20_000);
    let mut v = ArrangementView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    v.sync(&doc, &reg);
    v.set_snap(Snap::Bar);
    let mut text = TextEngine::new();
    let mut cache = SceneCache::new();
    let mut scene = vello::Scene::new();
    let frames = 120_i64;
    for (label, layers) in [("layers off", false), ("layers cached", true)] {
        v.set_layer_cache(layers);
        v.set_playhead(0);
        v.request_repaint();
        let mut recorded = DisplayList::new();
        v.paint(&mut recorded, &doc, &reg);
        let mut record_only = 0.0;
        let mut encode = 0.0;
        for f in 1..=frames {
            v.set_playhead(f * 9_000);
            let t = Instant::now();
            v.paint(&mut DisplayList::new(), &doc, &reg);
            record_only += t.elapsed().as_secs_f64();
            v.set_playhead(f * 9_000 + 4_500);
            let t = Instant::now();
            scene.reset();
            {
                let mut c = SceneCanvas::new(&mut scene, &mut text).with_cache(&mut cache);
                v.paint(&mut c, &doc, &reg);
            }
            cache.trim();
            encode += t.elapsed().as_secs_f64();
        }
        println!(
            "{label}: record {:.3} ms/frame, record + encode {:.3} ms/frame, {} ops",
            record_only * 1000.0 / frames as f64,
            encode * 1000.0 / frames as f64,
            recorded.len()
        );
    }
}
