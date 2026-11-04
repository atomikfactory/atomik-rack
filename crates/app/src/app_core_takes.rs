//! Takes follow-ups in the app core: the takes clipboard, a file imported as a take, and a take
//! opened in an editor.
//!
//! **Copy and paste.** Copy Take and Copy All Takes in a take menu (and Edit > Copy while a take
//! row has the keyboard focus) put a `TakeSnapshot` on the in-process clipboard, the one that
//! survives New and Open and that every open song shares (the workspace carries it along when
//! the routed song changes). Paste Takes puts the takes into a take group as new takes on top
//! (the group named in the menu, or the focused row's); with no take row focused, Edit > Paste
//! starts a new group on the selected lane at the playhead. The pool entries of audio takes are
//! registered in the target song before the paste, as for clips.
//!
//! **Import.** Import File as Take... asks for an audio file (an audio group) or a MIDI file
//! (a note group) and adds it to the group as a new take starting where the group starts. The
//! request is remembered in `take_import` until the file dialog answers.
//!
//! **Open.** A take opens in the piano roll or the audio editor by its clip id; the editors'
//! commands then edit the take (see the model's `plan_take_edit`).

use super::*;
use crate::clipboard::ClipboardData;
use atomik_model::{TakeSnapshot, TakeTarget, WorkflowCommand};

impl<S: EngineSink> AppCore<S> {
    /// Asks the next file the import dialog returns to become a take of group `group`
    /// (`None` forgets the request).
    pub fn set_take_import(&mut self, group: Option<u32>) {
        self.take_import = group;
    }

    /// The group the next imported file becomes a take of, consuming the request.
    pub fn take_import_request(&mut self) -> Option<u32> {
        self.take_import.take()
    }

    /// Copies takes `takes` (positions in the group's take list) of group `group` to the
    /// clipboard (`UiAction::CopyTakes`). A refusal is reported.
    pub fn copy_takes(&mut self, group: u32, takes: &[usize]) {
        match TakeSnapshot::capture(self.document.doc(), group, takes) {
            Ok(snapshot) => {
                let n = snapshot.takes.len();
                self.clipboard.set(ClipboardData::Takes(snapshot));
                self.report(format!("Copied {n} take{}", if n == 1 { "" } else { "s" }));
            }
            Err(e) => self.report(e.to_string()),
        }
    }

    /// Pastes the clipboard's takes into group `group` (`UiAction::PasteTakes`): the earliest
    /// copied take starts where the group starts. A refusal is reported.
    pub fn paste_takes(&mut self, group: u32) {
        let result = self
            .clipboard_takes()
            .and_then(|s| self.paste_takes_into(group, s));
        if let Err(message) = result
            && !message.is_empty()
        {
            self.report(message);
        }
    }

    /// The takes on the clipboard.
    fn clipboard_takes(&self) -> Result<TakeSnapshot, String> {
        match self.clipboard.get() {
            Some(ClipboardData::Takes(s)) if !s.takes.is_empty() => Ok(s.clone()),
            Some(held) if !held.is_empty() => Err(format!(
                "The clipboard holds {}: copy takes from a take row first",
                held.kind().noun()
            )),
            _ => Err("The clipboard is empty: copy takes from a take row first".into()),
        }
    }

    fn paste_takes_into(&mut self, group: u32, takes: TakeSnapshot) -> Result<(), String> {
        let at_tick = self
            .doc()
            .song
            .take_group(group)
            .map(|(_, _, g)| g.range().0)
            .ok_or_else(|| "That take group is gone".to_string())?;
        self.apply_workflow(WorkflowCommand::PasteTakes {
            target: TakeTarget::Group(group),
            at_tick,
            takes,
        })
    }

    /// Edit > Paste with takes on the clipboard: into the group of the focused take row, else
    /// as a new group on the selected lane at the playhead.
    pub(super) fn paste_takes_here(&mut self, takes: TakeSnapshot) -> Result<(), String> {
        if let Some((group, _)) = self.arrangement.take_focus() {
            return self.paste_takes_into(group, takes);
        }
        let (device, lane) = self
            .arrangement
            .selected_lane()
            .map(|(d, l)| (d.clone(), l))
            .ok_or_else(|| {
                "Select a take row, or a track to start a take group on, to paste the takes into"
                    .to_string()
            })?;
        let at_tick = self.position;
        self.apply_workflow(WorkflowCommand::PasteTakes {
            target: TakeTarget::Lane { device, lane },
            at_tick,
            takes,
        })
    }

    /// Capture for Edit > Copy while a take row has the focus: that take.
    pub(super) fn capture_focused_take(&self) -> Option<Result<ClipboardData, String>> {
        let (group, take) = self.arrangement.take_focus()?;
        Some(
            TakeSnapshot::capture(self.document.doc(), group, &[take])
                .map(ClipboardData::Takes)
                .map_err(|e| e.to_string()),
        )
    }

    /// Edit > Cut while a take row has the focus: deletes that take (after the copy).
    pub(super) fn delete_focused_take(&mut self) -> Option<Result<(), String>> {
        let (group, take) = self.arrangement.take_focus()?;
        Some(
            self.apply_command(Command::Song(SongCommand::DeleteTake { group, take }))
                .map_err(|_| String::new()),
        )
    }

    /// Opens take `id` in the piano roll or the audio editor, whichever its content needs
    /// (used by tests and the shell; the views send `OpenPianoRoll` / `OpenAudioEditor`).
    pub fn open_take(&mut self, id: ClipId) {
        match self
            .doc()
            .song
            .clip_or_take(id)
            .map(|(_, c)| c.content.is_audio())
        {
            Some(true) => self.open_audio_editor(id),
            Some(false) => self.open_piano_roll(id),
            None => {}
        }
    }
}
