//! Shared helpers of the VST3 backend tests and of the suites that run against both formats:
//! a `Kit` (one format's backend and test libraries) with the value mapping between the CLAP
//! plain values the tests think in and the host values each format takes, the test
//! hooks the test plugin libraries export, and rigs that put an instance behind a
//! `PluginDevice`.
#![allow(dead_code)] // Justification: each test binary uses a different subset of the helpers.

// The reference DSP devices and the host helpers of the plugin-host tests are reused as they are.
#[path = "../../../plugin-host/tests/support/mod.rs"]
pub mod host_support;

use atomik_core::BATCH;
use atomik_device_sdk::{
    DeviceDescriptor, PluginFormat, PluginLayout, PluginRef, RunMode, descriptor_for,
};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::paths::Os;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_bundle_binary, vst3_test_plugin};
use atomik_plugin_host::{
    ActivateConfig, HostHandle, InstanceId, ManagerEnd, PluginBackend, PluginDevice,
    PluginInstance, PluginProcessor, ReaperQueue, mark_main_thread, plugin_channel,
};
use atomik_plugin_vst3::Vst3Backend;
use atomik_test_plugin_core as core;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub const SR: f64 = 48_000.0;

/// The probe indexes of the test plugin libraries (the first 32 are shared by both formats).
pub mod probe {
    pub const PROCESS_CALLS: u32 = 0;
    pub const MASK_MAIN: u32 = 3;
    pub const MASK_SIDECHAIN: u32 = 4;
    pub const TRANSPORT_FLAGS: u32 = 5;
    pub const TEMPO: u32 = 6;
    pub const BEATS: u32 = 7;
    pub const SECONDS: u32 = 8;
    pub const STEADY: u32 = 9;
    pub const TSIG_NUM: u32 = 10;
    pub const TSIG_DEN: u32 = 11;
    pub const LOOP_START: u32 = 12;
    pub const LOOP_END: u32 = 13;
    pub const FRAMES: u32 = 14;
    pub const GUI_CALLS: u32 = 16;
    pub const GUI_PARENT: u32 = 17;
    pub const GUI_SCALE: u32 = 18;
    pub const GUI_WIDTH: u32 = 19;
    pub const GUI_HEIGHT: u32 = 20;
    pub const STATE_SAVES: u32 = 21;
    pub const STATE_LOADS: u32 = 22;
    pub const RENDER_MODE: u32 = 23;
    pub const MAX_FRAMES: u32 = 25;
    pub const SAMPLE_RATE: u32 = 26;
    pub const EVENTS_IN: u32 = 27;
    pub const ACTIVATIONS: u32 = 30;
    pub const DEACTIVATIONS: u32 = 31;
    // CLAP only.
    pub const AUDIO_THREAD_TRUE: u32 = 1;
    pub const MAIN_THREAD_TRUE: u32 = 2;
    // VST3 only.
    pub const ARRANGEMENT_CALLS: u32 = 32;
    pub const CONNECTS: u32 = 33;
    pub const MESSAGES: u32 = 34;
    pub const CTRL_STATE_SAVES: u32 = 35;
    pub const CTRL_STATE_LOADS: u32 = 36;
    pub const CTRL_COMPONENT_STATES: u32 = 37;
    pub const PARAM_POINTS: u32 = 38;
    pub const LAST_POINT_OFFSET: u32 = 39;
    pub const BUS_ACTIVE: u32 = 40;
    pub const VIEWS_CREATED: u32 = 41;
    pub const LAST_NOTE_ID: u32 = 42;
    pub const PROCESSING_STARTS: u32 = 43;
    pub const PROCESSING_STOPS: u32 = 44;
    pub const CTRL_SET_PARAMS: u32 = 45;
    pub const LAST_SET_ID: u32 = 46;
    pub const LAST_SET_VALUE: u32 = 47;
}

