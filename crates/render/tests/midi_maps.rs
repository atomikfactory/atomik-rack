//! MIDI mappings never change an offline render: a project with mappings of every kind (to
//! params, a VCA level, the tempo, a transport key, the selected strip), saved, loaded, edited and
//! undone, bounces bit for bit like the same project without them, for any block size and worker
//! count, and a version 27 file (no mappings) bounces as it always did.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, MapMode, MapOptions,
    MidiMapCommand, MidiSource, MidiTarget, Note, RackDoc, RelativeKind, SongCommand, SourceKind,
    TransportTarget, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// Two synth tracks with a one-bar note each (channels `channel` and `channel-2`) and a VCA over
/// the first.
fn song() -> Document {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    for (k, midi) in [("lead", 72u8), ("bass", 36)] {
        let length = 4 * PPQ;
        for c in [
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            },
            Command::Song(SongCommand::AddTrack { device: key(k) }),
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    controllers: Vec::new(),
                    grid: None,
                    content: ClipContent::Notes(vec![Note::new(0, length, midi, 0.8)]),
                },
            }),
        ] {
            d.apply(c.clone(), &registry)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }
    for c in [
        Command::Song(SongCommand::SetSongEnd(4 * PPQ)),
        Command::Console(ConsoleCommand::CreateVca {
            name: "Lead".into(),
            members: vec![key("channel")],
        }),
    ] {
        d.apply(c, &registry).unwrap();
    }
    d
}

fn bounce_with(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn map(d: &mut Document, source: MidiSource, target: MidiTarget, options: MapOptions) {
    d.apply(
        Command::MidiMap(MidiMapCommand::Learn { source, target }),
        &default_registry(),
    )
    .unwrap();
    let id = d.doc().midi_maps.iter().map(|m| m.id).max().unwrap();
    let mut m = d.doc().midi_map(id).unwrap().clone();
    if m.options == options {
        return;
    }
    m.options = options;
    d.apply(
        Command::MidiMap(MidiMapCommand::Set(m)),
        &default_registry(),
    )
    .unwrap();
}

fn cc(n: u8) -> MidiSource {
    MidiSource::any_port(Some(0), SourceKind::Cc(n))
}

fn param(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

/// The song with mappings of every kind.
fn mapped() -> Document {
    let mut d = song();
    let flags = MapOptions::default();
    map(&mut d, cc(7), param("channel", "fader"), flags);
    map(
        &mut d,
        cc(10),
        param("channel-2", "pan"),
        MapOptions {
            min: 0.2,
            max: 0.8,
            invert: true,
            pickup: false,
            consume: false,
            ..flags
        },
    );
    map(
        &mut d,
        cc(16),
        param("lead", "filter.cutoff"),
        MapOptions {
            mode: MapMode::Relative(RelativeKind::SignMagnitude),
            sensitivity: 0.02,
            ..flags
        },
    );
    map(
        &mut d,
        MidiSource::any_port(Some(9), SourceKind::Note(36)),
        param("channel", "mute"),
        MapOptions::for_source(SourceKind::Note(36)),
    );
    map(
        &mut d,
        MidiSource::any_port(None, SourceKind::Cc14(1)),
        MidiTarget::Vca { id: 1 },
        flags,
    );
    map(
        &mut d,
        MidiSource::any_port(Some(1), SourceKind::PitchBend),
        MidiTarget::Tempo,
        flags,
    );
    map(
        &mut d,
        cc(115),
        MidiTarget::Transport(TransportTarget::Play),
        flags,
    );
    map(
        &mut d,
        cc(74),
        MidiTarget::SelectedStrip {
            param: "fader".into(),
        },
        flags,
    );
    d
}

#[test]
fn a_project_with_mappings_bounces_bit_for_bit_like_one_without() {
    let plain = song();
    let with = mapped();
    assert_eq!(with.doc().midi_maps.len(), 8);
    let reference = bits(&bounce_with(plain.doc(), 512, 0));
    assert!(
        reference.iter().any(|b| f32::from_bits(*b).abs() > 0.01),
        "audible"
    );
    assert_eq!(bits(&bounce_with(with.doc(), 512, 0)), reference);
}

#[test]
fn the_render_does_not_depend_on_the_block_size_or_the_workers_with_mappings() {
    let with = mapped();
    let reference = bits(&bounce_with(with.doc(), 512, 0));
    for (block, workers) in [(64, 0), (128, 2), (333, 3), (1024, 1)] {
        assert_eq!(
            bits(&bounce_with(with.doc(), block, workers)),
            reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn saving_loading_editing_and_undoing_mappings_leave_the_render_alone() {
    let plain = song();
    let reference = bits(&bounce_with(plain.doc(), 512, 0));
    let registry = default_registry();
    let with = mapped();
    let text = save_project(with.doc());
    let loaded = load_project(&text, &registry).unwrap();
    assert_eq!(loaded.midi_maps, with.doc().midi_maps);
    assert_eq!(bits(&bounce_with(&loaded, 256, 0)), reference);
    // Editing the mappings (clear, then undo) changes nothing either way.
    let mut d = with;
    d.apply(Command::MidiMap(MidiMapCommand::Clear), &registry)
        .unwrap();
    assert_eq!(bits(&bounce_with(d.doc(), 512, 0)), reference);
    d.undo(&registry).unwrap();
    assert_eq!(d.doc().midi_maps.len(), 8);
    assert_eq!(bits(&bounce_with(d.doc(), 512, 0)), reference);
}

#[test]
fn a_version_27_file_without_mappings_loads_and_bounces_as_before() {
    let registry = default_registry();
    let plain = song();
    let reference = bits(&bounce_with(plain.doc(), 512, 0));
    let text = save_project(plain.doc());
    assert!(!text.contains("midi_maps"));
    let old = text.replace("\"version\": 36", "\"version\": 27");
    let loaded = load_project(&old, &registry).unwrap();
    assert!(loaded.midi_maps.is_empty());
    assert_eq!(&loaded, plain.doc());
    assert_eq!(bits(&bounce_with(&loaded, 512, 0)), reference);
    // Saved again it is the version 30 file, with nothing else changed.
    assert_eq!(save_project(&loaded), text);
}
