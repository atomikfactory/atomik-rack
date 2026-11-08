//! `Workspace` tests: several songs under a backend that counts the streams it holds open.

use super::*;
use crate::app_core::AppCore;
use crate::session::{AudioBackend, LiveSession, LiveStream, MidiPort};
use crate::settings::AudioSettings;
use crate::shell::ShellOptions;
use atomik_audio_io::{HostInfo, IoError, StreamInfo, StreamSpec, Telemetry};
use atomik_engine::{Engine, FrameClock};
use atomik_midi_io::MidiError;
use atomik_model::{Command, DeviceKey};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A backend whose streams are counted: how many are open now and how many were ever started.
#[derive(Clone, Default)]
struct Counting {
    open: Arc<AtomicUsize>,
    started: Arc<AtomicUsize>,
}

struct CountedStream {
    info: StreamInfo,
    engine: Engine,
    open: Arc<AtomicUsize>,
}

impl LiveStream for CountedStream {
    fn telemetry(&self) -> Telemetry {
        Telemetry::default()
    }
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn stop(self) -> Result<Engine, IoError> {
        self.open.fetch_sub(1, Ordering::SeqCst);
        Ok(self.engine)
    }
}

struct NoMidi;

impl MidiPort for NoMidi {
    fn port_name(&self) -> &str {
        ""
    }
    fn dropped(&self) -> u64 {
        0
    }
    fn close(self) {}
}

impl AudioBackend for Counting {
    type Stream = CountedStream;
    type Midi = NoMidi;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        Ok(Vec::new())
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(Vec::new())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        _input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<CountedStream, IoError> {
        self.open.fetch_add(1, Ordering::SeqCst);
        self.started.fetch_add(1, Ordering::SeqCst);
        Ok(CountedStream {
            info: StreamInfo {
                host: spec.host,
                output_device: "Speakers".into(),
                input_device: None,
                sample_rate: spec.sample_rate,
                output_channels: spec.output_channels,
                input_channels: 0,
                sample_format: "f32",
                requested_buffer_frames: Some(256),
            },
            engine,
            open: Arc::clone(&self.open),
        })
    }

    fn open_midi(&self, name: &str, _link: crate::midi::MidiLink) -> Result<NoMidi, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
}

fn song(backend: &Counting, enabled: bool) -> Shell<Counting> {
    let session = LiveSession::new(backend.clone(), AudioSettings::default(), enabled);
    let mut shell = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    shell.handle(&UiEvent::ScaleChanged(2.0));
    shell.handle(&UiEvent::Resize(atomik_ui::kurbo::Size::new(1000.0, 700.0)));
    shell
}

fn workspace() -> (Workspace<Counting>, Counting) {
    let backend = Counting::default();
    let first = song(&backend, true);
    let again = backend.clone();
    let make: SongFactory<Counting> = Box::new(move |from, _id| {
        let mut audio = from.audio_settings();
        audio.surfaces.clear();
        let session = LiveSession::new(again.clone(), audio, false);
        Shell::new(
            AppCore::new(session),
            ShellOptions {
                os: Some(atomik_ui::keymap::Os::Windows),
                profile: None,
                in_window_menu: true,
                settings_path: None,
                ui_state_path: None,
                library_path: None,
                library_root: None,
            },
        )
    });
    (Workspace::new(first, make), backend)
}

fn add_synth(shell: &mut Shell<Counting>, key: &str) {
    shell
        .core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(key.into())),
        })
        .unwrap();
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-workspace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The synths of the song (adding one also adds the channel that carries it).
fn devices(shell: &Shell<Counting>) -> usize {
    shell
        .core()
        .doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.poly-synth")
        .count()
}

#[test]
fn a_workspace_starts_with_one_song_that_has_the_audio() {
    let (w, backend) = workspace();
    assert_eq!(w.ids(), vec![DocId::FIRST]);
    assert_eq!(w.play_focus(), DocId::FIRST);
    assert_eq!(w.routed(), DocId::FIRST);
    assert!(w.core().sink().is_running());
    assert!(!w.is_silent());
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
}

#[test]
fn the_last_song_cannot_be_closed() {
    let (mut w, backend) = workspace();
    assert!(!w.close_song(DocId::FIRST));
    assert_eq!(w.len(), 1);
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
}

#[test]
fn a_new_song_takes_the_audio_and_the_old_one_goes_silent() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    assert_ne!(second, DocId::FIRST);
    assert_eq!(w.ids(), vec![DocId::FIRST, second]);
    assert_eq!(w.play_focus(), second);
    assert_eq!(w.routed(), second);
    // One stream is open at any time, and it is the new song's.
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    assert!(w.song(second).unwrap().core().sink().is_running());
    let first = w.song(DocId::FIRST).unwrap();
    assert!(!first.core().sink().is_running());
    assert!(first.is_silent());
    assert!(first.status_text().contains("another song has the audio"));
}

#[test]
fn the_focus_moves_the_audio_back_and_forth_without_a_second_stream() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    for _ in 0..3 {
        assert!(w.focus(DocId::FIRST));
        assert_eq!(backend.open.load(Ordering::SeqCst), 1);
        assert!(w.song(DocId::FIRST).unwrap().core().sink().is_running());
        assert!(!w.song(second).unwrap().core().sink().is_running());
        assert!(w.focus(second));
        assert_eq!(backend.open.load(Ordering::SeqCst), 1);
        assert!(w.song(second).unwrap().core().sink().is_running());
        assert!(!w.song(DocId::FIRST).unwrap().core().sink().is_running());
    }
    // Focusing the song that has the audio again restarts nothing.
    let started = backend.started.load(Ordering::SeqCst);
    assert!(w.focus(second));
    assert_eq!(backend.started.load(Ordering::SeqCst), started);
    assert!(!w.focus(DocId(99)));
}

#[test]
fn the_transport_of_the_song_that_loses_the_audio_stops() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    w.execute(AppCommand::Transport(atomik_ui::TransportAction::Play));
    assert!(w.core().is_playing());
    let second = w.new_song();
    assert!(!w.song(DocId::FIRST).unwrap().core().is_playing());
    assert!(!w.song(second).unwrap().core().is_playing());
}

