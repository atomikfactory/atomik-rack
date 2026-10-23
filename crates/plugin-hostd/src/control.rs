//! The control channel of the child: what the main thread does with each request.
//!
//! One [`Control`] owns the plugin instance and writes every frame to the parent. It is driven
//! by events (a frame from the parent, a tick, the end of stdin, a window event) and answers each
//! request in order. Errors become `Err` frames; they never end the child.

use crate::args::{Args, Format};
use crate::audio::{AudioThread, HANDOVER_TIMEOUT, SHUTDOWN_TIMEOUT};
use crate::builtin::BuiltinBackend;
use crate::editor::{Editor, WindowSystem};
use crate::notify::to_wire;
use atomik_device_sdk::{PluginFormat, PluginRef, PluginStateBlob, RunMode};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::{
    ActivateConfig, BackendError, HostHandle, InstanceId, ManagerEnd, PluginBackend,
    PluginInstance, PluginMessage, plugin_channel,
};
use atomik_plugin_ipc::{
    FromChild, PROTOCOL_VERSION, ToChild, WireErrorKind, WireLayout, write_frame,
};
use atomik_plugin_vst3::Vst3Backend;
use std::io::Write;

/// The exit code for a protocol or setup mismatch.
pub const EXIT_MISMATCH: u8 = 3;

/// What happened that the child has to react to.
#[derive(Debug)]
pub enum Event {
    /// A request from the parent, with its blob.
    Frame(Box<(ToChild, Option<Vec<u8>>)>),
    /// Stdin ended: the parent is gone or closed the channel.
    Eof,
    /// Time to poll the plugin and forward its messages.
    Tick,
}

/// What the user did to the editor window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowInput {
    /// The user closed the window.
    CloseRequested,
    /// The user resized the window; the size is in the plugin's units.
    Resized(u32, u32),
}

/// The plugin backend of a command line.
pub fn make_backend(args: &Args) -> Box<dyn PluginBackend> {
    match args.format {
        Format::Null => Box::new(BuiltinBackend),
        Format::Clap => {
            let backend = ClapBackend::new();
            backend.register_plugin_file(&args.plugin_id, &args.plugin_file);
            Box::new(backend)
        }
        Format::Vst3 => {
            let backend = Vst3Backend::new();
            backend.register_plugin_file(&args.plugin_id, &args.plugin_file);
            Box::new(backend)
        }
    }
}

/// The main-thread half of the child.
pub struct Control {
    plugin: PluginRef,
    backend: Box<dyn PluginBackend>,
    out: Box<dyn Write>,
    audio: AudioThread,
    greeted: bool,
    instance: Option<Box<dyn PluginInstance>>,
    /// The instance is activated: its processor lives on the audio thread.
    active: bool,
    messages: Option<ManagerEnd>,
    editor: Editor,
}

impl Control {
    /// A control that writes frames to `out`.
    pub fn new(
        args: &Args,
        backend: Box<dyn PluginBackend>,
        audio: AudioThread,
        out: Box<dyn Write>,
    ) -> Self {
        Self {
            plugin: PluginRef {
                format: match args.format {
                    Format::Vst3 => PluginFormat::Vst3,
                    Format::Clap | Format::Null => PluginFormat::Clap,
                },
                id: args.plugin_id.clone(),
            },
            backend,
            out,
            audio,
            greeted: false,
            instance: None,
            active: false,
            messages: None,
            editor: Editor::default(),
        }
    }

    /// Writes the child's Hello. False when the parent is gone.
    pub fn greet(&mut self) -> bool {
        let hello = FromChild::Hello {
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
        };
        write_frame(&mut self.out, &hello, None).is_ok()
    }

    /// Reacts to one event; `Some(code)` ends the child with that exit code.
    pub fn on_event(&mut self, event: Event, windows: &mut dyn WindowSystem) -> Option<u8> {
        match event {
            Event::Eof => {
                self.shutdown(windows);
                Some(0)
            }
            Event::Tick => self.tick(windows),
            Event::Frame(frame) => {
                let (msg, blob) = *frame;
                self.on_frame(msg, blob, windows)
            }
        }
    }

    /// Reacts to what the user did to the editor window.
    pub fn on_window(&mut self, input: WindowInput, windows: &mut dyn WindowSystem) -> Option<u8> {
        let instance = self.instance.as_deref_mut()?;
        match input {
            WindowInput::CloseRequested => {
                if self.editor.close(instance, windows) {
                    return self.notify(&FromChild::GuiClosed, windows);
                }
            }
            WindowInput::Resized(w, h) => self.editor.user_resized(instance, windows, w, h),
        }
        None
    }

