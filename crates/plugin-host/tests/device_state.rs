//! (device state machine): bypass crossfade, sleep and skip rules, power, the
//! FAILED state and the panic boundary, the restart hand-shake, latency, pinning and the
//! reaper.
// Justification: the crossfade and the dry path are exact arithmetic on known values.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{Device, RunMode};
use atomik_plugin_host::{
    PluginEvent, PluginEventKind as K, PluginIo, PluginMessage, PluginProcessor, ProcessStatus,
    SleepRule, TailLength,
};
use std::sync::{Arc, Mutex};
use support::*;

const FADE: usize = 240; // 5 ms at 48 kHz

fn on(offset: u16, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On { key, velocity: 1.0 },
    }
}

fn off(offset: u16, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Off { key },
    }
}

fn drain_messages(m: &atomik_plugin_host::ManagerEnd) -> Vec<PluginMessage> {
    std::iter::from_fn(|| m.try_recv()).collect()
}

// ----- bypass ---------------------------------------------------------------------------------

#[test]
fn bypass_is_a_five_millisecond_linear_crossfade_to_the_dry_main_input() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    let (mut h, _r) = harness(rig("st.bypass", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [1.0; BATCH];
    *h.input_mut("in_r") = [-1.0; BATCH];
    h.process();
    assert_eq!(h.output("out_l")[BATCH - 1], 0.5, "not bypassed: wet");
    h.set_param("bypass", 1.0);
    let mut l = Vec::new();
    for _ in 0..5 {
        h.process();
        l.extend_from_slice(h.output("out_l"));
    }
    for (i, s) in l.iter().enumerate().take(FADE) {
        let m = (i + 1) as f32 / FADE as f32;
        let want = 0.5 * (1.0 - m) + 1.0 * m;
        assert!((s - want).abs() < 1e-5, "frame {i}: {s} vs {want}");
    }
    assert!(
        l[FADE..].iter().all(|s| *s == 1.0),
        "fully dry after the fade"
    );
    assert_eq!(h.output("out_r")[BATCH - 1], -1.0);
    // Back: the fade runs the other way and ends on the wet signal.
    h.set_param("bypass", 0.0);
    let mut back = Vec::new();
    for _ in 0..5 {
        h.process();
        back.extend_from_slice(h.output("out_l"));
    }
    assert!(back[0] < 1.0 && back[0] > 0.99);
    assert!(back[FADE..].iter().all(|s| *s == 0.5));
    // The plugin kept processing the whole time (state stays warm).
    assert_eq!(shared.calls(), 11);
}

#[test]
fn a_bypassed_instrument_fades_to_silence_and_holds_new_notes_back() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(1.0));
    let (mut h, _r) = harness(rig("st.bypass-synth", &synth_layout(), Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    h.process();
    h.set_param("bypass", 1.0);
    for _ in 0..5 {
        h.process();
    }
    assert!(h.output("out_l").iter().all(|s| *s == 0.0), "silent");
    let before = shared.log.lock().unwrap().events.len();
    // A new note is held back; the Off of the note opened before the bypass still goes through
    // so nothing hangs, and so does an AllOff.
    h.push_note("notes_in", on(0, 64));
    h.push_note("notes_in", off(1, 60));
    h.process();
    h.push_note(
        "notes_in",
        NoteEvent {
            offset: 2,
            kind: NoteKind::AllOff,
        },
    );
    h.process();
    let log = shared.log.lock().unwrap();
    let new: Vec<_> = log.events[before..].iter().map(|e| e.kind).collect();
    assert!(
        !new.iter().any(|k| matches!(k, K::NoteOn { key: 64, .. })),
        "{new:?}"
    );
    assert!(new.iter().any(|k| matches!(k, K::NoteOff { key: 60, .. })));
    assert!(new.iter().any(|k| matches!(
        k,
        K::Cc {
            controller: 123,
            ..
        }
    )));
}

#[test]
fn power_off_bypasses_and_then_stops_processing_until_power_on() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    let (mut h, r) = harness(rig("st.power", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [1.0; BATCH];
    h.process();
    r.manager.slot().set_powered(false);
    for _ in 0..6 {
        h.process();
    }
    let calls = shared.calls();
    assert!(
        calls <= 5,
        "processing stops once the fade is done: {calls}"
    );
    assert_eq!(h.output("out_l")[0], 1.0, "dry");
    let mut meters = [0.0f32; 5];
    h.device().meter_values(&mut meters);
    assert_eq!(meters[2], 2.0, "state stopped");
    for _ in 0..5 {
        h.process();
    }
    assert_eq!(shared.calls(), calls, "still stopped");
    r.manager.slot().set_powered(true);
    for _ in 0..6 {
        h.process();
    }
    assert!(shared.calls() > calls, "processing again");
    assert_eq!(
        h.output("out_l")[BATCH - 1],
        0.5,
        "wet again after the fade"
    );
}

// ----- sleep and skip ---------------------------------------------------------------------------

#[test]
fn a_clap_style_sleep_status_skips_the_plugin_until_an_event_a_param_or_an_input() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.25));
    shared.set_status(ProcessStatus::Sleep);
    let (mut h, _r) = harness(rig("st.sleep", &synth_layout(), Box::new(probe)));
    h.process(); // processes once and answers Sleep
    assert_eq!(shared.calls(), 1);
    for _ in 0..5 {
        h.process();
    }
    assert_eq!(shared.calls(), 1, "skipped");
    assert!(
        h.output("out_l").iter().all(|s| *s == 0.0),
        "zeros while asleep"
    );
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 1.0, "sleeping");
    // A note wakes it.
    h.push_note("notes_in", on(0, 60));
    h.process();
    assert_eq!(shared.calls(), 2);
    // So does a param change.
    h.process();
    assert_eq!(shared.calls(), 2);
    h.set_param("p2", 0.3);
    h.process();
    assert_eq!(shared.calls(), 3);
}

