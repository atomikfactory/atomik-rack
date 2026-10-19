//! Patch preparation for live play: `--rate` override, ignored notes, and the `--to` target.

use atomik_device_sdk::{DeviceDescriptor, Direction, Registry, SocketKind};
use atomik_render::patch::{NoteSpec, Patch};

/// The notes input that live MIDI notes go to, by patch device id and socket key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Patch device id.
    pub device: String,
    /// Notes input socket key.
    pub socket: String,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.device, self.socket)
    }
}

/// A `--to` value that does not name a notes input of the patch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// Not of the form `DEVICE_ID:SOCKET`.
    #[error("--to {0:?} must have the form DEVICE_ID:SOCKET")]
    BadSyntax(String),
    /// No device with that id.
    #[error("--to: no device has id {device:?} (devices: {known})")]
    UnknownDevice {
        /// Requested id.
        device: String,
        /// Ids in the patch.
        known: String,
    },
    /// The device has no socket with that key.
    #[error("--to: {device:?} has no socket {socket:?} (notes inputs: {notes})")]
    UnknownSocket {
        /// Device id.
        device: String,
        /// Requested socket.
        socket: String,
        /// The device's notes inputs.
        notes: String,
    },
    /// The socket exists but is not a notes input.
    #[error("--to: {device}:{socket} is not a notes input (notes inputs: {notes})")]
    NotNotesInput {
        /// Device id.
        device: String,
        /// Requested socket.
        socket: String,
        /// The device's notes inputs.
        notes: String,
    },
    /// MIDI was requested but no device of the patch has a notes input.
    #[error("no device in the patch has a notes input to receive MIDI notes")]
    NoNotesInput,
}

/// Resolves `--to` against `patch`. With `to = None` this is the first device (in patch order)
/// that has a notes input, and its first notes input; `Ok(None)` if there is none. Devices of
/// unknown type are skipped here (the build reports them with their JSON path).
pub fn resolve_target(
    patch: &Patch,
    registry: &Registry,
    to: Option<&str>,
) -> Result<Option<Target>, TargetError> {
    let descriptor = |type_id: &str| registry.get(type_id).map(|f| f.descriptor);
    let Some(text) = to else {
        return Ok(patch.devices.iter().find_map(|d| {
            let socket = notes_inputs(descriptor(&d.device_type)?).next()?;
            Some(Target {
                device: d.id.clone(),
                socket: socket.to_string(),
            })
        }));
    };
    let (device, socket) = text
        .split_once(':')
        .filter(|(d, s)| !d.is_empty() && !s.is_empty())
        .ok_or_else(|| TargetError::BadSyntax(text.to_string()))?;
    let spec = patch
        .devices
        .iter()
        .find(|d| d.id == device)
        .ok_or_else(|| TargetError::UnknownDevice {
            device: device.to_string(),
            known: patch
                .devices
                .iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })?;
    let target = Target {
        device: device.to_string(),
        socket: socket.to_string(),
    };
    // An unknown device type is reported by the build, with its JSON path.
    let Some(desc) = descriptor(&spec.device_type) else {
        return Ok(Some(target));
    };
    let notes = || {
        let keys: Vec<&str> = notes_inputs(desc).collect();
        if keys.is_empty() {
            "none".to_string()
        } else {
            keys.join(", ")
        }
    };
    let found = desc
        .socket_by_key(socket)
        .ok_or_else(|| TargetError::UnknownSocket {
            device: target.device.clone(),
            socket: target.socket.clone(),
            notes: notes(),
        })?;
    if found.kind != SocketKind::Notes || found.direction != Direction::Input {
        return Err(TargetError::NotNotesInput {
            device: target.device,
            socket: target.socket,
            notes: notes(),
        });
    }
    Ok(Some(target))
}

