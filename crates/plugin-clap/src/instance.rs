//! `ClapInstance`: the main-thread half of one CLAP plugin.
//! Every method here is `[main-thread]`; each starts with `assert_main_thread`.

use crate::hub::{AudioHandle, ClapHost, Hub, MainHandle, SharedHandle, now};
use crate::layout::{self, AudioPlan, DescInfo, NotePlan, Read};
use crate::processor::ClapProcessor;
use atomik_device_sdk::{PluginLayout, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, NativeParent, PluginInstance as HostInstance, PluginProcessor,
    assert_main_thread,
};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui, Window};
use clack_extensions::latency::PluginLatency;
use clack_extensions::params::PluginParams;
use clack_extensions::remote_controls::{PluginRemoteControls, RemoteControlsPageBuffer};
use clack_extensions::render::{PluginRender, RenderMode};
use clack_extensions::state::PluginState;
use clack_extensions::tail::PluginTail;
use clack_extensions::timer::{PluginTimer, TimerId};
use clack_host::prelude::*;
use std::ffi::{CStr, c_void};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The `PluginInstance` of a CLAP plugin.
pub(crate) struct ClapInstance {
    plugin: PluginInstance<ClapHost>,
    hub: Arc<Hub>,
    desc: DescInfo,
    layout: PluginLayout,
    audio: AudioPlan,
    notes: NotePlan,
    latency: u32,
    layout_changed: bool,
    params_truncated: bool,
    gui_open: bool,
    rendering: Option<RunMode>,
}

fn failed(what: &str, e: impl std::fmt::Display) -> BackendError {
    BackendError::Failed(format!("{what}: {e}"))
}

impl ClapInstance {
    /// Creates (and initialises) the plugin `id` of `entry`.
    pub fn create(
        entry: &PluginEntry,
        info: &HostInfo,
        id: &CStr,
        desc: DescInfo,
        hub: Arc<Hub>,
    ) -> Result<Self, BackendError> {
        assert_main_thread("creating a CLAP plugin");
        let shared_hub = Arc::clone(&hub);
        let main_hub = Arc::clone(&hub);
        let mut plugin = PluginInstance::<ClapHost>::new(
            move |_| SharedHandle { hub: shared_hub },
            move |_| MainHandle { hub: main_hub },
            entry,
            id,
            info,
        )
        .map_err(|e| failed("the plugin could not be created", e))?;
        let Read {
            layout,
            audio,
            notes,
            params_truncated,
        } = layout::read(&mut plugin, &desc).map_err(BackendError::Unsupported)?;
        Ok(Self {
            plugin,
            hub,
            desc,
            layout,
            audio,
            notes,
            latency: 0,
            layout_changed: false,
            params_truncated,
            gui_open: false,
            rendering: None,
        })
    }

    /// The scan facts of this instance.
    pub fn read(&self) -> (PluginLayout, bool) {
        (self.layout.clone(), self.params_truncated)
    }

    fn refresh_layout(&mut self) {
        if let Ok(read) = layout::read(&mut self.plugin, &self.desc) {
            if read.layout != self.layout {
                self.layout_changed = true;
            }
            self.layout = read.layout;
            self.params_truncated = read.params_truncated;
            self.audio = read.audio;
            self.notes = read.notes;
        }
    }

    fn set_render_mode(&mut self, mode: RunMode) {
        if self.rendering == Some(mode) {
            return;
        }
        let handle = self.plugin.plugin_handle();
        if let Some(render) = handle.get_extension::<PluginRender>() {
            let m = match mode {
                RunMode::Realtime => RenderMode::Realtime,
                RunMode::Offline => RenderMode::Offline,
            };
            let _ = render.set(&handle, m);
        }
        self.rendering = Some(mode);
    }

    fn read_latency(&mut self) {
        let handle = self.plugin.plugin_handle();
        self.latency = handle
            .get_extension::<PluginLatency>()
            .map_or(0, |ext| ext.get(&handle));
    }
}

