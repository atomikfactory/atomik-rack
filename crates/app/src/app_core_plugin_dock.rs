//! Plugin editors inside the app window.
//!
//! A sandboxed plugin's editor is a window of the plugin's own process. Where the system allows
//! (Windows and X11: a window of one process can be a child of a window of another; macOS and
//! Wayland cannot) it can instead sit **inside** the app window, in a frame the app draws: a
//! title bar with the plugin's name, the Learn buttons, POP OUT and CLOSE, above the area the
//! plugin's window fills.
//!
//! The core never touches a window. It asks the platform layer for the frame
//! ([`DockRequest::Create`]); the platform makes a native window for the area and answers with
//! [`AppCore::plugin_dock_created`]; the core then asks the plugin's process to open its editor
//! as a child of that window. From then on the sizes meet in the middle: the plugin asks for a
//! size ([`ManagerEvent::GuiResize`], which becomes [`DockRequest::Resize`]), and the user
//! dragging the frame's grip asks the plugin through [`AppCore::plugin_dock_user_resized`],
//! which answers with the size it takes (a fixed-size editor keeps its own).
//!
//! **Where an editor opens** is [`atomik_plugin_host::EditorHome`] (Settings > Plugins) unless
//! the user moved that editor with its DOCK IN APP and POP OUT buttons: the choice is kept per
//! plugin device for as long as the song is open (view state, not in the project). Where the
//! system cannot dock ([`AppCore::set_plugin_docking_available`] was not called with true) every
//! editor opens in its own window, whatever the setting says.
//!
//! A plugin whose process ends leaves its frame open saying **The plugin stopped**
//! ([`DockRequest::Stopped`]); CLOSE removes it.

use super::*;
use atomik_plugin_host::EditorHome;
use atomik_ui::{PluginDockRequest, PluginDockUi};

/// Where an editor is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorPlacement {
    /// In a window of the plugin's own process.
    OwnWindow,
    /// Inside the app window.
    Docked,
}

/// A request for the platform layer about the frame of a docked editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockRequest {
    /// Make the frame and its area, then call [`AppCore::plugin_dock_created`] with the area's
    /// native handle (or [`AppCore::plugin_dock_failed`]).
    Create {
        /// The plugin device (path key).
        device: DeviceKey,
        /// The size of the area in pixels (the last size of this editor, or a default).
        size: (u32, u32),
        /// The frame's top-left corner inside the app window in pixels, when it was placed.
        at: Option<(i32, i32)>,
    },
    /// Give the area this size in pixels (the plugin asked, or took the size the user dragged).
    Resize {
        /// The plugin device.
        device: DeviceKey,
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// The plugin stopped: the frame says so instead of showing a dead area.
    Stopped {
        /// The plugin device.
        device: DeviceKey,
    },
    /// Remove the frame.
    Close {
        /// The plugin device.
        device: DeviceKey,
    },
}

/// The size an area has before the plugin says what it wants.
const DEFAULT_AREA: (u32, u32) = (640, 480);

/// The frame of one docked editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    /// The platform was asked for the frame and has not answered.
    waiting: bool,
    size: (u32, u32),
    stopped: bool,
}

/// What the platform draws for a docked editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockedEditor {
    /// The size of the area in pixels.
    pub size: (u32, u32),
    /// The plugin's process ended.
    pub stopped: bool,
}

/// The docking state of the open song's plugin editors.
#[derive(Debug, Default)]
pub(super) struct PluginDock {
    /// The system can show an editor inside the app window.
    available: bool,
    /// What the user chose per device with the buttons on the editors.
    choice: BTreeMap<DeviceKey, EditorPlacement>,
    frames: BTreeMap<DeviceKey, Frame>,
    /// The last place of each frame, for the next time it opens.
    places: BTreeMap<DeviceKey, (i32, i32)>,
    /// The last size of each docked editor.
    sizes: BTreeMap<DeviceKey, (u32, u32)>,
    requests: Vec<DockRequest>,
}

impl<S: EngineSink> AppCore<S> {
    /// The platform says whether it can show editors inside the app window.
    pub fn set_plugin_docking_available(&mut self, available: bool) {
        self.plugins.dock.available = available;
    }

    /// True when editors can be shown inside the app window here.
    pub fn plugin_docking_available(&self) -> bool {
        self.plugins.dock.available
    }

    /// Where the editor of `device` opens next: what the user chose for it, else the setting;
    /// always its own window where the system cannot dock.
    pub fn editor_placement(&self, device: &DeviceKey) -> EditorPlacement {
        if !self.plugins.dock.available {
            return EditorPlacement::OwnWindow;
        }
        self.plugins.dock.choice.get(device).copied().unwrap_or(
            match self.plugins.settings.editors {
                EditorHome::OwnWindow => EditorPlacement::OwnWindow,
                EditorHome::InApp => EditorPlacement::Docked,
            },
        )
    }

