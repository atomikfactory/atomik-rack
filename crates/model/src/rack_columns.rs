//! Rack columns: the top-level rack is shown as `RackDoc::columns` columns side by side.
//!
//! The document keeps one flat device list, **stable-sorted by column**, so every column is a
//! contiguous run of `RackDoc::devices` and nothing that reads the flat order (graph build,
//! engine, console, song) knows about columns. A position in the rack is a [`RackSlot`]
//! (column, index within the column); [`RackDoc::resolve_slot`] turns it into the flat
//! insertion index. Container insides are always a single column (column 0).
//!
//! Everything that changes a column is an [`Edit::Layout`] (the whole `(key, column)` order,
//! used by moves and column removal) or an [`Edit::Columns`] (the column count); both report
//! [`crate::SyncOp::ViewChanged`] only, so a column edit never rebuilds the audio graph.

use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, rules};
use std::ops::Range;

/// Most columns a rack may have.
pub const MAX_RACK_COLUMNS: u16 = 32;

/// A position in the top-level rack: `index` devices into column `column`.
///
/// `index` counts the devices already in the column (for a move: the devices that stay in it
/// once the moved ones are taken out); an index past the end means "at the end of the column".
/// `column == RackDoc::columns` (one past the last) means "in a new column appended on the
/// right", created by the same undo step. Inside a container only `index` is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RackSlot {
    /// Column, from 0 at the left.
    pub column: u16,
    /// Position within the column, from 0 at the top.
    pub index: usize,
}

/// A [`RackSlot`] resolved against a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedSlot {
    /// Flat insertion index into `RackDoc::devices`.
    pub index: usize,
    /// The column the device gets.
    pub column: u16,
    /// True when the slot asked for a new column to the right of the last one.
    pub adds_column: bool,
}

/// Where a moved or created device goes (internal: flat commands and slot commands share the
/// planners).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Placement {
    /// A flat rack index; the device takes the column of the device before it (column 0 at
    /// the top).
    Flat(usize),
    /// A column slot.
    Slot(RackSlot),
}

impl RackDoc {
    /// The flat index range of column `column` in `devices` (empty for an empty column; the
    /// range then sits where the column would start).
    pub fn column_range(&self, column: u16) -> Range<usize> {
        let start = self.devices.partition_point(|d| d.column < column);
        let end = self.devices.partition_point(|d| d.column <= column);
        start..end
    }

    /// The slot a top-level device occupies now: its column and its index within it.
    pub fn slot_of(&self, key: &DeviceKey) -> Option<RackSlot> {
        let at = self.devices.iter().position(|d| &d.key == key)?;
        let column = self.devices[at].column;
        Some(RackSlot {
            column,
            index: at - self.column_range(column).start,
        })
    }

    /// Resolves `slot` for inserting a new device: the flat index, the column and whether a
    /// column is appended. The index is clamped to the column, and column 0 never goes above
    /// the Master Section while it is the first device.
    ///
    /// # Errors
    /// [`ModelError::BadColumn`] when `slot.column` is past `columns` (or `columns` while the
    /// rack already has [`MAX_RACK_COLUMNS`]).
    pub fn resolve_slot(&self, slot: RackSlot) -> Result<ResolvedSlot, ModelError> {
        if slot.column > self.columns {
            return Err(bad_column(format!(
                "column {} does not exist (the rack has {})",
                slot.column, self.columns
            )));
        }
        if slot.column == self.columns {
            check_room(self)?;
            return Ok(ResolvedSlot {
                index: self.devices.len(),
                column: slot.column,
                adds_column: true,
            });
        }
        let range = self.column_range(slot.column);
        let mut at = slot.index.min(range.len());
        if slot.column == 0 && rules::master_first(self) {
            at = at.max(1).min(range.len());
        }
        Ok(ResolvedSlot {
            index: range.start + at,
            column: slot.column,
            adds_column: false,
        })
    }

    /// The column a device inserted at flat `index` takes by default: that of the device
    /// before it, or 0 at the top.
    pub(crate) fn column_at(&self, index: usize) -> u16 {
        index
            .checked_sub(1)
            .and_then(|i| self.devices.get(i))
            .map_or(0, |d| d.column)
    }

    /// The lowest and highest column a device inserted at flat `index` may have without
    /// breaking the sort order.
    pub(crate) fn column_bounds(&self, index: usize) -> (u16, u16) {
        let lo = self.column_at(index);
        let hi = self
            .devices
            .get(index)
            .map_or(self.columns.saturating_sub(1), |d| d.column);
        (lo, hi.max(lo))
    }

    /// Checks the column invariant: at least 1 and at most [`MAX_RACK_COLUMNS`] columns, every
    /// device in a column that exists, the device list sorted by column and every device
    /// inside a container in column 0.
    ///
    /// # Errors
    /// A message naming the first fault.
    pub fn check_columns(&self) -> Result<(), String> {
        if !(1..=MAX_RACK_COLUMNS).contains(&self.columns) {
            return Err(format!(
                "{} columns is outside 1..={MAX_RACK_COLUMNS}",
                self.columns
            ));
        }
        let mut last = 0;
        for (i, d) in self.devices.iter().enumerate() {
            if d.column >= self.columns {
                return Err(format!(
                    "devices[{i}] ({}) is in column {} of {}",
                    d.key, d.column, self.columns
                ));
            }
            if d.column < last {
                return Err(format!(
                    "devices[{i}] ({}) is in column {} after column {last}",
                    d.key, d.column
                ));
            }
            last = d.column;
            if let Err(path) = inner_columns_zero(d, &d.key.0) {
                return Err(format!("{path} is not in column 0"));
            }
        }
        Ok(())
    }
}

