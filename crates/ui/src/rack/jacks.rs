//! Patching beyond the plain drag between two jacks: the stereo partner of a cable, the peer a
//! tooltip names, the pop-up menu of a jack and scrolling the rack to a device.
//!
//! A cable between two `_l` jacks (or two `_r` jacks) of audio sockets also cables the `_r` (or
//! `_l`) pair when both of those exist and are free; Alt at the release makes it one cable.
//! The jack menu lists, per device in rack order, the jacks the cable rules allow ([`JackMenu`]);
//! a choice is a [`JackOp`], which the app turns into a command (one undo step) or a scroll.

use super::{Drag, RackView, UiAction};
use crate::cables::{self, SocketEnd};
use crate::event::Modifiers;
use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_model::console::is_console_type;
use atomik_model::{Cable, Command, ConsoleCommand, DeviceKey, RackDoc, SocketRef};
use vello::kurbo::Point;

/// Separates the fields of an encoded [`JackOp`] (a device key never holds it).
const SEP: char = '|';

/// What a choice of the jack menu does. It travels as the argument of the `rack.jack` command
/// ([`Self::encode`]) and is read back by [`Self::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JackOp {
    /// Add this cable.
    Connect(Cable),
    /// Replace the cable on the input of this one with it (one undo step).
    Replace(Cable),
    /// Remove every cable on this jack.
    Disconnect(SocketRef),
    /// Scroll the rack to this device and select it.
    Reveal(DeviceKey),
    /// Add a console channel fed by these two outputs of the device (the same one twice for a
    /// mono output).
    NewChannel {
        /// The device feeding the channel.
        device: DeviceKey,
        /// The output of the channel's left input.
        out_l: String,
        /// The output of the channel's right input.
        out_r: String,
    },
}

impl JackOp {
    /// The text form used as the command argument.
    pub fn encode(&self) -> String {
        let join = |parts: &[&str]| parts.join(&SEP.to_string());
        match self {
            JackOp::Connect(c) => join(&[
                "connect",
                &c.from.device.0,
                &c.from.socket,
                &c.to.device.0,
                &c.to.socket,
            ]),
            JackOp::Replace(c) => join(&[
                "replace",
                &c.from.device.0,
                &c.from.socket,
                &c.to.device.0,
                &c.to.socket,
            ]),
            JackOp::Disconnect(s) => join(&["disconnect", &s.device.0, &s.socket]),
            JackOp::Reveal(d) => join(&["reveal", &d.0]),
            JackOp::NewChannel {
                device,
                out_l,
                out_r,
            } => join(&["channel", &device.0, out_l, out_r]),
        }
    }

    /// Reads the text form back (`None` when it is not one).
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split(SEP).collect();
        let sock = |d: &str, s: &str| SocketRef {
            device: DeviceKey(d.to_owned()),
            socket: s.to_owned(),
        };
        match parts.as_slice() {
            ["connect", fd, fs, td, ts] => Some(JackOp::Connect(Cable {
                from: sock(fd, fs),
                to: sock(td, ts),
            })),
            ["replace", fd, fs, td, ts] => Some(JackOp::Replace(Cable {
                from: sock(fd, fs),
                to: sock(td, ts),
            })),
            ["disconnect", d, s] => Some(JackOp::Disconnect(sock(d, s))),
            ["reveal", d] => Some(JackOp::Reveal(DeviceKey((*d).to_owned()))),
            ["channel", d, l, r] => Some(JackOp::NewChannel {
                device: DeviceKey((*d).to_owned()),
                out_l: (*l).to_owned(),
                out_r: (*r).to_owned(),
            }),
            _ => None,
        }
    }

    /// The command that does it on `doc`, one undo step (`None` for a scroll, or when there is
    /// nothing to do).
    pub fn command(&self, doc: &RackDoc) -> Option<Command> {
        match self {
            JackOp::Connect(c) => Some(Command::Connect(c.clone())),
            JackOp::Replace(c) => Some(Command::Batch(vec![
                Command::Disconnect { to: c.to.clone() },
                Command::Connect(c.clone()),
            ])),
            JackOp::Disconnect(s) => {
                let mut cuts: Vec<Command> = doc
                    .cables
                    .iter()
                    .filter(|c| &c.to == s || &c.from == s)
                    .map(|c| Command::Disconnect { to: c.to.clone() })
                    .collect();
                match cuts.len() {
                    0 => None,
                    1 => cuts.pop(),
                    _ => Some(Command::Batch(cuts)),
                }
            }
            JackOp::Reveal(_) => None,
            JackOp::NewChannel {
                device,
                out_l,
                out_r,
            } => Some(Command::Console(ConsoleCommand::AddChannelFor {
                source: device.clone(),
                out_l: out_l.clone(),
                out_r: out_r.clone(),
                name: None,
            })),
        }
    }
}

