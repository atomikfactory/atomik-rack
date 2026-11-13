# Dependencies

Which dependencies are held on purpose, why, and how they are kept current. The licence policy is in
`deny.toml`; the upgrade chores are scheduled by `.github/workflows/deps.yml`.

## Policy

- Every dependency is declared once in `[workspace.dependencies]` of the root `Cargo.toml` (a
  crate used by one crate only may name its version in that crate). `Cargo.lock` is committed and every
  build in CI and in the release workflows uses `--locked`.
- A new dependency needs a reason in the pull request: what it does that the standard library and the
  crates already here do not, its licence (it must pass `cargo deny check`, GPL-3.0-or-later compatible),
  and whether it is maintained. Prefer small crates over frameworks.
- Dependabot opens weekly pull requests for compatible updates of cargo and of the GitHub actions
  (grouped). Actions are pinned to a commit hash with the version in a comment.
- Pre-1.0 crates (versions `0.x`) can break their API in a minor release. Where that has hurt, the
  version is pinned exactly (below); the rest follow the quarterly pass.

## Deliberate pins

| Dependency | Pin | Where | Why |
| --- | --- | --- | --- |
| `clack-host`, `clack-extensions` | `=0.2.0` (exact) | `crates/plugin-clap` | The CLAP host bindings are 0.x and the extension traits change between minors; the backend is written against 0.2.0. Bump both together, by hand, with the plugin tests. |
| `vst3` | `=0.3.0` (exact) | `crates/plugin-vst3` | The bindings are generated from one version of the VST3 SDK headers (3.8.0, MIT-licensed). The licence of the SDK differs between its versions, so a bump is a licence decision as well as an API one: read the licence of the headers behind the new version before moving. |
| `rust-version` and `rust-toolchain.toml` | 1.99 | root | One toolchain for CI, the release builds and the docs. The two files move together; a dependency that needs a newer compiler moves both in a pull request of their own. |
| `serde_json` | feature `float_roundtrip` | root | Saved files and settings must read back the floats that were written. |
| `symphonia`, `arboard` | `default-features = false` | root | Only the formats and backends the program uses are built: fewer crates, fewer advisories, shorter builds. |
| `ureq` | `default-features = false`, feature `rustls` | root, used by `crates/update` | The update check's one HTTPS request. The default features add gzip; no cookie store, no JSON layer and no redirect handling are used (the request is configured to follow none). rustls runs on `ring` with the bundled root certificates (`webpki-roots`), so the behaviour is the same on every platform. |
| `zune-jpeg` | `default-features = false`, feature `std` | root, used by `crates/video` | The JPEG decoder of the video viewer. The default features add hand-written SIMD paths (AVX2 and NEON intrinsics, `unsafe`); a 360-line proxy frame decodes in a few milliseconds without them, off the UI thread, so the build keeps the portable code only. |

The clipboard dependency (`arboard`, MIT or Apache-2.0) was added for the text fields; the system
clipboard is its only use.