/// Commands of the VST3 test library's `atomik_test_command`.
pub mod command {
    pub const GUI_REQUEST_RESIZE: u32 = 1;
    pub const LATENCY_CHANGED: u32 = 2;
    pub const MARK_DIRTY: u32 = 4;
    pub const REQUEST_RESTART: u32 = 5;
    pub const GUI_EDIT: u32 = 12;
    pub const RESCAN_PARAMS: u32 = 13;
    pub const MIDI_MAPPING_CHANGED: u32 = 14;
    pub const PARAM_VALUES_CHANGED: u32 = 15;
    pub const IO_CHANGED: u32 = 16;
}

/// The process-wide counters of a test library.
pub mod counter {
    pub const CREATED: u32 = 0;
    pub const DESTROYED: u32 = 1;
    pub const ACTIVATED: u32 = 2;
    pub const DEACTIVATED: u32 = 3;
    pub const ENTRY_INIT: u32 = 4;
    pub const ENTRY_DEINIT: u32 = 5;
}

// SAFETY: these are the signatures of the exported test hooks.
type ProbeFn = unsafe extern "C" fn(usize, u32) -> u64;
type CommandFn = unsafe extern "C" fn(usize, u32, u64, u64) -> u64;
type CounterFn = unsafe extern "C" fn(u32) -> u64;

/// A test plugin library loaded a second time by the test, to read its probes.
pub struct TestLib {
    lib: libloading::Library,
    pub path: PathBuf,
}

impl TestLib {
    /// Opens the library behind the plugin file or bundle at `path`.
    pub fn open_path(path: &std::path::Path) -> Self {
        let binary = if path.is_dir() {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            path.join(
                vst3_bundle_binary(Os::current(), std::env::consts::ARCH, &stem).expect("layout"),
            )
        } else {
            path.to_path_buf()
        };
        // SAFETY: loading a library built from this repository's test plugin sources.
        let lib = unsafe { libloading::Library::new(&binary) }.expect("loads");
        Self {
            lib,
            path: path.to_path_buf(),
        }
    }

    pub fn probe(&self, instance: &dyn PluginInstance, index: u32) -> u64 {
        self.probe_raw(instance.native_handle(), index)
    }

    pub fn probe_f64(&self, instance: &dyn PluginInstance, index: u32) -> f64 {
        f64::from_bits(self.probe(instance, index))
    }

    pub fn command(&self, instance: &dyn PluginInstance, cmd: u32, a: u64, b: u64) -> u64 {
        self.command_raw(instance.native_handle(), cmd, a, b)
    }

    pub fn probe_raw(&self, handle: usize, index: u32) -> u64 {
        // SAFETY: the symbol is exported by every test plugin library with this signature.
        let f: libloading::Symbol<'_, ProbeFn> =
            unsafe { self.lib.get(b"atomik_test_probe\0") }.expect("probe symbol");
        // SAFETY: the handle is a live instance's native pointer.
        unsafe { f(handle, index) }
    }

    pub fn command_raw(&self, handle: usize, cmd: u32, a: u64, b: u64) -> u64 {
        // SAFETY: the symbol is exported by every test plugin library with this signature.
        let f: libloading::Symbol<'_, CommandFn> =
            unsafe { self.lib.get(b"atomik_test_command\0") }.expect("command symbol");
        // SAFETY: the handle is a live instance's native pointer.
        unsafe { f(handle, cmd, a, b) }
    }

    pub fn counter(&self, index: u32) -> u64 {
        // SAFETY: the symbol is exported by every test plugin library with this signature.
        let f: libloading::Symbol<'_, CounterFn> =
            unsafe { self.lib.get(b"atomik_test_counter\0") }.expect("counter symbol");
        // SAFETY: a pure read of a process-wide counter.
        unsafe { f(index) }
    }
}

// ----- formats and kits ---------------------------------------------------------------------

/// The two formats the shared suites run against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Clap,
    Vst3,
}

pub const FORMATS: [Format; 2] = [Format::Clap, Format::Vst3];

/// The logical test plugins both formats have (the main library).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plug {
    Gain,
    Synth,
    Shift,
    Params,
}

