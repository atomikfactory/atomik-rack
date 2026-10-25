//! `.atomik` project bounces.
//!
//! A project is the rack (patch v2 content) plus the song. Bouncing one:
//!
//! 1. [`atomik_model::load_project`] reads and validates the file.
//! 2. The rack is built exactly like a patch ([`crate::build::build_with`] on the document's
//!    patch v2 text), so devices get ids 1, 2, 3… in rack order, the same ids and prepare seeds
//!    as the app's engine (`AppCore::rebuild_engine`). Console ([`console_options`]): the
//!    console's cables (channels and returns into the master's summing buses) are ordinary
//!    patch cables, the console's compensation switch goes to `Graph::set_compensation`, and
//!    the solo state is applied through each channel's `internal.solo_silence`, exactly as
//!    the app pushes it, so a bounce sounds like the live console.
//!    Samplers ([`sampler_maps`]): each sampler's zone assets come from the bundle (memory,
//!    with the song's audio) and its sample map is installed before the compile, so a bounce
//!    plays the zones the app plays.
//! 3. [`song_input`] turns the song into the engine's [`SongInput`] (the same function the app
//!    uses for its live engine), and [`compile_song`] compiles it.
//! 4. The song plays from tick 0 for `song.end` plus two beats of tail, in 64-frame batches with
//!    the engine's latency epoch followed after each batch
//!    ([`crate::plugin_pass::render_song_following_latency`]).
//!
//! **Frozen tracks** ([`atomik_model::freeze`]) bounce as the app plays them: their devices keep
//! their ids but are left out of the graph (`BuildOptions::omit`, so they are neither built nor
//! processed, and a frozen plugin is not instantiated), and the track's `~freeze` player plays
//! the render ([`song_input_with_warps`]).
//!
//! The song's loop is **disabled** for bounces unless [`ProjectOptions::loop_count`] asks for
//! `n` passes: then pass 1 runs from tick 0 to the loop end, passes 2…n repeat the loop, and
//! the song stops at the end of the last pass, followed by the two-beat tail.
//!
//! The output depends only on the project: not on the host block size (the loop drives the engine
//! in batches) or the worker count.

use crate::build::{BuildOptions, SuppliedDevices, build_with_devices};
use crate::patch::Warning;
use crate::plugins::{MissingPlugins, PluginHost};
use crate::samples::{build_sample_map, song_asset_buffer};
use crate::warp::WarpStore;
use crate::{RenderError, Rendered};
use atomik_core::{BATCH, PPQ, Tempo, Ticks};
use atomik_device_sdk::{
    DeviceDescriptor, Direction, Registry, RunMode, SampleMapRef, SocketId, SocketKind,
};
use atomik_engine::{
    AssetId, DeviceInstanceId, EngineOptions, HotEdit, OfflineJob, SongAsset, SongAudioRegion,
    SongAudioTrack, SongAutomation, SongController, SongControllerKind, SongCurve, SongFadeCurve,
    SongInput, SongNote, SongPoint, SongSnapshot, SongTempo, SongTrack, TimedEdit,
    TransportCommand, WorkerConfig, compile_song, new_engine_with_options,
};
use atomik_model::console::{SOLO_SILENCE_PARAM, VCA_DB_PARAM, VCA_MUTE_PARAM, VCA_SOLO_PARAM};
use atomik_model::song::{
    effective_tracks, flatten_audio, flatten_automation, flatten_controllers, flatten_notes,
};
use atomik_model::{
    AutomationPoint, ControllerKind, Curve, DeviceKey, ExpandedRack, FadeCurve, LaneKind,
    PROJECT_FORMAT, RackDoc, expand_bank_lane, expand_macro_lane, load_project,
};
use atomik_plugin_host::InstantiateRequest;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

/// Ticks of release tail rendered after the song end (two quarter-note beats).
pub const TAIL_TICKS: i64 = 2 * PPQ;

/// How a project is bounced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectOptions {
    /// Host block size in frames (the output does not depend on it).
    pub host_block: NonZeroUsize,
    /// Engine worker threads (the output does not depend on it).
    pub workers: u16,
    /// `Some(n)`: play the song loop `n` times (n >= 1); `None`: no loop.
    pub loop_count: Option<u32>,
}

impl Default for ProjectOptions {
    fn default() -> Self {
        Self {
            host_block: NonZeroUsize::new(512).unwrap_or(NonZeroUsize::MIN),
            workers: 0,
            loop_count: None,
        }
    }
}

/// True if `text` is a JSON document whose `format` is the project format tag.
pub fn is_project(text: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Tag {
        format: Option<String>,
    }
    serde_json::from_str::<Tag>(text)
        .ok()
        .and_then(|t| t.format)
        .is_some_and(|f| f == PROJECT_FORMAT)
}

