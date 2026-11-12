//! FX loops cabled by hand.
//!
//! The console lists an effect as a return only through `console.returns`. A user who cables
//! the master's `fx_send{k}_{l,r}` into an effect and the effect's `out_{l,r}` into the master's
//! `return{k}_{l,r}` has built the same loop `SetReturn` makes, so the slot is registered for
//! them: when a cable edit completes such a loop (see [`with_returns`]) and when a project is
//! opened with the loop cabled and the slot empty (see [`inferred_returns`]). Cutting a cable
//! of a registered loop frees its slot again, as moving the effect away does.

use crate::console::{FX_SLOTS, is_console_type, sockets};
use crate::history::Edit;
use crate::{DeviceKey, RackDoc};

/// The effect whose in/out sockets close the loop through the master's send and return `slot`
/// (0-based), whatever the console lists there. `None` for an open loop, a loop through two
/// effects, or a master-less document.
fn loop_effect(doc: &RackDoc, slot: usize) -> Option<DeviceKey> {
    let master = doc.console.master.as_ref()?;
    let k = slot + 1;
    let source_of = |device: &DeviceKey, socket: String| {
        doc.cables
            .iter()
            .find(|c| c.to.device == *device && c.to.socket == socket)
            .map(|c| &c.from)
    };
    // The effect fed by send `k`: its `in_l` is cabled from `fx_send{k}_l`.
    let first = doc.cables.iter().find(|c| {
        c.from.device == *master
            && c.from.socket == sockets::master_fx_send(k, "l")
            && c.to.socket == "in_l"
    })?;
    let e = &first.to.device;
    let complete = ["l", "r"].iter().all(|side| {
        let fed = source_of(e, format!("in_{side}"));
        let back = source_of(master, sockets::master_return(k, side));
        fed.is_some_and(|f| f.device == *master && f.socket == sockets::master_fx_send(k, side))
            && back.is_some_and(|b| b.device == *e && b.socket == format!("out_{side}"))
    });
    complete.then(|| e.clone())
}

/// Whether `effect` may be listed as a return: not a console device, not already on a return,
/// not the source of a channel (the rules `SetReturn` applies).
fn eligible(doc: &RackDoc, effect: &DeviceKey) -> bool {
    doc.device(effect)
        .is_some_and(|d| !is_console_type(&d.type_id))
        && doc.console.return_slot(effect).is_none()
        && doc.console.channel_of(effect).is_none()
}

/// The `(slot, effect)` pairs the cables of `doc` close as send/return loops while the slot is
/// empty, lowest slot first. Deterministic; an effect closing two loops takes the lower slot.
pub(crate) fn inferred_returns(doc: &RackDoc) -> Vec<(usize, DeviceKey)> {
    let mut found: Vec<(usize, DeviceKey)> = Vec::new();
    for slot in 0..FX_SLOTS {
        if doc.console.returns[slot].is_some() {
            continue;
        }
        let Some(effect) = loop_effect(doc, slot) else {
            continue;
        };
        if eligible(doc, &effect) && !found.iter().any(|(_, e)| *e == effect) {
            found.push((slot, effect));
        }
    }
    found
}

/// `edit`, a top-level cable change, plus the console returns it completes or breaks, in the
/// same undo step: a completed loop onto an empty slot registers the effect; a cut that opens
/// the loop of a registered return frees the slot. Other edits are returned as they are.
pub(crate) fn with_returns(doc: &RackDoc, edit: Edit) -> Edit {
    if doc.console.master.is_none() {
        return edit;
    }
    let mut after = doc.clone();
    if edit.perform(&mut after).is_err() {
        // Applying the edit reports the same fault, with the document unchanged.
        return edit;
    }
    let mut extra = Vec::new();
    for slot in 0..FX_SLOTS {
        match doc.console.returns[slot].clone() {
            Some(effect) => {
                let was = loop_effect(doc, slot).as_ref() == Some(&effect);
                let is = loop_effect(&after, slot).as_ref() == Some(&effect);
                if was && !is {
                    extra.push(Edit::ConsoleReturn {
                        slot,
                        old: Some(effect),
                        new: None,
                    });
                }
            }
            None => {
                let before = loop_effect(doc, slot);
                let now = loop_effect(&after, slot);
                if let Some(effect) = now
                    && before.as_ref() != Some(&effect)
                    && eligible(&after, &effect)
                    && !extra.iter().any(
                        |e| matches!(e, Edit::ConsoleReturn { new: Some(n), .. } if *n == effect),
                    )
                {
                    extra.push(Edit::ConsoleReturn {
                        slot,
                        old: None,
                        new: Some(effect),
                    });
                }
            }
        }
    }
    if extra.is_empty() {
        return edit;
    }
    let mut edits = vec![edit];
    edits.extend(extra);
    Edit::Compound(edits)
}
