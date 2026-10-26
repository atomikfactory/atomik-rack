//! Planning of the drum commands: `SetDrumStep`, `SetDrumLane`, `SetDrumPattern` and the three
//! pattern-list commands. Each becomes one reversible [`Edit::Drums`].

use crate::drums::{DrumDoc, DrumPatternDoc, DrumStepDoc};
use crate::history::Edit;
use crate::steps::{MAX_NAME_CHARS, MAX_PATTERNS, MAX_STEPS};
use crate::steps_cmd::{bad, lower_pattern_param};
use crate::{Command, DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::{DRUM_LANES, Registry};

/// The stored drum patterns of `key` and the working copy (the default when none is stored),
/// checking that the device exists and takes drum data.
fn current(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Result<(Option<DrumDoc>, DrumDoc), ModelError> {
    let d = rules::resolve(doc, key)?;
    if !registry.takes_drums(&d.type_id) {
        return Err(bad(format!(
            "{key} ({}) does not take drum data",
            d.type_id
        )));
    }
    let stored = d.drums.clone();
    let working = stored.clone().unwrap_or_default();
    Ok((stored, working))
}

fn index_in(drums: &DrumDoc, index: u8, what: &str) -> Result<usize, ModelError> {
    let i = usize::from(index);
    if i < drums.patterns.len() {
        Ok(i)
    } else {
        Err(bad(format!(
            "{what}: index {index} is outside 0..{}",
            drums.patterns.len()
        )))
    }
}

fn edit(key: DeviceKey, old: Option<DrumDoc>, new: DrumDoc) -> Edit {
    Edit::Drums {
        key,
        old,
        new: Some(new),
    }
}

/// Plans one drum command.
///
/// # Panics
/// If `command` is not one of the drum commands.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: Command,
) -> Result<Edit, ModelError> {
    match command {
        Command::SetDrumStep {
            device,
            index,
            lane,
            step,
            velocity,
            prob,
            ..
        } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "SetDrumStep")?;
            let (l, s) = (usize::from(lane), usize::from(step));
            if l >= DRUM_LANES || s >= MAX_STEPS {
                return Err(bad(format!(
                    "SetDrumStep: lane {lane} step {step} is outside 0..{DRUM_LANES} x 0..{MAX_STEPS}"
                )));
            }
            let value = DrumStepDoc { velocity, prob };
            value.check().map_err(bad)?;
            new.patterns[i].steps[l][s] = value;
            Ok(edit(device, old, new))
        }
        Command::SetDrumLane {
            device,
            index,
            lane,
            setting,
        } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "SetDrumLane")?;
            let l = usize::from(lane);
            if l >= DRUM_LANES {
                return Err(bad(format!(
                    "SetDrumLane: lane {lane} is outside 0..{DRUM_LANES}"
                )));
            }
            if setting.key > 127 {
                return Err(bad(format!("key {} is outside 0..=127", setting.key)));
            }
            new.patterns[i].lanes[l] = setting;
            Ok(edit(device, old, new))
        }
        Command::SetDrumPattern {
            device,
            index,
            pattern,
            ..
        } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "SetDrumPattern")?;
            pattern.check().map_err(bad)?;
            new.patterns[i] = pattern;
            Ok(edit(device, old, new))
        }
        Command::AddDrumPattern { device } => {
            let (old, mut new) = current(doc, registry, &device)?;
            if new.patterns.len() >= MAX_PATTERNS {
                return Err(too_many());
            }
            let name = format!("Pattern {}", new.patterns.len() + 1);
            new.patterns.push(DrumPatternDoc::new(name));
            Ok(edit(device, old, new))
        }
        Command::DuplicateDrumPattern { device, index } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "DuplicateDrumPattern")?;
            if new.patterns.len() >= MAX_PATTERNS {
                return Err(too_many());
            }
            let mut copy = new.patterns[i].clone();
            copy.name = format!("{} copy", copy.name)
                .chars()
                .take(MAX_NAME_CHARS)
                .collect();
            new.patterns.push(copy);
            Ok(edit(device, old, new))
        }
        Command::RemoveDrumPattern { device, index } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "RemoveDrumPattern")?;
            if new.patterns.len() == 1 {
                return Err(bad("the last pattern cannot be removed"));
            }
            new.patterns.remove(i);
            let count = new.patterns.len();
            lower_pattern_param(
                doc,
                registry,
                &device,
                count,
                edit(device.clone(), old, new),
            )
        }
        other => unreachable!("not a drum command: {other:?}"),
    }
}

fn too_many() -> ModelError {
    bad(format!("a generator holds at most {MAX_PATTERNS} patterns"))
}
