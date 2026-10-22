# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, any release may break APIs and the patch and project formats.

## [Unreleased]

### Added
- **Multi-column rack.** The rack can have several columns side by side (saved in the project, project format 11; older files load as one column). Add Column and Remove Column are in the rack context menu; an empty column stays until it is removed, and removing a column moves its devices to the one on its left. Each column scrolls with the rack, which now also scrolls sideways to reach all of them.
- **Dragging between columns.** A device, or a selected group, can be dragged to any place in any column; the drop line shows where it lands, and dropping to the right of the last column makes a new column. The whole move is one undo step. The rack scrolls by itself when you drag near its edges.
- **Drag devices from the browser.** Drag a row of the device browser onto the rack to create the device exactly where you drop it, in any column; an effect is routed as if the device above the drop point were selected. Release over the browser, or press Esc, to cancel. A double-click still adds below the selection.
- **Rack navigator.** A strip at the right edge of the rack (View, Rack Navigator; on by default) shows every column as small coloured blocks and the part of the rack you are looking at. Click to centre the view there, drag to scroll, use the mouse wheel as over the rack; inside a container it shows the container's column. The choice is kept between sessions.
- **Paste and plugin drops in columns.** Paste from the rack's context menu lands where you right-clicked, and a keyboard paste lands below the selected device in its column. A plugin that is neither an instrument nor an effect can be dragged from the browser into a column or the new-column zone like any other device, and the new-column zone now stays on screen while you drag, whatever the window width.
- **CV inputs on the effects, console channels and instruments.** One convention: a CV socket `<target>_cv_in` with a bipolar `<target>_cv_trim` (−1..+1, default +1). The stereo delay (mix, feedback), reverb, modulation and distortion (mix, drive), the EQ (gain of the four shelf and peak bands) and every console channel (fader, pan, the eight sends) take one; so do the poly synth (pitch, resonance, level, pan), the sampler (resonance, level, pan), the drum machine (level of each pad) and the loop slicer (level, and cutoff through a new global low-pass, `filter.cutoff`, which is open by default). Unplugged, nothing changes, and older patches and projects load as before.
- **CV activity indicators.** On the back of the rack, a CV cable that carries signal glows and lights both of its jacks, brighter for a stronger signal and fading smoothly when it falls silent.

## [0.1.0] - 2026-10-21

First public version. Windows (x86-64) is the primary platform and is tested by hand; the test suite runs in CI on Windows, macOS and Linux, Linux has been tried on Ubuntu 26.04, and macOS has not been run on real hardware yet. Project files are format version 10 (versions 1 to 9 load).

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
- Project format 10: per-note mute. A muted note is a fifth element `true` in the note's array; muted notes stay in the clip and are skipped by playback and bounce. Versions 1 to 9 still load. New `SetClipsMuted` workflow command mutes or unmutes several clips in one undo step.
- Mute in the editors: `M` mutes or unmutes the selected clips in the arrangement (any clip kind, one undo step) and the selected notes in the piano roll (in the step editor, the cell under the pointer); Mute / Unmute items in the clip and notes context menus; an `M` button in each clip header; muted notes and step cells are painted hollow.
- **File browser.** A docked panel (View, File Browser, `F4`) with a folder tree (Favourites, Tags, Bookmarks, drives) over a file list of folders and audio files, a search field (words, `#tag`, `fav`, optionally in sub-folders), instant preview on click with a 5 ms crossfade when switching (live only, outside the routing and the meters), favourites, tags and bookmarked folders kept in `library.json` in the configuration folder, and drag to the arrangement, sample editor and sampler panels like an OS file drop. New crate `atomik-library`; `atomik --file-browser [DIR]` shows the panel at start. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6j-the-file-browser).
- Release archives for Windows (zip), macOS (universal `Atomik Rack.app`, ad-hoc signed, in a zip) and Linux (tar.gz), each smoke-tested before a draft release is created, with per-archive `.sha256` files and a `SHA256SUMS` file.

### Known limitations
- Feedback loops are rejected; insert and return loops use stages instead.
- Parameter changes take effect at 64-frame boundaries (devices smooth audible parameters); notes are sample-accurate.
- No time-stretch for audio clips and no relinking of missing audio files.
- Plugins run inside the application process, so a plugin crash takes the application down.
- No note chase on play or locate.
- Linux VST3 editors that need file-descriptor handlers do not work, and there is no editor embedding on Wayland (the parameter pane is the fallback).
- Release builds are not code-signed or notarized (the macOS app is signed ad hoc); see the Install section of the README for the first start.