The JPEG decoder (`zune-jpeg` with its one dependency `zune-core`, both `MIT OR Apache-2.0 OR Zlib`, pure Rust, no
C code) was added for the video viewer: the proxy of a film is a file of JPEG frames and the viewer has to turn one
into pixels. It decodes only; nothing encodes. The alternatives were `jpeg-decoder` (no longer developed, its
authors point to `zune-jpeg`) and `image` (a framework with a codec per format, far more than one decoder needs).
The choice is in [DECISIONS.md](DECISIONS.md#p-the-jpeg-decoder-of-the-viewer); `cargo deny check` passes with the
licences the policy already allows.

The HTTPS client (`ureq` 3 with `rustls`, `rustls-webpki`, `ring` and `webpki-roots`, plus the small `http`,
`httparse`, `base64`, `subtle`, `untrusted` and `zeroize` crates they use) was added for the optional update check
and is used by nothing else: it makes the one request of that check and only after the user said yes. The
alternatives were `reqwest` (an asynchronous framework with its own runtime and far more crates), `rustls` used
directly with a hand-written HTTP exchange, and a platform verifier for the certificates (one code path per platform).
`ring` is the one part that is not pure Rust (a few C and assembly files, `Apache-2.0 AND ISC`); `aws-lc-rs` would be a
larger C build. `webpki-roots` is licensed `CDLA-Permissive-2.0`, a permissive data licence, which is now in the
allowed licences of `deny.toml`; `cargo deny check` passes. The choice is in
[DECISIONS.md](DECISIONS.md#s-the-update-check).

The language of the operating system (`sys-locale` 0.3, `MIT OR Apache-2.0`, one dependency `libc`) was added for the
System choice of the language setting: it asks the platform once at start for the locale (`fr-FR`), which the program
maps to a language it has texts for. The standard library has no way to read it (the environment variables are not set
on Windows and not reliable on macOS), and reading it by hand means platform calls that the workspace's `unsafe` policy
keeps out of the program. Nothing is sent anywhere; the value is read and compared. The choice is in
[DECISIONS.md](DECISIONS.md#w-how-the-interface-is-translated); `cargo deny check` passes with the licences the policy
already allows.

The hand tracking crate (`atomik-vision`) added three groups of dependencies; the choices are in
[DECISIONS.md](DECISIONS.md#ad-webcam-hand-tracking-the-camera-the-models-privacy-and-the-gesture-rules).

- **Inference: `tract-tflite` 0.23** (with `tract-core`, `tract-data`, `tract-linalg` and a few small crates;
  `MIT OR Apache-2.0`, pure Rust, by Sonos). It reads the official `.tflite` model files directly and runs them on the
  CPU, with no conversion and no native library. The `tract-linalg` build script asks the machine's C compiler (through
  the `cc` crate) which assembly kernels it can build, so a C compiler must be on the build machine (Xcode tools on
  macOS, `gcc` or `clang` on Linux, MSVC on Windows: every build machine has one). No `libclang`, no `bindgen`.
  The alternatives were ONNX Runtime (`ort`, a native library to download or build), the TensorFlow Lite C++ library
  (a CMake or Bazel build) and writing the layers by hand.
- **Camera, Windows and macOS: `nokhwa` 0.10** (`Apache-2.0`, with `nokhwa-core` and the system binding crates
  `nokhwa-bindings-windows`, over Media Foundation, and `nokhwa-bindings-macos`, over AVFoundation). It is used with
  `default-features = false` and one backend feature per system: its default `decoding` feature pulls a C JPEG library
  (`mozjpeg`) that needs a C compiler and an assembler, and the program already has `zune-jpeg`, so the pictures are
  converted in `crates/vision/src/camera/convert.rs`. On macOS the binding compiles one small Objective-C file with
  the system compiler. It uses `unsafe` inside; `atomik-vision` does not. Its build-time macro crate `paste` is
  unmaintained (RUSTSEC-2024-0436, a notice and not a vulnerability): the advisory is ignored in `deny.toml` with a
  review date.
- **Camera, Linux: `linuxvideo` 0.3** (`0BSD`, with `uoctl`, `bitflags` 1, `libc` and `log`). A pure Rust Video4Linux
  client: no `bindgen`, no `libclang`, no system library. `nokhwa`'s Linux backend goes through `v4l`, whose
  `v4l2-sys-mit` runs `bindgen` at build time, which would make libclang and the kernel headers a requirement of every
  build and of CI. `0BSD` is a permissive licence with no conditions; it is now in the allowed licences of
  `deny.toml`.
- **Already in the workspace:** `sha2` (the checksums of the model files; `ring` is also there through `ureq`, but
  `sha2` is the crate every other checksum of the program uses), `ureq` (the one HTTPS request of the model download,
  configured as the update check is: HTTPS only, no redirect, timeouts), `zune-jpeg`, `clap` and `ctrlc` (the demo
  tool only).

## Model files

The hand tracking models are not part of the installer, the repository or `Cargo.lock`. The user downloads them once
(a dialog says what is fetched, from where and how big), into `models/hands/` of the settings folder, and every file is
checked against the size and SHA-256 below, written in `crates/vision/src/download.rs` and nowhere else, when it is
downloaded and again before it is first loaded in a run. A mismatch deletes what was fetched.

| File | Size | SHA-256 | Source |
| --- | --- | --- | --- |
| `palm_detection_full.tflite` | 2,339,846 bytes | `1b14e9422c6ad006cde6581a46c8b90dd573c07ab7f3934b5589e7cea3f89a54` | `https://storage.googleapis.com/mediapipe-assets/palm_detection_full.tflite` |
| `hand_landmark_full.tflite` | 5,478,917 bytes | `11c272b891e1a99ab034208e23937a8008388cf11ed2a9d776ed3d01d0ba00e3` | `https://storage.googleapis.com/mediapipe-assets/hand_landmark_full.tflite` |

Both are the MediaPipe hand models (the palm detector and the 21-point hand landmark model), published by Google under
the Apache License 2.0 (see the model cards of the MediaPipe Hand Detection and Hand Landmarks models). The files are
not changed: they are used as published. The notice that goes with them:

> Hand tracking models: Copyright The MediaPipe Authors (Google LLC). Licensed under the Apache License, Version 2.0
> (http://www.apache.org/licenses/LICENSE-2.0). Used without changes; downloaded by the user from the publisher.

A new version of a model is a change of the hashes in `download.rs` and of this table, made by hand after the new file
was run through the tests of `crates/vision` (set `ATOMIK_VISION_MODELS` to the folder, see [TESTING.md](TESTING.md)).

### Stem separation

Stem separation uses no model: `atomik-stems` is a signal-processing split written in house (an STFT, median filters
and masks), so there is no weight file, no download, no data licence and nothing to host. A model whose weights or
training data carry non-commercial terms is not used (see [DECISIONS.md](DECISIONS.md) AK). The crates
`atomik-download` (the shared store and download, used by the hand models) and `atomik-stems` add no third-party
dependency: `atomik-stems` uses `thiserror` only.

## Packaging tools

Used only by the release and nightly workflows (and not linked into the program), at pinned versions:

| Tool | Version | Licence | Used for |
| --- | --- | --- | --- |
| `appimagetool` (official AppImage) | 1.9.1, SHA-256 pinned in `scripts/release/fetch-appimagetool.sh` | MIT | Turns the AppDir into the `.AppImage`. Downloaded from its GitHub release in CI and checked against the pinned checksum before it runs; nothing downloads it in a test. |
| `flatpak-builder` and `flatpak` | the Ubuntu runner's packages | LGPL-2.1-or-later | Builds the Flatpak bundle from `packaging/linux/io.github.atomikfactory.AtomikRack.yml`. The runtime `org.freedesktop.Platform` and `org.freedesktop.Sdk` 25.08 (free software, mostly LGPL and GPL components) come from Flathub when the bundle is built. |
| `shellcheck`, `desktop-file-utils`, `appstream` | the Ubuntu runner's packages | GPL-3.0-or-later, GPL-2.0-or-later, LGPL-2.1-or-later | Pull request checks of the packaging scripts, the desktop entry and the metainfo. |

None of them is part of the Cargo dependency graph, so `cargo deny` does not see them. Moving appimagetool to a new
release means changing the version and the checksum in the script together, from the checksum on the release page.

## Duplicate versions

`cargo deny check bans` warns when two versions of a crate are in the graph. The ones that cannot be
removed from this side are listed in `deny.toml` with the crate that causes them, so that only a new
duplicate warns:

- the `winit` 0.30 tree (the objc2 0.5 family, an older `core-graphics`, `bitflags` 1 and an older
  `rustix`): goes away with a winit and vello upgrade;
- the `midir` tree (an older `jni`, `coremidi`'s `core-foundation`): goes away with a midir release
  that moves to the current `jni`;
- the `rfd` tree (an older `pollster` next to ours);
- `hashbrown` 0.16 (under `gpu-allocator`, under wgpu, under vello) and `miniz_oxide` 0.8 (under `png`).

When a bump removes the cause, delete its entry in `deny.toml`. A duplicate that is not in the list is
a new one: look at who pulls it before adding a skip.

## Advisory ignores

An advisory is ignored in `deny.toml` only with a reason and a review date (`review-by YYYY-MM-DD` in
the reason). `scripts/check-advisory-ignores.sh` runs in the Security workflow (on every pull request
and every Monday) and fails once the date has passed; look at the advisory again, then remove the
ignore or move the date on purpose. Two are ignored today: the unmaintained-crate notice for `ttf-parser`
(it arrives through winit's Wayland decorations) and the one for `paste` (a build-time macro crate of the `nokhwa`
camera backends on Windows and macOS).

## The quarterly pass

On the first of January, April, July and October the `Dependencies` workflow opens a review issue
(only one at a time). The pass is:

1. Bump the 0.x GUI and audio stack together where a release exists: vello, vello_cpu, parley, winit,
   muda, rfd, cpal and midir. They share platform crates, so one at a time makes duplicates. Run the
   UI tests and look at what `cargo deny check bans` says; drop the skip entries that no longer apply.
2. Review the advisory ignores.
3. Check the exact pins above against their latest releases.
4. `cargo update`, the whole test suite, commit `Cargo.lock`.

Every Monday the same workflow prints `cargo update --dry-run` in its job summary: what a refresh of
the lock file would change today. Nothing is committed by it.