/// `Err(path)` of the first device inside `d` (a container) that is not in column 0.
fn inner_columns_zero(d: &RackDevice, path: &str) -> Result<(), String> {
    let Some(inner) = &d.inner else {
        return Ok(());
    };
    for i in &inner.devices {
        let p = format!("{path}/{}", i.key);
        if i.column != 0 {
            return Err(p);
        }
        inner_columns_zero(i, &p)?;
    }
    Ok(())
}

fn bad_column(message: String) -> ModelError {
    ModelError::BadColumn(message)
}

fn check_room(doc: &RackDoc) -> Result<(), ModelError> {
    if doc.columns >= MAX_RACK_COLUMNS {
        return Err(bad_column(format!(
            "a rack has at most {MAX_RACK_COLUMNS} columns"
        )));
    }
    Ok(())
}

/// The `(key, column)` of every device, in rack order.
fn layout_of(doc: &RackDoc) -> Vec<(DeviceKey, u16)> {
    doc.devices
        .iter()
        .map(|d| (d.key.clone(), d.column))
        .collect()
}

/// `AddRackColumn`.
pub(crate) fn plan_add_column(doc: &RackDoc) -> Result<Edit, ModelError> {
    check_room(doc)?;
    Ok(Edit::Columns {
        old: doc.columns,
        new: doc.columns + 1,
    })
}

/// `RemoveRackColumn`: the flat order stays, so the devices of the removed column land at the
/// end of the left column (the start of the next one for column 0), and higher columns shift
/// down.
pub(crate) fn plan_remove_column(doc: &RackDoc, column: u16) -> Result<Edit, ModelError> {
    if column >= doc.columns {
        return Err(bad_column(format!(
            "column {column} does not exist (the rack has {})",
            doc.columns
        )));
    }
    if doc.columns == 1 {
        return Err(bad_column("the last column cannot be removed".into()));
    }
    let old = layout_of(doc);
    let new = old
        .iter()
        .map(|(k, c)| {
            let c = match *c {
                c if c == column => column.saturating_sub(1),
                c if c > column => c - 1,
                c => c,
            };
            (k.clone(), c)
        })
        .collect();
    Ok(Edit::Compound(vec![
        Edit::Layout { old, new },
        Edit::Columns {
            old: doc.columns,
            new: doc.columns - 1,
        },
    ]))
}

/// Moves the top-level `group` (any order) together to `place`, keeping their relative rack
/// order. Plans a [`Edit::Layout`] (preceded by [`Edit::Columns`] when the slot asks for a new
/// column). The result may change nothing; callers that reject a no-op compare the layouts
/// with [`is_noop`].
pub(crate) fn plan_move(
    doc: &RackDoc,
    group: &[DeviceKey],
    place: Placement,
) -> Result<Edit, ModelError> {
    let old = layout_of(doc);
    let (moving, rest): (Vec<_>, Vec<_>) =
        old.iter().cloned().partition(|(k, _)| group.contains(k));
    let mut adds = false;
    let (at, column) = match place {
        Placement::Flat(to) => {
            let at = to.min(rest.len());
            (at, at.checked_sub(1).map_or(0, |i| rest[i].1))
        }
        Placement::Slot(slot) => {
            if slot.column > doc.columns {
                return Err(bad_column(format!(
                    "column {} does not exist (the rack has {})",
                    slot.column, doc.columns
                )));
            }
            if slot.column == doc.columns {
                check_room(doc)?;
                adds = true;
                (rest.len(), slot.column)
            } else {
                let start = rest.partition_point(|(_, c)| *c < slot.column);
                let count = rest.partition_point(|(_, c)| *c <= slot.column) - start;
                let mut idx = slot.index.min(count);
                if slot.column == 0 && rules::master_first(doc) {
                    idx = idx.max(1).min(count);
                }
                (start + idx, slot.column)
            }
        }
    };
    let mut new = rest[..at].to_vec();
    new.extend(moving.into_iter().map(|(k, _)| (k, column)));
    new.extend_from_slice(&rest[at..]);
    // The master stays where it is, and nothing goes above it.
    rules::check_move(doc, group, new.first().map(|(k, _)| k))?;
    let layout = Edit::Layout { old, new };
    Ok(if adds {
        Edit::Compound(vec![
            Edit::Columns {
                old: doc.columns,
                new: doc.columns + 1,
            },
            layout,
        ])
    } else {
        layout
    })
}

/// True if a [`plan_move`] edit changes neither the order nor any column.
pub(crate) fn is_noop(edit: &Edit) -> bool {
    match edit {
        Edit::Layout { old, new } => old == new,
        _ => false,
    }
}

/// Puts the new device of a creation `edit` (its first `Insert`) at the resolved `slot`:
/// sets the flat index and the column, and prepends the new column when the slot adds one.
///
/// # Panics
/// If `edit` holds no insert, which a successful creation always does.
pub(crate) fn place_new_device(doc: &RackDoc, slot: ResolvedSlot, edit: Edit) -> Edit {
    let mut edits = match edit {
        Edit::Compound(edits) => edits,
        single => vec![single],
    };
    let first = edits
        .iter_mut()
        .find_map(|e| match e {
            Edit::Insert { index, device, .. } => Some((index, device)),
            _ => None,
        })
        .expect("a creation inserts its device");
    *first.0 = slot.index;
    first.1.column = slot.column;
    if slot.adds_column {
        edits.insert(
            0,
            Edit::Columns {
                old: doc.columns,
                new: doc.columns + 1,
            },
        );
    }
    if edits.len() == 1 {
        edits.pop().expect("one edit")
    } else {
        Edit::Compound(edits)
    }
}
