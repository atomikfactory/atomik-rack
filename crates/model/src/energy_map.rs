//! The energy map of a song: one 0-1 number per section that says how much is going on there, and
//! the places where the arrangement's shape works against itself.
//!
//! The measuring is done elsewhere (a render of the master on a worker, read as loudness, brightness
//! and width); this module holds the parts that are pure functions of the song and of those
//! numbers, so they can be tested without rendering:
//!
//! * [`sections`]: where the sections are. Range markers are sections as they stand; without
//!   ranges, each point marker starts a section that runs to the next marker (or the song End),
//!   with a leading "Start" section when the first marker is not at tick 0; without any marker, a
//!   section is [`DEFAULT_SECTION_BARS`] bars.
//! * [`Role::guess`]: what a section is for, from its name (intro, verse, build, drop, break,
//!   outro); a name that says nothing is [`Role::Other`].
//! * [`active_counts`]: how many tracks play in each section (notes that sound or audio clips that
//!   are audible, on tracks that are not muted away).
//! * [`energy`]: the four measures combined into one value with the documented [`WEIGHTS`].
//! * [`EnergyMap::new`]: the sections with their values and the [`Flag`]s.
//!
//! The number is for comparing sections of one song with each other, not an absolute loudness: the
//! scales ([`LOUD_FLOOR_DB`] and the other `*_FLOOR`/`*_CEIL` constants) put an ordinary mix in the
//! middle of the range. Everything is plain `f32` arithmetic plus one `log2`, so the values agree
//! across platforms to within 0.001.

use crate::song::{ClipContent, Song, Track, audible_spans, effective_tracks, flatten_track_notes};

/// The length of a section, in bars, when the song has no marker.
pub const DEFAULT_SECTION_BARS: u32 = 8;

/// The weights of the four measures in [`energy`]; they add up to 1.
pub struct Weights {
    /// Loudness of the master over the section.
    pub loudness: f32,
    /// The number of tracks that play, as a share of the tracks the song uses.
    pub tracks: f32,
    /// Brightness: the spectral centroid.
    pub brightness: f32,
    /// Stereo width.
    pub width: f32,
}

/// The documented weights.
pub const WEIGHTS: Weights = Weights {
    loudness: 0.40,
    tracks: 0.25,
    brightness: 0.20,
    width: 0.15,
};

/// The loudness (dB, as the master render reads it) that counts as 0 energy.
pub const LOUD_FLOOR_DB: f32 = -40.0;
/// The loudness that counts as full energy.
pub const LOUD_CEIL_DB: f32 = -10.0;
/// The brightness (Hz) that counts as 0 energy.
pub const BRIGHT_FLOOR_HZ: f32 = 250.0;
/// The brightness that counts as full energy (four octaves above the floor).
pub const BRIGHT_CEIL_HZ: f32 = 4000.0;
/// The width that counts as full energy (a mix wider than this is not wider for the listener).
pub const WIDTH_CEIL: f32 = 0.7;

/// How much more energy a build must have than the drop after it to be flagged, and how far below
/// the top of the map a drop must be to be flagged.
pub const FLAG_MARGIN: f32 = 0.03;
/// Two sections whose energies differ by less than this are "near-equal".
pub const NEAR_EQUAL: f32 = 0.04;
/// The least length, in bars, of two near-equal neighbours for [`Flag::FlatRun`].
pub const FLAT_BARS: u32 = 16;
/// Sections with less energy than this are silent for [`Flag::FlatRun`].
pub const SILENT_ENERGY: f32 = 0.05;

/// What a section is for, guessed from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The opening.
    Intro,
    /// A verse.
    Verse,
    /// A build or pre-chorus: tension towards the drop.
    Build,
    /// A drop or chorus: the climax.
    Drop,
    /// A break, bridge or breakdown: the dip.
    Break,
    /// The ending.
    Outro,
    /// A name that says nothing about the role.
    Other,
}

