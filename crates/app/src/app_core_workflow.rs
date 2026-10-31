//! The daily keys in the app core.
//!
//! The shell resolves a key to registry commands and offers each to the focused view
//! ([`AppCore::pane_command`]); what is not a view's own (the transport steps, the create
//! commands) runs here. Nothing in this file touches the engine thread or the model: tools,
//! zoom, follow and snap are view state, edits go through the commands the views emit.

use super::*;
use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::{CommandOutcome, Roles};

/// Merge ids of the tempo taps start here (the views use multiples of 2^40 up to 13).
const TAP_MERGE_BASE: u64 = 14 << 40;

impl<S: EngineSink> AppCore<S> {
    /// Offers a registry command to the view of `pane`. `None`: the view did not take it
    /// (the next binding runs). A taken command's actions are applied (commands, selection,
    /// transport) and returned for the shell (opening panes); a refused command
    /// ([`CommandOutcome::NotApplicable`]) is reported and counts as taken.
    pub fn pane_command(
        &mut self,
        pane: Pane,
        id: &str,
        arg: Option<&str>,
    ) -> Option<Vec<UiAction>> {
        // Delete while recording throws away the take so far instead of deleting a selection.
        if id == "edit.delete" && self.discard_take_notes() {
            return Some(Vec::new());
        }
        if let Some(actions) = self.clipboard_command(pane, id) {
            return Some(actions);
        }
        let doc = self.document.doc();
        let outcome = match pane {
            Pane::Rack => self.view.command(id, arg, doc, &self.registry),
            Pane::Arrangement => self.arrangement.command(id, arg, doc, &self.registry),
            Pane::PianoRoll => self.piano_roll.command(id, arg, doc, &self.registry),
            Pane::StepEditor => self.step_editor.command(id, arg, doc, &self.registry),
            Pane::SampleEditor => self.sample_editor.command(id, arg, doc, &self.registry),
            Pane::SliceEditor => self.slice_editor.command(id, arg, doc, &self.registry),
            Pane::AudioEditor => self.audio_editor.command(id, arg, doc, &self.registry),
            Pane::Console => self.console.command(id, arg, doc, &self.registry),
            _ => CommandOutcome::Declined,
        };
        match outcome {
            CommandOutcome::Declined => None,
            CommandOutcome::NotApplicable(message) => {
                self.report(message);
                Some(Vec::new())
            }
            CommandOutcome::Handled(actions) => {
                for action in &actions {
                    self.handle_action(action.clone());
                }
                self.after_view_change();
                Some(actions)
            }
        }
    }

    /// Moves the playhead to the next bar line (`bars > 0`) or the previous one (`bars < 0`),
    /// `|bars|` times, never before the start. From the middle of a bar, one step back goes to
    /// the start of that bar.
    pub fn locate_bars(&mut self, bars: i64) {
        let table = self.doc().song.meter_table();
        let meter = table.map();
        let mut tick = self.position;
        for _ in 0..bars.unsigned_abs() {
            tick = if bars > 0 {
                meter.next_bar_line(tick)
            } else {
                meter.prev_bar_line(tick)
            };
        }
        self.transport_action(TransportAction::Locate(tick.max(0)));
    }

    /// Jumps to the left or the right locator (the song loop's ends).
    pub fn go_locator(&mut self, right: bool) {
        let (start, end) = self.doc().song.loop_range;
        self.transport_action(TransportAction::Locate(if right { end } else { start }));
    }

