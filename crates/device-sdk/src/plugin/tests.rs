//! (SDK half): descriptor building, interning, the stand-in device and the plain data.
// Justification: ranges and defaults are exact values copied through f64 -> f32.
#![allow(clippy::float_cmp)]

use super::*;
use crate::{Device, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketKind};
use atomik_core::{BATCH, NoteBuffer, SampleRate, Tempo, TimeInfo};

fn param(id: u32, name: &str) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: name.to_string(),
        module: "Main".to_string(),
        min: 0.0,
        max: 1.0,
        default: 0.5,
        stepped: false,
        steps: 0,
    }
}

fn layout(kind_hint: PluginKindHint) -> PluginLayout {
    PluginLayout {
        name: "Test".to_string(),
        vendor: "Atomik".to_string(),
        version: "1.0".to_string(),
        kind_hint,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: false,
        notes_out: false,
        params: vec![param(42, "Gain"), param(7, "Mix")],
    }
}

fn clap(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

const VST3_ID: &str = "0123456789abcdef0123456789abcdef";

fn vst3() -> PluginRef {
    PluginRef {
        format: PluginFormat::Vst3,
        id: VST3_ID.to_string(),
    }
}

fn keys(d: &DeviceDescriptor) -> Vec<&'static str> {
    d.sockets.iter().map(|s| s.key).collect()
}

fn synth_layout() -> PluginLayout {
    PluginLayout {
        main_in: None,
        notes_in: true,
        ..layout(PluginKindHint::Instrument)
    }
}

#[test]
fn type_ids_are_prefixed_by_format_and_round_trip() {
    assert_eq!(
        clap("org.surge-synth-team.surge-xt").type_id(),
        "clap.org.surge-synth-team.surge-xt"
    );
    assert_eq!(vst3().type_id(), format!("vst3.{VST3_ID}"));
    assert_eq!(
        PluginRef::from_type_id("clap.a.b"),
        Some(clap("a.b")),
        "the id may contain dots"
    );
    assert_eq!(
        PluginRef::from_type_id(&format!("vst3.{VST3_ID}")),
        Some(vst3())
    );
    assert_eq!(PluginRef::from_type_id("atomik.reverb"), None);
    assert_eq!(PluginRef::from_type_id("clap."), None);
    assert!(is_plugin_type_id("clap.x"));
    assert!(is_plugin_type_id("vst3.x"));
    assert!(!is_plugin_type_id("atomik.x"));
    assert!(!is_plugin_type_id("clapper"));
}

#[test]
fn an_instrument_descriptor_has_the_sockets_of_the_rule_in_order() {
    let d = descriptor_for(&clap("x.synth"), &synth_layout()).unwrap();
    assert_eq!(d.type_id, "clap.x.synth");
    assert_eq!(d.name, "Test");
    assert_eq!(d.kind, DeviceKind::Instrument);
    assert_eq!(d.version, 1);
    assert!(d.stages.is_empty());
    assert_eq!(keys(d), ["notes_in", "out_l", "out_r"]);
    assert_eq!(d.sockets[0].kind, SocketKind::Notes);
    assert_eq!(d.sockets[0].direction, Direction::Input);
    assert_eq!(d.sockets[1].kind, SocketKind::Audio);
    assert_eq!(d.sockets[1].direction, Direction::Output);
    assert_eq!(d.validate(), Ok(()));
}

#[test]
fn an_effect_with_a_sidechain_uses_the_compressor_socket_names() {
    let l = PluginLayout {
        sidechain: true,
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&clap("x.fx"), &l).unwrap();
    assert_eq!(d.kind, DeviceKind::Effect);
    assert_eq!(
        keys(d),
        ["in_l", "in_r", "sc_in_l", "sc_in_r", "out_l", "out_r"]
    );
    assert_eq!(d.sockets[2].name, "Sidechain L");
    assert!(
        d.sockets[..4]
            .iter()
            .all(|s| s.direction == Direction::Input)
    );
    assert_eq!(d.validate(), Ok(()));
}

