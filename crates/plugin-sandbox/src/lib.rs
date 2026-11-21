//! The plugin sandbox, app side: a [`PluginBackend`] that runs every plugin instance in its own
//! `atomik-plugin-host` child process, so a plugin that crashes or hangs cannot take the app
//! down.
//!
//! * [`SandboxBackend`] starts one child per instance (or, when the app asks for it with
//!   [`SandboxBackend::set_grouping`], one child for several instances) and talks to it over its
//!   stdin and stdout (main-thread calls, with timeouts) and a shared audio block (the
//!   engine-side processor).
//! * When a child ends, its instance is lost: the device turns CRASHED and the app replaces the
//!   instance on RELOAD. A child that hosts a group loses every instance of the group together.
//!
//! The contract is `not-for-git/plan/sandbox/03-interfaces.md`, section C.

mod clock;
mod group;
mod instance;
mod link;
mod processor;
mod transport;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use crate::group::{Attach, Hub};
use crate::instance::RemoteInstance;
use crate::link::{LiveGuard, Timeouts};
use crate::transport::Transport;
use atomik_device_sdk::{PluginFormat, PluginRef};
use atomik_plugin_host::{
    BackendError, HostHandle, PendingInstance, PluginBackend, PluginInstance, ProcessChoice,
    ProcessMode, ScannedPlugin,
};
use atomik_plugin_ipc::{
    Doorbell, DoorbellName, ParentPort, SharedBlock, block_file_name, default_block_dir,
    sweep_stale_blocks,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
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
    /// Loading: the child's greeting, `Instantiate` and opening the editor. A live child that
    /// has not answered yet is waiting for something (an online licence check, a sign-in
    /// dialog, a firewall prompt), so this is long, see [`LOAD_TIMEOUT`]; the app polls these
    /// waits and stays responsive.
    pub load_timeout: Duration,
    /// Activate, deactivate, capture and restore state.
    pub create_timeout: Duration,
    /// Every other main-thread call.
    pub call_timeout: Duration,
    /// The calls that only polish the interface (param text, pins, layout, resize, closing the
    /// editor): the longest one is waited for. A late answer is still used and never stops the
    /// child, see [`SandboxConfig::new`].
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
    /// The default timeouts ([`LOAD_TIMEOUT`], 10 s, 2 s, 250 ms, 30 s, 3 s) for `command`, for
    /// the live app.
    ///
    /// A call that matters (create, activate, state, deactivate) that is not answered in time
    /// stops the child, unless it shows a sign of life. A cosmetic request is waited for only a
    /// moment (a share of one frame's allowance), its late answer is used when it comes, and the
    /// child is marked slow after a few late answers within a few seconds.
    pub fn new(command: HostCommand) -> Self {
        Self {
            command,
            load_timeout: LOAD_TIMEOUT,
            create_timeout: Duration::from_secs(10),
            call_timeout: Duration::from_secs(2),
            text_timeout: Duration::from_millis(250),
            offline_watchdog: Duration::from_secs(30),
            hung_after: Duration::from_secs(3),
            bounce: false,
        }
    }
}

/// How long a plugin may take to load or to open its editor before its child is stopped: long
/// enough for a user to answer a firewall prompt and a plugin's own licence or sign-in dialogs.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The sandboxed backend of one plugin format.
#[derive(Debug)]
pub struct SandboxBackend {
    format: PluginFormat,
    config: SandboxConfig,
    /// Which file holds each plugin (fed from the scan cache; main thread only).
    files: Mutex<BTreeMap<PluginRef, PathBuf>>,
    /// Child processes alive now.
    live: Arc<AtomicUsize>,
    /// Instances that live in a group child now.
    members: Arc<AtomicUsize>,
    /// How instances share children (see [`SandboxBackend::set_grouping`]).
    grouping: Mutex<Grouping>,
    /// The group children by what their instances share.
    hubs: Arc<Hubs>,
}

/// The group children by what their instances share.
type Hubs = Mutex<BTreeMap<String, Vec<Weak<Hub>>>>;

/// The sharing rule: the setting and what single plugins ask for.
#[derive(Debug, Default)]
struct Grouping {
    mode: ProcessMode,
    choices: BTreeMap<PluginRef, ProcessChoice>,
}

impl Grouping {
    /// What the next instance of `plugin` (held in `file`) shares its child with: `None` for a
    /// child of its own.
    fn key(&self, plugin: &PluginRef, file: &Path) -> Option<String> {
        let choice = self.choices.get(plugin).copied().unwrap_or_default();
        match (choice, self.mode) {
            (ProcessChoice::Own, _) | (ProcessChoice::Default, ProcessMode::PerPlugin) => None,
            (ProcessChoice::Shared, _) | (ProcessChoice::Default, ProcessMode::Shared) => {
                Some("shared".to_string())
            }
            (ProcessChoice::Default, ProcessMode::PerFile) => {
                Some(format!("file:{}", file.display()))
            }
        }
    }
}

