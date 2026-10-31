//! The MIDI Out path end to end, without hardware: a song track plays into the real MIDI Out
//! device inside a real engine, the output thread logic ([`Core`]) turns its taps into timed
//! messages, and a fake port stands in for the cable. What comes out of the port is compared
//! with what the track holds: the same notes, on the device's channel, in order, at the time of
//! their frames (plus the device's latency offset), and nothing is left sounding after a stop, a
//! locate, a loop wrap, a deleted device or a flood.

use atomik_core::{ParamId, SampleRate, Tempo, Ticks};
use atomik_device_sdk::{Device, MidiTap, SocketId};
use atomik_devices::midi_out::MidiOut;
use atomik_engine::{
    DeviceInstanceId, Engine, EngineConfig, EngineHandle, FrameClock, Graph, SongInput, SongNote,
    SongTempo, SongTrack, TransportCommand, compile_song, new_engine,
};
use atomik_midi_io::MidiError;
use atomik_midi_io::out::{Core, MidiOutPort, Opener, Timeline};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SR: f64 = 48_000.0;
/// One quarter note at 120 BPM, in frames.
const QUARTER: u64 = 24_000;
const PPQ: i64 = atomik_core::PPQ;
const BLOCK: usize = 64;

#[allow(clippy::disallowed_methods)] // Justification: a test anchor; nothing reads the wall clock.
fn base() -> Instant {
    Instant::now()
}

/// `(time since the anchor, bytes)` of everything the fake cable carried.
type Wire = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

struct Cable {
    wire: Wire,
    now: Arc<Mutex<Instant>>,
}

impl MidiOutPort for Cable {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        let at = *self.now.lock().unwrap();
        self.wire.lock().unwrap().push((at, bytes.to_vec()));
        Ok(())
    }
}

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    device: DeviceInstanceId,
    tap: Arc<MidiTap>,
    clock: Arc<FrameClock>,
    core: Core,
    wire: Wire,
    now: Arc<Mutex<Instant>>,
    t0: Instant,
    /// The last frame the clock published.
    frames: u64,
}

/// The wall-clock time of engine frame `frame` on the rig's synthetic clock.
fn at(t0: Instant, frame: u64) -> Instant {
    t0 + Duration::from_secs_f64(frame as f64 / SR)
}

fn song_note(tick: i64, length: i64, key: u8, velocity: f32) -> SongNote {
    SongNote {
        tick: Ticks(tick),
        length,
        key,
        velocity,
    }
}

fn rig(notes: Vec<SongNote>, channel: u8) -> Rig {
    let c = EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let (engine, mut handle) = new_engine(c);
    let mut g = Graph::new(&c);
    let device = MidiOut::new();
    let tap = device.midi_tap().unwrap();
    let id = g.add_device(Box::new(device));
    handle.submit_plan(g.compile().unwrap()).unwrap();
    handle
        .set_param(id, ParamId(0), f32::from(channel))
        .unwrap();
    let input = SongInput {
        tracks: vec![SongTrack {
            device: id,
            notes_socket: SocketId(0),
            chase: true,
            notes,
            controllers: Vec::new(),
            automation: Vec::new(),
        }],
        tempo: vec![SongTempo {
            tick: Ticks(0),
            tempo: Tempo::new(120.0).unwrap(),
            ramp: false,
        }],
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    };
    handle.submit_song(compile_song(&input).unwrap()).unwrap();
    let clock = Arc::new(FrameClock::new());
    let timeline = Timeline::new(Arc::clone(&clock), SR, Arc::new(AtomicU64::new(0)));
    let t0 = base();
    let wire: Wire = Arc::default();
    let now = Arc::new(Mutex::new(t0));
    let opener: Opener = {
        let (wire, now) = (Arc::clone(&wire), Arc::clone(&now));
        Arc::new(move |_| {
            Ok(Box::new(Cable {
                wire: Arc::clone(&wire),
                now: Arc::clone(&now),
            }) as Box<dyn MidiOutPort>)
        })
    };
    let mut core = Core::new(opener, timeline);
    core.set_taps(vec![(1, Arc::clone(&tap))], t0);
    core.set_ports(
        &BTreeMap::from([(1u32, Some("Hardware Synth".to_owned()))]),
        t0,
    );
    Rig {
        engine,
        handle,
        device: id,
        tap,
        clock,
        core,
        wire,
        now,
        t0,
        frames: 0,
    }
}

