//! `Command::ConvertSampleRate`: moves a project to another sample rate in one undo step.
//!
//! The audio itself is converted by the app (it owns the files): every asset the song uses is
//! resampled and registered as a new pool asset first. This plan then switches the document
//! rate and points every clip, take, frozen render and zone at the converted asset, scaling
//! every position that counts source or clip frames by the rate ratio. Positions in ticks
//! (clip starts and lengths, comps) do not change, so the arrangement stays where it is.

use crate::container::join;
use crate::history::Edit;
use crate::song::{AudioClipContent, ClipContent};
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, SAMPLE_RATES, ZoneDoc, zones};
use std::collections::BTreeMap;

/// `x * to / from`, rounded to the nearest frame.
pub(crate) fn scale(x: u64, to: u32, from: u32) -> u64 {
    let (to, from) = (u128::from(to), u128::from(from.max(1)));
    ((u128::from(x) * to + from / 2) / from).min(u128::from(u64::MAX)) as u64
}

struct Ctx<'a> {
    doc: &'a RackDoc,
    rate: u32,
    map: BTreeMap<&'a str, &'a str>,
}

impl Ctx<'_> {
    /// The asset a reference becomes and its source rate: the converted asset when mapped,
    /// the asset itself when it already has the new rate.
    fn target(&self, asset: &str) -> Result<(String, u64, u32), ModelError> {
        let old = self
            .doc
            .assets
            .get(asset)
            .ok_or_else(|| ModelError::MissingAsset(asset.to_owned()))?;
        match self.map.get(asset) {
            Some(to) => {
                let new = self
                    .doc
                    .assets
                    .get(*to)
                    .ok_or_else(|| ModelError::MissingAsset((*to).to_owned()))?;
                if new.sample_rate != self.rate || new.channels != old.channels {
                    return Err(ModelError::BadSong(format!(
                        "converted asset {to} must have {} channels at {} Hz",
                        old.channels, self.rate
                    )));
                }
                Ok(((*to).to_owned(), new.frames, old.sample_rate))
            }
            None if old.sample_rate == self.rate => Ok((asset.to_owned(), old.frames, self.rate)),
            None => Err(ModelError::BadSong(format!(
                "asset {asset} is used but has no converted asset at {} Hz",
                self.rate
            ))),
        }
    }

    fn audio(&self, a: &AudioClipContent) -> Result<AudioClipContent, ModelError> {
        let (to, frames, from_rate) = self.target(&a.asset)?;
        let s = |x: u64| scale(x, self.rate, from_rate);
        let mut n = a.clone();
        n.asset = to;
        n.source_offset = s(a.source_offset).min(frames);
        n.fade_in = s(a.fade_in);
        n.fade_out = s(a.fade_out);
        let mut last: Option<u64> = None;
        for p in &mut n.envelope {
            // Strictly increasing stays so when two points land on one frame.
            let f = s(p.frame).max(last.map_or(0, |l| l + 1));
            p.frame = f;
            last = Some(f);
        }
        let mut last: Option<u64> = None;
        for m in &mut n.warp.markers {
            let f = s(m.source).max(last.map_or(0, |l| l + 1));
            m.source = f;
            last = Some(f);
        }
        Ok(n)
    }

    fn zone(&self, z: &ZoneDoc) -> Result<ZoneDoc, ModelError> {
        let (to, frames, from_rate) = self.target(&z.asset)?;
        let s = |x: u64| scale(x, self.rate, from_rate);
        let mut n = z.clone();
        n.asset = to;
        n.end = s(z.end).min(frames);
        n.start = s(z.start).min(n.end.saturating_sub(1));
        n.loop_end = s(z.loop_end).min(frames);
        n.loop_start = s(z.loop_start).min(n.loop_end.saturating_sub(1));
        n.crossfade = s(z.crossfade);
        Ok(n)
    }

    fn clip(&self, c: &crate::song::Clip) -> Result<Option<crate::song::Clip>, ModelError> {
        let ClipContent::Audio(a) = &c.content else {
            return Ok(None);
        };
        let mut new = c.clone();
        new.content = ClipContent::Audio(self.audio(a)?);
        Ok(Some(new))
    }

    fn zone_edits(
        &self,
        devices: &[RackDevice],
        parent: Option<&DeviceKey>,
        out: &mut Vec<Edit>,
    ) -> Result<(), ModelError> {
        for d in devices {
            let path = join(parent, &d.key);
            if !d.zones.is_empty() {
                let new = d
                    .zones
                    .iter()
                    .map(|z| self.zone(z))
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(Edit::Zones {
                    key: path.clone(),
                    old: d.zones.clone(),
                    new,
                });
            }
            if let Some(inner) = &d.inner {
                self.zone_edits(&inner.devices, Some(&path), out)?;
            }
        }
        Ok(())
    }
}

