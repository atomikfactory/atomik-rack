//! Cut, copy, paste and duplicate of console strips.
//!
//! A copied strip is the channel with what feeds it: the instrument or track device with its song
//! track (a device snapshot) and the settings of the channel (a [`ChannelSettingsClip`], every
//! section), because a device snapshot leaves console devices out and a pasted instrument gets a
//! fresh channel. Mixer channels (no source device) and output buses have nothing behind them:
//! they are copied as a name and their settings, and a paste makes a new mixer channel or bus
//! (named like the original, a bus getting a free name; routed to the master, like a new bus).
//! A paste is one undo step: the devices come in after the selected strip's device, the new
//! channels are moved to sit after the selected strip, and the copied settings are pasted onto
//! them. The master is not copied.

use super::*;
use crate::clipboard::{ClipboardData, LooseKind, LooseStrip, StripSnapshot};
use atomik_model::console::{MAX_BUS_NAME_CHARS, OutputDest};
use atomik_model::{
    ChannelSettingsClip, ConsoleCommand, DeviceSnapshot, SnapshotScope, WorkflowCommand,
};
use std::collections::{BTreeMap, BTreeSet};

impl<S: EngineSink> AppCore<S> {
    /// What Copy takes from the console: the selected channels and buses. A channel with a
    /// source device is copied with the device; the others by name and settings.
    pub(super) fn capture_strips(&self, selected: &[DeviceKey]) -> Result<StripSnapshot, String> {
        let doc = self.doc();
        if selected.is_empty() {
            return Err("Select one or more channels or buses to copy".into());
        }
        let mut sources: Vec<DeviceKey> = Vec::new();
        let mut channels: Vec<(DeviceKey, DeviceKey)> = Vec::new();
        let mut loose_strips: Vec<(LooseKind, DeviceKey)> = Vec::new();
        for strip in selected {
            if doc.console.bus_index(strip).is_some() {
                loose_strips.push((LooseKind::Bus, strip.clone()));
                continue;
            }
            let Some(at) = doc.console.channel_index(strip) else {
                continue;
            };
            match doc.console.channels[at].source.clone() {
                Some(source) => {
                    if !sources.contains(&source) {
                        sources.push(source.clone());
                        channels.push((strip.clone(), source));
                    }
                }
                None => loose_strips.push((LooseKind::Mix, strip.clone())),
            }
        }
        if sources.is_empty() && loose_strips.is_empty() {
            return Err("Select one or more channels or buses to copy".into());
        }
        let copy_clip = |strip: &DeviceKey| -> Result<ChannelSettingsClip, String> {
            doc.copy_channel_settings(&self.registry, strip, SnapshotScope::ALL)
                .map_err(|e| e.to_string())
        };
        let devices = if sources.is_empty() {
            DeviceSnapshot {
                devices: Vec::new(),
                cables: Vec::new(),
                tracks: Vec::new(),
                assets: BTreeMap::new(),
            }
        } else {
            DeviceSnapshot::capture(doc, &sources, true).map_err(|e| e.to_string())?
        };
        let mut settings = Vec::new();
        for (strip, source) in channels {
            let Some(at) = devices.devices.iter().position(|d| d.key == source) else {
                continue;
            };
            settings.push((at, copy_clip(&strip)?));
        }
        let mut loose = Vec::new();
        for (kind, strip) in loose_strips {
            let name = doc
                .device(&strip)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            loose.push(LooseStrip {
                kind,
                name,
                clip: copy_clip(&strip)?,
            });
        }
        Ok(StripSnapshot {
            devices,
            settings,
            loose,
        })
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
        let buses_before: Vec<DeviceKey> =
            doc.console.buses.iter().map(|b| b.bus.clone()).collect();
        let devices_before: BTreeSet<DeviceKey> =
            doc.devices.iter().map(|d| d.key.clone()).collect();
        let StripSnapshot {
            devices,
            settings,
            loose,
        } = snapshot;
        let copied: Vec<String> = devices.devices.iter().map(|d| d.type_id.clone()).collect();
        // Everything after the first command joins its undo step.
        let mut started = false;
        if !devices.devices.is_empty() {
            self.apply_workflow(WorkflowCommand::PasteDevices {
                snapshot: devices,
                container: None,
                at,
                route: false,
            })?;
            started = true;
        }

        // Mixer channels and buses are made from scratch; the settings follow below.
        let mut made: Vec<(DeviceKey, ChannelSettingsClip)> = Vec::new();
        for strip in loose {
            let command = match strip.kind {
                LooseKind::Mix => ConsoleCommand::AddMixChannel { name: strip.name },
                LooseKind::Bus => ConsoleCommand::CreateBus {
                    name: self.free_bus_name(&strip.name),
                    dest: OutputDest::Master,
                },
            };
            self.join_next = started;
            let result = self.apply_command(Command::Console(command));
            self.join_next = false;
            if let Err(e) = result {
                if started {
                    // The strips made so far stay; the refusal is reported.
                    break;
                }
                return Err(e.to_string());
            }
            started = true;
            let console = &self.doc().console;
            let key = match strip.kind {
                LooseKind::Mix => console.channels.last().map(|c| c.channel.clone()),
                LooseKind::Bus => console.buses.last().map(|b| b.bus.clone()),
            };
            if let Some(key) = key {
                made.push((key, strip.clip));
            }
        }

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
        let new_buses: Vec<DeviceKey> = doc
            .console
            .buses
            .iter()
            .map(|b| b.bus.clone())
            .filter(|k| !buses_before.contains(k))
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
        for (strip, clip) in made {
            follow.push(Command::Console(ConsoleCommand::PasteSettings {
                strips: vec![strip],
                clip,
            }));
        }
        for command in follow {
            self.join_next = true;
            // A refusal is reported; the paste itself stays.
            let _ = self.apply_command(command);
            self.join_next = false;
        }
        self.after_view_change();
        let selected: Vec<DeviceKey> = new_channels.into_iter().chain(new_buses).collect();
        self.console.set_selected_strips(&selected);
        self.after_view_change();
        Ok(())
    }

    /// `name` when no bus has it, else the first `name 2`, `name 3`, ... that is free (cut to
    /// the longest name a bus may have).
    fn free_bus_name(&self, name: &str) -> String {
        let console = &self.doc().console;
        let fit = |base: &str, suffix: &str| -> String {
            let room = MAX_BUS_NAME_CHARS.saturating_sub(suffix.chars().count());
            let head: String = base.trim().chars().take(room).collect();
            format!("{}{suffix}", head.trim_end())
        };
        let plain = fit(name, "");
        if !plain.is_empty() && console.bus_by_name(&plain).is_none() {
            return plain;
        }
        let base = if plain.is_empty() { "Bus" } else { name };
        (2usize..)
            .map(|n| fit(base, &format!(" {n}")))
            .find(|n| console.bus_by_name(n).is_none())
            .unwrap_or_else(|| console.default_bus_name())
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
