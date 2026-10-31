//! Generated documents for tests, benchmarks and the stress commands: a rack of many devices, a
//! song with a few busy tracks, and a song of any size. Everything is built through model
//! commands, so every document is valid. Not used by the editor itself.

use crate::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    Document, INTERFACE_KEY, Note, RackDoc, SocketRef, SongCommand, console::is_console_type,
    empty_doc,
};
use atomik_core::PPQ;
use atomik_device_sdk::{Direction, Registry, SocketKind};

/// `registry` without the console devices (`atomik.channel`, `atomik.master`): documents
/// built with it get no automatic channels. The stress rack (a rack stress test, not a
/// console one) and the sync tests use it.
pub fn without_console(registry: &Registry) -> Registry {
    let mut out = Registry::new();
    for type_id in registry.type_ids().filter(|t| !is_console_type(t)) {
        if let Some(f) = registry.get(type_id) {
            // Already validated by `registry`, and the ids are unique there.
            let _ = out.register(*f);
        }
    }
    out
}

/// Devices in the rack.
pub const STRESS_DEVICES: usize = 100;
/// Cables in the rack.
pub const STRESS_CABLES: usize = 300;

/// A rack of `devices` devices (cycling synth, delay, synth, mixer) and exactly `cables`
/// cables when that many valid cables exist (fewer otherwise). Cables run from earlier to
/// later devices (no cycles), each input takes at most one, and the last mixer feeds the
/// interface. Built through model commands, so the result is a valid document.
///
/// It is a rack stress, not a console one: it is built without the console devices,
/// so its synths get no automatic channels and the counts stay exact.
pub fn stress_doc(registry: &Registry, devices: usize, cables: usize) -> RackDoc {
    let rack_only = without_console(registry);
    let registry = &rack_only;
    const TYPES: [&str; 4] = [
        "atomik.poly-synth",
        "atomik.stereo-delay",
        "atomik.poly-synth",
        "atomik.line-mixer",
    ];
    let mut doc = Document::new(empty_doc());
    let key = |i: usize| DeviceKey(format!("d{:03}", i + 1));
    for i in 0..devices {
        let command = Command::AddDevice {
            index: i,
            type_id: TYPES[i % TYPES.len()].to_string(),
            key: Some(key(i)),
        };
        // Registered types and fresh keys: always accepted.
        let _ = doc.apply(command, registry);
    }
    let sockets = |i: usize, direction: Direction| -> Vec<(String, SocketKind)> {
        registry
            .get(TYPES[i % TYPES.len()])
            .map(|f| {
                f.descriptor
                    .sockets
                    .iter()
                    .filter(|s| s.direction == direction)
                    .map(|s| (s.key.to_string(), s.kind))
                    .collect()
            })
            .unwrap_or_default()
    };
    let sref = |i: usize, socket: &str| SocketRef {
        device: key(i),
        socket: socket.to_string(),
    };
    let mut count = 0;
    // The last mixer feeds the interface.
    if let Some(m) = (0..devices)
        .rev()
        .find(|i| TYPES[i % TYPES.len()].ends_with("mixer"))
    {
        let outs = sockets(m, Direction::Output);
        for (out, n) in outs
            .iter()
            .filter(|(_, k)| *k == SocketKind::Audio)
            .zip(1..=2)
        {
            if count < cables {
                let cable = Cable {
                    from: sref(m, &out.0),
                    to: SocketRef {
                        device: DeviceKey(INTERFACE_KEY.to_string()),
                        socket: format!("out_{n}"),
                    },
                };
                if doc.apply(Command::Connect(cable), registry).is_ok() {
                    count += 1;
                }
            }
        }
    }
    // Every other input takes an output of the same kind from an earlier device (no cycles),
    // chosen round-robin for a mix of short and long cables.
    let mut pick = 0usize;
    for i in 1..devices {
        for (input, kind) in sockets(i, Direction::Input) {
            if count >= cables {
                break;
            }
            let sources: Vec<(usize, String)> = (0..i)
                .flat_map(|j| {
                    sockets(j, Direction::Output)
                        .into_iter()
                        .filter(|(_, k)| *k == kind)
                        .map(move |(s, _)| (j, s))
                })
                .collect();
            if sources.is_empty() {
                continue;
            }
            let (j, out) = &sources[pick % sources.len()];
            pick = pick.wrapping_mul(31).wrapping_add(17);
            let cable = Cable {
                from: sref(*j, out),
                to: sref(i, &input),
            };
            if doc.apply(Command::Connect(cable), registry).is_ok() {
                count += 1;
            }
        }
    }
    // The rack stress is a patch, which has no host inputs.
    RackDoc {
        input_channels: 0,
        ..doc.doc().clone()
    }
}

