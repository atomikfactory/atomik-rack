//! The MIDI Out device's destination.
//!
//! A MIDI Out device (`atomik.midi-out`) sends its notes and controllers to one MIDI output port
//! of the computer. The port is a name, which the numeric param model cannot hold, so it is
//! document data of the device, like a plugin's [`crate::PluginDoc`]: [`RackDevice::midi_out`] is
//! `Some` once the user chose a port and `None` while the device uses the fallback port of the
//! app settings. The name is kept as chosen, also when no such port is plugged in (a project
//! moves between computers); the app resolves it when it opens the port.
//!
//! The field is stored only when set (project format 37) and only on this device type.

use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, rules};

/// The type id of the MIDI Out device.
pub const MIDI_OUT_TYPE: &str = "atomik.midi-out";

/// Most characters of a port name.
pub const MAX_MIDI_OUT_PORT_CHARS: usize = 128;

/// What the document keeps about one MIDI Out device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiOutDoc {
    /// The chosen output port: its name as the system lists it (the app also accepts a unique
    /// part of a name). Never empty.
    pub port: String,
}

impl MidiOutDoc {
    /// A doc for `port`.
    pub fn new(port: impl Into<String>) -> Self {
        Self { port: port.into() }
    }
}

/// Checks a port name as the project loader and the commands do: trimmed, 1 to
/// [`MAX_MIDI_OUT_PORT_CHARS`] characters, no control characters.
pub(crate) fn check_port_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_MIDI_OUT_PORT_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "a MIDI port name must be trimmed and 1 to {MAX_MIDI_OUT_PORT_CHARS} characters, without control characters"
        ));
    }
    Ok(())
}

/// Plans `SetMidiOutPort`: the port of a MIDI Out device (`None` goes back to the settings'
/// fallback port).
pub(crate) fn plan_set_port(
    doc: &RackDoc,
    key: &DeviceKey,
    port: Option<&str>,
) -> Result<Edit, ModelError> {
    let device = rules::resolve(doc, key)?;
    if device.type_id != MIDI_OUT_TYPE {
        return Err(ModelError::BadScope(format!(
            "{key} is not a MIDI Out device"
        )));
    }
    if let Some(name) = port {
        check_port_name(name).map_err(ModelError::BadScope)?;
    }
    let old = device.midi_out.as_ref().map(|m| m.port.clone());
    if old.as_deref() == port {
        return Err(ModelError::BadScope(format!(
            "{key} already has that MIDI port"
        )));
    }
    Ok(Edit::MidiOutPort {
        key: key.clone(),
        old,
        new: port.map(str::to_owned),
    })
}

impl RackDevice {
    /// The chosen MIDI output port of a MIDI Out device, if it has one.
    pub fn midi_out_port(&self) -> Option<&str> {
        self.midi_out.as_ref().map(|m| m.port.as_str())
    }
}