/// The first Notes input of a descriptor (where a track's notes go).
pub fn notes_input(desc: &DeviceDescriptor) -> Option<SocketId> {
    desc.sockets
        .iter()
        .find(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
        .map(|s| s.id)
}

/// The engine song input for a document.
///
/// - Tracks: [`effective_tracks`] (mute and solo applied), in display order. A track is
///   skipped when its device has no engine id (`id_of` returns `None`), its type is not in
///   `registry`, or it has no Notes input.
/// - Notes: [`flatten_notes`]; automation: [`flatten_automation`] of every automation lane
///   whose param key the descriptor knows, with the descriptor's `ParamId`.
/// - Tempo map from `song.tempo` (steps and ramps) and the meter map from `song.meter`.
/// - No audio: see [`song_input_with_assets`].
pub fn song_input(
    doc: &RackDoc,
    registry: &Registry,
    id_of: impl Fn(&DeviceKey) -> Option<DeviceInstanceId>,
) -> SongInput {
    song_input_with_assets(doc, registry, id_of, &BTreeMap::new())
}

fn song_point(tick: i64, value: f32, curve: Curve) -> SongPoint {
    SongPoint {
        tick: Ticks(tick),
        value,
        curve: match curve {
            Curve::Linear => SongCurve::Linear,
            Curve::Step => SongCurve::Step,
        },
    }
}

/// Adds one expanded macro lane (`target` is a path key, `param` one of its params) to the
/// automation-only track of that target device, creating the track on first use. A target that
/// has no engine id or no such param is left out.
fn push_macro_lane(
    doc: &RackDoc,
    registry: &Registry,
    id_of: &impl Fn(&DeviceKey) -> Option<DeviceInstanceId>,
    tracks: &mut Vec<SongTrack>,
    target: &DeviceKey,
    param: &str,
    points: &[AutomationPoint],
) {
    let Some(device) = id_of(target) else {
        return;
    };
    let Some(desc) = doc
        .resolve(target)
        .and_then(|d| registry.get(&d.type_id))
        .map(|f| f.descriptor)
    else {
        return;
    };
    let Some(pid) = desc.param_by_key(param).map(|p| p.id) else {
        return;
    };
    let lane = SongAutomation {
        param: pid,
        points: points
            .iter()
            .map(|p| song_point(p.tick, p.value, p.curve))
            .collect(),
    };
    match tracks.iter_mut().find(|t| t.device == device) {
        Some(track) => track.automation.push(lane),
        None => tracks.push(SongTrack {
            device,
            notes_socket: notes_input(desc).unwrap_or(SocketId(0)),
            chase: true,
            notes: Vec::new(),
            automation: vec![lane],
            controllers: Vec::new(),
        }),
    }
}

fn song_fade_curve(curve: FadeCurve) -> SongFadeCurve {
    match curve {
        FadeCurve::Linear => SongFadeCurve::Linear,
        FadeCurve::EqualPower => SongFadeCurve::EqualPower,
        FadeCurve::Exponential => SongFadeCurve::Exponential,
        FadeCurve::SCurve => SongFadeCurve::SCurve,
    }
}

/// [`song_input`] plus the audio tracks: the one function the app and the
/// bounce share.
///
/// - Audio: for each audible audio track (mute and solo as for notes) whose device has an
///   engine id, [`flatten_audio`] at the document's sample rate gives its regions. A region
///   whose asset is not in `assets` (not loaded yet, or missing from the bundle) is left
///   out, so that clip is silent. `SongInput::assets` lists, in order of first use, exactly
///   the assets some region reads (the region's [`AssetId`] is its index). A track with no
///   region is left out.
/// - The caller decides how each asset is held: [`SongAsset::Memory`] everywhere in part 1
///   of the app (which keeps an `Arc` cache, a bounce loads
///   [`crate::bundle::load_song_assets`]).
pub fn song_input_with_assets(
    doc: &RackDoc,
    registry: &Registry,
    id_of: impl Fn(&DeviceKey) -> Option<DeviceInstanceId>,
    assets: &BTreeMap<String, SongAsset>,
) -> SongInput {
    song_input_with_warps(doc, registry, id_of, assets, &WarpStore::new())
}

/// [`song_input_with_assets`] with the rendered buffers of the warped clips (see
/// [`crate::warp`]). A region of a warped clip whose buffer is in `warps` plays that buffer (the
/// pool gets the buffer instead of the asset, and the region reads it from the clip's own
/// frame); without it the region plays the asset in real time. A document without warped clips
/// gives exactly what [`song_input_with_assets`] always gave.
pub fn song_input_with_warps(
    doc: &RackDoc,
    registry: &Registry,
    id_of: impl Fn(&DeviceKey) -> Option<DeviceInstanceId>,
    assets: &BTreeMap<String, SongAsset>,
    warps: &WarpStore,
) -> SongInput {
    let song = &doc.song;
    // Containers: a track on a container plays through its `~split` device, and a
    // lane on one of its `macroN` params expands into one engine lane per macro target.
    let expanded = doc.has_containers().then(|| doc.expand(registry));
    let mut macro_tracks: Vec<SongTrack> = Vec::new();
    // The lanes a macro bank's lanes expand to: `(target, param, points)`.
    let mut bank_lanes: Vec<(DeviceKey, String, Vec<AutomationPoint>)> = Vec::new();
    let mut tracks: Vec<SongTrack> = Vec::new();
    for track in effective_tracks(song) {
        // A frozen track plays its render through its player (below), not its own devices.
        if track.frozen.is_some() {
            continue;
        }
        let is_container = doc.device(&track.device).is_some_and(|d| d.inner.is_some());
        let is_bank = doc
            .device(&track.device)
            .is_some_and(|d| d.type_id == atomik_model::MACRO_BANK_TYPE);
        let (flat, type_id) = match (&expanded, is_container) {
            (Some(exp), true) => {
                let Some(split) = exp.alias.get(&track.device) else {
                    continue;
                };
                (split.clone(), atomik_model::SPLIT_TYPE.to_string())
            }
            _ => {
                let Some(d) = doc.device(&track.device) else {
                    continue;
                };
                (track.device.clone(), d.type_id.clone())
            }
        };
        let Some(device) = id_of(&flat) else {
            continue;
        };
        let Some(desc) = registry.get(&type_id).map(|f| f.descriptor) else {
            continue;
        };
        let Some(notes_socket) = notes_input(desc) else {
            continue;
        };
        let notes = flatten_notes(track)
            .into_iter()
            .map(|(tick, length, key, velocity)| SongNote {
                tick: Ticks(tick),
                length,
                key,
                velocity,
            })
            .collect();
        let mut automation = Vec::new();
        for (i, lane) in track.lanes.iter().enumerate() {
            let LaneKind::Automation(key) = &lane.kind else {
                continue;
            };
            if is_container || is_bank {
                let points: Vec<AutomationPoint> = flatten_automation(track, i)
                    .into_iter()
                    .map(|(tick, value, curve)| AutomationPoint { tick, value, curve })
                    .collect();
                // A bank's lane follows its assignments exactly like a container's macro.
                let lanes = if is_bank {
                    expand_bank_lane(doc, registry, &track.device, key, &points)
                } else {
                    expand_macro_lane(doc, registry, &track.device, key, &points)
                };
                for (target, param, mapped) in lanes.into_iter().flatten() {
                    if is_bank {
                        // The target has its own track: its lane joins that track below.
                        bank_lanes.push((target, param, mapped));
                        continue;
                    }
                    push_macro_lane(
                        doc,
                        registry,
                        &id_of,
                        &mut macro_tracks,
                        &target,
                        &param,
                        &mapped,
                    );
                }
                continue;
            }
            let Some(param) = desc.param_by_key(key).map(|p| p.id) else {
                continue;
            };
            // A param a macro drives follows the macro, whatever its own lane holds.
            if doc.bank_driver(&track.device, key).is_some() {
                continue;
            }
            let points = flatten_automation(track, i)
                .into_iter()
                .map(|(tick, value, curve)| song_point(tick, value, curve))
                .collect();
            automation.push(SongAutomation { param, points });
        }
        let controllers = flatten_controllers(track)
            .into_iter()
            .map(|(kind, points)| SongController {
                kind: match kind {
                    ControllerKind::PitchBend => SongControllerKind::PitchBend,
                    ControllerKind::ChannelPressure => SongControllerKind::ChannelPressure,
                    ControllerKind::Cc(n) => SongControllerKind::Cc(n),
                    ControllerKind::PolyPressure(k) => SongControllerKind::PolyPressure(k),
                },
                points: points
                    .into_iter()
                    .map(|(tick, value, curve)| song_point(tick, value, curve))
                    .collect(),
            })
            .collect();
        tracks.push(SongTrack {
            device,
            notes_socket,
            chase: track.chase,
            notes,
            automation,
            controllers,
        });
    }
    // A macro bank drives top-level devices, which may have a track of their own: the lane joins
    // it (a target without one gets an automation-only track). A frozen target plays its render.
    for (target, param, mapped) in bank_lanes {
        if doc.song.track(&target).is_some_and(|t| t.frozen.is_some()) {
            continue;
        }
        push_macro_lane(doc, registry, &id_of, &mut tracks, &target, &param, &mapped);
    }
    tracks.extend(macro_tracks);
    let tempo = song
        .tempo
        .iter()
        .filter_map(|e| {
            Some(SongTempo {
                tick: Ticks(e.tick),
                tempo: Tempo::new(e.bpm).ok()?,
                ramp: e.ramp,
            })
        })
        .collect();
    let mut used: BTreeMap<String, AssetId> = BTreeMap::new();
    let mut pool: Vec<SongAsset> = Vec::new();
    // Audio tracks, and the frozen tracks as audio tracks of their render on their player.
    let sources: Vec<(std::borrow::Cow<'_, atomik_model::song::Track>, DeviceKey)> =
        effective_tracks(song)
            .into_iter()
            .filter_map(|track| match track.frozen_playback() {
                Some(playback) => Some((
                    std::borrow::Cow::Owned(playback),
                    atomik_model::freeze_key(&track.device),
                )),
                None => track
                    .is_audio()
                    .then(|| (std::borrow::Cow::Borrowed(track), track.device.clone())),
            })
            .collect();
    let audio = sources
        .iter()
        .filter_map(|(track, flat)| {
            let device = id_of(flat)?;
            // Gain envelopes are shared between the regions of one clip (they carry equal lists).
            let mut envelopes: Vec<Vec<(u64, f32)>> = Vec::new();
            let regions: Vec<SongAudioRegion> = flatten_audio(track, song, doc.sample_rate)
                .into_iter()
                .filter_map(|r| {
                    // A warped clip plays its rendered buffer from the clip's own frame.
                    let rendered = r
                        .warp
                        .as_ref()
                        .and_then(|job| warps.get(&job.key()).map(|b| (job.key(), b)));
                    let (key, asset, source_offset, reverse) = match rendered {
                        Some((k, buffer)) => (crate::warp::pool_key(k), buffer, r.clip_pos, false),
                        None => {
                            let (key, asset) = assets.get_key_value(r.asset.as_str())?;
                            (key.clone(), asset, r.source_offset, r.reverse)
                        }
                    };
                    let asset_id = *used.entry(key).or_insert_with(|| {
                        pool.push(asset.clone());
                        AssetId(u32::try_from(pool.len() - 1).unwrap_or(u32::MAX))
                    });
                    let env = if r.envelope.is_empty() {
                        None
                    } else {
                        let k = envelopes
                            .iter()
                            .position(|e| *e == r.envelope)
                            .unwrap_or_else(|| {
                                envelopes.push(r.envelope.clone());
                                envelopes.len() - 1
                            });
                        Some(u32::try_from(k).unwrap_or(u32::MAX))
                    };
                    Some(SongAudioRegion {
                        asset: asset_id,
                        song_start_frame: r.song_start_frame,
                        source_offset,
                        frames: r.frames,
                        gain: r.gain,
                        fade_in: r.fade_in,
                        fade_out: r.fade_out,
                        fade_curve: song_fade_curve(r.fade_curve),
                        clip_pos: r.clip_pos,
                        clip_frames: r.clip_frames,
                        reverse,
                        env,
                        x_in: r.x_in,
                        x_out: r.x_out,
                        x_curve: song_fade_curve(r.x_curve),
                    })
                })
                .collect();
            (!regions.is_empty()).then_some(SongAudioTrack {
                device,
                regions,
                envelopes,
            })
        })
        .collect();
    SongInput {
        tracks,
        tempo,
        meter: song.meter.clone(),
        audio,
        assets: pool,
    }
}

/// The engine ids a bounce (and a fresh app engine) gives the rack: 1, 2, 3... in the order of
/// the expansion ([`RackDoc::expand`]: rack order, each container's `~split` and inner devices
/// right after its slot), keyed by the flat **path key**. The container
/// itself has no id (see [`ExpandedRack::alias`]). A document without containers gets 1, 2, 3...
/// in rack order, as before.
pub fn expanded_ids(expanded: &ExpandedRack) -> BTreeMap<DeviceKey, DeviceInstanceId> {
    expanded
        .devices
        .iter()
        .zip(1u32..)
        .map(|(d, id)| (d.key.clone(), DeviceInstanceId(id)))
        .collect()
}

/// What the transport does during a bounce.
#[derive(Debug, Clone, PartialEq)]
pub struct BouncePlan {
    /// Frames to render.
    pub frames: usize,
    /// Commands queued before the first batch (loop setup).
    pub setup: Vec<TransportCommand>,
    /// Timed transport commands `(timeline frame, command)`.
    pub timed: Vec<(u64, TransportCommand)>,
}

/// Plans the transport of a bounce (module docs): the length, the loop setup and, with a
/// loop count, when the loop is released and the song stops.
pub fn bounce_plan(doc: &RackDoc, snapshot: &SongSnapshot, loop_count: Option<u32>) -> BouncePlan {
    let sr = f64::from(doc.sample_rate);
    let frame = |tick: i64| snapshot.song_frame(Ticks(tick), sr);
    let song = &doc.song;
    let (ls, le) = song.loop_range;
    let set_loop = |enabled| TransportCommand::SetLoop {
        start: Ticks(ls),
        end: Ticks(le),
        enabled,
    };
    match loop_count.filter(|&n| n >= 1) {
        None => BouncePlan {
            frames: usize::try_from(frame(song.end + TAIL_TICKS)).unwrap_or(usize::MAX),
            setup: vec![set_loop(false)],
            timed: Vec::new(),
        },
        Some(n) => {
            let (fls, fle) = (frame(ls), frame(le));
            let lap = fle.saturating_sub(fls);
            let end = fle + u64::from(n - 1) * lap;
            let tail = frame(le + TAIL_TICKS) - fle;
            let mut timed = Vec::new();
            if n >= 2 {
                // One batch into the last pass the wrap has happened: release the loop.
                let last = fle + u64::from(n - 2) * lap;
                timed.push((last + BATCH as u64, set_loop(false)));
            }
            timed.push((end, TransportCommand::Stop));
            BouncePlan {
                frames: usize::try_from(end + tail).unwrap_or(usize::MAX),
                setup: vec![set_loop(n >= 2)],
                timed,
            }
        }
    }
}

/// The build settings a document's console implies: its compensation switch,
/// `internal.solo_silence = 1` on every channel [`RackDoc::solo_silence`] silences, and the VCA
/// feed of every strip in a group that is not the default (`internal.vca_db`, `internal.vca_mute`,
/// `internal.vca_solo`, from [`RackDoc::vca_feeds`]), so a project without groups sets none. Phase
/// The interface also gets the document's input channels (`interface:in_k`), which a
/// bounce feeds with silence, so the audio tracks' input cables resolve.
pub fn console_options(doc: &RackDoc, registry: &Registry) -> BuildOptions {
    BuildOptions {
        input_channels: doc.input_channels,
        compensation: doc.console.compensation,
        params: doc
            .solo_silence(registry)
            .into_iter()
            .filter(|(_, silent)| *silent)
            .map(|(k, _)| (k.0, SOLO_SILENCE_PARAM.to_string(), 1.0))
            .chain(vca_params(doc))
            .collect(),
        samples: Vec::new(),
        player_data: Vec::new(),
        omit: Vec::new(),
    }
}

/// The `internal.vca_*` params of the strips whose VCA feed is not the default.
fn vca_params(doc: &RackDoc) -> Vec<(String, String, f32)> {
    let mut out = Vec::new();
    for (key, feed) in doc.vca_feeds() {
        if feed.db != 0.0 {
            out.push((key.0.clone(), VCA_DB_PARAM.to_string(), feed.db));
        }
        if feed.mute {
            out.push((key.0.clone(), VCA_MUTE_PARAM.to_string(), 1.0));
        }
        if feed.solo {
            out.push((key.0, VCA_SOLO_PARAM.to_string(), 1.0));
        }
    }
    out
}

/// The pattern data of every device of `doc` that takes it (the step sequencers), as
/// `(device key, data)`: the data the app sends to the engine
/// ([`atomik_model::player_data_for`]), so a bounce plays what the live path plays.
///
/// The devices are the expanded ones (`expanded` = `doc.expand(registry)`), keyed by
/// their flat path key, so a sequencer inside a container plays too.
pub fn player_data_maps(
    doc: &RackDoc,
    registry: &Registry,
    expanded: &ExpandedRack,
) -> Vec<(String, atomik_device_sdk::PlayerDataRef)> {
    expanded
        .devices
        .iter()
        .filter(|e| !e.frozen)
        .filter_map(|e| {
            let d = doc.resolve(&e.key)?;
            Some((e.key.0.clone(), atomik_model::player_data_for(d, registry)?))
        })
        .collect()
}

/// The sample map of every sampler of `doc` that has playable zones, as `(device key, map)`,
/// built from the memory assets in `assets` (the app builds the same maps
/// from its asset cache). Zones the map rejects become warnings.
pub fn sampler_maps(
    expanded: &ExpandedRack,
    registry: &Registry,
    assets: &BTreeMap<String, SongAsset>,
) -> (Vec<(String, SampleMapRef)>, Vec<Warning>) {
    let mut buffers: BTreeMap<&str, Option<std::sync::Arc<atomik_device_sdk::SampleBuffer>>> =
        BTreeMap::new();
    let (mut maps, mut warnings) = (Vec::new(), Vec::new());
    for device in &expanded.devices {
        if device.frozen || device.zones.is_empty() || !registry.takes_samples(&device.type_id) {
            continue;
        }
        let built = build_sample_map(&device.zones, |asset| {
            let (key, found) = assets.get_key_value(asset)?;
            buffers
                .entry(key.as_str())
                .or_insert_with(|| song_asset_buffer(found))
                .clone()
        });
        warnings.extend(built.rejected.into_iter().map(|message| Warning {
            path: format!("devices.{}.zones", device.key),
            message,
        }));
        if let Some(map) = built.map {
            maps.push((device.key.0.clone(), map));
        }
    }
    (maps, warnings)
}

/// Loads, builds and bounces project JSON that has no bundle: a project with audio clips
/// fails (use [`render_project_file`] or [`render_project_json_in`]).
pub fn render_project_json(
    text: &str,
    registry: &Registry,
    options: &ProjectOptions,
) -> Result<Rendered, RenderError> {
    render_project_json_in(text, registry, options, None)
}

/// [`render_project_json`] with the bundle folder the project's audio lives in.
pub fn render_project_json_in(
    text: &str,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&Path>,
) -> Result<Rendered, RenderError> {
    let doc = load_project(text, registry).map_err(|e| RenderError::Project(e.to_string()))?;
    render_doc_in(&doc, registry, options, bundle_root)
}

/// Reads and bounces a project file; its bundle is found with
/// [`crate::bundle::existing_bundle_root`] (a folder is accepted too, see
/// [`crate::bundle::resolve_project_path`]).
pub fn render_project_file(
    path: &Path,
    registry: &Registry,
    options: &ProjectOptions,
) -> Result<Rendered, RenderError> {
    let file = crate::bundle::resolve_project_path(path);
    let text = std::fs::read_to_string(&file).map_err(|source| RenderError::Read {
        path: file.clone(),
        source,
    })?;
    let root = crate::bundle::existing_bundle_root(&file);
    render_project_json_in(&text, registry, options, root.as_deref())
}

/// Bounces a project document that has no bundle (module docs); see [`render_doc_in`].
pub fn render_doc(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
) -> Result<Rendered, RenderError> {
    render_doc_in(doc, registry, options, None)
}

/// Bounces a project document (module docs). The song's audio assets are loaded
/// fully into memory from the bundle folder `bundle_root` (`<root>/Audio/<hash>.wav`), so the
/// bounce never streams and stays bit-identical. A missing asset fails with an
/// error that names it; so does a project with audio and no bundle.
pub fn render_doc_in(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&Path>,
) -> Result<Rendered, RenderError> {
    render_doc_with(
        doc,
        registry,
        options,
        bundle_root,
        &mut PluginBounce::default(),
    )
}

/// How the plugins of a project take part in a bounce.
#[derive(Debug)]
pub struct PluginBounce<'a> {
    /// The plugin host that builds the real devices. `None`: no plugin can be built, so a project
    /// with plugins needs [`MissingPlugins::StandIn`].
    pub host: Option<&'a mut PluginHost>,
    /// The run mode the plugins get: [`RunMode::Offline`] by default, [`RunMode::Realtime`] for a
    /// bounce that must match what was heard (`--plugin-mode`).
    pub mode: RunMode,
    /// What to do with a plugin that is not installed.
    pub missing: MissingPlugins,
}

impl Default for PluginBounce<'_> {
    fn default() -> Self {
        Self {
            host: None,
            mode: RunMode::Offline,
            missing: MissingPlugins::Error,
        }
    }
}

