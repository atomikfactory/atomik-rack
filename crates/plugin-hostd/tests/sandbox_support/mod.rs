//! Helpers of the end-to-end suites that join the parent (`SandboxBackend`) and the child: a
//! manager over a sandbox backend (or over the in-process backend, for the reference), the test
//! plugins' ids and files, and a guard that checks no child is left behind.
#![allow(dead_code)]
// Justification: each test binary uses a different part of the helpers.
// Justification: the helpers wait for child processes against the wall clock; nothing here is
// in a signal path.
#![allow(clippy::disallowed_methods)]
// Justification: meter states and copied samples are exact values.
#![allow(clippy::float_cmp)]

use atomik_core::Batch;
use atomik_device_sdk::{PluginFormat, PluginRef, PluginStateBlob, RunMode};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin};
use atomik_plugin_host::{
    BackendError, DeviceState, HostHandle, InstantiateRequest, PluginBackend, PluginHandle,
    PluginInstance, PluginManager, ScannedPlugin, mark_main_thread,
};
use atomik_plugin_sandbox::{HostCommand, SandboxBackend, SandboxConfig};
use atomik_plugin_vst3::Vst3Backend;
use atomik_test_plugin_core as core;
use atomik_testkit::harness::DeviceHarness;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The sample rate of the suites.
pub const SR: f64 = 48_000.0;

/// The two formats the suites run against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Clap,
    Vst3,
}

pub const FORMATS: [Format; 2] = [Format::Clap, Format::Vst3];

/// The logical test plugins both formats have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plug {
    Gain,
    Synth,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
        }
    }

    pub fn plugin_format(self) -> PluginFormat {
        match self {
            Format::Clap => PluginFormat::Clap,
            Format::Vst3 => PluginFormat::Vst3,
        }
    }

    /// Builds and installs a test library of this format.
    pub fn install(self, package: &str) -> PathBuf {
        match self {
            Format::Clap => clap_test_plugin(package).expect("the test plugin builds"),
            Format::Vst3 => vst3_test_plugin(package).expect("the test plugin builds"),
        }
    }

    /// The main test library.
    pub fn main_file(self) -> PathBuf {
        self.install(match self {
            Format::Clap => "atomik-test-clap",
            Format::Vst3 => "atomik-test-vst3",
        })
    }

    pub fn plugin(self, plug: Plug) -> PluginRef {
        let n = match plug {
            Plug::Gain => 1,
            Plug::Synth => 2,
        };
        let id = match (self, plug) {
            (Format::Clap, Plug::Gain) => "atomik.test.gain".to_string(),
            (Format::Clap, Plug::Synth) => "atomik.test.synth".to_string(),
            (Format::Vst3, _) => hex([0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, n]),
        };
        PluginRef {
            format: self.plugin_format(),
            id,
        }
    }

    /// The crash test plugin of this format and its library.
    pub fn crash_plugin(self) -> (PathBuf, PluginRef) {
        match self {
            Format::Clap => (
                self.install("atomik-test-clap-crash"),
                PluginRef {
                    format: PluginFormat::Clap,
                    id: "atomik.test.crash".to_string(),
                },
            ),
            Format::Vst3 => (
                self.install("atomik-test-vst3-crash"),
                PluginRef {
                    format: PluginFormat::Vst3,
                    id: hex([0xA70F_1C06, 0x4A5B_4C01, 0x8D2E_1F00, 1]),
                },
            ),
        }
    }

    /// The value to give `Device::set_param` so the plugin ends up with the plain gain `db`.
    pub fn gain_value(self, db: f64) -> f32 {
        match self {
            Format::Clap => db as f32,
            Format::Vst3 => {
                let n = core::normalized_from_plain(-64.0, 64.0, db) as f32 as f64;
                n as f32
            }
        }
    }
}

fn hex(words: [u32; 4]) -> String {
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        words[0], words[1], words[2], words[3]
    )
}

/// The state blob a format's gain plugin saves for a latency of `frames` (CLAP saves the DSP state
/// as it is, VST3 in the `AVS1` container with an empty controller section).
pub fn gain_state_with_latency(format: Format, frames: u32) -> PluginStateBlob {
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::LATENCY, f64::from(frames));
    let bytes = fx.save_state();
    match format {
        Format::Clap => PluginStateBlob(bytes),
        Format::Vst3 => {
            let mut out = b"AVS1".to_vec();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
            out.extend_from_slice(&0u32.to_le_bytes());
            PluginStateBlob(out)
        }
    }
}

/// A null plugin of the child's built-in format.
pub fn null_plugin(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

/// Every null plugin the child knows.
pub const NULL_IDS: [&str; 7] = [
    "null.gain",
    "null.latency",
    "null.crash",
    "null.hang",
    "null.hang-activate",
    "null.synth",
    "null.noisy",
];

/// The command of the child under test.
pub fn host_command() -> HostCommand {
    HostCommand::new(env!("CARGO_BIN_EXE_atomik-plugin-host"))
}

/// The configuration with the child under test.
pub fn config() -> SandboxConfig {
    SandboxConfig::new(host_command())
}

/// A configuration whose child gets an extra environment variable.
pub fn config_with_env(key: &str, value: &str) -> SandboxConfig {
    let mut c = config();
    c.command
        .envs
        .push((OsString::from(key), OsString::from(value)));
    c
}

/// A backend the manager owns and the test can still ask for the live children.
struct Shared(Arc<SandboxBackend>);

impl PluginBackend for Shared {
    fn format(&self) -> PluginFormat {
        self.0.format()
    }
    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        self.0.scan_file(path)
    }
    fn instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        self.0.instantiate(plugin, host)
    }
}

