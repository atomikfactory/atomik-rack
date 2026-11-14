//! Feedback routing in a bounce: the Feedback Route is a delay of at least one engine batch
//! (64 frames) that makes a loop legal. A click through it comes back exactly `delay` frames
//! later, a loop through it rings with the gain of the loop, and the render is bit-identical at
//! every host block size and with any number of workers (the delay is counted in engine batches,
//! never in host buffers).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SocketRef,
    SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

/// How the click is routed on the insert of its channel.
#[derive(Clone, Copy)]
enum Route {
    /// Nothing on the insert.
    Dry,
    /// The insert goes through a Feedback Route of `time` ms: a plain delay, no loop.
    Delay(f32),
    /// A gain tool that mixes in (`blend`) what a Feedback Route of `time` ms and `gain` dB
    /// brings back from the gain tool's own output: a loop.
    Loop { time: f32, gain: f32, blend: f32 },
}

/// One click synth, one click at the start of the song, with `route` on its channel's insert.
fn click_doc(route: Route) -> RackDoc {
    let registry = default_registry();
    let mut doc = Document::new(empty_doc());
    let mut apply = |c: Command| {
        doc.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    let set = |k: &str, p: &str, v: f32| Command::SetParam {
        key: key(k),
        param: p.into(),
        value: v,
        merge: None,
    };
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("click")),
    });
    for (p, v) in [
        ("osc1.wave", 1.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.resonance", 0.0),
        ("env.amp.attack", 0.5),
        ("env.amp.decay", 25.0),
        ("env.amp.sustain", 0.0),
        ("env.amp.release", 0.5),
        ("voices", 1.0),
    ] {
        apply(set("click", p, v));
    }
    apply(Command::Song(SongCommand::AddTrack {
        device: key("click"),
    }));
    let channel = channel_of(&doc, "click");
    let mut apply = |c: Command| {
        doc.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    let add = |type_id: &str, k: &str| Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    };
    let wire = |from: SocketRef, to: SocketRef| Command::Connect(Cable { from, to });
    match route {
        Route::Dry => {}
        Route::Delay(time) => {
            apply(add("atomik.feedback", "fb"));
            apply(set("fb", "time", time));
            for side in ["l", "r"] {
                apply(wire(
                    sref(&channel, &format!("insert_send_{side}")),
                    sref("fb", &format!("in_{side}")),
                ));
                apply(wire(
                    sref("fb", &format!("out_{side}")),
                    sref(&channel, &format!("insert_return_{side}")),
                ));
            }
        }
        Route::Loop { time, gain, blend } => {
            apply(add("atomik.gain-tool", "mix"));
            apply(set("mix", "mode", 1.0));
            apply(set("mix", "blend", blend));
            apply(add("atomik.feedback", "fb"));
            apply(set("fb", "time", time));
            apply(set("fb", "gain", gain));
            for side in ["l", "r"] {
                for (from, to) in [
                    (
                        sref(&channel, &format!("insert_send_{side}")),
                        sref("mix", &format!("in_{side}")),
                    ),
                    (
                        sref("mix", &format!("out_{side}")),
                        sref(&channel, &format!("insert_return_{side}")),
                    ),
                    (
                        sref("mix", &format!("out_{side}")),
                        sref("fb", &format!("in_{side}")),
                    ),
                    (
                        sref("fb", &format!("out_{side}")),
                        sref("mix", &format!("aux_{side}")),
                    ),
                ] {
                    apply(wire(from, to));
                }
            }
        }
    }
    let length = 2 * 4 * PPQ;
    apply(Command::Song(SongCommand::AddClip {
        device: key("click"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(vec![Note::new(0, PPQ / 8, 84, 1.0)].into()),
        },
    }));
    apply(Command::Song(SongCommand::SetSongEnd(length)));
    doc.doc().clone()
}

fn channel_of(doc: &Document, source: &str) -> String {
    doc.doc()
        .console
        .channel_of(&key(source))
        .unwrap()
        .channel
        .0
        .clone()
}

fn render(doc: &RackDoc, block: usize, workers: u16) -> Vec<Vec<f32>> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    common::render_guarded(doc, &default_registry(), &options)
        .unwrap()
        .channels
}

fn bits(channels: &[Vec<f32>]) -> Vec<u32> {
    channels.iter().flatten().map(|x| x.to_bits()).collect()
}

/// The first frame whose magnitude is over a thousandth.
fn onset(x: &[f32]) -> usize {
    x.iter().position(|v| v.abs() > 1e-3).expect("a click")
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn a_click_through_a_feedback_delay_comes_back_exactly_the_delay_later() {
    let dry = render(&click_doc(Route::Dry), 512, 0);
    let start = onset(&dry[0]);
    assert!(peak(&dry[0]) > 0.01, "the click sounds");
    // The shortest delay is one engine batch; a time is rounded to whole frames.
    for (time, frames) in [(0.0, 64), (0.5, 64), (10.0, 480), (100.0, 4800)] {
        for block in [64, 333] {
            let wet = render(&click_doc(Route::Delay(time)), block, 0);
            assert_eq!(
                onset(&wet[0]),
                start + frames,
                "time {time} ms at block {block}"
            );
            assert_eq!(onset(&wet[1]), onset(&dry[1]) + frames);
        }
    }
}

#[test]
fn a_loop_through_the_delay_rings_with_the_gain_of_the_loop() {
    // Every 100 ms (4800 frames) the click comes back at half its level: the mix is 0.5.
    let doc = click_doc(Route::Loop {
        time: 100.0,
        gain: 0.0,
        blend: 0.5,
    });
    let out = render(&doc, 256, 0);
    let window = |k: usize| peak(&out[0][k * 4800..(k + 1) * 4800]);
    let first = window(0);
    assert!(first > 0.01, "the click sounds: {first}");
    for (k, want) in [(1, 0.5), (2, 0.25), (3, 0.125)] {
        let ratio = window(k) / first;
        assert!(
            (ratio - want).abs() < 0.03,
            "echo {k}: {ratio} instead of {want}"
        );
    }
    // The same loop at -6 dB feeds back half as much again.
    let quiet = render(
        &click_doc(Route::Loop {
            time: 100.0,
            gain: -6.0206,
            blend: 0.5,
        }),
        256,
        0,
    );
    let ratio = peak(&quiet[0][4800..9600]) / peak(&quiet[0][..4800]);
    assert!((ratio - 0.25).abs() < 0.03, "{ratio}");
}

#[test]
fn a_loop_renders_the_same_bits_at_every_block_size_and_worker_count() {
    for route in [
        // The shortest delay: the loop turns over in every batch.
        Route::Loop {
            time: 0.0,
            gain: -3.0,
            blend: 0.7,
        },
        Route::Loop {
            time: 37.0,
            gain: -1.0,
            blend: 0.6,
        },
        Route::Delay(0.0),
    ] {
        let doc = click_doc(route);
        let reference = render(&doc, 64, 0);
        assert!(peak(&reference[0]) > 0.01);
        assert!(reference.iter().flatten().all(|x| x.is_finite()));
        let ref_bits = bits(&reference);
        for block in [1, 64, 333, 4096] {
            for workers in [0, 2] {
                assert!(
                    bits(&render(&doc, block, workers)) == ref_bits,
                    "block {block}, workers {workers}"
                );
            }
        }
    }
}
