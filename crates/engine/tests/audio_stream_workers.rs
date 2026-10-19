//! QA gap fill for / HI-1 / HI-2: streamed audio and a running record tap
//! with engine workers. Two audio tracks play (one reads streamed assets, one memory assets)
//! while a record tap copies two host inputs, under the allocation guard on the audio thread
//! (`assert_no_alloc` around every render) and on every worker (`forbid_on_this_thread`).
//!
//! The output must be bit-identical to the all-memory render and to the render without
//! workers, the streamer must keep up (no underruns), and the tap must return the host input
//! bit-exactly with its frame tags. Own binary: the worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::Ticks;
use atomik_devices::audio_track;
use atomik_engine::{AssetId, RecordTap, SongAsset, StreamSource, TransportCommand, compile_song};
use atomik_testkit::no_alloc::{forbid_on_this_thread, worker_violations};
use common::audio::*;
use common::hot::Rig;
use common::*;
use std::sync::Arc;
use std::time::Duration;

const PLAY_FRAMES: usize = 24_000;

/// A fake file served from memory (on the streamer thread only).
struct FakeFile(Arc<[Vec<f32>]>);

impl StreamSource for FakeFile {
    fn read(&self, start_frame: u64, out: &mut [Vec<f32>]) -> usize {
        let frames = self.0[0].len() as u64;
        let n = (out[0].len() as u64).min(frames.saturating_sub(start_frame)) as usize;
        for (o, d) in out.iter_mut().zip(self.0.iter()) {
            o[..n].copy_from_slice(&d[start_frame as usize..start_frame as usize + n]);
        }
        n
    }
}

fn assets(streamed: bool) -> Vec<SongAsset> {
    let stereo = planes(2, 20_000);
    let mono = planes(1, 30_000);
    if streamed {
        vec![
            SongAsset::stream(
                20_000,
                2,
                Arc::new(FakeFile(Arc::clone(&stereo))) as Arc<dyn StreamSource>,
            ),
            SongAsset::stream(
                30_000,
                1,
                Arc::new(FakeFile(Arc::clone(&mono))) as Arc<dyn StreamSource>,
            ),
        ]
    } else {
        vec![SongAsset::memory(stereo), SongAsset::memory(mono)]
    }
}

fn host_input(channel: usize, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| ((i * 11 + channel * 17) % 3_001) as f32 * 1.0e-3 - 1.5)
        .collect()
}

/// Play-phase output, drained tap runs `(start frame, planar frames)`, and the host input fed.
type Session = (Vec<Vec<f32>>, Vec<(u64, Vec<Vec<f32>>)>, Vec<Vec<f32>>);

