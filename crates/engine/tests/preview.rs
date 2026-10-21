//! The preview voice (engine side): feed API, rendering through the public engine API, fades,
//! resampling, switching, underruns, retirement.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{SampleRate, Tempo};
use atomik_engine::{
    Engine, EngineConfig, EngineError, EngineHandle, PREVIEW_RING_FRAMES, PreviewFeed, new_engine,
};
use common::*;

/// Latency of the engine FIFO: the first batch of preview output appears this many host frames in.
const LAT: usize = 64;

fn cfg_sr(sr: f64, outputs: u16) -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(sr).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: outputs,
        seed: 1,
    }
}

fn fade_frames(sr: f64) -> usize {
    (0.005 * sr).round() as usize
}

fn sine(freq: f64, rate: f64, amp: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (std::f64::consts::TAU * freq * i as f64 / rate).sin() as f32)
        .collect()
}

/// An engine plus everything rendered so far.
struct Played {
    engine: Engine,
    handle: EngineHandle,
    outs: usize,
    out: Vec<Vec<f32>>,
}

impl Played {
    fn new(sr: f64, outs: u16) -> Self {
        let (engine, handle) = new_engine(cfg_sr(sr, outs));
        Self {
            engine,
            handle,
            outs: usize::from(outs),
            out: vec![Vec::new(); usize::from(outs)],
        }
    }

    /// Renders `n` more frames (in host blocks of 100) and appends them to `out`.
    fn render(&mut self, n: usize) {
        let blocks = render_blocks(&mut self.engine, &[], self.outs, n, 100);
        for (o, b) in self.out.iter_mut().zip(blocks) {
            o.extend(b);
        }
        self.handle.collect_garbage();
    }

    /// Channel `ch` with the FIFO latency removed (index 0 = first frame the voice produced).
    fn ch(&self, ch: usize) -> &[f32] {
        &self.out[ch][LAT..]
    }
}

fn feed_all(feed: &mut PreviewFeed, planar: &[Vec<f32>]) {
    let mut from = 0;
    while from < planar[0].len() {
        let n = feed.push(planar, from);
        assert!(n > 0, "ring full while pre-filling");
        from += n;
    }
}

fn start(p: &mut Played, planar: &[Vec<f32>], rate: u32, gain: f32, finish: bool) -> PreviewFeed {
    let (mut feed, stream) = atomik_engine::preview_channel();
    feed.set_source_rate(rate);
    feed_all(&mut feed, planar);
    if finish {
        feed.finish();
    }
    p.handle.preview_start(stream, gain).unwrap();
    feed
}

fn stereo_ramp(n: usize) -> Vec<Vec<f32>> {
    vec![
        (0..n).map(|i| 0.1 + i as f32 * 1.0e-5).collect(),
        (0..n).map(|i| -0.2 - i as f32 * 2.0e-5).collect(),
    ]
}

// ---- feed API ----

#[test]
fn feed_push_mono_stereo_and_extra_channels() {
    let (mut feed, _stream) = atomik_engine::preview_channel();
    assert_eq!(feed.free_frames(), PREVIEW_RING_FRAMES);
    assert_eq!(feed.push(&[vec![1.0, 2.0, 3.0]], 1), 2);
    assert_eq!(feed.free_frames(), PREVIEW_RING_FRAMES - 2);
    let three = vec![vec![1.0; 4], vec![2.0; 4], vec![3.0; 4]];
    assert_eq!(feed.push(&three, 0), 4);
    assert_eq!(feed.free_frames(), PREVIEW_RING_FRAMES - 6);
    assert_eq!(feed.push(&three, 4), 0);
    assert_eq!(feed.push(&[], 0), 0);
    assert_eq!(feed.frames_played(), 0);
    assert!(!feed.is_done());
    assert!(!feed.is_abandoned());
}

#[test]
fn feed_push_is_partial_when_full() {
    let (mut feed, _stream) = atomik_engine::preview_channel();
    let big = vec![vec![0.5; PREVIEW_RING_FRAMES + 10]];
    assert_eq!(feed.push(&big, 0), PREVIEW_RING_FRAMES);
    assert_eq!(feed.free_frames(), 0);
    assert_eq!(feed.push(&big, PREVIEW_RING_FRAMES), 0);
}

#[test]
fn feed_sees_a_dropped_stream_as_abandoned() {
    let (feed, stream) = atomik_engine::preview_channel();
    assert!(!feed.is_abandoned());
    drop(stream);
    assert!(feed.is_abandoned());
}

// ---- rendering ----

