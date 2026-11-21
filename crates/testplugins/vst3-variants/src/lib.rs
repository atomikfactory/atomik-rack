//! Extra VST3 test plugins for the host edge cases (`atomik-test-vst3-variants`): a split
//! component and controller effect, a mono effect, a surround-only effect (the host must refuse
//! it) and plugins that fail at initialise, setup, start or process, or produce non-finite
//! output, plugins that ask the host for restarts (from inside setup, after every block) and one
//! that retitles a param. Kept apart from `atomik-test-vst3` so that library lists exactly the
//! four plugins of that set.

use atomik_test_plugin_core::{BLOCK, Event, GainFx, TestSynth, TimedEvent};
use atomik_test_vst3_shell::{
    Arrangement, Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Status, vst3_library,
};

/// The gain effect, split into a component and a controller class.
struct SplitGain {
    fx: GainFx,
}

impl Dsp for SplitGain {
    fn ports(&self) -> Ports {
        Ports::effect(true)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec {
                id: GainFx::GAIN,
                name: "Gain".to_string(),
                module: "Main".to_string(),
                min: -64.0,
                max: 64.0,
                default: 0.0,
                stepped: false,
            },
            ParamSpec {
                id: GainFx::LATENCY,
                name: "Latency".to_string(),
                module: "Advanced".to_string(),
                min: 0.0,
                max: 512.0,
                default: 0.0,
                stepped: true,
            },
        ]
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
                self.fx.set_param(id, value);
            }
        }
        let sidechain = io.sidechain;
        self.fx
            .process(&io.input, sidechain.as_ref(), &mut io.output);
        Status::Continue
    }
}

/// A mono-in, mono-out effect: `out = in * level`, level param 1 (0 to 2, default 1).
struct Mono {
    level: f64,
}

impl Dsp for Mono {
    fn ports(&self) -> Ports {
        Ports {
            audio_in: Some(Arrangement::Mono),
            sidechain: false,
            audio_out: Some(Arrangement::Mono),
            notes_in: false,
            notes_out: false,
        }
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![ParamSpec {
            id: 1,
            name: "Level".to_string(),
            module: String::new(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: false,
        }]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        if id == 1 && value.is_finite() {
            self.level = value.clamp(0.0, 2.0);
        }
    }

    fn param(&self, id: u32) -> f64 {
        if id == 1 { self.level } else { 0.0 }
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.set_param(id, value);
            }
        }
        let level = self.level as f32;
        for i in 0..BLOCK {
            io.output[0][i] = io.input[0][i] * level;
        }
        Status::Continue
    }
}

/// A 5.1-only effect: the host negotiates stereo, the plugin refuses, the host must say why it
/// cannot use it.
struct Surround;

