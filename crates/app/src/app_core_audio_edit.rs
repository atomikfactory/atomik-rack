//! Audio editing in the app core: the commands that need the samples.
//!
//! * **Normalise** sets the clip gain of each selected audio clip so that the peak of the audio it
//!   plays (the span of the asset it uses, before the clip's own gain, fades and envelope) lands
//!   on [`NORMALISE_TARGET_DB`]. Nothing is written to the asset: it is the clip's gain, so it
//!   can be moved by hand or reset. Silent clips are left alone.
//! * **Strip silence** runs the detector of `atomik_model` over the span each selected audio clip
//!   plays and turns the clip into the pieces it keeps (`SongCommand::StripSilence`). The
//!   pieces of every clip are selected afterwards.
//!
//! Each is one undo step however many clips are selected (a `Command::Batch`). The samples come
//! from the engine assets the app already holds: shared memory for small assets, and for assets
//! that stream from disk a read through the stream source, in chunks.

use super::*;
use atomik_engine::SongAsset;
use atomik_model::{
    AudioClipContent, ClipContent, StripParams, normalise_gain_db, peak, strip_silence,
};
use atomik_ui::SilenceValues;

/// The level Normalise aims for, in dBFS.
pub const NORMALISE_TARGET_DB: f32 = 0.0;
/// Frames read from a streamed asset at a time.
const READ_CHUNK: u64 = 1 << 16;

/// Calls `f` with the planar samples of the frames `[from, to)` of `asset` (clamped to its
/// end), in chunks.
fn for_each_chunk(asset: &SongAsset, from: u64, to: u64, mut f: impl FnMut(&[&[f32]])) {
    let to = to.min(asset.frames());
    let mut at = from;
    while at < to {
        let n = (to - at).min(READ_CHUNK);
        match asset {
            SongAsset::Memory { data, .. } => {
                let planes: Vec<&[f32]> = data
                    .iter()
                    .map(|c| &c[at as usize..(at + n) as usize])
                    .collect();
                f(&planes);
            }
            SongAsset::Stream {
                channels, source, ..
            } => {
                let mut buffers = vec![vec![0.0f32; n as usize]; usize::from(*channels)];
                let read = source.read(at, &mut buffers).min(n as usize);
                let planes: Vec<&[f32]> = buffers.iter().map(|c| &c[..read]).collect();
                f(&planes);
                if read == 0 {
                    break;
                }
            }
        }
        at += n;
    }
}

/// One selected audio clip with what the operations need.
struct Selected {
    id: ClipId,
    content: AudioClipContent,
    /// Frames the clip lasts at the project rate.
    frames: u64,
}

