//! The example projects of the factory library (test code): three short songs as tables, and the
//! writer that turns each into a project folder `Projects/<name>/<name>.atomik` (with the audio
//! of a sampled drum kit in `Audio/` when it has one), exactly as Save As writes one. See
//! `app_core_factory_build.rs` for how the tables become files.
//!
//! An example is built from what the factory already holds: its instruments are factory patches,
//! factory containers (modular instruments) or the drum machine with a kit's sound, its effects
//! are factory effect containers, and its notes are factory clips, placed and looped on the
//! tracks, plus a few short patterns of its own. Nothing in it needs a plugin or a file outside
//! the folder. Each example shows a different side of the program:
//!
//! * a beat: a sampled drum kit with pattern clips, a bass line, keys and a mallet hook, with
//!   effect containers as channel inserts and a mix-bus finish;
//! * a modular piece: the factory modular instruments (a chord-and-arpeggio stack, an
//!   envelope-swept pad, a wandering lead), a drone and a sequenced pulse, two send effects, and
//!   a macro of a container moved by automation;
//! * a mix: drum pads on their own channels, a bus for the drums, a track folder that plays into
//!   a bus, group levels, send effects, a mix-bus finish and automation of a filter, a fader and
//!   a group level.

use super::clips::{Pattern, clip_of};
use super::kits::{Built as KitBuilt, DRUM, KitDef};
use super::{
    CLIPS, CONTAINERS, ClipDef, DEFS, Def, KITS, container_text, run, set_mods, set_params,
};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::console::CHANNEL_TYPE;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ConsoleCommand, Curve, DeviceKey,
    Document, InsertEffect, LaneKind, SongCommand, SongInfo, WorkflowCommand, load_device_item,
    new_project_doc, save_project_checked,
};
use atomik_render::default_registry;
use std::path::Path;

