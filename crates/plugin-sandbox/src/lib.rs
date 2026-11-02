//! The plugin sandbox, app side: a [`PluginBackend`] that runs every plugin instance in its own
//! `atomik-plugin-host` child process, so a plugin that crashes or hangs cannot take the app
//! down.
//!
//! * [`SandboxBackend`] starts one child per instance and talks to it over its stdin and stdout
//!   (main-thread calls, with timeouts) and a shared audio block (the engine-side processor).
//! * When a child ends, its instance is lost: the device turns CRASHED and the app replaces the
//!   instance on RELOAD.
//!
//! The contract is `not-for-git/plan/sandbox/03-interfaces.md`, section C.

mod clock;
mod instance;
mod link;
mod processor;
mod transport;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use crate::instance::RemoteInstance;
use crate::link::{LiveGuard, Timeouts};
use crate::transport::Transport;
use atomik_device_sdk::{PluginFormat, PluginRef};
use atomik_plugin_host::{
    BackendError, HostHandle, PendingInstance, PluginBackend, PluginInstance, ScannedPlugin,
};
use atomik_plugin_ipc::{
    Doorbell, DoorbellName, ParentPort, SharedBlock, block_file_name, default_block_dir,
    sweep_stale_blocks,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[cfg(test)]
atomik_testkit::install_alloc_guard!();

/// How to start the child: the `atomik-plugin-host` program next to the app, or the app itself
/// with `--plugin-host` in front of the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    /// The program.
    pub program: PathBuf,
    /// Arguments before the child's own arguments.
    pub args: Vec<OsString>,
    /// Environment variables set for the child.
    pub envs: Vec<(OsString, OsString)>,
}

impl HostCommand {
    /// A command that is just the program.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
        }
    }
}

/// Timeouts and the child command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    /// How to start a child.
    pub command: HostCommand,
    /// Instantiate, activate, capture and restore state, and open the editor.
    pub create_timeout: Duration,
    /// Every other main-thread call.
    pub call_timeout: Duration,
    /// The calls that only polish the interface (param text, pins, layout, closing the editor):
    /// a miss never stops the child, see [`SandboxConfig::new`].
    pub text_timeout: Duration,
    /// Offline (bounce) audio: how long a batch may take before the child counts as hung.
    pub offline_watchdog: Duration,
    /// Live audio: how long a child may answer no batch at all before it is stopped and the
    /// app reloads it. Zero turns this off.
    pub hung_after: Duration,
    /// The host is a bounce, not the live app: every batch waits for the child (up to the
    /// watchdog) and the plugin's notifications are flushed after each batch, whatever run
    /// mode the plugin is told (a `--plugin-mode realtime` bounce is still rendered offline).
    /// False (the live app): a realtime instance gets a one-batch deadline.
    pub bounce: bool,
}

impl SandboxConfig {
    /// The default timeouts (10 s, 2 s, 250 ms, 30 s, 3 s) for `command`, for the live app.
    ///
    /// A call that matters (create, activate, state, deactivate) that is not answered in time
    /// stops the child. A cosmetic one only drops its request: the child is marked slow after a
    /// few misses in a row and stopped after many.
    pub fn new(command: HostCommand) -> Self {
        Self {
            command,
            create_timeout: Duration::from_secs(10),
            call_timeout: Duration::from_secs(2),
            text_timeout: Duration::from_millis(250),
            offline_watchdog: Duration::from_secs(30),
            hung_after: Duration::from_secs(3),
            bounce: false,
        }
    }
}

/// The sandboxed backend of one plugin format.
#[derive(Debug)]
pub struct SandboxBackend {
    format: PluginFormat,
    config: SandboxConfig,
    /// Which file holds each plugin (fed from the scan cache; main thread only).
    files: Mutex<BTreeMap<PluginRef, PathBuf>>,
    /// Children alive now.
    live: Arc<AtomicUsize>,
}

