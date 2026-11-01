//! Sampler zones in the document.
//!
//! A [`ZoneDoc`] is the SDK [`Zone`] with a pool asset id instead of a buffer index. Zones are
//! validated with the SDK's own rules ([`atomik_device_sdk::SampleMap::new`] on throwaway,
//! data-less buffers), so the model and the engine can never disagree about what a legal
//! zone is. [`auto_map`] turns dropped files into zones; [`parse_note_name`] reads the root
//! key from a file name.

use crate::{AssetMeta, is_valid_asset_hash};
use atomik_device_sdk::{
    LoopMode, SampleBuffer, SampleMap, SampleMapError, TriggerMode, Zone, ZoneExt,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One zone of a sampler device, as stored in the document. Fields and ranges are those of
/// [`Zone`]; `asset` (a pool id) replaces the buffer index, and frames are checked against
/// [`AssetMeta::frames`].
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneDoc {
    /// Pool asset id (64 lowercase hex digits, a key of `RackDoc::assets`).
    pub asset: String,
    /// Lowest key, 0..=127.
    pub key_lo: u8,
    /// Highest key, `key_lo..=127`.
    pub key_hi: u8,
    /// Lowest velocity, 1..=127.
    pub vel_lo: u8,
    /// Highest velocity, `vel_lo..=127`.
    pub vel_hi: u8,
    /// The key at which the sample plays at its recorded pitch, 0..=127.
    pub root_key: u8,
    /// Transpose in semitones, -48..=48.
    pub transpose: i8,
    /// Fine tune in cents, -100..=100.
    pub tune_cents: f32,
    /// Gain in dB, -60..=12.
    pub gain_db: f32,
    /// Pan, -1..=1.
    pub pan: f32,
    /// First frame played.
    pub start: u64,
    /// End frame (exclusive): `start < end <= asset frames`.
    pub end: u64,
    /// Loop mode.
    pub loop_mode: LoopMode,
    /// Loop start frame (checked only when looping).
    pub loop_start: u64,
    /// Loop end frame, exclusive (checked only when looping).
    pub loop_end: u64,
    /// Crossfade length in frames (checked only when looping).
    pub crossfade: u64,
    /// Note-off behaviour.
    pub trigger: TriggerMode,
    /// Play backwards from `end - 1` down to `start` (loops are then ignored, as in [`Zone`]).
    pub reverse: bool,
    /// Group, velocity crossfade, round-robin and the zone's own envelope and filter
    /// ([`ZoneExt`]). Saved only when it differs from [`ZoneExt::NONE`].
    pub ext: ZoneExt,
}

impl ZoneDoc {
    /// A zone over the whole asset: every key and velocity, root C4 (60), the whole sample
    /// (`0..frames`), unity gain and centre pan, forward, loop off (the loop fields span the sample, so
    /// switching the loop on is valid), gate.
    pub fn new(asset: impl Into<String>, frames: u64) -> Self {
        Self {
            asset: asset.into(),
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            root_key: 60,
            transpose: 0,
            tune_cents: 0.0,
            gain_db: 0.0,
            pan: 0.0,
            start: 0,
            end: frames,
            loop_mode: LoopMode::Off,
            loop_start: 0,
            loop_end: frames,
            crossfade: 0,
            trigger: TriggerMode::Gate,
            reverse: false,
            ext: ZoneExt::NONE,
        }
    }

    /// The SDK zone for buffer index `buffer` of a [`SampleMap`].
    pub fn to_zone(&self, buffer: u32) -> Zone {
        Zone {
            buffer,
            key_lo: self.key_lo,
            key_hi: self.key_hi,
            vel_lo: self.vel_lo,
            vel_hi: self.vel_hi,
            root_key: self.root_key,
            transpose: self.transpose,
            tune_cents: self.tune_cents,
            gain_db: self.gain_db,
            pan: self.pan,
            start: self.start,
            end: self.end,
            loop_mode: self.loop_mode,
            loop_start: self.loop_start,
            loop_end: self.loop_end,
            crossfade: self.crossfade,
            trigger: self.trigger,
            reverse: self.reverse,
            ext: self.ext,
        }
    }

    /// Whether the zone sounds for `key` at 7-bit velocity `vel7` (as [`Zone::contains`]).
    pub fn contains(&self, key: u8, vel7: u8) -> bool {
        (self.key_lo..=self.key_hi).contains(&key) && (self.vel_lo..=self.vel_hi).contains(&vel7)
    }
}

/// The indices of the zones that sound for `key` at 7-bit velocity `vel7`, in list order
/// (every match sounds: zones layer).
pub fn zones_for(key: u8, vel7: u8, zones: &[ZoneDoc]) -> impl Iterator<Item = usize> + '_ {
    zones
        .iter()
        .enumerate()
        .filter(move |(_, z)| z.contains(key, vel7))
        .map(|(i, _)| i)
}