#[test]
fn a_sleeping_effect_wakes_on_a_non_silent_input() {
    let (probe, shared) = Probe::new();
    shared.set_status(ProcessStatus::Sleep);
    let (mut h, _r) = harness(rig("st.sleep-fx", &fx_layout(), Box::new(probe)));
    h.input_mut("in_l"); // cabled but silent
    h.process();
    h.process();
    h.process();
    assert_eq!(shared.calls(), 1);
    h.input_mut("in_l")[5] = 0.001;
    h.process();
    assert_eq!(shared.calls(), 2, "input woke it");
    // Sidechain input wakes it as well.
    h.input_mut("in_l")[5] = 0.0;
    h.process();
    assert_eq!(shared.calls(), 2);
    h.input_mut("sc_in_l")[9] = 0.5;
    h.process();
    assert_eq!(shared.calls(), 3);
}

#[test]
fn the_vst3_style_rule_skips_only_with_zero_tail_silence_and_nothing_pending() {
    // Zero tail: nothing to do on a silent, eventless batch of an effect.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    let (mut h, _r) = harness(rig("st.zt", &fx_layout(), Box::new(probe)));
    h.process();
    h.process();
    assert_eq!(shared.calls(), 0);
    h.set_param("p2", 0.5);
    h.process();
    assert_eq!(shared.calls(), 1, "a param change is work");
    h.process();
    assert_eq!(shared.calls(), 1);
    // A finite tail above zero or an infinite one never sleeps.
    for tail in [TailLength::Finite(1_000), TailLength::Infinite] {
        let (probe, shared) = Probe::new();
        shared.set_rule(SleepRule::ZeroTail, tail);
        let (mut h, _r) = harness(rig("st.zt2", &synth_layout(), Box::new(probe)));
        for _ in 0..4 {
            h.process();
        }
        assert_eq!(shared.calls(), 4, "{tail:?}");
    }
    // A non-silent input is work too.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    let (mut h, _r) = harness(rig("st.zt3", &fx_layout(), Box::new(probe)));
    h.input_mut("in_l");
    h.process();
    assert_eq!(shared.calls(), 0);
    h.input_mut("in_l")[0] = 0.1;
    h.process();
    assert_eq!(shared.calls(), 1);
}

/// 1 s of 64-frame batches at 48 kHz.
const IDLE: usize = 750;