/// Runs the scripted session; returns the play-phase output, the drained tap runs and the
/// frames fed to the host input (zeros while priming).
fn run(streamed: bool, threads: u16, block: usize) -> Session {
    let c = cfg(2, 4);
    let mut rig = Rig::new(&c);
    for k in 0..2u32 {
        let id = rig.insert(0, k + 1, audio_track::factory(), &[]);
        rig.wire_outputs(
            id,
            &format!("out_{}", 2 * k + 1),
            &format!("out_{}", 2 * k + 2),
        );
    }
    rig.route(0);
    let baseline = worker_violations();
    let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
    for e in rig.edits {
        handle.schedule(0, e.edit).unwrap();
    }
    handle
        .transport(TransportCommand::SetLoop {
            start: Ticks(tick_of(6_000)),
            end: Ticks(tick_of(9_500)),
            enabled: true,
        })
        .unwrap();
    let mut r0 = region(1_000, 300, 8_000);
    r0.gain = 0.6;
    r0.fade_in = 1_500;
    r0.fade_out = 2_500;
    r0.fade_curve = atomik_engine::SongFadeCurve::EqualPower;
    let mut r1 = region(2_000, 0, 6_000);
    r1.asset = AssetId(1);
    r1.gain = 0.9;
    let input = audio_input(
        vec![audio_track_of(1, vec![r0]), audio_track_of(2, vec![r1])],
        assets(streamed),
    );
    handle.submit_song(compile_song(&input).unwrap()).unwrap();

    let total = 8_000 + PLAY_FRAMES + 64;
    let fed = [host_input(0, total), host_input(1, total)];
    let mut cursor = 0usize;
    let mut tap = handle.record_tap(&[0, 1], 8_192).unwrap();
    let mut runs: Vec<(u64, Vec<Vec<f32>>)> = Vec::new();
    let mut step = |engine: &mut atomik_engine::Engine,
                    tap: &mut RecordTap,
                    n: usize,
                    cursor: &mut usize|
     -> Vec<Vec<f32>> {
        let ins: Vec<Vec<f32>> = fed
            .iter()
            .map(|ch| ch[*cursor..*cursor + n].to_vec())
            .collect();
        let out = render_block(engine, &ins, 4, n);
        *cursor += n;
        loop {
            let mut buf = Vec::new();
            match tap.drain(&mut buf) {
                Some((start, _)) => runs.push((start, buf)),
                None => break,
            }
        }
        out
    };
    // Prime the streamer while stopped.
    let mut primed = false;
    for _ in 0..5_000 {
        step(&mut engine, &mut tap, 64, &mut cursor);
        if handle.stream_primed() {
            primed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(primed, "the streamer never primed");
    handle.transport(TransportCommand::Play).unwrap();
    let mut out = vec![Vec::new(); 4];
    let mut done = 0;
    while done < PLAY_FRAMES {
        let n = block.min(PLAY_FRAMES - done);
        let o = step(&mut engine, &mut tap, n, &mut cursor);
        for (acc, ch) in out.iter_mut().zip(o) {
            acc.extend_from_slice(&ch);
        }
        done += n;
    }
    assert_eq!(handle.stream_underruns(), 0, "the streamer kept up");
    assert_eq!(tap.overflow_frames(), 0, "the tap ring never overflowed");
    assert_eq!(
        worker_violations() - baseline,
        0,
        "no worker allocated (streamed {streamed}, threads {threads}, block {block})"
    );
    handle.collect_garbage();
    drop(engine);
    drop(handle);
    let fed_so_far: Vec<Vec<f32>> = fed.iter().map(|c| c[..cursor].to_vec()).collect();
    (out, runs, fed_so_far)
}

#[test]
fn streaming_and_recording_with_workers_are_allocation_free_and_bit_identical() {
    let (reference, _, _) = run(false, 0, 64);
    assert!(
        reference[0].iter().any(|s| s.abs() > 1e-3) && reference[2].iter().any(|s| s.abs() > 1e-3),
        "both tracks are audible"
    );
    for (streamed, threads, block) in [
        (true, 0u16, 64usize),
        (true, 3, 64),
        (true, 3, 333),
        (false, 3, 64),
    ] {
        let (out, runs, fed) = run(streamed, threads, block);
        assert_eq!(
            bits(&out),
            bits(&reference),
            "streamed {streamed}, threads {threads}, block {block}"
        );
        // The tap returned the host input bit-exactly, in contiguous runs with exact tags.
        let mut expect = runs.first().map_or(0, |r| r.0);
        let mut got = [Vec::new(), Vec::new()];
        for (start, run) in &runs {
            assert_eq!(*start, expect, "contiguous tags");
            expect += run[0].len() as u64;
            for (g, r) in got.iter_mut().zip(run) {
                g.extend_from_slice(r);
            }
        }
        assert!(
            got[0].len() >= PLAY_FRAMES,
            "the tap saw the whole play phase"
        );
        let first = runs[0].0 as usize;
        for ch in 0..2 {
            let want = &fed[ch][first..first + got[ch].len()];
            let b = |v: &[f32]| v.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
            assert_eq!(b(&got[ch]), b(want), "channel {ch}");
        }
    }
}
