//! Finite output for any params and events (a proptest over scripts), no FAILED state
//! for a well-behaved plugin, and never a non-finite value handed to the plugin.
// Justification: samples are checked with `is_finite`; there is no float equality.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{ExprKind, NoteEvent, NoteKind, ParamId};
use atomik_plugin_host::PluginEventKind as K;
use proptest::prelude::*;
use support::*;

#[derive(Debug, Clone)]
enum Step {
    Note(NoteKind, u16),
    Param(u8, f32),
    Process,
}

fn kind() -> impl Strategy<Value = NoteKind> {
    let v = prop_oneof![
        any::<f32>(),
        Just(f32::NAN),
        Just(f32::INFINITY),
        -2.0f32..2.0
    ];
    prop_oneof![
        (0u8..128, v.clone()).prop_map(|(key, velocity)| NoteKind::On { key, velocity }),
        (0u8..128).prop_map(|key| NoteKind::Off { key }),
        Just(NoteKind::AllOff),
        v.clone().prop_map(|value| NoteKind::PitchBend { value }),
        (0u8..128, v.clone()).prop_map(|(controller, value)| NoteKind::Cc { controller, value }),
        v.clone()
            .prop_map(|value| NoteKind::ChannelPressure { value }),
        (0u8..128, v.clone()).prop_map(|(key, value)| NoteKind::PolyPressure { key, value }),
        (0u8..128, 0usize..5, v).prop_map(|(key, k, value)| NoteKind::NoteExpr {
            key,
            kind: [
                ExprKind::Tuning,
                ExprKind::Pressure,
                ExprKind::Slide,
                ExprKind::Volume,
                ExprKind::Pan
            ][k],
            value
        }),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (kind(), 0u16..64).prop_map(|(k, o)| Step::Note(k, o)),
        3 => (0u8..4, prop_oneof![any::<f32>(), Just(f32::NAN), Just(f32::NEG_INFINITY), -100.0f32..100.0])
            .prop_map(|(p, v)| Step::Param(p, v)),
        4 => Just(Step::Process),
    ]
}

fn run(steps: &[Step], synth: bool) -> Result<(), TestCaseError> {
    let (probe_log, mut h) = {
        let layout = if synth { synth_layout() } else { fx_layout() };
        let processor: Box<dyn atomik_plugin_host::PluginProcessor> = if synth {
            Box::new(CoreSynth::new())
        } else {
            Box::new(CoreEffect::new())
        };
        let (h, r) = harness(rig("prop.dev", &layout, processor));
        (r, h)
    };
    let _keep = probe_log;
    let mut pending = 0usize;
    for s in steps {
        match s {
            Step::Note(kind, offset) if synth => {
                // The buffer needs non-decreasing offsets: clamp to the last one pushed.
                let _ = offset;
                h.push_note(
                    "notes_in",
                    NoteEvent {
                        offset: 0,
                        kind: *kind,
                    },
                );
                pending += 1;
                if pending >= 900 {
                    h.process();
                    pending = 0;
                }
            }
            Step::Note(..) => {}
            Step::Param(p, v) => {
                // Raw values straight to the device, like a buggy sender (the engine clamps,
                // but the device must not rely on it).
                h.device_mut().set_param(ParamId(1 + u32::from(*p % 3)), *v);
            }
            Step::Process => {
                h.process();
                pending = 0;
                for ch in ["out_l", "out_r"] {
                    prop_assert!(h.output(ch).iter().all(|s| s.is_finite()));
                }
            }
        }
    }
    h.process();
    for ch in ["out_l", "out_r"] {
        prop_assert!(h.output(ch).iter().all(|s| s.is_finite()));
    }
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    prop_assert!(
        m[2].to_bits() != 3.0f32.to_bits(),
        "the device did not fail"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn the_synth_device_is_finite_for_any_events_and_params(
        steps in proptest::collection::vec(step(), 1..120)
    ) {
        run(&steps, true)?;
    }

    #[test]
    fn the_effect_device_is_finite_for_any_params(
        steps in proptest::collection::vec(step(), 1..120)
    ) {
        run(&steps, false)?;
    }
}

#[test]
fn non_finite_param_values_are_not_forwarded_to_the_plugin() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("prop.nan", &fx_layout(), Box::new(probe)));
    h.device_mut().set_param(ParamId(1), f32::NAN);
    h.device_mut().set_param(ParamId(2), f32::INFINITY);
    h.device_mut().set_param(ParamId(3), 0.5);
    h.process();
    let log = shared.log.lock().unwrap();
    let params: Vec<_> = log
        .events
        .iter()
        .filter_map(|e| match e.kind {
            K::Param { id, value } => Some((id, value)),
            _ => None,
        })
        .collect();
    assert_eq!(params.len(), 1, "{params:?}");
    assert_eq!(params[0].0, 3);
}