#[test]
fn two_songs_edit_independently_with_separate_undo() {
    let (mut w, _backend) = workspace();
    let start = devices(&w);
    add_synth(&mut w, "lead");
    assert_eq!(devices(&w), start + 1);
    let second = w.new_song();
    assert_eq!(devices(&w), start, "the new song starts as a new song");
    add_synth(&mut w, "bass");
    add_synth(&mut w, "pad");
    assert_eq!(devices(w.song(DocId::FIRST).unwrap()), start + 1);
    assert_eq!(devices(w.song(second).unwrap()), start + 2);
    // Undo in the second song leaves the first untouched.
    assert!(w.song_mut(second).unwrap().core_mut().undo());
    assert_eq!(devices(w.song(second).unwrap()), start + 1);
    assert_eq!(devices(w.song(DocId::FIRST).unwrap()), start + 1);
    // And the first song's own history is its own.
    w.focus(DocId::FIRST);
    assert!(w.core_mut().undo());
    assert_eq!(devices(w.song(DocId::FIRST).unwrap()), start);
    assert_eq!(devices(w.song(second).unwrap()), start + 1);
    assert!(
        !w.core_mut().undo(),
        "the first song has nothing more to undo"
    );
}

#[test]
fn closing_a_song_gives_the_audio_to_its_neighbour_and_keeps_one_stream() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    let third = w.new_song();
    assert_eq!(w.play_focus(), third);
    // A song that does not have the audio closes without touching it.
    assert!(w.close_song(DocId::FIRST));
    assert_eq!(w.ids(), vec![second, third]);
    assert_eq!(w.play_focus(), third);
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    // The song that has it hands it on.
    assert!(w.close_song(third));
    assert_eq!(w.ids(), vec![second]);
    assert_eq!(w.play_focus(), second);
    assert_eq!(w.routed(), second);
    assert!(w.core().sink().is_running());
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    assert!(!w.is_silent());
}

#[test]
fn the_ids_of_closed_songs_are_not_reused() {
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    assert!(w.close_song(second));
    let third = w.new_song();
    assert_ne!(third, second);
}

#[test]
fn select_routes_calls_without_moving_the_audio() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    let started = backend.started.load(Ordering::SeqCst);
    assert!(w.select(DocId::FIRST));
    assert_eq!(w.routed(), DocId::FIRST);
    assert_eq!(w.play_focus(), second);
    assert_eq!(backend.started.load(Ordering::SeqCst), started);
    assert!(!w.select(DocId(42)));
    assert_eq!(w.routed(), DocId::FIRST);
}

#[test]
fn every_song_knows_the_open_songs_and_its_place_in_the_list() {
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    let names = w.titles();
    assert_eq!(names.len(), 2);
    for (i, id) in [DocId::FIRST, second].into_iter().enumerate() {
        let (list, at) = w.song(id).unwrap().song_list();
        assert_eq!(list, names.as_slice());
        assert_eq!(at, i);
    }
    w.close_song(DocId::FIRST);
    assert_eq!(w.song(second).unwrap().song_list().0.len(), 1);
}

#[test]
fn a_song_that_does_not_open_leaves_no_new_song() {
    let (mut w, backend) = workspace();
    let missing = temp("missing-song.atomik");
    let _ = std::fs::remove_file(&missing);
    assert_eq!(w.open_song(&missing), None);
    assert_eq!(w.ids(), vec![DocId::FIRST]);
    assert_eq!(w.play_focus(), DocId::FIRST);
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    assert!(w.core().sink().is_running());
    assert!(w.core().last_rejection().is_some(), "the message is shown");
}

#[test]
fn a_song_opens_in_a_window_of_its_own_and_the_first_one_is_left_alone() {
    let (mut w, backend) = workspace();
    let start = devices(&w);
    add_synth(&mut w, "lead");
    let path = temp("two-songs.atomik");
    w.core_mut().save_as(&path).unwrap();
    add_synth(&mut w, "bass");
    let opened = w.open_song(&path).expect("the song opens");
    assert_eq!(w.ids().len(), 2);
    assert_eq!(w.play_focus(), opened);
    assert_eq!(devices(w.song(opened).unwrap()), start + 1);
    assert_eq!(w.song(opened).unwrap().core().file(), Some(path.as_path()));
    // The first song keeps its unsaved second device.
    assert_eq!(devices(w.song(DocId::FIRST).unwrap()), start + 2);
    assert!(w.song(DocId::FIRST).unwrap().core().is_dirty());
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn quitting_asks_about_every_song_with_unsaved_changes() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let second = w.new_song();
    add_synth(&mut w, "bass");
    w.focus(DocId::FIRST);
    let quit = PendingAction::Command(AppCommand::Quit);
    // The first song asks...
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    // ...Don't Save moves on to the second, which gets the focus and the question...
    assert_eq!(
        w.discard_then(quit.clone()),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.play_focus(), second);
    assert_eq!(w.routed(), second);
    // ...and with every answer given the app quits.
    assert_eq!(w.discard_then(quit), vec![ShellRequest::Quit]);
}

#[test]
fn a_cancelled_quit_asks_again_next_time() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let second = w.new_song();
    add_synth(&mut w, "bass");
    w.focus(DocId::FIRST);
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    // Don't Save for the first song, then Cancel at the second's question.
    w.discard_then(quit.clone());
    assert_eq!(w.routed(), second);
    // A new quit starts over: the first song is asked again.
    w.focus(DocId::FIRST);
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.routed(), DocId::FIRST);
}

#[test]
fn a_clean_song_quits_at_once_when_nothing_else_has_changes() {
    let (mut w, _backend) = workspace();
    w.new_song();
    assert_eq!(w.execute(AppCommand::Quit), vec![ShellRequest::Quit]);
}

#[test]
fn a_clean_song_still_asks_for_another_song_with_changes() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let second = w.new_song();
    assert_eq!(w.routed(), second);
    // The second song is clean; the first has changes, so it gets the question.
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::Command(AppCommand::Quit)
        }]
    );
    assert_eq!(w.routed(), DocId::FIRST);
    assert_eq!(w.play_focus(), DocId::FIRST);
}

