//! The mixer group a device belongs to, for the chip on its header: a channel or a bus that is a
//! member of a VCA group, and the instrument whose automatic channel is one. A pure function of
//! the document, cached per sync by the view.

use atomik_model::{DeviceKey, RackDoc};
use std::collections::BTreeMap;

/// The group a device's header names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChip {
    /// The group's name.
    pub name: String,
    /// The group's colour (`None`: the neutral tag).
    pub color: Option<[u8; 3]>,
}

/// The chip of every device that is in a group, by device key.
pub fn chips_of(doc: &RackDoc) -> BTreeMap<DeviceKey, GroupChip> {
    let mut out = BTreeMap::new();
    for g in &doc.console.vcas {
        let chip = GroupChip {
            name: g.name.clone(),
            color: g.color,
        };
        for member in &g.members {
            out.insert(member.clone(), chip.clone());
            // The instrument of an automatic channel wears its channel's group.
            if let Some(source) = doc
                .console
                .channels
                .iter()
                .find(|c| &c.channel == member)
                .and_then(|c| c.source.clone())
            {
                out.insert(source, chip.clone());
            }
        }
    }
    out
}