#[test]
fn a_zero_tail_instrument_is_processed_until_a_second_after_its_last_sound_then_sleeps_for_real() {
    // The commercial wavetable synth bug: a plugin that reports a zero tail while a note sounds was skipped after
    // the batch that carried the note-on. An instrument is processed while a note is open, and
    // until its own output has been silent for a second; then it is stopped (setProcessing
    // false) and skipped; the next event starts it again before the block.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    shared.set_out(ProbeOut::Constant(0.25));
    let (mut h, _r) = harness(rig("st.zt-inst", &synth_layout(), Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    h.process();
    for _ in 0..1_500 {
        h.process();
    }
    assert_eq!(
        shared.calls(),
        1_501,
        "a held note is processed in every batch"
    );
    assert_eq!(shared.log.lock().unwrap().starts, 1);
    assert_eq!(shared.log.lock().unwrap().stops, 0);
    // Note off, the plugin goes quiet (a zero tail and silence).
    h.push_note("notes_in", off(0, 60));
    shared.set_out(ProbeOut::Silent);
    h.process();
    let after_off = shared.calls();
    for _ in 0..IDLE - 2 {
        h.process();
    }
    assert_eq!(
        shared.calls(),
        after_off + IDLE - 2,
        "still processed during the silent second"
    );
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 0.0, "not asleep yet");
    for _ in 0..5 {
        h.process();
    }
    let slept = shared.calls();
    assert!(
        slept < after_off + IDLE + 5,
        "asleep after the silent second"
    );
    for _ in 0..20 {
        h.process();
    }
    assert_eq!(shared.calls(), slept, "no calls while asleep");
    assert_eq!(shared.log.lock().unwrap().stops, 1, "told to stop");
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 1.0, "SLEEPING, and only now");
    // A param change or a note wakes it, started again before the block.
    h.push_note("notes_in", on(0, 64));
    shared.set_out(ProbeOut::Constant(0.25));
    h.process();
    assert_eq!(shared.calls(), slept + 1);
    assert_eq!(shared.log.lock().unwrap().starts, 2, "restarted");
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 0.0, "RUNNING again");
    assert_eq!(h.output("out_l")[0], 0.25);
}

#[test]
fn an_instrument_with_an_audio_input_follows_the_instrument_rule_too() {
    // Surge XT has an audio input (its effects take it) and is still an instrument: it must not
    // be skipped after the note-on batch when it reports a zero tail.
    let mut layout = synth_layout();
    layout.main_in = Some(atomik_device_sdk::Channels::Stereo);
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    shared.set_out(ProbeOut::Constant(0.25));
    let (mut h, _r) = harness(rig("st.zt-inst-in", &layout, Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    for _ in 0..50 {
        h.process();
    }
    assert_eq!(shared.calls(), 50);
}

#[test]
fn a_note_effect_with_an_open_note_is_processed_and_never_stopped_and_an_idle_effect_is_not_stopped()
 {
    // The render regression of this fix: a note effect that was told to stop while it held an
    // open note lost its state and the notes it forwarded hung downstream.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    let (mut h, _r) = harness(rig("st.zt-shift", &shift_layout(), Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    h.process();
    for _ in 0..2 * IDLE {
        h.process();
    }
    assert_eq!(shared.calls(), 2 * IDLE + 1, "an open note keeps it awake");
    assert_eq!(shared.log.lock().unwrap().stops, 0);
    h.push_note("notes_in", off(0, 60));
    h.process();
    for _ in 0..20 {
        h.process();
    }
    assert_eq!(
        shared.calls(),
        2 * IDLE + 2,
        "skipped once the note is closed"
    );
    assert_eq!(
        shared.log.lock().unwrap().stops,
        0,
        "a note effect is just not called"
    );
    // An effect with nothing to do is skipped, not stopped.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    let (mut h, _r) = harness(rig("st.zt-fx-stop", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.1; BATCH];
    h.process();
    *h.input_mut("in_l") = [0.0; BATCH];
    for _ in 0..10 {
        h.process();
    }
    assert_eq!(shared.calls(), 1);
    assert_eq!(shared.log.lock().unwrap().stops, 0);
}

#[test]
fn an_instrument_with_an_open_note_never_sleeps_even_when_silent() {
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    shared.set_out(ProbeOut::Silent);
    let (mut h, _r) = harness(rig("st.zt-open", &synth_layout(), Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    h.process();
    for _ in 0..3 * IDLE {
        h.process();
    }
    assert_eq!(shared.calls(), 3 * IDLE + 1);
}

// ----- failure --------------------------------------------------------------------------------

#[test]
fn an_error_status_marks_the_device_failed_dry_for_effects_and_posts_one_message() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Error);
    let (mut h, r) = harness(rig("st.error-fx", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.75; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    for _ in 0..4 {
        h.process();
        assert!(
            h.output("out_l").iter().all(|s| *s == 0.75),
            "the dry input"
        );
        assert!(h.output("out_r").iter().all(|s| *s == 0.25));
    }
    assert_eq!(shared.calls(), 1, "never called again");
    let msgs = drain_messages(&r.manager);
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(matches!(msgs[0], PluginMessage::Failed { .. }));
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 3.0, "failed");
}

#[test]
fn a_failed_instrument_is_silent() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Error);
    let (mut h, _r) = harness(rig("st.error-synth", &synth_layout(), Box::new(probe)));
    for _ in 0..3 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 0.0));
    }
}

