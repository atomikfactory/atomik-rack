//! The MIDI chart of `docs/DEVICES.md` against what every instrument does: each message is sent
//! to a freshly built device and its output compared with a render without it, so the chart is
//! checked by behaviour and not by reading the code.

use crate::register_all;
use crate::test_support::{off, on};
use atomik_core::{NoteEvent, NoteKind};
use atomik_device_sdk::{DeviceKind, Registry};
use atomik_testkit::harness::DeviceHarness;

const DOC: &str = include_str!("../../../docs/DEVICES.md");

/// The chart's columns after the device: the messages that are tested by rendering.
const COLUMNS: [&str; 6] = [
    "Pitch bend",
    "Bend range (RPN 0)",
    "Mod wheel (CC 1)",
    "Sustain (CC 64)",
    "Volume (CC 7)",
    "Expression (CC 11)",
];

/// The instruments that keep channel and key pressure as a modulation source (the last column;
/// each has a test of its own that sends the messages and reads the source).
const PRESSURE: [&str; 7] = [
    "atomik.poly-synth",
    "atomik.mono-bass",
    "atomik.dual-layer",
    "atomik.wavetable-synth",
    "atomik.fm-synth",
    "atomik.granular",
    "atomik.sampler",
];

fn cc(controller: u8, raw: u8) -> NoteKind {
    NoteKind::Cc {
        controller,
        value: f32::from(raw) / 127.0,
    }
}

/// A device of `id` with a loaded sample map where it takes one.
fn build(r: &Registry, id: &str) -> DeviceHarness {
    let mut h = DeviceHarness::with_options(r.create(id).unwrap(), 48_000.0, 120.0, 5);
    // The grain sampler plays its built-in source when it has no map, which has a real spectrum.
    if r.takes_samples(id) && id != "atomik.granular" {
        let map = match id {
            "atomik.drum-machine" => crate::test_support::drum_sample_map(),
            "atomik.loop-slicer" => crate::test_support::slicer_sample_map(),
            _ => crate::test_support::demo_sample_map(),
        };
        assert!(h.device_mut().swap_samples(Some(map)).is_none());
    }
    h
}

/// The key to play on `id`.
fn key_of(id: &str) -> u8 {
    match id {
        "atomik.drum-machine" => 36,
        "atomik.loop-slicer" => 24,
        _ => 60,
    }
}

/// Renders 90 batches of `script` (events by batch) and returns the bits of the left output.
fn render(r: &Registry, id: &str, script: &[(usize, NoteEvent)]) -> Vec<u32> {
    let mut h = build(r, id);
    let out = h
        .device()
        .descriptor()
        .sockets
        .iter()
        .find(|s| s.key == "out_l");
    let out = out.map_or("out", |s| s.key);
    let mut bits = Vec::new();
    for batch in 0..90 {
        for (at, e) in script {
            if *at == batch {
                h.push_note("notes_in", *e);
            }
        }
        bits.extend(h.render(1, &[out]).remove(0).iter().map(|s| s.to_bits()));
    }
    bits
}

fn at(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

/// Which of [`COLUMNS`] change the sound of `id`.
fn responses(r: &Registry, id: &str) -> [bool; 6] {
    if r.descriptor(id)
        .is_some_and(|d| d.sockets.iter().all(|s| s.key != "notes_in"))
    {
        // No notes socket (the audio track): nothing a note event could reach.
        return [false; 6];
    }
    let key = key_of(id);
    let note = on(8, key, 1.0);
    let base = render(r, id, &[(0, note)]);
    assert!(
        base.iter().any(|b| f32::from_bits(*b) != 0.0),
        "{id}: the test note sounds"
    );
    let differs = |script: &[(usize, NoteEvent)], than: &[u32]| render(r, id, script) != than;
    let bend = at(3, NoteKind::PitchBend { value: 1.0 });
    let bent = render(r, id, &[(0, bend), (0, note)]);
    let pitch_bend = bent != base;
    let rpn = [
        (0, at(0, cc(101, 0))),
        (0, at(1, cc(100, 0))),
        (0, at(2, cc(6, 12))),
        (0, bend),
        (0, note),
    ];
    let bend_range = differs(&rpn, &bent);
    let wheel = differs(&[(0, at(0, cc(1, 127))), (0, note)], &base);
    // The same note with its off at batch 10, with and without the pedal down before it.
    let released = [(0, note), (10, off(3, key))];
    let plain = render(r, id, &released);
    let pedal = differs(
        &[(0, at(0, cc(64, 127))), (0, note), (10, off(3, key))],
        &plain,
    );
    let volume = differs(&[(0, at(0, cc(7, 64))), (0, note)], &base);
    let expression = differs(&[(0, at(0, cc(11, 64))), (0, note)], &base);
    [pitch_bend, bend_range, wheel, pedal, volume, expression]
}

/// The rows of the chart: type id and the cells after the device name.
fn chart() -> Vec<(String, Vec<String>)> {
    let start = DOC
        .find("\n## MIDI\n")
        .expect("DEVICES.md has a MIDI section");
    let body = &DOC[start + 1..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    body[..end]
        .lines()
        .filter(|l| l.starts_with("| ") && l.contains("`atomik."))
        .map(|l| {
            let cells: Vec<String> = l
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            let id = cells[0].trim_matches('`').to_string();
            (id, cells[1..].to_vec())
        })
        .collect()
}

#[test]
fn the_midi_chart_lists_every_instrument_with_what_it_does() {
    let mut r = Registry::new();
    register_all(&mut r).unwrap();
    let header = DOC
        .lines()
        .skip_while(|l| *l != "## MIDI")
        .find(|l| l.starts_with("| Device"))
        .expect("chart header");
    let cols: Vec<&str> = header.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(cols[1..=6], COLUMNS, "the chart's columns");
    assert_eq!(cols[7], "Pressure source");
    let rows = chart();
    let mut instruments = 0;
    let ids: Vec<&str> = r.type_ids().collect();
    for id in ids {
        if r.get(id).unwrap().descriptor.kind != DeviceKind::Instrument {
            continue;
        }
        instruments += 1;
        let (_, cells) = rows
            .iter()
            .find(|(row, _)| row == id)
            .unwrap_or_else(|| panic!("{id} is missing from the MIDI chart"));
        let got = responses(&r, id);
        for (n, name) in COLUMNS.iter().enumerate() {
            let want = match cells[n].as_str() {
                "yes" => true,
                "no" => false,
                other => panic!("{id}: {name}: `{other}` is not yes or no"),
            };
            assert_eq!(got[n], want, "{id}: {name}");
        }
        assert_eq!(
            cells[6] == "yes",
            PRESSURE.contains(&id),
            "{id}: pressure source"
        );
    }
    assert_eq!(rows.len(), instruments, "the chart has no stray rows");
}
