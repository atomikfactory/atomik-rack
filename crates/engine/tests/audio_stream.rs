//! Streaming: a streamed asset plays exactly like the same
//! asset in memory when the streamer keeps up, locate and loop changes refill the ring, and a
//! slow disk costs underruns (counted, silent) but never blocks the audio thread.
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::Ticks;
use atomik_devices::audio_track;
use atomik_engine::{
    AssetId, Engine, EngineHandle, SongAsset, StreamSource, TransportCommand, compile_song,
};
use common::audio::*;
use common::hot::Rig;
use common::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A fake file: planar samples served after an optional delay (on the streamer thread only).
struct FakeFile {
    data: Arc<[Vec<f32>]>,
    delay: Duration,
    starts: Mutex<Vec<u64>>,
    reads: AtomicU64,
}

impl FakeFile {
    fn new(data: Arc<[Vec<f32>]>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            data,
            delay,
            starts: Mutex::new(Vec::new()),
            reads: AtomicU64::new(0),
        })
    }

    fn asset(self: &Arc<Self>) -> SongAsset {
        SongAsset::stream(
            self.data[0].len() as u64,
            self.data.len() as u16,
            Arc::clone(self) as Arc<dyn StreamSource>,
        )
    }
}

impl StreamSource for FakeFile {
    fn read(&self, start_frame: u64, out: &mut [Vec<f32>]) -> usize {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.starts.lock().unwrap().push(start_frame);
        let frames = self.data[0].len() as u64;
        let n = (out[0].len() as u64).min(frames.saturating_sub(start_frame)) as usize;
        for (o, d) in out.iter_mut().zip(self.data.iter()) {
            o[..n].copy_from_slice(&d[start_frame as usize..start_frame as usize + n]);
        }
        n
    }
}

fn setup(
    assets: Vec<SongAsset>,
    regions: Vec<atomik_engine::SongAudioRegion>,
    loop_frames: Option<(i64, i64)>,
) -> (Engine, EngineHandle) {
    let c = cfg(0, 2);
    let mut rig = Rig::new(&c);
    let id = rig.insert(0, 1, audio_track::factory(), &[]);
    rig.wire_outputs(id, "out_1", "out_2");
    rig.route(0);
    let (engine, mut handle) = engine_with(c, 0, None);
    for e in rig.edits {
        handle.schedule(0, e.edit).unwrap();
    }
    if let Some((a, b)) = loop_frames {
        handle
            .transport(TransportCommand::SetLoop {
                start: Ticks(tick_of(a)),
                end: Ticks(tick_of(b)),
                enabled: true,
            })
            .unwrap();
    }
    let input = audio_input(vec![audio_track_of(1, regions)], assets);
    handle.submit_song(compile_song(&input).unwrap()).unwrap();
    (engine, handle)
}

