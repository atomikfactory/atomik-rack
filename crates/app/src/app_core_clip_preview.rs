//! Library clip previews: Space on a clip row of the browser plays the note clip without
//! loading it (`BrowseAction::Preview` on a `.atomikclip`; Space again, Escape, another preview
//! or the transport stops it).
//!
//! The clip file is read once and played once, at the song tempo in effect at the playhead, on a
//! clock of its own while the transport stands still:
//!
//! * With a **note track selected** the notes go through the live note path to its instrument
//!   (and the players above it), as the Piano Keys do. The clock is the frame clock of the
//!   shell ([`AppCore::transport_frame`]): a note sounds on the first frame at or after its
//!   time. Nothing is allocated on the audio thread; a note off follows each note on, and a
//!   stop releases what sounds.
//! * With **no note track selected** (or a selected track that is not a note track) a small
//!   built-in voice renders the notes (a sine with an octave partial and a short release) on a
//!   worker thread into the **preview voice** of the file browser
//!   (`atomik_engine::preview_channel`), so the press/stop rules are those of an audio file
//!   preview: it ends by itself, a stop or another preview replaces it.
//!
//! The song transport starting ends a live preview (it is refused while the song plays). The
//! document, the selection and the undo history are never touched, and an offline render never
//! hears a preview: bounces and exports read neither the live note path of the app nor the
//! preview voice.

use super::*;
use atomik_core::PPQ;
use atomik_engine::{PreviewFeed, preview_channel};
use atomik_model::{Clip, ClipContent, load_clip_file};
use std::time::Duration;

/// The rate the built-in voice renders at (the preview voice resamples it to the interface).
const RENDER_RATE: u32 = 44_100;
/// The longest clip the preview plays, in seconds.
const MAX_SECONDS: f64 = 120.0;
/// How long the built-in voice rings after a note ends, in seconds.
const RELEASE: f64 = 0.25;
/// Frames pushed into the feed at a time.
const CHUNK: usize = 1 << 13;
/// How long the worker waits for room in a full ring.
const WAIT: Duration = Duration::from_millis(2);

/// A note of a clip in seconds from the start of the preview.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TimedNote {
    pub start: f64,
    pub end: f64,
    pub key: u8,
    pub velocity: f32,
}

/// The unmuted notes of a note clip that start inside its length, in seconds at a constant
/// `bpm`, sorted by start. A note that runs past the end of the clip is cut there.
pub(super) fn timed_notes(clip: &Clip, bpm: f64) -> Vec<TimedNote> {
    let ClipContent::Notes(notes) = &clip.content else {
        return Vec::new();
    };
    let seconds = |ticks: i64| ticks as f64 * 60.0 / (bpm.max(1.0) * PPQ as f64);
    let mut out: Vec<TimedNote> = notes
        .iter()
        .filter(|n| !n.muted && n.tick >= 0 && n.tick < clip.length && n.length > 0)
        .map(|n| TimedNote {
            start: seconds(n.tick),
            end: seconds((n.tick + n.length).min(clip.length)),
            key: n.key,
            velocity: n.velocity,
        })
        .filter(|n| n.start < MAX_SECONDS)
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    out
}

/// The seconds a preview of `notes` lasts: until the last note ends.
pub(super) fn span_of(notes: &[TimedNote]) -> f64 {
    notes
        .iter()
        .map(|n| n.end)
        .fold(0.0, f64::max)
        .min(MAX_SECONDS)
}

/// Renders `notes` with the built-in voice into one mono buffer at [`RENDER_RATE`].
pub(super) fn render_notes(notes: &[TimedNote]) -> Vec<f32> {
    let rate = f64::from(RENDER_RATE);
    let frames = ((span_of(notes) + RELEASE) * rate).ceil() as usize;
    let mut out = vec![0.0f32; frames];
    for n in notes {
        let freq = 440.0 * 2f64.powf((f64::from(n.key) - 69.0) / 12.0);
        let first = (n.start * rate) as usize;
        let held = (n.end - n.start).max(0.0);
        let total = ((held + RELEASE) * rate) as usize;
        let level = f64::from(n.velocity.clamp(0.0, 1.0)) * 0.2;
        for i in 0..total {
            let Some(slot) = out.get_mut(first + i) else {
                break;
            };
            let t = i as f64 / rate;
            let attack = (t / 0.004).min(1.0);
            let release = if t > held {
                (1.0 - (t - held) / RELEASE).max(0.0)
            } else {
                1.0
            };
            let phase = std::f64::consts::TAU * freq * t;
            let tone = phase.sin() + 0.3 * (2.0 * phase).sin();
            *slot += (tone * level * attack * release * (-1.5 * t).exp().max(0.35)) as f32;
        }
    }
    // Chords never clip: the mix is scaled down to a peak of 0.8 when it would exceed it.
    let peak = out.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    if peak > 0.8 {
        let scale = 0.8 / peak;
        for s in &mut out {
            *s *= scale;
        }
    }
    out
}