impl Dsp for Surround {
    fn ports(&self) -> Ports {
        Ports {
            audio_in: Some(Arrangement::Surround),
            sidechain: false,
            audio_out: Some(Arrangement::Surround),
            notes_in: false,
            notes_out: false,
        }
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// What a [`Faulty`] plugin gets wrong.
#[derive(Clone, Copy)]
enum Fault {
    Init,
    Activate,
    Start,
    Process,
    Nan,
    /// An error status on the third block only, a pass-through otherwise.
    ProcessOnce,
    /// Non-finite output on the third block only, a pass-through otherwise.
    NanOnce,
}

/// The block (counting from 1) a "once" fault hits.
const FAULT_BLOCK: u32 = 3;

/// A plugin with one thing wrong; the number is how many blocks it has processed.
struct Faulty(Fault, u32);

impl Dsp for Faulty {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn init(&mut self) -> bool {
        !matches!(self.0, Fault::Init)
    }

    fn activate(&mut self, _sample_rate: f64) -> bool {
        !matches!(self.0, Fault::Activate)
    }

    fn start_processing(&mut self) -> bool {
        !matches!(self.0, Fault::Start)
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        self.1 = self.1.saturating_add(1);
        let once = self.1 == FAULT_BLOCK;
        match self.0 {
            Fault::Process => Status::Error,
            Fault::ProcessOnce if once => Status::Error,
            Fault::Nan => {
                io.output = [[f32::NAN; BLOCK]; 2];
                Status::Continue
            }
            Fault::NanOnce if once => {
                io.output = [[f32::NAN; BLOCK]; 2];
                Status::Continue
            }
            Fault::ProcessOnce | Fault::NanOnce => {
                // Half the input, so that the dry input and the plugin's own output differ.
                for c in 0..2 {
                    for i in 0..BLOCK {
                        io.output[c][i] = io.input[c][i] * 0.5;
                    }
                }
                Status::Continue
            }
            _ => {
                io.output = io.input;
                Status::Continue
            }
        }
    }
}

/// A synth that behaves like a commercial wavetable synth towards the host: it reports a zero tail always (also while
/// a voice sounds or releases) and it makes no sound unless `setProcessing(true)` came after the
/// last `setProcessing(false)` or activation (its shell calls `reset` on `setProcessing(false)`).
struct Picky {
    synth: TestSynth,
    armed: bool,
    buf: Vec<TimedEvent>,
}

impl Dsp for Picky {
    fn ports(&self) -> Ports {
        Ports::instrument()
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![ParamSpec {
            id: TestSynth::LEVEL,
            name: "Level".to_string(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
        }]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.synth.set_param(id, value);
    }

    fn param(&self, id: u32) -> f64 {
        self.synth.param(id)
    }

    fn start_processing(&mut self) -> bool {
        self.armed = true;
        true
    }

    fn reset(&mut self) {
        self.armed = false;
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
        if self.armed {
            self.synth.process(&self.buf, &mut io.output);
        } else {
            io.output = [[0.0; BLOCK]; 2];
        }
        Status::Continue
    }
}

/// The first parameter id of the controller proxies of [`Proxies`]: `PROXY_BASE + channel * 128
/// + controller`.
pub const PROXY_BASE: u32 = 1_000;

/// An effect that stands every controller number of every channel for a parameter of its own
/// (16 x 128 of them), the way a plugin that emulates controllers does. The real parameters are
/// the first one (id 1), two that come after all the proxies (ids 2 and 3), and one that every
/// channel reaches through pitch bend (id 4).
struct Proxies {
    /// Answers each `IMidiMapping` call slowly.
    slow: bool,
}

impl Dsp for Proxies {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        let real = |id: u32, name: &str| ParamSpec {
            id,
            name: name.to_string(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
        };
        let mut out = vec![real(1, "First")];
        for channel in 0..16u32 {
            for controller in 0..128u32 {
                out.push(ParamSpec {
                    module: "MIDI Params".to_string(),
                    ..real(
                        PROXY_BASE + channel * 128 + controller,
                        &format!("MIDI CC {controller} Chan {}", channel + 1),
                    )
                });
            }
        }
        out.push(real(2, "Late A"));
        out.push(real(3, "Late B"));
        out.push(real(4, "Bend"));
        out
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.5
    }

    fn midi_assignment(&self, channel: i16, number: i16) -> Option<u32> {
        match number {
            0..=127 => Some(PROXY_BASE + u32::from(channel as u16) * 128 + number as u32),
            129 => Some(4),
            _ => None,
        }
    }

    fn midi_delay(&self) -> std::time::Duration {
        if self.slow {
            std::time::Duration::from_millis(2)
        } else {
            std::time::Duration::ZERO
        }
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// The class id words shared by this library: only the last word differs.
pub const fn cid(n: u32) -> [u32; 4] {
    [0xA70F_1C02, 0x4A5B_4C01, 0x8D2E_1F00, n]
}

/// `RestartFlags::kReloadComponent`.
const RELOAD: i32 = 1;
/// `RestartFlags::kLatencyChanged`.
const LATENCY: i32 = 8;
/// `RestartFlags::kParamTitlesChanged`.
const TITLES: i32 = 16;

/// A pass-through effect that tells the host from inside every activation that its latency
/// changed and asks for a reload, as plugins do while they set themselves up. Its latency is
/// the same all the time.
struct Chatty {
    asked: bool,
}

impl Dsp for Chatty {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn activate(&mut self, _sample_rate: f64) -> bool {
        self.asked = true;
        true
    }

    fn host_flags(&mut self) -> i32 {
        if std::mem::take(&mut self.asked) {
            LATENCY | RELOAD
        } else {
            0
        }
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// A pass-through effect that asks for a reload after every block, whatever happened.
struct Storm;

impl Dsp for Storm {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn host_flags(&mut self) -> i32 {
        RELOAD
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// An effect with one param whose title changes when the param is set to half or more or with the
/// `RESCAN_PARAMS` test command, and announces `kParamTitlesChanged`: the table has the same id,
/// range and steps after it.
struct Retitle {
    pending: bool,
    renamed: bool,
}

impl Dsp for Retitle {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![ParamSpec {
            id: 1,
            name: if self.renamed { "Drive" } else { "Level" }.to_string(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
        }]
    }

    fn set_param(&mut self, id: u32, value: f64) {
        if id == 1 && value >= 0.5 {
            self.pending = true;
        }
    }

    fn param(&self, _id: u32) -> f64 {
        0.5
    }

    fn host_flags(&mut self) -> i32 {
        if std::mem::take(&mut self.pending) {
            TITLES
        } else {
            0
        }
    }

    fn grow(&mut self) -> bool {
        self.renamed = true;
        true
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        for e in io.events {
            if let Event::Param { id, value } = e.event {
                self.set_param(id, value);
            }
        }
        io.output = io.input;
        Status::Continue
    }
}

macro_rules! info {
    ($name:ident, $text:expr, $n:expr) => {
        static $name: Info = Info {
            name: $text,
            cid: cid($n),
            controller_cid: None,
            sub_categories: "Fx",
            gui: false,
            midi: false,
        };
    };
}

static SPLIT_INFO: Info = Info {
    name: "Atomik Test Split Gain",
    cid: cid(1),
    controller_cid: Some(cid(0x101)),
    sub_categories: "Fx",
    gui: true,
    midi: false,
};

info!(MONO_INFO, "Atomik Test Mono", 2);
info!(SURROUND_INFO, "Atomik Test Surround", 3);
info!(INIT_INFO, "Atomik Test Init Fails", 4);
info!(ACTIVATE_INFO, "Atomik Test Activate Fails", 5);
info!(START_INFO, "Atomik Test Start Fails", 6);
info!(ERROR_INFO, "Atomik Test Error", 7);
info!(NAN_INFO, "Atomik Test Nan", 8);
info!(ERROR_ONCE_INFO, "Atomik Test Error Once", 12);
info!(NAN_ONCE_INFO, "Atomik Test Nan Once", 13);

static PICKY_INFO: Info = Info {
    name: "Atomik Test Picky Synth",
    cid: cid(9),
    controller_cid: None,
    sub_categories: "Instrument|Synth",
    gui: false,
    midi: false,
};

info!(PROXIES_INFO, "Atomik Test Proxies", 10);
info!(PROXIES_SLOW_INFO, "Atomik Test Slow Proxies", 11);
info!(CHATTY_INFO, "Atomik Test Chatty Activation", 14);
info!(STORM_INFO, "Atomik Test Restart Storm", 15);
info!(RETITLE_INFO, "Atomik Test Retitle", 16);

static PLUGINS: [PluginDef; 16] = [
    PluginDef {
        info: &CHATTY_INFO,
        create: || Box::new(Chatty { asked: false }),
    },
    PluginDef {
        info: &STORM_INFO,
        create: || Box::new(Storm),
    },
    PluginDef {
        info: &RETITLE_INFO,
        create: || {
            Box::new(Retitle {
                pending: false,
                renamed: false,
            })
        },
    },
    PluginDef {
        info: &SPLIT_INFO,
        create: || {
            Box::new(SplitGain {
                fx: GainFx::new(48_000.0),
            })
        },
    },
    PluginDef {
        info: &MONO_INFO,
        create: || Box::new(Mono { level: 1.0 }),
    },
    PluginDef {
        info: &SURROUND_INFO,
        create: || Box::new(Surround),
    },
    PluginDef {
        info: &INIT_INFO,
        create: || Box::new(Faulty(Fault::Init, 0)),
    },
    PluginDef {
        info: &ACTIVATE_INFO,
        create: || Box::new(Faulty(Fault::Activate, 0)),
    },
    PluginDef {
        info: &START_INFO,
        create: || Box::new(Faulty(Fault::Start, 0)),
    },
    PluginDef {
        info: &ERROR_INFO,
        create: || Box::new(Faulty(Fault::Process, 0)),
    },
    PluginDef {
        info: &NAN_INFO,
        create: || Box::new(Faulty(Fault::Nan, 0)),
    },
    PluginDef {
        info: &ERROR_ONCE_INFO,
        create: || Box::new(Faulty(Fault::ProcessOnce, 0)),
    },
    PluginDef {
        info: &NAN_ONCE_INFO,
        create: || Box::new(Faulty(Fault::NanOnce, 0)),
    },
    PluginDef {
        info: &PICKY_INFO,
        create: || {
            Box::new(Picky {
                synth: TestSynth::new(48_000.0),
                armed: false,
                buf: Vec::with_capacity(8_192),
            })
        },
    },
    PluginDef {
        info: &PROXIES_INFO,
        create: || Box::new(Proxies { slow: false }),
    },
    PluginDef {
        info: &PROXIES_SLOW_INFO,
        create: || Box::new(Proxies { slow: true }),
    },
];

vst3_library!(LibraryDef::new(&PLUGINS, || true));