/// One jack of the menu: a place the cable can go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackChoice {
    /// The jack's name.
    pub label: String,
    /// A plain input that already has a cable: choosing it replaces that cable.
    pub occupied: bool,
    /// What choosing it does.
    pub op: JackOp,
}

/// A device of the menu and its jacks the cable can go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackGroup {
    /// The device's name.
    pub device: String,
    /// Its compatible jacks, in panel order.
    pub jacks: Vec<JackChoice>,
}

/// A device on the other end of a cable of the jack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackPeer {
    /// The device (path key inside a container).
    pub device: DeviceKey,
    /// Its name.
    pub name: String,
}

/// What the pop-up menu of a jack lists (see [`RackView::jack_menu`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackMenu {
    /// The jack (path key inside a container).
    pub jack: SocketRef,
    /// The devices the jack can be cabled to, in rack order.
    pub groups: Vec<JackGroup>,
    /// The devices cabled to the jack.
    pub peers: Vec<JackPeer>,
    /// The jack is an audio output (the menu offers a new mixer channel for it).
    pub audio_output: bool,
    /// Add a console channel fed by this jack (an audio output nothing is cabled to).
    pub channel: Option<JackOp>,
}

/// The side letter of a stereo socket key and the key stem: `out_l` is `("out", 'l')`.
fn stereo_key(key: &str) -> Option<(&str, char)> {
    let stem = key.strip_suffix("_l").map(|s| (s, 'l'));
    stem.or_else(|| key.strip_suffix("_r").map(|s| (s, 'r')))
}

/// The key of the other side of a stereo socket key.
fn partner_key(key: &str) -> Option<String> {
    let (stem, side) = stereo_key(key)?;
    Some(format!("{stem}_{}", if side == 'l' { 'r' } else { 'l' }))
}

impl RackView {
    /// The cables the view works on (local keys): the document's at the top level, the scope's
    /// inside a container.
    fn view_cables(&self) -> &[Cable] {
        &self.cables
    }

    /// The second cable of a stereo pair: when `cable` joins an `_l` output to an `_l` input
    /// (or `_r` to `_r`) of audio sockets, the cable of the other side, provided both of its
    /// sockets exist with the right kind and direction and the input is free.
    pub(super) fn stereo_partner(&self, cable: &Cable) -> Option<Cable> {
        if stereo_key(&cable.from.socket)?.1 != stereo_key(&cable.to.socket)?.1 {
            return None;
        }
        let from = SocketRef {
            device: cable.from.device.clone(),
            socket: partner_key(&cable.from.socket)?,
        };
        let to = SocketRef {
            device: cable.to.device.clone(),
            socket: partner_key(&cable.to.socket)?,
        };
        let out = self.sockets.get(&from)?;
        let input = self.sockets.get(&to)?;
        if out.end.kind != SocketKind::Audio
            || input.end.kind != SocketKind::Audio
            || out.end.direction != Direction::Output
            || !input.end.direction.is_input()
        {
            return None;
        }
        let partner = Cable { from, to };
        let taken = match input.end.direction {
            Direction::SummingInput => self.view_cables().contains(&partner),
            _ => self.view_cables().iter().any(|c| c.to == partner.to),
        };
        (!taken).then_some(partner)
    }

    /// The name of the device the view calls `key`.
    fn device_name(&self, key: &DeviceKey) -> Option<&str> {
        self.devices
            .iter()
            .find(|d| d.socket_key() == key)
            .map(|d| d.name.as_str())
    }

