//! Dub and Alt: a new overdub layer on the fly.
//!
//! Dub adds a note lane to every armed note track and makes it the lane the next pass records
//! to; with a take running, the take so far is written to the lane it began on (as a locate
//! does) and a new take starts on the new lane at once, so the notes played before the key press
//! and after it each land on their own layer. Alt does the same and also mutes the lane the last
//! pass went to, so the new layer replaces it. Each press is one undo step.
//!
//! Armed audio tracks are not dubbed (they keep recording as they were); frozen tracks and
//! tracks at the lane limit are skipped.

use super::*;
use atomik_model::MAX_NOTE_LANES;

impl<S: EngineSink> AppCore<S> {
    /// The armed note tracks a dub adds a lane to, each with the lane its last pass went to.
    fn dub_targets(&self) -> Vec<(DeviceKey, usize)> {
        let audio = self.armed_audio_tracks();
        self.doc()
            .song
            .tracks
            .iter()
            .filter(|t| t.armed && !audio.contains(&t.device))
            .filter(|t| !t.is_audio() && !t.is_automation_only() && t.frozen.is_none())
            .filter(|t| t.note_lane_count() < MAX_NOTE_LANES)
            .map(|t| (t.device.clone(), self.arrangement.record_lane(&t.device)))
            .collect()
    }

    /// Dub (`alt` false) or Alt (`alt` true): see the module docs.
    pub(super) fn dub(&mut self, alt: bool) {
        let targets = self.dub_targets();
        if targets.is_empty() {
            self.reject(
                "Dub needs an armed note track that is not frozen and has room for another lane"
                    .to_string(),
            );
            return;
        }
        // The take so far goes to the lane it began on, then a new one starts.
        let was_recording = self.take.is_some();
        if was_recording {
            let state = self.sink.transport_state();
            self.finish_take(&state);
        }
        let mut commands = Vec::new();
        for (device, lane) in &targets {
            commands.push(Command::Song(SongCommand::AddNoteLane {
                device: device.clone(),
                name: None,
            }));
            let muted = self
                .doc()
                .song
                .track(device)
                .and_then(|t| t.lanes.get(*lane))
                .is_some_and(|l| l.muted);
            if alt && !muted {
                commands.push(Command::Song(SongCommand::SetLaneMuted {
                    device: device.clone(),
                    lane: *lane,
                    muted: true,
                }));
            }
        }
        // One undo step: the first command opens it, the others join it.
        for (i, command) in commands.into_iter().enumerate() {
            self.join_next = i > 0;
            let applied = self.apply_command(command).is_ok();
            self.join_next = false;
            if !applied {
                break;
            }
        }
        self.after_view_change();
        for (device, _) in &targets {
            let new = self
                .doc()
                .song
                .track(device)
                .map_or(0, |t| t.note_lane_count().saturating_sub(1));
            self.arrangement.set_selected_lane(device, new);
        }
        if was_recording {
            self.start_recording();
        }
    }
}