/// Checks `zones` against the pool: each asset is a valid id in `assets`, and the zone passes
/// [`SampleMap::new`] against a buffer of the asset's frame count. On failure, the index of
/// the first bad zone and what is wrong with it.
pub(crate) fn check_zones(
    assets: &BTreeMap<String, AssetMeta>,
    zones: &[ZoneDoc],
) -> Result<(), (usize, String)> {
    // Only `frames` matters to the validator, so no sample data is allocated.
    let no_data: Arc<[Vec<f32>]> = Arc::new([Vec::new()]);
    for (i, z) in zones.iter().enumerate() {
        if !is_valid_asset_hash(&z.asset) {
            return Err((i, format!("{:?} is not a valid asset id", z.asset)));
        }
        let meta = assets
            .get(&z.asset)
            .ok_or_else(|| (i, format!("asset {:?} is not in the assets pool", z.asset)))?;
        let buffer = Arc::new(SampleBuffer {
            data: no_data.clone(),
            frames: meta.frames,
            channels: meta.channels,
        });
        SampleMap::new(vec![buffer], vec![z.to_zone(0)]).map_err(|e| {
            let what = match e {
                SampleMapError::OutOfRange { field, .. } => format!("`{field}` is out of range"),
                SampleMapError::BadFrames { field, .. } => {
                    format!("`{field}` breaks the frame ordering")
                }
                other => other.to_string(),
            };
            (i, what)
        })?;
    }
    Ok(())
}

/// The note name or MIDI number in a file stem, as a key. C4 = 60.
///
/// A **note name** is a letter `A`-`G` (either case), an optional accidental (`#` sharp or
/// lowercase `b` flat) and one octave, `-1` to `9`, written as a digit or `-1` (so `A-1` is
/// the note A in octave -1). It counts only when what precedes it is the start of the stem
/// or a character that is not a letter or digit, and what follows is the end of the stem or
/// such a character: `Piano_C#3`, `Kick C1`, `str-A-1`, `vln_Db4_ff`, but not the `Be4` in
/// `Be4t` or the `Ba` in `Bass`. The **last** note name in the stem wins; if it lies outside
/// 0..=127 (`C-1`..=`G9`), as `G#9` or `Cb-1` do, the stem has no root at all.
///
/// With no note name, a **MIDI number** counts instead: a run of one to three digits, 0..=127,
/// delimited the same way (`kick_060_x`). The last one wins. Otherwise `None`.
pub fn parse_note_name(file_stem: &str) -> Option<u8> {
    let c: Vec<char> = file_stem.chars().collect();
    let boundary_before = |i: usize| i == 0 || !c[i - 1].is_alphanumeric();
    let boundary_after = |i: usize| i >= c.len() || !c[i].is_alphanumeric();
    let note_at = |start: usize| -> Option<i32> {
        let mut i = start;
        let letter = c.get(i)?.to_ascii_uppercase();
        let mut semitone: i32 = match letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        i += 1;
        match c.get(i) {
            Some('#') => {
                semitone += 1;
                i += 1;
            }
            Some('b') => {
                semitone -= 1;
                i += 1;
            }
            _ => {}
        }
        let negative = c.get(i) == Some(&'-');
        if negative {
            i += 1;
        }
        let digit = c.get(i)?.to_digit(10)? as i32;
        i += 1;
        if !boundary_after(i) {
            return None;
        }
        let octave = if negative { -digit } else { digit };
        Some((octave + 1) * 12 + semitone)
    };
    let by_name = (0..c.len())
        .rev()
        .filter(|&i| boundary_before(i))
        .find_map(note_at);
    if let Some(key) = by_name {
        // A note-shaped token outside 0..=127 means no root, never a stray digit of it.
        return u8::try_from(key).ok().filter(|k| *k <= 127);
    }
    let mut i = c.len();
    while i > 0 {
        if !c[i - 1].is_ascii_digit() {
            i -= 1;
            continue;
        }
        let end = i;
        while i > 0 && c[i - 1].is_ascii_digit() {
            i -= 1;
        }
        if end - i <= 3 && boundary_before(i) && boundary_after(end) {
            let n: u32 = c[i..end]
                .iter()
                .fold(0, |a, d| a * 10 + d.to_digit(10).unwrap());
            if n <= 127 {
                return Some(n as u8);
            }
        }
    }
    None
}