impl HostInstance for ClapInstance {
    fn layout(&self) -> &PluginLayout {
        &self.layout
    }

    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError> {
        assert_main_thread("activating a CLAP plugin");
        // A structural rescan while inactive takes effect now.
        if self.hub.rescan.swap(false, Ordering::AcqRel) {
            self.refresh_layout();
        }
        self.set_render_mode(cfg.mode);
        let config = PluginAudioConfiguration {
            sample_rate: cfg.sample_rate,
            min_frames_count: cfg.frames,
            max_frames_count: cfg.frames,
        };
        let stopped = self
            .plugin
            .activate(|_, _| AudioHandle, config)
            .map_err(|e| failed("the plugin could not be activated", e))?;
        self.read_latency();
        let tail_ext: Option<PluginTail> = self.plugin.plugin_handle().get_extension();
        Ok(Box::new(ClapProcessor::new(
            stopped.into(),
            Arc::clone(&self.hub),
            self.audio.clone(),
            self.notes,
            tail_ext,
        )))
    }

    fn deactivate(&mut self, mut processor: Box<dyn PluginProcessor>) {
        assert_main_thread("deactivating a CLAP plugin");
        processor.stop_processing();
        // Dropping the processor releases the last reference clack needs to deactivate.
        drop(processor);
        let _ = self.plugin.try_deactivate();
        // The plugin may have asked for a main-thread callback while deactivating (a table
        // change applied once inactive); run it now so the new layout is known at once.
        self.poll();
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        assert_main_thread("saving the state of a CLAP plugin");
        let handle = self.plugin.plugin_handle();
        let ext = handle
            .get_extension::<PluginState>()
            .ok_or_else(|| BackendError::Unsupported("the plugin has no state".to_string()))?;
        let mut bytes = Vec::new();
        ext.save(&handle, &mut bytes)
            .map_err(|e| failed("saving the state", e))?;
        Ok(PluginStateBlob(bytes))
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        assert_main_thread("loading the state of a CLAP plugin");
        let handle = self.plugin.plugin_handle();
        let ext = handle
            .get_extension::<PluginState>()
            .ok_or_else(|| BackendError::Unsupported("the plugin has no state".to_string()))?;
        let mut reader: &[u8] = &state.0;
        ext.load(&handle, &mut reader)
            .map_err(|e| failed("loading the state", e))?;
        // A state can change the latency; it is read again at the next activation.
        if self.plugin.is_active() {
            self.read_latency();
        }
        Ok(())
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        assert_main_thread("asking a CLAP plugin for a param text");
        let handle = self.plugin.plugin_handle();
        let ext = handle.get_extension::<PluginParams>()?;
        let mut buffer = [0u8; 256];
        let text = ext
            .value_to_text(&handle, ClapId::from_raw(id)?, plain, &mut buffer)
            .ok()?;
        Some(String::from_utf8_lossy(text).into_owned())
    }

    fn open_gui(&mut self, parent: NativeParent, scale: f64) -> Result<(u32, u32), BackendError> {
        assert_main_thread("opening a CLAP plugin editor");
        if self.gui_open {
            return Err(BackendError::Failed(
                "the editor is already open".to_string(),
            ));
        }
        let handle = self.plugin.plugin_handle();
        let ext = handle
            .get_extension::<PluginGui>()
            .ok_or_else(|| BackendError::Unsupported("the plugin has no editor".to_string()))?;
        let (api, window) = match parent {
            NativeParent::Win32(hwnd) => (GuiApiType::WIN32, hwnd as *mut c_void),
            NativeParent::Cocoa(view) => (GuiApiType::COCOA, view as *mut c_void),
            NativeParent::X11(id) => (GuiApiType::X11, id as usize as *mut c_void),
        };
        let config = GuiConfiguration {
            api_type: api,
            is_floating: false,
        };
        if !ext.is_api_supported(&handle, config) {
            return Err(BackendError::Unsupported(
                "the editor cannot be embedded in this window system".to_string(),
            ));
        }
        ext.create(&handle, config)
            .map_err(|e| failed("creating the editor", e))?;
        // Cocoa uses logical sizes and has no `set_scale` (CLAP gui.h).
        if api != GuiApiType::COCOA {
            let _ = ext.set_scale(&handle, scale);
        }
        let size = ext.get_size(&handle);
        // SAFETY: the window handle is the parent the caller created for this editor and keeps
        // alive until `close_gui` (the `open_gui` contract); the plugin only embeds into it.
        let parented = unsafe {
            let window = match parent {
                NativeParent::Win32(_) => Window::from_win32_hwnd(window),
                NativeParent::Cocoa(_) => Window::from_cocoa_nsview(window),
                NativeParent::X11(id) => Window::from_x11_handle(std::ffi::c_ulong::from(id)),
            };
            ext.set_parent(&handle, window)
        };
        if let Err(e) = parented.and_then(|()| ext.show(&handle)) {
            ext.destroy(&handle);
            return Err(failed("showing the editor", e));
        }
        self.gui_open = true;
        let size = size.unwrap_or(GuiSize {
            width: 640,
            height: 480,
        });
        Ok((size.width, size.height))
    }

