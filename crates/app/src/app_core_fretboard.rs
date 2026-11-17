//! The fretboard in the app core.
//!
//! One [`FretboardView`] is the pane of the Fretboard window ([`Pane::Fretboard`](crate::Pane));
//! the strip under the Tab Editor staff is drawn by the tab editor itself. Both show the clip the
//! tab editor shows, at the tab cursor (stopped) or the playhead (playing), and both get the same
//! [`FretboardOptions`] from here: the shell owns the choices (they live in `ui-state.json`) and
//! pushes them with [`AppCore::set_fretboard_options`].

use super::*;
use atomik_ui::{FretboardOptions, FretboardView};

impl<S: EngineSink> AppCore<S> {
    /// The fretboard window's view.
    pub fn fretboard(&self) -> &FretboardView {
        &self.fretboard
    }

    /// The fretboard window's view, mutably (bounds, scale).
    pub fn fretboard_mut(&mut self) -> &mut FretboardView {
        &mut self.fretboard
    }

    /// The options both fretboards are drawn with.
    pub fn fretboard_options(&self) -> FretboardOptions {
        self.fretboard.options()
    }

    /// Draws both fretboards with `options`.
    pub fn set_fretboard_options(&mut self, options: FretboardOptions) {
        self.fretboard.set_options(options);
        self.tab_editor.set_fretboard_options(options);
    }

    /// Shows or hides the strip under the tab staff.
    pub fn set_fretboard_strip(&mut self, shown: bool) {
        self.tab_editor.set_fretboard_shown(shown);
    }

    /// The fretboard window opened or closed (the shell): it is fed only while it is open.
    pub fn set_fretboard_window(&mut self, open: bool) {
        self.fretboard_window = open;
        if open {
            self.sync_fretboard();
        }
    }

    /// Gives the fretboard window what the tab editor shows now. Cheap when nothing changed, and
    /// nothing at all while the window is closed.
    pub fn sync_fretboard(&mut self) {
        if !self.fretboard_window {
            return;
        }
        let feed = self.tab_editor.fretboard_feed();
        self.fretboard.set_feed(&feed);
    }
}
