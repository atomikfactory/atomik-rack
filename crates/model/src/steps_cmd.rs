//! Planning of the pattern commands: `SetPattern`, `AddPattern`,
//! `RemovePattern` and `DuplicatePattern`. Each becomes one reversible [`Edit`].

use crate::history::Edit;
use crate::steps::{MAX_NAME_CHARS, MAX_PATTERNS, PatternDoc, StepSeqDoc};
use crate::{Command, DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;

/// The device param that selects the playing pattern (1-based).
const PATTERN_PARAM: &str = "pattern";

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadPatterns(message.into())
}

/// The stored patterns of `key` and the working copy (the default when none is stored),
/// checking that the device exists and takes pattern data.
fn current(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Result<(Option<StepSeqDoc>, StepSeqDoc), ModelError> {
    let d = rules::resolve(doc, key)?;
    if !registry.takes_player_data(&d.type_id) {
        return Err(bad(format!(
            "{key} ({}) does not take pattern data",
            d.type_id
        )));
    }
    let stored = d.steps.clone();
    let working = stored.clone().unwrap_or_default();
    Ok((stored, working))
}

fn index_in(seq: &StepSeqDoc, index: u8, what: &str) -> Result<usize, ModelError> {
    let i = usize::from(index);
    if i < seq.patterns.len() {
        Ok(i)
    } else {
        Err(bad(format!(
            "{what}: index {index} is outside 0..{}",
            seq.patterns.len()
        )))
    }
}

fn too_many() -> ModelError {
    bad(format!("a sequencer holds at most {MAX_PATTERNS} patterns"))
}

fn steps(key: DeviceKey, old: Option<StepSeqDoc>, new: StepSeqDoc) -> Edit {
    Edit::Steps {
        key,
        old,
        new: Some(new),
    }
}

/// Plans one pattern command.
///
/// # Panics
/// If `command` is not one of the four pattern commands.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: Command,
) -> Result<Edit, ModelError> {
    match command {
        Command::SetPattern {
            device,
            index,
            pattern,
            ..
        } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "SetPattern")?;
            new.patterns[i] = pattern;
            new.check().map_err(bad)?;
            Ok(steps(device, old, new))
        }
        Command::AddPattern { device } => {
            let (old, mut new) = current(doc, registry, &device)?;
            if new.patterns.len() >= MAX_PATTERNS {
                return Err(too_many());
            }
            let name = format!("Pattern {}", new.patterns.len() + 1);
            new.patterns.push(PatternDoc::new(name));
            Ok(steps(device, old, new))
        }
        Command::DuplicatePattern { device, index } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "DuplicatePattern")?;
            if new.patterns.len() >= MAX_PATTERNS {
                return Err(too_many());
            }
            let mut copy = new.patterns[i].clone();
            copy.name = format!("{} copy", copy.name)
                .chars()
                .take(MAX_NAME_CHARS)
                .collect();
            new.patterns.push(copy);
            Ok(steps(device, old, new))
        }
        Command::RemovePattern { device, index } => {
            let (old, mut new) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "RemovePattern")?;
            if new.patterns.len() == 1 {
                return Err(bad("the last pattern cannot be removed"));
            }
            new.patterns.remove(i);
            let count = new.patterns.len();
            let edit = steps(device.clone(), old, new);
            // The `pattern` param must keep pointing at a pattern: lower it in the same step.
            let d = rules::resolve(doc, &device)?;
            let Some(p) = registry
                .get(&d.type_id)
                .and_then(|f| f.descriptor.param_by_key(PATTERN_PARAM))
            else {
                return Ok(edit);
            };
            let stored = d.params.get(PATTERN_PARAM).copied();
            if stored.unwrap_or(p.default) <= count as f32 {
                return Ok(edit);
            }
            Ok(Edit::Compound(vec![
                Edit::Param {
                    key: device,
                    param: PATTERN_PARAM.to_owned(),
                    old: stored,
                    new: Some(p.clamp(count as f32)),
                    default: p.default,
                },
                edit,
            ]))
        }
        other => unreachable!("not a pattern command: {other:?}"),
    }
}
