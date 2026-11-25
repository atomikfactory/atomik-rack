//! One shared, scriptable fake audio backend (feature `backend`): the audio side of the app's
//! `AudioBackend` and of the player's `Backend`, without a device.
//!
//! The app and the player each have a trait for "the outside world" and each implements it in
//! its tests with a thin adapter over [`FakeAudio`]: the adapter answers the trait's methods by
//! calling [`FakeAudio::list_audio`], [`FakeAudio::list_midi`] and [`FakeAudio::start`] (MIDI
//! input ports carry app types, so opening one stays in the adapter). Everything a test wants to
//! script or count is here, once.
//!
//! **Time is simulated.** Nothing in this module sleeps or reads a clock. A stream's callbacks
//! happen when the test says so ([`FakeAudio::run_callbacks`], [`FakeAudio::advance`]), or a
//! fixed number of them on every telemetry read ([`FakeAudio::callbacks_per_read`]). Each
//! callback "takes" [`FakeAudio::callback_cost_ns`] of render time against its period
//! (`buffer / rate`), so the load figures and the overrun counters follow without a delay. The
//! code under test that reads wall time (the stream liveness check) takes the time as an
//! argument (`LiveSession::poll_at`), so a test passes simulated instants.
//!
//! The telemetry a stream reports is built the way the real one is: counters are totals, the
//! load figures cover the callbacks since the previous read, a stream that lost its device
//! reports it and stays lost until it is rebuilt. [`FakeAudio::set_telemetry`] pins the figures
//! instead (the stream then reports exactly that value on every read), for a test that only
//! needs one number.

use atomik_audio_io::{
    DeviceInfo, HostInfo, HostKind, IoError, StreamInfo, StreamSpec, Telemetry, XrunEvent,
};
use atomik_engine::Engine;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The device a stream opens when its spec names none, unless [`FakeAudio::set_default_output`].
pub const DEFAULT_OUTPUT: &str = "Speakers";

/// What the host says about the fixed buffer size a start asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcceptedBuffer {
    /// The host refuses the fixed size and runs its default period (`requested_buffer_frames`
    /// is `None`).
    #[default]
    Refused,
    /// The host accepts the size the start asked for.
    AsRequested,
    /// The host accepts this size whatever was asked.
    Fixed(u32),
}

/// What one start looked like, for tests that check what the session asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRecord {
    /// The spec of the call.
    pub spec: StreamSpec,
    /// The input device name of the call.
    pub input_device: Option<String>,
    /// How many render workers the engine given to the stream has.
    pub workers: usize,
    /// The worker linger spins of that engine.
    pub linger: u64,
}

/// Shapes a telemetry value after the fake built it: `(reads so far, telemetry)`.
type Shape = Box<dyn FnMut(u64, &mut Telemetry) + Send>;

/// The scriptable knobs, copied out of the state for a callback run.
#[derive(Clone, Copy, Default)]
struct Script {
    cost_ns: u64,
    peak_cost_ns: Option<u64>,
    stall_hold: bool,
    stall_ns: u64,
    buffer: Option<u32>,
    callbacks_per_read: u64,
    render_per_read: usize,
}

struct State {
    host: HostKind,
    devices: Vec<DeviceInfo>,
    no_host_without_devices: bool,
    midi_ports: Vec<String>,
    midi_hidden: Vec<String>,
    default_input: String,
    default_output: String,
    rate: Option<u32>,
    accepted: AcceptedBuffer,
    fail_next_start: Option<IoError>,
    fail_starts: Option<IoError>,
    fail_list: Option<IoError>,
    broken: Option<(String, Option<IoError>)>,
    fail_input: bool,
    host_inputs: Option<u16>,
    script: Script,
    pinned: Option<Telemetry>,
    shape: Option<Shape>,
    events: Vec<XrunEvent>,
    gone: Vec<(String, Option<DeviceInfo>)>,
    start_calls: Vec<StreamSpec>,
    starts: Vec<StartRecord>,
    stops: usize,
    stop_frames: Vec<u64>,
    lists: usize,
    midi_lists: usize,
    live: Vec<Arc<Core>>,
    peak_output: f32,
}

/// The counters of one stream, as the real callback would keep them.
#[derive(Default)]
struct Sim {
    callbacks: u64,
    xruns: u64,
    late: u64,
    overruns: u64,
    errors: u64,
    lost: bool,
    buffer: u32,
    load_sum: f64,
    load_n: u64,
    load_peak: f64,
    reads: u64,
    events: Vec<XrunEvent>,
}