    /// Polls the plugin and forwards what it posted since the last tick.
    fn tick(&mut self, windows: &mut dyn WindowSystem) -> Option<u8> {
        if let Some(instance) = self.instance.as_mut() {
            instance.poll();
        }
        self.forward(windows)
    }

    /// Writes every notification the plugin has posted so far, in the order it posted them. Done
    /// before each reply and on `Flush`, so what a call made the plugin post reaches the parent
    /// before the answer, and never waits for a timer (HI-2).
    fn forward(&mut self, windows: &mut dyn WindowSystem) -> Option<u8> {
        while let Some(message) = self.messages.as_ref().and_then(ManagerEnd::try_recv) {
            if let Some(instance) = self.instance.as_deref_mut() {
                match &message {
                    PluginMessage::GuiResize { w, h } => {
                        self.editor.plugin_resized(windows, *w, *h);
                    }
                    PluginMessage::GuiRequestShow => self.editor.plugin_visible(windows, true),
                    PluginMessage::GuiRequestHide => self.editor.plugin_visible(windows, false),
                    PluginMessage::GuiClosed => {
                        self.editor.close(instance, windows);
                    }
                    _ => {}
                }
            }
            if let Some(wire) = to_wire(message)
                && let Some(code) = self.notify(&wire, windows)
            {
                return Some(code);
            }
        }
        None
    }

    fn notify(&mut self, msg: &FromChild, windows: &mut dyn WindowSystem) -> Option<u8> {
        if write_frame(&mut self.out, msg, None).is_ok() {
            None
        } else {
            self.shutdown(windows);
            Some(0)
        }
    }

    fn on_frame(
        &mut self,
        msg: ToChild,
        blob: Option<Vec<u8>>,
        windows: &mut dyn WindowSystem,
    ) -> Option<u8> {
        if !self.greeted {
            return match msg {
                ToChild::Hello { protocol } if protocol == PROTOCOL_VERSION => {
                    self.greeted = true;
                    None
                }
                ToChild::Hello { protocol } => {
                    eprintln!(
                        "atomik-plugin-host: the parent speaks protocol {protocol}, this child speaks {PROTOCOL_VERSION}"
                    );
                    Some(EXIT_MISMATCH)
                }
                _ => {
                    eprintln!("atomik-plugin-host: the first message must be Hello");
                    Some(EXIT_MISMATCH)
                }
            };
        }
        match msg {
            ToChild::Hello { .. } => None,
            ToChild::Shutdown => {
                self.shutdown(windows);
                Some(0)
            }
            ToChild::Instantiate { id } => {
                let reply = self.instantiate(id).unwrap_or_else(|e| err(id, &e));
                self.reply(&reply, None, windows)
            }
            request => {
                let id = request_id(&request);
                let (reply, blob) = self
                    .request(request, blob, windows)
                    .unwrap_or_else(|e| (err(id, &e), None));
                self.reply(&reply, blob.as_deref(), windows)
            }
        }
    }