/// How long a slow instance is left alone between two waits for its cosmetic answers.
const SLOW_RETRY: Duration = Duration::from_secs(1);

/// How long a late cosmetic answer counts toward an instance being slow.
const SLOW_WINDOW: Duration = Duration::from_secs(5);

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
            members: Arc::new(AtomicUsize::new(0)),
            grouping: Mutex::new(Grouping::default()),
            hubs: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Sets how the instances created from now on share child processes: the `mode` for every
    /// plugin, and the `choices` of the plugins that ask for something else. Instances that are
    /// running keep their child.
    pub fn set_grouping(&self, mode: ProcessMode, choices: BTreeMap<PluginRef, ProcessChoice>) {
        *self.grouping.lock().unwrap_or_else(PoisonError::into_inner) = Grouping { mode, choices };
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

    /// Child processes alive now (for tests and diagnostics): a group counts once.
    pub fn live_children(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Instances that live in a group child now (for tests and diagnostics).
    pub fn grouped_instances(&self) -> usize {
        self.members.load(Ordering::SeqCst)
    }

    /// Gathers what starting the child for `plugin` needs (the numbers, the file, the sharing
    /// rule) on the calling thread, so the part that costs time can run on another one.
    fn prepare(&self, plugin: &PluginRef) -> Result<Launch, BackendError> {
        let file = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(plugin)
            .cloned()
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let number = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let key = self
            .grouping
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .key(plugin, &file);
        Ok(Launch {
            format: self.format,
            command: self.config.command.clone(),
            file,
            plugin_id: plugin.id.clone(),
            pid: std::process::id(),
            number,
            key,
            live: Arc::clone(&self.live),
            members: Arc::clone(&self.members),
            hubs: Arc::clone(&self.hubs),
        })
    }

    /// Makes the shared block and the doorbell and starts the child for `plugin`.
    fn spawn(
        &self,
        plugin: &PluginRef,
    ) -> Result<(Transport, ParentPort, LiveGuard), BackendError> {
        self.prepare(plugin)?.run()
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts {
            load: self.config.load_timeout,
            create: self.config.create_timeout,
            call: self.config.call_timeout,
            text: self.config.text_timeout,
            watchdog: self.config.offline_watchdog,
            hung: self.config.hung_after,
            slow_retry: SLOW_RETRY,
            slow_window: SLOW_WINDOW,
            bounce: self.config.bounce,
        }
    }
}

/// Everything it takes to start the child of one instance, owned, so that it can run on a
/// helper thread: the shared block and the doorbell, then the process (or the join of a group
/// child). Creating a process costs milliseconds (more with a virus scanner watching), which
/// the interface thread must not spend for every plugin of a project that opens.
struct Launch {
    format: PluginFormat,
    command: HostCommand,
    file: PathBuf,
    plugin_id: String,
    pid: u32,
    number: u64,
    /// What the instance shares its child with (`None`: a child of its own).
    key: Option<String>,
    live: Arc<AtomicUsize>,
    members: Arc<AtomicUsize>,
    hubs: Arc<Hubs>,
}

impl Launch {
    fn run(self) -> Result<(Transport, ParentPort, LiveGuard), BackendError> {
        let io_error = |what: &str, e: std::io::Error| {
            BackendError::Failed(format!("cannot create the plugin's {what}: {e}"))
        };
        let block_path = default_block_dir().join(block_file_name(self.pid, self.number));
        let doorbell_name = DoorbellName::for_instance(self.pid, self.number);
        let block = SharedBlock::create(&block_path).map_err(|e| io_error("audio block", e))?;
        let doorbell =
            Doorbell::create(&doorbell_name, &block).map_err(|e| io_error("doorbell", e))?;
        let port = ParentPort::new(block, doorbell);

        // A path that is not text cannot travel over the control channel: that instance gets a
        // child of its own.
        if let (Some(key), Some(plugin_file), Some(block)) =
            (&self.key, self.file.to_str(), block_path.to_str())
        {
            let attach = Attach {
                plugin_file: plugin_file.to_string(),
                plugin_id: self.plugin_id.clone(),
                block: block.to_string(),
                doorbell: doorbell_name.0.clone(),
            };
            let transport = self.join_group(key, attach)?;
            return Ok((transport, port, LiveGuard::new(&self.members)));
        }

        let command = &self.command;
        let mut args = command.args.clone();
        args.push(self.format.name().into());
        args.push(self.file.clone().into_os_string());
        args.push(self.plugin_id.clone().into());
        args.push("--block".into());
        args.push(block_path.into_os_string());
        args.push("--doorbell".into());
        args.push(doorbell_name.0.into());
        args.push("--parent-pid".into());
        args.push(self.pid.to_string().into());
        let transport =
            transport::spawn_process(&command.program, &args, &command.envs).map_err(|e| {
                BackendError::Failed(format!(
                    "cannot start {} for {}: {e}",
                    command.program.display(),
                    self.file.display()
                ))
            })?;
        Ok((transport, port, LiveGuard::new(&self.live)))
    }

    /// Adds an instance to a group child that shares `key`: the first one with room, or a new
    /// one when there is none (every one is full or gone).
    fn join_group(&self, key: &str, attach: Attach) -> Result<Transport, BackendError> {
        let mut hubs = self.hubs.lock().unwrap_or_else(PoisonError::into_inner);
        let list = hubs.entry(key.to_string()).or_default();
        list.retain(|w| w.upgrade().is_some_and(|h| h.alive()));
        for hub in list.iter().filter_map(Weak::upgrade) {
            if let Some(transport) = hub.join(attach.clone()) {
                return Ok(transport);
            }
        }
        let command = &self.command;
        let mut args = command.args.clone();
        args.push("--group".into());
        args.push(self.format.name().into());
        args.push("--parent-pid".into());
        args.push(self.pid.to_string().into());
        let transport =
            transport::spawn_process(&command.program, &args, &command.envs).map_err(|e| {
                BackendError::Failed(format!(
                    "cannot start {} for a plugin group: {e}",
                    command.program.display()
                ))
            })?;
        let hub = Hub::start(transport, LiveGuard::new(&self.live));
        let transport = hub.join(attach).ok_or_else(|| {
            BackendError::Failed("the plugin group ended before it took the plugin".to_string())
        })?;
        list.push(Arc::downgrade(&hub));
        Ok(transport)
    }
}

/// What the helper thread hands over: the instance that is starting, or why it could not.
type Begun = Result<instance::PendingRemote, BackendError>;

/// An instance whose child is being started on a helper thread (see [`Launch`]): the interface
/// thread only gets "not ready yet" until the child is there, then the start goes on as any other
/// ([`instance::PendingRemote`]). Abandoning it while the child is still starting ends that child
/// as soon as it is there.
struct Spawning {
    from: std::sync::mpsc::Receiver<Begun>,
    /// The start itself once the helper is done.
    begun: Option<instance::PendingRemote>,
    /// The helper failed: the error, until it is handed out.
    failed: Option<BackendError>,
    /// The helper is gone and nothing is left to take from it.
    closed: bool,
}

impl Spawning {
    /// Takes the helper's answer if it came.
    fn arrive(&mut self) {
        if self.begun.is_some() || self.failed.is_some() || self.closed {
            return;
        }
        match self.from.try_recv() {
            Ok(Ok(pending)) => self.begun = Some(pending),
            Ok(Err(e)) => self.failed = Some(e),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.closed = true;
                self.failed = Some(BackendError::Failed(
                    "the thread that starts the plugin's process ended without an answer".into(),
                ));
            }
        }
    }
}

