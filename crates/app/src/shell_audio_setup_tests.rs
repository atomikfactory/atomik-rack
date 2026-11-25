//! The first-run audio and MIDI setup in the shell, on a fake audio host that renders the engine
//! when a test says the device calls back, and fake MIDI ports a test plays notes into.

use super::*;
use crate::audio_setup::{LAMP_FRAMES, Page};
use crate::audio_setup_view::Kind;
use crate::midi::{MidiLink, PortInput};
use crate::session::{AudioBackend, MidiPort};
use crate::settings::HostChoice;
use crate::settings_dialog::Choice;
use crate::sink::EngineSink;
use crate::test_backend::{AcceptedBuffer, FakeAudio, FakeStream};
use atomik_audio_io::{DeviceInfo, HostInfo, IoError, StreamSpec, Telemetry};
use atomik_engine::{Engine, FrameClock};
use atomik_midi_io::MidiError;
use atomik_ui::{Key, Modifiers, PointerButton};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

const BLOCK: usize = 256;

/// What the fake devices do, shared with the test: the shared audio backend and the MIDI inputs.
struct World {
    audio: FakeAudio,
    /// The inputs open on the MIDI ports.
    inputs: RefCell<Vec<(String, Rc<RefCell<PortInput>>)>>,
    /// The level the fake input device delivers.
    input_level: Cell<f32>,
}

impl Default for World {
    fn default() -> Self {
        let audio = FakeAudio::new();
        audio.set_host(HostChoice::platform_default().kind());
        audio.no_host_without_devices();
        audio.set_default_input("Mic");
        audio.accept_buffer(AcceptedBuffer::AsRequested);
        // The device calls back once, at 480 frames; the test renders the engine itself.
        audio.set_telemetry(Telemetry {
            callbacks: 1,
            buffer_frames: 480,
            ..Telemetry::default()
        });
        Self {
            audio,
            inputs: RefCell::default(),
            input_level: Cell::default(),
        }
    }
}

impl World {
    /// The device calls back `blocks` times: the engine renders and the peaks are published.
    fn render(&self, blocks: usize) {
        let input = vec![self.input_level.get(); BLOCK];
        self.audio
            .with_engine(|engine| {
                for _ in 0..blocks {
                    let mut left = vec![0.0; BLOCK];
                    let mut right = vec![0.0; BLOCK];
                    engine.render(&[&input, &input], &mut [&mut left, &mut right]);
                }
            })
            .expect("a stream is running");
    }

    /// Plays `bytes` into the open MIDI port `port`.
    fn send(&self, port: &str, bytes: &[u8]) {
        let input = self
            .inputs
            .borrow()
            .iter()
            .find(|(name, _)| name == port)
            .map(|(_, input)| Rc::clone(input))
            .unwrap_or_else(|| panic!("port {port} is not open"));
        input.borrow_mut().on_message(bytes);
    }
}

#[derive(Clone, Default)]
struct Fake(Rc<World>);

struct FakeMidi {
    name: String,
    world: Rc<World>,
}

impl MidiPort for FakeMidi {
    fn port_name(&self) -> &str {
        &self.name
    }
    fn dropped(&self) -> u64 {
        0
    }
    fn close(self) {
        self.world
            .inputs
            .borrow_mut()
            .retain(|(n, _)| *n != self.name);
    }
}

