//! Comparing two songs: a readable summary of what differs.
//!
//! [`compare_docs`] diffs a `before` document against an `after` one (an older version and the
//! song now, or two versions) and returns a [`DocDiff`]: sections of short lines a text panel
//! shows as they are. It is pure model code: it reads the documents and changes nothing, and it
//! compares what the song holds, never audio.
//!
//! The sections, each present only when something differs:
//!
//! * **Song**: tempo and meter (a single value, or the number of changes when there is a map),
//!   the song length, the sample rate, the master tune, the song information, the number of
//!   markers and sections.
//! * **Tracks**: tracks added, removed and renamed, and for a track in both: its clip and note
//!   counts, its automation lanes, mute, solo and freeze, and the effects of its insert chain
//!   that came or went.
//! * **Devices**: devices added and removed (a device of an insert chain says which track it is
//!   on), devices renamed, and how many parameters changed on a device in both. Console strips
//!   are listed under the mixer.
//! * **Mixer**: strips added, removed and renamed; fader, pan, mute and solo of a strip in both
//!   (and how many other settings changed); buses, groups, snapshots and FX returns.
//!
//! A line starts with `+` (added), `-` (removed) or `~` (changed); a line that belongs to the one
//! above it is indented by two spaces.

use crate::console::is_console_type;
use crate::{DeviceKey, RackDevice, RackDoc, Song, Track, insert_chain};
use atomik_device_sdk::Registry;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// A parameter counts as changed when its values differ by more than this.
const PARAM_EPSILON: f32 = 1.0e-6;

/// One section of a [`DocDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSection {
    /// The section's name ("Song", "Tracks", "Devices" or "Mixer").
    pub title: &'static str,
    /// Its lines (see the module documentation).
    pub items: Vec<String>,
}

/// What differs between two documents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocDiff {
    /// The sections that have lines, in the order of the module documentation.
    pub sections: Vec<DiffSection>,
}

impl DocDiff {
    /// True when the two documents do not differ in anything compared.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The summary as text lines: each section's title in capitals, then its items indented by
    /// two spaces. An empty diff reads as one line.
    pub fn lines(&self) -> Vec<String> {
        if self.sections.is_empty() {
            return vec!["No differences: the songs hold the same".to_owned()];
        }
        let mut out = Vec::new();
        for s in &self.sections {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push(s.title.to_uppercase());
            out.extend(s.items.iter().map(|i| format!("  {i}")));
        }
        out
    }
}

/// Compares `before` with `after` (see the module documentation). The registry gives the
/// default value of a parameter that a document does not store, so a stored default and a missing
/// value read as the same, and tells the insert effects of a strip apart.
pub fn compare_docs(before: &RackDoc, after: &RackDoc, registry: &Registry) -> DocDiff {
    let cx = Cx {
        a: before,
        b: after,
        registry,
    };
    let mut sections = Vec::new();
    let mut push = |title: &'static str, items: Vec<String>| {
        if !items.is_empty() {
            sections.push(DiffSection { title, items });
        }
    };
    push("Song", cx.song());
    push("Tracks", cx.tracks());
    push("Devices", cx.devices());
    push("Mixer", cx.mixer());
    DocDiff { sections }
}

struct Cx<'a> {
    a: &'a RackDoc,
    b: &'a RackDoc,
    registry: &'a Registry,
}