/// The file name without its directory and last extension.
fn stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => base,
    }
}

/// Zones for dropped `files` (`(asset id, original file name, frames)`). Every zone
/// covers all velocities and the whole asset, with the loop off.
///
/// * **Every file has a root** ([`parse_note_name`] of its stem; the extension and directory
///   are ignored): the zones are sorted by root, ties by file name, and each root's key range
///   runs from just above the halfway point to the previous root (`floor((a + b) / 2)`, the
///   lower root's side) up to the halfway point to the next root. The first root's range
///   starts at 0 and the last one's ends at 127. Files with the same root get the same range
///   and **layer** (both sound). Distinct roots therefore never overlap and cover 0..=127.
/// * **Otherwise** (any file without a root), every file is mapped to one key, in plain byte
///   order of the file name (ties keep the input order), upwards from `at_key` (default 60,
///   C4). Past key 127 the extra files stack on 127. The root key is the key and so is the
///   range. Note names found in the other files are ignored.
/// * **One file without a root**: a single zone over the whole keyboard with root `at_key`
///   (default 60, C4), so a single dropped sample plays on every key. (A single file with a
///   note name keeps the note name as its root, and covers the whole keyboard through the
///   first rule.)
///
/// No files give no zones.
pub fn auto_map(files: &[(String, String, u64)], at_key: Option<u8>) -> Vec<ZoneDoc> {
    let roots: Vec<Option<u8>> = files
        .iter()
        .map(|(_, name, _)| parse_note_name(stem(name)))
        .collect();
    let zone = |i: usize, root: u8, lo: u8, hi: u8| {
        let (asset, _, frames) = &files[i];
        let mut z = ZoneDoc::new(asset.clone(), *frames);
        (z.root_key, z.key_lo, z.key_hi) = (root, lo, hi);
        z
    };
    if files.is_empty() {
        return Vec::new();
    }
    if roots.iter().all(Option::is_some) {
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by(|&a, &b| (roots[a], &files[a].1).cmp(&(roots[b], &files[b].1)));
        let mut distinct: Vec<u8> = order.iter().filter_map(|&i| roots[i]).collect();
        distinct.dedup();
        let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
        return order
            .into_iter()
            .map(|i| {
                let root = roots[i].expect("every file has a root");
                let at = distinct.binary_search(&root).expect("root is listed");
                let lo = if at == 0 {
                    0
                } else {
                    mid(distinct[at - 1], root) + 1
                };
                let hi = distinct.get(at + 1).map_or(127, |&next| mid(root, next));
                zone(i, root, lo, hi)
            })
            .collect();
    }
    if let [_] = files {
        return vec![zone(0, at_key.unwrap_or(60).min(127), 0, 127)];
    }
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&a, &b| files[a].1.as_bytes().cmp(files[b].1.as_bytes()));
    let first = u32::from(at_key.unwrap_or(60).min(127));
    order
        .into_iter()
        .enumerate()
        .map(|(n, i)| {
            let key = (first + n as u32).min(127) as u8;
            zone(i, key, key, key)
        })
        .collect()
}