/// What makes the sound of a voice.
pub(in crate::app_core) enum Source {
    /// A factory patch (by name) on a new device of its type.
    Patch(&'static str),
    /// A factory drum kit with its samples: a drum machine whose pads play them (the audio goes
    /// into the project's `Audio` folder).
    Kit(&'static str),
    /// The pads of a factory drum kit as the drum machine's own synthesised modules, with no
    /// audio, and the kick, snare and closed hat on channels of their own.
    Pads(&'static str),
    /// A factory container that is an instrument (the modular ones).
    Container(&'static str),
}

/// A clip on a track: `clip` (a factory clip or one of the example's own) from bar `bar` (the
/// first bar is 1), looped to fill `bars` bars.
pub(in crate::app_core) struct Place {
    pub clip: &'static str,
    pub bar: i64,
    pub bars: i64,
    /// Which notes of the clip play.
    pub keys: Keys,
}

/// The key of the kick in the factory drum clips (and the drum machine's first pad).
pub(in crate::app_core) const KICK_KEY: u8 = 36;

/// Which notes of a clip a [`Place`] plays: a drum clip can feed the kick and the rest of the kit
/// to two tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app_core) enum Keys {
    /// Every note.
    All,
    /// The kick only.
    Kick,
    /// Everything but the kick.
    Rest,
}

impl Keys {
    /// Whether a note of `key` plays.
    pub(in crate::app_core) fn takes(self, key: u8) -> bool {
        match self {
            Keys::All => true,
            Keys::Kick => key == KICK_KEY,
            Keys::Rest => key != KICK_KEY,
        }
    }
}

pub(in crate::app_core) const fn place(clip: &'static str, bar: i64, bars: i64) -> Place {
    Place {
        clip,
        bar,
        bars,
        keys: Keys::All,
    }
}

/// [`place`] with the kick of the clip only.
pub(in crate::app_core) const fn kick(clip: &'static str, bar: i64, bars: i64) -> Place {
    Place {
        clip,
        bar,
        bars,
        keys: Keys::Kick,
    }
}

/// [`place`] with the clip without its kick.
pub(in crate::app_core) const fn rest(clip: &'static str, bar: i64, bars: i64) -> Place {
    Place {
        clip,
        bar,
        bars,
        keys: Keys::Rest,
    }
}

/// One track of an example: an instrument with its channel and its clips.
pub(in crate::app_core) struct Voice {
    /// The name the track shows (its device key is the name in lower case).
    pub name: &'static str,
    pub source: Source,
    /// The channel fader in dB, and its pan.
    pub level: f32,
    pub pan: f32,
    /// Factory effect containers in the channel's insert chain, in order.
    pub inserts: &'static [&'static str],
    /// `(send number from 1, level in dB)`.
    pub sends: &'static [(u8, f32)],
    pub clips: &'static [Place],
}

/// One example project.
pub(in crate::app_core) struct ExampleDef {
    /// The project name (and the folder and file name).
    pub name: &'static str,
    /// Words to search and filter by.
    pub tags: &'static [&'static str],
    /// What it sounds like, in a line or two (the notes of the song).
    pub sound: &'static str,
    /// What it shows of the program, in a line.
    pub shows: &'static str,
    pub bpm: f64,
    pub bars: i64,
    /// The patterns of its own, besides the factory clips.
    pub clips: &'static [ClipDef],
    pub voices: &'static [Voice],
    /// The rest of the mix: returns, buses, groups, the master and the automation.
    pub mix: fn(&mut Build),
}

/// The category the browser shows for an example.
pub(in crate::app_core) const CATEGORY: &str = "Example";

// ---------------------------------------------------------------------------------- the songs

pub(in crate::app_core) const EXAMPLES: &[ExampleDef] = &[
    ExampleDef {
        name: "Basalt Beat",
        tags: &["beat", "drums", "kit", "minor"],
        sound: "A slow, broken beat in A minor at 96 BPM: a deep sampled kit with a syncopated \
                kick and ghosted snare, a round walking sub, a dry clavier on four chords and a \
                wooden mallet hook that comes in halfway. Sixteen bars of sparse hats, the full \
                groove and a heavier half-time ending.",
        shows: "The drum kits and pattern clips: a sampled kit, drum patterns looped over bars, \
                effect containers as channel inserts and on the master.",
        bpm: 96.0,
        bars: 16,
        clips: &[
            ClipDef {
                name: "Beat Intro",
                category: "Drums",
                tags: &[],
                sound: "Quiet kicks, rim clicks and steady hats: the beat before it starts.",
                bpm: 96.0,
                bars: 2,
                pattern: Pattern::Drums {
                    step: 0.25,
                    lanes: &[
                        (36, "o...............o.......o......."),
                        (37, "..x.......x.......x.......x...x."),
                        (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x."),
                    ],
                },
            },
            ClipDef {
                name: "Mallet Hook",
                category: "Lead",
                tags: &[],
                sound: "A pentatonic hook in A minor, four bars.",
                bpm: 96.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.9,
                    keys: &[
                        69, 0, 72, 0, 74, 0, 72, 69, 0, 0, 76, 0, 74, 72, 0, 0, 69, 0, 72, 0, 74,
                        0, 76, 74, 72, 0, 69, 0, 67, 0, 0, 0,
                    ],
                    accents: "Xxxx",
                },
            },
        ],
        voices: &[
            Voice {
                name: "Drums",
                source: Source::Kit("Basalt Kit"),
                level: -1.5,
                pan: 0.0,
                inserts: &["Drum Punch"],
                sends: &[],
                clips: &[
                    place("Beat Intro", 1, 4),
                    place("Broken Beat", 5, 8),
                    place("Half-Time Groove", 13, 4),
                ],
            },
            Voice {
                name: "Sub",
                source: Source::Patch("Basalt Sub"),
                level: -13.0,
                pan: 0.0,
                inserts: &[],
                sends: &[],
                clips: &[place("Walking Minor", 5, 12)],
            },
            Voice {
                name: "Keys",
                source: Source::Patch("Dusk Clavier"),
                level: -3.0,
                pan: -0.2,
                inserts: &["Tape Warmth"],
                sends: &[(1, -12.0)],
                clips: &[place("Minor Four Chords", 1, 16)],
            },
            Voice {
                name: "Mallet",
                source: Source::Patch("Pebble Mallet"),
                level: -3.0,
                pan: 0.25,
                inserts: &[],
                sends: &[(1, -6.0)],
                clips: &[place("Mallet Hook", 9, 8)],
            },
        ],
        mix: beat_mix,
    },
    ExampleDef {
        name: "Moonrise Modular",
        tags: &["modular", "ambient", "arpeggio", "generative"],
        sound: "A slow, spacious piece in A minor at 84 BPM built from the modular instruments: \
                a pad that swells open on each chord, a deep drone, an arpeggio that climbs the \
                seventh chords by itself, a sequenced pulse and a lead that wanders through the \
                scale on its own, with a shimmering reverb and a ping-pong echo.",
        shows: "The modular side: instruments made of containers with CV cables, a semi-modular \
                synth with its sequencer, two send effects and a container macro moved by \
                automation.",
        bpm: 84.0,
        bars: 16,
        clips: &[
            ClipDef {
                name: "Arp Roots",
                category: "Chords",
                tags: &[],
                sound: "One root per bar: A, F, C and G, held.",
                bpm: 84.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.8,
                    changes: &[
                        (0.0, 4.0, &[45]),
                        (4.0, 4.0, &[41]),
                        (8.0, 4.0, &[48]),
                        (12.0, 4.0, &[43]),
                    ],
                },
            },
            ClipDef {
                name: "Sub Roots",
                category: "Bass",
                tags: &[],
                sound: "Low roots of the same four chords, held.",
                bpm: 84.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.8,
                    changes: &[
                        (0.0, 4.0, &[33]),
                        (4.0, 4.0, &[29]),
                        (8.0, 4.0, &[36]),
                        (12.0, 4.0, &[31]),
                    ],
                },
            },
            ClipDef {
                name: "Wander Holds",
                category: "Lead",
                tags: &[],
                sound: "Three long notes for the wandering lead to start from.",
                bpm: 84.0,
                bars: 4,
                pattern: Pattern::Chords {
                    velocity: 0.7,
                    changes: &[(0.0, 8.0, &[57]), (8.0, 4.0, &[60]), (12.0, 4.0, &[55])],
                },
            },
        ],
        voices: &[
            Voice {
                name: "Drone",
                source: Source::Patch("Fold Drone"),
                level: -9.5,
                pan: 0.0,
                inserts: &[],
                sends: &[],
                clips: &[place("Sub Roots", 1, 16)],
            },
            Voice {
                name: "Sweep Pad",
                source: Source::Container("Envelope Sweep Pad"),
                level: -2.5,
                pan: 0.0,
                inserts: &[],
                sends: &[(1, -9.0)],
                clips: &[place("Minor Four Chords", 1, 16)],
            },
            Voice {
                name: "Arp Stack",
                source: Source::Container("Chord Arp Stack"),
                level: 0.5,
                pan: -0.25,
                inserts: &[],
                sends: &[(2, -7.0)],
                clips: &[place("Arp Roots", 5, 12)],
            },
            Voice {
                name: "Pulse",
                source: Source::Patch("Sequenced Pulse"),
                level: -2.0,
                pan: 0.2,
                inserts: &[],
                sends: &[(2, -10.0)],
                clips: &[place("Arp Roots", 9, 8)],
            },
            Voice {
                name: "Wanderer",
                source: Source::Container("Random Walk Lead"),
                level: -1.0,
                pan: 0.3,
                inserts: &[],
                sends: &[(1, -8.0), (2, -6.0)],
                clips: &[place("Wander Holds", 9, 8)],
            },
        ],
        mix: modular_mix,
    },
    ExampleDef {
        name: "Bus Room",
        tags: &["mix", "buses", "groups", "automation"],
        sound: "A steady four-on-the-floor tune in A minor at 118 BPM, built to be mixed: \
                synthesised drums, a gritty bass, a soft electric keys arpeggio, a slow pad and \
                a reedy lead. The beat ducks for a breakdown, the pad swells in and the lead \
                opens its filter, all by automation.",
        shows: "The mixing side: drum pads on their own channels into a Drums bus, a track folder \
                that plays into a Music bus, group levels, two send effects, a mix-bus finish and \
                automation of a filter, a fader and a group level.",
        bpm: 118.0,
        bars: 24,
        clips: &[
            ClipDef {
                name: "Chord Pump",
                category: "Bass",
                tags: &[],
                sound: "Eighth-note bass on A, F, C and G with an octave jump, four bars.",
                bpm: 118.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.8,
                    keys: &[
                        33, 33, 45, 33, 33, 45, 33, 0, 29, 29, 41, 29, 29, 41, 29, 0, 36, 36, 48,
                        36, 36, 48, 36, 0, 31, 31, 43, 31, 31, 43, 31, 0,
                    ],
                    accents: "Xxxx",
                },
            },
            ClipDef {
                name: "Reed Line",
                category: "Lead",
                tags: &[],
                sound: "A slow pentatonic line over the four chords.",
                bpm: 118.0,
                bars: 4,
                pattern: Pattern::Line {
                    step: 0.5,
                    gate: 0.95,
                    keys: &[
                        69, 0, 72, 0, 76, 0, 72, 0, 72, 0, 69, 0, 65, 0, 69, 0, 67, 0, 72, 0, 76,
                        0, 79, 0, 74, 0, 71, 0, 67, 0, 0, 0,
                    ],
                    accents: "Xxxx",
                },
            },
        ],
        voices: &[
            Voice {
                name: "Drums",
                source: Source::Pads("Static Kit"),
                level: 2.5,
                pan: 0.0,
                inserts: &[],
                sends: &[],
                clips: &[place("Four On The Floor", 5, 20)],
            },
            Voice {
                name: "Bass",
                source: Source::Patch("Gravel Drive"),
                level: -18.0,
                pan: 0.0,
                inserts: &[],
                sends: &[],
                clips: &[place("Chord Pump", 9, 16)],
            },
            Voice {
                name: "Tines",
                source: Source::Patch("Velvet Tines"),
                level: -18.5,
                pan: -0.25,
                inserts: &[],
                sends: &[(2, -10.0)],
                clips: &[place("Wide Minor Arp", 5, 20)],
            },
            Voice {
                name: "Pad",
                source: Source::Patch("Aurora Stack"),
                level: -23.5,
                pan: 0.0,
                inserts: &[],
                sends: &[(1, -9.0)],
                clips: &[place("Minor Four Chords", 1, 24)],
            },
            Voice {
                name: "Reed",
                source: Source::Patch("Hollow Reed Lead"),
                level: -26.0,
                pan: 0.2,
                inserts: &[],
                sends: &[(1, -10.0), (2, -7.0)],
                clips: &[place("Reed Line", 13, 12)],
            },
        ],
        mix: room_mix,
    },
];

