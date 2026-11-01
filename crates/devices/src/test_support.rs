//! Shared test helpers for device tests (all devices are driven through `DeviceHarness`).

use atomik_core::{NoteEvent, NoteKind};
use atomik_device_sdk::{Device, DeviceDescriptor};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

/// Sample rate used by most device tests.
pub const SR: f64 = 48_000.0;

/// A harness at [`SR`], 120 BPM, seed 0.
pub fn harness(device: Box<dyn Device>) -> DeviceHarness {
    DeviceHarness::new(device, SR)
}

/// A note-on event.
pub fn on(offset: u16, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On { key, velocity },
    }
}

/// A note-off event.
pub fn off(offset: u16, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Off { key },
    }
}

/// Every audio/CV output key of a descriptor.
pub fn output_keys(d: &DeviceDescriptor) -> Vec<&'static str> {
    use atomik_device_sdk::{Direction, SocketKind};
    d.sockets
        .iter()
        .filter(|s| s.direction == Direction::Output && s.kind != SocketKind::Notes)
        .map(|s| s.key)
        .collect()
}

/// Sets each param to its min then its max (processing a few batches each time) and asserts
/// every output stays finite. `set_param` and `reset` also run under the allocation guard
/// (covers them as well as `process`, which the harness guards itself).
pub fn sweep_params_min_max(h: &mut DeviceHarness, d: &DeviceDescriptor) {
    let outs = output_keys(d);
    for p in d.params {
        for v in [p.min, p.max, p.default] {
            assert_no_alloc(|| h.set_param(p.key, v));
            let rendered = h.render(4, &outs);
            for (key, x) in outs.iter().zip(&rendered) {
                assert!(
                    x.iter().all(|s| s.is_finite()),
                    "{}: non-finite output on {key} with {} = {v}",
                    d.type_id,
                    p.key
                );
            }
        }
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.render(2, &outs);
}

/// Asserts the descriptor's keys appear in exactly this order and validate.
pub fn assert_keys(d: &DeviceDescriptor, params: &[&str], sockets: &[&str]) {
    assert_eq!(d.validate(), Ok(()));
    let p: Vec<&str> = d.params.iter().map(|p| p.key).collect();
    let s: Vec<&str> = d.sockets.iter().map(|s| s.key).collect();
    assert_eq!(p, params);
    assert_eq!(s, sockets);
}

/// Processes `batches` batches, feeding `signal(n)` (absolute sample index from the first batch
/// of this call) into every key of `inputs`, and returns the concatenated `outputs`.
pub fn run_signal(
    h: &mut DeviceHarness,
    inputs: &[&str],
    signal: impl Fn(usize) -> f32,
    batches: usize,
    outputs: &[&str],
) -> Vec<Vec<f32>> {
    use atomik_core::BATCH;
    let mut out: Vec<Vec<f32>> = outputs.iter().map(|_| Vec::new()).collect();
    for b in 0..batches {
        let mut x = [0.0f32; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = signal(b * BATCH + i);
        }
        for key in inputs {
            *h.input_mut(key) = x;
        }
        h.process();
        for (dst, key) in out.iter_mut().zip(outputs) {
            dst.extend_from_slice(h.output(key));
        }
    }
    out
}

/// A sine of peak `amp` at `hz` (sample rate [`SR`]) as a function of the sample index.
pub fn sine(hz: f64, amp: f64) -> impl Fn(usize) -> f32 {
    move |n| (amp * (std::f64::consts::TAU * hz * n as f64 / SR).sin()) as f32
}

/// Amplitude of the `hz` component of `y`, where `y[0]` is absolute sample `start`
/// (single-bin DFT; exact when `y` spans whole cycles).
pub fn tone_amplitude(y: &[f32], hz: f64, start: usize) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for (i, v) in y.iter().enumerate() {
        let ph = std::f64::consts::TAU * hz * (start + i) as f64 / SR;
        s += f64::from(*v) * ph.sin();
        c += f64::from(*v) * ph.cos();
    }
    2.0 * (s * s + c * c).sqrt() / y.len() as f64
}

/// Gain in dB of a sine at `hz` through `input → output`: settles `settle_s` seconds, then
/// measures exactly one second (`SR` samples, whole cycles for integer `hz`).
pub fn sine_gain_db(
    h: &mut DeviceHarness,
    inputs: &[&str],
    output: &str,
    hz: f64,
    amp: f64,
    settle_s: f64,
) -> f64 {
    use atomik_core::BATCH;
    let settle = ((settle_s * SR) as usize).div_ceil(BATCH);
    let measure = (SR as usize).div_ceil(BATCH);
    let y = run_signal(h, inputs, sine(hz, amp), settle + measure, &[output]);
    let start = settle * BATCH;
    let window = &y[0][start..start + SR as usize];
    20.0 * (tone_amplitude(window, hz, start) / amp).log10()
}