/// Plans the conversion to `rate`. `assets` pairs each used asset with its converted copy
/// (both in the pool). Fails when a used asset has no copy, a copy has the wrong rate or
/// channel count, or a zone no longer fits.
pub(crate) fn plan(
    doc: &RackDoc,
    rate: u32,
    assets: &[(String, String)],
) -> Result<Edit, ModelError> {
    if !SAMPLE_RATES.contains(&rate) {
        return Err(ModelError::OutOfRange(format!(
            "{rate} Hz is not a supported sample rate"
        )));
    }
    if rate == doc.sample_rate {
        return Err(ModelError::BadSong(format!(
            "the project already runs at {rate} Hz"
        )));
    }
    let mut map = BTreeMap::new();
    for (from, to) in assets {
        if map.insert(from.as_str(), to.as_str()).is_some() {
            return Err(ModelError::BadSong(format!(
                "asset {from} is converted twice"
            )));
        }
    }
    let cx = Ctx { doc, rate, map };
    let mut edits = vec![Edit::SampleRate {
        old: doc.sample_rate,
        new: rate,
    }];
    for track in &doc.song.tracks {
        if let Some(f) = &track.frozen {
            let mut new = f.clone();
            new.audio = cx.audio(&f.audio)?;
            edits.push(Edit::Frozen {
                device: track.device.clone(),
                old: Some(f.clone()),
                new: Some(new),
            });
        }
        for (lane_index, lane) in track.lanes.iter().enumerate() {
            for clip in &lane.clips {
                if let Some(new) = cx.clip(clip)? {
                    edits.push(Edit::Clip {
                        device: track.device.clone(),
                        lane: lane_index,
                        old: Some(clip.clone()),
                        new: Some(new),
                    });
                }
            }
            if lane.take_groups.iter().any(|g| !g.takes.is_empty()) {
                let mut groups = lane.take_groups.clone();
                for c in groups.iter_mut().flat_map(|g| &mut g.takes) {
                    if let Some(n) = cx.clip(c)? {
                        *c = n;
                    }
                }
                if groups != lane.take_groups {
                    edits.push(Edit::TakeGroups {
                        device: track.device.clone(),
                        lane: lane_index,
                        old: lane.take_groups.clone(),
                        new: groups,
                    });
                }
            }
        }
    }
    let mut zone_edits = Vec::new();
    cx.zone_edits(&doc.devices, None, &mut zone_edits)?;
    for edit in &zone_edits {
        if let Edit::Zones { new, .. } = edit {
            zones::check_zones(&doc.assets, new).map_err(|(i, m)| {
                ModelError::BadZones(format!("zone {i} after conversion: {m}"))
            })?;
        }
    }
    edits.extend(zone_edits);
    Ok(Edit::Compound(edits))
}

#[cfg(test)]
mod tests {
    use super::scale;

    #[test]
    fn scale_rounds_to_the_nearest_frame() {
        assert_eq!(scale(44_100, 48_000, 44_100), 48_000);
        assert_eq!(scale(1, 48_000, 44_100), 1);
        assert_eq!(scale(0, 96_000, 44_100), 0);
        assert_eq!(scale(48_000, 44_100, 48_000), 44_100);
    }
}