/// The worker of the built-in voice: renders the notes and feeds them until all are in or the
/// stream is let go of.
fn feed_notes(mut feed: PreviewFeed, notes: &[TimedNote]) {
    let mono = render_notes(notes);
    let mut at = 0;
    while at < mono.len() {
        let end = (at + CHUNK).min(mono.len());
        let planes = [mono[at..end].to_vec()];
        let mut done = 0;
        while done < planes[0].len() {
            if feed.is_abandoned() {
                return;
            }
            let n = feed.push(&planes, done);
            if n == 0 {
                std::thread::sleep(WAIT);
            }
            done += n;
        }
        at = end;
    }
    feed.finish();
}

/// A note event of the live preview.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Event {
    at: f64,
    kind: NoteKind,
}

/// The clip preview that plays through a track's instrument.
#[derive(Debug, Clone)]
pub(super) struct LiveClipPreview {
    /// The clip file (what the browser row shows as playing).
    path: PathBuf,
    /// The instrument track the notes go to.
    device: DeviceKey,
    /// Note ons and offs in time order.
    events: Vec<Event>,
    /// The next event to send.
    next: usize,
    /// The clock second the preview began at.
    started: f64,
    /// Seconds until the last note ends.
    span: f64,
    /// Keys that sound (to release on a stop).
    down: Vec<u8>,
}

/// The note on and off events of `notes` in time order (an off before an on at one instant).
fn events_of(notes: &[TimedNote]) -> Vec<Event> {
    let mut events: Vec<Event> = notes
        .iter()
        .flat_map(|n| {
            [
                Event {
                    at: n.start,
                    kind: NoteKind::On {
                        key: n.key,
                        velocity: n.velocity.clamp(0.01, 1.0),
                    },
                },
                Event {
                    at: n.end,
                    kind: NoteKind::Off { key: n.key },
                },
            ]
        })
        .collect();
    let rank = |k: &NoteKind| u8::from(matches!(k, NoteKind::On { .. }));
    events.sort_by(|a, b| {
        a.at.total_cmp(&b.at)
            .then(rank(&a.kind).cmp(&rank(&b.kind)))
    });
    events
}

impl<S: EngineSink> AppCore<S> {
    /// The clip preview that plays through a track, if any (the file it plays).
    pub fn previewing_clip(&self) -> Option<&Path> {
        self.clip_preview.as_ref().map(|p| p.path.as_path())
    }

    /// `BrowseAction::Preview` of a clip row: see the module docs. A refusal goes to the status
    /// line.
    pub(super) fn preview_library_clip(&mut self, path: &Path) {
        if let Err(message) = self.start_clip_preview(path) {
            self.reject(message);
        }
    }

    fn start_clip_preview(&mut self, path: &Path) -> Result<(), String> {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let text = atomik_model::read_document_text(path)
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        let file = load_clip_file(&text).map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        // Every other preview gives way: a browser file, a speaker audition, an earlier clip.
        self.stop_preview();
        if self.playing {
            return Err("Stop the song to preview a clip: it plays with the transport".into());
        }
        if !self.sink.has_engine() {
            return Err("Previewing needs audio: it is off or its stream did not open".into());
        }
        let bpm = self
            .doc()
            .song
            .tempo
            .get(self.tempo_event_at_playhead())
            .map_or(120.0, |e| e.bpm);
        let notes = timed_notes(&file.clip, bpm);
        if notes.is_empty() {
            return Err(format!("'{name}' has no notes to preview"));
        }
        let span = span_of(&notes);
        match self.preview_track() {
            Some(device) => {
                self.clip_preview = Some(LiveClipPreview {
                    path: path.to_path_buf(),
                    device,
                    events: events_of(&notes),
                    next: 0,
                    started: self.now_seconds,
                    span,
                    down: Vec::new(),
                });
                self.feed_clip_preview(self.now_seconds);
                Ok(())
            }
            None => self.start_built_in_preview(path, notes, span),
        }
    }