impl Role {
    /// The word for the role.
    pub fn label(self) -> &'static str {
        match self {
            Role::Intro => "intro",
            Role::Verse => "verse",
            Role::Build => "build",
            Role::Drop => "drop",
            Role::Break => "break",
            Role::Outro => "outro",
            Role::Other => "section",
        }
    }

    /// The role a section name suggests: the first word of the name that means a role decides
    /// (case and digits do not matter): `Pre-Chorus` is a build, `Drop 2` a drop, `Chorus` a drop.
    pub fn guess(name: &str) -> Self {
        let lower = name.to_lowercase();
        for word in lower.split(|c: char| !c.is_alphabetic()) {
            if word.is_empty() {
                continue;
            }
            if let Some(role) = word_role(word) {
                return role;
            }
        }
        Role::Other
    }
}

/// The role one lower-case word stands for.
fn word_role(word: &str) -> Option<Role> {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| word.starts_with(p));
    if word == "pre" || word == "rise" || starts(&["build", "riser", "prechorus", "predrop"]) {
        Some(Role::Build)
    } else if starts(&["drop", "chorus", "hook", "climax", "refrain", "payoff"]) {
        Some(Role::Drop)
    } else if starts(&["break", "bridge", "interlude", "lull"]) {
        Some(Role::Break)
    } else if starts(&["intro", "opening"]) {
        Some(Role::Intro)
    } else if starts(&["outro", "outtro", "ending", "fade"]) || word == "end" {
        Some(Role::Outro)
    } else if starts(&["verse"]) {
        Some(Role::Verse)
    } else {
        None
    }
}

/// One section of the song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The name (a marker's, or `Bars 1-8`).
    pub name: String,
    /// The guessed role.
    pub role: Role,
    /// First tick.
    pub start: i64,
    /// End tick (exclusive), after `start`.
    pub end: i64,
}

impl Section {
    fn new(name: String, start: i64, end: i64) -> Self {
        let role = Role::guess(&name);
        Self {
            name,
            role,
            start,
            end,
        }
    }
}

/// The sections of `song` (see the module docs), in time order, all inside `0..song.end_tick()`.
/// A song with no length has none.
pub fn sections(song: &Song) -> Vec<Section> {
    let end = song.end_tick();
    if end <= 0 {
        return Vec::new();
    }
    let mut out: Vec<Section> = if !song.ranges.is_empty() {
        song.ranges
            .iter()
            .filter(|r| r.start < end)
            .map(|r| Section::new(r.name.clone(), r.start.max(0), r.end.min(end)))
            .filter(|s| s.end > s.start)
            .collect()
    } else if !song.markers.is_empty() {
        let mut ticks: Vec<(i64, &str)> = song
            .markers
            .iter()
            .filter(|m| m.tick < end)
            .map(|m| (m.tick, m.name.as_str()))
            .collect();
        ticks.dedup_by_key(|(t, _)| *t);
        let mut out = Vec::new();
        if ticks.first().is_none_or(|(t, _)| *t > 0) {
            let first = ticks.first().map_or(end, |(t, _)| *t);
            out.push(Section::new("Start".to_owned(), 0, first));
        }
        for (i, (tick, name)) in ticks.iter().enumerate() {
            let next = ticks.get(i + 1).map_or(end, |(t, _)| *t);
            out.push(Section::new((*name).to_owned(), *tick, next));
        }
        out
    } else {
        let table = song.meter_table();
        let map = table.map();
        let last_bar = map.bar_at(end - 1);
        let mut out = Vec::new();
        let mut bar = 1u32;
        while bar <= last_bar {
            let to_bar = bar + DEFAULT_SECTION_BARS - 1;
            let start = map.bar_start(bar);
            let stop = map.bar_start(to_bar + 1).min(end);
            out.push(Section::new(
                format!("Bars {bar}-{}", to_bar.min(last_bar)),
                start,
                stop,
            ));
            bar += DEFAULT_SECTION_BARS;
        }
        out
    };
    out.sort_by_key(|s| (s.start, s.end));
    out
}

/// The stretches of the song a track sounds in: its notes (as they play, with tails) or its
/// audible audio clips.
fn sounding_spans(song: &Song, track: &Track) -> Vec<(i64, i64)> {
    if track.is_audio() {
        let mut out = Vec::new();
        for lane in track.lanes.iter().filter(|l| l.kind.is_audio()) {
            for (id, start, end) in audible_spans(lane) {
                let audio = lane
                    .clips
                    .iter()
                    .any(|c| c.id == id && matches!(c.content, ClipContent::Audio(_)));
                if audio {
                    out.push((start, end));
                }
            }
        }
        out
    } else {
        flatten_track_notes(song, track)
            .into_iter()
            .map(|(tick, length, _, _)| (tick, tick + length.max(1)))
            .collect()
    }
}

