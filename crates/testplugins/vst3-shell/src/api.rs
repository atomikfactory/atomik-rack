//! What a plugin author writes: static facts, ports, a parameter table and the DSP trait.
//! The same shape as the CLAP shell's, so the two test plugin libraries wrap the same DSP.

use atomik_test_plugin_core::{Block, TimedEvent};

/// The speaker arrangement of a bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    /// One channel.
    Mono,
    /// Two channels.
    Stereo,
    /// 5.1 (a host without surround support must refuse the plugin).
    Surround,
}

/// Which buses a plugin has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// The main audio input, if any.
    pub audio_in: Option<Arrangement>,
    /// A stereo aux input (the sidechain) exists.
    pub sidechain: bool,
    /// The main audio output, if any.
    pub audio_out: Option<Arrangement>,
    /// An event input bus exists.
    pub notes_in: bool,
    /// An event output bus exists.
    pub notes_out: bool,
}

impl Ports {
    /// A stereo effect, with a sidechain when `sidechain`.
    pub const fn effect(sidechain: bool) -> Self {
        Self {
            audio_in: Some(Arrangement::Stereo),
            sidechain,
            audio_out: Some(Arrangement::Stereo),
            notes_in: false,
            notes_out: false,
        }
    }

    /// A stereo instrument.
    pub const fn instrument() -> Self {
        Self {
            audio_in: None,
            sidechain: false,
            audio_out: Some(Arrangement::Stereo),
            notes_in: true,
            notes_out: false,
        }
    }

    /// A note effect.
    pub const fn note_effect() -> Self {
        Self {
            audio_in: None,
            sidechain: false,
            audio_out: None,
            notes_in: true,
            notes_out: true,
        }
    }
}

/// Static facts about one plugin.
#[derive(Debug)]
pub struct Info {
    /// Name.
    pub name: &'static str,
    /// The class id as the four 32-bit words of `uid(a, b, c, d)`.
    pub cid: [u32; 4],
    /// The controller's class id when the plugin is split into component and controller.
    pub controller_cid: Option<[u32; 4]>,
    /// VST3 sub categories (`Fx`, `Instrument|Synth`, ...).
    pub sub_categories: &'static str,
    /// The editor is offered (a headless fake view).
    pub gui: bool,
    /// The controller maps pitch bend, CC 1, CC 123 and channel pressure to hidden params
    /// (`IMidiMapping`) and offers note expressions.
    pub midi: bool,
}

/// One parameter of a plugin, in the plain values of the DSP.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    /// Id.
    pub id: u32,
    /// Name.
    pub name: String,
    /// Module (group) path, `/` separated.
    pub module: String,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// Default.
    pub default: f64,
    /// Stepped: the plain range is a whole number of steps.
    pub stepped: bool,
}

/// What `process` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Fine.
    Continue,
    /// Fail.
    Error,
}

/// One 64-frame block.
pub struct Io<'a> {
    /// Events in order (params first at an equal offset).
    pub events: &'a [TimedEvent],
    /// Output events (capacity-limited; never grows).
    pub out_events: &'a mut Vec<TimedEvent>,
    /// Main input (a mono input is copied to both).
    pub input: [Block; 2],
    /// The host flagged the main input silent.
    pub input_constant: bool,
    /// The sidechain, when the plugin has one.
    pub sidechain: Option<[Block; 2]>,
    /// The host flagged the sidechain silent.
    pub sidechain_constant: bool,
    /// Main output.
    pub output: [Block; 2],
}

/// Ways a test plugin behaves like a strict real one. All off by default: a plugin that asks for
/// none of them behaves as before.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Quirks {
    /// A split plugin that lets go of its peer in `disconnect(other)` only when `other` is the
    /// peer it stored (it answers `kResultFalse` otherwise) and does not let go of it in
    /// `terminate`: the plugin whose component and controller keep each other alive through the
    /// host's proxies until the host disconnects them properly.
    pub strict_peer: bool,
    /// The plugin sends a message to its peer from inside `disconnect` and `terminate`.
    pub chatty_teardown: bool,
    /// The component initialises fine and is not an audio processor.
    pub not_a_processor: bool,
    /// The separate controller's `initialize` returns an error.
    pub controller_refuses: bool,
    /// The factory answers the controller class with the component's own object.
    pub controller_is_component: bool,
}

