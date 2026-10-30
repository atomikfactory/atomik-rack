//! The daily keys in the app core.
//!
//! The shell resolves a key to registry commands and offers each to the focused view
//! ([`AppCore::pane_command`]); what is not a view's own (the transport steps, the create
//! commands) runs here. Nothing in this file touches the engine thread or the model: tools,
//! zoom, follow and snap are view state, edits go through the commands the views emit.

use super::*;
use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::{CommandOutcome, Roles};

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

    /// Stop with a second press: stops while playing or recording, returns to the start when
    /// already stopped (Numpad 0, Shift+Return).
    pub fn stop_or_rewind(&mut self) {
        if self.playing || self.take.is_some() {
            self.transport_action(TransportAction::Stop);
        } else {
            self.transport_action(TransportAction::ReturnToZero);
        }
    }

    /// Changes the tempo (the first tempo event) by `delta` beats per minute, undoable like
    /// the tempo field.
    pub fn nudge_tempo(&mut self, delta: f64) {
        let mut map = self.doc().song.tempo.clone();
        let Some(first) = map.first_mut() else {
            return;
        };
        let bpm = (first.bpm + delta).clamp(atomik_model::TEMPO_MIN, atomik_model::TEMPO_MAX);
        if (bpm - first.bpm).abs() < 1e-9 {
            return;
        }
        first.bpm = bpm;
        let _ = self.apply_command(Command::Song(SongCommand::SetTempoMap(map)));
    }

    /// Gives the time views the modifier role table of the keymap (the alternate tool, snap
    /// off while dragging).
    pub fn set_roles(&mut self, roles: &Roles) {
        self.arrangement.set_roles(roles.clone());
        self.piano_roll.set_roles(roles.clone());
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