/// The class id words of the VST3 test plugins (`atomik_test_vst3::*_CID`).
fn vst3_hex(words: [u32; 4]) -> String {
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        words[0], words[1], words[2], words[3]
    )
}

/// The VST3 class id of a main-library test plugin.
pub fn vst3_id(plug: Plug) -> String {
    let last = match plug {
        Plug::Gain => 1,
        Plug::Synth => 2,
        Plug::Shift => 3,
        Plug::Params => 4,
    };
    vst3_hex([0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, last])
}

/// The VST3 class id of a variants-library plugin (`atomik_test_vst3_variants::cid`).
pub fn variant_id(n: u32) -> String {
    vst3_hex([0xA70F_1C02, 0x4A5B_4C01, 0x8D2E_1F00, n])
}

pub const V_SPLIT: u32 = 1;
pub const V_MONO: u32 = 2;
pub const V_SURROUND: u32 = 3;
pub const V_INIT: u32 = 4;
pub const V_ACTIVATE: u32 = 5;
pub const V_START: u32 = 6;
pub const V_ERROR: u32 = 7;
pub const V_NAN: u32 = 8;
pub const V_PICKY: u32 = 9;

/// The edge-case plugins both formats have in their variants library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Mono,
    Error,
    Nan,
    StartFails,
    ActivateFails,
}

impl Variant {
    pub fn id(self, format: Format) -> String {
        match (format, self) {
            (Format::Clap, Variant::Mono) => "atomik.test.mono".to_string(),
            (Format::Clap, Variant::Error) => "atomik.test.error".to_string(),
            (Format::Clap, Variant::Nan) => "atomik.test.nan".to_string(),
            (Format::Clap, Variant::StartFails) => "atomik.test.start-fails".to_string(),
            (Format::Clap, Variant::ActivateFails) => "atomik.test.activate-fails".to_string(),
            (Format::Vst3, Variant::Mono) => variant_id(V_MONO),
            (Format::Vst3, Variant::Error) => variant_id(V_ERROR),
            (Format::Vst3, Variant::Nan) => variant_id(V_NAN),
            (Format::Vst3, Variant::StartFails) => variant_id(V_START),
            (Format::Vst3, Variant::ActivateFails) => variant_id(V_ACTIVATE),
        }
    }
}

impl Kit {
    pub fn variant(&self, v: Variant) -> PluginRef {
        PluginRef {
            format: match self.format {
                Format::Clap => PluginFormat::Clap,
                Format::Vst3 => PluginFormat::Vst3,
            },
            id: v.id(self.format),
        }
    }
}

impl Kit {
    /// The split plugin of the VST3 variants library.
    pub fn variant_of_split(&self) -> PluginRef {
        vst3_ref(&variant_id(V_SPLIT))
    }
}

pub fn clap_ref(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

pub fn vst3_ref(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Vst3,
        id: id.to_string(),
    }
}

/// One format's backend, loaded with the main test library.
pub struct Kit {
    pub format: Format,
    pub backend: Box<dyn PluginBackend>,
    pub path: PathBuf,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
        }
    }

    /// The package of the main test library.
    pub fn package(self) -> &'static str {
        match self {
            Format::Clap => "atomik-test-clap",
            Format::Vst3 => "atomik-test-vst3",
        }
    }

    /// Builds and installs a test library of this format (`clap` or `vst3` suffix swapped in).
    pub fn install(self, package: &str) -> PathBuf {
        match self {
            Format::Clap => clap_test_plugin(package).expect("test plugin builds"),
            Format::Vst3 => vst3_test_plugin(package).expect("test plugin builds"),
        }
    }

    pub fn new_backend(self) -> Box<dyn PluginBackend> {
        match self {
            Format::Clap => Box::new(ClapBackend::new()),
            Format::Vst3 => Box::new(Vst3Backend::new()),
        }
    }
}

/// The scanned backend of the main library of `format`.
pub fn kit(format: Format) -> Kit {
    kit_of(format, format.package())
}

