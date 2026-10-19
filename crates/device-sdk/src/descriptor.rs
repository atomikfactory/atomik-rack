//! Static device descriptions: kinds, sockets and parameters.

use atomik_core::ParamDescriptor;

/// Broad category of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Produces sound from notes.
    Instrument,
    /// Processes audio.
    Effect,
    /// Mixes several channels.
    Mixer,
    /// CV/notes utilities.
    Utility,
    /// The engine's host audio interface.
    Interface,
    /// A device whose inside is another rack. The model flattens it before the
    /// engine sees it; the engine never instantiates one.
    Container,
}

/// What a socket carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    /// Mono audio, nominal ±1.0.
    Audio,
    /// Control voltage, nominal −1..+1.
    Cv,
    /// Note events.
    Notes,
}

/// Socket direction, seen from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Receives a signal.
    Input,
    /// Produces a signal.
    Output,
    /// Receives the **sum** of any number of cables: a console bus input.
    /// Contributions are summed in deterministic order by source endpoint (device id, socket).
    SummingInput,
}

impl Direction {
    /// True for [`Direction::Input`] and [`Direction::SummingInput`].
    pub fn is_input(self) -> bool {
        matches!(self, Direction::Input | Direction::SummingInput)
    }
}

/// Identifies a socket within one device. `SocketId(i)` is index `i` in the socket table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SocketId(pub u16);

/// Static description of one socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketDescriptor {
    /// Id == index in the socket table.
    pub id: SocketId,
    /// Stable machine key, e.g. `"out_l"`. Used in patch files.
    pub key: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Signal kind.
    pub kind: SocketKind,
    /// Direction.
    pub direction: Direction,
}

/// Static description of a device type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceDescriptor {
    /// Globally unique type id, e.g. `"atomik.poly-synth"`.
    pub type_id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Category.
    pub kind: DeviceKind,
    /// Version of this device's state/params layout.
    pub version: u32,
    /// Parameters; `ParamId(i)` is index `i`.
    pub params: &'static [ParamDescriptor],
    /// Sockets; `SocketId(i)` is index `i`.
    pub sockets: &'static [SocketDescriptor],
    /// Processing stage of each socket, in socket order. Empty = every socket is
    /// stage 0 (single-stage device). Stages run in order within one batch; loops are checked
    /// per (device, stage).
    pub stages: &'static [u8],
}

