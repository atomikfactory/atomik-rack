//! A search over every parameter of the project.
//!
//! [`ParamIndex::build`] lists the params of every device (the rack's devices, a container's
//! inside by path key, the console strips, the plugin params the model knows) and the VCA group
//! levels, each with the **track** it belongs to, the device and the param, in rack order. The
//! index holds names only, never values (the caller reads the current value of the few rows it
//! shows), and carries a [`ParamIndex::signature`] of the structure it was built from, so a
//! caller keeps one index and rebuilds it only when [`ParamIndex::signature_of`] says the rack
//! changed (a param edit does not).
//!
//! # Where a param belongs
//!
//! The track label is the name of the instrument (or any device) that has the track or the
//! console channel: the instrument itself, its channel strip, and the effects on that strip's
//! insert chain all read as that track. A bus is its own name, the master is `Master`, an effect
//! on a return is `Returns`, a container's inside is the container's track, a VCA group is
//! `Mixer`, and what is left (a macro bank, a loose device) is `Rack`.
//!
//! # Matching and ranking
//!
//! [`ParamIndex::search`] splits the query on spaces and `>` (so `lead > filter > cut` works) and
//! ignores case. **Every** word must match one of the five fields (param name, param key, device,
//! track, unit), or the row is out. A word is scored by how it matches the best field: the start
//! of a field or of a word in it beats a substring, which beats a fuzzy match (the letters in
//! order, at most three characters apart, for words of three or more letters); the param name
//! counts most, then the device, the track, the key and the unit. Rows are ordered by score,
//! then shorter param names, then rack order, and cut at `limit` (never above
//! [`MAX_RESULTS`]). An empty query lists the first rows in rack order.

use crate::{DeviceKey, RackDoc, console, console_insert};
use atomik_core::Unit;
use atomik_device_sdk::Registry;

/// The most rows a search returns, whatever the caller asks for.
pub const MAX_RESULTS: usize = 500;

/// What a row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    /// A param of a device (a path key inside a container).
    Param {
        /// The device.
        device: DeviceKey,
        /// The param key.
        param: String,
    },
    /// The level of a VCA group.
    Vca {
        /// The group's id.
        id: u32,
    },
}

/// One row of the index.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamEntry {
    /// What the row is.
    pub target: SearchTarget,
    /// The track it belongs to (see the module documentation).
    pub track: String,
    /// The device's name.
    pub device: String,
    /// The param's name.
    pub param: String,
    /// The param's key (`filter.cutoff`); empty for a VCA level.
    pub key: String,
    /// The unit as short text (`Hz`, `dB`, `%`, `choice`...), empty when there is none.
    pub unit: String,
    /// The lower-case fields in match order: param, device, track, key, unit.
    hay: [String; 5],
}

/// The index of a project's params.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamIndex {
    entries: Vec<ParamEntry>,
    signature: u64,
}

/// The unit as short text for the search and its rows.
pub fn unit_label(unit: Unit) -> &'static str {
    match unit {
        Unit::None => "",
        Unit::Hz => "Hz",
        Unit::Ms => "ms",
        Unit::Db => "dB",
        Unit::Percent => "%",
        Unit::Semitones => "st",
        Unit::Cents => "ct",
        Unit::Octaves => "oct",
        Unit::Choice(_) => "choice",
        Unit::Toggle => "switch",
    }
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    *h ^= 0xff;
    *h = h.wrapping_mul(0x0000_0100_0000_01b3);
}

