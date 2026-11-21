//! [`RemoteInstance`]: the main-thread half of a plugin that runs in a child process.
//!
//! Every method is a call over the control link with a timeout (see [`crate::link`]), except the
//! cosmetic ones (a parameter's text, resizing or closing the editor, the default pins, a layout
//! change), which ask and wait only a moment: what the plugin answers later is handed over by
//! [`PluginInstance::take_soft_answers`]. The instance goes through the states of the
//! contract: Spawning (the Hello exchange, in [`RemoteInstance::start`]), Ready, Active (the
//! processor is out) and Lost. No transition leaves Lost: once the child is gone or has been
//! killed for not answering, every call fails with [`LOST`] and does no I/O; recovery is a new
//! instance.

use crate::link::{Grace, Link, LiveGuard, Reply, SoftAsked, SoftDone, SoftKind, Timeouts};
use crate::processor::{PortCell, RemoteProcessor};
use crate::transport::Transport;
use atomik_core::BATCH;
use atomik_device_sdk::{PluginLayout, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, EditorKind, HostMessenger, LogLevel, NativeParent,
    PendingInstance, PluginInstance, PluginMessage, PluginProcessor, PresetInfo, SoftAnswer,
    SoftBudget, warmed,
};
use atomik_plugin_ipc::{FromChild, ParentPort, ToChild, WireParent};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[cfg(test)]
thread_local! {
    /// How many times this thread removed a block file (a hook for the tests).
    pub(crate) static UNLINKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The shortest live deadline of a batch, whatever the sample rate.
const MIN_PERIOD: Duration = Duration::from_micros(50);

/// The most late cosmetic answers kept for the app to collect.
const MAX_READY: usize = 256;

/// A plugin instance in its own process.
pub(crate) struct RemoteInstance {
    link: Link,
    port: PortCell,
    layout: PluginLayout,
    has_gui: bool,
    latency: u32,
    /// Activated for an offline bounce: `poll` flushes the child's notifications.
    offline_active: bool,
    /// Cosmetic answers that came after their call returned, for
    /// [`PluginInstance::take_soft_answers`] (the newest [`MAX_READY`]).
    ready: Vec<SoftAnswer>,
    /// The latest size the editor was asked for while a resize was still out: it is sent when
    /// that one is answered, so a drag never queues a backlog of sizes.
    wanted_resize: Option<(u32, u32)>,
    /// When the `OpenGui` that is still out was sent (see
    /// [`PluginInstance::begin_open_external_gui`]).
    opening_since: Option<std::time::Instant>,
    /// When a [`PluginInstance::begin_deactivate`] went out and has no answer yet.
    deactivating_since: Option<std::time::Instant>,
    /// When a [`PluginInstance::begin_activate`] went out and has no answer yet, and what it
    /// asked for.
    activating: Option<(std::time::Instant, ActivateConfig)>,
    /// When a [`PluginInstance::begin_restore_state`] went out and has no answer yet.
    restoring_since: Option<std::time::Instant>,
    /// When a [`PluginInstance::begin_load_preset`] went out and has no answer yet.
    loading_since: Option<std::time::Instant>,
}

/// The name of the editor request, in errors and the held-call guard.
const OPEN_GUI: &str = "OpenGui";
const LOAD_PRESET: &str = "LoadPreset";
const DEACTIVATE: &str = "Deactivate";
const ACTIVATE: &str = "Activate";
const RESTORE_STATE: &str = "RestoreState";

enum Stage {
    /// The Hello is out.
    Greeting,
    /// The `Instantiate` is out.
    Creating,
}

/// A remote instance that is starting: see [`RemoteInstance::begin`].
pub(crate) struct PendingRemote {
    link: Option<Link>,
    port: Option<ParentPort>,
    warn: HostMessenger,
    stage: Stage,
    /// When the request of this stage went out.
    since: std::time::Instant,
}

impl PendingInstance for PendingRemote {
    fn poll(&mut self) -> Option<Result<Box<dyn PluginInstance>, BackendError>> {
        let create = self.link.as_ref()?.timeouts().load;
        let link = self.link.as_mut()?;
        match self.stage {
            Stage::Greeting => {
                let reply = match link.poll_reply("Hello", self.since, create)? {
                    Ok(reply) => reply,
                    Err(e) => return Some(Err(link.explain(e))),
                };
                if let Err(e) = link.check_hello(reply) {
                    return Some(Err(e));
                }
                if let Some(port) = &self.port {
                    RemoteInstance::unlink_block(port, &self.warn);
                }
                // A plugin that shows a licence or sign-in dialog while it loads may bring it to
                // the front.
                atomik_plugin_ipc::lifetime::allow_foreground(link.shared().pid());
                if let Err(e) = link.begin_call(|id| ToChild::Instantiate { id }) {
                    return Some(Err(link.explain(e)));
                }
                self.stage = Stage::Creating;
                self.since = crate::clock::now();
                None
            }
            Stage::Creating => {
                let reply = match link.poll_reply("Instantiate", self.since, create)? {
                    Ok(reply) => reply,
                    Err(e) => return Some(Err(link.explain(e))),
                };
                let (link, port) = (self.link.take()?, self.port.take()?);
                Some(RemoteInstance::instantiated(link, port, reply).map(|i| Box::new(i) as _))
            }
        }
    }

    fn begin_shutdown(&mut self) {
        if let Some(link) = &mut self.link {
            link.begin_shutdown();
        }
    }

    fn poll_shutdown(&mut self) -> bool {
        let Some(link) = &mut self.link else {
            return true;
        };
        if link.timeouts().bounce {
            link.wait_for_end();
            return true;
        }
        link.poll_shutdown()
    }
}

fn unexpected_none(what: &str) -> BackendError {
    BackendError::Failed(format!(
        "the plugin process answered {what} without its data"
    ))
}

impl RemoteInstance {
    /// Starts the link over `transport`, greets the child and creates its plugin.
    ///
    /// # Errors
    /// The child did not greet or create the plugin; the error names how a child that ended
    /// did so.
    pub fn start(
        transport: Transport,
        port: ParentPort,
        timeouts: Timeouts,
        messenger: HostMessenger,
        live: LiveGuard,
        budget: SoftBudget,
    ) -> Result<Self, BackendError> {
        let death = port.death_handle();
        let warn = messenger.clone();
        let mut link = Link::start(transport, death, timeouts, messenger, live, budget)?;
        Self::unlink_block(&port, &warn);
        atomik_plugin_ipc::lifetime::allow_foreground(link.shared().pid());
        let reply = link
            .call(
                "Instantiate",
                timeouts.load,
                |id| ToChild::Instantiate { id },
                None,
            )
            .map_err(|e| link.explain(e))?;
        Self::instantiated(link, port, reply)
    }

    /// The first half of [`RemoteInstance::start`] without the waiting: spawns the link and
    /// sends the greeting; [`PendingRemote::poll`] carries on as the child answers.
    ///
    /// # Errors
    /// The helper threads could not start.
    pub fn begin(
        transport: Transport,
        port: ParentPort,
        timeouts: Timeouts,
        messenger: HostMessenger,
        live: LiveGuard,
        budget: SoftBudget,
    ) -> Result<PendingRemote, BackendError> {
        let death = port.death_handle();
        let warn = messenger.clone();
        let link = Link::launch(transport, death, timeouts, messenger, live, budget)?;
        Ok(PendingRemote {
            link: Some(link),
            port: Some(port),
            warn,
            stage: Stage::Greeting,
            since: crate::clock::now(),
        })
    }

    /// The instance from the answer to `Instantiate`.
    fn instantiated(link: Link, port: ParentPort, reply: Reply) -> Result<Self, BackendError> {
        let FromChild::Instantiated {
            layout,
            has_gui,
            latency,
            ..
        } = reply.msg
        else {
            return Err(link.unexpected("Instantiate", &reply.msg));
        };
        Ok(Self {
            link,
            port: Arc::new(warmed(Mutex::new(Some(port)))),
            layout: layout.into(),
            has_gui,
            latency,
            offline_active: false,
            ready: Vec::new(),
            wanted_resize: None,
            opening_since: None,
            deactivating_since: None,
            activating: None,
            restoring_since: None,
            loading_since: None,
        })
    }

    /// The block file is no longer needed once the child has greeted (it has mapped the block)
    /// and must not outlive a crash of the app.
    fn unlink_block(port: &ParentPort, warn: &HostMessenger) {
        #[cfg(test)]
        UNLINKS.with(|n| n.set(n.get() + 1));
        if let Err(e) = port.unlink_block() {
            warn.post(PluginMessage::Log {
                level: LogLevel::Warning,
                text: format!("cannot remove the plugin's block file: {e}"),
            });
        }
    }

    fn timeouts(&self) -> Timeouts {
        self.link.timeouts()
    }

    /// Waits for a [`PluginInstance::begin_deactivate`] whose answer nobody polled (the device
    /// went away meanwhile), so that the next call is not refused as busy.
    fn finish_deactivate(&mut self) {
        let Some(since) = self.deactivating_since else {
            return;
        };
        let wait = self.timeouts().create;
        while self.link.poll_reply(DEACTIVATE, since, wait).is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.deactivating_since = None;
    }

    /// What an activation needs before it is sent: a usable sample rate and the audio port.
    fn check_activatable(&self, cfg: &ActivateConfig) -> Result<(), BackendError> {
        if !(cfg.sample_rate.is_finite() && cfg.sample_rate > 0.0) {
            return Err(BackendError::Failed(format!(
                "cannot activate at the sample rate {}",
                cfg.sample_rate
            )));
        }
        if self
            .port
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
        {
            return Err(BackendError::Failed(
                "the plugin is already active".to_string(),
            ));
        }
        Ok(())
    }

    /// The audio half from the child's answer to `Activate`.
    fn activated(
        &mut self,
        reply: Reply,
        cfg: &ActivateConfig,
    ) -> Result<Box<dyn PluginProcessor>, BackendError> {
        let offline = cfg.mode == RunMode::Offline;
        let FromChild::Activated { latency, .. } = reply.msg else {
            return Err(self.link.unexpected(ACTIVATE, &reply.msg));
        };
        self.latency = latency;
        // A bounce waits for every batch whatever mode the plugin was told.
        let wait_all = offline || self.timeouts().bounce;
        self.offline_active = wait_all;
        let port = self
            .port
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| BackendError::Failed("the plugin is already active".to_string()))?;
        let period = Self::period(cfg.sample_rate);
        let mut processor = RemoteProcessor::new(
            port,
            Arc::clone(&self.port),
            wait_all,
            period,
            self.timeouts().watchdog,
        );
        if !wait_all {
            // Live only: an offline batch waits for its answer up to the watchdog instead.
            processor = processor.with_hang_guard(
                Self::hang_batches(period, self.timeouts().hung),
                self.link.shared().hung_flag(),
            );
        }
        Ok(Box::new(processor))
    }

    fn period(sample_rate: f64) -> Duration {
        Duration::from_secs_f64(BATCH as f64 / sample_rate).max(MIN_PERIOD)
    }

    /// How many late batches in a row make `hung` long, at one batch per `period`; 0 turns the
    /// guard off.
    fn hang_batches(period: Duration, hung: Duration) -> u32 {
        if hung.is_zero() {
            return 0;
        }
        (hung.as_secs_f64() / period.as_secs_f64()).ceil().max(1.0) as u32
    }

    /// Sends `OpenGui` without waiting for the answer (see
    /// [`PluginInstance::begin_open_external_gui`]); with `embed` the editor becomes a child of
    /// `owner` instead of being owned by it.
    fn begin_open(&mut self, owner: Option<WireParent>, embed: bool) -> Result<bool, BackendError> {
        if self.link.held() == Some(OPEN_GUI) {
            // Still opening: asking again changes nothing.
            return Ok(false);
        }
        self.link.begin_held_call(OPEN_GUI, |id| ToChild::OpenGui {
            id,
            owner,
            scale: 1.0,
            embed,
        })?;
        self.opening_since = Some(crate::clock::now());
        // The user just asked for the editor: its window, and any dialog the plugin shows
        // first, may come to the front.
        atomik_plugin_ipc::lifetime::allow_foreground(self.link.shared().pid());
        Ok(false)
    }

    /// The state a `State` reply carries.
    fn state_of(&self, reply: Reply) -> Result<PluginStateBlob, BackendError> {
        match (&reply.msg, reply.blob) {
            (FromChild::State { .. }, Some(blob)) => Ok(PluginStateBlob(blob)),
            (FromChild::State { .. }, None) => Err(unexpected_none("CaptureState")),
            (other, _) => Err(self.link.unexpected("CaptureState", other)),
        }
    }

    /// Collects the cosmetic answers that came since the last time (once per frame, from
    /// [`PluginInstance::poll`]).
    fn service_soft(&mut self) {
        for done in self.link.service_soft() {
            self.apply_soft(done);
        }
    }

    /// Puts a late cosmetic answer to use: a text or a size is kept for the app, a layout change
    /// is kept for the next [`PluginInstance::take_layout_changed`] (the child says it once),
    /// and the next size of a resize that was merged goes out.
    fn apply_soft(&mut self, done: SoftDone) {
        let Ok(reply) = done.answer else {
            // An error to a request made for show is nothing to tell anyone, except that an
            // editor that refuses to resize says so, and that a list of presets that failed
            // is asked for again later.
            match done.kind {
                SoftKind::ResizeGui { w, h } => self.resized(w, h, None),
                SoftKind::ListPresets => self.keep_ready(SoftAnswer::Presets(None)),
                SoftKind::ReadParams => self.keep_ready(SoftAnswer::Params(None)),
                _ => {}
            }
            return;
        };
        match (done.kind, reply.msg) {
            (
                SoftKind::ParamText { param, plain },
                FromChild::Text {
                    text: Some(text), ..
                },
            ) => {
                self.keep_ready(SoftAnswer::ParamText { param, plain, text });
            }
            (SoftKind::ResizeGui { w, h }, FromChild::GuiSized { w: tw, h: th, .. }) => {
                self.resized(w, h, Some((tw, th)));
            }
            (SoftKind::ResizeGui { .. }, other) => {
                let _ = self.link.unexpected("ResizeGui", &other);
            }
            (SoftKind::DefaultPins, FromChild::Pins { pins, .. }) => {
                self.keep_ready(SoftAnswer::DefaultPins(pins));
            }
            (SoftKind::ListPresets, FromChild::Presets { presets, .. }) => {
                let list = presets.into_iter().map(Into::into).collect();
                self.keep_ready(SoftAnswer::Presets(Some(list)));
            }
            (SoftKind::ReadParams, FromChild::ParamValues { values, .. }) => {
                self.keep_ready(SoftAnswer::Params(Some(values)));
            }
            (SoftKind::ListPresets, _) => self.keep_ready(SoftAnswer::Presets(None)),
            (SoftKind::ReadParams, _) => self.keep_ready(SoftAnswer::Params(None)),
            (
                SoftKind::TakeLayoutChanged,
                FromChild::LayoutChanged {
                    layout: Some(layout),
                    ..
                },
            ) => self.link.shared().keep_late_layout(layout),
            _ => {}
        }
    }

    /// The answer to the resize to `(w, h)`: when a later size was asked for meanwhile it is sent
    /// now and this answer is superseded, else the app is told what the editor took.
    fn resized(&mut self, w: u32, h: u32, took: Option<(u32, u32)>) {
        if let Some((nw, nh)) = self.wanted_resize.take()
            && (nw, nh) != (w, h)
        {
            // Nothing is waited for: its answer is collected like this one.
            let _ = self
                .link
                .soft_ask(SoftKind::ResizeGui { w: nw, h: nh }, Grace::None, |id| {
                    ToChild::ResizeGui { id, w: nw, h: nh }
                });
            return;
        }
        self.keep_ready(SoftAnswer::GuiSized {
            asked: (w, h),
            took,
        });
    }

    fn keep_ready(&mut self, answer: SoftAnswer) {
        if self.ready.len() >= MAX_READY {
            self.ready.remove(0);
        }
        self.ready.push(answer);
    }

    /// An `Ok`-or-`GuiOpened` reply (the child may say either to an `OpenGui`).
    fn is_ok(reply: &Reply) -> bool {
        matches!(
            reply.msg,
            FromChild::Ok { .. } | FromChild::GuiOpened { .. }
        )
    }
}