/// Drives a multi-stage device one stage at a time, the way the engine does: before
/// stage `s` the outputs of stage `s` are zeroed, then `process_stage(s)` runs under the
/// allocation guard. Signals persist across calls (inputs are set by the test).
pub struct StageRig {
    /// The device under test (prepared at [`SR`]).
    pub device: Box<dyn Device>,
    /// One batch per socket.
    pub signals: Vec<atomik_core::Batch>,
    notes: Vec<atomik_core::NoteBuffer>,
    /// Connection flag per socket.
    pub connected: Vec<bool>,
}

impl StageRig {
    /// Prepares `device` at [`SR`] with every socket disconnected.
    pub fn new(mut device: Box<dyn Device>) -> Self {
        use atomik_core::{BATCH, NoteBuffer, SampleRate};
        device.prepare(&atomik_device_sdk::PrepareInfo {
            sample_rate: SampleRate::new(SR).unwrap(),
            seed: 0,
        });
        let n = device.descriptor().sockets.len();
        Self {
            device,
            signals: vec![[0.0; BATCH]; n],
            notes: (0..n).map(|_| NoteBuffer::with_capacity(0)).collect(),
            connected: vec![false; n],
        }
    }

    fn id(&self, key: &str) -> usize {
        self.device.descriptor().socket_by_key(key).unwrap().id.0 as usize
    }

    /// Sets an input to a constant and marks it connected.
    pub fn set_input(&mut self, key: &str, v: f32) {
        let i = self.id(key);
        self.signals[i] = [v; atomik_core::BATCH];
        self.connected[i] = true;
    }

    /// The current batch of a socket.
    pub fn get(&self, key: &str) -> &atomik_core::Batch {
        &self.signals[self.id(key)]
    }

    /// Runs one stage.
    pub fn stage(&mut self, stage: u8) {
        use atomik_core::{SampleRate, Tempo, TimeInfo};
        use atomik_device_sdk::{Direction, ProcessContext};
        let d = self.device.descriptor();
        for (i, s) in d.sockets.iter().enumerate() {
            if s.direction == Direction::Output && d.stage_of(s.id) == stage {
                self.signals[i] = [0.0; atomik_core::BATCH];
            }
        }
        let time = TimeInfo::at(
            0,
            SampleRate::new(SR).unwrap(),
            Tempo::new(120.0).unwrap(),
            true,
        );
        let device = &mut self.device;
        let mut ctx =
            ProcessContext::new(time, &mut self.signals, &mut self.notes, &self.connected);
        assert_no_alloc(|| device.process_stage(stage, &mut ctx));
    }
}

/// Asserts each socket's stage: the keys listed for stage `s` are exactly the sockets of stage `s`.
pub fn assert_stages(d: &DeviceDescriptor, stages: &[Vec<&str>]) {
    assert_eq!(d.validate(), Ok(()));
    assert_eq!(d.stages.len(), d.sockets.len());
    assert_eq!(usize::from(d.stage_count()), stages.len());
    for s in d.sockets {
        let want = stages
            .iter()
            .position(|keys| keys.contains(&s.key))
            .unwrap_or_else(|| panic!("{} not listed", s.key));
        assert_eq!(usize::from(d.stage_of(s.id)), want, "stage of {}", s.key);
    }
}

/// Renders the first batch, then `settle` more, and asserts that every sample of the first
/// batch already equals the settled (last) sample of each output: params set between `prepare`
/// and the first `process` apply without a ramp from the defaults.
pub fn assert_first_batch_settled(h: &mut DeviceHarness, outputs: &[&str], settle: usize) {
    let first = h.render(1, outputs);
    let later = h.render(settle, outputs);
    for ((key, f), l) in outputs.iter().zip(&first).zip(&later) {
        let steady = *l.last().unwrap();
        for (i, v) in f.iter().enumerate() {
            assert!(
                (v - steady).abs() <= 1e-5 * steady.abs().max(1.0),
                "{key}[{i}] = {v}, settled {steady}: ramped from the defaults"
            );
        }
    }
}