#[test]
fn the_answer_goes_to_the_song_that_asked() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let second = w.new_song();
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    // Whatever the platform layer routed in between, the answer is for the first song.
    w.select(second);
    let path = temp("answered.atomik");
    let again = w.save_to_then(&path, quit);
    assert_eq!(again, vec![ShellRequest::Quit]);
    assert_eq!(
        w.song(DocId::FIRST).unwrap().core().file(),
        Some(path.as_path())
    );
    assert!(w.song(second).unwrap().core().file().is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_preference_changed_in_one_song_reaches_the_others() {
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    w.select(second);
    let folder = temp("scratch-folder");
    w.set_scratch_folder(folder);
    assert_ne!(
        w.song(DocId::FIRST).unwrap().shared_preferences(),
        w.song(second).unwrap().shared_preferences()
    );
    w.sync_preferences();
    assert_eq!(
        w.song(DocId::FIRST).unwrap().shared_preferences(),
        w.song(second).unwrap().shared_preferences()
    );
}

#[test]
fn a_new_song_opens_with_the_layout_of_the_song_it_came_from() {
    let (mut w, _backend) = workspace();
    w.execute(AppCommand::ToggleFileBrowser);
    let shown = w.menu_state().file_browser;
    let second = w.new_song();
    assert_eq!(w.song(second).unwrap().menu_state().file_browser, shown);
}

#[test]
fn background_songs_keep_working_between_frames() {
    let (mut w, _backend) = workspace();
    let _second = w.new_song();
    // Frames and polls reach every song and do not start a stream for the silent one.
    w.frame_all(0.5);
    w.poll_all();
    assert!(w.song(DocId::FIRST).unwrap().is_silent());
    assert!(!w.any_busy());
}

#[test]
fn neighbours_wrap_around() {
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    let third = w.new_song();
    assert_eq!(w.neighbour(true), DocId::FIRST);
    assert_eq!(w.neighbour(false), second);
    w.focus(DocId::FIRST);
    assert_eq!(w.neighbour(true), second);
    assert_eq!(w.neighbour(false), third);
}

#[test]
fn shutdown_closes_every_song() {
    let (mut w, backend) = workspace();
    w.new_song();
    w.shutdown_all();
    assert_eq!(backend.open.load(Ordering::SeqCst), 0);
}

#[test]
fn the_new_song_window_command_opens_a_song_and_asks_for_its_window() {
    let (mut w, backend) = workspace();
    let requests = w.execute(AppCommand::NewSongWindow);
    let ids = w.ids();
    assert_eq!(ids.len(), 2);
    assert_eq!(requests, vec![ShellRequest::FocusSong(ids[1])]);
    assert_eq!(w.play_focus(), ids[1]);
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
}

#[test]
fn the_open_song_window_command_asks_for_a_file() {
    let (mut w, _backend) = workspace();
    assert_eq!(
        w.execute(AppCommand::OpenSongWindow),
        vec![ShellRequest::PickOpenSong]
    );
    assert_eq!(w.len(), 1, "the song opens when the file is picked");
}

#[test]
fn next_previous_and_a_listed_song_move_the_audio() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    let third = w.new_song();
    assert_eq!(
        w.execute(AppCommand::NextSong),
        vec![ShellRequest::FocusSong(DocId::FIRST)]
    );
    assert_eq!(w.play_focus(), DocId::FIRST);
    assert_eq!(
        w.execute(AppCommand::PreviousSong),
        vec![ShellRequest::FocusSong(third)]
    );
    assert_eq!(w.play_focus(), third);
    assert_eq!(
        w.execute(AppCommand::ActivateSong(1)),
        vec![ShellRequest::FocusSong(second)]
    );
    assert_eq!(w.play_focus(), second);
    assert_eq!(w.routed(), second);
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    // A place beyond the list does nothing.
    assert_eq!(w.execute(AppCommand::ActivateSong(8)), Vec::new());
    assert_eq!(w.play_focus(), second);
}

#[test]
fn closing_a_clean_song_window_needs_no_question() {
    let (mut w, backend) = workspace();
    let second = w.new_song();
    assert_eq!(w.execute(AppCommand::CloseSongWindow), Vec::new());
    assert_eq!(w.ids(), vec![DocId::FIRST]);
    assert_eq!(w.play_focus(), DocId::FIRST);
    assert!(w.core().sink().is_running());
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    assert!(w.song(second).is_none());
}

#[test]
fn closing_a_song_window_with_changes_runs_its_own_save_prompt() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let second = w.new_song();
    add_synth(&mut w, "bass");
    w.focus(DocId::FIRST);
    // The prompt is for the song whose window is closed, the other song's changes stay put.
    w.select(second);
    let close = PendingAction::Command(AppCommand::CloseSongWindow);
    assert_eq!(
        w.execute(AppCommand::CloseSongWindow),
        vec![ShellRequest::ConfirmSave {
            then: close.clone()
        }]
    );
    assert_eq!(w.ids().len(), 2, "nothing closes before the answer");
    assert_eq!(w.discard_then(close), Vec::new());
    assert_eq!(w.ids(), vec![DocId::FIRST]);
    assert!(
        w.song(DocId::FIRST).unwrap().core().is_dirty(),
        "the other song keeps its changes and its question for later"
    );
}

