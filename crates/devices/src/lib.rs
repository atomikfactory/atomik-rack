//! Atomik Rack built-in devices: [`poly_synth`], [`mono_bass`], [`dual_layer`], [`semi_modular`], [`organ`], [`wavetable_synth`], [`fm_synth`], [`sampler`], [`granular`], [`drum_machine`], [`loop_slicer`], [`line_mixer`], [`stereo_delay`], [`echo`], [`reverb`], the effects
//! [`compressor`], [`eq`], [`distortion`], [`guitar_amp`], [`modulation`] and [`mastering`], the analysers
//! [`spectrum`] and [`phase_meter`] (effects that pass the audio through and publish what they see), and the
//! console devices [`channel`] (`atomik.channel`), [`bus`] (`atomik.bus`, the channel strip as an output bus) and [`master`] (`atomik.master`).
//!
//! The utility devices [`gain_tool`], [`stereo_tool`], [`feedback`] (the delay stage that makes a loop legal), [`gate`], [`dynamics`] (gate then
//! compressor), [`duck`] (the sidechain tool), [`env_filter`], [`auto_filter`], [`transient_shaper`], [`de_esser`], [`tuner`], [`pitch_shifter`], [`rotary_speaker`], the
//! CV utilities [`cv_math`], [`cv_quantizer`], [`sample_hold`], [`slew_limiter`], [`gate_processor`] and
//! [`envelope_follower`], and the two cable mergers in [`spider`] share the descriptor builders of the private `tool_support` module.
//!
//! The effects are wrapped in [`bypass::Bypassable`], which adds their `bypass` param.
//!
//! Each module exposes a `pub static DESCRIPTOR`, a `pub fn factory()` and the device struct.
//! All three are original designs.
//! `process`, `reset` and `set_param` never allocate, lock or block; any randomness comes
//! from [`atomik_dsp::Rng`] seeded by `PrepareInfo::seed`.

pub mod arpeggiator;
pub mod audio_track;
pub mod auto_filter;
pub mod bus;
pub mod bypass;
mod chain_clock;
pub mod channel;
pub mod character;
pub mod clipper;
pub mod compressor;
pub mod container;
pub mod controller_state;
pub mod convolver;
mod cv_in;
pub mod cv_math;
pub mod cv_quantizer;
pub mod cv_sequencer;
pub mod de_esser;
pub mod distortion;
pub mod drum_generator;
pub mod drum_machine;
pub mod dual_layer;
pub mod duck;
pub mod dynamics;
pub mod echo;
pub mod env_filter;
pub mod envelope_follower;
pub mod eq;
pub mod feedback;
pub mod fm_synth;
pub mod gain_tool;
pub mod gate;
pub mod gate_processor;
pub mod granular;
pub mod guitar_amp;
pub mod key_chord;
pub mod line_mixer;
pub mod loop_slicer;
pub mod macro_bank;
pub mod master;
pub mod mastering;
pub mod midi_out;
pub mod mod_source;
pub mod modulation;
pub mod mono_bass;
mod note_clock;
pub mod note_delay;
pub mod note_filter;
mod note_fx;
mod note_queue;
pub mod note_repeater;
mod note_shape;
pub mod note_split;
pub mod organ;
pub mod phase_meter;
pub mod pitch_shifter;
pub mod poly_synth;
mod recent_peak;
pub mod reverb;
pub mod rotary_speaker;
pub mod sample_hold;
pub mod sampler;
pub mod semi_modular;
pub mod slew_limiter;
pub mod space_reverb;
pub mod spectrum;
pub mod spider;
pub mod step_sequencer;
pub mod stereo_delay;
pub mod stereo_tool;
mod tool_support;
pub mod transient_shaper;
pub mod tuner;
pub mod vocoder;
pub mod wavetable_synth;

use atomik_device_sdk::{Registry, RegistryError};

#[cfg(test)]
atomik_testkit::install_alloc_guard!();

#[cfg(test)]
mod bounded_tests;
#[cfg(test)]
mod midi_chart;
#[cfg(test)]
mod mod_matrix_tests;
#[cfg(test)]
mod test_support;
/// Hand-made panel layout for a built-in device type, or `None` to use the
/// layout generated from the descriptor. Layouts are added as the UI matures.
pub fn panel_layout(type_id: &str) -> Option<&'static atomik_device_sdk::PanelLayout> {
    let _ = type_id;
    None
}

