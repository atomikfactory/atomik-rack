//! Resolves a validated [`Patch`] against a device [`Registry`] into a compiled [`RenderPlan`].

use crate::patch::{INTERFACE_ID, clamp_param};
use crate::patch::{JsonPath, Patch, PatchError, Warning};
use atomik_core::{NoteKind, SampleRate};
use atomik_device_sdk::{Device, Direction, Registry, SocketDescriptor, SocketKind};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, Graph, GraphError, RenderPlan, ScheduledNote,
};
use std::collections::BTreeMap;

/// Ready-made devices by patch device id (see [`build_with_devices`]).
pub type SuppliedDevices = BTreeMap<String, Box<dyn Device>>;

/// Everything needed to render a patch offline.
pub struct Built {
    /// The compiled plan (devices prepared, params applied).
    pub plan: RenderPlan,
    /// Engine settings derived from the patch (host inputs only from [`BuildOptions`]).
    pub config: EngineConfig,
    /// Note events sorted by frame; at equal frames an Off comes before an On.
    pub notes: Vec<ScheduledNote>,
    /// Frames to render (`length_beats` + 2 beats of tail).
    pub frames: usize,
    /// Non-fatal problems, such as clamped param values.
    pub warnings: Vec<Warning>,
    /// The routing of `plan` (devices and cables, every latency 0): a bounce with plugins
    /// recompiles it when a plugin's latency changes.
    pub topology: atomik_engine::Topology,
}

impl std::fmt::Debug for Built {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Built")
            .field("order", &self.plan.order())
            .field("config", &self.config)
            .field("notes", &self.notes.len())
            .field("frames", &self.frames)
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// What went wrong while building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildErrorKind {
    /// The patch failed [`Patch::validate`].
    InvalidPatch,
    /// No registered device has this type id.
    UnknownDeviceType,
    /// The device type has no param with this key.
    UnknownParam,
    /// No device in the patch has this id.
    UnknownDevice,
    /// The device has no socket with this key, or the socket is the wrong one for the use.
    UnknownSocket,
    /// An endpoint is not of the form `device:socket`.
    BadEndpoint,
    /// The engine rejected a cable or the compile.
    Graph(GraphError),
}

/// A patch that cannot be built, with the JSON path of the cause.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct BuildError {
    /// JSON path of the offending value.
    pub path: String,
    /// Category, for matching.
    pub kind: BuildErrorKind,
    /// Human-readable explanation.
    pub message: String,
}

impl From<PatchError> for BuildError {
    fn from(e: PatchError) -> Self {
        Self {
            path: e.path,
            kind: BuildErrorKind::InvalidPatch,
            message: e.message,
        }
    }
}

/// Extra build settings that are not part of a patch file: they come from a
/// project's console section.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOptions {
    /// Host input channels of the interface (`interface:in_1..in_N`, default 0). A render
    /// feeds them with silence; cables from them must still resolve.
    pub input_channels: u16,
    /// Delay compensation for the compile ([`Graph::set_compensation`]; on by default).
    pub compensation: bool,
    /// Params set after the patch's own, as `(device id, param key, plain value)` (clamped by
    /// the engine, no warnings): e.g. the app-only `internal.solo_silence`.
    pub params: Vec<(String, String, f32)>,
    /// Sample maps installed before the compile, as `(device id, map)`: the samplers of a
    /// project.
    pub samples: Vec<(String, atomik_device_sdk::SampleMapRef)>,
    /// Pattern data installed before the compile, as `(device id, data)`: the step sequencers
    /// of a project.
    pub player_data: Vec<(String, atomik_device_sdk::PlayerDataRef)>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            input_channels: 0,
            compensation: true,
            params: Vec::new(),
            samples: Vec::new(),
            player_data: Vec::new(),
        }
    }
}

