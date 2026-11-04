//! The player stack commands of the Track menu: Direct Record, Send Players to Track, Fold All
//! Players and Bypass All Players. They act on the selected tracks that have a player stack (note
//! players above the instrument, see [`atomik_model::player_stack`]); each press is one undo step.
//!
//! * **Direct Record** switches the track flag of the same name: while recording, the take
//!   records the notes the stack generates (what the instrument receives, tapped in the engine)
//!   instead of the notes played into the top of the stack. See [`crate::record`].
//! * **Send Players to Track** renders the document offline
//!   ([`atomik_render::players::render_player_notes`]) and puts what the stack generated, from
//!   tick 0 to the song end, into one clip on a new note lane of the track (named "Players", the
//!   clip rounded out to whole bars). Nothing else changes: the stack keeps playing, so mute it
//!   (or the lane the notes came from) to hear the copy alone. The render runs on this thread.
//! * **Fold All Players** folds every player of the stack, or unfolds them when all are folded.
//! * **Bypass All Players** bypasses every player of the stack, or releases them when all are
//!   bypassed.
//! * **Add Pattern Lane** adds the automation lane of the `pattern` param of the selected pattern
//!   device (or of the players of the selected track) as one undo step; a lane that exists is
//!   left alone.
//! * **Pattern Lane to Notes** is Send Players to Track for the tracks that play a device with a
//!   pattern lane: the lane's switches are part of what the render plays, so the clip holds the
//!   notes of every pattern in the order the lane chose them, on a new note lane named "Pattern".

use super::*;
use atomik_model::{
    Clip, ClipContent, ClipId, MAX_NOTE_LANES, Note, PATTERN_PARAM, WorkflowCommand, player_stack,
};

/// The lane name of what Send Players to Track writes.
const SEND_LANE_NAME: &str = "Players";

impl<S: EngineSink> AppCore<S> {
    /// The selected tracks that have a player stack, each with its players (top first).
    fn stack_tracks(&self) -> Vec<(DeviceKey, Vec<DeviceKey>)> {
        let doc = self.doc();
        self.arrangement
            .command_tracks()
            .into_iter()
            .filter_map(|d| {
                let t = doc.song.track(&d)?;
                let stack = player_stack(doc, &self.registry, t);
                (!stack.is_empty()).then_some((d, stack))
            })
            .collect()
    }

    /// What the Track menu shows about the player stacks of the selection: `(any stack, all
    /// record direct, every player folded, every player bypassed)`.
    pub fn players_menu(&self) -> (bool, bool, bool, bool) {
        let doc = self.doc();
        let stacks = self.stack_tracks();
        let players = || stacks.iter().flat_map(|(_, s)| s.iter());
        let direct = !stacks.is_empty()
            && stacks
                .iter()
                .all(|(d, _)| doc.song.track(d).is_some_and(|t| t.direct_record));
        let folded =
            !stacks.is_empty() && players().all(|k| doc.device(k).is_some_and(|d| d.folded));
        let bypassed = !stacks.is_empty()
            && players().all(|k| {
                doc.device(k)
                    .is_some_and(|d| d.params.get("bypass").copied().unwrap_or(0.0) >= 0.5)
            });
        (!stacks.is_empty(), direct, folded, bypassed)
    }

    /// Applies `commands` as one undo step (the first opens it, the others join it).
    fn apply_as_one_step(&mut self, commands: Vec<Command>) -> bool {
        let mut all = true;
        for (i, command) in commands.into_iter().enumerate() {
            self.join_next = i > 0;
            let applied = self.apply_command(command).is_ok();
            self.join_next = false;
            if !applied {
                all = false;
                break;
            }
        }
        all
    }

    /// Switches Direct Record on for the selected tracks with a stack, or off when every one of
    /// them has it on.
    pub fn toggle_direct_record(&mut self) {
        let stacks = self.stack_tracks();
        if stacks.is_empty() {
            self.reject("Direct Record needs a track with players above its instrument".into());
            return;
        }
        let on = !self.players_menu().1;
        let commands = stacks
            .iter()
            .filter(|(d, _)| {
                self.doc()
                    .song
                    .track(d)
                    .is_some_and(|t| t.direct_record != on)
            })
            .map(|(d, _)| {
                Command::Song(SongCommand::SetDirectRecord {
                    device: d.clone(),
                    on,
                })
            })
            .collect::<Vec<_>>();
        self.apply_as_one_step(commands);
    }