impl ParamIndex {
    /// The structure the index depends on, hashed: the devices (key, type, name, the plugin's
    /// layout size), the cables (which decide an insert chain), the console and its groups.
    /// Param values are not part of it.
    pub fn signature_of(doc: &RackDoc) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        fn device(h: &mut u64, prefix: &str, d: &crate::RackDevice) {
            fnv(h, prefix.as_bytes());
            fnv(h, d.key.0.as_bytes());
            fnv(h, d.type_id.as_bytes());
            fnv(h, d.name.as_bytes());
            if let Some(p) = &d.plugin {
                fnv(h, &(p.layout.params.len() as u64).to_le_bytes());
            }
            if let Some(inner) = &d.inner {
                let inside = format!("{prefix}{}/", d.key.0);
                for i in &inner.devices {
                    device(h, &inside, i);
                }
            }
        }
        for d in &doc.devices {
            device(&mut h, "", d);
        }
        for c in &doc.cables {
            fnv(&mut h, c.from.device.0.as_bytes());
            fnv(&mut h, c.from.socket.as_bytes());
            fnv(&mut h, c.to.device.0.as_bytes());
            fnv(&mut h, c.to.socket.as_bytes());
        }
        for c in &doc.console.channels {
            fnv(&mut h, c.channel.0.as_bytes());
            fnv(
                &mut h,
                c.source.as_ref().map_or("", |s| s.0.as_str()).as_bytes(),
            );
        }
        for b in &doc.console.buses {
            fnv(&mut h, b.bus.0.as_bytes());
            fnv(&mut h, b.name.as_bytes());
        }
        for v in &doc.console.vcas {
            fnv(&mut h, &u64::from(v.id).to_le_bytes());
            fnv(&mut h, v.name.as_bytes());
        }
        h
    }

    /// The index of `doc`: one row per param of every device, and per VCA group.
    pub fn build(doc: &RackDoc, registry: &Registry) -> Self {
        let mut entries = Vec::new();
        let track_of = TrackLabels::new(doc, registry);
        fn walk(
            out: &mut Vec<ParamEntry>,
            devices: &[crate::RackDevice],
            prefix: &str,
            registry: &Registry,
            track: &dyn Fn(&str, &crate::RackDevice) -> String,
        ) {
            for d in devices {
                let path = format!("{prefix}{}", d.key.0);
                if let Some(desc) = registry.get(&d.type_id).map(|f| f.descriptor) {
                    let t = track(&path, d);
                    for p in desc.params {
                        if console::is_internal_param(p.key) {
                            continue;
                        }
                        out.push(ParamEntry::new(
                            SearchTarget::Param {
                                device: DeviceKey(path.clone()),
                                param: p.key.to_owned(),
                            },
                            &t,
                            &d.name,
                            p.name,
                            p.key,
                            unit_label(p.unit),
                        ));
                    }
                }
                if let Some(inner) = &d.inner {
                    walk(out, &inner.devices, &format!("{path}/"), registry, track);
                }
            }
        }
        walk(&mut entries, &doc.devices, "", registry, &|path, d| {
            track_of.label(path, d)
        });
        for v in &doc.console.vcas {
            entries.push(ParamEntry::new(
                SearchTarget::Vca { id: v.id },
                "Mixer",
                &v.name,
                "Level",
                "",
                "dB",
            ));
        }
        Self {
            entries,
            signature: Self::signature_of(doc),
        }
    }

    /// The signature the index was built from (see [`ParamIndex::signature_of`]).
    pub fn signature(&self) -> u64 {
        self.signature
    }

    /// Every row, in rack order.
    pub fn entries(&self) -> &[ParamEntry] {
        &self.entries
    }

    /// How many rows there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the project has no param.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The rows that match `query`, best first, at most `limit` (and [`MAX_RESULTS`]).
    pub fn search(&self, query: &str, limit: usize) -> Vec<&ParamEntry> {
        let limit = limit.min(MAX_RESULTS);
        let words: Vec<String> = query
            .to_lowercase()
            .split(|c: char| c.is_whitespace() || c == '>')
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        if words.is_empty() {
            return self.entries.iter().take(limit).collect();
        }
        let mut scored: Vec<(u32, usize, &ParamEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.score(&words).map(|s| (s, i, e)))
            .collect();
        // Best score first, then the shorter param name, then rack order.
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.2.param.len().cmp(&b.2.param.len()))
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().take(limit).map(|(_, _, e)| e).collect()
    }
}

impl ParamEntry {
    fn new(
        target: SearchTarget,
        track: &str,
        device: &str,
        param: &str,
        key: &str,
        unit: &str,
    ) -> Self {
        let hay = [param, device, track, key, unit].map(str::to_lowercase);
        Self {
            target,
            track: track.to_owned(),
            device: device.to_owned(),
            param: param.to_owned(),
            key: key.to_owned(),
            unit: unit.to_owned(),
            hay,
        }
    }

    /// The score of the row for the words of a query, or `None` when a word matches nothing.
    fn score(&self, words: &[String]) -> Option<u32> {
        // Weights of the fields: param, device, track, key, unit.
        const WEIGHT: [u32; 5] = [10, 6, 5, 4, 3];
        let mut total = 0;
        for w in words {
            let best = self
                .hay
                .iter()
                .zip(WEIGHT)
                .filter_map(|(field, weight)| match_quality(field, w).map(|q| q * weight))
                .max()?;
            total += best;
        }
        Some(total)
    }
}