#[test]
fn non_finite_output_fails_the_device_and_never_reaches_the_cables() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Nan);
    let (mut h, r) = harness(rig("st.nan", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.5; BATCH];
    h.process();
    assert!(h.output("out_l").iter().all(|s| *s == 0.5), "dry, not NaN");
    h.process();
    assert_eq!(shared.calls(), 1);
    assert!(
        drain_messages(&r.manager)
            .iter()
            .any(|m| matches!(m, PluginMessage::Failed { .. }))
    );
}

/// Runs `batches` batches of a device with every socket cabled and `input` on the main inputs,
/// without the allocation guard (a panicking plugin allocates while it unwinds). Returns
/// the last `out_l` batch.
fn run_unguarded(rg: &mut Rig, batches: usize, input: f32) -> [f32; BATCH] {
    use atomik_core::{NoteBuffer, SampleRate, Tempo, TimeInfo};
    use atomik_device_sdk::{ProcessContext, SocketKind};
    let d = rg.descriptor;
    let n = d.sockets.len();
    let mut signals = vec![[0.0; BATCH]; n];
    let mut notes: Vec<NoteBuffer> = d
        .sockets
        .iter()
        .map(|s| NoteBuffer::with_capacity(if s.kind == SocketKind::Notes { 8 } else { 0 }))
        .collect();
    let connected = vec![true; n];
    let mut last = [0.0; BATCH];
    for b in 0..batches {
        for (i, s) in d.sockets.iter().enumerate() {
            signals[i] = if s.kind == SocketKind::Audio && s.direction.is_input() {
                [input; BATCH]
            } else {
                [0.0; BATCH]
            };
        }
        let time = TimeInfo::at(
            (b * BATCH) as u64,
            SampleRate::new(SR).unwrap(),
            Tempo::new(120.0).unwrap(),
            true,
        );
        let mut ctx = ProcessContext::new(time, &mut signals, &mut notes, &connected);
        rg.device.process(&mut ctx);
        last = *ctx.output(d.socket_by_key("out_l").unwrap().id);
    }
    last
}

#[test]
fn a_panic_in_the_plugin_is_caught_and_marks_the_device_failed() {
    let (probe, shared) = Probe::new();
    shared.panic_at(1);
    let mut rg = rig("st.panic", &fx_layout(), Box::new(probe));
    let last = run_unguarded(&mut rg, 4, 0.5);
    assert!(last.iter().all(|s| *s == 0.5), "dry after the panic");
    assert_eq!(shared.calls(), 2, "the plugin is not called again");
    assert!(
        drain_messages(&rg.manager)
            .iter()
            .any(|m| matches!(m, PluginMessage::Failed { .. }))
    );
}

// ----- restart hand-shake -----------------------------------------------------------------------

