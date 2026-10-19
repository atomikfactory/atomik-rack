//! The CLAP test plugin library of Atomik Rack: `atomik.test.gain` (effect),
//! `atomik.test.synth` (instrument), `atomik.test.noteshift` (note effect) and
//! `atomik.test.params` (2 000 params and one that appears at run time), each a thin CLAP shell
//! over the DSP of `atomik-test-plugin-core`.

use atomik_test_clap_shell::{
    Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Requests, Status, clap_library,
    dialect, sys::plugin_features as feat,
};
use atomik_test_plugin_core::{BLOCK, Event, GainFx, NoteShift, ParamBank, TestSynth, TimedEvent};

fn spec(
    id: u32,
    name: &str,
    module: &str,
    min: f64,
    max: f64,
    default: f64,
    stepped: bool,
) -> ParamSpec {
    ParamSpec {
        id,
        name: name.to_string(),
        module: module.to_string(),
        min,
        max,
        default,
        stepped,
    }
}

// ----- the effect ---------------------------------------------------------------------------

struct Gain {
    fx: GainFx,
    restart: bool,
}

impl Dsp for Gain {
    fn ports(&self) -> Ports {
        Ports::effect(true)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            spec(GainFx::GAIN, "Gain", "Main", -60.0, 12.0, 0.0, false),
            spec(GainFx::MUTE, "Mute", "Main", 0.0, 1.0, 0.0, true),
            spec(
                GainFx::LATENCY,
                "Latency",
                "Advanced",
                0.0,
                512.0,
                0.0,
                true,
            ),
            spec(GainFx::DUCK, "Duck", "Advanced", 0.0, 1.0, 1.0, false),
        ]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        let before = self.fx.latency_frames();
        self.fx.set_param(id, value);
        if self.fx.latency_frames() != before {
            // A latency change needs a restart so the host re-reads `latency.get` (CLAP).
            self.restart = true;
        }
    }

    fn param(&self, id: u32) -> f64 {
        self.fx.param(id)
    }

    fn latency(&self) -> u32 {
        self.fx.latency_frames()
    }

    fn tail(&self) -> u32 {
        self.fx.tail_frames()
    }

    fn save(&self) -> Vec<u8> {
        self.fx.save_state()
    }

    fn load(&mut self, bytes: &[u8]) -> bool {
        self.fx.load_state(bytes)
    }

    fn reset(&mut self) {
        self.fx.reset();
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.set_param(id, value);
            }
        }
        let sidechain = io.sidechain;
        self.fx
            .process(&io.input, sidechain.as_ref(), &mut io.output);
        Status::Continue
    }

    fn take_requests(&mut self) -> Requests {
        Requests {
            restart: std::mem::take(&mut self.restart),
            callback: false,
        }
    }
}

// ----- the synth ----------------------------------------------------------------------------

struct Synth {
    synth: TestSynth,
    rate: f64,
    buf: Vec<TimedEvent>,
}

impl Dsp for Synth {
    fn activate(&mut self, sample_rate: f64) -> bool {
        if (sample_rate - self.rate).abs() > f64::EPSILON {
            let (wave, level) = (
                self.synth.param(TestSynth::WAVE),
                self.synth.param(TestSynth::LEVEL),
            );
            self.synth = TestSynth::new(sample_rate);
            self.synth.set_param(TestSynth::WAVE, wave);
            self.synth.set_param(TestSynth::LEVEL, level);
            self.rate = sample_rate;
        }
        true
    }

    fn ports(&self) -> Ports {
        Ports::instrument(dialect::CLAP | dialect::MIDI, dialect::CLAP)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            spec(TestSynth::WAVE, "Wave", "Main", 0.0, 1.0, 0.0, true),
            spec(TestSynth::LEVEL, "Level", "Main", 0.0, 1.0, 0.5, false),
        ]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.synth.set_param(id, value);
    }

    fn param(&self, id: u32) -> f64 {
        self.synth.param(id)
    }

    fn save(&self) -> Vec<u8> {
        self.synth.save_state()
    }

    fn load(&mut self, bytes: &[u8]) -> bool {
        self.synth.load_state(bytes)
    }

    fn reset(&mut self) {
        self.synth.reset();
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        self.buf.clear();
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.synth.set_param(id, value);
            } else if self.buf.len() < self.buf.capacity() {
                self.buf.push(*e);
            }
        }
        self.synth.process(&self.buf, &mut io.output);
        if self.synth.is_idle() {
            Status::Sleep
        } else {
            Status::Continue
        }
    }
}

// ----- the note effect ----------------------------------------------------------------------

struct Shift {
    shift: NoteShift,
    buf: Vec<TimedEvent>,
    out: Vec<TimedEvent>,
}

impl Dsp for Shift {
    fn ports(&self) -> Ports {
        Ports::note_effect()
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![spec(
            NoteShift::SEMITONES,
            "Semitones",
            "Main",
            -24.0,
            24.0,
            0.0,
            true,
        )]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.shift.set_param(id, value);
    }

    fn param(&self, id: u32) -> f64 {
        self.shift.param(id)
    }

    fn reset(&mut self) {
        self.shift.reset();
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        self.buf.clear();
        self.out.clear();
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.shift.set_param(id, value);
            } else if self.buf.len() < self.buf.capacity() {
                self.buf.push(*e);
            }
        }
        self.shift.process(&self.buf, &mut self.out);
        for e in &self.out {
            if io.out_events.len() < io.out_events.capacity() {
                io.out_events.push(*e);
            }
        }
        Status::Continue
    }
}