#[test]
fn the_full_socket_order_is_pinned() {
    let l = PluginLayout {
        sidechain: true,
        notes_in: true,
        notes_out: true,
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&clap("x.all"), &l).unwrap();
    assert_eq!(
        keys(d),
        [
            "in_l",
            "in_r",
            "sc_in_l",
            "sc_in_r",
            "notes_in",
            "out_l",
            "out_r",
            "notes_out"
        ]
    );
    for (i, s) in d.sockets.iter().enumerate() {
        assert_eq!(usize::from(s.id.0), i);
    }
}

#[test]
fn a_note_effect_is_a_utility_and_an_instrument_without_notes_is_an_effect() {
    let note_fx = PluginLayout {
        main_in: None,
        main_out: None,
        notes_in: true,
        notes_out: true,
        ..layout(PluginKindHint::NoteEffect)
    };
    let d = descriptor_for(&clap("x.arp"), &note_fx).unwrap();
    assert_eq!(d.kind, DeviceKind::Utility);
    assert_eq!(keys(d), ["notes_in", "notes_out"]);

    let odd = PluginLayout {
        main_in: None,
        notes_in: false,
        ..layout(PluginKindHint::Instrument)
    };
    assert_eq!(
        descriptor_for(&clap("x.odd"), &odd).unwrap().kind,
        DeviceKind::Effect,
        "an instrument that takes no notes cannot be played"
    );

    // An effect that also takes notes stays an effect.
    let fx_notes = PluginLayout {
        notes_in: true,
        ..layout(PluginKindHint::Effect)
    };
    assert_eq!(
        descriptor_for(&clap("x.fxn"), &fx_notes).unwrap().kind,
        DeviceKind::Effect
    );
}

#[test]
fn bypass_comes_first_then_the_plugin_params_in_the_plugins_own_order() {
    let d = descriptor_for(&clap("x.order"), &layout(PluginKindHint::Effect)).unwrap();
    let k: Vec<_> = d.params.iter().map(|p| p.key).collect();
    assert_eq!(k, ["bypass", "p42", "p7"]);
    for (i, p) in d.params.iter().enumerate() {
        assert_eq!(p.id.0 as usize, i, "ParamId is the index");
    }
    let b = &d.params[0];
    assert_eq!(b.name, "Bypass");
    assert!(b.discrete);
    assert_eq!(b.min.to_bits(), 0.0f32.to_bits());
    assert_eq!(b.max.to_bits(), 1.0f32.to_bits());
    assert_eq!(b.default.to_bits(), 0.0f32.to_bits());
    assert_eq!(b.unit, atomik_core::Unit::Toggle);
    assert_eq!(d.params[1].name, "Gain");
    assert_eq!(d.params[1].taper, atomik_core::Taper::Linear);
    assert_eq!(d.validate(), Ok(()));
}

fn range(p: &atomik_core::ParamDescriptor) -> (f32, f32, f32, bool) {
    (p.min, p.max, p.default, p.discrete)
}

#[test]
fn clap_params_use_the_plugins_plain_range_and_stepped_means_discrete() {
    let l = PluginLayout {
        params: vec![
            PluginParamInfo {
                min: -24.0,
                max: 12.0,
                default: 0.0,
                ..param(1, "Gain")
            },
            PluginParamInfo {
                min: 2.0,
                max: 9.0,
                default: 4.0,
                stepped: true,
                steps: 8,
                ..param(2, "Voices")
            },
        ],
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&clap("x.clapmap"), &l).unwrap();
    assert_eq!(
        range(d.param_by_key("p1").unwrap()),
        (-24.0, 12.0, 0.0, false)
    );
    assert_eq!(range(d.param_by_key("p2").unwrap()), (2.0, 9.0, 4.0, true));
}

