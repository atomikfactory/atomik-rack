//! The automatic colour of new tracks (Interface > Colour new tracks): a track that a command
//! has just made and that has no colour of its own takes the palette colour the fewest tracks
//! use, the first one on a tie, so the colours go round the palette and a deleted track's
//! colour is used again. The track and its device take it together, as the colour menu does,
//! and the colour joins the undo step of the command that made the track.

use super::*;
use atomik_model::{SongCommand, VCA_PALETTE};

impl<S: EngineSink> AppCore<S> {
    /// Whether new tracks take a colour of their own.
    pub fn auto_color(&self) -> bool {
        self.auto_color
    }

    /// Sets whether new tracks take the next palette colour.
    pub fn set_auto_color(&mut self, on: bool) {
        self.auto_color = on;
    }

    /// The tracks before a command runs, when new ones are to be coloured (nothing otherwise,
    /// so the common case costs nothing).
    pub(super) fn tracks_before_command(&self) -> Option<Vec<DeviceKey>> {
        self.auto_color.then(|| {
            self.document
                .doc()
                .song
                .tracks
                .iter()
                .map(|t| t.device.clone())
                .collect()
        })
    }

    /// Colours the tracks the command made (those not in `before`) that have no colour yet, in
    /// the undo step of the command.
    pub(super) fn colour_new_tracks(&mut self, before: Option<Vec<DeviceKey>>) {
        let Some(before) = before else {
            return;
        };
        let song = &self.document.doc().song;
        let new: Vec<DeviceKey> = song
            .tracks
            .iter()
            .filter(|t| !before.contains(&t.device) && song.track_color(&t.device).is_none())
            .map(|t| t.device.clone())
            .collect();
        if new.is_empty() {
            return;
        }
        let mut used: Vec<usize> = VCA_PALETTE
            .iter()
            .map(|c| song.org.colors.values().filter(|u| *u == c).count())
            .collect();
        let mut commands = Vec::with_capacity(new.len() * 2);
        for device in new {
            let at = (0..VCA_PALETTE.len())
                .min_by_key(|i| used[*i])
                .unwrap_or_default();
            used[at] += 1;
            let color = Some(VCA_PALETTE[at]);
            commands.push(Command::Song(SongCommand::SetTrackColor {
                device: device.clone(),
                color,
            }));
            commands.push(Command::SetColor { key: device, color });
        }
        self.join_next = true;
        // A rejection is already reported to the view; the track keeps its device's colour.
        let _ = self.apply_command(Command::Batch(commands));
        self.join_next = false;
    }
}