impl<S: EngineSink> AppCore<S> {
    /// The selected audio clips, in song order.
    fn selected_audio(&self) -> Vec<Selected> {
        let song = &self.doc().song;
        let rate = self.doc().sample_rate;
        let mut out: Vec<(i64, Selected)> = self
            .arrangement
            .selected_clips()
            .into_iter()
            .filter_map(|id| {
                let (_, _, clip) = song.clip(id)?;
                let ClipContent::Audio(a) = &clip.content else {
                    return None;
                };
                let frames = song
                    .frame_at(clip.end(), rate)
                    .saturating_sub(song.frame_at(clip.start, rate));
                Some((
                    clip.start,
                    Selected {
                        id,
                        content: a.clone(),
                        frames,
                    },
                ))
            })
            .collect();
        out.sort_by_key(|(start, s)| (*start, s.id));
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// What the strip silence panel says under its title, and whether APPLY can work.
    pub fn strip_silence_target(&self) -> (String, bool) {
        match self.selected_audio().len() {
            0 => (String::new(), false),
            1 => ("1 audio clip".to_owned(), true),
            n => (format!("{n} audio clips"), true),
        }
    }

    /// The command for `commands`: itself for one, a batch (one undo step) for several.
    fn one_step(mut commands: Vec<Command>) -> Option<Command> {
        match commands.len() {
            0 => None,
            1 => Some(commands.remove(0)),
            _ => Some(Command::Batch(commands)),
        }
    }

    /// Normalise: the gain of every selected audio clip that brings its peak to
    /// [`NORMALISE_TARGET_DB`], as one undo step. A rejection says why nothing changed.
    pub fn normalise_audio(&mut self) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to normalise".to_owned());
        }
        let song = &self.doc().song;
        let mut commands = Vec::new();
        let (mut silent, mut missing, mut unchanged) = (0, 0, 0);
        for s in &clips {
            let Some(asset) = self.audio.assets.get(&s.content.asset) else {
                missing += 1;
                continue;
            };
            let from = s.content.source_offset;
            let mut top = 0.0f32;
            for_each_chunk(asset, from, from + s.frames, |planes| {
                top = top.max(peak(planes, 0, u64::MAX));
            });
            let Some(db) = normalise_gain_db(top, NORMALISE_TARGET_DB) else {
                silent += 1;
                continue;
            };
            if (db - s.content.gain_db).abs() < 1e-4 {
                unchanged += 1;
                continue;
            }
            let Some((_, _, clip)) = song.clip(s.id) else {
                continue;
            };
            commands.push(Command::Song(SongCommand::SetAudioClip {
                id: s.id,
                start: clip.start,
                length: clip.length,
                muted: clip.muted,
                content: AudioClipContent {
                    gain_db: db,
                    ..s.content.clone()
                },
                merge: None,
            }));
        }
        match Self::one_step(commands) {
            Some(command) => {
                let _ = self.apply_command(command);
            }
            None if unchanged > 0 && silent == 0 && missing == 0 => {
                self.reject("The selected audio is already normalised".to_owned());
            }
            None if missing > 0 => self.reject("The audio file is missing".to_owned()),
            None => self.reject("The selected audio is silent: nothing to normalise".to_owned()),
        }
    }

    /// Strip silence from every selected audio clip with the panel's settings, as one undo step;
    /// the pieces are selected afterwards.
    pub fn strip_silence(&mut self, values: &SilenceValues) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to strip".to_owned());
        }
        let rate = self.doc().sample_rate;
        let ms = |v: u32| u64::from(v) * u64::from(rate) / 1000;
        let (mut silent, mut clean, mut missing) = (0, 0, 0);
        let mut commands = Vec::new();
        let mut selection: Vec<ClipId> = Vec::new();
        let mut next = self.doc().song.next_clip_id;
        for s in &clips {
            let Some(asset) = self.audio.assets.get(&s.content.asset) else {
                missing += 1;
                continue;
            };
            let from = s.content.source_offset;
            let n = s.frames;
            let mut planes: Vec<Vec<f32>> = vec![Vec::new(); usize::from(asset.channels())];
            for_each_chunk(asset, from, from + n, |chunk| {
                for (dst, src) in planes.iter_mut().zip(chunk) {
                    dst.extend_from_slice(src);
                }
            });
            // The detector walks the source forwards; a reversed clip plays it backwards, so
            // what comes before a piece in time is above it in the source.
            let (before, after) = if s.content.reverse {
                (values.pad_after_ms, values.pad_before_ms)
            } else {
                (values.pad_before_ms, values.pad_after_ms)
            };
            let params = StripParams {
                threshold_db: values.threshold_db as f32,
                min_silence: ms(values.min_silence_ms).max(1),
                pad_before: ms(before),
                pad_after: ms(after),
            };
            let kept = strip_silence(&planes, 0, n, &params);
            if kept.is_empty() {
                silent += 1;
                continue;
            }
            if kept == [(0, n)] {
                clean += 1;
                continue;
            }
            let pieces: Vec<(u64, u64)> = if s.content.reverse {
                kept.iter().rev().map(|&(a, b)| (n - b, n - a)).collect()
            } else {
                kept
            };
            selection.push(s.id);
            selection.extend((0..pieces.len() as u64 - 1).map(|k| ClipId(next + k)));
            next += pieces.len() as u64 - 1;
            commands.push(Command::Song(SongCommand::StripSilence {
                id: s.id,
                pieces,
                fade: ms(values.fade_ms),
            }));
        }
        let Some(command) = Self::one_step(commands) else {
            let message = if missing > 0 {
                "The audio file is missing"
            } else if clean > 0 {
                "No silence to strip with these settings"
            } else if silent > 0 {
                "The selected audio is silent: nothing to keep"
            } else {
                "Nothing to strip"
            };
            return self.reject(message.to_owned());
        };
        if self.apply_command(command).is_ok() {
            let song = &self.doc().song;
            selection.retain(|id| song.clip(*id).is_some());
            self.arrangement.set_selected_clips(&selection);
        }
    }
}