/// Keys of a device type's notes inputs, in socket order.
fn notes_inputs(desc: &DeviceDescriptor) -> impl Iterator<Item = &'static str> + '_ {
    desc.sockets
        .iter()
        .filter(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
        .map(|s| s.key)
}

/// What [`prepare`] changed in the patch, to report to the user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prepared {
    /// `Some((patch_rate, new_rate))` when `--rate` replaced the patch's sample rate.
    pub rate_override: Option<(u32, u32)>,
    /// Notes of the patch that are ignored in live mode.
    pub ignored_notes: usize,
}

impl Prepared {
    /// The `warning: ...` and `info: ...` lines to print on stderr.
    pub fn messages(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((patch_rate, rate)) = self.rate_override {
            out.push(format!(
                "warning: --rate {rate} Hz differs from the patch's {patch_rate} Hz; the engine runs at {rate} Hz"
            ));
        }
        if self.ignored_notes > 0 {
            out.push(format!(
                "info: the patch's {} note(s) are ignored in live mode",
                self.ignored_notes
            ));
        }
        out
    }
}

/// Applies `--rate`, drops the patch's notes (live mode) and, when `target` is given, adds a
/// single probe note on it so the build resolves its engine ids (the note itself is never
/// sent to the engine).
pub fn prepare(patch: &mut Patch, rate: Option<u32>, target: Option<&Target>) -> Prepared {
    let mut prepared = Prepared {
        rate_override: None,
        ignored_notes: patch.notes.len(),
    };
    if let Some(rate) = rate
        && rate != patch.sample_rate
    {
        prepared.rate_override = Some((patch.sample_rate, rate));
        patch.sample_rate = rate;
    }
    patch.notes.clear();
    if let Some(target) = target {
        patch.notes.push(NoteSpec {
            device: target.device.clone(),
            socket: target.socket.clone(),
            at_beats: 0.0,
            length_beats: PROBE_LENGTH_BEATS.min(patch.length_beats),
            key: 60,
            velocity: 1.0,
        });
    }
    prepared
}