#[test]
fn vst3_params_are_normalised_and_stepped_ones_are_step_indices() {
    let l = PluginLayout {
        params: vec![
            PluginParamInfo {
                // A scanner reports the controller's own numbers; the host maps by rule.
                min: -5.0,
                max: 5.0,
                default: 0.25,
                ..param(10, "Cutoff")
            },
            PluginParamInfo {
                default: 0.5,
                stepped: true,
                steps: 4,
                ..param(11, "Mode")
            },
        ],
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&vst3(), &l).unwrap();
    assert_eq!(
        range(d.param_by_key("p10").unwrap()),
        (0.0, 1.0, 0.25, false)
    );
    assert_eq!(range(d.param_by_key("p11").unwrap()), (0.0, 4.0, 2.0, true));
}

#[test]
fn a_default_outside_the_range_is_clamped_and_nan_becomes_the_minimum() {
    let l = PluginLayout {
        params: vec![
            PluginParamInfo {
                default: 9.0,
                ..param(1, "High")
            },
            PluginParamInfo {
                default: f64::NAN,
                min: 0.25,
                ..param(2, "Nan")
            },
        ],
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&clap("x.clamp"), &l).unwrap();
    assert_eq!(
        d.param_by_key("p1").unwrap().default.to_bits(),
        1.0f32.to_bits()
    );
    assert_eq!(
        d.param_by_key("p2").unwrap().default.to_bits(),
        0.25f32.to_bits()
    );
    assert_eq!(d.validate(), Ok(()));
}

#[test]
fn bad_layouts_are_refused_with_a_reason() {
    let dup = PluginLayout {
        params: vec![param(5, "A"), param(5, "B")],
        ..layout(PluginKindHint::Effect)
    };
    assert_eq!(
        descriptor_for(&clap("x.dup"), &dup),
        Err(DescriptorError::DuplicatePluginParam(5))
    );
    let inf = PluginLayout {
        params: vec![PluginParamInfo {
            max: f64::INFINITY,
            ..param(6, "Inf")
        }],
        ..layout(PluginKindHint::Effect)
    };
    assert_eq!(
        descriptor_for(&clap("x.inf"), &inf),
        Err(DescriptorError::InvalidPluginParam(6))
    );
    let inverted = PluginLayout {
        params: vec![PluginParamInfo {
            min: 2.0,
            max: 1.0,
            ..param(8, "Inv")
        }],
        ..layout(PluginKindHint::Effect)
    };
    assert_eq!(
        descriptor_for(&clap("x.inv"), &inverted),
        Err(DescriptorError::InvalidPluginParam(8))
    );
    assert!(matches!(
        descriptor_for(&clap(""), &layout(PluginKindHint::Effect)),
        Err(DescriptorError::InvalidPluginId(_))
    ));
    assert!(matches!(
        descriptor_for(&clap("has space"), &layout(PluginKindHint::Effect)),
        Err(DescriptorError::InvalidPluginId(_))
    ));
    for bad in [
        "",
        "ABCDEF0123456789ABCDEF0123456789",
        "0123",
        &"g".repeat(32),
    ] {
        let r = PluginRef {
            format: PluginFormat::Vst3,
            id: bad.to_string(),
        };
        assert!(
            matches!(
                descriptor_for(&r, &layout(PluginKindHint::Effect)),
                Err(DescriptorError::InvalidPluginId(_))
            ),
            "{bad}"
        );
    }
    let no_audio_no_notes = PluginLayout {
        main_in: None,
        main_out: None,
        ..layout(PluginKindHint::Effect)
    };
    assert_eq!(
        descriptor_for(&clap("x.empty"), &no_audio_no_notes),
        Err(DescriptorError::NoSockets)
    );
}

#[test]
fn the_param_cap_refuses_a_longer_table() {
    let mut l = layout(PluginKindHint::Effect);
    l.params = (0..MAX_PLUGIN_PARAMS as u32)
        .map(|i| param(i, "P"))
        .collect();
    let ok = descriptor_for(&clap("x.cap-ok"), &l).unwrap();
    assert_eq!(
        ok.params.len(),
        MAX_PLUGIN_PARAMS + 1,
        "bypass plus the cap"
    );
    l.params.push(param(99_999, "One too many"));
    assert_eq!(
        descriptor_for(&clap("x.cap-over"), &l),
        Err(DescriptorError::TooManyParams {
            count: MAX_PLUGIN_PARAMS + 1,
            max: MAX_PLUGIN_PARAMS
        })
    );
}