impl PendingInstance for Spawning {
    fn poll(&mut self) -> Option<Result<Box<dyn PluginInstance>, BackendError>> {
        self.arrive();
        if let Some(e) = self.failed.take() {
            return Some(Err(e));
        }
        self.begun.as_mut()?.poll()
    }

    fn begin_shutdown(&mut self) {
        self.arrive();
        if let Some(pending) = &mut self.begun {
            pending.begin_shutdown();
        }
    }

    fn poll_shutdown(&mut self) -> bool {
        self.arrive();
        match &mut self.begun {
            Some(pending) => {
                // The helper may have answered after the abandon: the shutdown is asked for
                // again (asking twice is harmless).
                pending.begin_shutdown();
                pending.poll_shutdown()
            }
            // The helper is still working (the polls wait for it) or it failed, in which case
            // there is no child to end.
            None => self.failed.is_some() || self.closed,
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
            host.budget().clone(),
        )?;
        Ok(Box::new(instance))
    }

    fn begin_instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Option<Box<dyn PendingInstance>>, BackendError> {
        let launch = self.prepare(plugin)?;
        let (timeouts, messenger, budget) = (
            self.timeouts(),
            host.messenger().clone(),
            host.budget().clone(),
        );
        let (to, from) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("plugin-spawn".into())
            .spawn(move || {
                let begun = launch.run().and_then(|(transport, port, live)| {
                    RemoteInstance::begin(transport, port, timeouts, messenger, live, budget)
                });
                // The receiver is gone when the start was dropped: the child ends with it.
                let _ = to.send(begun);
            })
            .map_err(|e| {
                BackendError::Failed(format!(
                    "cannot start the thread that starts the plugin: {e}"
                ))
            })?;
        Ok(Some(Box::new(Spawning {
            from,
            begun: None,
            failed: None,
            closed: false,
        })))
    }
}
