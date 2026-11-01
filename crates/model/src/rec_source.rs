//! Where an audio track records from: the Audio I/O inputs it is cabled to (the default), or the
//! output of a console strip (resampling).
//!
//! The choice is the audio track device's `rec_source` param ([`REC_SOURCE_PARAM`]): a plain
//! number, so it is an ordinary undoable `SetParam` and needs no project format change (a project
//! without it records the inputs, as before). The number is positional:
//!
//! ```text
//! 0            the Audio I/O inputs (the cables into in_l / in_r)
//! 1..=999      console channel k, in console order
//! 1001..=1999  bus k, in console order
//! 2000         the master
//! ```
//!
//! Channels and buses have their own ranges, so adding a bus never shifts a channel's number.
//! Moving or removing a strip does shift the numbers behind it (the choice follows the position,
//! not the strip), and a number with no strip behind it records nothing and is reported when the
//! take starts.
//!
//! **Cycle rule.** A track must not record a signal it contributes to: the recorded clip would
//! play into the very strip it was recorded from. [`source_refusal`] walks the cables upstream
//! from the strip and refuses when it reaches the recording track's device (its own channel, or a
//! bus or the master the track feeds). Route the track's channel to an Audio I/O pair, or record
//! another strip.

use crate::console::{Console, MASTER_TYPE};
use crate::{DeviceKey, INTERFACE_KEY, RackDoc};
use atomik_core::{REC_SOURCE_MAX, REC_SOURCE_PARAM};
use atomik_device_sdk::Registry;

/// Most strips of one kind a source number can name.
const RANGE: u16 = 999;
/// The first bus number minus one.
const BUS_BASE: u16 = 1000;
/// The master's number.
const MASTER: u16 = 2000;

/// What an audio track records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecSource {
    /// The Audio I/O inputs the track is cabled to.
    #[default]
    Interface,
    /// Console channel `n` (1-based, console order).
    Channel(u16),
    /// Bus `n` (1-based, console order).
    Bus(u16),
    /// The master.
    Master,
}

impl RecSource {
    /// The source a param value names (rounded; anything out of range or not a number records
    /// the inputs).
    pub fn from_value(value: f32) -> Self {
        if !value.is_finite() || !(0.0..=REC_SOURCE_MAX).contains(&value) {
            return Self::Interface;
        }
        let n = value.round() as u16;
        match n {
            1..=RANGE => Self::Channel(n),
            n if n == MASTER => Self::Master,
            n if n > BUS_BASE && n <= BUS_BASE + RANGE => Self::Bus(n - BUS_BASE),
            _ => Self::Interface,
        }
    }

    /// The param value of this source.
    pub fn value(self) -> f32 {
        match self {
            Self::Interface => 0.0,
            Self::Channel(n) => f32::from(n.clamp(1, RANGE)),
            Self::Bus(n) => f32::from(BUS_BASE + n.clamp(1, RANGE)),
            Self::Master => f32::from(MASTER),
        }
    }

    /// True for a console strip (anything but the inputs).
    pub fn is_strip(self) -> bool {
        self != Self::Interface
    }

    /// The device key of the strip this source names in `console`; `None` for the inputs, or when
    /// there is no such strip.
    pub fn strip(self, console: &Console) -> Option<DeviceKey> {
        match self {
            Self::Interface => None,
            Self::Channel(n) => console
                .channels
                .get(usize::from(n) - 1)
                .map(|c| c.channel.clone()),
            Self::Bus(n) => console.buses.get(usize::from(n) - 1).map(|b| b.bus.clone()),
            Self::Master => console.master.clone(),
        }
    }
}

/// The recording source of the audio track `device`: its `rec_source` param, the inputs when it
/// has none stored.
pub fn rec_source(doc: &RackDoc, device: &DeviceKey, registry: &Registry) -> RecSource {
    doc.param_value(device, REC_SOURCE_PARAM, registry)
        .map_or(RecSource::Interface, RecSource::from_value)
}

