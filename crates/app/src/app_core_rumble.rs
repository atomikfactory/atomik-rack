//! Rumble Maker and Reverse Reverb Print in the app core: two renders on the freeze worker whose
//! result is made into new audio, not frozen or bounced as it is.
//!
//! # Rumble Maker
//!
//! **Track > Rumble Maker** (`sequencer.rumble_maker`, with `tail` or `reverb`) works on the
//! selected kick track: the selected note or audio clips of the track, else all of its clips. A
//! drum machine counts its kick pad alone ([`atomik_model::rumble::kick_key`]: the first pad that
//! plays a synthesised kick, else the first pad), a note track every note, and an audio track the
//! hits found in the audio.
//!
//! The kick is rendered offline, through the track's insert effects and before its strip, with a
//! short tail, on the freeze worker and queue. The worker builds the rumble layer from the render
//! ([`atomik_dsp::rumble`]), one piece per hit, in one of two ways: **tail** repeats a trimmed
//! chunk of the kick's tail with crossfades, **reverb** runs the kick through a long, dark reverb
//! low-passed to the sub band (its decay is twice the gap between the hits). The layer is printed
//! into the project's bundle as one stereo file like any other render, so it is saved, collected
//! and undone like other audio, and [`atomik_model::WorkflowCommand::AddRumble`] puts it on a new
//! track after the kick's, with a Sidechain Tool keyed by the kick and a low-pass, in one undo
//! step. The layer starts in phase with the kick at every hit.
//!
//! # Reverse Reverb Print
//!
//! **Track > Reverse Reverb Print** (`sequencer.reverse_reverb_print`, with the length in
//! seconds) works on the selected clips (each track's clips together), else on the range selected
//! on a lane or between the locators of the selected tracks. The source is rendered dry (a clip's
//! own sound with its gain and fades, or the instrument's output, before the insert effects),
//! reversed into the large reverb, and the tail that rings after it is reversed again: a swell
//! that rises to its loudest where the source starts. The print holds the swell alone, on whole
//! ticks, on a new track right after the source's, ending exactly at the first frame of the
//! source ([`crate::freeze_worker::swell_place`]); one undo step. A source at the very start of
//! the song has no room before it and is refused.

use super::*;
use atomik_dsp::rumble::{RumbleMode, RumbleParams};
use atomik_model::rumble::{
    RumbleTrackOptions, kick_hits, kick_key, suggested_decay, suggested_release,
};

/// How long the kick's tail is kept after the last hit, in seconds.
const RUMBLE_TAIL_SECONDS: f64 = 2.0;

/// What a source is made of: the clips to render (all of the track's when `None`) and the span in
/// ticks they cover.
type Source = (Option<Vec<ClipId>>, (i64, i64));

impl<S: EngineSink> AppCore<S> {
    /// The track Rumble Maker works on: the first selected track or the track of the selected
    /// clips, else the top-level track of the device selected in the rack.
    fn rumble_source(&self) -> Option<DeviceKey> {
        let doc = self.doc();
        let fits = |k: &DeviceKey| !k.0.contains('/') && doc.song.track(k).is_some();
        if let Some(k) = self.arrangement.command_tracks().into_iter().find(fits) {
            return Some(k);
        }
        if self.view.scope_depth() == 0 {
            return self.view.selection().into_iter().find(fits);
        }
        None
    }

    /// The clips of `device` a rumble is made from: the selected note and audio clips, else every
    /// clip of its played note lanes (or its audio lane); `None` when there is none.
    fn rumble_clips(&self, device: &DeviceKey) -> Option<Source> {
        let doc = self.doc();
        let track = doc.song.track(device)?;
        let chosen: Vec<(ClipId, i64, i64)> = self
            .arrangement
            .selected_clips()
            .into_iter()
            .filter_map(|id| {
                let (t, _, clip) = doc.song.clip(id)?;
                (t.device == *device && !matches!(clip.content, ClipContent::Automation(_)))
                    .then_some((id, clip.start, clip.end()))
            })
            .collect();
        if !chosen.is_empty() {
            let start = chosen.iter().map(|c| c.1).min()?;
            let end = chosen.iter().map(|c| c.2).max()?;
            return Some((Some(chosen.iter().map(|c| c.0).collect()), (start, end)));
        }
        let mut lanes: Vec<&atomik_model::Lane> = track.played_note_lanes().collect();
        lanes.extend(track.lanes.iter().filter(|l| l.kind.is_audio()));
        let spans: Vec<(i64, i64)> = lanes
            .into_iter()
            .flat_map(|l| &l.clips)
            .filter(|c| !c.muted)
            .map(|c| (c.start, c.end()))
            .collect();
        let start = spans.iter().map(|s| s.0).min()?;
        let end = spans.iter().map(|s| s.1).max()?;
        Some((None, (start, end)))
    }