/// A descriptor that breaks the descriptor invariants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorError {
    /// A param id differs from its index.
    #[error("param `{key}` has id {id} but index {index}")]
    ParamIdMismatch {
        /// Param key.
        key: &'static str,
        /// Declared id.
        id: u32,
        /// Actual index.
        index: usize,
    },
    /// A socket id differs from its index.
    #[error("socket `{key}` has id {id} but index {index}")]
    SocketIdMismatch {
        /// Socket key.
        key: &'static str,
        /// Declared id.
        id: u16,
        /// Actual index.
        index: usize,
    },
    /// Two params or two sockets share a key.
    #[error("duplicate key `{0}`")]
    DuplicateKey(&'static str),
    /// `stages` is neither empty nor one entry per socket.
    #[error("stage table has {stages} entries for {sockets} sockets")]
    StageMismatch {
        /// Stage table length.
        stages: usize,
        /// Socket count.
        sockets: usize,
    },
    /// A param's default, min and max are inconsistent.
    #[error("param `{0}` has an invalid range or default")]
    InvalidRange(&'static str),
    /// A plugin id is empty, too long, contains whitespace, or (VST3) is not 32 lowercase hex
    /// digits.
    #[error("`{0}` is not a valid plugin id")]
    InvalidPluginId(String),
    /// A plugin layout has more params than the cap.
    #[error("plugin has {count} params, the limit is {max}")]
    TooManyParams {
        /// Params in the layout.
        count: usize,
        /// The cap.
        max: usize,
    },
    /// Two params of a plugin layout share an id.
    #[error("plugin param id {0} appears twice")]
    DuplicatePluginParam(u32),
    /// A plugin param has a non-finite or inverted range, or a stepped VST3 param has no steps.
    #[error("plugin param {0} has an invalid range")]
    InvalidPluginParam(u32),
    /// A plugin layout has neither audio nor notes, so the device would have no socket.
    #[error("plugin has no audio or note port")]
    NoSockets,
    /// The process-wide table of interned plugin descriptors is full.
    #[error("more than {max} distinct plugin layouts are interned")]
    InternLimit {
        /// The cap.
        max: usize,
    },
}

impl DeviceDescriptor {
    /// Stage of `socket` (0 for single-stage devices or out-of-range ids).
    pub fn stage_of(&self, socket: SocketId) -> u8 {
        self.stages.get(socket.0 as usize).copied().unwrap_or(0)
    }

    /// Number of processing stages (at least 1).
    pub fn stage_count(&self) -> u8 {
        self.stages.iter().copied().max().map_or(1, |m| m + 1)
    }

    /// Finds a parameter by key.
    pub fn param_by_key(&self, key: &str) -> Option<&ParamDescriptor> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Finds a socket by key.
    pub fn socket_by_key(&self, key: &str) -> Option<&SocketDescriptor> {
        self.sockets.iter().find(|s| s.key == key)
    }

    /// Checks: ids equal indices, keys unique (params and sockets separately),
    /// `min < max` (or equal for constant params), `default` within range,
    /// and `min > 0` for exponential tapers.
    pub fn validate(&self) -> Result<(), DescriptorError> {
        if !self.stages.is_empty() && self.stages.len() != self.sockets.len() {
            return Err(DescriptorError::StageMismatch {
                stages: self.stages.len(),
                sockets: self.sockets.len(),
            });
        }
        for (index, p) in self.params.iter().enumerate() {
            if p.id.0 as usize != index {
                return Err(DescriptorError::ParamIdMismatch {
                    key: p.key,
                    id: p.id.0,
                    index,
                });
            }
            if self.params[..index].iter().any(|q| q.key == p.key) {
                return Err(DescriptorError::DuplicateKey(p.key));
            }
            let exp_ok = !matches!(p.taper, atomik_core::Taper::Exponential) || p.min > 0.0;
            let range_ok = p.min.is_finite() && p.max.is_finite() && p.min <= p.max;
            if !(exp_ok && range_ok && p.default >= p.min && p.default <= p.max) {
                return Err(DescriptorError::InvalidRange(p.key));
            }
        }
        for (index, s) in self.sockets.iter().enumerate() {
            if s.id.0 as usize != index {
                return Err(DescriptorError::SocketIdMismatch {
                    key: s.key,
                    id: s.id.0,
                    index,
                });
            }
            if self.sockets[..index].iter().any(|t| t.key == s.key) {
                return Err(DescriptorError::DuplicateKey(s.key));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use atomik_core::{ParamId, Taper, Unit};

    pub(crate) const GAIN: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "gain",
        name: "Gain",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    };
    pub(crate) const SOCKETS: &[SocketDescriptor] = &[
        SocketDescriptor {
            id: SocketId(0),
            key: "in",
            name: "In",
            kind: SocketKind::Audio,
            direction: Direction::Input,
        },
        SocketDescriptor {
            id: SocketId(1),
            key: "out",
            name: "Out",
            kind: SocketKind::Audio,
            direction: Direction::Output,
        },
        SocketDescriptor {
            id: SocketId(2),
            key: "notes",
            name: "Notes",
            kind: SocketKind::Notes,
            direction: Direction::Input,
        },
    ];
    pub(crate) const GOOD: DeviceDescriptor = DeviceDescriptor {
        stages: &[],
        type_id: "test.good",
        name: "Good",
        kind: DeviceKind::Effect,
        version: 1,
        params: &[GAIN],
        sockets: SOCKETS,
    };

    #[test]
    fn descriptor_validate_accepts_good_descriptor() {
        assert_eq!(GOOD.validate(), Ok(()));
    }

    #[test]
    fn descriptor_lookup_by_key() {
        assert_eq!(GOOD.param_by_key("gain").map(|p| p.id), Some(ParamId(0)));
        assert_eq!(GOOD.socket_by_key("out").map(|s| s.id), Some(SocketId(1)));
        assert!(GOOD.param_by_key("nope").is_none());
        assert!(GOOD.socket_by_key("nope").is_none());
    }

    #[test]
    fn descriptor_validate_rejects_mismatched_ids() {
        const BAD_P: &[ParamDescriptor] = &[ParamDescriptor {
            id: ParamId(3),
            ..GAIN
        }];
        let d = DeviceDescriptor {
            params: BAD_P,
            ..GOOD
        };
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::ParamIdMismatch { .. })
        ));

        const BAD_S: &[SocketDescriptor] = &[SocketDescriptor {
            id: SocketId(9),
            ..SOCKETS[0]
        }];
        let d = DeviceDescriptor {
            sockets: BAD_S,
            ..GOOD
        };
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::SocketIdMismatch { .. })
        ));
    }

    #[test]
    fn descriptor_validate_rejects_duplicate_keys() {
        const DUP: &[SocketDescriptor] = &[
            SOCKETS[0],
            SocketDescriptor {
                id: SocketId(1),
                ..SOCKETS[0]
            },
        ];
        let d = DeviceDescriptor {
            sockets: DUP,
            ..GOOD
        };
        assert_eq!(d.validate(), Err(DescriptorError::DuplicateKey("in")));
    }

    #[test]
    fn descriptor_validate_rejects_default_out_of_range() {
        const BAD: &[ParamDescriptor] = &[ParamDescriptor {
            default: 2.0,
            ..GAIN
        }];
        let d = DeviceDescriptor {
            params: BAD,
            ..GOOD
        };
        assert_eq!(d.validate(), Err(DescriptorError::InvalidRange("gain")));

        const EXP0: &[ParamDescriptor] = &[ParamDescriptor {
            taper: Taper::Exponential,
            min: 0.0,
            ..GAIN
        }];
        let d = DeviceDescriptor {
            params: EXP0,
            ..GOOD
        };
        assert_eq!(d.validate(), Err(DescriptorError::InvalidRange("gain")));
    }
}