impl PluginInstance for RemoteInstance {
    fn layout(&self) -> &PluginLayout {
        &self.layout
    }

    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError> {
        self.finish_deactivate();
        self.check_activatable(cfg)?;
        let (rate, frames, offline) = (cfg.sample_rate, cfg.frames, cfg.mode == RunMode::Offline);
        let reply = self.link.call(
            ACTIVATE,
            self.timeouts().create,
            |id| ToChild::Activate {
                id,
                sample_rate: rate,
                frames,
                offline,
            },
            None,
        )?;
        self.activated(reply, cfg)
    }

    fn begin_activate(
        &mut self,
        cfg: &ActivateConfig,
    ) -> Result<Option<Box<dyn PluginProcessor>>, BackendError> {
        if self.timeouts().bounce {
            // A bounce waits for the child, whatever mode the plugin is told.
            return self.activate(cfg).map(Some);
        }
        self.finish_deactivate();
        self.check_activatable(cfg)?;
        let (rate, frames, offline) = (cfg.sample_rate, cfg.frames, cfg.mode == RunMode::Offline);
        self.link
            .begin_held_call(ACTIVATE, |id| ToChild::Activate {
                id,
                sample_rate: rate,
                frames,
                offline,
            })?;
        self.activating = Some((crate::clock::now(), *cfg));
        Ok(None)
    }

