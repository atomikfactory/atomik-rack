//! The Play Focus and the Input Focus of the open songs: which song owns the stream, which one
//! holds the MIDI inputs, and what the setting "Focus follows the active song" moves. The
//! backend is the shared fake audio host with a MIDI port named `Keys` that counts what is open.

use super::*;
use crate::app_core::AppCore;
use crate::focus_follows::FocusFollows;
use crate::midi::MidiLink;
use crate::session::{AudioBackend, LiveSession, MidiPort, Starter};
use crate::settings::{AudioSettings, MidiInputSetting};
use crate::shell::ShellOptions;
use crate::test_backend::{FakeAudio, FakeStream};
use atomik_audio_io::HostInfo;
use atomik_audio_io::IoError;
use atomik_midi_io::MidiError;
use std::sync::{Arc, Mutex};

/// The shared fake audio host, and the names of the MIDI ports that are open now.
#[derive(Clone, Default)]
struct Keys {
    audio: FakeAudio,
    open: Arc<Mutex<Vec<String>>>,
}

impl Keys {
    fn open_ports(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

struct Port {
    name: String,
    open: Arc<Mutex<Vec<String>>>,
}

impl MidiPort for Port {
    fn port_name(&self) -> &str {
        &self.name
    }
    fn dropped(&self) -> u64 {
        0
    }
    fn close(self) {
        let mut open = self.open.lock().unwrap();
        if let Some(at) = open.iter().position(|n| *n == self.name) {
            open.remove(at);
        }
    }
}

impl AudioBackend for Keys {
    type Stream = FakeStream;
    type Midi = Port;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        self.audio.list_audio()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.audio.list_midi())
    }

    fn starter(&self) -> Starter<FakeStream> {
        let audio = self.audio.clone();
        Box::new(move |spec, input_device, engine, _clock| audio.start(spec, input_device, engine))
    }

    fn open_midi(&self, name: &str, _link: MidiLink) -> Result<Port, MidiError> {
        let visible = self.audio.visible_midi();
        let at = crate::midi::match_port(&visible, name)?;
        let port = visible[at].clone();
        self.open.lock().unwrap().push(port.clone());
        Ok(Port {
            name: port,
            open: Arc::clone(&self.open),
        })
    }
}

fn options() -> ShellOptions {
    ShellOptions {
        os: Some(atomik_ui::keymap::Os::Windows),
        profile: None,
        in_window_menu: true,
        settings_path: None,
        ui_state_path: None,
        library_path: None,
        library_root: None,
    }
}

/// Two songs on one host with one keyboard; `Keys` is in the settings of both. The second song
/// is open and, with the default setting, has both foci.
fn two_songs() -> (Workspace<Keys>, Keys, DocId) {
    let host = Keys::default();
    host.audio.set_midi_ports(&["Keys"]);
    let settings = AudioSettings {
        midi_inputs: vec![MidiInputSetting::enabled("Keys")],
        ..AudioSettings::default()
    };
    let first = Shell::new(
        AppCore::new(LiveSession::new(host.clone(), settings, true)),
        options(),
    );
    let again = host.clone();
    let make: SongFactory<Keys> = Box::new(move |from, _id| {
        let mut audio = from.audio_settings();
        audio.surfaces.clear();
        Shell::new(
            AppCore::new(LiveSession::new(again.clone(), audio, false)),
            options(),
        )
    });
    let mut w = Workspace::new(first, make);
    let second = w.new_song();
    (w, host, second)
}

/// The songs that hold the keyboard (its port is open in their session).
fn holders(w: &Workspace<Keys>) -> Vec<DocId> {
    w.ids()
        .into_iter()
        .filter(|id| {
            w.song(*id)
                .is_some_and(|s| s.core().sink().status().midi_port.is_some())
        })
        .collect()
}