#[test]
fn a_restart_hands_the_processor_back_and_resumes_with_a_new_one() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Gain(0.5));
    let (mut h, mut r) = harness(rig("st.restart", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [1.0; BATCH];
    *h.input_mut("in_r") = [1.0; BATCH];
    h.set_param("p1", -3.0);
    h.set_param("p3", 20.0);
    for _ in 0..3 {
        h.process();
    }
    assert_eq!(shared.calls(), 3);
    r.manager.request_stop();
    h.process();
    // Handed back at the batch boundary, stopped on the audio thread, dry in the meantime.
    let old = r.manager.take_processor().expect("the processor came back");
    assert_eq!(shared.log.lock().unwrap().stops, 1);
    assert_eq!(shared.calls(), 3);
    assert!(
        h.output("out_l").iter().all(|s| *s == 1.0),
        "dry while restarting"
    );
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 4.0, "restarting");
    for _ in 0..4 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 1.0));
    }
    assert_eq!(
        shared.calls(),
        3,
        "nobody processes while the manager holds it"
    );
    // The manager reactivates and sends a processor back; the device resumes at the next batch
    // and the rack-side params are sent again (state restore first, then params).
    let (probe2, shared2) = Probe::new();
    shared2.set_out(ProbeOut::Gain(0.25));
    assert!(r.manager.resume(Box::new(probe2)).is_ok());
    h.process();
    assert_eq!(shared2.calls(), 1);
    assert_eq!(shared2.log.lock().unwrap().starts, 1);
    assert_eq!(h.output("out_l")[0], 0.25);
    let first = shared2.log.lock().unwrap().events_of_call(0).to_vec();
    let ids: Vec<u32> = first
        .iter()
        .filter_map(|e| match e.kind {
            K::Param { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![1, 3], "every param that was set is sent again");
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 0.0, "running again");
    drop(old);
}

#[test]
fn a_failed_device_is_reloaded_through_the_same_hand_shake() {
    let (probe, shared) = Probe::new();
    shared.set_status(ProcessStatus::Error);
    let (mut h, mut r) = harness(rig("st.reload", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.5; BATCH];
    h.process();
    r.manager.request_stop();
    h.process();
    let old = r
        .manager
        .take_processor()
        .expect("a failed device hands it back too");
    let (fresh, fresh_shared) = Probe::new();
    fresh_shared.set_out(ProbeOut::Constant(0.125));
    r.manager.resume(Box::new(fresh)).ok().unwrap();
    h.process();
    assert_eq!(h.output("out_l")[0], 0.125);
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 0.0, "no longer failed");
    drop(old);
}

#[test]
fn stopping_a_device_that_has_no_processor_is_harmless() {
    let (probe, _shared) = Probe::new();
    let (mut h, mut r) = harness(rig("st.stop-twice", &fx_layout(), Box::new(probe)));
    h.process();
    r.manager.request_stop();
    h.process();
    h.process();
    assert!(r.manager.take_processor().is_some());
    assert!(r.manager.take_processor().is_none());
}

// ----- latency, pinning, run mode, prepare ---------------------------------------------------------

#[test]
fn latency_follows_the_slot_and_is_clamped_to_one_second() {
    let (probe, _shared) = Probe::new();
    let (h, r) = harness(rig("st.latency", &fx_layout(), Box::new(probe)));
    assert_eq!(h.device().latency_frames(), 0);
    assert!(!r.manager.slot().set_latency(256, SR));
    assert_eq!(h.device().latency_frames(), 256);
    assert!(r.manager.slot().set_latency(10_000_000, SR), "clamped");
    assert_eq!(h.device().latency_frames(), 48_000);
    assert_eq!(r.manager.slot().latency(), 48_000);
}

#[test]
fn pinning_follows_the_slot_at_run_time() {
    let (probe, _shared) = Probe::new();
    let (h, r) = harness(rig("st.pin", &fx_layout(), Box::new(probe)));
    assert!(!h.device().pinned_to_audio_thread());
    r.manager.slot().set_pinned(true);
    assert!(h.device().pinned_to_audio_thread());
    r.manager.slot().set_pinned(false);
    assert!(!h.device().pinned_to_audio_thread());
}

#[test]
fn run_mode_is_recorded_and_defaults_to_realtime() {
    let (probe, _shared) = Probe::new();
    let mut rg = rig("st.mode", &fx_layout(), Box::new(probe));
    assert_eq!(rg.device.run_mode(), RunMode::Realtime);
    rg.device.set_run_mode(RunMode::Offline);
    assert_eq!(rg.device.run_mode(), RunMode::Offline);
}