    /// Moves the loop by `bars` bar lines (its length stays), or changes its length by `bars`
    /// bars (the right end moves, the loop keeps at least one bar), as one undo step. The loop
    /// does not go before the first bar; a change that would is not made. The enabled flag
    /// stays.
    pub fn move_loop(&mut self, bars: i64, resize: bool) {
        let table = self.doc().song.meter_table();
        let meter = table.map();
        let (start, end) = self.doc().song.loop_range;
        let enabled = self.doc().song.loop_enabled;
        let shift = |tick: i64| -> Option<i64> {
            let bar = i64::from(meter.bar_at(tick)) + bars;
            let bar = u32::try_from(bar).ok().filter(|b| *b >= 1)?;
            Some(meter.bar_start(bar))
        };
        let range = if resize {
            let end = shift(end).filter(|e| *e >= meter.next_bar_line(start));
            end.map(|e| (start, e))
        } else {
            shift(start).map(|s| (s, end + (s - start)))
        };
        let Some(range) = range.filter(|r| *r != (start, end)) else {
            return;
        };
        // A rejection is already reported.
        let _ = self.apply_command(Command::Song(SongCommand::SetLoop { range, enabled }));
    }

    /// Releases every sounding note: all-notes-off to the Piano Keys' held targets and to every
    /// device that takes notes (a stuck note from a MIDI keyboard, an audition or a plugin).
    pub fn all_notes_off(&mut self) {
        let current = self.notes_target();
        let mut sends = Vec::new();
        self.keys_router
            .route(NoteKind::AllOff, current, |target, kind| {
                sends.push((target, kind));
            });
        for (target, kind) in sends {
            let result = self.sink.play_note(target, kind);
            self.engine_result(result);
        }
        let mut targets: Vec<Target> = Vec::new();
        for d in &self.doc().devices {
            if let Some(t) = self.target_of(&d.key)
                && !targets.contains(&t)
            {
                targets.push(t);
            }
        }
        for target in targets {
            let result = self.sink.live_note(target, NoteKind::AllOff);
            self.engine_result(result);
        }
    }

    /// Stop with a second press: stops while playing or recording, returns to the start when
    /// already stopped (Numpad 0, Shift+Return).
    pub fn stop_or_rewind(&mut self) {
        if self.playing || self.take.is_some() {
            self.transport_action(TransportAction::Stop);
        } else {
            self.transport_action(TransportAction::ReturnToZero);
        }
    }

    /// The index of the tempo event in effect at the playhead (what the tempo readout edits).
    fn tempo_event_at_playhead(&self) -> usize {
        let at = self.position.max(0);
        self.doc()
            .song
            .tempo
            .partition_point(|e| e.tick <= at)
            .saturating_sub(1)
    }

    /// The tempo map with the event in effect at the playhead set to `bpm` (clamped); `None`
    /// when nothing changes.
    fn tempo_map_at_playhead(&self, bpm: f64) -> Option<Vec<atomik_model::TempoEvent>> {
        let mut map = self.doc().song.tempo.clone();
        let event = map.get_mut(self.tempo_event_at_playhead())?;
        let bpm = bpm.clamp(atomik_model::TEMPO_MIN, atomik_model::TEMPO_MAX);
        if !bpm.is_finite() || (bpm - event.bpm).abs() < 1e-9 {
            return None;
        }
        event.bpm = bpm;
        Some(map)
    }

    /// Changes the tempo in effect at the playhead by `delta` beats per minute, undoable like
    /// the tempo field (which edits the same event).
    pub fn nudge_tempo(&mut self, delta: f64) {
        let Some(event) = self.doc().song.tempo.get(self.tempo_event_at_playhead()) else {
            return;
        };
        let bpm = event.bpm + delta;
        if let Some(map) = self.tempo_map_at_playhead(bpm) {
            let _ = self.apply_command(Command::Song(SongCommand::SetTempoMap(map)));
        }
    }

    /// A tap of the tap-tempo button: with two taps or more the tempo in effect at the
    /// playhead becomes their average. The taps of one sequence are one undo step.
    pub fn tap_tempo(&mut self) {
        let bpm = self.tap.tap(self.now_seconds);
        if self.tap.len() == 1 {
            self.tap_merge += 1;
        }
        let Some(map) = bpm.and_then(|b| self.tempo_map_at_playhead(b)) else {
            return;
        };
        let merge = TAP_MERGE_BASE + self.tap_merge;
        let _ = self.apply_command(Command::Song(SongCommand::DragTempoMap { map, merge }));
    }

