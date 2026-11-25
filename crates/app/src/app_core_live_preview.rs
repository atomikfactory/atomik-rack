//! Library patch previews that follow the keyboard: while a patch, a container or an effect
//! item is the one selected row of the file browser, the notes of a MIDI keyboard and of the
//! Piano Keys strip play through that item instead of through the selected track, and go back
//! to the track when the selection leaves it. Space on the row still plays its built-in phrase
//! (`app_core_patch_preview.rs`); the two share the browser's preview voice, so a phrase that
//! is playing keeps the voice and the keyboard follows again when it ends.
//!
//! * **The scratch project** is the one the phrase preview builds (`preview_doc`): an
//!   instrument item plays through its own channel, an item of note players in front of a
//!   built-in synth, an effect item in the insert chain of a built-in synth. The song of the
//!   scratch project is not played: the keys go to the notes input of its first track, as the
//!   phrase does. An item the phrase preview refuses (a plugin, no sound) is not followed and
//!   says nothing: the keyboard stays on the track.
//! * **The worker** (`atomik-live-preview`) builds the devices off the UI thread after the
//!   selection has rested for a moment (so arrowing down a list builds nothing), then drives an
//!   engine of its own block by block ([`atomik_render::live_preview::LivePreview`]) and feeds
//!   the result into the preview voice, keeping about 30 ms of audio ahead of the voice. Without
//!   notes for a while (every frame silent for three seconds) it stops rendering and waits for
//!   the next key; the voice plays silence meanwhile. The audio thread only ever sees a stream
//!   of samples, as for an audio file preview: nothing is allocated, locked or built on it.
//! * **The keys** reach the worker through the notes target: while a preview follows, the app's
//!   notes target is [`crate::preview_notes::PREVIEW_TARGET`] and the live note producer queues events for it in
//!   [`crate::preview_notes::PreviewNotes`] instead of in the engine, so a note-off still goes
//!   where its note-on went when the selection leaves while a key is held. The preview is never
//!   recorded (no armed device has that target) and the document, the selection and the undo
//!   history are never touched.
//! * **When it follows**: the browser panel shows, audio runs, no take records, no other
//!   preview plays on the voice and the one selected row is a patch, container or plugin preset
//!   (`FileKind`). A stop of the voice by another preview ends it; an engine restart ends it
//!   and the next frame starts it again.

use super::*;
use crate::preview_notes::PreviewNotes;
use atomik_device_sdk::Registry;
use atomik_engine::{PREVIEW_RING_FRAMES, PreviewFeed, PreviewMonitor, preview_channel};
use atomik_library::FileKind;
use atomik_render::live_preview::{LiveDevices, LivePreview};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// How long the selection must rest before the worker builds anything: this many naps of
/// [`SETTLE_NAP`].
const SETTLE_NAPS: u32 = 15;
/// One nap of the settle wait.
const SETTLE_NAP: Duration = Duration::from_millis(10);
/// How long the worker waits for a note when the ring is full enough.
const NAP: Duration = Duration::from_millis(2);
/// How long the idle worker waits for a note.
const IDLE_NAP: Duration = Duration::from_millis(50);
/// Frames rendered at a time.
const BLOCK: usize = 128;
/// The ring is kept filled to a 32nd of a second.
const LEAD_DIVISOR: usize = 32;
/// Seconds of silence after which the worker rests until a note comes.
const IDLE_SECONDS: usize = 3;
/// The level below which a block counts as silent.
const QUIET: f32 = 1.0e-5;

/// What the worker reports to the core, readable from any thread.
#[derive(Debug, Default)]
pub(super) struct FollowStats {
    /// The devices are built and the worker plays.
    built: AtomicBool,
    /// Frames pushed into the preview ring.
    fed: AtomicU64,
    /// Of them, frames that were not silent.
    sounding: AtomicU64,
}