/// Type ids of the note players (step sequencer, arpeggiator). They are `DeviceKind::Utility`; a
/// browser that groups by kind can list these under a "Players" heading with this hook.
pub const PLAYER_TYPE_IDS: [&str; 8] = [
    "atomik.arpeggiator",
    "atomik.cv-sequencer",
    "atomik.drum-generator",
    "atomik.key-chord",
    "atomik.note-delay",
    "atomik.note-filter",
    "atomik.note-repeater",
    "atomik.step-sequencer",
];

/// Whether `type_id` is a note player (see [`PLAYER_TYPE_IDS`]).
pub fn is_player(type_id: &str) -> bool {
    PLAYER_TYPE_IDS.contains(&type_id)
}

/// Type ids of the modulators (CV sources). They are `DeviceKind::Utility`; the browser lists them
/// under a "Modulators" heading, and future CV sources join the list.
pub const MODULATOR_TYPE_IDS: [&str; 1] = ["atomik.mod-source"];

/// Whether `type_id` is a modulator (see [`MODULATOR_TYPE_IDS`]).
pub fn is_modulator(type_id: &str) -> bool {
    MODULATOR_TYPE_IDS.contains(&type_id)
}

/// Type ids of the internal devices: created by the model's container expansion, never shown in
/// the browser or offered by an add menu, and refused when added by hand.
pub const INTERNAL_TYPE_IDS: [&str; 1] = ["atomik.note-split"];

/// Whether `type_id` is internal (see [`INTERNAL_TYPE_IDS`]).
pub fn is_internal(type_id: &str) -> bool {
    INTERNAL_TYPE_IDS.contains(&type_id)
}

