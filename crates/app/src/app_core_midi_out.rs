//! MIDI Out devices in the app core: which output port each one sends to.
//!
//! The port is document data of the device ([`atomik_model::MidiOutDoc`]), changed by
//! [`WorkflowCommand::SetMidiOutPort`] (one undo step). The engine has no idea of ports: its
//! MIDI Out device queues timestamped messages on a tap, and the sink's MIDI output thread sends
//! them. This module keeps the sink told where each device's messages go, by engine device id:
//! after every edit and every engine rebuild it lists the MIDI Out devices of the expanded rack
//! with their chosen port (`None` = the settings' fallback port) and hands the list to
//! [`EngineSink::set_midi_out_ports`] when it differs from what the sink was last told.

use super::*;
use atomik_model::MIDI_OUT_TYPE;

impl<S: EngineSink> AppCore<S> {
    /// Tells the sink which port each MIDI Out device sends to (only when it changed since the
    /// last time). Walks the engine's own device ids, never the raw document lists.
    pub(super) fn push_midi_out_ports(&mut self) {
        let mut list: Vec<(DeviceInstanceId, Option<String>)> = self
            .ids
            .iter()
            .filter_map(|(key, id)| {
                let device = self.document.doc().resolve(key)?;
                (device.type_id == MIDI_OUT_TYPE)
                    .then(|| (*id, device.midi_out_port().map(str::to_owned)))
            })
            .collect();
        list.sort_by_key(|(id, _)| id.0);
        if list != self.midi_out_sent {
            self.sink.set_midi_out_ports(&list);
            self.midi_out_sent = list;
        }
    }

    /// The port of MIDI Out device `device`: `Some(port)` where the outer `Option` says whether
    /// the device is a MIDI Out device at all and the inner one whether it chose a port.
    pub fn midi_out_port_of(&self, device: &DeviceKey) -> Option<Option<String>> {
        let d = self.document.doc().resolve(device)?;
        (d.type_id == MIDI_OUT_TYPE).then(|| d.midi_out_port().map(str::to_owned))
    }

    /// Sets the port of MIDI Out device `device` (`None` goes back to the settings' port): one
    /// undo step. Says why when the device is not a MIDI Out device.
    pub fn set_midi_out_port(&mut self, device: &DeviceKey, port: Option<String>) {
        if self.midi_out_port_of(device).is_none() {
            self.report("Select a MIDI Out device first");
            return;
        }
        let shown = port.clone();
        // A rejection is already reported by the command.
        if self
            .apply_workflow(WorkflowCommand::SetMidiOutPort {
                device: device.clone(),
                port,
            })
            .is_ok()
        {
            self.report(match shown {
                Some(p) => format!("MIDI Out sends to {p}"),
                None => "MIDI Out uses the port of the settings".to_owned(),
            });
        }
    }
}