/// Renders stopped blocks until the streamer has the read-ahead for the current position.
fn wait_primed(engine: &mut Engine, handle: &mut EngineHandle) {
    for _ in 0..5_000 {
        render_block(engine, &[], 2, 64);
        if handle.stream_primed() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the streamer never primed");
}

fn regions() -> Vec<atomik_engine::SongAudioRegion> {
    let mut r0 = region(1_000, 300, 8_000);
    r0.gain = 0.6;
    r0.fade_in = 1_500;
    r0.fade_out = 2_500;
    r0.fade_curve = atomik_engine::SongFadeCurve::EqualPower;
    let mut r1 = region(12_000, 0, 8_000);
    r1.asset = AssetId(1);
    r1.gain = 0.9;
    vec![r0, r1]
}

/// The scripted session: loop play, stop, loop off, locate, play. Returns the output of both
/// play phases and the underruns seen.
fn session(assets: Vec<SongAsset>, block: usize) -> (Vec<Vec<f32>>, u64) {
    let (mut engine, mut handle) = setup(assets, regions(), Some((6_000, 9_500)));
    wait_primed(&mut engine, &mut handle);
    handle.transport(TransportCommand::Play).unwrap();
    let mut out = render_blocks(&mut engine, &[], 2, 24_000, block);
    handle.transport(TransportCommand::Stop).unwrap();
    handle
        .transport(TransportCommand::SetLoop {
            start: Ticks(tick_of(6_000)),
            end: Ticks(tick_of(9_500)),
            enabled: false,
        })
        .unwrap();
    handle
        .transport(TransportCommand::Locate(Ticks(tick_of(11_000))))
        .unwrap();
    render_block(&mut engine, &[], 2, 64);
    wait_primed(&mut engine, &mut handle);
    handle.transport(TransportCommand::Play).unwrap();
    let more = render_blocks(&mut engine, &[], 2, 19_200, block);
    for (o, m) in out.iter_mut().zip(more) {
        o.extend_from_slice(&m);
    }
    handle.collect_garbage();
    let underruns = handle.stream_underruns();
    (out, underruns)
}

fn mixed_assets(streamed: bool) -> Vec<SongAsset> {
    let stereo = planes(2, 20_000);
    let mono = planes(1, 30_000);
    if streamed {
        vec![
            FakeFile::new(stereo, Duration::ZERO).asset(),
            FakeFile::new(mono, Duration::ZERO).asset(),
        ]
    } else {
        vec![SongAsset::memory(stereo), SongAsset::memory(mono)]
    }
}

#[test]
fn a_fast_stream_equals_the_memory_asset_bit_for_bit() {
    for block in [64usize, 333] {
        let (reference, none) = session(mixed_assets(false), block);
        assert_eq!(none, 0);
        assert!(
            reference[0].iter().any(|s| s.abs() > 1e-3),
            "the session is audible"
        );
        let (streamed, underruns) = session(mixed_assets(true), block);
        assert_eq!(underruns, 0, "block {block}: the streamer kept up");
        assert_eq!(bits(&streamed), bits(&reference), "block {block}");
    }
}

#[test]
fn a_mixed_lane_plays_streamed_and_memory_regions() {
    // Only the second asset (mono) streams; the first stays in memory.
    let stereo = planes(2, 20_000);
    let mono = FakeFile::new(planes(1, 30_000), Duration::ZERO);
    let assets = vec![SongAsset::memory(stereo), mono.asset()];
    let (a, _) = session(assets, 64);
    let (b, _) = session(mixed_assets(false), 64);
    assert_eq!(bits(&a), bits(&b));
}

#[test]
fn locate_and_loop_changes_refill_from_the_new_position() {
    let file = FakeFile::new(planes(2, 20_000), Duration::ZERO);
    let (mut engine, mut handle) = setup(vec![file.asset(), asset(1, 30_000)], regions(), None);
    wait_primed(&mut engine, &mut handle);
    // The ring was filled from the start of the song: the first streamed read is the region's
    // source offset (the region starts at frame 1000, offset 300).
    assert_eq!(file.starts.lock().unwrap().first().copied(), Some(300));
    // Locate to song frame 3000 (2000 frames into the region): the next reads begin at
    // source frame 300 + 2000.
    handle
        .transport(TransportCommand::Locate(Ticks(tick_of(3_000))))
        .unwrap();
    render_block(&mut engine, &[], 2, 64);
    wait_primed(&mut engine, &mut handle);
    let starts = file.starts.lock().unwrap().clone();
    assert!(
        starts.contains(&2_300),
        "the ring refills from the locate position: {starts:?}"
    );
    // A loop set while stopped makes the streamer follow it: reads wrap back to the loop start.
    handle
        .transport(TransportCommand::SetLoop {
            start: Ticks(tick_of(2_000)),
            end: Ticks(tick_of(3_500)),
            enabled: true,
        })
        .unwrap();
    render_block(&mut engine, &[], 2, 64);
    wait_primed(&mut engine, &mut handle);
    let starts = file.starts.lock().unwrap().clone();
    // Loop start 2000 -> source frame 300 + 1000 = 1300 is read again after the loop change.
    assert!(
        starts.contains(&1_300),
        "the ring follows the loop: {starts:?}"
    );
    assert_eq!(handle.stream_underruns(), 0);
}

#[test]
#[allow(clippy::disallowed_methods)] // Justification: test-only wall-clock check that the audio thread never waits for a read.
fn a_slow_disk_gives_counted_silent_underruns_and_never_blocks_the_audio_thread() {
    let data = planes(2, 20_000);
    let slow = FakeFile::new(Arc::clone(&data), Duration::from_millis(300));
    let mut r = region(0, 0, 20_000);
    r.gain = 1.0;
    let (mut engine, mut handle) = setup(vec![slow.asset()], vec![r], None);
    // No priming: play at once, the disk cannot deliver.
    handle.transport(TransportCommand::Play).unwrap();
    let mut worst = Duration::ZERO;
    let mut out = vec![Vec::new(); 2];
    for _ in 0..(24_000 / 64) {
        let t = std::time::Instant::now();
        let block = render_block(&mut engine, &[], 2, 64); // under assert_no_alloc
        worst = worst.max(t.elapsed());
        for (o, b) in out.iter_mut().zip(block) {
            o.extend_from_slice(&b);
        }
    }
    assert!(
        worst < Duration::from_millis(100),
        "a render call took {worst:?}: the audio thread must never wait for the disk"
    );
    assert!(handle.stream_underruns() > 0, "underruns are counted");
    // Every sample is either the real audio or silence, never misaligned data.
    for c in 0..2 {
        for (n, s) in out[c].iter().enumerate() {
            let want = value(c, n);
            assert!(
                s.to_bits() == 0 || s.to_bits() == want.to_bits(),
                "channel {c} frame {n}: {s} vs {want}"
            );
        }
    }
    drop(engine);
    drop(handle); // joins the streamer
}

#[test]
fn dropping_the_handle_joins_the_streamer() {
    let file = FakeFile::new(planes(2, 5_000), Duration::ZERO);
    let (mut engine, mut handle) = setup(vec![file.asset()], vec![region(0, 0, 5_000)], None);
    wait_primed(&mut engine, &mut handle);
    drop(handle);
    render_block(&mut engine, &[], 2, 64); // the engine survives its handle
}
