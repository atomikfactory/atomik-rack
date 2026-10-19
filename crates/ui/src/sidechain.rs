//! The compressor's sidechain helper: the "SC from..." choices of a
//! compressor panel and the single [`Command::Batch`] that applies one.
//!
//! # Source choice (documented)
//! There is no "sidechain from" model command, so the helper cables ordinary sockets into the
//! compressor's `sc_in_l` / `sc_in_r`:
//!
//! * a **console channel** is keyed from its **insert send** (`insert_send_l` / `insert_send_r`):
//!   the signal after the channel's input gain, filters, dynamics and EQ and *before* the fader
//!   and pan, so it is pre-fader; it is not silenced by mute or solo (the channel's insert send
//!   is always live), which is what a kick channel keying a bass compressor needs. The insert
//!   send is an ordinary output (it may also feed the channel's own insert effect: outputs fan
//!   out);
//! * any other (non-console) device with `out_l` and `out_r` audio outputs is keyed from those
//!   two (an instrument, an effect, the interface is not offered).
//!
//! The helper never edits the sources, only cables into the compressor.
//!
//! # The command
//! [`sidechain_command`] returns one `Command::Batch`: `Disconnect` of each of `sc_in_l` /
//! `sc_in_r` that has a cable, `Connect` of the chosen pair and `SetParam sc_enable 1`; for
//! [`ScSource::Off`] the disconnects and `SetParam sc_enable 0`. A batch is one undo step, and
//! the model rejects it as a whole (cycle, unknown device) with nothing changed. `None` means
//! the document already is in the requested state.

use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_model::console::is_console_type;
use atomik_model::{Cable, Command, DeviceKey, RackDevice, RackDoc, SocketRef};

/// The compressor's sidechain input sockets.
pub const SC_IN: [&str; 2] = ["sc_in_l", "sc_in_r"];
/// The compressor's sidechain switch param.
pub const SC_ENABLE: &str = "sc_enable";

/// A sidechain source choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScSource {
    /// No sidechain: cables removed, `sc_enable` 0.
    Off,
    /// A console channel (its pre-fader insert send).
    Channel(DeviceKey),
    /// Another device (its `out_l` / `out_r`).
    Device(DeviceKey),
}

/// One row of the "SC from..." list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScEntry {
    /// Text shown.
    pub label: String,
    /// What choosing it does.
    pub source: ScSource,
}

fn output_pair(source: &ScSource) -> Option<(&DeviceKey, &'static str, &'static str)> {
    match source {
        ScSource::Off => None,
        ScSource::Channel(k) => Some((k, "insert_send_l", "insert_send_r")),
        ScSource::Device(k) => Some((k, "out_l", "out_r")),
    }
}

fn has_audio_out(registry: &Registry, device: &RackDevice, l: &str, r: &str) -> bool {
    registry.get(&device.type_id).is_some_and(|f| {
        [l, r].iter().all(|k| {
            f.descriptor.sockets.iter().any(|s| {
                s.key == *k && s.kind == SocketKind::Audio && s.direction == Direction::Output
            })
        })
    })
}

/// The choices for compressor `comp`: Off, then the console channels in console order, then the
/// other devices with `out_l` / `out_r` in rack order. The compressor itself is not offered.
pub fn sidechain_entries(doc: &RackDoc, registry: &Registry, comp: &DeviceKey) -> Vec<ScEntry> {
    let mut out = vec![ScEntry {
        label: "Off".to_string(),
        source: ScSource::Off,
    }];
    for ch in &doc.console.channels {
        let Some(d) = doc.devices.iter().find(|d| d.key == ch.channel) else {
            continue;
        };
        if d.key != *comp && has_audio_out(registry, d, "insert_send_l", "insert_send_r") {
            out.push(ScEntry {
                label: d.name.clone(),
                source: ScSource::Channel(d.key.clone()),
            });
        }
    }
    for d in &doc.devices {
        if d.key == *comp
            || is_console_type(&d.type_id)
            || !has_audio_out(registry, d, "out_l", "out_r")
        {
            continue;
        }
        out.push(ScEntry {
            label: d.name.clone(),
            source: ScSource::Device(d.key.clone()),
        });
    }
    out
}