    fn poll_activate(&mut self) -> Option<Result<Box<dyn PluginProcessor>, BackendError>> {
        let (since, cfg) = self.activating?;
        // As long as a load's: the app stays responsive meanwhile, and a child that is alive and
        // working (a sampler loading its library as it activates) is not stopped for being slow.
        let wait = self.timeouts().load;
        let answer = self.link.poll_reply(ACTIVATE, since, wait)?;
        self.activating = None;
        Some(answer.and_then(|reply| self.activated(reply, &cfg)))
    }

    fn deactivate(&mut self, processor: Box<dyn PluginProcessor>) {
        // Dropping the processor puts the port back; the child's `Deactivate` then runs with
        // nothing in flight.
        drop(processor);
        self.offline_active = false;
        // A plugin that is busy but alive (it just finished a sign-in, it rebuilds its buses)
        // may take longer than an ordinary call: as long as an activation.
        let wait = self.timeouts().create;
        // A failure is a lost child, which the link has already recorded.
        let _ = self
            .link
            .call(DEACTIVATE, wait, |id| ToChild::Deactivate { id }, None);
    }

    fn begin_deactivate(&mut self, processor: Box<dyn PluginProcessor>) -> bool {
        if self.timeouts().bounce {
            // A bounce finishes a restart at the same batch as in process, whatever mode the
            // plugin was told: it waits for the child.
            self.deactivate(processor);
            return true;
        }
        drop(processor);
        self.offline_active = false;
        if self
            .link
            .begin_held_call(DEACTIVATE, |id| ToChild::Deactivate { id })
            .is_err()
        {
            // A lost child (recorded by the link) or one busy with another held request: there
            // is nothing to wait for.
            return true;
        }
        self.deactivating_since = Some(crate::clock::now());
        false
    }