    fn close_gui(&mut self) {
        assert_main_thread("closing a CLAP plugin editor");
        if !self.gui_open {
            return;
        }
        let handle = self.plugin.plugin_handle();
        if let Some(ext) = handle.get_extension::<PluginGui>() {
            let _ = ext.hide(&handle);
            ext.destroy(&handle);
        }
        self.gui_open = false;
    }

    fn resize_gui(&mut self, w: u32, h: u32) -> Result<(u32, u32), BackendError> {
        assert_main_thread("resizing a CLAP plugin editor");
        if !self.gui_open {
            return Err(BackendError::Failed("the editor is not open".to_string()));
        }
        let handle = self.plugin.plugin_handle();
        let ext = handle
            .get_extension::<PluginGui>()
            .ok_or_else(|| BackendError::Unsupported("the plugin has no editor".to_string()))?;
        if !ext.can_resize(&handle) {
            return Err(BackendError::Unsupported(
                "this plugin's editor cannot be resized".to_string(),
            ));
        }
        let wanted = GuiSize {
            width: w,
            height: h,
        };
        let size = ext.adjust_size(&handle, wanted).unwrap_or(wanted);
        ext.set_size(&handle, size)
            .map_err(|e| failed("resizing the editor", e))?;
        Ok((size.width, size.height))
    }

    fn has_gui(&self) -> bool {
        self.plugin_has_gui()
    }

    fn default_pins(&mut self) -> Vec<u32> {
        assert_main_thread("reading the remote controls of a CLAP plugin");
        let handle = self.plugin.plugin_handle();
        let Some(ext) = handle.get_extension::<PluginRemoteControls>() else {
            return Vec::new();
        };
        if ext.count(&handle) == 0 {
            return Vec::new();
        }
        let mut buffer = RemoteControlsPageBuffer::new();
        ext.get(&handle, 0, &mut buffer)
            .map(|page| page.param_ids.iter().flatten().map(|id| id.get()).collect())
            .unwrap_or_default()
    }

    fn take_layout_changed(&mut self) -> bool {
        std::mem::take(&mut self.layout_changed)
    }

    fn native_handle(&self) -> usize {
        std::ptr::from_ref(self.plugin.raw_instance()) as usize
    }

    fn poll(&mut self) {
        assert_main_thread("polling a CLAP plugin");
        if self.hub.callback.swap(false, Ordering::AcqRel) {
            self.plugin.call_on_main_thread_callback();
        }
        if self.hub.rescan.swap(false, Ordering::AcqRel) {
            self.refresh_layout();
        }
        let due = self.hub.due_timers(now());
        if !due.is_empty() {
            let handle = self.plugin.plugin_handle();
            if let Some(timer) = handle.get_extension::<PluginTimer>() {
                for id in due {
                    timer.on_timer(&handle, TimerId(id));
                }
            }
        }
    }
}

impl ClapInstance {
    fn plugin_has_gui(&self) -> bool {
        self.plugin
            .plugin_shared_handle()
            .get_extension::<PluginGui>()
            .is_some()
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        // The editor must be gone and the plugin deactivated before it is destroyed; clack
        // deactivates an instance that is still active when it is dropped.
        if self.gui_open && is_main() {
            self.close_gui();
        }
    }
}

fn is_main() -> bool {
    atomik_plugin_host::is_main_thread()
}
