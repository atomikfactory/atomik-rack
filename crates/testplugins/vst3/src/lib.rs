//! The VST3 test plugin library of Atomik Rack: the same four plugins as the CLAP one,
//! `Atomik Test Gain` (effect), `Atomik Test Synth` (instrument), `Atomik Test Note Shift` (note
//! effect) and `Atomik Test Params` (2 000 params and one that appears at run time), each a
//! thin VST3 shell over the DSP of `atomik-test-plugin-core`, so the same signal through either
//! format gives the same bits. All four are single-component plugins; the split
//! variant lives in `atomik-test-vst3-variants`.

use atomik_test_plugin_core::{BLOCK, Event, GainFx, NoteShift, ParamBank, TestSynth, TimedEvent};
use atomik_test_vst3_shell::{
    Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Status, vst3_library,
};

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
}

impl Dsp for Gain {
    fn ports(&self) -> Ports {
        Ports::effect(true)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            // -64 to 64 dB: every whole dB is an exact normalised value, so a rack value and the plugin
            // plain value it maps to are the same number (the host tests compare bits).
            spec(GainFx::GAIN, "Gain", "Main", -64.0, 64.0, 0.0, false),
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

    fn activate(&mut self, sample_rate: f64) -> bool {
        // The reference effect has no rate-dependent state.
        let _ = sample_rate;
        true
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.fx.set_param(id, value);
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
        Ports::instrument()
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

    fn tail(&self) -> u32 {
        // Voices that sound or release are a tail without an end the host can count.
        if self.synth.is_idle() { 0 } else { u32::MAX }
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

    fn stop_processing(&mut self) {
        // A stop keeps the controller state (bend, wheel, pressure) like a real instrument. The
        // host only stops an idle one (no voice), so no sound is cut, and a sleeping synth stays
        // bit-identical to its CLAP twin, which sleeps without any reset.
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
        Status::Continue
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

/// The id of the param that, set to 0.5 or more, would make the extra param appear in the CLAP
/// plugin. The VST3 plugin grows on the `RESCAN_PARAMS` test command instead.
const GROW: u32 = 1_999;

struct Bank {
    bank: ParamBank,
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
    }

    fn param(&self, id: u32) -> f64 {
        self.bank.get(id)
    }

    fn grow(&mut self) -> bool {
        if self.bank.extra_present() {
            return false;
        }
        self.bank.add_extra();
        true
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
}

// ----- the library --------------------------------------------------------------------------

/// The class id of the gain effect (hex `a70f1c01...`, see `atomik_test_vst3::GAIN_CID`).
pub const GAIN_CID: [u32; 4] = [0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, 0x0000_0001];
/// The class id of the synth.
pub const SYNTH_CID: [u32; 4] = [0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, 0x0000_0002];
/// The class id of the note effect.
pub const SHIFT_CID: [u32; 4] = [0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, 0x0000_0003];
/// The class id of the param bank.
pub const PARAMS_CID: [u32; 4] = [0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, 0x0000_0004];

static GAIN_INFO: Info = Info {
    name: "Atomik Test Gain",
    cid: GAIN_CID,
    controller_cid: None,
    sub_categories: "Fx",
    gui: true,
    midi: false,
};

static SYNTH_INFO: Info = Info {
    name: "Atomik Test Synth",
    cid: SYNTH_CID,
    controller_cid: None,
    sub_categories: "Instrument|Synth",
    gui: false,
    midi: true,
};

static SHIFT_INFO: Info = Info {
    name: "Atomik Test Note Shift",
    cid: SHIFT_CID,
    controller_cid: None,
    sub_categories: "Instrument",
    gui: false,
    midi: false,
};

static PARAMS_INFO: Info = Info {
    name: "Atomik Test Params",
    cid: PARAMS_CID,
    controller_cid: None,
    sub_categories: "Fx",
    gui: false,
    midi: false,
};

static PLUGINS: [PluginDef; 4] = [
    PluginDef {
        info: &GAIN_INFO,
        create: || {
            Box::new(Gain {
                fx: GainFx::new(48_000.0),
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
            })
        },
    },
];

vst3_library!(LibraryDef::new(&PLUGINS, || true));