impl AudioBackend for Fake {
    type Stream = FakeStream;
    type Midi = FakeMidi;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        self.0.audio.list_audio()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.0.audio.list_midi())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<FakeStream, IoError> {
        self.0.audio.start(spec, input_device, engine)
    }

    fn open_midi(&self, name: &str, link: MidiLink) -> Result<FakeMidi, MidiError> {
        let ports = self.0.audio.visible_midi();
        let at = crate::midi::match_port(&ports, name)?;
        let port = ports[at].clone();
        self.0.inputs.borrow_mut().push((
            port.clone(),
            Rc::new(RefCell::new(PortInput::new(&port, link))),
        ));
        Ok(FakeMidi {
            name: port,
            world: Rc::clone(&self.0),
        })
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-audio-setup-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn world() -> Fake {
    let world = Fake::default();
    for name in ["Speakers", "Headset"] {
        world.0.audio.plug_device(DeviceInfo {
            name: name.into(),
            max_inputs: 2,
            max_outputs: 2,
            sample_rates: vec![48_000],
            buffer_range: None,
            is_default_output: false,
        });
    }
    world.0.audio.set_midi_ports(&["Keys"]);
    world
}

/// A shell on the fake devices whose settings file is `dir/settings.json` (not written yet unless
/// the test did).
fn shell(world: &Fake, dir: &Path) -> Shell<Fake> {
    let session = LiveSession::new(world.clone(), AudioSettings::default(), true);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(dir.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn click(s: &mut Shell<Fake>, kind: &Kind) {
    let size = s.size();
    let at = s
        .audio_setup()
        .expect("the setup is open")
        .center_of(size, kind)
        .unwrap_or_else(|| panic!("{kind:?} is on the page"));
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
}

fn press(s: &mut Shell<Fake>, key: Key) {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

fn settings_file(dir: &Path) -> Option<AudioSettings> {
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    Some(AudioSettings::from_json(&text).unwrap())
}

/// Advances the shell one frame the way the window loop does.
fn frame(s: &mut Shell<Fake>) {
    s.poll_status();
    s.frame();
}

#[test]
fn the_setup_shows_at_the_first_start_once_and_never_again() {
    let dir = scratch("once");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(settings_file(&dir).is_none(), "nothing was ever saved here");
    assert!(s.first_run_setup(), "the first start opens the setup");
    assert!(s.audio_setup().is_some());
    assert!(s.modal_open());
    click(&mut s, &Kind::Skip);
    assert!(s.audio_setup().is_none());
    assert!(
        settings_file(&dir).is_some(),
        "Skip marks the setup as done"
    );

    // The next start finds the file and stays quiet.
    let mut again = shell(&world(), &dir);
    assert!(!again.first_run_setup());
    assert!(again.audio_setup().is_none());
    // Asking twice at one start does not open it twice either.
    assert!(!s.first_run_setup());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn done_also_marks_the_setup_as_done() {
    let dir = scratch("done");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Done);
    assert!(s.audio_setup().is_none());
    assert!(settings_file(&dir).is_some());
    assert!(!shell(&world(), &dir).first_run_setup());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shell_without_a_settings_file_path_never_opens_it_by_itself() {
    let w = world();
    let session = LiveSession::new(w.clone(), AudioSettings::default(), true);
    let mut s = Shell::new(
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
    assert!(!s.first_run_setup());
    assert!(s.audio_setup().is_none());
}

#[test]
fn the_command_opens_it_whenever_asked_and_stays_one_panel() {
    let dir = scratch("command");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.audio_setup().is_none());
    s.execute(AppCommand::AudioSetup);
    assert_eq!(s.audio_setup().unwrap().page(), Page::Output);
    s.execute(AppCommand::AudioSetup);
    assert_eq!(s.audio_setup().unwrap().page(), Page::Output);
    click(&mut s, &Kind::Next);
    s.execute(AppCommand::AudioSetup);
    assert_eq!(
        s.audio_setup().unwrap().page(),
        Page::Input,
        "asking again leaves the open panel alone"
    );
    press(&mut s, Key::Escape);
    assert!(s.audio_setup().is_none());
    s.execute(AppCommand::AudioSetup);
    assert_eq!(
        s.audio_setup().unwrap().page(),
        Page::Output,
        "a fresh panel"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_menu_has_the_entry_in_options() {
    let dir = scratch("menu");
    let s = shell(&world(), &dir);
    let menu = s.menu_model();
    let options = menu
        .iter()
        .find(|m| m.title == "Options")
        .expect("an Options menu");
    let found = options.items().any(|i| {
        i.command == AppCommand::AudioSetup && i.label.starts_with("Audio and MIDI Setup")
    });
    assert!(found, "Options > Audio and MIDI Setup...");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_is_chosen_is_saved_in_the_audio_settings_file() {
    let dir = scratch("saved");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    click(&mut s, &Kind::Pick(Choice::Device(Some("Headset".into()))));
    click(&mut s, &Kind::Pick(Choice::Buffer(512)));
    assert!(
        settings_file(&dir).is_none(),
        "nothing is written until Skip or Done"
    );
    assert_eq!(
        s.core().sink().settings().output_device.as_deref(),
        Some("Headset"),
        "but the choice is live at once"
    );
    assert_eq!(s.core().sink().settings().buffer_frames, 512);
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Pick(Choice::MidiInput("Keys".into())));
    click(&mut s, &Kind::Done);

    let file = settings_file(&dir).expect("the file");
    assert_eq!(file.output_device.as_deref(), Some("Headset"));
    assert_eq!(file.buffer_frames, 512);
    let keys: Vec<&str> = file
        .enabled_midi_inputs()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(keys, ["Keys"]);
    // The very file Audio Settings reads: its dialog shows the same values.
    assert_eq!(s.audio_settings().buffer_frames, file.buffer_frames);
    assert_eq!(s.audio_settings().output_device, file.output_device);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_test_button_plays_the_tone_and_the_output_meter_moves() {
    let dir = scratch("tone");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    w.0.render(4);
    frame(&mut s);
    assert_eq!(s.audio_setup().unwrap().out_meter.level(), 0.0);
    assert!(!s.core().sink().preview_active());

    click(&mut s, &Kind::Test);
    w.0.render(8);
    frame(&mut s);
    assert!(
        s.core().sink().preview_active(),
        "the tone plays through the preview voice of the engine"
    );
    let level = s.audio_setup().unwrap().out_meter.level();
    assert!(
        level > 0.02 && level <= 0.1259,
        "the tone peaks at -18 dBFS, not above: {level}"
    );
    assert!(s.audio_setup().unwrap().out_meter.fraction() > 0.0);
    // The meter falls back once the tone is over (one second of audio is 188 blocks).
    w.0.render(200);
    frame(&mut s);
    for _ in 0..300 {
        frame(&mut s);
    }
    assert_eq!(s.audio_setup().unwrap().out_meter.level(), 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_no_audio_device_the_panel_says_so_and_skip_works() {
    let dir = scratch("nodevice");
    let w = Fake::default();
    w.0.audio
        .fail_starts(IoError::DeviceNotFound("the device is busy".into()));
    let mut s = shell(&w, &dir);
    assert!(!s.core().sink().is_running());
    assert!(s.first_run_setup());
    frame(&mut s);
    let panel = s.audio_setup().unwrap();
    let text = panel.lines(s.size()).join("\n");
    assert!(text.contains("No sound output was found"), "{text}");
    // The Test button does nothing without an engine, and does not fail.
    click(&mut s, &Kind::Test);
    click(&mut s, &Kind::Skip);
    assert!(s.audio_setup().is_none());
    assert!(settings_file(&dir).is_some(), "Skip still marks it done");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_device_that_does_not_open_gives_a_plain_message_and_keeps_the_old_devices() {
    let dir = scratch("busy");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    w.0.audio
        .fail_starts(IoError::DeviceNotFound("the device is busy".into()));
    click(&mut s, &Kind::Pick(Choice::Device(Some("Headset".into()))));
    frame(&mut s);
    let text = s.audio_setup().unwrap().lines(s.size()).join("\n");
    assert!(
        text.contains("did not open") && text.contains("Pick another device"),
        "{text}"
    );
    // Going on and finishing writes the devices that opened before, not the broken one.
    click(&mut s, &Kind::Skip);
    let file = settings_file(&dir).expect("the file");
    assert_ne!(file.output_device.as_deref(), Some("Headset"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_note_from_the_keyboard_lights_the_lamp_and_plays_a_tone() {
    let dir = scratch("midi");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Pick(Choice::MidiInput("Keys".into())));
    assert_eq!(w.0.inputs.borrow().len(), 1, "the port is open");
    frame(&mut s);
    assert!(!s.audio_setup().unwrap().lamp_lit());

    w.0.send("Keys", &[0x90, 60, 100]);
    frame(&mut s);
    assert!(s.audio_setup().unwrap().lamp_lit(), "the lamp is lit");
    let size = s.size();
    assert!(
        s.audio_setup()
            .unwrap()
            .lines(size)
            .contains(&"Note received: C3".to_owned())
    );
    w.0.render(4);
    assert!(
        s.core().sink().preview_active(),
        "the key plays its tone through the engine"
    );
    for _ in 0..LAMP_FRAMES + 1 {
        frame(&mut s);
    }
    assert!(!s.audio_setup().unwrap().lamp_lit());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_input_page_turns_the_input_on_and_its_meter_moves() {
    let dir = scratch("input");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    click(&mut s, &Kind::Next);
    w.0.input_level.set(0.25);
    w.0.render(8);
    frame(&mut s);
    let panel = s.audio_setup().unwrap();
    assert!(!panel.input_unwanted, "the starter project has inputs");
    assert!(panel.input_open, "two input channels are open");
    assert!(panel.in_meter.level() > 0.2, "{}", panel.in_meter.level());

    // A project with no input channels gets the button that turns them on.
    s.execute(AppCommand::InputChannels(0));
    frame(&mut s);
    assert!(s.audio_setup().unwrap().input_unwanted);
    assert!(!s.audio_setup().unwrap().input_open);
    click(&mut s, &Kind::TurnOnInput);
    w.0.render(8);
    frame(&mut s);
    let panel = s.audio_setup().unwrap();
    assert!(panel.input_open && !panel.input_unwanted);
    assert!(panel.in_meter.level() > 0.2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_up_controller_opens_the_wizard_over_the_setup_and_the_setup_comes_back() {
    let dir = scratch("wizard");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::Next);
    click(&mut s, &Kind::SetUpController);
    assert!(s.controller_wizard().is_some());
    assert!(s.audio_setup().is_some(), "the setup waits behind it");
    press(&mut s, Key::Escape);
    assert!(s.controller_wizard().is_none());
    assert_eq!(s.audio_setup().unwrap().page(), Page::Midi);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_update_question_waits_for_the_setup_and_the_start_screen_comes_after_it() {
    let dir = scratch("order");
    let w = world();
    let mut s = shell(&w, &dir);
    assert!(s.first_run_setup());
    s.start_without_project();
    s.start_update_check();
    assert!(s.update_notice().is_some(), "the question is waiting");
    assert!(
        !s.update_notice_reachable(),
        "but it does not show over the setup"
    );
    assert!(s.start_screen.is_none(), "the start screen waits too");
    // A click that would answer the question does nothing while the setup is open.
    click(&mut s, &Kind::Skip);
    assert!(s.audio_setup().is_none());
    assert!(
        s.start_screen.is_some(),
        "the start screen opens when the setup closes"
    );
    assert!(s.update_notice_reachable(), "and then the question shows");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_later_start_shows_the_start_screen_and_the_question_without_the_setup() {
    let dir = scratch("later");
    let w = world();
    let mut first = shell(&w, &dir);
    assert!(first.first_run_setup());
    click(&mut first, &Kind::Skip);
    let mut s = shell(&world(), &dir);
    assert!(!s.first_run_setup());
    s.start_without_project();
    s.start_update_check();
    assert!(s.start_screen.is_some());
    assert!(s.update_notice_reachable());
    let _ = std::fs::remove_dir_all(&dir);
}
