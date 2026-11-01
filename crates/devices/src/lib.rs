//! Atomik Rack built-in devices: [`poly_synth`], [`wavetable_synth`], [`fm_synth`], [`sampler`], [`granular`], [`drum_machine`], [`loop_slicer`], [`line_mixer`], [`stereo_delay`], [`reverb`], the effects
//! [`compressor`], [`eq`], [`distortion`], [`guitar_amp`], [`modulation`] and [`mastering`], the analysers
//! [`spectrum`] and [`phase_meter`] (effects that pass the audio through and publish what they see), and the
//! console devices [`channel`] (`atomik.channel`), [`bus`] (`atomik.bus`, the channel strip as an output bus) and [`master`] (`atomik.master`).
//!
//! The utility devices [`gain_tool`], [`stereo_tool`], [`gate`], [`dynamics`] (gate then
//! compressor), [`duck`] (the sidechain tool), [`env_filter`] and the two cable mergers in
//! [`spider`] share the descriptor builders of the private `tool_support` module.
//!
//! The effects are wrapped in [`bypass::Bypassable`], which adds their `bypass` param.
//!
//! Each module exposes a `pub static DESCRIPTOR`, a `pub fn factory()` and the device struct.
//! All three are original designs.
//! `process`, `reset` and `set_param` never allocate, lock or block; any randomness comes
//! from [`atomik_dsp::Rng`] seeded by `PrepareInfo::seed`.

pub mod arpeggiator;
pub mod audio_track;
pub mod bus;
pub mod bypass;
mod chain_clock;
pub mod channel;
pub mod character;
pub mod compressor;
pub mod container;
pub mod controller_state;
mod cv_in;
pub mod cv_sequencer;
pub mod distortion;
pub mod drum_generator;
pub mod drum_machine;
pub mod duck;
pub mod dynamics;
pub mod env_filter;
pub mod eq;
pub mod fm_synth;
pub mod gain_tool;
pub mod gate;
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
mod note_clock;
pub mod note_delay;
pub mod note_filter;
mod note_fx;
mod note_queue;
pub mod note_repeater;
pub mod note_split;
pub mod phase_meter;
pub mod poly_synth;
mod recent_peak;
pub mod reverb;
pub mod sampler;
pub mod spectrum;
pub mod spider;
pub mod step_sequencer;
pub mod stereo_delay;
pub mod stereo_tool;
mod tool_support;
pub mod wavetable_synth;

use atomik_device_sdk::{Registry, RegistryError};

#[cfg(test)]
atomik_testkit::install_alloc_guard!();

#[cfg(test)]
mod midi_chart;
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
    registry.register(poly_synth::factory())?;
    registry.register(wavetable_synth::factory())?;
    registry.register(fm_synth::factory())?;
    registry.register_with_samples(sampler::factory())?;
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
    registry.register(compressor::factory())?;
    registry.register(container::factory())?;
    registry.register(distortion::factory())?;
    registry.register(duck::factory())?;
    registry.register(dynamics::factory())?;
    registry.register(env_filter::factory())?;
    registry.register(eq::factory())?;
    registry.register(gain_tool::factory())?;
    registry.register(gate::factory())?;
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
    registry.register(bus::factory())?;
    registry.register(channel::factory())?;
    registry.register(character::factory())?;
    registry.register(master::factory())?;
    registry.register(spectrum::factory())?;
    registry.register(spider::audio_factory())?;
    registry.register(spider::cv_factory())?;
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

    const TYPE_IDS: [&str; 44] = [
        "atomik.arpeggiator",
        "atomik.audio-track",
        "atomik.bus",
        "atomik.channel",
        "atomik.character",
        "atomik.compressor",
        "atomik.container",
        "atomik.cv-sequencer",
        "atomik.distortion",
        "atomik.drum-generator",
        "atomik.drum-machine",
        "atomik.duck",
        "atomik.dynamics",
        "atomik.env-filter",
        "atomik.eq",
        "atomik.fm-synth",
        "atomik.gain-tool",
        "atomik.gate",
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
        "atomik.note-delay",
        "atomik.note-filter",
        "atomik.note-repeater",
        "atomik.note-split",
        "atomik.phase-meter",
        "atomik.poly-synth",
        "atomik.reverb",
        "atomik.sampler",
        "atomik.spectrum",
        "atomik.spider-audio",
        "atomik.spider-cv",
        "atomik.step-sequencer",
        "atomik.stereo-delay",
        "atomik.stereo-tool",
        "atomik.wavetable-synth",
    ];

    #[test]
    // Name kept; it now covers all forty-four devices.
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
                    || id == "atomik.loop-slicer",
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
            "atomik.duck",
            "atomik.dynamics",
            "atomik.env-filter",
            "atomik.gain-tool",
            "atomik.gate",
            "atomik.spider-audio",
            "atomik.spider-cv",
            "atomik.stereo-tool",
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
}
