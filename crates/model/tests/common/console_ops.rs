//! Random console commands resolved against the current document (project
//! round trips), and the console invariants every document must keep.

use super::*;
use atomik_model::{ConsoleCommand, FX_SLOTS, RackDoc};
use proptest::prelude::*;

/// A console command in abstract form; indices wrap around channels and devices.
#[derive(Debug, Clone)]
pub enum ConsoleOp {
    AddChannel(usize),
    RemoveChannel(usize),
    MoveChannel(usize, usize),
    SetReturn(usize, Option<usize>),
    Compensation(bool),
    AlignReturns(bool),
    SendFollow(bool),
    AddMixChannel,
    CreateParallel(usize),
    ClearSolos,
    ClearMutes,
    /// `solo` (true) or `mute` (false) on a channel.
    Flag(usize, bool, bool),
    /// Reset a channel (keeping or not its routing).
    ResetStrip(usize, bool),
    ClearInserts(usize),
    /// Paste a fixed set of settings on a channel.
    PasteSettings(usize),
    /// A VCA group that follows a folder (the folders wrap around).
    VcaForFolder(usize),
    /// A group starts or stops following a folder.
    FollowFolder(usize, Option<usize>),
    /// A folder plays into a new bus (`None`) or into an existing one.
    RouteFolder(usize, Option<usize>),
    /// A folder stops playing into its bus.
    UnrouteFolder(usize),
    /// A strip joins a group or leaves its group.
    AssignVca(usize, Option<usize>),
    /// A group gets a lane (level, mute or solo by `kind % 3`) on the master's track.
    VcaLane(usize, usize),
    /// A group plays into a new bus (`None`) or into an existing one.
    RouteVca(usize, Option<usize>),
    /// A group stops playing into its bus.
    UnrouteVca(usize),
    /// A group is deleted (with its level lane).
    DeleteVca(usize),
}

pub fn console_op() -> impl Strategy<Value = ConsoleOp> {
    let i = || 0usize..64;
    prop_oneof![
        3 => i().prop_map(ConsoleOp::AddChannel),
        1 => i().prop_map(ConsoleOp::RemoveChannel),
        1 => (i(), 0usize..6).prop_map(|(c, t)| ConsoleOp::MoveChannel(c, t)),
        2 => (0usize..FX_SLOTS + 1, proptest::option::of(i())).prop_map(|(s, e)| ConsoleOp::SetReturn(s, e)),
        1 => any::<bool>().prop_map(ConsoleOp::Compensation),
        1 => any::<bool>().prop_map(ConsoleOp::AlignReturns),
        1 => any::<bool>().prop_map(ConsoleOp::SendFollow),
        1 => Just(ConsoleOp::AddMixChannel),
        1 => i().prop_map(ConsoleOp::CreateParallel),
        1 => Just(ConsoleOp::ClearSolos),
        1 => Just(ConsoleOp::ClearMutes),
        3 => (i(), any::<bool>(), any::<bool>()).prop_map(|(c, s, on)| ConsoleOp::Flag(c, s, on)),
        1 => (i(), any::<bool>()).prop_map(|(c, k)| ConsoleOp::ResetStrip(c, k)),
        1 => i().prop_map(ConsoleOp::ClearInserts),
        1 => i().prop_map(ConsoleOp::PasteSettings),
        2 => i().prop_map(ConsoleOp::VcaForFolder),
        1 => (i(), proptest::option::of(i())).prop_map(|(v, f)| ConsoleOp::FollowFolder(v, f)),
        2 => (i(), proptest::option::of(i())).prop_map(|(f, b)| ConsoleOp::RouteFolder(f, b)),
        1 => i().prop_map(ConsoleOp::UnrouteFolder),
        2 => (i(), proptest::option::of(i())).prop_map(|(c, v)| ConsoleOp::AssignVca(c, v)),
        2 => (i(), 0usize..3).prop_map(|(v, k)| ConsoleOp::VcaLane(v, k)),
        2 => (i(), proptest::option::of(i())).prop_map(|(v, b)| ConsoleOp::RouteVca(v, b)),
        1 => i().prop_map(ConsoleOp::UnrouteVca),
        1 => i().prop_map(ConsoleOp::DeleteVca),
    ]
}

