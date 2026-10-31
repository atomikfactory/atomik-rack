//! The document invariants, checked: [`RackDoc::validate`].
//!
//! The rules live next to the data they protect (the column order in [`crate::rack_columns`],
//! the tempo mirror in [`crate::RackDoc`]), and every command and loader keeps them. This module
//! gathers them in one place so a mistake in a planner shows up as a named violation at the
//! point it was made (in debug builds after every command, and before a project is written)
//! rather than as a file that does not load or a render that stops short.

use crate::song::{Clip, ClipContent, LaneKind, MAX_TICK};
use crate::song_cmd::{check_end, check_geometry, check_note};
use crate::{DeviceKey, RackDevice, RackDoc};

/// One broken invariant: where, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Where in the document (`song.tracks[1].lanes[0].clips[2]`).
    pub path: String,
    /// What is wrong there.
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// The violations of `list`, one per line.
pub fn describe(list: &[Violation]) -> String {
    list.iter()
        .map(Violation::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// At most this many violations are collected: a document that is wrong in one place is usually
/// wrong in many, and the first few say what to look at.
const MAX_VIOLATIONS: usize = 16;

struct Found(Vec<Violation>);

impl Found {
    fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        if self.0.len() < MAX_VIOLATIONS {
            self.0.push(Violation {
                path: path.into(),
                message: message.into(),
            });
        }
    }

    fn finite(&mut self, path: &str, what: &str, value: f64) {
        if !value.is_finite() {
            self.add(path, format!("{what} is {value}, not a finite number"));
        }
    }
}

impl RackDoc {
    /// Checks the document invariants the commands and the loader keep:
    ///
    /// - the rack columns (see [`RackDoc::check_columns`]);
    /// - the tempo mirror: the song has a tempo map and `song.tempo[0].bpm == tempo_bpm`;
    /// - the console only names devices the rack has (master, channels and their sources, buses,
    ///   FX returns);
    /// - every track has its device, and every clip fits `0..=MAX_TICK` (start, length, offset,
    ///   notes and automation points) with its lane sorted by `(start, id)`; the song End is a
    ///   valid length;
    /// - every stored number is finite: device params, tempo, note velocities, automation
    ///   values and audio gains. A project holding a NaN could not be read back.
    ///
    /// # Errors
    /// Every violation found, up to a limit (the first few are the ones to read).
    pub fn validate(&self) -> Result<(), Vec<Violation>> {
        let mut found = Found(Vec::new());
        if let Err(message) = self.check_columns() {
            found.add("devices", message);
        }
        if let Err(e) = self.info.check() {
            found.add("info", e.to_string());
        }
        self.validate_tempo(&mut found);
        self.validate_console(&mut found);
        self.validate_song(&mut found);
        self.validate_params(&mut found);
        if found.0.is_empty() {
            Ok(())
        } else {
            Err(found.0)
        }
    }

    fn validate_tempo(&self, found: &mut Found) {
        found.finite("tempo_bpm", "the tempo", self.tempo_bpm);
        let Some(first) = self.song.tempo.first() else {
            found.add("song.tempo", "the tempo map has no event");
            return;
        };
        if first.bpm.to_bits() != self.tempo_bpm.to_bits()
            && first.bpm.is_finite()
            && self.tempo_bpm.is_finite()
        {
            found.add(
                "song.tempo[0]",
                format!(
                    "{} BPM is not the rack tempo, {} BPM",
                    first.bpm, self.tempo_bpm
                ),
            );
        }
        for (i, e) in self.song.tempo.iter().enumerate() {
            let path = format!("song.tempo[{i}]");
            found.finite(&path, "the tempo", e.bpm);
            if !(0..=MAX_TICK).contains(&e.tick) {
                found.add(&path, format!("tick {} is outside 0..={MAX_TICK}", e.tick));
            }
        }
    }

    fn validate_console(&self, found: &mut Found) {
        let c = &self.console;
        let mut known = |path: String, key: &DeviceKey| {
            if self.resolve(key).is_none() {
                found.add(path, format!("names {key}, which is not in the rack"));
            }
        };
        if let Some(master) = &c.master {
            known("console.master".into(), master);
        }
        for (i, ch) in c.channels.iter().enumerate() {
            known(format!("console.channels[{i}]"), &ch.channel);
            if let Some(source) = &ch.source {
                known(format!("console.channels[{i}].source"), source);
            }
        }
        for (i, b) in c.buses.iter().enumerate() {
            known(format!("console.buses[{i}]"), &b.bus);
        }
        for (i, r) in c.returns.iter().enumerate() {
            if let Some(device) = r {
                known(format!("console.returns[{i}]"), device);
            }
        }
    }

    fn validate_song(&self, found: &mut Found) {
        let song = &self.song;
        if let Err(message) = check_end(song.end) {
            found.add("song.end", message);
        }
        for (t, track) in song.tracks.iter().enumerate() {
            let at = format!("song.tracks[{t}]");
            if self.resolve(&track.device).is_none() {
                found.add(&at, format!("{} is not in the rack", track.device));
            }
            if track.lanes.is_empty() {
                found.add(&at, "a track needs a lane".to_owned());
            }
            for (l, lane) in track.lanes.iter().enumerate() {
                let is_automation = matches!(lane.kind, LaneKind::Automation(_));
                if lane
                    .static_value
                    .is_some_and(|v| !is_automation || !v.is_finite())
                {
                    found.add(
                        format!("{at}.lanes[{l}]"),
                        "a static value is a finite number on an automation lane only",
                    );
                }
                if lane.muted && lane.kind.is_audio() {
                    found.add(format!("{at}.lanes[{l}]"), "an audio lane cannot be muted");
                }
                for (c, clip) in lane.clips.iter().enumerate() {
                    let path = format!("{at}.lanes[{l}].clips[{c}]");
                    validate_clip(&path, clip, found);
                    if c > 0 {
                        let before = &lane.clips[c - 1];
                        if (before.start, before.id) >= (clip.start, clip.id) {
                            found.add(&path, "the lane is not sorted by (start, id)");
                        }
                    }
                }
            }
        }
    }

    fn validate_params(&self, found: &mut Found) {
        fn walk(path: &str, devices: &[RackDevice], found: &mut Found) {
            for d in devices {
                let at = format!("{path}{}", d.key);
                for (param, value) in &d.params {
                    found.finite(
                        &format!("{at}.params.{param}"),
                        "the value",
                        f64::from(*value),
                    );
                }
                if let Some(inner) = &d.inner {
                    walk(&format!("{at}/"), &inner.devices, found);
                }
            }
        }
        walk("devices.", &self.devices, found);
    }
}

fn validate_clip(path: &str, clip: &Clip, found: &mut Found) {
    if let Err(message) = check_geometry(clip.start, clip.length, clip.offset, clip.loop_length) {
        found.add(path, message);
    }
    match &clip.content {
        ClipContent::Notes(notes) => {
            for (i, n) in notes.iter().enumerate() {
                if let Err(message) = check_note(n.tick, n.length, n.key, n.velocity) {
                    found.add(format!("{path}.notes[{i}]"), message);
                }
            }
        }
        ClipContent::Automation(points) => {
            for (i, p) in points.iter().enumerate() {
                let at = format!("{path}.points[{i}]");
                found.finite(&at, "the value", f64::from(p.value));
                if !(0..=MAX_TICK).contains(&p.tick) {
                    found.add(&at, format!("tick {} is outside 0..={MAX_TICK}", p.tick));
                }
            }
        }
        ClipContent::Audio(a) => {
            found.finite(path, "the gain", f64::from(a.gain_db));
            for (i, p) in a.envelope.iter().enumerate() {
                found.finite(
                    &format!("{path}.envelope[{i}]"),
                    "the gain",
                    f64::from(p.gain_db),
                );
            }
        }
    }
    for (i, lane) in clip.controllers.iter().enumerate() {
        for (j, p) in lane.points.iter().enumerate() {
            found.finite(
                &format!("{path}.controllers[{i}].points[{j}]"),
                "the value",
                f64::from(p.value),
            );
        }
    }
}