/// The DSP and state of one plugin.
pub trait Dsp: Send {
    /// How strictly the plugin behaves towards the host (see [`Quirks`]).
    fn quirks(&self) -> Quirks {
        Quirks::default()
    }
    /// The buses.
    fn ports(&self) -> Ports;
    /// The parameter table now.
    fn params(&self) -> Vec<ParamSpec>;
    /// `initialize`; false fails it.
    fn init(&mut self) -> bool {
        true
    }
    /// `setupProcessing`; false fails it.
    fn activate(&mut self, _sample_rate: f64) -> bool {
        true
    }
    /// `setProcessing(true)`; false fails it.
    fn start_processing(&mut self) -> bool {
        true
    }
    /// Sets a param (plain value).
    fn set_param(&mut self, id: u32, value: f64);
    /// A param value (plain).
    fn param(&self, id: u32) -> f64;
    /// Latency in frames.
    fn latency(&self) -> u32 {
        0
    }
    /// Tail in frames.
    fn tail(&self) -> u32 {
        0
    }
    /// Saves the state.
    fn save(&self) -> Vec<u8> {
        Vec::new()
    }
    /// Loads a state; false fails.
    fn load(&mut self, _bytes: &[u8]) -> bool {
        true
    }
    /// Resets the audio state.
    fn reset(&mut self) {}
    /// `setProcessing(false)`: the host stopped the audio side (it also does so before sleeping
    /// an idle plugin). Defaults to [`Dsp::reset`]; a plugin that keeps its controller state
    /// across a stop (as real instruments do) overrides it.
    fn stop_processing(&mut self) {
        self.reset();
    }
    /// Processes one block.
    fn process(&mut self, io: &mut Io<'_>) -> Status;
    /// The `restartComponent` flags the plugin announces (read and cleared): asked right after
    /// `setupProcessing`, from inside the activation, and after every processed block. 0 for none.
    fn host_flags(&mut self) -> i32 {
        0
    }
    /// Makes the run-time param appear (the param bank); true when the table changed.
    fn grow(&mut self) -> bool {
        false
    }
    /// The parameter the controller assigns to controller `number` on `channel` of the first
    /// event bus, besides the shell's own mapping (a plugin that stands each controller and
    /// channel for a parameter of its own).
    fn midi_assignment(&self, _channel: i16, _number: i16) -> Option<u32> {
        None
    }
    /// How long the controller takes to answer each `IMidiMapping` call (a slow plugin).
    fn midi_delay(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

/// A stereo effect with no params that copies its input (the plugin of the isolation test
/// libraries).
#[derive(Debug, Default)]
pub struct Passthrough;

impl Dsp for Passthrough {
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

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// One plugin of a library.
pub struct PluginDef {
    /// Static facts.
    pub info: &'static Info,
    /// Makes the DSP.
    pub create: fn() -> Box<dyn Dsp>,
}

/// The plugins of a library and its entry behaviour.
pub struct LibraryDef {
    /// The plugins.
    pub plugins: &'static [PluginDef],
    /// Module entry; false fails loading the library.
    pub init: fn() -> bool,
    /// The factory answers `setHostContext` with an error instead of keeping the context.
    pub refuse_host_context: bool,
}

impl LibraryDef {
    /// A library of `plugins` whose entry is `init`.
    pub const fn new(plugins: &'static [PluginDef], init: fn() -> bool) -> Self {
        Self {
            plugins,
            init,
            refuse_host_context: false,
        }
    }

    /// The same library whose factory refuses the host context (the call is advisory: the host
    /// must carry on).
    #[must_use]
    pub const fn refusing_host_context(mut self) -> Self {
        self.refuse_host_context = true;
        self
    }
}
