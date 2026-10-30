//! Drum machines in the app core: the step editor pane, the
//! Edit button's "create a pattern" path and the per-device-type import rule.
//!
//! # Imports
//!
//! `UiAction::ImportSamples` goes through the same worker, asset cache and one-`SetZones`
//! step as for the sampler; only the zone rule differs, by device type
//! ([`AppCore::import_zones`]): a sampler maps the files with `auto_map`, a drum machine puts
//! them on the pad under `at_key` (`with_pad_files`, velocity layers) or, with no key, fills
//! the empty pads in order (`fill_empty_pads`; files left over are counted in the status
//! message). Maps go to the engine exactly as for the sampler (`takes_samples`).
//!
//! # Step editor pane
//!
//! [`Pane::StepEditor`](crate::Pane::StepEditor) shows [`StepEditorView`] in place of the
//! arrangement, exclusive with the piano roll, the sample editor and the console (the shell
//! owns the console). It opens on [`UiAction::OpenStepEditor`] (a `None` clip is resolved
//! with [`StepEditorView::resolve_clip`]: the selected note clip of the drum machine's track,
//! else the clip under the playhead, else the empty state), on a double-click on a note clip
//! of a drum machine's track in the arrangement (the arrangement emits `OpenPianoRoll`; the
//! app branches by device type; the piano roll stays reachable with View > Piano Roll), and
//! on View > Step Editor. It closes on [`UiAction::CloseStepEditor`], when the device or the
//! clip goes away (`is_present`), and on a new or opened document.
//!
//! # Creating a pattern
//!
//! [`UiAction::CreateStepClip`] adds a note clip of `16 x 1/16` (one 4/4 bar) with that grid
//! at the playhead snapped to the bar. The clip is one undo step: an `AddClip`, or, if the
//! device has no track yet, a `Command::Batch` of `AddTrack` and `AddClip`, so one undo takes
//! the clip and the new track away together.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, RackDevice, SongCommand, StepGrid,
    StepResolution, ZoneDoc, auto_map, fill_empty_pads, pad_of_key, with_pad_files,
};
use atomik_ui::StepEditorView;

/// The drum machine's device type.
pub(super) const DRUM_MACHINE: &str = "atomik.drum-machine";

/// The grid of a new pattern: 16 steps of 1/16, no swing.
const NEW_GRID: StepGrid = StepGrid {
    resolution: StepResolution::Sixteenth,
    steps: 16,
    swing: 0,
};

/// The zones after importing `files` into `device`, and the names of the files that found no
/// pad (a drum machine's background drop with more files than empty pads; in the byte order
/// of the file names, the last ones). A granular sampler's one zone is the first file, replacing
/// the old zones.
pub(super) fn zones_after_import(
    device: &RackDevice,
    files: &[(String, String, u64)],
    at_key: Option<u8>,
) -> (Vec<ZoneDoc>, Vec<String>) {
    if device.type_id == DRUM_MACHINE {
        return match at_key.and_then(pad_of_key) {
            Some(pad) => (with_pad_files(&device.zones, pad, files), Vec::new()),
            None => {
                let (zones, dropped) = fill_empty_pads(&device.zones, files);
                let mut names: Vec<&String> = files.iter().map(|f| &f.1).collect();
                names.sort();
                let skip = names.len() - dropped.min(names.len());
                (zones, names.into_iter().skip(skip).cloned().collect())
            }
        };
    }
    if device.type_id == super::samples::GRANULAR {
        // One sample: the first file replaces the old one.
        return (auto_map(&files[..files.len().min(1)], at_key), Vec::new());
    }
    let mut zones = device.zones.clone();
    zones.extend(auto_map(files, at_key));
    (zones, Vec::new())
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ step editor pane

    /// The step editor.
    pub fn step_editor(&self) -> &StepEditorView {
        &self.step_editor
    }

    /// The step editor, mutably (bounds, scale, focus).
    pub fn step_editor_mut(&mut self) -> &mut StepEditorView {
        &mut self.step_editor
    }

    /// The drum machine open in the step editor (shown in place of the arrangement).
    pub fn step_editor_device(&self) -> Option<&DeviceKey> {
        self.step_editor.device()
    }

    fn is_drum_machine(&self, device: &DeviceKey) -> bool {
        self.doc()
            .device(device)
            .is_some_and(|d| d.type_id == DRUM_MACHINE)
    }

    /// The drum machine whose track holds clip `id`, if any.
    pub(super) fn drum_device_of_clip(&self, id: ClipId) -> Option<DeviceKey> {
        let (track, _, _) = self.doc().song.clip(id)?;
        let device = track.device.clone();
        self.is_drum_machine(&device).then_some(device)
    }

    /// Opens the step editor on a drum machine (ignored for other devices) and a clip; `None`
    /// resolves the selected note clip of its track, else the clip under the playhead, else
    /// the empty state. It replaces the piano roll and the sample editor.
    pub fn open_step_editor(&mut self, device: DeviceKey, clip: Option<ClipId>) {
        if self.refuse_inner(&device, "the step editor") {
            return;
        }
        if !self.is_drum_machine(&device) {
            return;
        }
        let clip = clip.or_else(|| {
            let selected = self.arrangement.selected_clips().first().copied();
            StepEditorView::resolve_clip(self.doc(), &device, selected, self.position)
        });
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.step_editor.open(device, clip);
        self.step_editor.sync(self.document.doc(), &self.registry);
    }

    /// Opens the step editor on the selected drum machine, else the first one of the rack.
    /// False when the rack has no drum machine.
    pub fn open_step_editor_default(&mut self) -> bool {
        let doc = self.doc();
        let is_drum = |d: &RackDevice| d.type_id == DRUM_MACHINE;
        let key = self
            .view
            .selected()
            .and_then(|k| doc.device(k))
            .filter(|d| is_drum(d))
            .or_else(|| doc.devices.iter().find(|d| is_drum(d)))
            .map(|d| d.key.clone());
        match key {
            Some(key) => {
                self.open_step_editor(key, None);
                true
            }
            None => false,
        }
    }

    /// Closes the step editor (the arrangement shows again).
    pub fn close_step_editor(&mut self) {
        self.step_editor.close();
    }

    // ------------------------------------------------------------------ creating a pattern

    /// Adds a one-bar `16 x 1/16` note clip on the drum machine's track at the playhead
    /// snapped to the bar (the track is created with it if the device has none, in one undo
    /// step) and opens the step editor on it.
    pub fn create_step_clip(&mut self, device: DeviceKey) {
        if self.refuse_inner(&device, "a step clip") {
            return;
        }
        if !self.is_drum_machine(&device) {
            return self.reject(format!("{device} is not a drum machine"));
        }
        let song = &self.doc().song;
        let start = song.nearest_bar_line(self.position).max(0);
        let has_track = song.track(&device).is_some();
        let step = NEW_GRID.resolution.ticks();
        let clip = Clip {
            id: ClipId(0),
            start,
            length: step * i64::from(NEW_GRID.steps),
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: Some(NEW_GRID),
            content: ClipContent::Notes(Vec::new()),
        };
        let add_clip = Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip,
        });
        let command = if has_track {
            add_clip
        } else {
            Command::Batch(vec![
                Command::Song(SongCommand::AddTrack {
                    device: device.clone(),
                }),
                add_clip,
            ])
        };
        let added = self.apply_command(command);
        self.document.end_merge();
        if added.is_err() {
            return;
        }
        let id = self.document.last_clip_id();
        self.arrangement
            .set_selected_clips(&id.into_iter().collect::<Vec<_>>());
        self.open_step_editor(device, id);
    }
}
