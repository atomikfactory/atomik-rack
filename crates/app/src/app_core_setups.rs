//! Device setups: a `setup` choice that loads a whole set of param values.
//!
//! The guitar amp has a `setup` param (Clean, Crunch, Industrial Rhythm, 80s Lead, Synthwave), the
//! FM synth a `sound` param (Init, E. Piano, Bell, Bass, Brass, Pad) and the granular sampler a
//! `sound` param (Init, Frozen Pad, Shimmer, Stutter, Texture). The device itself ignores it; here
//! a `SetParam` of `setup` (or `sound`) is turned into a `Command::Batch`: the choice param
//! followed by every param of the chosen setup (`atomik_devices::guitar_amp::setups`,
//! `atomik_devices::fm_synth::sounds`, `atomik_devices::granular::sounds`).
//! A batch is one undo step, so one undo brings back everything the setup changed, the player tweaks
//! from there with ordinary param changes, and choosing the same setup again restores its values.
//! The conversion sits in front of the model, so the rack panel, MIDI, the MCP tools and the macros'
//! direct edits all get it, and a project file only ever holds plain param values. A `bypass` is not
//! part of a setup.

use super::*;

/// The type id of the guitar amp, whose `setup` param loads a factory setup.
const SETUP_DEVICE: &str = atomik_devices::guitar_amp::DESCRIPTOR.type_id;
/// The type id of the FM synth, whose `sound` param loads a factory sound.
const SOUND_DEVICE: &str = atomik_devices::fm_synth::DESCRIPTOR.type_id;
/// The type id of the granular sampler, whose `sound` param loads a factory sound.
const GRANULAR_DEVICE: &str = atomik_devices::granular::DESCRIPTOR.type_id;

impl<S: EngineSink> AppCore<S> {
    /// `command`, or, when it chooses a setup on a guitar amp, the batch that loads it.
    pub(super) fn expand_setup(&self, command: Command) -> Command {
        let Command::SetParam {
            key, param, value, ..
        } = &command
        else {
            return command;
        };
        let Some(dev) = self.doc().resolve(key) else {
            return command;
        };
        let (choice, index, values): (&str, usize, Vec<(&'static str, f32)>) =
            match (dev.type_id.as_str(), param.as_str()) {
                (SETUP_DEVICE, "setup") => {
                    let setups = &atomik_devices::guitar_amp::setups::SETUPS;
                    let i = clamp_index(*value, setups.len());
                    ("setup", i, setups[i].values.to_vec())
                }
                (SOUND_DEVICE, "sound") => {
                    let sounds = &atomik_devices::fm_synth::sounds::SOUNDS;
                    let i = clamp_index(*value, sounds.len());
                    ("sound", i, atomik_devices::fm_synth::sounds::values(i))
                }
                (GRANULAR_DEVICE, "sound") => {
                    let sounds = &atomik_devices::granular::sounds::SOUNDS;
                    let i = clamp_index(*value, sounds.len());
                    ("sound", i, atomik_devices::granular::sounds::values(i))
                }
                _ => return command,
            };
        let set = |param: &str, value: f32| Command::SetParam {
            key: key.clone(),
            param: param.to_string(),
            value,
            merge: None,
        };
        let mut batch = Vec::with_capacity(values.len() + 1);
        batch.push(set(choice, index as f32));
        batch.extend(values.iter().map(|(p, v)| set(p, *v)));
        Command::Batch(batch)
    }
}

/// The index a choice value stands for: rounded, clamped to `0..len`, 0 for a non-finite value.
fn clamp_index(value: f32, len: usize) -> usize {
    if value.is_finite() {
        (value.round().max(0.0) as usize).min(len - 1)
    } else {
        0
    }
}
