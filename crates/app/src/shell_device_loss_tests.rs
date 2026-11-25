//! `Shell` tests for a stream that stops or whose device is lost, and for applying audio
//! settings that do not open (a fake backend: no audio hardware).

use super::*;
use crate::midi::{MidiConnection, MidiLink};
use crate::session::{AudioBackend, LiveStream};
use atomik_audio_io::{HostInfo, IoError, StreamInfo, StreamSpec, Telemetry};
use atomik_engine::{Engine, FrameClock};
use atomik_midi_io::MidiError;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// What the fake devices do, shared with the test.
#[derive(Default)]
struct World {
    telemetry: Cell<Telemetry>,
    /// Every start naming this output device fails.
    broken: RefCell<Option<String>>,
    /// The output device of every start.
    starts: RefCell<Vec<Option<String>>>,
    /// The MIDI input ports that are plugged in.
    midi: RefCell<Vec<String>>,
    /// The output devices the host lists, and the host that lists them.
    plugged: RefCell<Vec<String>>,
    host: Cell<Option<atomik_audio_io::HostKind>>,
    /// How many times the audio hosts and devices were listed.
    audio_listings: Cell<u32>,
}

#[derive(Clone, Default)]
struct Fake(Rc<World>);

struct FakeStream {
    info: StreamInfo,
    engine: Engine,
    world: Rc<World>,
}

impl LiveStream for FakeStream {
    fn telemetry(&self) -> Telemetry {
        self.world.telemetry.get()
    }
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn stop(self) -> Result<Engine, IoError> {
        Ok(self.engine)
    }
}

impl AudioBackend for Fake {
    type Stream = FakeStream;
    type Midi = MidiConnection;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        self.0.audio_listings.set(self.0.audio_listings.get() + 1);
        let devices = self
            .0
            .plugged
            .borrow()
            .iter()
            .map(|name| atomik_audio_io::DeviceInfo {
                name: name.clone(),
                max_inputs: 0,
                max_outputs: 2,
                sample_rates: vec![48_000],
                buffer_range: None,
                is_default_output: false,
            })
            .collect();
        Ok(vec![HostInfo {
            kind: self
                .0
                .host
                .get()
                .unwrap_or(atomik_audio_io::HostKind::Default),
            devices,
        }])
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.0.midi.borrow().clone())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        _input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<FakeStream, IoError> {
        self.0.starts.borrow_mut().push(spec.device.clone());
        if self.0.broken.borrow().is_some() && *self.0.broken.borrow() == spec.device {
            return Err(IoError::DeviceNotFound(
                spec.device.clone().unwrap_or_default(),
            ));
        }
        Ok(FakeStream {
            info: StreamInfo {
                host: spec.host,
                output_device: spec.device.clone().unwrap_or_else(|| "Speakers".into()),
                input_device: None,
                sample_rate: spec.sample_rate,
                output_channels: spec.output_channels,
                input_channels: 0,
                sample_format: "f32",
                requested_buffer_frames: Some(spec.buffer_frames),
            },
            engine,
            world: Rc::clone(&self.0),
        })
    }

    fn open_midi(&self, name: &str, _link: MidiLink) -> Result<MidiConnection, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-device-loss-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn saved(device: &str, reset_to_default: bool) -> AudioSettings {
    AudioSettings {
        output_device: Some(device.into()),
        workers: 0,
        device_loss: crate::settings::DeviceLossSettings {
            reset_to_default,
            last_good: None,
        },
        ..AudioSettings::default()
    }
}

/// A shell whose settings file holds `settings` and whose audio runs on the fake devices.
fn shell(world: &Fake, settings: &AudioSettings, dir: &Path) -> Shell<Fake> {
    let path = dir.join("settings.json");
    settings::save(&path, settings).unwrap();
    let session = LiveSession::new(world.clone(), settings.clone(), true);
    Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(path),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    )
}

fn running_telemetry(callbacks: u64) -> Telemetry {
    Telemetry {
        callbacks,
        buffer_frames: 480,
        ..Telemetry::default()
    }
}

fn file_text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("settings.json")).unwrap()
}

