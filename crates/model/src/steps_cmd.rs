//! Planning of the pattern commands: `SetPattern`, `AddPattern`,
//! `RemovePattern` and `DuplicatePattern` (and `SetCurves`, planned in `curves_cmd`). Each
//! becomes one reversible [`Edit`].

use crate::curves::{CurveDoc, CurvePatternDoc};
use crate::history::Edit;
use crate::steps::{MAX_NAME_CHARS, PatternDoc, StepSeqDoc, pattern_cap};
use crate::{Command, DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;

/// The device param that selects the playing pattern (1-based).
const PATTERN_PARAM: &str = "pattern";

pub(crate) fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadPatterns(message.into())
}

/// The stored patterns of `key` and the working copy (the default when none is stored),
/// checking that the device exists and takes pattern data.
fn current(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Result<(Option<StepSeqDoc>, StepSeqDoc, usize), ModelError> {
    let d = rules::resolve(doc, key)?;
    if registry.takes_drums(&d.type_id) {
        return Err(bad(format!(
            "{key} ({}) takes drum data, use the drum pattern commands",
            d.type_id
        )));
    }
    if !registry.takes_player_data(&d.type_id) {
        return Err(bad(format!(
            "{key} ({}) does not take pattern data",
            d.type_id
        )));
    }
    let stored = d.steps.clone();
    let working = stored.clone().unwrap_or_default();
    Ok((stored, working, pattern_cap(registry, &d.type_id)))
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

fn too_many(cap: usize) -> ModelError {
    bad(format!("a sequencer holds at most {cap} patterns"))
}

fn steps(key: DeviceKey, old: Option<StepSeqDoc>, new: StepSeqDoc) -> Edit {
    Edit::Steps {
        key,
        old,
        new: Some(new),
    }
}

/// The edit that keeps the stored curves of `key` with their patterns when `follow` changes the
/// pattern list; `None` when no curves are stored or nothing changes.
fn curves_edit(doc: &RackDoc, key: &DeviceKey, follow: impl FnOnce(&mut CurveDoc)) -> Option<Edit> {
    let old = rules::resolve(doc, key).ok()?.curves.clone()?;
    let mut new = old.clone();
    follow(&mut new);
    (new != old).then(|| Edit::Curves {
        key: key.clone(),
        old: Some(old),
        new: Some(new),
    })
}

/// `edit` together with the curve edit that goes with it, as one undo step.
fn with_curves(edit: Edit, curves: Option<Edit>) -> Edit {
    match curves {
        Some(c) => Edit::Compound(vec![edit, c]),
        None => edit,
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
            let (old, mut new, _) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "SetPattern")?;
            new.patterns[i] = pattern;
            new.check().map_err(bad)?;
            Ok(steps(device, old, new))
        }
        Command::AddPattern { device } => {
            let (old, mut new, cap) = current(doc, registry, &device)?;
            if new.patterns.len() >= cap {
                return Err(too_many(cap));
            }
            let name = format!("Pattern {}", new.patterns.len() + 1);
            new.patterns.push(PatternDoc::new(name));
            Ok(steps(device, old, new))
        }
        Command::DuplicatePattern { device, index } => {
            let (old, mut new, cap) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "DuplicatePattern")?;
            if new.patterns.len() >= cap {
                return Err(too_many(cap));
            }
            let mut copy = new.patterns[i].clone();
            copy.name = format!("{} copy", copy.name)
                .chars()
                .take(MAX_NAME_CHARS)
                .collect();
            let count = new.patterns.len();
            new.patterns.push(copy);
            // The copy of the curves lands at the same new index.
            let curves = curves_edit(doc, &device, |c| {
                if i < c.patterns.len() && count < cap {
                    while c.patterns.len() < count {
                        c.patterns.push(CurvePatternDoc::default());
                    }
                    let copy = c.patterns[i].clone();
                    c.patterns.insert(count, copy);
                }
            });
            Ok(with_curves(steps(device, old, new), curves))
        }
        Command::RemovePattern { device, index } => {
            let (old, mut new, _) = current(doc, registry, &device)?;
            let i = index_in(&new, index, "RemovePattern")?;
            if new.patterns.len() == 1 {
                return Err(bad("the last pattern cannot be removed"));
            }
            new.patterns.remove(i);
            let count = new.patterns.len();
            let curves = curves_edit(doc, &device, |c| {
                if i < c.patterns.len() {
                    c.patterns.remove(i);
                    if c.patterns.is_empty() {
                        c.patterns.push(CurvePatternDoc::default());
                    }
                }
            });
            let edit = with_curves(steps(device.clone(), old, new), curves);
            lower_pattern_param(doc, registry, &device, count, edit)
        }
        command @ Command::SetCurves { .. } => crate::curves_cmd::plan(doc, registry, command),
        other => unreachable!("not a pattern command: {other:?}"),
    }
}

/// `edit` (a pattern removal that leaves `count` patterns), with the `pattern` param lowered in
/// the same undo step when it would point past the end.
pub(crate) fn lower_pattern_param(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    count: usize,
    edit: Edit,
) -> Result<Edit, ModelError> {
    let d = rules::resolve(doc, device)?;
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
            key: device.clone(),
            param: PATTERN_PARAM.to_owned(),
            old: stored,
            new: Some(p.clamp(count as f32)),
            default: p.default,
        },
        edit,
    ]))
}
