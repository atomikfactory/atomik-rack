//! Console links and the output display: which cables the rack hides by
//! default (the channel to master links, classified by `atomik_model::link_kind`) and the label
//! "Output: Master" a channel shows instead, computed from the document by the model's
//! `output_dest`. Pure functions of the document; the view caches the result per sync.

use crate::interface::interface_descriptor;
use atomik_device_sdk::Registry;
use atomik_model::console::CHANNEL_TYPE;
use atomik_model::{
    Cable, DeviceKey, INTERFACE_KEY, OutputDest, RackDoc, SocketRef, link_kind, output_dest,
};
use std::collections::{BTreeMap, BTreeSet};

/// What a channel or the master shows about where its output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDisplay {
    /// The label, e.g. `Output: Master`.
    pub text: String,
    /// The output leaves the master: painted in the warning colour.
    pub warn: bool,
    /// The tooltip (only for an output that leaves the master).
    pub tooltip: Option<String>,
}

/// The cables of `doc` that are console links.
pub fn links_of(doc: &RackDoc) -> BTreeSet<Cable> {
    doc.cables
        .iter()
        .filter(|c| link_kind(doc, c).is_some())
        .cloned()
        .collect()
}

fn is_channel(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.console.channels.iter().any(|c| &c.channel == key)
        || doc
            .devices
            .iter()
            .any(|d| &d.key == key && d.type_id == CHANNEL_TYPE)
}

/// The short name of a socket: its descriptor name without a leading "Output " ("Output 3" is
/// "3", so "Audio I/O 3"), the key when the descriptor is unknown.
fn socket_label(doc: &RackDoc, registry: &Registry, at: &SocketRef) -> String {
    let name = if at.device.0 == INTERFACE_KEY {
        let d = interface_descriptor(doc.input_channels, doc.output_channels);
        d.sockets
            .iter()
            .find(|s| s.key == at.socket)
            .map(|s| s.name.to_string())
    } else {
        doc.devices
            .iter()
            .find(|d| d.key == at.device)
            .and_then(|d| registry.get(&d.type_id))
            .and_then(|f| f.descriptor.socket_by_key(&at.socket))
            .map(|s| s.name.to_string())
    };
    match name {
        Some(n) => n.strip_prefix("Output ").map_or(n.clone(), str::to_string),
        None => at.socket.clone(),
    }
}

fn device_label(doc: &RackDoc, at: &SocketRef) -> String {
    if at.device.0 == INTERFACE_KEY {
        return "Audio I/O".to_string();
    }
    doc.devices
        .iter()
        .find(|d| d.key == at.device)
        .map_or_else(|| at.device.0.clone(), |d| d.name.clone())
}

/// `Audio I/O 3+4`, `Line Mixer Ch1 in L+Ch1 in R`: the destinations grouped by device, in
/// cable order, the sockets of one device joined with `+`.
fn destinations_text(doc: &RackDoc, registry: &Registry, to: &[SocketRef]) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for at in to {
        let device = device_label(doc, at);
        let socket = socket_label(doc, registry, at);
        match groups.iter_mut().find(|(d, _)| *d == device) {
            Some((_, sockets)) => sockets.push(socket),
            None => groups.push((device, vec![socket])),
        }
    }
    groups
        .iter()
        .map(|(d, s)| format!("{d} {}", s.join("+")))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The output display of channel `key`, `None` for a device that is not a channel.
pub fn channel_display(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Option<OutputDisplay> {
    if !is_channel(doc, key) {
        return None;
    }
    Some(match output_dest(doc, key) {
        OutputDest::Master => OutputDisplay {
            text: "Output: Master".into(),
            warn: false,
            tooltip: None,
        },
        OutputDest::Bus(bus) => {
            let name = doc
                .console
                .buses
                .iter()
                .find(|b| b.bus == bus)
                .map_or(bus.0.as_str(), |b| b.name.as_str());
            OutputDisplay {
                text: format!("Output: Bus {name}"),
                warn: false,
                tooltip: None,
            }
        }
        // Interim: an Audio I/O pair still reads like a direct cable (warning colour) until the
        // console UI shows it as a first-class destination.
        OutputDest::AudioIo(n) => {
            let interface = DeviceKey("interface".into());
            let to: Vec<SocketRef> = [2 * n - 1, 2 * n]
                .iter()
                .map(|o| SocketRef {
                    device: interface.clone(),
                    socket: format!("out_{o}"),
                })
                .collect();
            let text = destinations_text(doc, registry, &to);
            OutputDisplay {
                tooltip: Some(format!("Leaves the master: cabled to {text}")),
                text: format!("Output: {text}"),
                warn: true,
            }
        }
        OutputDest::None => OutputDisplay {
            text: "Output: none".into(),
            warn: false,
            tooltip: None,
        },
        OutputDest::Direct(to) => {
            let text = destinations_text(doc, registry, &to);
            OutputDisplay {
                tooltip: Some(format!("Leaves the master: cabled to {text}")),
                text: format!("Output: {text}"),
                warn: true,
            }
        }
        OutputDest::Mixed => OutputDisplay {
            text: "Output: Master + other".into(),
            warn: true,
            tooltip: Some(
                "Leaves the master: some outputs are cabled to the master, some elsewhere".into(),
            ),
        },
    })
}

/// The output display of the master: where `main_l` and `main_r` go.
pub fn master_display(doc: &RackDoc, registry: &Registry) -> Option<OutputDisplay> {
    let master = doc.console.master.as_ref()?;
    let to: Vec<SocketRef> = doc
        .cables
        .iter()
        .filter(|c| {
            &c.from.device == master && matches!(c.from.socket.as_str(), "main_l" | "main_r")
        })
        .map(|c| c.to.clone())
        .collect();
    let text = if to.is_empty() {
        "Output: none".to_string()
    } else {
        format!("Output: {}", destinations_text(doc, registry, &to))
    };
    Some(OutputDisplay {
        text,
        warn: false,
        tooltip: None,
    })
}

/// Every output display of the document: the channels and the master.
pub fn displays_of(doc: &RackDoc, registry: &Registry) -> BTreeMap<DeviceKey, OutputDisplay> {
    let mut map = BTreeMap::new();
    for d in &doc.devices {
        if let Some(display) = channel_display(doc, registry, &d.key) {
            map.insert(d.key.clone(), display);
        }
    }
    if let (Some(master), Some(display)) = (&doc.console.master, master_display(doc, registry)) {
        map.insert(master.clone(), display);
    }
    map
}
