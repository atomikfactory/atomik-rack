//! While playing, every automated parameter receives, at each batch
//! boundary, the curve's value at that batch's start tick (linear or step, held before the
//! first and after the last point, clamped by the descriptor), within 1e-6 of the analytic
//! curve — across a tempo change and loop wraps. Nothing is applied while stopped.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, Ticks};
use atomik_engine::{
    Graph, SongAutomation, SongCurve, SongInput, SongPoint, SongTrack, TransportCommand,
    compile_song,
};
use atomik_testkit::fakes::{ParamLog, ParamLogEntry};
use common::song::*;
use common::*;

fn point(tick: i64, value: f32, curve: SongCurve) -> SongPoint {
    SongPoint {
        tick: Ticks(tick),
        value,
        curve,
    }
}

/// 120 BPM until 3/2 beat (song frame 36000), then 90 BPM (0.48 ticks per frame).
fn tick_at(frame: u64) -> f64 {
    let f = frame as f64;
    if f <= 36_000.0 {
        f / 1.5625
    } else {
        23_040.0 + (f - 36_000.0) * 0.48
    }
}

fn curve_a(t: f64) -> f64 {
    let q = PPQ as f64;
    if t < q {
        0.2 + 0.6 * t.max(0.0) / q
    } else if t < 2.0 * q {
        0.8 // step
    } else if t < 2.25 * q {
        0.4 + 1.1 * (t - 2.0 * q) / (0.25 * q)
    } else {
        1.5
    }
}

fn curve_b(t: f64) -> f64 {
    let q = PPQ as f64;
    if t < 0.5 * q {
        -5.0
    } else if t < 1.5 * q {
        -5.0 + 10.0 * (t - 0.5 * q) / q
    } else {
        5.0
    }
}

#[test]
fn ac_s4_automation_matches_the_curve_at_every_batch_start() {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let (dev, log) = ParamLog::boxed(20_000);
    let id = g.add_device(dev);
    let (mut e, mut h) = engine_with(c, 0, None);
    h.submit_plan(g.compile().unwrap()).unwrap();
    let lin = SongCurve::Linear;
    let track = SongTrack {
        device: id,
        notes_socket: ParamLog::NOTES_IN,
        chase: true,
        notes: Vec::new(),
        controllers: Vec::new(),
        automation: vec![
            SongAutomation {
                param: ParamLog::A,
                points: vec![
                    point(2 * PPQ, 0.4, lin),
                    point(0, 0.2, lin),
                    point(PPQ, 0.8, SongCurve::Step),
                    point(9 * PPQ / 4, 1.5, lin), // above the descriptor max (1.0)
                ],
            },
            SongAutomation {
                param: ParamLog::B,
                points: vec![point(PPQ / 2, -5.0, lin), point(3 * PPQ / 2, 5.0, lin)],
            },
        ],
    };
    let input = SongInput {
        tracks: vec![track],
        tempo: tempo(&[(0, 120.0), (3 * PPQ / 2, 90.0)]),
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    };
    let snap = compile_song(&input).unwrap();
    // Loop [1/2 beat, 5/2 beats) = song frames [12000, 68000).
    let (ls, le) = (
        snap.song_frame(Ticks(PPQ / 2), SR),
        snap.song_frame(Ticks(5 * PPQ / 2), SR),
    );
    assert_eq!((ls, le), (12_000, 68_000));
    h.submit_song(snap).unwrap();
    // Stopped: nothing is applied.
    run(&mut e, 640, 64);
    assert!(log.applied(ParamLog::A).is_empty());
    h.transport(TransportCommand::SetLoop {
        start: Ticks(PPQ / 2),
        end: Ticks(5 * PPQ / 2),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    let batches = 2_000u64;
    run(&mut e, (batches * 64) as usize, 333);
    h.transport(TransportCommand::Stop).unwrap();
    run(&mut e, 6_400, 64);
    // Expected batch-start positions, playing from frame 640.
    let mut pos = 0u64;
    let mut want = Vec::new();
    for k in 0..batches {
        want.push((640 + 64 * k, pos));
        pos += 64;
        if pos > le {
            pos = ls + (pos - le);
        } else if pos == le {
            pos = ls;
        }
    }
    let a = log.applied(ParamLog::A);
    let b = log.applied(ParamLog::B);
    // A value equal to the one already applied is not sent again (plateaus, the held ends), but
    // the first batch and the first batch after every loop wrap always send one; none after stop.
    assert!(a.len() < batches as usize && b.len() < batches as usize);
    assert!(
        a.len() > 500 && b.len() > 500,
        "moving stretches send each batch"
    );
    assert!(a.iter().all(|e| e.0 >= 640 && e.0 < 640 + 64 * batches));
    // What the device holds at each batch start: the latest value sent at or before it.
    let held =
        |log: &[(u64, f32)], frame: u64| log.iter().rev().find(|e| e.0 <= frame).map(|e| e.1);
    let mut wrapped = false;
    for (k, &(frame, pos)) in want.iter().enumerate() {
        let t = tick_at(pos);
        let (wa, wb) = (curve_a(t).clamp(0.0, 1.0), curve_b(t));
        let (ha, hb) = (held(&a, frame).unwrap(), held(&b, frame).unwrap());
        assert!(
            (f64::from(ha) - wa).abs() <= 1e-6,
            "a at {frame} (tick {t}): {ha} vs {wa}",
        );
        assert!(
            (f64::from(hb) - wb).abs() <= 1e-6,
            "b at {frame} (tick {t}): {hb} vs {wb}",
        );
        if k > 0 && pos < want[k - 1].1 {
            wrapped = true;
            assert!(a.iter().any(|e| e.0 == frame), "a is sent again at a wrap");
            assert!(b.iter().any(|e| e.0 == frame), "b is sent again at a wrap");
        }
    }
    assert!(wrapped, "the run covers loop wraps");
    assert!(
        a.iter().any(|&(_, v)| v.to_bits() == 1.0f32.to_bits()),
        "clamped values were reached"
    );
    // Every Set entry came before its batch's process call.
    assert!(matches!(
        log.entries().last(),
        Some(ParamLogEntry::Process(_))
    ));
}