/// Validates `patch`, instantiates its devices from `registry`, applies params (clamped, with
/// warnings), connects cables, schedules notes and compiles the graph.
pub fn build(patch: &Patch, registry: &Registry) -> Result<Built, BuildError> {
    build_with(patch, registry, &BuildOptions::default())
}

/// [`build`] with [`BuildOptions`]. Summing inputs (the master's buses) take any
/// number of cables; device latencies (`Device::latency_frames()`, read after `prepare` and
/// every param) are compensated at the compile unless `options.compensation` is off.
///
/// Errors in `options.params` use the JSON path `options.params[i]`.
pub fn build_with(
    patch: &Patch,
    registry: &Registry,
    options: &BuildOptions,
) -> Result<Built, BuildError> {
    build_with_devices(patch, registry, options, BTreeMap::new())
}

/// [`build_with`] where some devices are supplied ready-made, keyed by patch device id: the
/// plugin devices of a project, which only a plugin provider can build. A
/// device in `devices` replaces the one the registry would create for its id (the patch's
/// params are applied to it as to any other); a key that names no patch device is ignored.
pub fn build_with_devices(
    patch: &Patch,
    registry: &Registry,
    options: &BuildOptions,
    devices: SuppliedDevices,
) -> Result<Built, BuildError> {
    patch.validate()?;
    let root = JsonPath::root();
    let sample_rate = SampleRate::new(f64::from(patch.sample_rate))
        .map_err(|e| err(&root.field("sample_rate"), BuildErrorKind::InvalidPatch, e))?;
    let config = EngineConfig {
        sample_rate,
        tempo: patch.tempo()?,
        input_channels: options.input_channels,
        output_channels: patch.output_channels,
        seed: patch.seed,
    };
    let mut b = Builder {
        graph: Graph::new(&config),
        ids: BTreeMap::new(),
        warnings: Vec::new(),
        supplied: devices,
    };
    b.ids.insert(INTERFACE_ID, Graph::INTERFACE);
    b.add_devices(patch, registry, &root.field("devices"))?;
    b.connect_cables(patch, &root.field("cables"))?;
    b.apply_options(options, &JsonPath::root().field("options"))?;
    let frames = patch.total_frames();
    let notes = b.schedule_notes(patch, &config, frames, &root.field("notes"))?;
    let Builder {
        mut graph,
        warnings,
        ..
    } = b;
    graph.set_compensation(options.compensation);
    let topology = graph.topology();
    let plan = graph
        .compile()
        .map_err(|e| err(&root, BuildErrorKind::Graph(e), e))?;
    Ok(Built {
        plan,
        config,
        notes,
        frames,
        warnings,
        topology,
    })
}

struct Builder<'p> {
    graph: Graph,
    /// Patch device id → instance id (including `interface`).
    ids: BTreeMap<&'p str, DeviceInstanceId>,
    warnings: Vec<Warning>,
    /// Ready-made devices by patch id (see [`build_with_devices`]).
    supplied: SuppliedDevices,
}

impl<'p> Builder<'p> {
    fn add_devices(
        &mut self,
        patch: &'p Patch,
        registry: &Registry,
        devices: &JsonPath,
    ) -> Result<(), BuildError> {
        for (i, spec) in patch.devices.iter().enumerate() {
            let path = devices.index(i);
            if spec.device_type == "atomik.container" {
                return Err(err(
                    &path.field("type"),
                    BuildErrorKind::UnknownDeviceType,
                    "atomik.container has no engine device: expand the rack first (flat::expanded_patch)",
                ));
            }
            let device = match self.supplied.remove(&spec.id) {
                Some(device) => device,
                None => registry.create(&spec.device_type).ok_or_else(|| {
                    let known: Vec<_> = registry.type_ids().collect();
                    err(
                        &path.field("type"),
                        BuildErrorKind::UnknownDeviceType,
                        format!(
                            "unknown device type {:?}; known types: {}",
                            spec.device_type,
                            known.join(", ")
                        ),
                    )
                })?,
            };
            let descriptor = device.descriptor();
            let id = self.graph.add_device(device);
            self.ids.insert(&spec.id, id);
            for (key, &value) in &spec.params {
                let param_path = path.field("params").field(key);
                let param = descriptor.param_by_key(key).ok_or_else(|| {
                    let hint = closest(key, descriptor.params.iter().map(|p| p.key))
                        .map(|k| format!("; did you mean {k:?}?"))
                        .unwrap_or_default();
                    err(
                        &param_path,
                        BuildErrorKind::UnknownParam,
                        format!("{} has no param {key:?}{hint}", descriptor.type_id),
                    )
                })?;
                let (plain, warning) = clamp_param(param, value, &param_path);
                self.warnings.extend(warning);
                self.graph
                    .set_param(id, param.id, plain)
                    .map_err(|e| err(&param_path, BuildErrorKind::Graph(e), e))?;
            }
        }
        Ok(())
    }

