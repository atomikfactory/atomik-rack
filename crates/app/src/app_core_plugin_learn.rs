//! Learn in the plugin window: touch a control in a plugin's own editor to show its automation
//! lane or to map it to MIDI.
//!
//! Each plugin editor window has a Learn strip ([`atomik_ui::PluginLearnStrip`]) with two
//! toggles, LEARN AUTOMATION and LEARN MIDI; the parameter pane offers the same two where a window
//! cannot have a strip. The mode is **picked before touching** and is view state only: kept per
//! plugin device, never saved, dropped when the editor closes, when Esc is pressed in the main
//! window, and when the global MIDI Learn mode is switched on (arming a plugin mode switches the
//! global mode off in turn).
//!
//! **The touch.** A gesture begin from the plugin's editor (VST3 `beginEdit`, a CLAP gesture
//! begin) is the touch. A plugin that never sends one is caught by its first change of a param
//! not seen for [`TOUCH_GAP_SECS`]. A touch on a row of the parameter pane
//! ([`atomik_ui::UiAction::ParamTouched`]) counts the same. The edit itself is never swallowed:
//! it goes through as it would without Learn.
//!
//! * **LEARN AUTOMATION** shows the touched param's lane in the arrangement, adding an empty lane
//!   when it has none (the path the "follow the touched param" option takes).
//! * **LEARN MIDI** arms the touched param as the MIDI learn target; the next hardware control
//!   moved is mapped to it (one undo step, the usual learn), and the strip names the source.
//!
//! Both stay armed for the next touch. A control that is not one of the plugin's params says so.

use super::*;
use atomik_model::{MidiSource, MidiTarget};
use atomik_ui::{PluginLearnMode, PluginLearnRequest, PluginLearnState, PluginLearnStatus};

/// A change of a param not seen for this long is a touch, for plugins that send no gesture begin.
pub const TOUCH_GAP_SECS: f64 = 0.5;

/// The Learn mode of one plugin window.
#[derive(Debug, Clone)]
pub(super) struct LearnArm {
    mode: PluginLearnMode,
    status: PluginLearnStatus,
    /// When each param of the plugin last changed (the touch of a plugin without gestures).
    seen: BTreeMap<u32, f64>,
    /// The param key LEARN MIDI armed last, until it is mapped.
    pending: Option<String>,
}

/// The Learn modes of the open plugin windows (none of it is saved).
#[derive(Debug, Default)]
pub(super) struct PluginLearn {
    arms: BTreeMap<DeviceKey, LearnArm>,
    /// The last status of a window whose mode was dropped, until it is armed again.
    last: BTreeMap<DeviceKey, PluginLearnStatus>,
}

impl<S: EngineSink> AppCore<S> {
    /// The armed Learn mode of `device`'s plugin window, if any.
    pub fn plugin_learn_mode(&self, device: &DeviceKey) -> Option<PluginLearnMode> {
        self.plugins.learn.arms.get(device).map(|a| a.mode)
    }

    /// What the Learn strip of `device`'s plugin window shows.
    pub fn plugin_learn_state(&self, device: &DeviceKey) -> PluginLearnState {
        let plugin = self
            .doc()
            .resolve(device)
            .and_then(|d| d.plugin.as_ref())
            .map_or_else(String::new, |p| p.name().to_owned());
        let arm = self.plugins.learn.arms.get(device);
        PluginLearnState {
            device: device.clone(),
            plugin,
            mode: arm.map(|a| a.mode),
            status: arm.map_or_else(
                || {
                    self.plugins
                        .learn
                        .last
                        .get(device)
                        .cloned()
                        .unwrap_or_default()
                },
                |a| a.status.clone(),
            ),
        }
    }

