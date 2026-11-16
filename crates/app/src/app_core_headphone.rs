//! Headphone Correction devices in the app core: importing the user's headphone data.
//!
//! The data is document data of the device ([`atomik_model::RackDevice::headphone`]), changed by
//! [`WorkflowCommand::SetHeadphoneData`] (one undo step) and handed to the engine as
//! [`atomik_device_sdk::PlayerData::Headphone`] by the player-data path (on insert, and on every
//! `PatternsChanged`). The import reads a text file the user chose, parses it
//! ([`atomik_device_sdk::headphone::parse`]; the file's name becomes the display name) and stores
//! the numbers in the project: nothing keeps the file path.
//!
//! The shell's `rack.headphone_import` command (the device's context menu) opens the file dialog;
//! `rack.headphone_import_pick`, whose argument is a path, imports without a dialog (a headless
//! session, the MCP, the dialog's result). The panel of phase 2 calls the same functions.

use super::*;
use atomik_device_sdk::headphone::{self, HeadphoneData, MAX_TEXT_BYTES};
use std::path::Path;

impl<S: EngineSink> AppCore<S> {
    /// Whether `device` is a Headphone Correction device, and if so whether it holds data.
    pub fn headphone_data_of(&self, device: &DeviceKey) -> Option<bool> {
        let d = self.document.doc().resolve(device)?;
        self.registry
            .takes_headphone(&d.type_id)
            .then(|| d.headphone_data().is_some())
    }

    /// The imported data of a Headphone Correction device (for a panel to draw).
    pub fn headphone_data(&self, device: &DeviceKey) -> Option<&HeadphoneData> {
        self.document.doc().resolve(device)?.headphone_data()
    }

    /// The Headphone Correction device a command without a device acts on: the selected one, else
    /// the first one of the rack (inside containers too).
    pub fn headphone_target(&self) -> Option<DeviceKey> {
        if let Some(primary) = self.view.primary()
            && self.headphone_data_of(&primary).is_some()
        {
            return Some(primary);
        }
        self.flat_devices()
            .into_iter()
            .find(|(_, d)| self.registry.takes_headphone(&d.type_id))
            .map(|(k, _)| k)
    }

    /// Reads the file at `path` and imports it into Headphone Correction device `device` as one
    /// undo step. Says why in a toast when the file cannot be read or is not a frequency
    /// response or a parametric EQ text (the message names the line); nothing changes then.
    /// Returns whether the data was imported.
    pub fn import_headphone_file(&mut self, device: &DeviceKey, path: &Path) -> bool {
        if self.headphone_data_of(device).is_none() {
            self.report("Select a Headphone Correction device first");
            return false;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Headphones");
        let bytes = match std::fs::metadata(path) {
            Ok(m) if m.len() > MAX_TEXT_BYTES as u64 => {
                self.report(format!(
                    "{name} is too large for headphone data (the limit is {} MB)",
                    MAX_TEXT_BYTES >> 20
                ));
                return false;
            }
            Ok(_) => std::fs::read(path),
            Err(e) => Err(e),
        };
        match bytes {
            // A degree sign or an accent in a header is not valid UTF-8 in some exports.
            Ok(bytes) => self.import_headphone_text(device, &String::from_utf8_lossy(&bytes), name),
            Err(e) => {
                self.report(format!("Cannot read {name}: {e}"));
                false
            }
        }
    }

    /// Imports `text` (a frequency response or a parametric EQ) into `device` under the display
    /// name `name`, as [`Self::import_headphone_file`] does for a file.
    pub fn import_headphone_text(&mut self, device: &DeviceKey, text: &str, name: &str) -> bool {
        let data = match headphone::parse(text, name) {
            Ok(data) => data,
            Err(e) => {
                self.report(format!("Cannot use {name}: {e}"));
                return false;
            }
        };
        let kind = if data.is_response() {
            "frequency response"
        } else {
            "parametric EQ"
        };
        let shown = data.name.clone();
        let ok = self
            .apply_workflow(WorkflowCommand::SetHeadphoneData {
                device: device.clone(),
                data: Some(data),
            })
            .is_ok();
        if ok {
            self.report(format!("Imported {kind} {shown}"));
        }
        ok
    }

    /// Removes the imported data of `device` (one undo step); the device passes the signal.
    pub fn remove_headphone_data(&mut self, device: &DeviceKey) {
        match self.headphone_data_of(device) {
            None => self.report("Select a Headphone Correction device first"),
            Some(false) => self.report("The device has no headphone data"),
            Some(true) => {
                if self
                    .apply_workflow(WorkflowCommand::SetHeadphoneData {
                        device: device.clone(),
                        data: None,
                    })
                    .is_ok()
                {
                    self.report("Removed the headphone data");
                }
            }
        }
    }
}
