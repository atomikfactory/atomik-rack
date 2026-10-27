//! Writer thread tests with a real engine tap (offline engine, scripted input; no devices).

use super::*;
use crate::audio_record::{TrackInput, plan_tap};
use atomik_core::{SampleRate, Tempo, Ticks};
use atomik_engine::{EngineConfig, new_engine};
use std::sync::mpsc::Receiver;

fn cfg() -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 2,
        output_channels: 2,
        seed: 0,
    }
}

/// Input channel `c` at engine frame `f`: unique per frame, exactly representable.
fn sample(c: usize, f: u64) -> f32 {
    (f as f32) * 2.0 + c as f32
}

fn temp_bundle(name: &str) -> Bundle {
    let dir = std::env::temp_dir().join(format!("atomik-writer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Bundle::create(&dir).unwrap()
}

fn key(s: &str) -> DeviceKey {
    DeviceKey(s.to_owned())
}

/// Renders `frames` frames starting at engine frame `from`, in blocks of `block`.
fn render(engine: &mut atomik_engine::Engine, from: u64, frames: u64, block: usize) {
    let mut done = 0u64;
    while done < frames {
        let n = (block as u64).min(frames - done) as usize;
        let ins: Vec<Vec<f32>> = (0..2)
            .map(|c| (0..n).map(|i| sample(c, from + done + i as u64)).collect())
            .collect();
        let in_refs: Vec<&[f32]> = ins.iter().map(Vec::as_slice).collect();
        let mut outs = vec![vec![0.0f32; n]; 2];
        let mut out_refs: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
        engine.render(&in_refs, &mut out_refs);
        done += n as u64;
    }
}

struct Rig {
    engine: atomik_engine::Engine,
    handle: atomik_engine::EngineHandle,
    bundle: Bundle,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
}

impl Rig {
    fn new(name: &str) -> Self {
        let (engine, handle) = new_engine(cfg());
        let (tx, rx) = channel();
        Self {
            engine,
            handle,
            bundle: temp_bundle(name),
            rx,
            tx,
        }
    }

    fn config(&self, tracks: &[(&str, TrackInput)], take: u64, threshold: u64) -> WriterConfig {
        let planned = plan_tap(&tracks.iter().map(|(k, i)| (key(k), *i)).collect::<Vec<_>>());
        WriterConfig {
            tracks: planned
                .tracks
                .iter()
                .enumerate()
                .map(|(n, p)| WriterTrack {
                    planned: p.clone(),
                    name: p.device.0.clone(),
                    path: self
                        .bundle
                        .audio_dir()
                        .join(format!("rec-test-{take}-{n}.wav")),
                })
                .collect(),
            bundle: self.bundle.clone(),
            sample_rate: 48_000,
            stream_threshold: threshold,
            generation: 7,
            take,
        }
    }

    fn result(&self) -> Result<TakeResult, String> {
        match self
            .rx
            .recv_timeout(Duration::from_secs(20))
            .expect("a result")
        {
            Msg::Recorded { generation, result } => {
                assert_eq!(generation, 7);
                result
            }
            _ => panic!("another message"),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.bundle.root());
    }
}

fn linear(start: u64) -> Box<dyn Fn(u64) -> SongPosition + Send> {
    Box::new(move |e| SongPosition {
        song_frame: e.saturating_sub(start),
        tick: Ticks(0),
        loop_count: 0,
    })
}

fn looping(ls: u64, le: u64) -> Box<dyn Fn(u64) -> SongPosition + Send> {
    Box::new(move |e| SongPosition {
        song_frame: ls + e % (le - ls),
        tick: Ticks(0),
        loop_count: e / (le - ls),
    })
}

fn finish(end: u64, compensation: i64, position: Box<dyn Fn(u64) -> SongPosition + Send>) -> Ctl {
    Ctl::Finish(Box::new(FinishInfo {
        end_engine_frame: end,
        compensation,
        valid_from: 0,
        position,
    }))
}

fn memory(asset: &SongAsset) -> &[Vec<f32>] {
    match asset {
        SongAsset::Memory { data, .. } => data,
        SongAsset::Stream { .. } => panic!("expected a memory asset"),
    }
}

fn expect(channel: usize, from: u64, frames: u64) -> Vec<u32> {
    (from..from + frames)
        .map(|f| sample(channel, f).to_bits())
        .collect()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

#[test]
fn a_take_is_written_bit_exactly_one_asset_per_track() {
    let mut rig = Rig::new("exact");
    // The plan taps every track's channels separately (each has its own gain): a takes host
    // channels 0 and 1, b takes host channel 1 again.
    let tap = rig.handle.record_tap(&[0, 1, 1], 32_768).unwrap();
    let cfgw = rig.config(
        &[("a", TrackInput::Stereo(0, 1)), ("b", TrackInput::Mono(1))],
        1,
        u64::MAX,
    );
    let partials: Vec<_> = cfgw.tracks.iter().map(|t| t.path.clone()).collect();
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 20_000, 333);
    // 20 000 frames: 312 whole batches (19 968 frames) have been processed.
    ctl.send(finish(19_968, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.overflow_frames, 0);
    assert_eq!(result.gap_frames, 0);
    assert_eq!(result.frames, 19_968);
    assert_eq!(result.laps.len(), 2, "one lap per track");
    let a = &result.laps[0];
    assert_eq!((a.device.0.as_str(), a.lap, a.song_frame), ("a", 0, 0));
    assert_eq!(a.info.channels, 2);
    assert_eq!(a.info.frames, 19_968);
    assert_eq!(a.info.sample_rate, 48_000);
    assert_eq!(bits(&memory(&a.asset)[0]), expect(0, 0, 19_968));
    assert_eq!(bits(&memory(&a.asset)[1]), expect(1, 0, 19_968));
    let b = &result.laps[1];
    assert_eq!(b.info.channels, 1);
    assert_eq!(bits(&memory(&b.asset)[0]), expect(1, 0, 19_968));
    // The stored WAV equals the input bit for bit, and the in-progress files are gone.
    let stored = rig.bundle.load(&a.info.hash).unwrap();
    assert_eq!(bits(&stored.channels[0]), expect(0, 0, 19_968));
    for p in partials {
        assert!(!p.exists(), "{} was removed", p.display());
    }
    assert_eq!(a.peaks.frames(), 19_968);
}

#[test]
fn each_loop_pass_is_its_own_asset() {
    let mut rig = Rig::new("laps");
    let tap = rig.handle.record_tap(&[0], 32_768).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 2, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 20_000, 64);
    ctl.send(finish(19_968, 0, looping(100, 5_100))).unwrap();
    let result = rig.result().unwrap();
    // 5 000-frame passes: 3 whole ones and 4 968 frames of the fourth.
    let sizes: Vec<_> = result.laps.iter().map(|l| (l.lap, l.info.frames)).collect();
    assert_eq!(sizes, vec![(0, 5_000), (1, 5_000), (2, 5_000), (3, 4_968)]);
    for lap in &result.laps {
        assert_eq!(lap.song_frame, 100);
        let from = lap.lap * 5_000;
        assert_eq!(
            bits(&memory(&lap.asset)[0]),
            expect(0, from, lap.info.frames),
            "lap {}",
            lap.lap
        );
    }
}

#[test]
fn the_compensation_moves_the_audio_earlier_and_drops_what_precedes_the_start() {
    let mut rig = Rig::new("comp");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 3, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 5_000, 64);
    // Transport started at engine frame 0; 300 frames of compensation.
    ctl.send(finish(4_992, 300, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.laps.len(), 1);
    let lap = &result.laps[0];
    assert_eq!(lap.song_frame, 0, "the first kept frame is song frame 0");
    assert_eq!(lap.info.frames, 4_992 - 300);
    // File frame 300 (engine frame 300) is song frame 0.
    assert_eq!(bits(&memory(&lap.asset)[0]), expect(0, 300, 4_992 - 300));
}

#[test]
fn an_overflow_is_counted_and_the_gap_is_written_as_silence() {
    let mut rig = Rig::new("overflow");
    // Room for four batches; nobody drains while 10 batches are rendered.
    let tap = rig.handle.record_tap(&[0], 256).unwrap();
    render(&mut rig.engine, 0, 640, 64);
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 4, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(60)); // the writer drains the first four batches
    for b in 10..20u64 {
        render(&mut rig.engine, b * 64, 64, 64);
        std::thread::sleep(Duration::from_millis(8));
    }
    ctl.send(finish(1_280, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.overflow_frames, 6 * 64);
    assert_eq!(result.gap_frames, 6 * 64);
    assert_eq!(result.frames, 1_280);
    let data = &memory(&result.laps[0].asset)[0];
    assert_eq!(data.len(), 1_280);
    assert_eq!(bits(&data[..256]), expect(0, 0, 256));
    assert!(
        data[256..640].iter().all(|s| s.to_bits() == 0),
        "the gap is silent"
    );
    assert_eq!(
        bits(&data[640..]),
        expect(0, 640, 640),
        "later frames stay aligned"
    );
}

#[test]
fn a_finish_beyond_the_rendered_frames_ends_when_the_tap_goes_quiet() {
    let mut rig = Rig::new("quiet");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 5, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 1_024, 64);
    let t0 = test_now();
    ctl.send(finish(1_000_000, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert_eq!(result.frames, 1_024);
    assert_eq!(result.laps[0].info.frames, 1_024);
}

#[test]
fn frames_after_the_end_are_not_recorded() {
    let mut rig = Rig::new("end");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 6, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 4_096, 64);
    // End in the middle of a drained chunk.
    ctl.send(finish(1_000, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.frames, 1_000);
    assert_eq!(bits(&memory(&result.laps[0].asset)[0]), expect(0, 0, 1_000));
}

#[test]
fn an_end_before_frames_already_written_still_cuts_the_take() {
    // The writer drains faster than the UI sends its finish: frames past the end are already in
    // the file when the finish arrives, and must not reach the take.
    let mut rig = Rig::new("late-end");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 7, u64::MAX);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 4_096, 64);
    std::thread::sleep(Duration::from_millis(300)); // let the writer drain all 4096 frames
    ctl.send(finish(1_000, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.frames, 1_000);
    assert_eq!(bits(&memory(&result.laps[0].asset)[0]), expect(0, 0, 1_000));
}

#[test]
fn a_cancel_stores_nothing_and_removes_the_in_progress_files() {
    let mut rig = Rig::new("cancel");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 7, u64::MAX);
    let partial = cfgw.tracks[0].path.clone();
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 2_048, 64);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        partial.exists(),
        "the file is created as soon as the take starts"
    );
    ctl.send(Ctl::Cancel).unwrap();
    for _ in 0..200 {
        if !partial.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!partial.exists());
    assert!(rig.rx.try_recv().is_err(), "no result for a cancelled take");
    assert!(rig.bundle.orphans(&Default::default()).is_empty());
}