/// A running keyboard preview.
pub(super) struct LiveFollow {
    /// The row it follows.
    pub(super) path: PathBuf,
    /// The voice's view of the stream.
    monitor: PreviewMonitor,
    /// Set to end the worker.
    stop: Arc<AtomicBool>,
    /// What the worker reports (read by the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    stats: Arc<FollowStats>,
    /// The worker's failure, if it has one.
    failures: Receiver<String>,
}

/// What a frame does about the selection.
enum Step {
    /// Nothing changes.
    Keep,
    /// Nothing is to be followed.
    Stop,
    /// An item of the library came under the selection.
    Item(PathBuf, FileKind),
    /// A plugin preset came under the selection.
    Preset(PathBuf),
}

/// The state the core keeps.
#[derive(Default)]
pub(super) struct LiveFollowState {
    /// The preview that runs.
    pub(super) active: Option<LiveFollow>,
    /// The row last tried: a row that cannot be followed is not tried again every frame.
    pub(super) tried: Option<PathBuf>,
    /// The preview instance of the plugin whose presets are browsed (see
    /// `app_core_plugin_preview.rs`).
    pub(super) plugin: Option<super::plugin_follow_ops::PluginSession>,
}

/// What a keyboard preview is built from.
pub(super) struct Spawn {
    /// The scratch project.
    pub(super) doc: atomik_model::RackDoc,
    /// The folder of the audio the project's pool refers to.
    pub(super) root: Option<PathBuf>,
    /// The registry to build it with (`None`: the built-in devices).
    pub(super) registry: Option<Registry>,
    /// Devices made by the app for keys of the project (a plugin instance).
    pub(super) supplied: LiveDevices,
}

/// What the worker needs.
struct Job {
    spawn: Spawn,
    /// Whether the selection must rest first.
    settle: bool,
    feed: PreviewFeed,
    target: Arc<crate::target::NoteTarget>,
    stop: Arc<AtomicBool>,
    stats: Arc<FollowStats>,
    failures: Sender<String>,
}

/// The worker: waits for the selection to rest, builds the scratch project and plays it.
fn run(job: Job) {
    let Job {
        spawn,
        settle,
        mut feed,
        target,
        stop,
        stats,
        failures,
    } = job;
    let Spawn {
        doc,
        root,
        registry,
        supplied,
    } = spawn;
    for _ in 0..if settle { SETTLE_NAPS } else { 0 } {
        if stop.load(Ordering::Relaxed) || feed.is_abandoned() {
            return;
        }
        std::thread::sleep(SETTLE_NAP);
    }
    let registry = registry.unwrap_or_else(atomik_render::default_registry);
    let built = catch_unwind(AssertUnwindSafe(|| {
        LivePreview::build(&doc, &registry, root.as_deref(), supplied)
    }));
    let mut live = match built {
        Ok(Ok(live)) => live,
        Ok(Err(e)) => {
            let _ = failures.send(e.to_string());
            return;
        }
        Err(_) => {
            let _ = failures.send("a device failed while the preview was built".into());
            return;
        }
    };
    stats.built.store(true, Ordering::Release);
    let finished = catch_unwind(AssertUnwindSafe(|| {
        play(&mut live, &mut feed, target.preview(), &stop, &stats)
    }));
    match finished {
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            let _ = failures.send(message);
        }
        Err(_) => {
            let _ = failures.send("a device failed while the preview played".into());
        }
    }
}

