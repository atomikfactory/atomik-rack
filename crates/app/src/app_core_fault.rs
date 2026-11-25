//! What the core does for the automatic restart after an engine fault (see
//! [`crate::fault_restart`]): it names the device the engine reports and switches it off in the
//! model, as an ordinary undoable edit, so the engine that is rebuilt next does not meet the
//! same crash. Undo brings the device back.

use super::*;

/// What [`AppCore::bypass_faulted_device`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultBypass {
    /// The fault names no device (a panic outside any device), or the device is no longer in the
    /// rack: nothing was bypassed.
    NoDevice,
    /// The device is bypassed (by this call, or already was).
    Bypassed(String),
    /// The device has no bypass switch (an instrument): it stays as it is.
    Unsupported(String),
    /// The plugin path owns the device (its process crashed and is being reloaded): it is left
    /// to that path and nothing is restarted for it.
    PluginPath(String),
}

impl<S: EngineSink> AppCore<S> {
    /// The rack key of the device the engine knows as instance `id` (a container's split stands
    /// for the container), if it is still in the rack.
    fn device_key_of_instance(&self, id: u32) -> Option<DeviceKey> {
        let key = self.ids.iter().find(|(_, v)| v.0 == id)?.0;
        Some(containers::container_of_split(key))
    }

    /// The engine instance id of the device `key` (used by tests to name a fault).
    #[cfg(test)]
    pub(crate) fn instance_id_of(&self, key: &DeviceKey) -> Option<u32> {
        self.ids.get(key).map(|id| id.0)
    }

    /// Whether the device the engine names is one the plugin crash path is handling: a plugin
    /// whose process ended is CRASHED and reloaded there, never restarted from here.
    pub fn fault_belongs_to_plugin_path(&self, id: u32) -> bool {
        self.device_key_of_instance(id)
            .is_some_and(|key| self.plugin_is_crashed(&key))
    }

    /// Bypasses the device the engine reported as faulted (instance `id`), as one undoable edit.
    /// A device already bypassed is left alone and still counts as bypassed.
    pub fn bypass_faulted_device(&mut self, id: Option<u32>) -> FaultBypass {
        let Some(key) = id.and_then(|id| self.device_key_of_instance(id)) else {
            return FaultBypass::NoDevice;
        };
        let Some(device) = self.doc().resolve(&key) else {
            return FaultBypass::NoDevice;
        };
        let name = device.name.clone();
        if self.plugin_is_crashed(&key) {
            return FaultBypass::PluginPath(name);
        }
        let has_switch = device.plugin.is_some()
            || self
                .registry
                .get(&device.type_id)
                .is_some_and(|f| f.descriptor.param_by_key("bypass").is_some());
        if !has_switch {
            return FaultBypass::Unsupported(name);
        }
        if device.params.get("bypass").copied().unwrap_or(0.0) < 0.5 {
            let command = Command::SetParam {
                key,
                param: "bypass".to_owned(),
                value: 1.0,
                merge: None,
            };
            if self.apply_command(command).is_err() {
                return FaultBypass::Unsupported(name);
            }
        }
        FaultBypass::Bypassed(name)
    }
}