/// How many tracks play in each of `sections`, and how many tracks play anywhere (the tracks that
/// are not muted away, see [`effective_tracks`]).
pub fn active_counts(song: &Song, sections: &[Section]) -> (Vec<usize>, usize) {
    let mut counts = vec![0usize; sections.len()];
    let mut total = 0;
    for track in effective_tracks(song) {
        let spans = sounding_spans(song, track);
        if spans.is_empty() {
            continue;
        }
        total += 1;
        for (count, section) in counts.iter_mut().zip(sections) {
            if spans
                .iter()
                .any(|&(a, b)| a < section.end && b > section.start)
            {
                *count += 1;
            }
        }
    }
    (counts, total)
}

/// What a section measures: the three numbers read off the master and the track count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    /// Loudness in dB.
    pub loudness_db: f32,
    /// Brightness (spectral centroid) in Hz; 0 for silence.
    pub brightness_hz: f32,
    /// Width, 0 (mono) to 1.
    pub width: f32,
    /// The tracks that play in the section.
    pub tracks: usize,
}

fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The energy of a section, 0 to 1: the [`WEIGHTS`] applied to the four measures, each put on a
/// 0-1 scale first (loudness between [`LOUD_FLOOR_DB`] and [`LOUD_CEIL_DB`], brightness on a log
/// scale between [`BRIGHT_FLOOR_HZ`] and [`BRIGHT_CEIL_HZ`], width up to [`WIDTH_CEIL`], and the
/// tracks as a share of `total_tracks`).
pub fn energy(m: &Measure, total_tracks: usize) -> f32 {
    let loud = unit((m.loudness_db - LOUD_FLOOR_DB) / (LOUD_CEIL_DB - LOUD_FLOOR_DB));
    let tracks = if total_tracks == 0 {
        0.0
    } else {
        unit(m.tracks as f32 / total_tracks as f32)
    };
    let bright = if m.brightness_hz > 0.0 {
        unit((m.brightness_hz / BRIGHT_FLOOR_HZ).log2() / (BRIGHT_CEIL_HZ / BRIGHT_FLOOR_HZ).log2())
    } else {
        0.0
    };
    let width = unit(m.width / WIDTH_CEIL);
    unit(
        WEIGHTS.loudness * loud
            + WEIGHTS.tracks * tracks
            + WEIGHTS.brightness * bright
            + WEIGHTS.width * width,
    )
}

/// A section with what it measures and its energy.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionEnergy {
    /// The section.
    pub section: Section,
    /// What it measures.
    pub measure: Measure,
    /// Its energy, 0 to 1.
    pub energy: f32,
}

/// A place where the shape of the energy map works against the arrangement. Indexes are into
/// [`EnergyMap::sections`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// A build with more energy than the drop right after it: the drop reads as a letdown.
    BuildLouderThanDrop {
        /// The build.
        build: usize,
        /// The drop after it.
        drop: usize,
    },
    /// The strongest drop is not the top of the map: another section has more energy.
    DropNotTop {
        /// The strongest drop.
        drop: usize,
        /// The section with the most energy.
        top: usize,
    },
    /// Two neighbours with near-equal energy that together run [`FLAT_BARS`] bars or more: the
    /// song does not move there.
    FlatRun {
        /// The first section.
        first: usize,
        /// The section after it.
        second: usize,
    },
}

impl Flag {
    /// The sections the flag is about, first to last.
    pub fn sections(&self) -> (usize, usize) {
        match *self {
            Flag::BuildLouderThanDrop { build, drop } => (build, drop),
            Flag::DropNotTop { drop, top } => (drop.min(top), drop.max(top)),
            Flag::FlatRun { first, second } => (first, second),
        }
    }