    /// The requests that need an instance: the answer, with a blob when it carries one.
    fn request(
        &mut self,
        msg: ToChild,
        blob: Option<Vec<u8>>,
        windows: &mut dyn WindowSystem,
    ) -> Result<(FromChild, Option<Vec<u8>>), BackendError> {
        let Some(instance) = self.instance.as_mut() else {
            return Err(failed(
                "the plugin is not instantiated yet (send Instantiate first)",
            ));
        };
        let answer = match msg {
            ToChild::Activate {
                id,
                sample_rate,
                frames,
                offline,
            } => {
                if self.active {
                    return Err(failed("the plugin is already active"));
                }
                if !(sample_rate.is_finite() && sample_rate > 0.0) {
                    return Err(failed("the sample rate is not a positive number"));
                }
                let mode = if offline {
                    RunMode::Offline
                } else {
                    RunMode::Realtime
                };
                let processor = instance.activate(&ActivateConfig {
                    sample_rate,
                    frames,
                    mode,
                })?;
                if !self.audio.install(processor) {
                    return Err(failed("the audio thread did not take the processor"));
                }
                self.active = true;
                FromChild::Activated {
                    id,
                    latency: instance.latency_frames(),
                }
            }
            ToChild::Deactivate { id } => {
                if !self.active {
                    return Err(failed("the plugin is not active"));
                }
                let Some(processor) = self.audio.take(HANDOVER_TIMEOUT) else {
                    return Err(failed("the audio thread did not hand the processor back"));
                };
                instance.deactivate(processor);
                self.active = false;
                FromChild::Ok { id }
            }
            ToChild::CaptureState { id } => {
                let state = instance.capture_state()?;
                return Ok((FromChild::State { id }, Some(state.0)));
            }
            ToChild::RestoreState { id } => {
                let blob = blob.ok_or_else(|| failed("RestoreState came without a state"))?;
                instance.restore_state(&PluginStateBlob(blob))?;
                FromChild::Ok { id }
            }
            ToChild::ParamText { id, param, plain } => FromChild::Text {
                id,
                text: instance.param_text(param, plain),
            },
            ToChild::LatencyFrames { id } => FromChild::Latency {
                id,
                frames: instance.latency_frames(),
            },
            ToChild::DefaultPins { id } => FromChild::Pins {
                id,
                pins: instance.default_pins(),
            },
            ToChild::TakeLayoutChanged { id } => FromChild::LayoutChanged {
                id,
                layout: instance
                    .take_layout_changed()
                    .then(|| WireLayout::from(instance.layout())),
            },
            ToChild::OpenGui { id, owner, scale } => {
                let title = instance.layout().name.clone();
                let (w, h) = self.editor.open(
                    instance.as_mut(),
                    windows,
                    &title,
                    owner.map(Into::into),
                    scale,
                )?;
                FromChild::GuiOpened { id, w, h }
            }
            ToChild::Flush { id } => {
                // The same order as the manager's poll in process: the plugin's main-thread
                // work first, then what it posted. The notifications go out before the `Ok`.
                instance.poll();
                FromChild::Ok { id }
            }
            ToChild::CloseGui { id } => {
                self.editor.close(instance.as_mut(), windows);
                FromChild::Ok { id }
            }
            ToChild::Hello { .. } | ToChild::Shutdown | ToChild::Instantiate { .. } => {
                return Err(failed("not a request for an instance"));
            }
        };
        Ok((answer, None))
    }

    fn reply(
        &mut self,
        msg: &FromChild,
        blob: Option<&[u8]>,
        windows: &mut dyn WindowSystem,
    ) -> Option<u8> {
        if let Some(code) = self.forward(windows) {
            return Some(code);
        }
        if write_frame(&mut self.out, msg, blob).is_ok() {
            None
        } else {
            self.shutdown(windows);
            Some(0)
        }
    }

    fn instantiate(&mut self, id: u64) -> Result<FromChild, BackendError> {
        if self.instance.is_some() {
            return Err(failed("the plugin is already instantiated"));
        }
        let (_device_end, manager) = plugin_channel(InstanceId(id));
        let host = HostHandle::new(manager.messenger());
        let instance = self.backend.instantiate(&self.plugin, host)?;
        let reply = FromChild::Instantiated {
            id,
            layout: WireLayout::from(instance.layout()),
            has_gui: instance.has_gui(),
            latency: instance.latency_frames(),
        };
        self.instance = Some(instance);
        self.messages = Some(manager);
        Ok(reply)
    }

    /// Ends the plugin: it is deactivated if it is active and the instance is dropped. When the
    /// audio thread cannot hand the processor back (it is stuck inside the plugin) the instance
    /// is leaked rather than destroyed under a running processor; the process ends right after.
    pub fn shutdown(&mut self, windows: &mut dyn WindowSystem) {
        if let Some(mut instance) = self.instance.take() {
            self.editor.close(instance.as_mut(), windows);
            if self.active {
                self.active = false;
                match self.audio.take(SHUTDOWN_TIMEOUT) {
                    Some(processor) => instance.deactivate(processor),
                    None => {
                        std::mem::forget(instance);
                        self.messages = None;
                        return;
                    }
                }
            }
            drop(instance);
        }
        self.messages = None;
    }
}

fn failed(text: &str) -> BackendError {
    BackendError::Failed(text.to_string())
}

fn err(id: u64, e: &BackendError) -> FromChild {
    let (kind, text) = WireErrorKind::of(e);
    FromChild::Err { id, kind, text }
}

/// The id of a request (0 for `Hello` and `Shutdown`, which have none).
fn request_id(msg: &ToChild) -> u64 {
    match msg {
        ToChild::Instantiate { id }
        | ToChild::Activate { id, .. }
        | ToChild::Deactivate { id }
        | ToChild::CaptureState { id }
        | ToChild::RestoreState { id }
        | ToChild::ParamText { id, .. }
        | ToChild::LatencyFrames { id }
        | ToChild::DefaultPins { id }
        | ToChild::TakeLayoutChanged { id }
        | ToChild::OpenGui { id, .. }
        | ToChild::CloseGui { id }
        | ToChild::Flush { id } => *id,
        ToChild::Hello { .. } | ToChild::Shutdown => 0,
    }
}

#[cfg(test)]
mod tests;
