//! The Tab Editor in the app core.
//!
//! [`Pane::TabEditor`](crate::Pane::TabEditor) shows [`TabEditorView`] in place of the arrangement,
//! as the piano roll does, on one note clip. It opens from View > Editors > Tab Editor
//! (`view.toggle_tab_editor`) on the selected note clip (else the open piano roll clip, else the
//! first note clip of the selected device's track, else the song's first) and closes with the
//! other editors. `edit.toggle_tab_or_piano_roll` switches the open clip between the two editors.
//! The editor's edits are `Command::Batch`es of the clip's notes and its tab sidecar, applied like
//! the piano roll's.

use super::*;

impl<S: EngineSink> AppCore<S> {
    /// The tab editor view.
    pub fn tab_editor(&self) -> &TabEditorView {
        &self.tab_editor
    }

    /// The tab editor view, mutably (bounds, scale, focus).
    pub fn tab_editor_mut(&mut self) -> &mut TabEditorView {
        &mut self.tab_editor
    }

    /// The clip open in the tab editor.
    pub fn tab_editor_clip(&self) -> Option<ClipId> {
        self.tab_clip
    }

    /// Whether the tab editor is open.
    pub fn tab_editor_open(&self) -> bool {
        self.tab_clip.is_some()
    }

    /// Opens a note clip in the tab editor (ignored for other clips). The other editors close.
    pub fn open_tab_editor(&mut self, id: ClipId) {
        let is_notes = self
            .doc()
            .song
            .clip_or_take(id)
            .is_some_and(|(_, c)| c.content.is_notes());
        if !is_notes {
            return;
        }
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.tab_editor.open(id);
        self.tab_clip = Some(id);
        self.tab_editor.sync(self.document.doc(), &self.registry);
    }

    /// Closes the tab editor (the arrangement shows again).
    pub fn close_tab_editor(&mut self) {
        if self.tab_clip.is_none() {
            return;
        }
        for action in self.tab_editor.release_audition() {
            self.handle_action(action);
        }
        self.tab_editor.close();
        self.tab_clip = None;
    }

    /// The note clip an editor command opens: the first selected note clip of the arrangement,
    /// else the first note clip of the selected device's track, else the song's first.
    fn default_note_clip(&self) -> Option<ClipId> {
        let song = &self.doc().song;
        let is_notes = |id: &ClipId| song.clip(*id).is_some_and(|(_, _, c)| c.content.is_notes());
        let first_in = |t: &atomik_model::Track| {
            t.lanes
                .iter()
                .filter(|l| l.kind.is_notes())
                .flat_map(|l| l.clips.iter())
                .map(|c| c.id)
                .next()
        };
        self.arrangement
            .selected_clips()
            .into_iter()
            .find(is_notes)
            .or_else(|| {
                self.view
                    .selected()
                    .and_then(|k| song.track(k))
                    .and_then(first_in)
            })
            .or_else(|| song.tracks.iter().find_map(first_in))
    }

    /// Opens the tab editor on the clip the piano roll shows, else on the default note clip.
    /// False when the song has no note clip.
    pub fn open_tab_editor_default(&mut self) -> bool {
        let id = self.piano_clip.or_else(|| self.default_note_clip());
        match id {
            Some(id) => {
                self.open_tab_editor(id);
                self.tab_clip.is_some()
            }
            None => false,
        }
    }

    /// Switches the open clip between the piano roll and the tab editor; with neither open it
    /// opens the tab editor on the default note clip. False when there is no note clip.
    pub fn toggle_tab_or_piano_roll(&mut self) -> bool {
        if let Some(id) = self.tab_clip {
            self.close_tab_editor();
            self.open_piano_roll(id);
            return self.piano_clip.is_some();
        }
        if let Some(id) = self.piano_clip {
            self.open_tab_editor(id);
            return self.tab_clip.is_some();
        }
        self.open_tab_editor_default()
    }

    /// Per-frame clock of the tab editor (`now` in seconds): ends the note a typed fret
    /// sounds and the fret whose digits ran out.
    pub fn tab_editor_tick(&mut self, now: f64) {
        for action in self.tab_editor.tick(now) {
            self.handle_action(action);
        }
    }
}