#[test]
fn saving_in_the_close_prompt_keeps_the_file_and_closes_the_song() {
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    add_synth(&mut w, "bass");
    let path = temp("closed-song.atomik");
    let close = PendingAction::Command(AppCommand::CloseSongWindow);
    w.execute(AppCommand::CloseSongWindow);
    assert_eq!(w.save_to_then(&path, close), Vec::new());
    assert!(path.exists(), "the song was saved before it closed");
    assert!(w.song(second).is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn closing_the_last_song_window_quits() {
    let (mut w, _backend) = workspace();
    assert_eq!(
        w.execute(AppCommand::CloseSongWindow),
        vec![ShellRequest::Quit]
    );
    assert_eq!(w.len(), 1);
}

#[test]
fn the_window_menu_lists_the_open_songs_and_checks_the_one_with_the_audio() {
    let (mut w, _backend) = workspace();
    let menu_of = |w: &Workspace<Counting>| {
        w.menu_model()
            .into_iter()
            .find(|m| m.title == "Window")
            .expect("a Window menu")
    };
    let single = menu_of(&w);
    assert!(
        single
            .items()
            .all(|i| !matches!(i.command, AppCommand::ActivateSong(_))),
        "one song needs no list"
    );
    let second = w.new_song();
    w.focus(DocId::FIRST);
    let menu = menu_of(&w);
    let songs: Vec<_> = menu
        .items()
        .filter(|i| matches!(i.command, AppCommand::ActivateSong(_)))
        .collect();
    assert_eq!(songs.len(), 2);
    assert_eq!(songs[0].command, AppCommand::ActivateSong(0));
    assert_eq!(songs[0].checked, Some(true));
    assert_eq!(songs[1].checked, Some(false));
    assert_eq!(songs[1].label.as_ref(), w.song(second).unwrap().song_name());
    for command in [
        AppCommand::CloseSongWindow,
        AppCommand::NextSong,
        AppCommand::PreviousSong,
    ] {
        let item = menu.items().find(|i| i.command == command).unwrap();
        assert!(item.enabled, "{command:?} works with two songs");
    }
    // The menu belongs to the song it is shown in: the second song checks the second entry.
    w.select(second);
    let own = menu_of(&w);
    let checked: Vec<_> = own
        .items()
        .filter(|i| matches!(i.command, AppCommand::ActivateSong(_)))
        .map(|i| i.checked)
        .collect();
    assert_eq!(checked, vec![Some(false), Some(true)]);
}

#[test]
fn song_windows_have_keys() {
    let (mut w, _backend) = workspace();
    let keys = |w: &Workspace<Counting>, c: AppCommand| {
        w.keymap().shortcut_text(c.command_id().unwrap().0, None)
    };
    for command in [
        AppCommand::NewSongWindow,
        AppCommand::OpenSongWindow,
        AppCommand::CloseSongWindow,
        AppCommand::NextSong,
        AppCommand::PreviousSong,
    ] {
        assert!(keys(&w, command).is_some(), "{command:?} has a key");
    }
    // The key of New Song Window does what the menu item does.
    let (key, modifiers) =
        atomik_ui::keymap::script::event_for(atomik_ui::keymap::Os::Windows, "Cmd+Shift+N")
            .unwrap();
    let press = UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    };
    let requests = w.handle(&press);
    assert_eq!(w.len(), 2, "{requests:?}");
}

#[test]
fn a_copy_in_one_song_pastes_in_the_song_the_calls_are_routed_to_next() {
    use crate::clipboard::ClipboardKind;
    use atomik_model::{Clip, ClipContent, ClipId, Note, SongCommand};
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let core = w.core_mut();
    core.apply_command(Command::Song(SongCommand::AddTrack {
        device: DeviceKey("lead".into()),
    }))
    .unwrap();
    let take = |key: u8| Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * atomik_core::PPQ,
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
        content: ClipContent::Notes(vec![Note::new(0, atomik_core::PPQ, key, 0.8)].into()),
    };
    core.apply_command(Command::Song(SongCommand::NewTakeGroup {
        device: DeviceKey("lead".into()),
        lane: 0,
        takes: vec![take(60), take(62)],
    }))
    .unwrap();
    core.copy_takes(1, &[0, 1]);
    assert_eq!(core.clipboard().kind(), Some(ClipboardKind::Takes));
    // A new song takes the focus and sees the same clipboard.
    let second = w.new_song();
    assert_eq!(
        w.song(second).unwrap().core().clipboard().kind(),
        Some(ClipboardKind::Takes)
    );
    // An empty clipboard in the song the focus leaves does not wipe the other one.
    w.focus(DocId::FIRST);
    w.core_mut().clipboard_mut().clear();
    w.focus(second);
    assert_eq!(w.core().clipboard().get().map(|d| d.len()), Some(2));
}

/// A mono ramp wav in a fresh temporary folder: the folder and the file.
fn ramp_file(tag: &str) -> (PathBuf, PathBuf) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-workspace-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let file = dir.join("ramp.wav");
    atomik_audio_file::write_wav_f32(
        &file,
        &atomik_audio_file::AudioData::new(48_000, vec![data]).unwrap(),
    )
    .unwrap();
    (dir, file)
}