    /// Per-frame work of the transport bar at `now_seconds` (any monotonic clock): the held
    /// Rewind and Forward repeat, and the taps of tap tempo are timed by it.
    pub fn transport_frame(&mut self, now_seconds: f64) {
        self.now_seconds = now_seconds;
        for action in self.transport_bar.tick(now_seconds) {
            self.handle_action(action);
        }
    }

    /// Stop returns to where playback started (Workflow preference); the second Stop goes to
    /// the start.
    pub fn set_return_to_start_on_stop(&mut self, on: bool) {
        self.return_to_start_on_stop = on;
    }

    /// Where playback last started from.
    pub fn last_start(&self) -> Option<i64> {
        self.last_start
    }

    /// What the running take has captured, per armed track (`None` when not recording): shown
    /// as a growing red clip. Nothing here is saved or undoable.
    pub fn take_preview(&self) -> Option<Vec<atomik_ui::arrangement::TakeGhost>> {
        let take = self.take.as_ref()?;
        let start = take.origin()?.0;
        let now = self.position;
        let mut ghosts: Vec<_> = take
            .track_previews(now)
            .into_iter()
            .map(|(device, lane, notes)| atomik_ui::arrangement::TakeGhost {
                device,
                lane,
                start,
                notes: notes.iter().map(|n| (n.tick, n.length, n.key)).collect(),
            })
            .collect();
        // An audio take shows its span only, on the track's own row.
        ghosts.extend(self.armed_audio_tracks().into_iter().map(|device| {
            atomik_ui::arrangement::TakeGhost {
                device,
                lane: 0,
                start,
                notes: Vec::new(),
            }
        }));
        Some(ghosts)
    }

    /// Drops the notes the running take captured so far; the transport and the take go on.
    /// False when nothing is recording.
    pub fn discard_take_notes(&mut self) -> bool {
        if self.take.is_none() {
            return false;
        }
        let state = self.sink.transport_state();
        self.feed_take(&state);
        if let Some(take) = &mut self.take {
            take.discard_notes();
        }
        self.show_transport();
        true
    }

    /// Gives the time views the modifier role table of the keymap (the alternate tool, snap
    /// off while dragging).
    pub fn set_roles(&mut self, roles: &Roles) {
        self.arrangement.set_roles(roles.clone());
        self.piano_roll.set_roles(roles.clone());
        self.audio_editor.set_roles(roles.clone());
        self.view.set_roles(roles.clone());
        self.console.set_roles(roles.clone());
    }

    /// Sets the start state of follow in the arrangement and the piano roll (the Workflow
    /// preference "Follow song while playing").
    pub fn set_follow(&mut self, follow: bool) {
        self.arrangement.set_follow(follow);
        self.piano_roll.set_follow(follow);
    }

    /// Adds an audio track (Create Audio Track): one undo step.
    pub fn add_audio_track(&mut self) {
        let _ = self.apply_command(Command::Song(SongCommand::AddAudioTrack { name: None }));
    }

    /// Opens the device browser in create mode (Create Instrument, Create Effect).
    pub fn open_create_browser(&mut self, kind: CreateKind) {
        self.view.open_browser_create(kind, &self.registry);
    }

    /// Closes the editor that shows in place of the arrangement (piano roll, sample, step,
    /// slice, sequencer, container or plugin parameter pane); false when none is open.
    pub fn close_open_editor(&mut self) -> bool {
        if self.piano_clip.is_some() {
            self.close_piano_roll();
        } else if self.sample_editor_device().is_some() {
            self.close_sample_editor();
        } else if self.step_editor_device().is_some() {
            self.close_step_editor();
        } else if self.slice_editor_device().is_some() {
            self.close_slice_editor();
        } else if self.audio_editor_clip().is_some() {
            self.close_audio_editor();
        } else if self.seq_editor_device().is_some() {
            self.close_seq_editor();
        } else if self.container_editor_device().is_some() {
            self.close_container_editor();
        } else if self.plugin_params_device().is_some() {
            self.close_plugin_params();
        } else {
            return false;
        }
        true
    }
}