/// What the stream and the script share.
struct Core {
    info: StreamInfo,
    requested_frames: u32,
    sim: Mutex<Sim>,
    engine: Mutex<Option<Engine>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The fake audio backend. Clones share one state: give one to the code under test and keep one
/// to script and to read the counts.
#[derive(Clone)]
pub struct FakeAudio(Arc<Mutex<State>>);

impl Default for FakeAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeAudio {
    /// A backend that lists no device, accepts any start and has no stream open.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            host: HostKind::Wasapi,
            devices: Vec::new(),
            no_host_without_devices: false,
            midi_ports: Vec::new(),
            midi_hidden: Vec::new(),
            default_input: "Default input".into(),
            default_output: DEFAULT_OUTPUT.into(),
            rate: None,
            accepted: AcceptedBuffer::Refused,
            fail_next_start: None,
            fail_starts: None,
            fail_list: None,
            broken: None,
            fail_input: false,
            host_inputs: None,
            script: Script::default(),
            pinned: None,
            shape: None,
            events: Vec::new(),
            gone: Vec::new(),
            start_calls: Vec::new(),
            starts: Vec::new(),
            stops: 0,
            stop_frames: Vec::new(),
            lists: 0,
            midi_lists: 0,
            live: Vec::new(),
            peak_output: 0.0,
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.0)
    }

    // ---- The world the code under test sees -------------------------------------------------

    /// The host the listing reports.
    pub fn set_host(&self, host: HostKind) {
        self.state().host = host;
    }

    /// With no device listed, the listing has no host at all (as a machine without audio).
    pub fn no_host_without_devices(&self) {
        self.state().no_host_without_devices = true;
    }

    /// Lists an output device (2 outputs, no input) named `name`.
    pub fn plug(&self, name: &str) {
        self.plug_device(DeviceInfo {
            name: name.to_owned(),
            max_inputs: 0,
            max_outputs: 2,
            sample_rates: vec![48_000],
            buffer_range: None,
            is_default_output: false,
        });
    }

    /// Lists a device as given. [`FakeAudio::set_rate`] overrides its `sample_rates`.
    pub fn plug_device(&self, device: DeviceInfo) {
        self.state().devices.push(device);
    }

    /// Lists exactly these output devices.
    pub fn set_plugged(&self, names: &[&str]) {
        self.state().devices.clear();
        for name in names {
            self.plug(name);
        }
    }

    /// The names of the devices the host lists right now.
    pub fn plugged(&self) -> Vec<String> {
        self.state()
            .devices
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }

    /// The name given to the input device when a start names none.
    pub fn set_default_input(&self, name: &str) {
        self.state().default_input = name.to_owned();
    }

    /// The name of the output device a start that names none opens (default `Speakers`).
    pub fn set_default_output(&self, name: &str) {
        self.state().default_output = name.to_owned();
    }

    /// The MIDI input ports of the machine, plugged in.
    pub fn set_midi_ports(&self, ports: &[&str]) {
        self.state().midi_ports = ports.iter().map(|p| (*p).to_owned()).collect();
    }

    /// Unplugs a MIDI port: it stays known, is not listed and cannot be opened.
    pub fn hide_midi_port(&self, port: &str) {
        self.state().midi_hidden.push(port.to_owned());
    }

    /// Plugs a hidden MIDI port in again.
    pub fn show_midi_port(&self, port: &str) {
        self.state().midi_hidden.retain(|p| p != port);
    }

    /// The MIDI ports a listing shows: every port but the unplugged ones.
    pub fn visible_midi(&self) -> Vec<String> {
        let state = self.state();
        state
            .midi_ports
            .iter()
            .filter(|p| !state.midi_hidden.contains(p))
            .cloned()
            .collect()
    }

    // ---- Failures -----------------------------------------------------------------------------

    /// The next start fails with `error`; the one after works.
    pub fn fail_next_start(&self, error: IoError) {
        self.state().fail_next_start = Some(error);
    }

    /// Every start fails with `error` until [`FakeAudio::start_works`].
    pub fn fail_starts(&self, error: IoError) {
        self.state().fail_starts = Some(error);
    }

    /// Starts work again.
    pub fn start_works(&self) {
        let mut state = self.state();
        state.fail_starts = None;
        state.broken = None;
    }

    /// Every start naming this output device fails (`"*"`: every start fails): the device
    /// rejects the configuration.
    pub fn break_device(&self, device: &str) {
        self.state().broken = Some((device.to_owned(), None));
    }

    /// [`FakeAudio::break_device`] with the error the start answers (default: an
    /// [`IoError::UnsupportedConfig`] naming the device).
    pub fn break_device_with(&self, device: &str, error: IoError) {
        self.state().broken = Some((device.to_owned(), Some(error)));
    }

    /// The audio listing fails with `error` (`None`: it works again).
    pub fn fail_list(&self, error: Option<IoError>) {
        self.state().fail_list = error;
    }

    /// Every start that asks for input channels fails (the output alone would open).
    pub fn fail_input(&self, fail: bool) {
        self.state().fail_input = fail;
    }

    /// The host opens at most this many input channels.
    pub fn limit_host_inputs(&self, channels: Option<u16>) {
        self.state().host_inputs = channels;
    }

    /// What the host does with the fixed buffer size a start asks for.
    pub fn accept_buffer(&self, accepted: AcceptedBuffer) {
        self.state().accepted = accepted;
    }

    // ---- The script ---------------------------------------------------------------------------

    /// The device's native sample rate changes to `hz`, as it does when the user switches the
    /// format in the system sound settings. A running stream is invalidated the way the real
    /// backend reports it: a stream error and a lost device ([`Telemetry::device_lost`]), sticky
    /// until the stream is rebuilt. From now on the listing offers only `hz` and a start at
    /// another rate fails with [`IoError::UnsupportedConfig`].
    pub fn set_rate(&self, hz: u32) {
        let live = {
            let mut state = self.state();
            state.rate = Some(hz);
            for device in &mut state.devices {
                device.sample_rates = vec![hz];
            }
            state.live.clone()
        };
        for core in live {
            core.lose();
        }
    }

    /// The host's callbacks now arrive in blocks of `frames` (0: no size is ever reported). The
    /// stream reports it from its next callback on, in [`Telemetry::buffer_frames`], and
    /// nothing else changes: that is how a driver changes its period under a running stream.
    pub fn set_buffer(&self, frames: u32) {
        self.state().script.buffer = Some(frames);
    }

    /// The output device of every open stream disappears: the stream reports a stream error and
    /// a lost device, the device leaves the listing and a start naming it fails with
    /// [`IoError::DeviceNotFound`], until [`FakeAudio::replug`]. An open stream stays lost.
    pub fn unplug(&self) {
        let names: Vec<String> = self
            .state()
            .live
            .iter()
            .map(|core| core.info.output_device.clone())
            .collect();
        for name in names {
            self.unplug_device(&name);
        }
    }

    /// [`FakeAudio::unplug`] for the device `name`, whether a stream uses it or not.
    pub fn unplug_device(&self, name: &str) {
        let live = {
            let mut state = self.state();
            let listed = state
                .devices
                .iter()
                .position(|d| d.name == name)
                .map(|i| state.devices.remove(i));
            if !state.gone.iter().any(|(n, _)| n == name) {
                state.gone.push((name.to_owned(), listed));
            }
            state.live.clone()
        };
        for core in live.iter().filter(|c| c.info.output_device == name) {
            core.lose();
        }
    }

    /// Every unplugged device comes back: listed again if it was listed, openable again. A
    /// stream that was open when it went stays lost.
    pub fn replug(&self) {
        let mut state = self.state();
        let gone = std::mem::take(&mut state.gone);
        for (_, listed) in gone {
            if let Some(device) = listed {
                state.devices.push(device);
            }
        }
    }

    /// The callbacks stop coming until [`FakeAudio::resume`]. The stream is not lost: it is
    /// what a driver that stops delivering buffers looks like.
    pub fn stall(&self) {
        self.state().script.stall_hold = true;
    }

    /// The callbacks come again.
    pub fn resume(&self) {
        let mut state = self.state();
        state.script.stall_hold = false;
        state.script.stall_ns = 0;
    }

    /// The callbacks that would arrive during the next `duration` of simulated time do not
    /// arrive (the time passes as the test runs callbacks or [`FakeAudio::advance`]s).
    pub fn stall_for(&self, duration: Duration) {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        self.state().script.stall_ns = ns;
    }

    /// The next `n` callbacks do not arrive.
    pub fn stall_callbacks(&self, n: u64) {
        let period = self.first_period_ns().unwrap_or(1);
        self.state().script.stall_ns = period.saturating_mul(n);
    }

    /// Each callback takes `ns` nanoseconds of render time (simulated: nothing waits). Against
    /// the period of the buffer (`frames / rate`) that is the load, and a load above 1 is a
    /// render overrun, counted as an xrun.
    pub fn callback_cost_ns(&self, ns: u64) {
        self.state().script.cost_ns = ns;
    }

    /// The dearest callback of every telemetry read takes `ns` (default: the callback cost).
    pub fn callback_peak_cost_ns(&self, ns: u64) {
        self.state().script.peak_cost_ns = Some(ns);
    }

    /// Every telemetry read of a stream first runs `n` callbacks (a stream that is polled at
    /// a steady pace).
    pub fn callbacks_per_read(&self, n: u64) {
        self.state().script.callbacks_per_read = n;
    }

    /// Every telemetry read also renders `buffers` buffers of the engine, as the device would,
    /// so notes sent live become audible; the loudest sample is [`FakeAudio::peak_output`].
    pub fn render_per_read(&self, buffers: usize) {
        self.state().script.render_per_read = buffers;
    }

    /// Pins the telemetry: every read of every stream returns exactly `telemetry` (the
    /// simulation is not consulted).
    pub fn set_telemetry(&self, telemetry: Telemetry) {
        self.state().pinned = Some(telemetry);
    }

    /// Edits what a stream reports after the fake built it: `shape(reads, &mut telemetry)`,
    /// `reads` counting the stream's reads from 1.
    pub fn shape_telemetry(&self, shape: impl FnMut(u64, &mut Telemetry) + Send + 'static) {
        self.state().shape = Some(Box::new(shape));
    }

    /// Xrun events every stream opened from now on hands out on its first drain.
    pub fn queue_events(&self, events: Vec<XrunEvent>) {
        self.state().events = events;
    }

    /// The open streams run `n` callbacks (subject to the stall, the unplugged device and the
    /// cost). Without a stream it does nothing.
    pub fn run_callbacks(&self, n: u64) {
        let (live, script) = {
            let state = self.state();
            (state.live.clone(), state.script)
        };
        let mut stall_left = script.stall_ns;
        for core in &live {
            stall_left = core.run(n, &script, script.stall_ns);
        }
        self.state().script.stall_ns = stall_left;
    }

    /// The open streams run the callbacks of `duration` of simulated time (whole periods).
    pub fn advance(&self, duration: Duration) {
        let Some(period) = self.first_period_ns() else {
            return;
        };
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        self.run_callbacks(ns / period.max(1));
    }

    fn first_period_ns(&self) -> Option<u64> {
        let state = self.state();
        let script = state.script;
        state.live.first().map(|core| core.period_ns(&script))
    }

    // ---- What the code under test did --------------------------------------------------------

    /// Every start call, failed or not, in order.
    pub fn start_calls(&self) -> Vec<StreamSpec> {
        self.state().start_calls.clone()
    }

    /// The starts that found their device: not refused by a scripted failure, a broken or an
    /// unplugged device or a rate the device does not run. A start whose input then failed is
    /// included (the output was opened first).
    pub fn starts(&self) -> Vec<StreamSpec> {
        self.state().starts.iter().map(|r| r.spec.clone()).collect()
    }

    /// [`FakeAudio::starts`] with the input device name and the engine's workers.
    pub fn start_records(&self) -> Vec<StartRecord> {
        self.state().starts.clone()
    }

    /// The input device name of every start in [`FakeAudio::starts`].
    pub fn input_devices(&self) -> Vec<Option<String>> {
        self.state()
            .starts
            .iter()
            .map(|r| r.input_device.clone())
            .collect()
    }

    /// How many streams were stopped (handed their engine back).
    pub fn stops(&self) -> usize {
        self.state().stops
    }

    /// The engine's host frame count at each stop.
    pub fn stop_frames(&self) -> Vec<u64> {
        self.state().stop_frames.clone()
    }

    /// How many streams are open now.
    pub fn open_streams(&self) -> usize {
        self.state().live.len()
    }

    /// How many times the audio devices were listed.
    pub fn lists(&self) -> usize {
        self.state().lists
    }

    /// How many times the MIDI input ports were listed.
    pub fn midi_lists(&self) -> usize {
        self.state().midi_lists
    }

    /// The loudest output sample rendered by [`FakeAudio::render_per_read`].
    pub fn peak_output(&self) -> f32 {
        self.state().peak_output
    }

    /// Runs `f` on the engine of the stream opened last and still open, as the device's
    /// callback would (the test renders blocks, for example). `None` without an open stream.
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        let core = self.state().live.last().cloned()?;
        let mut engine = lock(&core.engine);
        engine.as_mut().map(f)
    }

    // ---- The backend calls --------------------------------------------------------------------

    /// What `list_audio` answers.
    pub fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        let mut state = self.state();
        state.lists += 1;
        if let Some(error) = &state.fail_list {
            return Err(error.clone());
        }
        if state.devices.is_empty() && state.no_host_without_devices {
            return Ok(Vec::new());
        }
        Ok(vec![HostInfo {
            kind: state.host,
            devices: state.devices.clone(),
        }])
    }

    /// What `list_midi` answers: the ports that are plugged in.
    pub fn list_midi(&self) -> Vec<String> {
        self.state().midi_lists += 1;
        self.visible_midi()
    }

    /// What `start` does: records the call, applies the scripted failures and opens a stream
    /// that drives nothing by itself (the test runs its callbacks).
    pub fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
    ) -> Result<FakeStream, IoError> {
        let mut state = self.state();
        state.start_calls.push(spec.clone());
        if let Some(error) = state.fail_next_start.take() {
            return Err(error);
        }
        if let Some(error) = &state.fail_starts {
            return Err(error.clone());
        }
        let device = spec
            .device
            .clone()
            .unwrap_or_else(|| state.default_output.clone());
        if let Some((name, error)) = &state.broken
            && (name == "*" || spec.device.as_deref() == Some(name.as_str()))
        {
            return Err(error.clone().unwrap_or_else(|| {
                let device = spec.device.as_deref().unwrap_or("the default device");
                IoError::UnsupportedConfig(format!("{device} cannot be opened"))
            }));
        }
        if state.gone.iter().any(|(name, _)| *name == device) {
            return Err(IoError::DeviceNotFound(device));
        }
        if let Some(hz) = state.rate
            && hz != spec.sample_rate
        {
            return Err(IoError::UnsupportedConfig(format!(
                "{device} has no output configuration at {} Hz (it runs at {hz} Hz)",
                spec.sample_rate
            )));
        }
        state.starts.push(StartRecord {
            spec: spec.clone(),
            input_device: input_device.map(str::to_owned),
            workers: engine.worker_count(),
            linger: engine.worker_linger_spins(),
        });
        if spec.input_channels > 0 && state.fail_input {
            return Err(IoError::DeviceNotFound("no input device".into()));
        }
        let opened = state
            .host_inputs
            .map_or(spec.input_channels, |n| n.min(spec.input_channels));
        let info = StreamInfo {
            host: spec.host,
            output_device: device,
            input_device: (opened > 0)
                .then(|| input_device.unwrap_or(&state.default_input).to_owned()),
            sample_rate: spec.sample_rate,
            output_channels: spec.output_channels,
            input_channels: opened,
            sample_format: "f32",
            requested_buffer_frames: match state.accepted {
                AcceptedBuffer::Refused => None,
                AcceptedBuffer::AsRequested => Some(spec.buffer_frames),
                AcceptedBuffer::Fixed(n) => Some(n),
            },
        };
        let core = Arc::new(Core {
            info: info.clone(),
            requested_frames: spec.buffer_frames,
            sim: Mutex::new(Sim {
                events: state.events.clone(),
                ..Sim::default()
            }),
            engine: Mutex::new(Some(engine)),
        });
        state.live.push(Arc::clone(&core));
        Ok(FakeStream {
            core,
            audio: self.clone(),
        })
    }
}

