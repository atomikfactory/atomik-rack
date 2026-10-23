//! The built-in `null` plugins through the real child: create, activate, process, the ops,
//! state, text and latency (AC-4 child side, AC-9, AC-10).

mod support;

use atomik_plugin_host::{ProcessStatus, SleepRule, TailLength};
use atomik_plugin_ipc::{
    AudioOp, FromChild, ProcessorInfo, ToChild, WireChannels, WireErrorKind, WireKind,
};
use std::time::Duration;
use support::{SR, Spawn, TestParent, note_off, note_on, param, ramp};

fn started(plugin: &str) -> TestParent {
    let mut p = TestParent::spawn(&Spawn::null(plugin));
    p.hello();
    p
}

fn bits(b: &[f32]) -> Vec<u32> {
    b.iter().map(|s| s.to_bits()).collect()
}

#[test]
fn instantiate_reports_the_layout_of_the_null_gain() {
    let mut p = started("null.gain");
    let (layout, has_gui, latency) = p.instantiate();
    assert_eq!(layout.kind, WireKind::Effect);
    assert_eq!(layout.main_in, Some(WireChannels::Stereo));
    assert_eq!(layout.main_out, Some(WireChannels::Stereo));
    assert!(!layout.sidechain && !layout.notes_in && !layout.notes_out);
    assert!(!has_gui);
    assert_eq!(latency, 0);
    assert_eq!(layout.params.len(), 1);
    let pass = &layout.params[0];
    assert_eq!((pass.id, pass.name.as_str()), (0, "Pass"));
    assert_eq!((pass.min, pass.max, pass.default), (0.0, 1.0, 0.0));
    assert!(pass.stepped);
    assert_eq!(pass.steps, 1);
}

#[test]
fn the_null_gain_halves_its_input_exactly() {
    let mut p = started("null.gain");
    p.instantiate();
    assert_eq!(p.activate(false), 0);
    for round in 0..5 {
        let input = [ramp(1.0 + round as f32), ramp(-0.37)];
        let out = p.process(&input, &[]);
        assert_eq!(out.status, ProcessStatus::Continue);
        for (ch, source) in input.iter().enumerate() {
            let want: Vec<f32> = source.iter().map(|s| s * 0.5).collect();
            assert_eq!(
                bits(&out.main_out[ch]),
                bits(&want),
                "round {round} ch {ch}"
            );
        }
    }
}

#[test]
fn the_pass_param_switches_to_passthrough_at_its_frame_offset() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    let input = [ramp(1.0), ramp(2.0)];
    let out = p.process(&input, &[param(32, 0, 1.0)]);
    for (ch, source) in input.iter().enumerate() {
        for (i, sample) in source.iter().enumerate() {
            let want = if i < 32 { sample * 0.5 } else { *sample };
            assert_eq!(
                out.main_out[ch][i].to_bits(),
                want.to_bits(),
                "ch {ch} frame {i}"
            );
        }
    }
    // The value stays: the next block is passthrough from its first frame.
    let out = p.process(&input, &[]);
    assert_eq!(bits(&out.main_out[0]), bits(&input[0]));
}

#[test]
fn the_ops_start_stop_and_reset_are_answered_in_order() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    assert!(p.op(AudioOp::Start).ok, "start_processing succeeded");
    let out = p.process(&[ramp(1.0), ramp(1.0)], &[]);
    assert_eq!(out.status, ProcessStatus::Continue);
    let r = p.op(AudioOp::Reset);
    assert_eq!(r.status, ProcessStatus::Continue);
    assert!(p.op(AudioOp::Stop).ok);
}

#[test]
fn the_processor_info_is_written_at_activation() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    // The Activated reply comes after the audio thread took the processor over.
    assert_eq!(
        p.port.info(),
        ProcessorInfo {
            sleep_rule: SleepRule::PluginDecides,
            tail: TailLength::Finite(0)
        }
    );
}

