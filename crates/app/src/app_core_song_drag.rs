//! A drag of the selection from one song to another song's window.
//!
//! Dragging clips, devices or tracks and letting go over a window of another open song has the
//! same result as Copy in the first song and Paste in the second, audio and sample rate
//! included (see [`AppCore::set_audio_lenders`] and the paste at another rate): the first song
//! keeps what it has and the second takes one undo step. The drag in the first song is
//! cancelled ([`AppCore::end_carried_drag`]), what it carried is the snapshot Copy would take
//! ([`AppCore::carried_data`], the clipboard is left alone), and the second song places it at
//! the point of the drop ([`AppCore::drop_carried`]): clips on the lane and the snapped tick
//! under the pointer, devices at the rack slot there, tracks after the track there.

use super::*;
use crate::clipboard::ClipboardData;
use atomik_model::{ClipSnapshot, WorkflowCommand};

/// Whether every clip of the snapshot can sit on the lane `lane` of `device`'s track: note
/// clips on a note lane, audio clips on an audio track. The model decides for anything else.
fn fits_lane(
    song: &atomik_model::Song,
    clips: &ClipSnapshot,
    device: &DeviceKey,
    lane: usize,
) -> bool {
    let Some(track) = song.track(device) else {
        return false;
    };
    let notes = clips.entries.iter().all(|e| e.lane.is_notes());
    let audio = clips.entries.iter().all(|e| e.lane.is_audio());
    (notes && track.is_note_lane(lane)) || (audio && track.is_audio())
}

impl<S: EngineSink> AppCore<S> {
    /// What the drag of the selection in `pane` carries: the snapshot Copy would take (clips,
    /// devices or tracks), without touching the clipboard. `None` when nothing is selected or
    /// the selection is something that does not travel between songs.
    pub fn carried_data(&self, pane: Pane) -> Option<ClipboardData> {
        self.capture(pane).ok().filter(|d| {
            matches!(
                d,
                ClipboardData::Clips(_) | ClipboardData::Devices(_) | ClipboardData::Tracks(_)
            )
        })
    }

    /// Ends the drag of the selection in `pane` without moving anything (the pointer was let go
    /// over another song): what the drag had moved so far is put back.
    pub fn end_carried_drag(&mut self, pane: Pane) {
        match pane {
            Pane::Arrangement => {
                for action in self.arrangement.cancel_carried() {
                    self.handle_action(action);
                }
            }
            Pane::Rack => self.view.drop_carried(),
            _ => {}
        }
    }

    /// Pastes what another song dragged here, at `pos` of `pane` (the pane's own coordinates,
    /// as its views take them): one undo step, with the audio brought along and converted to
    /// this song's rate. A refusal is the message to show.
    pub fn drop_carried(
        &mut self,
        pane: Pane,
        pos: Point,
        data: ClipboardData,
    ) -> Result<(), String> {
        match (pane, data) {
            (Pane::Arrangement, ClipboardData::Clips(clips)) => {
                let (track, lane, at_tick) = match self.arrangement.drop_spot_at(pos) {
                    Some((Some((device, lane)), tick)) => {
                        if fits_lane(&self.doc().song, &clips, &device, lane) {
                            let note_lane = clips.entries.iter().all(|e| e.lane.is_notes());
                            (Some(device), note_lane.then_some(lane), tick)
                        } else {
                            (None, None, tick)
                        }
                    }
                    Some((None, tick)) => (None, None, tick),
                    None => (None, None, self.position),
                };
                self.apply_workflow(WorkflowCommand::PasteClips {
                    track,
                    lane,
                    at_tick,
                    clips,
                })
            }
            (Pane::Arrangement, ClipboardData::Tracks(tracks)) => {
                // After the track under the pointer, as a paste goes after the selected one.
                if let Some((Some((device, _)), _)) = self.arrangement.drop_spot_at(pos) {
                    self.arrangement.set_selected_track(Some(device));
                }
                self.paste_devices(tracks, Pane::Arrangement)
            }
            (Pane::Rack, ClipboardData::Devices(snapshot) | ClipboardData::Tracks(snapshot)) => {
                match self.view.drop_slot_at(pos, self.document.doc()) {
                    Some((container, slot)) => {
                        let route = self.view.auto_route();
                        self.apply_workflow(WorkflowCommand::PasteDevicesAt {
                            snapshot,
                            container,
                            slot,
                            route,
                        })
                    }
                    None => self.paste_devices(snapshot, Pane::Rack),
                }
            }
            (Pane::Arrangement, ClipboardData::Devices(_)) => {
                Err("Drop devices on the rack, or tracks on the arrangement".into())
            }
            (Pane::Rack, ClipboardData::Clips(_)) => Err("Drop clips on the arrangement".into()),
            _ => Err("Drop on the arrangement or the rack".into()),
        }
    }
}