fn cable_into<'a>(doc: &'a RackDoc, comp: &DeviceKey, socket: &str) -> Option<&'a Cable> {
    doc.cables
        .iter()
        .find(|c| c.to.device == *comp && c.to.socket == socket)
}

/// The source device cabled to the compressor's sidechain input (left, else right), and what
/// kind of choice it corresponds to.
pub fn current_source(doc: &RackDoc, comp: &DeviceKey) -> Option<ScSource> {
    let c = cable_into(doc, comp, SC_IN[0]).or_else(|| cable_into(doc, comp, SC_IN[1]))?;
    let is_channel = doc.console.channel_index(&c.from.device).is_some()
        && c.from.socket.starts_with("insert_send");
    Some(if is_channel {
        ScSource::Channel(c.from.device.clone())
    } else {
        ScSource::Device(c.from.device.clone())
    })
}

/// The display name of the device keying the compressor, if its sidechain is cabled.
pub fn current_name(doc: &RackDoc, comp: &DeviceKey) -> Option<String> {
    let src = match current_source(doc, comp)? {
        ScSource::Channel(k) | ScSource::Device(k) => k,
        ScSource::Off => return None,
    };
    Some(
        doc.devices
            .iter()
            .find(|d| d.key == src)
            .map_or_else(|| src.0.clone(), |d| d.name.clone()),
    )
}

fn sc_enabled(doc: &RackDoc, comp: &DeviceKey, registry: &Registry) -> bool {
    doc.param_value(comp, SC_ENABLE, registry)
        .is_some_and(|v| v >= 0.5)
}

fn set_enable(comp: &DeviceKey, on: bool) -> Command {
    Command::SetParam {
        key: comp.clone(),
        param: SC_ENABLE.to_string(),
        value: if on { 1.0 } else { 0.0 },
        merge: None,
    }
}

/// The batch that makes `source` the sidechain of compressor `comp` (see the module docs), or
/// `None` when nothing would change.
pub fn sidechain_command(
    doc: &RackDoc,
    registry: &Registry,
    comp: &DeviceKey,
    source: &ScSource,
) -> Option<Command> {
    let socket = |s: &str| SocketRef {
        device: comp.clone(),
        socket: s.to_string(),
    };
    let mut batch = Vec::new();
    match output_pair(source) {
        None => {
            for s in SC_IN {
                if cable_into(doc, comp, s).is_some() {
                    batch.push(Command::Disconnect { to: socket(s) });
                }
            }
            if sc_enabled(doc, comp, registry) {
                batch.push(set_enable(comp, false));
            }
        }
        Some((from, l, r)) => {
            let want = [
                Cable {
                    from: SocketRef {
                        device: from.clone(),
                        socket: l.to_string(),
                    },
                    to: socket(SC_IN[0]),
                },
                Cable {
                    from: SocketRef {
                        device: from.clone(),
                        socket: r.to_string(),
                    },
                    to: socket(SC_IN[1]),
                },
            ];
            let already = want
                .iter()
                .all(|w| cable_into(doc, comp, &w.to.socket) == Some(w));
            if already && sc_enabled(doc, comp, registry) {
                return None;
            }
            for w in want {
                match cable_into(doc, comp, &w.to.socket) {
                    Some(have) if *have == w => {}
                    have => {
                        if have.is_some() {
                            batch.push(Command::Disconnect { to: w.to.clone() });
                        }
                        batch.push(Command::Connect(w));
                    }
                }
            }
            if !sc_enabled(doc, comp, registry) {
                batch.push(set_enable(comp, true));
            }
        }
    }
    (!batch.is_empty()).then_some(Command::Batch(batch))
}