#[test]
fn a_request_without_a_processor_is_answered_with_an_error_status() {
    let mut p = started("null.gain");
    p.instantiate();
    let out = p.process(&[ramp(1.0), ramp(1.0)], &[]);
    assert_eq!(out.status, ProcessStatus::Error);
    assert!(!p.op(AudioOp::Start).ok);
    p.activate(false);
    let out = p.process(&[ramp(1.0), ramp(1.0)], &[]);
    assert_eq!(out.status, ProcessStatus::Continue);
    p.deactivate();
    assert_eq!(
        p.process(&[ramp(1.0), ramp(1.0)], &[]).status,
        ProcessStatus::Error,
        "the processor went back to the main thread"
    );
}

#[test]
fn a_deactivated_plugin_can_be_activated_again() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    p.process(&[ramp(1.0), ramp(1.0)], &[]);
    p.deactivate();
    p.activate(true);
    let out = p.process(&[ramp(1.0), ramp(1.0)], &[]);
    assert_eq!(out.status, ProcessStatus::Continue);
}

#[test]
fn activating_twice_or_deactivating_without_activation_is_an_error() {
    let mut p = started("null.gain");
    p.instantiate();
    assert!(matches!(
        p.call(|id| ToChild::Deactivate { id }, None).0,
        FromChild::Err {
            kind: WireErrorKind::Failed,
            ..
        }
    ));
    p.activate(false);
    let (reply, _) = p.call(
        |id| ToChild::Activate {
            id,
            sample_rate: SR,
            frames: 64,
            offline: false,
        },
        None,
    );
    assert!(
        matches!(
            reply,
            FromChild::Err {
                kind: WireErrorKind::Failed,
                ..
            }
        ),
        "{reply:?}"
    );
}

#[test]
fn a_bad_sample_rate_is_refused() {
    let mut p = started("null.gain");
    p.instantiate();
    let (reply, _) = p.call(
        |id| ToChild::Activate {
            id,
            sample_rate: 0.0,
            frames: 64,
            offline: false,
        },
        None,
    );
    assert!(matches!(reply, FromChild::Err { .. }), "{reply:?}");
}

#[test]
fn the_state_round_trips_through_a_new_child() {
    let mut a = started("null.gain");
    a.instantiate();
    a.activate(false);
    a.process(&[ramp(1.0), ramp(1.0)], &[param(0, 0, 1.0)]);
    let (reply, blob) = a.call(|id| ToChild::CaptureState { id }, None);
    assert!(matches!(reply, FromChild::State { .. }), "{reply:?}");
    let blob = blob.expect("the state comes as a blob");
    assert!(!blob.is_empty());

    let mut b = started("null.gain");
    b.instantiate();
    let (reply, _) = b.call(|id| ToChild::RestoreState { id }, Some(&blob));
    assert!(matches!(reply, FromChild::Ok { .. }), "{reply:?}");
    b.activate(false);
    let input = [ramp(1.0), ramp(1.0)];
    let out = b.process(&input, &[]);
    assert_eq!(
        bits(&out.main_out[0]),
        bits(&input[0]),
        "the restored Pass is 1"
    );
    let (_, again) = b.call(|id| ToChild::CaptureState { id }, None);
    assert_eq!(again.unwrap(), blob);
}

#[test]
fn a_state_the_plugin_cannot_read_is_an_error_and_a_missing_blob_too() {
    let mut p = started("null.gain");
    p.instantiate();
    let (reply, _) = p.call(|id| ToChild::RestoreState { id }, Some(&[1, 2, 3]));
    assert!(
        matches!(
            reply,
            FromChild::Err {
                kind: WireErrorKind::Failed,
                ..
            }
        ),
        "{reply:?}"
    );
    let (reply, _) = p.call(|id| ToChild::RestoreState { id }, None);
    assert!(
        matches!(
            reply,
            FromChild::Err {
                kind: WireErrorKind::Failed,
                ..
            }
        ),
        "{reply:?}"
    );
}

#[test]
fn param_text_comes_from_the_plugin() {
    let mut p = started("null.gain");
    p.instantiate();
    let text = |p: &mut TestParent, param: u32, plain: f64| match p
        .call(|id| ToChild::ParamText { id, param, plain }, None)
        .0
    {
        FromChild::Text { text, .. } => text,
        other => panic!("{other:?}"),
    };
    assert_eq!(text(&mut p, 0, 0.0).as_deref(), Some("half"));
    assert_eq!(text(&mut p, 0, 1.0).as_deref(), Some("pass"));
    assert_eq!(text(&mut p, 9, 1.0), None);
}

