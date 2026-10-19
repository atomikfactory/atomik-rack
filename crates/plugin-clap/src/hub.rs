//! The host side of the CLAP callbacks: what a plugin may call, and where each call
//! goes. Thread-safe callbacks only set atomics or post to the instance's wait-free message
//! ring; the main thread (`ClapInstance::poll`, the manager) acts on them.
//!
//! clack calls the handler for a main-thread extension without checking the calling thread, so
//! every handler here is safe to run on any thread: it touches atomics, a message ring and, for
//! timers, a mutex that only the main thread holds for more than an instant.

use atomik_plugin_host::{HostMessenger, LogLevel, PluginMessage, is_audio_thread, is_main_thread};
use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl};
use clack_extensions::latency::{HostLatency, HostLatencyImpl};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{
    HostNotePorts, HostNotePortsImpl, NoteDialects, NotePortRescanFlags,
};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_extensions::timer::{HostTimer, HostTimerImpl, TimerId};
use clack_host::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The shortest timer period the host grants (CLAP asks hosts to allow about 30 Hz).
const MIN_TIMER_PERIOD_MS: u32 = 10;

/// A timer a plugin registered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timer {
    pub id: u32,
    pub period: Duration,
    pub next_due: Instant,
}

/// State shared by the handlers of one instance and its `ClapInstance` / `ClapProcessor`.
#[derive(Debug)]
pub(crate) struct Hub {
    pub messenger: HostMessenger,
    /// `request_callback`: call the plugin's `on_main_thread`.
    pub callback: AtomicBool,
    /// `request_process` and `params.request_flush`: process although asleep.
    pub wake: AtomicBool,
    /// `params.rescan` with a structural flag: re-read the param table.
    pub rescan: AtomicBool,
    pub timers: Mutex<Vec<Timer>>,
    next_timer: AtomicU32,
}

// Justification: plugin timers are wall-clock by nature (a GUI refresh, a blinking cursor) and
// never reach the audio path, so HI-2's ban on clocks does not apply to this one function.
#[allow(clippy::disallowed_methods)]
pub(crate) fn now() -> Instant {
    Instant::now()
}

impl Hub {
    pub fn new(messenger: HostMessenger) -> Arc<Self> {
        Arc::new(Self {
            messenger,
            callback: AtomicBool::new(false),
            wake: AtomicBool::new(false),
            rescan: AtomicBool::new(false),
            timers: Mutex::new(Vec::new()),
            next_timer: AtomicU32::new(1),
        })
    }

    fn timers(&self) -> std::sync::MutexGuard<'_, Vec<Timer>> {
        self.timers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The timers due at `at`, advanced to their next tick.
    pub fn due_timers(&self, at: Instant) -> Vec<u32> {
        let mut due = Vec::new();
        for t in self.timers().iter_mut() {
            if t.next_due <= at {
                due.push(t.id);
                t.next_due = at + t.period;
            }
        }
        due
    }
}

fn level(severity: LogSeverity) -> LogLevel {
    match severity {
        LogSeverity::Debug => LogLevel::Debug,
        LogSeverity::Info => LogLevel::Info,
        LogSeverity::Warning => LogLevel::Warning,
        _ => LogLevel::Error,
    }
}

/// Thread-safe callbacks (`[thread-safe]` in the CLAP headers).
pub(crate) struct SharedHandle {
    pub hub: Arc<Hub>,
}

impl<'a> SharedHandler<'a> for SharedHandle {
    fn request_restart(&self) {
        self.hub.messenger.post(PluginMessage::RequestRestart);
    }

    fn request_process(&self) {
        self.hub.wake.store(true, Ordering::Release);
    }

    fn request_callback(&self) {
        self.hub.callback.store(true, Ordering::Release);
        self.hub.messenger.post(PluginMessage::RequestCallback);
    }
}

impl HostParamsImplShared for SharedHandle {
    fn request_flush(&self) {
        self.hub.wake.store(true, Ordering::Release);
    }
}

impl HostGuiImpl for SharedHandle {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        self.hub.messenger.post(PluginMessage::GuiResize {
            w: new_size.width,
            h: new_size.height,
        });
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        self.hub.messenger.post(PluginMessage::GuiRequestShow);
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        self.hub.messenger.post(PluginMessage::GuiRequestHide);
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.hub.messenger.post(PluginMessage::GuiClosed);
    }
}

impl HostLogImpl for SharedHandle {
    fn log(&self, severity: LogSeverity, message: &str) {
        self.hub.messenger.post(PluginMessage::Log {
            level: level(severity),
            text: message.to_string(),
        });
    }
}

impl HostThreadCheckImpl for SharedHandle {
    fn is_main_thread(&self) -> bool {
        is_main_thread()
    }

    fn is_audio_thread(&self) -> bool {
        is_audio_thread()
    }
}

/// Callbacks that the CLAP headers mark `[main-thread]`.
pub(crate) struct MainHandle {
    pub hub: Arc<Hub>,
}

impl<'a> MainThreadHandler<'a> for MainHandle {}

impl HostParamsImplMainThread for MainHandle {
    fn rescan(&self, flags: ParamRescanFlags) {
        // Values and texts are read on demand; a change of the table itself (names, flags,
        // ranges, new or removed params) needs the layout re-read and the device restarted.
        if flags.intersects(ParamRescanFlags::INFO | ParamRescanFlags::ALL) {
            self.hub.rescan.store(true, Ordering::Release);
            self.hub.messenger.post(PluginMessage::RescanParams);
        }
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostLatencyImpl for MainHandle {
    fn changed(&self) {
        self.hub.messenger.post(PluginMessage::LatencyChanged);
    }
}

impl HostStateImpl for MainHandle {
    fn mark_dirty(&self) {
        self.hub.messenger.post(PluginMessage::StateDirty);
    }
}

impl HostNotePortsImpl for MainHandle {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }

    fn rescan(&self, _flags: NotePortRescanFlags) {}
}

impl HostAudioPortsImpl for MainHandle {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }

    fn rescan(&self, _flags: AudioPortRescanFlags) {}
}

impl HostTimerImpl for MainHandle {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        let id = self.hub.next_timer.fetch_add(1, Ordering::Relaxed);
        let period = Duration::from_millis(u64::from(period_ms.max(MIN_TIMER_PERIOD_MS)));
        self.hub.timers().push(Timer {
            id,
            period,
            next_due: now() + period,
        });
        Ok(TimerId(id))
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        let mut timers = self.hub.timers();
        let before = timers.len();
        timers.retain(|t| t.id != timer_id.0);
        if timers.len() < before {
            Ok(())
        } else {
            Err(HostError::Message("unknown timer id"))
        }
    }
}

/// Audio-thread callbacks: the host has none to offer.
pub(crate) struct AudioHandle;

impl AudioProcessorHandler<'_> for AudioHandle {}

/// The handler set of a CLAP instance.
pub(crate) struct ClapHost;

impl HostHandlers for ClapHost {
    type Shared<'a> = SharedHandle;
    type MainThread<'a> = MainHandle;
    type AudioProcessor<'a> = AudioHandle;

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &SharedHandle) {
        builder
            .register::<HostLog>()
            .register::<HostGui>()
            .register::<HostThreadCheck>()
            .register::<HostParams>()
            .register::<HostLatency>()
            .register::<HostState>()
            .register::<HostNotePorts>()
            .register::<HostAudioPorts>()
            .register::<HostTimer>();
    }
}