#[test]
fn both_foci_follow_the_active_song_by_default_and_the_keyboard_goes_with_them() {
    let (mut w, host, second) = two_songs();
    assert_eq!((w.play_focus(), w.input_focus()), (second, second));
    assert_eq!(holders(&w), vec![second]);
    assert_eq!(host.open_ports(), 1);
    for id in [DocId::FIRST, second, DocId::FIRST] {
        assert!(w.focus(id));
        assert_eq!((w.play_focus(), w.input_focus()), (id, id));
        assert_eq!(
            holders(&w),
            vec![id],
            "the keyboard is open in one song only"
        );
        assert_eq!(host.open_ports(), 1);
        assert_eq!(host.audio.open_streams(), 1);
    }
}

#[test]
fn play_focus_moves_the_stream_and_leaves_the_keyboard_where_it_is() {
    let (mut w, host, second) = two_songs();
    assert!(w.set_play_focus(DocId::FIRST));
    assert_eq!(w.play_focus(), DocId::FIRST);
    assert_eq!(w.input_focus(), second);
    // One stream, in the song that plays; the other song is silent.
    assert_eq!(host.audio.open_streams(), 1);
    assert!(w.song(DocId::FIRST).unwrap().core().sink().is_running());
    assert!(!w.song(second).unwrap().core().sink().is_running());
    assert!(w.song(second).unwrap().is_silent());
    // The keyboard belongs to the song that has no stream: nothing is open.
    assert_eq!(holders(&w), Vec::<DocId>::new());
    assert_eq!(host.open_ports(), 0);
    assert!(!w.set_play_focus(DocId(99)));
}

#[test]
fn input_focus_moves_the_midi_inputs_and_they_open_only_in_the_song_that_plays() {
    let (mut w, host, second) = two_songs();
    // The keyboard goes to the song that does not play: the ports close, the stream goes on.
    assert!(w.set_input_focus(DocId::FIRST));
    assert_eq!((w.play_focus(), w.input_focus()), (second, DocId::FIRST));
    assert_eq!(host.open_ports(), 0);
    assert_eq!(host.audio.open_streams(), 1);
    assert!(w.song(second).unwrap().core().sink().is_running());
    // The song with the keyboard takes the audio too: the ports open in it as it starts.
    assert!(w.set_play_focus(DocId::FIRST));
    assert_eq!(holders(&w), vec![DocId::FIRST]);
    assert_eq!(host.open_ports(), 1);
    // The keyboard goes back to the other song while this one plays: closed again.
    assert!(w.set_input_focus(second));
    assert_eq!(host.open_ports(), 0);
    assert_eq!(host.audio.open_streams(), 1);
    assert!(!w.set_input_focus(DocId(99)));
}

#[test]
fn a_song_that_loses_the_input_focus_keeps_playing_without_restarting() {
    let (mut w, host, second) = two_songs();
    let starts = host.audio.starts().len();
    assert!(w.set_input_focus(DocId::FIRST));
    assert_eq!(
        host.audio.starts().len(),
        starts,
        "closing the ports does not restart the stream"
    );
    assert!(w.set_input_focus(second));
    assert_eq!(host.audio.starts().len(), starts);
    assert_eq!(holders(&w), vec![second]);
}

#[test]
fn the_setting_names_which_foci_follow_the_song_you_work_in() {
    for (follows, play, input) in [
        (FocusFollows::Both, true, true),
        (FocusFollows::PlayOnly, true, false),
        (FocusFollows::InputOnly, false, true),
        (FocusFollows::Neither, false, false),
    ] {
        let (mut w, host, second) = two_songs();
        for id in w.ids() {
            w.song_mut(id).unwrap().set_focus_follows(follows);
        }
        // Both foci in the first song to start from.
        w.set_input_focus(DocId::FIRST);
        w.set_play_focus(DocId::FIRST);
        assert!(w.focus(second));
        assert_eq!(
            w.routed(),
            second,
            "{follows:?}: the calls follow you whatever it says"
        );
        assert_eq!(
            w.play_focus(),
            if play { second } else { DocId::FIRST },
            "{follows:?}: play"
        );
        assert_eq!(
            w.input_focus(),
            if input { second } else { DocId::FIRST },
            "{follows:?}: input"
        );
        assert_eq!(host.audio.open_streams(), 1, "{follows:?}");
        // The keyboard sounds only where both foci are.
        let expected = if play && input {
            vec![second]
        } else if !play && !input {
            vec![DocId::FIRST]
        } else {
            Vec::new()
        };
        assert_eq!(holders(&w), expected, "{follows:?}: ports");
    }
}