#[test]
fn the_file_is_readable_while_the_take_runs() {
    // the header is refreshed as the take goes, so a crash keeps almost everything.
    let mut rig = Rig::new("crash");
    let tap = rig.handle.record_tap(&[0, 1], 16_384).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Stereo(0, 1))], 8, u64::MAX);
    let partial = cfgw.tracks[0].path.clone();
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    for b in 0..12 {
        render(&mut rig.engine, b * 4_096, 4_096, 64);
        std::thread::sleep(Duration::from_millis(12));
    }
    // The writer thread flushes on its own schedule: on a loaded machine it can lag the renders,
    // so wait (bounded) for the header to show the first flush instead of a fixed pause.
    let mut frames = 0;
    for _ in 0..500 {
        let reader = WavStreamReader::open(&partial).expect("a valid WAV mid-take");
        assert_eq!(reader.channel_count(), 2);
        frames = reader.frames();
        if frames >= 24_000 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        frames >= 24_000,
        "at least the first flush is in the header: {frames}"
    );
    ctl.send(Ctl::Cancel).unwrap();
}

#[test]
fn big_laps_stream_from_the_bundle_instead_of_staying_in_ram() {
    let mut rig = Rig::new("stream");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    // Threshold 0: everything streams.
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 9, 0);
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 2_048, 64);
    ctl.send(finish(2_048, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    let lap = &result.laps[0];
    assert!(lap.asset.is_stream());
    assert_eq!((lap.asset.frames(), lap.asset.channels()), (2_048, 1));
}

#[allow(clippy::disallowed_methods)] // Justification: test-only timing of blocking behaviour.
fn test_now() -> std::time::Instant {
    std::time::Instant::now()
}
