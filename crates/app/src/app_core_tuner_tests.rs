//! The audio track tuner through the core: the shown state and `internal.tuner_live`, the feed
//! from a fake tap (smoothing, hold, the reference pitch, a paused engine), and with a real
//! offline engine that the tuner reads the input after the track's gain and that the track is
//! muted while it shows.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::song::AUDIO_TRACK_TYPE;
use atomik_model::{SongCommand, empty_doc};
use atomik_ui::TransportAction;
use atomik_ui::analyser::AnalyserData;

const SR: f64 = 48_000.0;

fn sine(hz: f64, amp: f32, from: u64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amp * (std::f64::consts::TAU * hz * (from + i as u64) as f64 / SR).sin() as f32)
        .collect()
}

fn add_audio_track<S: EngineSink>(core: &mut AppCore<S>) -> DeviceKey {
    core.apply_command(Command::Song(SongCommand::AddAudioTrack {
        name: Some("Guitar".to_owned()),
    }))
    .unwrap();
    core.doc().song.tracks.last().unwrap().device.clone()
}

fn set_param<S: EngineSink>(core: &mut AppCore<S>, key: &DeviceKey, param: &str, value: f32) {
    core.apply_command(Command::SetParam {
        key: key.clone(),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

fn tuner_params(core: &mut AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, f32)> {
    let pid = core
        .registry()
        .get(AUDIO_TRACK_TYPE)
        .unwrap()
        .descriptor
        .param_by_key("internal.tuner_live")
        .unwrap()
        .id;
    core.sink_mut()
        .take()
        .into_iter()
        .filter_map(|c| match c {
            SinkCall::Param { id, param, plain } if param == pid => Some((id, plain)),
            _ => None,
        })
        .collect()
}

fn show<S: EngineSink>(core: &mut AppCore<S>, key: &DeviceKey, on: bool) {
    core.handle_action(UiAction::SetTuner {
        device: key.clone(),
        on,
    });
}

fn data<S: EngineSink>(core: &AppCore<S>, key: &DeviceKey) -> AnalyserData {
    core.view()
        .analyser_data(key)
        .expect("an audio track panel")
        .clone()
}

/// A fake tap holding `frames` of a sine ending at `head`, the same on both channels.
fn tap(hz: f64, amp: f32, frames: usize, head: u64) -> (Vec<f32>, Vec<f32>, u64) {
    let l = sine(hz, amp, head - frames as u64, frames);
    (l.clone(), l, head)
}

// ------------------------------------------------------------------ the shown state

#[test]
fn showing_the_tuner_tells_the_track_and_hiding_tells_it_again() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    assert!(
        tuner_params(&mut core).is_empty(),
        "a new track starts at 0"
    );
    assert!(!core.tuner_shown(&key));
    show(&mut core, &key, true);
    assert!(core.tuner_shown(&key));
    assert_eq!(tuner_params(&mut core), vec![(id, 1.0)]);
    // Nothing is sent again while nothing changes, frame after frame.
    core.frame();
    core.frame();
    assert!(tuner_params(&mut core).is_empty());
    show(&mut core, &key, false);
    assert_eq!(tuner_params(&mut core), vec![(id, 0.0)]);
    assert!(!core.tuner_shown(&key));
    // It is UI state: not in the document, not dirty because of it.
    let device = core.doc().device(&key).unwrap();
    assert!(!device.params.contains_key("internal.tuner_live"));
    assert!(!device.params.contains_key("tuner_mute"));
}

#[test]
fn the_tuner_is_sent_again_after_an_engine_restart_and_a_re_add() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    show(&mut core, &key, true);
    tuner_params(&mut core);
    core.rebuild_engine();
    let id = core.device_id(&key).unwrap();
    assert_eq!(tuner_params(&mut core), vec![(id, 1.0)]);
    // Removing the track drops the tuner; undoing the removal does not bring it back.
    core.apply_command(Command::RemoveDevice { key: key.clone() })
        .unwrap();
    core.frame();
    assert!(!core.tuner_shown(&key));
    tuner_params(&mut core);
    assert!(core.undo());
    core.frame();
    assert!(!core.tuner_shown(&key));
    assert!(tuner_params(&mut core).iter().all(|(_, v)| *v == 0.0));
}

#[test]
fn only_an_audio_track_can_show_a_tuner() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(DeviceKey("synth".into())),
    })
    .unwrap();
    let synth = DeviceKey("synth".into());
    show(&mut core, &synth, true);
    assert!(!core.tuner_shown(&synth));
    assert!(tuner_params(&mut core).is_empty());
    show(&mut core, &DeviceKey("nope".into()), true);
}