/// Registers every built-in device factory.
pub fn register_all(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register(arpeggiator::factory())?;
    registry.register(audio_track::factory())?;
    registry.register_with_mods(poly_synth::factory(), &poly_synth::MOD_TABLES)?;
    registry.register_with_mods(mono_bass::factory(), &poly_synth::MOD_TABLES)?;
    registry.register(dual_layer::factory())?;
    registry.register(organ::factory())?;
    registry.register_with_samples(wavetable_synth::factory())?;
    registry.register(fm_synth::factory())?;
    registry.register_with_samples(sampler::factory())?;
    registry.register_with_mods(semi_modular::factory(), &semi_modular::MOD_TABLES)?;
    registry.register_with_samples(granular::factory())?;
    registry.register_with_samples(drum_machine::factory())?;
    registry.register_with_samples(loop_slicer::factory())?;
    registry.register(key_chord::factory())?;
    registry.register(line_mixer::factory())?;
    registry.register_with_curves(cv_sequencer::factory())?;
    registry.register_with_drums(drum_generator::factory())?;
    registry.register_with_player_data(step_sequencer::factory())?;
    registry.register(stereo_delay::factory())?;
    registry.register(stereo_tool::factory())?;
    registry.register(reverb::factory())?;
    registry.register(rotary_speaker::factory())?;
    registry.register(space_reverb::factory())?;
    registry.register(vocoder::factory())?;
    registry.register_with_samples(convolver::factory())?;
    registry.register(auto_filter::factory())?;
    registry.register(compressor::factory())?;
    registry.register(container::factory())?;
    registry.register(cv_math::factory())?;
    registry.register(cv_quantizer::factory())?;
    registry.register(de_esser::factory())?;
    registry.register(distortion::factory())?;
    registry.register(duck::factory())?;
    registry.register(echo::factory())?;
    registry.register(dynamics::factory())?;
    registry.register(env_filter::factory())?;
    registry.register(envelope_follower::factory())?;
    registry.register(eq::factory())?;
    registry.register(feedback::factory())?;
    registry.register(gain_tool::factory())?;
    registry.register(gate::factory())?;
    registry.register(gate_processor::factory())?;
    registry.register(guitar_amp::factory())?;
    registry.register(macro_bank::factory())?;
    registry.register(mastering::factory())?;
    registry.register(midi_out::factory())?;
    registry.register(mod_source::factory())?;
    registry.register(modulation::factory())?;
    registry.register(note_delay::factory())?;
    registry.register(note_filter::factory())?;
    registry.register(note_repeater::factory())?;
    registry.register(note_split::factory())?;
    registry.register(phase_meter::factory())?;
    registry.register(pitch_shifter::factory())?;
    registry.register(bus::factory())?;
    registry.register(channel::factory())?;
    registry.register(character::factory())?;
    registry.register(clipper::factory())?;
    registry.register(master::factory())?;
    registry.register(sample_hold::factory())?;
    registry.register(slew_limiter::factory())?;
    registry.register(spectrum::factory())?;
    registry.register(spider::audio_factory())?;
    registry.register(spider::cv_factory())?;
    registry.register(transient_shaper::factory())?;
    registry.register(tuner::factory())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Every built-in device registers, validates and runs through the harness.
    use super::register_all;
    use atomik_core::{NoteEvent, NoteKind};
    use atomik_device_sdk::{Registry, RegistryError};
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::no_alloc::assert_no_alloc;

    const TYPE_IDS: [&str; 66] = [
        "atomik.arpeggiator",
        "atomik.audio-track",
        "atomik.auto-filter",
        "atomik.bus",
        "atomik.channel",
        "atomik.character",
        "atomik.clipper",
        "atomik.compressor",
        "atomik.container",
        "atomik.convolver",
        "atomik.cv-math",
        "atomik.cv-quantizer",
        "atomik.cv-sequencer",
        "atomik.de-esser",
        "atomik.distortion",
        "atomik.drum-generator",
        "atomik.drum-machine",
        "atomik.dual-layer",
        "atomik.duck",
        "atomik.dynamics",
        "atomik.echo",
        "atomik.env-filter",
        "atomik.envelope-follower",
        "atomik.eq",
        "atomik.feedback",
        "atomik.fm-synth",
        "atomik.gain-tool",
        "atomik.gate",
        "atomik.gate-processor",
        "atomik.granular",
        "atomik.guitar-amp",
        "atomik.key-chord",
        "atomik.line-mixer",
        "atomik.loop-slicer",
        "atomik.macro-bank",
        "atomik.master",
        "atomik.mastering",
        "atomik.midi-out",
        "atomik.mod-source",
        "atomik.modulation",
        "atomik.mono-bass",
        "atomik.note-delay",
        "atomik.note-filter",
        "atomik.note-repeater",
        "atomik.note-split",
        "atomik.organ",
        "atomik.phase-meter",
        "atomik.pitch-shifter",
        "atomik.poly-synth",
        "atomik.reverb",
        "atomik.rotary-speaker",
        "atomik.sample-hold",
        "atomik.sampler",
        "atomik.semi-modular",
        "atomik.slew-limiter",
        "atomik.space-reverb",
        "atomik.spectrum",
        "atomik.spider-audio",
        "atomik.spider-cv",
        "atomik.step-sequencer",
        "atomik.stereo-delay",
        "atomik.stereo-tool",
        "atomik.transient-shaper",
        "atomik.tuner",
        "atomik.vocoder",
        "atomik.wavetable-synth",
    ];

    #[test]
    // Name kept; it now covers all sixty-six devices.
    fn register_all_registers_nine_devices_in_sorted_order() {
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        assert_eq!(r.type_ids().collect::<Vec<_>>(), TYPE_IDS);
        for id in TYPE_IDS {
            let f = r.get(id).unwrap();
            assert_eq!(f.descriptor.validate(), Ok(()));
            assert_eq!(r.create(id).unwrap().descriptor().type_id, id);
            assert_eq!(
                r.takes_samples(id),
                id == "atomik.sampler"
                    || id == "atomik.granular"
                    || id == "atomik.drum-machine"
                    || id == "atomik.loop-slicer"
                    || id == "atomik.convolver"
                    || id == "atomik.wavetable-synth",
                "{id}"
            );
            assert_eq!(
                r.takes_player_data(id),
                id == "atomik.step-sequencer"
                    || id == "atomik.cv-sequencer"
                    || id == "atomik.drum-generator",
                "{id}"
            );
            assert_eq!(r.takes_curves(id), id == "atomik.cv-sequencer", "{id}");
            assert_eq!(r.takes_drums(id), id == "atomik.drum-generator", "{id}");
        }
    }

    #[test]
    fn the_utility_devices_have_a_section_in_the_device_reference() {
        let doc = include_str!("../../../docs/DEVICES.md");
        for id in [
            "atomik.auto-filter",
            "atomik.clipper",
            "atomik.cv-math",
            "atomik.cv-quantizer",
            "atomik.de-esser",
            "atomik.duck",
            "atomik.dynamics",
            "atomik.env-filter",
            "atomik.envelope-follower",
            "atomik.feedback",
            "atomik.gain-tool",
            "atomik.gate",
            "atomik.gate-processor",
            "atomik.pitch-shifter",
            "atomik.rotary-speaker",
            "atomik.sample-hold",
            "atomik.slew-limiter",
            "atomik.spider-audio",
            "atomik.spider-cv",
            "atomik.stereo-tool",
            "atomik.transient-shaper",
            "atomik.tuner",
        ] {
            assert!(
                doc.lines().any(|l| l.starts_with('#') && l.contains(id)),
                "{id} has no section in docs/DEVICES.md"
            );
        }
    }

    #[test]
    fn modulator_type_ids_are_pinned() {
        assert_eq!(super::MODULATOR_TYPE_IDS, ["atomik.mod-source"]);
        assert!(super::is_modulator("atomik.mod-source"));
        assert!(!super::is_modulator("atomik.modulation"));
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        for id in super::MODULATOR_TYPE_IDS {
            let d = r.get(id).unwrap().descriptor;
            assert_eq!(d.kind, atomik_device_sdk::DeviceKind::Utility);
        }
    }

    #[test]
    fn internal_type_ids_are_pinned() {
        assert_eq!(super::INTERNAL_TYPE_IDS, ["atomik.note-split"]);
        assert!(super::is_internal("atomik.note-split"));
        assert!(!super::is_internal("atomik.container"));
        assert!(!super::is_internal("atomik.poly-synth"));
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        for id in super::INTERNAL_TYPE_IDS {
            assert!(r.get(id).is_some(), "{id}");
        }
    }

    #[test]
    fn container_and_split_have_their_kinds() {
        use atomik_device_sdk::DeviceKind;
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        assert_eq!(
            r.get("atomik.container").unwrap().descriptor.kind,
            DeviceKind::Container
        );
        assert_eq!(
            r.get("atomik.note-split").unwrap().descriptor.kind,
            DeviceKind::Utility
        );
    }

    #[test]
    fn register_all_twice_returns_duplicate() {
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        assert!(matches!(
            register_all(&mut r),
            Err(RegistryError::Duplicate(_))
        ));
    }

    #[test]
    fn process_is_allocation_free_for_every_registered_device() {
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        for id in TYPE_IDS {
            for sr in [44_100.0, 48_000.0, 96_000.0] {
                let mut h = DeviceHarness::with_options(r.create(id).unwrap(), sr, 133.0, 9);
                let drum = id == "atomik.drum-machine";
                let slicer = id == "atomik.loop-slicer";
                let sample_map = || {
                    if drum {
                        crate::test_support::drum_sample_map()
                    } else if slicer {
                        crate::test_support::slicer_sample_map()
                    } else {
                        crate::test_support::demo_sample_map()
                    }
                };
                if drum {
                    // Some individual outputs connected: those pads leave the main mix.
                    for key in [
                        "pad1_out_l",
                        "pad2_out_r",
                        "pad4_out_l",
                        "pad4_out_r",
                        "pad16_out_l",
                    ] {
                        h.set_connected(key, true);
                    }
                }
                if r.takes_samples(id) {
                    // A loaded map, so the voices below sound (and swaps are guarded too).
                    let first = sample_map();
                    assert!(assert_no_alloc(|| h.device_mut().swap_samples(Some(first))).is_none());
                }
                if r.takes_player_data(id) {
                    let first = if r.takes_drums(id) {
                        crate::test_support::drum_player_data(1)
                    } else {
                        crate::test_support::seq_player_data(1)
                    };
                    assert!(
                        assert_no_alloc(|| h.device_mut().swap_player_data(Some(first))).is_none()
                    );
                }
                let d = h.device().descriptor();
                for s in d.sockets {
                    if s.kind == atomik_device_sdk::SocketKind::Notes {
                        let keys: &[u8] = if drum {
                            &[36, 37, 38, 39, 51]
                        } else if slicer {
                            &[24, 25, 26, 27, 28, 87]
                        } else {
                            &[64]
                        };
                        for key in keys {
                            h.push_note(
                                s.key,
                                NoteEvent {
                                    offset: 3,
                                    kind: NoteKind::On {
                                        key: *key,
                                        velocity: 0.9,
                                    },
                                },
                            );
                        }
                        // Controllers: pedal, volume, expression, pressure, bend and the
                        // bend-range RPN, a note-off held by the pedal, then the pedal up.
                        let cc = |controller, value| NoteKind::Cc { controller, value };
                        for (offset, kind) in [
                            (4, cc(64, 1.0)),
                            (4, cc(7, 0.8)),
                            (4, cc(11, 0.9)),
                            (5, NoteKind::ChannelPressure { value: 0.4 }),
                            (
                                5,
                                NoteKind::PolyPressure {
                                    key: 64,
                                    value: 0.6,
                                },
                            ),
                            (5, NoteKind::PitchBend { value: 0.5 }),
                            (6, cc(101, 0.0)),
                            (6, cc(100, 0.0)),
                            (6, cc(6, 7.0 / 127.0)),
                            (6, cc(38, 0.0)),
                            (7, NoteKind::Off { key: keys[0] }),
                            (8, cc(64, 0.0)),
                        ] {
                            h.push_note(s.key, NoteEvent { offset, kind });
                        }
                    } else if s.direction.is_input() {
                        *h.input_mut(s.key) = [0.25; atomik_core::BATCH];
                    }
                }
                for _ in 0..50 {
                    h.process(); // guarded by assert_no_alloc inside the harness
                }
                if r.takes_player_data(id) {
                    let next = if r.takes_drums(id) {
                        crate::test_support::drum_player_data(2)
                    } else {
                        crate::test_support::seq_player_data(2)
                    };
                    let back = assert_no_alloc(|| h.device_mut().swap_player_data(Some(next)));
                    assert!(back.is_some());
                    drop(back);
                    h.process();
                }
                if r.takes_samples(id) {
                    let next = sample_map();
                    let back = assert_no_alloc(|| h.device_mut().swap_samples(Some(next)));
                    drop(back); // freed here, off the guarded section
                    h.process();
                }
                h.device_mut().reset();
                h.process();
            }
        }
    }

    /// Every registered device, with every audio and CV input fed and a loaded sample or player
    /// map where it takes one, stays finite while each parameter visits its min, max and default.
    #[test]
    fn every_registered_device_passes_the_min_max_sweep() {
        let mut r = Registry::new();
        register_all(&mut r).unwrap();
        for id in TYPE_IDS {
            let mut h = DeviceHarness::with_options(r.create(id).unwrap(), 48_000.0, 133.0, 9);
            if r.takes_samples(id) {
                let map = match id {
                    "atomik.drum-machine" => crate::test_support::drum_sample_map(),
                    "atomik.loop-slicer" => crate::test_support::slicer_sample_map(),
                    _ => crate::test_support::demo_sample_map(),
                };
                let _ = h.device_mut().swap_samples(Some(map));
            }
            if r.takes_player_data(id) {
                let data = if r.takes_drums(id) {
                    crate::test_support::drum_player_data(1)
                } else {
                    crate::test_support::seq_player_data(1)
                };
                let _ = h.device_mut().swap_player_data(Some(data));
            }
            let d = h.device().descriptor();
            for s in d.sockets {
                if s.kind != atomik_device_sdk::SocketKind::Notes && s.direction.is_input() {
                    *h.input_mut(s.key) = [0.25; atomik_core::BATCH];
                }
            }
            crate::test_support::sweep_params_min_max(&mut h, d);
        }
    }
}
