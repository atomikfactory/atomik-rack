//! `Command::RelinkAsset`: swaps every reference to one pool asset for another.

use crate::container::join;
use crate::history::Edit;
use crate::song::ClipContent;
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, ZoneDoc, zones};

/// Plans the relink of `from` to `to` as one compound edit: a `Clip` edit per audio clip of
/// `from` and a `Zones` edit per device (inside containers too) with a zone of `from`. Zone
/// ranges and clip source offsets are kept, so the replacement must be long enough for them.
pub(crate) fn plan(doc: &RackDoc, from: &str, to: &str) -> Result<Edit, ModelError> {
    let bad = |m: String| ModelError::BadSong(m);
    if !crate::is_valid_asset_hash(from) || !crate::is_valid_asset_hash(to) {
        return Err(bad("relink needs two valid asset ids".into()));
    }
    if from == to {
        return Err(bad(format!("asset {from} cannot be relinked to itself")));
    }
    if !doc.assets.contains_key(from) {
        return Err(bad(format!("asset {from} is not in the pool")));
    }
    let target = doc
        .assets
        .get(to)
        .ok_or_else(|| bad(format!("asset {to} is not in the pool (register it first)")))?;
    let mut edits = Vec::new();
    for track in &doc.song.tracks {
        for (lane_index, lane) in track.lanes.iter().enumerate() {
            for clip in &lane.clips {
                let ClipContent::Audio(a) = &clip.content else {
                    continue;
                };
                if a.asset != from {
                    continue;
                }
                if a.source_offset > target.frames {
                    return Err(bad(format!(
                        "a clip starts {} frames into the asset but the replacement has {}",
                        a.source_offset, target.frames
                    )));
                }
                let mut new = clip.clone();
                if let ClipContent::Audio(n) = &mut new.content {
                    n.asset = to.to_owned();
                }
                edits.push(Edit::Clip {
                    device: track.device.clone(),
                    lane: lane_index,
                    old: Some(clip.clone()),
                    new: Some(new),
                });
            }
            if lane
                .take_groups
                .iter()
                .flat_map(|g| &g.takes)
                .any(|c| matches!(&c.content, ClipContent::Audio(a) if a.asset == from))
            {
                let mut groups = lane.take_groups.clone();
                for c in groups.iter_mut().flat_map(|g| &mut g.takes) {
                    if let ClipContent::Audio(a) = &mut c.content
                        && a.asset == from
                    {
                        if a.source_offset > target.frames {
                            return Err(bad(format!(
                                "a take starts {} frames into the asset but the replacement has {}",
                                a.source_offset, target.frames
                            )));
                        }
                        a.asset = to.to_owned();
                    }
                }
                edits.push(Edit::TakeGroups {
                    device: track.device.clone(),
                    lane: lane_index,
                    old: lane.take_groups.clone(),
                    new: groups,
                });
            }
        }
    }
    let mut zone_edits = Vec::new();
    collect_zone_edits(&doc.devices, None, from, to, &mut zone_edits);
    let mut pool = doc.assets.clone();
    pool.entry(to.to_owned()).or_insert_with(|| target.clone());
    for edit in &zone_edits {
        if let Edit::Zones { new, .. } = edit {
            zones::check_zones(&pool, new).map_err(|(i, m)| {
                ModelError::BadZones(format!("zone {i} on the replacement: {m}"))
            })?;
        }
    }
    edits.extend(zone_edits);
    if edits.is_empty() {
        return Err(bad(format!("no clip or zone uses asset {from}")));
    }
    Ok(Edit::Compound(edits))
}

fn collect_zone_edits(
    devices: &[RackDevice],
    parent: Option<&DeviceKey>,
    from: &str,
    to: &str,
    out: &mut Vec<Edit>,
) {
    for d in devices {
        let path = join(parent, &d.key);
        if d.zones.iter().any(|z| z.asset == from) {
            let swap = |z: &ZoneDoc| ZoneDoc {
                asset: if z.asset == from {
                    to.to_owned()
                } else {
                    z.asset.clone()
                },
                ..z.clone()
            };
            out.push(Edit::Zones {
                key: path.clone(),
                old: d.zones.clone(),
                new: d.zones.iter().map(swap).collect(),
            });
        }
        if let Some(inner) = &d.inner {
            collect_zone_edits(&inner.devices, Some(&path), from, to, out);
        }
    }
}