/// [`render_doc_in`] with plugins. Every plugin device of the expansion is built by `plugins.host`
/// (state restored, then params, then 64-frame blocks like live); a plugin the host does not have
/// is an error that names it, or a stand-in with [`MissingPlugins::StandIn`]. The caller registers
/// the device types first ([`PluginHost::set_catalog`], then
/// [`atomik_model::register_stand_ins`] for the missing ones).
pub fn render_doc_with(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&Path>,
    plugins: &mut PluginBounce<'_>,
) -> Result<Rendered, RenderError> {
    let rendered = render_core(
        doc,
        registry,
        (options, bundle_root),
        plugins,
        &Extra::default(),
    )?;
    // Without a control that can stop it, the render always finishes.
    rendered
        .map(|core| core.rendered)
        .ok_or_else(|| RenderError::Project("the render was cancelled".to_string()))
}

/// What a render adds to a project bounce: taps on the flat rack, a length of its own and a
/// control (the freeze render, [`crate::freeze`]).
#[derive(Debug, Default)]
pub(crate) struct Extra<'a> {
    /// Outputs of the flat rack to capture, each as a stereo pair: tap `k` is cabled to the
    /// interface outputs `n + 2k + 1` and `n + 2k + 2`, where `n` is the document's
    /// `output_channels`, and comes back as those channels of the result.
    pub taps: Vec<[atomik_model::SocketRef; 2]>,
    /// Frames to render instead of the song's length and tail.
    pub frames: Option<usize>,
    /// Devices (flat keys) whose latency, as the engine published it when the render ended, is
    /// reported in [`Core::latencies`].
    pub probes: Vec<DeviceKey>,
    /// Devices (flat keys) whose published meter values and scope frames are read when the
    /// render ends ([`Core::meters`], [`probe_devices`]).
    pub meter_probes: Vec<DeviceKey>,
    /// Progress and cancel.
    pub control: crate::plugin_pass::RenderControl<'a>,
}