    /// One sentence for the status line and the tooltip.
    pub fn message(&self, map: &EnergyMap) -> String {
        let name = |i: usize| map.sections.get(i).map_or("?", |s| s.section.name.as_str());
        let value = |i: usize| map.sections.get(i).map_or(0.0, |s| s.energy);
        match *self {
            Flag::BuildLouderThanDrop { build, drop } => format!(
                "{} ({:.2}) is stronger than the drop after it, {} ({:.2})",
                name(build),
                value(build),
                name(drop),
                value(drop)
            ),
            Flag::DropNotTop { drop, top } => format!(
                "{} ({:.2}) is not the top of the map: {} has {:.2}",
                name(drop),
                value(drop),
                name(top),
                value(top)
            ),
            Flag::FlatRun { first, second } => format!(
                "{} and {} have near-equal energy ({:.2}, {:.2}) over {} or more bars",
                name(first),
                name(second),
                value(first),
                value(second),
                FLAT_BARS
            ),
        }
    }
}

/// The energy of every section, and the flags.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnergyMap {
    /// The sections in time order.
    pub sections: Vec<SectionEnergy>,
    /// The flags, in the order of the first section they are about.
    pub flags: Vec<Flag>,
}

impl EnergyMap {
    /// The map of `sections` with their `measures` (one each; extra entries of either are
    /// ignored); `total_tracks` is the number of tracks the song uses ([`active_counts`]).
    pub fn new(
        song: &Song,
        sections: Vec<Section>,
        measures: &[Measure],
        total_tracks: usize,
    ) -> Self {
        let entries: Vec<SectionEnergy> = sections
            .into_iter()
            .zip(measures)
            .map(|(section, m)| SectionEnergy {
                energy: energy(m, total_tracks),
                measure: *m,
                section,
            })
            .collect();
        let flags = flags_of(song, &entries);
        Self {
            sections: entries,
            flags,
        }
    }

    /// The index of the section that holds `tick`.
    pub fn section_at(&self, tick: i64) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.section.start <= tick && tick < s.section.end)
    }

    /// The flags that are about section `i`.
    pub fn flags_of(&self, i: usize) -> impl Iterator<Item = &Flag> {
        self.flags.iter().filter(move |f| {
            let (a, b) = f.sections();
            a == i || b == i
        })
    }
}

fn flags_of(song: &Song, entries: &[SectionEnergy]) -> Vec<Flag> {
    let mut flags = Vec::new();
    for i in 0..entries.len().saturating_sub(1) {
        let (a, b) = (&entries[i], &entries[i + 1]);
        if a.section.role == Role::Build
            && b.section.role == Role::Drop
            && a.energy - b.energy >= FLAG_MARGIN
        {
            flags.push(Flag::BuildLouderThanDrop {
                build: i,
                drop: i + 1,
            });
        }
    }
    let top = entries
        .iter()
        .enumerate()
        .fold(None::<usize>, |best, (i, e)| match best {
            Some(b) if entries[b].energy >= e.energy => Some(b),
            _ => Some(i),
        });
    let strongest_drop = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.section.role == Role::Drop)
        .fold(None::<usize>, |best, (i, e)| match best {
            Some(b) if entries[b].energy >= e.energy => Some(b),
            _ => Some(i),
        });
    if let (Some(top), Some(drop)) = (top, strongest_drop)
        && entries[top].section.role != Role::Drop
        && entries[top].energy - entries[drop].energy >= FLAG_MARGIN
    {
        flags.push(Flag::DropNotTop { drop, top });
    }
    let table = song.meter_table();
    let map = table.map();
    for i in 0..entries.len().saturating_sub(1) {
        let (a, b) = (&entries[i], &entries[i + 1]);
        let bars = map
            .bar_at((b.section.end - 1).max(0))
            .saturating_sub(map.bar_at(a.section.start))
            + 1;
        if (a.energy - b.energy).abs() < NEAR_EQUAL
            && a.energy.min(b.energy) >= SILENT_ENERGY
            && bars >= FLAT_BARS
        {
            flags.push(Flag::FlatRun {
                first: i,
                second: i + 1,
            });
        }
    }
    flags.sort_by_key(|f| f.sections());
    flags
}

#[cfg(test)]
#[path = "energy_map_tests.rs"]
mod tests;
