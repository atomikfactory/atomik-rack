//! Stems: the strips of the console that can be exported or bounced on their own.
//!
//! A **stem** is the output of one strip of the console over the whole song: a channel (its
//! fader, pan and inserts applied), an output bus, an FX return or the master. Each strip has a
//! **tap**, the stereo pair of sockets of the flat rack ([`crate::RackDoc::expand`]) that carries
//! its output; the offline render cables every tap to its own pair of interface outputs, so all
//! the stems are read in one deterministic pass of the same engine that plays the song.
//!
//! A channel also has a **dry** tap: the signal that enters the strip, the source's own output
//! with no fader, pan, insert or send applied ([`freeze_tap`]). A source that feeds a channel
//! through a container has none.
//!
//! The list is a pure function of the document: console order first (channels, then buses, then
//! the FX returns by slot, then the master), so the stems of a project come out in the order the
//! mixer shows them.

use crate::console::{ConsoleBus, ConsoleChannel};
use crate::{DeviceKey, RackDoc, SocketRef, freeze_tap};
use atomik_device_sdk::Registry;

/// What kind of strip a stem is the output of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StemKind {
    /// A channel strip.
    Channel,
    /// An output bus.
    Bus,
    /// An FX return: the output of the effect on a return slot.
    Return,
    /// The master section.
    Master,
}

impl StemKind {
    /// The word for the kind, as the mixer says it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Bus => "bus",
            Self::Return => "return",
            Self::Master => "master",
        }
    }
}

/// One strip that can be taken as a stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemSource {
    /// What the strip is.
    pub kind: StemKind,
    /// The strip's device (the effect for a return).
    pub key: DeviceKey,
    /// The name the stem goes by: the track or device name.
    pub name: String,
    /// The sockets that carry the strip's output (left, right).
    pub tap: [SocketRef; 2],
    /// The sockets that carry the signal entering the strip (channels only).
    pub dry: Option<[SocketRef; 2]>,
}

fn pair(key: &DeviceKey, left: &str, right: &str) -> [SocketRef; 2] {
    let at = |socket: &str| SocketRef {
        device: key.clone(),
        socket: socket.to_owned(),
    };
    [at(left), at(right)]
}

/// The name of the device `key`, or the key itself when the device is gone.
fn device_name(doc: &RackDoc, key: &DeviceKey) -> String {
    doc.device(key)
        .map(|d| d.name.trim())
        .filter(|n| !n.is_empty())
        .map_or_else(|| key.0.clone(), ToOwned::to_owned)
}

/// Every strip of the console of `doc` that has an output, in console order (see the module
/// docs). A document without a console has none.
pub fn stem_sources(doc: &RackDoc, registry: &Registry) -> Vec<StemSource> {
    let console = &doc.console;
    let flat = doc.expand(registry);
    let mut out = Vec::new();
    for ConsoleChannel { channel, source } in &console.channels {
        // A channel is named after the track it plays, else after its own device.
        let name = source
            .as_ref()
            .map_or_else(|| device_name(doc, channel), |s| device_name(doc, s));
        out.push(StemSource {
            kind: StemKind::Channel,
            key: channel.clone(),
            name,
            tap: pair(channel, "out_l", "out_r"),
            dry: source.as_ref().and_then(|s| freeze_tap(&flat, s, &[])),
        });
    }
    for ConsoleBus { bus, name } in &console.buses {
        out.push(StemSource {
            kind: StemKind::Bus,
            key: bus.clone(),
            name: name.clone(),
            tap: pair(bus, "out_l", "out_r"),
            dry: None,
        });
    }
    for effect in console.returns.iter().flatten() {
        out.push(StemSource {
            kind: StemKind::Return,
            key: effect.clone(),
            name: device_name(doc, effect),
            tap: pair(effect, "out_l", "out_r"),
            dry: None,
        });
    }
    if let Some(master) = &console.master {
        out.push(StemSource {
            kind: StemKind::Master,
            key: master.clone(),
            name: "Master".to_owned(),
            tap: pair(master, "main_l", "main_r"),
            dry: None,
        });
    }
    out
}

/// The characters a file name cannot hold on some platform.
const FORBIDDEN: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// The longest stem file name stem, in characters (the extension and the number come on top).
pub const MAX_STEM_NAME_CHARS: usize = 80;

/// A name safe to use as a file name: the characters no platform allows become `-`, control
/// characters go, the ends are trimmed of spaces and dots, and an empty result is `Stem`.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if FORBIDDEN.contains(&c) { '-' } else { c })
        .take(MAX_STEM_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "Stem".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The file names of `sources`, without the extension: `01 Kick`, `02 Bass`, ... numbered in
/// order so they sort as the mixer does and never collide, whatever the names.
pub fn stem_file_names(sources: &[StemSource]) -> Vec<String> {
    let width = sources.len().to_string().len().max(2);
    sources
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{:0width$} {}", i + 1, safe_file_name(&s.name)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_numbered() {
        assert_eq!(safe_file_name("a/b:c*d"), "a-b-c-d");
        assert_eq!(safe_file_name("  .. "), "Stem");
        assert_eq!(safe_file_name("Kick\u{7}"), "Kick");
        assert_eq!(safe_file_name(&"x".repeat(200)).chars().count(), 80);
        let source = |name: &str| StemSource {
            kind: StemKind::Channel,
            key: DeviceKey("k".into()),
            name: name.to_owned(),
            tap: pair(&DeviceKey("k".into()), "out_l", "out_r"),
            dry: None,
        };
        let names = stem_file_names(&[source("Kick"), source("Kick"), source("a|b")]);
        assert_eq!(names, ["01 Kick", "02 Kick", "03 a-b"]);
    }
}