impl Rig {
    /// Renders `blocks` blocks of 64 frames, publishing the frame clock at each one's start.
    fn render(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.clock.publish(
                self.engine.host_frames(),
                at(self.t0, self.engine.host_frames()),
            );
            let mut left = [0.0f32; BLOCK];
            let mut right = [0.0f32; BLOCK];
            self.engine.render(&[], &mut [&mut left, &mut right]);
            self.frames = self.engine.host_frames();
        }
    }

    /// Runs the output thread's passes every half millisecond up to `frame`, as the thread
    /// would, and logs the time each message left at.
    fn sweep(&mut self, until_frame: u64) {
        let mut t = *self.now.lock().unwrap();
        let end = at(self.t0, until_frame);
        while t <= end {
            *self.now.lock().unwrap() = t;
            self.core.step(t);
            t += Duration::from_micros(500);
        }
        *self.now.lock().unwrap() = end;
    }

    /// What the cable carried so far, with the times relative to the anchor.
    fn wire(&self) -> Vec<(Duration, Vec<u8>)> {
        self.wire
            .lock()
            .unwrap()
            .iter()
            .map(|(t, b)| (t.duration_since(self.t0), b.clone()))
            .collect()
    }
}

/// The notes still sounding after replaying `wire`: `(channel, key)`.
fn sounding(wire: &[(Duration, Vec<u8>)]) -> BTreeSet<(u8, u8)> {
    let mut held = BTreeSet::new();
    for (_, b) in wire {
        let channel = b[0] & 0x0F;
        match (b[0] & 0xF0, b.get(1), b.get(2)) {
            (0x90, Some(k), Some(v)) if *v > 0 => {
                held.insert((channel, *k));
            }
            (0x90 | 0x80, Some(k), _) => {
                held.remove(&(channel, *k));
            }
            (0xB0, Some(123 | 120), _) => held.retain(|(c, _)| *c != channel),
            _ => {}
        }
    }
    held
}

fn secs(frame: u64) -> Duration {
    Duration::from_secs_f64(frame as f64 / SR)
}

#[test]
fn a_track_comes_out_as_the_same_notes_on_the_channel_in_order_and_on_time() {
    // Quarter notes, a chord and a note that starts between two blocks.
    let q = PPQ;
    let notes = vec![
        song_note(0, q, 60, 1.0),
        song_note(q, q / 2, 64, 0.5),
        song_note(q + q / 4, q / 4, 67, 0.25),
        song_note(2 * q, q, 62, 0.8),
        song_note(2 * q, q, 66, 0.8),
        song_note(3 * q + 7, 100, 71, 0.6),
    ];
    let mut rig = rig(notes.clone(), 10);
    rig.render(((5 * QUARTER) as usize).div_ceil(BLOCK));
    rig.handle.transport(TransportCommand::Stop).unwrap();
    rig.render(2);
    // Nothing was played: the transport never started.
    rig.sweep(rig.frames);
    assert!(rig.wire().is_empty(), "a stopped transport plays nothing");

    // Again, playing.
    let mut rig = self::rig(notes.clone(), 10);
    rig.handle.transport(TransportCommand::Play).unwrap();
    rig.render(((5 * QUARTER) as usize).div_ceil(BLOCK));
    rig.sweep(rig.frames);
    let wire = rig.wire();
    let frame_of = |tick: i64| (tick as f64 * QUARTER as f64 / PPQ as f64).round() as u64;
    let mut want: Vec<(u64, Vec<u8>)> = Vec::new();
    for n in &notes {
        let velocity = (n.velocity * 127.0).round() as u8;
        want.push((frame_of(n.tick.0), vec![0x99, n.key, velocity]));
        want.push((frame_of(n.tick.0 + n.length), vec![0x89, n.key, 0]));
    }
    want.sort_by_key(|(f, _)| *f);
    assert_eq!(wire.len(), want.len(), "{wire:?}");
    // The bytes are the track's notes in time order (equal times in any order).
    let by_time = |v: &[(Duration, Vec<u8>)]| -> Vec<Vec<u8>> {
        let mut v = v.to_vec();
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, b)| b).collect()
    };
    let mut want_sorted = want.clone();
    want_sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    assert_eq!(
        by_time(&wire),
        want_sorted.into_iter().map(|(_, b)| b).collect::<Vec<_>>()
    );
    // Each left no earlier than its frame is heard and within a pass (0.5 ms) of it. The notes
    // keep their order on the cable.
    for ((sent, bytes), (frame, expected)) in wire.iter().zip(&want) {
        let due = secs(*frame);
        if bytes == expected {
            assert!(
                *sent >= due && *sent - due <= Duration::from_micros(600),
                "{bytes:?} due {due:?} sent {sent:?}"
            );
        }
    }
    assert!(sounding(&wire).is_empty());
}