/// Tracks in the song.
pub const STRESS_SONG_TRACKS: usize = 8;
/// Bars in the song.
pub const STRESS_SONG_BARS: i64 = 64;
/// FX returns in the console stress (stereo delays on returns 1..=4).
pub const STRESS_SONG_RETURNS: usize = 4;
/// Send level of every stress channel, dB.
pub const STRESS_SEND_DB: f32 = -12.0;

/// The / stress project: `STRESS_SONG_TRACKS` poly synths (keys `s1`…`s8`), each on
/// a track with a 64-bar note clip (a 2-bar pattern looped, a different one per track) and two
/// automation lanes (`filter.cutoff` with linear ramps every 4 bars, `master.level` with step
/// changes every 8 bars).
///
/// Every synth plays through its automatic console channel into the master
/// (which feeds the interface); four stereo delays (`fx1`…`fx4`, wet only) sit on FX returns
/// 1–4, and channel `i` sends to return `i mod 4 + 1` at −12 dB. Built through model
/// commands, so it is valid. (With a registry without console devices the synths stay
/// unwired.)
pub fn stress_song_doc(registry: &Registry) -> RackDoc {
    let mut doc = Document::new(empty_doc());
    let apply = |doc: &mut Document, c: Command| {
        // Generated from known-good parts: always accepted.
        let applied = doc.apply(c, registry);
        debug_assert!(applied.is_ok(), "{applied:?}");
    };
    let key = |s: &str| DeviceKey(s.to_string());
    let bar = 4 * PPQ;
    let synths: Vec<String> = (1..=STRESS_SONG_TRACKS).map(|i| format!("s{i}")).collect();
    for k in &synths {
        apply(
            &mut doc,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            },
        );
    }
    if doc.doc().console.master.is_some() {
        for slot in 0..STRESS_SONG_RETURNS {
            let fx = key(&format!("fx{}", slot + 1));
            apply(
                &mut doc,
                Command::AddDevice {
                    index: usize::MAX,
                    type_id: "atomik.stereo-delay".into(),
                    key: Some(fx.clone()),
                },
            );
            apply(
                &mut doc,
                Command::SetParam {
                    key: fx.clone(),
                    param: "mix".into(),
                    value: 1.0,
                    merge: None,
                },
            );
            apply(
                &mut doc,
                Command::Console(ConsoleCommand::SetReturn {
                    slot,
                    effect: Some(fx),
                }),
            );
        }
        let channels: Vec<DeviceKey> = doc
            .doc()
            .console
            .channels
            .iter()
            .map(|c| c.channel.clone())
            .collect();
        for (i, ch) in channels.into_iter().enumerate() {
            let k = i % STRESS_SONG_RETURNS + 1;
            for (param, value) in [
                (format!("send{k}.on"), 1.0),
                (format!("send{k}.level"), STRESS_SEND_DB),
            ] {
                apply(
                    &mut doc,
                    Command::SetParam {
                        key: ch.clone(),
                        param,
                        value,
                        merge: None,
                    },
                );
            }
        }
    }
    let song = |doc: &mut Document, c: SongCommand| apply(doc, Command::Song(c));
    let length = STRESS_SONG_BARS * bar;
    let clip = |content: ClipContent, loop_length: i64| Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: Some(loop_length),
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content,
    };
    for (i, k) in synths.iter().enumerate() {
        song(&mut doc, SongCommand::AddTrack { device: key(k) });
        // A 2-bar pattern: eighth notes over a minor arpeggio, rotated per track.
        let root = 36 + 3 * i as u8;
        let steps = [0u8, 3, 7, 10, 12, 10, 7, 3];
        let notes = (0..16)
            .filter(|n| (n + i) % 3 != 2)
            .map(|n| {
                Note::new(
                    n as i64 * PPQ / 2,
                    PPQ / 2 - PPQ / 8,
                    root + steps[(n + i) % steps.len()],
                    0.5 + 0.05 * ((n + i) % 8) as f32,
                )
            })
            .collect();
        song(
            &mut doc,
            SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: clip(ClipContent::Notes(notes), 2 * bar),
            },
        );
        for (lane, (param, points, every)) in [
            (
                "filter.cutoff",
                vec![
                    (0, 300.0 + 100.0 * i as f32),
                    (2 * bar, 4000.0),
                    (4 * bar - 1, 600.0),
                ],
                4 * bar,
            ),
            ("master.level", vec![(0, -6.0), (4 * bar, -12.0)], 8 * bar),
        ]
        .into_iter()
        .enumerate()
        {
            song(
                &mut doc,
                SongCommand::AddAutomationLane {
                    device: key(k),
                    param: param.into(),
                },
            );
            let curve = if lane == 0 {
                Curve::Linear
            } else {
                Curve::Step
            };
            let points = points
                .into_iter()
                .map(|(tick, value)| AutomationPoint { tick, value, curve })
                .collect();
            song(
                &mut doc,
                SongCommand::AddClip {
                    device: key(k),
                    lane: lane + 1,
                    clip: clip(ClipContent::Automation(points), every),
                },
            );
        }
    }
    song(&mut doc, SongCommand::SetSongEnd(length));
    song(
        &mut doc,
        SongCommand::SetLoop {
            range: (0, length),
            enabled: true,
        },
    );
    doc.doc().clone()
}