/// A finished [`render_core`].
pub(crate) struct Core {
    /// The audio, with every tap's channels after the document's.
    pub rendered: Rendered,
    /// The latency in frames of each of `Extra::probes` (0 for a device that is not built).
    pub latencies: Vec<u32>,
    /// What each of `Extra::meter_probes` showed when the render ended (devices that are not
    /// built are left out).
    pub meters: Vec<DeviceProbe>,
}

/// Frames of a device's scope tap kept by a probe (the largest analysis window).
const PROBE_FRAMES: usize = 8192;

/// What a metering device showed when an offline render ended: its published meter values
/// (`Device::meter_values`) and the newest frames of its scope tap (`Device::scope_tap`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProbe {
    /// The device's flat key.
    pub key: DeviceKey,
    /// The last published meter values (empty for a device that publishes none).
    pub values: Vec<f32>,
    /// The newest frames of the tap's left channel, oldest first (at most 8192; empty without a
    /// tap).
    pub left: Vec<f32>,
    /// The right channel of the same frames.
    pub right: Vec<f32>,
    /// The tap's write position: the number of frames the device processed.
    pub head: u64,
}

/// Renders `doc` offline for `frames` frames (a bounce that stops early: the same code, so the
/// devices see exactly the audio of the bounce) and returns what each of the devices `keys`
/// (flat keys; those that are not built are left out) showed at the end. Nothing in the render
/// depends on the probe. A project with plugins is refused.
///
/// # Errors
/// [`RenderError`] as for [`render_doc_in`].
pub fn probe_devices(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&Path>,
    frames: usize,
    keys: &[DeviceKey],
) -> Result<Vec<DeviceProbe>, RenderError> {
    let extra = Extra {
        frames: Some(frames),
        meter_probes: keys.to_vec(),
        ..Extra::default()
    };
    let core = render_core(
        doc,
        registry,
        (options, bundle_root),
        &mut PluginBounce::default(),
        &extra,
    )?;
    core.map(|c| c.meters)
        .ok_or_else(|| RenderError::Project("the render was cancelled".to_string()))
}