#[test]
fn a_sample_rate_other_than_the_activation_rate_requests_a_restart() {
    let (probe, _shared) = Probe::new();
    let Rig {
        device, manager, ..
    } = rig("st.rate", &fx_layout(), Box::new(probe));
    let _h = atomik_testkit::harness::DeviceHarness::new(Box::new(device), 44_100.0);
    let msgs = drain_messages(&manager);
    assert!(
        msgs.iter()
            .any(|m| matches!(m, PluginMessage::RequestRestart)),
        "{msgs:?}"
    );
    // The same rate asks for nothing.
    let (probe, _shared) = Probe::new();
    let Rig {
        device, manager, ..
    } = rig("st.rate2", &fx_layout(), Box::new(probe));
    let _h = atomik_testkit::harness::DeviceHarness::new(Box::new(device), SR);
    assert!(drain_messages(&manager).is_empty());
}

#[test]
fn reset_resets_the_plugin_and_forgets_open_notes() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("st.reset", &synth_layout(), Box::new(probe)));
    h.push_note("notes_in", on(0, 60));
    h.process();
    h.device_mut().reset();
    assert_eq!(shared.log.lock().unwrap().resets, 1);
    h.push_note("notes_in", off(0, 60));
    h.process();
    assert!(
        shared.log.lock().unwrap().events_of_call(1).is_empty(),
        "the Off of a note forgotten by reset is dropped"
    );
}

// ----- the reaper ---------------------------------------------------------------------------------

struct Witness {
    inner: Probe,
    dropped_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
}

impl PluginProcessor for Witness {
    fn start_processing(&mut self) -> bool {
        self.inner.start_processing()
    }
    fn stop_processing(&mut self) {
        self.inner.stop_processing();
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.inner.process(io)
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        *self.dropped_on.lock().unwrap() = Some(std::thread::current().id());
    }
}

#[test]
fn dropping_a_device_posts_the_processor_to_the_reaper_instead_of_destroying_it() {
    let (probe, _shared) = Probe::new();
    let witness = Arc::new(Mutex::new(None));
    let Rig {
        device,
        reaper,
        manager: _manager,
        ..
    } = rig(
        "st.reaper",
        &fx_layout(),
        Box::new(Witness {
            inner: probe,
            dropped_on: Arc::clone(&witness),
        }),
    );
    // The engine drops retired devices on some non-audio thread.
    let dropper = std::thread::spawn(move || {
        drop(device);
        std::thread::current().id()
    });
    let dropper_id = dropper.join().unwrap();
    assert!(
        witness.lock().unwrap().is_none(),
        "the processor survived the drop of the device"
    );
    let items = reaper.drain();
    assert_eq!(items.len(), 1);
    drop(items);
    let destroyed_on = witness
        .lock()
        .unwrap()
        .expect("destroyed by the reaper's owner");
    assert_eq!(destroyed_on, std::thread::current().id());
    assert_ne!(destroyed_on, dropper_id);
    assert!(reaper.drain().is_empty());
}

#[test]
fn a_device_that_gave_its_processor_back_still_tells_the_reaper_it_is_gone() {
    let (probe, _shared) = Probe::new();
    let (mut h, mut r) = harness(rig("st.reaper2", &fx_layout(), Box::new(probe)));
    h.process();
    r.manager.request_stop();
    h.process();
    let p = r.manager.take_processor().unwrap();
    drop(h);
    // The manager owns the processor; the item has none, but it tells the manager that the
    // device is gone so it can tear the instance down ().
    let items = r.reaper.drain();
    assert_eq!(items.len(), 1);
    assert!(items[0].processor.is_none());
    assert_eq!(items[0].slot.id(), atomik_plugin_host::InstanceId(1));
    drop(p);
}

#[test]
fn the_message_type_is_plain_data() {
    let m = PluginMessage::ParamEdit {
        id: 3,
        value: 0.5,
        phase: atomik_plugin_host::ParamPhase::Begin,
    };
    assert_eq!(m.clone(), m);
    let _ = PluginEvent {
        offset: 0,
        kind: K::Param { id: 0, value: 0.0 },
    };
}
