//! The creation chain in the app core: the model commands do the
//! work in one undo step; the app tells them which tracks an earlier auto-route armed
//! (the model cannot know, the project format has no such flag), selects what was created so the
//! live keyboard plays the new instrument, and names what happened in a status message.

use super::clipboard_ops::WorkflowPre;
use super::*;
use atomik_model::{SongCommand, WorkflowCommand, insert_chain};

impl<S: EngineSink> AppCore<S> {
    /// Fills in what only the app knows before a creation command runs: the tracks to disarm.
    pub(super) fn prepare_creation(&self, command: Command) -> Command {
        let with_armed = |mut options: atomik_model::CreateOptions| {
            for k in &self.auto_armed {
                if !options.disarm.contains(k) {
                    options.disarm.push(k.clone());
                }
            }
            options
        };
        match command {
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index,
                source,
                key,
                options,
            }) => Command::Workflow(WorkflowCommand::CreateInstrument {
                index,
                source,
                key,
                options: with_armed(options),
            }),
            Command::Workflow(WorkflowCommand::CreateInstrumentAt {
                slot,
                source,
                key,
                options,
            }) => Command::Workflow(WorkflowCommand::CreateInstrumentAt {
                slot,
                source,
                key,
                options: with_armed(options),
            }),
            Command::Workflow(WorkflowCommand::AutoRoute { devices, options }) => {
                Command::Workflow(WorkflowCommand::AutoRoute {
                    devices,
                    options: with_armed(options),
                })
            }
            other => other,
        }
    }

    /// After a creation command: remembers the tracks it armed, selects the new device (the
    /// instrument, not its channel) and reports what was made.
    pub(super) fn created(&mut self, command: &WorkflowCommand, pre: &WorkflowPre) {
        let doc = self.document.doc();
        let new_tracks: Vec<DeviceKey> = doc
            .song
            .tracks
            .iter()
            .filter(|t| t.armed && !pre.tracks.contains(&t.device))
            .map(|t| t.device.clone())
            .collect();
        for k in new_tracks {
            if !self.auto_armed.contains(&k) {
                self.auto_armed.push(k);
            }
        }
        let doc = self.document.doc();
        if let WorkflowCommand::AutoRoute { devices, .. } = command {
            let n = devices.len();
            let message = format!("Auto-routed {n} device{}", if n == 1 { "" } else { "s" });
            self.after_view_change();
            self.report(message);
            return;
        }
        // "Add the device only" is the plain add: no selection change, no message.
        let routed = match command {
            WorkflowCommand::CreateInstrument { options, .. }
            | WorkflowCommand::CreateInstrumentAt { options, .. }
            | WorkflowCommand::CreateEffect { options, .. }
            | WorkflowCommand::CreateEffectAt { options, .. }
            | WorkflowCommand::CreatePlayer { options, .. } => options.route,
            _ => true,
        };
        if !routed {
            return;
        }
        let new_device = doc
            .devices
            .iter()
            .filter(|d| !pre.scope_keys.contains(&d.key))
            .find(|d| !atomik_model::console::is_console_type(&d.type_id))
            .map(|d| (d.key.clone(), d.name.clone()));
        let Some((key, name)) = new_device else {
            return;
        };
        let message = match pre.note {
            // All eight send/return slots are used: the effect was added unrouted.
            Some(note) => Some(note.text().to_string()),
            None => self.created_message(command, &key, &name),
        };
        self.after_view_change();
        self.view
            .set_selection(std::slice::from_ref(&key), Some(&key));
        self.after_view_change();
        if let Some(m) = message {
            self.report(m);
        }
    }

    /// "Created X with a mixer channel and a song track (armed)" or "Created X on insert 2 of
    /// Y"; `None` when nothing was routed (the plain add).
    fn created_message(
        &self,
        command: &WorkflowCommand,
        key: &DeviceKey,
        name: &str,
    ) -> Option<String> {
        let doc = self.document.doc();
        match command {
            WorkflowCommand::CreateInstrument { .. }
            | WorkflowCommand::CreateInstrumentAt { .. } => {
                let mut parts = Vec::new();
                if doc.console.channel_of(key).is_some() {
                    parts.push("a mixer channel");
                }
                if doc.song.track(key).is_some_and(|t| t.armed) {
                    parts.push("an armed song track");
                }
                if parts.is_empty() {
                    return None;
                }
                Some(format!("Created {name} with {}", parts.join(" and ")))
            }
            WorkflowCommand::CreatePlayer { .. } => {
                // The device its notes go on to: the head of the stack it was put on.
                let below = doc
                    .cables
                    .iter()
                    .find(|c| &c.from.device == key && c.from.socket == "notes_out")?
                    .to
                    .device
                    .clone();
                let below = doc.device(&below).map_or("the device", |d| &d.name);
                Some(format!("Created {name} above {below}"))
            }
            WorkflowCommand::CreateEffect { .. } | WorkflowCommand::CreateEffectAt { .. } => {
                // Created with the master selected: the first free send/return.
                if let Some(slot) = doc.console.return_slot(key) {
                    return Some(format!("Effect routed to send/return {}", slot + 1));
                }
                for ch in &doc.console.channels {
                    let chain = insert_chain(doc, &self.registry, &ch.channel);
                    if let Some(slot) = chain.iter().position(|k| k == key) {
                        let channel = doc.device(&ch.channel).map_or("channel", |d| &d.name);
                        return Some(format!(
                            "Created {name} on insert {} of {channel}",
                            slot + 1
                        ));
                    }
                }
                let feeder = doc
                    .cables
                    .iter()
                    .find(|c| &c.to.device == key && c.to.socket == "in_l")
                    .map(|c| c.from.device.clone())?;
                let source = doc.device(&feeder).map_or("the device", |d| &d.name);
                Some(format!("Created {name} after {source}"))
            }
            _ => None,
        }
    }

    /// A new console selection arms the song track of its primary strip (the last one clicked)
    /// and disarms the tracks an earlier auto-arm armed, as creating an instrument does; the
    /// tracks the user armed stay. The track's device is selected in the rack, which is what
    /// the live keyboard plays while tracks are armed. A strip without a track (a bus, an FX
    /// return, a channel with no song track) changes nothing. Unlike creation there is no
    /// enclosing step, so arming and disarming are one batched undo step of their own.
    pub(super) fn follow_console_selection(&mut self) {
        let selection = self.console.selected_strips();
        if selection == self.console_selection {
            return;
        }
        self.console_selection = selection;
        self.selection_changed = true;
        let doc = self.document.doc();
        let Some(device) = self
            .console
            .primary_strip()
            .and_then(|strip| doc.console.channel_index(&strip))
            .and_then(|i| doc.console.channels[i].source.clone())
            .filter(|source| doc.song.track(source).is_some())
        else {
            return;
        };
        self.arm_track_device(&device);
    }

    /// A track header clicked (or stepped to) in the arrangement is a channel strip clicked in
    /// the console: the strip is selected there and the track armed and selected in the rack
    /// through the same path, and a plain click hands the keys to the rack.
    pub(super) fn follow_arrangement_track(&mut self, event: &UiEvent) {
        let Some(device) = self.arrangement.selected_track().cloned() else {
            self.arranged_track = None;
            return;
        };
        if self.arranged_track.as_ref() != Some(&device) {
            self.arranged_track = Some(device.clone());
            self.arrangement_track_changed = true;
            let channel = self
                .document
                .doc()
                .console
                .channel_of(&device)
                .map(|c| c.channel.clone());
            if let Some(channel) = channel {
                self.console.set_selected_strips(&[channel]);
                self.console_selection = self.console.selected_strips();
            }
            self.arm_track_device(&device);
        }
        if let UiEvent::PointerUp { modifiers, .. } = event {
            self.rack_focus_request |= std::mem::take(&mut self.arrangement_track_changed)
                && !modifiers.shift
                && !modifiers.command();
        }
    }

    /// Arms the song track of `device` and disarms the tracks an earlier auto-arm armed, then
    /// selects the device in the rack (what the live keyboard plays). One batched undo step.
    fn arm_track_device(&mut self, device: &DeviceKey) {
        let device = device.clone();
        let doc = self.document.doc();
        // An automation track (a channel strip, an effect) is never armed: nothing to play live.
        // It is still selected in the rack, and the other tracks keep their arming.
        if doc
            .song
            .track(&device)
            .is_some_and(|t| t.is_automation_only())
        {
            self.view
                .set_selection(std::slice::from_ref(&device), Some(&device));
            self.after_view_change();
            return;
        }
        let flags = |device: &DeviceKey, armed: bool| {
            doc.song.track(device).map(|t| {
                Command::Song(SongCommand::SetTrackFlags {
                    device: device.clone(),
                    muted: t.muted,
                    solo: t.solo,
                    armed,
                })
            })
        };
        let mut batch: Vec<Command> = self
            .auto_armed
            .iter()
            .filter(|k| **k != device && doc.song.track(k).is_some_and(|t| t.armed))
            .filter_map(|k| flags(k, false))
            .collect();
        self.auto_armed.retain(|k| *k == device);
        if doc.song.track(&device).is_some_and(|t| !t.armed) {
            batch.extend(flags(&device, true));
            self.auto_armed.push(device.clone());
        }
        if !batch.is_empty() {
            // A rejection is already reported to the view.
            let _ = self.apply_command(Command::Batch(batch));
        }
        self.view
            .set_selection(std::slice::from_ref(&device), Some(&device));
        self.after_view_change();
    }
}