    /// The name of socket `at` on its device.
    fn socket_name(&self, at: &SocketRef) -> Option<&'static str> {
        let d = self.devices.iter().find(|d| d.socket_key() == &at.device)?;
        Some(d.desc?.socket_by_key(&at.socket)?.name)
    }

    /// The tooltip of the jack (`i`, `slot`): its name, and the jacks it is cabled to
    /// ("In L from Synth > Out L").
    pub(super) fn jack_tooltip(&self, i: usize, slot: usize) -> Option<String> {
        let d = &self.devices[i];
        let s = &d.desc?.sockets[d.spec.back[slot].socket];
        let at = SocketRef {
            device: d.socket_key().clone(),
            socket: s.key.to_string(),
        };
        let input = s.direction.is_input();
        let peers: Vec<String> = self
            .view_cables()
            .iter()
            .filter(|c| if input { c.to == at } else { c.from == at })
            .filter_map(|c| {
                let other = if input { &c.from } else { &c.to };
                Some(format!(
                    "{} > {}",
                    self.device_name(&other.device)?,
                    self.socket_name(other)?
                ))
            })
            .collect();
        if peers.is_empty() {
            return Some(s.name.to_string());
        }
        let shown = peers.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let more = peers.len().saturating_sub(3);
        let word = if input { "from" } else { "to" };
        Some(if more > 0 {
            format!("{} {word} {shown} and {more} more", s.name)
        } else {
            format!("{} {word} {shown}", s.name)
        })
    }

    /// A right click on a jack: the shell opens the jack menu.
    pub(super) fn jack_menu_down(
        &mut self,
        i: usize,
        slot: usize,
        pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        let Some(g) = self.socket_geo(i, slot) else {
            return;
        };
        out.push(UiAction::JackMenu {
            socket: g.end.at.clone(),
            at: pos,
        });
    }

    /// The menu of jack `jack` (a path key inside a container): the devices it can be cabled
    /// to in rack order with the jacks the cable rules allow, an occupied plain input marked
    /// (choosing it replaces its cable), the devices already cabled to it, and whether a new
    /// mixer channel can be fed by it. At the top level the rules are those of `Command::Connect`
    /// judged on `doc` without changing it; inside a container the kind, direction and device
    /// rules of the drag apply and the model has the last word.
    pub fn jack_menu(
        &self,
        jack: &SocketRef,
        doc: &RackDoc,
        registry: &Registry,
    ) -> Option<JackMenu> {
        let local = SocketRef {
            device: self.local_key(&jack.device),
            socket: jack.socket.clone(),
        };
        let end = self.sockets.get(&local)?.end.clone();
        // The rules of `Command::Connect` on the project are judged for the rack's own jacks;
        // inside a container (open, or unfolded in the rack) the model has the last word.
        let top = self.scope.is_empty() && !super::inline::is_inner(&local.device);
        let cables = self.view_cables();
        let occupant = |input: &SocketRef| cables.iter().find(|c| &c.to == input);
        let plain_input = |e: &SocketEnd| e.direction == Direction::Input;

        let mut groups = Vec::new();
        for d in &self.devices {
            if d.socket_device.is_some() || d.socket_key() == &local.device {
                continue;
            }
            let Some(desc) = d.desc else { continue };
            let mut jacks = Vec::new();
            for ps in &d.spec.back {
                let s = &desc.sockets[ps.socket];
                let at = SocketRef {
                    device: d.socket_key().clone(),
                    socket: s.key.to_string(),
                };
                let other = SocketEnd {
                    at,
                    kind: s.kind,
                    direction: s.direction,
                };
                if !self.can_cable(&end, &other) {
                    continue;
                }
                let cable = cables::orient(&end, &other);
                if cables.contains(&cable) {
                    continue;
                }
                // The input of the cable: a plain one gives up its cable.
                let input = if end.direction.is_input() {
                    &end
                } else {
                    &other
                };
                let replaced = plain_input(input).then(|| occupant(&input.at)).flatten();
                if top {
                    let lifted = Cable {
                        from: self.up_socket(&cable.from),
                        to: self.up_socket(&cable.to),
                    };
                    let gone = replaced.map(|c| Cable {
                        from: self.up_socket(&c.from),
                        to: self.up_socket(&c.to),
                    });
                    if !doc.cable_fits(registry, &lifted, gone.as_ref()) {
                        continue;
                    }
                }
                let lifted = Cable {
                    from: self.up_socket(&cable.from),
                    to: self.up_socket(&cable.to),
                };
                let occupied = replaced.is_some();
                jacks.push(JackChoice {
                    label: s.name.to_string(),
                    occupied,
                    op: if occupied {
                        JackOp::Replace(lifted)
                    } else {
                        JackOp::Connect(lifted)
                    },
                });
            }
            if !jacks.is_empty() {
                groups.push(JackGroup {
                    device: d.name.clone(),
                    jacks,
                });
            }
        }

        let mut peers: Vec<JackPeer> = Vec::new();
        for c in cables {
            let input = end.direction.is_input();
            let (mine, other) = if input {
                (&c.to, &c.from)
            } else {
                (&c.from, &c.to)
            };
            if mine != &local
                || peers
                    .iter()
                    .any(|p| self.local_key(&p.device) == other.device)
            {
                continue;
            }
            if let Some(name) = self.device_name(&other.device) {
                peers.push(JackPeer {
                    device: self.abs_key(&other.device),
                    name: name.to_owned(),
                });
            }
        }

        Some(JackMenu {
            jack: jack.clone(),
            groups,
            peers,
            audio_output: end.kind == SocketKind::Audio && end.direction == Direction::Output,
            channel: self.channel_op(&end, jack, doc),
        })
    }

    /// The op that adds a console channel fed by output `end` of the top level, when that is
    /// possible: an audio output of a device that is not a console device, with no cable, and
    /// the other side of its stereo pair (when it has one) free too.
    fn channel_op(&self, end: &SocketEnd, jack: &SocketRef, doc: &RackDoc) -> Option<JackOp> {
        if !self.scope.is_empty()
            || end.kind != SocketKind::Audio
            || end.direction != Direction::Output
        {
            return None;
        }
        let device = doc.device(&jack.device)?;
        if is_console_type(&device.type_id) || doc.console.return_slot(&jack.device).is_some() {
            return None;
        }
        let local = &end.at;
        let free = |key: &str| {
            !self
                .view_cables()
                .iter()
                .any(|c| c.from.device == local.device && c.from.socket == key)
        };
        let partner = partner_key(&jack.socket).filter(|p| {
            self.sockets
                .get(&SocketRef {
                    device: local.device.clone(),
                    socket: p.clone(),
                })
                .is_some_and(|g| {
                    g.end.kind == SocketKind::Audio && g.end.direction == Direction::Output
                })
        });
        let (out_l, out_r) = match (stereo_key(&jack.socket), partner) {
            (Some((_, 'l')), Some(p)) => (jack.socket.clone(), p),
            (Some((_, 'r')), Some(p)) => (p, jack.socket.clone()),
            _ => (jack.socket.clone(), jack.socket.clone()),
        };
        (free(&out_l) && free(&out_r)).then_some(JackOp::NewChannel {
            device: jack.device.clone(),
            out_l,
            out_r,
        })
    }

    /// Scrolls the rack, columns included, to show device `key` (a path key inside a container). Returns whether the view has it.
    pub fn reveal_key(&mut self, key: &DeviceKey) -> bool {
        let Some(i) = self.device_index(key) else {
            return false;
        };
        self.reveal_device(i);
        true
    }
}