    fn poll_deactivate(&mut self) -> bool {
        let Some(since) = self.deactivating_since else {
            return true;
        };
        // The app stays responsive while it waits, so the wait is as long as a load's: a child
        // that is alive and working is not stopped for being slow; a dead one ends the wait.
        let wait = self.timeouts().load;
        if self.link.poll_reply(DEACTIVATE, since, wait).is_none() {
            return false;
        }
        self.deactivating_since = None;
        true
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    fn begin_capture_state(&mut self) {
        if self.link.background_out() {
            // The request that is out answers the call that collects.
            return;
        }
        // A failure shows again when the state is collected.
        let _ = self.link.begin_call(|id| ToChild::CaptureState { id });
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        let timeout = self.timeouts().create;
        // A background request that is out is waited for, not repeated.
        let reply = match self.link.finish_background("CaptureState", timeout) {
            Some(reply) => reply?,
            None => match self.link.finish_call("CaptureState", timeout) {
                Some(reply) => reply?,
                None => self.link.call(
                    "CaptureState",
                    timeout,
                    |id| ToChild::CaptureState { id },
                    None,
                )?,
            },
        };
        self.state_of(reply)
    }

    fn background_capture(&self) -> bool {
        true
    }

    fn request_capture(&mut self) -> Result<(), BackendError> {
        self.link
            .begin_background(|id| ToChild::CaptureState { id })
    }

    fn poll_capture(&mut self) -> Option<Result<PluginStateBlob, BackendError>> {
        let reply = match self.link.poll_background()? {
            Ok(reply) => reply,
            Err(e) => return Some(Err(e)),
        };
        Some(self.state_of(reply))
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        let reply = self.link.call(
            RESTORE_STATE,
            self.timeouts().create,
            |id| ToChild::RestoreState { id },
            Some(&state.0),
        )?;
        if Self::is_ok(&reply) {
            Ok(())
        } else {
            Err(self.link.unexpected(RESTORE_STATE, &reply.msg))
        }
    }

    fn begin_restore_state(&mut self, state: &PluginStateBlob) -> Result<bool, BackendError> {
        if self.timeouts().bounce {
            return self.restore_state(state).map(|()| true);
        }
        self.link.begin_held_call_with(
            RESTORE_STATE,
            |id| ToChild::RestoreState { id },
            Some(&state.0),
        )?;
        self.restoring_since = Some(crate::clock::now());
        Ok(false)
    }

    fn poll_restore_state(&mut self) -> Option<Result<(), BackendError>> {
        let Some(since) = self.restoring_since else {
            return Some(Ok(()));
        };
        // A sampler may restore for minutes: as long as a load, judged by its sign of life.
        let wait = self.timeouts().load;
        let answer = self.link.poll_reply(RESTORE_STATE, since, wait)?;
        self.restoring_since = None;
        Some(answer.and_then(|reply| {
            if Self::is_ok(&reply) {
                Ok(())
            } else {
                Err(self.link.unexpected(RESTORE_STATE, &reply.msg))
            }
        }))
    }

    fn read_params(&mut self) -> Vec<(u32, f64)> {
        match self.link.call(
            "ReadParams",
            self.timeouts().create,
            |id| ToChild::ReadParams { id },
            None,
        ) {
            Ok(Reply {
                msg: FromChild::ParamValues { values, .. },
                ..
            }) => values,
            _ => Vec::new(),
        }
    }

    fn list_presets(&mut self) -> Vec<PresetInfo> {
        match self.link.call(
            "ListPresets",
            self.timeouts().create,
            |id| ToChild::ListPresets { id },
            None,
        ) {
            Ok(Reply {
                msg: FromChild::Presets { presets, .. },
                ..
            }) => presets.into_iter().map(Into::into).collect(),
            _ => Vec::new(),
        }
    }

    fn load_preset(&mut self, preset: &str) -> Result<(), BackendError> {
        let reply = self.link.call(
            "LoadPreset",
            self.timeouts().create,
            |id| ToChild::LoadPreset {
                id,
                preset: preset.to_string(),
            },
            None,
        )?;
        if Self::is_ok(&reply) {
            Ok(())
        } else {
            Err(self.link.unexpected("LoadPreset", &reply.msg))
        }
    }

    fn begin_list_presets(&mut self) -> Result<Option<Vec<PresetInfo>>, BackendError> {
        if self.timeouts().bounce {
            return Ok(Some(self.list_presets()));
        }
        if self.link.soft_has(|k| *k == SoftKind::ListPresets) {
            // Asked already: the list comes with that request's answer.
            return Ok(None);
        }
        self.link
            .soft_ask(SoftKind::ListPresets, Grace::None, |id| {
                ToChild::ListPresets { id }
            })
            .map(|_| None)
    }

    fn begin_read_params(&mut self) -> Result<Option<Vec<(u32, f64)>>, BackendError> {
        if self.timeouts().bounce {
            return Ok(Some(self.read_params()));
        }
        if self.link.soft_has(|k| *k == SoftKind::ReadParams) {
            return Ok(None);
        }
        self.link
            .soft_ask(SoftKind::ReadParams, Grace::None, |id| {
                ToChild::ReadParams { id }
            })
            .map(|_| None)
    }

    fn begin_load_preset(&mut self, preset: &str) -> Result<bool, BackendError> {
        if self.timeouts().bounce {
            // A bounce waits for the child, whatever mode the plugin is told.
            return self.load_preset(preset).map(|()| true);
        }
        self.link
            .begin_held_call(LOAD_PRESET, |id| ToChild::LoadPreset {
                id,
                preset: preset.to_string(),
            })?;
        self.loading_since = Some(crate::clock::now());
        Ok(false)
    }

    fn poll_load_preset(&mut self) -> Option<Result<(), BackendError>> {
        let Some(since) = self.loading_since else {
            return Some(Ok(()));
        };
        // A sampler may read its library for minutes: as long as a load, judged by its sign of
        // life.
        let wait = self.timeouts().load;
        let answer = self.link.poll_reply(LOAD_PRESET, since, wait)?;
        self.loading_since = None;
        Some(answer.and_then(|reply| {
            if Self::is_ok(&reply) {
                Ok(())
            } else {
                Err(self.link.unexpected(LOAD_PRESET, &reply.msg))
            }
        }))
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        if !plain.is_finite() {
            return None;
        }
        let kind = SoftKind::ParamText { param: id, plain };
        if self.link.soft_has(|k| *k == kind) {
            // Asked already: the text comes with that request's answer.
            return None;
        }
        // A plugin too busy to answer has no text yet, and none is made up: the app shows the
        // number until the text arrives (see `take_soft_answers`).
        match self
            .link
            .soft_ask(kind, Grace::Frame, |rid| ToChild::ParamText {
                id: rid,
                param: id,
                plain,
            }) {
            Ok(SoftAsked::Answered(Reply {
                msg: FromChild::Text { text, .. },
                ..
            })) => text,
            _ => None,
        }
    }

    fn open_gui(&mut self, _parent: NativeParent, _scale: f64) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "a sandboxed plugin's editor is not embedded".to_string(),
        ))
    }

    fn close_gui(&mut self) {
        // The one cosmetic call that waits its whole timeout: the window must not go while the
        // plugin is still using it. It is one wait; the state is asked for afterwards in the
        // background.
        let _ = self
            .link
            .soft_ask(SoftKind::CloseGui, Grace::Full, |id| ToChild::CloseGui {
                id,
            });
    }

    fn poll(&mut self) {
        // The reader thread does the work; the main thread only finishes what the audio side
        // could not do itself (it may not lock): ending a child it declared dead.
        self.link.shared().finish_audio_death();
        self.service_soft();
        if self.offline_active && !self.link.is_lost() {
            // The child forwards notifications on a timer; a bounce must not depend on it. The
            // reader posts every notification before it delivers a reply, so when the Flush is
            // answered they are all in the messenger.
            let call = self.timeouts().call;
            let _ = self
                .link
                .call("Flush", call, |id| ToChild::Flush { id }, None);
        }
        // The param edits the reader held back go to the messenger now, just before the manager
        // drains it.
        self.link.shared().flush_edits();
    }

    fn has_gui(&self) -> bool {
        self.has_gui
    }

    fn default_pins(&mut self) -> Vec<u32> {
        if self.link.soft_has(|k| *k == SoftKind::DefaultPins) {
            return Vec::new();
        }
        match self
            .link
            .soft_ask(SoftKind::DefaultPins, Grace::Frame, |id| {
                ToChild::DefaultPins { id }
            }) {
            Ok(SoftAsked::Answered(Reply {
                msg: FromChild::Pins { pins, .. },
                ..
            })) => pins,
            _ => Vec::new(),
        }
    }

    fn take_layout_changed(&mut self) -> bool {
        // The child tells a layout change once: one whose answer came after its call gave up
        // is kept for this one.
        self.service_soft();
        if let Some(layout) = self.link.shared().take_late_layout() {
            self.layout = layout.into();
            return true;
        }
        if self.link.soft_has(|k| *k == SoftKind::TakeLayoutChanged) {
            // Asked already: its answer is kept for the next call when it comes.
            return false;
        }
        match self
            .link
            .soft_ask(SoftKind::TakeLayoutChanged, Grace::Frame, |id| {
                ToChild::TakeLayoutChanged { id }
            }) {
            Ok(SoftAsked::Answered(Reply {
                msg:
                    FromChild::LayoutChanged {
                        layout: Some(layout),
                        ..
                    },
                ..
            })) => {
                self.layout = layout.into();
                true
            }
            _ => false,
        }
    }

    fn native_handle(&self) -> usize {
        self.link.shared().pid() as usize
    }

    fn editor_kind(&self) -> EditorKind {
        EditorKind::External
    }

    fn begin_open_external_gui(
        &mut self,
        owner: Option<NativeParent>,
    ) -> Result<bool, BackendError> {
        self.begin_open(owner.map(WireParent::from), false)
    }

    fn begin_open_embedded_gui(&mut self, parent: NativeParent) -> Result<bool, BackendError> {
        self.begin_open(Some(WireParent::from(parent)), true)
    }

    fn resize_gui(&mut self, w: u32, h: u32) -> Result<(u32, u32), BackendError> {
        // A plugin too busy to answer is not a failed one: the caller keeps the size it asked
        // for, and the editor takes the next one it is given.
        self.request_resize_gui(w, h)?.ok_or_else(|| {
            BackendError::Failed("the plugin has not answered the resize yet".to_string())
        })
    }

    fn request_resize_gui(&mut self, w: u32, h: u32) -> Result<Option<(u32, u32)>, BackendError> {
        if self
            .link
            .soft_has(|k| matches!(k, SoftKind::ResizeGui { .. }))
        {
            // One is out: only the latest size is kept, and goes when that one is answered.
            self.wanted_resize = Some((w, h));
            return Ok(None);
        }
        self.wanted_resize = None;
        let asked = self
            .link
            .soft_ask(SoftKind::ResizeGui { w, h }, Grace::Frame, |id| {
                ToChild::ResizeGui { id, w, h }
            })?;
        match asked {
            SoftAsked::Pending => Ok(None),
            SoftAsked::Answered(reply) => match reply.msg {
                FromChild::GuiSized { w, h, .. } => Ok(Some((w, h))),
                other => Err(self.link.unexpected("ResizeGui", &other)),
            },
        }
    }

    fn take_soft_answers(&mut self) -> Vec<SoftAnswer> {
        std::mem::take(&mut self.ready)
    }

    fn poll_open_external_gui(&mut self) -> Option<Result<(), BackendError>> {
        let since = self.opening_since?;
        let wait = self.timeouts().load;
        let answer = self.link.poll_reply(OPEN_GUI, since, wait)?;
        self.opening_since = None;
        Some(match answer {
            Ok(reply) if Self::is_ok(&reply) => Ok(()),
            Ok(reply) => Err(self.link.unexpected(OPEN_GUI, &reply.msg)),
            Err(e) => Err(self.link.explain(e)),
        })
    }

    fn open_external_gui(&mut self, owner: Option<NativeParent>) -> Result<(), BackendError> {
        // An editor may take minutes to open (licence or sign-in dialogs first): as long as a
        // load.
        let wait = self.timeouts().load;
        let owner = owner.map(WireParent::from);
        let reply = self.link.call(
            "OpenGui",
            wait,
            |id| ToChild::OpenGui {
                id,
                owner,
                scale: 1.0,
                embed: false,
            },
            None,
        )?;
        if Self::is_ok(&reply) {
            Ok(())
        } else {
            Err(self.link.unexpected("OpenGui", &reply.msg))
        }
    }

    fn is_lost(&self) -> bool {
        self.link.is_lost()
    }

    fn not_responding(&self) -> Option<String> {
        self.link.stuck()
    }

    fn stop_process(&mut self) {
        self.link
            .shared()
            .kill("the plugin did not respond and was stopped at the user's request".to_string());
    }

    fn begin_shutdown(&mut self) {
        self.link.begin_shutdown();
    }

    fn poll_shutdown(&mut self) -> bool {
        if self.timeouts().bounce {
            // A bounce waits for its children (the drop would, too).
            self.link.wait_for_end();
            return true;
        }
        self.link.poll_shutdown()
    }
}

#[cfg(test)]
impl RemoteInstance {
    /// Notifications refused for being out of bounds.
    pub(crate) fn rejected_frames(&self) -> u64 {
        self.link.shared().rejected()
    }

    /// True while the child's cosmetic answers are late.
    pub(crate) fn is_slow(&self) -> bool {
        self.link.is_slow()
    }

    /// The cosmetic requests that are out or not yet collected.
    pub(crate) fn soft_out(&self) -> bool {
        self.link.soft_has(|_| true)
    }

    /// The allowance of waiting for cosmetic answers this instance draws on.
    pub(crate) fn budget(&self) -> atomik_plugin_host::SoftBudget {
        self.link.budget().clone()
    }
}
