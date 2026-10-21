# Atomik Rack

[![CI](https://github.com/atomikfactory/atomik-rack/actions/workflows/ci.yml/badge.svg)](https://github.com/atomikfactory/atomik-rack/actions/workflows/ci.yml)
[![Licence: GPL-3.0-or-later](https://img.shields.io/badge/licence-GPL--3.0--or--later-blue.svg)](LICENSE)

Atomik Rack is a free, open-source, rack-based digital audio workstation written in Rust. You build sounds by putting virtual devices in a rack and patching them together with **audio**, **CV** and **note** cables, then sequencing, recording and mixing the result with a console, a master section and an arrangement.

The project is original work, released under the GPL-3.0-or-later. It is pre-1.0: file formats, APIs and behaviour will change (see [CHANGELOG.md](CHANGELOG.md)).

☕ Buy me a coffe: https://buymeacoffee.com/atomik.factory

## Status

- **Windows (x86-64) is the primary platform.** It is the platform the application is developed and hand-tested on, including ASIO audio.
- **Linux has been built and tried by the maintainer on Ubuntu 26.04.** Other distributions, audio setups and plugins have not been checked yet, so expect rough edges and please report them.
- **macOS compiles and is linted, but has not been run on a Mac yet.** Reports from Mac users are especially welcome.
- Nothing is stable yet. See [ROADMAP.md](ROADMAP.md) for what is planned and what is missing.

There are no screenshots in the repository yet. The user interface is drawn procedurally (vector graphics, system fonts); build the application and run one of the example projects below to see it.

## Features

- **The rack.** A device rack with front and back views, audio, CV and note cables, hot editing while audio plays (adding, removing and cabling devices never resets the others), undo and redo, a cable graph that is checked when you connect (types must match, no cycles).
- **Console.** A channel strip per instrument (gain, filters, dynamics, EQ, insert chain, 8 sends, pan, fader), solo and mute, meters and automatic delay compensation across the whole graph.
- **Master section.** A locked master with 8 send buses and FX returns, a bus compressor with a sidechain key, a control-room output and the Audio I/O device with per-socket meters.
- **Sampler, drum machine and loop slicer.** A multi-zone sampler with loops, crossfades and reverse; a 16-pad drum machine with choke groups, velocity layers and individual outputs; a loop slicer with transient detection and "slices to track".
- **Effects.** Reverb, compressor (sidechain, look-ahead), 6-band EQ, oversampled distortion, chorus/flanger/phaser, and a mastering suite (multiband, width, true-peak limiter, BS.1770 loudness meter).
- **The Character effect.** A stereo insert with 16 presets in a 4 x 4 matrix and a Transform macro that morphs each preset, built from an original set of DSP stages.
- **Players.** A step sequencer and an arpeggiator that send notes and CV to any device, synchronised to the transport.
- **Container device.** A device that holds another rack: its own devices and cables, 8 macro knobs and key/velocity note splits. Containers are flattened for the engine, so they cost nothing special at run time.
- **Plugin hosting.** CLAP and VST3 instruments and effects are rack devices like any built-in one (cabled, automated, macro-mapped, saved and bounced). Scanning runs out of process. VST2 is not supported.
- **Arrangement and audio.** Note, automation and audio clips; a piano roll and step/slice/sample editors; import of WAV, AIFF, FLAC, MP3 and OGG; audio recording with latency compensation; large files stream from disk.
- **File browser.** A docked panel (`F4`) to browse folders, search by name, `#tag` or favourites, preview audio files with one click and drag them onto the arrangement, sample editor or a sampler; tags and favourites are saved in `library.json` next to the settings.
- **Workflow and keymap.** A data-driven keymap with a standard profile and a legacy profile, remappable in Settings (see [docs/KEYBOARD.md](docs/KEYBOARD.md)), tools (select, pencil, eraser, razor), a clipboard for clips, notes, devices and tracks, context menus, and a creation chain that wires new instruments and effects automatically.
- **Offline bounce.** `atomik-render` renders patches and projects to WAV, bit-identical for any block size and worker count.
- **Projects.** A project with audio is a bundle folder (`name/name.atomik` plus `Audio/`, files named by content hash); a project without audio is a single `.atomik` file. Saves are atomic.

## Install

Release builds are attached to each [GitHub release](https://github.com/atomikfactory/atomik-rack/releases): `atomik-rack-<version>-windows-x86_64.zip`, `atomik-rack-<version>-macos-universal.zip` (Apple Silicon and Intel) and `atomik-rack-<version>-linux-x86_64.tar.gz`, with a `SHA256SUMS` file to check them against. The builds are not code-signed, so each system warns the first time.

- **Windows.** Unzip, run `atomik.exe`. If SmartScreen says "Windows protected your PC", choose **More info**, then **Run anyway**.
- **macOS.** Unzip and move `Atomik Rack.app` to Applications. The app is only ad-hoc signed and not notarised: right-click it, choose **Open**, then **Open** again. Or remove the quarantine flag once: `xattr -dr com.apple.quarantine "Atomik Rack.app"`.
- **Linux.** Extract the archive and run `./atomik`. It needs ALSA (`libasound2`), fontconfig (`libfontconfig1`) and a working Vulkan or OpenGL driver; on Debian/Ubuntu: `sudo apt-get install libasound2 libfontconfig1`.

## Building

You need a Rust toolchain. [rust-toolchain.toml](rust-toolchain.toml) pins the version and `rustup` installs it automatically.

### Windows

Install [rustup](https://rustup.rs/) with the MSVC toolchain (Visual Studio Build Tools with the C++ workload), then:

```sh
cargo build --release --workspace
```

### macOS

Install [rustup](https://rustup.rs/) and the Xcode command-line tools (`xcode-select --install`), then:

```sh
cargo build --release --workspace
```

### Linux

Install the build dependencies (Debian/Ubuntu names shown), then build:

```sh
sudo apt-get install libasound2-dev libfontconfig1-dev pkg-config
cargo build --release --workspace
```

The window uses the GPU through wgpu, so a working Vulkan (or OpenGL) driver is needed to run the application.

### Optional: ASIO on Windows

ASIO support is an opt-in feature (`asio`). **The Steinberg ASIO SDK is not included in this repository and is never committed.** If you want ASIO, obtain the SDK from Steinberg under its licence, unpack it somewhere outside the repository, install LLVM/Clang (needed by `bindgen`), and build with the SDK location in the environment:

```sh
# PowerShell
$env:CPAL_ASIO_DIR = "<path to the ASIO SDK>"
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"   # if it is not found automatically
cargo build --release -p atomik-app --features asio
```

Without the feature, WASAPI is used on Windows. A binary built with the `asio` feature contains code derived from the SDK, so check the licence terms before you redistribute it; builds of this repository do not enable it by default.

## Running

```sh
# a new project (Audio I/O on top, the locked Master Section under it)
cargo run --release -p atomik-app

# open a project
cargo run --release -p atomik-app -- examples/projects/demo.atomik
```

Example projects (all in [examples/projects](examples/projects), described in [examples/README.md](examples/README.md)):

```sh
cargo run --release -p atomik-app -- examples/projects/audio-demo.atomik
cargo run --release -p atomik-app -- examples/projects/sampler-demo/sampler-demo.atomik
cargo run --release -p atomik-app -- examples/projects/drum-demo/drum-demo.atomik
cargo run --release -p atomik-app -- examples/projects/slicer-demo/slicer-demo.atomik
cargo run --release -p atomik-app -- examples/projects/fx-demo/fx-demo.atomik
cargo run --release -p atomik-app -- examples/projects/player-demo/player-demo.atomik
cargo run --release -p atomik-app -- examples/projects/container-demo/container-demo.atomik
cargo run --release -p atomik-app -- examples/projects/character-demo/character-demo.atomik
cargo run --release -p atomik-app -- examples/projects/master-demo/master-demo.atomik
```

In the app, Space plays and Ctrl+Enter records. `Ctrl+I` creates an instrument, `Ctrl+E` an effect and `Ctrl+T` an audio track (Cmd instead of Ctrl on macOS). F5 shows the console, F6 the rack and F3 the device browser. [docs/KEYBOARD.md](docs/KEYBOARD.md) lists every key, and Settings > Keyboard remaps them.

Useful options: `--host`, `--audio-device` and `--buffer` choose the audio device, `--workers N` sets the number of render threads, and `--stress-*` options open generated stress projects for checking real-time load. Run `atomik --help` for the full list.

### Plugins

Plugin hosting is off until you enable it in Settings and add your plugin folders. Atomik Rack hosts CLAP and VST3 plugins; the in-repo test plugins under `crates/testplugins` and the `plugin-demo` projects let you try the feature without installing anything (see [examples/README.md](examples/README.md)). A plugin runs inside the application process, so a plugin crash takes the application down; scanning is out of process, and recovery snapshots limit the damage.

## Rendering a patch or a project

```sh
cargo run --release -p atomik-render -- examples/patches/slice-demo.json -o out.wav --stats
cargo run --release -p atomik-render -- examples/projects/audio-demo.atomik -o audio-demo.wav
```

```text
atomik-render <PATCH|PROJECT> -o <OUT.wav> [--format f32|pcm24] [--block N] [--workers N] [--loop-count N] [--stats] [--verbose]
```

- `--format pcm24` writes 24-bit PCM instead of 32-bit float.
- `--block N` sets the host block size (default 512) and `--workers N` the worker threads. Neither changes the output.
- A project bounces its song (end plus 2 beats, loop off unless `--loop-count N`).
- A project that uses plugins needs `--allow-plugins`; see `atomik-render --help`.
- On any error the renderer prints `error: <JSON path or file>: <message>` and exits with code 2.

A patch is a versioned JSON file listing devices, parameters, cables, tempo, notes and length; see [examples/patches/README.md](examples/patches/README.md). The devices and their parameter keys are in [docs/DEVICES.md](docs/DEVICES.md). `atomik-play` is a small live player for patches.

## Hard invariants

Four rules are never traded away. Each one is enforced by tests or process, not only by convention.

1. **HI-1: real-time safety.** The audio render path never allocates or frees memory, takes a lock or blocks. This includes audio playback, disk streaming and the record tap. Engine and device tests run render calls under an allocation guard that fails the test on any heap operation.
2. **HI-2: determinism.** The same project and the same input always render to bit-identical output, whatever host block size or worker count is used.
3. **HI-3: graph validity.** The engine only ever runs a graph that is type-correct (audio to audio, CV to CV, notes to notes) and has no cycles. Invalid connections are rejected when you make them.
4. **HI-4: originality.** Contributions are original work or compatibly licensed; no code, data or assets are copied from proprietary products. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Crate map

```
                          atomik-app  (bin `atomik`)
                  ┌──────┬────┴─────┬───────────┬─────────────┐
            atomik-ui  atomik-render  atomik-audio-io  atomik-midi-io      atomik-play (bin)
                  │          │               │               │
          atomik-model       │         atomik-devices ── atomik-engine
                  │          │               │                │
          atomik-audio-file ◄┘      atomik-dsp ── atomik-device-sdk ── atomik-core
      file browser data: atomik-library (used by audio-file, ui and app)
      plugin hosting: atomik-plugin-host, -clap, -vst3, -scan (scanner binary)
      atomik-testkit (dev-dependency only: fakes, signal analysis, no-alloc harness)
```

A crate depends only on crates below it; `engine` and `devices` meet only through `device-sdk`; the engine knows neither the model nor the audio files; production crates never depend on `testkit`. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) has the full crate table, data flow and threading model.

## Documentation

- [docs/README.md](docs/README.md): index of the documentation.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): crates, data flow, threads and how each hard invariant is enforced.
- [docs/DEVICES.md](docs/DEVICES.md): sockets and parameters of every device.
- [docs/EXTENDING.md](docs/EXTENDING.md): how to write a new device.
- [docs/TESTING.md](docs/TESTING.md): gates, the allocation guard, determinism tests, goldens and manual tests.
- [docs/KEYBOARD.md](docs/KEYBOARD.md): the keyboard reference (generated).
- [ROADMAP.md](ROADMAP.md) and [CHANGELOG.md](CHANGELOG.md).

## Checking your changes

These are the gates CI runs (details in [docs/TESTING.md](docs/TESTING.md)):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                     # needs: cargo install cargo-deny
```

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first: it covers the original-work policy, the DCO sign-off (`git commit -s`), the test-driven workflow and the real-time safety rules. All participants follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately as described in [SECURITY.md](SECURITY.md). For questions and help, see [SUPPORT.md](SUPPORT.md).

## Licence

Copyright © Atomik Rack contributors.

Atomik Rack is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version (`GPL-3.0-or-later`). It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY. See [LICENSE](LICENSE) for the full text.

No fonts, images or audio from third parties are bundled: the interface uses system fonts and procedural graphics, and the example audio is generated by the project's own code (see [examples/README.md](examples/README.md)).

## Trademark notice

Atomik Rack is an independent project and is not affiliated with, endorsed by or sponsored by any other company or product.

VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries. ASIO is a trademark of Steinberg Media Technologies GmbH. CLAP is an open plugin standard. Atomik Rack hosts plugins in the CLAP and VST3 formats; the names VST3, ASIO and CLAP are used only to say which formats and drivers are supported, and the project uses no VST or ASIO logo. Windows, macOS, Linux and all other product, plugin and company names that appear in the application or on this page belong to their respective owners; plugin and vendor names shown in the browser come from the plugin files themselves.