/// The bounce of [`render_doc_with`], with the additions of `extra`. `None` when `extra.control`
/// stopped it. The documented determinism holds: nothing in `extra` changes a sample of what the
/// document renders (a tap is one more cable from an output).
pub(crate) fn render_core(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    extra: &Extra<'_>,
) -> Result<Option<Core>, RenderError> {
    let bundle = bundle_root.and_then(|root| atomik_audio_file::Bundle::open(root).ok());
    let assets =
        crate::bundle::load_song_assets(doc, bundle.as_ref()).map_err(RenderError::Project)?;
    // Warped clips: every buffer is rendered before the song is built, so a bounce never plays
    // a clip dry and its bits do not depend on timing.
    let warps = crate::warp::render_warps(doc, &assets);
    // Containers are compiled away first: the engine graph is the expanded rack.
    let mut expanded = doc.expand(registry);
    let output_channels = tap_cables(doc, &mut expanded, &extra.taps);
    let (mut patch, patch_ids) = crate::flat::expanded_patch(doc, &expanded);
    patch.output_channels = output_channels;
    let patch_id = |key: String| {
        patch_ids
            .get(&DeviceKey(key.clone()))
            .cloned()
            .unwrap_or(key)
    };
    let mut build_options = console_options(doc, registry);
    // The devices of frozen tracks keep their ids but are never built into the graph.
    build_options.omit = expanded
        .devices
        .iter()
        .filter(|d| d.frozen)
        .map(|d| patch_id(d.key.0.clone()))
        .collect();
    let (maps, sampler_warnings) = sampler_maps(&expanded, registry, &assets);
    build_options.samples = maps.into_iter().map(|(k, m)| (patch_id(k), m)).collect();
    build_options.player_data = player_data_maps(doc, registry, &expanded)
        .into_iter()
        .map(|(k, d)| (patch_id(k), d))
        .collect();
    let (supplied, plugin_count) =
        build_plugin_devices(&expanded, registry, doc, plugins, &patch_id)?;
    // Real plugin devices (stand-ins are plain devices and need no manager).
    let plugin_live = supplied.len();
    let mut built = build_with_devices(&patch, registry, &build_options, supplied)?;
    let topology = std::mem::replace(
        &mut built.topology,
        atomik_engine::Topology::new(&built.config),
    );
    let mut warnings = std::mem::take(&mut built.warnings);
    warnings.extend(sampler_warnings);
    let ids = expanded_ids(&expanded);
    let input = song_input_with_warps(doc, registry, |k| ids.get(k).copied(), &assets, &warps);
    let snapshot = compile_song(&input).map_err(|e| RenderError::Project(e.to_string()))?;
    let mut plan = bounce_plan(doc, &snapshot, options.loop_count);
    if let Some(frames) = extra.frames {
        plan.frames = frames;
    }
    let engine_options = EngineOptions {
        workers: WorkerConfig {
            threads: options.workers,
            on_thread_start: None,
        },
        worker_linger_spins: 0,
    };
    let (mut engine, mut handle) = new_engine_with_options(built.config, engine_options);
    let queued = handle.submit_plan(built.plan).and_then(|()| {
        plan.setup
            .iter()
            .try_for_each(|&command| handle.transport(command))
    });
    queued.map_err(|e| RenderError::Project(format!("engine: {e}")))?;
    let edits = plan
        .timed
        .iter()
        .map(|&(frame, command)| TimedEdit {
            frame,
            edit: HotEdit::Transport(command),
        })
        .collect();
    let job = OfflineJob {
        frames: plan.frames,
        host_block: options.host_block.get(),
        notes: &[],
        params: &[],
        inputs: &[],
    };
    // A project with plugins is bounced by the plugin loop (pre-roll, a manager poll and a PDC
    // recompile after every batch, `plugin_pass`); a project without them, or a bounce with
    // stand-ins only, uses the same batch loop without the manager (latency epoch only).
    let channels = match plugins.host.as_deref_mut().filter(|_| plugin_live > 0) {
        Some(host) => {
            let mut pass = crate::plugin_pass::PluginPass::new(host, topology);
            let channels = crate::plugin_pass::render_song_with_plugins(
                &mut engine,
                &mut handle,
                &job,
                snapshot,
                edits,
                &mut pass,
                &extra.control,
            );
            warnings.append(&mut pass.warnings);
            channels
        }
        // The plain bounce follows the engine's latency epoch like the live app
        // does, in 64-frame batches (`render_song_following_latency`), so a preset change across
        // the latency classes of `atomik.character` is compensated from the next batch on.
        None => crate::plugin_pass::render_song_following_latency(
            &mut engine,
            &mut handle,
            &job,
            snapshot,
            edits,
            topology,
            &mut warnings,
            &extra.control,
        ),
    };
    let latencies = extra
        .probes
        .iter()
        .map(|k| {
            ids.get(k)
                .and_then(|id| handle.device_latency(*id))
                .unwrap_or(0)
        })
        .collect();
    let meters = extra
        .meter_probes
        .iter()
        .filter_map(|k| {
            let id = *ids.get(k)?;
            let mut values = [0.0f32; atomik_engine::MAX_METER_VALUES];
            let published = handle.device_meter_values(id, &mut values);
            let tap = handle.device_scope_tap(id);
            if published.is_none() && tap.is_none() {
                return None;
            }
            let (mut left, mut right) = (vec![0.0; PROBE_FRAMES], vec![0.0; PROBE_FRAMES]);
            let (n, head) = tap.map_or((0, 0), |t| t.read_latest(&mut left, &mut right));
            left.truncate(n);
            right.truncate(n);
            Some(DeviceProbe {
                key: k.clone(),
                values: values[..published.map_or(0, |(n, _)| n)].to_vec(),
                left,
                right,
                head,
            })
        })
        .collect();
    // The devices go away with the engine; the plugins are destroyed on this (the main) thread.
    drop(engine);
    drop(handle);
    if let Some(host) = plugins.host.as_deref_mut() {
        host.poll();
    }
    Ok(channels.map(|channels| Core {
        rendered: Rendered {
            channels,
            sample_rate: doc.sample_rate,
            warnings,
            plugins: plugin_count,
        },
        latencies,
        meters,
    }))
}

