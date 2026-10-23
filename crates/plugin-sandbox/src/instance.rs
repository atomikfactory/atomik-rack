//! [`RemoteInstance`]: the main-thread half of a plugin that runs in a child process.
//!
//! Every method is a call over the control link with a timeout (see [`crate::link`]). The
//! instance goes through the states of the contract: Spawning (the Hello exchange, in
//! [`RemoteInstance::start`]), Ready, Active (the processor is out) and Lost. No transition
//! leaves Lost: once the child is gone or has been killed for not answering, every call
//! fails with [`LOST`] and does no I/O; recovery is a new instance.

use crate::link::{Link, LiveGuard, Reply, Timeouts};
use crate::processor::{PortCell, RemoteProcessor};
use crate::transport::Transport;
use atomik_core::BATCH;
use atomik_device_sdk::{PluginLayout, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, EditorKind, HostMessenger, LogLevel, NativeParent,
    PluginInstance, PluginMessage, PluginProcessor, warmed,
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

/// A plugin instance in its own process.
pub(crate) struct RemoteInstance {
    link: Link,
    port: PortCell,
    layout: PluginLayout,
    has_gui: bool,
    latency: u32,
    /// Activated for an offline bounce: `poll` flushes the child's notifications.
    offline_active: bool,
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
    ) -> Result<Self, BackendError> {
        let death = port.death_handle();
        let warn = messenger.clone();
        let mut link = Link::start(transport, death, timeouts, messenger, live)?;
        // The child has greeted, so it has mapped the block: the file is no longer needed and
        // must not outlive a crash of the app.
        #[cfg(test)]
        UNLINKS.with(|n| n.set(n.get() + 1));
        if let Err(e) = port.unlink_block() {
            warn.post(PluginMessage::Log {
                level: LogLevel::Warning,
                text: format!("cannot remove the plugin's block file: {e}"),
            });
        }
        let reply = link
            .call(
                "Instantiate",
                timeouts.create,
                |id| ToChild::Instantiate { id },
                None,
            )
            .map_err(|e| link.explain(e))?;
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
        })
    }

    fn timeouts(&self) -> Timeouts {
        self.link.timeouts()
    }

    fn period(sample_rate: f64) -> Duration {
        Duration::from_secs_f64(BATCH as f64 / sample_rate).max(MIN_PERIOD)
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
        let offline = cfg.mode == RunMode::Offline;
        let (rate, frames) = (cfg.sample_rate, cfg.frames);
        let reply = self.link.call(
            "Activate",
            self.timeouts().create,
            |id| ToChild::Activate {
                id,
                sample_rate: rate,
                frames,
                offline,
            },
            None,
        )?;
        let FromChild::Activated { latency, .. } = reply.msg else {
            return Err(self.link.unexpected("Activate", &reply.msg));
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
        Ok(Box::new(RemoteProcessor::new(
            port,
            Arc::clone(&self.port),
            wait_all,
            Self::period(cfg.sample_rate),
            self.timeouts().watchdog,
        )))
    }

    fn deactivate(&mut self, processor: Box<dyn PluginProcessor>) {
        // Dropping the processor puts the port back; the child's `Deactivate` then runs with
        // nothing in flight.
        drop(processor);
        self.offline_active = false;
        let call = self.timeouts().call;
        // A failure is a lost child, which the link has already recorded.
        let _ = self
            .link
            .call("Deactivate", call, |id| ToChild::Deactivate { id }, None);
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        let reply = self.link.call(
            "CaptureState",
            self.timeouts().create,
            |id| ToChild::CaptureState { id },
            None,
        )?;
        match (&reply.msg, reply.blob) {
            (FromChild::State { .. }, Some(blob)) => Ok(PluginStateBlob(blob)),
            (FromChild::State { .. }, None) => Err(unexpected_none("CaptureState")),
            (other, _) => Err(self.link.unexpected("CaptureState", other)),
        }
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        let reply = self.link.call(
            "RestoreState",
            self.timeouts().create,
            |id| ToChild::RestoreState { id },
            Some(&state.0),
        )?;
        if Self::is_ok(&reply) {
            Ok(())
        } else {
            Err(self.link.unexpected("RestoreState", &reply.msg))
        }
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        if !plain.is_finite() {
            return None;
        }
        let reply = self
            .link
            .call(
                "ParamText",
                self.timeouts().text,
                |rid| ToChild::ParamText {
                    id: rid,
                    param: id,
                    plain,
                },
                None,
            )
            .ok()?;
        match reply.msg {
            FromChild::Text { text, .. } => text,
            _ => None,
        }
    }

    fn open_gui(&mut self, _parent: NativeParent, _scale: f64) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "a sandboxed plugin's editor is not embedded".to_string(),
        ))
    }

    fn close_gui(&mut self) {
        let call = self.timeouts().call;
        let _ = self
            .link
            .call("CloseGui", call, |id| ToChild::CloseGui { id }, None);
    }

    fn poll(&mut self) {
        // The reader thread does the work; the main thread only finishes what the audio side
        // could not do itself (it may not lock): ending a child it declared dead.
        self.link.shared().finish_audio_death();
        if self.offline_active && !self.link.is_lost() {
            // The child forwards notifications on a timer; a bounce must not depend on it. The
            // reader posts every notification before it delivers a reply, so when the Flush is
            // answered they are all in the messenger.
            let call = self.timeouts().call;
            let _ = self
                .link
                .call("Flush", call, |id| ToChild::Flush { id }, None);
        }
    }

    fn has_gui(&self) -> bool {
        self.has_gui
    }

    fn default_pins(&mut self) -> Vec<u32> {
        let call = self.timeouts().call;
        match self
            .link
            .call("DefaultPins", call, |id| ToChild::DefaultPins { id }, None)
        {
            Ok(Reply {
                msg: FromChild::Pins { pins, .. },
                ..
            }) => pins,
            _ => Vec::new(),
        }
    }

    fn take_layout_changed(&mut self) -> bool {
        let call = self.timeouts().call;
        match self.link.call(
            "TakeLayoutChanged",
            call,
            |id| ToChild::TakeLayoutChanged { id },
            None,
        ) {
            Ok(Reply {
                msg:
                    FromChild::LayoutChanged {
                        layout: Some(layout),
                        ..
                    },
                ..
            }) => {
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

    fn open_external_gui(&mut self, owner: Option<NativeParent>) -> Result<(), BackendError> {
        let call = self.timeouts().call;
        let owner = owner.map(WireParent::from);
        let reply = self.link.call(
            "OpenGui",
            call,
            |id| ToChild::OpenGui {
                id,
                owner,
                scale: 1.0,
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
}