/// How well `word` matches `field` (both lower case): 8 at the start of the field, 6 at the
/// start of a word in it, 4 anywhere, 1 to 2 as a fuzzy match; `None` for no match.
fn match_quality(field: &str, word: &str) -> Option<u32> {
    if field.is_empty() {
        return None;
    }
    if field.starts_with(word) {
        return Some(8);
    }
    let mut best = None;
    for (at, _) in field.match_indices(word) {
        let starts_word = field[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let q = if starts_word { 6 } else { 4 };
        best = best.max(Some(q));
    }
    if best.is_some() {
        return best;
    }
    fuzzy(field, word)
}

/// The letters of `word` in order in `field`, at most three characters apart, for a word of at
/// least three letters: 2 when the first letter starts the field or a word, else 1.
fn fuzzy(field: &str, word: &str) -> Option<u32> {
    if word.chars().count() < 3 {
        return None;
    }
    let mut want = word.chars();
    let mut next = want.next()?;
    let mut last: Option<usize> = None;
    let mut starts_word = false;
    let mut previous: Option<char> = None;
    for (index, c) in field.chars().enumerate() {
        if c == next {
            if last.is_some_and(|l| index - l > 4) {
                return None;
            }
            if last.is_none() {
                starts_word = previous.is_none_or(|p| !p.is_alphanumeric());
            }
            last = Some(index);
            match want.next() {
                Some(n) => next = n,
                None => return Some(if starts_word { 2 } else { 1 }),
            }
        }
        previous = Some(c);
    }
    None
}

/// Which track label each device gets (see the module documentation).
struct TrackLabels<'a> {
    doc: &'a RackDoc,
    /// Insert effect to the track label of its strip.
    inserts: Vec<(DeviceKey, String)>,
}

impl<'a> TrackLabels<'a> {
    fn new(doc: &'a RackDoc, registry: &Registry) -> Self {
        let mut inserts = Vec::new();
        let strips = doc
            .console
            .channels
            .iter()
            .map(|c| c.channel.clone())
            .chain(doc.console.buses.iter().map(|b| b.bus.clone()))
            .chain(doc.console.master.iter().cloned());
        for strip in strips {
            let label = Self::strip_label(doc, &strip);
            for fx in console_insert::insert_chain(doc, registry, &strip) {
                inserts.push((fx, label.clone()));
            }
        }
        Self { doc, inserts }
    }

    fn name(doc: &RackDoc, key: &DeviceKey) -> String {
        doc.device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    }

    /// A strip reads as its source's track, a bus as its name, the master as `Master`.
    fn strip_label(doc: &RackDoc, strip: &DeviceKey) -> String {
        if let Some(c) = doc.console.channels.iter().find(|c| &c.channel == strip) {
            return c
                .source
                .as_ref()
                .map_or_else(|| Self::name(doc, strip), |s| Self::name(doc, s));
        }
        if let Some(b) = doc.console.buses.iter().find(|b| &b.bus == strip) {
            return b.name.clone();
        }
        "Master".to_owned()
    }

    fn label(&self, path: &str, d: &crate::RackDevice) -> String {
        let doc = self.doc;
        // A container's inside is its container's track.
        if let Some((top, _)) = path.split_once('/') {
            return Self::name(doc, &DeviceKey(top.to_owned()));
        }
        let key = &d.key;
        if doc.console.channels.iter().any(|c| &c.channel == key)
            || doc.console.buses.iter().any(|b| &b.bus == key)
            || doc.console.master.as_ref() == Some(key)
        {
            return Self::strip_label(doc, key);
        }
        if doc
            .console
            .channels
            .iter()
            .any(|c| c.source.as_ref() == Some(key))
        {
            return d.name.clone();
        }
        if let Some((_, label)) = self.inserts.iter().find(|(fx, _)| fx == key) {
            return label.clone();
        }
        if doc.console.returns.iter().any(|r| r.as_ref() == Some(key)) {
            return "Returns".to_owned();
        }
        if doc.song.track(key).is_some() {
            return d.name.clone();
        }
        "Rack".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_matches_best_at_the_start_of_a_field() {
        assert_eq!(match_quality("filter cutoff", "filter"), Some(8));
        assert_eq!(match_quality("filter cutoff", "cut"), Some(6));
        assert_eq!(match_quality("filter cutoff", "ter"), Some(4));
        assert_eq!(match_quality("filter cutoff", "fco"), None);
        assert_eq!(match_quality("", "a"), None);
    }

    #[test]
    fn a_fuzzy_match_needs_three_letters_close_together() {
        assert_eq!(fuzzy("filter cutoff", "fcu"), None);
        assert_eq!(fuzzy("filter resonance", "frs"), None);
        assert_eq!(fuzzy("filter cutoff", "fltr"), Some(2));
        assert_eq!(fuzzy("attack", "atk"), Some(2));
        assert_eq!(fuzzy("release", "rls"), Some(2));
        assert_eq!(fuzzy("release", "ls"), None);
        assert_eq!(fuzzy("a very long name", "anm"), None);
    }

    #[test]
    fn units_have_short_labels() {
        assert_eq!(unit_label(Unit::Hz), "Hz");
        assert_eq!(unit_label(Unit::Choice(&["a"])), "choice");
        assert_eq!(unit_label(Unit::None), "");
    }
}
