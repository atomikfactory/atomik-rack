//! `LiveSession` tests for the worker that opens and stops the audio stream (the shared fake
//! audio backend scripts slow, hung and failing drivers). Nothing here depends on a clock: a
//! driver is slow while the test holds it, and the tests wait on conditions, with a bound of
//! seconds, for what the worker does next.

use super::*;
use crate::settings::HostChoice;
use crate::test_backend::FakeAudio;
use atomik_core::SampleRate;
use atomik_testkit::backend::Hold;
use atomik_testkit::fakes::ParamLog;

type Session = LiveSession<FakeAudio>;

fn settings() -> AudioSettings {
    AudioSettings {
        host: HostChoice::Wasapi,
        output_device: Some("Interface".into()),
        buffer_frames: 256,
        workers: 0,
        workers_auto: false,
        worker_linger_spins: 0,
        ..AudioSettings::default()
    }
}

fn config(rate: f64) -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(rate).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 0,
    }
}

/// A session that opens its streams on the worker, over `audio`.
fn background(audio: &FakeAudio) -> Session {
    let mut session = LiveSession::new(audio.clone(), settings(), true);
    session.set_start_mode(StartMode::Background);
    session
}

/// Waits (for seconds, not for a count) until `done` holds.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let limit = clock_now() + Duration::from_secs(60);
    while !done() {
        assert!(clock_now() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Polls until the worker has nothing left to open.
fn wait_settled(session: &mut Session) {
    wait_until("the stream to open", || {
        session.poll();
        !session.starting()
    });
}

/// Waits until a call of the fake is blocked on `hold`.
fn wait_blocked(hold: &Hold) {
    wait_until("a driver call to block", || hold.waiting() >= 1);
}

fn gain() -> Box<dyn Device> {
    crate::app_core::default_registry()
        .create("atomik.gain-tool")
        .unwrap()
}

/// Renders one block through the engine the fake stream holds.
fn render_block(audio: &FakeAudio) {
    let (mut left, mut right) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    audio
        .with_engine(|engine| engine.render(&[], &mut [&mut left, &mut right]))
        .expect("a running engine");
}

#[test]
fn a_start_that_blocks_leaves_the_session_serviceable_and_says_so() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    // `reset` came back although the driver has not answered.
    assert!(s.starting());
    assert!(!s.is_running());
    assert!(!s.has_engine());
    wait_blocked(&hold);
    for _ in 0..20 {
        s.poll();
    }
    assert!(s.starting(), "still waiting for the driver");
    let status = s.status().clone();
    assert!(status.starting && !status.running);
    assert!(
        crate::status::status_text(&status).contains("Starting audio..."),
        "{}",
        crate::status::status_text(&status)
    );
    hold.release();
    wait_settled(&mut s);
    assert!(s.is_running());
    let status = s.status();
    assert!(!status.starting && status.running);
    assert_eq!(status.device, "Interface");
    assert!(!crate::status::status_text(status).contains("Starting"));
}

#[test]
fn calls_made_while_starting_are_all_there_once_running() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_blocked(&hold);
    let (first, second, gone) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    let (probe, log) = ParamLog::boxed(100_000);
    assert!(s.insert_device(first, gain(), &[]).is_ok());
    assert!(s.insert_device(second, probe, &[]).is_ok());
    assert!(s.insert_device(gone, gain(), &[]).is_ok());
    assert!(s.remove_device(gone).is_ok());
    assert!(s.set_tempo(Tempo::new(99.0).unwrap()).is_ok());
    // Two values of one parameter: the newest one is sent, after the device exists.
    assert!(s.set_param(second, ParamLog::A, 0.2).is_ok());
    assert!(s.set_param(second, ParamLog::A, 0.6).is_ok());
    assert!(s.transport(TransportCommand::Play).is_ok());
    assert!(!s.contains_device(first), "there is no engine yet");
    hold.release();
    wait_settled(&mut s);
    assert!(s.queued.is_empty(), "everything that waited was sent");
    assert!(s.contains_device(first) && s.contains_device(second));
    assert!(!s.contains_device(gone), "the removal followed the insert");
    render_block(&audio);
    s.poll();
    render_block(&audio);
    let sets: Vec<f32> = log
        .entries()
        .into_iter()
        .filter_map(|e| match e {
            atomik_testkit::fakes::ParamLogEntry::Set(p, v) if p == ParamLog::A => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(sets.last().map(|v| v.to_bits()), Some(0.6f32.to_bits()));
    assert!(s.transport_state().playing, "the Play that waited ran");
    // A call made now goes straight to the engine.
    assert!(s.insert_device(DeviceInstanceId(4), gain(), &[]).is_ok());
    assert!(s.contains_device(DeviceInstanceId(4)));
}

#[test]
fn only_the_newest_request_in_flight_applies() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(44_100.0));
    wait_blocked(&hold);
    // Two more requests while the first is in the driver: the second is replaced by the third.
    s.reset(&config(48_000.0));
    s.reset(&config(96_000.0));
    assert!(s.starting());
    hold.release();
    wait_settled(&mut s);
    assert!(s.is_running());
    assert_eq!(s.status().sample_rate, 96_000);
    let rates: Vec<u32> = audio.starts().iter().map(|c| c.sample_rate).collect();
    assert_eq!(
        rates,
        vec![44_100, 96_000],
        "the replaced request never reached the driver"
    );
    assert_eq!(
        audio.stops(),
        1,
        "the stream of the first request was stopped"
    );
    assert_eq!(audio.open_streams(), 1);
}