    fn apply_options(&mut self, options: &BuildOptions, path: &JsonPath) -> Result<(), BuildError> {
        for (i, (device, key, value)) in options.params.iter().enumerate() {
            let path = path.field("params").index(i);
            let id = self.device(device, &path)?;
            let descriptor = self
                .graph
                .descriptor(id)
                .ok_or_else(|| err(&path, BuildErrorKind::UnknownDevice, "device vanished"))?;
            let param = descriptor.param_by_key(key).ok_or_else(|| {
                err(
                    &path,
                    BuildErrorKind::UnknownParam,
                    format!("{} has no param {key:?}", descriptor.type_id),
                )
            })?;
            self.graph
                .set_param(id, param.id, *value)
                .map_err(|e| err(&path, BuildErrorKind::Graph(e), e))?;
        }
        for (i, (device, map)) in options.samples.iter().enumerate() {
            let path = path.field("samples").index(i);
            let id = self.device(device, &path)?;
            self.graph
                .set_device_samples(id, Some(std::sync::Arc::clone(map)))
                .map_err(|e| err(&path, BuildErrorKind::Graph(e), e))?;
        }
        for (i, (device, data)) in options.player_data.iter().enumerate() {
            let path = path.field("player_data").index(i);
            let id = self.device(device, &path)?;
            self.graph
                .set_device_player_data(id, Some(std::sync::Arc::clone(data)))
                .map_err(|e| err(&path, BuildErrorKind::Graph(e), e))?;
        }
        Ok(())
    }

    /// Resolves `"device:socket"` to an endpoint and its socket descriptor.
    fn endpoint(
        &self,
        text: &str,
        path: &JsonPath,
    ) -> Result<(Endpoint, &'static SocketDescriptor), BuildError> {
        let (device, socket) = text
            .split_once(':')
            .filter(|(d, s)| !d.is_empty() && !s.is_empty())
            .ok_or_else(|| {
                err(
                    path,
                    BuildErrorKind::BadEndpoint,
                    format!("{text:?} must have the form \"device:socket\""),
                )
            })?;
        let id = self.device(device, path)?;
        let socket_desc = self.socket(id, device, socket, path)?;
        let endpoint = Endpoint {
            device: id,
            socket: socket_desc.id,
        };
        Ok((endpoint, socket_desc))
    }

    fn device(&self, name: &str, path: &JsonPath) -> Result<DeviceInstanceId, BuildError> {
        self.ids.get(name).copied().ok_or_else(|| {
            err(
                path,
                BuildErrorKind::UnknownDevice,
                format!("no device has id {name:?}"),
            )
        })
    }