/// Resolves `op` into a command; `None` if it targets nothing.
pub fn resolve_console(op: &ConsoleOp, doc: &RackDoc) -> Option<Command> {
    let n = doc.devices.len();
    let dev = |i: usize| (n > 0).then(|| doc.devices[i % n].key.clone());
    let chans = &doc.console.channels;
    let chan = |i: usize| (!chans.is_empty()).then(|| chans[i % chans.len()].channel.clone());
    let c = |c: ConsoleCommand| Command::Console(c);
    Some(match op {
        ConsoleOp::AddChannel(d) => c(ConsoleCommand::AddChannel { source: dev(*d)? }),
        ConsoleOp::RemoveChannel(ch) => c(ConsoleCommand::RemoveChannel {
            channel: chan(*ch)?,
        }),
        ConsoleOp::MoveChannel(ch, to) => c(ConsoleCommand::MoveChannel {
            channel: chan(*ch)?,
            to: *to,
        }),
        ConsoleOp::SetReturn(slot, e) => c(ConsoleCommand::SetReturn {
            slot: *slot,
            effect: match e {
                Some(e) => Some(dev(*e)?),
                None => None,
            },
        }),
        ConsoleOp::Compensation(on) => c(ConsoleCommand::SetCompensation(*on)),
        ConsoleOp::AlignReturns(on) => c(ConsoleCommand::SetAlignReturns(*on)),
        ConsoleOp::SendFollow(on) => c(ConsoleCommand::SetSendFollow(*on)),
        ConsoleOp::AddMixChannel => c(ConsoleCommand::AddMixChannel {
            name: String::new(),
        }),
        ConsoleOp::CreateParallel(ch) => c(ConsoleCommand::CreateParallel { strip: chan(*ch)? }),
        ConsoleOp::ClearSolos => c(ConsoleCommand::ClearSolos),
        ConsoleOp::ClearMutes => c(ConsoleCommand::ClearMutes),
        ConsoleOp::ResetStrip(ch, keep_routing) => c(ConsoleCommand::ResetStrip {
            strip: chan(*ch)?,
            keep_routing: *keep_routing,
        }),
        ConsoleOp::ClearInserts(ch) => c(ConsoleCommand::ClearInserts { strip: chan(*ch)? }),
        ConsoleOp::PasteSettings(ch) => c(ConsoleCommand::PasteSettings {
            strips: vec![chan(*ch)?],
            clip: atomik_model::ChannelSettingsClip {
                master: false,
                sections: atomik_model::SnapshotScope::ALL,
                params: [("pan".to_owned(), 0.25), ("mute".to_owned(), 1.0)].into(),
                inserts: Vec::new(),
            },
        }),
        ConsoleOp::VcaForFolder(f) => {
            let groups = &doc.song.org.groups;
            let folder = groups.get(f % groups.len().max(1))?.id;
            c(ConsoleCommand::CreateVcaForFolder {
                folder,
                name: format!("Folder {}", doc.console.next_vca_id()),
            })
        }
        ConsoleOp::FollowFolder(v, f) => {
            let vcas = &doc.console.vcas;
            let vca = vcas.get(v % vcas.len().max(1))?.id;
            let groups = &doc.song.org.groups;
            let folder = match f {
                Some(f) => Some(groups.get(f % groups.len().max(1))?.id),
                None => None,
            };
            c(ConsoleCommand::SetVcaFollow { vca, folder })
        }
        ConsoleOp::RouteFolder(f, bus) => {
            let groups = &doc.song.org.groups;
            let folder = groups.get(f % groups.len().max(1))?.id;
            match bus {
                None => c(ConsoleCommand::RouteFolderToNewBus {
                    folder,
                    name: format!("Folder bus {}", doc.console.buses.len() + 1),
                }),
                Some(b) => {
                    let buses = &doc.console.buses;
                    c(ConsoleCommand::RouteFolderToBus {
                        folder,
                        bus: buses.get(b % buses.len().max(1))?.bus.clone(),
                    })
                }
            }
        }
        ConsoleOp::UnrouteFolder(f) => {
            let routes = &doc.console.folder_routes;
            c(ConsoleCommand::UnrouteFolder {
                folder: routes.get(f % routes.len().max(1))?.folder,
            })
        }
        ConsoleOp::AssignVca(ch, v) => {
            let vcas = &doc.console.vcas;
            let vca = match v {
                Some(v) => Some(vcas.get(v % vcas.len().max(1))?.id),
                None => None,
            };
            c(ConsoleCommand::AssignToVca {
                vca,
                strips: vec![chan(*ch)?],
            })
        }
        ConsoleOp::VcaLane(v, kind) => {
            let vcas = &doc.console.vcas;
            let id = vcas.get(v % vcas.len().max(1))?.id;
            Command::Song(atomik_model::SongCommand::AddAutomationLane {
                device: doc.console.master.clone()?,
                param: atomik_model::vca_lane_key_of(id, atomik_model::VcaLaneKind::ALL[kind % 3]),
            })
        }
        ConsoleOp::RouteVca(v, bus) => {
            let vcas = &doc.console.vcas;
            let vca = vcas.get(v % vcas.len().max(1))?.id;
            match bus {
                None => c(ConsoleCommand::RouteVcaToNewBus {
                    vca,
                    name: format!("Group bus {}", doc.console.buses.len() + 1),
                }),
                Some(b) => {
                    let buses = &doc.console.buses;
                    c(ConsoleCommand::RouteVcaToBus {
                        vca,
                        bus: buses.get(b % buses.len().max(1))?.bus.clone(),
                    })
                }
            }
        }
        ConsoleOp::UnrouteVca(v) => {
            let vcas = &doc.console.vcas;
            c(ConsoleCommand::UnrouteVca {
                vca: vcas.get(v % vcas.len().max(1))?.id,
            })
        }
        ConsoleOp::DeleteVca(v) => {
            let vcas = &doc.console.vcas;
            c(ConsoleCommand::DeleteVca {
                vca: vcas.get(v % vcas.len().max(1))?.id,
            })
        }
        ConsoleOp::Flag(ch, solo, on) => Command::SetParam {
            key: chan(*ch)?,
            param: if *solo { "solo" } else { "mute" }.into(),
            value: if *on { 1.0 } else { 0.0 },
            merge: None,
        },
    })
}

