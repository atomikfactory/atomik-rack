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
    ClearSolos,
    ClearMutes,
    /// `solo` (true) or `mute` (false) on a channel.
    Flag(usize, bool, bool),
}

pub fn console_op() -> impl Strategy<Value = ConsoleOp> {
    let i = || 0usize..64;
    prop_oneof![
        3 => i().prop_map(ConsoleOp::AddChannel),
        1 => i().prop_map(ConsoleOp::RemoveChannel),
        1 => (i(), 0usize..6).prop_map(|(c, t)| ConsoleOp::MoveChannel(c, t)),
        2 => (0usize..FX_SLOTS + 1, proptest::option::of(i())).prop_map(|(s, e)| ConsoleOp::SetReturn(s, e)),
        1 => any::<bool>().prop_map(ConsoleOp::Compensation),
        1 => Just(ConsoleOp::ClearSolos),
        1 => Just(ConsoleOp::ClearMutes),
        3 => (i(), any::<bool>(), any::<bool>()).prop_map(|(c, s, on)| ConsoleOp::Flag(c, s, on)),
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
        ConsoleOp::ClearSolos => c(ConsoleCommand::ClearSolos),
        ConsoleOp::ClearMutes => c(ConsoleCommand::ClearMutes),
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