impl Core {
    /// The stream's device is gone: one stream error, lost for good.
    fn lose(&self) {
        let mut sim = lock(&self.sim);
        if !sim.lost {
            sim.lost = true;
            sim.errors += 1;
            sim.xruns += 1;
        }
    }

    /// The period of one callback in nanoseconds, at the size the callbacks have (the size the
    /// start asked for until the script says another).
    fn period_ns(&self, script: &Script) -> u64 {
        let frames = script
            .buffer
            .filter(|b| *b > 0)
            .unwrap_or(self.requested_frames)
            .max(1);
        let rate = u64::from(self.info.sample_rate.max(1));
        (u64::from(frames) * 1_000_000_000 / rate).max(1)
    }

    /// Runs up to `n` callbacks; returns what is left of the stall (in nanoseconds).
    fn run(&self, n: u64, script: &Script, mut stall_ns: u64) -> u64 {
        let period = self.period_ns(script);
        let mut sim = lock(&self.sim);
        for _ in 0..n {
            if sim.lost {
                break;
            }
            if script.stall_hold || stall_ns > 0 {
                stall_ns = stall_ns.saturating_sub(period);
                continue;
            }
            sim.callbacks += 1;
            sim.buffer = script.buffer.unwrap_or(self.requested_frames);
            let load = script.cost_ns as f64 / period as f64;
            sim.load_sum += load;
            sim.load_n += 1;
            let peak = script
                .peak_cost_ns
                .map_or(load, |ns| ns as f64 / period as f64);
            sim.load_peak = sim.load_peak.max(peak.max(load));
            if load > 1.0 {
                sim.overruns += 1;
                sim.xruns += 1;
            }
        }
        stall_ns
    }
}

