//! Cut, copy, paste and duplicate of console strips.
//!
//! A copied strip is the channel with what feeds it: the instrument or track device with its song
//! track (a device snapshot) and the settings of the channel (a [`ChannelSettingsClip`], every
//! section), because a device snapshot leaves console devices out and a pasted instrument gets a
//! fresh channel. A paste is one undo step: the devices come in after the selected strip's
//! device, the new channels are moved to sit after the selected strip, and the copied settings
//! are pasted onto them. Buses, the master and channels with no source device are not copied.

use super::*;
use crate::clipboard::{ClipboardData, StripSnapshot};
use atomik_model::{
    ChannelSettingsClip, ConsoleCommand, DeviceSnapshot, SnapshotScope, WorkflowCommand,
};
use std::collections::BTreeSet;

impl<S: EngineSink> AppCore<S> {
    /// What Copy takes from the console: the selected channels that have a source device.
    pub(super) fn capture_strips(&self, selected: &[DeviceKey]) -> Result<StripSnapshot, String> {
        let doc = self.doc();
        if selected.is_empty() {
            return Err("Select one or more channels to copy".into());
        }
        let mut sources: Vec<DeviceKey> = Vec::new();
        let mut channels: Vec<(DeviceKey, DeviceKey)> = Vec::new();
        for strip in selected {
            let source = doc
                .console
                .channel_index(strip)
                .and_then(|i| doc.console.channels[i].source.clone());
            if let Some(source) = source
                && !sources.contains(&source)
            {
                sources.push(source.clone());
                channels.push((strip.clone(), source));
            }
        }
        if sources.is_empty() {
            return Err(
                "Only channels that carry an instrument or a track can be copied: \
                 buses and mixer channels cannot"
                    .into(),
            );
        }
        let devices = DeviceSnapshot::capture(doc, &sources, true).map_err(|e| e.to_string())?;
        let mut settings = Vec::new();
        for (strip, source) in channels {
            let Some(at) = devices.devices.iter().position(|d| d.key == source) else {
                continue;
            };
            let clip: ChannelSettingsClip = doc
                .copy_channel_settings(&self.registry, &strip, SnapshotScope::ALL)
                .map_err(|e| e.to_string())?;
            settings.push((at, clip));
        }
        Ok(StripSnapshot { devices, settings })
    }

    /// Copy in the console.
    pub(super) fn copy_strips(&mut self) -> Result<(), String> {
        let snapshot = self.capture_strips(&self.strips_for_vca(Pane::Console))?;
        self.clipboard.set(ClipboardData::Strips(snapshot));
        Ok(())
    }

    /// Duplicate in the console: a copy of the selected strips right after the last of them,
    /// leaving the clipboard alone.
    pub(super) fn duplicate_strips(&mut self) -> Result<(), String> {
        let snapshot = self.capture_strips(&self.strips_for_vca(Pane::Console))?;
        self.paste_strips(snapshot)
    }

    /// Paste in the console: the strips of `snapshot` after the last selected channel (at the
    /// end with none selected), as one undo step, selected afterwards.
    pub(super) fn paste_strips(&mut self, snapshot: StripSnapshot) -> Result<(), String> {
        let doc = self.doc();
        let anchor = self
            .strips_for_vca(Pane::Console)
            .iter()
            .filter_map(|k| doc.console.channel_index(k))
            .max();
        self.paste_strips_at(snapshot, anchor.map(|i| i + 1))
    }

    /// Pastes `snapshot` so that its first channel lands at console position `first` (the end
    /// with `None`); the devices come in after the device of the channel just before it.
    fn paste_strips_at(
        &mut self,
        snapshot: StripSnapshot,
        first: Option<usize>,
    ) -> Result<(), String> {
        let doc = self.doc();
        let at = first
            .and_then(|f| f.checked_sub(1))
            .and_then(|i| doc.console.channels.get(i))
            .and_then(|c| c.source.as_ref())
            .and_then(|source| doc.devices.iter().position(|d| &d.key == source))
            .map_or(if first == Some(0) { 0 } else { usize::MAX }, |i| i + 1);
        let channels_before: Vec<DeviceKey> = doc
            .console
            .channels
            .iter()
            .map(|c| c.channel.clone())
            .collect();
        let devices_before: BTreeSet<DeviceKey> =
            doc.devices.iter().map(|d| d.key.clone()).collect();
        let StripSnapshot { devices, settings } = snapshot;
        let copied: Vec<String> = devices.devices.iter().map(|d| d.type_id.clone()).collect();
        self.apply_workflow(WorkflowCommand::PasteDevices {
            snapshot: devices,
            container: None,
            at,
            route: false,
        })?;

        // The new devices, in rack order, line up with the copied ones.
        let doc = self.doc();
        let fresh: Vec<&atomik_model::RackDevice> = doc
            .devices
            .iter()
            .filter(|d| {
                !devices_before.contains(&d.key)
                    && !atomik_model::console::is_console_type(&d.type_id)
            })
            .collect();
        let lined_up =
            fresh.len() == copied.len() && fresh.iter().zip(&copied).all(|(d, t)| &d.type_id == t);
        let mut follow: Vec<Command> = Vec::new();
        let new_channels: Vec<DeviceKey> = doc
            .console
            .channels
            .iter()
            .map(|c| c.channel.clone())
            .filter(|k| !channels_before.contains(k))
            .collect();
        if let Some(first) = first {
            for (n, channel) in new_channels.iter().enumerate() {
                follow.push(Command::Console(ConsoleCommand::MoveChannel {
                    channel: channel.clone(),
                    to: first + n,
                }));
            }
        }
        if lined_up {
            for (at, clip) in settings {
                let channel = doc
                    .console
                    .channel_of(&fresh[at].key)
                    .map(|c| c.channel.clone());
                if let Some(channel) = channel {
                    follow.push(Command::Console(ConsoleCommand::PasteSettings {
                        strips: vec![channel],
                        clip,
                    }));
                }
            }
        }
        for command in follow {
            self.join_next = true;
            // A refusal is reported; the paste itself stays.
            let _ = self.apply_command(command);
            self.join_next = false;
        }
        self.after_view_change();
        self.console.set_selected_strips(&new_channels);
        self.after_view_change();
        Ok(())
    }

    /// Ctrl-drag of strips in the console: copies of `strips` placed so that the first lands at
    /// console position `to`, as one undo step; the clipboard is untouched. False, with a
    /// message, when nothing can be copied or the model refuses.
    pub fn copy_strips_to(&mut self, strips: &[DeviceKey], to: usize) -> bool {
        let result = self
            .capture_strips(strips)
            .and_then(|s| self.paste_strips_at(s, Some(to)));
        match result {
            Ok(()) => true,
            Err(message) => {
                if !message.is_empty() {
                    self.report(message);
                }
                false
            }
        }
    }
}
