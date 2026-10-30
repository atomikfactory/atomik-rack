//! Song test helpers: clip builders and a random song/rack command generator resolved against
//! the current document (project round trips).

use super::*;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, LaneKind, Note, RackDoc, SongCommand,
    TempoEvent, Track,
};
use proptest::prelude::*;

pub const PPQ: i64 = atomik_core::PPQ;
pub const BAR: i64 = 4 * PPQ;

pub fn sc(c: SongCommand) -> Command {
    Command::Song(c)
}

pub fn note(tick: i64, length: i64, key: u8, velocity: f32) -> Note {
    Note::new(tick, length, key, velocity)
}

pub fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

pub fn clip(start: i64, length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content,
    }
}

pub fn notes_clip(start: i64, length: i64, notes: Vec<Note>) -> Clip {
    clip(start, length, ClipContent::Notes(notes))
}

pub fn auto_clip(start: i64, length: i64, points: Vec<AutomationPoint>) -> Clip {
    clip(start, length, ClipContent::Automation(points))
}

pub fn add_track(k: &str) -> Command {
    sc(SongCommand::AddTrack { device: key(k) })
}

pub fn add_clip(k: &str, lane: usize, c: Clip) -> Command {
    sc(SongCommand::AddClip {
        device: key(k),
        lane,
        clip: c,
    })
}

/// Every clip id in the song, in track/lane/clip order.
pub fn clip_ids(doc: &RackDoc) -> Vec<ClipId> {
    doc.song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter().map(|c| c.id))
        .collect()
}

/// The song/tempo invariant: the first tempo event mirrors the rack tempo.
pub fn assert_tempo_invariant(doc: &RackDoc) {
    assert!(!doc.song.tempo.is_empty());
    assert_eq!(doc.song.tempo[0].tick, 0);
    assert_eq!(
        doc.song.tempo[0].bpm.to_bits(),
        doc.tempo_bpm.to_bits(),
        "song.tempo[0].bpm mirrors tempo_bpm"
    );
}

/// An abstract song or rack command, resolved against the current document.
#[derive(Debug, Clone)]
pub enum SongOp {
    AddDevice(bool, Option<u8>),
    RemoveDevice(usize),
    Tempo(f64),
    AddTrack(usize),
    RemoveTrack(usize),
    Flags(usize, bool, bool, bool),
    AddLane(usize, usize),
    /// An automation lane (picked by index over all tracks) and what to do with it: 0 remove,
    /// 1 mute or unmute, 2 set a static value (the last number), 3 clear the static value.
    LaneOp(usize, u8, u8),
    AddClip(usize, usize, Geo, u64, bool),
    RemoveClip(usize),
    Geometry(usize, Geo, Option<u64>),
    Content(usize, u64, Option<u64>, bool),
    Split(usize, u16),
    Duplicate(usize, i64),
    TempoMap(Vec<(i64, f64)>),
    Meter(u8, u8),
    Loop(i64, i64, bool),
    End(i64),
    EndMerge,
}

/// Clip geometry: start, length, offset, loop length, mute.
#[derive(Debug, Clone, Copy)]
pub struct Geo(pub i64, pub i64, pub i64, pub Option<i64>, pub bool);

fn geo() -> impl Strategy<Value = Geo> {
    (
        0i64..8 * BAR,
        1i64..4 * BAR,
        0i64..BAR,
        proptest::option::of(1i64..2 * BAR),
        any::<bool>(),
    )
        .prop_map(|(s, l, o, lp, m)| Geo(s, l, o, lp, m))
}

