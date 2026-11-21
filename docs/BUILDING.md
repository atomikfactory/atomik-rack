# Building and developing Atomik Rack

How to build the program from source, run it, render patches and projects from the command line,
and check a change before you open a pull request. Users who only want to play music can download
a release instead: see [Install](../README.md#install).

## Building

You need a Rust toolchain. [rust-toolchain.toml](../rust-toolchain.toml) pins the version and `rustup` installs it automatically.

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

The window is drawn on the GPU through wgpu with compute shaders, and there is no other drawing path: a graphics driver that supports DirectX 12 or Vulkan (Metal on macOS) is needed to run the application. A machine without one gets a message that says so, and the log lists the adapters that were found; a software adapter is never used on its own.

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

Example projects (all in [examples/projects](../examples/projects), described in [examples/README.md](../examples/README.md)):

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

In the app, Space plays and Ctrl+Enter records. `Ctrl+I` creates an instrument, `Ctrl+E` an effect and `Ctrl+T` an audio track (Cmd instead of Ctrl on macOS). F5 shows the console, F6 the rack and F3 the device browser. [docs/KEYBOARD.md](KEYBOARD.md) lists every key, and Settings > Keyboard remaps them.

Useful options: `--host`, `--audio-device` and `--buffer` choose the audio device, `--workers N` sets the number of render threads, and `--stress-*` options open generated stress projects for checking real-time load. Run `atomik --help` for the full list.

### Plugins

Plugin hosting is off until you enable it in Settings and add your plugin folders. Atomik Rack hosts CLAP and VST3 plugins; the in-repo test plugins under `crates/testplugins` and the `plugin-demo` projects let you try the feature without installing anything (see [examples/README.md](../examples/README.md)). Each plugin instance runs in its own `atomik-plugin-host` process (Settings > Plugins, on by default), so a plugin that crashes or hangs cannot take the application down: its device shows CRASHED and RELOAD starts it again from its last saved state. Its editor opens in a window of that process. The PROCESSES button in the same place lets several plugins share a process (one per plugin file, or one for all of them, up to 16 in each) for fewer processes at the price of isolation: a crash then stops every plugin in that process, and each says so; the plugin manager's PROCESS column does it for single plugins. A plugin that crashed or failed to load three times in a row is quarantined (the manager says why); a project that uses it opens without it, its devices say QUARANTINED, and LOAD ANYWAY loads one. Scanning is out of process too. With the option off, plugins run inside the application and a plugin crash takes it down; recovery snapshots limit the damage. `atomik-render` uses the sandbox as well (`--plugin-sandbox on|off`).

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

A patch is a versioned JSON file listing devices, parameters, cables, tempo, notes and length; see [examples/patches/README.md](../examples/patches/README.md). The devices and their parameter keys are in [docs/DEVICES.md](DEVICES.md). `atomik-play` is a small live player for patches.

## Hard invariants

Four rules are never traded away. Each one is enforced by tests or process, not only by convention.

1. **HI-1: real-time safety.** The audio render path never allocates or frees memory, takes a lock or blocks. This includes audio playback, disk streaming and the record tap. Engine and device tests run render calls under an allocation guard that fails the test on any heap operation.
2. **HI-2: determinism.** The same project and the same input always render to bit-identical output, whatever host block size or worker count is used.
3. **HI-3: graph validity.** The engine only ever runs a graph that is type-correct (audio to audio, CV to CV, notes to notes) and has no cycles. Invalid connections are rejected when you make them.
4. **HI-4: originality.** Contributions are original work or compatibly licensed; no code, data or assets are copied from proprietary products. See [CONTRIBUTING.md](../CONTRIBUTING.md).

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
      video: atomik-video (FFmpeg discovery, probe and proxy; used by app)
      update check: atomik-update (the one optional request for the newest release; used by app)
      plugin hosting: atomik-plugin-host, -clap, -vst3, -scan (scanner binary)
      plugin sandbox: atomik-plugin-ipc, -sandbox, -hostd (child binary atomik-plugin-host)
      atomik-testkit (dev-dependency only: fakes, signal analysis, no-alloc harness)
      atomik-mcp (bin, development tool: MCP server for QA over render, model, app and ui)
```

A crate depends only on crates below it; `engine` and `devices` meet only through `device-sdk`; the engine knows neither the model nor the audio files; production crates never depend on `testkit`. [docs/ARCHITECTURE.md](ARCHITECTURE.md) has the full crate table, data flow and threading model.

## Checking your changes

These are the gates CI runs (details in [docs/TESTING.md](TESTING.md)):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                     # needs: cargo install cargo-deny
```

For a change to the sound or to what the app shows, [docs/MCP.md](MCP.md) describes the QA steps (bounce and compare, drive the app headlessly, read and screenshot the UI) and the `atomik-mcp` server that does them for an MCP client.