// ----------------------------------------------------------------------------------- the mixes

fn beat_mix(b: &mut Build) {
    b.set_return(0, "Ping Pong Tide");
    b.master_insert("Bus Glue");
}

fn modular_mix(b: &mut Build) {
    b.set_return(0, "Shimmer Cathedral");
    b.set_return(1, "Ping Pong Tide");
    // The arpeggio's cutoff macro opens over eight bars and settles back.
    let arp = b.key("Arp Stack");
    b.automate(&arp, "macro2", &[(5.0, 0.15), (13.0, 0.9), (17.0, 0.45)]);
}

fn room_mix(b: &mut Build) {
    b.set_return(0, "Shimmer Cathedral");
    b.set_return(1, "Ping Pong Tide");
    let drums = b.key("Drums");
    let main = b.channel(&drums);
    // The kick, the snare and the closed hat get channels of their own.
    let mut pad_channels = Vec::new();
    for (pad, name) in [(1u8, "Kick"), (3, "Snare"), (7, "Hats")] {
        b.run(Command::Console(ConsoleCommand::AddChannelFor {
            source: drums.clone(),
            out_l: format!("pad{pad}_out_l"),
            out_r: format!("pad{pad}_out_r"),
            name: Some(name.to_owned()),
        }));
        let channel = b.last_channel();
        b.set_channel(&channel, 2.5, 0.0);
        pad_channels.push(channel);
    }
    b.set_channel(&main, 2.5, 0.0);
    let mut strips = pad_channels.clone();
    strips.push(main);
    b.run(Command::Console(ConsoleCommand::RouteToNewBus {
        strips,
        name: "Drums".to_owned(),
    }));
    let drum_bus = b.last_bus();
    b.insert(&drum_bus, 0, "Drum Punch");
    // The music tracks go in a folder that plays into a bus of its own.
    let music: Vec<DeviceKey> = ["Bass", "Tines", "Pad", "Reed"]
        .iter()
        .map(|n| b.key(n))
        .collect();
    b.run(Command::Song(SongCommand::GroupTracks {
        devices: music.clone(),
        name: "Music".to_owned(),
    }));
    let folder = b
        .doc
        .doc()
        .song
        .org
        .groups
        .last()
        .map(|g| g.id)
        .expect("the folder");
    b.run(Command::Console(ConsoleCommand::RouteFolderToNewBus {
        folder,
        name: "Music".to_owned(),
    }));
    let music_bus = b.last_bus();
    b.insert(&music_bus, 0, "Tape Warmth");
    // Two groups: the beat (the drum bus and the bass) and the harmony (tines and pad).
    let bass = b.channel(&b.key("Bass"));
    b.run(Command::Console(ConsoleCommand::CreateVca {
        name: "Beat".to_owned(),
        members: vec![drum_bus, bass],
    }));
    let beat = b.last_vca();
    let harmony: Vec<DeviceKey> = ["Tines", "Pad"]
        .iter()
        .map(|n| b.channel(&b.key(n)))
        .collect();
    b.run(Command::Console(ConsoleCommand::CreateVca {
        name: "Harmony".to_owned(),
        members: harmony,
    }));
    b.master_insert("Bus Glue");
    // The beat ducks for the breakdown (bars 17 to 20) and comes back.
    let master = b.master();
    b.automate_db(
        &master,
        &format!("vca.{beat}"),
        &[(16.0, 0.0), (17.0, -18.0), (21.0, -18.0), (21.5, 0.0)],
    );
    // The pad swells in over the first eight bars.
    let pad = b.channel(&b.key("Pad"));
    b.automate_db(&pad, "fader", &[(1.0, -30.0), (9.0, -23.5)]);
    // The lead opens its filter from its entrance to the end.
    let reed = b.key("Reed");
    b.automate(&reed, "filter.cutoff", &[(13.0, 500.0), (25.0, 5500.0)]);
}