/// The key of pad 1: pad `n` plays on key `PAD_KEY_BASE + n - 1` (GM drum keys 36–51).
/// Equal to `atomik_devices::drum_machine::PAD_KEY_BASE` (a test in `atomik-render` checks).
pub const PAD_KEY_BASE: u8 = 36;
/// Number of pads of a drum machine. Equal to `atomik_devices::drum_machine::PAD_COUNT`.
pub const PAD_COUNT: u8 = 16;

/// The key of pad `pad` (1..=16): `PAD_KEY_BASE + pad - 1`, so pad 1 is key 36 and pad 16 is key
/// 51. A pad outside 1..=16 is clamped into it.
pub fn pad_key(pad: u8) -> u8 {
    PAD_KEY_BASE + pad.clamp(1, PAD_COUNT) - 1
}

/// The pad (1..=16) that plays on `key`, or `None` for a key outside 36..=51.
pub fn pad_of_key(key: u8) -> Option<u8> {
    key.checked_sub(PAD_KEY_BASE)
        .filter(|i| *i < PAD_COUNT)
        .map(|i| i + 1)
}

/// The indices of the zones that belong to pad `pad`, in list order: those whose key range is
/// exactly the pad's key (`key_lo == key_hi == pad_key(pad)`). Zones on other keys, or
/// spanning several keys, belong to no pad.
pub fn pad_zones(zones: &[ZoneDoc], pad: u8) -> impl Iterator<Item = usize> + '_ {
    let key = pad_key(pad);
    zones
        .iter()
        .enumerate()
        .filter(move |(_, z)| z.key_lo == key && z.key_hi == key)
        .map(|(i, _)| i)
}

/// A zone for one file on a pad, over velocities `vel_lo..=vel_hi`: key, key range and root
/// are the pad key, the whole asset plays.
fn pad_zone(file: &(String, String, u64), pad: u8, vel_lo: u8, vel_hi: u8) -> ZoneDoc {
    let key = pad_key(pad);
    let mut z = ZoneDoc::new(file.0.clone(), file.2);
    (z.key_lo, z.key_hi, z.root_key) = (key, key, key);
    (z.vel_lo, z.vel_hi) = (vel_lo, vel_hi);
    z
}

/// `files` (`(asset id, original file name, frames)`) in plain byte order of the file name
/// (equal names keep their input order).
fn by_name(files: &[(String, String, u64)]) -> Vec<&(String, String, u64)> {
    let mut sorted: Vec<&(String, String, u64)> = files.iter().collect();
    sorted.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
    sorted
}

/// `zones` with pad `pad`'s zones ([`pad_zones`]) replaced by `files` as velocity layers.
/// The new zones take the place of the pad's first old zone (at the end when the
/// pad had none); every other zone is untouched and keeps its order.
///
/// * No files: the pad's zones are removed.
/// * One file: one zone over velocities 1..=127.
/// * Several: the files are ordered by file name in byte order (equal names keep their input
///   order) and share velocities 1..=127 as contiguous ranges, lowest velocity first. Each
///   range holds `127 / n` velocities and the first `127 % n` ranges one more, so the ranges
///   cover 1..=127 exactly, without overlap. At most 127 files are used (one velocity each).
///
/// Every zone has key range and root key equal to the pad key, plays the whole asset, loop
/// off ([`ZoneDoc::new`]).
pub fn with_pad_files(zones: &[ZoneDoc], pad: u8, files: &[(String, String, u64)]) -> Vec<ZoneDoc> {
    let files = by_name(files);
    let n = files.len().min(127);
    let (each, extra) = (127 / n.max(1), 127 % n.max(1));
    let mut layers = Vec::with_capacity(n);
    let mut lo = 1usize;
    for (i, f) in files.iter().take(n).enumerate() {
        let len = each + usize::from(i < extra);
        layers.push(pad_zone(f, pad, lo as u8, (lo + len - 1) as u8));
        lo += len;
    }
    let owned: Vec<usize> = pad_zones(zones, pad).collect();
    let at = owned.first().copied().unwrap_or(zones.len());
    let mut out = Vec::with_capacity(zones.len() + layers.len());
    for (i, z) in zones.iter().enumerate() {
        if i == at {
            out.append(&mut layers);
        }
        if !owned.contains(&i) {
            out.push(z.clone());
        }
    }
    out.append(&mut layers); // the pad had no zones: at the end
    out
}