/// The strips an audio track can record, in menu order, with their labels: every channel
/// (`Channel: name`), every bus (`Bus: name`), then the master. The inputs are not listed.
pub fn source_options(doc: &RackDoc) -> Vec<(RecSource, String)> {
    let name = |key: &DeviceKey| {
        doc.device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    };
    let console = &doc.console;
    let mut out = Vec::new();
    for (k, c) in console.channels.iter().enumerate().take(usize::from(RANGE)) {
        out.push((
            RecSource::Channel(k as u16 + 1),
            format!("Channel: {}", name(&c.channel)),
        ));
    }
    for (k, b) in console.buses.iter().enumerate().take(usize::from(RANGE)) {
        out.push((RecSource::Bus(k as u16 + 1), format!("Bus: {}", b.name)));
    }
    if let Some(master) = &console.master {
        out.push((RecSource::Master, format!("Master: {}", name(master))));
    }
    out
}

/// A short label of `source` for a header: `Ch 3`, `Bus 2`, `Master`.
pub fn source_label(source: RecSource) -> String {
    match source {
        RecSource::Interface => "Inputs".to_string(),
        RecSource::Channel(n) => format!("Ch {n}"),
        RecSource::Bus(n) => format!("Bus {n}"),
        RecSource::Master => "Master".to_string(),
    }
}

/// Why `track` cannot record `source`, or `None` when it can: there is no such strip, or the
/// strip is fed by the track (module docs, cycle rule).
pub fn source_refusal(doc: &RackDoc, track: &DeviceKey, source: RecSource) -> Option<String> {
    if !source.is_strip() {
        return None;
    }
    let Some(strip) = source.strip(&doc.console) else {
        return Some(format!(
            "its record source ({}) does not exist in the mixer",
            source_label(source)
        ));
    };
    if feeds(doc, track, &strip) {
        let name = |key: &DeviceKey| {
            doc.device(key)
                .map_or_else(|| key.0.clone(), |d| d.name.clone())
        };
        return Some(format!(
            "it would record itself: {} is fed by {} (send the track's channel to an Audio I/O pair, or pick another source)",
            name(&strip),
            name(track)
        ));
    }
    None
}

/// True when `from` reaches `target` through the cables (or is it): `target`'s upstream closure
/// holds `from`. Every cable counts, the hidden console links included, with two exceptions that
/// are not signal paths: the walk ends at the Audio I/O device (its inputs and outputs are
/// different wires), and a send bus of the master feeds only its FX send (a channel's send reaches
/// the main mix only through an effect cabled between that FX send and a return).
fn feeds(doc: &RackDoc, from: &DeviceKey, target: &DeviceKey) -> bool {
    // (device, reached through a master FX send output)
    let mut seen: Vec<(&DeviceKey, bool)> = vec![(target, false)];
    let mut at = 0;
    while at < seen.len() {
        let (here, via_send) = seen[at];
        at += 1;
        if here == from {
            return true;
        }
        if here.0 == INTERFACE_KEY {
            continue;
        }
        let master = doc.device(here).is_some_and(|d| d.type_id == MASTER_TYPE);
        for cable in doc.cables.iter().filter(|c| &c.to.device == here) {
            if master && cable.to.socket.starts_with("send_bus") != via_send {
                continue;
            }
            let next = (&cable.from.device, cable.from.socket.starts_with("fx_send"));
            if !seen.contains(&next) {
                seen.push(next);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_stay_apart() {
        for source in [
            RecSource::Interface,
            RecSource::Channel(1),
            RecSource::Channel(999),
            RecSource::Bus(1),
            RecSource::Bus(999),
            RecSource::Master,
        ] {
            assert_eq!(RecSource::from_value(source.value()), source, "{source:?}");
        }
        assert_eq!(RecSource::Bus(1).value(), 1001.0);
        assert_eq!(RecSource::from_value(1000.0), RecSource::Interface);
        assert_eq!(RecSource::from_value(1500.4), RecSource::Bus(500));
        assert_eq!(RecSource::from_value(f32::NAN), RecSource::Interface);
        assert_eq!(RecSource::from_value(-3.0), RecSource::Interface);
        assert_eq!(RecSource::from_value(1.0e9), RecSource::Interface);
    }
}
