//! Audio test support: a stub `atomik.audio-track` with the exact contract keys,
//! asset helpers and command builders.

use super::song::sc;
use super::*;
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    Registry, SocketDescriptor, SocketId, SocketKind,
};
use atomik_model::{
    AssetMeta, AudioClipContent, ClipContent, ClipId, FadeCurve, LaneKind, RackDoc, SongCommand,
    empty_doc,
};
use proptest::prelude::*;

pub const AUDIO_TRACK: &str = "atomik.audio-track";

const fn sock_d(id: u16, key: &'static str, direction: Direction) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind: SocketKind::Audio,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 4] = [
    sock_d(0, "in_l", Direction::Input),
    sock_d(1, "in_r", Direction::Input),
    sock_d(2, "out_l", Direction::Output),
    sock_d(3, "out_r", Direction::Output),
];

static PARAMS: [ParamDescriptor; 2] = [
    ParamDescriptor {
        id: ParamId(0),
        key: "monitor",
        name: "Monitor",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["off", "auto", "on"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(1),
        key: "internal.monitor_live",
        name: "Monitor live",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
];

/// Stub `atomik.audio-track`: an Instrument with a single stage.
pub static AUDIO_TRACK_DESC: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: AUDIO_TRACK,
    name: "Audio Track",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

struct Null;

impl Device for Null {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &AUDIO_TRACK_DESC
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {}
}

/// [`console_registry`] plus the stub audio track.
pub fn audio_registry() -> Registry {
    let mut r = console_registry();
    r.register(DeviceFactory {
        descriptor: &AUDIO_TRACK_DESC,
        create: || Box::new(Null),
    })
    .unwrap();
    r
}

/// [`registry`] (no console devices) plus the stub audio track.
pub fn audio_registry_no_console() -> Registry {
    let mut r = registry();
    r.register(DeviceFactory {
        descriptor: &AUDIO_TRACK_DESC,
        create: || Box::new(Null),
    })
    .unwrap();
    r
}

/// A valid asset id: 64 hex digits made of `n`.
pub fn hash(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

pub fn meta(frames: u64) -> AssetMeta {
    AssetMeta {
        channels: 2,
        sample_rate: 48_000,
        frames,
        original_name: format!("take-{frames}.wav"),
    }
}

pub fn register(n: u8, frames: u64) -> Command {
    Command::RegisterAsset {
        hash: hash(n),
        meta: meta(frames),
    }
}

pub fn audio(n: u8) -> AudioClipContent {
    AudioClipContent {
        asset: hash(n),
        source_offset: 0,
        gain_db: 0.0,
        fade_in: 0,
        fade_out: 0,
        fade_curve: FadeCurve::Linear,
    }
}

pub fn add_audio_track() -> Command {
    sc(SongCommand::AddAudioTrack { name: None })
}

/// `AddAudioClip` of asset `n` (offset 0, 0 dB, no fades).
pub fn add_audio_clip(device: &str, start: i64, length: i64, n: u8) -> Command {
    add_audio_clip_with(device, start, length, audio(n))
}

pub fn add_audio_clip_with(device: &str, start: i64, length: i64, a: AudioClipContent) -> Command {
    sc(SongCommand::AddAudioClip {
        device: key(device),
        start,
        length,
        asset: a.asset,
        source_offset: a.source_offset,
        gain_db: a.gain_db,
        fade_in: a.fade_in,
        fade_out: a.fade_out,
        fade_curve: a.fade_curve,
    })
}

/// An empty document with the inputs of a stereo interface and assets 1 and 2 registered.
pub fn with_assets(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    d.apply(register(1, 480_000), reg).unwrap();
    d.apply(register(2, 96_000), reg).unwrap();
    d
}

/// `with_assets` plus one audio track `audio-track`.
pub fn with_audio_track(reg: &Registry) -> Document {
    let mut d = with_assets(reg);
    d.apply(add_audio_track(), reg).unwrap();
    d
}

/// The id of the clip the last command added.
pub fn last(d: &Document) -> ClipId {
    d.last_clip_id().unwrap()
}

pub fn doc_clip(doc: &RackDoc, id: ClipId) -> &atomik_model::Clip {
    doc.song.clip(id).unwrap().2
}

// ---- random audio commands (project round trips) --------------------------------

/// An audio command in abstract form; indices wrap around the current audio tracks and clips.
#[derive(Debug, Clone)]
pub enum AudioOp {
    /// Registers asset 1 or 2 (`bad`: with a different length, which is rejected).
    Register(u8, bool),
    AddTrack(bool),
    AddClip(usize, ClipSpec),
    SetAudioClip(usize, ClipSpec, bool, Option<u64>),
    Inputs(u16),
}

/// Geometry and content of a random audio clip: start, length, asset 1 or 2, source offset,
/// gain, fades, equal power, and whether the asset is bogus.
#[derive(Debug, Clone, Copy)]
pub struct ClipSpec {
    pub start: i64,
    pub length: i64,
    pub asset: u8,
    pub source_offset: u64,
    pub gain_db: f32,
    pub fade_in: u64,
    pub fade_out: u64,
    pub equal_power: bool,
    pub bad_asset: bool,
}

impl ClipSpec {
    pub fn content(&self) -> AudioClipContent {
        AudioClipContent {
            asset: hash(if self.bad_asset { 99 } else { self.asset }),
            source_offset: self.source_offset,
            gain_db: self.gain_db,
            fade_in: self.fade_in,
            fade_out: self.fade_out,
            fade_curve: if self.equal_power {
                FadeCurve::EqualPower
            } else {
                FadeCurve::Linear
            },
        }
    }
}

fn clip_spec() -> impl Strategy<Value = ClipSpec> {
    (
        0i64..8 * super::song::BAR,
        1i64..4 * super::song::BAR,
        1u8..=2,
        0u64..500_000,
        -40f32..30f32,
        0u64..5_000,
        0u64..5_000,
        any::<bool>(),
        prop::bool::weighted(0.08),
    )
        .prop_map(|(s, l, a, o, g, fi, fo, eq, bad)| ClipSpec {
            start: s,
            length: l,
            asset: a,
            source_offset: o,
            gain_db: g,
            fade_in: fi,
            fade_out: fo,
            equal_power: eq,
            bad_asset: bad,
        })
}

pub fn audio_op() -> impl Strategy<Value = AudioOp> {
    let i = || 0usize..64;
    prop_oneof![
        1 => (1u8..=2, prop::bool::weighted(0.3)).prop_map(|(n, b)| AudioOp::Register(n, b)),
        3 => any::<bool>().prop_map(AudioOp::AddTrack),
        8 => (i(), clip_spec()).prop_map(|(t, c)| AudioOp::AddClip(t, c)),
        3 => (i(), clip_spec(), any::<bool>(), proptest::option::of(0u64..3))
            .prop_map(|(c, s, m, g)| AudioOp::SetAudioClip(c, s, m, g)),
        1 => (0u16..=34).prop_map(AudioOp::Inputs),
    ]
}

/// Resolves `op` into a command; `None` if it targets nothing.
pub fn resolve_audio(op: &AudioOp, doc: &RackDoc) -> Option<Command> {
    let tracks: Vec<_> = doc.song.tracks.iter().filter(|t| t.is_audio()).collect();
    Some(match op {
        AudioOp::Register(n, bad) => {
            // The pool only grows and registering is no undo step, so the runs that compare a
            // document with its undone self only register assets they started with (re-
            // registering them, or a conflicting length that is rejected).
            if doc.assets.is_empty() {
                return None;
            }
            register(*n, if *n == 1 { 480_000 } else { 96_000 } + u64::from(*bad))
        }
        AudioOp::AddTrack(named) => sc(SongCommand::AddAudioTrack {
            name: named.then(|| "Take".to_owned()),
        }),
        AudioOp::AddClip(t, c) => {
            if tracks.is_empty() {
                return None;
            }
            let t = tracks[t % tracks.len()];
            add_audio_clip_with(t.device.as_str(), c.start, c.length, c.content())
        }
        AudioOp::SetAudioClip(i, c, muted, merge) => {
            let ids: Vec<ClipId> = tracks
                .iter()
                .flat_map(|t| t.lanes[0].clips.iter().map(|c| c.id))
                .collect();
            if ids.is_empty() {
                return None;
            }
            sc(SongCommand::SetAudioClip {
                id: ids[i % ids.len()],
                start: c.start,
                length: c.length,
                muted: *muted,
                content: c.content(),
                merge: *merge,
            })
        }
        AudioOp::Inputs(n) => Command::SetInputChannels(*n),
    })
}

/// Registers assets 1 (480 000 frames) and 2 (96 000), which the random audio ops use. The
/// pool only grows, so the generators never register anything else.
pub fn pre_register(d: &mut Document, reg: &Registry) {
    d.apply(register(1, 480_000), reg).unwrap();
    d.apply(register(2, 96_000), reg).unwrap();
}

/// What every document must keep: audio clips live on audio lanes and name pooled
/// assets, audio tracks have one audio lane, interface inputs exist.
pub fn assert_audio_invariants(doc: &RackDoc) {
    assert!(doc.input_channels <= atomik_model::MAX_INPUT_CHANNELS);
    for t in &doc.song.tracks {
        if t.is_audio() {
            assert_eq!(t.lanes.len(), 1, "an audio track has a single lane");
        }
        for l in &t.lanes {
            for c in &l.clips {
                match &c.content {
                    ClipContent::Audio(a) => {
                        assert_eq!(l.kind, LaneKind::Audio);
                        let meta = doc.assets.get(&a.asset).expect("asset is pooled");
                        assert!(a.source_offset <= meta.frames);
                        assert!(a.gain_db.is_finite());
                        assert_eq!((c.offset, c.loop_length), (0, None));
                    }
                    _ => assert_ne!(l.kind, LaneKind::Audio),
                }
            }
        }
    }
    for c in &doc.cables {
        if c.from.device.0 == "interface" {
            let k: u16 = c.from.socket.strip_prefix("in_").unwrap().parse().unwrap();
            assert!((1..=doc.input_channels).contains(&k), "{c:?}");
        }
    }
    assert!(
        doc.referenced_assets()
            .iter()
            .all(|h| doc.assets.contains_key(h))
    );
}