pub fn song_op() -> impl Strategy<Value = SongOp> {
    let i = || 0usize..64;
    prop_oneof![
        2 => (any::<bool>(), proptest::option::of(0u8..4)).prop_map(|(s, k)| SongOp::AddDevice(s, k)),
        1 => i().prop_map(SongOp::RemoveDevice),
        1 => (20f64..=999f64).prop_map(SongOp::Tempo),
        3 => i().prop_map(SongOp::AddTrack),
        1 => i().prop_map(SongOp::RemoveTrack),
        1 => (i(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(t, a, b, c)| SongOp::Flags(t, a, b, c)),
        2 => (i(), i()).prop_map(|(t, p)| SongOp::AddLane(t, p)),
        2 => (i(), 0u8..4, any::<u8>()).prop_map(|(l, a, v)| SongOp::LaneOp(l, a, v)),
        6 => (i(), i(), geo(), any::<u64>(), prop::bool::weighted(0.1)).prop_map(|(t, l, g, s, bad)| SongOp::AddClip(t, l, g, s, bad)),
        1 => i().prop_map(SongOp::RemoveClip),
        3 => (i(), geo(), proptest::option::of(0u64..3)).prop_map(|(c, g, m)| SongOp::Geometry(c, g, m)),
        3 => (i(), any::<u64>(), proptest::option::of(0u64..3), prop::bool::weighted(0.1)).prop_map(|(c, s, m, bad)| SongOp::Content(c, s, m, bad)),
        2 => (i(), any::<u16>()).prop_map(|(c, f)| SongOp::Split(c, f)),
        2 => (i(), 0i64..16 * BAR).prop_map(|(c, s)| SongOp::Duplicate(c, s)),
        2 => (prop::bool::weighted(0.8), 10f64..1000f64, prop::collection::vec((0i64..16 * BAR, 10f64..1000f64), 0..3))
            .prop_map(|(at0, bpm, mut rest)| {
                if at0 {
                    rest.insert(0, (0, bpm));
                }
                SongOp::TempoMap(rest)
            }),
        1 => (0u8..34, 0u8..17).prop_map(|(b, u)| SongOp::Meter(b, u)),
        1 => (-BAR..8 * BAR, 0i64..8 * BAR, any::<bool>()).prop_map(|(s, e, en)| SongOp::Loop(s, e, en)),
        1 => (-1i64..32 * BAR).prop_map(SongOp::End),
        1 => Just(SongOp::EndMerge),
    ]
}

/// A tiny deterministic generator for clip content.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Random content for a lane kind; `bad` makes one value invalid (key 200 or a NaN value).
pub fn content(kind: &LaneKind, seed: u64, bad: bool) -> ClipContent {
    let mut r = Lcg(seed);
    let n = r.below(6) as usize;
    match kind {
        LaneKind::Notes => {
            let mut notes: Vec<Note> = (0..n)
                .map(|_| {
                    note(
                        r.below(2 * BAR as u64) as i64,
                        1 + r.below(BAR as u64) as i64,
                        r.below(128) as u8,
                        (1 + r.below(100)) as f32 / 100.0,
                    )
                })
                .collect();
            notes.sort_by_key(|n| (n.tick, n.key));
            notes.dedup_by_key(|n| (n.tick, n.key));
            if bad {
                notes.push(note(3 * BAR, 10, 200, 0.5));
            }
            ClipContent::Notes(notes)
        }
        LaneKind::Automation(_) => {
            let mut pts: Vec<AutomationPoint> = (0..n)
                .map(|_| {
                    let curve = if r.below(2) == 0 {
                        Curve::Linear
                    } else {
                        Curve::Step
                    };
                    // Values beyond any range are clamped by the descriptor.
                    point(
                        r.below(2 * BAR as u64) as i64,
                        r.below(40_000) as f32 - 10_000.0,
                        curve,
                    )
                })
                .collect();
            pts.sort_by_key(|p| p.tick);
            pts.dedup_by_key(|p| p.tick);
            if bad {
                pts.push(point(3 * BAR, f32::NAN, Curve::Step));
            }
            ClipContent::Automation(pts)
        }
        LaneKind::Audio => {
            // Assets 1 and 2 of `audio::with_assets`; `bad` names an asset that is not in the pool.
            let n = if bad { 99 } else { 1 + (seed % 2) as u8 };
            let mut a = super::audio::audio(n);
            a.source_offset = r.below(50_000);
            a.gain_db = r.below(60) as f32 - 36.0;
            a.fade_in = r.below(3_000);
            a.fade_out = r.below(3_000);
            if r.below(2) == 0 {
                a.fade_curve = atomik_model::FadeCurve::EqualPower;
            }
            ClipContent::Audio(a)
        }
    }
}

/// The clip ids paired with their track's lane kind.
fn clips_with_kind(doc: &RackDoc) -> Vec<(ClipId, LaneKind)> {
    doc.song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter().map(|c| (c.id, l.kind.clone())))
        .collect()
}