#[test]
fn the_same_layout_gives_the_same_pointer_and_another_layout_another_one() {
    let a = descriptor_for(&clap("x.intern"), &layout(PluginKindHint::Effect)).unwrap();
    let b = descriptor_for(&clap("x.intern"), &layout(PluginKindHint::Effect)).unwrap();
    assert!(std::ptr::eq(a, b));
    let mut other = layout(PluginKindHint::Effect);
    other.params.push(param(100, "More"));
    let c = descriptor_for(&clap("x.intern"), &other).unwrap();
    assert!(!std::ptr::eq(a, c));
    assert_eq!(c.params.len(), a.params.len() + 1);
    let d = descriptor_for(&clap("x.intern2"), &layout(PluginKindHint::Effect)).unwrap();
    assert!(!std::ptr::eq(a, d), "another plugin, another descriptor");
    let e = descriptor_for(&clap("x.intern"), &layout(PluginKindHint::Effect)).unwrap();
    assert!(std::ptr::eq(a, e));
}

#[test]
fn the_intern_cap_refuses_new_layouts_but_keeps_serving_known_ones() {
    let interner = Interner::with_limit(3);
    let l = layout(PluginKindHint::Effect);
    let first = interner.descriptor_for(&clap("a"), &l).unwrap();
    interner.descriptor_for(&clap("b"), &l).unwrap();
    interner.descriptor_for(&clap("c"), &l).unwrap();
    assert_eq!(interner.len(), 3);
    assert_eq!(
        interner.descriptor_for(&clap("d"), &l),
        Err(DescriptorError::InternLimit { max: 3 })
    );
    assert_eq!(interner.len(), 3, "a refused layout is not counted");
    assert!(std::ptr::eq(
        first,
        interner.descriptor_for(&clap("a"), &l).unwrap()
    ));
    assert_eq!(MAX_INTERNED_DESCRIPTORS, 4_096);
}

#[test]
fn the_layout_hash_is_deterministic_and_sensitive_to_every_field() {
    let base = layout(PluginKindHint::Effect);
    assert_eq!(base.layout_hash(), base.clone().layout_hash());
    let edits: [fn(&mut PluginLayout); 19] = [
        |l| l.name.push('x'),
        |l| l.vendor.push('x'),
        |l| l.version.push('x'),
        |l| l.kind_hint = PluginKindHint::Instrument,
        |l| l.main_in = Some(Channels::Mono),
        |l| l.main_out = None,
        |l| l.sidechain = true,
        |l| l.notes_in = true,
        |l| l.notes_out = true,
        |l| l.params[0].id = 1,
        |l| l.params[0].name.push('x'),
        |l| l.params[0].module.push('x'),
        |l| l.params[0].min = -1.0,
        |l| l.params[0].max = 2.0,
        |l| l.params[0].default = 0.1,
        |l| l.params[0].stepped = true,
        |l| l.params[0].steps = 3,
        |l| l.params.swap(0, 1),
        |l| l.params.truncate(1),
    ];
    let mut hashes: Vec<u64> = edits
        .iter()
        .map(|f| {
            let mut v = base.clone();
            f(&mut v);
            v.layout_hash()
        })
        .collect();
    hashes.push(base.layout_hash());
    hashes.sort_unstable();
    let n = hashes.len();
    hashes.dedup();
    assert_eq!(hashes.len(), n, "every change moves the hash");
}

#[test]
fn state_blob_is_plain_bytes() {
    let b = PluginStateBlob(vec![1, 2, 3]);
    assert_eq!(b.0.len(), 3);
    assert_eq!(b.clone(), b);
    assert_eq!(PluginFormat::Clap.name(), "clap");
    assert_eq!(PluginFormat::Vst3.name(), "vst3");
}

// ----- stand-in --------------------------------------------------------------------------

fn time() -> TimeInfo {
    TimeInfo::at(
        0,
        SampleRate::new(48_000.0).unwrap(),
        Tempo::new(120.0).unwrap(),
        true,
    )
}

