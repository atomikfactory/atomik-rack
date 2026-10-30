//! The engine's MIDI clock out through the output scheduler into the clock follower, on a fake
//! port and a synthetic clock: what a cable from an output port to an input port would do, with
//! the time under the test's control.

use atomik_core::{MeterEvent, SampleRate, Tempo, Ticks};
use atomik_engine::{
    EngineConfig, ExternalClock, FrameClock, SongInput, SongTempo, SyncConfig, TransportCommand,
    compile_song, new_engine,
};
use atomik_midi_io::MidiError;
use atomik_midi_io::clock::ClockFollower;
use atomik_midi_io::out::{Core, MidiOutPort, Opener, SyncPort, Timeline};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The port that hands every message to a follower, at the time the test says it is.
struct Loop {
    follower: Arc<Mutex<ClockFollower>>,
    now: Arc<Mutex<Instant>>,
}

impl MidiOutPort for Loop {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        let at = *self.now.lock().unwrap();
        self.follower.lock().unwrap().on_message(bytes, at);
        Ok(())
    }
}

#[allow(clippy::disallowed_methods)] // Justification: a test anchor; nothing reads the wall clock.
fn anchor() -> Instant {
    Instant::now()
}

fn tempo_points(points: &[(i64, f64, bool)]) -> Vec<SongTempo> {
    points
        .iter()
        .map(|&(tick, bpm, ramp)| SongTempo {
            tick: Ticks(tick),
            tempo: Tempo::new(bpm).unwrap(),
            ramp,
        })
        .collect()
}

/// Plays a song with `tempo` for `seconds` through the loop and samples the follower's tempo once
/// a second of song time, with the running flag.
struct Run {
    cell: Arc<ExternalClock>,
    samples: Vec<(f64, Option<f64>, bool)>,
}

fn run(tempo: &[(i64, f64, bool)], seconds: f64, offset_ms: i32) -> Run {
    let cfg = EngineConfig {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let (mut engine, mut handle) = new_engine(cfg);
    let song = compile_song(&SongInput {
        tracks: Vec::new(),
        tempo: tempo_points(tempo),
        meter: vec![MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    })
    .unwrap();
    handle.submit_song(song).unwrap();
    handle.sync_out().configure(SyncConfig {
        enabled: true,
        ..SyncConfig::default()
    });
    handle.transport(TransportCommand::Play).unwrap();
    let frames = (seconds * 48_000.0) as usize;
    let mut out = [vec![0.0f32; 64], vec![0.0f32; 64]];
    for _ in 0..frames / 64 {
        let mut refs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
        engine.render(&[], &mut refs);
    }
    // The engine has produced every message; now the scheduler sends them as time passes.
    let cell = ExternalClock::new();
    let follower = Arc::new(Mutex::new(ClockFollower::new(Arc::clone(&cell))));
    let now = Arc::new(Mutex::new(anchor()));
    let (f2, n2) = (Arc::clone(&follower), Arc::clone(&now));
    let opener: Opener = Arc::new(move |_| {
        Ok(Box::new(Loop {
            follower: Arc::clone(&f2),
            now: Arc::clone(&n2),
        }) as Box<dyn MidiOutPort>)
    });
    let t0 = *now.lock().unwrap();
    let clock = Arc::new(FrameClock::new());
    clock.publish(0, t0);
    let timeline = Timeline::new(clock, 48_000.0, Arc::new(AtomicU64::new(0)));
    let mut core = Core::new(opener, timeline);
    core.set_sync(
        Some(handle.sync_out().tap()),
        vec![SyncPort {
            name: "Loop".into(),
            offset_ms,
        }],
        t0,
    );
    let mut samples = Vec::new();
    let step = Duration::from_micros(100);
    let mut t = t0;
    let end = t0 + Duration::from_secs_f64(seconds);
    let mut next_sample = 0.25;
    while t < end {
        *now.lock().unwrap() = t;
        core.step(t);
        let at = t.duration_since(t0).as_secs_f64();
        if at >= next_sample {
            let s = cell.read();
            samples.push((at, s.tempo, s.running));
            next_sample += 0.25;
        }
        t += step;
    }
    Run { cell, samples }
}

#[test]
fn a_100_bpm_clock_is_followed_within_a_beat_and_reads_100_after_two_seconds() {
    let r = run(&[(0, 100.0, false)], 2.5, 0);
    let state = r.cell.read();
    assert!(state.running, "Start arrived");
    assert_eq!(state.position, 0);
    // A beat at 100 BPM is 0.6 s.
    let (_, tempo, _) = r.samples.iter().find(|(t, _, _)| *t >= 0.6).unwrap();
    assert!((tempo.unwrap() - 100.0).abs() < 0.5, "{tempo:?}");
    let (_, tempo, _) = r.samples.iter().find(|(t, _, _)| *t >= 2.0).unwrap();
    assert!((tempo.unwrap() - 100.0).abs() < 0.2, "{tempo:?}");
    // 24 clocks a beat: 100 BPM is 40 a second.
    assert!((95..=105).contains(&state.clocks), "{}", state.clocks);
}

#[test]
fn a_ramp_from_100_to_140_is_followed() {
    // The ramp runs over eight beats (about 5.8 s); then 140 BPM.
    let r = run(&[(0, 100.0, true), (8 * 15_360, 140.0, false)], 9.0, 0);
    let tempos: Vec<f64> = r.samples.iter().filter_map(|s| s.1).collect();
    assert!(tempos.windows(2).all(|w| w[1] >= w[0] - 0.05), "{tempos:?}");
    let last = *tempos.last().unwrap();
    assert!((last - 140.0).abs() < 0.3, "{last}");
    // Half way through the ramp the reading is near the middle.
    let mid = r.samples.iter().find(|s| s.0 >= 3.0).unwrap().1.unwrap();
    assert!((105.0..135.0).contains(&mid), "{mid}");
}

#[test]
fn an_offset_moves_every_clock_without_changing_the_tempo() {
    let r = run(&[(0, 120.0, false)], 2.5, 7);
    let tempo = r.cell.read().tempo.unwrap();
    assert!((tempo - 120.0).abs() < 0.2, "{tempo}");
}