/// The console invariants (see `atomik_model::console`).
pub fn assert_console_invariants(doc: &RackDoc) {
    let console = &doc.console;
    let type_of = |k: &atomik_model::DeviceKey| {
        doc.device(k)
            .unwrap_or_else(|| panic!("console references missing device {k}"))
            .type_id
            .clone()
    };
    // A group that follows a folder holds only channels of that folder's tracks.
    for g in console.vcas.iter().filter(|g| g.follows.is_some()) {
        let folder = g.follows.unwrap();
        assert!(
            doc.song.org.group(folder).is_some(),
            "{} follows {folder}",
            g.name
        );
        let of_folder = doc.folder_strips(folder);
        for m in &g.members {
            assert!(
                of_folder.contains(m),
                "{} holds {m} outside folder {folder}",
                g.name
            );
        }
    }
    let mut seen = Vec::new();
    for ch in &console.channels {
        assert_eq!(type_of(&ch.channel), CHANNEL, "{}", ch.channel);
        assert!(!seen.contains(&ch.channel), "{} listed twice", ch.channel);
        seen.push(ch.channel.clone());
        if let Some(s) = &ch.source {
            assert!(![CHANNEL, MASTER].contains(&type_of(s).as_str()));
            assert_eq!(
                console
                    .channels
                    .iter()
                    .filter(|c| c.source.as_ref() == Some(s))
                    .count(),
                1,
                "{s} feeds one channel"
            );
            assert!(!console.returns.contains(&Some(s.clone())));
        }
    }
    let mut buses = Vec::new();
    let mut names = Vec::new();
    for b in &console.buses {
        assert_eq!(type_of(&b.bus), BUS, "{}", b.bus);
        assert!(!buses.contains(&b.bus), "{} listed twice", b.bus);
        buses.push(b.bus.clone());
        assert!(console.master.is_some(), "a bus needs the master");
        let name = b.name.to_lowercase();
        assert!(
            !b.name.trim().is_empty() && b.name.trim() == b.name,
            "{:?}",
            b.name
        );
        assert!(!names.contains(&name), "{:?} used twice", b.name);
        names.push(name);
    }
    let mut grouped = Vec::new();
    let mut vca_ids = Vec::new();
    let mut vca_names = Vec::new();
    for v in &console.vcas {
        assert!(v.id >= 1 && !vca_ids.contains(&v.id), "VCA id {}", v.id);
        vca_ids.push(v.id);
        assert!(
            !v.name.trim().is_empty() && v.name.trim() == v.name,
            "{:?}",
            v.name
        );
        let name = v.name.to_lowercase();
        assert!(!vca_names.contains(&name), "{:?} used twice", v.name);
        vca_names.push(name);
        assert!((-60.0..=12.0).contains(&v.level_db), "{}", v.level_db);
        match &v.bus {
            Some(b) => assert!(console.bus_index(b).is_some(), "group bus {b} is not a bus"),
            None => assert!(v.routed.is_empty(), "routed strips without a bus"),
        }
        for r in &v.routed {
            assert!(v.members.contains(r), "routed strip {r} is not a member");
        }
        for m in &v.members {
            assert!(
                console.channel_index(m).is_some() || console.bus_index(m).is_some(),
                "VCA member {m} is not a strip"
            );
            assert!(!grouped.contains(m), "{m} is in two VCA groups");
            grouped.push(m.clone());
        }
    }
    let mut routed = Vec::new();
    for r in &console.folder_routes {
        assert!(
            console.bus_index(&r.bus).is_some(),
            "folder route bus {} is not a bus",
            r.bus
        );
        assert!(
            !routed.contains(&r.folder),
            "folder {} routed twice",
            r.folder
        );
        routed.push(r.folder);
        assert!(
            !doc.song.group_tracks(r.folder).is_empty(),
            "stale folder route"
        );
        for m in &r.members {
            assert!(console.channel_index(m).is_some(), "route member {m}");
        }
    }
    let mut children = Vec::new();
    for (parent, child) in &console.parallels {
        assert!(
            console.channel_index(parent).is_some() || console.bus_index(parent).is_some(),
            "parallel parent {parent} is not a strip"
        );
        assert!(
            console.channel_index(child).is_some(),
            "parallel child {child} is not a channel"
        );
        assert!(
            parent != child && !children.contains(child),
            "parallel child {child}"
        );
        children.push(child.clone());
    }
    if let Some(m) = &console.master {
        assert_eq!(type_of(m), MASTER);
    } else {
        assert!(console.returns.iter().all(Option::is_none));
    }
    for (i, r) in console.returns.iter().enumerate() {
        if let Some(r) = r {
            assert!(![CHANNEL, MASTER].contains(&type_of(r).as_str()));
            assert!(!console.returns[i + 1..].contains(&Some(r.clone())));
        }
    }
    for d in &doc.devices {
        assert!(
            d.params.keys().all(|p| !p.starts_with("internal.")),
            "{} stores an internal param",
            d.key
        );
    }
}