impl RackView {
    /// Whether `cable` is the one a cable drag has picked up (it is drawn as the dragged one
    /// and ends only at the release).
    pub(super) fn is_detached(&self, cable: &Cable) -> bool {
        matches!(&self.drag, Drag::Cable { detached: Some(d), .. } if d == cable)
    }

    /// The release of a cable drag from `anchor`: `target` is the compatible jack under the
    /// pointer, `detached` the cable the drag picked up from a connected input. One undo step
    /// whatever it takes: a cable picked up and dropped on nothing is cut, one dropped on its
    /// own input stays, a plain input that is taken gives up its cable, and a stereo pair
    /// (see [`Self::stereo_partner`]) is cabled whole unless Alt is held.
    pub(super) fn cable_up(
        &self,
        anchor: &SocketEnd,
        target: Option<SocketEnd>,
        detached: Option<Cable>,
        modifiers: Modifiers,
        doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) {
        let cable = target.as_ref().map(|t| cables::orient(anchor, t));
        if cable.is_some() && cable == detached {
            return;
        }
        let mut cmds: Vec<Command> = Vec::new();
        if let Some(d) = &detached {
            cmds.push(Command::Disconnect { to: d.to.clone() });
        }
        if let (Some(cable), Some(t)) = (&cable, &target) {
            // A summing input takes any number of cables: only an identical cable counts as
            // existing; a plain input's cable is replaced.
            let summing = [anchor, t]
                .iter()
                .any(|e| e.direction == Direction::SummingInput);
            let existing = doc.cables.iter().find(|c| {
                c.to == cable.to && Some(*c) != detached.as_ref() && (!summing || *c == cable)
            });
            match existing {
                Some(e) if e == cable => {}
                Some(_) => {
                    cmds.push(Command::Disconnect {
                        to: cable.to.clone(),
                    });
                    cmds.push(Command::Connect(cable.clone()));
                }
                None => cmds.push(Command::Connect(cable.clone())),
            }
            if detached.is_none()
                && !modifiers.alt
                && !matches!(existing, Some(e) if e == cable)
                && let Some(partner) = self.stereo_partner(cable)
            {
                cmds.push(Command::Connect(partner));
            }
        }
        match cmds.len() {
            0 => {}
            1 => out.extend(cmds.pop().map(UiAction::Command)),
            _ => out.push(UiAction::Command(Command::Batch(cmds))),
        }
    }
}