// ---------------------------------------------------------------------------------- the builder

/// The project being built.
pub(in crate::app_core) struct Build {
    pub(in crate::app_core) doc: Document,
    pub(in crate::app_core) registry: Registry,
    pub(in crate::app_core) item: &'static str,
    /// The audio of the sampled kits the project uses, by asset id.
    pub(in crate::app_core) audio: Vec<(String, AudioData)>,
}

pub(in crate::app_core) fn beats_ticks(beats: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)] // a few bars of beats
    let t = (beats * PPQ as f64).round() as i64;
    t
}

/// The tick where bar `bar` (the first is 1) starts, in 4/4.
pub(in crate::app_core) fn bar_tick(bar: f64) -> i64 {
    beats_ticks((bar - 1.0) * 4.0)
}

impl Build {
    pub(in crate::app_core) fn run(&mut self, command: Command) {
        run(&mut self.doc, &self.registry, self.item, command);
    }

    pub(in crate::app_core) fn keys(&self) -> Vec<DeviceKey> {
        self.doc
            .doc()
            .devices
            .iter()
            .map(|d| d.key.clone())
            .collect()
    }

    /// The key of the instrument of the voice `name`.
    pub(in crate::app_core) fn key(&self, name: &str) -> DeviceKey {
        self.doc
            .doc()
            .devices
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.key.clone())
            .unwrap_or_else(|| panic!("{}: no device named {name}", self.item))
    }

    pub(in crate::app_core) fn master(&self) -> DeviceKey {
        self.doc
            .doc()
            .console
            .master
            .clone()
            .unwrap_or_else(|| panic!("{}: no master", self.item))
    }

    /// The first channel that `source` feeds.
    pub(in crate::app_core) fn channel(&self, source: &DeviceKey) -> DeviceKey {
        self.doc
            .doc()
            .console
            .channel_of(source)
            .map(|c| c.channel.clone())
            .unwrap_or_else(|| panic!("{}: {source} has no channel", self.item))
    }

    pub(in crate::app_core) fn last_channel(&self) -> DeviceKey {
        self.doc
            .doc()
            .console
            .channels
            .last()
            .map(|c| c.channel.clone())
            .expect("a channel")
    }

    pub(in crate::app_core) fn last_bus(&self) -> DeviceKey {
        self.doc
            .doc()
            .console
            .buses
            .last()
            .map(|b| b.bus.clone())
            .expect("a bus")
    }

    pub(in crate::app_core) fn last_vca(&self) -> u32 {
        self.doc
            .doc()
            .console
            .vcas
            .last()
            .map(|g| g.id)
            .expect("a group")
    }

    pub(in crate::app_core) fn set_channel(&mut self, channel: &DeviceKey, db: f32, pan: f32) {
        let fader = atomik_dsp::fader_pos_for_db(db);
        set_params(
            &mut self.doc,
            &self.registry,
            self.item,
            channel,
            CHANNEL_TYPE,
            &[("fader", fader), ("pan", pan)],
        );
    }

    /// Pastes the item `text` and returns the devices it added, in rack order.
    pub(in crate::app_core) fn paste(&mut self, text: &str, route: bool) -> Vec<DeviceKey> {
        let snapshot =
            load_device_item(text, &self.registry).unwrap_or_else(|e| panic!("{}: {e}", self.item));
        for (hash, meta) in &snapshot.assets {
            if !self.doc.doc().assets.contains_key(hash) {
                self.run(Command::RegisterAsset {
                    hash: hash.clone(),
                    meta: meta.clone(),
                });
            }
        }
        let before = self.keys();
        self.run(Command::Workflow(WorkflowCommand::PasteDevices {
            snapshot,
            container: None,
            at: usize::MAX,
            route,
        }));
        self.keys()
            .into_iter()
            .filter(|k| !before.contains(k))
            .collect()
    }

    /// The factory effect container `name`, added to the rack.
    pub(in crate::app_core) fn effect(&mut self, name: &str) -> DeviceKey {
        let def = CONTAINERS
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("{}: no factory container {name}", self.item));
        assert!(def.chain, "{}: {name} is not an effect chain", self.item);
        self.paste(&container_text(def), false)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{}: {name} added nothing", self.item))
    }

    pub(in crate::app_core) fn insert(&mut self, strip: &DeviceKey, slot: usize, name: &str) {
        let effect = self.effect(name);
        self.run(Command::Console(ConsoleCommand::SetInsert {
            channel: strip.clone(),
            slot,
            effect: Some(InsertEffect::Existing(effect)),
        }));
    }

    pub(in crate::app_core) fn master_insert(&mut self, name: &str) {
        let master = self.master();
        self.insert(&master, 0, name);
    }

    pub(in crate::app_core) fn set_return(&mut self, slot: usize, name: &str) {
        let effect = self.effect(name);
        self.run(Command::Console(ConsoleCommand::SetReturn {
            slot,
            effect: Some(effect),
        }));
    }

    /// An automation lane of `param` on `device`: `(bar, value)` points, linear between.
    pub(in crate::app_core) fn automate(
        &mut self,
        device: &DeviceKey,
        param: &str,
        points: &[(f64, f32)],
    ) {
        self.lane(device, param, points);
    }

    /// An automation lane of a fader or a group in dB.
    pub(in crate::app_core) fn automate_db(
        &mut self,
        device: &DeviceKey,
        param: &str,
        points: &[(f64, f32)],
    ) {
        let to_value = |db: f32| {
            if param == "fader" {
                atomik_dsp::fader_pos_for_db(db)
            } else {
                db
            }
        };
        let points: Vec<(f64, f32)> = points
            .iter()
            .map(|&(bar, db)| (bar, to_value(db)))
            .collect();
        self.lane(device, param, &points);
    }

    pub(in crate::app_core) fn lane(
        &mut self,
        device: &DeviceKey,
        param: &str,
        points: &[(f64, f32)],
    ) {
        self.run(Command::Song(SongCommand::AddAutomationLane {
            device: device.clone(),
            param: param.to_owned(),
        }));
        let lane = self
            .doc
            .doc()
            .song
            .track(device)
            .and_then(|t| t.automation_lane(param))
            .unwrap_or_else(|| panic!("{}: no lane for {param}", self.item));
        let first = points.first().map(|p| p.0).expect("a point");
        let last = points.last().map(|p| p.0).expect("a point");
        let start = bar_tick(first);
        let content: Vec<AutomationPoint> = points
            .iter()
            .map(|&(bar, value)| AutomationPoint {
                tick: bar_tick(bar) - start,
                value,
                curve: Curve::Linear,
            })
            .collect();
        let clip = Clip {
            id: ClipId(0),
            start,
            length: bar_tick(last) - start,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Automation(content),
        };
        self.run(Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane,
            clip,
        }));
        debug_assert!(matches!(
            self.doc
                .doc()
                .song
                .track(device)
                .map(|t| &t.lanes[lane].kind),
            Some(LaneKind::Automation(_))
        ));
    }

    /// A device of the patch `def` on a new device of its type.
    pub(in crate::app_core) fn patch(&mut self, key: &DeviceKey, def: &Def) {
        self.run(Command::AddDevice {
            index: usize::MAX,
            type_id: def.device.to_owned(),
            key: Some(key.clone()),
        });
        set_params(
            &mut self.doc,
            &self.registry,
            self.item,
            key,
            def.device,
            def.params,
        );
        set_mods(
            &mut self.doc,
            &self.registry,
            self.item,
            key,
            def.device,
            def.mods,
        );
        self.run(Command::Workflow(WorkflowCommand::SetPatchName {
            device: key.clone(),
            name: def.name.to_owned(),
        }));
    }

    /// The drum machine `key` with the synthesised pads of `kit`.
    pub(in crate::app_core) fn pads(&mut self, key: &DeviceKey, kit: &KitDef) {
        self.run(Command::AddDevice {
            index: usize::MAX,
            type_id: DRUM.to_owned(),
            key: Some(key.clone()),
        });
        let mut params: Vec<(String, f32)> = vec![("master.level".to_owned(), -1.0)];
        for hit in kit.hits {
            let pad = |name: &str| format!("pad{}.{name}", hit.pad);
            params.push((pad("module"), f32::from(hit.module)));
            params.push((pad("level"), hit.level));
            params.push((pad("pan"), hit.pan));
            params.push((pad("choke"), f32::from(hit.choke)));
            for &(name, value) in hit.params {
                params.push((pad(name), value));
            }
        }
        let borrowed: Vec<(&str, f32)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        set_params(
            &mut self.doc,
            &self.registry,
            self.item,
            key,
            DRUM,
            &borrowed,
        );
    }

    /// Adds the instrument of `voice` (its channel, its track and its clips) and returns its key.
    pub(in crate::app_core) fn voice(
        &mut self,
        own: &[ClipDef],
        bars: i64,
        voice: &Voice,
        kits: &[(&str, KitBuilt)],
    ) {
        let key = match &voice.source {
            Source::Patch(name) => {
                let patch = DEFS
                    .iter()
                    .find(|d| d.name == *name)
                    .unwrap_or_else(|| panic!("{}: no factory patch {name}", self.item));
                let key = DeviceKey(slug(voice.name));
                self.patch(&key, patch);
                key
            }
            Source::Pads(name) => {
                let kit = kit_named(name);
                let key = DeviceKey(slug(voice.name));
                self.pads(&key, kit);
                key
            }
            Source::Kit(name) => {
                let built = &kits
                    .iter()
                    .find(|(n, _)| n == name)
                    .unwrap_or_else(|| panic!("{}: kit {name} was not built", self.item))
                    .1;
                for (hash, data) in &built.audio {
                    if !self.audio.iter().any(|(h, _)| h == hash) {
                        self.audio.push((hash.clone(), data.clone()));
                    }
                }
                self.paste(&built.text, true)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("{}: kit {name} added nothing", self.item))
            }
            Source::Container(name) => {
                let container = CONTAINERS
                    .iter()
                    .find(|c| c.name == *name)
                    .unwrap_or_else(|| panic!("{}: no factory container {name}", self.item));
                assert!(!container.chain, "{}: {name} is an effect", self.item);
                self.paste(&container_text(container), true)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("{}: {name} added nothing", self.item))
            }
        };
        self.run(Command::Rename {
            key: key.clone(),
            name: voice.name.to_owned(),
        });
        if self.doc.doc().console.channel_of(&key).is_none() {
            self.run(Command::Console(ConsoleCommand::AddChannel {
                source: key.clone(),
            }));
        }
        let channel = self.channel(&key);
        self.set_channel(&channel, voice.level, voice.pan);
        for (slot, name) in voice.inserts.iter().enumerate() {
            self.insert(&channel, slot, name);
        }
        for &(send, level) in voice.sends {
            let on = format!("send{send}.on");
            let at = format!("send{send}.level");
            set_params(
                &mut self.doc,
                &self.registry,
                self.item,
                &channel,
                CHANNEL_TYPE,
                &[(&on, 1.0), (&at, level)],
            );
        }
        self.run(Command::Song(SongCommand::AddTrack {
            device: key.clone(),
        }));
        for p in voice.clips {
            let clip_def = clip_named(own, self.item, p.clip);
            let length = clip_def.bars;
            assert!(
                p.bars % length == 0 && p.bars >= length,
                "{}: {} does not fill {} bars",
                self.item,
                p.clip,
                p.bars
            );
            assert!(
                p.bar >= 1 && p.bar - 1 + p.bars <= bars,
                "{}: {} runs past the end",
                self.item,
                p.clip
            );
            let mut clip = clip_of(clip_def, bar_tick(p.bar as f64));
            if p.keys != Keys::All
                && let ClipContent::Notes(notes) = &mut clip.content
            {
                notes.retain(|n| p.keys.takes(n.key));
            }
            clip.length = beats_ticks(p.bars as f64 * 4.0);
            if p.bars > length {
                clip.loop_length = Some(beats_ticks(length as f64 * 4.0));
            }
            self.run(Command::Song(SongCommand::AddClip {
                device: key.clone(),
                lane: 0,
                clip,
            }));
        }
    }
}