/// An open fake stream. The code under test owns it; the test scripts it through
/// [`FakeAudio`]. The traits of the app and of the player are implemented for it in their tests.
pub struct FakeStream {
    core: Arc<Core>,
    audio: FakeAudio,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        // A stream that is dropped instead of stopped is not open any more.
        let core = &self.core;
        self.audio.state().live.retain(|c| !Arc::ptr_eq(c, core));
    }
}

impl FakeStream {
    /// Telemetry since the previous read (loads reset on read; counters are totals).
    pub fn telemetry(&self) -> Telemetry {
        let (script, pinned) = {
            let state = self.audio.state();
            (state.script, state.pinned)
        };
        if let Some(t) = pinned {
            return t;
        }
        let reads = {
            let mut sim = lock(&self.core.sim);
            sim.reads += 1;
            sim.reads
        };
        if script.callbacks_per_read > 0 {
            let stall = script.stall_ns;
            let left = self.core.run(script.callbacks_per_read, &script, stall);
            self.audio.state().script.stall_ns = left;
        }
        if script.render_per_read > 0 {
            self.render(script.render_per_read, &script);
        }
        let mut t = {
            let mut sim = lock(&self.core.sim);
            let t = Telemetry {
                callbacks: sim.callbacks,
                xruns: sim.xruns,
                buffer_frames: sim.buffer,
                dsp_load_avg: if sim.load_n > 0 {
                    (sim.load_sum / sim.load_n as f64) as f32
                } else {
                    0.0
                },
                dsp_load_peak: sim.load_peak as f32,
                late_callbacks: sim.late,
                render_overruns: sim.overruns,
                stream_errors: sim.errors,
                device_lost: sim.lost,
                ..Telemetry::default()
            };
            sim.load_sum = 0.0;
            sim.load_n = 0;
            sim.load_peak = 0.0;
            t
        };
        if let Some(shape) = &mut self.audio.state().shape {
            shape(reads, &mut t);
        }
        t
    }