/// A sample map for devices that take samples: a stereo forward loop with a crossfade, a mono
/// ping-pong loop and a mono one-shot, layered over key 64 at every velocity (so a note on key
/// 64 sounds three voices). Buffers are 4096 frames of a deterministic wave.
pub fn demo_sample_map() -> atomik_device_sdk::SampleMapRef {
    use atomik_device_sdk::{LoopMode, SampleBuffer, SampleMap, TriggerMode, Zone};
    use std::sync::Arc;
    let wave = |period: f32| -> Vec<f32> {
        (0..4096)
            .map(|i| 0.5 * (i as f32 * std::f32::consts::TAU / period).sin())
            .collect()
    };
    let buffer = |chs: Vec<Vec<f32>>| Arc::new(SampleBuffer::new(chs.into()).unwrap());
    let zone = |buffer: u32| Zone {
        buffer,
        key_lo: 60,
        key_hi: 68,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 64,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: -6.0,
        pan: 0.0,
        start: 10,
        end: 4000,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: atomik_device_sdk::ZoneExt::NONE,
    };
    let looped = |buffer: u32, mode: LoopMode, crossfade: u64| Zone {
        loop_mode: mode,
        loop_start: 1000,
        loop_end: 3000,
        crossfade,
        pan: -0.3,
        ..zone(buffer)
    };
    let one_shot = Zone {
        trigger: TriggerMode::OneShot,
        ..zone(2)
    };
    Arc::new(
        SampleMap::new(
            vec![
                buffer(vec![wave(37.0), wave(41.0)]),
                buffer(vec![wave(53.0)]),
                buffer(vec![wave(29.0)]),
            ],
            vec![
                looped(0, LoopMode::Forward, 400),
                looped(1, LoopMode::PingPong, 0),
                one_shot,
            ],
        )
        .unwrap(),
    )
}

/// A sample map for the drum machine: pads 1..=4 (keys 36..=39) with eight overlapping layers
/// each (so one hit of the four pads takes all 32 voices), pad 16 (key 51) as a looped zone,
/// and a zone on key 60 that no pad plays.
pub fn drum_sample_map() -> atomik_device_sdk::SampleMapRef {
    use atomik_device_sdk::{LoopMode, SampleBuffer, SampleMap, TriggerMode, Zone};
    use std::sync::Arc;
    let wave = |period: f32| -> Vec<f32> {
        (0..4096)
            .map(|i| 0.1 * (i as f32 * std::f32::consts::TAU / period).sin())
            .collect()
    };
    let buffers = vec![
        Arc::new(SampleBuffer::new(vec![wave(37.0), wave(41.0)].into()).unwrap()),
        Arc::new(SampleBuffer::new(vec![wave(53.0)].into()).unwrap()),
    ];
    let zone = |buffer: u32, key: u8| Zone {
        buffer,
        key_lo: key,
        key_hi: key,
        vel_lo: 1,
        vel_hi: 127,
        root_key: key,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: -6.0,
        pan: 0.0,
        start: 10,
        end: 4000,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: atomik_device_sdk::ZoneExt::NONE,
    };
    let mut zones = Vec::new();
    for key in 36..=39u8 {
        for layer in 0..8u32 {
            zones.push(zone(layer % 2, key));
        }
    }
    zones.push(Zone {
        loop_mode: LoopMode::Forward,
        loop_start: 1000,
        loop_end: 3000,
        ..zone(0, 51)
    });
    zones.push(zone(1, 60));
    Arc::new(SampleMap::new(buffers, zones).unwrap())
}

/// A sample map for the loop slicer: one stereo loop (4096 frames) cut into six slices on keys
/// 24..=28 and 87 (one reversed, one panned, one pitched), plus a zone on key 60 that no slice
/// plays.
pub fn slicer_sample_map() -> atomik_device_sdk::SampleMapRef {
    use atomik_device_sdk::{LoopMode, SampleBuffer, SampleMap, TriggerMode, Zone};
    use std::sync::Arc;
    let wave = |period: f32| -> Vec<f32> {
        (0..4096)
            .map(|i| 0.3 * (i as f32 * std::f32::consts::TAU / period).sin())
            .collect()
    };
    let buffers = vec![Arc::new(
        SampleBuffer::new(vec![wave(37.0), wave(41.0)].into()).unwrap(),
    )];
    let slice = |key: u8, start: u64, end: u64| Zone {
        buffer: 0,
        key_lo: key,
        key_hi: key,
        vel_lo: 1,
        vel_hi: 127,
        root_key: key,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: -3.0,
        pan: 0.0,
        start,
        end,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: atomik_device_sdk::ZoneExt::NONE,
    };
    let zones = vec![
        slice(24, 0, 700),
        Zone {
            reverse: true,
            ext: atomik_device_sdk::ZoneExt::NONE,
            ..slice(25, 700, 1400)
        },
        Zone {
            pan: -0.5,
            ..slice(26, 1400, 2100)
        },
        Zone {
            transpose: 7,
            tune_cents: 20.0,
            ..slice(27, 2100, 2800)
        },
        slice(28, 2800, 3500),
        slice(87, 3500, 4096),
        slice(60, 0, 4096),
    ];
    Arc::new(SampleMap::new(buffers, zones).unwrap())
}

