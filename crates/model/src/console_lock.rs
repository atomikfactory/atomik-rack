//! Locconsole.is_strip_or_master(strip)ed console strips.
//!
//! A channel, a bus or the master can be locked ([`ConsoleCommand::SetLocked`],
//! [`Console::locked`]): the strip then refuses every edit of its own values and structure, from
//! every source, so a finished fader, an EQ or an insert chain cannot move by accident. The lock
//! is a document edit like any other (undoable, saved with the project), and the lock itself is
//! the only thing a locked strip lets through.
//!
//! # What is locked
//!
//! The strip's params (fader, pan, width, sends, input, polarity, filters, EQ, dynamics, mute,
//! solo, parallel mix), its inserts (add, remove, reorder, bypass and the params of each effect:
//! the insert chain is derived from the cables, see [`crate::console_insert`]), its name and
//! colour, its removal, a change of its source and a change of how a macro drives it. Moving the
//! strip along the console, its output routing (a cable at a summing or output socket) and the
//! view state (folded, selected) are not part of the lock.
//!
//! **Automation still plays.** A lock is not a frozen value: automation lanes and the macros
//! that already drive a param of the strip keep moving it, because they are not edits of the
//! strip. Recording automation writes the lane, not the strip.
//!
//! # Where it is enforced
//!
//! In one place: [`crate::Document::apply`] screens the planned edit with [`screen`] before it
//! performs it, so the mouse, the keyboard, the wheel, MIDI learn, macros, the MCP server, paste,
//! reset, snapshot recall and every other source obey without a check of their own. Undo and redo
//! restore consistent states and never come here.
//!
//! An edit that touches a locked strip is refused with [`ModelError::Locked`] ("Channel Bass is
//! locked") and the document is unchanged. A command that edits several strips at once (a ganged
//! fader move, a snapshot recall, "Clear Mutes", a paste onto the selection) skips the locked
//! strips instead and edits the others, when what it would do to a locked strip is a plain value:
//! a param, a name, a colour. [`crate::Document::take_lock_skips`] says which strips were skipped
//! so the app can tell the user once. A structural edit (an insert, a removal, a re-cabling) is
//! never split: it is refused whole.

use crate::console::{ConsoleCommand, is_console_type};
use crate::history::{Edit, Swap};
use crate::{Cable, Command, Console, DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

impl Console {
    fn is_strip_or_master(&self, key: &DeviceKey) -> bool {
        self.channel_index(key).is_some()
            || self.bus_index(key).is_some()
            || self.master.as_ref() == Some(key)
    }

    /// Whether the strip `key` (a channel, a bus or the master) is locked.
    pub fn is_locked(&self, key: &DeviceKey) -> bool {
        self.locked.binary_search(key).is_ok()
    }
}

impl RackDoc {
    /// The locked strip that owns device `key`: the strip itself, or an effect of its insert
    /// chain (a device inside a container effect counts as the container). `None` when the
    /// device is not part of a locked strip.
    pub fn locked_owner(&self, registry: &Registry, key: &DeviceKey) -> Option<DeviceKey> {
        if self.console.locked.is_empty() {
            return None;
        }
        Guard::new(self, registry).owner(key).cloned()
    }
}

/// The text of the refusal.
fn locked_error(doc: &RackDoc, strip: &DeviceKey) -> ModelError {
    let name = doc
        .device(strip)
        .map_or(strip.0.as_str(), |d| d.name.as_str());
    ModelError::Locked(format!("Channel {name} is locked"))
}

/// The devices a lock covers and the strip each belongs to.
struct Guard<'a> {
    doc: &'a RackDoc,
    /// The locked strips, and the effects of their insert chains, by the strip that owns them.
    owners: BTreeMap<DeviceKey, DeviceKey>,
}

impl<'a> Guard<'a> {
    fn new(doc: &'a RackDoc, registry: &Registry) -> Self {
        let mut owners = BTreeMap::new();
        for strip in &doc.console.locked {
            owners.insert(strip.clone(), strip.clone());
            for effect in crate::console_insert::insert_chain(doc, registry, strip) {
                owners.insert(effect, strip.clone());
            }
        }
        Self { doc, owners }
    }

    /// The locked strip that owns `key` (the first segment of a container path key counts).
    fn owner(&self, key: &DeviceKey) -> Option<&DeviceKey> {
        self.owners.get(key).or_else(|| {
            key.0
                .split_once('/')
                .and_then(|(top, _)| self.owners.get(&DeviceKey(top.to_owned())))
        })
    }

    /// Whether `key` is a locked strip itself (not one of its effects).
    fn is_strip(&self, key: &DeviceKey) -> bool {
        self.doc.console.is_locked(key)
    }

    /// The locked strip a cable belongs to: one end is an effect of its chain or the strip's own
    /// insert send or return socket.
    fn cable(&self, cable: &Cable) -> Option<&DeviceKey> {
        [&cable.from, &cable.to].into_iter().find_map(|end| {
            let owner = self.owner(&end.device)?;
            (!self.is_strip(&end.device) || end.socket.starts_with("insert_")).then_some(owner)
        })
    }

