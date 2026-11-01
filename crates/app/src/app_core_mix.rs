//! Mixer commands that create and delete strips: Create Mixer Channel, Create Parallel Channel and
//! Delete Channel, on the strips a pane selects.

use super::*;
use atomik_device_sdk::DeviceKind;
use atomik_model::{ConsoleCommand, WorkflowCommand};

impl<S: EngineSink> AppCore<S> {
    /// Create > Create Mixer Channel: a channel with no source, wired to the master, named by the
    /// model; it is selected in the console. False, with a message, when the model refuses.
    pub fn add_mix_channel(&mut self) -> bool {
        let command = Command::Console(ConsoleCommand::AddMixChannel {
            name: String::new(),
        });
        if self.apply_command(command).is_err() {
            return false;
        }
        if let Some(channel) = self
            .doc()
            .console
            .channels
            .last()
            .map(|c| c.channel.clone())
        {
            self.console.set_selected_strips(&[channel]);
        }
        self.after_view_change();
        true
    }

    /// Mixer > Create Parallel Channel: a parallel channel for each selected strip (one undo step
    /// each); the last one made is selected. False, with a message, when nothing is selected or
    /// the model refuses.
    pub fn create_parallel_channels(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select a channel to create a parallel channel of".into());
            return false;
        }
        let mut last = None;
        for strip in strips {
            let command = Command::Console(ConsoleCommand::CreateParallel { strip });
            if self.apply_command(command).is_err() {
                return false;
            }
            last = self.doc().console.parallels.last().map(|(_, c)| c.clone());
        }
        if let Some(channel) = last {
            self.console.set_selected_strips(&[channel]);
        }
        self.after_view_change();
        true
    }

    /// The device whose removal removes `strip` the way the user means it: the instrument (or
    /// container, or audio track) that feeds a channel, so its track goes too, else the strip
    /// itself.
    fn device_behind_strip(&self, strip: &DeviceKey) -> DeviceKey {
        let doc = self.doc();
        let Some(source) = doc
            .console
            .channel_index(strip)
            .and_then(|i| doc.console.channels[i].source.clone())
        else {
            return strip.clone();
        };
        let owns = doc
            .device(&source)
            .and_then(|d| self.registry.get(&d.type_id))
            .is_some_and(|f| {
                matches!(
                    f.descriptor.kind,
                    DeviceKind::Instrument | DeviceKind::Container
                )
            });
        if owns { source } else { strip.clone() }
    }

    /// How many clips (and takes) the song track of `device` holds, if it has a track.
    fn track_clip_count(&self, device: &DeviceKey) -> Option<usize> {
        self.doc().song.track(device).map(|t| {
            t.lanes
                .iter()
                .map(|l| l.clips.len() + l.take_groups.iter().map(|g| g.takes.len()).sum::<usize>())
                .sum()
        })
    }

    /// Mixer > Delete Channel (and the Delete key in the console): removes the selected strips in
    /// one undo step. A strip with no source goes with its parallel channels. A strip an
    /// instrument feeds goes with the instrument and its track, which asks first when the track
    /// holds clips: the first call only says so, and the same call again with the same strips
    /// confirms. False, with a message, when nothing is selected, the strips need that
    /// confirmation or the model refuses.
    pub fn remove_selected_strips(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels to delete".into());
            return false;
        }
        let mut keys: Vec<DeviceKey> = Vec::new();
        for strip in &strips {
            let key = self.device_behind_strip(strip);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let with_clips: Vec<(String, usize)> = keys
            .iter()
            .filter_map(|k| {
                let n = self.track_clip_count(k).filter(|n| *n > 0)?;
                Some((
                    self.doc()
                        .device(k)
                        .map_or_else(String::new, |d| d.name.clone()),
                    n,
                ))
            })
            .collect();
        if !with_clips.is_empty() && self.strip_delete_ask.as_ref() != Some(&keys) {
            let (name, n) = &with_clips[0];
            let more = if with_clips.len() > 1 {
                format!(" (and {} more tracks)", with_clips.len() - 1)
            } else {
                String::new()
            };
            let clips = if *n == 1 {
                "1 clip".to_owned()
            } else {
                format!("{n} clips")
            };
            self.reject(format!(
                "Deleting this channel also deletes {name} and its track with {clips}{more}: \
                 choose Delete Channel again to confirm"
            ));
            self.strip_delete_ask = Some(keys);
            return false;
        }
        self.strip_delete_ask = None;
        if self
            .apply_command(Command::Workflow(WorkflowCommand::RemoveDevices { keys }))
            .is_err()
        {
            return false;
        }
        self.console.set_selected_strips(&[]);
        self.after_view_change();
        true
    }
}