/// Step sequencer pattern data with every step on (a 16-step 1/16 pattern of rising keys, key
/// base shifted by `variant`), for harness checks of the player devices.
pub fn seq_player_data(variant: u8) -> atomik_device_sdk::PlayerDataRef {
    use atomik_device_sdk::{PlayerData, StepData, StepSeqData};
    let mut d = StepSeqData::default();
    for (i, s) in d.patterns[0].steps.iter_mut().enumerate().take(16) {
        *s = StepData {
            on: true,
            key: 48 + variant * 3 + (i % 12) as u8,
            velocity: 100,
            gate: 60,
            prob: 100,
            tie: i % 5 == 4,
        };
    }
    std::sync::Arc::new(PlayerData::StepSeq(d))
}

/// Drum pattern data with a kick on the beats, a snare on the backbeats and hats on the eighths
/// (a 16-step 1/16 pattern; the kick moves one step later per `variant`), for harness checks.
pub fn drum_player_data(variant: u8) -> atomik_device_sdk::PlayerDataRef {
    use atomik_device_sdk::{DrumData, DrumStep, PlayerData};
    let mut d = DrumData::default();
    let hit = |velocity| DrumStep {
        velocity,
        prob: 100,
    };
    for i in 0..16usize {
        if i % 4 == usize::from(variant) % 4 {
            d.patterns[0].steps[0][i] = hit(110);
        }
        if i % 8 == 4 {
            d.patterns[0].steps[2][i] = hit(100);
        }
        if i % 2 == 0 {
            d.patterns[0].steps[6][i] = hit(80);
        }
    }
    std::sync::Arc::new(PlayerData::Drums(d))
}

/// A controller event on a notes socket.
fn ctl_event(offset: u16, controller: u8, raw: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Cc {
            controller,
            value: f32::from(raw) / 127.0,
        },
    }
}

/// Renders `batches` batches, feeding each `(batch, event)` into `notes_in` before its batch.
fn render_scripted(
    mut h: DeviceHarness,
    outs: &[&str],
    batches: usize,
    script: &[(usize, NoteEvent)],
) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = outs.iter().map(|_| Vec::new()).collect();
    for b in 0..batches {
        for (at, e) in script {
            if *at == b {
                h.push_note("notes_in", *e);
            }
        }
        for (dst, x) in out.iter_mut().zip(h.render(1, outs)) {
            dst.extend(x);
        }
    }
    out
}

/// Frames per batch as an index factor for the scripted renders below.
const B: usize = atomik_core::BATCH;

/// Instrument contract: with the pedal down a note-off does not end the note, and the release
/// starts at the frame of the pedal-up event. `make` returns a fresh harness whose sound sustains
/// and releases within a few ms. Checks that the whole render equals a render without a pedal
/// whose note-off sits at the pedal-up frame, and that a note-off without a pedal does silence it.
pub fn assert_sustain_pedal(make: impl Fn() -> DeviceHarness, outs: &[&str], key: u8) {
    let (t_off, t_up, total) = (100usize, 200usize, 300usize);
    let up_offset = 10u16;
    let pedal = [
        (0, ctl_event(0, 64, 127)),
        (0, on(1, key, 1.0)),
        (t_off, off(3, key)),
        (t_up, ctl_event(up_offset, 64, 0)),
    ];
    let reference = [(0, on(1, key, 1.0)), (t_up, off(up_offset, key))];
    let plain = [(0, on(1, key, 1.0)), (t_off, off(3, key))];
    let a = render_scripted(make(), outs, total, &pedal);
    let b = render_scripted(make(), outs, total, &reference);
    let c = render_scripted(make(), outs, total, &plain);
    for (n, out) in outs.iter().enumerate() {
        let held = &a[n][(t_off + 20) * B..t_up * B];
        let gone = &c[n][(t_off + 100) * B..];
        let p_held = atomik_testkit::signal::peak(held);
        let p_gone = atomik_testkit::signal::peak(gone);
        assert!(p_held > 0.01, "{out}: the pedal holds the note ({p_held})");
        assert!(
            p_gone < p_held * 0.05,
            "{out}: without the pedal the note ends ({p_gone} vs {p_held})"
        );
        assert_eq!(a[n], b[n], "{out}: pedal up releases at its own frame");
        let tail = atomik_testkit::signal::peak(&a[n][(t_up + 100) * B..]);
        assert!(
            tail < p_held * 0.05,
            "{out}: and the note then ends ({tail})"
        );
    }
}