#[test]
fn failed_apply_keeps_last_good_settings() {
    let dir = scratch("failed-apply");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    assert!(s.core().sink().is_running());
    let before = file_text(&dir);
    *world.0.broken.borrow_mut() = Some("Unplugged".into());

    let mut wanted = s.core().sink().settings().clone();
    wanted.output_device = Some("Unplugged".into());
    s.apply_settings(wanted);

    assert_eq!(
        file_text(&dir),
        before,
        "the settings file is unchanged after the failing restart"
    );
    let session = s.core().sink();
    assert!(
        session.is_running(),
        "the last working device is open again"
    );
    assert_eq!(
        session.settings().output_device.as_deref(),
        Some("Interface")
    );
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Unplugged") && message.contains("last working"),
        "{message}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_apply_still_saves_the_other_changes_with_the_old_devices() {
    let dir = scratch("failed-apply-other");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    *world.0.broken.borrow_mut() = Some("Unplugged".into());
    let mut wanted = s.core().sink().settings().clone();
    wanted.output_device = Some("Unplugged".into());
    wanted.pre_count_bars = 2;
    s.apply_settings(wanted);
    let file = AudioSettings::from_json(&file_text(&dir)).unwrap();
    assert_eq!(file.pre_count_bars, 2);
    assert_eq!(file.output_device.as_deref(), Some("Interface"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_working_configuration_a_failed_apply_says_so_and_saves_no_devices() {
    let dir = scratch("failed-first");
    let world = Fake::default();
    // The saved device is gone at start-up: audio is off and nothing is known to work.
    *world.0.broken.borrow_mut() = Some("Interface".into());
    let mut s = shell(&world, &saved("Interface", false), &dir);
    assert!(!s.core().sink().is_running());
    let before = file_text(&dir);
    *world.0.broken.borrow_mut() = Some("Unplugged".into());
    let mut wanted = s.core().sink().settings().clone();
    wanted.output_device = Some("Unplugged".into());
    s.apply_settings(wanted);
    assert_eq!(file_text(&dir), before);
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("Audio settings not applied"), "{message}");
    // A device that opens is saved, with the configuration that worked.
    let mut good = s.core().sink().settings().clone();
    good.output_device = Some("Other".into());
    s.apply_settings(good);
    let file = AudioSettings::from_json(&file_text(&dir)).unwrap();
    assert_eq!(file.output_device.as_deref(), Some("Other"));
    assert_eq!(
        file.device_loss.last_good.unwrap().output_device.as_deref(),
        Some("Other")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_configuration_that_fails_at_start_up_falls_back_with_a_message() {
    let dir = scratch("startup");
    let world = Fake::default();
    let mut remembered = saved("Unplugged", false);
    remembered.device_loss.last_good = Some(crate::settings::LastGood {
        host: remembered.host,
        output_device: Some("Interface".into()),
        input_device: None,
        buffer_frames: remembered.buffer_frames,
    });
    *world.0.broken.borrow_mut() = Some("Unplugged".into());
    let mut s = shell(&world, &remembered, &dir);
    assert!(s.core().sink().is_running());
    s.poll_status();
    assert_eq!(s.audio_status.device, "Interface");
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("last working configuration"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stalled_stream_is_announced_once_and_clears_when_it_runs_again() {
    let dir = scratch("stall");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    let t0 = crate::session::clock_now();
    world.0.telemetry.set(running_telemetry(10));
    s.poll_status_at(t0);
    assert!(
        s.core()
            .last_rejection()
            .is_none_or(|m| !m.contains("stopped"))
    );

    // The callbacks stop.
    s.poll_status_at(t0 + Duration::from_millis(400));
    let message = s.core().last_rejection().unwrap().to_string();
    assert!(message.contains("Audio stopped: Interface"), "{message}");
    assert!(message.contains("Restart Audio"), "{message}");
    assert!(
        s.status_text().contains("AUDIO STOPPED"),
        "{}",
        s.status_text()
    );
    // Not repeated at every poll.
    s.notify("something else");
    s.poll_status_at(t0 + Duration::from_millis(650));
    assert_eq!(s.core().last_rejection(), Some("something else"));

    // The callbacks come back.
    world.0.telemetry.set(running_telemetry(12));
    s.poll_status_at(t0 + Duration::from_millis(700));
    assert!(!s.status_text().contains("AUDIO STOPPED"));
    assert!(!s.stream_announced);
    // And a second stall is announced again.
    s.poll_status_at(t0 + Duration::from_millis(1100));
    assert!(s.core().last_rejection().unwrap().contains("Audio stopped"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lost_device_is_offered_a_restart_unless_the_setting_resets_it() {
    let dir = scratch("lost-offer");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    world.0.telemetry.set(Telemetry {
        device_lost: true,
        ..running_telemetry(10)
    });
    s.poll_status_at(crate::session::clock_now());
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Audio device lost: Interface"),
        "{message}"
    );
    assert!(message.contains("Restart Audio"), "{message}");
    assert_eq!(world.0.starts.borrow().len(), 1, "no restart on its own");
    assert!(s.status_text().contains("AUDIO DEVICE LOST"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_the_setting_a_lost_device_switches_to_the_default_devices_without_saving() {
    let dir = scratch("lost-reset");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", true), &dir);
    let before = file_text(&dir);
    world.0.telemetry.set(Telemetry {
        device_lost: true,
        ..running_telemetry(10)
    });
    s.poll_status_at(crate::session::clock_now());
    assert_eq!(
        *world.0.starts.borrow(),
        vec![Some("Interface".to_string()), None],
        "restarted on the default output"
    );
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Switched to the default devices"),
        "{message}"
    );
    assert_eq!(file_text(&dir), before, "the chosen device stays saved");
    assert!(s.core().sink().is_running());

    // The new stream runs: another loss may be reset again, a dead one is not retried at once.
    world.0.telemetry.set(running_telemetry(5));
    s.poll_status_at(crate::session::clock_now());
    assert!(!s.stream_reset);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Polls the status `secs` seconds after `t0` with a stream that keeps calling back.
fn poll_alive(s: &mut Shell<Fake>, world: &Fake, t0: std::time::Instant, secs: u64) {
    world.0.telemetry.set(running_telemetry(100 + secs));
    s.poll_status_at(t0 + Duration::from_secs(secs));
}

#[test]
fn the_chosen_device_comes_back_by_itself_when_it_returns_after_a_loss() {
    let dir = scratch("lost-return");
    let world = Fake::default();
    let settings = saved("Interface", true);
    world.0.host.set(Some(settings.host.kind()));
    let mut s = shell(&world, &settings, &dir);
    world.0.telemetry.set(Telemetry {
        device_lost: true,
        ..running_telemetry(10)
    });
    let t0 = crate::session::clock_now();
    s.poll_status_at(t0);
    assert_eq!(
        *world.0.starts.borrow(),
        vec![Some("Interface".to_string()), None]
    );
    poll_alive(&mut s, &world, t0, 1);

    // The interface is gone from the list for a while, then listed again: the app goes back
    // to it on its own, once, and says so.
    poll_alive(&mut s, &world, t0, 5);
    assert_eq!(world.0.starts.borrow().len(), 2, "still unplugged");
    world.0.plugged.borrow_mut().push("Interface".into());
    poll_alive(&mut s, &world, t0, 9);
    assert_eq!(
        *world.0.starts.borrow(),
        vec![
            Some("Interface".to_string()),
            None,
            Some("Interface".to_string())
        ]
    );
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Interface") && message.contains("back"),
        "{message}"
    );
    assert_eq!(s.core().sink().status().device, "Interface");
    poll_alive(&mut s, &world, t0, 20);
    poll_alive(&mut s, &world, t0, 30);
    assert_eq!(world.0.starts.borrow().len(), 3, "no more restarts");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_return_waits_while_the_song_plays() {
    let dir = scratch("lost-return-playing");
    let world = Fake::default();
    let settings = saved("Interface", true);
    world.0.host.set(Some(settings.host.kind()));
    let mut s = shell(&world, &settings, &dir);
    world.0.telemetry.set(Telemetry {
        device_lost: true,
        ..running_telemetry(10)
    });
    let t0 = crate::session::clock_now();
    s.poll_status_at(t0);
    poll_alive(&mut s, &world, t0, 4);
    world.0.plugged.borrow_mut().push("Interface".into());
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Play);
    assert!(s.core().is_playing());
    poll_alive(&mut s, &world, t0, 8);
    assert_eq!(world.0.starts.borrow().len(), 2, "not while playing");
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Stop);
    assert!(!s.core().is_playing());
    poll_alive(&mut s, &world, t0, 9);
    assert_eq!(world.0.starts.borrow().len(), 3, "once it stopped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stall_resets_to_the_default_devices_only_after_two_seconds() {
    let dir = scratch("stall-reset");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", true), &dir);
    let t0 = crate::session::clock_now();
    world.0.telemetry.set(running_telemetry(10));
    s.poll_status_at(t0);
    s.poll_status_at(t0 + Duration::from_millis(500));
    assert_eq!(
        world.0.starts.borrow().len(),
        1,
        "a short stall may recover"
    );
    s.poll_status_at(t0 + Duration::from_millis(2_500));
    assert_eq!(world.0.starts.borrow().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_open_settings_dialog_lists_the_midi_ports_again_every_second() {
    let dir = scratch("port-relist");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    s.execute(AppCommand::AudioSettings);
    let t0 = crate::session::clock_now();
    s.poll_status_at(t0);
    let ports = |s: &Shell<Fake>| s.dialog.as_ref().unwrap().lists().midi_ports.clone();
    assert!(ports(&s).is_empty());
    // A controller is plugged in: the list does not change before the second is up...
    world.0.midi.borrow_mut().push("Example Pads".into());
    s.poll_status_at(t0 + Duration::from_millis(500));
    assert!(ports(&s).is_empty());
    // ...and does after it.
    s.poll_status_at(t0 + Duration::from_millis(1_100));
    assert_eq!(ports(&s), ["Example Pads"]);
    // Closing the dialog stops the listing.
    s.dialog = None;
    world.0.midi.borrow_mut().clear();
    s.poll_status_at(t0 + Duration::from_millis(5_000));
    assert!(s.ports_listed_at.is_none());
}

/// The centre of the Rescan chip of the open Audio Settings dialog.
fn rescan_chip(s: &Shell<Fake>) -> Point {
    let layout = s.dialog.as_ref().unwrap().layout(s.size());
    let (rect, ..) = layout
        .options
        .iter()
        .find(|(_, _, c)| *c == crate::settings_dialog::Choice::Rescan)
        .expect("the Audio tab has a Rescan chip");
    rect.center()
}

fn device_names(s: &Shell<Fake>) -> Vec<String> {
    let lists = s.dialog.as_ref().unwrap().lists();
    lists
        .hosts
        .iter()
        .flat_map(|h| h.devices.iter().map(|d| d.name.clone()))
        .collect()
}

#[test]
fn the_open_settings_dialog_lists_the_audio_devices_once_and_again_only_on_rescan() {
    let dir = scratch("device-rescan");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    let before = world.0.audio_listings.get();
    s.execute(AppCommand::AudioSettings);
    assert_eq!(world.0.audio_listings.get(), before + 1, "listed on open");
    // Ticks while the dialog stays open, a minute of them, list no audio device again...
    let t0 = crate::session::clock_now();
    for second in 0..60 {
        s.poll_status_at(t0 + Duration::from_secs(second));
    }
    assert_eq!(world.0.audio_listings.get(), before + 1, "not while open");
    // ...a USB interface plugged in meanwhile is not shown yet...
    world.0.plugged.borrow_mut().push("USB Interface".into());
    s.poll_status_at(t0 + Duration::from_secs(61));
    assert!(!device_names(&s).contains(&"USB Interface".to_string()));
    // ...and Rescan lists the audio devices again and shows it.
    let at = rescan_chip(&s);
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert_eq!(world.0.audio_listings.get(), before + 2, "listed on Rescan");
    assert!(device_names(&s).contains(&"USB Interface".to_string()));
    // Reopening lists once more.
    s.dialog = None;
    s.execute(AppCommand::AudioSettings);
    assert_eq!(world.0.audio_listings.get(), before + 3, "listed on reopen");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rescan_midi_ports_says_what_is_open() {
    let dir = scratch("rescan");
    let world = Fake::default();
    let mut s = shell(&world, &saved("Interface", false), &dir);
    s.execute(AppCommand::RescanMidi);
    let said = s.core().last_rejection().unwrap();
    assert!(said.contains("No MIDI input is switched on"), "{said}");
    // A switched-on port that is not there is named.
    let mut wanted = saved("Interface", false);
    wanted.midi_inputs = vec![crate::settings::MidiInputSetting::enabled("Example Pads")];
    let dir = scratch("rescan-missing");
    let mut s = shell(&world, &wanted, &dir);
    s.execute(AppCommand::RescanMidi);
    let said = s.core().last_rejection().unwrap();
    assert!(said.contains("Example Pads"), "{said}");
}

// ------------------------------------------------------------------ a contained engine fault

fn add_device(s: &mut Shell<Fake>, type_id: &str, key: &str) -> u32 {
    let key = DeviceKey(key.into());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key.clone()),
        })
        .unwrap();
    s.core().instance_id_of(&key).unwrap()
}

fn faulted(device: Option<u32>) -> AudioStatus {
    AudioStatus {
        faulted: true,
        faulted_device: device,
        ..AudioStatus::default()
    }
}

fn bypass_of(s: &Shell<Fake>, key: &str) -> f32 {
    s.core()
        .doc()
        .device(&DeviceKey(key.into()))
        .and_then(|d| d.params.get("bypass").copied())
        .unwrap_or(0.0)
}

fn fault_shell(name: &str) -> (Shell<Fake>, Fake, PathBuf) {
    let dir = scratch(name);
    let world = Fake::default();
    let s = shell(&world, &saved("Interface", false), &dir);
    (s, world, dir)
}

#[test]
fn a_device_fault_while_stopped_bypasses_the_device_and_restarts_once() {
    let (mut s, world, dir) = fault_shell("fault-stopped");
    let fx = add_device(&mut s, "atomik.reverb", "fx");
    let starts = world.0.starts.borrow().len();
    let t0 = crate::session::clock_now();

    assert!(
        s.handle_fault(&faulted(Some(fx)), t0),
        "the engine was rebuilt"
    );
    assert_eq!(world.0.starts.borrow().len(), starts + 1);
    assert!(bypass_of(&s, "fx") >= 0.5, "the faulted device is bypassed");
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Audio restarted after a fault in")
            && message.contains("the device is bypassed"),
        "{message}"
    );
    // The same fault, still reported by an old poll, does not restart again.
    assert!(!s.handle_fault(&faulted(Some(fx)), t0 + Duration::from_secs(1)));
    assert_eq!(world.0.starts.borrow().len(), starts + 1);

    // The bypass is an ordinary edit: undo puts the device back in.
    assert!(s.core_mut().undo());
    assert!(bypass_of(&s, "fx") < 0.5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fault_during_playback_waits_for_the_stop() {
    let (mut s, world, dir) = fault_shell("fault-playing");
    let fx = add_device(&mut s, "atomik.reverb", "fx");
    s.execute(AppCommand::Transport(atomik_ui::TransportAction::Play));
    assert!(s.core().is_playing());
    let starts = world.0.starts.borrow().len();
    let t0 = crate::session::clock_now();

    assert!(!s.handle_fault(&faulted(Some(fx)), t0));
    assert!(!s.handle_fault(&faulted(Some(fx)), t0 + Duration::from_secs(30)));
    assert_eq!(
        world.0.starts.borrow().len(),
        starts,
        "nothing while playing"
    );
    assert!(bypass_of(&s, "fx") < 0.5, "nor a bypass");
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("when playback stops"), "{message}");

    s.execute(AppCommand::Transport(atomik_ui::TransportAction::Stop));
    assert!(!s.core().is_playing());
    let after = world.0.starts.borrow().len();
    assert!(s.handle_fault(&faulted(Some(fx)), t0 + Duration::from_secs(31)));
    assert_eq!(world.0.starts.borrow().len(), after + 1, "one restart");
    assert!(bypass_of(&s, "fx") >= 0.5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn faults_a_minute_apart_restart_three_times_then_the_user_is_asked() {
    let (mut s, world, dir) = fault_shell("fault-limit");
    let t0 = crate::session::clock_now();
    let starts = || world.0.starts.borrow().len();
    let fault = faulted(None);
    let clear = AudioStatus::default();
    let secs = Duration::from_secs;

    // The first fault restarts at once; the new engine has no fault.
    let base = starts();
    assert!(s.handle_fault(&fault, t0));
    assert!(!s.handle_fault(&clear, t0 + secs(1)));
    assert_eq!(starts(), base + 1);
    assert_eq!(
        s.core().last_rejection(),
        Some("Audio restarted after a fault")
    );

    // The second, ten seconds later, waits for the minute.
    assert!(!s.handle_fault(&fault, t0 + secs(10)));
    assert_eq!(starts(), base + 1);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("within a minute")
    );
    assert!(!s.handle_fault(&fault, t0 + secs(59)));
    assert!(s.handle_fault(&fault, t0 + secs(60)));
    assert_eq!(starts(), base + 2);

    // The third.
    assert!(!s.handle_fault(&clear, t0 + secs(61)));
    assert!(s.handle_fault(&fault, t0 + secs(130)));
    assert_eq!(starts(), base + 3);

    // The fourth is left to the user: the manual message, no restart, however long it waits.
    assert!(!s.handle_fault(&clear, t0 + secs(131)));
    assert!(!s.handle_fault(&fault, t0 + secs(1000)));
    assert!(!s.handle_fault(&fault, t0 + secs(5000)));
    assert_eq!(starts(), base + 3);
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("Audio > Restart Audio"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undoing_the_bypass_of_a_device_that_faults_again_still_obeys_the_minute() {
    let (mut s, world, dir) = fault_shell("fault-undo");
    add_device(&mut s, "atomik.reverb", "fx");
    let fx = s.core().instance_id_of(&DeviceKey("fx".into())).unwrap();
    let t0 = crate::session::clock_now();
    assert!(s.handle_fault(&faulted(Some(fx)), t0));
    assert!(s.core_mut().undo());
    assert!(bypass_of(&s, "fx") < 0.5);
    s.handle_fault(&AudioStatus::default(), t0 + Duration::from_secs(2));
    let fx = s.core().instance_id_of(&DeviceKey("fx".into())).unwrap();
    let starts = world.0.starts.borrow().len();

    // It panics again twenty seconds later: nothing happens until the minute is over.
    assert!(!s.handle_fault(&faulted(Some(fx)), t0 + Duration::from_secs(20)));
    assert_eq!(world.0.starts.borrow().len(), starts);
    assert!(
        bypass_of(&s, "fx") < 0.5,
        "nothing is touched before the restart"
    );
    assert!(s.handle_fault(&faulted(Some(fx)), t0 + Duration::from_secs(61)));
    assert!(bypass_of(&s, "fx") >= 0.5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_device_without_a_bypass_switch_is_named_and_left_in_place() {
    let (mut s, _world, dir) = fault_shell("fault-instrument");
    let synth = add_device(&mut s, "atomik.poly-synth", "synth");
    assert!(s.handle_fault(&faulted(Some(synth)), crate::session::clock_now()));
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("it has no bypass"), "{message}");
    assert!(s.core().doc().device(&DeviceKey("synth".into())).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_callback_panic_seen_through_the_poll_restarts_the_stream() {
    let (mut s, world, dir) = fault_shell("fault-poll");
    world.0.telemetry.set(Telemetry {
        faulted: true,
        ..running_telemetry(10)
    });
    let starts = world.0.starts.borrow().len();
    s.poll_status_at(crate::session::clock_now());
    assert_eq!(world.0.starts.borrow().len(), starts + 1);
    assert_eq!(
        s.core().last_rejection(),
        Some("Audio restarted after a fault")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