#[test]
fn audio_clips_copied_in_one_song_bring_their_audio_to_the_song_they_are_pasted_in() {
    use crate::app_core::Pane;
    use atomik_ui::ImportTarget;
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("paste-audio");
    let first = w.routed();
    w.core_mut()
        .import_audio(vec![file], ImportTarget::NewTracks, 0);
    w.core_mut().wait_background();
    let clip = w.core().doc().song.tracks[0].lanes[0].clips[0].id;
    let hash = w.core().doc().assets.keys().next().cloned().unwrap();
    w.core_mut().arrangement_mut().set_selected_clips(&[clip]);
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.copy", None);

    let second = w.new_song();
    assert_ne!(second, first);
    assert!(
        w.core().doc().assets.is_empty(),
        "the new song has no audio"
    );
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    w.core_mut().wait_background();

    assert!(
        w.core().doc().assets.contains_key(&hash),
        "the asset is registered"
    );
    let root = w
        .core()
        .bundle_root()
        .expect("the song got a bundle")
        .to_path_buf();
    assert!(
        root.join("Audio").join(format!("{hash}.wav")).is_file(),
        "the file was copied into the song's own bundle"
    );
    assert_ne!(
        Some(root.as_path()),
        w.song(first).unwrap().core().bundle_root()
    );
    assert!(
        w.core().missing_assets().is_empty(),
        "{:?}",
        w.core().missing_assets()
    );
    assert_eq!(w.core().doc().song.tracks.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_paste_whose_source_song_lost_the_file_lists_it_as_missing() {
    use crate::app_core::Pane;
    use atomik_ui::ImportTarget;
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("paste-gone");
    w.core_mut()
        .import_audio(vec![file], ImportTarget::NewTracks, 0);
    w.core_mut().wait_background();
    let clip = w.core().doc().song.tracks[0].lanes[0].clips[0].id;
    let hash = w.core().doc().assets.keys().next().cloned().unwrap();
    let first_root = w.core().bundle_root().unwrap().to_path_buf();
    w.core_mut().arrangement_mut().set_selected_clips(&[clip]);
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.copy", None);
    let _second = w.new_song();
    std::fs::remove_file(first_root.join("Audio").join(format!("{hash}.wav"))).unwrap();
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    w.core_mut().wait_background();
    let missing: Vec<&str> = w
        .core()
        .missing_assets()
        .iter()
        .map(|m| m.hash.as_str())
        .collect();
    assert_eq!(missing, vec![hash.as_str()]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Song 1 holds one audio clip (a 48 kHz ramp) that is selected and copied; song 2 is made at
/// `rate` and has the keys. Returns the workspace, the ramp's folder and the original asset.
fn copied_to_a_song_at(rate: u32, tag: &str) -> (Workspace<Counting>, PathBuf, String) {
    use crate::app_core::Pane;
    use atomik_ui::ImportTarget;
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file(tag);
    w.core_mut()
        .import_audio(vec![file], ImportTarget::NewTracks, 0);
    w.core_mut().wait_background();
    let clip = w.core().doc().song.tracks[0].lanes[0].clips[0].id;
    let hash = w.core().doc().assets.keys().next().cloned().unwrap();
    w.core_mut().arrangement_mut().set_selected_clips(&[clip]);
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.copy", None);
    let _second = w.new_song();
    w.core_mut().new_document_at(rate);
    (w, dir, hash)
}

fn pasted_audio(w: &Workspace<Counting>) -> Vec<(i64, String, u64)> {
    use atomik_model::ClipContent;
    w.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| &l.clips))
        .filter_map(|c| match &c.content {
            ClipContent::Audio(a) => Some((c.start, a.asset.clone(), a.source_offset)),
            _ => None,
        })
        .collect()
}

#[test]
fn audio_pasted_from_a_song_at_another_rate_is_converted_on_a_worker_in_one_undo_step() {
    use crate::app_core::Pane;
    let (mut w, dir, hash) = copied_to_a_song_at(44_100, "paste-rate");
    let first_root = w
        .song(DocId::FIRST)
        .unwrap()
        .core()
        .bundle_root()
        .unwrap()
        .to_path_buf();
    let original = first_root.join("Audio").join(format!("{hash}.wav"));
    let before = std::fs::read(&original).unwrap();
    let steps = w.core().doc().song.tracks.len();

    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    // The paste waits for the worker: nothing is in the song yet.
    assert_eq!(w.core().pastes_converting(), 1);
    assert!(pasted_audio(&w).is_empty());
    w.core_mut().wait_background();
    assert!(
        w.core().last_rejection().is_none(),
        "{:?}",
        w.core().last_rejection()
    );
    assert_eq!(w.core().pastes_converting(), 0);

    let pasted = pasted_audio(&w);
    assert_eq!(pasted.len(), 1, "{pasted:?}");
    let (_, asset, _) = &pasted[0];
    assert_ne!(asset, &hash, "the clip plays the converted copy");
    let meta = &w.core().doc().assets[asset];
    assert_eq!((meta.sample_rate, meta.frames), (44_100, 4_410));
    assert!(
        !w.core().doc().assets.contains_key(&hash),
        "the original is not registered"
    );
    // The copy is in the song's own bundle; the original file is untouched and not copied over.
    let root = w.core().bundle_root().expect("a bundle").to_path_buf();
    assert!(root.join("Audio").join(format!("{asset}.wav")).is_file());
    assert!(!root.join("Audio").join(format!("{hash}.wav")).exists());
    assert_eq!(std::fs::read(&original).unwrap(), before);
    assert!(w.core().missing_assets().is_empty());
    // One undo step takes the whole paste back, and redo brings it again.
    assert!(w.core_mut().undo());
    assert!(pasted_audio(&w).is_empty());
    assert_eq!(w.core().doc().song.tracks.len(), steps);
    assert!(w.core_mut().redo());
    assert_eq!(pasted_audio(&w).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_second_paste_of_converted_audio_needs_no_worker() {
    use crate::app_core::Pane;
    let (mut w, dir, _hash) = copied_to_a_song_at(96_000, "paste-rate-twice");
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    w.core_mut().wait_background();
    let first = pasted_audio(&w);
    assert_eq!(first.len(), 1);
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    assert_eq!(w.core().pastes_converting(), 0, "the copy is known");
    let both = pasted_audio(&w);
    assert_eq!(both.len(), 2, "{both:?}");
    assert_eq!(both[0].1, both[1].1, "both clips share the one copy");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_paste_at_another_rate_whose_source_lost_the_file_changes_nothing() {
    use crate::app_core::Pane;
    let (mut w, dir, hash) = copied_to_a_song_at(44_100, "paste-rate-gone");
    let first_root = w
        .song(DocId::FIRST)
        .unwrap()
        .core()
        .bundle_root()
        .unwrap()
        .to_path_buf();
    std::fs::remove_file(first_root.join("Audio").join(format!("{hash}.wav"))).unwrap();
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    w.core_mut().wait_background();
    assert!(pasted_audio(&w).is_empty());
    assert!(
        w.core()
            .last_rejection()
            .is_some_and(|m| m.contains("paste"))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn audio_pasted_at_the_same_rate_is_not_converted() {
    use crate::app_core::Pane;
    let (mut w, dir, hash) = copied_to_a_song_at(48_000, "paste-rate-same");
    let _ = w
        .core_mut()
        .pane_command(Pane::Arrangement, "edit.paste", None);
    assert_eq!(w.core().pastes_converting(), 0);
    w.core_mut().wait_background();
    let pasted = pasted_audio(&w);
    assert_eq!(pasted.len(), 1);
    assert_eq!(pasted[0].1, hash, "the same file, as before");
    let _ = std::fs::remove_dir_all(dir);
}

fn pointer_down(p: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: atomik_ui::Modifiers::NONE,
        clicks: 1,
    }
}

fn pointer_move(p: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: atomik_ui::Modifiers::NONE,
    }
}

fn over(kind: WindowKind, p: atomik_ui::kurbo::Point) -> WindowHit {
    WindowHit {
        kind,
        logical: (p.x, p.y),
    }
}

#[test]
fn clips_dragged_onto_another_songs_window_are_pasted_there_with_their_audio_at_its_rate() {
    use atomik_ui::kurbo::{Point, Vec2};
    let (mut w, dir, hash) = copied_to_a_song_at(44_100, "drag-clips");
    let (first, second) = (DocId::FIRST, w.ids()[1]);
    w.song_mut(second)
        .unwrap()
        .handle(&UiEvent::Resize(atomik_ui::kurbo::Size::new(1000.0, 700.0)));
    // Take the copy back out of both clipboards: the drag must not need it.
    for id in [first, second] {
        w.song_mut(id).unwrap().core_mut().clipboard_mut().clear();
    }
    // Press on the clip in the first song and drag it away.
    let clip = w.song(first).unwrap().core().doc().song.tracks[0].lanes[0].clips[0].clone();
    let rect = w
        .song(first)
        .unwrap()
        .core()
        .arrangement()
        .clip_rect(clip.id)
        .unwrap();
    let from = Point::new(rect.center().x, rect.center().y);
    {
        let shell = w.song_mut(first).unwrap();
        shell.handle(&pointer_down(from));
        shell.handle(&pointer_move(from + Vec2::new(40.0, 0.0)));
        shell.handle(&pointer_move(from + Vec2::new(400.0, 0.0)));
        assert_eq!(shell.song_drag(), Some(crate::app_core::Pane::Arrangement));
    }
    // Let go over the arrangement of the second song.
    let layout_song = w
        .song(second)
        .unwrap()
        .layout()
        .song
        .expect("the lower pane shows");
    let target = Point::new(layout_song.x0 + 500.0, layout_song.y0 + 80.0);
    assert!(w.drag_to_song(first, second, over(WindowKind::Main, target)));
    w.song_mut(second).unwrap().core_mut().wait_background();
    let second_core = w.song(second).unwrap().core();
    assert!(
        second_core.last_rejection().is_none(),
        "{:?}",
        second_core.last_rejection()
    );

    // The second song has the clip, converted to its rate; it is one undo step.
    let pasted: Vec<_> = second_core
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| &l.clips))
        .filter_map(|c| match &c.content {
            atomik_model::ClipContent::Audio(a) => Some(a.asset.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(pasted.len(), 1, "{pasted:?}");
    assert_ne!(pasted[0], hash);
    assert_eq!(second_core.doc().assets[&pasted[0]].sample_rate, 44_100);
    // The first song still has its clip where it was and nothing changed there but the drag's
    // own undo-neutral steps; neither clipboard was filled.
    let first_core = w.song(first).unwrap().core();
    let after = &first_core.doc().song.tracks[0].lanes[0].clips[0];
    assert_eq!((after.start, after.length), (clip.start, clip.length));
    assert!(first_core.clipboard().get().is_none());
    assert!(w.song(second).unwrap().core().clipboard().get().is_none());
    assert_eq!(w.song(first).unwrap().song_drag(), None, "the drag ended");
    w.song_mut(second).unwrap().core_mut().undo();
    let undone = w.song(second).unwrap().core();
    assert!(
        undone
            .doc()
            .song
            .tracks
            .iter()
            .all(|t| t.lanes.iter().all(|l| l.clips.is_empty())),
        "one undo takes the whole drop back"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn devices_dragged_onto_another_songs_rack_are_pasted_there_and_stay_in_the_first() {
    use atomik_ui::kurbo::{Point, Vec2};
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let first = DocId::FIRST;
    // A tall window, so the synth at the end of the rack is on the screen.
    w.song_mut(first)
        .unwrap()
        .handle(&UiEvent::Resize(atomik_ui::kurbo::Size::new(
            1000.0, 2400.0,
        )));
    let second = w.new_song();
    w.song_mut(second)
        .unwrap()
        .handle(&UiEvent::Resize(atomik_ui::kurbo::Size::new(
            1000.0, 2400.0,
        )));
    let before = devices(w.song(second).unwrap());
    // Press on the plate of the synth in the first song and drag it off the window.
    let origin = |s: &Shell<Counting>| {
        s.pane_rect(crate::app_core::Pane::Rack)
            .map_or(Point::ORIGIN, |(_, r)| r.origin())
    };
    let plate = {
        let s = w.song(first).unwrap();
        let p = s
            .core()
            .view()
            .plate_center(&DeviceKey("lead".into()))
            .unwrap();
        p + origin(s).to_vec2()
    };
    {
        let shell = w.song_mut(first).unwrap();
        shell.handle(&pointer_down(plate));
        shell.handle(&pointer_move(plate + Vec2::new(20.0, 90.0)));
        shell.handle(&pointer_move(plate + Vec2::new(-200.0, 200.0)));
        assert_eq!(shell.song_drag(), Some(crate::app_core::Pane::Rack));
    }
    let rack = w
        .song(second)
        .unwrap()
        .pane_rect(crate::app_core::Pane::Rack)
        .unwrap()
        .1;
    let target = Point::new(rack.x0 + 400.0, rack.y0 + 300.0);
    assert!(w.drag_to_song(first, second, over(WindowKind::Main, target)));
    assert_eq!(devices(w.song(second).unwrap()), before + 1);
    assert_eq!(
        devices(w.song(first).unwrap()),
        1,
        "the first song keeps its synth"
    );
    // One undo step in the second song.
    w.song_mut(second).unwrap().core_mut().undo();
    assert_eq!(devices(w.song(second).unwrap()), before);
}

#[test]
fn a_drag_that_carries_nothing_or_stays_in_one_song_is_not_taken() {
    use atomik_ui::kurbo::Point;
    let (mut w, _backend) = workspace();
    let second = w.new_song();
    let at = over(WindowKind::Main, Point::new(500.0, 300.0));
    // No drag is in progress.
    assert!(!w.drag_to_song(DocId::FIRST, second, at));
    assert!(w.song_mut(DocId::FIRST).unwrap().take_song_drag().is_none());
    // The same song is no other song; an unknown one is none.
    assert!(!w.drag_to_song(DocId::FIRST, DocId::FIRST, at));
    assert!(!w.drag_to_song(DocId::FIRST, DocId(77), at));
    assert!(!w.files_to_song(DocId::FIRST, second, at, false));
    assert!(w.file_drag_paths(DocId::FIRST).is_none());
}

#[test]
fn a_song_saved_in_one_window_shows_in_the_recent_list_of_every_song() {
    let (mut w, _backend) = workspace();
    let first = w.routed();
    let second = w.new_song();
    let dir = std::env::temp_dir().join(format!("atomik-workspace-recent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.atomik"), dir.join("b.atomik"));
    w.core_mut().save_as(&a).unwrap();
    w.frame_all(0.0);
    assert_eq!(
        w.song(first).unwrap().core().recent(),
        std::slice::from_ref(&a)
    );
    assert_eq!(
        w.song(second).unwrap().core().recent(),
        std::slice::from_ref(&a)
    );

    // The other song saves too: the list is the same everywhere, newest first.
    w.select(first);
    w.core_mut().save_as(&b).unwrap();
    w.frame_all(0.0);
    for id in [first, second] {
        assert_eq!(
            w.song(id).unwrap().core().recent(),
            [b.clone(), a.clone()],
            "song {id:?}"
        );
    }
    // Clearing the list in one song clears it in the others.
    w.core_mut().clear_recent();
    w.frame_all(0.0);
    assert!(w.song(second).unwrap().core().recent().is_empty());
    // A song opened later starts from the shared list.
    w.core_mut().save_as(&a).unwrap();
    let third = w.new_song();
    assert_eq!(w.song(third).unwrap().core().recent(), [a]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_song_pinned_in_one_window_is_pinned_in_every_song_and_survives_clear_menu() {
    let (mut w, _backend) = workspace();
    let first = w.routed();
    let second = w.new_song();
    let dir = std::env::temp_dir().join(format!("atomik-workspace-pins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.atomik"), dir.join("b.atomik"));
    w.core_mut().save_as(&a).unwrap();
    w.frame_all(0.0);
    w.select(first);
    w.core_mut().save_as(&b).unwrap();
    w.frame_all(0.0);
    // Pin the older song in one window: it moves to the top of the list of both.
    assert!(w.core_mut().pin_recent(&a));
    w.frame_all(0.0);
    for id in [first, second] {
        let core = w.song(id).unwrap().core();
        assert_eq!(core.recent(), [a.clone(), b.clone()], "song {id:?}");
        assert_eq!(core.pinned_count(), 1, "song {id:?}");
    }
    // Clear Menu in the other window keeps the pin.
    w.select(second);
    w.core_mut().clear_recent();
    w.frame_all(0.0);
    for id in [first, second] {
        let core = w.song(id).unwrap().core();
        assert_eq!(core.recent(), std::slice::from_ref(&a), "song {id:?}");
        assert!(core.is_pinned(&a));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_favourite_marked_in_one_song_shows_in_the_others_and_a_new_song_starts_with_it() {
    use atomik_ui::BrowseAction;
    let (mut w, _backend) = workspace();
    let first = w.routed();
    let second = w.new_song();
    let pick = std::env::temp_dir().join("atomik-workspace-fav.wav");
    w.core_mut()
        .browse(BrowseAction::ToggleFavorite(vec![pick.clone()]));
    w.frame_all(0.0);
    assert!(
        w.song(second)
            .unwrap()
            .core()
            .library_db()
            .is_favorite(&pick)
    );
    assert!(
        w.song(first)
            .unwrap()
            .core()
            .library_db()
            .is_favorite(&pick)
    );
    // The change made in the other song reaches this one, and does not undo the first.
    w.select(first);
    let other = std::env::temp_dir().join("atomik-workspace-fav-2.wav");
    w.core_mut()
        .browse(BrowseAction::ToggleFavorite(vec![other.clone()]));
    w.frame_all(0.0);
    let db = w.song(second).unwrap().core().library_db();
    assert!(db.is_favorite(&pick) && db.is_favorite(&other));
    let third = w.new_song();
    let db = w.song(third).unwrap().core().library_db();
    assert!(db.is_favorite(&pick) && db.is_favorite(&other));
    // Adopting a change is not a change: a song that only took it writes nothing.
    assert_eq!(w.song(third).unwrap().core().library_revision(), 0);
    assert_eq!(w.song(first).unwrap().core().library_revision(), 1);
    assert_eq!(w.song(second).unwrap().core().library_revision(), 1);
}

#[test]
fn recovered_songs_open_in_windows_of_their_own_without_moving_the_audio() {
    use atomik_model::{SongInfo, empty_doc};
    let dir = std::env::temp_dir().join(format!("atomik-workspace-recover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = empty_doc();
    doc.info = SongInfo {
        title: "Lost song".into(),
        ..SongInfo::default()
    };
    let good = dir.join("lost-00000000000000000001.recover.atomik");
    std::fs::write(&good, atomik_model::save_project(&doc)).unwrap();
    let broken = dir.join("broken-00000000000000000001.recover.atomik");
    std::fs::write(&broken, "not a project").unwrap();

    let (mut w, backend) = workspace();
    let first = w.routed();
    let opened = w.recover_songs(&[broken, good]);
    assert_eq!(opened, 1, "the unreadable copy is skipped");
    assert_eq!(w.len(), 2, "and leaves no window");
    assert_eq!(w.routed(), first);
    assert_eq!(w.play_focus(), first, "the audio stays where it was");
    assert_eq!(backend.open.load(Ordering::SeqCst), 1);
    let second = w.ids()[1];
    let song = w.song(second).unwrap();
    assert_eq!(song.core().doc().info.title, "Lost song");
    assert!(song.core().is_dirty(), "a recovered song is not saved yet");
    assert!(song.core().file().is_none());
    let message = w.core().last_rejection().unwrap();
    assert!(message.contains("came back"), "{message}");
    // Nothing to recover: nothing happens.
    assert_eq!(w.recover_songs(&[]), 0);
    let _ = std::fs::remove_dir_all(dir);
}

/// A song saved to a project file in `dir` whose undo history still holds an audio asset
/// that no clip uses (the saved file does not have it, the live pool does), then changed.
fn saved_song_with_history_asset(
    w: &mut Workspace<Counting>,
    dir: &std::path::Path,
    file: PathBuf,
    name: &str,
) -> PathBuf {
    use atomik_ui::ImportTarget;
    let path = dir.join(name);
    w.core_mut()
        .import_audio(vec![file], ImportTarget::NewTracks, 0);
    w.core_mut().wait_background();
    w.core_mut().save_as(&path).unwrap();
    let clip = w.core().doc().song.tracks[0].lanes[0].clips[0].id;
    w.core_mut()
        .apply_command(Command::Song(atomik_model::SongCommand::RemoveClip {
            id: clip,
        }))
        .unwrap();
    assert!(w.core().is_dirty());
    path
}

#[test]
fn quit_with_save_writes_the_file_and_quits_without_asking_again() {
    let (mut w, _backend) = workspace();
    let path = temp("quit-save.atomik");
    add_synth(&mut w, "lead");
    w.core_mut().save_as(&path).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    add_synth(&mut w, "bass");
    let quit = PendingAction::Command(AppCommand::Quit);
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.save_then(quit), vec![ShellRequest::Quit]);
    assert_ne!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(!w.core().is_dirty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_song_is_clean_right_after_a_save_even_when_the_undo_history_holds_an_asset() {
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("clean-after-save");
    saved_song_with_history_asset(&mut w, &dir, file, "proj.atomik");
    w.core_mut().save().unwrap();
    assert!(!w.core().is_dirty(), "the saved state is the live state");
    // Undo brings the clip back (a change), redo returns to the saved state (clean again).
    assert!(w.core_mut().undo());
    assert!(w.core().is_dirty());
    assert!(w.core_mut().redo());
    assert!(!w.core().is_dirty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn quit_with_save_does_not_loop_when_the_undo_history_holds_an_asset() {
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("quit-loop");
    saved_song_with_history_asset(&mut w, &dir, file, "proj.atomik");
    let quit = PendingAction::Command(AppCommand::Quit);
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.save_then(quit), vec![ShellRequest::Quit]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn quit_goes_on_after_a_save_that_worked_even_if_the_song_is_dirty_again() {
    let (mut w, _backend) = workspace();
    let path = temp("quit-dirty-again.atomik");
    add_synth(&mut w, "lead");
    w.core_mut().save_as(&path).unwrap();
    add_synth(&mut w, "bass");
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    // Whatever marks the song changed after the save, the answer was given and the save worked.
    let after = w.save_then(quit);
    assert_eq!(after, vec![ShellRequest::Quit]);
    add_synth(&mut w, "pad");
    assert!(w.core().is_dirty());
    assert_eq!(w.execute(AppCommand::Quit).len(), 1, "a new quit asks anew");
    let _ = std::fs::remove_file(path);
}

#[test]
fn quit_with_a_failing_save_stays_open_and_asks_nothing_more() {
    let (mut w, _backend) = workspace();
    let path = temp("quit-failing.atomik");
    add_synth(&mut w, "lead");
    w.core_mut().save_as(&path).unwrap();
    add_synth(&mut w, "bass");
    // The project file becomes a folder: the write cannot replace it.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    assert_eq!(
        w.save_then(quit.clone()),
        Vec::new(),
        "no quit, no new question"
    );
    assert!(w.core().is_dirty(), "nothing is lost");
    assert_eq!(w.ids().len(), 1);
    // A later quit asks again.
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit }]
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn quit_with_a_cancelled_save_as_dialog_stays_open() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    // A song with no file yet asks for one; the dialog is cancelled, nothing else happens.
    assert_eq!(
        w.save_then(quit.clone()),
        vec![ShellRequest::PickSaveFileThen { then: quit.clone() }]
    );
    assert!(w.core().is_dirty());
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit }]
    );
}

#[test]
fn quit_with_cancel_leaves_the_song_and_asks_again_next_time() {
    let (mut w, _backend) = workspace();
    add_synth(&mut w, "lead");
    let quit = PendingAction::Command(AppCommand::Quit);
    w.execute(AppCommand::Quit);
    // Cancel: the platform layer does nothing at all.
    assert!(w.core().is_dirty());
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit }]
    );
}

#[test]
fn quit_with_save_asks_each_song_once() {
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("quit-two");
    let (dir2, file2) = ramp_file("quit-two-b");
    saved_song_with_history_asset(&mut w, &dir, file, "one.atomik");
    let second = w.new_song();
    saved_song_with_history_asset(&mut w, &dir2, file2, "two.atomik");
    w.focus(DocId::FIRST);
    let quit = PendingAction::Command(AppCommand::Quit);
    assert_eq!(
        w.execute(AppCommand::Quit),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.routed(), DocId::FIRST);
    // Save for the first song: the second one is asked next, once.
    assert_eq!(
        w.save_then(quit.clone()),
        vec![ShellRequest::ConfirmSave { then: quit.clone() }]
    );
    assert_eq!(w.routed(), second);
    assert_eq!(w.save_then(quit), vec![ShellRequest::Quit]);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

#[test]
fn closing_the_last_window_with_save_quits_without_asking_again() {
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("close-last");
    saved_song_with_history_asset(&mut w, &dir, file, "proj.atomik");
    let close = PendingAction::Command(AppCommand::CloseSongWindow);
    assert_eq!(
        w.execute(AppCommand::CloseSongWindow),
        vec![ShellRequest::ConfirmSave {
            then: close.clone()
        }]
    );
    assert_eq!(w.save_then(close), vec![ShellRequest::Quit]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn closing_one_of_two_windows_with_save_closes_only_that_song() {
    let (mut w, _backend) = workspace();
    let (dir, file) = ramp_file("close-one");
    add_synth(&mut w, "lead");
    let second = w.new_song();
    saved_song_with_history_asset(&mut w, &dir, file, "proj.atomik");
    let close = PendingAction::Command(AppCommand::CloseSongWindow);
    w.execute(AppCommand::CloseSongWindow);
    assert_eq!(w.save_then(close), Vec::new());
    assert!(w.song(second).is_none());
    assert_eq!(w.ids(), vec![DocId::FIRST]);
    let _ = std::fs::remove_dir_all(dir);
}
