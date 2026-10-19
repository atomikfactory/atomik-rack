//! PDC through `atomik.character` in a bounce:
//! identical clicks summed through parallel console channels stay sample-aligned at the master
//! when one of them goes through a Character, in both latency classes and across a class change
//! in the middle of the song.
//!
//! Each project has two identical click synths, "Click A" hard left (dry) and "Click B" hard
//! right (the Character on its insert, or inside a container for the automated preset). The
//! Character's declared latency at 48 kHz is **46 frames for the short class** (every preset but
//! one) and **1070 frames for the long class** (`lossy`, the spectral stage). At `mix` 0 the
//! device's output is its dry tap delayed by exactly that latency, so with compensation on both
//! clicks start at the same frame and with it off Click B starts later by the latency: the test
//! is sensitive to a missing or a wrong delay. At `mix` 1 the wet path is checked by the energy
//! centroid of the click (the filters of a preset move the edge, they do not move the
//! compensation).
//!
//! The mid-song case needs the bounce to follow a latency change the way the live app does: the
//! engine publishes a new latency epoch when the crossfade between two presets of different
//! classes ends, and the routing is recompiled with it one batch later (see `render::project`).

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand,
    Curve, DeviceKey, Document, MacroTarget, Note, RackDoc, SocketRef, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const BEAT: usize = 24_000;
/// The declared latencies at 48 kHz: `29 + ceil(0.3 ms x fs) + 2` for the short class and the
/// short class plus the 1024-frame spectral frame for the long one.
const SHORT: usize = 46;
const LONG: usize = 1070;
const LOSSY: f32 = 8.0;

#[test]
fn the_constants_are_the_devices_declared_latencies() {
    use atomik_devices::character::chain::{long_latency, short_latency};
    assert_eq!(short_latency(48_000.0) as usize, SHORT);
    assert_eq!(long_latency(48_000.0) as usize, LONG);
    // The other rates: (rate, short, long); the spectral frame doubles at 96 kHz.
    for (rate, short, long) in [(44_100.0, 45, 1069), (96_000.0, 60, 2108)] {
        assert_eq!(short_latency(rate), short, "{rate}");
        assert_eq!(long_latency(rate), long, "{rate}");
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

struct Builder {
    doc: Document,
}

impl Builder {
    fn apply(&mut self, c: Command) {
        let registry = default_registry();
        self.doc
            .apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn set(&mut self, k: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(k),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn click_synth(&mut self, k: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key(k)),
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
            self.set(k, p, v);
        }
    }

    fn channel(&self, source: &str) -> String {
        self.doc
            .doc()
            .console
            .channel_of(&key(source))
            .unwrap()
            .channel
            .0
            .clone()
    }

    fn clicks(&mut self, device: &str, length: i64, bars: i64) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        let notes = (0..bars * 4)
            .map(|b| Note {
                tick: b * PPQ,
                length: PPQ / 8,
                key: 84,
                velocity: 1.0,
            })
            .collect();
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: clip(length, ClipContent::Notes(notes)),
        }));
    }
}

fn clip(length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        content,
    }
}

/// The two-click project with a Character (`preset`, `mix`) on Click B's insert.
fn click_doc(preset: f32, mix: f32) -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };
    b.click_synth("click-a");
    b.click_synth("click-b");
    let (ch_a, ch_b) = (b.channel("click-a"), b.channel("click-b"));
    b.set(&ch_a, "pan", -1.0);
    b.set(&ch_b, "pan", 1.0);
    b.apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.character".into(),
        key: Some(key("fx")),
    });
    b.set("fx", "preset", preset);
    b.set("fx", "mix", mix);
    for side in ["l", "r"] {
        for (from, to) in [
            (
                sref(&ch_b, &format!("insert_send_{side}")),
                sref("fx", &format!("in_{side}")),
            ),
            (
                sref("fx", &format!("out_{side}")),
                sref(&ch_b, &format!("insert_return_{side}")),
            ),
        ] {
            b.apply(Command::Connect(Cable { from, to }));
        }
    }
    let bars = 2;
    let length = bars * 4 * PPQ;
    b.clicks("click-a", length, bars);
    b.clicks("click-b", length, bars);
    b.apply(Command::Song(SongCommand::SetSongEnd(length)));
    b.doc.doc().clone()
}

/// The two-click project where Click B is a container (a click synth and a Character, `mix` 0)
/// whose `macro1` is the preset: `reel` until beat `step_beat`, then `lossy` (a step lane).
fn crossing_doc(step_beat: i64, bars: i64) -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };
    b.click_synth("click-a");
    b.apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.container".into(),
        key: Some(key("box")),
    });
    for (type_id, local, auto_split) in [
        ("atomik.poly-synth", "syn", true),
        ("atomik.character", "chr", false),
    ] {
        b.apply(Command::Container(ContainerCommand::AddInner {
            container: key("box"),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(local)),
            auto_split,
        }));
    }
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
        b.set("box/syn", p, v);
    }
    b.set("box/chr", "mix", 0.0);
    for side in ["l", "r"] {
        for (from, to) in [
            (
                sref("box/syn", &format!("out_{side}")),
                sref("box/chr", &format!("in_{side}")),
            ),
            (
                sref("box/chr", &format!("out_{side}")),
                sref("box/boundary", &format!("out_{side}")),
            ),
        ] {
            b.apply(Command::Connect(Cable { from, to }));
        }
    }
    b.apply(Command::Container(ContainerCommand::SetMacro {
        container: key("box"),
        index: 0,
        target: MacroTarget {
            device: key("chr"),
            param: "preset".into(),
            min: 0.0,
            max: 15.0,
        },
        merge: None,
    }));
    b.apply(Command::Console(ConsoleCommand::AddChannel {
        source: key("box"),
    }));
    let (ch_a, ch_b) = (b.channel("click-a"), b.channel("box"));
    b.set(&ch_a, "pan", -1.0);
    b.set(&ch_b, "pan", 1.0);
    let length = bars * 4 * PPQ;
    b.clicks("click-a", length, bars);
    b.clicks("box", length, bars);
    b.apply(Command::Song(SongCommand::AddAutomationLane {
        device: key("box"),
        param: "macro1".into(),
    }));
    b.apply(Command::Song(SongCommand::AddClip {
        device: key("box"),
        lane: 1,
        clip: clip(
            length,
            ClipContent::Automation(vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.0,
                    curve: Curve::Step,
                },
                AutomationPoint {
                    tick: step_beat * PPQ,
                    value: LOSSY / 15.0,
                    curve: Curve::Step,
                },
            ]),
        ),
    }));
    b.apply(Command::Song(SongCommand::SetSongEnd(length)));
    b.doc.doc().clone()
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn render(doc: &RackDoc, block: usize, workers: u16) -> Vec<Vec<f32>> {
    render_doc(doc, &default_registry(), &options(block, workers))
        .unwrap()
        .channels
}