/// Length of the probe note (never played).
const PROBE_LENGTH_BEATS: f64 = 0.25;

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_device_sdk::SocketId;
    use atomik_engine::DeviceInstanceId;
    use atomik_render::{build::build, default_registry};

    const MINIMAL: &str = include_str!("../../../examples/patches/minimal.json");
    const SLICE: &str = include_str!("../../../examples/patches/slice-demo.json");

    /// A patch whose first device (an effect) has no notes input.
    const DELAY_FIRST: &str = r#"{
      "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
      "length_beats": 4, "output_channels": 2,
      "devices": [
        { "id": "echo", "type": "atomik.stereo-delay" },
        { "id": "keys", "type": "atomik.poly-synth" }
      ],
      "cables": [
        { "from": "keys:out_l", "to": "echo:in_l" },
        { "from": "echo:out_l", "to": "interface:out_1" }
      ]
    }"#;

    const NO_SYNTH: &str = r#"{
      "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
      "length_beats": 4, "output_channels": 2,
      "devices": [ { "id": "echo", "type": "atomik.stereo-delay" } ]
    }"#;

    fn patch(text: &str) -> Patch {
        Patch::from_json(text).unwrap()
    }

    fn t(device: &str, socket: &str) -> Target {
        Target {
            device: device.into(),
            socket: socket.into(),
        }
    }

    #[test]
    fn default_target_is_the_first_device_with_a_notes_input() {
        let r = default_registry();
        assert_eq!(
            resolve_target(&patch(MINIMAL), &r, None),
            Ok(Some(t("synth", "notes_in")))
        );
        assert_eq!(
            resolve_target(&patch(DELAY_FIRST), &r, None),
            Ok(Some(t("keys", "notes_in")))
        );
        assert_eq!(resolve_target(&patch(NO_SYNTH), &r, None), Ok(None));
    }

    #[test]
    fn explicit_target_is_accepted() {
        let r = default_registry();
        let p = patch(SLICE);
        assert_eq!(
            resolve_target(&p, &r, Some("lead:notes_in")),
            Ok(Some(t("lead", "notes_in")))
        );
        assert_eq!(t("lead", "notes_in").to_string(), "lead:notes_in");
    }

    #[test]
    fn bad_targets_are_explained() {
        let r = default_registry();
        let p = patch(MINIMAL);
        for bad in ["synth", "synth:", ":notes_in", ""] {
            assert_eq!(
                resolve_target(&p, &r, Some(bad)),
                Err(TargetError::BadSyntax(bad.into()))
            );
        }
        let e = resolve_target(&p, &r, Some("piano:notes_in")).unwrap_err();
        assert!(matches!(e, TargetError::UnknownDevice { .. }), "{e}");
        assert!(e.to_string().contains("\"piano\"") && e.to_string().contains("synth"));
        let e = resolve_target(&p, &r, Some("interface:out_1")).unwrap_err();
        assert!(matches!(e, TargetError::UnknownDevice { .. }), "{e}");
        let e = resolve_target(&p, &r, Some("synth:notes")).unwrap_err();
        assert!(matches!(e, TargetError::UnknownSocket { .. }), "{e}");
        assert!(e.to_string().contains("notes_in"), "{e}");
    }

    #[test]
    fn non_notes_sockets_are_rejected() {
        let r = default_registry();
        let p = patch(MINIMAL);
        let e = resolve_target(&p, &r, Some("synth:out_l")).unwrap_err();
        assert_eq!(
            e,
            TargetError::NotNotesInput {
                device: "synth".into(),
                socket: "out_l".into(),
                notes: "notes_in".into()
            }
        );
        assert_eq!(
            e.to_string(),
            "--to: synth:out_l is not a notes input (notes inputs: notes_in)"
        );
        let e = resolve_target(&patch(NO_SYNTH), &r, Some("echo:in_l")).unwrap_err();
        assert!(e.to_string().contains("(notes inputs: none)"), "{e}");
    }

    #[test]
    fn prepare_drops_notes_and_reports_them() {
        let mut p = patch(MINIMAL);
        let prepared = prepare(&mut p, None, None);
        assert!(p.notes.is_empty());
        assert_eq!(prepared.ignored_notes, 1);
        assert_eq!(prepared.rate_override, None);
        assert_eq!(
            prepared.messages(),
            ["info: the patch's 1 note(s) are ignored in live mode"]
        );
        let mut p = patch(NO_SYNTH);
        assert!(prepare(&mut p, Some(48_000), None).messages().is_empty());
    }

    #[test]
    fn rate_override_rebuilds_the_engine_config_and_warns() {
        let mut p = patch(MINIMAL);
        let prepared = prepare(&mut p, Some(44_100), None);
        assert_eq!(prepared.rate_override, Some((48_000, 44_100)));
        assert_eq!(p.sample_rate, 44_100);
        let msgs = prepared.messages();
        assert!(
            msgs.contains(
                &"warning: --rate 44100 Hz differs from the patch's 48000 Hz; \
                  the engine runs at 44100 Hz"
                    .to_string()
            ),
            "{msgs:?}"
        );
        let built = build(&p, &default_registry()).unwrap();
        assert!((built.config.sample_rate.hz() - 44_100.0).abs() < 1e-9);
    }

    #[test]
    fn probe_note_resolves_engine_ids_through_the_build() {
        let r = default_registry();
        let mut p = patch(SLICE);
        let target = resolve_target(&p, &r, Some("lead:notes_in"))
            .unwrap()
            .unwrap();
        prepare(&mut p, None, Some(&target));
        let built = build(&p, &r).unwrap();
        let lead = p.devices.iter().position(|d| d.id == "lead").unwrap();
        let first = built.notes.first().expect("the probe note");
        assert_eq!(
            first.device,
            DeviceInstanceId(u32::try_from(lead).unwrap() + 1)
        );
        assert_eq!(first.socket, SocketId(0));
        assert!(built.notes.iter().all(|n| n.device == first.device));
    }
}