fn run(dev: &mut dyn Device, input: f32) -> (Vec<[f32; BATCH]>, Vec<NoteBuffer>) {
    let d = dev.descriptor();
    let n = d.sockets.len();
    let mut signals = vec![[0.0; BATCH]; n];
    let mut notes: Vec<NoteBuffer> = d
        .sockets
        .iter()
        .map(|s| NoteBuffer::with_capacity(if s.kind == SocketKind::Notes { 16 } else { 0 }))
        .collect();
    for (i, s) in d.sockets.iter().enumerate() {
        if s.kind == SocketKind::Audio && s.direction.is_input() {
            signals[i] = [input; BATCH];
        }
    }
    let connected = vec![true; n];
    let mut ctx = ProcessContext::new(time(), &mut signals, &mut notes, &connected);
    dev.process(&mut ctx);
    (signals, notes)
}

fn idx(d: &DeviceDescriptor, k: &str) -> usize {
    usize::from(d.socket_by_key(k).unwrap().id.0)
}

#[test]
fn the_stand_in_of_an_effect_passes_the_dry_main_input_through() {
    let l = PluginLayout {
        sidechain: true,
        ..layout(PluginKindHint::Effect)
    };
    let d = descriptor_for(&clap("x.standin-fx"), &l).unwrap();
    let mut dev = PluginStandIn::new(d);
    dev.prepare(&PrepareInfo {
        sample_rate: SampleRate::new(48_000.0).unwrap(),
        seed: 1,
    });
    dev.set_param(atomik_core::ParamId(1), 0.3);
    let (out, _) = run(&mut dev, 0.25);
    assert!(
        out[idx(d, "out_l")]
            .iter()
            .all(|s| s.to_bits() == 0.25f32.to_bits())
    );
    assert!(
        out[idx(d, "out_r")]
            .iter()
            .all(|s| s.to_bits() == 0.25f32.to_bits())
    );
    assert_eq!(dev.latency_frames(), 0);
    assert!(!dev.pinned_to_audio_thread());
    assert_eq!(dev.descriptor().type_id, "clap.x.standin-fx");
}

#[test]
fn the_stand_in_of_an_instrument_is_silent() {
    let d = descriptor_for(&clap("x.standin-synth"), &synth_layout()).unwrap();
    let mut dev = PluginStandIn::new(d);
    let (out, _) = run(&mut dev, 0.9);
    assert!(out[idx(d, "out_l")].iter().all(|s| s.to_bits() == 0));
    assert!(out[idx(d, "out_r")].iter().all(|s| s.to_bits() == 0));
}

#[test]
fn the_stand_in_of_a_note_effect_emits_nothing() {
    let l = PluginLayout {
        main_in: None,
        main_out: None,
        notes_in: true,
        notes_out: true,
        ..layout(PluginKindHint::NoteEffect)
    };
    let d = descriptor_for(&clap("x.standin-notes"), &l).unwrap();
    let mut dev = PluginStandIn::new(d);
    let (out, notes) = run(&mut dev, 0.0);
    assert_eq!(out.len(), 2);
    assert!(notes[idx(d, "notes_out")].is_empty());
}

#[test]
fn the_stand_in_owns_no_growable_data() {
    // `process` touches only borrowed slices: a stand-in is a descriptor reference and two
    // socket ids.
    assert!(std::mem::size_of::<PluginStandIn>() <= 32);
}

// ----- the Device additions ---------------------------------------------------------------

#[test]
fn run_mode_defaults_to_a_no_op_and_devices_are_not_pinned() {
    struct Null;
    impl Device for Null {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            &crate::descriptor::tests::GOOD
        }
        fn prepare(&mut self, _: &PrepareInfo) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: atomik_core::ParamId, _: f32) {}
        fn process(&mut self, _: &mut ProcessContext<'_>) {}
    }
    let mut n = Null;
    assert!(!n.pinned_to_audio_thread());
    n.set_run_mode(RunMode::Offline);
    n.set_run_mode(RunMode::Realtime);
    assert_ne!(RunMode::Realtime, RunMode::Offline);
}