    fn socket(
        &self,
        id: DeviceInstanceId,
        device: &str,
        socket: &str,
        path: &JsonPath,
    ) -> Result<&'static SocketDescriptor, BuildError> {
        let descriptor = self
            .graph
            .descriptor(id)
            .ok_or_else(|| err(path, BuildErrorKind::UnknownDevice, "device vanished"))?;
        descriptor.socket_by_key(socket).ok_or_else(|| {
            let hint = closest(socket, descriptor.sockets.iter().map(|s| s.key))
                .map(|k| format!("; did you mean {k:?}?"))
                .unwrap_or_default();
            err(
                path,
                BuildErrorKind::UnknownSocket,
                format!("{device:?} has no socket {socket:?}{hint}"),
            )
        })
    }

    /// Turns every note into an On and an Off event, sorted by frame. At equal frames Offs come
    /// first, so a note that starts where a previous one of the same key ends is not cut off.
    fn schedule_notes(
        &mut self,
        patch: &Patch,
        config: &EngineConfig,
        frames: usize,
        notes: &JsonPath,
    ) -> Result<Vec<ScheduledNote>, BuildError> {
        let mut events = Vec::with_capacity(patch.notes.len() * 2);
        for (i, note) in patch.notes.iter().enumerate() {
            let path = notes.index(i);
            let socket_path = path.field("socket");
            let device = self.device(&note.device, &path.field("device"))?;
            let socket = self.socket(device, &note.device, &note.socket, &socket_path)?;
            if socket.kind != SocketKind::Notes || socket.direction != Direction::Input {
                return Err(err(
                    &socket_path,
                    BuildErrorKind::UnknownSocket,
                    format!("{:?} is not a notes input", note.socket),
                ));
            }
            let (on, off) = note.frames(config.tempo, config.sample_rate);
            if on >= frames as u64 {
                self.warnings.push(Warning {
                    path: path.field("at_beats").to_string(),
                    message: "the note starts after the end of the render and is never heard"
                        .into(),
                });
            }
            let event = |frame, kind| ScheduledNote {
                frame,
                device,
                socket: socket.id,
                kind,
            };
            events.push(event(
                on,
                NoteKind::On {
                    key: note.key,
                    velocity: note.velocity,
                },
            ));
            events.push(event(off, NoteKind::Off { key: note.key }));
        }
        events.sort_by_key(|e| (e.frame, !matches!(e.kind, NoteKind::Off { .. })));
        Ok(events)
    }

    fn connect_cables(&mut self, patch: &Patch, cables: &JsonPath) -> Result<(), BuildError> {
        let mut input_owner: BTreeMap<Endpoint, usize> = BTreeMap::new();
        for (i, cable) in patch.cables.iter().enumerate() {
            let path = cables.index(i);
            let (from_path, to_path) = (path.field("from"), path.field("to"));
            let (from, from_desc) = self.endpoint(&cable.from, &from_path)?;
            let (to, to_desc) = self.endpoint(&cable.to, &to_path)?;
            let graph_err =
                |p: &JsonPath, e: GraphError, msg: String| err(p, BuildErrorKind::Graph(e), msg);
            if from_desc.direction != Direction::Output {
                return Err(graph_err(
                    &from_path,
                    GraphError::DirectionMismatch,
                    format!("{:?} is an input; a cable starts at an output", cable.from),
                ));
            }
            if !to_desc.direction.is_input() {
                return Err(graph_err(
                    &to_path,
                    GraphError::DirectionMismatch,
                    format!("{:?} is an output; a cable ends at an input", cable.to),
                ));
            }
            if from_desc.kind != to_desc.kind {
                return Err(graph_err(
                    &path,
                    GraphError::KindMismatch,
                    format!(
                        "cannot connect {} output {:?} to {} input {:?}",
                        kind_name(from_desc.kind),
                        cable.from,
                        kind_name(to_desc.kind),
                        cable.to
                    ),
                ));
            }
            match self.graph.connect(from, to) {
                Ok(()) => {
                    input_owner.insert(to, i);
                }
                Err(e @ GraphError::InputAlreadyConnected) => {
                    let first = input_owner
                        .get(&to)
                        .map(|j| format!(" by {}", cables.index(*j)))
                        .unwrap_or_default();
                    return Err(graph_err(
                        &to_path,
                        e,
                        format!("{:?} is already connected{first}", cable.to),
                    ));
                }
                Err(e @ GraphError::WouldCreateCycle) => {
                    return Err(graph_err(
                        &path,
                        e,
                        format!(
                            "{:?} -> {:?} would create a cycle (feedback loops are not supported)",
                            cable.from, cable.to
                        ),
                    ));
                }
                Err(e) => return Err(graph_err(&path, e, e.to_string())),
            }
        }
        Ok(())
    }
}