/// A song of `tracks` poly-synth tracks (keys `t001`…) holding `clips` note clips in all, dealt
/// round-robin over the tracks, and `notes` notes in all, dealt round-robin over the clips (a
/// clip may stay empty when there are fewer notes than clips). Clips of a track follow each
/// other, one bar apart, and the notes of a clip sit on a sixteenth-note grid over minor-scale
/// pitches, so the song is dense but always playable. Built through model commands.
///
/// With a registry that has the console devices every track also gets its console channel.
pub fn large_song_doc(registry: &Registry, tracks: usize, clips: usize, notes: usize) -> RackDoc {
    let mut doc = Document::new(empty_doc());
    let apply = |doc: &mut Document, c: Command| {
        // Generated from known-good parts: always accepted.
        let applied = doc.apply(c, registry);
        debug_assert!(applied.is_ok(), "{applied:?}");
    };
    let bar = 4 * PPQ;
    let key = |i: usize| DeviceKey(format!("t{:03}", i + 1));
    for i in 0..tracks {
        apply(
            &mut doc,
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(i)),
            },
        );
        apply(
            &mut doc,
            Command::Song(SongCommand::AddTrack { device: key(i) }),
        );
    }
    if tracks == 0 {
        return doc.doc().clone();
    }
    // Notes per clip (only asked for when `clips > 0`): the first `notes % clips` clips take one more.
    let per_clip = |c: usize| notes / clips.max(1) + usize::from(c < notes % clips.max(1));
    let steps = [0u8, 3, 7, 10, 12, 10, 7, 3];
    for c in 0..clips {
        let (track, nth) = (c % tracks, c / tracks);
        let count = per_clip(c);
        let list: Vec<Note> = (0..count)
            .map(|n| {
                Note::new(
                    n as i64 * PPQ / 4,
                    PPQ / 4 - PPQ / 16,
                    36 + steps[(n + c) % steps.len()] + (n / steps.len() % 3) as u8 * 12,
                    0.5 + 0.05 * ((n + c) % 8) as f32,
                )
            })
            .collect();
        let length = (count as i64 * PPQ / 4).max(bar);
        let length = (length + bar - 1) / bar * bar;
        apply(
            &mut doc,
            Command::Song(SongCommand::AddClip {
                device: key(track),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: nth as i64 * length,
                    length,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    controllers: Vec::new(),
                    grid: None,
                    content: ClipContent::Notes(list),
                },
            }),
        );
    }
    doc.doc().clone()
}

/// A console of `strips` channel strips (one poly-synth track each, every strip playing a
/// looping pattern) with the gate and the compressor of every strip on. Needs a registry with
/// the console devices; without them there are no strips and the result is the bare song.
pub fn console_doc(registry: &Registry, strips: usize) -> RackDoc {
    let mut doc = Document::new(large_song_doc(registry, strips, strips, strips * 8));
    let channels: Vec<DeviceKey> = doc
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .collect();
    for ch in channels {
        for param in ["gate.on", "comp.on"] {
            let applied = doc.apply(
                Command::SetParam {
                    key: ch.clone(),
                    param: param.into(),
                    value: 1.0,
                    merge: None,
                },
                registry,
            );
            debug_assert!(applied.is_ok(), "{applied:?}");
        }
    }
    doc.doc().clone()
}
