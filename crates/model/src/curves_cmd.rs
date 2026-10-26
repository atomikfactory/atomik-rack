//! Planning of `SetCurves`: one reversible [`Edit::Curves`].

use crate::curves::CurvePatternDoc;
use crate::history::Edit;
use crate::steps_cmd::bad;
use crate::{Command, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;

/// Plans `SetCurves`.
///
/// # Panics
/// If `command` is not `SetCurves`.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: Command,
) -> Result<Edit, ModelError> {
    let Command::SetCurves {
        device,
        index,
        curves,
        ..
    } = command
    else {
        unreachable!("not SetCurves: {command:?}")
    };
    let d = rules::resolve(doc, &device)?;
    if !registry.takes_curves(&d.type_id) {
        return Err(bad(format!(
            "{device} ({}) does not take curves",
            d.type_id
        )));
    }
    let count = d.steps.as_ref().map_or(1, |s| s.patterns.len());
    let i = usize::from(index);
    if i >= count {
        return Err(bad(format!(
            "SetCurves: index {index} is outside 0..{count}"
        )));
    }
    curves.check().map_err(bad)?;
    let old = d.curves.clone();
    let mut new = old.clone().unwrap_or_default();
    while new.patterns.len() <= i {
        new.patterns.push(CurvePatternDoc::default());
    }
    new.patterns[i] = curves;
    Ok(Edit::Curves {
        key: device,
        old,
        new: Some(new),
    })
}
