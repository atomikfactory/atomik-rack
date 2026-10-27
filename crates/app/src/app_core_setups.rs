//! Device setups: a `setup` choice that loads a whole set of param values.
//!
//! The guitar amp has a `setup` param (Clean, Crunch, Industrial Rhythm, 80s Lead, Synthwave). The
//! device itself ignores it; here a `SetParam` of `setup` is turned into a `Command::Batch`: the
//! `setup` param followed by every param of the chosen setup (`atomik_devices::guitar_amp::setups`).
//! A batch is one undo step, so one undo brings back everything the setup changed, the player tweaks
//! from there with ordinary param changes, and choosing the same setup again restores its values.
//! The conversion sits in front of the model, so the rack panel, MIDI, the MCP tools and the macros'
//! direct edits all get it, and a project file only ever holds plain param values. A `bypass` is not
//! part of a setup.

use super::*;

/// The type id of the one device that has setups.
const SETUP_DEVICE: &str = atomik_devices::guitar_amp::DESCRIPTOR.type_id;

impl<S: EngineSink> AppCore<S> {
    /// `command`, or, when it chooses a setup on a guitar amp, the batch that loads it.
    pub(super) fn expand_setup(&self, command: Command) -> Command {
        let Command::SetParam {
            key, param, value, ..
        } = &command
        else {
            return command;
        };
        if param != "setup" {
            return command;
        }
        if !self
            .doc()
            .resolve(key)
            .is_some_and(|d| d.type_id == SETUP_DEVICE)
        {
            return command;
        }
        let setups = &atomik_devices::guitar_amp::setups::SETUPS;
        let index = if value.is_finite() {
            (value.round().max(0.0) as usize).min(setups.len() - 1)
        } else {
            0
        };
        let set = |param: &str, value: f32| Command::SetParam {
            key: key.clone(),
            param: param.to_string(),
            value,
            merge: None,
        };
        let mut batch = Vec::with_capacity(setups[index].values.len() + 1);
        batch.push(set("setup", index as f32));
        batch.extend(setups[index].values.iter().map(|(p, v)| set(p, *v)));
        Command::Batch(batch)
    }
}
