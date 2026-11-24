//! Writer thread tests with a real engine tap (offline engine, scripted input; no devices).

use super::*;
use crate::audio_record::{PlannedTrack, TrackInput, plan_tap};
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
            stream: threshold.into(),
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

#[test]
fn a_take_streams_when_keeping_it_would_pass_the_session_budget() {
    for (budget, resident, streams) in [(1 << 20, 0u64, false), (1 << 20, 1 << 20, true)] {
        let mut rig = Rig::new("stream-budget");
        let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
        let mut cfgw = rig.config(&[("a", TrackInput::Mono(0))], 9, 1 << 30);
        // A floor of nothing: the 8 KiB take is the one the budget decides about.
        cfgw.stream = StreamRule {
            floor: 0,
            budget: Some(budget),
            resident,
            ..cfgw.stream
        };
        let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
        render(&mut rig.engine, 0, 2_048, 64);
        ctl.send(finish(2_048, 0, linear(0))).unwrap();
        let result = rig.result().unwrap();
        assert_eq!(result.laps[0].asset.is_stream(), streams, "{resident} held");
    }
}

#[allow(clippy::disallowed_methods)] // Justification: test-only timing of blocking behaviour.
fn test_now() -> std::time::Instant {
    std::time::Instant::now()
}

/// The files of the take under the name the orphan scan reads (`rec-<pid>-<take>-<n>.wav`).
fn orphan_named(cfg: &mut WriterConfig, rig: &Rig) {
    for (n, t) in cfg.tracks.iter_mut().enumerate() {
        t.path = rig
            .bundle
            .audio_dir()
            .join(format!("rec-4242-{}-{n}.wav", cfg.take));
    }
}

#[test]
fn a_lap_that_cannot_be_allocated_is_refused_without_allocating() {
    assert!(lap_buffers(2, 1_000, None).is_some_and(|b| b.len() == 2 && b[1].len() == 1_000));
    assert!(lap_buffers(2, 1_000, Some(2_000)).is_some(), "at the limit");
    assert!(
        lap_buffers(2, 1_000, Some(1_999)).is_none(),
        "over the limit"
    );
    // A request no machine can satisfy fails instead of aborting the process.
    assert!(lap_buffers(2, usize::MAX / 8, None).is_none());
    assert!(lap_buffers(usize::MAX / 8, 1, None).is_none());
}

#[test]
fn a_take_too_long_to_finish_in_memory_ends_with_a_notice_and_keeps_its_files() {
    let mut rig = Rig::new("toolong");
    let tap = rig.handle.record_tap(&[0, 1], 16_384).unwrap();
    let mut cfgw = rig.config(&[("a", TrackInput::Stereo(0, 1))], 21, u64::MAX);
    orphan_named(&mut cfgw, &rig);
    let partial = cfgw.tracks[0].path.clone();
    let faults = Faults {
        lap_limit_samples: Some(1_000),
        fail_after_frames: None,
    };
    let (ctl, _) = spawn_with(tap, cfgw, rig.tx.clone(), faults).unwrap();
    render(&mut rig.engine, 0, 4_096, 64);
    ctl.send(finish(4_096, 0, linear(0))).unwrap();
    let message = match rig.result() {
        Ok(_) => panic!("a take over the limit must not be stored"),
        Err(m) => m,
    };
    assert!(
        message.contains("too long to finish in memory"),
        "{message}"
    );
    assert!(message.contains("rec-4242-21-0.wav"), "{message}");
    // Nothing on disk was lost: the in-progress file holds the whole take and the orphan scan
    // finds it where it looks.
    let reader = WavStreamReader::open(&partial).expect("a valid WAV");
    assert_eq!((reader.channel_count(), reader.frames()), (2, 4_096));
    let found = crate::recovery::scan_orphans(&[rig.bundle.root().to_path_buf()], None, &|_| false);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, partial);
    // No asset was stored for it.
    let stored: Vec<_> = std::fs::read_dir(rig.bundle.audio_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file() && !e.file_name().to_string_lossy().starts_with("rec-"))
        .collect();
    assert!(stored.is_empty(), "no half-stored asset: {stored:?}");
}

#[test]
fn a_take_under_the_limit_is_stored_as_usual() {
    let mut rig = Rig::new("underlimit");
    let tap = rig.handle.record_tap(&[0, 1], 16_384).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Stereo(0, 1))], 22, u64::MAX);
    let partial = cfgw.tracks[0].path.clone();
    let faults = Faults {
        lap_limit_samples: Some(1_000_000),
        fail_after_frames: None,
    };
    let (ctl, _) = spawn_with(tap, cfgw, rig.tx.clone(), faults).unwrap();
    render(&mut rig.engine, 0, 4_096, 64);
    ctl.send(finish(4_096, 0, linear(0))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.laps.len(), 1);
    assert_eq!(bits(&memory(&result.laps[0].asset)[0]), expect(0, 0, 4_096));
    assert!(!partial.exists(), "stored takes still delete their files");
}