/// `120`, `127.5`, `99.25`: a tempo without trailing zeros.
fn bpm_text(bpm: f64) -> String {
    let text = format!("{bpm:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A value kept with its name when it is not empty.
fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

/// A pan value as `centre`, `L30` or `R30` (percent of the travel).
fn pan_text(pan: f32) -> String {
    let pct = (pan.abs() * 100.0).round() as i32;
    match pan {
        _ if pct == 0 => "centre".to_owned(),
        p if p < 0.0 => format!("L{pct}"),
        _ => format!("R{pct}"),
    }
}

fn percent_text(position: f32) -> String {
    format!("{}%", (position * 100.0).round() as i32)
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// `M:SS` of `seconds`.
fn clock_text(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// The length of a song as `32 bars (1:04)`.
fn length_text(song: &Song) -> String {
    let end = song.end_tick().max(1);
    let meter = song.meter_table();
    let bars = meter.map().bar_at(end - 1);
    let seconds = song.tempo_table().map().seconds(end as f64);
    format!(
        "{} ({})",
        plural(bars as usize, "bar", "bars"),
        clock_text(seconds)
    )
}

impl Cx<'_> {
    // ------------------------------------------------------------------ the song

    fn song(&self) -> Vec<String> {
        let (a, b) = (&*self.a.song, &*self.b.song);
        let mut out = Vec::new();
        if a.tempo != b.tempo {
            out.push(format!("~ Tempo: {} -> {}", tempo_text(a), tempo_text(b)));
        }
        if a.meter != b.meter {
            out.push(format!("~ Meter: {} -> {}", meter_text(a), meter_text(b)));
        }
        if a.end_tick() != b.end_tick() {
            out.push(format!(
                "~ Length: {} -> {}",
                length_text(a),
                length_text(b)
            ));
        }
        if self.a.sample_rate != self.b.sample_rate {
            out.push(format!(
                "~ Sample rate: {} Hz -> {} Hz",
                self.a.sample_rate, self.b.sample_rate
            ));
        }
        if self.a.tune_cents.to_bits() != self.b.tune_cents.to_bits() {
            out.push(format!(
                "~ Master tune: {:+} cents -> {:+} cents",
                self.a.tune_cents, self.b.tune_cents
            ));
        }
        let (ia, ib) = (&self.a.info, &self.b.info);
        for (what, x, y) in [
            ("title", &ia.title, &ib.title),
            ("author", &ia.author, &ib.author),
        ] {
            if x != y {
                out.push(format!("~ Song {what}: {} -> {}", quoted(x), quoted(y)));
            }
        }
        if ia.notes != ib.notes {
            out.push("~ Song notes changed".to_owned());
        }
        if (&ia.url, &ia.email) != (&ib.url, &ib.email) {
            out.push("~ Song contact details changed".to_owned());
        }
        if a.markers.len() != b.markers.len() {
            out.push(format!(
                "~ Markers: {} -> {}",
                a.markers.len(),
                b.markers.len()
            ));
        }
        if a.ranges.len() != b.ranges.len() {
            out.push(format!(
                "~ Sections: {} -> {}",
                a.ranges.len(),
                b.ranges.len()
            ));
        }
        out
    }

    // ------------------------------------------------------------------ tracks

    fn name_in(doc: &RackDoc, key: &DeviceKey) -> String {
        doc.device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    }

    fn tracks(&self) -> Vec<String> {
        let (a, b) = (&*self.a.song, &*self.b.song);
        let mut out = Vec::new();
        for t in &a.tracks {
            if b.track(&t.device).is_none() {
                out.push(format!(
                    "- Removed {} ({})",
                    Self::name_in(self.a, &t.device),
                    track_size(t)
                ));
            }
        }
        for t in &b.tracks {
            if a.track(&t.device).is_none() {
                out.push(format!(
                    "+ Added {} ({})",
                    Self::name_in(self.b, &t.device),
                    track_size(t)
                ));
            }
        }
        for tb in &b.tracks {
            let Some(ta) = a.track(&tb.device) else {
                continue;
            };
            let (na, nb) = (
                Self::name_in(self.a, &tb.device),
                Self::name_in(self.b, &tb.device),
            );
            let mut lines = self.track_changes(ta, tb);
            if na != nb {
                out.push(format!("~ Renamed {na} -> {nb}"));
            }
            if !lines.is_empty() {
                out.push(format!("~ {nb}"));
                out.extend(lines.drain(..).map(|l| format!("  {l}")));
            }
        }
        if a.tracks.len() != b.tracks.len() {
            out.push(format!(
                "~ Tracks: {} -> {}",
                a.tracks.len(),
                b.tracks.len()
            ));
        }
        out
    }

    fn track_changes(&self, ta: &Track, tb: &Track) -> Vec<String> {
        let mut out = Vec::new();
        let (sa, sb) = (TrackSize::of(ta), TrackSize::of(tb));
        if sa.clips != sb.clips {
            out.push(format!("Clips: {} -> {}", sa.clips, sb.clips));
        }
        if sa.notes != sb.notes {
            out.push(format!("Notes: {} -> {}", sa.notes, sb.notes));
        }
        if sa.automation != sb.automation {
            out.push(format!(
                "Automation lanes: {} -> {}",
                sa.automation, sb.automation
            ));
        }
        for (what, x, y) in [
            ("Mute", ta.muted, tb.muted),
            ("Solo", ta.solo, tb.solo),
            ("Frozen", ta.frozen.is_some(), tb.frozen.is_some()),
        ] {
            if x != y {
                out.push(format!("{what}: {} -> {}", on_off(x), on_off(y)));
            }
        }
        let before = self.chain_names(self.a, &ta.device);
        let after = self.chain_names(self.b, &tb.device);
        for (key, name) in &after {
            if !before.iter().any(|(k, _)| k == key) {
                out.push(format!("+ Effect {name}"));
            }
        }
        for (key, name) in &before {
            if !after.iter().any(|(k, _)| k == key) {
                out.push(format!("- Effect {name}"));
            }
        }
        out
    }

    /// The insert effects of the strip that plays `track`'s device, in order.
    fn chain_names(&self, doc: &RackDoc, track: &DeviceKey) -> Vec<(DeviceKey, String)> {
        let Some(channel) = doc
            .console
            .channels
            .iter()
            .find(|c| c.source.as_ref() == Some(track))
        else {
            return Vec::new();
        };
        insert_chain(doc, self.registry, &channel.channel)
            .into_iter()
            .map(|k| {
                let name = Self::name_in(doc, &k);
                (k, name)
            })
            .collect()
    }

    // ------------------------------------------------------------------ devices

    /// Whether `device` is a track's own device or a console strip: those are reported under
    /// the tracks and the mixer.
    fn reported_elsewhere(doc: &RackDoc, device: &RackDevice) -> bool {
        is_console_type(&device.type_id) || doc.song.track(&device.key).is_some()
    }

    /// The track whose insert chain holds `device`, as ` on <track>`, or an empty text.
    fn on_track(&self, doc: &RackDoc, device: &DeviceKey) -> String {
        for channel in &doc.console.channels {
            let Some(source) = &channel.source else {
                continue;
            };
            if insert_chain(doc, self.registry, &channel.channel).contains(device) {
                return format!(" on {}", Self::name_in(doc, source));
            }
        }
        String::new()
    }

    fn devices(&self) -> Vec<String> {
        let mut out = Vec::new();
        for d in &self.a.devices {
            if self.b.device(&d.key).is_none() && !Self::reported_elsewhere(self.a, d) {
                out.push(format!(
                    "- Removed {}{}",
                    d.name,
                    self.on_track(self.a, &d.key)
                ));
            }
        }
        for d in &self.b.devices {
            if self.a.device(&d.key).is_none() && !Self::reported_elsewhere(self.b, d) {
                out.push(format!(
                    "+ Added {}{}",
                    d.name,
                    self.on_track(self.b, &d.key)
                ));
            }
        }
        for db in &self.b.devices {
            let Some(da) = self.a.device(&db.key) else {
                continue;
            };
            if is_console_type(&db.type_id) {
                continue;
            }
            let own = self.a.song.track(&db.key).is_none();
            if own && da.name != db.name {
                out.push(format!("~ Renamed {} -> {}", da.name, db.name));
            }
            if da.type_id != db.type_id {
                out.push(format!(
                    "~ {} is now a different device ({})",
                    db.name, db.type_id
                ));
                continue;
            }
            let n = self.changed_params(da, db).len();
            if n > 0 {
                out.push(format!(
                    "~ {}: {} changed",
                    db.name,
                    plural(n, "parameter", "parameters")
                ));
            }
        }
        if self.a.cables.len() != self.b.cables.len() {
            out.push(format!(
                "~ Cables: {} -> {}",
                self.a.cables.len(),
                self.b.cables.len()
            ));
        }
        out
    }

    /// The keys of the params that differ between two devices of one type, by effective value.
    fn changed_params(&self, a: &RackDevice, b: &RackDevice) -> Vec<String> {
        let entry = self.registry.get(&b.type_id);
        let desc = entry.as_ref().map(|e| &e.descriptor);
        let keys: BTreeSet<&String> = a.params.keys().chain(b.params.keys()).collect();
        keys.into_iter()
            .filter(|k| {
                let default = desc.and_then(|d| d.param_by_key(k)).map(|p| p.default);
                let va = a.params.get(*k).copied().or(default);
                let vb = b.params.get(*k).copied().or(default);
                match (va, vb) {
                    (Some(x), Some(y)) => (x - y).abs() > PARAM_EPSILON,
                    (None, None) => false,
                    _ => true,
                }
            })
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------------ mixer

    fn param(&self, doc: &RackDoc, device: &DeviceKey, key: &str) -> Option<f32> {
        doc.param_value(device, key, self.registry)
    }

    fn mixer(&self) -> Vec<String> {
        let (a, b) = (&self.a.console, &self.b.console);
        let mut out = Vec::new();
        for c in &a.channels {
            if b.channel_index(&c.channel).is_none() {
                out.push(format!(
                    "- Removed strip {}",
                    Self::name_in(self.a, &c.channel)
                ));
            }
        }
        for c in &b.channels {
            if a.channel_index(&c.channel).is_none() {
                out.push(format!(
                    "+ Added strip {}",
                    Self::name_in(self.b, &c.channel)
                ));
            }
        }
        for c in &b.channels {
            if a.channel_index(&c.channel).is_some() {
                out.extend(self.strip_changes(&c.channel));
            }
        }
        for x in &a.buses {
            if b.bus_index(&x.bus).is_none() {
                out.push(format!("- Removed bus {}", x.name));
            }
        }
        for x in &b.buses {
            match a.buses.iter().find(|y| y.bus == x.bus) {
                None => out.push(format!("+ Added bus {}", x.name)),
                Some(y) => {
                    if y.name != x.name {
                        out.push(format!("~ Renamed bus {} -> {}", y.name, x.name));
                    }
                    out.extend(self.strip_changes(&x.bus));
                }
            }
        }
        if let (Some(ma), Some(mb)) = (&a.master, &b.master)
            && ma == mb
        {
            out.extend(self.strip_changes(mb));
        }
        for v in &a.vcas {
            if !b.vcas.iter().any(|w| w.id == v.id) {
                out.push(format!("- Removed group {}", v.name));
            }
        }
        for v in &b.vcas {
            match a.vcas.iter().find(|w| w.id == v.id) {
                None => out.push(format!("+ Added group {}", v.name)),
                Some(w) => {
                    if w.name != v.name {
                        out.push(format!("~ Renamed group {} -> {}", w.name, v.name));
                    }
                    if w.level_db.to_bits() != v.level_db.to_bits() {
                        out.push(format!(
                            "~ Group {}: level {:+.1} dB -> {:+.1} dB",
                            v.name, w.level_db, v.level_db
                        ));
                    }
                    for (what, x, y) in [("mute", w.mute, v.mute), ("solo", w.solo, v.solo)] {
                        if x != y {
                            out.push(format!(
                                "~ Group {}: {what} {} -> {}",
                                v.name,
                                on_off(x),
                                on_off(y)
                            ));
                        }
                    }
                }
            }
        }
        if a.snapshots.len() != b.snapshots.len() {
            out.push(format!(
                "~ Snapshots: {} -> {}",
                a.snapshots.len(),
                b.snapshots.len()
            ));
        }
        for (slot, (x, y)) in a.returns.iter().zip(&b.returns).enumerate() {
            if x != y {
                let name = |doc: &RackDoc, k: &Option<DeviceKey>| {
                    k.as_ref()
                        .map_or_else(|| "empty".to_owned(), |k| Self::name_in(doc, k))
                };
                out.push(format!(
                    "~ FX return {}: {} -> {}",
                    slot + 1,
                    name(self.a, x),
                    name(self.b, y)
                ));
            }
        }
        out
    }

    /// The changes of one strip (a channel, a bus or the master) present in both documents.
    fn strip_changes(&self, key: &DeviceKey) -> Vec<String> {
        let (Some(da), Some(db)) = (self.a.device(key), self.b.device(key)) else {
            return Vec::new();
        };
        if da.type_id != db.type_id {
            return Vec::new();
        }
        let mut parts: Vec<String> = Vec::new();
        let mut shown: Vec<&str> = Vec::new();
        let mut pair = |param: &'static str, text: &dyn Fn(f32) -> String, what: &str| {
            shown.push(param);
            let (x, y) = (
                self.param(self.a, key, param),
                self.param(self.b, key, param),
            );
            if let (Some(x), Some(y)) = (x, y)
                && (x - y).abs() > PARAM_EPSILON
            {
                parts.push(format!("{what} {} -> {}", text(x), text(y)));
            }
        };
        pair("fader", &percent_text, "fader");
        pair("pan", &pan_text, "pan");
        pair("mute", &|v| on_off(v >= 0.5).to_owned(), "mute");
        pair("solo", &|v| on_off(v >= 0.5).to_owned(), "solo");
        let others = self
            .changed_params(da, db)
            .iter()
            .filter(|k| !shown.contains(&k.as_str()))
            .count();
        if others > 0 {
            parts.push(format!(
                "{} changed",
                plural(others, "other setting", "other settings")
            ));
        }
        if parts.is_empty() {
            return Vec::new();
        }
        let mut line = String::new();
        let _ = write!(line, "~ {}: {}", db.name, parts.join(", "));
        vec![line]
    }
}

/// What a track holds, for the added and removed lines and for the counts.
struct TrackSize {
    clips: usize,
    notes: usize,
    automation: usize,
}

impl TrackSize {
    fn of(t: &Track) -> Self {
        use crate::{ClipContent, LaneKind};
        let mut s = Self {
            clips: 0,
            notes: 0,
            automation: 0,
        };
        for lane in &t.lanes {
            if matches!(lane.kind, LaneKind::Automation(_)) {
                s.automation += 1;
                continue;
            }
            for clip in &lane.clips {
                s.clips += 1;
                if let ClipContent::Notes(n) = &clip.content {
                    s.notes += n.len();
                }
            }
        }
        s
    }
}

fn track_size(t: &Track) -> String {
    let s = TrackSize::of(t);
    plural(s.clips, "clip", "clips")
}

/// A tempo map as text: `120 BPM`, or the first tempo and how many changes follow it.
fn tempo_text(song: &Song) -> String {
    let first = song.tempo.first().map_or(120.0, |e| e.bpm);
    match song.tempo.len() {
        0 | 1 => format!("{} BPM", bpm_text(first)),
        n => format!(
            "{} BPM, {}",
            bpm_text(first),
            plural(n - 1, "change", "changes")
        ),
    }
}

/// A meter map as text: `4/4`, or the first signature and how many changes follow it.
fn meter_text(song: &Song) -> String {
    let (num, den) = song.meter();
    match song.meter.len() {
        0 | 1 => format!("{num}/{den}"),
        n => format!("{num}/{den}, {}", plural(n - 1, "change", "changes")),
    }
}