#[test]
fn the_latency_offset_moves_every_message_in_time_and_nothing_else() {
    let notes = vec![song_note(0, PPQ, 60, 1.0), song_note(PPQ, PPQ, 62, 1.0)];
    let mut plain = rig(notes.clone(), 1);
    plain.handle.transport(TransportCommand::Play).unwrap();
    plain.render(((3 * QUARTER) as usize).div_ceil(BLOCK));
    plain.sweep(plain.frames);

    let mut late = rig(notes, 1);
    late.handle
        .set_param(late.device, ParamId(4), 25.0)
        .unwrap();
    late.handle.transport(TransportCommand::Play).unwrap();
    late.render(((3 * QUARTER) as usize).div_ceil(BLOCK));
    late.sweep(late.frames + 2_400);
    let (a, b) = (plain.wire(), late.wire());
    assert_eq!(a.len(), b.len());
    for ((ta, ba), (tb, bb)) in a.iter().zip(&b) {
        assert_eq!(ba, bb, "the same messages");
        let shift = tb.as_secs_f64() - ta.as_secs_f64();
        // 25 ms of latency, give or take the two passes' granularity.
        assert!((shift - 0.025).abs() < 0.0012, "{shift}");
    }
}

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

#[test]
fn nothing_hangs_after_stops_locates_loop_wraps_and_a_dense_passage() {
    // Stops and locates that found notes sounding: the cases the test is about.
    let mut interrupted = 0;
    for seed in 0..12u64 {
        let mut rng = Lcg(seed + 1);
        // A dense pass: up to 400 notes over 8 beats, lengths from a sixteenth to two beats.
        let notes: Vec<SongNote> = (0..400)
            .map(|_| {
                song_note(
                    rng.next(8 * PPQ as u64) as i64,
                    (PPQ / 4) + rng.next(2 * PPQ as u64) as i64,
                    36 + rng.next(60) as u8,
                    0.2 + rng.next(80) as f32 / 100.0,
                )
            })
            .collect();
        let mut rig = rig(notes, 1 + (seed % 16) as u8);
        if seed % 3 == 0 {
            rig.handle
                .transport(TransportCommand::SetLoop {
                    start: Ticks(PPQ),
                    end: Ticks(3 * PPQ),
                    enabled: true,
                })
                .unwrap();
        }
        rig.handle.transport(TransportCommand::Play).unwrap();
        for _ in 0..30 {
            rig.render(1 + rng.next(300) as usize);
            rig.sweep(rig.frames);
            let op = rng.next(4);
            if op < 2 && !sounding(&rig.wire()).is_empty() {
                interrupted += 1;
            }
            match op {
                0 => rig.handle.transport(TransportCommand::Stop).unwrap(),
                1 => rig
                    .handle
                    .transport(TransportCommand::Locate(Ticks(
                        rng.next(8 * PPQ as u64) as i64
                    )))
                    .unwrap(),
                2 => rig.handle.transport(TransportCommand::Play).unwrap(),
                _ => {}
            }
            // The output thread drains while the engine runs.
            rig.sweep(rig.frames);
        }
        rig.handle.transport(TransportCommand::Stop).unwrap();
        rig.render(4);
        rig.sweep(rig.frames + 4_800);
        let wire = rig.wire();
        assert!(!wire.is_empty(), "seed {seed}: something played");
        assert_eq!(
            sounding(&wire),
            BTreeSet::new(),
            "seed {seed}: no note is left sounding after a stop"
        );
        assert_eq!(rig.tap.dropped(), 0, "seed {seed}");
    }
    assert!(
        interrupted > 20,
        "{interrupted} stops and locates cut notes short"
    );
}

#[test]
fn a_deleted_device_leaves_nothing_sounding() {
    let notes = vec![
        song_note(0, 8 * PPQ, 60, 1.0),
        song_note(0, 8 * PPQ, 64, 1.0),
    ];
    let mut rig = rig(notes, 3);
    rig.handle.transport(TransportCommand::Play).unwrap();
    rig.render(100);
    rig.sweep(rig.frames);
    assert_eq!(sounding(&rig.wire()).len(), 2);
    // The device goes away without a word: the app forgets its tap.
    let t = at(rig.t0, rig.frames);
    rig.core.set_taps(Vec::new(), t);
    let wire = rig.wire();
    assert!(sounding(&wire).is_empty(), "{wire:?}");
}

#[test]
fn a_flood_of_controllers_never_costs_a_note_off() {
    let notes = vec![song_note(0, PPQ, 60, 1.0)];
    let mut rig = rig(notes, 1);
    rig.handle.transport(TransportCommand::Play).unwrap();
    // The reader is away for a while: live controllers (pitch bend) fill the queue.
    rig.render(20);
    let tap = Arc::clone(&rig.tap);
    for frame in 0..10_000u64 {
        let _ = tap.push(atomik_device_sdk::MidiEvent::new(frame, &[0xE0, 0, 64]).unwrap());
    }
    assert!(tap.dropped() > 0, "the flood overflowed");
    rig.render(((2 * QUARTER) as usize).div_ceil(BLOCK));
    rig.sweep(rig.frames + 2_400);
    let wire = rig.wire();
    assert!(
        sounding(&wire).is_empty(),
        "the note off was never dropped: {} messages",
        wire.len()
    );
}