    /// The instrument track the selected note lane belongs to.
    fn preview_track(&self) -> Option<DeviceKey> {
        let song = &self.doc().song;
        self.arrangement
            .selected_lane()
            .filter(|(t, l)| song.track(t).is_some_and(|t| t.is_note_lane(*l)))
            .map(|(t, _)| t.clone())
            .filter(|t| !self.audition_targets(t).is_empty())
    }

    /// Plays `notes` with the built-in voice through the preview voice.
    fn start_built_in_preview(
        &mut self,
        path: &Path,
        notes: Vec<TimedNote>,
        span: f64,
    ) -> Result<(), String> {
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(RENDER_RATE);
        let monitor = stream.monitor();
        self.sink
            .preview_start(stream, self.preview_gain())
            .map_err(|e| format!("preview: {e}"))?;
        let spawned = std::thread::Builder::new()
            .name("atomik-clip-preview".into())
            .spawn(move || feed_notes(feed, &notes));
        if let Err(e) = spawned {
            let _ = self.sink.preview_stop();
            return Err(format!("preview: cannot start the renderer: {e}"));
        }
        let frames = ((span + RELEASE) * f64::from(RENDER_RATE)).ceil() as u64;
        self.adopt_preview(path.to_path_buf(), monitor, frames);
        Ok(())
    }

    /// Ends the live clip preview and releases the notes it holds (nothing without one).
    pub(super) fn stop_clip_preview(&mut self) {
        let Some(preview) = self.clip_preview.take() else {
            return;
        };
        for key in preview.down {
            self.live_note_to(&preview.device, NoteKind::Off { key });
        }
        self.view_preview(None, 0.0);
    }

    /// Forgets the live preview without telling the engine: a new engine holds no notes.
    pub(super) fn forget_clip_preview(&mut self) {
        if self.clip_preview.take().is_some() {
            self.view_preview(None, 0.0);
        }
    }

    /// One frame of the live preview at `now` seconds: sends the events that are due, shows the
    /// position and ends it after the last note. The transport playing ends it too.
    pub(super) fn feed_clip_preview(&mut self, now: f64) {
        let Some(mut preview) = self.clip_preview.take() else {
            return;
        };
        if self.playing {
            self.clip_preview = Some(preview);
            self.stop_clip_preview();
            return;
        }
        if self.doc().song.track(&preview.device).is_none() {
            // The track is gone and cannot release its notes: everything is.
            self.view_preview(None, 0.0);
            self.all_notes_off();
            return;
        }
        let elapsed = (now - preview.started).max(0.0);
        while let Some(event) = preview.events.get(preview.next).copied() {
            if event.at > elapsed {
                break;
            }
            match event.kind {
                NoteKind::On { key, .. } => preview.down.push(key),
                NoteKind::Off { key } => {
                    if let Some(i) = preview.down.iter().position(|k| *k == key) {
                        preview.down.remove(i);
                    }
                }
                _ => {}
            }
            self.live_note_to(&preview.device, event.kind);
            preview.next += 1;
        }
        if preview.next >= preview.events.len() {
            self.view_preview(None, 0.0);
            return;
        }
        #[allow(clippy::cast_possible_truncation)] // a progress fraction needs no exactness
        let progress = if preview.span > 0.0 {
            (elapsed / preview.span) as f32
        } else {
            0.0
        };
        self.view_preview(Some(preview.path.clone()), progress);
        self.clip_preview = Some(preview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{RecordingSink, SinkCall};
    use atomik_model::{ClipFile, ClipId, Command, Note, SongCommand, empty_doc, save_clip_file};
    use atomik_ui::BrowseAction;
    use std::sync::atomic::{AtomicU64, Ordering};

    const SYNTH: &str = "atomik.poly-synth";
    const BAR: i64 = 4 * PPQ;

    /// A scratch folder that is removed when it goes out of scope.
    struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(tag: &str) -> Tmp {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-clip-preview-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }

    fn clip_with(notes: Vec<Note>) -> Clip {
        Clip {
            id: ClipId(0),
            start: 0,
            length: BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(notes.into()),
        }
    }

    fn write_clip(dir: &Path, notes: Vec<Note>) -> PathBuf {
        let file = ClipFile {
            tempo_bpm: 120.0,
            meter: (4, 4),
            clip: clip_with(notes),
        };
        let path = dir.join("riff.atomikclip");
        std::fs::write(&path, save_clip_file(&file).unwrap()).unwrap();
        path
    }

    /// Two notes: key 60 for a beat at the start and key 64 for a beat on the second beat.
    fn riff() -> Vec<Note> {
        vec![Note::new(0, PPQ, 60, 0.8), Note::new(PPQ, PPQ, 64, 0.6)]
    }

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    /// A core with synth `a` on a track whose note lane is selected.
    fn with_track() -> AppCore<RecordingSink> {
        let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        })
        .unwrap();
        c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
            .unwrap();
        c.after_view_change();
        c.arrangement_mut().set_selected_track(Some(key("a")));
        c
    }

