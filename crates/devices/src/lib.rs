//! Atomik Rack built-in devices: [`poly_synth`], [`sampler`], [`drum_machine`], [`loop_slicer`], [`line_mixer`], [`stereo_delay`], [`reverb`], the effects
//! [`compressor`], [`eq`], [`distortion`], [`modulation`] and [`mastering`], and the
//! console devices [`channel`] (`atomik.channel`) and [`master`] (`atomik.master`).
//!
//! Each module exposes a `pub static DESCRIPTOR`, a `pub fn factory()` and the device struct.
//! All three are original designs.
//! `process`, `reset` and `set_param` never allocate, lock or block; any randomness comes
//! from [`atomik_dsp::Rng`] seeded by `PrepareInfo::seed`.

pub mod arpeggiator;
pub mod audio_track;
pub mod channel;
pub mod character;
pub mod compressor;
pub mod container;
mod cv_in;
pub mod distortion;
pub mod drum_machine;
pub mod eq;
pub mod line_mixer;
pub mod loop_slicer;
pub mod master;
pub mod mastering;
pub mod mod_source;
pub mod modulation;
mod note_clock;
pub mod note_split;
pub mod poly_synth;
pub mod reverb;
pub mod sampler;
pub mod step_sequencer;
pub mod stereo_delay;

use atomik_device_sdk::{Registry, RegistryError};

#[cfg(test)]
atomik_testkit::install_alloc_guard!();

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
pub const PLAYER_TYPE_IDS: [&str; 2] = ["atomik.arpeggiator", "atomik.step-sequencer"];

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
    registry.register_with_samples(sampler::factory())?;
    registry.register_with_samples(drum_machine::factory())?;
    registry.register_with_samples(loop_slicer::factory())?;
    registry.register(line_mixer::factory())?;
    registry.register_with_player_data(step_sequencer::factory())?;
    registry.register(stereo_delay::factory())?;
    registry.register(reverb::factory())?;
    registry.register(compressor::factory())?;
    registry.register(container::factory())?;
    registry.register(distortion::factory())?;
    registry.register(eq::factory())?;
    registry.register(mastering::factory())?;
    registry.register(mod_source::factory())?;
    registry.register(modulation::factory())?;
    registry.register(note_split::factory())?;
    registry.register(channel::factory())?;
    registry.register(character::factory())?;
    registry.register(master::factory())?;
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

    const TYPE_IDS: [&str; 21] = [
        "atomik.arpeggiator",
        "atomik.audio-track",
        "atomik.channel",
        "atomik.character",
        "atomik.compressor",
        "atomik.container",
        "atomik.distortion",
        "atomik.drum-machine",
        "atomik.eq",
        "atomik.line-mixer",
        "atomik.loop-slicer",
        "atomik.master",
        "atomik.mastering",
        "atomik.mod-source",
        "atomik.modulation",
        "atomik.note-split",
        "atomik.poly-synth",
        "atomik.reverb",
        "atomik.sampler",
        "atomik.step-sequencer",
        "atomik.stereo-delay",
    ];

    #[test]
    // Name kept; it now covers all twenty-one devices.
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
                id == "atomik.sampler" || id == "atomik.drum-machine" || id == "atomik.loop-slicer",
                "{id}"
            );
            assert_eq!(
                r.takes_player_data(id),
                id == "atomik.step-sequencer",
                "{id}"
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
                    let first = crate::test_support::seq_player_data(1);
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
                    } else if s.direction.is_input() {
                        *h.input_mut(s.key) = [0.25; atomik_core::BATCH];
                    }
                }
                for _ in 0..50 {
                    h.process(); // guarded by assert_no_alloc inside the harness
                }
                if r.takes_player_data(id) {
                    let next = crate::test_support::seq_player_data(2);
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