/// Cables each tap of `taps` to its own pair of interface outputs, after the document's, and
/// returns the number of output channels the render needs (the document's when there is no tap).
fn tap_cables(
    doc: &RackDoc,
    expanded: &mut ExpandedRack,
    taps: &[[atomik_model::SocketRef; 2]],
) -> u16 {
    let base = doc.output_channels;
    for (k, tap) in taps.iter().enumerate() {
        for (side, from) in tap.iter().enumerate() {
            let out = usize::from(base) + 2 * k + side + 1;
            expanded.cables.push(atomik_model::Cable {
                from: from.clone(),
                to: atomik_model::SocketRef {
                    device: DeviceKey(atomik_model::INTERFACE_KEY.to_owned()),
                    socket: format!("out_{out}"),
                },
            });
        }
    }
    base.saturating_add(u16::try_from(2 * taps.len()).unwrap_or(u16::MAX))
}

/// The plugin devices of the expansion, by patch id, and how many plugin devices there are.
fn build_plugin_devices(
    expanded: &ExpandedRack,
    registry: &Registry,
    doc: &RackDoc,
    plugins: &mut PluginBounce<'_>,
    patch_id: &impl Fn(String) -> String,
) -> Result<(SuppliedDevices, usize), RenderError> {
    let mut supplied = SuppliedDevices::new();
    let mut count = 0;
    let mut missing: Vec<String> = Vec::new();
    for e in &expanded.devices {
        // A frozen plugin is not built: its track plays the render.
        let Some(pd) = e.plugin.as_ref().filter(|_| !e.frozen) else {
            continue;
        };
        count += 1;
        let plugin = pd.plugin_ref();
        let name = format!("{} ({})", pd.name(), plugin.type_id());
        let installed = plugins
            .host
            .as_deref()
            .is_some_and(|h| h.is_installed(&plugin));
        if !installed {
            missing.push(name);
            continue;
        }
        let mode = plugins.mode;
        let Some(host) = plugins.host.as_deref_mut() else {
            continue;
        };
        let request = InstantiateRequest {
            plugin,
            state: pd.state.as_deref().cloned(),
            sample_rate: f64::from(doc.sample_rate),
            mode,
            pinned: pd.run_on_audio_thread,
            powered: pd.powered,
        };
        let handle = host
            .manager()
            .instantiate(&request)
            .map_err(|err| RenderError::Project(format!("plugin {name}: {err}")))?;
        if let Some(reason) = handle.restore_error {
            return Err(RenderError::Project(format!(
                "plugin {name}: its saved state could not be restored: {reason}"
            )));
        }
        supplied.insert(patch_id(e.key.0.clone()), Box::new(handle.device));
    }
    if !missing.is_empty() {
        if plugins.missing == MissingPlugins::Error {
            return Err(RenderError::Project(format!(
                "missing plugin{}: {}; install and scan {}, or bounce with --missing-plugins stand-in",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", "),
                if missing.len() == 1 { "it" } else { "them" },
            )));
        }
        let unregistered = expanded
            .devices
            .iter()
            .any(|d| d.plugin.is_some() && !d.frozen && registry.get(&d.type_id).is_none());
        if unregistered {
            return Err(RenderError::Project(
                "a plugin device type is not registered (call register_stand_ins after loading the project)"
                    .to_string(),
            ));
        }
    }
    Ok((supplied, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::{
        AutomationPoint, Clip, ClipContent, ClipId, Command, Document, Note, SongCommand, empty_doc,
    };

    const SYNTH: &str = "atomik.poly-synth";

    fn registry() -> Registry {
        crate::default_registry()
    }

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    /// Two synths with tracks: `a` has a note clip and a cutoff automation clip.
    fn doc() -> RackDoc {
        let r = registry();
        let mut d = Document::new(empty_doc());
        let mut apply = |c: Command| {
            d.apply(c, &r).unwrap();
        };
        for k in ["a", "b"] {
            apply(Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.into(),
                key: Some(key(k)),
            });
            apply(Command::Song(SongCommand::AddTrack { device: key(k) }));
        }
        apply(Command::Song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: PPQ,
                length: 4 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, PPQ / 2, 60, 0.5)]),
            },
        }));
        apply(Command::Song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: "filter.cutoff".into(),
        }));
        apply(Command::Song(SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Automation(vec![
                    AutomationPoint {
                        tick: 0,
                        value: 500.0,
                        curve: Curve::Linear,
                    },
                    AutomationPoint {
                        tick: 2 * PPQ,
                        value: 1500.0,
                        curve: Curve::Step,
                    },
                ]),
            },
        }));
        d.doc().clone()
    }

    #[test]
    fn song_input_track_without_chase_compiles_as_not_chasing() {
        let mut doc = doc();
        let r = registry();
        let ids = expanded_ids(&doc.expand(&r));
        let chase = |doc: &RackDoc| -> Vec<bool> {
            song_input(doc, &r, |k| ids.get(k).copied())
                .tracks
                .iter()
                .map(|t| t.chase)
                .collect()
        };
        assert_eq!(chase(&doc), [true, true], "tracks chase by default");
        doc.song.tracks[1].chase = false;
        assert_eq!(chase(&doc), [true, false]);
    }

    #[test]
    fn song_input_maps_controller_lanes_to_the_track() {
        use atomik_model::{ControllerKind, ControllerLane};
        let mut doc = doc();
        let r = registry();
        let ids = expanded_ids(&doc.expand(&r));
        let point = |tick, value, curve| AutomationPoint { tick, value, curve };
        doc.song.tracks[0].lanes[0].clips[0].controllers = vec![
            ControllerLane::new(
                ControllerKind::PitchBend,
                vec![point(0, -0.5, Curve::Linear), point(PPQ, 0.5, Curve::Step)],
            ),
            ControllerLane::new(ControllerKind::Cc(1), vec![point(100, 1.0, Curve::Step)]),
        ];
        let input = song_input(&doc, &r, |k| ids.get(k).copied());
        let lanes = &input.tracks[0].controllers;
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].kind, SongControllerKind::PitchBend);
        assert_eq!(lanes[1].kind, SongControllerKind::Cc(1));
        // The clip starts at PPQ: the points are in song ticks, curves mapped.
        let ticks: Vec<i64> = lanes[0].points.iter().map(|p| p.tick.0).collect();
        assert_eq!(ticks, [PPQ, 2 * PPQ]);
        assert_eq!(lanes[0].points[0].curve, SongCurve::Linear);
        assert_eq!(lanes[1].points[0].tick, Ticks(PPQ + 100));
        assert!(input.tracks[1].controllers.is_empty());
        assert!(compile_song(&input).is_ok());
    }

    #[test]
    fn song_input_maps_notes_automation_ids_and_tempo() {
        let doc = doc();
        let r = registry();
        let ids = expanded_ids(&doc.expand(&r));
        // Each synth gets its channel right after it, and the master is appended
        // with the first channel, so ids follow the deterministic rack order a, a's channel,
        // master, b, b's channel.
        let order: Vec<&str> = doc.devices.iter().map(|d| d.type_id.as_str()).collect();
        assert_eq!(
            order,
            [
                SYNTH,
                "atomik.channel",
                "atomik.master",
                SYNTH,
                "atomik.channel"
            ]
        );
        assert_eq!(ids[&key("a")], DeviceInstanceId(1));
        assert_eq!(ids[&key("b")], DeviceInstanceId(4));
        assert_eq!(
            expanded_ids(&self::tests::doc().expand(&r)),
            ids,
            "deterministic"
        );
        let input = song_input(&doc, &r, |k| ids.get(k).copied());
        let desc = r.get(SYNTH).unwrap().descriptor;
        assert_eq!(input.tracks.len(), 2);
        let a = &input.tracks[0];
        assert_eq!(a.device, DeviceInstanceId(1));
        assert_eq!(input.tracks[1].device, DeviceInstanceId(4));
        assert_eq!(Some(a.notes_socket), notes_input(desc));
        assert_eq!(
            a.notes,
            vec![SongNote {
                tick: Ticks(PPQ),
                length: PPQ / 2,
                key: 60,
                velocity: 0.5
            }]
        );
        assert_eq!(a.automation.len(), 1);
        assert_eq!(
            a.automation[0].param,
            desc.param_by_key("filter.cutoff").unwrap().id
        );
        assert_eq!(a.automation[0].points[0].tick, Ticks(0));
        assert_eq!(a.automation[0].points[0].curve, SongCurve::Linear);
        assert!(input.tracks[1].notes.is_empty());
        assert_eq!(input.tempo.len(), 1);
        assert!((input.tempo[0].tempo.bpm() - 120.0).abs() < 1e-12);
        assert_eq!(input.meter, vec![atomik_core::MeterEvent::new(1, 4, 4)]);
        assert!(compile_song(&input).is_ok());
    }

    #[test]
    fn audio_tracks_become_engine_regions_over_the_assets_they_use() {
        let t = crate::bundle::tests::Tmp::new("song-input");
        let (doc, bundle, hash) =
            crate::bundle::tests::project_with_asset(&t.0.join("song"), 1000, 48_000);
        let r = registry();
        let ids = expanded_ids(&doc.expand(&r));
        let assets = crate::bundle::load_song_assets(&doc, Some(&bundle)).unwrap();
        let input = song_input_with_assets(&doc, &r, |k| ids.get(k).copied(), &assets);
        assert_eq!(input.audio.len(), 1);
        let track = &input.audio[0];
        assert_eq!(track.device, ids[&key("audio-track")]);
        assert_eq!(track.regions.len(), 1);
        let region = &track.regions[0];
        assert_eq!(region.asset, atomik_engine::AssetId(0));
        // One beat at 120 BPM and 48 kHz.
        assert_eq!((region.song_start_frame, region.frames), (0, 24_000));
        assert_eq!(region.source_offset, 0);
        assert_eq!(input.assets, vec![assets[&hash].clone()]);
        assert!(compile_song(&input).is_ok());
        // Without the data the clip is silent: no regions, no assets.
        let bare = song_input(&doc, &r, |k| ids.get(k).copied());
        assert!(bare.audio.is_empty() && bare.assets.is_empty());
        // A muted audio track plays nothing.
        let mut muted = doc.clone();
        muted.song.tracks[0].muted = true;
        let input = song_input_with_assets(&muted, &r, |k| ids.get(k).copied(), &assets);
        assert!(input.audio.is_empty());
    }

    #[test]
    fn bounces_of_audio_projects_name_the_assets_a_missing_bundle_lacks() {
        let t = crate::bundle::tests::Tmp::new("render-missing");
        let (doc, bundle, hash) =
            crate::bundle::tests::project_with_asset(&t.0.join("song"), 1000, 48_000);
        let r = registry();
        let e = render_doc(&doc, &r, &ProjectOptions::default()).unwrap_err();
        assert!(e.to_string().contains("ramp.wav"), "{e}");
        std::fs::remove_file(bundle.asset_path(&hash)).unwrap();
        let e =
            render_doc_in(&doc, &r, &ProjectOptions::default(), Some(bundle.root())).unwrap_err();
        assert!(e.to_string().contains(&hash), "{e}");
    }

    #[test]
    fn muted_tracks_and_devices_without_ids_are_left_out() {
        let mut doc = doc();
        doc.song.tracks[0].muted = true;
        let r = registry();
        let input = song_input(&doc, &r, |k| {
            (k == &key("b")).then_some(DeviceInstanceId(9))
        });
        assert_eq!(input.tracks.len(), 1);
        assert_eq!(input.tracks[0].device, DeviceInstanceId(9));
        let none = song_input(&doc, &r, |_| None);
        assert!(none.tracks.is_empty());
    }

    #[test]
    fn the_bounce_is_the_song_end_plus_two_beats_without_a_loop() {
        let doc = doc();
        let snapshot = compile_song(&song_input(&doc, &registry(), |_| None)).unwrap();
        let plan = bounce_plan(&doc, &snapshot, None);
        // 16 bars at 120 BPM = 32 s, plus 1 s.
        assert_eq!(plan.frames, 33 * 48_000);
        assert_eq!(
            plan.setup,
            vec![TransportCommand::SetLoop {
                start: Ticks(0),
                end: Ticks(16 * PPQ),
                enabled: false
            }]
        );
        assert!(plan.timed.is_empty());
    }

    #[test]
    fn a_loop_count_repeats_the_loop_then_stops() {
        let doc = doc();
        let snapshot = compile_song(&song_input(&doc, &registry(), |_| None)).unwrap();
        // Loop bars 1–5 (8 s) three times.
        let plan = bounce_plan(&doc, &snapshot, Some(3));
        let lap = 8 * 48_000;
        assert_eq!(plan.frames, 3 * lap + 48_000);
        assert!(matches!(
            plan.setup[..],
            [TransportCommand::SetLoop { enabled: true, .. }]
        ));
        assert_eq!(plan.timed.len(), 2);
        assert_eq!(plan.timed[0].0, 2 * lap as u64 + 64);
        assert!(matches!(
            plan.timed[0].1,
            TransportCommand::SetLoop { enabled: false, .. }
        ));
        assert_eq!(plan.timed[1], (3 * lap as u64, TransportCommand::Stop));
        let once = bounce_plan(&doc, &snapshot, Some(1));
        assert_eq!(once.frames, lap + 48_000);
        assert_eq!(once.timed, vec![(lap as u64, TransportCommand::Stop)]);
    }

    #[test]
    fn projects_are_told_from_patches_by_their_format_tag() {
        assert!(is_project(r#"{"format":"atomik-project","version":1}"#));
        assert!(!is_project(r#"{"format":"atomik-patch","version":2}"#));
        assert!(!is_project("not json"));
        assert!(!is_project("[]"));
    }

    /// The mode a bounce asks for is the mode the plugins are activated in: offline by
    /// default, realtime for a bounce that must match what was heard.
    #[test]
    fn plugins_are_built_in_the_run_mode_the_bounce_asks_for() {
        use atomik_device_sdk::PluginRef;
        use atomik_model::PluginCommand;
        use atomik_plugin_host::testing::clap_test_plugin;
        let file = clap_test_plugin("atomik-test-clap").expect("the test plugin builds");
        let cache = crate::plugins::scan_in_process(&[file]);
        for mode in [RunMode::Offline, RunMode::Realtime] {
            let mut registry = registry();
            let mut host = PluginHost::new();
            host.set_catalog(&cache, &crate::plugins::bounce_settings(), &mut registry);
            let plugin = PluginRef::from_type_id("clap.atomik.test.synth").unwrap();
            let layout = host.catalog().entry(&plugin).unwrap().layout.clone();
            let mut d = Document::new(empty_doc());
            d.apply(
                Command::Plugin(PluginCommand::Add {
                    index: usize::MAX,
                    container: None,
                    plugin,
                    layout,
                    path_hint: None,
                    key: Some(key("lead")),
                    auto_split: false,
                }),
                &registry,
            )
            .unwrap();
            let doc = d.doc().clone();
            let expanded = doc.expand(&registry);
            let mut bounce = PluginBounce {
                host: Some(&mut host),
                mode,
                missing: MissingPlugins::Error,
            };
            let (supplied, count) =
                build_plugin_devices(&expanded, &registry, &doc, &mut bounce, &|k| k).unwrap();
            assert_eq!((supplied.len(), count), (1, 1));
            let ids = host.manager_ref().instance_ids();
            assert_eq!(ids.len(), 1);
            assert_eq!(host.manager_ref().run_mode(ids[0]), Some(mode), "{mode:?}");
        }
    }
}
