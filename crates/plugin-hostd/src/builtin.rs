//! The built-in `null` test plugins: small deterministic plugins that need no plugin file, so
//! the sandbox can be tested without building a real one. They are private to this crate; the
//! parent names them by id with the format `null`:
//!
//! * `null.gain`: stereo effect, output = input x 0.5; param 0 "Pass" (0 or 1) at 1 gives
//!   output = input. Events apply at their frame offset. Its state is the Pass value.
//! * `null.latency`: passthrough that reports 32 frames of latency.
//! * `null.crash`: passthrough that aborts the process in `process` after N batches (N from
//!   `ATOMIK_NULL_CRASH_AFTER`, default 0, read at activation).
//! * `null.hang`: `process` never returns.
//! * `null.hang-activate`: `activate` never returns.
//! * `null.noisy`: a passthrough that prints to stdout everywhere (instantiate, activate, process,
//!   poll), with `println!` and with raw writes, to test that stdout is not the control channel.
//! * `null.synth`: an instrument, a sine per held note, cut at note off.

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    PluginStateBlob,
};
use atomik_plugin_host::{
    ActivateConfig, BackendError, HostHandle, NativeParent, PluginBackend, PluginEventKind,
    PluginInstance, PluginIo, PluginProcessor, ProcessStatus, ScannedPlugin,
};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Environment variable: how many batches `null.crash` processes before it aborts.
pub const CRASH_AFTER_ENV: &str = "ATOMIK_NULL_CRASH_AFTER";

/// Frames of latency `null.latency` reports.
const LATENCY: u32 = 32;

/// The backend of the `null` format.
#[derive(Debug, Default)]
pub struct BuiltinBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gain,
    Latency,
    Crash,
    Hang,
    HangActivate,
    Synth,
    Noisy,
}

impl Kind {
    fn of(id: &str) -> Option<Self> {
        Some(match id {
            "null.gain" => Self::Gain,
            "null.latency" => Self::Latency,
            "null.crash" => Self::Crash,
            "null.hang" => Self::Hang,
            "null.hang-activate" => Self::HangActivate,
            "null.synth" => Self::Synth,
            "null.noisy" => Self::Noisy,
            _ => return None,
        })
    }
}

fn layout_of(kind: Kind, id: &str) -> PluginLayout {
    let synth = kind == Kind::Synth;
    PluginLayout {
        name: id.to_string(),
        vendor: "Atomik".to_string(),
        version: "1".to_string(),
        kind_hint: if synth {
            PluginKindHint::Instrument
        } else {
            PluginKindHint::Effect
        },
        main_in: (!synth).then_some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: synth,
        notes_out: false,
        params: if kind == Kind::Gain {
            vec![PluginParamInfo {
                id: 0,
                name: "Pass".to_string(),
                module: String::new(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                stepped: true,
                steps: 1,
            }]
        } else {
            Vec::new()
        },
    }
}

/// Sleeps for ever, one second at a time (a hang that does not burn a core).
fn hang() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

struct BuiltinInstance {
    kind: Kind,
    layout: PluginLayout,
    /// The Pass value as f64 bits: shared with the processor, which applies param events to it.
    pass: Arc<AtomicU64>,
}

impl PluginInstance for BuiltinInstance {
    fn layout(&self) -> &PluginLayout {
        &self.layout
    }

    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError> {
        Ok(match self.kind {
            Kind::HangActivate => hang(),
            Kind::Gain => Box::new(GainProcessor {
                pass: Arc::clone(&self.pass),
            }),
            Kind::Latency => Box::new(PassProcessor {
                crash_after: None,
                batches: 0,
            }),
            Kind::Crash => {
                let after = std::env::var(CRASH_AFTER_ENV)
                    .ok()
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .unwrap_or(0);
                Box::new(PassProcessor {
                    crash_after: Some(after),
                    batches: 0,
                })
            }
            Kind::Hang => Box::new(HangProcessor),
            Kind::Noisy => {
                chatter("noisy activate");
                Box::new(NoisyProcessor)
            }
            Kind::Synth => Box::new(SynthProcessor::new(cfg.sample_rate)),
        })
    }

    fn deactivate(&mut self, _processor: Box<dyn PluginProcessor>) {}

    fn latency_frames(&self) -> u32 {
        if self.kind == Kind::Latency {
            LATENCY
        } else {
            0
        }
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        Ok(PluginStateBlob(
            self.pass.load(Ordering::Relaxed).to_le_bytes().to_vec(),
        ))
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        let bytes: [u8; 8] =
            state.0.as_slice().try_into().map_err(|_| {
                BackendError::Failed("the null plugin state is 8 bytes".to_string())
            })?;
        if !f64::from_le_bytes(bytes).is_finite() {
            return Err(BackendError::Failed(
                "the null plugin state is not a number".to_string(),
            ));
        }
        self.pass
            .store(u64::from_le_bytes(bytes), Ordering::Relaxed);
        Ok(())
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        (self.kind == Kind::Gain && id == 0)
            .then(|| if plain >= 0.5 { "pass" } else { "half" }.to_string())
    }

    fn open_gui(&mut self, _parent: NativeParent, _scale: f64) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "the null plugins have no editor".to_string(),
        ))
    }

    fn close_gui(&mut self) {}

    fn poll(&mut self) {
        if self.kind == Kind::Noisy {
            chatter("noisy poll");
        }
    }
}