/// The scanned backend of the variants library of `format` (the edge-case plugins).
pub fn kit_variants(format: Format) -> Kit {
    kit_of(
        format,
        match format {
            Format::Clap => "atomik-test-clap-variants",
            Format::Vst3 => "atomik-test-vst3-variants",
        },
    )
}

/// The scanned backend of the library of `package`.
pub fn kit_of(format: Format, package: &str) -> Kit {
    mark_main_thread();
    let path = format.install(package);
    let backend = format.new_backend();
    backend.scan_file(&path).expect("the library scans");
    Kit {
        format,
        backend,
        path,
    }
}

/// The plain range of a test plugin param: `(min, max, stepped)` in the DSP's own values. The
/// VST3 gain spans -64 to 64 dB (every whole dB is then an exact normalised value).
pub fn range(format: Format, plug: Plug, id: u32) -> (f64, f64, bool) {
    match (plug, id) {
        (Plug::Gain, core::GainFx::GAIN) if format == Format::Vst3 => (-64.0, 64.0, false),
        (Plug::Gain, core::GainFx::GAIN) => (-60.0, 12.0, false),
        (Plug::Gain, core::GainFx::MUTE) => (0.0, 1.0, true),
        (Plug::Gain, core::GainFx::LATENCY) => (0.0, 512.0, true),
        (Plug::Gain, core::GainFx::DUCK) => (0.0, 1.0, false),
        (Plug::Synth, core::TestSynth::WAVE) => (0.0, 1.0, true),
        (Plug::Synth, core::TestSynth::LEVEL) => (0.0, 1.0, false),
        (Plug::Shift, core::NoteShift::SEMITONES) => (-24.0, 24.0, true),
        (Plug::Params, _) => (0.0, 1.0, false),
        other => panic!("unknown param {other:?}"),
    }
}

impl Kit {
    pub fn id(&self, plug: Plug) -> String {
        match self.format {
            Format::Clap => match plug {
                Plug::Gain => "atomik.test.gain",
                Plug::Synth => "atomik.test.synth",
                Plug::Shift => "atomik.test.noteshift",
                Plug::Params => "atomik.test.params",
            }
            .to_string(),
            Format::Vst3 => vst3_id(plug),
        }
    }

    pub fn plugin(&self, plug: Plug) -> PluginRef {
        PluginRef {
            format: match self.format {
                Format::Clap => PluginFormat::Clap,
                Format::Vst3 => PluginFormat::Vst3,
            },
            id: self.id(plug),
        }
    }

    /// The value to give `Device::set_param` so the plugin's DSP ends up with `plain`, and the
    /// plain value its DSP then really holds (VST3 maps through the normalised range, so the two
    /// can differ in the last bit; the reference DSP is fed the second).
    pub fn host_value(&self, plug: Plug, id: u32, plain: f64) -> (f64, f64) {
        match self.format {
            Format::Clap => (plain, plain),
            Format::Vst3 => {
                let (min, max, stepped) = range(Format::Vst3, plug, id);
                if stepped {
                    let index = (plain - min).round();
                    (index, min + index)
                } else {
                    // The device stores params as f32, so the plugin sees the rounded value.
                    let n = core::normalized_from_plain(min, max, plain) as f32 as f64;
                    (n, core::plain_from_normalized(min, max, n))
                }
            }
        }
    }
}

/// The state blob a format's plugin would save for the DSP state `bytes`: CLAP saves them as
/// they are, VST3 as the `AVS1` container with the component section and an empty controller
/// section.
pub fn wrap_state(format: Format, bytes: &[u8]) -> atomik_device_sdk::PluginStateBlob {
    match format {
        Format::Clap => atomik_device_sdk::PluginStateBlob(bytes.to_vec()),
        Format::Vst3 => {
            let mut out = b"AVS1".to_vec();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(&0u32.to_le_bytes());
            atomik_device_sdk::PluginStateBlob(out)
        }
    }
}