    /// Builds a rumble layer from the selected kick track in `mode` (see the module docs): the
    /// kick renders on the freeze worker and the result lands on a new track in one undo step.
    /// A track that is not a kick source says why in the status line.
    pub fn rumble_maker(&mut self, mode: RumbleMode) {
        let Some(device) = self.rumble_source() else {
            return self
                .reject("Select a kick track or a drum machine to make a rumble from".into());
        };
        let name = self.track_name(&device);
        let Some((clips, span)) = self.rumble_clips(&device) else {
            return self.reject(format!("{name} has no clips to make a rumble from"));
        };
        let doc = self.doc();
        let Some(track) = doc.song.track(&device) else {
            return;
        };
        if track.frozen.is_some() {
            return self.reject(format!("{name} is frozen: unfreeze it to make a rumble"));
        }
        let effects = match track_chain(doc, &self.registry, &device) {
            Ok(chain) => chain.effects.len(),
            Err(why) => return self.reject(why),
        };
        // The hits: the notes of the kick (the pad of a drum machine), or found in the audio.
        let only_key = if track.is_audio() {
            None
        } else {
            kick_key(doc, &device)
        };
        let ticks: Vec<i64> = if track.is_audio() {
            Vec::new()
        } else {
            kick_hits(&doc.song, track, only_key)
                .into_iter()
                .filter(|t| (span.0..span.1).contains(t))
                .collect()
        };
        let hits = if track.is_audio() {
            None
        } else if ticks.is_empty() {
            return self.reject(format!("{name} plays no kick hit in its clips"));
        } else {
            let clock = doc.song.frame_clock(doc.sample_rate);
            let base = clock.at(span.0);
            Some(ticks.iter().map(|&t| clock.at(t) - base).collect())
        };
        let params = RumbleParams {
            mode,
            decay: suggested_decay(&doc.song, &ticks),
            ..RumbleParams::default()
        };
        let options = RumbleTrackOptions {
            release_ms: suggested_release(&doc.song, &ticks),
            ..RumbleTrackOptions::default()
        };
        let id = self.next_freeze_job();
        self.audio.freeze.queue.push_back(Job {
            id,
            kind: Kind::Rumble {
                params,
                options,
                hits,
                only_key,
            },
            request: FreezeRequest {
                device,
                effects,
                strip: false,
                clips,
                span,
                tail_seconds: RUMBLE_TAIL_SECONDS,
            },
            name,
        });
        self.start_next_job();
    }

    /// Prints the reverse reverb of the selected clips, else of the range of the selected lane or
    /// the locators, with a swell of `seconds` (see the module docs): one render per track, each
    /// one new track in one undo step.
    pub fn reverse_reverb_print(&mut self, seconds: f32) {
        if !(0.2..=20.0).contains(&seconds) {
            return self.reject("The swell is between 0.2 and 20 seconds long".into());
        }
        let (sources, mut skipped) = self.reverse_sources();
        if sources.is_empty() {
            let why = skipped.join(" \u{b7} ");
            return self.reject(if why.is_empty() {
                "Select an audio clip, or a range of a track, to print a reverse reverb of".into()
            } else {
                why
            });
        }
        let mut jobs = Vec::new();
        for (device, (clips, span)) in sources {
            let name = self.track_name(&device);
            if span.0 <= 0 {
                skipped.push(format!(
                    "{name} starts at the beginning of the song: there is no room before it for \
                     the swell"
                ));
                continue;
            }
            let id = self.next_freeze_job();
            jobs.push(Job {
                id,
                kind: Kind::ReverseReverb {
                    seconds,
                    anchor: span.0,
                },
                request: FreezeRequest {
                    device,
                    effects: 0,
                    strip: false,
                    clips,
                    span,
                    tail_seconds: 0.0,
                },
                name,
            });
        }
        if jobs.is_empty() {
            return self.reject(skipped.join(" \u{b7} "));
        }
        self.audio.freeze.queue.extend(jobs);
        self.start_next_job();
        if !skipped.is_empty() {
            self.reject(skipped.join(" \u{b7} "));
        }
    }

    /// The tracks a reverse reverb is printed of with their clips and span, and why the others
    /// are left out: the selected clips of each track together, else the ranges of
    /// [`Self::range_candidates`].
    fn reverse_sources(&self) -> (Vec<(DeviceKey, Source)>, Vec<String>) {
        let doc = self.doc();
        let selected = self.arrangement.selected_clips();
        let mut sources: Vec<(DeviceKey, Source)> = Vec::new();
        let mut skipped = Vec::new();
        if selected.is_empty() {
            let (spans, why) = self.range_candidates();
            sources.extend(spans.into_iter().map(|(d, span)| (d, (None, span))));
            skipped.extend(
                why.into_iter()
                    .map(|w| w.replace("bounced", "printed").replace("bounce", "print")),
            );
        }
        for id in selected {
            let Some((track, _, clip)) = doc.song.clip(id) else {
                continue;
            };
            let name = self.track_name(&track.device);
            if track.frozen.is_some() {
                skipped.push(format!("{name} is frozen: unfreeze it to print from it"));
                continue;
            }
            if matches!(clip.content, ClipContent::Automation(_)) {
                skipped.push("Only note and audio clips can be printed".to_owned());
                continue;
            }
            if let Err(why) = track_chain(doc, &self.registry, &track.device) {
                skipped.push(why);
                continue;
            }
            match sources.iter_mut().find(|(d, _)| *d == track.device) {
                Some((_, (ids, (start, end)))) => {
                    ids.get_or_insert_with(Vec::new).push(id);
                    *start = (*start).min(clip.start);
                    *end = (*end).max(clip.end());
                }
                None => sources.push((
                    track.device.clone(),
                    (Some(vec![id]), (clip.start, clip.end())),
                )),
            }
        }
        skipped.sort();
        skipped.dedup();
        (sources, skipped)
    }
}