/// Instrument contract: CC 7 and CC 11 scale the output with a square law and CC 11 at zero is
/// silent; the volume does not change anything while it is full. `make` returns a fresh harness
/// with a steady sound.
pub fn assert_volume_and_expression(make: impl Fn() -> DeviceHarness, outs: &[&str], key: u8) {
    let batches = 120;
    let play = on(1, key, 1.0);
    let base = render_scripted(make(), outs, batches, &[(0, play)]);
    let full = render_scripted(
        make(),
        outs,
        batches,
        &[
            (0, ctl_event(0, 7, 127)),
            (0, ctl_event(0, 11, 127)),
            (0, play),
        ],
    );
    let half7 = render_scripted(
        make(),
        outs,
        batches,
        &[(0, ctl_event(0, 7, 64)), (0, play)],
    );
    let half11 = render_scripted(
        make(),
        outs,
        batches,
        &[(0, ctl_event(0, 11, 64)), (0, play)],
    );
    let zero = render_scripted(
        make(),
        outs,
        batches,
        &[(0, ctl_event(0, 11, 0)), (0, play)],
    );
    let late = 40 * B;
    for n in 0..outs.len() {
        assert_eq!(base[n], full[n], "{}: full volume changes nothing", outs[n]);
        let r0 = atomik_testkit::signal::rms(&base[n][late..]);
        assert!(r0 > 0.005, "{}: audible ({r0})", outs[n]);
        let want = (64.0f32 / 127.0).powi(2);
        for (name, x) in [("CC 7", &half7[n]), ("CC 11", &half11[n])] {
            let ratio = atomik_testkit::signal::rms(&x[late..]) / r0;
            assert!(
                (ratio - want).abs() < 0.02,
                "{}: {name} ratio {ratio}",
                outs[n]
            );
        }
        assert!(
            atomik_testkit::signal::peak(&zero[n][late..]) < 1e-6,
            "{}: expression 0 is silent",
            outs[n]
        );
    }
}

/// Instrument contract: the `bend.range` param and RPN 0 set the pitch-bend range in semitones.
/// Bend 0.5 at range 4 sounds exactly like bend 1 at the default 2, by the param and by RPN 0
/// (CC 101/100/6), and differs from no bend. `make` returns a fresh harness.
pub fn assert_bend_range(make: impl Fn() -> DeviceHarness, outs: &[&str], key: u8) {
    let batches = 40;
    let play = (0, on(8, key, 1.0));
    let bend = |value: f32| {
        (
            0,
            NoteEvent {
                offset: 3,
                kind: NoteKind::PitchBend { value },
            },
        )
    };
    let run =
        |h: DeviceHarness, script: &[(usize, NoteEvent)]| render_scripted(h, outs, batches, script);
    let reference = run(make(), &[bend(1.0), play]);
    let none = run(make(), &[play]);
    assert_ne!(reference, none, "the bend is heard");
    let mut by_param = make();
    by_param.set_param("bend.range", 4.0);
    assert_eq!(run(by_param, &[bend(0.5), play]), reference, "bend.range");
    let by_rpn = run(
        make(),
        &[
            (0, ctl_event(0, 101, 0)),
            (0, ctl_event(1, 100, 0)),
            (0, ctl_event(2, 6, 4)),
            bend(0.5),
            play,
        ],
    );
    assert_eq!(by_rpn, reference, "RPN 0");
    // The param is the starting point again after a reset.
    let mut h = make();
    for (_, e) in [
        (0, ctl_event(0, 101, 0)),
        (0, ctl_event(1, 100, 0)),
        (0, ctl_event(2, 6, 12)),
    ] {
        h.push_note("notes_in", e);
    }
    h.render(1, outs);
    h.device_mut().reset();
    assert_eq!(
        run(h, &[bend(1.0), play]),
        reference,
        "reset restores the param"
    );
}