    /// Arms `mode` for `device`'s plugin window, or disarms it when it is the armed one. Arming
    /// switches the global MIDI Learn mode off. False, with a message, for a device that is not a
    /// loaded plugin.
    pub fn toggle_plugin_learn(&mut self, device: &DeviceKey, mode: PluginLearnMode) -> bool {
        if self.plugin_learn_mode(device) == Some(mode) {
            self.disarm_plugin_learn(device);
            return true;
        }
        if !self.plugins.instances.contains_key(device) {
            self.report("Learn from the plugin window needs a loaded plugin device");
            return false;
        }
        // Switching from LEARN MIDI drops the target it armed.
        self.drop_pending_midi(device);
        if self.midi_learn_mode() {
            self.set_midi_learn(false);
        }
        self.plugins.learn.last.remove(device);
        self.plugins.learn.arms.insert(
            device.clone(),
            LearnArm {
                mode,
                status: PluginLearnStatus::Waiting,
                seen: BTreeMap::new(),
                pending: None,
            },
        );
        true
    }

    /// Disarms the Learn mode of `device`'s plugin window (the strip keeps its last message).
    pub fn disarm_plugin_learn(&mut self, device: &DeviceKey) {
        self.drop_pending_midi(device);
        if let Some(arm) = self.plugins.learn.arms.remove(device) {
            self.plugins.learn.last.insert(device.clone(), arm.status);
        }
    }

    /// Disarms every plugin window (Esc in the main window). True when one was armed.
    pub fn disarm_all_plugin_learn(&mut self) -> bool {
        let armed: Vec<DeviceKey> = self.plugins.learn.arms.keys().cloned().collect();
        for device in &armed {
            self.disarm_plugin_learn(device);
        }
        !armed.is_empty()
    }

    /// The strip's buttons.
    pub(super) fn plugin_learn_request(&mut self, device: &DeviceKey, request: PluginLearnRequest) {
        match request {
            PluginLearnRequest::Toggle(mode) => {
                self.toggle_plugin_learn(device, mode);
            }
            PluginLearnRequest::Off => self.disarm_plugin_learn(device),
        }
    }

    /// The commands of the plugin device menu and the palette: the mode of the window the user
    /// last focused, else of the primary selected plugin device; the editor opens when it is
    /// closed.
    pub fn plugin_learn_command(&mut self, mode: PluginLearnMode) {
        let Some(device) = self.plugin_learn_target() else {
            self.report("Select a plugin device first");
            return;
        };
        if self.toggle_plugin_learn(&device, mode)
            && self.plugin_learn_mode(&device).is_some()
            && !self.plugin_gui_open(&device)
            && self
                .plugins
                .instances
                .get(&device)
                .is_some_and(|id| self.plugins.host.manager_ref().has_gui(*id))
        {
            self.open_plugin_gui(&device);
        }
    }

    /// The plugin device the Learn commands work on.
    pub fn plugin_learn_target(&self) -> Option<DeviceKey> {
        self.plugins
            .focused_window
            .clone()
            .filter(|d| self.plugin_gui_open(d))
            .or_else(|| {
                self.view
                    .primary()
                    .filter(|d| self.plugins.instances.contains_key(d))
            })
    }

    /// The user brought `device`'s editor window forward (the Learn commands work on it).
    pub fn plugin_window_focused(&mut self, device: &DeviceKey) {
        self.plugins.focused_window = Some(device.clone());
    }

    /// The editor of `device` closed: its Learn mode goes, and so does its last message.
    pub(super) fn plugin_learn_window_closed(&mut self, device: &DeviceKey) {
        self.disarm_plugin_learn(device);
        self.plugins.learn.last.remove(device);
        if self.plugins.focused_window.as_ref() == Some(device) {
            self.plugins.focused_window = None;
        }
    }

    /// The global MIDI Learn mode was switched on: every plugin window is disarmed.
    pub(crate) fn plugin_learn_global_on(&mut self) {
        let armed: Vec<DeviceKey> = self.plugins.learn.arms.keys().cloned().collect();
        for device in armed {
            if let Some(arm) = self.plugins.learn.arms.remove(&device) {
                self.plugins.learn.last.insert(device, arm.status);
            }
        }
    }