#[test]
fn the_null_latency_reports_32_frames_everywhere() {
    let mut p = started("null.latency");
    let (_, _, at_instantiate) = p.instantiate();
    assert_eq!(at_instantiate, 32);
    assert!(matches!(
        p.call(|id| ToChild::LatencyFrames { id }, None).0,
        FromChild::Latency { frames: 32, .. }
    ));
    assert_eq!(p.activate(false), 32);
    let input = [ramp(1.0), ramp(-1.0)];
    let out = p.process(&input, &[]);
    assert_eq!(bits(&out.main_out[0]), bits(&input[0]), "a passthrough");
}

#[test]
fn default_pins_and_layout_changes_are_answered() {
    let mut p = started("null.gain");
    p.instantiate();
    assert!(matches!(
        p.call(|id| ToChild::DefaultPins { id }, None).0,
        FromChild::Pins { pins, .. } if pins.is_empty()
    ));
    assert!(matches!(
        p.call(|id| ToChild::TakeLayoutChanged { id }, None).0,
        FromChild::LayoutChanged { layout: None, .. }
    ));
}

#[test]
fn the_null_synth_plays_a_deterministic_note_until_it_is_released() {
    let run = || {
        let mut p = started("null.synth");
        let (layout, _, _) = p.instantiate();
        assert_eq!(layout.kind, WireKind::Instrument);
        assert_eq!(layout.main_in, None);
        assert_eq!(layout.main_out, Some(WireChannels::Stereo));
        assert!(layout.notes_in);
        p.activate(false);
        let silent = p.process(&[], &[]);
        assert!(silent.main_out[0].iter().all(|s| *s == 0.0));
        let playing = p.process(&[], &[note_on(8, 69, 1.0, 1)]);
        assert!(playing.main_out[0][..8].iter().all(|s| *s == 0.0));
        assert!(playing.main_out[0][8..].iter().any(|s| s.abs() > 0.01));
        assert_eq!(bits(&playing.main_out[0]), bits(&playing.main_out[1]));
        let held = p.process(&[], &[]);
        assert!(
            held.main_out[0].iter().any(|s| s.abs() > 0.01),
            "the note is held"
        );
        let released = p.process(&[], &[note_off(16, 69, 1)]);
        assert!(released.main_out[0][16..].iter().all(|s| *s == 0.0));
        let after = p.process(&[], &[]);
        assert!(after.main_out[0].iter().all(|s| *s == 0.0));
        (bits(&playing.main_out[0]), bits(&held.main_out[0]))
    };
    assert_eq!(run(), run(), "two children make the same bits");
}

#[test]
fn a_closed_stdin_with_an_active_plugin_ends_the_child_with_exit_0_within_a_second() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    p.process(&[ramp(1.0), ramp(1.0)], &[]);
    p.close_stdin();
    let status = p
        .wait_exit(Duration::from_secs(1))
        .expect("it ends in time");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn shutdown_with_an_active_plugin_ends_the_child_with_exit_0() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    p.send(&ToChild::Shutdown);
    let status = p
        .wait_exit(Duration::from_secs(1))
        .expect("it ends in time");
    assert_eq!(status.code(), Some(0));
}

/// Not a gate: prints the median and the worst round trip of one block through the child.
/// `cargo test -p atomik-plugin-hostd --release -- --ignored --nocapture round_trip`
#[test]
#[ignore = "a measurement, not a check"]
// Justification: this measures wall-clock time on purpose; nothing in the signal path reads it.
#[allow(clippy::disallowed_methods)]
fn the_round_trip_of_one_block_is_measured() {
    let mut p = started("null.gain");
    p.instantiate();
    p.activate(false);
    let input = [ramp(1.0), ramp(-1.0)];
    let mut times = Vec::new();
    for _ in 0..4000 {
        let start = std::time::Instant::now();
        p.process(&input, &[]);
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "round trip: median {:?}, 99th {:?}, worst {:?}",
        times[times.len() / 2],
        times[times.len() * 99 / 100],
        times[times.len() - 1]
    );
}
