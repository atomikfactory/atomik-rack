//! Benchmark-style test: 100 devices and 300 cables.
//! Run: `cargo test -p atomik-ui --release --test bench -- --ignored --nocapture`.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

mod support;

use atomik_device_sdk::{Direction, Registry};
use atomik_model::{Cable, RackDoc, SocketRef};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{DisplayList, Flip, RackView, SceneCache, SceneCanvas, TextEngine, Theme, UiEvent};
use std::time::Instant;
use support::*;

fn big_rack(reg: &Registry) -> RackDoc {
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
    let mut outs = Vec::new();
    let mut ins = Vec::new();
    for d in &doc.devices {
        for s in reg.get(&d.type_id).unwrap().descriptor.sockets {
            let r = SocketRef {
                device: d.key.clone(),
                socket: s.key.to_string(),
            };
            match s.direction {
                Direction::Output => outs.push((r, s.kind)),
                Direction::Input | Direction::SummingInput => ins.push((r, s.kind)),
            }
        }
    }
    let mut used = vec![false; ins.len()];
    let mut k = 0;
    for (i, (from, kind)) in outs.iter().cycle().enumerate() {
        if doc.cables.len() == 300 || i > 100_000 {
            break;
        }
        k = (k + 37) % ins.len();
        let (to, tk) = &ins[k];
        if !used[k] && tk == kind && to.device != from.device {
            used[k] = true;
            doc.cables.push(Cable {
                from: from.clone(),
                to: to.clone(),
            });
        }
    }
    doc.cables.sort();
    doc
}

fn ms(start: Instant, frames: u32) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames)
}

#[test]
#[ignore = "benchmark; run explicitly in release"]
fn hundred_devices_three_hundred_cables() {
    let reg = registry();
    let doc = big_rack(&reg);
    assert_eq!(doc.cables.len(), 300);
    for (label, size, scale) in [
        ("1920x1080 @1x", Size::new(1920.0, 1080.0), 1.0),
        ("1920x1080 @2x", Size::new(1920.0, 1080.0), 2.0),
        (
            "whole rack visible (zoom 0.5)",
            Size::new(1920.0, 20_000.0),
            1.0,
        ),
    ] {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(size), &doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), &doc, &reg);
        if size.height > 10_000.0 {
            v.set_zoom(0.5);
        }
        v.set_flip(Flip::Back);
        let t = Instant::now();
        v.paint(&mut DisplayList::new(), &doc, &reg);
        let cold = ms(t, 1);
        let frames = 50;
        let t = Instant::now();
        let mut ops = 0;
        for f in 0..frames {
            // Move the pointer so hover/cable hit testing runs every frame too.
            let _ = v.handle(
                &UiEvent::PointerMove {
                    pos: Point::new(400.0 + f64::from(f), 300.0),
                    modifiers: Default::default(),
                },
                &doc,
                &reg,
            );
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &doc, &reg);
            ops = dl.len();
        }
        let warm = ms(t, frames);
        // The GPU path: encode into a vello Scene with cached per-device scenes.
        let mut text = TextEngine::new();
        let mut cache = SceneCache::new();
        let mut scene = vello::Scene::new();
        let t = Instant::now();
        {
            let mut c = SceneCanvas::new(&mut scene, &mut text).with_cache(&mut cache);
            v.paint(&mut c, &doc, &reg);
        }
        let scene_cold = ms(t, 1);
        let t = Instant::now();
        for _ in 0..frames {
            scene.reset();
            let mut c = SceneCanvas::new(&mut scene, &mut text).with_cache(&mut cache);
            v.request_repaint();
            v.paint(&mut c, &doc, &reg);
            cache.trim();
        }
        let scene_warm = ms(t, frames);
        println!(
            "{label}: recorder cold {cold:.2} ms, warm {warm:.3} ms/frame ({ops} ops); \
             vello scene cold {scene_cold:.2} ms, warm {scene_warm:.3} ms/frame"
        );
    }
}