/// Renders blocks into the feed while the voice has less than the lead ahead.
fn play(
    live: &mut LivePreview,
    feed: &mut PreviewFeed,
    notes: &PreviewNotes,
    stop: &AtomicBool,
    stats: &FollowStats,
) -> Result<(), String> {
    let rate = live.sample_rate() as usize;
    let lead = (rate / LEAD_DIVISOR).max(BLOCK);
    let idle_after = rate * IDLE_SECONDS;
    let mut quiet = 0usize;
    loop {
        if stop.load(Ordering::Relaxed) || feed.is_abandoned() {
            return Ok(());
        }
        while let Some(kind) = notes.pop() {
            live.note(kind);
            quiet = 0;
        }
        let queued = PREVIEW_RING_FRAMES.saturating_sub(feed.free_frames());
        if quiet >= idle_after || queued >= lead {
            let nap = if quiet >= idle_after { IDLE_NAP } else { NAP };
            if let Some(kind) = notes.wait(nap) {
                live.note(kind);
                quiet = 0;
            }
            continue;
        }
        live.render(BLOCK).map_err(|e| e.to_string())?;
        let planes = live.output();
        let loud = planes
            .iter()
            .flatten()
            .any(|s| !s.is_finite() || s.abs() > QUIET);
        let mut done = 0;
        while done < BLOCK {
            if stop.load(Ordering::Relaxed) || feed.is_abandoned() {
                return Ok(());
            }
            let n = feed.push(planes, done);
            if n == 0 {
                std::thread::sleep(NAP);
            }
            done += n;
        }
        stats.fed.fetch_add(BLOCK as u64, Ordering::Relaxed);
        if loud {
            stats.sounding.fetch_add(BLOCK as u64, Ordering::Relaxed);
            quiet = 0;
        } else {
            quiet += BLOCK;
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Whether the keyboard goes to a library preview: the notes target is then
    /// [`crate::preview_notes::PREVIEW_TARGET`].
    pub(super) fn live_follow_has_keys(&self) -> bool {
        self.live_follow.active.is_some()
    }

    /// The row to follow: the one selected row of the panel, when something may follow it.
    fn follow_candidate(&self) -> Option<(&Path, FileKind)> {
        if !self.file_browser_visible()
            || self.take.is_some()
            || self.audio.recording.is_some()
            || self.file_preview_playing()
            || self.clip_preview.is_some()
            || self.player_preview.is_some()
            || !self.sink.has_engine()
        {
            return None;
        }
        self.file_browser.selected_row().filter(|(_, kind)| {
            matches!(
                kind,
                FileKind::Patch | FileKind::Container | FileKind::Preset
            )
        })
    }

    /// One frame of the keyboard preview: follows the selection, and ends when the worker failed
    /// or the engine let go of the stream.
    pub(super) fn poll_live_follow(&mut self) {
        if let Some(follow) = &self.live_follow.active {
            if let Ok(message) = follow.failures.try_recv() {
                self.drop_live_follow();
                self.report(format!("The keyboard preview failed: {message}"));
                return;
            }
            if follow.monitor.is_abandoned() || !self.sink.has_engine() {
                self.live_follow.tried = None;
                self.drop_live_follow();
            }
        }
        let step = match self.follow_candidate() {
            None => Step::Stop,
            Some((path, FileKind::Preset)) => match &self.live_follow.plugin {
                Some(session) if session.row == path => Step::Keep,
                None if self.live_follow.tried.as_deref() == Some(path) => Step::Keep,
                _ => Step::Preset(path.to_path_buf()),
            },
            Some((path, kind)) => {
                let following = self.live_follow.plugin.is_none()
                    && self
                        .live_follow
                        .active
                        .as_ref()
                        .is_some_and(|f| f.path == path);
                let refused = self.live_follow.active.is_none()
                    && self.live_follow.tried.as_deref() == Some(path);
                if following || refused {
                    Step::Keep
                } else {
                    Step::Item(path.to_path_buf(), kind)
                }
            }
        };
        match step {
            Step::Keep => {}
            Step::Stop => {
                self.live_follow.tried = None;
                self.stop_live_follow();
            }
            Step::Item(path, kind) => {
                self.stop_live_follow();
                self.start_live_follow(&path, kind);
            }
            Step::Preset(path) => self.begin_plugin_follow(&path),
        }
        self.drive_plugin_follow();
    }

    /// Starts the keyboard preview of the row `path`; a row that cannot be followed starts
    /// nothing and says nothing.
    fn start_live_follow(&mut self, path: &Path, kind: FileKind) {
        self.live_follow.tried = Some(path.to_path_buf());
        let Some((doc, root)) = self.follow_doc(path, kind) else {
            return;
        };
        let spawn = Spawn {
            doc,
            root,
            registry: None,
            supplied: LiveDevices::new(),
        };
        self.spawn_follow(path, spawn);
    }

    /// Starts the voice and the worker that plays `spawn` for the row `path`; `false` when
    /// either cannot start.
    pub(super) fn spawn_follow(&mut self, path: &Path, spawn: Spawn) -> bool {
        // A patch waits for the selection to rest; a plugin has waited for its instance already.
        let settle = spawn.supplied.is_empty();
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(spawn.doc.sample_rate);
        let monitor = stream.monitor();
        if self
            .sink
            .preview_start(stream, self.preview_gain())
            .is_err()
        {
            return false;
        }
        // Whatever a stale key left in the queue belongs to no preview.
        self.target.preview().clear();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(FollowStats::default());
        let (send, failures) = channel();
        let job = Job {
            spawn,
            settle,
            feed,
            target: Arc::clone(&self.target),
            stop: Arc::clone(&stop),
            stats: Arc::clone(&stats),
            failures: send,
        };
        let spawned = std::thread::Builder::new()
            .name("atomik-live-preview".into())
            .spawn(move || run(job));
        if spawned.is_err() {
            let _ = self.sink.preview_stop();
            return false;
        }
        self.live_follow.active = Some(LiveFollow {
            path: path.to_path_buf(),
            monitor,
            stop,
            stats,
            failures,
        });
        self.target.set(self.notes_target());
        true
    }

    /// The followed row moved to another row of the same plugin's presets.
    pub(super) fn live_follow_moved(&mut self, row: &Path) {
        if let Some(follow) = &mut self.live_follow.active {
            follow.path = row.to_path_buf();
        }
    }

    /// The scratch project of the item `path` and the folder of its audio.
    fn follow_doc(
        &self,
        path: &Path,
        kind: FileKind,
    ) -> Option<(atomik_model::RackDoc, Option<PathBuf>)> {
        let text = atomik_model::read_document_text(path).ok()?;
        let snapshot = atomik_model::load_device_item(&text, &self.registry).ok()?;
        if kind == FileKind::Container
            && atomik_model::ItemKind::of(&snapshot) != atomik_model::ItemKind::Container
        {
            return None;
        }
        let doc = super::patch_preview_ops::preview_doc(&snapshot, &self.registry).ok()?;
        let root = path.with_extension("");
        Some((doc, root.is_dir().then_some(root)))
    }

    /// Ends the keyboard preview and stops the voice when the stream is still the one playing.
    pub(super) fn stop_live_follow(&mut self) {
        if let Some(monitor) = self.live_follow.active.as_ref().map(|f| f.monitor.clone()) {
            self.drop_live_follow();
            if !monitor.is_abandoned() {
                let _ = self.sink.preview_stop();
            }
        }
    }

    /// Forgets the keyboard preview without touching the voice (another preview or a stop has it,
    /// or the engine is gone): the worker ends, the keys go back to the track.
    pub(super) fn drop_live_follow(&mut self) {
        self.end_plugin_session();
        if let Some(follow) = self.live_follow.active.take() {
            follow.stop.store(true, Ordering::Relaxed);
            self.target.set(self.notes_target());
        }
    }

    /// The frames the worker has pushed and how many of them were not silent (tests).
    #[cfg(test)]
    pub(crate) fn live_follow_frames(&self) -> Option<(u64, u64)> {
        self.live_follow.active.as_ref().map(|f| {
            (
                f.stats.fed.load(Ordering::Relaxed),
                f.stats.sounding.load(Ordering::Relaxed),
            )
        })
    }

    /// Whether the worker has built the scratch project (tests).
    #[cfg(test)]
    pub(crate) fn live_follow_built(&self) -> bool {
        self.live_follow
            .active
            .as_ref()
            .is_some_and(|f| f.stats.built.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_notes::PREVIEW_TARGET;
    use crate::sink::{RecordingSink, SinkCall};
    use atomik_core::NoteKind;
    use atomik_model::{Command, DeviceKey, DeviceSnapshot, Document, empty_doc, save_device_item};
    use atomik_render::default_registry;
    use atomik_ui::{BrowseAction, Place};
    use std::sync::atomic::AtomicU64;

    const SYNTH: &str = "atomik.poly-synth";
    const DELAY: &str = "atomik.stereo-delay";
    /// A generous bound for anything the worker does.
    const BOUND: Duration = Duration::from_secs(30);

    fn tmp(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-live-preview-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    /// The item text of one device of `type_id`, as Save to Library writes it.
    fn item_of(type_id: &str) -> String {
        let registry = default_registry();
        let mut doc = Document::new(empty_doc());
        doc.apply(
            Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.to_string(),
                key: Some(key("d0")),
            },
            &registry,
        )
        .unwrap();
        let snapshot = DeviceSnapshot::capture(doc.doc(), &[key("d0")], false).unwrap();
        save_device_item(&snapshot, doc.doc())
    }

    /// A core whose panel lists a folder with the library files `a.atomikpatch` (a synth),
    /// `b.atomikpatch` (an effect), `c.atomikpatch` (a utility that makes no sound) and
    /// `loop.wav`, with a synth on its rack: the track the keyboard plays when nothing is
    /// followed.
    fn core_listing() -> (AppCore<RecordingSink>, PathBuf) {
        let dir = tmp("core");
        std::fs::write(dir.join("a.atomikpatch"), item_of(SYNTH)).unwrap();
        std::fs::write(dir.join("b.atomikpatch"), item_of(DELAY)).unwrap();
        std::fs::write(dir.join("c.atomikpatch"), item_of("atomik.cv-math")).unwrap();
        let data: Vec<f32> = (0..4_800).map(|i| (i % 100) as f32 / 100.0).collect();
        atomik_audio_file::write_wav_f32(
            &dir.join("loop.wav"),
            &atomik_audio_file::AudioData::new(48_000, vec![data]).unwrap(),
        )
        .unwrap();
        let mut c =
            AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("track")),
        })
        .unwrap();
        c.after_view_change();
        c.view_mut().set_selection(&[key("track")], None);
        c.set_file_browser_visible(true);
        c.browse(BrowseAction::Show(Place::Folder(dir.clone())));
        until(&mut c, |c| !c.browser_busy());
        (c, dir)
    }

    /// Runs frames until `done` holds (a generous bound, never a count of frames).
    fn until(c: &mut AppCore<RecordingSink>, done: impl Fn(&AppCore<RecordingSink>) -> bool) {
        let mut waited = Duration::ZERO;
        loop {
            // Stands in for the voice, which plays what the worker feeds.
            for held in c.sink_mut().previews.iter_mut() {
                held.0.discard_buffered();
            }
            c.frame();
            if done(c) {
                return;
            }
            assert!(
                waited < BOUND,
                "the condition was not reached: {:?} {:?}",
                c.last_rejection(),
                c.live_follow_frames()
            );
            std::thread::sleep(Duration::from_millis(2));
            waited += Duration::from_millis(2);
        }
    }

    fn select(c: &mut AppCore<RecordingSink>, path: &Path) {
        assert!(c.file_browser.select_path(path), "{path:?} is listed");
        c.frame();
    }

    fn previews(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
        c.sink()
            .calls
            .iter()
            .filter(|call| matches!(call, SinkCall::PreviewStart { .. } | SinkCall::PreviewStop))
            .cloned()
            .collect()
    }

    fn sounding(c: &AppCore<RecordingSink>) -> u64 {
        c.live_follow_frames().map_or(0, |(_, sounding)| sounding)
    }

    fn notes_of(c: &AppCore<RecordingSink>) -> Vec<(crate::target::Target, NoteKind)> {
        c.sink()
            .calls
            .iter()
            .filter_map(|call| match call {
                SinkCall::PlayNote { target, kind } => Some((*target, *kind)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_selected_patch_takes_the_keyboard_and_the_track_gets_it_back() {
        let (mut c, dir) = core_listing();
        let track_target = c.notes_target().expect("the synth takes notes");
        assert_ne!(track_target, PREVIEW_TARGET);
        let revision = c.document().revision();
        select(&mut c, &dir.join("a.atomikpatch"));
        assert_eq!(previews(&c), [SinkCall::PreviewStart { gain: 1.0 }]);
        assert_eq!(c.notes_target(), Some(PREVIEW_TARGET));
        assert_eq!(c.note_target_cell().get(), Some(PREVIEW_TARGET));
        // The Piano Keys strip sends to the preview target.
        c.keys_note(60, 100, true);
        assert_eq!(
            notes_of(&c).last().map(|(t, _)| *t),
            Some(PREVIEW_TARGET),
            "{:?}",
            notes_of(&c)
        );
        // Notes of the MIDI input are queued for the worker, which sounds the patch.
        until(&mut c, AppCore::live_follow_built);
        assert_eq!(sounding(&c), 0, "silent until a key is played");
        c.note_target_cell().preview().push(NoteKind::On {
            key: 60,
            velocity: 0.8,
        });
        until(&mut c, |c| sounding(c) > 0);
        // Selecting something else ends it: the voice stops and the keys go back to the track.
        select(&mut c, &dir.join("loop.wav"));
        assert_eq!(previews(&c).last(), Some(&SinkCall::PreviewStop));
        assert!(c.live_follow_frames().is_none());
        assert_eq!(c.notes_target(), Some(track_target));
        assert_eq!(c.note_target_cell().get(), Some(track_target));
        c.keys_note(62, 100, true);
        assert_eq!(notes_of(&c).last().map(|(t, _)| *t), Some(track_target));
        assert_eq!(
            c.document().revision(),
            revision,
            "the document is untouched"
        );
        assert!(!c.sink().armed.contains(&PREVIEW_TARGET.0));
    }

    #[test]
    fn an_effect_patch_is_played_through_the_built_in_synth() {
        let (mut c, dir) = core_listing();
        select(&mut c, &dir.join("b.atomikpatch"));
        assert_eq!(c.notes_target(), Some(PREVIEW_TARGET));
        until(&mut c, AppCore::live_follow_built);
        c.note_target_cell().preview().push(NoteKind::On {
            key: 64,
            velocity: 0.8,
        });
        until(&mut c, |c| sounding(c) > 0);
    }

    #[test]
    fn an_item_that_cannot_be_played_is_left_alone_and_not_tried_every_frame() {
        let (mut c, dir) = core_listing();
        let track_target = c.notes_target();
        select(&mut c, &dir.join("c.atomikpatch"));
        for _ in 0..5 {
            c.frame();
        }
        assert!(previews(&c).is_empty());
        assert_eq!(c.notes_target(), track_target);
        assert!(
            c.last_rejection().is_none(),
            "no message for a row that is skipped"
        );
    }

    #[test]
    fn a_phrase_preview_keeps_the_voice_and_the_keyboard_follows_again_when_it_ends() {
        let (mut c, dir) = core_listing();
        let a = dir.join("a.atomikpatch");
        select(&mut c, &a);
        c.browse(BrowseAction::Preview(a.clone()));
        c.frame();
        assert_ne!(
            c.notes_target(),
            Some(PREVIEW_TARGET),
            "the phrase plays, the track has the keys"
        );
        assert_eq!(
            previews(&c)
                .iter()
                .filter(|p| matches!(p, SinkCall::PreviewStart { .. }))
                .count(),
            2
        );
        c.browse(BrowseAction::StopPreview);
        c.frame();
        assert_eq!(c.notes_target(), Some(PREVIEW_TARGET));
    }

    #[test]
    fn closing_the_panel_ends_it() {
        let (mut c, dir) = core_listing();
        select(&mut c, &dir.join("a.atomikpatch"));
        assert!(c.live_follow_has_keys());
        c.set_file_browser_visible(false);
        c.frame();
        assert!(!c.live_follow_has_keys());
        assert_ne!(c.notes_target(), Some(PREVIEW_TARGET));
    }
}
