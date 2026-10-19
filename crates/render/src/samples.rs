//! Sample maps from document zones: the one
//! builder the app and the bounce share, so a sampler gets the same map live and offline.
//!
//! A sampler's zones name pool assets; the caller says how an asset's audio is held
//! (`buffer_of`). Zones whose audio is not available are left out of the map (they play
//! silence). Buffers are shared by [`Arc`]: zones of one asset use one
//! [`SampleBuffer`], and a buffer built from an audio track's memory asset shares its sample
//! data ([`song_asset_buffer`]). The map is built off the audio thread.

use atomik_device_sdk::{SampleBuffer, SampleMap, SampleMapRef};
use atomik_engine::SongAsset;
use atomik_model::ZoneDoc;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The zones' sample map, and what had to be left out.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltMap {
    /// The map, `None` when no zone is playable.
    pub map: Option<SampleMapRef>,
    /// One message per zone left out because the map rejected it (a zone that is merely
    /// waiting for its audio is not listed).
    pub rejected: Vec<String>,
}

/// The buffer of a memory song asset, sharing its sample data; `None` for a streamed asset
/// (samplers always play from RAM) or malformed data.
pub fn song_asset_buffer(asset: &SongAsset) -> Option<Arc<SampleBuffer>> {
    match asset {
        SongAsset::Memory { data, .. } => SampleBuffer::new(Arc::clone(data)).ok().map(Arc::new),
        SongAsset::Stream { .. } => None,
    }
}

/// Builds the [`SampleMap`] of `zones`. `buffer_of(asset id)` returns the asset's audio, or
/// `None` when it is missing or not loaded yet: those zones are left out. Buffers appear in
/// the map in order of first use.
pub fn build_sample_map(
    zones: &[ZoneDoc],
    mut buffer_of: impl FnMut(&str) -> Option<Arc<SampleBuffer>>,
) -> BuiltMap {
    let mut buffers: Vec<Arc<SampleBuffer>> = Vec::new();
    let mut index: BTreeMap<&str, Option<u32>> = BTreeMap::new();
    let mut playable: Vec<(usize, ZoneDoc, u32)> = Vec::new();
    for (i, z) in zones.iter().enumerate() {
        let slot = *index.entry(z.asset.as_str()).or_insert_with(|| {
            buffer_of(&z.asset).map(|b| {
                buffers.push(b);
                u32::try_from(buffers.len() - 1).unwrap_or(u32::MAX)
            })
        });
        if let Some(b) = slot {
            playable.push((i, z.clone(), b));
        }
    }
    let to_zones = |list: &[(usize, ZoneDoc, u32)]| -> Vec<_> {
        list.iter().map(|(_, z, b)| z.to_zone(*b)).collect()
    };
    if playable.is_empty() {
        return BuiltMap {
            map: None,
            rejected: Vec::new(),
        };
    }
    if let Ok(map) = SampleMap::new(buffers.clone(), to_zones(&playable)) {
        return BuiltMap {
            map: Some(Arc::new(map)),
            rejected: Vec::new(),
        };
    }
    // Some zone does not fit its loaded audio (e.g. the file changed length): keep the others.
    let mut rejected = Vec::new();
    let mut kept = Vec::new();
    for entry in playable {
        match SampleMap::new(buffers.clone(), to_zones(std::slice::from_ref(&entry))) {
            Ok(_) => kept.push(entry),
            Err(e) => rejected.push(format!("zone {}: {e}", entry.0 + 1)),
        }
    }
    let map = if kept.is_empty() {
        None
    } else {
        SampleMap::new(buffers, to_zones(&kept)).ok().map(Arc::new)
    };
    BuiltMap { map, rejected }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(frames: usize) -> Arc<SampleBuffer> {
        let data: Arc<[Vec<f32>]> = Arc::from(vec![vec![0.25; frames]]);
        Arc::new(SampleBuffer::new(data).unwrap())
    }

    fn asset(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn zone(a: char, frames: u64, keys: (u8, u8)) -> ZoneDoc {
        let mut z = ZoneDoc::new(asset(a), frames);
        (z.key_lo, z.key_hi) = keys;
        z
    }

    #[test]
    fn zones_of_one_asset_share_one_buffer_in_order_of_first_use() {
        let (a, b) = (buffer(100), buffer(50));
        let zones = [
            zone('b', 50, (0, 10)),
            zone('a', 100, (11, 20)),
            zone('b', 50, (21, 30)),
        ];
        let built = build_sample_map(&zones, |id| {
            Some(Arc::clone(if id == asset('a') { &a } else { &b }))
        });
        let map = built.map.unwrap();
        assert!(built.rejected.is_empty());
        assert_eq!(map.buffers().len(), 2);
        assert!(Arc::ptr_eq(&map.buffers()[0], &b));
        assert!(Arc::ptr_eq(&map.buffers()[1], &a));
        let used: Vec<u32> = map.zones().iter().map(|z| z.buffer).collect();
        assert_eq!(used, [0, 1, 0]);
    }

    #[test]
    fn zones_without_audio_are_left_out_and_none_means_no_map() {
        let a = buffer(100);
        let zones = [zone('a', 100, (0, 10)), zone('m', 100, (11, 20))];
        let only_a = |id: &str| (id == asset('a')).then(|| Arc::clone(&a));
        let built = build_sample_map(&zones, only_a);
        assert_eq!(built.map.unwrap().zones().len(), 1);
        assert!(built.rejected.is_empty());
        assert!(build_sample_map(&zones, |_| None).map.is_none());
        assert!(build_sample_map(&[], only_a).map.is_none());
    }

    #[test]
    fn a_zone_that_does_not_fit_its_audio_is_dropped_alone() {
        let a = buffer(100);
        // The second zone claims 200 frames of a 100-frame buffer.
        let zones = [zone('a', 100, (0, 10)), zone('a', 200, (11, 20))];
        let built = build_sample_map(&zones, |_| Some(Arc::clone(&a)));
        assert_eq!(built.map.unwrap().zones().len(), 1);
        assert_eq!(built.rejected.len(), 1);
        assert!(
            built.rejected[0].starts_with("zone 2:"),
            "{:?}",
            built.rejected
        );
    }

    #[test]
    fn memory_assets_share_their_samples_and_streams_give_nothing() {
        let data: Arc<[Vec<f32>]> = Arc::from(vec![vec![0.5; 10]]);
        let asset = SongAsset::memory(Arc::clone(&data));
        let b = song_asset_buffer(&asset).unwrap();
        assert!(Arc::ptr_eq(&b.data, &data));
        struct Nothing;
        impl atomik_engine::StreamSource for Nothing {
            fn read(&self, _: u64, _: &mut [Vec<f32>]) -> usize {
                0
            }
        }
        let streamed = SongAsset::stream(10, 1, Arc::new(Nothing));
        assert!(song_asset_buffer(&streamed).is_none());
    }
}
