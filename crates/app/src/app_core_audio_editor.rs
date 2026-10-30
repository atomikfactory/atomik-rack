//! The audio clip editor in the app core.
//!
//! [`Pane::AudioEditor`](crate::Pane::AudioEditor) shows [`AudioEditorView`] in place of the
//! arrangement, exclusive with the piano roll, the sample, step and slice editors and the other
//! editor panes (the shell owns the console). It opens on `UiAction::OpenAudioEditor` (a
//! double-click on an audio clip, Enter on a selected one, F8) and keeps its mode when it opens
//! another clip; it closes on `CloseAudioEditor`, when the clip goes away
//! ([`AudioEditorView::is_present`]) and on a new or opened document. Opening a clip selects it
//! in the arrangement. Peaks, samples and missing assets are fed as for the slice editor.
//!
//! # Clip to loop slicer
//!
//! `UiAction::ClipToSlicer` adds a loop slicer at the end of the rack and gives it the slices of
//! the clip: one slice per span between the clip's warp markers, cut from the clip's span of
//! its file (the slices keep their positions in the file), with `loop.beats` set from the clip
//! length at the song tempo. The device and its slices are **one undo step**. A clip without
//! markers, a reversed clip and a clip whose file is missing are refused with a message.

use super::AppCore;
use super::slicer::LOOP_SLICER;
use crate::sink::EngineSink;
use atomik_model::slice_edit;
use atomik_model::{
    ClipContent, ClipId, Command, DeviceKey, default_loop_beats, slices_from_markers,
};
use atomik_ui::AudioEditorView;
use std::sync::Arc;

/// The loop slicer's length parameter.
const BEATS_PARAM: &str = "loop.beats";

impl<S: EngineSink> AppCore<S> {
    /// The audio clip editor.
    pub fn audio_editor(&self) -> &AudioEditorView {
        &self.audio_editor
    }

    /// The audio clip editor, mutably (bounds, scale, focus, mode).
    pub fn audio_editor_mut(&mut self) -> &mut AudioEditorView {
        &mut self.audio_editor
    }

    /// The audio clip open in the editor (shown in place of the arrangement).
    pub fn audio_editor_clip(&self) -> Option<ClipId> {
        self.audio_editor.clip()
    }

    /// Opens an audio clip in the audio clip editor (ignored for other clips); it replaces the
    /// other editors and selects the clip in the arrangement.
    pub fn open_audio_editor(&mut self, id: ClipId) {
        let is_audio = self
            .doc()
            .song
            .clip(id)
            .is_some_and(|(_, _, c)| c.content.is_audio());
        if !is_audio {
            return;
        }
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.audio_editor.open(id);
        for (hash, peaks) in &self.audio.peaks {
            self.audio_editor.set_peaks(hash, Arc::clone(peaks));
        }
        self.audio_editor
            .set_missing_assets(self.audio.missing.iter().map(|m| m.hash.clone()));
        self.arrangement.set_selected_clips(&[id]);
        self.audio_editor.sync(self.document.doc(), &self.registry);
    }

    /// Opens the audio clip editor on the first selected audio clip of the arrangement. False
    /// when none is selected.
    pub fn open_audio_editor_selected(&mut self) -> bool {
        let song = &self.doc().song;
        let id = self
            .arrangement
            .selected_clips()
            .into_iter()
            .find(|id| song.clip(*id).is_some_and(|(_, _, c)| c.content.is_audio()));
        match id {
            Some(id) => {
                self.open_audio_editor(id);
                true
            }
            None => false,
        }
    }

    /// Closes the audio clip editor (the arrangement shows again).
    pub fn close_audio_editor(&mut self) {
        self.audio_editor.close();
    }

    /// `UiAction::ClipToSlicer`: see the module docs.
    pub fn clip_to_slicer(&mut self, id: ClipId) {
        let doc = self.doc();
        let rate = doc.sample_rate;
        let Some((_, _, clip)) = doc.song.clip(id) else {
            return;
        };
        let ClipContent::Audio(a) = &clip.content else {
            return;
        };
        if a.reverse {
            return self.reject("A reversed clip cannot be sliced".to_owned());
        }
        let Some(meta) = doc.assets.get(&a.asset) else {
            return self.reject("The audio file of the clip is missing".to_owned());
        };
        if !a.warp.enabled || a.warp.markers.is_empty() {
            return self.reject("The clip has no markers to slice at".to_owned());
        }
        let offset = a.source_offset;
        let span = (a
            .warp
            .source_frames_at(rate, clip.length as f64)
            .round()
            .max(0.0) as u64)
            .min(meta.frames.saturating_sub(offset));
        if span == 0 {
            return self.reject("The clip plays no audio to slice".to_owned());
        }
        let markers: Vec<u64> = slice_edit::framed(&a.warp, rate, clip.length)
            .iter()
            .map(|m| m.source)
            .collect();
        let min_len = u64::from(rate / 1000).max(1);
        let mut zones = slices_from_markers(&a.asset, span, &markers, &[], min_len);
        // The slices keep their place in the file.
        for z in &mut zones {
            z.start += offset;
            z.end += offset;
            z.loop_start += offset;
            z.loop_end += offset;
        }
        let beats = default_loop_beats(span, rate, doc.tempo_bpm);
        let before: Vec<DeviceKey> = doc
            .devices
            .iter()
            .filter(|d| d.type_id == LOOP_SLICER)
            .map(|d| d.key.clone())
            .collect();
        if self
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: LOOP_SLICER.to_owned(),
                key: None,
            })
            .is_err()
        {
            return;
        }
        let Some(key) = self
            .doc()
            .devices
            .iter()
            .find(|d| d.type_id == LOOP_SLICER && !before.contains(&d.key))
            .map(|d| d.key.clone())
        else {
            return;
        };
        self.join_next = true;
        let _ = self.apply_command(Command::Batch(vec![
            Command::SetParam {
                key: key.clone(),
                param: BEATS_PARAM.to_owned(),
                value: beats as f32,
                merge: None,
            },
            Command::SetZones {
                device: key,
                zones,
                merge: None,
            },
        ]));
        self.join_next = false;
    }
}