    /// The requests for the platform about docked frames, in order.
    pub fn take_plugin_dock_requests(&mut self) -> Vec<DockRequest> {
        std::mem::take(&mut self.plugins.dock.requests)
    }

    /// The docked editor of `device`, once its frame exists.
    pub fn docked_editor(&self, device: &DeviceKey) -> Option<DockedEditor> {
        self.plugins
            .dock
            .frames
            .get(device)
            .filter(|f| !f.waiting)
            .map(|f| DockedEditor {
                size: f.size,
                stopped: f.stopped,
            })
    }

    /// True when the editor of `device` sits (or is about to sit) inside the app window.
    pub fn plugin_editor_docked(&self, device: &DeviceKey) -> bool {
        self.plugins.dock.frames.contains_key(device)
    }

    /// What the docking buttons of the editor of `device` offer.
    pub fn plugin_dock_ui(&self, device: &DeviceKey) -> PluginDockUi {
        match self.plugins.dock.frames.get(device) {
            Some(f) if f.stopped => PluginDockUi::Stopped,
            Some(_) => PluginDockUi::Docked,
            // Only the editors of a plugin's own process can move into the app window.
            None if self.plugins.dock.available
                && matches!(
                    self.plugins.windows.get(device),
                    Some(EditorWindow::External | EditorWindow::ExternalOpening)
                ) =>
            {
                PluginDockUi::CanDock
            }
            None => PluginDockUi::Hidden,
        }
    }

    /// OPEN GUI of a sandboxed editor that goes inside the app window: asks the platform for the
    /// frame; [`Self::plugin_dock_created`] opens the editor in it.
    pub(super) fn open_docked_editor(&mut self, device: &DeviceKey) {
        if let Some(frame) = self.plugins.dock.frames.get(device) {
            // Asking again raises it: nothing to do while it opens, the frame stays put.
            if frame.waiting
                || self.plugins.windows.get(device) == Some(&EditorWindow::ExternalOpening)
            {
                self.reject(EDITOR_WAITING.to_string());
            }
            return;
        }
        let size = self
            .plugins
            .dock
            .sizes
            .get(device)
            .copied()
            .unwrap_or(DEFAULT_AREA);
        self.plugins.dock.frames.insert(
            device.clone(),
            Frame {
                waiting: true,
                size,
                stopped: false,
            },
        );
        self.plugins
            .windows
            .insert(device.clone(), EditorWindow::ExternalOpening);
        let at = self.plugins.dock.places.get(device).copied();
        self.plugins.dock.requests.push(DockRequest::Create {
            device: device.clone(),
            size,
            at,
        });
    }

    /// The platform made the frame and its area: the plugin's process opens its editor inside it.
    pub fn plugin_dock_created(&mut self, device: &DeviceKey, parent: NativeParent) {
        let waiting = self
            .plugins
            .dock
            .frames
            .get(device)
            .is_some_and(|f| f.waiting);
        if !waiting {
            return;
        }
        let Some(id) = self.plugins.instances.get(device).copied() else {
            self.plugin_dock_failed(device, "the plugin is not loaded");
            return;
        };
        match self
            .plugins
            .host
            .manager()
            .begin_open_embedded_editor(id, parent)
        {
            Ok(open) => {
                if let Some(f) = self.plugins.dock.frames.get_mut(device) {
                    f.waiting = false;
                }
                let state = if open {
                    EditorWindow::External
                } else {
                    EditorWindow::ExternalOpening
                };
                self.plugins.windows.insert(device.clone(), state);
                if open {
                    log::info!("{}", editor_opened_line(&device.0, id));
                }
            }
            Err(e) => self.plugin_dock_failed(device, &e.to_string()),
        }
    }

    /// The platform could not make the frame (or the editor could not open in it): the editor
    /// is not open and the frame, if any, goes.
    pub fn plugin_dock_failed(&mut self, device: &DeviceKey, reason: &str) {
        if self.plugins.dock.frames.remove(device).is_some() {
            self.plugins.dock.requests.push(DockRequest::Close {
                device: device.clone(),
            });
        }
        self.plugins.windows.remove(device);
        self.reject(format!(
            "cannot open the plugin editor inside the app window: {reason}"
        ));
    }