/// Resolves `op` into a command; `None` if it targets nothing.
pub fn resolve_song(
    op: &SongOp,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> Option<Command> {
    let n = doc.devices.len();
    let dev = |i: usize| (n > 0).then(|| doc.devices[i % n].key.clone());
    let nt = doc.song.tracks.len();
    let track = |i: usize| (nt > 0).then(|| &doc.song.tracks[i % nt]);
    let clips = clips_with_kind(doc);
    let clip_at = |i: usize| (!clips.is_empty()).then(|| clips[i % clips.len()].clone());
    Some(match op {
        SongOp::AddDevice(synth, k) => Command::AddDevice {
            index: n,
            type_id: if *synth { SYNTH } else { DELAY }.into(),
            key: k.map(|k| key(&format!("dev{k}"))),
        },
        SongOp::RemoveDevice(d) => Command::RemoveDevice { key: dev(*d)? },
        SongOp::Tempo(t) => Command::SetTempo(*t),
        SongOp::AddTrack(d) => sc(SongCommand::AddTrack { device: dev(*d)? }),
        SongOp::RemoveTrack(t) => sc(SongCommand::RemoveTrack {
            device: track(*t)?.device.clone(),
        }),
        SongOp::Flags(t, m, s, a) => sc(SongCommand::SetTrackFlags {
            device: track(*t)?.device.clone(),
            muted: *m,
            solo: *s,
            armed: *a,
        }),
        SongOp::AddLane(d, p) => {
            let k = dev(*d)?;
            let ty = &doc.device(&k).unwrap().type_id;
            let params = reg.get(ty).unwrap().descriptor.params;
            sc(SongCommand::AddAutomationLane {
                device: k,
                param: params[p % params.len()].key.into(),
            })
        }
        SongOp::LaneOp(i, action, v) => {
            let lanes: Vec<(&Track, usize, &str)> = doc
                .song
                .tracks
                .iter()
                .flat_map(|t| {
                    t.lanes
                        .iter()
                        .enumerate()
                        .filter_map(move |(l, lane)| match &lane.kind {
                            LaneKind::Automation(p) => Some((t, l, p.as_str())),
                            _ => None,
                        })
                })
                .collect();
            let (t, lane, param) = *lanes.get(i % lanes.len().max(1))?;
            let device = t.device.clone();
            let param = param.to_owned();
            match action {
                0 => sc(SongCommand::RemoveAutomationLane { device, param }),
                1 => sc(SongCommand::SetLaneMuted {
                    device,
                    lane,
                    muted: !t.lanes[lane].muted,
                }),
                2 => sc(SongCommand::SetLaneStatic {
                    device,
                    param,
                    value: Some(f32::from(*v) / 8.0),
                }),
                _ => sc(SongCommand::SetLaneStatic {
                    device,
                    param,
                    value: None,
                }),
            }
        }
        SongOp::AddClip(t, l, Geo(s, len, o, lp, m), seed, bad) => {
            let t = track(*t)?;
            let lane = l % t.lanes.len();
            sc(SongCommand::AddClip {
                device: t.device.clone(),
                lane,
                clip: Clip {
                    id: ClipId(0),
                    start: *s,
                    length: *len,
                    offset: *o,
                    loop_length: *lp,
                    muted: *m,
                    scale: None,
                    controllers: Vec::new(),
                    grid: None,
                    content: content(&t.lanes[lane].kind, *seed, *bad),
                },
            })
        }
        SongOp::RemoveClip(c) => sc(SongCommand::RemoveClip { id: clip_at(*c)?.0 }),
        SongOp::Geometry(c, Geo(s, len, o, lp, m), merge) => sc(SongCommand::SetClipGeometry {
            id: clip_at(*c)?.0,
            start: *s,
            length: *len,
            offset: *o,
            loop_length: *lp,
            muted: *m,
            merge: *merge,
        }),
        SongOp::Content(c, seed, merge, bad) => {
            let (id, kind) = clip_at(*c)?;
            sc(SongCommand::SetClipContent {
                id,
                content: content(&kind, *seed, *bad),
                merge: *merge,
            })
        }
        SongOp::Split(c, f) => {
            let (id, _) = clip_at(*c)?;
            let (_, _, cl) = doc.song.clip(id)?;
            sc(SongCommand::SplitClip {
                id,
                at: cl.start + (cl.length * i64::from(*f)) / 65_536,
            })
        }
        SongOp::Duplicate(c, s) => sc(SongCommand::DuplicateClip {
            id: clip_at(*c)?.0,
            start: *s,
        }),
        SongOp::TempoMap(events) => {
            let mut ev: Vec<TempoEvent> = events
                .iter()
                .map(|&(tick, bpm)| TempoEvent::step(tick, bpm))
                .collect();
            ev.sort_by_key(|e| e.tick);
            sc(SongCommand::SetTempoMap(ev))
        }
        SongOp::Meter(b, u) => sc(SongCommand::SetMeter {
            beats_per_bar: *b,
            beat_unit: *u,
        }),
        SongOp::Loop(s, e, en) => sc(SongCommand::SetLoop {
            range: (*s, *e),
            enabled: *en,
        }),
        SongOp::End(e) => sc(SongCommand::SetSongEnd(*e)),
        SongOp::EndMerge => return None,
    })
}

/// A document with the cabled chain (synth → delay → mixer) plus a second synth `lead`.
pub fn song_start(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = chain(reg);
    d.apply(add(3, SYNTH, Some("lead")), reg).unwrap();
    Document::new(d.doc().clone())
}