    fn live(c: &mut AppCore<RecordingSink>) -> Vec<NoteKind> {
        c.sink_mut()
            .take()
            .into_iter()
            .filter_map(|call| match call {
                SinkCall::LiveNote { kind, .. } => Some(kind),
                _ => None,
            })
            .collect()
    }

    fn previews(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
        c.sink()
            .calls
            .iter()
            .filter(|call| matches!(call, SinkCall::PreviewStart { .. } | SinkCall::PreviewStop))
            .cloned()
            .collect()
    }

    #[test]
    fn notes_are_timed_at_the_tempo_and_a_muted_or_outside_note_is_left_out() {
        let mut muted = Note::new(PPQ, PPQ, 62, 0.5);
        muted.muted = true;
        let clip = clip_with(vec![
            Note::new(0, PPQ, 60, 0.8),
            muted,
            Note::new(BAR - PPQ, 2 * PPQ, 67, 0.7),
            Note::new(BAR, PPQ, 72, 0.7),
        ]);
        let notes = timed_notes(&clip, 120.0);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].start, notes[0].end), (0.0, 0.5));
        // The last note starts at beat 4 and is cut at the clip end (one beat, not two).
        assert_eq!((notes[1].start, notes[1].end), (1.5, 2.0));
        assert_eq!(span_of(&notes), 2.0);
        // A slower tempo stretches the same notes.
        assert_eq!(timed_notes(&clip, 60.0)[0].end, 1.0);
    }

    #[test]
    fn the_built_in_voice_renders_sound_that_stays_inside_full_scale() {
        let loud: Vec<Note> = (0..12)
            .map(|i| Note::new(0, PPQ, 48 + i * 3, 1.0))
            .collect();
        let notes = timed_notes(&clip_with(loud), 120.0);
        let out = render_notes(&notes);
        let peak = out.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.05 && peak <= 0.8 + 1e-6, "{peak}");
        assert!(out.len() as f64 >= (0.5 + RELEASE) * f64::from(RENDER_RATE));
        // After the release, silence.
        assert!(out.last().is_some_and(|s| s.abs() < 1e-4));
    }

    #[test]
    fn a_selected_note_track_plays_the_notes_live_on_its_own_clock() {
        let dir = tmp("live");
        let path = write_clip(&dir, riff());
        let mut c = with_track();
        c.transport_frame(10.0);
        c.sink_mut().take();
        c.browse(BrowseAction::Preview(path.clone()));
        // The first note sounds at once and nothing else yet.
        assert_eq!(
            live(&mut c),
            [NoteKind::On {
                key: 60,
                velocity: 0.8
            }]
        );
        assert_eq!(c.previewing_clip(), Some(path.as_path()));
        assert!(previews(&c).is_empty(), "no preview voice for a track");
        // 120 BPM: a beat is half a second.
        c.transport_frame(10.2);
        assert!(live(&mut c).is_empty());
        c.transport_frame(10.5);
        assert_eq!(
            live(&mut c),
            [
                NoteKind::Off { key: 60 },
                NoteKind::On {
                    key: 64,
                    velocity: 0.6
                }
            ]
        );
        c.transport_frame(11.5);
        assert_eq!(live(&mut c), [NoteKind::Off { key: 64 }]);
        assert_eq!(c.previewing_clip(), None, "it ends after the last note");
        // Nothing changed in the document and no undo step was made.
        assert!(c.doc().song.tracks[0].lanes[0].clips.is_empty());
    }

    #[test]
    fn a_stop_releases_the_notes_that_sound_and_the_document_is_untouched() {
        let dir = tmp("stop");
        let path = write_clip(&dir, riff());
        let mut c = with_track();
        let revision = c.document().revision();
        let depth = c.document().undo_depth();
        c.transport_frame(0.0);
        c.browse(BrowseAction::Preview(path));
        c.sink_mut().take();
        c.browse(BrowseAction::StopPreview);
        assert_eq!(live(&mut c), [NoteKind::Off { key: 60 }]);
        assert_eq!(c.previewing_clip(), None);
        assert_eq!(c.document().revision(), revision);
        assert_eq!(c.document().undo_depth(), depth);
    }

    #[test]
    fn the_transport_starting_or_another_preview_ends_it() {
        let dir = tmp("ends");
        let path = write_clip(&dir, riff());
        let mut c = with_track();
        c.transport_frame(0.0);
        c.browse(BrowseAction::Preview(path.clone()));
        c.sink_mut().take();
        c.playing = true;
        c.transport_frame(0.1);
        assert_eq!(live(&mut c), [NoteKind::Off { key: 60 }]);
        assert_eq!(c.previewing_clip(), None);
        // While the song plays a preview is refused.
        c.browse(BrowseAction::Preview(path.clone()));
        assert_eq!(c.previewing_clip(), None);
        // A new preview starts again once the song stands still, and stop_preview ends it.
        c.playing = false;
        c.browse(BrowseAction::Preview(path));
        assert!(c.previewing_clip().is_some());
        c.stop_preview();
        assert_eq!(c.previewing_clip(), None);
    }

    #[test]
    fn deleting_the_track_stops_the_preview() {
        let dir = tmp("gone");
        let path = write_clip(&dir, riff());
        let mut c = with_track();
        c.transport_frame(0.0);
        c.browse(BrowseAction::Preview(path));
        c.apply_command(Command::Song(SongCommand::RemoveTrack { device: key("a") }))
            .unwrap();
        c.sink_mut().take();
        c.transport_frame(0.1);
        assert_eq!(c.previewing_clip(), None);
    }

    #[test]
    fn without_a_note_track_the_built_in_voice_plays_through_the_preview_voice() {
        let dir = tmp("built-in");
        let path = write_clip(&dir, riff());
        let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
        c.browse(BrowseAction::Preview(path.clone()));
        assert_eq!(previews(&c), [SinkCall::PreviewStart { gain: 1.0 }]);
        assert_eq!(c.previewing_clip(), None, "that one is the preview voice");
        assert_eq!(c.preview_path(), Some(path.as_path()));
        // Space again, or Escape, stops it like an audio file.
        c.browse(BrowseAction::StopPreview);
        assert_eq!(previews(&c).last(), Some(&SinkCall::PreviewStop));
        assert_eq!(c.preview_path(), None);
    }

    #[test]
    fn a_clip_preview_plays_at_the_preview_level() {
        let dir = tmp("level");
        let path = write_clip(&dir, riff());
        let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
        c.set_preview_db(-12.0);
        c.browse(BrowseAction::Preview(path));
        let [SinkCall::PreviewStart { gain }] = previews(&c)[..] else {
            panic!("{:?}", previews(&c))
        };
        assert!((gain - 0.251_189).abs() < 1e-4, "{gain}");
    }

    #[test]
    fn a_refusal_is_a_status_message_and_starts_nothing() {
        let dir = tmp("refuse");
        let mut c = with_track();
        // Not a clip file.
        let bad = dir.join("bad.atomikclip");
        std::fs::write(&bad, "nonsense").unwrap();
        c.browse(BrowseAction::Preview(bad));
        assert_eq!(c.previewing_clip(), None);
        assert!(c.preview_path().is_none());
        // A clip without notes.
        let empty = write_clip(&dir, Vec::new());
        c.browse(BrowseAction::Preview(empty));
        assert_eq!(c.previewing_clip(), None);
        assert!(previews(&c).is_empty());
    }

    #[test]
    fn without_audio_the_preview_is_refused() {
        let dir = tmp("no-audio");
        let path = write_clip(&dir, riff());
        let mut c = with_track();
        c.sink_mut().no_engine = true;
        c.browse(BrowseAction::Preview(path));
        assert_eq!(c.previewing_clip(), None);
    }
}