// ----- the param bank -----------------------------------------------------------------------

/// The id of the param that, set to 0.5 or more, makes the extra param appear (the plugin asks
/// for a restart, grows its table while deactivated and rescans).
const GROW: u32 = 1_999;

struct Bank {
    bank: ParamBank,
    grow: bool,
    restart: bool,
    pending: bool,
}

impl Dsp for Bank {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        let mut v: Vec<ParamSpec> = (0..2_000u32)
            .map(|id| {
                let name = if id == GROW {
                    "Grow".to_string()
                } else {
                    format!("Param {id}")
                };
                spec(
                    id,
                    &name,
                    &format!("Bank/Group {}", id / 100),
                    0.0,
                    1.0,
                    0.0,
                    false,
                )
            })
            .collect();
        if self.bank.extra_present() {
            v.push(spec(
                ParamBank::EXTRA_ID,
                "Extra",
                "Bank/Extra",
                0.0,
                1.0,
                0.0,
                false,
            ));
        }
        v
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.bank.set(id, value);
        if id == GROW && value >= 0.5 && !self.bank.extra_present() && !self.grow {
            self.grow = true;
            self.restart = true;
        }
    }

    fn param(&self, id: u32) -> f64 {
        self.bank.get(id)
    }

    fn deactivate(&mut self) -> bool {
        if self.grow {
            self.pending = true;
            self.grow = false;
        }
        self.pending
    }

    fn on_main_thread(&mut self) -> atomik_test_clap_shell::MainAction {
        if std::mem::take(&mut self.pending) {
            self.bank.add_extra();
            return atomik_test_clap_shell::MainAction {
                rescan_params: true,
            };
        }
        atomik_test_clap_shell::MainAction::default()
    }

    fn save(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 8 * self.bank.count());
        out.extend_from_slice(b"ATB1");
        out.extend_from_slice(&(self.bank.count() as u32).to_le_bytes());
        for id in 0..self.bank.count() as u32 {
            out.extend_from_slice(&self.bank.get(id).to_le_bytes());
        }
        out
    }

    fn load(&mut self, bytes: &[u8]) -> bool {
        let Some(rest) = bytes.strip_prefix(b"ATB1") else {
            return false;
        };
        let Some((count, values)) = rest.split_first_chunk::<4>() else {
            return false;
        };
        let count = u32::from_le_bytes(*count) as usize;
        if values.len() != count * 8 || count < 2_000 {
            return false;
        }
        if count > 2_000 {
            self.bank.add_extra();
        }
        let (chunks, _) = values.as_chunks::<8>();
        for (id, v) in chunks.iter().enumerate() {
            self.bank.set(id as u32, f64::from_le_bytes(*v));
        }
        true
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.set_param(id, value);
            }
        }
        io.output = io.input;
        let _ = BLOCK;
        Status::Continue
    }

    fn take_requests(&mut self) -> Requests {
        Requests {
            restart: std::mem::take(&mut self.restart),
            callback: false,
        }
    }
}

// ----- the library --------------------------------------------------------------------------

static GAIN_INFO: Info = Info {
    id: c"atomik.test.gain",
    name: c"Atomik Test Gain",
    features: &[
        feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT,
        feat::CLAP_PLUGIN_FEATURE_UTILITY,
        feat::CLAP_PLUGIN_FEATURE_STEREO,
    ],
    gui: true,
    remote_page: None,
};

static SYNTH_INFO: Info = Info {
    id: c"atomik.test.synth",
    name: c"Atomik Test Synth",
    features: &[
        feat::CLAP_PLUGIN_FEATURE_INSTRUMENT,
        feat::CLAP_PLUGIN_FEATURE_SYNTHESIZER,
        feat::CLAP_PLUGIN_FEATURE_STEREO,
    ],
    gui: false,
    remote_page: Some([
        TestSynth::LEVEL,
        TestSynth::WAVE,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
    ]),
};

static SHIFT_INFO: Info = Info {
    id: c"atomik.test.noteshift",
    name: c"Atomik Test Note Shift",
    features: &[feat::CLAP_PLUGIN_FEATURE_NOTE_EFFECT],
    gui: false,
    remote_page: None,
};

static PARAMS_INFO: Info = Info {
    id: c"atomik.test.params",
    name: c"Atomik Test Params",
    features: &[
        feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT,
        feat::CLAP_PLUGIN_FEATURE_UTILITY,
        feat::CLAP_PLUGIN_FEATURE_STEREO,
    ],
    gui: false,
    remote_page: None,
};

static PLUGINS: [PluginDef; 4] = [
    PluginDef {
        info: &GAIN_INFO,
        create: || {
            Box::new(Gain {
                fx: GainFx::new(48_000.0),
                restart: false,
            })
        },
    },
    PluginDef {
        info: &SYNTH_INFO,
        create: || {
            Box::new(Synth {
                synth: TestSynth::new(48_000.0),
                rate: 48_000.0,
                buf: Vec::with_capacity(8_192),
            })
        },
    },
    PluginDef {
        info: &SHIFT_INFO,
        create: || {
            Box::new(Shift {
                shift: NoteShift::new(),
                buf: Vec::with_capacity(8_192),
                out: Vec::with_capacity(1_024),
            })
        },
    },
    PluginDef {
        info: &PARAMS_INFO,
        create: || {
            Box::new(Bank {
                bank: ParamBank::new(),
                grow: false,
                restart: false,
                pending: false,
            })
        },
    },
];

clap_library!(LibraryDef::new(&PLUGINS, || true));