    /// Folds every player of the selected stacks, or unfolds them when all are folded.
    pub fn fold_all_players(&mut self) {
        let keys: Vec<DeviceKey> = self
            .stack_tracks()
            .into_iter()
            .flat_map(|(_, s)| s)
            .collect();
        if keys.is_empty() {
            self.reject("Fold All Players needs a track with players above its instrument".into());
            return;
        }
        let folded = !self.players_menu().2;
        let _ = self.apply_command(Command::Workflow(WorkflowCommand::SetFoldedMany {
            keys,
            folded,
        }));
    }

    /// Bypasses every player of the selected stacks, or releases them when all are bypassed.
    pub fn bypass_all_players(&mut self) {
        let keys: Vec<DeviceKey> = self
            .stack_tracks()
            .into_iter()
            .flat_map(|(_, s)| s)
            .collect();
        if keys.is_empty() {
            self.reject(
                "Bypass All Players needs a track with players above its instrument".into(),
            );
            return;
        }
        let on = !self.players_menu().3;
        let value = if on { 1.0 } else { 0.0 };
        let commands = keys
            .iter()
            .filter(|k| {
                self.doc()
                    .device(k)
                    .is_some_and(|d| (d.params.get("bypass").copied().unwrap_or(0.0) >= 0.5) != on)
            })
            .map(|k| Command::SetParam {
                key: k.clone(),
                param: "bypass".to_string(),
                value,
                merge: None,
            })
            .collect::<Vec<_>>();
        self.apply_as_one_step(commands);
    }

    /// Renders what the stack of each selected track generates and writes it as a clip on a new
    /// note lane (see the module docs).
    pub fn send_players_to_track(&mut self) {
        let stacks = self.stack_tracks();
        if stacks.is_empty() {
            self.reject(
                "Send Players to Track needs a track with players above its instrument".into(),
            );
            return;
        }
        for (device, _) in stacks {
            self.send_stack_to_track(&device, SEND_LANE_NAME);
        }
    }

    /// Renders the notes the stack of `device` generates into a clip on a new note lane named
    /// `lane_name`.
    fn send_stack_to_track(&mut self, device: &DeviceKey, lane_name: &str) {
        let at_limit = self
            .doc()
            .song
            .track(device)
            .is_none_or(|t| t.frozen.is_some() || t.note_lane_count() >= MAX_NOTE_LANES);
        if at_limit {
            self.reject(format!(
                "{device} is frozen or has no room for another note lane"
            ));
            return;
        }
        let bundle = self.bundle_root().map(std::path::Path::to_path_buf);
        let notes = match atomik_render::players::render_player_notes(
            self.doc(),
            &self.registry,
            &atomik_render::project::ProjectOptions::default(),
            bundle.as_deref(),
            device,
        ) {
            Ok(notes) => notes,
            Err(e) => {
                self.reject(format!("rendering the players of {device} failed: {e}"));
                return;
            }
        };
        if notes.is_empty() {
            self.reject(format!("the players of {device} generated no notes"));
            return;
        }
        let song = &self.doc().song;
        let last = notes.iter().map(|n| n.tick + n.length).max().unwrap_or(0);
        let end = song.meter_table().map().next_bar_line((last - 1).max(0));
        let new_lane = song.track(device).map_or(0, |t| t.note_lane_count());
        let clip = Clip {
            id: ClipId(0),
            start: 0,
            length: end.max(last).max(1),
            offset: 0,
            loop_length: None,
            muted: false,
            name: None,
            color: None,
            grid: None,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            controllers: Vec::new(),
            content: ClipContent::Notes(
                notes
                    .iter()
                    .map(|n| Note::new(n.tick, n.length, n.key, n.velocity))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        };
        let applied = self.apply_as_one_step(vec![
            Command::Song(SongCommand::AddNoteLane {
                device: device.clone(),
                name: Some(lane_name.to_string()),
            }),
            Command::Song(SongCommand::AddClip {
                device: device.clone(),
                lane: new_lane,
                clip,
            }),
        ]);
        self.after_view_change();
        if applied {
            self.arrangement.set_selected_lane(device, new_lane);
        }
    }
}

/// The lane name of what Pattern Lane to Notes writes.
const PATTERN_NOTES_LANE_NAME: &str = "Pattern";

impl<S: EngineSink> AppCore<S> {
    /// True when `device` has a `pattern` param (a step or CV sequencer, a drum generator).
    fn takes_patterns(&self, device: &DeviceKey) -> bool {
        self.doc().resolve(device).is_some_and(|d| {
            self.registry
                .get(&d.type_id)
                .is_some_and(|f| f.descriptor.param_by_key(PATTERN_PARAM).is_some())
        })
    }