/// The device key of a voice: its name in lower case, a dash for anything but letters and digits.
pub(in crate::app_core) fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

pub(in crate::app_core) fn kit_named(name: &str) -> &'static KitDef {
    KITS.iter()
        .find(|k| k.name == name)
        .unwrap_or_else(|| panic!("no factory kit {name}"))
}

/// The clip `name`: one of the song's own, else a factory clip.
pub(in crate::app_core) fn clip_named<'a>(
    own: &'a [ClipDef],
    song: &str,
    name: &str,
) -> &'a ClipDef {
    own.iter()
        .chain(CLIPS.iter())
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("{song}: no clip {name}"))
}

// ------------------------------------------------------------------------------- the writer

/// A built example: the project text and the audio its kit samples need.
pub(in crate::app_core) struct Built {
    /// The project file text.
    pub text: String,
    /// `(asset id, audio)` of every sample the project holds.
    pub audio: Vec<(String, AudioData)>,
}

/// The project of `def`. `kits` are the built kits (by name) the project may use.
///
/// # Panics
/// A name the tables do not have, a value outside a param's range, or an edit the model refuses.
pub(in crate::app_core) fn build_example(def: &ExampleDef, kits: &[(&str, KitBuilt)]) -> Built {
    let registry = default_registry();
    let mut build = Build {
        doc: Document::new(new_project_doc()),
        registry,
        item: def.name,
        audio: Vec::new(),
    };
    build.run(Command::SetTempo(def.bpm));
    for voice in def.voices {
        build.voice(def.clips, def.bars, voice, kits);
    }
    (def.mix)(&mut build);
    build.run(Command::Song(SongCommand::SetSongEnd(bar_tick(
        def.bars as f64 + 1.0,
    ))));
    build.run(Command::SetSongInfo(SongInfo {
        title: def.name.to_owned(),
        author: super::AUTHOR.to_owned(),
        notes: format!("{} {}", def.sound, def.shows),
        ..SongInfo::default()
    }));
    let text =
        save_project_checked(build.doc.doc()).unwrap_or_else(|e| panic!("{}: {e}", def.name));
    Built {
        text,
        audio: build.audio,
    }
}