/// A plugin manager over one sandbox backend.
pub struct Sandbox {
    pub manager: PluginManager,
    pub backend: Arc<SandboxBackend>,
}

impl Sandbox {
    /// A sandbox for `format` with `config`; `files` say which file holds which plugins.
    pub fn new(
        format: PluginFormat,
        config: SandboxConfig,
        files: &[(PathBuf, Vec<PluginRef>)],
    ) -> Self {
        mark_main_thread();
        let backend = Arc::new(SandboxBackend::new(format, config));
        for (path, plugins) in files {
            backend.register_plugin_file(path, plugins);
        }
        let mut manager = PluginManager::new();
        manager.add_backend(Box::new(Shared(Arc::clone(&backend))));
        Self { manager, backend }
    }

    /// A sandbox over the null plugins (the plugin file is not read): the child is told to host
    /// the built-in format whatever the sandbox names.
    pub fn null(mut config: SandboxConfig) -> Self {
        config.command.envs.push((
            OsString::from("ATOMIK_PLUGIN_HOST_FORMAT"),
            OsString::from("null"),
        ));
        let plugins: Vec<PluginRef> = NULL_IDS.iter().map(|id| null_plugin(id)).collect();
        Self::new(PluginFormat::Clap, config, &[(PathBuf::from("-"), plugins)])
    }

    /// A sandbox over the main test library of `format`.
    pub fn real(format: Format) -> Self {
        Self::real_with(format, config())
    }

    /// As [`Sandbox::real`] with a configuration.
    pub fn real_with(format: Format, config: SandboxConfig) -> Self {
        let file = format.main_file();
        let plugins = vec![format.plugin(Plug::Gain), format.plugin(Plug::Synth)];
        Self::new(format.plugin_format(), config, &[(file, plugins)])
    }

    /// Waits until `n` children are alive (they end asynchronously).
    pub fn wait_children(&self, n: usize, within: Duration) -> bool {
        let limit = std::time::Instant::now() + within;
        while self.backend.live_children() != n {
            if std::time::Instant::now() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// Ends the test: the manager goes, and no child may be left within five seconds.
    pub fn finish(self) {
        let Sandbox { manager, backend } = self;
        drop(manager);
        let limit = std::time::Instant::now() + Duration::from_secs(5);
        while backend.live_children() != 0 {
            assert!(
                std::time::Instant::now() < limit,
                "{} children are left behind",
                backend.live_children()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A manager over the in-process backend of `format` that knows the plugins of `paths`.
pub fn in_process(format: Format, paths: &[PathBuf]) -> PluginManager {
    mark_main_thread();
    let backend: Box<dyn PluginBackend> = match format {
        Format::Clap => Box::new(ClapBackend::new()),
        Format::Vst3 => Box::new(Vst3Backend::new()),
    };
    for p in paths {
        backend.scan_file(p).expect("the library scans");
    }
    let mut m = PluginManager::new();
    m.add_backend(backend);
    m
}

/// A request for `plugin` at [`SR`].
pub fn request(plugin: &PluginRef, mode: RunMode) -> InstantiateRequest {
    InstantiateRequest {
        plugin: plugin.clone(),
        state: None,
        sample_rate: SR,
        mode,
        pinned: false,
        powered: true,
    }
}

/// As [`request`], restoring `state` first.
pub fn request_with_state(
    plugin: &PluginRef,
    mode: RunMode,
    state: PluginStateBlob,
) -> InstantiateRequest {
    InstantiateRequest {
        state: Some(state),
        ..request(plugin, mode)
    }
}

// ----- driving a device -----

/// The meter slots of a plugin device.
pub const METER_STATE: usize = 2;
pub const METER_LATE: usize = 5;

pub fn meters(h: &DeviceHarness) -> [f32; 6] {
    let mut m = [0.0f32; 6];
    h.device().meter_values(&mut m);
    m
}

/// A harness over a plugin device with a constant stereo input.
pub fn effect(handle: PluginHandle, level: f32) -> DeviceHarness {
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    h.input_mut("in_l").fill(level);
    h.input_mut("in_r").fill(level);
    h
}

pub fn out_l(h: &DeviceHarness) -> Batch {
    *h.output("out_l")
}

pub fn bits(b: &Batch) -> Vec<u32> {
    b.iter().map(|s| s.to_bits()).collect()
}

/// Processes until the device is Crashed (at most `limit` batches); returns how many batches it
/// took, and checks that every batch from the first late one on was the dry input.
pub fn run_to_crash(h: &mut DeviceHarness, level: f32, limit: usize) -> usize {
    for n in 1..=limit {
        h.process();
        if meters(h)[METER_STATE] == DeviceState::Crashed.as_f32() {
            assert!(
                out_l(h).iter().all(|s| *s == level),
                "the batch that found the crash passes the dry input"
            );
            return n;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the device did not turn Crashed in {limit} batches");
}