#[test]
fn a_new_song_takes_only_the_foci_the_setting_names() {
    let (mut w, _host, _second) = two_songs();
    for id in w.ids() {
        w.song_mut(id)
            .unwrap()
            .set_focus_follows(FocusFollows::Neither);
    }
    w.set_input_focus(DocId::FIRST);
    w.set_play_focus(DocId::FIRST);
    let third = w.new_song();
    assert_eq!(w.routed(), third);
    assert_eq!(
        (w.play_focus(), w.input_focus()),
        (DocId::FIRST, DocId::FIRST)
    );
    assert!(w.song(third).unwrap().is_silent());
    assert!(!w.song(third).unwrap().has_input_focus());
    assert!(w.song(DocId::FIRST).unwrap().has_input_focus());
}

#[test]
fn closing_the_song_with_a_focus_hands_it_to_its_neighbour() {
    let (mut w, host, second) = two_songs();
    let third = w.new_song();
    // The keyboard is in the second song, the audio in the third.
    w.set_input_focus(second);
    assert_eq!((w.play_focus(), w.input_focus()), (third, second));
    assert!(w.close_song(second));
    assert_eq!(
        w.input_focus(),
        third,
        "the keyboard moved on with the song that closed"
    );
    assert_eq!(w.play_focus(), third);
    assert_eq!(holders(&w), vec![third]);
    assert_eq!(host.open_ports(), 1);
    assert!(w.close_song(third));
    assert_eq!(
        (w.play_focus(), w.input_focus()),
        (DocId::FIRST, DocId::FIRST)
    );
    assert_eq!(holders(&w), vec![DocId::FIRST]);
    assert_eq!(host.audio.open_streams(), 1);
}

#[test]
fn the_buttons_of_the_panel_ask_the_workspace_for_the_foci_of_their_song() {
    let (mut w, host, second) = two_songs();
    w.select(DocId::FIRST);
    let out = w.route(vec![ShellRequest::TakePlayFocus], true);
    assert!(out.is_empty(), "the workspace answers it itself");
    assert_eq!((w.play_focus(), w.input_focus()), (DocId::FIRST, second));
    let out = w.route(vec![ShellRequest::TakeInputFocus], true);
    assert!(out.is_empty());
    assert_eq!(
        (w.play_focus(), w.input_focus()),
        (DocId::FIRST, DocId::FIRST)
    );
    assert_eq!(host.open_ports(), 1);
    assert_eq!(w.routed(), DocId::FIRST);
}

#[test]
fn every_panel_shows_the_foci_its_song_has() {
    let (mut w, _host, second) = two_songs();
    let shown = |w: &Workspace<Keys>, id: DocId| {
        let focus = w.song(id).unwrap().core().view().io_focus();
        (focus.play, focus.input)
    };
    assert_eq!(shown(&w, second), (true, true));
    assert_eq!(shown(&w, DocId::FIRST), (false, false));
    w.set_input_focus(DocId::FIRST);
    assert_eq!(shown(&w, second), (true, false));
    assert_eq!(shown(&w, DocId::FIRST), (false, true));
    w.set_play_focus(DocId::FIRST);
    assert_eq!(shown(&w, second), (false, false));
    assert_eq!(shown(&w, DocId::FIRST), (true, true));
}

#[test]
fn the_neighbour_is_counted_from_the_song_you_work_in() {
    let (mut w, _host, second) = two_songs();
    let third = w.new_song();
    for id in w.ids() {
        w.song_mut(id)
            .unwrap()
            .set_focus_follows(FocusFollows::Neither);
    }
    // The foci stay in the third song while the work moves to the first.
    w.focus(DocId::FIRST);
    assert_eq!((w.routed(), w.play_focus()), (DocId::FIRST, third));
    assert_eq!(w.neighbour(true), second);
    assert_eq!(w.neighbour(false), third);
}