/// How long a slow instance is left alone between two cosmetic calls.
const SLOW_RETRY: Duration = Duration::from_secs(1);

/// The number the next instance's block and doorbell carry: one counter for the whole process,
/// since the CLAP and the VST3 backend share the block folder and the doorbell namespace.
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

impl SandboxBackend {
    /// A backend for `format` that starts children with `config`.
    pub fn new(format: PluginFormat, config: SandboxConfig) -> Self {
        // Files left by an app that crashed (Unix keeps them): once per process.
        static SWEPT: std::sync::Once = std::sync::Once::new();
        SWEPT.call_once(|| {
            sweep_stale_blocks(&default_block_dir());
        });
        Self {
            format,
            config,
            files: Mutex::new(BTreeMap::new()),
            live: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Records which plugins a file holds (from the scan cache), as the in-process backends'
    /// `register_plugin_file` does. The file is not opened.
    pub fn register_plugin_file(&self, path: &Path, plugins: &[PluginRef]) {
        let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        for plugin in plugins {
            files.insert(plugin.clone(), path.to_path_buf());
        }
    }

    /// Children alive now (for tests and diagnostics).
    pub fn live_children(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Makes the shared block and the doorbell and starts the child for `plugin`.
    fn spawn(
        &self,
        plugin: &PluginRef,
    ) -> Result<(Transport, ParentPort, LiveGuard), BackendError> {
        let file = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(plugin)
            .cloned()
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let number = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let io_error = |what: &str, e: std::io::Error| {
            BackendError::Failed(format!("cannot create the plugin's {what}: {e}"))
        };
        let block_path = default_block_dir().join(block_file_name(pid, number));
        let doorbell_name = DoorbellName::for_instance(pid, number);
        let block = SharedBlock::create(&block_path).map_err(|e| io_error("audio block", e))?;
        let doorbell =
            Doorbell::create(&doorbell_name, &block).map_err(|e| io_error("doorbell", e))?;
        let port = ParentPort::new(block, doorbell);

        let command = &self.config.command;
        let mut args = command.args.clone();
        args.push(self.format.name().into());
        args.push(file.clone().into_os_string());
        args.push(plugin.id.clone().into());
        args.push("--block".into());
        args.push(block_path.into_os_string());
        args.push("--doorbell".into());
        args.push(doorbell_name.0.into());
        args.push("--parent-pid".into());
        args.push(pid.to_string().into());
        let transport =
            transport::spawn_process(&command.program, &args, &command.envs).map_err(|e| {
                BackendError::Failed(format!(
                    "cannot start {} for {}: {e}",
                    command.program.display(),
                    file.display()
                ))
            })?;
        Ok((transport, port, LiveGuard::new(&self.live)))
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts {
            create: self.config.create_timeout,
            call: self.config.call_timeout,
            text: self.config.text_timeout,
            watchdog: self.config.offline_watchdog,
            hung: self.config.hung_after,
            slow_retry: SLOW_RETRY,
            bounce: self.config.bounce,
        }
    }
}

impl PluginBackend for SandboxBackend {
    fn format(&self) -> PluginFormat {
        self.format
    }

    fn scan_file(&self, _path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        Err(BackendError::Unsupported(
            "the sandbox does not scan: the scanner does".to_string(),
        ))
    }

    fn instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        let (transport, port, live) = self.spawn(plugin)?;
        let instance = RemoteInstance::start(
            transport,
            port,
            self.timeouts(),
            host.messenger().clone(),
            live,
        )?;
        Ok(Box::new(instance))
    }

    fn begin_instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Option<Box<dyn PendingInstance>>, BackendError> {
        let (transport, port, live) = self.spawn(plugin)?;
        let pending = RemoteInstance::begin(
            transport,
            port,
            self.timeouts(),
            host.messenger().clone(),
            live,
        )?;
        Ok(Some(Box::new(pending)))
    }
}