fn kind_name(kind: SocketKind) -> &'static str {
    match kind {
        SocketKind::Audio => "an audio",
        SocketKind::Cv => "a CV",
        SocketKind::Notes => "a notes",
    }
}

fn err(path: &JsonPath, kind: BuildErrorKind, message: impl std::fmt::Display) -> BuildError {
    BuildError {
        path: path.to_string(),
        kind,
        message: message.to_string(),
    }
}

/// The candidate closest to `key` by edit distance, if it is close enough to be a typo
/// (at most a third of the key's length, or the key is a prefix of the candidate).
fn closest<'a>(key: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (key.chars().count() / 3).max(1);
    candidates
        .map(|c| (edit_distance(key, c), c))
        .filter(|(d, c)| *d <= limit || (!key.is_empty() && c.starts_with(key)))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::{EXAMPLE, edited};
    use serde_json::json;

    pub(crate) fn registry() -> Registry {
        let mut r = Registry::new();
        atomik_devices::register_all(&mut r).unwrap();
        r
    }

    fn try_build(text: &str) -> Result<Built, BuildError> {
        build(
            &Patch::from_json(text).expect("valid patch json"),
            &registry(),
        )
    }

    fn assert_build_error(text: &str, path: &str, kind: BuildErrorKind, needle: &str) {
        let e = try_build(text).expect_err("build must fail");
        assert_eq!(e.path, path, "{e}");
        assert_eq!(e.kind, kind, "{e}");
        assert!(e.message.contains(needle), "{e:?} lacks {needle:?}");
    }

    #[test]
    fn build_example_compiles_with_config_and_length() {
        let built = try_build(EXAMPLE).unwrap();
        assert_eq!(built.plan.order().len(), 3);
        assert!((built.config.sample_rate.hz() - 48_000.0).abs() < 1e-9);
        assert!((built.config.tempo.bpm() - 120.0).abs() < 1e-9);
        assert_eq!(
            (built.config.input_channels, built.config.output_channels),
            (0, 2)
        );
        assert_eq!(built.config.seed, 1);
        assert_eq!(built.frames, 18 * 24_000);
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    }

    #[test]
    fn patch_unknown_device_type_errors() {
        let text = edited(|v| v["devices"][1]["type"] = json!("atomik.flanger"));
        assert_build_error(
            &text,
            "devices[1].type",
            BuildErrorKind::UnknownDeviceType,
            "atomik.stereo-delay",
        );
    }

    #[test]
    fn patch_param_out_of_range_clamps_with_warning() {
        let text = edited(|v| {
            v["devices"][0]["params"]["filter.cutoff"] = json!(50_000);
            v["devices"][0]["params"]["osc1.wave"] = json!(1.4);
            v["devices"][0]["params"]["voices"] = json!(-3);
            v["devices"][1]["params"]["feedback"] = json!(0.5);
        });
        let built = try_build(&text).unwrap();
        let w: Vec<String> = built.warnings.iter().map(ToString::to_string).collect();
        assert_eq!(w.len(), 3, "{w:?}");
        // Params are visited in key order per device.
        assert_eq!(
            w[0],
            "devices[0].params.\"filter.cutoff\": 50000 is outside 20..=20000; clamped to 20000"
        );
        assert_eq!(
            w[1],
            "devices[0].params.\"osc1.wave\": 1.4 is not a whole number; rounded to 1"
        );
        assert_eq!(
            w[2],
            "devices[0].params.voices: -3 is outside 1..=16; clamped to 1"
        );
    }

    #[test]
    fn build_schedules_notes_sorted_with_off_before_on() {
        use atomik_core::NoteKind::{Off, On};
        let text = edited(|v| {
            let n = |at: f64, len: f64, key: u8| {
                json!({"device": "synth", "socket": "notes_in", "at_beats": at,
                       "length_beats": len, "key": key, "velocity": 0.5})
            };
            v["notes"] = json!([n(1.0, 1.0, 60), n(0.0, 1.0, 60), n(0.5, 0.5, 64)]);
        });
        let built = try_build(&text).unwrap();
        let got: Vec<_> = built.notes.iter().map(|n| (n.frame, n.kind)).collect();
        let on = |key| On { key, velocity: 0.5 };
        assert_eq!(
            got,
            [
                (0, on(60)),
                (12_000, on(64)),
                (24_000, Off { key: 60 }),
                (24_000, Off { key: 64 }),
                (24_000, on(60)),
                (48_000, Off { key: 60 }),
            ]
        );
        for n in &built.notes {
            assert_eq!(n.device, DeviceInstanceId(1), "synth is the first device");
            assert_eq!(n.socket, atomik_device_sdk::SocketId(0), "notes_in");
        }
    }

    #[test]
    fn build_rejects_bad_note_targets_with_paths() {
        use BuildErrorKind as K;
        let note = |field: &'static str, value: &'static str| {
            edited(move |v| v["notes"][0][field] = json!(value))
        };
        assert_build_error(
            &note("device", "nope"),
            "notes[0].device",
            K::UnknownDevice,
            "nope",
        );
        assert_build_error(
            &note("device", "interface"),
            "notes[0].socket",
            K::UnknownSocket,
            "notes_in",
        );
        assert_build_error(
            &note("socket", "out_l"),
            "notes[0].socket",
            K::UnknownSocket,
            "not a notes input",
        );
        assert_build_error(
            &note("socket", "notes"),
            "notes[0].socket",
            K::UnknownSocket,
            "did you mean \"notes_in\"",
        );
    }

    #[test]
    fn build_warns_about_notes_after_the_end() {
        let text = edited(|v| v["notes"][0]["at_beats"] = json!(18));
        let built = try_build(&text).unwrap();
        assert_eq!(built.warnings.len(), 1);
        assert_eq!(built.warnings[0].path, "notes[0].at_beats");
        assert!(built.warnings[0].message.contains("after the end"));
    }

    #[test]
    fn patch_bad_cable_endpoint_errors() {
        use BuildErrorKind as K;
        let cable = |i: usize, end: &'static str, value: &'static str| {
            edited(move |v| v["cables"][i][end] = json!(value))
        };
        let cases = [
            (
                cable(0, "from", "synth-out_l"),
                "cables[0].from",
                K::BadEndpoint,
                "device:socket",
            ),
            (
                cable(0, "from", ":out_l"),
                "cables[0].from",
                K::BadEndpoint,
                "device:socket",
            ),
            (
                cable(0, "from", "synth:"),
                "cables[0].from",
                K::BadEndpoint,
                "device:socket",
            ),
            (
                cable(0, "from", "nope:out_l"),
                "cables[0].from",
                K::UnknownDevice,
                "\"nope\"",
            ),
            (
                cable(0, "from", "synth:out_x"),
                "cables[0].from",
                K::UnknownSocket,
                "\"out_l\"",
            ),
            (
                cable(3, "to", "mixer:ch9_in_r"),
                "cables[3].to",
                K::UnknownSocket,
                "ch9_in_r",
            ),
            (
                cable(4, "to", "interface:out_3"),
                "cables[4].to",
                K::UnknownSocket,
                "out_3",
            ),
            (
                cable(0, "from", "synth:notes_in"),
                "cables[0].from",
                K::Graph(GraphError::DirectionMismatch),
                "is an input",
            ),
            (
                cable(0, "to", "delay:out_l"),
                "cables[0].to",
                K::Graph(GraphError::DirectionMismatch),
                "is an output",
            ),
            (
                cable(0, "to", "delay:time_cv_in"),
                "cables[0]",
                K::Graph(GraphError::KindMismatch),
                "audio",
            ),
            (
                cable(1, "to", "delay:in_l"),
                "cables[1].to",
                K::Graph(GraphError::InputAlreadyConnected),
                "cables[0]",
            ),
            (
                // mixer → delay is added first; delay → mixer (cables[2]) closes the loop.
                cable(0, "from", "mixer:master_l"),
                "cables[2]",
                K::Graph(GraphError::WouldCreateCycle),
                "cycle",
            ),
        ];
        for (text, path, kind, needle) in cases {
            assert_build_error(&text, path, kind, needle);
        }
    }

    /// Two synths summed into the master bus (a summing input takes several cables).
    const SUMMED: &str = r#"{
        "format": "atomik-patch", "version": 2, "sample_rate": 48000, "tempo_bpm": 120,
        "seed": 1, "output_channels": 2, "length_beats": 1,
        "devices": [
            {"id": "a", "type": "atomik.poly-synth"},
            {"id": "b", "type": "atomik.poly-synth"},
            {"id": "ch", "type": "atomik.channel", "params": {"latency.adjust": -480}},
            {"id": "m", "type": "atomik.master"}
        ],
        "cables": [
            {"from": "a:out_l", "to": "m:bus_l"},
            {"from": "b:out_l", "to": "ch:in_l"},
            {"from": "ch:out_l", "to": "m:bus_l"},
            {"from": "m:main_l", "to": "interface:out_1"}
        ],
        "notes": []
    }"#;

    #[test]
    fn summing_inputs_take_several_cables_and_compensate_by_default() {
        let built = try_build(SUMMED).unwrap();
        let pdc = built.plan.pdc_report();
        assert!(pdc.active);
        // `a` is delayed by the channel's reported (phantom) 480 samples.
        assert!(
            pdc.cable_delays.iter().any(|(_, _, d)| *d == 480),
            "{pdc:?}"
        );
    }

    #[test]
    fn build_options_switch_compensation_off_and_set_extra_params() {
        let patch = Patch::from_json(SUMMED).unwrap();
        let options = BuildOptions {
            compensation: false,
            params: vec![("ch".into(), "internal.solo_silence".into(), 1.0)],
            ..BuildOptions::default()
        };
        let built = build_with(&patch, &registry(), &options).unwrap();
        let pdc = built.plan.pdc_report();
        assert!(!pdc.active && !pdc.limit_exceeded);
        assert!(pdc.cable_delays.iter().all(|(_, _, d)| *d == 0));
        let bad = BuildOptions {
            params: vec![("nope".into(), "fader".into(), 0.5)],
            ..BuildOptions::default()
        };
        let e = build_with(&patch, &registry(), &bad).unwrap_err();
        assert_eq!(e.kind, BuildErrorKind::UnknownDevice, "{e}");
        let bad = BuildOptions {
            params: vec![("ch".into(), "nope".into(), 0.5)],
            ..BuildOptions::default()
        };
        let e = build_with(&patch, &registry(), &bad).unwrap_err();
        assert_eq!(e.kind, BuildErrorKind::UnknownParam, "{e}");
        assert!(BuildOptions::default().compensation);
    }

    #[test]
    fn patch_unknown_param_names_json_path() {
        let text = edited(|v| v["devices"][0]["params"]["filter.cutof"] = json!(800));
        let e = try_build(&text).unwrap_err();
        assert_eq!(e.path, "devices[0].params.\"filter.cutof\"");
        assert_eq!(e.kind, BuildErrorKind::UnknownParam);
        assert!(e.message.contains("did you mean \"filter.cutoff\""), "{e}");
        assert!(
            e.to_string()
                .starts_with("devices[0].params.\"filter.cutof\": "),
            "{e}"
        );
    }
}