/// The DSP state inside a saved blob: the blob itself for CLAP, the component section for VST3.
pub fn component_state(format: Format, blob: &atomik_device_sdk::PluginStateBlob) -> Vec<u8> {
    match format {
        Format::Clap => blob.0.clone(),
        Format::Vst3 => {
            let rest = blob
                .0
                .strip_prefix(b"AVS1".as_slice())
                .expect("an AVS1 blob");
            let (len, tail) = rest.split_first_chunk::<4>().expect("a length");
            tail[..u32::from_le_bytes(*len) as usize].to_vec()
        }
    }
}

pub fn activate_config(mode: RunMode) -> ActivateConfig {
    ActivateConfig {
        sample_rate: SR,
        frames: BATCH as u32,
        mode,
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// One plugin instance behind a `PluginDevice`, as the manager sets it up.
pub struct Rig {
    pub instance: Box<dyn PluginInstance>,
    pub device: Option<PluginDevice>,
    pub manager: ManagerEnd,
    pub reaper: ReaperQueue,
    pub descriptor: &'static DeviceDescriptor,
    pub layout: PluginLayout,
    pub plugin: PluginRef,
}

/// Instantiates `plugin` from `backend`, activates it at 48 kHz in `mode` and builds its device.
pub fn rig_for(backend: &dyn PluginBackend, plugin: &PluginRef, mode: RunMode) -> Rig {
    mark_main_thread();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let (device_end, manager) = plugin_channel(InstanceId(n));
    let reaper = ReaperQueue::new();
    let mut instance = backend
        .instantiate(plugin, HostHandle::new(manager.messenger()))
        .expect("instantiates");
    let layout = instance.layout().clone();
    let processor: Box<dyn PluginProcessor> = instance
        .activate(&activate_config(mode))
        .expect("activates");
    manager.slot().set_latency(instance.latency_frames(), SR);
    let descriptor = descriptor_for(plugin, &layout).expect("descriptor");
    let device = PluginDevice::new(descriptor, &layout, processor, device_end, SR)
        .with_reaper(reaper.clone());
    Rig {
        instance,
        device: Some(device),
        manager,
        reaper,
        descriptor,
        layout,
        plugin: plugin.clone(),
    }
}

pub fn rig_with(kit: &Kit, plug: Plug, mode: RunMode) -> Rig {
    rig_for(&*kit.backend, &kit.plugin(plug), mode)
}

pub fn rig(kit: &Kit, plug: Plug) -> Rig {
    rig_with(kit, plug, RunMode::Realtime)
}

impl Rig {
    /// Moves the device into a test harness; the rest stays with the rig.
    pub fn harness(&mut self) -> atomik_testkit::harness::DeviceHarness {
        atomik_testkit::harness::DeviceHarness::new(Box::new(self.device.take().unwrap()), SR)
    }
}

/// Serialises the tests of one binary that read a library's process-wide counters.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A manager with the backend of `format` that knows the plugins of every path in `paths`.
pub fn manager_for(format: Format, paths: &[PathBuf]) -> atomik_plugin_host::PluginManager {
    mark_main_thread();
    let backend = format.new_backend();
    for p in paths {
        backend.scan_file(p).expect("the library scans");
    }
    let mut m = atomik_plugin_host::PluginManager::new();
    m.add_backend(backend);
    m
}

pub fn request(plugin: &PluginRef) -> atomik_plugin_host::InstantiateRequest {
    atomik_plugin_host::InstantiateRequest {
        plugin: plugin.clone(),
        state: None,
        sample_rate: SR,
        mode: RunMode::Realtime,
        pinned: false,
        powered: true,
    }
}

/// An instance that is created but not activated, with the manager end that keeps its message
/// ring alive.
pub fn bare(
    backend: &dyn PluginBackend,
    plugin: &PluginRef,
) -> (Box<dyn PluginInstance>, ManagerEnd) {
    mark_main_thread();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let (_device_end, manager) = plugin_channel(InstanceId(n));
    let instance = backend
        .instantiate(plugin, HostHandle::new(manager.messenger()))
        .expect("instantiates");
    (instance, manager)
}