    /// LEARN MIDI of `device` armed a target that is still waiting: it is dropped.
    fn drop_pending_midi(&mut self, device: &DeviceKey) {
        let Some(param) = self
            .plugins
            .learn
            .arms
            .get_mut(device)
            .and_then(|a| a.pending.take())
        else {
            return;
        };
        let target = MidiTarget::Param {
            device: device.clone(),
            param,
        };
        if self.midi_armed() == Some(&target) {
            self.midi_cancel_arm();
        }
    }

    /// A gesture begin of `param` came from `device`'s editor: a touch when Learn is armed.
    pub(super) fn plugin_learn_begin(&mut self, device: &DeviceKey, param: u32) {
        let now = self.plugins.clock;
        let Some(arm) = self.plugins.learn.arms.get_mut(device) else {
            return;
        };
        arm.seen.insert(param, now);
        self.plugin_learn_touch(device, param);
    }

    /// A change of `param` outside a gesture came from `device`'s editor: the touch of a plugin
    /// that sends no gesture begin, when the param was not seen for [`TOUCH_GAP_SECS`].
    pub(super) fn plugin_learn_change(&mut self, device: &DeviceKey, param: u32) {
        let now = self.plugins.clock;
        let Some(arm) = self.plugins.learn.arms.get_mut(device) else {
            return;
        };
        let fresh = arm
            .seen
            .insert(param, now)
            .is_none_or(|at| now - at >= TOUCH_GAP_SECS);
        if fresh {
            self.plugin_learn_touch(device, param);
        }
    }

    /// A row of the parameter pane was pressed: a touch when Learn is armed for its device.
    pub(crate) fn plugin_learn_pane_touch(&mut self, device: &DeviceKey, param: &str) {
        if !self.plugins.learn.arms.contains_key(device) {
            return;
        }
        if let Some(id) = param.strip_prefix('p').and_then(|n| n.parse::<u32>().ok()) {
            self.plugin_learn_touch(device, id);
        }
    }

    /// Acts on a touch of `param` of `device` with the armed mode.
    fn plugin_learn_touch(&mut self, device: &DeviceKey, param: u32) {
        let Some(mode) = self.plugin_learn_mode(device) else {
            return;
        };
        let key = format!("p{param}");
        let name = self
            .doc()
            .resolve(device)
            .and_then(|d| self.registry.get(&d.type_id))
            .and_then(|f| f.descriptor.param_by_key(&key))
            .map(|p| p.name.to_owned());
        let status = match (name, mode) {
            (None, _) => PluginLearnStatus::NotAutomatable,
            (Some(name), PluginLearnMode::Automation) => {
                if self.follow_param(device, &key, true, false) {
                    PluginLearnStatus::LaneShown(name)
                } else {
                    PluginLearnStatus::NoLane(name)
                }
            }
            (Some(name), PluginLearnMode::Midi) => {
                self.drop_pending_midi(device);
                let target = MidiTarget::Param {
                    device: device.clone(),
                    param: key.clone(),
                };
                if self.midi_arm(target) {
                    if let Some(arm) = self.plugins.learn.arms.get_mut(device) {
                        arm.pending = Some(key);
                    }
                    PluginLearnStatus::MoveHardware(name)
                } else {
                    PluginLearnStatus::NotAutomatable
                }
            }
        };
        if let Some(arm) = self.plugins.learn.arms.get_mut(device) {
            arm.status = status;
        }
    }

    /// The MIDI learn mapped `source` to `target`: the strip of the window that armed it names
    /// the source.
    pub(crate) fn plugin_learn_mapped(&mut self, target: &MidiTarget, source: &MidiSource) {
        let MidiTarget::Param { device, param } = target else {
            return;
        };
        let Some(arm) = self.plugins.learn.arms.get_mut(device) else {
            return;
        };
        if arm.pending.as_deref() != Some(param.as_str()) {
            return;
        }
        arm.pending = None;
        let name = match &arm.status {
            PluginLearnStatus::MoveHardware(n) => n.clone(),
            _ => param.clone(),
        };
        arm.status = PluginLearnStatus::Mapped {
            param: name,
            source: source.describe(),
        };
    }
}