#[test]
fn same_rate_is_a_bit_exact_passthrough_after_the_fade_in() {
    let mut p = Played::new(48_000.0, 3);
    let n = 5_000;
    let src = stereo_ramp(n);
    let feed = start(&mut p, &src, 48_000, 1.0, true);
    p.render(LAT + n + 500);
    let f = fade_frames(48_000.0);
    for (i, (l, r)) in src[0].iter().zip(&src[1]).enumerate().skip(f) {
        assert_eq!(p.ch(0)[i], *l, "left {i}");
        assert_eq!(p.ch(1)[i], *r, "right {i}");
    }
    for (i, l) in src[0].iter().enumerate().take(f) {
        let env = i as f32 / f as f32;
        assert!((p.ch(0)[i] - l * env).abs() < 1e-6, "fade {i}");
    }
    // Only outputs 1 and 2 are touched, and silence follows the end.
    assert!(p.ch(2).iter().all(|&s| s == 0.0));
    assert!(p.ch(0)[n..].iter().all(|&s| s == 0.0));
    assert_eq!(feed.frames_played(), n as u64);
    assert!(feed.is_done());
    assert!(feed.is_abandoned());
    assert_eq!(feed.underruns(), 0);
}

#[test]
fn gain_is_linear() {
    let mut p = Played::new(48_000.0, 2);
    let src = vec![vec![0.5; 2_000]];
    let _feed = start(&mut p, &src, 48_000, 0.25, true);
    p.render(LAT + 1_000);
    assert_eq!(p.ch(0)[600], 0.125);
    assert_eq!(p.ch(1)[600], 0.125);
}

#[test]
fn fade_in_starts_at_zero_and_ramps_to_one() {
    for sr in [48_000.0, 44_100.0] {
        let mut p = Played::new(sr, 2);
        let src = vec![vec![1.0; 3_000]];
        let _feed = start(&mut p, &src, sr as u32, 1.0, true);
        p.render(LAT + 1_000);
        let f = fade_frames(sr);
        assert_eq!(p.ch(0)[0], 0.0);
        for i in 1..f {
            assert!(p.ch(0)[i] > p.ch(0)[i - 1], "monotone at {i}");
        }
        assert_eq!(p.ch(0)[f], 1.0);
        assert_eq!(p.ch(0)[f + 10], 1.0);
    }
}

/// Rising zero crossings (linearly interpolated positions).
fn rising_crossings(x: &[f32]) -> Vec<f64> {
    let mut v = Vec::new();
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            v.push((i - 1) as f64 + f64::from(-x[i - 1] / (x[i] - x[i - 1])));
        }
    }
    v
}

fn check_resampled(src_rate: f64, engine_rate: f64) {
    let freq = 1_000.0;
    let n = 20_000;
    let src = vec![sine(freq, src_rate, 0.5, n)];
    let mut p = Played::new(engine_rate, 2);
    let _feed = start(&mut p, &src, src_rate as u32, 1.0, true);
    let out_n = (n as f64 * engine_rate / src_rate) as usize;
    p.render(LAT + out_n + 200);
    let win = &p.ch(0)[1_000..out_n - 50];
    let cross = rising_crossings(win);
    assert!(cross.len() > 50);
    let period = (cross[cross.len() - 1] - cross[0]) / (cross.len() - 1) as f64;
    let measured = engine_rate / period;
    assert!(
        (measured - freq).abs() < freq * 0.002,
        "{src_rate}->{engine_rate}: {measured} Hz"
    );
    let rms = (win.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / win.len() as f64).sqrt();
    assert!(
        (rms * 2f64.sqrt() - 0.5).abs() < 0.01,
        "{src_rate}->{engine_rate}: amplitude {}",
        rms * 2f64.sqrt()
    );
    // No discontinuity: the largest step of a sine is 2*pi*f/sr*A.
    let max_step = std::f64::consts::TAU * freq / engine_rate * 0.5;
    for w in p.ch(0)[..out_n - 50].windows(2) {
        assert!(f64::from((w[1] - w[0]).abs()) <= max_step * 1.1);
    }
}

#[test]
fn resamples_48k_to_44k1() {
    check_resampled(48_000.0, 44_100.0);
}

#[test]
fn resamples_22k05_to_48k() {
    check_resampled(22_050.0, 48_000.0);
}

#[test]
fn frames_played_counts_source_frames_when_resampling() {
    let mut p = Played::new(48_000.0, 2);
    let src = vec![vec![0.3; 22_050]];
    let feed = start(&mut p, &src, 22_050, 1.0, true);
    p.render(LAT + 48_000 + 500);
    assert!(feed.is_done());
    assert_eq!(feed.frames_played(), 22_050);
}

