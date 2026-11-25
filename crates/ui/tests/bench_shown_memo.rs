//! Benchmark-style test: paints of a stopped rack of 100 devices while one automation lane
//! shows a value, with and without the revision of the document declared (without it the
//! document with the shown values is built on every paint).
//! Run: `cargo test -p atomik-ui --release --test bench_shown_memo -- --ignored --nocapture`.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

mod support;

use atomik_model::ShownValues;
use atomik_ui::kurbo::Size;
use atomik_ui::{DisplayList, RackView, Theme, UiEvent};
use std::time::Instant;
use support::*;

atomik_testkit::install_alloc_guard!();

#[test]
#[ignore = "benchmark; run explicitly in release"]
fn stopped_rack_with_a_shown_lane() {
    let reg = registry();
    let types = [
        "atomik.poly-synth",
        "atomik.stereo-delay",
        "atomik.line-mixer",
    ];
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    for i in 0..100 {
        doc.devices.push(device(
            &format!("d{i}"),
            types[i % 3],
            &format!("Device {i}"),
        ));
    }
    let mut shown = ShownValues::new();
    shown.insert((key("d0"), "filter.cutoff".to_string()), 1234.0);
    let frames = 200u32;
    for declared in [false, true] {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(1920.0, 1080.0)), &doc, &reg);
        v.set_shown_values(&shown);
        let paint = |v: &mut RackView| {
            v.set_doc_revision(declared.then_some(7));
            v.request_repaint();
            v.paint(&mut DisplayList::new(), &doc, &reg);
        };
        paint(&mut v);
        paint(&mut v);
        let t = Instant::now();
        let ((), calls) = atomik_testkit::no_alloc::allocations_of(|| {
            for _ in 0..frames {
                paint(&mut v);
            }
        });
        let ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
        println!(
            "shown lane, revision {}: {ms:.3} ms/paint, {} allocations/paint",
            if declared {
                "declared (kept)"
            } else {
                "not declared (built each paint)"
            },
            calls / u64::from(frames)
        );
    }
}