/// The first frame in `[from, from + BEAT / 2)` whose magnitude reaches half the window peak.
fn start_of_click(x: &[f32], from: usize) -> usize {
    let w = &x[from..from + BEAT / 2];
    let peak = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "a click at {from}: peak {peak}");
    from + w.iter().position(|v| v.abs() >= 0.5 * peak).unwrap()
}

/// The energy centroid of the click in `[from, from + BEAT / 2)`, in frames from `from`.
fn centroid(x: &[f32], from: usize) -> f64 {
    let w = &x[from..from + BEAT / 2];
    let (mut m0, mut m1) = (0.0f64, 0.0f64);
    for (i, v) in w.iter().enumerate() {
        let e = f64::from(*v).powi(2);
        m0 += e;
        m1 += e * i as f64;
    }
    assert!(m0 > 1e-6, "a click at {from}");
    m1 / m0
}

#[test]
fn compensation_aligns_the_clicks_in_both_latency_classes_and_off_it_offsets_by_the_latency() {
    for (name, preset, latency) in [
        ("reel, short class", 0.0, SHORT),
        ("camcorder, short class", 6.0, SHORT),
        ("lossy, long class", LOSSY, LONG),
    ] {
        let mut doc = click_doc(preset, 0.0);
        let on = render(&doc, 512, 0);
        doc.console.compensation = false;
        let off = render(&doc, 512, 0);
        for beat in 1..8 {
            let at = beat * BEAT;
            let (a_on, b_on) = (start_of_click(&on[0], at), start_of_click(&on[1], at));
            let (a_off, b_off) = (start_of_click(&off[0], at), start_of_click(&off[1], at));
            assert!(
                a_on.abs_diff(b_on) <= 1,
                "{name}, beat {beat}: aligned with compensation ({a_on} vs {b_on})"
            );
            assert!(
                (b_off as i64 - a_off as i64 - latency as i64).abs() <= 1,
                "{name}, beat {beat}: offset {} without compensation, want {latency}",
                b_off as i64 - a_off as i64
            );
        }
        eprintln!("{name}: aligned, {latency} frames of PDC");
    }
}

#[test]
fn the_wet_click_of_each_class_sits_where_the_compensation_puts_it() {
    // At mix 1 the click is the preset's own sound: its energy centroid must stay within the
    // preset's filter spread of the dry click (an uncompensated long class would be 1070 late).
    for (name, preset, tolerance) in [("reel", 0.0, 24.0), ("lossy", LOSSY, 24.0)] {
        let doc = click_doc(preset, 1.0);
        let on = render(&doc, 512, 0);
        let mut worst = 0.0f64;
        for beat in 1..8 {
            let at = beat * BEAT;
            let d = centroid(&on[1], at) - centroid(&on[0], at);
            worst = worst.max(d.abs());
            assert!(
                d.abs() <= tolerance,
                "{name}, beat {beat}: the wet click's centroid is {d:.1} frames from the dry one"
            );
        }
        eprintln!("{name}: wet centroid within {worst:.1} frames of the dry click");
    }
}

#[test]
fn a_class_change_in_the_song_realigns_the_clicks_and_the_bounce_does_not_depend_on_the_block() {
    // The preset steps from `reel` (46 frames) to `lossy` (1070) at beat 4; the click on each
    // later beat must still start with the dry one (the routing was recompiled with the new
    // latency), and the bounce must be the same for every host block and worker count.
    let doc = crossing_doc(4, 3);
    let reference = render(&doc, 512, 0);
    let mut worst = 0;
    for beat in (1..4).chain(6..12) {
        let at = beat * BEAT;
        let d = start_of_click(&reference[0], at).abs_diff(start_of_click(&reference[1], at));
        worst = worst.max(d);
        assert!(
            d <= 1,
            "beat {beat}: Click B starts {d} frames after Click A (before the step the delay is \
             {SHORT}, after it {LONG})"
        );
    }
    eprintln!("class change: worst misalignment {worst} frames");
    for (block, workers) in [(64, 0), (333, 2), (480, 3), (4096, 0)] {
        assert!(
            render(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn the_pdc_bounces_are_bit_identical_across_host_blocks_and_workers() {
    for (name, preset, mix) in [("reel", 0.0, 0.5), ("lossy", LOSSY, 0.5)] {
        let doc = click_doc(preset, mix);
        let reference = render(&doc, 512, 0);
        for block in [64, 333, 4096] {
            for workers in [0, 3] {
                assert!(
                    render(&doc, block, workers) == reference,
                    "{name}: block {block}, workers {workers}"
                );
            }
        }
    }
}