#[test]
fn switching_crossfades_without_a_jump_and_retires_the_old_stream() {
    let mut p = Played::new(48_000.0, 2);
    let a = vec![sine(440.0, 48_000.0, 0.5, 30_000)];
    let b = vec![sine(1_000.0, 48_000.0, 0.5, 30_000)];
    let fa = start(&mut p, &a, 48_000, 1.0, false);
    p.render(LAT + 2_000);
    assert!(!fa.is_abandoned());
    let fb = start(&mut p, &b, 48_000, 1.0, false);
    p.render(2_000);
    let max = p
        .ch(0)
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(max < 0.1, "jump {max}");
    assert!(fa.is_abandoned());
    assert!(!fb.is_abandoned());
    assert!(p.handle.preview_active());
}

#[test]
fn a_third_start_retires_the_fading_stream_immediately() {
    let mut p = Played::new(48_000.0, 2);
    let c = vec![vec![0.2; 5_000]];
    let fa = start(&mut p, &c, 48_000, 1.0, false);
    p.render(LAT + 1_000);
    let fb = start(&mut p, &c, 48_000, 1.0, false);
    p.render(100);
    assert!(!fa.is_abandoned(), "A is fading out");
    let fc = start(&mut p, &c, 48_000, 1.0, false);
    p.render(100);
    assert!(fa.is_abandoned(), "A is cut: both slots were busy");
    assert!(!fb.is_abandoned());
    assert!(!fc.is_abandoned());
}

#[test]
fn stop_fades_to_exact_zero_then_retires() {
    let mut p = Played::new(48_000.0, 2);
    let src = vec![vec![1.0; 20_000]];
    let feed = start(&mut p, &src, 48_000, 1.0, false);
    p.render(LAT + 2_000);
    p.handle.preview_stop().unwrap();
    p.render(1_000);
    let tail = &p.ch(0)[2_000..];
    let first = tail.iter().position(|&s| s < 1.0).unwrap();
    let f = fade_frames(48_000.0);
    assert!(tail[first..first + f - 1].windows(2).all(|w| w[1] < w[0]));
    assert!(tail[first + f..].iter().all(|&s| s == 0.0));
    assert!(feed.is_abandoned());
    assert!(!p.handle.preview_active());
    // Stopping again is a no-op.
    p.handle.preview_stop().unwrap();
    p.render(200);
    assert!(p.ch(0)[3_000..].iter().all(|&s| s == 0.0));
}

#[test]
fn finish_plays_out_then_is_done() {
    let mut p = Played::new(48_000.0, 2);
    let src = stereo_ramp(1_000);
    let mut feed = start(&mut p, &src, 48_000, 1.0, false);
    p.render(LAT + 2_000);
    assert!(!feed.is_done());
    assert!(p.handle.preview_active());
    feed.finish();
    p.render(200);
    assert!(feed.is_done());
    assert!(feed.is_abandoned());
    assert_eq!(feed.frames_played(), 1_000);
    assert!(!p.handle.preview_active());
}

#[test]
fn an_empty_finished_stream_ends_at_once() {
    let mut p = Played::new(48_000.0, 2);
    let (mut feed, stream) = atomik_engine::preview_channel();
    feed.finish();
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(LAT + 200);
    assert!(feed.is_done());
    assert!(feed.is_abandoned());
    assert!(!p.handle.preview_active());
}

#[test]
fn underrun_is_silent_counted_and_resumes_in_order() {
    let mut p = Played::new(48_000.0, 2);
    let n = 3_000;
    let src: Vec<f32> = (0..n).map(|i| 0.1 + i as f32 * 1.0e-4).collect();
    let (mut feed, stream) = atomik_engine::preview_channel();
    assert_eq!(feed.push(&[src[..1_000].to_vec()], 0), 1_000);
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(LAT + 3_000);
    assert!(feed.underruns() > 0);
    assert!(!feed.is_done());
    let played = feed.frames_played();
    assert!((900..=1_000).contains(&played), "played {played}");
    assert!(p.ch(0)[1_500..].iter().all(|&s| s == 0.0));
    // Resume with the rest.
    assert_eq!(feed.push(&[src[1_000..].to_vec()], 0), 2_000);
    feed.finish();
    p.render(3_000);
    let nonzero: Vec<f32> = p.ch(0).iter().copied().filter(|&s| s != 0.0).collect();
    // Everything after the fade-in arrives in order, without a gap or repeat.
    let f = fade_frames(48_000.0);
    assert_eq!(&nonzero[f - 1..], &src[f..]);
    assert!(feed.is_done());
}

#[test]
fn queue_full_drops_the_stream_on_the_handle_side() {
    let mut p = Played::new(48_000.0, 2);
    let mut last = None;
    for _ in 0..100_000 {
        let (feed, stream) = atomik_engine::preview_channel();
        match p.handle.preview_start(stream, 1.0) {
            Ok(()) => {}
            Err(e) => {
                last = Some((e, feed));
                break;
            }
        }
    }
    let (err, feed) = last.expect("the control queue never filled");
    assert!(matches!(err, EngineError::QueueFull));
    assert!(feed.is_abandoned());
}