// ------------------------------------------------------------------ the feed from a fake tap

#[test]
fn a_shown_tuner_reads_the_tap_each_frame_and_a_hidden_one_does_not() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    core.sink_mut()
        .device_scopes
        .insert(id, tap(110.0, 0.3, 8192, 8192));
    core.frame();
    assert_eq!(
        data(&core, &key),
        AnalyserData::default(),
        "hidden: nothing"
    );
    show(&mut core, &key, true);
    assert!(data(&core, &key).tuner.is_none() || !data(&core, &key).tuner_on);
    core.frame();
    let d = data(&core, &key);
    assert!(d.tuner_on);
    let r = d.tuner.expect("A2 is 110 Hz");
    assert_eq!(r.note.key, 45);
    assert!(r.note.cents.abs() < 1.0, "{}", r.note.cents);
    assert!((r.hz - 110.0).abs() < 0.2, "{}", r.hz);
    assert!(!r.held);
    // The same write position: skipped, even though the frames behind it changed.
    core.sink_mut()
        .device_scopes
        .insert(id, tap(220.0, 0.3, 8192, 8192));
    core.frame();
    assert_eq!(data(&core, &key).tuner.unwrap().note.key, 45);
    // A new position is read: the note moved an octave.
    core.sink_mut()
        .device_scopes
        .insert(id, tap(220.0, 0.3, 8192, 8192 + 800));
    core.frame();
    assert_eq!(data(&core, &key).tuner.unwrap().note.key, 57);
}

#[test]
fn the_note_is_held_when_the_signal_stops_then_cleared() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    show(&mut core, &key, true);
    let mut head = 8192u64;
    core.sink_mut()
        .device_scopes
        .insert(id, tap(82.41, 0.3, 8192, head));
    core.frame();
    assert_eq!(data(&core, &key).tuner.unwrap().note.key, 40);
    // Silence: held for a while.
    head += 4000;
    core.sink_mut()
        .device_scopes
        .insert(id, (vec![0.0; 8192], vec![0.0; 8192], head));
    core.frame();
    let r = data(&core, &key).tuner.expect("held");
    assert!(r.held && r.note.key == 40);
    // Long after: gone, and the panel says to play a note.
    head += 48_000;
    core.sink_mut()
        .device_scopes
        .insert(id, (vec![0.0; 8192], vec![0.0; 8192], head));
    core.frame();
    let d = data(&core, &key);
    assert!(d.tuner_on && d.tuner.is_none());
}

#[test]
fn the_reference_pitch_of_the_track_is_the_a4_of_the_reading() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    show(&mut core, &key, true);
    core.sink_mut()
        .device_scopes
        .insert(id, tap(440.0, 0.3, 8192, 8192));
    core.frame();
    assert!(data(&core, &key).tuner.unwrap().note.cents.abs() < 1.0);
    set_param(&mut core, &key, "tuner_ref", 432.0);
    core.sink_mut()
        .device_scopes
        .insert(id, tap(440.0, 0.3, 8192, 8192 + 800));
    core.frame();
    let r = data(&core, &key).tuner.unwrap();
    assert_eq!(r.note.key, 69);
    assert!((r.note.cents - 31.8).abs() < 1.0, "{}", r.note.cents);
}

#[test]
fn the_louder_channel_is_the_one_analysed() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    show(&mut core, &key, true);
    // A guitar on the right input only.
    let r = sine(196.0, 0.3, 0, 8192);
    core.sink_mut()
        .device_scopes
        .insert(id, (vec![0.0; 8192], r, 8192));
    core.frame();
    assert_eq!(data(&core, &key).tuner.unwrap().note.key, 55);
}

#[test]
fn an_unrouted_session_without_a_tap_shows_the_idle_display() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    show(&mut core, &key, true);
    core.frame();
    let d = data(&core, &key);
    assert_eq!(
        d,
        AnalyserData {
            tuner_on: true,
            ..AnalyserData::default()
        }
    );
}

// ------------------------------------------------------------------ a real engine

fn core_with_input(
    channels: u16,
    input: impl Fn(usize, u64) -> f32 + 'static,
) -> AppCore<OfflineSink> {
    let mut doc = empty_doc();
    doc.input_channels = channels;
    let mut sink = OfflineSink::new();
    sink.input = Box::new(input);
    AppCore::with_doc(sink, doc, None)
}