impl PluginBackend for BuiltinBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn scan_file(&self, _path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        Err(BackendError::Unsupported(
            "the null format has no plugin files".to_string(),
        ))
    }

    fn instantiate(
        &self,
        plugin: &PluginRef,
        _host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        let kind =
            Kind::of(&plugin.id).ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        if kind == Kind::Noisy {
            chatter("noisy instantiate");
        }
        Ok(Box::new(BuiltinInstance {
            kind,
            layout: layout_of(kind, &plugin.id),
            pass: Arc::new(AtomicU64::new(0f64.to_bits())),
        }))
    }
}

// ----- processors ------------------------------------------------------------------------------

/// Copies the main input to the main output, channel for channel (a mono input feeds every
/// output channel, no input gives silence).
fn copy_through(io: &mut PluginIo<'_>) {
    for (ch, out) in io.main_out.iter_mut().enumerate() {
        match io.main_in.get(ch).or_else(|| io.main_in.first()) {
            Some(src) => *out = *src,
            None => *out = [0.0; BATCH],
        }
    }
}

struct GainProcessor {
    pass: Arc<AtomicU64>,
}

impl GainProcessor {
    fn gain_of(pass: f64) -> Option<f32> {
        (pass < 0.5).then_some(0.5)
    }

    fn fill(io: &mut PluginIo<'_>, from: usize, to: usize, gain: Option<f32>) {
        for (ch, out) in io.main_out.iter_mut().enumerate() {
            let Some(src) = io.main_in.get(ch).or_else(|| io.main_in.first()) else {
                continue;
            };
            match gain {
                Some(g) => {
                    for i in from..to {
                        out[i] = src[i] * g;
                    }
                }
                None => out[from..to].copy_from_slice(&src[from..to]),
            }
        }
    }
}

impl PluginProcessor for GainProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let mut pass = f64::from_bits(self.pass.load(Ordering::Relaxed));
        let mut pos = 0;
        for e in io.events_in {
            if let PluginEventKind::Param { id: 0, value } = e.kind {
                let at = usize::from(e.offset).min(BATCH).max(pos);
                Self::fill(io, pos, at, Self::gain_of(pass));
                pos = at;
                if value.is_finite() {
                    pass = value;
                }
            }
        }
        Self::fill(io, pos, BATCH, Self::gain_of(pass));
        self.pass.store(pass.to_bits(), Ordering::Relaxed);
        ProcessStatus::Continue
    }
}

/// A passthrough that can abort the process after some batches.
struct PassProcessor {
    crash_after: Option<u32>,
    batches: u32,
}

impl PluginProcessor for PassProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        if self.crash_after.is_some_and(|n| self.batches >= n) {
            std::process::abort();
        }
        self.batches = self.batches.saturating_add(1);
        copy_through(io);
        ProcessStatus::Continue
    }
}

/// Prints `what` to stdout twice: with `println!` and with a raw write to the handle.
fn chatter(what: &str) {
    println!("{what}");
    let mut out = std::io::stdout();
    let raw = what.replacen("noisy", "raw", 1);
    let _ = out.write_all(raw.as_bytes());
    let _ = out.flush();
}

struct NoisyProcessor;

impl PluginProcessor for NoisyProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        chatter("noisy process");
        copy_through(io);
        ProcessStatus::Continue
    }
}

struct HangProcessor;

impl PluginProcessor for HangProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, _io: &mut PluginIo<'_>) -> ProcessStatus {
        hang()
    }
}

/// Voices of the synth.
const VOICES: usize = 16;

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    key: u8,
    note_id: i32,
    phase: f64,
    step: f64,
    amp: f32,
}

struct SynthProcessor {
    sample_rate: f64,
    voices: [Voice; VOICES],
}

impl SynthProcessor {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            voices: [Voice::default(); VOICES],
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, note_id: i32) {
        let hz = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0);
        let slot = self.voices.iter().position(|v| !v.active).unwrap_or(0);
        self.voices[slot] = Voice {
            active: true,
            key,
            note_id,
            phase: 0.0,
            step: hz / self.sample_rate,
            amp: velocity * 0.25,
        };
    }

    fn note_off(&mut self, key: u8, note_id: i32) {
        for v in &mut self.voices {
            if v.active && v.key == key && v.note_id == note_id {
                v.active = false;
            }
        }
    }
}

impl PluginProcessor for SynthProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
    }

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let mut next = 0;
        for i in 0..BATCH {
            while let Some(e) = io.events_in.get(next)
                && usize::from(e.offset) <= i
            {
                match e.kind {
                    PluginEventKind::NoteOn {
                        key,
                        velocity,
                        note_id,
                    } => self.note_on(key, velocity, note_id),
                    PluginEventKind::NoteOff { key, note_id } => self.note_off(key, note_id),
                    _ => {}
                }
                next += 1;
            }
            let mut sample = 0.0f32;
            for v in self.voices.iter_mut().filter(|v| v.active) {
                sample += (v.phase * std::f64::consts::TAU).sin() as f32 * v.amp;
                v.phase += v.step;
                v.phase -= v.phase.floor();
            }
            for out in io.main_out.iter_mut() {
                let out: &mut Batch = out;
                out[i] = sample;
            }
        }
        ProcessStatus::Continue
    }
}
