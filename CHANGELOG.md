# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, any release may break APIs and the patch and project formats.

## [Unreleased]

### Added
- **File browser.** A docked panel (View, File Browser, `F4`) with a folder tree (Favourites, Tags, Bookmarks, drives) over a file list of folders and audio files, a search field (words, `#tag`, `fav`, optionally in sub-folders), instant preview on click with a 5 ms crossfade when switching (live only, outside the routing and the meters), favourites, tags and bookmarked folders kept in `library.json` in the configuration folder, and drag to the arrangement, sample editor and sampler panels like an OS file drop. New crate `atomik-library`; `atomik --file-browser [DIR]` shows the panel at start. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6j-the-file-browser).
- Release archives for Windows (zip), macOS (universal `Atomik Rack.app`, ad-hoc signed, in a zip) and Linux (tar.gz), each smoke-tested before a draft release is created, with per-archive `.sha256` files and a `SHA256SUMS` file.

### Changed
- Documentation, templates and contribution policy prepared for the first public release.

## [0.1.0]

First public version. Windows (x86-64) is the primary platform; Linux has been tried on Ubuntu 26.04; macOS is compile-checked only. Project files are format version 9 (older versions load).

### Added
- **Engine.** A real-time engine with fixed 64-frame batches that is allocation-free and lock-free on the render path, hot edits of the rack while audio plays, multicore rendering with bit-identical output for any worker count and host block size, plugin delay compensation across the whole graph, and cable validation (types, direction, cycles).
- **Rack and console.** A device rack with audio, CV and note cables and front and back views; a console with a channel strip per instrument (gain, filters, dynamics, EQ, insert chain, 8 sends, pan, fader), solo and mute, meters; a locked master section with 8 send buses and FX returns, a bus compressor with a key input, a control-room output and the Audio I/O device.
- **Devices.** Poly synth, line mixer, stereo delay, sampler, drum machine, loop slicer, audio track, reverb, compressor, 6-band EQ, distortion with oversampling, modulation (chorus, flanger, phaser), mastering suite, the Character effect (16 presets, Transform macro), step sequencer, arpeggiator and the container device with macros and note splits.
- **Plugin hosting.** CLAP and VST3 instruments and effects as rack devices, with an out-of-process scanner, a plugin cache, editor windows, state saving, crash journal and recovery snapshots. VST2 is not supported.
- **Arrangement and audio.** Note, automation and audio clips, a piano roll, sample, step, slice and container editors, import of WAV, AIFF, FLAC, MP3 and OGG, audio recording with latency compensation, disk streaming for large files, and project bundle folders with content-addressed audio.
- **Workflow.** A data-driven, remappable keymap with a standard and a legacy profile, tools (select, pencil, eraser, razor), a clipboard for clips, notes, points, devices and tracks, context menus, rack multi-selection and an auto-routing creation chain.
- **Tools.** `atomik` (the application), `atomik-render` (offline bounce of patches and projects to WAV), `atomik-play` (a live patch player) and `atomik-plugin-scan` (the plugin scanner).
- **Audio and MIDI I/O.** Live audio through WASAPI, CoreAudio and ALSA (and ASIO as an opt-in build feature that needs the Steinberg SDK, which is not included), and MIDI input with exact event timing.
- **Project hygiene.** GPL-3.0-or-later licence, contribution guide with DCO sign-off and test-driven development rules, code of conduct, security policy, CI for Windows, macOS and Linux (fmt, clippy, tests, cargo-deny, coverage), architecture, device, testing and keyboard documentation, and example projects.

### Known limitations
- Feedback loops are rejected; insert and return loops use stages instead.
- Parameter changes take effect at 64-frame boundaries (devices smooth audible parameters); notes are sample-accurate.
- No time-stretch for audio clips and no relinking of missing audio files.
- Plugins run inside the application process, so a plugin crash takes the application down.
- No note chase on play or locate.
- Linux VST3 editors that need file-descriptor handlers do not work, and there is no editor embedding on Wayland (the parameter pane is the fallback).
