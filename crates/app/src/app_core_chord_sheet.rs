//! The Chord Sheet pane in the app core.
//!
//! [`Pane::ChordSheet`](crate::Pane::ChordSheet) shows [`ChordSheetView`] in place of the
//! arrangement, as the piano roll and the tab editor do. It is not tied to a clip: it shows the
//! song's sheets (`Song::sheets`), opens from View > Editors > Chord Sheet
//! (`view.toggle_chord_sheet`) and closes with the other editors. The view's edits are
//! `SongCommand::SetSheets`, applied like any command; File > Import > Chord Sheet adds a sheet
//! and the pane shows it.

use super::*;

impl<S: EngineSink> AppCore<S> {
    /// The chord sheet view.
    pub fn chord_sheet(&self) -> &ChordSheetView {
        &self.chord_sheet
    }

    /// The chord sheet view, mutably (bounds, scale, focus).
    pub fn chord_sheet_mut(&mut self) -> &mut ChordSheetView {
        &mut self.chord_sheet
    }

    /// Whether the chord sheet pane is open.
    pub fn chord_sheet_open(&self) -> bool {
        self.sheet_open
    }

    /// Opens the chord sheet pane. The other editors close.
    pub fn open_chord_sheet(&mut self) {
        self.close_chord_trainer();
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.sheet_open = true;
        self.chord_sheet.sync(self.document.doc(), &self.registry);
    }

    /// Closes the chord sheet pane (the arrangement shows again). Text being typed in it is
    /// dropped with it.
    pub fn close_chord_sheet(&mut self) {
        if !self.sheet_open {
            return;
        }
        self.chord_sheet.leave_text();
        self.sheet_open = false;
    }

    /// Per-frame clock of the chord sheet (`now` in seconds): advances the speed autoscroll.
    pub fn chord_sheet_tick(&mut self, now: f64) {
        if self.sheet_open {
            self.chord_sheet.tick(now);
        }
    }
}