#[test]
fn preview_active_follows_the_voice() {
    let mut p = Played::new(48_000.0, 2);
    assert!(!p.handle.preview_active());
    let src = vec![vec![0.3; 5_000]];
    let _feed = start(&mut p, &src, 48_000, 1.0, false);
    p.render(200);
    assert!(p.handle.preview_active());
    p.handle.preview_stop().unwrap();
    p.render(400);
    assert!(!p.handle.preview_active());
}

#[test]
fn idle_stop_leaves_the_output_untouched() {
    let mut a = Played::new(48_000.0, 2);
    let mut b = Played::new(48_000.0, 2);
    b.handle.preview_stop().unwrap();
    a.render(1_000);
    b.render(1_000);
    assert_eq!(a.out, b.out);
}

#[test]
fn a_pending_stream_is_silent_uncounted_and_takes_its_rate_from_the_feed() {
    let engine_rate = 44_100.0;
    let mut p = Played::new(engine_rate, 2);
    let (mut feed, stream) = atomik_engine::preview_channel();
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(LAT + 2_000);
    assert!(p.ch(0).iter().all(|&s| s == 0.0));
    assert_eq!(feed.underruns(), 0);
    assert!(p.handle.preview_active());
    // The data arrives later, at 48 kHz on a 44.1 kHz engine.
    let n = 20_000;
    feed.set_source_rate(48_000);
    feed_all(&mut feed, &[sine(1_000.0, 48_000.0, 0.5, n)]);
    feed.finish();
    let out_n = (n as f64 * engine_rate / 48_000.0) as usize;
    let before = p.out[0].len();
    p.render(out_n + 200);
    let tail = &p.out[0][before..];
    // Starts at zero and ramps (the fade did not run while pending).
    let first = tail.iter().position(|&s| s != 0.0).unwrap();
    assert!(first <= 64 + 64);
    assert!(tail[first].abs() < 0.01);
    let win = &tail[first + 1_000..first + out_n - 100];
    let cross = rising_crossings(win);
    let period = (cross[cross.len() - 1] - cross[0]) / (cross.len() - 1) as f64;
    let measured = engine_rate / period;
    assert!((measured - 1_000.0).abs() < 2.0, "{measured} Hz");
    assert!(feed.is_done());
}

#[test]
fn the_old_stream_fades_out_while_the_new_one_is_pending() {
    let mut p = Played::new(48_000.0, 2);
    let src = vec![vec![1.0; 20_000]];
    let old = start(&mut p, &src, 48_000, 1.0, false);
    p.render(LAT + 2_000);
    let (_feed, stream) = atomik_engine::preview_channel();
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(1_000);
    let tail = &p.ch(0)[2_000..];
    let first = tail.iter().position(|&s| s < 1.0).unwrap();
    assert!(
        tail[first + fade_frames(48_000.0)..]
            .iter()
            .all(|&s| s == 0.0)
    );
    assert!(old.is_abandoned());
    assert!(p.handle.preview_active(), "the new stream is still pending");
}

#[test]
fn a_pending_stream_whose_feed_is_dropped_is_retired() {
    let mut p = Played::new(48_000.0, 2);
    let (feed, stream) = atomik_engine::preview_channel();
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(300);
    assert!(p.handle.preview_active());
    drop(feed);
    p.render(300);
    assert!(!p.handle.preview_active());
    // A pending stream that finishes without data ends too.
    let (mut feed, stream) = atomik_engine::preview_channel();
    p.handle.preview_start(stream, 1.0).unwrap();
    feed.finish();
    p.render(300);
    assert!(feed.is_done());
    assert!(!p.handle.preview_active());
}

#[test]
fn the_monitor_mirrors_the_feed_and_outlives_it() {
    let mut p = Played::new(48_000.0, 2);
    let (mut feed, stream) = atomik_engine::preview_channel();
    let from_stream = stream.monitor();
    let m = feed.monitor();
    assert_eq!(m.source_rate(), 0);
    feed.set_source_rate(44_100);
    assert_eq!(m.source_rate(), 44_100);
    assert_eq!(from_stream.source_rate(), 44_100);
    feed_all(&mut feed, &[vec![0.4; 1_000]]);
    p.handle.preview_start(stream, 1.0).unwrap();
    p.render(LAT + 500);
    assert_eq!(m.frames_played(), feed.frames_played());
    assert!(m.frames_played() > 0);
    assert_eq!(m.underruns(), feed.underruns());
    assert!(!m.is_done() && !m.is_abandoned());
    feed.finish();
    p.render(2_000);
    drop(feed);
    let m2 = m.clone();
    assert!(m.is_done() && m.is_abandoned());
    assert!(m2.is_done() && from_stream.is_done());
    assert_eq!(m.frames_played(), 1_000);
}
