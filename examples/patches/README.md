# Example patches

Patches are JSON files rendered offline by `atomik-render`:

```text
atomik-render <PATCH> -o <OUT.wav> [--format f32|pcm24] [--block N] [--stats] [--verbose]
```

For example, `cargo run --release -p atomik-render -- examples/patches/slice-demo.json -o demo.wav --stats`.

| File | What it is |
|---|---|
| `minimal.json` | One poly-synth playing one note, cabled straight to the audio interface. |
| `slice-demo.json` | A 16-beat demo at 120 BPM, 48 kHz. A saw/square poly-synth plays a bass line and pad chords in A minor (Am, F, C, G) with an LFO on the filter cutoff. It goes through a ping-pong stereo delay (dotted eighth) into mixer channel 1. A sine/triangle lead synth plays a melody into channel 2, panned right. The mixer master feeds outputs 1 and 2. The tests use it for the golden render. |
| `mod-source-demo.json` | An 8-beat patch: a poly synth bass note whose filter cutoff is swept by a tempo-synced saw LFO from an `atomik.mod-source` device (`lfo1_cv_out` into `cutoff_cv_in`). |
| `slice-demo.v2.json` | `slice-demo.json` in format version 2, with rack layout (names, a folded delay, accent colours). It renders bit-identically to the v1 file. |

## Format (`"atomik-patch"`, version 1)

```json
{
  "format": "atomik-patch", "version": 1,
  "sample_rate": 48000, "tempo_bpm": 120, "seed": 1,
  "length_beats": 2, "output_channels": 2,
  "devices": [
    { "id": "synth", "type": "atomik.poly-synth", "params": { "filter.cutoff": 1500 } }
  ],
  "cables": [
    { "from": "synth:out_l", "to": "interface:out_1" },
    { "from": "synth:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1, "key": 57, "velocity": 0.8 }
  ]
}
```

| Field | Rules |
|---|---|
| `format`, `version` | Must be `"atomik-patch"` and `1` (or `2`, see below). |
| `sample_rate` | 44100, 48000 or 96000. |
| `tempo_bpm` | 20 to 999. |
| `seed` | Optional (default 0). Seeds every pseudo-random source, so a render is reproducible. |
| `length_beats` | Greater than 0. The render lasts `length_beats` plus a 2-beat tail, at most 1 hour. |
| `output_channels` | 1 to 32. These are the audio interface's `out_1`…`out_N` sockets. |
| `devices[]` | `id` must match `[a-z][a-z0-9_-]*` and be unique. `interface` is reserved. `type` is a registered device type id (the list is in [../../docs/DEVICES.md](../../docs/DEVICES.md)): the instruments `atomik.poly-synth`, `atomik.sampler`, `atomik.drum-machine` and `atomik.loop-slicer`, the players `atomik.step-sequencer`, `atomik.arpeggiator`, `atomik.note-filter`, `atomik.key-chord`, `atomik.note-repeater` and `atomik.note-delay`, the effects `atomik.stereo-delay`, `atomik.line-mixer`, `atomik.reverb`, `atomik.compressor`, `atomik.eq`, `atomik.distortion`, `atomik.modulation`, `atomik.mastering`, the CV source `atomik.mod-source` and the factory effect `atomik.character`, and `atomik.container`; the console devices and the internal note split are made by the project format, not by hand. `params` is optional and maps param keys to plain values (Hz, ms, dB, choice index…). Out-of-range values are clamped with a warning, and discrete params are rounded. |
| `cables[]` | Optional. Each cable runs from an output socket to an input socket, written `"device:socket"`. Both ends must have the same kind (audio, CV or notes). An input takes one cable, and loops are rejected. |
| `notes[]` | Optional. Each note targets a notes input (`socket`, usually `notes_in`). It starts at `at_beats` (0 or more) and is released `length_beats` (greater than 0) later. `key` is 0–127 and `velocity` is in (0, 1]. Frames are `round(beats × sample_rate × 60 / tempo)`. |

## Format version 2 (rack layout)

Version 2 is version 1 plus three optional fields per device. They describe the rack view only and never change the sound; `atomik-render` validates and then ignores them, so a v2 file renders exactly like the same v1 file. The rack model (`atomik-model`, used by the `atomik` app) saves version 2, without `notes` and with `length_beats` 16, and reads both versions.

| Field | Rules |
|---|---|
| `devices[].name` | Optional string shown on the name plate. Names need not be unique. Default (v1 files, or when missing): the device type's name, made unique in rack order (`Poly Synth`, `Poly Synth 2`). |
| `devices[].folded` | Optional boolean, default `false`. |
| `devices[].color` | Optional accent colour `"#rrggbb"` (either case) or `null` for none. |

The order of `devices[]` is the rack order, top to bottom. In version 1 files these fields are unknown fields, and therefore errors.

Unknown fields are errors. Every error names the JSON path of the problem, and the renderer exits with code 2:

```text
error: devices[0].params."filter.cutof": atomik.poly-synth has no param "filter.cutof"; did you mean "filter.cutoff"?
```

The param and socket keys of each device are listed in [docs/DEVICES.md](../../docs/DEVICES.md).

## Golden files

`crates/render/tests/golden/` holds two goldens of `slice-demo.json` rendered as 32-bit float with the default block size:

- `slice-demo.f32.sha256` is the SHA-256 of the WAV bytes. Only Windows x86-64 checks it, because math libraries differ in the last bit between platforms.
- `slice-demo.fingerprint.json` holds the RMS of each half beat per channel in dB. Every platform checks it, within ±0.5 dB.

After an intentional change to the sound, listen to the new render. Then regenerate both files on Windows x86-64:

```text
ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test e2e golden
```
