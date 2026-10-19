//! Extra CLAP test plugins for the host's edge cases: a MIDI-only synth, a mono effect, and
//! plugins that fail at init, activation, start or process, or produce non-finite output
//! (`atomik-test-clap-variants`). Kept apart from `atomik-test-clap` so that library keeps
//! exactly the four plugins of the CLAP test set.

use atomik_test_clap_shell::{
    Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Status, clap_library, dialect,
    sys::plugin_features as feat,
};
use atomik_test_plugin_core::{BLOCK, Event, TestSynth, TimedEvent};

/// The test synth, but its note port offers MIDI 1.0 only.
struct MidiSynth {
    synth: TestSynth,
    buf: Vec<TimedEvent>,
}

impl Dsp for MidiSynth {
    fn ports(&self) -> Ports {
        Ports::instrument(dialect::MIDI, dialect::MIDI)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.synth.set_param(id, value);
    }

    fn param(&self, id: u32) -> f64 {
        self.synth.param(id)
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        self.buf.clear();
        for e in io.events {
            if self.buf.len() < self.buf.capacity() && !matches!(e.event, Event::Param { .. }) {
                self.buf.push(*e);
            }
        }
        self.synth.process(&self.buf, &mut io.output);
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
            audio_in: 1,
            sidechain: false,
            audio_out: 1,
            notes_in: 0,
            notes_in_preferred: 0,
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

/// What a [`Faulty`] plugin gets wrong.
#[derive(Clone, Copy)]
enum Fault {
    Init,
    Activate,
    Start,
    Process,
    Nan,
}

struct Faulty(Fault);

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
        match self.0 {
            Fault::Process => Status::Error,
            Fault::Nan => {
                io.output = [[f32::NAN; BLOCK]; 2];
                Status::Continue
            }
            _ => {
                io.output = io.input;
                Status::Continue
            }
        }
    }
}

macro_rules! info {
    ($name:ident, $id:expr, $feature:expr) => {
        static $name: Info = Info {
            id: $id,
            name: $id,
            features: &[$feature],
            gui: false,
            remote_page: None,
        };
    };
}

info!(
    MIDI_INFO,
    c"atomik.test.midisynth",
    feat::CLAP_PLUGIN_FEATURE_INSTRUMENT
);
info!(
    MONO_INFO,
    c"atomik.test.mono",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);
info!(
    INIT_INFO,
    c"atomik.test.init-fails",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);
info!(
    ACTIVATE_INFO,
    c"atomik.test.activate-fails",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);
info!(
    START_INFO,
    c"atomik.test.start-fails",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);
info!(
    ERROR_INFO,
    c"atomik.test.error",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);
info!(
    NAN_INFO,
    c"atomik.test.nan",
    feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT
);

static PLUGINS: [PluginDef; 7] = [
    PluginDef {
        info: &MIDI_INFO,
        create: || {
            Box::new(MidiSynth {
                synth: TestSynth::new(48_000.0),
                buf: Vec::with_capacity(8_192),
            })
        },
    },
    PluginDef {
        info: &MONO_INFO,
        create: || Box::new(Mono { level: 1.0 }),
    },
    PluginDef {
        info: &INIT_INFO,
        create: || Box::new(Faulty(Fault::Init)),
    },
    PluginDef {
        info: &ACTIVATE_INFO,
        create: || Box::new(Faulty(Fault::Activate)),
    },
    PluginDef {
        info: &START_INFO,
        create: || Box::new(Faulty(Fault::Start)),
    },
    PluginDef {
        info: &ERROR_INFO,
        create: || Box::new(Faulty(Fault::Process)),
    },
    PluginDef {
        info: &NAN_INFO,
        create: || Box::new(Faulty(Fault::Nan)),
    },
];

clap_library!(LibraryDef::new(&PLUGINS, || true));