    fn render(&self, buffers: usize, script: &Script) {
        let frames = script
            .buffer
            .filter(|b| *b > 0)
            .unwrap_or(self.core.requested_frames) as usize;
        let mut l = vec![0.0f32; frames];
        let mut r = vec![0.0f32; frames];
        let mut peak = 0.0f32;
        if let Some(engine) = lock(&self.core.engine).as_mut() {
            for _ in 0..buffers {
                engine.render(&[], &mut [&mut l, &mut r]);
                peak = l.iter().chain(&r).fold(peak, |m, x| m.max(x.abs()));
            }
        }
        let mut state = self.audio.state();
        state.peak_output = state.peak_output.max(peak);
    }

    /// What was opened.
    pub fn info(&self) -> &StreamInfo {
        &self.core.info
    }

    /// Xrun events queued since the previous call, oldest first.
    pub fn drain_events(&self) -> Vec<XrunEvent> {
        std::mem::take(&mut lock(&self.core.sim).events)
    }

    /// Stops the stream and hands the engine back.
    pub fn stop(self) -> Result<Engine, IoError> {
        let engine = lock(&self.core.engine).take();
        let mut state = self.audio.state();
        state.live.retain(|c| !Arc::ptr_eq(c, &self.core));
        state.stops += 1;
        match engine {
            Some(engine) => {
                state.stop_frames.push(engine.host_frames());
                Ok(engine)
            }
            None => Err(IoError::Backend("the engine is gone".into())),
        }
    }
}