    /// The locked strip `edit` touches, if any.
    fn touched<'b>(&self, edit: &'b Edit) -> Option<&DeviceKey> {
        match edit {
            Edit::Param { key, .. }
            | Edit::PluginState { key, .. }
            | Edit::PluginPins { key, .. }
            | Edit::PluginRun { key, .. }
            | Edit::PluginPower { key, .. }
            | Edit::PluginRelink { key, .. }
            | Edit::Swap(
                Swap::Name { key, .. }
                | Swap::Color { key, .. }
                | Swap::PatchName { key, .. }
                | Swap::ModSlots { key, .. },
            ) => self.owner(key),
            Edit::Insert { device, cables, .. } | Edit::Remove { device, cables, .. } => self
                .owner(&device.key)
                .or_else(|| cables.iter().find_map(|c| self.cable(c))),
            Edit::Connect(cable) | Edit::Disconnect(cable) => self.cable(cable),
            Edit::ConsoleChannel { old: Some(c), .. } => self.owner(&c.channel),
            Edit::ConsoleBus { old: Some(b), .. } => self.owner(&b.bus),
            Edit::ConsoleMaster { old: Some(m), .. } => self.owner(m),
            Edit::ConsoleReturn { .. } => {
                self.doc.console.master.as_ref().and_then(|m| self.owner(m))
            }
            Edit::Macros { old, new } => {
                let driven = |set: &'b crate::MacroSet| {
                    set.assigns
                        .iter()
                        .filter(|a| self.owner(&a.device).is_some())
                        .collect::<Vec<_>>()
                };
                let (before, after) = (driven(old), driven(new));
                if before == after {
                    return None;
                }
                before
                    .iter()
                    .chain(&after)
                    .find_map(|a| self.owner(&a.device))
            }
            _ => None,
        }
    }

    /// A plain value of one strip, which a command on several strips may leave out.
    fn is_value(edit: &Edit) -> bool {
        matches!(
            edit,
            Edit::Param { .. }
                | Edit::Swap(Swap::Name { .. } | Swap::Color { .. } | Swap::PatchName { .. })
        )
    }
}

/// Puts the edits of a compound in one flat list (the order is kept).
fn flatten(edit: Edit, out: &mut Vec<Edit>) {
    match edit {
        Edit::Compound(items) => items.into_iter().for_each(|e| flatten(e, out)),
        other => out.push(other),
    }
}

/// Screens a planned edit against the locks of `doc`: the edit to perform, with the plain value
/// edits of locked strips left out, and the strips that were left out; or the refusal. The edit
/// is returned as it is when nothing is locked (the usual case costs one test).
pub(crate) fn screen(
    doc: &RackDoc,
    registry: &Registry,
    edit: Edit,
) -> Result<(Edit, Vec<DeviceKey>), ModelError> {
    if doc.console.locked.is_empty() {
        return Ok((edit, Vec::new()));
    }
    let guard = Guard::new(doc, registry);
    let mut items = Vec::new();
    let whole = !matches!(edit, Edit::Compound(_));
    flatten(edit, &mut items);
    let mut kept = Vec::with_capacity(items.len());
    let mut skipped: Vec<DeviceKey> = Vec::new();
    for item in items {
        match guard.touched(&item) {
            None => kept.push(item),
            Some(strip) if !whole && Guard::is_value(&item) => {
                if !skipped.contains(strip) {
                    skipped.push(strip.clone());
                }
            }
            Some(strip) => return Err(locked_error(doc, strip)),
        }
    }
    if kept.is_empty() {
        return Err(skipped.first().map_or_else(
            || ModelError::Locked("A channel is locked".into()),
            |s| locked_error(doc, s),
        ));
    }
    let edit = if whole {
        kept.pop().expect("one edit")
    } else {
        Edit::Compound(kept)
    };
    Ok((edit, skipped))
}

/// Takes the locked strips out of a paste onto several strips (the others still take it); the
/// refusal when every strip is locked. Every other command passes unchanged.
pub(crate) fn trim_command(
    doc: &RackDoc,
    command: Command,
) -> Result<(Command, Vec<DeviceKey>), ModelError> {
    let Command::Console(ConsoleCommand::PasteSettings { strips, clip }) = command else {
        return Ok((command, Vec::new()));
    };
    if doc.console.locked.is_empty() {
        return Ok((
            Command::Console(ConsoleCommand::PasteSettings { strips, clip }),
            Vec::new(),
        ));
    }
    let (skipped, strips): (Vec<_>, Vec<_>) =
        strips.into_iter().partition(|s| doc.console.is_locked(s));
    if strips.is_empty()
        && let Some(first) = skipped.first()
    {
        return Err(locked_error(doc, first));
    }
    Ok((
        Command::Console(ConsoleCommand::PasteSettings { strips, clip }),
        skipped,
    ))
}

/// Plans [`ConsoleCommand::SetLocked`]: the new sorted list of locked strips.
pub(crate) fn plan_set(
    doc: &RackDoc,
    strips: &[DeviceKey],
    locked: bool,
) -> Result<Edit, ModelError> {
    if strips.is_empty() {
        return Err(crate::console_cmd::bad("there is no strip to lock"));
    }
    let console = &doc.console;
    let mut new = console.locked.clone();
    for strip in strips {
        let is_strip = console.is_strip_or_master(strip);
        if !is_strip || !is_console_type(&crate::rules::device(doc, strip)?.type_id) {
            return Err(crate::console_cmd::bad(format!(
                "{strip} is not a console strip"
            )));
        }
        match (new.binary_search(strip), locked) {
            (Err(at), true) => new.insert(at, strip.clone()),
            (Ok(at), false) => {
                new.remove(at);
            }
            _ => {}
        }
    }
    if new == console.locked {
        return Err(crate::console_cmd::bad(if locked {
            "the strips are already locked"
        } else {
            "the strips are not locked"
        }));
    }
    Ok(Edit::Swap(Swap::Locks {
        old: console.locked.clone(),
        new,
    }))
}