#[test]
fn a_reset_that_disables_audio_cancels_the_start_in_flight() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_blocked(&hold);
    s.set_enabled(false);
    s.reset(&config(48_000.0));
    assert!(s.next.is_none(), "nothing waits to be opened");
    hold.release();
    wait_until("the stream that arrived to be stopped", || {
        s.poll();
        !s.starting() && s.job.is_none() && audio.open_streams() == 0
    });
    assert!(!s.is_running());
    assert_eq!(audio.stops(), 1);
}

#[test]
fn a_stop_that_blocks_does_not_block_the_session_and_the_start_waits_for_it() {
    let audio = FakeAudio::new();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(s.insert_device(DeviceInstanceId(1), gain(), &[]).is_ok());
    let hold = audio.hold_stops();
    s.reset(&config(48_000.0));
    assert!(s.starting() && !s.is_running());
    wait_blocked(&hold);
    assert_eq!(
        audio.start_calls().len(),
        1,
        "the device is not opened while the old stream is still stopping"
    );
    hold.release();
    wait_settled(&mut s);
    assert!(s.is_running());
    assert_eq!(audio.start_calls().len(), 2);
    assert_eq!(audio.stops(), 1);
    assert!(
        !s.contains_device(DeviceInstanceId(1)),
        "the new engine is empty"
    );
}

#[test]
fn a_start_that_fails_shows_the_same_error_as_a_blocking_one() {
    let audio = FakeAudio::new();
    audio.fail_next_start(IoError::DeviceNotFound("Interface".into()));
    let mut blocking = LiveSession::new(audio.clone(), settings(), true);
    blocking.reset(&config(48_000.0));
    let expected = blocking.status().error.clone().expect("an error");

    audio.fail_next_start(IoError::DeviceNotFound("Interface".into()));
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(!s.is_running() && !s.status().starting);
    assert_eq!(s.status().error.as_deref(), Some(expected.as_str()));
    assert!(crate::status::status_text(s.status()).contains(&format!("audio off: {expected}")));
    // Calls are ignored as with audio off, and the next reset tries again.
    assert!(s.insert_device(DeviceInstanceId(1), gain(), &[]).is_ok());
    assert!(s.queued.is_empty());
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(s.is_running());
    assert_eq!(s.status().error, None);
}