/// Fills the pads that have no zones with `files`, one file per pad: the files in
/// byte order of the file name, the empty pads lowest first, each file a zone over velocities
/// 1..=127. Pads that have zones, and every existing zone, are untouched; the new zones are
/// appended in pad order. Returns the new list and the number of files dropped because no
/// empty pad was left.
pub fn fill_empty_pads(
    zones: &[ZoneDoc],
    files: &[(String, String, u64)],
) -> (Vec<ZoneDoc>, usize) {
    let empty: Vec<u8> = (1..=PAD_COUNT)
        .filter(|&p| pad_zones(zones, p).next().is_none())
        .collect();
    let files = by_name(files);
    let mut out = zones.to_vec();
    out.extend(
        files
            .iter()
            .zip(&empty)
            .map(|(f, &pad)| pad_zone(f, pad, 1, 127)),
    );
    (out, files.len().saturating_sub(empty.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_in_stems() {
        let cases: &[(&str, Option<u8>)] = &[
            ("C4", Some(60)),
            ("c4", Some(60)),
            ("Piano_C#3", Some(49)),
            ("Piano_Db3", Some(49)),
            ("Kick C1", Some(24)),
            ("str-A-1", Some(9)),
            ("A-1", Some(9)),
            ("C-1", Some(0)),
            ("G9", Some(127)),
            ("G#9", None), // out of range as a note: no root, the 9 is not read as a number
            ("Cb-1", None),
            ("vln_Db4_ff", Some(61)),
            ("vln_Db4_ff_G5", Some(79)),
            ("Bb3", Some(58)),
            ("bb3", Some(58)),
            ("B#3", Some(60)),
            ("Bass", None),
            ("Deep", None),
            ("Be4t", None),
            ("Fender_Rhodes", None),
            ("Acoustic Bass", None),
            ("E2x", None),
            ("xC4", None),
            ("C4x", None),
            ("C", None),
            ("A-3", None),
            ("Piano.C3", Some(48)),
            ("Strings 440 A4", Some(69)),
            ("kick_060_x", Some(60)),
            ("", None),
        ];
        for &(stem, want) in cases {
            assert_eq!(parse_note_name(stem), want, "{stem:?}");
        }
    }

    #[test]
    fn midi_numbers_count_only_without_a_note_name() {
        let cases: &[(&str, Option<u8>)] = &[
            ("_060_", Some(60)),
            ("sample 7", Some(7)),
            ("a_0_b", Some(0)),
            ("x_127", Some(127)),
            ("x_128", None),
            ("x_1000", None),
            ("take12b", None),
            ("x_12_y_99", Some(99)),
            ("060_C2", Some(36)),
            ("kick-1", Some(1)),
        ];
        for &(stem, want) in cases {
            assert_eq!(parse_note_name(stem), want, "{stem:?}");
        }
    }

    fn files(items: &[(&str, u64)]) -> Vec<(String, String, u64)> {
        items
            .iter()
            .enumerate()
            .map(|(i, (name, frames))| (format!("{i:064x}"), (*name).to_owned(), *frames))
            .collect()
    }

    fn ranges(zones: &[ZoneDoc]) -> Vec<(u8, u8, u8)> {
        zones
            .iter()
            .map(|z| (z.root_key, z.key_lo, z.key_hi))
            .collect()
    }

    #[test]
    fn roots_split_the_keyboard_halfway() {
        let f = files(&[("p_C4.wav", 10), ("p_C3.wav", 10), ("p_G4.wav", 10)]);
        let z = auto_map(&f, Some(10));
        assert_eq!(ranges(&z), [(48, 0, 54), (60, 55, 63), (67, 64, 127)]);
        assert_eq!(z[0].asset, f[1].0);
        assert_eq!(
            (z[0].start, z[0].end, z[0].vel_lo, z[0].vel_hi),
            (0, 10, 1, 127)
        );
        // Adjacent roots: the lower one keeps the lower half.
        let z = auto_map(&files(&[("a_C4", 5), ("a_C#4", 5)]), None);
        assert_eq!(ranges(&z), [(60, 0, 60), (61, 61, 127)]);
    }

    #[test]
    fn duplicate_roots_layer_and_tie_break_by_name() {
        let z = auto_map(&files(&[("b_C4", 5), ("a_C4", 5), ("x_E4", 5)]), None);
        assert_eq!(ranges(&z), [(60, 0, 62), (60, 0, 62), (64, 63, 127)]);
        assert_eq!(z[0].asset, format!("{:064x}", 1));
    }

    #[test]
    fn one_file_with_a_note_name_covers_the_keyboard() {
        let z = auto_map(&files(&[("Lead_E2.flac", 9)]), Some(72));
        assert_eq!(ranges(&z), [(40, 0, 127)]);
    }

    #[test]
    fn one_file_without_a_name_goes_to_the_drop_key_or_c4() {
        let z = auto_map(&files(&[("noise.wav", 9)]), Some(72));
        assert_eq!(ranges(&z), [(72, 0, 127)]);
        let z = auto_map(&files(&[("noise.wav", 9)]), None);
        assert_eq!(ranges(&z), [(60, 0, 127)]);
    }

    #[test]
    fn files_without_roots_map_one_key_each_in_name_order() {
        let z = auto_map(&files(&[("c", 1), ("a_C2", 2), ("b", 3)]), None);
        assert_eq!(ranges(&z), [(60, 60, 60), (61, 61, 61), (62, 62, 62)]);
        let names: Vec<u64> = z.iter().map(|z| z.end).collect();
        assert_eq!(names, [2, 3, 1]);
        let z = auto_map(&files(&[("b", 1), ("a", 1), ("c", 1)]), Some(126));
        assert_eq!(
            ranges(&z),
            [(126, 126, 126), (127, 127, 127), (127, 127, 127)]
        );
        assert!(auto_map(&[], Some(1)).is_empty());
    }

    #[test]
    fn extension_and_directory_are_not_part_of_the_stem() {
        assert_eq!(stem("dir/sub\\Piano C3.wav"), "Piano C3");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(stem("a.b.c"), "a.b");
        let z = auto_map(&files(&[("Bass.A4", 3)]), None);
        assert_eq!(ranges(&z), [(60, 0, 127)]); // "Bass" has no note; ".A4" is the extension
    }

    // ---- pad helpers ----

    fn on_pad(asset: u64, pad: u8) -> ZoneDoc {
        let mut z = ZoneDoc::new(format!("{asset:064x}"), 10);
        (z.key_lo, z.key_hi, z.root_key) = (pad_key(pad), pad_key(pad), pad_key(pad));
        z
    }

    fn vels(zones: &[ZoneDoc]) -> Vec<(u64, u8, u8)> {
        zones
            .iter()
            .map(|z| (z.end, z.vel_lo, z.vel_hi))
            .collect::<Vec<_>>()
    }

    #[test]
    fn pad_keys_are_gm_36_to_51() {
        assert_eq!((PAD_KEY_BASE, PAD_COUNT), (36, 16));
        assert_eq!(pad_key(1), 36);
        assert_eq!(pad_key(16), 51);
        assert_eq!(pad_key(0), 36, "clamped");
        assert_eq!(pad_key(99), 51, "clamped");
        for pad in 1..=16 {
            assert_eq!(pad_of_key(pad_key(pad)), Some(pad));
        }
        for key in (0..36).chain(52..=127) {
            assert_eq!(pad_of_key(key), None, "{key}");
        }
    }

    #[test]
    fn pad_zones_match_only_the_exact_pad_key() {
        let mut wide = on_pad(1, 2);
        wide.key_hi += 1; // spans pads 2 and 3: belongs to neither
        let zones = vec![
            on_pad(2, 1),
            wide,
            on_pad(3, 2),
            ZoneDoc::new("x", 1),
            on_pad(4, 1),
        ];
        assert_eq!(pad_zones(&zones, 1).collect::<Vec<_>>(), [0, 4]);
        assert_eq!(pad_zones(&zones, 2).collect::<Vec<_>>(), [2]);
        assert_eq!(pad_zones(&zones, 3).count(), 0);
    }

    #[test]
    fn one_file_fills_the_pad_at_full_velocity() {
        let f = files(&[("kick.wav", 7)]);
        let z = with_pad_files(&[], 3, &f);
        assert_eq!(z.len(), 1);
        let z = &z[0];
        assert_eq!(z.asset, f[0].0);
        assert_eq!(
            (z.key_lo, z.key_hi, z.root_key, z.vel_lo, z.vel_hi),
            (38, 38, 38, 1, 127)
        );
        assert_eq!((z.start, z.end), (0, 7));
    }

    #[test]
    fn several_files_split_velocity_in_name_order() {
        let f = files(&[("c.wav", 3), ("a.wav", 1), ("b.wav", 2)]);
        let z = with_pad_files(&[], 1, &f);
        // 127 = 3 * 42 + 1: the lowest layer gets the extra velocity.
        assert_eq!(vels(&z), [(1, 1, 43), (2, 44, 85), (3, 86, 127)]);
        let z = with_pad_files(&[], 1, &files(&[("b", 2), ("a", 1)]));
        assert_eq!(vels(&z), [(1, 1, 64), (2, 65, 127)]);
        // Byte order: uppercase before lowercase; equal names keep input order.
        let z = with_pad_files(&[], 1, &files(&[("a", 1), ("B", 2), ("a", 3)]));
        assert_eq!(z.iter().map(|z| z.end).collect::<Vec<_>>(), [2, 1, 3]);
    }

    #[test]
    fn replacing_a_pad_keeps_other_zones_and_their_order() {
        let other = ZoneDoc::new("o", 5); // whole keyboard: not a pad zone
        let zones = vec![on_pad(1, 1), other.clone(), on_pad(2, 2), on_pad(3, 1)];
        let z = with_pad_files(&zones, 1, &files(&[("n", 9)]));
        assert_eq!(z.len(), 3);
        assert_eq!(z[0].end, 9, "the new zone takes the first old zone's place");
        assert_eq!(z[1], other);
        assert_eq!(z[2], zones[2]);
        // A pad without zones appends; no files clears the pad.
        let z = with_pad_files(&zones, 5, &files(&[("n", 9)]));
        assert_eq!(&z[..4], &zones[..]);
        assert_eq!(z[4].key_lo, pad_key(5));
        let z = with_pad_files(&zones, 1, &[]);
        assert_eq!(z, [other, zones[2].clone()]);
    }

    #[test]
    fn at_most_127_files_layer_on_a_pad() {
        let f: Vec<_> = (0..200)
            .map(|i| (format!("{i:064x}"), format!("f{i:03}"), 5))
            .collect();
        let z = with_pad_files(&[], 1, &f);
        assert_eq!(z.len(), 127);
        assert!(
            z.iter()
                .enumerate()
                .all(|(i, z)| z.vel_lo == z.vel_hi && usize::from(z.vel_lo) == i + 1)
        );
    }

    #[test]
    fn background_files_fill_empty_pads_lowest_first() {
        let zones = vec![on_pad(1, 1), on_pad(2, 3), ZoneDoc::new("o", 5)];
        let (z, dropped) = fill_empty_pads(&zones, &files(&[("b", 2), ("a", 1), ("c", 3)]));
        assert_eq!(dropped, 0);
        assert_eq!(&z[..3], &zones[..]);
        let keys: Vec<(u8, u64)> = z[3..].iter().map(|z| (z.key_lo, z.end)).collect();
        assert_eq!(keys, [(pad_key(2), 1), (pad_key(4), 2), (pad_key(5), 3)]);
        assert!(z[3..].iter().all(|z| (z.vel_lo, z.vel_hi) == (1, 127)));
    }

    #[test]
    fn files_beyond_the_empty_pads_are_counted_and_dropped() {
        let zones: Vec<ZoneDoc> = (1..=14).map(|p| on_pad(u64::from(p), p)).collect();
        let (z, dropped) =
            fill_empty_pads(&zones, &files(&[("a", 1), ("b", 2), ("c", 3), ("d", 4)]));
        assert_eq!(dropped, 2);
        assert_eq!(z.len(), 16);
        assert_eq!((z[14].key_lo, z[14].end), (pad_key(15), 1));
        assert_eq!((z[15].key_lo, z[15].end), (pad_key(16), 2));
        let (z, dropped) = fill_empty_pads(&zones, &[]);
        assert_eq!((z, dropped), (zones, 0));
    }

    proptest::proptest! {
        #[test]
        fn velocity_layers_cover_1_to_127_without_overlap(n in 1usize..=127) {
            let f: Vec<_> = (0..n)
                .map(|i| (format!("{i:064x}"), format!("f{i:03}"), 5u64))
                .collect();
            let z = with_pad_files(&[], 7, &f);
            proptest::prop_assert_eq!(z.len(), n);
            proptest::prop_assert_eq!(z[0].vel_lo, 1);
            proptest::prop_assert_eq!(z[n - 1].vel_hi, 127);
            for w in z.windows(2) {
                proptest::prop_assert_eq!(u16::from(w[0].vel_hi) + 1, u16::from(w[1].vel_lo));
            }
            for zone in &z {
                proptest::prop_assert!(zone.vel_lo <= zone.vel_hi);
                let wide = i32::from(zone.vel_hi) - i32::from(zone.vel_lo) + 1;
                proptest::prop_assert!((127 / n as i32..=127 / n as i32 + 1).contains(&wide));
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn distinct_roots_tile_the_keyboard(
            roots in proptest::collection::btree_set(0u8..=127, 1..20),
            shuffle in 0usize..1000,
        ) {
            const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            let mut roots: Vec<u8> = roots.into_iter().collect();
            let n = roots.len();
            roots.rotate_left(shuffle % n);
            let f: Vec<(String, String, u64)> = roots
                .iter()
                .enumerate()
                .map(|(i, &r)| {
                    let name = format!("s_{}{}.wav", NAMES[usize::from(r % 12)], i32::from(r / 12) - 1);
                    (format!("{i:064x}"), name, 100)
                })
                .collect();
            let z = auto_map(&f, None);
            proptest::prop_assert_eq!(z.len(), n);
            let mut owners = [0u32; 128];
            for zone in &z {
                proptest::prop_assert!(zone.key_lo <= zone.root_key && zone.root_key <= zone.key_hi);
                for k in zone.key_lo..=zone.key_hi {
                    owners[usize::from(k)] += 1;
                }
            }
            proptest::prop_assert!(owners.iter().all(|&o| o == 1));
            for w in z.windows(2) {
                proptest::prop_assert_eq!(w[0].key_hi + 1, w[1].key_lo);
            }
        }

        #[test]
        fn zones_for_agrees_with_zone_contains(
            specs in proptest::collection::vec((0u8..=127, 0u8..=127, 1u8..=127, 1u8..=127), 0..8),
            key in 0u8..=127,
            vel in 1u8..=127,
        ) {
            let zones: Vec<ZoneDoc> = specs
                .iter()
                .map(|&(a, b, c, d)| {
                    let mut z = ZoneDoc::new("x", 10);
                    (z.key_lo, z.key_hi) = (a.min(b), a.max(b));
                    (z.vel_lo, z.vel_hi) = (c.min(d), c.max(d));
                    z
                })
                .collect();
            let want: Vec<usize> = zones
                .iter()
                .enumerate()
                .filter(|(i, z)| z.to_zone(*i as u32).contains(key, vel))
                .map(|(i, _)| i)
                .collect();
            let got: Vec<usize> = zones_for(key, vel, &zones).collect();
            proptest::prop_assert_eq!(got, want);
        }
    }
}