    /// The pattern devices the pattern commands act on: the device selected in the rack, else
    /// the selected tracks' devices and the players of their stacks that take patterns.
    fn pattern_candidates(&self) -> Vec<DeviceKey> {
        let doc = self.doc();
        let mut out: Vec<DeviceKey> = Vec::new();
        let take = |d: DeviceKey, out: &mut Vec<DeviceKey>| {
            if !out.contains(&d) && self.takes_patterns(&d) {
                out.push(d);
            }
        };
        if let Some(d) = self.view.selected() {
            take(d.clone(), &mut out);
        }
        if out.is_empty() {
            for d in self.arrangement.command_tracks() {
                if let Some(t) = doc.song.track(&d) {
                    for p in player_stack(doc, &self.registry, t) {
                        take(p, &mut out);
                    }
                }
                take(d, &mut out);
            }
        }
        out
    }

    /// Adds a pattern lane (the automation lane of the `pattern` param) for the selected pattern
    /// device or track: its clips switch the device's pattern. One undo step.
    pub fn create_pattern_lane(&mut self) {
        let candidates = self.pattern_candidates();
        if candidates.is_empty() {
            self.reject(
                "Add Pattern Lane needs a selected pattern device (step sequencer, CV sequencer or drum generator) or its track"
                    .into(),
            );
            return;
        }
        let commands: Vec<Command> = candidates
            .iter()
            .filter(|d| {
                self.doc()
                    .song
                    .track(d)
                    .is_none_or(|t| t.pattern_lane().is_none())
            })
            .map(|d| {
                Command::Song(SongCommand::AddAutomationLane {
                    device: d.clone(),
                    param: PATTERN_PARAM.to_owned(),
                })
            })
            .collect();
        if commands.is_empty() {
            self.reject("the selection has a pattern lane already".into());
            return;
        }
        self.apply_as_one_step(commands);
        self.after_view_change();
    }

    /// Renders the notes the players of the selected pattern lanes generate (the pattern lane
    /// switches included) into a clip on a new note lane of each track that plays them, as Send
    /// Players to Track does for a whole stack.
    pub fn pattern_lane_to_notes(&mut self) {
        let with_lane: Vec<DeviceKey> = self
            .pattern_candidates()
            .into_iter()
            .filter(|d| {
                self.doc()
                    .song
                    .track(d)
                    .is_some_and(|t| t.pattern_lane().is_some())
            })
            .collect();
        if with_lane.is_empty() {
            self.reject(
                "Pattern Lane to Notes needs a selected pattern device or track with a pattern lane"
                    .into(),
            );
            return;
        }
        let doc = self.doc();
        let targets: Vec<DeviceKey> = doc
            .song
            .tracks
            .iter()
            .filter(|t| {
                player_stack(doc, &self.registry, t)
                    .iter()
                    .any(|p| with_lane.contains(p))
            })
            .map(|t| t.device.clone())
            .collect();
        if targets.is_empty() {
            self.reject(format!(
                "no track plays {}: its notes go to no instrument",
                with_lane[0]
            ));
            return;
        }
        for device in targets {
            self.send_stack_to_track(&device, PATTERN_NOTES_LANE_NAME);
        }
    }
}
