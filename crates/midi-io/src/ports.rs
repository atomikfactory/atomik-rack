//! Raw MIDI ports for the control path: an input that hands every message (SysEx included) to
//! a callback, and an output. Control surfaces use both; neither touches the engine.

use crate::{CLIENT_NAME, MidiError, match_port};

/// Lists MIDI output port names.
pub fn list_outputs() -> Result<Vec<String>, MidiError> {
    let output = new_output()?;
    Ok(output
        .ports()
        .iter()
        .map(|p| output.port_name(p).unwrap_or_default())
        .collect())
}

fn new_output() -> Result<midir::MidiOutput, MidiError> {
    midir::MidiOutput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))
}

/// An open MIDI input that calls back with each message as it arrives (on midir's thread);
/// dropping it closes the port. Unlike [`crate::open_input`] it keeps SysEx messages.
pub struct MidiListener {
    _connection: midir::MidiInputConnection<()>,
    port: String,
}

impl std::fmt::Debug for MidiListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiListener")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl MidiListener {
    /// The full name of the connected port.
    pub fn port_name(&self) -> &str {
        &self.port
    }
}

/// Opens input port `name` (exact or unique case-insensitive substring) and calls `on_message`
/// with every message (clock and active sensing are dropped at the source).
///
/// # Errors
/// [`MidiError::PortNotFound`] when no port matches, [`MidiError::Backend`] otherwise.
pub fn listen(
    name: &str,
    mut on_message: impl FnMut(&[u8]) + Send + 'static,
) -> Result<MidiListener, MidiError> {
    let mut input =
        midir::MidiInput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))?;
    input.ignore(midir::Ignore::TimeAndActiveSense);
    let ports = input.ports();
    let names: Vec<String> = ports
        .iter()
        .map(|p| input.port_name(p).unwrap_or_default())
        .collect();
    let index = match_port(&names, name)?;
    let connection = input
        .connect(
            &ports[index],
            "atomik-rack-surface-in",
            move |_stamp, bytes, ()| on_message(bytes),
            (),
        )
        .map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(MidiListener {
        _connection: connection,
        port: names[index].clone(),
    })
}

/// An open MIDI output; dropping it closes the port.
pub struct MidiSender {
    connection: midir::MidiOutputConnection,
    port: String,
}

impl std::fmt::Debug for MidiSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiSender")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl MidiSender {
    /// The full name of the connected port.
    pub fn port_name(&self) -> &str {
        &self.port
    }

    /// Sends one message (a channel message or a whole SysEx frame).
    ///
    /// # Errors
    /// [`MidiError::Backend`] when the port refuses it.
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.connection
            .send(bytes)
            .map_err(|e| MidiError::Backend(e.to_string()))
    }
}

/// Opens output port `name` (exact or unique case-insensitive substring).
///
/// # Errors
/// [`MidiError::PortNotFound`] when no port matches, [`MidiError::Backend`] otherwise.
pub fn open_output(name: &str) -> Result<MidiSender, MidiError> {
    let output = new_output()?;
    let ports = output.ports();
    let names: Vec<String> = ports
        .iter()
        .map(|p| output.port_name(p).unwrap_or_default())
        .collect();
    let index = match_port(&names, name)?;
    let connection = output
        .connect(&ports[index], "atomik-rack-surface-out")
        .map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(MidiSender {
        connection,
        port: names[index].clone(),
    })
}