#[test]
fn a_write_error_sets_the_flag_names_the_reason_and_keeps_the_files() {
    let mut rig = Rig::new("diskfull");
    let tap = rig.handle.record_tap(&[0, 1], 16_384).unwrap();
    let mut cfgw = rig.config(&[("a", TrackInput::Stereo(0, 1))], 23, u64::MAX);
    orphan_named(&mut cfgw, &rig);
    let partial = cfgw.tracks[0].path.clone();
    let faults = Faults {
        lap_limit_samples: None,
        fail_after_frames: Some(2_000),
    };
    let (_ctl, _, failed) = spawn_watched(tap, cfgw, rig.tx.clone(), faults).unwrap();
    assert!(
        !failed.load(Ordering::Acquire),
        "clear until something fails"
    );
    render(&mut rig.engine, 0, 8_192, 64);
    // No finish is sent: the writer ends by itself, as soon as it cannot write.
    let message = match rig.result() {
        Ok(_) => panic!("a take that cannot be written has no laps"),
        Err(m) => m,
    };
    assert!(
        failed.load(Ordering::Acquire),
        "set before the result is sent"
    );
    assert!(message.contains("the disk is full"), "{message}");
    assert!(message.contains("rec-4242-23-0.wav"), "{message}");
    let reader = WavStreamReader::open(&partial).expect("a valid WAV");
    assert!(
        reader.frames() >= 2_000,
        "what was written stays: {}",
        reader.frames()
    );
    assert!(rig.rx.try_recv().is_err(), "one result, one notice");
    // The next take has a flag of its own, clear.
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let next = rig.config(&[("b", TrackInput::Mono(0))], 24, u64::MAX);
    let (ctl, _, failed_next) =
        spawn_watched(tap, next, rig.tx.clone(), Faults::default()).unwrap();
    assert!(!failed_next.load(Ordering::Acquire));
    ctl.send(Ctl::Cancel).unwrap();
}

#[test]
fn another_write_error_gives_its_own_text() {
    let e = hound::Error::IoError(std::io::Error::other("the device went away"));
    assert!(write_reason(&e).contains("the device went away"));
    let full = hound::Error::IoError(std::io::ErrorKind::StorageFull.into());
    assert_eq!(write_reason(&full), "the disk is full");
}

/// The signal of a long in-progress file: finite, with a negative zero in it.
fn long_sample(c: usize, f: u64) -> f32 {
    match f % 997 {
        0 => -0.0,
        n => (n as f32 + c as f32 * 3.0) / 500.0 - 1.0,
    }
}

/// Writes an in-progress file of `channels` x `frames` of [`long_sample`].
fn write_partial(path: &std::path::Path, channels: usize, frames: u64) {
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for f in 0..frames {
        for c in 0..channels {
            w.write_sample(long_sample(c, f)).unwrap();
        }
    }
    w.finalize().unwrap();
}

/// A config of two tracks, one stereo and one of six channels, with the in-progress files
/// written into the bundle.
fn long_config(bundle: &Bundle, frames: u64, threshold: u64) -> WriterConfig {
    let shapes = [("stereo", 2usize), ("six", 6)];
    let tracks = shapes
        .iter()
        .enumerate()
        .map(|(n, (name, channels))| {
            let path = bundle.audio_dir().join(format!("rec-4242-30-{n}.wav"));
            write_partial(&path, *channels, frames);
            WriterTrack {
                planned: PlannedTrack {
                    device: key(name),
                    input: TrackInput::Mono(0),
                    tap_index: (0..*channels).collect(),
                },
                name: (*name).to_owned(),
                path,
            }
        })
        .collect();
    WriterConfig {
        tracks,
        bundle: bundle.clone(),
        sample_rate: 48_000,
        stream: threshold.into(),
        generation: 7,
        take: 30,
    }
}

fn long_finish(position: Box<dyn Fn(u64) -> SongPosition + Send>) -> FinishInfo {
    FinishInfo {
        end_engine_frame: u64::MAX,
        compensation: 0,
        valid_from: 0,
        position,
    }
}

fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_streamed_long_take_stores_the_same_assets_as_the_in_memory_path() {
    const FRAMES: u64 = 1_000_000;
    const LAP: u64 = 300_000;
    // Both takes are the same files; one is kept in memory (the reference: every lap is read back
    // whole and stored from memory), the other streams (block by block).
    let mut results = Vec::new();
    for (name, threshold) in [("long-memory", u64::MAX), ("long-stream", 0)] {
        let bundle = temp_bundle(name);
        let config = long_config(&bundle, FRAMES, threshold);
        let finish = long_finish(looping(0, LAP));
        let laps = store_laps(&config, 0, FRAMES, &finish, Faults::default())
            .ok()
            .unwrap();
        results.push((bundle, laps));
    }
    let (memory_bundle, memory_laps) = &results[0];
    let (stream_bundle, stream_laps) = &results[1];
    // 2 tracks x 4 laps (three of 300 000 frames, one of 100 000).
    assert_eq!(memory_laps.len(), 8);
    assert_eq!(stream_laps.len(), 8);
    for (m, s) in memory_laps.iter().zip(stream_laps) {
        assert_eq!(
            (&m.device, m.lap, m.song_frame),
            (&s.device, s.lap, s.song_frame)
        );
        assert_eq!(m.info, s.info, "same hash, frames and name");
        assert_eq!(*m.peaks, *s.peaks, "same peaks");
        assert_eq!(
            std::fs::read(memory_bundle.asset_path(&m.info.hash)).unwrap(),
            std::fs::read(stream_bundle.asset_path(&s.info.hash)).unwrap(),
            "same file bytes"
        );
        assert_eq!(
            std::fs::read(memory_bundle.peaks_path(&m.info.hash)).unwrap(),
            std::fs::read(stream_bundle.peaks_path(&s.info.hash)).unwrap(),
            "same peak file"
        );
        assert!(!m.asset.is_stream() && s.asset.is_stream());
    }
    // And the lap holds what the file held: the second lap of the stereo track starts at
    // frame 300 000 of the file.
    let second = &stream_laps[1];
    assert_eq!((second.info.channels, second.lap), (2, 1));
    let stored = stream_bundle.load(&second.info.hash).unwrap();
    assert_eq!(stored.frames() as u64, LAP);
    assert_eq!(
        stored.channels[1][5].to_bits(),
        long_sample(1, LAP + 5).to_bits()
    );
    // No temporary file anywhere in the project's audio folder; the in-progress files stay until
    // the caller deletes them after every lap is stored.
    for bundle in [memory_bundle, stream_bundle] {
        let names = names_in(&bundle.audio_dir());
        assert!(names.iter().all(|n| !n.ends_with("tmp~")), "{names:?}");
        assert!(names.contains(&"rec-4242-30-0.wav".to_owned()), "{names:?}");
    }
    for (bundle, ..) in results {
        let _ = std::fs::remove_dir_all(bundle.root());
    }
}

#[test]
fn a_streamed_take_ends_with_its_assets_and_no_in_progress_or_temporary_file() {
    let mut rig = Rig::new("stream-end");
    let tap = rig.handle.record_tap(&[0, 1, 1], 32_768).unwrap();
    let cfgw = rig.config(
        &[("a", TrackInput::Stereo(0, 1)), ("b", TrackInput::Mono(1))],
        31,
        0,
    );
    let partials: Vec<_> = cfgw.tracks.iter().map(|t| t.path.clone()).collect();
    let (ctl, _) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 20_000, 333);
    ctl.send(finish(19_968, 0, looping(0, 8_000))).unwrap();
    let result = rig.result().unwrap();
    assert_eq!(result.laps.len(), 6, "three laps of two tracks");
    assert!(result.laps.iter().all(|l| l.asset.is_stream()));
    let first = &result.laps[0];
    let stored = rig.bundle.load(&first.info.hash).unwrap();
    assert_eq!(bits(&stored.channels[1]), expect(1, 0, 8_000));
    for p in partials {
        assert!(!p.exists(), "{} was removed", p.display());
    }
    let names = names_in(&rig.bundle.audio_dir());
    assert!(
        names
            .iter()
            .all(|n| n.ends_with(".wav") && !n.starts_with("rec-")),
        "{names:?}"
    );
}

#[test]
fn a_cancel_of_a_streaming_take_leaves_nothing_in_the_audio_folder() {
    let mut rig = Rig::new("stream-cancel");
    let tap = rig.handle.record_tap(&[0], 8_192).unwrap();
    let cfgw = rig.config(&[("a", TrackInput::Mono(0))], 32, 0);
    let (ctl, handle) = spawn(tap, cfgw, rig.tx.clone()).unwrap();
    render(&mut rig.engine, 0, 2_048, 64);
    ctl.send(Ctl::Cancel).unwrap();
    handle.join().unwrap();
    assert_eq!(names_in(&rig.bundle.audio_dir()), Vec::<String>::new());
}

#[test]
fn an_in_progress_file_that_ends_early_fails_the_store_and_leaves_no_temporary_file() {
    let bundle = temp_bundle("stream-short");
    let mut config = long_config(&bundle, 100_000, 0);
    config.tracks.truncate(1);
    let finish = long_finish(looping(0, 60_000));
    // The take claims 130 000 frames; the file holds 100 000: the first lap is stored, the
    // second cannot be read.
    let Err(e) = store_laps(&config, 0, 130_000, &finish, Faults::default()) else {
        panic!("a short file must fail the store");
    };
    let message = store_failure(&config, e);
    assert!(message.contains("shorter than expected"), "{message}");
    assert!(message.contains("rec-4242-30-0.wav"), "{message}");
    assert!(config.tracks[0].path.exists(), "the in-progress file stays");
    let names = names_in(&bundle.audio_dir());
    assert!(names.iter().all(|n| !n.ends_with("tmp~")), "{names:?}");
    let _ = std::fs::remove_dir_all(bundle.root());
}