    /// The plugin asked for another size of its editor: the frame follows.
    pub(super) fn plugin_dock_plugin_resized(&mut self, device: &DeviceKey, w: u32, h: u32) {
        if let Some(f) = self.plugins.dock.frames.get_mut(device) {
            f.size = (w, h);
            self.plugins.dock.sizes.insert(device.clone(), (w, h));
            self.plugins.dock.requests.push(DockRequest::Resize {
                device: device.clone(),
                w,
                h,
            });
        }
    }

    /// The user dragged the frame to make its area `w` x `h`: the plugin answers with the size it
    /// takes and the frame follows it (a fixed-size editor makes the frame spring back).
    pub fn plugin_dock_user_resized(&mut self, device: &DeviceKey, w: u32, h: u32) {
        let Some(frame) = self.plugins.dock.frames.get(device).copied() else {
            return;
        };
        if frame.waiting || frame.stopped {
            return;
        }
        let Some(id) = self.plugins.instances.get(device).copied() else {
            return;
        };
        let (aw, ah) = match self.plugins.host.manager().request_resize_gui(id, w, h) {
            Ok(Some(took)) => took,
            // The plugin is busy: the frame follows the drag, and the size the plugin takes is
            // applied when it answers (see `plugin_gui_sized`).
            Ok(None) => return,
            Err(_) => frame.size,
        };
        self.plugin_dock_plugin_resized(device, aw, ah);
    }

    /// A resize that was still out when the user stopped dragging was answered by a plugin that
    /// had been busy: the frame or the window follows the size it took.
    pub(super) fn plugin_gui_sized(
        &mut self,
        device: &DeviceKey,
        asked: (u32, u32),
        took: Option<(u32, u32)>,
    ) {
        if let Some(frame) = self.plugins.dock.frames.get(device).copied() {
            let (w, h) = took.unwrap_or(frame.size);
            self.plugin_dock_plugin_resized(device, w, h);
        } else if self.plugins.windows.get(device) == Some(&EditorWindow::Open) {
            self.plugin_window_sized(device, asked, took);
        }
    }

    /// The user moved the frame: its top-left corner inside the app window, in pixels.
    pub fn plugin_dock_moved(&mut self, device: &DeviceKey, x: i32, y: i32) {
        if self.plugins.dock.frames.contains_key(device) {
            self.plugins.dock.places.insert(device.clone(), (x, y));
        }
    }

    /// The editor of `device` finished opening inside its frame.
    pub(super) fn plugin_dock_opened(&mut self, device: &DeviceKey) {
        if let Some(f) = self.plugins.dock.frames.get_mut(device) {
            f.waiting = false;
        }
    }

    /// The plugin's process ended: its frame says so.
    pub(super) fn plugin_dock_stopped(&mut self, device: &DeviceKey) {
        if let Some(f) = self.plugins.dock.frames.get_mut(device)
            && !f.stopped
        {
            f.stopped = true;
            f.waiting = false;
            self.plugins.dock.requests.push(DockRequest::Stopped {
                device: device.clone(),
            });
        }
    }

    /// The editor of `device` closed: its frame goes.
    pub(super) fn plugin_dock_closed(&mut self, device: &DeviceKey) {
        if self.plugins.dock.frames.remove(device).is_some() {
            self.plugins.dock.requests.push(DockRequest::Close {
                device: device.clone(),
            });
        }
    }

    /// DOCK IN APP and POP OUT: moves the editor of `device` to `placement`, keeping the choice.
    /// An editor that is open is closed (its state kept) and opened again where it was asked to
    /// go; one that is not open only remembers the choice.
    pub fn move_plugin_editor(&mut self, device: &DeviceKey, placement: EditorPlacement) {
        if placement == EditorPlacement::Docked && !self.plugins.dock.available {
            self.reject(
                "editors cannot be shown inside the app window on this system (macOS and Wayland)"
                    .to_string(),
            );
            return;
        }
        self.plugins.dock.choice.insert(device.clone(), placement);
        if self.plugins.windows.contains_key(device) {
            self.close_plugin_gui_inner(device, true);
            self.open_plugin_gui(device);
        }
    }

    /// A button of the docking part of an editor's strip.
    pub(super) fn plugin_dock_request(&mut self, device: &DeviceKey, request: PluginDockRequest) {
        match request {
            PluginDockRequest::Dock => self.move_plugin_editor(device, EditorPlacement::Docked),
            PluginDockRequest::PopOut => {
                self.move_plugin_editor(device, EditorPlacement::OwnWindow)
            }
            PluginDockRequest::Close => self.close_plugin_gui(device),
        }
    }

    /// Settings > Plugins: where editors open.
    pub(super) fn set_editor_home(&mut self, home: EditorHome) {
        if self.plugins.settings.editors != home {
            self.plugins.settings.editors = home;
            self.plugins.settings_changed = true;
        }
    }
}