fn pump(core: &mut AppCore<OfflineSink>, frames: usize) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = 800.min(frames - done);
        let block = core.sink_mut().render(n);
        core.frame();
        if out.is_empty() {
            out = vec![Vec::new(); block.len()];
        }
        for (o, b) in out.iter_mut().zip(&block) {
            o.extend_from_slice(b);
        }
        done += n;
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn the_tuner_reads_the_input_after_the_gain_even_unarmed() {
    // A 220 Hz tone at -62 dBFS is under the tuner's gate; +12 dB of input gain lifts it over.
    let amp = 0.0008;
    let mut core = core_with_input(2, move |_, f| {
        amp * (std::f64::consts::TAU * 220.0 * f as f64 / SR).sin() as f32
    });
    let key = add_audio_track(&mut core);
    assert!(!core.doc().song.track(&key).unwrap().armed, "not armed");
    show(&mut core, &key, true);
    pump(&mut core, 24_000);
    assert!(
        data(&core, &key).tuner.is_none(),
        "at 0 dB the tone is under the gate"
    );
    set_param(&mut core, &key, "rec_gain", 12.0);
    pump(&mut core, 24_000);
    let r = data(&core, &key).tuner.expect("A3 after the gain");
    assert_eq!(r.note.key, 57);
    assert!(r.note.cents.abs() < 1.0, "{}", r.note.cents);
    assert!((r.hz - 220.0).abs() < 0.5, "{}", r.hz);
}

#[test]
fn a_guitar_a_few_cents_flat_reads_flat() {
    let hz = 110.0 * (-14.0f64 / 1200.0).exp2();
    let mut core = core_with_input(1, move |_, f| {
        0.2 * (std::f64::consts::TAU * hz * f as f64 / SR).sin() as f32
    });
    let key = add_audio_track(&mut core);
    show(&mut core, &key, true);
    pump(&mut core, 24_000);
    let r = data(&core, &key).tuner.expect("a reading");
    assert_eq!(r.note.key, 45);
    assert!((r.note.cents + 14.0).abs() < 1.0, "{}", r.note.cents);
}

#[test]
fn the_track_is_muted_while_the_tuner_shows_unless_the_option_is_off() {
    let mut core = core_with_input(2, |_, _| 0.25);
    let key = add_audio_track(&mut core);
    core.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key.clone(),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    core.transport_action(TransportAction::Stop);
    let heard = pump(&mut core, 4_800);
    assert!(peak(&heard[0][3_000..]) > 0.01, "the armed input is heard");
    show(&mut core, &key, true);
    let muted = pump(&mut core, 4_800);
    assert!(
        peak(&muted[0][1_000..]) < 1e-6 && peak(&muted[1][1_000..]) < 1e-6,
        "silent: {}",
        peak(&muted[0][1_000..])
    );
    show(&mut core, &key, false);
    let back = pump(&mut core, 4_800);
    assert!(peak(&back[0][3_000..]) > 0.01);
    // With the option off the tuner shows and the track keeps sounding.
    set_param(&mut core, &key, "tuner_mute", 0.0);
    show(&mut core, &key, true);
    let open = pump(&mut core, 4_800);
    assert!(peak(&open[0][3_000..]) > 0.01);
}

#[test]
fn a_click_on_the_tuner_button_shows_the_tuner_and_mutes_the_track() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core);
    let id = core.device_id(&key).unwrap();
    let _ = core.handle_event(&atomik_ui::UiEvent::Resize(atomik_ui::kurbo::Size::new(
        1200.0, 3_000.0,
    )));
    let mut dl = atomik_ui::DisplayList::new();
    core.paint(&mut dl);
    tuner_params(&mut core);
    let at = core.view().tuner_button_center(&key).expect("the button");
    for event in [
        atomik_ui::UiEvent::PointerDown {
            pos: at,
            button: atomik_ui::PointerButton::Primary,
            modifiers: atomik_ui::Modifiers::NONE,
            clicks: 1,
        },
        atomik_ui::UiEvent::PointerUp {
            pos: at,
            button: atomik_ui::PointerButton::Primary,
            modifiers: atomik_ui::Modifiers::NONE,
        },
    ] {
        let _ = core.handle_event(&event);
    }
    assert!(core.tuner_shown(&key));
    assert_eq!(tuner_params(&mut core), vec![(id, 1.0)]);
    core.paint(&mut dl);
    assert!(dl.texts().contains(&"PLAY A NOTE"), "{:?}", dl.texts());
}