#[test]
fn a_device_that_does_not_open_falls_back_to_the_last_working_one() {
    let audio = FakeAudio::new();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(s.is_running());
    audio.break_device("Unplugged");
    let mut wanted = settings();
    wanted.output_device = Some("Unplugged".into());
    assert_eq!(s.set_settings(wanted), SettingsChange::RestartAudio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(s.is_running(), "the last working device is open again");
    assert_eq!(s.settings().output_device.as_deref(), Some("Interface"));
    let notice = s.take_notice().expect("the fallback is told");
    assert!(
        notice.contains("Unplugged") && notice.contains("last working"),
        "{notice}"
    );
    let devices: Vec<_> = audio.starts().iter().map(|c| c.device.clone()).collect();
    assert_eq!(
        devices.last(),
        Some(&Some("Interface".to_string())),
        "{devices:?}"
    );
}

#[test]
fn an_input_that_does_not_open_still_opens_the_output() {
    let audio = FakeAudio::new();
    audio.fail_input(true);
    let mut s = background(&audio);
    let mut with_input = config(48_000.0);
    with_input.input_channels = 2;
    s.reset(&with_input);
    wait_settled(&mut s);
    assert!(s.is_running());
    assert!(s.status().input_error.is_some());
    assert_eq!(s.status().input_channels_open, 0);
}

#[test]
fn a_failed_stop_is_shown_while_audio_stays_off() {
    let audio = FakeAudio::new();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    audio.fail_stops(IoError::Backend("the driver kept the callback".into()));
    s.set_enabled(false);
    s.reset(&config(48_000.0));
    wait_until("the stop to answer", || {
        s.poll();
        s.job.is_none()
    });
    assert!(!s.is_running());
    let error = s.status().error.clone().expect("the stop error");
    assert!(error.contains("kept the callback"), "{error}");
    // Opening again goes on without the old error.
    audio.stop_works();
    s.set_enabled(true);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    assert!(s.is_running());
    assert_eq!(s.status().error, None);
}

#[test]
fn a_quit_while_a_start_is_pending_returns_without_the_driver() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.set_quit_wait(Duration::from_millis(200));
    s.reset(&config(48_000.0));
    wait_blocked(&hold);
    s.shutdown();
    // The driver never answered: the quit went on without it.
    assert_eq!(hold.waiting(), 1, "the call is still blocked in the driver");
    assert!(!s.starting() && !s.is_running());
    assert!(!s.status().starting);
    hold.release();
    // What the abandoned worker opens after the release is nobody's: the session is unharmed.
    drop(s);
}

#[test]
fn a_quit_stops_the_stream_that_arrives_after_the_request_was_dropped() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_blocked(&hold);
    let release = hold.clone();
    // The driver answers a moment after the quit began: the stream is stopped, not kept.
    let helper = std::thread::spawn(move || release.release());
    s.shutdown();
    helper.join().unwrap();
    assert_eq!(audio.open_streams(), 0);
    assert_eq!(audio.stops(), 1);
}

#[test]
fn a_backlog_larger_than_the_queue_is_sent_as_the_engine_takes_it() {
    let audio = FakeAudio::new();
    let hold = audio.hold_starts();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_blocked(&hold);
    let count = u32::try_from(atomik_engine::CONTROL_QUEUE_CAPACITY).unwrap() + 80;
    for id in 1..=count {
        assert!(s.insert_device(DeviceInstanceId(id), gain(), &[]).is_ok());
    }
    hold.release();
    wait_settled(&mut s);
    assert!(!s.queued.is_empty(), "the queue had room for part of it");
    assert!(s.contains_device(DeviceInstanceId(1)));
    // The engine takes what is queued at its next blocks; each poll sends more.
    for _ in 0..50 {
        if s.queued.is_empty() {
            break;
        }
        render_block(&audio);
        s.poll();
    }
    assert!(s.queued.is_empty(), "the whole backlog was sent");
    assert!(s.contains_device(DeviceInstanceId(count)));
    assert_eq!(s.poll().overload.control_stalled, 0, "nothing was lost");
}

#[test]
fn a_lost_device_is_replaced_through_the_worker() {
    let audio = FakeAudio::new();
    let mut s = background(&audio);
    s.reset(&config(48_000.0));
    wait_settled(&mut s);
    audio.unplug();
    s.poll();
    s.fall_back_to_defaults();
    s.set_enabled(true);
    let hold = audio.hold_starts();
    s.reset(&config(48_000.0));
    assert!(s.starting() && !s.is_running());
    wait_blocked(&hold);
    hold.release();
    wait_settled(&mut s);
    assert!(s.is_running());
    assert_eq!(s.settings().output_device, None, "the default device runs");
    assert_eq!(audio.stops(), 1, "the dead stream was stopped on the way");
}