/// Where an example lives in the factory folder, relative to it: the project file in a folder of
/// its name (the layout Save As makes for a project with audio).
pub(in crate::app_core) fn project_path(name: &str) -> String {
    format!("Projects/{name}/{name}.atomik")
}

/// Writes every example below `root/Projects`: the project files and, beside them, the audio of
/// the kits they use in `<name>/Audio/<id>.wav`. The folder is made fresh.
///
/// # Panics
/// A file cannot be written or an example cannot be built.
pub(in crate::app_core) fn write_examples(root: &Path, kits: &[(&str, KitBuilt)]) {
    let projects = root.join("Projects");
    let _ = std::fs::remove_dir_all(&projects);
    for def in EXAMPLES {
        let built = build_example(def, kits);
        let folder = projects.join(def.name);
        std::fs::create_dir_all(&folder).expect("a project folder");
        let file = root.join(project_path(def.name));
        std::fs::write(&file, &built.text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        if !built.audio.is_empty() {
            let audio = folder.join("Audio");
            std::fs::create_dir_all(&audio).expect("an audio folder");
            for (hash, data) in &built.audio {
                write_wav_f32(&audio.join(format!("{hash}.wav")), data)
                    .unwrap_or_else(|e| panic!("{}: {e}", def.name));
            }
        }
    }
}
