# Architecture

> This document describes the code as it stands. Where it differs from a comment or a name in a test, the code is right.

Atomik Rack is a rack of devices joined by audio, CV and note cables, a song (clips on tracks), a
console, and audio tracks, played by one real-time engine. Everything that touches files, devices or
the screen is kept off the audio thread.

## 1. Crates and the dependency rule

```
                          atomik-app  (bin `atomik`: window, menus, workers)
                  ┌──────┬────┴─────┬───────────┬─────────────┐
            atomik-ui  atomik-render  atomik-audio-io  atomik-midi-io      atomik-play (bin)
                  │      │   │              │               │                    │
                  │      │   └─────┐        └───────┬───────┘      (audio-io, midi-io, render)
                  │      │         │                │
          atomik-model   │   atomik-devices ── atomik-engine
                  │      │         │                │
   atomik-audio-file ◄───┴─────────┼────────────────┘     (ui and render use audio-file)
          │                        │
   atomik-library                  │          (audio-file, ui and app use library)
                                   │
                  atomik-dsp ── atomik-device-sdk ── atomik-core

      atomik-app also uses atomik-dsp (onset detection on a worker);
      atomik-render dev-depends on it (the slicer demo test)

      atomik-testkit: dev-dependency only (core + device-sdk)
```

The picture shows the main arrows only; the table below is exact.

| Crate | Path | Workspace dependencies | `unsafe` allowed in | What it is |
|---|---|---|---|---|
| `atomik-core` | [crates/core](../crates/core) | none | none | Batch constants, time (`SampleRate`, `Tempo`, ticks, `PPQ` = 15 360, `TimeInfo` with the transport position `tick`, `ticks_per_frame`, `loop_wrap`, `time_signature`), the tempo map (steps and linear ramps) and the meter map (`tempo_map.rs`, `meter_map.rs`), params, note events, the scales and chords of `theory.rs` (allocation-free, shared by the note editors and the note players) |
| `atomik-device-sdk` | [crates/device-sdk](../crates/device-sdk) | core | none | `DeviceDescriptor`, the `Device` trait (with stages and `swap_samples`), `ProcessContext`, `Registry` (with `register_with_samples`), `SampleMap` and `Zone` with `reverse` (`samples.rs`), `PlayerData` (step sequencer patterns, `player.rs`), `Device::swap_player_data`, `Registry::register_with_player_data`, `ScopeTap` and `Device::scope_tap` (the ring of recent frames a metering device hands to the UI, `scope.rs`) |
| `atomik-dsp` | [crates/dsp](../crates/dsp) | core | none | Oscillators, the FM synth's routings and bandwidth limiter (`fm.rs`), the band-limited factory wavetables (`wavetable.rs`: twelve generated tables and a 32-wave additive bank, mip levels by FFT truncation), filters, the 24 dB ladder with an anti-aliased drive (`ladder.rs`), envelopes, delay line, dynamics, EQ, fader and pan laws, the Hermite sample reader and the loop and reverse playhead (`sample_read.rs`), the granular sampler's windows, grain pool, clock and anti-aliased read (`grain.rs`), offline onset detection (`onset.rs`), the integer hash `rng::hash64` for position-derived randomness, the seeded requantiser with TPDF dither of an audio export (`dither`), the analyser display (`spectrum`: windowed FFT, one-third-octave bands, peak hold) and the stereo statistics (`stereo_meter`: correlation, balance, width) |
| `atomik-devices` | [crates/devices](../crates/devices) | core, device-sdk, dsp | none | `poly-synth` (and the `mono-bass` and `dual-layer` built on its voice), the `organ`, `wavetable-synth`, `fm-synth`, `sampler`, `granular`, `drum-machine`, `loop-slicer`, `line-mixer`, `stereo-delay`, the `echo`, the six effects, the `guitar-amp`, `step-sequencer`, `arpeggiator`, `channel`, `master`, `audio-track`, the `spectrum` and `phase-meter` analysers (see [DEVICES.md](DEVICES.md)) |
| `atomik-engine` | [crates/engine](../crates/engine) | core, device-sdk, dsp | none | Graph and topology, compile, the real-time `Engine`, the non-RT `EngineHandle`, workers, the song player, console features, audio playback, streaming, the record tap, sample-map swaps (`ControlOp::Samples`), pattern-data swaps (`ControlOp::PlayerData`), the musical position of each batch (`BatchTime`), the scope taps of metering devices (in `Meters`), the reference voice (`reference.rs`, after the master chain) |
| `atomik-library` | [crates/library](../crates/library) | none (serde, serde_json, thiserror) | none | The data behind the file browser and the library, pure and headless: file kinds (audio, MIDI and the library items), `LibrarySection`, device URIs, free item names, rows, `PathKey`, `Tag`, `LibraryDb` (favourites, tags, bookmarked folders; JSON), the search `Query`, natural sort |
| `atomik-audio-file` | [crates/audio-file](../crates/audio-file) | library (symphonia, rubato, hound, sha2, thiserror) | none | Decode, resample, WAV I/O, content hash, peaks, the project audio bundle, the header probe and the streaming decoder of the file browser, the tempo metadata of a WAV file |
| `atomik-video` | [crates/video](../crates/video) | none (serde_json, sha2, thiserror, log, zune-jpeg) | none | The foundation of video tracks, pure and headless, with no codec of its own: finding the FFmpeg the user installed (Settings folder, then `PATH`, version check; `discover.rs`), the one `VideoTool` trait every `ffmpeg` and `ffprobe` call goes through (`SystemTool` with no shell, a list of arguments, no console window and a kill on cancel; a fake for the tests), the `ffprobe` parser (`probe.rs`), the proxy file of small independent JPEG frames with a frame index (`proxy.rs`, `jpeg.rs`), the build of it from the piped frames with progress and cancel (`prepare.rs`), the per-user proxy cache (`cache.rs`), and what the viewer reads it with: the pure-Rust JPEG decode to RGBA (`decode.rs`), the decode worker and its byte-limited frame cache (`frames.rs`) and the exact song-time to frame and timecode arithmetic (`timeline.rs`), and the film export that puts the song's mix and the picture of its film together (`film.rs`) |
| `atomik-update` | [crates/update](../crates/update) | none (serde_json, thiserror, log, ureq) | none | The optional update check, the only network request the program makes: `Version` (semantic order, release candidates included), the list of releases and the choice of the newest eligible one, `UpdateState` (the daily throttle and the skipped version, `updates.json`), `Consent` (the answer to the question) and the one request behind the `HttpGet` trait: `UreqGet` (rustls, HTTPS only, no redirect, no cookie, a timeout) for the application window, `FakeHttp` and `Offline` for tests and every other shell. Pure and headless |
| `atomik-model` | [crates/model](../crates/model) | core, device-sdk | none | The document: rack, cables, console, song, sample pool, sampler zones, clip step grids; commands, undo, `.atomik` files (format v45), track folders, colours and comments (`organise.rs`), the music theory of the note editors (`theory.rs`), sequencer patterns (`steps.rs`), auto-mapping, pad helpers and slicing helpers (`slices.rs`), the generated documents of the stress options and benches (`stress.rs`) |
| `atomik-audio-io` | [crates/audio-io](../crates/audio-io) | core, engine | none | cpal streams driving the engine, duplex input, telemetry |
| `atomik-midi-io` | [crates/midi-io](../crates/midi-io) | core, device-sdk, engine | none | MIDI input and exact event timing, the timed MIDI output scheduler (`out`: a queue per port, the frame-to-time conversion and the output thread), the parser of the control path (`parse_message`), raw ports for control surfaces (`ports`), the control-surface drivers (`surface`: pure, bytes in, actions out, bytes out) and the Standard MIDI File reader and writer (`smf`, pure bytes) |
| `atomik-plugin-host` | [crates/plugin-host](../crates/plugin-host) | core, device-sdk | `src/fpenv.rs`, `src/ole.rs` | Format-neutral plugin hosting: `PluginDevice`, `PluginManager`, slot and hand-shake, scan, cache, catalog, journal, settings, stand-ins |
| `atomik-plugin-clap` | [crates/plugin-clap](../crates/plugin-clap) | core, device-sdk, plugin-host (clack-host, clack-extensions) | `crate` | The CLAP backend (every `unsafe` block has a `SAFETY:` comment) |
| `atomik-plugin-vst3` | [crates/plugin-vst3](../crates/plugin-vst3) | core, device-sdk, plugin-host (vst3, libloading) | `crate` | The VST3 backend, hosting `.vst3` modules through the `vst3` bindings (every `unsafe` block has a `SAFETY:` comment) |
| `atomik-plugin-scan` | [crates/plugin-scan](../crates/plugin-scan) | device-sdk, plugin-host, plugin-clap, plugin-vst3 | none | The out-of-process scanner (bin `atomik-plugin-scan`) |
| `atomik-plugin-ipc` | [crates/plugin-ipc](../crates/plugin-ipc) | core, device-sdk, plugin-host (memmap2, windows-sys, libc) | `crate` | The sandbox wire: the shared audio block, the doorbell, the control-channel frames and messages; the only sandbox crate with `unsafe` |
| `atomik-plugin-sandbox` | [crates/plugin-sandbox](../crates/plugin-sandbox) | core, device-sdk, plugin-host, plugin-ipc | none | The sandbox, app side: `SandboxBackend`, one child process per plugin instance |
| `atomik-plugin-hostd` | [crates/plugin-hostd](../crates/plugin-hostd) | core, device-sdk, plugin-host, plugin-ipc, plugin-clap, plugin-vst3 (winit) | `tests/support/owner_window.rs` (a test helper) | The sandbox child (bin `atomik-plugin-host`): hosts one plugin instance |
| `atomik-render` | [crates/render](../crates/render) | core, device-sdk, engine, devices, model, dsp, audio-file, plugin-host, plugin-clap, plugin-vst3, plugin-sandbox (dev: testkit) | none | Patch and project bounces, the audio export of the song or the loop (`export`), bundle rules, the shared sample-map builder, `probe_devices` (a bounce that stops early and reads the metering devices), the `atomik-render` CLI |
| `atomik-play` | [crates/play](../crates/play) | core, device-sdk, engine, render, audio-io, midi-io | none | `atomik-play`, a live player for patches |
| `atomik-ui` | [crates/ui](../crates/ui) | audio-file, library, core, device-sdk, dsp (the wavetable panel's frame display and the table editor's maths), model | none | Platform-independent views, widgets, display lists, theme |
| `atomik-app` | [crates/app](../crates/app) | core, device-sdk, devices, dsp, engine, model, audio-file, library, video, update, render, ui, audio-io, midi-io, plugin-host, plugin-scan, plugin-hostd, plugin-sandbox | none | The `atomik` application: `AppCore`, `Shell`, background workers (import, onset detection, file browser), the window |
| `atomik-mcp` | [crates/mcp](../crates/mcp) | core, audio-file, device-sdk, library, model, render, app, ui (feature `cpu-canvas`), plugin-host | none | Development tool, not shipped: the MCP server (bin `atomik-mcp`) that bounces, measures and compares audio and drives a headless `Shell` for QA; see [MCP.md](MCP.md) |
| `atomik-testkit` | [crates/testkit](../crates/testkit) | core, device-sdk | `src/no_alloc.rs` | Test-only: fakes, signal analysis, the allocation guard, the bench recorder (`bench.rs`) |

Rules (all hold in the `Cargo.toml` files today):

- A crate depends only on crates below it.
- `atomik-engine` and `atomik-devices` never depend on each other in production code. They meet only
  through `atomik-device-sdk`: the engine sees a device as `Box<dyn Device>` plus a
  `&'static DeviceDescriptor`. (The engine's tests use the devices as dev-dependencies.)
- The engine does not know the model or the audio files. The app turns the model into engine-native
  `SongInput` data, and implements the engine's `StreamSource` trait over the file reader.
- `atomik-render` is the offline composition root and `atomik-app` the live one. They share one song
  builder, [`project::song_input`](../crates/render/src/project.rs), so a bounce and live playback
  build identical engine input.
- `atomik-testkit` is only ever a `[dev-dependencies]` entry.
- `atomik-mcp` is a development tool above the app: nothing depends on it, release archives do not
  contain it, and it only wraps code that has its own tests (bounces, documents, the shell). It
  opens no audio device. `atomik-ui`'s `cpu-canvas` feature (a `vello_cpu` canvas, also used by the
  preview tests) is turned on only by it and by the UI's own tests, so the app does not link
  `vello_cpu`.

To check the rule yourself, read the `[dependencies]` section of each `crates/*/Cargo.toml`.

## 2. Data flow, in text

### 2.1 Patch file to WAV

```
patch.json ──Patch::from_json──► Patch ──build::build──► Graph ──Graph::compile──► RenderPlan
                                  (validated,                (devices, cables,      (order, slots,
                                   JSON-path errors)          clamped params)        prepared devices)
                                                                                         │
WAV ◄──wav::write── Vec<Vec<f32>> ◄──render_offline── Engine + EngineHandle ◄────────────┘
                                     (host blocks of --block frames, 64-frame batches inside)
```

1. **Parse and validate** ([crates/render/src/patch.rs](../crates/render/src/patch.rs)): unknown
   fields, bad ranges and bad ids are errors that name a JSON path. The format is in
   [examples/patches/README.md](../examples/patches/README.md).
2. **Build** ([build.rs](../crates/render/src/build.rs)): creates a `Graph` from an `EngineConfig`,
   creates each device from the registry, sets params by key (clamped, with a warning), connects the
   cables through `Graph::connect`, and turns every note into an On and an Off `ScheduledNote`.
3. **Compile** (`Graph::compile` in [plan.rs](../crates/engine/src/plan.rs)): re-validates every
   cable, orders the devices with Kahn's algorithm (ties by `DeviceInstanceId`), assigns buffer
   slots by liveness, calls `prepare` once per device with a seed mixed from the patch seed and the
   device id, re-applies the stored params and preallocates all scratch.
4. **Render** (`render_offline`): drives `Engine::render` with host blocks, pushes due notes,
   collects retired plans and removes the engine's 64 frames of latency.
5. **Write** ([wav.rs](../crates/render/src/wav.rs)): 32-bit float (bit-exact) or 24-bit PCM.

### 2.2 Project open, edit and live sync

```
 .atomik file ──load_project──► Document (RackDoc + undo history)
                                     │  Command ──apply──► SyncOps (what changed)
 UI events ──views──► UiAction ──AppCore──┘                    │
                                                               ▼
                         AppCore translates SyncOps into EngineSink calls
        reset → insert devices (ids 1, 2, 3 …) → routing → song → transport/loop → params
                                                               │
                              EngineSink (live: Session, tests: RecordingSink / OfflineSink)
                                                               │  wait-free queues
                                                               ▼
                                                     Engine (audio thread)
```

- The **document** ([crates/model](../crates/model)) is the single source of truth. It is pure data:
  no UI and no engine. `Document::apply(Command)` validates, changes the `RackDoc`, records an undo
  step (drag edits merge by a key) and returns `SyncOp`s. Commands cover the rack (add or remove a
  device, cables, params), the console, the song, the sample pool (`RegisterAsset`, the one command
  that is not an undo step; `RelinkAsset`, which swaps every reference to one pooled asset for
  another in one step) and `Command::Batch` (several cable commands as one undo step).
- **Invariants are checked, not assumed.** `Edit::perform` returns an `InvariantError` when an edit
  names a device, track, lane, clip, cable or index the document does not have: every leaf checks
  before it changes anything and a compound puts back the leaves it had performed, so
  `Document::apply`, `try_undo` and `try_redo` either do their whole step or leave the document, the
  history and the revision as they were (`ModelError::Invariant`; `undo` and `redo` keep their
  `Option` and report such a step as nothing done). `RackDoc::validate` lists the rules in one place
  (column order, the tempo mirror, the console naming only devices the rack has, clips and notes
  inside `0..=MAX_TICK` with sorted lanes, every stored number finite); debug builds check it after
  every `apply` and in `Document::new`, and `save_project_checked` refuses to write a document that
  fails it. The song End follows every edit that brings a clip in (a clip, a lane or a whole track,
  so a pasted or duplicated track too) through one rule, `song_end_growth`. In the app, `AppCore`
  runs a command, an undo and a redo under `catch_unwind`: a panic is shown as "Internal error,
  document restored", closes the drag-merge group and asks the recovery writer for a snapshot.
- **Ordering rules** when syncing: adding a device is insert, route, then submit the song; removing
  is song without its track, routing, then `remove_device`; a restart is reset, inserts, routing,
  song, loop, metronome. The engine applies these in queue order at batch boundaries.
- **Rack columns.** The top-level rack is shown as `RackDoc.columns` columns side by side (empty
  columns count; at most 32). Each `RackDevice` has a `column`, and `RackDoc.devices` stays one flat
  list **stable-sorted by column**, so a column is a contiguous run of it and nothing that reads the
  flat order (graph build, engine, console, song) knows about columns; `RackDoc::check_columns`
  validates the invariant. A position is a `RackSlot { column, index }` (index within the column);
  `RackDoc::resolve_slot` turns it into the flat index (column 0 never goes above the locked Master
  Section; a column one past the last appends a column in the same step). Slot commands
  (`Command::AddDeviceAt`, `WorkflowCommand::MoveDevicesToSlot`, `PasteDevicesAt`,
  `CreateInstrumentAt`, `CreateEffectAt`) and `AddRackColumn` / `RemoveRackColumn` are single undo
  steps built from the `Layout` and `Columns` edits and report `ViewChanged` only. `CreateEffectAt`
  routes as if the device above the slot (the previous device of that column) were selected, and
  does not route at the top of a column. A device added or moved by a flat index takes the column of
  the device before it. Container insides are always one column (column 0).
- **Rack sorting** ([model/src/rack_groups.rs](../crates/model/src/rack_groups.rs)). A device group
  is what belongs to one track: the instrument (or audio track device), its note players, its
  console channel with the channel's inserts, and the effects cabled after the instrument;
  `RackDoc::device_groups` derives it from the document and stores nothing. An effect two
  instruments both reach, and an effect on a send/return slot, is in no group, so instruments that
  share a return stay separate. `RackDoc.sort_mode` (`RackSort`) is `Free` (manual columns, the
  default), `PerTrack` (loose devices such as the master stay in column 0, each group has a
  column of its own and no column is empty) or `AutoGroup` (the members of a group are gathered
  into one run). `Command::SetRackSort` changes the mode and arranges the rack in one undo step;
  while the mode is not `Free`, every edit that adds, removes or moves a top-level device ends with
  the arranging `Layout` (and `Columns`) edit inside the same compound, and a move of one device
  moves its whole group. It reports `ViewChanged` only and never changes a sound. It is saved as
  the root `"rack_sort"` (`"per_track"` or `"auto_group"`, written only then, ignorable), so an
  older project loads as `Free`. View, Rack Sorting picks the mode; View, Single Column
  (`RackView::set_single_column`, view state, never saved) lays the columns out as one without
  touching the document; `rack.select_group` selects the group of the selected device.
- **Track Panel** ([ui/src/track_panel.rs](../crates/ui/src/track_panel.rs)). View, Track Panel
  docks a column on the right of the window (`Pane::TrackPanel`, `Layout::track_panel`; the choice
  is not saved) that lists the track selected in the arrangement: its note players, its
  instrument, the insert slots of its console channel, the sends and the fader with its mute.
  It stores nothing and edits through the commands the rack and the console use: a bypass is a
  `SetParam`, a move is `MoveDevices` (players, routed) or `MoveInsert`, a removal is
  `RemoveDevices` or `SetInsert` with no effect, a send switch and the fader (one undo step per
  drag) are `SetParam`. Each move and removal is planned on a scratch document when the panel
  syncs and its button shows only when the model accepts it. A note track also has a Groove
  section: the arrows send `SetTrackGroove` to step through none, the built-in grooves and the
  song's (see 2.14), and a strength bar sends one `SetTrackGroove` when the button is released. The panel is rebuilt only when the
  document revision or the selected track changes (`AppCore::sync_track_panel`). Adding and
  replacing a device from the panel is not built; the rack and the console do it.
- **Rack columns in the view** ([ui/src/rack/columns.rs](../crates/ui/src/rack/columns.rs),
  [device_drag.rs](../crates/ui/src/rack/device_drag.rs)). `RackView` lays the top-level rack out as
  `doc.columns` columns side by side, column `c` at x = `c x (RACK_WIDTH + COLUMN_GAP)`
  (`COLUMN_GAP` is 32), each with its own rails and stacked from y = 0; the Audio I/O strip heads
  column 0 only, `content_height` is the tallest column and `content_width` covers all of them, and
  the horizontal origin, scroll range and clamp use that width, so a single column is laid out
  exactly as before. A container's inside stays one column. Every panel, hit rectangle, socket and
  menu position carries its column's x, so hit testing, selection, folding and cables work in any
  column. A move drag is two-dimensional: the pointer picks the column (nearest by x) and the index
  among the devices that stay in it (the slot), the drop line is drawn in that column, and the
  release emits one `MoveDevicesToSlot` (never a no-op; inside a container the flat `MoveDevice` /
  `MoveDevices` as before). While a device or a browser row is dragged, a dashed "New column" zone
  shows to the right of the last column; dropping past the last column uses `column == doc.columns`,
  which the model turns into a new column in the same undo step. A press on a device-browser row
  that travels past the 3 px threshold starts a `Drag::Browser`: a ghost label follows the pointer,
  the same drop line shows, and the release over the rack emits `CreateInstrumentAt`,
  `CreateEffectAt` or `AddDeviceAt` (the creation options as for the double-click, Shift inverting
  the route); a release over the browser or outside the rack, or Escape, emits nothing. The
  double-click add is unchanged. Both drags scroll the rack when the pointer is within 48 px of an
  edge of the rack area (speed up to 700 px/s, horizontal and vertical): the host calls
  `RackView::tick(now_seconds)` from `Shell::frame_at` (the first tick of a drag only starts the
  clock, each step is at most 50 ms, so it is deterministic under test) and keeps ticking while
  `is_auto_scrolling()`. The rack context menu has Add Column and Remove Column (`rack.add_column`,
  `rack.remove_column`; the column of the right click, disabled with one column) and runs
  `AddRackColumn` / `RemoveRackColumn`. While the new-column zone shows (a drag), the range includes
  it: a rack that fits and is centred moves left only as far as it takes to keep the zone on screen
  (it never moves when the zone fits beside it), and a drop is worked out in the layout of the drag.
  Pasting from the rack's context menu (`WorkflowCommand::PasteDevicesAt`) lands at the slot under
  the right click (`RackView::take_context_slot`, cleared by any other click or key); a keyboard
  paste with a selection lands below the selected device in its column; a container paste is
  unchanged. A browser plugin that is neither instrument nor effect is added at the drop slot with
  `PluginCommand::AddAt` (column included, one undo step). Escape ends a browser drag whichever pane
  has the focus (`Shell` forwards it to the rack while `RackView::is_browser_dragging`).
- **Device names in the rack** ([ui/src/rack/name_input.rs](../crates/ui/src/rack/name_input.rs)). A
  double-click on the name of a device's plate (`Hit::Name`, `paint::name_rect`: the two name lines
  unfolded, the stretch before the type label folded; the Master Section and the interface have
  none) types the name in place, with the editor of the pattern names: a field with a caret replaces
  the name (`PanelInfo::editing_name`), Enter commits, Escape cancels, a press elsewhere commits and
  a press on the field keeps editing, and `RackView::is_text_owner` is true meanwhile so the shell
  resolves `Text` over `Rack` (the detached Rack window is the same view). The commit is
  `Command::Rename` with the local key (mapped to the path key inside a container like every
  command); an unchanged name sends nothing. The device name **is** the track and channel name: the
  arrangement and the console already send this same command (a channel shows the name of its source
  instrument), so renaming here renames the track and the channel, and there is no second field to
  keep in sync. The model trims the name and, for a blank one, gives the device the default of a new
  device of its type (the descriptor name, made unique among its siblings, `rules::default_name`); a
  bus keeps refusing a blank name. The name has a tooltip ("Double-click to rename"); a double-click
  anywhere else on the panel (a knob's reset, a pad, the fold button) is unchanged.
- **Rack navigator** ([ui/src/rack/navigator.rs](../crates/ui/src/rack/navigator.rs)). A strip
  `NAVIGATOR_WIDTH` (136) logical pixels wide at the right edge of the rack (below the breadcrumb
  bar inside a container) that `rack_area()` gives up, so nothing draws under it; a window too
  narrow to leave the rack 240 px hides it. It draws one uniform-scale overview of the whole
  content: the narrower of "all columns fit the width" and "the whole height fits", so the strip
  always shows every column and the viewport rectangle (the rack area's part of the rack, with the
  zoom) is the only moving part; a very tall rack gets small pictures, not a scrolling overview. Each
  device is a miniature of its real panel: the panel's own display list (`record_panel_list`, the
  list the rack caches; one the rack already recorded is reused) is replayed under a scale
  transform by `replay_scaled`, which drops fills too small to see, keeps text of 4 px or more,
  draws smaller text as a bar of its colour and drops text under 1.4 px. A folded panel is drawn
  folded, the Audio I/O strip tops column 0, and a panel shorter than 4 px is a flat block of its
  device colour (at least 1.5 px high). Each panel is recorded for the strip once per picture
  change (`Mini`, keyed by `still_revision`: the panel revision without the live meter, scope and
  analyser feeds counted by `bump_live`, so playback never records it again), and the miniature is one
  cached `DisplayList` keyed by those stamps, the layout revision (bumped by `rebuild`), the shown
  side and the strip geometry; scrolling, selecting and hovering redraw only the viewport
  rectangle and the selection outlines. A press outside the viewport rectangle centres it on the
  point, a press inside keeps the grab point, a drag follows the pointer (`Drag::Navigator`, also
  outside the strip, clamped to the content), the wheel scrolls as over the rack (Shift sideways).
  The strip never starts a device drag, and a device or browser row released over it is dropped.
  Inside a container it shows the container's single column. View, Rack Navigator
  (`view.toggle_rack_navigator`, no default key) toggles it; the choice is `show_rack_navigator` in
  `ui-state.json` (default on). A new `RackView` has it off, so hosts and goldens that do not ask
  for it are unchanged.
- **Project files** are `.atomik` JSON, format version 45 (older versions still load; version 45 files say
  `"compat": 44` and list the launcher, the per-note expression and the song's `"key"`, `"key_changes"` and
  `"chords"` (the song key and the chord lane, written only when set; see `Song::harmony`) as ignorable, so a reader of
  44 opens them; version 44
  adds a note track's own `"controllers"` (pitch bend, aftertouch and CC lanes outside the note clips, written
  only when a track has any; a reader of 43 refuses them, since it would play the track without them; see
  2.12); version 43
  adds a track's `"groove"` and `"groove_strength"` (the live groove, written only when a track has one;
  a reader of 42 refuses them, since it would play the notes ungrooved; see 2.14) and a folder's
  `"muted": true` and `"solo": true` (each written only when set, with the same refusal; see 2.16); version 42
  adds the root's `"rack_sort"` after `"tune_cents"` (written only when the rack is sorted, ignorable;
  see "Rack sorting") and, to a container's `"inner"`, a macro's `"kind": "switch"`, macros 9 to 16
  (the list has 16
  entries instead of 8 when one of them is used) and a split's `"pass_bend"`, `"pass_cc"` and
  `"pass_pressure"` (each written only when set; a reader of 41 refuses them; see 6e); version 41
  adds the device field `"mods"` (the modulation slots in use, written only when one is; see 3,
  "Modulation matrix"), a note clip's `"param_lanes"` (device-parameter lanes, written only when it has
  any) and the bent point `[tick, value, "bend", amount]` (written only when used; see 2.12); version 40
  adds the root's `"tune_cents"` after `"info"` (the master tune in cents, written only when not 0, a
  file without it loads at 0, refused in a version 39 file; see 3, "Master tune"); version 39
  adds the console's `"send_follow": true` (the sends of a strip that plays into a bus follow the
  bus fader and mute; written only when on, a file without it loads with it off) and
  `"parallels": [[parent, child], ...]` (the parallel channels, written only when there are some;
  both refused in a version 38 file; see 5, "Mix channels and parallel channels"); version 38
  adds the root's `"info"` after `"macros"`, the song information `{"title"?, "author"?, "email"?,
  "url"?, "notes"?}` (`song_info.rs`), written only when something is set and kept as typed, and
  refused in a version 37 file; see 6, "The app: workers and rules"; version 37
  adds the console's `"align_returns": true` (return alignment, written only when on; a file
  without it loads with it off; refused in a version 36 file; see 5, "Plugin delay compensation"),
  and a MIDI Out device's `"midi_out": { "port": name }`, the output port it sends to, written only
  once a port is chosen and refused on any other device type and in a version 36 file (see 4,
  "MIDI output"); version 36
  also adds a device's `"patch"` after its `"name"`, the name of the patch it took its sound from
  (display only, written only when non-empty; see 6l); version 36 adds an automation lane's
  `"muted": true` and `"static": value` after its `"folded"`, written only when set (see 2.6a);
  version 35 adds the song's `"tails": true` after its `"next_clip_id"`, written only when on (a
  note then plays to its own end past its clip end, see 2.13); version 34 lets a device without a
  Notes input (a console channel, an effect) have a track of automation lanes alone, see 2.6b;
  version 33 adds a drum generator's `"drums"` after its `"curves"` (16 lanes of 64 steps per
  pattern, one lane per line), written only for the types that take drum data, such a device having
  no `"steps"`; version 32 adds a CV sequencer's `"curves"` after its `"steps"`, written only when
  the device has curves stored; version 31 adds a track's `"notes_to"` after its `"no_chase"`, the
  device its notes enter (the top of a stack of players above its instrument), written only when set
  (see 2.22); version 30 adds a notes lane's `"name"` and `"muted": true` after its `"kind"`, and
  lets a track have several note lanes before its automation lanes, each written only when used (see
  2.21); versions 23 to 29 add audio editing, warp, frozen tracks, VCA groups, mixer snapshots, MIDI
  mappings and macro banks (the module docs of `project/mod.rs` list each); version 22 adds the song's
  `"groups"` (folders: `"id"`, `"name"`, `"parent"?`, `"collapsed"?`, `"color"?`) after its
  `"grooves"`, a track's `"group"`, `"color"` and `"comment"` after its `"no_chase"`, an automation
  lane's `"folded": true` after its `"write"` and a take group's `"open": true` after its `"clips"`,
  each written only when used (see 2.16); version 21 adds the song's `"grooves"`,
  `[{"name", "step": "eighth" | "sixteenth" | "thirty_second", "slots": [[offset, velocity]…]}]`
  after its `"track_heights"`, written only when non-empty (see 2.14); version 20 adds a lane's
  `"take_groups"`, written only when the lane has any (see 2.13); version 19 adds a note clip's
  `"controllers"`, a list of
  `{"kind": "pitch_bend" | "pressure" | "cc" | "poly_pressure", "number"?, "points": [[tick, value, "linear" | "step"]…]}`
  after its `"scale"`, written only when the clip has any (see 2.12); version 18 adds an automation
  lane's `"write"` (`"overwrite"` or `"latch"`, written only when it is not the default Touch);
  version 17 adds a track's `"no_chase": true`, written only for a track that does not chase notes,
  so a file without it chases; version 16 adds a note clip's `"scale"`,
  `{"root": 0..=11, "kind": "<scale>"}` after its `"grid"`, written only when the clip has one (a
  chromatic scale is never stored); version 15 adds the song's `"track_heights"`, an object from
  device key to the height in pixels of that track's first lane in the arrangement (`24..=400`),
  written only when non-empty; version 14 adds the song's markers and range markers,
  `"markers": [{"tick", "name", "color"?}]` and `"ranges": [{"start", "end", "name", "color"?}]`
  (colours `"#rrggbb"`), each written only when non-empty, so a song without them differs from its
  v13 file only in the version line; version 13 adds tempo ramps and meter changes: a tempo event
  may carry `"ramp": true` (written only then) and `"meter"` is a list of `{"bar", "num", "den"}`
  changes instead of the single `[beats_per_bar, beat_unit]` pair, which the reader still takes in
  any version; version 10 added the per-note mute flag, written as an optional fifth element `true`
  of a note; version 11 adds the rack columns: an optional `"column"` on each top-level device,
  after `color`, and an optional root `"rack_columns"` after `input_channels`, both omitted for a
  single-column rack, so versions 1 to 10 load as one column and an unchanged single-column project
  differs from its v10 file only in the version line; version 12 adds the optional `"buses"` of the
  console, below, omitted when there are none, so a project without buses differs from its v11 file
  only in the version line). Patches carry no columns and no buses. Saving is canonical: save, load,
  save gives identical bytes.
- **One table drives the reader.** Every kind of object a project file holds (the root, a device,
  a track, a lane, a clip, the console and so on) is an `Obj`, and every field it may carry is one
  row of `SPECS` in [project/schema.rs](../crates/model/src/project/schema.rs): its name, the
  version that added it (`since`), the version that dropped it (`until`), whether a file must have
  it and whether an older reader may skip it (`ignorable`). The reader asks the table for the
  fields of an object at the file's version, so a new field is one row plus the code that reads and
  writes its value. The module is split by area under
  [crates/model/src/project](../crates/model/src/project) (`read_*.rs`, `write*.rs`, `schema.rs`);
  its `mod.rs` holds the format description.
- **Newer files.** A file whose version is above the reader's is refused unless its root says an
  older reader may open it: `"compat"` is the oldest version that can, and `"ignorable"` lists the
  fields to skip, each by its path (`song.tracks[].lanes[].clips[].tail`, arrays written `[]`) with
  the version that added it. The reader drops those it does not know, with one warning each, and
  reads the rest as its own version; any other unknown field is still an error. The writer adds the
  two root fields only when every change since `compat` is ignorable (a row marked `ignorable`, and
  no required or removed field and no entry of `BREAKING_VERSIONS`), so a file of today's format
  carries neither. New optional fields should be ignorable; see
  [DECISIONS.md](DECISIONS.md#l-format-version-cadence).
- **Nothing is dropped silently.** `load_project_with_warnings` returns one line for everything the
  file had that the document does not (see its docs): the `boundary` rename, a version 1
  instrument that got no console channel, the height of a device without a track, a comp clip id
  that is not on its lane, and the fields skipped from a newer file. The app logs each line and
  shows "N items ignored while opening".
- **One saved file per version.** `crates/model/tests/fixtures` holds a project written by each
  released format version (`demo-v13` onwards come from the demo project at the commit that
  introduced the version, see `scripts/extract-format-fixtures.sh`; versions 2 to 5 have none).
  `format_corpus.rs` loads every file, checks that its resave is canonical, and that a file of the
  current version resaves byte for byte. A new version adds its file.
- **Errors.** A file that cannot be read is `ModelError::Load { path, message }` (the JSON path
  and what is wrong there), a batch that is empty or holds a command that cannot be batched is
  `BadBatch`, and a command that names an asset the pool lacks is `MissingAsset`.
- **The undo history is bounded.** A document keeps at most `MAX_UNDO_STEPS` (1,000) undo steps
  and an estimated `MAX_UNDO_BYTES` (512 MiB) of what they hold (`Edit::approx_bytes`: clips,
  devices and lists count by size, and the notes of a clip count in full for every step that holds
  the clip). The oldest steps are forgotten first, the latest step is always kept, and the assets
  only forgotten steps referenced leave the pool at the next `prune_unreferenced_assets`.
  `Document::set_undo_limits` changes the limits. A new, opened or replaced document starts with
  no history. The defaults are a working assumption
  ([DECISIONS.md](DECISIONS.md#k-undo-history-budget)). Undo runs on the step itself
  (`Edit::invert` swaps old and new in place, then back), so it copies none of the step.
- **A big note clip's edit is sparse.** `SetClipContent` on a note clip of `SPARSE_MIN_NOTES` (2,048) or more
  notes that keeps the list's length and changes at most an eighth of its positions is stored as
  `Edit::Notes` (the changed positions with the note before and after, `content_edit` in
  [song_cmd.rs](../crates/model/src/song_cmd.rs)) instead of the clip twice over; every other change still
  replaces the clip. A sparse edit is a barrier for `fold_clip_edit`, so a drag's merged steps stay in order,
  and `approx_bytes` counts only the changed notes. A test on a 50,000-note clip checks that forty inspector
  steps fit a 1 MiB budget and undo and redo walk the notes exactly.
- **A swap edit is one table entry.** An edit that only replaces one value (a device's name, colour,
  fold or patch name, the column count, the song information, the master tune, a track's chase,
  Direct Record or note entry) is an entry of `Swap` (`history/swap.rs`): its name, what it is keyed
  by, the type of the value and how to store it. The inverse and the perform arm come from the
  table, so adding one touches nothing else.
- **A drag is a `Gesture`.** `Document::begin_gesture` returns a guard that merges the commands of
  one merge id into one undo step and ends the group when it is dropped, on any path out of the
  scope. `end_merge` remains for a gesture that spans events, where no guard can be held (the
  pointer release). `undo` and `redo` need no registry.
- **Fades and envelope points stay inside their clip.** `AddAudioClip`, `SetAudioClip` and
  `StripSilence` refuse a fade or a gain envelope point longer than the clip (in frames at the
  song's tempo). A fade a later trim or tempo change leaves past the clip is a request: flattening
  plays it clamped to each repetition. A project written before format 40 loads clamped, with one
  warning per clip.
- **Copies of a document are cheap.** `RackDoc::song` is an `Arc<Song>` and the notes of a note
  clip are a `NoteList` (shared, copied on first write), so cloning a document, a planner scratch
  or a clip copies no note: a rack, console or workflow command costs the same on a song of a
  million notes as on an empty one. Read the song through `Deref`; write it through
  `RackDoc::song_mut`, which copies the song's tracks, lanes and clips (never their notes) only
  while another copy shares it.

### 2.3 Audio import

```
 drop / File > Import > Import Audio ──UiAction::ImportAudio { paths, target, tick }──► AppCore::import_audio
        │
        ▼  one worker thread "atomik-import" per drop or pick
   decode (symphonia) ─► resample to session rate (rubato) ─► content hash ─► Audio/<hash>.wav
        ─► peaks (Audio/.peaks/<hash>.peaks) ─► read back (RAM, or open a stream source)
        │  Msg over an mpsc channel, one per file
        ▼
   UI thread, every frame (AppCore::poll_background):
     RegisterAsset ─► AddAudioClip (existing track)   or   AddAudioTrack ─► AddAudioClip (new track)
     ─► SyncOps ─► song resubmitted to the engine; peaks handed to the arrangement view
```

Messages carry a generation number, so work for a document that was replaced is dropped. A
`DoneGuard` always sends the completion message, even if a worker panics.

The decoded audio is moved, never copied, along that chain: `Bundle::import` hands the decoded
buffer to `resample_owned` (a file already at the session rate comes back as it is), `validate`
checks the invariants by inspection, and the decoder sizes its buffers once from the container's
frame count (believed only as far as the file's own size allows). An import therefore peaks at
about the decoded size; `crates/audio-file/tests/import_peak.rs` holds the figure. WAV files are
written and read in 64 KiB blocks, and a `WavStreamReader` reads ahead 4,096 frames, so sequential
playback in small chunks costs one read per block (`read_calls` counts them).

**The project's sample rate and channel counts** are document fields with commands.
`Command::SetInputChannels` and `SetOutputChannels` change a count (cables to the channels that go
away leave in the same step). File > New at Sample Rate starts a project at 44.1, 48 or 96 kHz.
File > Convert Sample Rate (`AppCore::convert_sample_rate`, `app_core_rate.rs`) resamples every
asset the project uses on a worker (`Bundle::convert_rate`, the sinc resampler; the originals
stay), registers the copies and applies one `Command::ConvertSampleRate { rate, assets }` step: the
rate changes, every clip, take, frozen render and zone points at its copy, and the positions that
count frames (source offsets, fades, gain envelope points, warp marker sources, zone ranges) scale
by the rate ratio while positions in ticks stay. Undo restores the rate and the originals, which
stay loaded. A rate or channel change restarts the engine (`rebuild_engine_in_place`): the
transport returns to the tick it was at and keeps playing, but effect tails do not survive.

**Relinking missing audio** goes through the same worker. `AppCore::relink_asset(from, path)`
imports the picked file as a batch whose job names the missing asset it stands for; when the file
lands (`app_core_relink.rs`) there are two cases. If its content hash **is** `from` (the file was
only moved or renamed) the bundle simply has its file back: the clips load again and the document
does not change, so there is no undo step. Otherwise `RegisterAsset` (under the new file's name) and
one `Command::RelinkAsset { from, to }` follow, which swaps every audio clip and every zone
(samplers, slicers, drum pads, inside containers too) in one undo step; the old pool entry stays
while the history references it, and a replacement too short for a zone's frame range or a clip's
source offset is rejected, naming the file, with nothing changed.
`AppCore::relink_search_folder(root)` first runs a scan worker (`audio_relink.rs`: breadth first, at
most 8 levels and 50 000 entries, symbolic links not followed, files in name order) that matches the
missing assets by original file name in any case (when several files share a name, the one whose
header agrees on channels and length wins, else the shallowest), then relinks the matches as above;
unmatched assets stay missing. Each finished batch toasts `relinked N, M still missing`. The missing
list (`AppCore::missing_assets`) follows undo and redo: an asset a relink replaced is missing again
once the document uses it again. In the window, File > Project Files > Relink Missing Audio... (`file.relink_audio`,
enabled while audio is missing) opens `RelinkView`, a modal list with LOCATE... per file and SEARCH
FOLDER... and PROJECT FOLDER..., which raise `ShellRequest::PickRelinkFile`, `PickRelinkFolder` and
`PickRelinkProjectFolder`; the MCP tool `session_relink` answers them headlessly. PROJECT FOLDER...
(`AppCore::relink_project_folder`) is for a project whose `.atomik` file was moved without its
folder, or unpacked from an archive: the picked folder (or its `Audio` folder, or the folder beside
the project file that holds `<name>/Audio`) gives back each missing file by its hash, copied into the
session bundle and loaded by the same adopt worker a library item's audio uses, with no decoding.

**Collect and relocate** are the save worker's uses of `Bundle::collect_from_with` (`audio-file`, `CollectReport`:
copied, already present, missing, bytes, cancelled). `AppCore::collect_files` (`file.collect_files`,
File > Project Files > Collect Files) is a Save in place whose destination is the project's own bundle folder
(`collect_root`: the file's folder when it is named like the file, else the sibling `<name>/`); the job copies
each used asset the destination lacks, with
its peak cache, from the session bundle and then from the folders near the project file that hold an `Audio/`
directory (`audio_folders`), writes the project atomically and removes the orphans of the destination only. Save As
uses the same copy instead of copying the whole `Audio/` tree. Audio that was missing and was found is loaded by the
load worker, and a toast is the report. Files that the text state of a plugin names outside the project
(`RackDoc::plugin_file_refs`) are gathered by `AppCore::collect_plugin_files` on the UI thread, from `prepare_save`,
for Collect Files and for any save to a new place: `collect_external_file` (`audio-file`) copies each into the
`Files/` folder of the bundle folder (never over a file; a size cap of 64 MiB) and `rewrite_paths` points the state
at the copy through `PluginCommand::SetState`, the later plugins joined to the first undo step. Binary states are
never read, and nothing is collected into the factory folder. Assets are named by their hash, so the project text
never changes.

The copy is read in blocks and watches a `CollectProgress` (a cancel flag and a file count) that the UI thread holds in
the running `Flight`: `AppCore::cancel_collect` (`file.cancel_collect`) makes the worker stop before the next block, no
file is left half written (a copy goes to a temporary file first), nothing is written and nothing removed, and the
status line (`save_progress`) counts the files while it runs. `AppCore::collect_preview` (`file.collect_preview`) asks
`plan_collect_into` what a collection would do without creating anything. `AppCore::move_project`
(`file.move_project`, the platform's folder dialog `ShellRequest::PickMoveFolder`) is a Save As into
`<folder>/<name>/` whose job also runs `retire_old_project` (`audio-file`, `relocate.rs`) once the new project file is
written: it checks that the new file reads back as written and that `verify_copy` finds every asset of the old bundle in
the new one with the same bytes, and only then removes the old project file and the old copies of the assets the project
uses, then the old folders that are empty (`remove_dir`, never recursive). A place that is the old one, inside it or
around it makes it remove nothing. Other project files in the old folder or below it are all read first (with the
versions kept beside them, `VersionStore::assets`) and the old audio any of them names stays; only the rest of the
old audio goes, and if any project, version index or folder cannot be read (a link is not followed, and a folder
more than twelve levels down counts as unreadable) none of the audio goes, only the old project file
(`RetireReport::unreadable`, `kept_audio`). That loop is the only place old audio is removed. A failed check is an
error that leaves the old project as it was. `AppCore::move_project` and `AppCore::collect_files` refuse a project in
the factory folder (`AppCore::in_factory`). The project's versions (below) are copied to the new place and compared
byte for byte with the same checks before the old ones are removed.

Versions are [`audio-file/src/versions.rs`](../crates/audio-file/src/versions.rs): `VersionStore` reads and writes the
index (`versions.txt`, tab separated, one line per version: number, automatic or named, protected, time, file, audio,
name) of the folder `versions_dir` finds (`Versions/` in the bundle folder, else `<name> Versions/` beside the file; an
existing index wins). Each version is the text of a project file named `<name> v<number>.atomikver`, deliberately not
`.atomik` so no other project check mistakes it for a project sharing the folder. `prune` removes only listed,
automatic, unprotected versions beyond the number to keep, and a name in the index that is not a plain file name of that
folder is dropped when the index is read, so no delete can reach outside it. `Job::run` in `app_core_save.rs` keeps
the file a save replaces (`keep_replaced_file`: not when it equals the new text or the newest version, failures only
logged), and the audio the kept versions list (`scan_assets` of the text) joins the set the orphan tidy keeps.
`AppCore::save_version`, `versions`, `set_version_protected`, `restore_version` and `compare_version` are in
`app_core_versions.rs`; `compare_version` loads the two songs and hands them to `compare_docs`
([`model/src/doc_compare.rs`](../crates/model/src/doc_compare.rs)), a pure diff of two `RackDoc`s into `DocDiff`
sections (song, tracks, devices, mixer) of short text lines, which the panel shows with `VersionsView::show_report`; the
panel is [`ui/src/versions_panel.rs`](../crates/ui/src/versions_panel.rs) (`VersionsView`, `UiAction::Versions`), opened by
`file.save_version` and `file.versions`. Restore after the question about unsaved changes is
`PendingAction::RestoreVersion`. Archives and Collect Files write only the project text and audio, so they leave the
versions out.

Backup copies are [`audio-file/src/backup.rs`](../crates/audio-file/src/backup.rs): `backup_project` copies the saved
project file to `<folder>/<name>/<name> <YYYY-MM-DD HHMMSS>.atomik` (UTC) through a temporary file and a rename, first
copying the audio its text names (`scan_assets`) that `<folder>/<name>/Audio/` does not hold yet, then prunes: copies
beyond the newest N go, and an audio file goes only when no `.atomik` file left in the folder names it (all of them
are read; one that cannot be read, or is over the size cap, keeps all the audio, the same rule as Move Project). Files
that are not copies are never removed. `list_backups` reads the copies of one project, newest first. In the app,
`finish_save` calls `AppCore::backup_after_save` once a save is confirmed (a save worker's result or a blocking save,
never an autosave snapshot): it hands a worker (`BACKUP_THREAD`) a request that owns everything (the file, a clone of
the bundle, the settings, the time), queues the newest request while one runs so two workers never write into one
folder, and `poll_backup` takes the result each frame, showing the first failure as a toast and the rest in the log.
The settings are `backup_folder`, `backup_keep` and `backup_audio` of `WorkflowPrefs` (Settings > Backup, `BackupTab`).
`AppCore::start_backup_list` lists the copies on a worker too; the panel is
[`ui/src/backup_panel.rs`](../crates/ui/src/backup_panel.rs) (`BackupView`, `UiAction::Backup`), opened by
`file.restore_backup`. `AppCore::restore_backup` accepts only a copy the list holds and opens it with `open_template`,
so it is a new unsaved song whose audio is read from the copy's own `Audio` folder; after the question about unsaved
changes it is `PendingAction::RestoreBackup`.

### 2.4 Playback, per 64-frame batch

```
 Engine::run_batch (audio thread)
   1. apply control messages due now   (inserts, routing swaps, song snapshots, transport, tap, tempo)
   2. (nothing: the record tap runs after the routing, as step 6a)
   3. apply queued params
   4. song player: advance the transport (loop wraps, locates); derive the batch's TimeInfo
        (tick, ticks_per_frame, loop_wrap; song-less: a free-running tick at the engine tempo);
        notes → per-device note events; automation → params (lanes the recording hold names are skipped);
        audio regions → per-track playback batches   (Memory: read samples; Stream: pop the lane ring)
        publish each track's playback to its audio-track device
   5. deliver scheduled and live notes
   6. routing.process / process_parallel:  for each (device, stage) in level order
        gather inputs (summing inputs add in source order; PDC delay lines on summing cables)
        Device::process_stage (audio-track: out = playback + monitored input)
        scatter outputs, publish meters
   6a. record tap: copy the host input batch and the tapped device outputs (a console strip being
        resampled) into the tap ring, tagged with the engine frame
   7. mix the metronome (and a pre-count's click, see 2.6) into the host outputs; frame += 64
```

**Controller chase.** Pitch bend, aftertouch and CC lanes are chased on every seek too, whether or
not the track chases notes: each lane gets its value at the position (the last one set strictly
before it), a lane with no value yet is zeroed when it is pitch bend, aftertouch or the sustain
pedal, and Stop and a Locate while playing zero those lanes right after the `AllOff`. See 2.12.

**Note chase.** A track with `SongTrack::chase` (the model's per-track `Track::chase`, on by
default, saved as `"no_chase"` in project format 17) starts the notes that span the play position
whenever the position jumps: on Play, on a Locate while playing and on a loop wrap. A note spans the
position when it starts before it and ends after it; it starts with its own velocity and is marked
as held, so its Off is sent at its end. The notes that start exactly at the position are not chased,
because the normal walk plays them. The chase index (notes by start tick and a running maximum of
end ticks) is built when the song is compiled, off the audio thread, so the walk does not allocate.
A snapshot swap (a live edit) keeps the keys that the changed track still covers and never starts
new ones, so editing during playback does not retrigger a note that is already sounding.

### 2.5 Streaming

```
 EngineHandle::submit_song (non-RT) ─► one lane per audio track with a streamed asset:
        ring of 256-frame stereo chunks (2 s)  ── producer + region list ──► Streamer thread
                                             └─ consumer travels in the snapshot ──► audio thread

 audio thread:  position not a continuation (first batch, locate, new snapshot, loop change)
                  → bump the lane epoch, push a StreamRequest (wait-free)
                every playing batch consumes exactly its frames from the ring (discards outside regions);
                missing data = silence + underrun counter, never a wait
 streamer thread "atomik-streamer": polls requests every 1 ms, restarts lanes, fills each lane a few
                chunks per round by calling StreamSource::read (the app's reader over the bundle WAV)
```

### 2.6 Recording

```
 ToggleRecord ─► AppCore: plan the armed tracks' inputs from the cables interface:in_k → in_l / in_r
        │  EngineHandle::record_tap(channels, 4 s)  (control queue, starts at a batch boundary)
        ▼
 audio thread: each batch appends [engine frame (2 words), 64 words per tapped channel] to a ring,
        │   each channel first through its track's strip: input gain, then the clip-safe limiter
        ▼
 writer thread "atomik-record": drain (contiguous runs only) ─► Audio/rec-<pid>-<take>-<n>.wav, header
        refreshed about every half second (crash safety); a gap is written as silence
 stop or locate ─► Ctl::Finish { end frame, compensation, position function }
        ▼  split into laps (one per loop pass) ─► Bundle::add_recording per lap (hash, peaks) ─► delete the
        in-progress file ─► Msg::Recorded
        ▼
 UI thread: RegisterAsset per lap; per track one AddAudioClip (one lap) or one NewTakeGroup (several laps:
 a loop recording, see 2.13), one undo step each
```

`compensation = input_latency + output_latency + 64 + user_offset`; positive means the take is placed earlier.

**Pre-count.** `AudioSettings::pre_count_bars` (0 to 4, saved in `settings.json`, default 0;
Settings > Audio, Options > Pre-count and `Ctrl+P`, `transport.pre_count`) counts a recording in.
The engine has the mechanism (`TransportCommand::PlayWithPreRoll(PreRoll)`, `player.rs`):
`SongSnapshot::pre_roll(tick, bars, sample_rate)` runs off the audio thread and is a pure function
of the tempo map, the meter map and the sample rate. The count is the beats of the meter at the
start position, at the tempo in effect there (the instantaneous one inside a ramp), rounded to a
whole number of batches; the plan is three integers (`frames`, `beats`, `per_bar`) and click `i`
sounds at frame `i * frames / beats`. The player then holds `pos` while a counter runs down one
batch at a time: nothing of the song plays, `TransportState::playing` stays false (devices see
`playing` false too), the click's onsets (first of each bar accented) sound whatever the metronome
switch says, and `TransportState::pre_roll_frames` / `pre_roll_beat` report the frames left and the
beat of the bar. At zero the next batch starts the song from the held position exactly as a Play
does (seek and chase), a loop included, so a loop is counted in once; a Stop, or a plain Play,
cancels the count. Offline renders never send it, and with the pre-count off `start_recording` sends
the same `Play` as before. In the app `AppCore::recording_count_in` plans the count from the
transport position when a take starts from a stopped transport (a take that starts while playing has
nothing to count in), `Take::counting_in` drops the notes that fall before the song's first engine
frame (announced by the count state: `engine_frame + pre_roll_frames`), and the first playing state
is the origin, so the clip starts where the count started. The audio take needs no change: its
origin is also the first playing state, so `valid_from` is the engine frame the song started at and
the frames the tap wrote during the count are dropped by the existing compensation maths; the tap
runs through the count (it is not started at the end of it, because the compensation places a take
earlier than the song's first frame). The transport bar's POS readout shows `COUNT` and the beat
while the count runs, and `PRE n` while the pre-count is on.

**Transport bar controls.** Right of the four readouts the bar has a second cluster (`cluster.rs`
beside `transport.rs`): Rewind and Forward, tap tempo, the left and right locator readouts with
go-to buttons, a click-level slider, and Q Rec with its grid and a QUANT button. Space is shared
with the clip inspector and the recording-input strip, which need about 998 px: the cluster is four
groups in priority order that take what those two leave, and the groups that do not fit drop out
from the end, replaced by a `...` button whose menu (`ContextTarget::TransportMore`) lists the same
controls as commands. At 1000 px only that button fits. The bar has no clock: a held Rewind or
Forward repeats from `TransportBar::tick(now)` (the app calls it through
`AppCore::transport_frame`), after 20 bars moved the step grows to 4 and then 16
(`repeat_step`), and the repeat is not started while a MIDI clock is followed, the same gate that
refuses the transport buttons. POS, TIME, L and R are typed with a double-click: `5.1.0` is bar,
beat and tick, `00:12.500` is `h:m:s.ms` through the tempo map, and a locator is one `SetLoop`
(one undo step; a locator never crosses the other). Tap tempo is `TapTempo`: the average
of the last 2 to 16 taps with a 2 second timeout, set on the tempo event in effect at the playhead,
the taps of one sequence merging into one undo step. The click slider sends
`UiAction::SetMetronomeGain` and `settings.json` is rewritten once the pointer lets go. Q Rec is
`InputQuantize { grid_ticks }`, any grid of the quantize panel (the saved names `off`, `1/8` and
`1/16` still load); the QUANT button applies the panel's last settings. The workflow preference
`return_to_start_on_stop` makes Stop go back to where playback started (`AppCore::last_start`) and
a second Stop to bar 1. While recording, `Take::preview` feeds a translucent red clip that grows
with the playhead on each armed track (`TakeGhost`, drawn only: not saved, not undoable; an audio
take draws its span), and `edit.delete` calls `Take::discard_notes`: the notes captured so far
are dropped and the take and the transport run on. Nothing here changes the project format.

**Resampling a console strip.** An audio track's stored `rec_source` param (0 to 2000, additive: a
project without it records the inputs, and no project format change) says what a take records:
0 the Audio I/O inputs it is cabled to, otherwise the stereo output of a console strip, by position
(`atomik_model::RecSource`: channels 1..=999 in console order, buses 1001..=1999, the master 2000).
The track header's input menu lists `Channel: name`, `Bus: name` and `Master: name` after the
inputs, and each is one `SetParam`. A take that records a strip puts `TapChannel::source(device,
socket)` channels in the same tap as the host channels, so inputs and strips can be recorded in one
take, each track through its own gain and clip-safe strip. The engine reads the strip's `out_l` and
`out_r` (the master's `main_l` and `main_r`) at the end of the batch, so the recorded frames are
bit-identical to what the routing rendered (the null test compares them with the host output, one
batch later). Placement uses the **path's latency** as the take's compensation instead of the
hardware round trip: the strip's readout from the delay compensation report (the master's is the
report's output latency), set per track in `WriterTrack` through `TrackInput::Source { latency }`, so
a clip lands where the strip's audio sits on the timeline. A delay that a strip adds without
reporting it (a positive `latency.adjust`) is part of what is heard, so it is part of the clip; a
negative `latency.adjust` is phantom latency and is counted as a delay. **Cycle rule:** a track
cannot record a strip it feeds (`atomik_model::source_refusal` walks the cables upstream from the
strip, stopping at the Audio I/O device and treating a master send bus as feeding only its FX send):
that take records nothing and the message says why (send the track's channel to an Audio I/O pair,
or pick another source). The header and transport meters of such a track show the strip's output.

**Input gain, clip-safe and the input meter.** Each armed audio track has two stored params on its
device, `rec_gain` (dB, -60 is off, up to +24, default 0) and `rec_clip_safe` (off). They are plain
`SetParam`s, so they are undoable, merge while dragged and need no project format change (a project
without them loads at 0 dB with clip-safe off).

- *Where the gain acts.* **Before both the take and the monitor**: the track's device scales the
  input it monitors by the gain (5 ms crossfade), and the record tap scales the signal it writes by
  the same gain. The plan gives every armed track its own tap channels and its own *strip* (so two
  tracks recording one input keep their own gains), and `AppCore` copies each track's settings into
  the shared `TapControls` (atomics, wait-free) when the take starts and every UI frame while it
  runs. The audio thread reads them once per batch; the gain glides a quarter of the way to its
  target per batch and ramps linearly inside the batch, so a turn is never a step, and 0 dB with
  clip-safe off is bit-exact. No allocation, lock or wait.
- *Clip-safe.* A deterministic safety limiter on the written take (not on the monitor): per strip, a
  feed-forward peak limiter with instant attack and a release of about 43 ms (2048 frames), linked
  across the strip's channels so a stereo take keeps its image, and a final clamp to ±1 so the bound
  is exact (not-a-number and infinite samples are written as silence). It never touches a take that
  stays below 0 dBFS, to the bit. Why a limiter and not an auto-reduced gain or a fixed headroom:
  the take must still be what was played (no gain the user did not choose), the result must be
  reproducible from the input alone (no look-ahead and no hidden state but one value), and an
  instant-attack limiter is the one simple rule that guarantees the bound at every sample. The files
  are 32-bit float, so with clip-safe **off** an over is kept in the file (nothing is lost; the take
  clips at the output): the switch is for takes that must be usable as they are.
- *Meter.* Shown in the transport bar (visible in every view and while recording) for the selected
  armed track, else the first armed one. The level is the host input's peak that the Audio I/O
  meters already read once per frame (`feed_io_meters`) times the track's gain (the peak of a scaled
  signal is the scaled peak), so it works while armed and not recording and needs nothing new in the
  engine. The bar keeps peak and hold with the console's ballistics, and the clip lamp **latches**
  when the post-gain peak reaches 0 dBFS until it is clicked; with clip-safe on it still lights (the
  input is hot; the take is protected). A change of the gain made by automation is not followed by
  the take or the meter: they use the value the document holds.
- *Tuner.* The audio track device keeps a scope tap of its input after `rec_gain` (pushed every
  batch, so it works armed or not, like the analysers' taps) and the name plate of the track has a
  TUNER switch (`UiAction::SetTuner`, view state: not stored, not undoable; a project opens with
  every tuner hidden); the display and the tuner settings are laid out only while it is on
  (`fx_layout::audio_track_layout`), so the rack rebuilds the panel with its height when the switch
  flips. While a tuner shows, `AppCore` (`app_core_tuner.rs`) sets the device's
  `internal.tuner_live` (sent only when it changed, again after an engine restart or a re-add) so
  that the device fades its output out when its stored `tuner_mute` is on, and once per frame reads
  the tap (skipped when its head did not move), picks the louder channel and runs
  `atomik_dsp::tuner` on the UI thread with the track's `tuner_ref` as A4: a coarse YIN at about 12
  kHz, then the normalised square difference at the full rate around the coarse period (within a
  cent for a steady tone from 30 Hz to 1.3 kHz), smoothed over the elapsed frames, with the last
  note held 0.8 s. The reading reaches the panel through `RackView::set_analyser`
  (`AnalyserData::tuner`). The audio thread only copies 2 x 64 floats per batch. `tuner_mute` and
  `tuner_ref` are additive params, so there is no format change.
- *Header meters.* Every audio track header in the arrangement has its own meter
  (`arrangement/meters.rs`): the input after the gain, per channel, while the track is armed (the
  same host peaks, through `input_peaks`), else the track's playback level, read from the output of
  the console channel that has the track as its source (post-fader, post-mute: what you hear from
  it). A meter read resets the peak, so `AppCore` reads the console's meters once per frame
  (`update_console_meters` while the mixer shows, else the header feed) and both consumers use that
  reading. Ballistics, scale and hold are the console's; the clip lamp latches per track and source
  and is cleared by a click, which `AppCore` also applies to the transport bar's latch of the same
  track (and the reverse), so the two agree. The view marks itself for repaint only when what a
  visible meter draws changes (levels to a quarter of a dB, the readout, the lamp). A track without
  a console channel reads silence; note tracks have no header meter.

### 2.6a Automation recording

```
 arm button on an automation lane ─► AppCore (session set of (device, param); never saved)
 ToggleRecord with a lane (or a track) armed ─► one PassRecorder per armed lane, at the position
        │   (the lane's write mode from Song::lane_write: Overwrite, Touch, Latch)
        ▼
 UI thread, per applied command:  Command::SetParam ─► ParamChanged ─► the lane's recorder
        (tick = the engine's last published transport tick; a drag-merge id = a held control;
         UiAction::EndMerge = the release)          ─► EngineSink::hold_automation(lanes being written)
        ▼  loop wrap (TransportState::loop_count moved): end the pass at the loop end, start the next
 stop or locate ─► Segments ─► segment_clips + thin_points ─► one Command::Batch of AddClip
```

- **Model** ([automation_rec.rs](../crates/model/src/automation_rec.rs)). `WriteMode` (`Overwrite`,
  `Touch`, the default, and `Latch`) is stored per lane in `Song::lane_write` (`(device, param)` to
  mode; the default is never stored; `SetLaneWriteMode` is one undo step and reports `ViewChanged`
  only; project format 18 saves a non-default mode as `"write"` on the lane and older files load on
  Touch). `PassRecorder` is the pure state machine of one pass over one lane: Overwrite writes from
  the start of the pass (first point: the value the param had) to its end, Touch writes only while
  the control is held (each touch is its own stretch, an instant touch is one point), Latch writes
  from the first touch to the end of the pass. Its output is a list of `Segment`s (`[start, end)` in
  ticks and the points).
- **Thinning.** `thin_points` is Ramer-Douglas-Peucker with the vertical distance in value (the
  error of the straight line between the kept neighbours at a dropped point's tick, in parameter
  units): the first and last points and every `Step` point stay, the rest go when within the
  tolerance. The app passes `AppCore::automation_thinning` (default 0.5 %, 0 keeps everything) times
  the parameter's range; the level is the Interface setting Automation thinning (`ThinLevel`:
  Normal 0.5 %, Heavy 2 %, Maximum 5 %), which the pencil and Cleanup Automation use too.
- **A take is a Batch of `AddClip`.** Each segment becomes an automation clip on the lane (ticks
  relative to the clip start, `Linear` points), and a later clip hides an earlier one where they
  overlap, which is how Overwrite and Touch replace what was there without deleting it: the old
  clips stay under the new ones and one undo (the batch is one step) brings the old lane back whole.
  A loop recording lays one clip per pass, the later laps on top.
- **The hold.** While a lane is being written the engine must not apply its old points, or they
  would overwrite the value being moved at every batch. `EngineHandle::hold_automation` names up to
  `HOLD_CAPACITY` (32) `(device, param)` lanes in a fixed array of atomics that the song player
  reads once per batch and skips (no queue message, no allocation, no lock; an empty set costs one
  load). A container's macro lane holds every target it expands to. Offline renders never set it, so
  bounces are unchanged.
- **Lane lifecycle.** `SongCommand::RemoveAutomationLane` removes a lane with its clips in one undo
  step (a compound of the lane edit and, when the lane has them, the removal of its write mode and
  its fold; a track that holds automation lanes alone goes with its last lane), `SetLaneMuted` takes
  an automation lane and `SetLaneStatic` sets `Lane::static_value`. A **muted** lane flattens to
  nothing, so the engine never sees it and its param keeps the value it has; `AppCore::hold_on_mute`
  makes the mute the first command of a `Command::Batch` whose second is a `SetParam` with the value
  the lane gave at the playhead (`automation_value_at` of `flatten_automation`), so the hold is part
  of the same undo step. A **static value** is added by `flatten_automation` itself (a lone Step
  point at tick 0 for a lane without clips, else a leading Step point at tick 0 and a Step point at
  the end of every audible piece that is not followed at once by the next), so the engine, the
  bounce and the views, which all read the flattened lane, need no other rule. Project format 36
  saves `"muted"` and `"static"` on an automation lane after its `"folded"`, only when set; a
  project without them bounces bit for bit as before (the muted and static rules apply only to lanes
  that have them).
- **Auto-create lanes.** With `AppCore::set_auto_create_lanes` on (the transport bar's switch;
  session state, never saved) a take also records parameters that have no lane: the first change of
  one while the take runs starts a Touch `PassRecorder` for it (`LaneTake::created`), provided
  `AppCore::lane_host` names the parameter itself (so not an internal one, an audio track's, one
  inside a container, or one a macro drives, which is reported once per take). It holds nothing in
  the engine, as it has no lane there yet. When the take ends, the new lanes are `AddAutomationLane`
  commands at the head of the same `Command::Batch` as the clips (the lane index of each comes from
  a scratch copy of the document that applies them first, a lane that cannot be added is dropped
  with its clips), so one undo removes lanes and clips. A parameter that already has a lane records
  only when that lane is armed.
- **Override.** A `SetParam` with a drag-merge id, while the song plays and no take runs, on a param
  that has a playing lane puts `(device, param)` in `AutoRec::overridden`. `update_automation_hold`
  sends the union of the lanes a take writes and the overridden ones to
  `EngineHandle::hold_automation` (a container's or a bank's macro lane holds every target it
  expands to; the engine names at most `HOLD_CAPACITY` = 32 of them, and a move that would pass it
  is refused with a message), the controls show the user's value for those lanes
  (`automation_shown`), and `clear_override` empties the set from the OVERRIDE lamp of the transport
  bar, Stop, a locate and an engine restart (the lane is also dropped when it is removed). The hold
  is one load per batch on the audio thread, as for a take, and an offline render never sets it.
- **What is captured.** `ParamChanged` ops of commands that go through `AppCore::apply_command`
  while the take runs: knob drags, text entry, a plugin's own window, the MCP server. Undo and redo
  are not captured. A MIDI controller reaches a parameter through the mappings (see "MIDI mappings
  and control surfaces" in section 5): a mapped move is an ordinary `SetParam` with a gesture's
  merge id, so it is captured here as a touch that lets go when the control has been still for half
  a second (a surface's touch-sensitive fader holds it exactly as long as the finger rests).
  Controller data can also be recorded into note clips (2.12). The tick of a move is the transport
  tick of the engine's latest batch, so it is at most one batch (1.3 ms at 48 kHz) early.

**Controls that follow automation.** The engine drives a param to its lane's value every batch and
does not touch the document, so a panel that painted the stored value would sit still while the
sound moved. The views show an overlay instead
([app_core_autoview.rs](../crates/app/src/app_core_autoview.rs)):

```
 AppCore::frame ─► position (the engine's published tick, or the last locate)
   ─► AutomationCurves::values_at(tick)         lanes flattened once per revision, the player's own rule
   ─► minus the lanes a take is writing, minus the params edited by hand while stopped
   ─► RackView / ConsoleView / PluginParamsView::set_shown_values   (compared; unchanged = nothing)
   ─► sync: only the panels showing a changed value are re-recorded (each window repaints from its view)
```

- **Why the UI evaluates it.** The model flattens a lane with the code the engine's own input comes
  from (`flatten_automation`: clip offsets, loops, muted clips) and `automation_value_at` is the
  player's rule for a point list (held outside the points, linear or step between);
  `atomik_engine::lane_value` is exported and `crates/render/tests/automation_shown.rs` pins the two
  bit for bit over a lane with an offset and a loop. The tempo map needs no part in it: the tick is
  the one the engine published. Nothing is added to the audio thread (no new publish, no
  allocation), and a stopped transport shows the value at the playhead, which a publish from the
  player would not (it applies lanes only while playing).
- **Which lanes play.** The tracks of `effective_tracks` (mute and solo applied), not frozen ones
  (they play their render), lanes with clips, and not a param a macro bank drives (it follows the
  macro, or its lane plus the macro, see "A lane on a driven param" below; the shown value is the macro's alone for
  now). A container's or a bank's macro knob is an ordinary param of that device, so its own lane
  moves it, and the params a bank drives show the macro's value (`view_with_driven_values` runs on
  the overlaid document); the view of a container's inside shows stored values, so its driven
  controls do not follow a lane on the container's macro yet (the params a bank drives there do show
  the macro's value).
- **The document is not touched.** `RackDoc::view_with_shown` copies the devices with the shown
  values in place (an empty song: no view reads it); the document, its history and `is_dirty` never
  see the overlay. The rack applies it in `prepare`, the console and the plugin list in `sync`;
  inside a container's own view the controls show the stored values.
- **What the user holds.** A lane in `AutoTake` that `PassRecorder::is_writing` (Overwrite from the
  start, Latch from the first touch, Touch while held) is left out, so the control shows the user's
  value and, in Touch, follows the lane again at the release. With the song stopped `ParamChanged`
  marks the param as touched until the playhead moves or the song plays; with it playing and no
  take, the engine overwrites the value at every batch, and the control shows what the engine plays.
- **The mark.** A param with a lane (`RackDoc::automated_params`, whether or not the lane has clips)
  gets a cyan ring and a dot at the top-left of its control (`paint::automation_mark`, drawn in the
  panel record of the rack and over the console strip), and its hover tooltip ends with
  "(automated)". A lane on a device the console strips or the rack show needs nothing more: the
  overlay is keyed by `(device, param key)`.

### 2.6b Automation tracks

A device without a Notes input (a console channel, bus or the master, an effect) can be automated
like an instrument: `SongCommand::AddAutomationLane` makes it a **track that holds automation lanes
alone** (`Track::is_automation_only()`: at least one lane, all of them automation lanes; no note
lane, no audio lane). A device with a Notes input still gets a note track (lane 0 notes), an
`atomik.audio-track` still has its single audio lane and takes no automation lane, internal params
(`console::is_internal_param`) are refused as before; a param a macro bank drives takes a lane too (its base). The track is
bound to the device by its key, so it is renamed, coloured, moved, foldered and removed like any
track, goes with the device when the device is removed (`Planner`), and is copied with it
(`DuplicateTrack`, paste, `paste_tracks`).

- **Invariants** (kept by every command and the loader). A track on a device without a Notes input
  is an automation track; a track on a device with a Notes input starts with its notes lane, so a
  device with a Notes input never has an automation track (a plugin replacement that would break
  either side is refused). An automation track is never soloed, armed, frozen, stacked (`notes_to`)
  or given note lanes (`SetTrackFlags` refuses `solo` and `armed`, `AddNoteLane` and `FreezeTrack`
  refuse it, `RemoveNoteLane` has no note lane to remove) and has the default chase flag; its mute
  is the only flag it uses.
- **Mute and solo.** `effective_tracks` keeps an automation track when it is not muted, whatever is
  soloed: a solo is about what is heard from notes and audio, and it must not stop a fader or an
  effect that is moving. A muted track plays no lanes, so the param keeps the value it has in the
  document, in the engine and in the views (`automation_values_at` reads the same tracks).
- **Engine.** `song_input_with_warps` gives such a track an engine track with no notes and
  `SocketId(0)` for its Notes socket, as a macro target's lane already gets (`push_macro_lane`); the
  engine maps lanes by device and param, so it needed no change and a lane plays live and in a
  bounce like an instrument's, bit-identical across block sizes and workers
  (`crates/render/tests/automation_only.rs`).
- **File.** Project format 34: the loader takes a track on a device without a Notes input only from
  version 34, with `"automation"` lanes only (at least one, params the device has) and no `"solo"`,
  `"armed"`, `"no_chase"`, `"notes_to"` or `"frozen"` (refused). The writer is unchanged, so a
  project without such a track differs from its version 33 file in the version line alone
  (`crates/model/tests/automation_only.rs`).
- **Lane header and picker.** The header of an automation lane after the first has fold, write mode,
  arm, **M** (mute, `SetLaneMuted`) and **X** (`RemoveAutomationLane`) buttons; the first lane of an
  automation track shares the track's header row and keeps write mode and arm, and the track has a
  small **+** picker beside them. The picker (note and automation tracks) lists every non-internal
  param of the track's device, checks the ones that have a lane and a click on a checked one removes
  it. The lane header pressed last is the *picked lane* (view state) that the track header menu's
  Delete, Mute, Set Static Value and Clear Static Value items act on, and a control's right-click
  menu has Clear Automation and Set Static Automation Value for its own lane. A click on a control
  with the `Role::EditAutomation` modifier (Alt) emits `UiAction::EditAutomation`, which the shell
  runs like `automation.edit_control`.
- **View** ([arrangement.rs](../crates/ui/src/arrangement.rs)). Every place that treats lane 0 as
  the track's header row keeps doing so: for an automation track lane 0 is its first automation
  lane, drawn in the header row. The header keeps the chevron, the name and the mute button
  (`AUTOMATION_BUTTONS`: the lane picker is a small `+`; no solo or arm button) and carries that
  lane's param name, write mode and arm buttons on its second line
  (`lane_button_rect(.., lane 0, ..)`; a short row shows none of them); that lane cannot fold away
  (`is_lane_folded`), the others collapse and fold as on any track. A folder's arm button skips
  such tracks, the Chase Notes and note-lane menu items are off for them, and the console's
  master strip draws the automation mark like the channel strips (`paint_master_marks`).
- **App.** `AppCore::lane_host` allows every device but an audio track (a path key, a device inside
  a container, has no top-level device and so no lane), so Edit Automation, the follow-the-selection
  lane and the parameter search work for strips and effects; selecting an automation track arms
  nothing (`arm_track_device`) and it is not offered for freezing. A take records into its lanes
  like any lane (the armed set is by device and param) and `project_inspect` marks the track
  `automation_only`.

### 2.6c Pattern lanes

A **pattern lane** is the automation lane of the `pattern` param (`PATTERN_PARAM`) of a step sequencer, a
CV sequencer or a drum generator, found with `Track::pattern_lane` and labelled "Pattern". It needs no
format of its own: its clips are automation clips of stepped points (a pattern number held until the next
point), flattened like any lane, so the first point switches the device's pattern at the clip start and
the lane's static value takes over at the clip end. Playback, bounce and old projects are unchanged.

- **View.** `ArrangementView` draws the held numbers as labelled blocks (`pattern_lane.rs`) instead of a
  curve. A right click on a clip opens the pattern list (its rows are `SetClipContent` commands that set
  the number of the block under the pointer; the last row deletes the clip). A clip drawn on the lane
  starts with one `Step` point, the number under the pointer. Draw, move, resize, split and copy are the
  automation clip gestures, one undo step each.
- **Recording.** A take catches `pattern` changes of the rack's pattern devices whether or not AUTO LANES
  is on (`has_pattern_device`): the first change adds the lane and every change after it is written on
  the next downbeat (`downbeat_at_or_after`) in Latch mode with stepped points (`segment_clips_with`), so
  the clip switches the pattern on a bar line and never ramps through the numbers between two patterns.
  A pattern lane that exists records when it is armed, like any lane.
- **Commands.** Track > Add Pattern Lane (`create_pattern_lane`) adds the lane for the selected pattern
  device or track. Track > Pattern Lane to Notes (`pattern_lane_to_notes`) renders the tracks that play the
  device offline, as Send Players to Track does, and writes the notes the patterns played, in the order the
  lane chose them, into a clip on a new note lane named "Pattern". The stack keeps playing: mute it to hear
  the copy alone.
- **Not there yet.** A clip with its own pattern *offset* (starting the pattern part-way) and a bank view of
  the device's patterns are not built: a pattern clip is the automation clip of the number.

### 2.7 Save and bounce

```
 Save:   Document ─► pruned copy (only assets the song uses) ─► save_project text
         ─► atomic write of <name>.atomik ─► Bundle::remove_orphans(song assets + undo/redo assets)
 Save As (with audio): new folder <name>/, Bundle::copy_bundle, then the same
 Bounce: load_project ─► Bundle::open ─► load_song_assets (every asset fully in RAM, never streamed)
         ─► song_input ─► render_offline_song ─► WAV      (bit-identical for any block size and workers)
```

The window saves on a worker ([crates/app/src/app_core_save.rs](../crates/app/src/app_core_save.rs)).
On the UI thread a save asks every sandboxed plugin for its state first (one request each, then the
answers: `PluginInstance::begin_capture_state`, so the wait is the slowest plugin's), clones the
document (the song, the note lists and the plugin states are shared, so nothing bulky is copied) and
plans the bundle. A worker builds and validates the project text, copies the bundle for Save As,
writes the file atomically and removes the orphans. The file name, the bundle and the saved baseline
switch when the worker reports in (`AppCore::poll_background`), so an edit made meanwhile is still
unsaved afterwards and undoing back to the saved state is clean; a failed save is reported and
changes nothing. An import, a recording or opening another project waits for the save first
(`AppCore::settle_save`), and a save that starts while audio is loading, recording, converting or
freezing runs blocking. Scripts, tests and the QA server keep the blocking `AppCore::save`.

The event loop sleeps until an event when nothing is due. A loaded plugin only asks for a look every
100 ms (`PLUGIN_IDLE_POLL`: its notifications, dirty flags and main-thread work); frames at 60 Hz
run while audio runs, a worker is busy, a plugin is being created or an editor is open.
`ui-state.json` is written atomically and at most every two seconds (`Shell::flush_ui_state`, and on
shutdown, detaching and attaching a pane); `keymap.json` is looked at when a window gets the focus
and when a menu opens.

A project opens its plugins together: `PluginManager::begin_create` starts the child processes of up
to eight pending devices (`PluginBackend::begin_instantiate`, `PendingInstance::poll`), and the
devices attach one per frame in document order, so the result does not depend on which plugin
loads first. Plugins loaded into the app process are still created one per frame. A freeze or a bounce
with sandboxed plugins starts its own children next to the live ones, so the process count doubles
while it runs; the live children are not reused, because the offline output must stay bit-identical.

### 2.7a Export as an audio file

File > Export > Export Song as Audio File, Export Loop as Audio File and Export Stems as Audio Files
(`file.export_song_audio`, `file.export_loop_audio`, `file.export_stems`, remappable, no default
keys) write the project as a WAV, AIFF, FLAC, MP3, AAC or Opus file, or one file per mixer strip (stems are WAV,
AIFF or FLAC only).

```
 command ─► ShellRequest::PickExportAudio ─► native save dialog ─► Shell::export_audio_to ─► options panel (ExportView)
 EXPORT  ─► AppCore::start_audio_export ─► atomik-export worker (a copy of the document, its own registry and plugin host)
            ─► render::export::export_audio_file ─► render_core (the bounce) ─► slice, tail ─► [resample] ─► Dither ─► <file>.partial ─► rename
```

- **Range** ([render/src/export.rs](../crates/render/src/export.rs)). The project plays from tick 0
  through the bounce path (`render_core`: the console with mute and solo, the master section,
  automation, tempo ramps, plugins), so a song export is bit for bit the first frames of a bounce.
  *Song*: frame 0 to the song End (`Song::end_tick`) plus a **tail**: the render continues for up to
  10 s (`DEFAULT_TAIL_SECONDS`, 60 s at most) and the file is cut where the output stays at or below
  -90 dBFS (`TAIL_FLOOR`, the same floor as a freeze), never before the End; with the tail switched
  off it ends at the End. The plain bounce (`atomik-render`, `session_bounce`) keeps its fixed two
  beats after the End. *Loop*: exactly `frame(right locator) - frame(left locator)` frames, no tail,
  so the file loops on the grid; the part before the left locator is rendered and dropped, so the
  loop starts in the state the song has there (held notes and running effects included). Whether the
  loop is switched on does not matter.
- **Format.** The WAV writer (`render/src/wav.rs`) writes 16-bit PCM, 24-bit PCM or 32-bit float.
  The rendering is at the project's sample rate; another rate (44.1, 48, 88.2, 96 kHz in the panel,
  8 to 192 kHz through the API) goes through the sinc converter of `atomik-audio-file` first
  (`round(frames * rate / project rate)` frames, so a loop of whole beats is not sample exact at a
  rate that does not divide it). The samples are quantised **once**, last, by
  [dsp/src/dither.rs](../crates/dsp/src/dither.rs): 16-bit gets triangular dither of +-1 LSB by
  default (or none, or the same with first-order noise shaping, `1 - z^-1`); 24-bit and float get
  none (the dither applies to 16-bit in the panel). The dither is seeded (`DITHER_SEED`, one stream
  per channel from `Rng`), so the same project exports the same bytes. Silence is dithered too: a
  16-bit file has a noise floor of about -96 dBFS.
- **Containers.** `ExportFormat::container` is WAV, AIFF or FLAC. The AIFF writer
  ([render/src/aiff.rs](../crates/render/src/aiff.rs)) writes 16-bit and 24-bit PCM as plain AIFF
  and 32-bit float as AIFF-C `fl32`, quantising through the same `Dither` as the WAV writer, so
  both containers hold the same codes for the same audio (a test decodes both and compares). The
  file keeps the dialog's name with the extension of the container (`.wav`, `.aif`, `.flac`).
  The FLAC writer ([render/src/flac.rs](../crates/render/src/flac.rs)) is an in-house encoder
  rather than a crate (the format is small, the decoder already ships through symphonia, and it
  adds no licence or advisory): fixed blocks of 4096 frames, per block and channel the smallest of
  a constant, a verbatim and the fixed predictors of order 0 to 4 with Rice-coded residuals over
  the best partition order, stereo as left/right, left/side, side/right or mid/side, and the MD5
  of the unencoded samples in the stream info. It takes the 16-bit and 24-bit codes of the same
  `Dither`, so the decoded file equals the WAV's samples (a test decodes both through the project's
  reader); every choice is a pure function of the codes, so the bytes are deterministic. There is
  no 32-bit float FLAC (the panel and `write_export` refuse it). Tempo chunks are not written.
- **Compressed formats** ([video/src/encode.rs](../crates/video/src/encode.rs)). MP3 (constant bitrate 128,
  192, 256 or 320 kbit/s, or variable quality V0, V2, V4), AAC in `.m4a` (128 to 320 kbit/s) and Opus in
  `.opus` (64 to 192 kbit/s) are encoded by the FFmpeg the user installed ([video through
  FFmpeg](#223-video-through-the-users-ffmpeg)), so no codec ships. The panel always shows the three
  buttons; `AppCore::export_encoder` says whether FFmpeg is `Ready`, being looked for or missing, and a
  missing one greys them with a line that says to set its folder in Settings > Video or put it on `PATH`
  (the same places as video import). The export worker renders as for a WAV file, writes the audio to a
  float WAV file in a temporary folder at the rate of the result (the asked rate, else the song's, brought
  to one the format holds by `LossyCodec::output_rate`: MP3 and Opus stop at 48 kHz), runs `encode_audio`
  through the same `VideoTool` seam (a list of arguments, no shell; the song's title and author are the
  values of `-metadata` arguments with control characters turned into spaces), writes under `<file>.partial`
  and renames when FFmpeg has finished. Progress is the render (80 %) and then FFmpeg's own `out_time_us`
  lines; a cancel kills FFmpeg; a cancel or a failure removes the partial file and the temporary WAV and
  leaves an existing result file as it was. The loudness in the report is that of the audio that went into
  the encoder, and the encoder can raise the true peak (an MP3 or AAC decoder may overshoot a signal that
  peaks at 0 dBTP by a decibel or more): leave about 1 dB of true-peak headroom, for instance with the
  true-peak limiter of the mastering suite. The panel's bit depth and dither rows are greyed for these
  formats. `export.json` keeps the format, the MP3 quality and the AAC and Opus bitrates. The tests use the
  fake tool; one ignored test runs the real FFmpeg.
- **Not replacing unasked.** The native save dialog confirms an overwrite of the name it picked, but the
  container is chosen afterwards, so the file gets the extension of the format (`export_file`). The dialog
  proposes the extension of the format used last (`Shell::export_audio_extension`), and `export_action` in
  [shell_export.rs](../crates/app/src/shell_export.rs) compares the picked name with the file the export
  would write: when they differ and the file exists, nothing starts, `ExportView::set_warning` says so under
  the options, and a second EXPORT on the same file confirms. A changed choice clears the line.
- **Film with mix** ([app_core_film.rs](../crates/app/src/app_core_film.rs),
  [video/src/film.rs](../crates/video/src/film.rs)). File > Export > Film with Mix (`file.export_film`) opens the
  film panel (`FilmView`, a modal of its own with the same options and progress modes as the export panel) and
  `AppCore::start_film_export` starts the same export worker with a `Film` step instead of `Lossy`. `film_plan`
  turns the film's start, trim and the song's tempo map into the range of the song the mix covers (an
  `ExportSpan::Range` under the film, or `ExportSpan::Song` for the whole song) and `FilmCut::layout` into
  integer microseconds: the part of the film that plays, the black before and after (whole song only, a stretch
  under half a frame is dropped), how long the mix waits (a film that begins before the song, or a mix that starts
  a tick after the film) and the length of the result. The worker checks FFmpeg, probes the film, decides the
  picture with `plan_picture` (a refusal comes before the render), renders the mix to a temporary float WAV file at
  the rate the sound codec holds, and `export_film` runs one FFmpeg call built by `film_args`: the film is
  the first input with the trim as input options (frame accurate when the picture is re-encoded), the mix the
  second, delayed in samples with `adelay` and padded, black pictures from the `color` source joined to the film
  with `concat`, the picture copied or encoded to H.264, the mix as AAC in MP4 or 24-bit PCM in MOV, the film's own
  first audio track as a second one, and the output cut to the length of the cut. Copy is automatic only for a
  cut at the film's start and a codec the container holds (a copy cuts at keyframes, so any other cut is
  re-encoded unless the user asks for the copy). Progress is the render (40 %) and then FFmpeg's `out_time_us`;
  a cancel kills FFmpeg and, like a failure, leaves no partial file and no temporary mix. The tests use the fake
  tool; the ignored test in `crates/video/tests/real_ffmpeg.rs` runs the real FFmpeg for every mode.
- **Presets and the loudness target** ([app/src/export_presets.rs](../crates/app/src/export_presets.rs),
  [render/src/normalise.rs](../crates/render/src/normalise.rs)). An export preset is a name and the export
  panel's fields (`PresetChoice`: `stems` says one file per strip, `values` is an `ExportValues`, which has
  gained `mono`, `loudness` (`ExportLoudness`: off, a target in LUFS, or a guide) and `also_mp3`). The built-in
  presets (`builtin`: Streaming, Podcast, Video, Stems for mixing, Club) are in code; the user's live in
  `export_presets.json` beside `settings.json` (`PresetStore`, a `version` field, a missing or corrupt file is
  no presets, at most 16, names unique and never a built-in name). The panel's Preset row fills its fields from
  a preset and shows `(changed)` once a field differs (`ExportView::preset_changed`); a preset of stems turns
  the panel into a stems export. File > Export for (`file.export_for`, the preset's place in the list as its
  argument) asks for the file and starts the preset at once with the progress panel (`Shell::run_preset`); the
  Video preset with a film in the song runs the film export instead (`AppCore::start_film_export_with`).
  The export worker finishes the render before it is written (`Work::finish`, `finish_render`): `Finish::mono`
  mixes every channel into one, and `Loudness::Target` measures the render with `loudness::measure` (the
  delivery targets meter), applies one gain, and when the true peak would pass the ceiling runs the Mastering
  device's look-ahead true-peak `Limiter` offline and raises the gain again until the limited result is within
  0.1 LU of the target (`NormaliseReport` says the loudness before and after, the gain and the limiting;
  `ExportReport::normalise`). `Loudness::Guide` changes nothing and only warns when samples reach full scale.
  Stems are never finished. `Work::also` encodes an MP3 at 320 kbps from the same finished render next to a
  lossless file.
- **Loudness report.** `write_export` measures the audio it writes with
  [render/src/loudness.rs](../crates/render/src/loudness.rs): integrated loudness, loudness range
  (3 s short-term windows, -70 LUFS absolute and -20 LU relative gates, 10th to 95th percentile),
  true peak and sample peak, using the K-weighting, gating and 4x true-peak detector of the
  loudness meter device. `ExportReport::loudness` carries it, `StemsReport::loudness` has one per
  file; the app puts it in the toast and the log, and `session_export` returns it.
- **Range of stems.** `ExportValues::locators` makes a stems export use `ExportSpan::Loop`
  (the frames between the locators, no tail) instead of the song with its tail.
- **Time selection.** `ExportSpan::Range(start, end)` is a range in ticks, cut out like a loop with
  no tail. File > Export > Export Selection as Audio File (`ExportScope::Selection`) and the SELECTION range
  of stems (`ExportValues::selection`, which is not saved in `export.json`) take the range selected
  on the lanes of the arrangement (`AppCore::export_selection`, the first and last tick of
  `lane_ranges`); with none selected the command is greyed and says so.
- **Stems** ([render/src/stems.rs](../crates/render/src/stems.rs),
  [model/src/stems.rs](../crates/model/src/stems.rs)). `stem_sources` lists the strips of the
  console in mixer order (channels, buses, FX returns, master) with the sockets that carry each
  output; the render cables every tap to its own pair of extra interface outputs, as a freeze
  taps a track, so one pass reads several strips without changing a sample, at most
  `TAPS_PER_PASS` strips per pass (each stem is written as its pass ends, so memory does not grow
  with the strip count). `StemStrip::Dry` takes the channels' sources before the strip. The tap sits
  in front of the master's delay compensation, so each stem is **delayed here** by the latency of
  the slowest path to the outputs minus the latency of the path to its own tap (`path_latencies`
  sums the latencies the engine published, along the flat cables, like its compensation does; none
  when delay compensation is off): stems line up with each other and with the bounce. All the
  files have the length the song export has (the tail follows the document's own outputs) and are
  numbered `01 Name` in mixer order in a `Bounced <song>` folder; a stopped or failed export removes
  the files it wrote. Without sends the channel stems add up to the master within float
  rounding (tested).
- **Dry clip bounce.** Track > Bounce > Bounce Clips to New Track (Dry) (`sequencer.bounce_clip_dry`) is a
  bounce in place through zero insert effects: the freeze render with `effects: 0` taps the
  instrument's own output (an audio clip's gain, fades and warp included), so neither the inserts,
  the fader, the pan nor the sends are in it.
- **No partial file.** The file is written under `<name>.partial` next to the target and renamed
  when complete, and the render checks the cancel flag once per 64-frame batch: a cancelled or
  failed export removes the temporary file and leaves an existing target untouched.
- **App** ([app_core_export.rs](../crates/app/src/app_core_export.rs),
  [export_worker.rs](../crates/app/src/export_worker.rs),
  [shell_export.rs](../crates/app/src/shell_export.rs),
  [export_prefs.rs](../crates/app/src/export_prefs.rs),
  [ui/src/export_panel.rs](../crates/ui/src/export_panel.rs)). One export at a time, on a worker
  thread (`Msg::Exported`, counted like the other workers) that renders a copy of the document taken
  when EXPORT is pressed, so the live engine, the transport and the session are not touched: the
  song can play and be edited meanwhile (the file has the document as it was at the start). The
  panel is modal and turns into a progress bar with CANCEL (Esc does the same); the result is the
  toast ("Exported mix.wav: 24-bit, 48 kHz, 1:05.3, with 2.4 s of tail"). Opening or creating a
  project cancels a running export. With plugins the worker needs the plugin sandbox, as a freeze
  does. The last choices (depth, dither, rate, tail, container, dry stems, folder) are in
  `export.json` next to `settings.json`. The stems dialog picks the folder the `Bounced <song>`
  folder is made in. The QA server has `session_export` (MCP.md): the same render, synchronous,
  with a report of the file, or with `stems` of every file.

### 2.8 Tempo, meter and time

One module of `atomik-core` does every conversion between ticks, seconds, frames and bars; the
model, the engine, the bounce and the UI call it instead of keeping copies.

- **Tempo map** (`core/src/tempo_map.rs`). `compile_tempo` turns the song's tempo events (`tick`,
  `bpm`, `ramp`) into a table of segments off the audio thread; `TempoMap` is a `Copy` view over the
  table with allocation-free maths (`seconds`, `frame`, `song_frame`, `tick`, `bpm`), so the RT
  player uses it too (`TempoTable` owns a table for the model and the UI). A step segment holds its
  BPM; a ramp moves linearly in BPM over ticks to the next event's BPM, so
  `seconds(x) = 60 / PPQ * L / (b1 - b0) * ln(b(x) / b0)` and the inverse uses `exp`; an equal-BPM
  ramp is a step. Every segment's start time is accumulated once, and a map without ramps gives the
  same bits as the original step-only maths. Ticks before 0 extend the first tempo backwards, and
  the last event is constant.
- **Meter map** (`core/src/meter_map.rs`). A meter event is a time signature from a bar line
  (1-based bar); changes only happen on bar lines. `MeterMap` converts bars, beats and ticks
  (`bar_start`, `bar_at`, `bbt`, `next_bar_line`, `prev_bar_line`, `beat_at_or_before`);
  `Song::meter_table` and `Song::nearest_bar_line` give the model's view. `SongCommand::SetMeter`
  still means "the meter at bar 1"; `SetMeterMap` and `SetTempoMap` replace a whole map, and
  `DragMeterMap` and `DragTempoMap` merge a drag into one undo step.
- **Engine.** `SongInput` carries `tempo` (with `ramp`) and `meter`; the snapshot holds both tables.
  Notes, audio regions, loop points and clicks sit at `song_frame(tick)`, the exact closed form. A
  batch's `TimeInfo` is evaluated at the batch's first tick: `tempo` and `ticks_per_frame` follow a
  ramp in 64-frame steps (the tempo-synced devices read them once per batch), `time_signature` is
  the meter in effect there. The metronome clicks on the beats of the meter map (a quarter in 4/4,
  an eighth in 6/8) with the accent on the first beat of each bar.
- **UI.** `timeline::Bars` (the meter map) gives the ruler's bar numbers, grid lines, bar keys and
  `SnapGrid` (a snap grid that restarts at every bar line); the tempo lane and the meter tags are in
  `ui/src/tempo_lane.rs` and `ui/src/arrangement/tempo.rs`. The tempo lane is a view setting (View,
  Tempo Lane; saved in `ui-state.json`), not part of the project. The transport bar types both:
  double-click the tempo readout (BPM of the event at the playhead) or the meter readout (`3/4`, a
  bare number keeps the beat unit; `SetMeterMap` on the change at the playhead, so the first
  signature, which has no ruler tag in 4/4, is editable). Both readouts also take a vertical drag
  and the wheel on the field under the pointer (BPM whole part or decimals; beats per bar or beat
  unit): `ui/src/step_drag.rs` turns the pointer travel into whole steps (5 px each, the remainder
  kept), the drag sends `DragTempoMap` or `DragMeterMap` with one merge id from
  `TRANSPORT_MERGE_BASE` (13 * 2^40) and `EndMerge` on release, so it applies live and is one undo
  step. The snap selectors and the quantize panel use the same helper.

### 2.9 Markers and ranges

- **Model.** `Song.markers` (`Marker { id, tick, name, color }`, sorted by `(tick, id)`) and
  `Song.ranges` (`RangeMarker { id, start, end, name, color }`, sorted by `(start, id)`, overlap
  allowed) are tick-based, so they follow the bars when the tempo or the meter changes. The ids only
  tell items apart while editing (a loaded song numbers them from 1 in file order; files do not
  store them). `SetMarkers` / `SetRanges` replace a whole list (the model validates names, ticks and
  ids and sorts it); `DragMarkers` / `DragRanges` do the same with a merge id, so a drag is one undo
  step. They report `SyncOp::ViewChanged` only: markers never reach the engine, the bounce or the
  song snapshot.
- **Strip, list and edits.** The marker strip sits under the ruler (`ui/src/arrangement/markers.rs`;
  View, Marker Strip, default on): a row of ranges over a row of point markers. A click on a marker
  locates the playhead there, a click on a range sets the loop to it (and switches the loop on), a
  drag moves them (snapped by the current snap, the disable-snap role frees it; the edges of a range
  resize it), a double-click on a label types a new name in place, the eraser, Delete and the
  context menu delete, and a double-click or the pencil on empty strip adds one. The marker list
  (View, Marker List, default off) is docked at the right edge of the arrangement and gives up its
  width from the timeline: each row shows the name, the position as `bar.beat.tick` through the
  meter map and, for a range, its end; a click on a row locates (a range row also sets the loop),
  the name is renamed by double-click, `x` deletes, and `+ Marker` / `+ Range` add at the playhead
  or from the loop. A marker also draws a faint line over the lanes while the strip is shown, and a
  press within three pixels of it (Select tool, no Shift or Ctrl, not on a clip's own controls)
  takes the marker like its label does (`ArrangementView::line_hit`). The pure edits are in
  `ui/src/marker_edit.rs`. While a name is typed the view is a text owner
  (`ArrangementView::is_text_owner`), so the shell resolves `Text` over `Sequencer` and plain keys
  are typed instead of run.
- **Inspector strip.** View, Inspector (default off, `show_inspector` in `ui-state.json`) reserves
  `ui/src/arrangement/inspector.rs`'s strip under the lanes: the arrangement takes its height off
  `bounds.y1` as the marker list takes its width. It edits the selected clips of any kind through
  `SetClipGeometry` (`SetAudioClip` for audio) with the fields of `ui/src/segment_field.rs`
  (`SegmentField`: split and join a tick into bar, beat, sixteenth and tick through the `Bars` meter
  map, step, parse a typed `+n`, `-n`, digits or dotted entry; `SegmentDrag` and `SegmentEdit` hold
  a gesture). A multi-selection edit is `new - shown` of the first clip added to each clip's own
  value and clamped per clip; a typed or wheel edit is one `Command::Batch`, a drag one merge group.
  While a value is typed the view is a text owner. No format change. The same switch
  (`view.toggle_inspector`) gives the piano roll its own strip (`ui/src/piano_roll/inspector.rs`:
  the roll keeps the whole rectangle as `outer` and a smaller `bounds`): Note, Position, Length and
  Velocity of `selected_notes()`, edited with one `SetClipContent` of the whole normalised note list
  (`seq_edit::normalize_notes` keeps the selection on its notes; a drag recomputes from the notes at
  the press, so the document catching up never compounds). With automation points selected the
  arrangement strip shows Position and Value instead (`arrangement/points.rs`), also one
  `SetClipContent`, the value clamped by `ParamDescriptor::clamp`. While a value is typed either
  view is a text owner.
- **Commands.** `transport.add_marker` (`Shift+M`), `transport.prev_marker` (`,`) and
  `transport.next_marker` (`.`) are bound in the `standard` profile only (the `legacy` profile is
  frozen); `transport.add_range_from_loop`, the `sequencer.marker_*` commands of the context menus
  and the two `view.toggle_marker_*` commands have no default key. Previous and next look at point
  markers only, strictly before and after the playhead. The strip and the list are view settings
  (`show_marker_strip`, `show_marker_list` in `ui-state.json`).
- **The song End.** `Song.end` (read it with `Song::end_tick()`; saved as `"end"` since format 1, so
  no format change) is the length of the song: what a render covers and what the arrangement's
  scroll extent includes. Playback does not stop there. `SongCommand::SetSongEnd` sets it exactly
  (any value above 0, also inside the clips). `Document::apply` also moves it out to the next bar
  line, in the same undo step, whenever an edit puts a clip past it (draw, move, stretch, paste,
  record; a drag stays one step); it never moves back by itself. The ruler draws it as an E flag
  hanging to the right of its line at the top of the ruler's lower band (`Hit::EndMark`,
  `Drag::End`): a drag snaps and commits one `SetSongEnd` on release, `transport.set_end_marker`
  (Transport menu, no default key) sets it at the playhead and `sequencer.marker_end_here` (the
  ruler's context menu) at the snapped click. The lanes past it are shaded. `project_inspect` and
  `session_status` report `song.end_ticks` and `song.end_beats`. **The loop's flags.** The loop has
  no bar: `Song.loop_range` is drawn as an L flag at its start and an R flag at its end in the same
  band (`ArrangementView::flags`, `Hit::LoopEdge`, `Drag::LoopEdge`), with a tint over the ruler
  between them (stronger while `loop_enabled`) and the lanes' loop shade between them while it is
  on. L and E hang to the right of their line and R to the left, so R and E on one tick sit side by
  side; a flag whose body would cover an earlier one (End, then R, then L) takes the second row
  (`FLAG_H` lower). A press grabs a flag by its body (R before L before E where the grab areas
  touch) and a line within `LINE_GRAB` (locators before the End); a drag snaps like the End,
  `locator_range` keeps the minimum length (one snap step) and one `SetLoop` is sent on release,
  keeping `enabled`. `transport.set_left_locator` / `transport.set_right_locator` (Transport menu,
  no default key) set one at the playhead and `sequencer.locator_left_here` /
  `sequencer.locator_right_here` (the ruler's context menu) at the snapped click. A press on the
  band away from the flags is `Hit::RulerBand` and locates like the bar labels; there is no gesture
  that draws a loop in the ruler.

### 2.10 Deep zoom and vertical zoom

- **Time zoom and grid.** `MAX_PX_PER_TICK` is 16 px per tick (`ui/src/timeline.rs`, 128 times the
  old limit of a 1/16 note over 480 px): a tick is 1.5625 frames at 120 BPM and 48 kHz, so the
  deepest zoom draws a sample about 10 px wide (9 px at 44.1 kHz, 5 px at 96 kHz, 2 to 3 px per
  frame down to 40 BPM). The arrangement, the piano roll and the step editor share the limits
  through `TimeAxis`. `grid_step` walks down from the beat through 1/8 to 1/1024 of a quarter and
  ticks (`1, 5, 15, 30, 60, 120, 240, 480, ...` ticks; each step divides the next, so a coarser grid
  is a subset of a finer one). Lines finer than a sixteenth stay at least 40 px apart
  (`FINE_GRID_PX`) so a deep zoom shows a calm grid, and `grid_lines` thins a range that is far too
  long for the zoom (at most 20 000 lines per meter segment) and never loops on a zero, negative or
  NaN zoom. Below the bars the ruler names its lines `bar.beat.sixteenth.tick` (`deep_label`,
  `deep_labels`, only once the zoom shows lines finer than a beat).
- **Snap.** `Snap` has Bar, 1/2, Beat, 1/8, 1/16, 1/32, 1/64, 1/128, the triplets 1/2T, 1/4T, 1/8T,
  1/16T, 1/32T (`PPQ * 4 / 3`, `PPQ * 2 / 3`, `/ 3`, `/ 6`, `/ 12` ticks), Grid and Off; the
  selector cycles them in that order and `sequencer.set_snap` takes `half`, `thirtysecond`,
  `sixtyfourth`, `onetwentyeighth`, `half_triplet`, `quarter_triplet`, `eighth_triplet`,
  `sixteenth_triplet`, `thirtysecond_triplet` and `grid` besides the old values. A second setting,
  the **snap mode** (`SnapMode` in `ViewportState`, `sequencer.set_snap_mode` with `absolute` or
  `relative`, shown as `SNAP REL` in the arrangement corner), decides how a moved clip or note meets
  the grid: *absolute* puts its start on a grid line, *relative* (`seq_edit::drag_start_rel`) moves
  it by whole steps and keeps its distance from the grid. Neither is saved in a file. **Grid** is
  the visible grid: `Snap::step_zoomed` (and `SnapGrid::zoomed`) resolve it to the step `grid_lines`
  draws with at the current zoom, at most one bar, and a view without a zoom treats it as a
  sixteenth. A triplet step does not divide every bar (a 3/4 bar holds 4.5 quarter-note triplets):
  `SnapGrid` still restarts at every bar line of the meter map and the last cell ends at it. Snap is
  not saved in a file, so nothing about old files changed. The piano roll's Quantize takes the same
  step, triplets and Grid included.
- **Waveforms.** Below the finest peak level (256 frames per bucket, `FINEST_PEAK_FRAMES`) the
  arrangement's audio clips, the slice editor and the sample editor read the samples of the asset
  when the app handed them in (`set_samples`: the RAM assets of the engine, shared, never copied;
  assets streamed from disk above the 64 MiB threshold have none and keep the finest peak level).
  `ui/src/wave_paint.rs` is the shared painter: columns of at most 255 frames use `raw_min_max`, and
  below one frame per pixel the waveform is a line through the sample values with a dot on every
  sample once a frame is 6 px wide. A paint reads only the visible columns (at most 4096) or the
  frames under them, so its cost does not depend on the length of the clip or the file. The sample
  and slice editors' wave zoom reaches 32 px per frame whatever the file length (`max_wave_zoom`; it
  was a fixed 4096 times the fit).
- **Track heights.** A row is a base height times the arrangement's vertical zoom, within 24 to 400
  px. The base of a track's first lane is `Song::track_heights` (device key to pixels at zoom 1,
  project format 15, default 60; `SetTrackHeights` and the drag-merging `DragTrackHeights` are
  undoable and report `ViewChanged` only), the base of an automation lane is 52. The vertical zoom
  (`ArrangementView::track_zoom`, 0.4 to 4) is window state in `ui-state.json` (`track_zoom`); the
  piano roll's key height (6 to 48 px, `piano_roll_key_height`) is saved the same way. Alt+wheel or
  Ctrl+Shift+wheel zooms the tracks in the arrangement and the keys in the piano roll (Ctrl+wheel
  stays the time zoom, Shift+wheel scrolls sideways), keeping the row or key under the pointer where
  it is; `sequencer.zoom_tracks_in/out/reset` and `editor.zoom_keys_in/out/reset` are bound to
  Shift+H, Shift+G and Shift+Z in the standard profile. Dragging the bottom edge of a track header
  sets its height (a double-click puts the default back); a header shorter than 52 px keeps one line
  (chevron, name, M, S, R). Entries of devices without a track are dropped when saving. The Small,
  Medium, Large and Fit presets are in 2.16.

### 2.11 Scales, chords and fold in the piano roll

- **Theory** (`core/src/theory.rs`, re-exported by `model/src/theory.rs`, which adds the `Vec`
  helpers `chord_keys` and `diatonic_chord` and `detect_scale`; dependency-free, no UI).
  `chord_into` and `diatonic_chord_into` write a chord into a fixed array without allocating, for
  the devices. `ScaleKind` has sixteen kinds (Chromatic, Major, Natural, Harmonic and Melodic Minor,
  the modes, the two pentatonics, Blues, Whole Tone, Phrygian Dominant, Hungarian Minor); a `Scale`
  is a root pitch class and a kind. `Scale::nearest` gives the in-scale key nearest to a key (a tie
  goes **down**, the result stays in 0..=127); `Scale::step` walks scale degrees (from a key outside
  the scale the first step lands on the next scale key in the direction of travel; a step off the
  keyboard is `None`); `degrees_between` is its inverse. `ChordKind` has twenty-one chord shapes,
  `chord_keys` builds one in an inversion (the lowest note moves up an octave per inversion; chords
  of fewer than three notes have none; keys above 127 are dropped) and `diatonic_chord` stacks
  thirds through the scale tones (triad, seventh or ninth) from the nearest scale key.
  `detect_scale` picks the scale that leaves the least note weight outside it, then the fewest
  tones, then the heaviest root.
- **Model.** `Clip.scale: Option<Scale>` lives on note clips only and never on a chromatic scale
  (`SetClipScale` stores a chromatic scale as none and refuses audio and automation clips); it is
  one undo step, reports `SyncOp::SongChanged` like the other clip edits, does not change the notes
  or the engine's view of the clip, and survives split, duplicate and (the first clip's) join.
- **Clip names and colours.** `Clip::name` (a trimmed text of at most 64 characters) and
  `Clip::color` are the clip's own label and colour; without them the arrangement draws the
  generated label and the track's colour. `SongCommand::SetClipMeta { ids, name, color }` sets,
  clears or keeps each of the two on every listed clip (`MetaChange`), one `Edit::Clip` per clip
  that changes in one undo step; a blank name clears it. The rule for every copy: a name and a
  colour are part of the clip, so duplicate, copy and paste, the clip library, a split (both
  halves) and the first clip of a join keep them; a clip a command makes from nothing (a
  recording, a slice, a take compiled by a comp) has none. The fields are `"name"` and
  `"color"` after `"muted"` in the project and the clip file, written only when set, and the
  MCP `project_inspect` lists them in a lane's `named_clips`. The MIDI export of one clip is
  named after the clip when it has a name.
- **Loops turned into clips** ([loop_convert.rs](../crates/model/src/loop_convert.rs)).
  `WorkflowCommand::ConvertLoop { ids, mode }` turns the loop of every listed clip into one clip
  (`Single`: the content of every repetition unrolled into a clip without loop or offset, the
  automation and controller points through the same piece rules the flattening uses) or one clip
  per repetition (`Separate`: each piece keeps its content window as its offset; the first keeps
  the id). `DuplicateLoopInstance { id, index }` makes one repetition a clip of its own and leaves
  the ones before and after it looped. All three keep what the lane plays, which the flatten
  equivalence tests check; the cases that could not (a clip with swing for `Single`, another clip
  overlapping the looped one) are refused. A looped audio clip has no content to unroll, so the
  model refuses `Single` for it and the app renders it instead: `AppCore::apply_command` sends the
  looped audio clips of a `Single` conversion to the freeze worker as dry renders of the clip
  alone (`Kind::Loop` in `app_core_freeze.rs`), and the finished render comes back as
  `WorkflowCommand::RenderLoop { id, audio }`, which replaces the clip by one plain clip of the
  render (same id, place, name, colour and crossfade) in one undo step. The arrangement offers
  them in the clip context menu (`sequencer.convert_loop`, `sequencer.loop_instance` for the
  repetition under the playhead), a loop handle in the upper right corner of a clip's body
  (dragging it repeats what the clip shows; Shift on an audio clip's end edge does the same),
  `sequencer.clip_color` and `sequencer.rename_clip` (also a double-click on the clip's header strip).
- **Rows and fold** (`ui/src/key_rows.rs`). `KeyRows` is the list of visible keys, top row first,
  with `row_of` and `key_at_row`; the piano roll's geometry (`key_at`, `key_center_y`, hit testing,
  painting, the scroll range, band select and the key zoom) goes through it, so a folded roll is the
  same code with fewer rows. Fold to scale shows the scale keys plus any key a note of the clip uses
  (no note is ever hidden); fold to used notes shows only the used keys; a fold with nothing to fold
  to (no scale, no notes) shows every key. The rows are frozen during a gesture and rebuilt when it
  ends. A vertical drag moves by rows in a folded roll, by scale degree with snap to scale, and by
  semitone otherwise.
- **Edits** (`ui/src/chord_edit.rs`, pure functions on a note list). Conform, degree moves, the
  `Stamp` (none, a chord, or a diatonic chord), chordify, arpeggiate (a group is the selected notes
  on one start tick; steps are the current snap, the last cut at the chord's end, at most 4096 notes
  per group; Random is a seeded splitmix64 stream, so it is repeatable; As Played equals Up because
  the model keeps no entry order) and strum (the notes of a group start one delay apart from the low
  or the high end and keep their ends). The view sends one `SetClipContent`, so each is one undo
  step; notes that land on one start and key are merged (the longer stays).
- **Surface.** The scale bar under the toolbar (root, scale, DET, SCL SNAP, fold, chord, inversion),
  the commands `editor.set_scale`, `editor.set_scale_root`, `editor.detect_scale`,
  `editor.scale_snap_toggle`, `editor.fold_set`, `editor.fold_toggle`, `editor.conform_to_scale`,
  `editor.set_chord`, `editor.set_chord_inversion`, `editor.chordify`, `editor.arpeggiate`,
  `editor.set_arp_*`, `editor.strum` and `editor.set_strum_*` (the arguments are lists in
  `keymap/command.rs`, pinned to the theory names by a test), and context menus with submenus
  (`ContextMenu` opens submenus to any depth beside their row, at once when the pointer is on the row
  and none is open there; a switch waits `SUBMENU_DELAY`, which `Shell::context_menu_wake` hands to the
  platform loop as a deadline; a panel taller than the window scrolls with the `menu_layout` math shared
  with the menu bar; a release that ends the press which opened a menu chooses nothing).
  `Shift+S`, `Shift+F` and `Shift+C` are bound in the standard profile only. Snap to scale and the fold are window state (`piano_roll_snap_scale`,
  `piano_roll_fold` in `ui-state.json`).

### 2.12 MIDI controller data

Pitch bend, channel aftertouch, polyphonic aftertouch and CC lanes live in note clips, next to the
notes, and reach the track's device as the note events it already understands
(`NoteKind::PitchBend`, `ChannelPressure`, `PolyPressure`, `Cc`).

- **Model** ([controller.rs](../crates/model/src/controller.rs)).
  `Clip.controllers: Vec<ControllerLane>` (note clips only, sorted by `ControllerKind`: pitch bend,
  pressure, CC by number, poly pressure by key; one lane per kind; no empty lane). A lane holds
  `AutomationPoint`s in content ticks (`Linear` ramps to the next point, `Step` holds), values
  normalised like `NoteKind` (pitch bend `-1..=1`, the rest `0..=1`).
  `SongCommand::SetClipControllers { clip, lanes, merge }` replaces a clip's whole controller data
  in one undo step (a drag reuses its merge id; it can sit in a `Batch`); the model sorts the lanes,
  drops empty ones and refuses a bad kind number, an out-of-range or NaN value, a tick outside
  `0..=MAX_TICK` and unsorted points. A split and a duplicate keep the lanes on both halves (like
  the notes, the content stays aligned by the offset), a join merges them per kind with the later
  clip winning where clips overlap (each window is cut with a value at its start and a closing point
  when a ramp is cut). Project format 19 stores them as `"controllers"`; version 18 files load
  without lanes. Editing several points at once is pure functions on the lane list in
  [controller_edit.rs](../crates/model/src/controller_edit.rs) (`ControllerSnapshot`, `paste_points`,
  `remove_points`, `move_points`, `thin_selection`), each giving the whole canonical list for one
  `SetClipControllers`.
- **A track's own lanes** (format 44). `Track::controllers` holds lanes of the same shape outside any clip, in song
  ticks, for a note track only (`validate` and the reader refuse them on an audio or automation track).
  `SongCommand::SetTrackControllers { device, lanes, merge }` replaces the whole list as one `Swap::TrackControllers`
  (sorted and checked like a clip's; refused on a frozen track; a point past the song end moves the end through
  `song_end_growth`; a drag reuses its merge id, `MergeKey::TrackControllers`). `Track` is cloned whole by a
  duplicate, so the lanes follow it; `MergeNoteLanes` uses `flatten_clip_controllers`, the clips' lanes alone, so the
  track's lanes are not copied into the merged clips. The file field is `"controllers"` (written by
  `write_controller_lanes`, the writer a clip uses too, before the track's `"lanes"`); it is not ignorable, which is
  why the format moved to 44.
- **Flattening** (`flatten_controllers`, [flatten.rs](../crates/model/src/song/flatten.rs)). The
  notes' audible spans, offsets and loops apply. The track's own lanes join the clips' points by tick and win an equal
  tick, with no pieces: they play whether or not a clip is audible. A lane says nothing before its first point, so a
  piece emits a point at its start only when the lane already has a value there; a ramp still
  running at the end of a piece gets a closing point. Swing does not touch controllers.
- **Engine** ([controllers.rs](../crates/engine/src/controllers.rs)). `SongTrack::controllers`
  (`SongController`, `SongControllerKind`) is expanded in `compile_song`, off the audio thread:
  every point becomes an event at its tick with its exact value, and a `Linear` point also gets the
  values on the way to the next one every `RAMP_STEP` (`PPQ / 384` = 40 ticks) rounded to the MIDI
  resolution (14 bit for pitch bend, 7 bit for the others), skipping a value that did not change.
  The events join the track's note events in the one sorted list (at one tick: offs, then
  controllers, then ons, controllers in lane order), so the player walks them exactly like notes
  (sample-accurate, no allocation, no lock; an overflowing batch drops and counts as before). The
  same expansion is the chase index per lane. Chase and reset are described in the player's module
  docs: a seek restores each lane's value at the position; pitch bend, aftertouch and the sustain
  pedal (CC 64) go back to zero when there is no value yet and on Stop or a Locate while playing;
  other CCs (volume, expression, a filter cutoff) keep the last value they were given.
- **Devices and plugins.** CLAP and VST3 plugins get the events through the plugin host's note path,
  as live MIDI input always did (`crates/plugin-host`, `plugin-clap`, `plugin-vst3` translate every
  `NoteKind`). Of the built-in devices the poly synth, wavetable synth, FM synth, grain sampler and
  sampler respond through one shared helper, `ControllerState`
  ([controller_state.rs](../crates/devices/src/controller_state.rs)): pitch bend moves the voices by
  up to `bend.range` semitones (default 2; RPN 0 changes it), the sustain pedal (CC 64) holds
  note-offs until it goes up, volume and expression (CC 7, CC 11) scale the output, and channel and
  key pressure are kept as a modulation source; the mod wheel (CC 1) adds up to one semitone of LFO
  vibrato in the synths (`WHEEL_VIBRATO`, at the 16-sample control rate). The sampler reads it too,
  with two params of its own: `wheel.to_pitch` (semitones of LFO vibrato, default 1) and
  `wheel.to_cutoff` (octaves of filter cutoff, default 1), both zero-effect with the wheel at rest. `AllOff` keeps bend and
  the mod wheel, `reset` clears them. The chart of every instrument is under "MIDI" in
  [DEVICES.md](DEVICES.md) and is checked by a test. The other instruments ignore the messages.
- **Recording** ([record.rs](../crates/app/src/record.rs)). The MIDI forwarder already offered every
  event of an armed device to the record tap; a `Take` now keeps pitch bend, aftertouch and CC per
  loop lap next to the notes and writes them as the controller lanes of the lap's clip: `Step`
  points, a burst closer than `PPQ / 128` ticks folded into its last value, repeats dropped, values
  clamped, the clip range widened to the points. A lap with only controller data makes no clip when the
  take is built with `controllers_to_tracks` (the app always does): `Take::finish_all` returns it as a
  `ControllerPass` (the lanes in song ticks, `Step`, and the song time the lap covered), and the app lays all the
  passes of a track over `Track::controllers` with `controller_edit::replace_range` (what the lanes held in that time
  goes, other kinds and other times stay) as one `SetTrackControllers`; without the flag, `finish` still makes a
  clip of controllers alone. No MIDI learn is involved: the lanes need no mapping to a parameter.
- **Editor** ([controller_lane.rs](../crates/ui/src/controller_lane.rs),
  [piano_roll.rs](../crates/ui/src/piano_roll.rs)). The CONTROLLERS field of the piano roll's scale
  bar opens an area under the velocity lane (hidden by default and not saved; closed, the roll lays
  out as before): a row of chips (Bend, Press, Mod 1, Vol 7, Pan 10, Expr 11, Sust 64, a CC stepper
  whose arrows step the number 0 to 127, a poly aftertouch stepper for a key, and RAMP or STEP for
  the curve of new points; the pedal always steps) and the lane of the chosen controller on the
  roll's time axis. A press on empty space (select or pencil tool) adds a point and the drag draws
  freehand, one point per snap-grid step (about three pixels with snap off); a press on a point
  drags it (a click without a move changes nothing), the eraser or the secondary button deletes it,
  Ctrl or Cmd toggles its curve, Esc cancels. Time snaps to the roll's grid, the value to the MIDI
  resolution (pitch bend within 0.03 of the centre, 1.5 % of its range, lands on it); a selected
  point shows its value as 0 to 127 (pitch bend as -8192 to +8191). All edits are pure functions on
  the lane list (`set_point`, `move_point`, `remove_point`, `toggle_curve`) and one
  `SetClipControllers` each. Several points: Shift-click toggles a point in the selection, a Shift press on
  empty space drags a rubber band that adds the points inside it, dragging a selected point moves the whole
  selection together (time on the grid, value at the MIDI resolution, the group stopping at the edges of
  the range), Select All and Delete act on the points while the lane has the focus (a press in the lane
  sets it, a press anywhere else gives it back to the notes), and the THIN chip (or
  `editor.thin_controllers`, with `normal`, `heavy` or `maximum`, the level of the Interface thinning
  preference when none is given) simplifies the selection, or the whole lane when none is selected: the
  ramp points the line through their neighbours reproduces and the held values that repeat go, keeping
  the ends of each run. Copy and Cut put the points on the clipboard (`ClipboardData::Controllers`, the
  kind and the points relative to the first), Paste lays them into the lane shown from the playhead (values
  keep their place in the range of the target, points at or after the clip's content end are dropped, a
  point on an existing one replaces it) in one undo step and selects them. `editor.toggle_controllers`
  (Shift+L) shows and hides the area; the chevron at the left end of the chip row folds the lane away
  (`LaneEditor::folded`, view-only) so only the chip row stays, as an automation lane folds in the
  arrangement, and picking a lane chip unfolds it. The MSG chip at the end of the chip row switches the area to the
  clip's program changes and SysEx messages ([midi_lane.rs](../crates/ui/src/midi_lane.rs)): program
  changes on an upper row (`P5`, or `B3.0 P5` with a bank select) and SysEx messages on a lower one
  (`SYX 4`, the data length). A press on empty space of the upper row adds a program change (the program
  in force there), a press on a marker selects it and a drag moves it on the grid (one undo step), the
  eraser or the secondary button deletes it, Delete deletes the selected one, the PRG, MSB and LSB steppers
  change the selected program change (the bank bytes pass through "none"), + SYSEX adds General MIDI System
  On at the playhead and CLEAR empties the clip's data. Every edit is one `SetClipMidi`; the bytes of a SysEx
  message cannot be typed yet.
- **Track mode** (`PianoRollView::open_track`, `AppCore::open_track_controllers`). Track > Edit Track Controllers
  (`sequencer.track_controllers`) opens the roll on a note track's own lanes: the first selected clip's track, else the
  selected track or device, else the first note track. The roll keeps `clip_id` empty and `track` set, and its
  `sync` builds a stand-in clip (start 0, the length of the song, the track's lanes, no notes) so the axis, ruler,
  meter and the controller code read it like a clip. Every note edit starts from `clip_id` and so sends nothing; the
  grid, keys, velocity lane, scale bar, quantize buttons and the fold and message chips are not drawn or hit, and the
  area is always open and takes everything under the ruler. `ctl_command` and the drag send `SetTrackControllers`
  instead of `SetClipControllers`; the clipboard (`paste_track_controllers`: song ticks from the playhead up to the
  song end) and the Delete and Thin commands follow the same lanes. `AppCore::piano_roll_open` is true for a clip
  or a track; the roll closes when the track goes or stops playing notes. The roll's ruler and grid follow the
  song's whole meter map in this mode (`Bars::of`, `PianoRollView::bars`; a clip's grid is one meter from its
  start), while the Bar and Beat snap steps still use the first meter. A note track that holds lanes of its own
  has a CC badge in its arrangement header (`TrackButton::Controllers`, only on rows of 52 px or more, left of
  the master keyboard arrow); a click sends `UiAction::OpenTrackControllers`, which calls
  `AppCore::open_track_controllers`.
- **Parameter lanes** ([param_lane.rs](../crates/model/src/param_lane.rs)). A note clip can also carry
  `Clip.param_lanes: Vec<ParamLane>`: the points of one parameter of one device (`device`, `param`
  key, `AutomationPoint`s in content ticks, `Linear`, `Step` or `Bend`), sorted by `(device, param)`,
  at most 32, no empty lane. They are clip data like the notes: a move, a duplicate, a split (the
  content stays aligned) and a loop keep them, a join and a loop conversion rebuild them from the
  pieces each clip plays, and a reverse and a tempo scale mirror and stretch them.
  `SongCommand::SetClipParamLanes { clip, lanes, merge }` replaces a clip's lanes in one undo step; it
  refuses a device that is not in the rack or has no such param, clamps the values into the param's
  range and checks the points like an automation clip's (the check at `AddClip` and at load looks at
  the shape only, so a clip pasted from another project keeps a lane whose device is missing; it
  plays nothing). `WorkflowCommand::AutomationIntoClip { automation, into }` (Edit > Time and Clips > Move Automation
  into Note Clip; Edit > Time and Clips > Clear Clip Parameter Lanes sends the empty list) turns what an automation
  clip plays under a note clip into a lane of that clip and removes the automation clip.
  `flatten_param_lanes` flattens them like an automation lane (a piece starts with the value it has
  there, a ramp cut by the piece end is closed, the last point of a piece holds), merging the lanes of
  several note lanes by tick. The render ([project.rs](../crates/render/src/project.rs)) hands them to the
  engine as ordinary `SongAutomation` after every track lane and macro lane, and skips a param that
  a track lane (even a muted or empty one), a macro bank or an earlier track already drives, so a
  track lane wins; the engine needs no new code for them. The value a control shows
  (`RackDoc::automation_curves`) follows the same rule. A frozen track plays its render and none of
  its clips' lanes. The project file stores them as `"param_lanes"` after the clip's notes, a list of
  `{"device", "param", "points"}`, written only when the clip has any (format 41; a file without it
  is a file that never had them). The library's clip file leaves them out. `project_inspect` lists,
  for a note lane, each `(device, param)` with its point count and how many points are bent
  (`"param_lanes"`).
- **Bent segments.** `Curve::Bend(amount)` (`-64..=64`) ramps to the next point along `x^k`, with
  `k = 2^(amount / 32)`: 0 is the straight line, positive eases in (slow start), negative eases out,
  and the extremes are `k = 4` and `k = 1/4`. The shape is `atomik_core::bend::bend_shape`, which
  uses neither libm nor a table (an `atanh` series for the logarithm, a Taylor series and the
  exponent bits for the exponential, only IEEE `+ - * /`), so it is bit-identical on every platform,
  block size and worker count; it allocates and locks nothing, and costs about 100 flops for a
  bent segment (a straight or held one costs what it did). The player (`lane_value`), the overlay
  (`automation_value_at`) and the flattening all call it (`Curve::blend` in the model), which keeps
  what a control shows equal to what the engine applies. A bend is not a power curve any more when a
  piece cuts the segment, so a segment a clip edge, an offset or a loop end cuts is replayed as a
  chain of at most 128 straight ramps that follow the curve; a segment the piece holds whole keeps
  its bend. The file stores a bent point as `[tick, value, "bend", amount]` (format 41, only when
  used; a bend in a controller lane is refused). Thinning keeps a bent point and the point it leads
  to; a reverse flips the sign of a bend (the mirror image of `x^k` is close to `x^(1/k)`).
  In the arrangement, a drag on an automation segment with the fine-adjust role held (Shift by
  default) bends it so the curve passes through the pointer at the middle of the segment (one undo
  step); a click without a drag, or a Shift-click on a point, straightens a bent segment. Points
  added by a click or the pencil on a choice, switch or other whole-valued param are `Step`.
- **Inspection.** `project_inspect` and `session_status` list, for a note lane, the controller kinds
  in use with their point counts (`"controllers"`, omitted when there are none).

### 2.13 Loop recording, takes and comping

A loop recording stacks its passes as **takes** of one **take group** on the lane; one of them (or a
mix of parts of them, the **comp**) plays and the others are kept, silent. Punch and non-loop
recording are unchanged: a track with one pass still gets one plain clip.

- **Model** ([takes.rs](../crates/model/src/takes.rs),
  [takes_cmd.rs](../crates/model/src/takes_cmd.rs)). `Lane.take_groups: Vec<TakeGroup>` (note,
  audio and automation lanes). A
  `TakeGroup { id, takes: Vec<Clip>, comp: Vec<CompSegment>, clips: Vec<ClipId>, link }` keeps its
  takes (whole note, audio or automation clips, bottom to top in recording order) **off** `Lane::clips`, so nothing
  that reads the lane's clip list (the engine's flatten, the editors, selection, the clipboard, the
  clip-drag gestures) can see a hidden take. The comp is a sorted list of disjoint
  `CompSegment { take, start, end }` (a take is its index in `takes`; a range with no segment is
  silent) inside the group's range, which is derived (`TakeGroup::range`: the earliest take start to
  the latest take end). Group ids are unique in the song and not saved; a loaded song numbers them
  from 1 in file order.
- **The comp is compiled to plain clips at command time.** Every take command is one compound edit
  that changes the group list **and** swaps the group's compiled clips on the lane
  (`TakeGroup::clips` remembers their ids), so `flatten_*`, the engine's song input, the bounce and
  every older reader see only ordinary clips and needed no change. The alternative, a comp the
  engine resolves while playing, would need new engine and song-input types and a second place that
  knows the comp rules, with takes stored as hidden clips that the playback path has to skip. The
  cost of compiling is one copy of the clip (the notes, or the clip header and the asset reference)
  per comp segment. A compiled clip is the take clip cut to its segment: the content offset moves by
  the cut exactly as `SplitClip` moves it, so notes and controller lanes keep their alignment, and
  an audio clip starts the frames between the two ticks later in its asset (tempo map and document
  rate, clamped to the asset).
- **Commands** (`SongCommand`, each one undo step: one `Edit::TakeGroups` plus the clip edits).
  `NewTakeGroup { device, lane, takes }` validates the takes like `AddClip`, allocates their ids and
  makes the comp the takes layered in order, a later take hiding an earlier one where both have
  content, which is what stacked clips played, so grouping does not change the sound.
  `PickTake { group, take }` makes the comp that one take over the whole range.
  `SetCompSegment { group, take, start, end }` cuts the range out of what played there and joins
  neighbours of the same take. `DeleteTake { group, take }` gives its ranges to the take below it
  (above it for the bottom take) and renumbers the takes above it; the last take takes the group and
  its clips with it. `FlattenTakes { group }` drops the group and keeps the compiled clips as plain
  clips (no `SongChanged`: the sound does not change). `DuplicateTake` puts a copy of a take just
  above it, `MoveTake { group, take, to }` reorders the stack (both renumber the comp so what plays
  does not change), `DeleteUnusedTakes` drops the takes no segment names, `SetCompSilence` cuts a gap
  out of the comp, `SetCompXfade { group, at, xfade }` sets the crossfade of the cut at tick `at`
  and `SetTake { group, take, gain_db, source_offset_delta }` sets the level of an audio take and
  moves its audio inside the asset (the take keeps its start). `SetTakeLoop { group, take, repeats }`
  repeats a take: its extent (or its loop length) becomes the loop and the take runs `repeats`
  repetitions, the comp segments that ended at its end run on to the new end, and 1 plays it once
  again. A looped note take keeps its loop in the compiled clips; a looped audio take is compiled
  to one plain clip per repetition (`takes::repetitions`), each restarting the source and keeping
  the fades and warp of a clip of its own, so the lane plays what the looped take played and a cut
  inside a repetition fades like a cut in any audio take. Take clips count as referenced
  assets (the bundle keeps their files and undo finds them) and `RelinkAsset` swaps them too.
- **Takes clipboard** ([workflow.rs](../crates/model/src/workflow.rs) `TakeSnapshot`). Copy Take and
  Copy All Takes capture takes of a group with their layout relative to the earliest copied take and
  the pool entries of audio takes. `WorkflowCommand::PasteTakes { target, at_tick, takes }` puts them
  into a group as new takes on top (`TakeTarget::Group`; each plays over its extent, as `AddTake` does)
  or starts a group on a lane (`TakeTarget::Lane`); the takes must be of the target's kind and an audio
  take needs its asset in the pool, which the app registers first (a cross-song paste, as for clips).
  The app keeps the snapshot on the in-process clipboard (`ClipboardData::Takes`), so it survives New
  and Open, and the workspace carries the clipboard to the song the calls are routed to next, so a copy
  pastes into another open song. Edit > Copy, Cut and Paste work on the take row that has the keyboard
  focus; Paste with no row focused starts a group on the selected lane at the playhead.
- **Editing a take.** A take has a clip id like any clip but lives in its group, so the clip editors
  open it by that id (`Song::clip_or_take`). The commands they send for a clip (`SetClipContent`,
  `SetAudioClip`, `SetWarpMarkers`, `SetClipGrid`, `SetClipControllers`, `SetClipMidi`,
  `SetClipParamLanes`, `SetClipScale`, and `PasteNotes` and `DuplicateNotes` for the piano roll's clipboard) are planned
  by `plan_take_edit` when the id is a take: the command is planned on a scratch document where the
  take is a clip of its lane, and the clip it comes out as replaces the take in its group, with the
  comp compiled again, in one undo step. A command that is not one of these (a split, a strip silence)
  names no lane clip and is refused as before.
- **Take rows by key and import.** The arrangement keeps a take row with the keyboard focus
  (`ArrangementView::take_focus`, valid while no clip is selected): a click on a take row or its menu
  focuses it, `sequencer.take_next` and `sequencer.take_prev` (Alt+Down, Alt+Up) step through the take
  rows that are shown, `sequencer.take_pick` (Alt+P) plays the focused take alone and
  `sequencer.open_clip` (Enter) opens it in the piano roll or the audio editor. The take menu also
  has Copy, Paste, Open in Piano Roll or Audio Editor and Import File as Take
  (`UiAction::ImportTake`): the shell remembers the group, asks for an audio file or a MIDI file
  with the usual dialog request and the file becomes a take starting where the group starts
  (`AppCore::import_takes`, `AppCore::import_midi_take`; a MIDI file's tracks are merged into one
  take). The project format is unchanged.
- **Audio boundaries.** A comp cut that is not the natural edge of its take (a segment that begins
  or ends strictly inside the take) gets a fade of `COMP_FADE_FRAMES` (240, 5 ms at 48 kHz, at most
  half the piece), equal power; a natural edge keeps the own fade of the take. Overlapping audio
  clips hide an earlier one where they overlap unless the later one has an `xfade`, so by default
  the boundary between two takes is a fade out followed by a fade in, a dip of a few milliseconds.
  A cut can carry a **crossfade** instead: `CompSegment.xfade` is a length in ticks at the segment's
  start (0 is the dip). `comp_clips` then lets the clip before the cut run on past it by that
  length and gives the clip after the cut `xfade: Some(EqualPower)` and no dip, so `flatten_audio`
  makes two overlapping regions (the engine has mixed overlapped audio regions since format 23)
  whose equal-power gains square-sum to 1. The length is clamped to the later segment and to what
  is left of the earlier take after the cut; it applies only where one segment ends exactly where
  another of a different audio take starts. A comp without `xfade` compiles as before.
- **Project format 20** saves a lane's `"take_groups"` after its `"clips"`:
  `{"takes": [<clip>…], "comp": [[take, start, end]…], "clips": [id…]}`; a segment with a crossfade
  has a fourth element, its length in ticks (written only when above 0; a reader of this build
  accepts three or four). The take clips use the
  lane's clip fields and ids from the same pool (unique, below `next_clip_id`, none in `"clips"`);
  `"clips"` must name clips of the same lane, each at most once. The field is omitted for a lane
  without groups and unknown in version 19 and older, which load without groups. Under format 44 an
  automation lane may carry `"take_groups"` too, and a group an optional `"link"` tag (an integer
  from 1, written only when set); both are ignorable, so an older build drops the groups and
  keeps the compiled clips.
- **Linked groups.** `SongCommand::LinkTakeGroups { groups }` gives two to 64 groups one new tag
  and `SongCommand::UnlinkTakeGroup { group }` clears one; the groups that share a tag are
  linked, whatever lane they are on. The arrangement sends a comp gesture in a linked group as
  one `SetLinkedComp` over the linked groups (a saved link wins over the track selection that
  linked groups before), and a duplicated track starts unlinked. Link With Selected Tracks and
  Unlink Take Group are in the take menus.
- **Recording** ([app_core.rs](../crates/app/src/app_core.rs) `finish_take`,
  [app_core_rec_audio.rs](../crates/app/src/app_core_rec_audio.rs)). The MIDI `Take` and the audio
  writer already end a loop pass in one clip per lap. When a track has more than one lap the app
  sends them as one `NewTakeGroup` instead of one `AddClip` per lap, so the whole recording is one
  undo step per track and the last pass plays. A recording that never wraps the loop (or runs
  without one) is a single `AddClip` or `AddAudioClip`. An automation take does the same per lane
  (`finish_automation_take`): each pass that wrote is a lap, and a lane with several laps gets one
  `NewTakeGroup` of automation clips inside the take's `Command::Batch` (which accepts it), the
  last pass playing; one lap is plain `AddClip`s as before.
- **Recording over clips** ([record_over.rs](../crates/model/src/record_over.rs),
  [app/src/record_over.rs](../crates/app/src/record_over.rs)). A lane's rule is by start (a
  later-starting clip hides an earlier one over the overlap, with or without notes), so a plain take
  that starts before an older clip is hidden by it from the older clip's start. The **record mode**
  (Settings, Workflow, **Recording over clips**; `workflow.json` `"record_mode"`: `"replace"`, the
  default, or `"layer"`) chooses what a finished MIDI take does. Replace is what is described above.
  Layer builds one clip from all the passes (`merge_laps`: a later pass wins a note at the same
  start and key, controller kinds merge with the later pass replacing the older points over its
  span) and then either merges it into the unlooped note clip under its start
  (`WorkflowCommand::LayerRecording { clip, take }`: the clip keeps its id, grows to hold the take,
  keeps its notes, a new note replaces one with the same start and key, a controller kind the take
  carries replaces the old points of that kind over the take's span) or, with no clip under the
  start, adds it as a new clip that plays over what it overlaps
  (`WorkflowCommand::EngulfRecording { device, lane, clip }`: an unmuted clip that starts inside the
  new one and ends within it goes, one that runs on has its start trimmed to the new clip's end with
  the offset advanced, a loop keeping its phase; clips that start earlier are untouched). Both are
  one compound edit, so one undo step per take. Audio recorded over existing clips ignores the mode:
  the finished laps become `SongCommand::AddTake { group, clip }` on the take group whose compiled
  clips they overlap, or `SongCommand::GroupAsTakes { device, lane, clips, extra }` (the overlapped
  plain clips leave the lane and become the first takes, ordered by start and id, the laps the takes
  above them; the comp layers them in order), so the new take plays over the overlap and the old
  clip everywhere else; several laps are one undo step (`Document::apply_joined` folds a command
  into the undo step before it).
- **Keep events in clip and note tails.** A note that starts at or after its clip's end never plays.
  Settings, Workflow, **Notes outside a clip** (`"keep_events"`: `"keep"`, the default, `"expand"`,
  `"new_clip"`) says what the editors do about it
  ([app/src/record_over.rs](../crates/app/src/record_over.rs) `keep_events_follow_up`):
  `AppCore::apply_command` follows a `SetClipContent` of a note clip, `PasteNotes` and
  `DuplicateNotes` with `SetClipGeometry` (Expand: the clip grows to the bar line after the farthest
  note end) or with a `SetClipContent` that trims the clip and an `AddClip` that takes the outside
  notes (New clip: the new clip starts on the bar line at or before the first of them, never before
  the old clip's end). The follow-up is folded into the undo step of the edit; a drag (a
  `SetClipContent` with a merge id) is completed when it ends (`UiAction::EndMerge`). A looped clip
  is left alone. **Tails** (`Song::tails`, project format 35, `flatten_notes_with`): with the flag a
  note keeps its own length and only its start is held to the audible span, so it rings past the
  clip end, the loop end or the start of a later clip; the engine releases held keys at a loop wrap
  and at stop as it always did (`song_chase.rs`). A project without the flag cuts notes at the end
  of their piece as before, so older projects play and bounce bit for bit as they did; a new project
  (File, New) has it on. The piano roll dims what the clip does not play, tails and masked notes
  included; for a looped clip the dimmed part is what no repetition of the loop window plays
  (`looped_masked_spans`).
- **View** ([arrangement/takes.rs](../crates/ui/src/arrangement/takes.rs)). Under lane 0 of a track
  with groups the arrangement lays out a thin comp strip per group (each segment in the colour of
  its take) and, when the group is opened with the chevron in the strip's header, one row per take
  (saved with the project, see 2.16). A click on the name of a take, or inside its row, picks it; a
  drag across a take row comps the snapped range from it (one `SetCompSegment` on release, Escape
  cancels); the cross in the header of a take deletes it and FLATTEN dissolves the group. An audio
  take row draws its waveform. A drag across the strip silences the range, a drag of a cut between
  two takes moves it (one `SetCompSegment` for the take that grows) and the tail of a crossfade
  has a handle for its length; the right-click menus of the strip and of a take row offer the
  crossfade lengths, duplicate, move, silence, delete unused, and for audio a level and offset
  step (each a single command). The
  compiled clips stay ordinary clips on the lane: they can be moved, split or edited like any, but
  the next take command on the group recompiles them from the takes and replaces such edits (undo
  brings them back); flatten first to keep an edit. A take cannot be opened in the piano roll yet.
- **Inspection.** `project_inspect` and `session_status` list a lane's groups under `"take_groups"`:
  the id, the number of takes, the range, the comp as `{take, start, end}` (takes 0-based), the take
  that plays alone when the comp is one whole pick, and the ids of the compiled clips.

### 2.14 Quantize, swing, groove templates and humanize

Quantize edits the notes: it is **baked** into the note times as one undo step, so the engine,
`flatten_*` and a bounce see ordinary notes and nothing in playback changed. The other way to
use a groove is **live**: a track names a groove and a strength (`Track::groove`, a `TrackGroove`)
and `flatten_track_notes` applies it after the ordinary flattening, so the stored notes stay as
they are (below). A clip's `StepGrid` swing is a third thing, on a different scale: it never edits
a note and is applied first.

- **Pure functions** ([groove.rs](../crates/model/src/groove.rs), all unit-tested).
  `quantize_notes(notes, selected, origin, clip_seed, params, groove)` moves each selected note's
  start towards the nearest *target* by `strength` (0 stays, 1 lands). The targets are the grid
  positions `i * grid` **of the song** (`origin = clip.start - clip.offset` is the song tick of
  content tick 0, so a clip off the grid still lands on the bar lines), each delayed by the swing
  when `i` is odd and offset by the groove slot `i mod len`; the nearest is picked among those
  targets, not among the plain grid lines, so a heavily swung off-beat captures the notes near it.
  Swing is on the 50..75 % scale: an odd step is delayed by `(swing - 50) / 50` of a step (66 % is a
  triplet feel). The commands plan through `quantize_notes_in`, which takes the song's bar lines: a
  step is odd when it is the odd step of its own bar, so a meter change or a bar of an odd number of
  steps restarts the swing at the bar line (a 4/4 song is unchanged; `quantize_notes` counts from the
  start of the song). `humanize_length` changes each length by a bounded random amount (one tick at
  the least, its own hash stream) and `legato` stretches each quantized note to the next selected
  start. A
  `Groove { name, step, slots: [GrooveSlot { offset (steps, -0.5..=0.5), velocity (scale, 0..=2) }] }`
  has its own step, and a quantize that names one runs on the groove's step (the grid field is not
  used). `snap_ends` also moves each note's end towards its target (a note never ends up empty).
  `extract_groove` measures a groove from a clip's notes: the average offset to the nearest grid
  line and the average velocity relative to the clip's average, per slot (rounded to 1/1000; a slot
  no note is near stays straight). Six built-in grooves (`builtin_grooves`) are not stored in the
  song; their names are reserved.
- **Humanize is deterministic.** The timing and velocity offsets are
  `signed_unit(hash64(seed, clip id, tick ^ key << 56))`: a pure function of the seed, the clip and
  the note's own position before the edit (not of the other notes, the session or the clock), so the
  same command on the same song gives the same notes, a bounce is reproducible, and another `seed`
  is another take. `hash64` is the integer-only splitmix hash of `atomik_dsp::rng`; the model does
  not depend on the DSP crate, so it keeps a copy whose value a test pins to the DSP crate's. Timing
  is `humanize_time` (0..=1) times a 1/16 note either way, velocity `humanize_velocity` times half
  the range, both clamped to a valid note.
- **Collisions.** A clip holds at most one note per `(tick, key)`. Notes that were not selected keep
  their places and claim them first; a moved note that lands on a taken `(tick, key)` moves on a
  tick at a time to the next free one. No note is ever dropped.
- **Controller points follow their notes.** With `QuantizeParams::controllers` (on by default)
  `groove_cmd::clip_edit` hands the `(old start, new start)` of every note that moved
  (`Quantized::moves`) to `controller_edit::follow_note_moves` and puts the result in the same
  `Edit::Clip`. A point between a note's old start (included) and its new start (excluded) moves by
  the note's distance; where two spans cover a point the nearest old start claims it; a point that
  ends on a tick another holds loses to the later one; ticks stay in `0..=MAX_TICK`. Values and
  curves are untouched, and the lanes stay canonical. Points outside every span, device-parameter
  lanes (`Clip::param_lanes`) and a track's own controller lanes do not move.
- **A grid per note.** With `QuantizeParams::grid_by_length` (and no groove, whose step is the one
  grid) each selected note snaps to `grid_for_length(length, params.grid)`: the straight note value
  from a 1/32 note up to `params.grid` (or `params.grid` itself) whose ratio to the note's length is
  nearest, the finer on a tie. The targets of every grid in use are built once per command.
- **Commands** (`SongCommand`, each one undo step). `ApplyQuantize { clips, quantize }` plans one
  `Edit::Clip` per note clip that changes (audio and automation clips and the compiled clips of a
  take group are skipped; an error "quantize changes no note" when nothing changes, so an empty undo
  step never exists). `QuantizeNotes { clip, notes, quantize }` does the same for some notes of one
  clip by index. `SetGroove`, `DeleteGroove { name }` and
  `ExtractGroove { clip, name, resolution, slots }` edit the library through one `Edit::Grooves` (a
  view change, `ViewChanged`; at most 64 grooves). A groove is referred to **by name**, from the
  command (`QuantizeParams::groove`), so changing a template and applying again re-times the notes
  to the new offsets (at full strength the notes snap to the new targets as long as the offsets are
  small against the step).
- **Live groove.** `SetTrackGroove { device, groove: Option<TrackGroove> }` (one `Swap::TrackGroove`,
  `SongChanged`; a note track only, the groove must exist, strength 0 to 100) sets it. `flatten_track_notes`
  (the one flattening the engine's `song_input`, the offline render and the MIDI export use) runs
  `flatten_notes_with` and then `groove_notes`: each note moves by the offset of the slot of the
  song-grid position nearest to its start times the strength, its velocity scales by the slot's scale,
  its length stays, its start never goes below tick 0, and the result is sorted again. Strength 0 gives
  `flatten_notes_with` exactly. `SetGroove` on a groove a track uses reports `SongChanged` too, so a
  template edit re-times every user at once; `RenameGroove { from, to }` renames the groove and the
  tracks that use it; `DeleteGroove` takes it off them (both one undo step, a compound of the library edit
  and a swap per track). The track fields are `"groove"` and `"groove_strength"` (format 43, omitted when
  100). A note clip can carry a groove of its own (`Clip::groove`, `SetClipGroove { clips, groove }`, one
  compound of `Edit::Clip`): `flatten_track_notes` plays the track's groove for the clips without one and
  each distinct clip groove for its own clips (which clip hides which is decided first, over all of them),
  then merges by `(tick, key)`. The rename and delete above follow on the clips too. The clip fields are
  `"groove"` and `"groove_strength"` after `"sysex"`, ignorable under format 43; a clip that names a groove
  the song lacks (pasted from another song) loads without one. A live groove never moves controller
  points (it moves note starts at play time, as above).
- **A groove from a MIDI file.** `AppCore::groove_from_midi(path, step, slots)` reads the file as an
  import does (`read_midi`), merges the notes of all its tracks and channels, measures them with
  `extract_groove` from tick 0 and adds the groove through `SetGroove` under the file's name
  (numbered when the song or the built-in grooves have it, cut to 32 characters). A file that cannot
  be read, or has no notes, is reported and changes nothing. The quantize panel's `QuantizeAction::FromMidi`
  becomes `ShellRequest::PickGrooveMidi` (the platform asks for a file and calls
  `Shell::groove_from_midi_file`), and a `FileDrop` of a MIDI file on the open panel calls the same.
- **Library in the project.** `Song.grooves` is the song's shared library, saved by project format
  21 as `"grooves"` (see section 2 above for the fields; names are unique, not a built-in name, and
  at most 32 characters; a version 20 file loads without it and the field is unknown there).
- **Panel** ([quantize_panel.rs](../crates/ui/src/quantize_panel.rs),
  [app_core_quantize.rs](../crates/app/src/app_core_quantize.rs), `Shell`). `QuantizeView` is a
  modal like the relink panel with a `-` / `+` pair and the wheel per value (grid, strength, swing,
  groove, groove velocity, humanize timing, velocity and length, seed, snap ends, legato, grid per
  note, move controllers), a text
  field that names and renames the chosen groove of the song (`RENAME` or Enter, `RenameGroove`;
  `Shell::wants_text_input` follows its focus), `LIVE ON CLIPS` and `CLEAR LIVE` (`SetClipGroove` on the
  target's clips at the panel's strength) and the groove library list. It has no document: the
  shell tells it what APPLY works on (`AppCore::quantize_target`: the
  piano roll's selected notes, else the arrangement's selected note clips, the roll first while it
  is the focused pane) and the grooves on offer, and reads `QuantizeValues` back (kept between
  openings, not in the project). Edit > Quantize Options (`edit.quantize_options`, unbound by
  default, also in the clip and note context menus and reachable by `session_command`) opens it. The
  quantize key (`editor.quantize`: `Ctrl+K` / `Cmd+K` in the editor and sequencer contexts, `Q` in
  the legacy profile) and the piano roll's QUANT button apply the panel's last settings without
  opening it: the roll answers with `UiAction::Quantize(QuantizeAction::ApplyLast)` (its `...`
  button and a right click on QUANT send `Options`) and `Shell` applies `quantize_values().params()`
  through `AppCore::apply_quantize`; from the arrangement the shell runs the command itself on the
  selected clips. The arrangement corner has a QUANT button (`CornerButton::Quantize`, between S and
  MAN REC) that sends `QuantizeAction::ApplyClips` (the same on the selected clips, also while the roll is
  open; a right click sends `Options`). The roll has no quantize strength or grid of its own any more.
- **Inspection.** `project_inspect` and `session_status` list the song's grooves under
  `song.grooves` (name, step, slots), left out when there are none.
- **Note tools** ([note_tools.rs](../crates/model/src/note_tools.rs)). `ApplyNoteTool` (clips, an optional
  selection, a tool) edits the notes of several clips, or the selected notes of one clip, in one
  undo step: `NoteTool` sets, scales, ramps or randomizes velocity, alters notes
  (each takes, by chance, the pitch of another note of the set), moves pitches at random within a
  range, or sets one pitch. Audio and automation clips and the compiled clip of a take group are
  skipped, and a command that changes no note is refused. The random tools take a seed and derive
  each note's offset from `hash64` of the seed, the clip id and the note's tick and key before the
  edit, so the same command on the same notes always gives the same notes. The pitch tools keep
  one note per tick and key: a note that would stack keeps its old key. The roll and the
  arrangement send the command (`editor.note_tool` and `sequencer.note_tool`, one argument per
  menu entry, listed in [note_tool_args.rs](../crates/ui/src/note_tool_args.rs)); the roll counts
  its uses and passes the count as the seed, so each run is a new take, and it computes the same
  result (`apply_note_tool_indexed`) to keep its selection on the same notes. Legato, join and
  chop are pure list edits in [seq_edit.rs](../crates/ui/src/seq_edit.rs) (`make_legato`,
  `join_notes` merges the selected notes of one key into one, a gap filled; Ctrl+J and Ctrl+Shift+J
  run join and legato; `chop_notes`), each one `SetClipContent`. The velocity lane is a pencil on its
  empty part: a drag sets every bar it passes, Shift draws a straight line from the press and Alt
  limits the stroke to the selected notes (`velocity_stroke`); a stroke is one merged undo step.
  `AppCore::sync_roll_labels` names the keys of a drum machine track after its pad files on the
  roll's keyboard strip (display only; the notes keep their keys).

### 2.15 MIDI files

Standard MIDI Files (format 0 and 1) go in and out of clips, tracks and the song. Three layers, none
of them touching the engine or the audio thread.

- **The file format** ([smf.rs](../crates/midi-io/src/smf.rs), `atomik-midi-io`, no dependency
  added). `smf::read` gives an `SmfFile` (format, ticks per quarter note, tracks of events at
  absolute ticks) and `SmfFile::to_bytes` writes one back with running status and an end-of-track
  event per track. It keeps channel messages, whole SysEx messages (`F0 ... F7`, no packets) and the
  tempo, time signature, marker and track-name meta events, skips unknown meta events and unknown
  chunks, reads a RIFF-wrapped file,
  accepts running status across meta events, reads a truncated track up to the cut and refuses type
  2, an SMPTE division, a bad header, a data byte with no status, a number longer than four bytes
  and a file with no track, each with a message. It never panics (property tests feed it random and
  mutated bytes).
- **The conversion** ([midi_file.rs](../crates/app/src/midi_file.rs), pure functions in the app
  crate, which is where the model and `atomik-midi-io` meet). *Export* writes what plays: a clip, a
  track or the song is flattened with the model's own `flatten_notes` and `flatten_controllers`
  (loops, offsets, swing, a later clip hiding an earlier one, muted notes and clips left out). The
  file's resolution is a choice (`MidiResolution`): the model's own, `PPQ` = 15 360 (a valid 15-bit
  division), so every tick survives a round trip, or 960 or 480 ticks per quarter note for hardware
  that dislikes 15 360 (each tick is rounded to the nearest file tick; a note keeps at least one
  tick and one that a following note of its key leaves no tick for is dropped); velocities are 7 bit,
  pitch bend 14 bit. A clip or a track is a type 0 file on
  channel 1 (a clip carries the tempo and time signature in effect where it starts and ends at the
  clip's length; a track carries the whole tempo map, meter map and markers); the song is type 1: a
  conductor track (name, tempo map, time signatures on the bar lines, point markers) and one track
  per note track on channel `index mod 16`. Tempo ramps become a staircase, a linear controller
  segment is sampled, overlapping notes of one key are cut so each note-off pairs with its note-on.
  Audio and automation lanes are not exported; a range marker is a marker at its start named after it
  and one named `<name> (end)` at its end. The program changes and SysEx messages a clip carries
  (`flatten_midi_data`: the same pieces, offsets, loops and hiding as the controllers) are written on
  the track's channel after the controllers of their tick; a program change that has a bank select
  (`bank_msb`, `bank_lsb`) writes `CC 0` and `CC 32` just before it (a bank select an older project keeps
  in a controller lane is written with the lane, as before). *Import* makes one `MidiTrack` per
  MIDI track and channel that has notes (type 0 splits by channel): a note-on with velocity 0 is a
  note-off, a re-struck key ends the earlier note, a hanging note ends with its track, a note is at
  least one tick, two notes at one tick and key keep the first; controllers become `Step` points
  (the last value at a tick wins) with channel-mode, data-entry and (N)RPN controllers skipped; a
  `CC 0` or `CC 32` just before a program change at the same tick joins it and leaves the lane;
  program changes (the last at a tick wins) and the SysEx messages are kept in `Clip.midi` (at most
  4096 changes, 1024 messages of 4096 bytes per clip; the SysEx of a file track goes to its first clip,
  and a track with SysEx but no notes becomes a clip without notes that carries it); a channel with
  controllers and no notes is not a clip: its lanes are `MidiTrack::track_controllers`, which go to the
  first track made from the same file track that has notes, else to a track of their own that has no clip
  (`ImportedTrack.controllers`, set by `ImportMidi` with `SetTrackControllers` in the same undo step); a
  channel with only program changes is left out; a marker named
  `<name> (end)` closes the latest earlier marker named `<name>` into a range (`MidiImport.ranges`);
  tempo (as BPM rounded to 0.001), time signatures (a change in the middle of a bar takes effect at
  the next bar line, unsupported signatures are skipped) and markers (trimmed, 64 characters) come
  from every track. The file's ticks are rescaled with rounding and clamped to `MAX_TICK`.
- **The commands.** `WorkflowCommand::ImportMidi(MidiImport)`
  ([midi_import.rs](../crates/model/src/midi_import.rs)) is the one model change: planned on a
  scratch copy like the other workflow commands, it adds per imported track an instrument (with its
  console channel, as `CreateInstrument` does), an unarmed song track, the device name and a note
  clip, then replaces the tempo map and the meter map when given, adds markers and ranges with fresh ids, and
  grows the song end to the next bar line when a clip reaches past it, all as **one undo step**; a
  rejected import changes nothing. A clip import is a plain `AddClip`. A clip's program changes and
  SysEx messages are the new data `Clip.midi` ([midi_data.rs](../crates/model/src/midi_data.rs), shared
  through an `Arc`): saved as the optional, ignorable `"programs"` (`[tick, program]`, or
  `[tick, program, bank MSB, bank LSB]` with `null` for a byte not sent) and `"sysex"`
  (`[tick, "7E 7F 09 01"]`) fields of a note clip under format 43, kept by moves, splits, duplicates,
  loops, joins, loop conversion and tempo scaling, and played by nothing (a later field of the same data, the
  per-note `"expression"`, is played: see "Per-note expression"). `SongCommand::SetClipMidi
  { clip, midi, merge }` replaces the whole data in one undo step (a drag shares a merge id); the pure
  edits (add, move, delete, step) are methods of `MidiData` and the functions of
  [midi_lane.rs](../crates/ui/src/midi_lane.rs), which the piano roll's message lane uses (below).
- **The app** ([app_core_midi.rs](../crates/app/src/app_core_midi.rs), `Shell`, the menu). File >
  Import MIDI puts the whole file (all tracks and channels merged) in one clip on the selected note
  track (the track selected in the arrangement, the track of a selected or open clip, or the
  selected rack device) at the snapped playhead, and makes new tracks (a poly synth each) at the
  playhead when no note track is selected; File > Import > Import MIDI as Song makes new tracks from tick 0
  and also takes the tempo map, meter map and markers. The file browser lists `.mid` and `.midi`
  files (`FileKind::Midi`, the extensions are `atomik_library::MIDI_EXTENSIONS`, shared with the
  import dialog) with a MIDI label, and a double click or Enter on one is File > Import > Import MIDI. A
  MIDI file dropped from the browser or the system on the arrangement goes where it lands
  (`UiAction::ImportMidi`, `AppCore::import_midi_at`): on a note lane it is one clip on that lane
  at the snapped drop tick (a file of several tracks is merged into that one clip, as the import
  does), elsewhere in the track area it makes new tracks from the drop tick; dropped anywhere else
  in the window (the rack included, an instrument there is not a target) it makes new tracks at the
  playhead (audio files in the same drop go on as before).
  File > Export > Export MIDI Clip, Track and Song are disabled while there is nothing to write (a note clip
  selected or open, a note track selected, a note track in the song). The dialogs are
  `ShellRequest::PickImportMidi` and `PickExportMidi`; the file is read and converted before
  anything is applied, so a bad file is reported by name and changes nothing, and files above 64 MiB
  are refused. The registry ids are `file.import_midi`, `file.import_midi_song`,
  `file.export_midi_clip`, `file.export_midi_track` and `file.export_midi_song`; none has a key.
  File > Export > MIDI Export Resolution (`file.midi_resolution` with `native`, `960` or `480`) sets the
  resolution of the files written from then on and is kept in `workflow.json`. The QA server has
  `session_midi` (MCP.md).

### 2.16 Track organisation

Folders of tracks, track colours and comments, folded automation lanes and the track height presets.
All of it is view state saved with the project; none of it reaches the engine, the routing or a
bounce, except the mute and solo of a folder, which decide which tracks play.

- **Model** ([organise.rs](../crates/model/src/organise.rs),
  [organise_cmd.rs](../crates/model/src/organise_cmd.rs)). `Song::org` (`Organisation`) holds the
  folders (`TrackGroup { id, name, parent, collapsed, color, muted, solo }`, at most 256, nested up to 8 deep),
  the innermost folder of each track, the track colours and comments, the automation lanes that are
  folded (`(device, param)`) and the take groups whose rows are open, all keyed by device like
  `track_heights`. A folder is not a track and has no device. **Invariant:** the tracks of a folder,
  with those of its sub-folders, are one block in `Song::tracks`, a sub-folder's block lies inside
  its parent's, and a folder has a track. The commands keep it; `Song::outline` (folder headers and
  tracks in display order with depth and a hidden flag) repairs a document that breaks it (a track
  outside its folder's block is shown outside it), and `Song::normalised_org` drops entries of
  tracks, lanes and take groups that are gone, empty folders and stray links: it is what is saved
  and what each command starts from. Removing a track leaves its entries behind, so undoing the
  removal brings them back.
- **Commands.** `GroupTracks` (the tracks must share their innermost folder; they are gathered at
  the place of the first one and the new folder goes inside that folder), `Ungroup` (the tracks and
  sub-folders go into the folder's parent, no track is deleted: this is also what Delete does to a
  folder), `RenameGroup`, `SetGroupCollapsed`, `SetGroupColor`, `MoveTrack` and `MoveGroup` (one
  place up or down: a track passes its neighbour, leaves a folder at its edge (one press, so it
  shows above the folder or below its last track) and enters an open folder it meets, or hops over a collapsed
  one, which would hide it; a folder moves with everything in it and leaves its parent at the edge;
  `Song::can_move_track` and `can_move_group` say whether it would), `MoveTracks` (several tracks
  to one place: `TrackPlace { to_index, into }` counts the places among the tracks that stay and
  names the folder they join; a place in the middle of another folder, a frozen track and a move
  that changes nothing are refused, and `Song::moves_tracks` and `accepts_copies_at` say whether
  a drop would be taken), `MoveGroupTo` (a folder with its tracks and sub-folders to a
  `TrackPlace`, into or out of another folder; it cannot go into itself or past the depth limit, and
  `Song::moves_group` says whether a drop would be taken), `SetGroupFlags`, `SetTrackColor`,
  `SetTrackComment`, `SetLaneFolded` and `SetTakesOpen`. Each is one undo step on one
  `Edit::Organise` that swaps the track order and the organisation, reporting `ViewChanged`
  (`SongChanged` when tracks changed place or what plays changed); `SetTrackColor`, `SetGroupCollapsed` and `SetLaneFolded` can be
  batched (Alt on a folder chevron collapses or expands every folder in one step).
  **Mute and solo on a folder** are the folder's own flags (`SetGroupFlags`): its tracks keep theirs.
  `Song::played_flags` gives every track's mute and solo as played, a track counting as muted or
  soloed when it or a folder above it is, and `effective_tracks` (so the engine, the bounce, the
  warp pass and the automation view) reads them, so a mute still beats a solo and the automation
  tracks are never stopped by a solo; a track's own M and S button shows dimmed while a folder holds
  it (`Song::muted_by_folder`, `soloed_by_folder`). Freeze renders a track whatever its folders
  say. **Arm on a folder** is still the flag on each track: the arrangement sends one
  `Command::Batch` of `SetTrackFlags` for every track of the folder (`Song::group_tracks`). Folders
  do not change the routing, and there is no group bus: routing the tracks of a folder to an output bus stays what
  the console already offers per channel. `WorkflowCommand::DuplicateTracks` copies several tracks
  (devices, channels, clips, not armed) in one step and keeps the copies together after the last
  original, or at a `TrackPlace` (the drop of a copy drag). A track colour is the batch of
  `SetTrackColor` and `SetColor`, so the track, its device in the rack and its console strip agree
  and one undo restores both. A track without a colour of its own shows the colour of the nearest
  folder above it that has one (`Song::shown_color`, `inherited_color`); that colour is the
  arrangement's (header accent, clips), the device and its strip keep theirs. Colour > Custom...
  in the header and folder menus (`sequencer.track_color` with the argument `custom`) emits
  `UiAction::PickColor`, and the shell opens `ColorPickerView`
  ([ui/src/color_picker.rs](../crates/ui/src/color_picker.rs)): a modal panel with a
  saturation/value square, a hue strip, a hex text (a `TextField`) and the six palette colours as
  quick picks; APPLY gives the colour to the folder or to the tracks and their devices in one undo
  step. Interface > Colour new tracks (`auto_color` in `workflow.json`, off by default;
  `AppCore::set_auto_color`) gives a track that a command made, and that has no colour of its own, the
  palette colour the fewest tracks use, in the undo step of that command.
- **File.** Project format 22: a song's `"groups"`, a track's `"group"`, `"color"` and `"comment"`,
  an automation lane's `"folded": true` and a take group's `"open": true`, each omitted when unused,
  so a project without organisation differs from its version 21 file only in the version line.
  Format 43 adds a folder's `"muted": true` and `"solo": true`, omitted when unset. The
  loader refuses an unknown or looping parent, more than 8 levels, an empty folder and a folder
  whose tracks are not one block, naming the JSON path.
- **Dragging** ([arrangement/track_move.rs](../crates/ui/src/arrangement/track_move.rs)). A press
  on a track header starts a drag (one of several selected tracks keeps the selection, so all of
  them go); past the drag threshold the pointer picks an insertion place from the rows (the upper
  half of a track's first row is before it, the rest after it, in its folder; the upper half of a
  folder row is before the folder, the lower half its start) and an accent line, indented by the
  folder depth, shows it. A place the model would refuse shows no line. The release sends one
  `MoveTracks` (or, with the copy role, one `DuplicateTracks` with the place); Escape cancels. A
  press on the name part of a folder row drags the folder the same way, with the same places and
  line, and the release sends one `MoveGroupTo`. A rubber band over the headers
  ([arrangement/header_band.rs](../crates/ui/src/arrangement/header_band.rs)) starts from a press
  on the empty part of the header column below the last track, or from Alt on any header: the
  tracks it touches are selected (Shift or Ctrl keeps the selection), Escape restores it. The
  bottom edge of a header resizes every selected track by the same factor (Alt gives them all one
  height) in one merge group, and Track Height > Minimise sets the shortest height
  (`MIN_TRACK_HEIGHT`) and, pressed again, brings back the heights the view remembers.
- **View** ([arrangement/organise.rs](../crates/ui/src/arrangement/organise.rs)). `rebuild` walks
  the outline: a folder row (`FOLDER_ROW_HEIGHT` 26) above its first track and no rows for the
  tracks of a collapsed folder, so `lane_rect`, `track_header_center` and the like answer `None` for
  them; a folded lane is `FOLDED_LANE_HEIGHT` (16) tall. The folder row has a chevron, the name
  (double-click types a new one), M, S and R buttons (lit when every track has the flag) and, over
  the timeline, its folder clips; right-click opens the folder menu. A folder clip
  ([folder_clips.rs](../crates/ui/src/arrangement/folder_clips.rs)) is a bar standing for the clips
  of the folder's tracks whose spans overlap, computed when the view lays out. Pressing one
  selects exactly those clips (Shift or Ctrl adds or toggles them), so mute, delete, duplicate,
  split, reverse and quantise act on all of them; dragging it moves them in time only (one
  `MoveClips`, no clip changes track or lane), and the razor, eraser and mute tools act on them. The content
  of a track header and of its lanes' headers is moved right by `TRACK_INDENT` (10 px, scaled with
  the UI) per level of folder, at most three (`track_indent`; the buttons at the right end keep
  their place and the input selector gets narrower, a one-line header moves M, S and R by at most 24
  px); the hit tests use the same `track_button_rect` and `lane_label_rect`, and the clips do not
  move. A header outlines the selected tracks (shift-click or ctrl-click adds one; a selected folder
  stands for its tracks), draws the folder guides of its ancestors at its left edge, takes its
  accent from the track colour (clips follow; clips have no colour of their own) and shows a note
  mark when it has a comment, with the first line as a tooltip. A folder name is typed in place (one
  line, Enter commits, Escape or a press elsewhere cancels or commits as for the marker names). A
  comment is typed in a box of five lines over the header, a shared multi-line `TextField` (Shift+Enter
  breaks the line, Enter or a press outside commits, Escape cancels; the caret,
  selection, clipboard and input method work as in the song information panel). `is_text_owner`
  covers both, so the shell resolves `Text` over `Sequencer`. In a track under 32 px (a clip
  rectangle under 26 px) a note clip has no header band: its notes are stacked bars over the whole
  height of the clip (the played pitch range, not at least an octave), its name is small text on
  them, and the mute button is left out (`clip_header` in `arrangement.rs`). The
  take rows still open at once on a click (the view keeps its own set), and the click also sends
  `SetTakesOpen`; the view takes the song's set again when the document changes under it (undo,
  redo, load).
- **Height presets.** `sequencer.track_height` with `small` (32 px), `medium` (the default, the
  entries are removed), `large` (120 px) or `fit` sets the base height of the selected tracks, or of
  every track when none is selected, as one `SetTrackHeights`; Fit shares what the view shows (after
  the rows that are not the targets' first lanes, at the current vertical zoom) among the targets,
  within 24 to 400 px.
- **Commands and menus.** `sequencer.group_tracks` (Cmd+Shift+T), `ungroup_tracks`, `toggle_folder`,
  `rename_folder`, `move_track_up` and `move_track_down` (Cmd+Shift+Alt+Up, Cmd+Shift+Alt+Down), `rename_track`,
  `track_comment`, `track_color` (the marker palette and `none`), `fold_lanes` (`fold` or `unfold`)
  and `track_height` are registry commands; all but `track_color` are in the Track menu (enabled
  from `ArrangementView::org_menu`), and the track header and folder row context menus carry them
  with Colour and Height submenus. The up and down keys of `sequencer.select_next_track` skip the
  tracks of a collapsed folder. Controller lanes of a note clip live in the piano roll, not in
  arrangement rows, and keep their own toggle there.

### 2.17 Audio editing

Fades and crossfades, clip gain and gain envelopes, reverse, normalise and strip silence on audio
clips. All of it is non-destructive: nothing is written to an asset, and every edit is a field of
the clip (`AudioClipContent`) or a split of the clip.

- **Model** ([song.rs](../crates/model/src/song.rs),
  [song/flatten.rs](../crates/model/src/song/flatten.rs),
  [audio_edit.rs](../crates/model/src/audio_edit.rs)). An audio clip has `gain_db`, a fade in and a
  fade out in frames with a `FadeCurve` (`Linear`, `EqualPower` = `sin(t pi / 2)`, `Exponential` =
  `t^2`, `SCurve` = `t^2 (3 - 2t)`; `fade_curve` shapes the fade-in and the fade-out too unless
  `fade_out_curve` is set, `out_curve()` is the one the fade-out follows, `with_fade_shape` changes
  one fade and leaves the other), `reverse`, a gain envelope (`GainPoint { frame, gain_db }`, at
  most 512, strictly increasing frames from the clip start) and `xfade: Option<FadeCurve>`. Fades,
  envelope and reverse are all in **clip time** (frames from the clip start; audio is never
  stretched), so reversing a clip leaves its fades and envelope where they are.
  `AudioClipContent::new` is a plain clip with an equal-power crossfade. `SetAudioClip` changes any
  of it in one undo step (the arrangement's drags share a merge id), `SplitClip` slices the envelope
  and hands the *upper* part of a reversed span to the left half so both halves sound as the whole
  did, and `StripSilence { id, pieces, fade }` turns a clip into the given frame ranges of itself in
  one undo step (the first piece keeps the id; piece edges are rounded to ticks and the source
  offset follows the tick, so the sound stays where it was to the frame). `SetAudioClip` and
  `StripSilence` can be batched. The pure parts are in `audio_edit.rs`: `strip_silence` (the
  detector) and `StripScan` (the same detector fed in blocks, with state that does not grow with
  the span), `peak`, `normalise_gain_db`, `slice_envelope`. The transport bar's **clip inspector**
  (`crates/ui/src/transport/inspector.rs`) edits the selected audio clips in place: the app hands it
  `ArrangementView::audio_inspector()` (the clips with their asset rate and length in frames, or the
  selected audio track) whenever the views sync, and it emits `SetAudioClip` commands, one per clip.
  A click, a wheel notch or typed text is one `Command::Batch`; a drag sends the commands under one
  merge id, which the history folds into one step across clips. Fades are shown in milliseconds,
  transposition in semitones with cents, and a reversed clip or the Repitch mode is left out of a
  transposition.
- **The crossfade rule** (`flatten_audio`, `crossfades`). A clip `B` with `xfade: Some(curve)` whose
  first audible span begins at its own start, and which starts inside an earlier audible clip `P`
  that ends inside `B` (`P.end <= B.end`), crossfades with it: over the overlap `[B.start, P.end)`,
  shortened to where a later clip covers `B`, `B` fades in with `curve` and `P` fades out with the
  mirrored curve, and both play. With equal power the two gains square-sum to 1; with linear they
  sum to 1. The fades and gains of both clips apply on top. A clip wholly inside an earlier one
  hides it with hard edges (as before), as does a clip with `xfade: None`. The rule is on ticks in
  `crossfades(lane)`, which the arrangement draws as a shaded zone, and `flatten_audio` turns each
  zone into an extra region (the tail of `P` over the overlap, with `x_out`) and an `x_in` on the
  first region of `B`. Regions therefore may overlap, never more than two at a time and never one
  inside another's end (`compile_audio` checks both).
- **Engine** ([audio.rs](../crates/engine/src/audio.rs),
  [stream.rs](../crates/engine/src/stream.rs)). A region may play backwards (`reverse`: frame `i`
  reads `source_offset - i`, silent until it reaches the asset), carry an envelope (per-track
  `(frame, linear gain)` lists, `env` indexes them; ramped linearly in amplitude in `f64`, held
  outside the points) and a crossfade (`x_in` frames of `curve(i / x_in)` or `x_out` frames of
  `curve((x_out - i) / x_out)`). The per-frame gain is `gain * fade(i)` in `f32`, then times the
  envelope and the crossfade *only when the region has them*, so a region without them is
  bit-identical to what the player always produced. The second region over a frame adds to the first
  and the first assigns, so a lone region is unchanged to the bit. The streamer applies the same
  gains while it reads (reversed regions are read backwards, overlapping ones mixed), so the ring
  carries finished output and the audio thread copies it before adding the memory regions; a
  streamed asset still plays bit for bit like the same asset in memory. Allocation-free: gains are
  computed into stack blocks.
- **File.** Project format 23: an audio clip's `"reverse"` and `"envelope"` (omitted when unused),
  `"xfade"` (`"off"` or a curve; omitted when equal power) and the two new `"fade_curve"` names.
  Format 43 added `"fade_out_curve"`, written only when the fade-out is shaped differently from
  the fade-in (an ignorable field: a reader that lacks it plays both fades with `"fade_curve"`). A
  file older than 23 loads with every clip at `"off"` so that its overlaps stay hard cuts and its
  bounce keeps its bytes; saved by this version it says so. A version 23 file without `"xfade"` is
  equal power.
- **Warp on import** ([import_warp.rs](../crates/app/src/import_warp.rs),
  [audio-file/src/meta.rs](../crates/audio-file/src/meta.rs); the choice is in
  [docs/DECISIONS.md](DECISIONS.md#m-warp-on-import-default)). The import worker reads the tempo
  metadata of the file (`read_file_meta`, a bounded RIFF chunk walk: at most 256 chunk headers and
  512 bytes of a chunk, `acid` tempo, beats and one-shot flag, `smpl` loop count, a tempo in the
  `bext` description; in an AIFF or AIFF-C file the `basc` beat count, a tempo written in an `ANNO`,
  `NAME` or `COMT` text, and an `INST` loop flag) and sends it with the loaded asset (`Loaded::meta`). `AppCore::place` asks the
  pure function `import_warp(meta, frames, rate, song_bpm, bar_ticks, mode)`: a file that is not
  flagged a one-shot warps at the tempo it states (the exact tempo of its beat count when that is
  within 2 percent of the stated one), else at the tempo that makes its length 1, 2, 4 or 8 bars
  when that tempo is within 0.1 BPM of a whole number between 70 and 180 (the one nearest the song
  tempo when several fit); All files warps the rest at the song tempo. The clip is added with the
  warp length (the bars it holds, or the beats of the file at its tempo) and `SetAudioClip` turns
  warp on, folded into the undo step of the add (`join_next`). The mode is `AppCore::warp_imports` /
  `WorkflowPrefs::warp_imports` (`"off"`, `"loops"` default, `"all"`); with Off the import is
  exactly the unwarped one. The Warp panel adds **Scale** (the source tempo divided by the percent
  when APPLY runs) and **RESET** (`AppCore::reset_warp`: warp off, markers cleared, a warped clip
  resized to the same stretch of audio at its own speed).
- **View** ([arrangement/audio_edit.rs](../crates/ui/src/arrangement/audio_edit.rs)). The gain
  envelope is drawn over the waveform on the clip gain scale (-60 to +12 dB); the pencil adds a
  point and drags it at once, a point drags with any tool but the eraser, and a right click or the
  eraser removes one. A point being placed or dragged snaps to the grid (the disable-snap role turns
  it off); Shift-click or Ctrl-click (Cmd on macOS) adds a point to the selection or takes it out
  (`ArrangementView::selected_envelope_points`), dragging a selected point moves all of them by the
  same frames and dB (`audio::envelope_move_many`: the group stops at the points it does not hold
  and at the clip's ends) and `edit.delete` removes them together. A right click on a fade handle
  opens the shape menu of that fade (`open_fade_menu`: one `SetAudioClip` with `with_fade_shape`);
  the Fade Shape commands still shape both. A reversed clip draws its waveform right to left with
  an `R` in its header;
  trimming its left edge keeps the source offset, its right edge moves it (the end of a reversed
  clip is the low end of its span), and a left trim of any clip moves the envelope with the audio
  (`audio::envelope_after_trim`). The fade handles, the gain handle and their drags were there
  before; the new fade shapes are drawn with `FadeCurve::eval`. A crossfade zone is shaded at the
  start of the later clip with both curves; where its length can change, a grip marks the line
  where it ends (`ClipPart::CrossfadeLength`, see `Drag::Crossfade` below).
- **Commands.** `sequencer.audio_reverse`, `audio_normalise`, `audio_strip_silence`, `audio_gain`
  (`up`, `down`, `reset`; 1 dB), `audio_fade_shape` and `audio_crossfade` (a curve name, `off` too
  for the crossfade), `audio_fades` (`declick`: 5 ms in and out, or `clear`) and
  `audio_envelope_clear`: Edit holds Reverse Audio, Normalise Audio and Strip Silence..., the Audio
  menu the rest, the clip context menu carries the first three and the two shape submenus. The
  arrangement runs the ones that need no samples as one `SetAudioClip` per selected audio clip (one
  `Command::Batch`, one undo step, for several). Normalise and Strip Silence live in the app
  ([app_core_audio_edit.rs](../crates/app/src/app_core_audio_edit.rs)) because they read the
  samples: Normalise sets each selected clip's gain from the peak of the span of the asset it plays
  (before gain, fades and envelope) to 0 dBFS; Strip Silence runs the detector over the span a block
  at a time (`StripScan`: a long clip is never copied into memory, and a test holds it to
  `strip_silence` at any block size; a frame is silent below the threshold in every channel;
  silent runs of at least the minimum length go, with a silent start and end; each piece grows by
  the pads and pieces that touch join), maps the pieces to clip time (a reversed clip swaps the
  pads and the order) and sends one
  `StripSilence` per clip in one batch, then selects the pieces. The panel is
  [silence_panel.rs](../crates/ui/src/silence_panel.rs) (`SilenceView`, `UiAction::StripSilence`), a
  sibling of the quantize panel with its settings kept between openings.

### 2.18 Time-stretch, pitch-shift and warp

Audio clips can follow the song tempo and be transposed without changing length. The model keeps
audio in ticks and frames as before; a warped clip *plays a rendered buffer*, so the audio thread
learns nothing new.

- **Engines** ([dsp/src/stretch](../crates/dsp/src/stretch)).
  `stretch(source, map, out_frames, params, cancel)` renders output frame `t` from the source at
  `map.src_at(t)` (`TimeMap`: a piecewise-linear, non-decreasing map of knots, extended with the
  slope of its end segments). `Repitch` reads with Hermite interpolation, so speed and pitch move
  together (and the same resampler turns a stretch into a pitch shift). `Beats` cuts the source at
  the onsets of `detect_onsets`, keeps each onset on the map (the output frame is the map's inverse
  of the onset's source frame), copies the first 30 ms of every piece verbatim with a 2 ms crossfade
  that starts just before the onset, and fills the rest of the piece with WSOLA (30 ms Hann frames
  at half-frame hop, a similarity search over about 8 ms around the nominal source position, coarse
  at a stride of 4 then refined; the source past the next onset reads as silence so a hit never
  leaks backwards). `Tonal` is a phase vocoder (2048-point frames at 44.1 and 48 kHz, hop a quarter,
  Hann twice) with identity phase locking: each analysis frame is paired with the frame a hop
  earlier to get the instantaneous frequency of each spectral peak, peaks advance their synthesis
  phase by a hop of it, and the bins around a peak take its phase rotation. Pitch shift by `r` is a
  stretch of a signal `r` times as long followed by a read `r` times as fast, so length stays the
  map's; the ratio is clamped to two octaves either way (`pitch_ratio_for_cents`). Everything is
  `f64` with the deterministic trigonometry of this crate (an own radix-2 FFT and a polynomial
  arctangent), allocates only inside the call, and polls a cancel callback.
- **Model** ([warp.rs](../crates/model/src/warp.rs)). `AudioClipContent::warp` is a `WarpSettings`:
  `enabled`, a `WarpMode` (`Beats`, `Tonal`, `Repitch`), the source tempo in BPM (20 to 999), a
  transposition in cents (+-2400; Repitch ignores it) and `markers` (`WarpMarker { tick, source }`,
  ticks from the clip start, source frames from `source_offset`, strictly increasing in both, at
  most 512). `active()` is `enabled` or a transposition that sounds; with warp off, a transposed
  clip reads the asset in real time. The **time map** counts source frames in beats: the frame
  played at `dt` ticks after the start is `source_offset + dt * 60 sr / (bpm PPQ)`, bent through the
  markers (straight between two, at the source tempo outside), so a tempo step or ramp changes only
  where the beats fall in frames. `warp_job` turns a clip into a `WarpJob` (asset, mode, cents, the
  map as `(frame from the clip start, asset frame)` knots, the buffer length
  `frame_at(end) - frame_at(start)`, the rate): knots sit at the ends, the markers and the tempo
  events, and a ramp is sampled every 4096 frames. `WarpJob::key` is an FNV-1a hash of everything
  the buffer depends on. `flatten_audio` puts the job on each region of a warped clip
  (`AudioRegion::warp`) with the real-time reading as the region's own source fields, and
  `warp_jobs` lists the jobs of the audible clips. `SplitClip`, `StripSilence` and comp pieces take
  their source offset from the map (`AudioClipContent::piece_from`) and re-base the markers
  (`WarpSettings::markers_after`, with a marker at 0 so the stretch in front of the first one is
  kept). A reversed clip can be warped and transposed: `WarpJob::reverse` is part of the key, the
  knots are the forward ones from the clip's source offset, and the renderer turns the finished
  buffer around, so the engine plays it forward like any other (the reversed reading of the asset
  stays the real-time fallback, without an interim time map). Trimming the start of such a clip
  cuts the end of the stretched span and leaves the source offset and the markers alone.
  `bpm_for_beats` (a loop of N beats), `estimate_bpm` (a comb filter over onsets between 70 and
  180 BPM) and `quantize_audio` (markers that move each transient to the nearest grid line by a
  strength, the clip's start and end pinned, any markers replaced, a clip without warp switched on
  at the song tempo at its start; a reversed clip is quantized where it plays, mirrored in frames)
  are pure.
- **Render** ([render/src/warp.rs](../crates/render/src/warp.rs)). `render_warp(job, asset, cancel)`
  reads only the span of the asset the map uses (plus 8192 frames each side; a streamed asset
  through its source) and returns a `SongAsset::Memory` of the clip's whole span.
  `song_input_with_warps` gives the engine that buffer, read from the clip's own frame (`clip_pos`),
  instead of the asset when the `WarpStore` has it; without warped clips it is exactly
  `song_input_with_assets`. A bounce calls `render_warps` before it builds the song, so it never
  plays dry and its bits do not depend on timing. The buffer is an ordinary memory asset, so
  fades, envelopes and crossfades apply to it like to any clip.
- **Interim warp** ([engine/src/audio.rs](../crates/engine/src/audio.rs)). `song_input_live` (the
  app's builder; a bounce never calls it) gives a warped clip of a memory asset whose buffer is not
  ready (and that does not play backwards) a region with `SongAudioRegion::warp`, an index into `SongAudioTrack::warp_maps` (the job's
  `(clip frame, asset frame)` knots, one map per job). The engine reads the asset through the
  piecewise-linear map with four-point Hermite interpolation, on stack blocks, so a tempo ramp,
  markers and trims are right from the next batch; it is a resampling (the pitch follows the ratio,
  a transposition is not heard) and a streamed asset keeps playing unwarped. When a song swap
  replaces a track that played an interim warp, `start_handoffs` keeps the old lane's data in the
  new track's scratch (an `Arc` clone, dropped with the snapshot, never on the audio thread) and
  `Handoff` mixes the two over 5 ms, so the finished buffer arrives without a click; a track that
  reads streamed assets is left out. The clips still rendering are listed by `calculating_clips`
  and drawn with a CALC chip. A cheaper stretcher than resampling (WSOLA) could replace the Hermite
  read behind the same region field.
- **App** ([app_core_warp.rs](../crates/app/src/app_core_warp.rs)). The cache lives in `AudioState`.
  Each `submit_song` calls `sync_warps`: it lists the jobs the document needs, drops buffers nobody
  needs and cancels their workers, queues the missing jobs whose asset is loaded and starts at most
  two `atomik-warp` threads (`spawn_warp`, a `Msg::Warped` per result, counted like the other
  workers so `wait_background` waits for them). A finished buffer that is still wanted is kept and
  the song is submitted again, so it plays from the next batch; the cache is bounded to 512 MiB and
  a job that does not fit is skipped and reported once. Opening a project empties it. Commands: Warp
  Audio, Warp Mode, Transpose (the arrangement, one `SetAudioClip` per clip), Warp to Song Tempo,
  Warp Loop as N Bars, Quantize Audio and the Warp Settings panel (`WarpView`, a sibling of the
  strip silence panel: warp, mode, source tempo with DETECT TEMPO, /2 and x2, semitones and cents).
  The detectors read the clip's source span (at most 5 minutes) as a mono mix; Normalise reads the
  span through the map, and Strip Silence refuses a warped or transposed clip.
- **View** ([arrangement/audio_edit.rs](../crates/ui/src/arrangement/audio_edit.rs),
  [audio.rs](../crates/ui/src/audio.rs)). The waveform of a warped clip is drawn through
  `audio::warped_source` and its inverse, the clip header shows `W` and the transposition, markers
  are thin lines, and the left trim moves the offset through the map (`audio::trim_left_warped`).
- **Audio clip editor** ([audio_editor.rs](../crates/ui/src/audio_editor.rs),
  [slice_edit.rs](../crates/model/src/slice_edit.rs),
  [app_core_audio_editor.rs](../crates/app/src/app_core_audio_editor.rs)). A double-click on an
  audio clip, Enter on a selected one or F8 opens `Pane::AudioEditor` in place of the arrangement
  (`UiAction::OpenAudioEditor`; Up and Down open the previous or next audio clip, Esc closes).
  **Overview** shows the clip in clip time (the waveform is drawn per span between markers, so a
  stretched span is wider) and **Slice** edits the warp markers; the mode is saved in
  `ui-state.json` (`audio_editor_mode`). `slice_edit` is the pure marker maths: it works on the
  *framed* list (the clip's markers plus one at tick 0 and one at the clip end, so both ends stay
  pinned) and offers add at a tick or source frame (pinned where the audio already plays, so nothing
  changes), stretch, slide (marker and frame move together along the current time map), group
  (accordion over the markers after the dragged one), nudge, delete and revert; every result is
  strictly increasing in tick and frame and is checked by property tests.
  `SongCommand::SetWarpMarkers` (merge ids make a drag one undo step) replaces the markers; entering
  Slice on a clip without warp, or adding its first marker, switches warp on at the song tempo at
  the clip start (`warped_at_song_tempo`, as Quantize Audio does, in the same step). Split at
  Markers is one `Batch` of `SplitClip`; Clip to Loop Slicer adds a Loop Slicer and its slices (cut
  at the markers' source frames, in the clip's span of the file) as one step. `Command::RenameAsset`
  (an `Edit::AssetName`) renames the pool asset from the editor's title. The time map multiplies
  before it divides, so markers on the natural line bounce bit for bit like a warped clip without
  markers (`render/tests/warp.rs`; the repitch engine needs the markers on whole output frames,
  which a beat position is). **Transients and the speaker.** Entering Slice, opening a clip in it
  and the SENS buttons emit `UiAction::DetectTransients`; `AppCore::detect_transients` reads the
  span of the file the clip plays (mono mix, at most 300 s, through `for_each_chunk` so a streamed
  asset is read in chunks) on a worker (`atomik-clip-onsets`, `Msg::Onsets`,
  `audio_import::spawn_clip_onsets`) and runs `atomik_dsp::detect_onsets`; results are cached in
  memory by (asset, sensitivity) with the span they cover, only the newest request is shown, and
  `AudioEditorView::set_transients` draws them. A double-click near one calls
  `slice_edit::add_at_source` (the marker lands on the detected frame), Add All
  `slice_edit::add_at_sources` (one `SetWarpMarkers`, one undo step). The speaker (`Tool::Speaker`,
  `T`; a toggle in the editor) emits `UiAction::AuditionSpan` with the span `model::audition_span`
  finds (the frames the clip plays from the pressed tick to its end in the file, from the top down
  for a reversed clip) and `UiAction::StopAudition` on release or Esc; `app_core_audition.rs` feeds
  it to the preview voice of the file browser (`preview_channel`, source rate of the asset, clip
  gain) from an `atomik-audition` thread that reads the engine's copy of the asset and waits when
  the ring is full, so the audio thread allocates nothing and offline renders never receive it.
- **File.** Project format 24: an optional
  `"warp": {"mode", "bpm", "enabled"?, "transpose"?, "markers"?}` in the clip's `"audio"` object,
  omitted for the default. A version 23 file loads with no warp and refuses the field.
- **Determinism.** The stretch maths uses only IEEE `+ - * /`, `sqrt` and the deterministic
  trigonometry, so a bounce of a warped project is bit-identical across block sizes, worker counts
  and runs (`crates/render/tests/warp.rs`), and a project that does not warp is untouched to the bit
  (the version 22 fixture of audio editing keeps its SHA-256).

### 2.19 Freeze and bounce in place

A track can be **frozen**: its instrument and the first effects of its channel's insert chain are
rendered to audio and the render plays instead, so none of those devices run. **Bounce in place** is
the additive twin: the selected clips are rendered through the same chain into a new audio track,
and nothing leaves the engine. The new track is an ordinary audio track (`AddAudioTrack`'s wiring)
placed right after its source, in the source's folder and colour, in the same undo step. Both are
the project bounce (2.7) with one tap on the rack; neither changes the engine or the audio thread.

- **The chain** ([model/src/freeze.rs](../crates/model/src/freeze.rs)). A track's chain is its
  instrument (or an audio track's player) and the effects of its console channel's insert loop
  ([`insert_chain`], at most sixteen, a container effect is one of them). A freeze goes *through* the
  first `n` of them (`0..=len`): the render is the signal at the output of the last of them, and the
  effects after it stay live on the frozen audio. Unless the strip is baked in, the channel strip
  (fader, pan, width, sends, mute, solo, output) is not frozen, nor are send effects; the strip's
  input section (gain, filters, dynamics and EQ when they come before the insert point) is part of
  the signal at the tap, so it is frozen with a chain of one or more effects. A track needs a console
  channel it is cabled to. A container instrument or effect freezes with everything inside it: every
  flat device whose path starts with the container's key (`within`) is flagged, and the tap of a
  container effect is the inner output that feeds the container's output (`ExpandedRack::exits`,
  traced by the expansion itself, so a bypassed container taps what feeds it). **Baking the strip**
  (`Frozen::strip`, `frozen_tap`) takes the tap at the channel's `out_l`/`out_r`, includes the whole
  insert chain, flags the channel like the instrument and re-sources what the channel fed to the
  player; the channel's sends and parallel output cables are dropped, so `strip_blocker` refuses a
  strip with a send on into a return that holds an effect, or one that feeds a parallel channel.
- **The frozen state.** `Track::frozen` is a `Frozen { start, length, audio, effects, strip }`: the
  render as a plain audio clip (an asset of the pool, no reverse, no warp, placed in ticks), the
  frozen effects and whether the strip is baked in (the saved `strip` field is ignorable, so older
  files load and format 44 is unchanged). The track's clips, instrument and effects stay in the
  document untouched, so unfreezing restores it exactly. `SongCommand::FreezeTrack`, `RefreezeTrack` (the render of a track that is
  frozen already replaced by a new one through the same checks, `plan_refreeze`; the effects must
  still be the first of the insert chain), `UnfreezeTrack { keep_audio }` (which can also
  leave the render as a clip on a new audio track, in the same step) and `AddRenderedTrack` (bounce
  in place: a new audio track with one clip, the originals muted when asked) are single undo steps
  with their own `Edit` (`Edit::Frozen`); the asset counts as referenced while the track is frozen
  and while the undo history holds it, so it goes at the first save after that. A relink swaps it.
  Commands that would change a frozen track (its clips, notes and automation), a device it freezes
  (parameters, cables, removal; a device inside a frozen container counts, and so does the channel
  of a baked strip), the console channel it plays through (removal), the frozen part of
  an insert chain or the tempo and meter maps are refused with a message that says to unfreeze
  (`freeze::guard`, called by `Document::plan`, so every path is covered; undo and redo restore
  consistent states and never come through it). The tempo is locked because the render is tied to
  the tempo map. Removing the frozen instrument removes the track with its freeze, and undo brings
  both back.
- **Applying it to the engine** (`RackDoc::expand` calls `freeze::apply`). The devices of a frozen
  track keep their place in the flat list, so every other device keeps its engine id and its prepare
  seed (freezing a track does not change the sound of another one), but they are flagged
  `ExpandedDevice::frozen`, every cable that touches them is dropped, and the cables that left the
  last frozen device's tap are re-sourced to a synthetic `atomik.audio-track` device keyed
  `<track>/~freeze` (monitor off), appended at the end of the list. The song feeds that player the
  render as the playback of an audio track (`Track::frozen_playback`, flattened like any audio
  track, so mute and solo apply). Frozen devices are not built (`BuildOptions::omit`: added to the
  graph and removed again before the compile, which keeps their ids taken), not inserted into the
  live engine, never processed, and a frozen plugin gets no instance; their notes, automation and
  warp jobs are skipped.
- **The render** ([render/src/freeze.rs](../crates/render/src/freeze.rs)). `render_freeze` runs
  `render_core`, the body of `render_doc_with`, with an `Extra`: the tap (`freeze_tap`: the last
  effect's `out_l` and `out_r`, or the sockets of the instrument that feed the channel) is cabled to
  two more interface outputs after the document's, and those two channels are the result. The render
  starts at tick 0 like a bounce, so the chain is in the state it has when the song plays, then the
  span `[start, end)` of the track's clips (or of the chosen clips) is sliced out. The track is
  rendered as if audible (unmuted, soloed when another track is) and everything else of the project
  plays, so a chain keyed by another track freezes with that source playing. With a tail, the render
  goes on for up to 10 s past the end (60 s at most) and is cut where the signal stays below -90
  dBFS (`TAIL_FLOOR`), so a dry track gets none; without one it is exactly the span. A chain that
  reports latency is compensated: the tap sits in front of the master's delay compensation, so the
  render is cut by the latency of the chain (`path_latency`: the longest path of the latencies the
  engine published for the devices of the chain, read at the end of the render after the plugins'
  pre-roll, over the flat cables between them, with the channel split into its input stage and the
  rest), which keeps the track aligned with the others. For bounce in place
  the other clips of the track are removed from the rendered copy of the document. `RenderControl`
  (a cancel flag read once per 64-frame batch, a progress callback) can stop a render, which then
  returns `None`; neither changes a sample.
- **App** ([app_core_freeze.rs](../crates/app/src/app_core_freeze.rs),
  [freeze_worker.rs](../crates/app/src/freeze_worker.rs)). The commands in the Track menu
  (`sequencer.freeze_track`, `freeze_track_to_device`, `freeze_tail`, `freeze_strip`, `freeze_cancel`,
  `unfreeze_track`, `unfreeze_track_keep`, `refreeze_track`, `bounce_in_place`,
  `bounce_in_place_mute`, `bounce_range`; no default keys), in the track header menu and (bounce)
  in the clip menu queue jobs; one `atomik-freeze`
  worker at a time renders a copy of the document into the project's bundle (the scratch bundle
  before the first save) and sends `Msg::Frozen`, counted like the other workers so
  `wait_background` waits for it; the status line shows `Freezing Lead 37%` and Cancel Freeze stops
  it. When the result arrives it is thrown away if the track, its chain, the tempo or the meter
  changed meanwhile (`print_of` compares them), else the asset is registered and one step applies.
  **Bounce Range in Place** renders a time range instead of whole clips: the range selected on a
  lane (that track alone) or the locators (`song.loop_range`) on each selected track that plays
  a note or audio clip in it, through the whole chain, with `clips: None` and `span` set to the range,
  so a clip that crosses an edge contributes the part inside; the new track's clip starts at the
  range. **Re-render Frozen Track** queues a `Refreeze` job through the effects the track froze; the
  worker renders a copy of the document with that track unfrozen (its devices run for the
  render), the track keeps playing the old render, and `RefreezeTrack` swaps the new one in as one
  undo step (the render is thrown away if the track or its chain changed meanwhile, as for a
  freeze).
  `SyncOp::FrozenChanged` makes the app restart its engine: `rebuild_engine` skips the frozen
  devices but spends their ids, so freezing or unfreezing stops playback. With plugins, the worker
  makes its own `PluginHost` from the app's scan cache and settings with the sandbox of the app in
  bounce mode (an instance may only live on the thread that made the host); without the sandbox the
  app's plugins live in its process and the freeze is refused with a message that says so.
- **View** ([arrangement.rs](../crates/ui/src/arrangement.rs), `seq_paint::snowflake`). A frozen
  track has a snowflake after its name and draws its render in ice blue in place of its clips,
  labelled `(frozen)` and without handles (the clips behind it cannot be grabbed); the render is a
  clip with id 0 that no lane has.
- **File.** Project format 25: an optional
  `"frozen": {"start", "length", "effects"?, "audio": {...}}` in the track, after its `"comment"`;
  version 24 files load unchanged and refuse the field, and a project without a frozen track differs
  from its version 24 file in the version line alone. On load the frozen effects must be the first
  of the insert chain of the channel the track plays through.
- **Determinism and limits.** The render is deterministic and independent of the block size and the
  workers (`render/tests/freeze.rs`), and a project bounced with a track frozen equals the unfrozen
  bounce bit for bit when the chain has no latency (the tail is cut at -90 dBFS, so a chain with a
  long tail differs by less than that); a chain with latency gives the unfrozen bounce earlier by
  that latency. Not frozen: sends, the channel strip's fader, pan and output, effects after the
  freeze point, container effects. The frozen audio does not follow tempo changes (they are refused
  while a track is frozen). The render is stereo at the project rate; a mono tap plays on both
  sides. Bounce in place renders the chosen clips as they play, or the range of Bounce Range in
  Place. The channel strip and the sends are never part of a render, so they stay live on a frozen
  track and on a bounced one; baking them into a render is not offered.

### 2.20 Audio to notes

**Audio > Audio to Notes...** turns the audio of a clip into a note clip: a **pitched** voice or
instrument (one note at a time) or a **drum** loop. It is an offline analysis in the `dsp` crate,
written in house (no new dependency), and the result goes in as an ordinary note clip, so the
engine, the audio thread and the file format are untouched.

- **Pitched** ([dsp/src/notes/pitched.rs](../crates/dsp/src/notes/pitched.rs)). The mono mix is
  averaged down (a box filter, at least six samples per period of the highest key of the range) and
  a **YIN** tracker (de Cheveigne and Kawahara) runs every 5 ms on a window of 1.5 periods of the
  lowest key plus one: the squared difference function, its cumulative mean normalisation, the first
  dip below a threshold (`0.10 + 0.30 * sensitivity`) walked down to its minimum and a parabolic
  interpolation of the period. Frames below the loudest by `30 + 30 * sensitivity` dB (never below
  -70 dBFS) are unvoiced and skipped, so silence costs nothing. The pitch (fractional semitones,
  `log2` of the deterministic maths) goes through a median over five frames, and the segmentation
  keeps a note while the pitch stays within the **pitch drift** of its running median: vibrato and a
  glide inside it do not split it, and a deviation becomes a new note only when it holds still for
  half the minimum note length (at least 30 ms), so a blip does not. Unvoiced gaps up to 25 ms are
  bridged, and an amplitude onset ([`detect_onsets`]) at least 30 ms into a note splits it (a
  re-attack of the same pitch). The key is the median pitch rounded to a semitone inside the chosen
  range; a note after silence starts at the nearest onset, one after a pitch change where the pitch
  moved, and a note ends where the voicing ends or its level has decayed `24 + 12 * sensitivity` dB
  below its peak. The velocity follows the peak level in dB (24 dB range, floor 0.2) relative to the
  loudest note. Notes of one key with at most 40 ms between them and no onset at the second are
  joined; notes under the minimum length are dropped. With **Keep pitch bend** (`PitchParams::bend`)
  each note also carries its **contour**: the smoothed pitch of its voiced frames, leaving out the
  half window next to each end of the note (the silence beside a note pulls the pitch there) and
  frames more than two semitones from the key, as semitone offsets from the key, thinned with
  Ramer-Douglas-Peucker to 0.05 semitone; a note that never leaves 0.1 semitone has none.
- **Drums** ([dsp/src/notes/drums.rs](../crates/dsp/src/notes/drums.rs)). The onsets of the existing
  detector, then for each hit a 30 ms FFT of the sound after it (a falling half-Hann window, which
  keeps the attack; cut at the next hit): the energy shares of a low (20 to 250 Hz), a mid and a
  high band (above 6 kHz) and the zero-crossing rate of the first 6 ms. A hit with a low share and
  most of what is above 250 Hz up in the high band is a kick and a hat struck together (two notes),
  a high band of half the energy (or 35% with a high crossing rate) is a hat, a low band of half is
  a kick, anything else is a snare. A hat is open when the high end alone (a second difference, so a
  kick ringing under it does not count) stays above a tenth of its peak for 120 ms. The full kit
  (`DrumKit::Full`) also tells a **tom** from a kick, by the centre of the low band 25 to 75 ms after
  the hit (a kick has swept down to its fundamental by then, a tom holds its pitch; above 95 Hz it is
  a tom, low, mid or high at 135 and 185 Hz), and a hat that rings for 600 ms is a **crash**. The
  classes are written on the keys of a `DrumMap`: General MIDI (36, 38, 42, 46, and 45, 47, 50 and 49
  for the toms and crash) or the **pads** map, the classes in a row from key 36, the first pad of the
  drum machine (kick, snare, closed hat, open hat, low, mid and high tom, crash). Each has a fixed
  length (120 ms, 60 ms for a closed hat, 200 ms for a tom, 300 ms for an open hat, 800 ms for a
  crash, cut where the next note of the class starts), and the velocity follows the peak of the first
  10 ms in dB.
- **Long takes** ([dsp/src/notes/long.rs](../crates/dsp/src/notes/long.rs)). `extract_in_segments`
  runs either detector over a take of any length a minute at a time: it asks the caller for the
  mono audio of each minute with 3 s on each side, keeps the notes that start inside the minute (so
  every note belongs to one minute), tells the caller how far it has got after each minute and
  stops when the caller says so. A take of at most 66 s is one pass, identical to the detector alone;
  beyond that the minutes are separate analyses (a quiet passage is judged against the loudest sound
  of its own minute, and a note that runs more than 3 s past the end of a minute is cut there), and the
  velocities are worked out again at the end against the loudest note of the whole take. Memory is
  one minute of audio whatever the length of the clip.
- **Time** ([model/src/audio_notes.rs](../crates/model/src/audio_notes.rs)). `notes_clip` maps each
  frame of the analysed audio to the tick where the clip plays it with `tick_at_source`: through the
  tempo map for an unwarped clip (a tempo change or ramp under the clip moves the notes with the
  audio), through the warp time map, markers included, for a warped one. The clip is analysed in the
  order it plays (a reversed clip is read from its end, backwards), so every clip maps the same way;
  the note clip starts and ends with the audio clip and has no offset. **Beats timing** rounds the
  start of each note to a grid (1/4 to 1/32) keeping its length, and the notes of one key never
  overlap afterwards. The transposition of a transposed clip is added to the keys. A note found
  with a contour puts one **pitch-bend lane** in the clip (`Clip::controllers`, the existing
  controller data), scaled for the default bend range of 2 semitones (`BEND_RANGE_SEMITONES`): each
  note's contour starts at the note's first tick and its last point holds until the next note's
  contour starts, so notes never glide into each other; a note without a contour recentres the bend
  when the note before it left it, and the last note recentres it after it ends. A snapped note takes
  its contour along. There is no new command and no format change: the clip goes in with `AddClip`
  or, for a new track, `ImportMidi` (the poly synth, named after the audio track), one undo step
  either way.
- **App** ([app_core_notes.rs](../crates/app/src/app_core_notes.rs), `audio_import::spawn_notes`).
  EXTRACT takes **every selected audio clip**, one after the other: it reads the span a clip plays
  (from the clip's offset, for a warped clip the span the time map reads; a streamed asset through
  the stream source in chunks, like strip silence), all of it, a minute at a time, mixed to mono,
  and runs the analysis on an `atomik-notes` worker that sends `Msg::Noted` (counted like the other
  workers, so `wait_background` waits for it, a panic still sends a failure, and the next clip of
  the request starts when the previous result has been applied). The song keeps playing. The worker
  publishes how many frames it has finished in an atomic (`Shared`), which `AppCore::notes_progress`
  turns into the text of the status line and the bar of the panel, and looks at a stop flag after
  each minute (`AppCore::cancel_notes`): a stopped clip adds nothing, the clips queued behind it are
  dropped, the clips already finished keep their notes. A clip that cannot be read (Repitch mode, a
  missing file) is skipped and counted in the final message. Each result is dropped with a message
  when its audio clip changed or the project was closed meanwhile; a clip in Repitch mode is refused
  (its pitch follows the tempo, which the analysis cannot see). The notes of each clip go on a
  **new track** or on the note track selected in the rack (`AddClip`, lane 0); the last new clip is
  selected.
- **View** ([notes_panel.rs](../crates/ui/src/notes_panel.rs)). A modal panel like Strip Silence:
  mode, sensitivity, minimum note, lowest and highest note, pitch drift, keep pitch bend, open hats,
  drum kit, drum keys, Beats timing and target. The rows the mode does not use read `n/a`. EXTRACT
  closes the panel when the work starts; the same command opened while the work runs shows a bar
  with the progress and a CANCEL button in place of EXTRACT. The registry command
  `sequencer.audio_to_notes` has no default key.
- **Determinism and limits.** Same samples and settings, same notes on every platform (only IEEE
  `+ - * /`, `sqrt`, sorting and the deterministic `log2`, `exp2` and cosine are on the decision
  path), and silence, non-finite samples and noise give no notes and no `NaN`. It is monophonic:
  chords and several voices give the loudest line at best. A re-attack of one pitch needs a dip in
  level of about 20 ms to be heard; a hit is one drum (a kick with a hat is the only combination),
  so a snare with a hat is a snare. The drum notes land on the keys of the chosen map whatever
  instrument the track has: point the track at a drum machine with samples on those pads to hear
  them (the new track has the poly synth); the drum machine is not filled from the audio. The
  bend lane assumes a bend range of 2 semitones. The analysis of 3 minutes of dense audio takes a
  few seconds on a worker, and the memory it holds is one minute of audio.

### 2.21 Note lanes

A track holds one or more note lanes that feed its device together: lane `i` for `i < Track::note_lane_count()` is `lanes[i]`, the note lanes come first, then
the automation lanes; an audio track keeps its single audio lane. A lane has an optional `name` and
a `muted` flag, a `solo` flag and an optional `color` (fields of the lane, not part of `LaneKind`,
whose equality decides which lane a move or paste matches). `Track::lane_label` is what the headers
show: the name, else "Notes", "Notes 2", ...

- **Commands.** `AddNoteLane` (inserted after the last note lane, up to `MAX_NOTE_LANES` = 64),
  `RemoveNoteLane` (never the last one), `RenameLane` (blank resets, `MAX_LANE_NAME_CHARS` = 32) and
  `SetLaneMuted`, `SetLaneSolo` and `SetLaneColor`: each one undo step, refused on frozen and audio
  tracks. While any note lane of a track is soloed only the soloed ones play (`Track::
  played_note_lanes`, which every flatten function, the merge and the render use, so the engine and
  the offline render agree; a mute still silences a soloed lane); a bounce in place of chosen clips
  plays them whatever the other lanes' solo says. The solo and the colour are the optional,
  ignorable lane fields `"solo"` and `"color"` of project format 43. `ClipMove.lane` and
  `PasteClips.lane` name the target note lane; without one a move keeps the source lane (same track)
  or the note-lane ordinal clamped to the target track, and a paste uses the ordinal each clip had
  when it was copied.
- **Lane workflows** ([lane_cmd.rs](../crates/model/src/lane_cmd.rs)). `MoveNoteLane` moves a lane
  within its track or to another note track (`to_index` counts the target's lanes without it); with
  `copy` the clips take new ids and the take groups stay behind. `MergeNoteLanes` writes what each
  track plays (`flatten_notes_with` in the song's tails mode, `flatten_controllers`) as one clip per
  connected stretch of audible sound on lane 0 and removes the other lanes in one compound edit, so
  the flattened notes and controller points are the ones before (but for one note per tick and key:
  of two equal ones the later lane's stays). `WorkflowCommand::ExplodeNotes` and `ExtractNotes` put
  notes on new lanes under clips of the source's place, length, offset, loop and grid; the source
  keeps its controllers, and a clip another clip partly hides is refused. `Song::moves_note_lane`,
  `explode_refusal` and `extract_refusal` let the views draw only what the model takes. Moving a
  lane only reorders ties; merge, explode and extract-move keep the flattened events, which
  `note_lane_workflows.rs` checks with proptests.
- **Merge.** `flatten_notes` runs its usual body over each played note lane in lane order and sorts
  the result by `(tick, key)`; `flatten_controllers` builds each lane's controllers per kind and
  merges them by tick. The tie rule: at an equal tick, or an equal start of the same key, the later
  lane wins (the engine plays one lane per controller kind with strictly increasing ticks, and a
  note that starts on a sounding key cuts it). Splitting a lane's clips over several lanes therefore
  plays and bounces exactly as one lane that holds them all. Freeze and bounce use the same
  flattening, so a muted lane is silent in both; Bounce in Place of selected clips renders a muted
  lane's clips (it unmutes the lanes that hold them, in the render only).
- **Chase.** Chase works on the merged notes, so a note that spans the play position in any unmuted
  lane starts on Play, a locate and a loop wrap. The engine needed no change.
- **File.** Project format 30 (the module docs of `project/mod.rs`).
- **View** ([arrangement/lanes.rs](../crates/ui/src/arrangement/lanes.rs)). Lane 0 is the track's
  header row; each further note lane has a row of the track's height (with its take rows) that stays
  when the track collapses; the automation rows come last. A lane from 1 on has a header with its
  label and an M button (`LaneButton::Mute`); with several lanes the header of lane 0 shows a small
  chip with the same, hidden when the row is short. The selected lane
  (`ArrangementView::record_lane`, picked by a click on a header, an empty lane or a clip) is view
  state, not saved. A clip dragged to another row goes to the lane under the pointer (`Drag::Move`
  holds `(track, lane)`; a note clip follows the change of track and of note lane, clamped), and a
  double-click on a lane's label (or `sequencer.rename_note_lane`) types its name with the
  folder-name editor. The Track menu and the track header menu have Add Note Lane, Rename Note
  Lane..., Delete Note Lane (two lanes or more), Mute Lane, Solo Lane (`sequencer.toggle_lane_solo`,
  also an S button on each lane header after the first), Record to Lane
  (`sequencer.toggle_lane_record`: a take goes to every armed lane of its track, `Take::to_lanes`,
  one undo step), a Lane Colour submenu
  (`sequencer.lane_color`, in the track header menu), Merge Note Lanes (Ctrl+R) and Explode
  Notes to Lanes (also in the clip menu); the piano roll's Notes menu has Extract Notes to a New
  Lane. A press on a lane's label and a drag (`Drag::LaneMove`, [lane_move.rs](../crates/ui/src/arrangement/lane_move.rs))
  picks an insertion line between note lane rows, also on other tracks; Ctrl (the copy role) copies.
- **Dub and Alt** ([app_core_dub.rs](../crates/app/src/app_core_dub.rs)). `transport.dub`
  (Numpad3) adds a note lane to each armed, unfrozen note track and selects it; with a take
  running the take so far is finished as a locate does and a new one starts on the new lane at
  once, so each note lands on exactly one layer. `transport.alt` (Numpad6) also mutes the lane the
  last pass went to. The lane and the mute are one undo step. Armed audio tracks are not dubbed.
- **App.** A take records to the lane selected when it began (`Take::new` captures it per armed
  track; a lane deleted meanwhile falls back to lane 0), and MIDI import into a track, Audio to
  Notes on an existing track and Paste use the selected lane. Drum, slicer and new-track imports use
  lane 0. MIDI export writes one SMF track per track through `flatten_notes`, so lanes merge and
  muted ones are left out.

### 2.22 Player stacks

A note player can sit above an instrument and play through it for the instrument's track (the
workflow is in [DEVICES.md](DEVICES.md), "Stacking players"). A stack is ordinary note cables; what
the document adds is where a track's notes enter.

- **`Track.notes_to`.** `Option<DeviceKey>`: the device the track's notes enter, the top of its
  stack. `None` is the track's own device and the only value before project format 31.
  `RackDoc::notes_entry(track)` is the one place that resolves it: a `notes_to` that is not a
  top-level device of the document (or is the track's own device) gives the track's own device. The
  track stays the instrument's: its channel, its automation lanes and its freeze do not move.
- **Commands.** `WorkflowCommand::CreatePlayer { type_id, target, options }` plans on the scratch
  copy like the other creation commands: it finds the stack from the selection through the note
  cables (down to the instrument, then to the head: the entry of the instrument's track, or without
  a track the topmost player that feeds it), inserts the player in the head's rack column at the
  head's index, cables `new.notes_out` to the head's Notes input after taking over the cable that
  fed the head, and sets the entry of every track that entered at the head (`Edit::TrackEntry`, one
  undo step). Anything it cannot route is a plain add after the selection. A device is a player when
  it is a utility with `notes_in` and `notes_out` sockets (`is_note_player`; the model cannot see
  the devices crate, and a test pins the shape to `PLAYER_TYPE_IDS`). Removing any device goes
  through `Planner::remove_device`, which first moves the entry of the tracks that entered at it to
  the device its Notes output is cabled to; `DeleteDevices` bridges a player's note cables the way
  it bridges audio.
- **Engine input.** `song_input_with_warps` sends a track's notes and controller lanes to its
  entry's Notes input (a container through its split) and keeps the instrument's automation on an
  engine track of its own; the engine takes one track per device, so tracks for one device join
  (`push_track`). An entry that is not in the rack, has no engine id or no Notes input plays the
  track's own device. Bounce, freeze and the live engine share this function, so they agree.
- **Live.** `AppCore::target_of` tries the track's entry, then the device itself, for the live
  keyboard and the audition; a take records at the entry (the input notes, before the players),
  or, with the track's Direct Record flag (`"direct_record"`, format 39, written only when set),
  at the instrument: `EngineHandle::tap_notes` copies what the instrument receives into a
  `MidiTap` and the take records those notes. Send Players to Track (`crates/render/src/players.rs`)
  renders the document offline with the same tap and writes the notes as a clip on a new lane.
- **File.** Project format 31: `"notes_to"` after `"no_chase"`, omitted when unset.

### 2.23 Video through the user's FFmpeg

Video tracks are decoded by an FFmpeg the user installs: nothing is bundled and no codec ships with the program.
Without FFmpeg the video features are off and nothing else changes. This section is the foundation (the crate
`atomik-video` and its place in the app); the viewer, the thumbnail track and the film's sound come on top of it
(the whole feature is in the last items of this list).

- **Finding it.** `discover` (`crates/video/src/discover.rs`) looks in the folder set in Settings > Video (the
  `ffmpeg_folder` of `workflow.json`; its `bin` folder too, or the `ffmpeg` program itself), else in the folders of
  `PATH`, then on macOS in the folders the usual package managers install into (a windowed app starts with a minimal
  `PATH`). The first folder with both `ffmpeg` and `ffprobe` wins. A folder that is set and has no FFmpeg is not
  silently replaced by the system path: the answer is "missing" with the folder named. `ffmpeg -version` is run and
  parsed (`parse_version`): a release below 4 is `TooOld`, a program that does not run or does not say it is FFmpeg
  is `Broken`, a development build is accepted. Every outcome is an `FfmpegStatus` with a message for the user
  (`FFmpeg 7.1 found at ...`, or `FFmpeg is not installed: video needs it. Install it and set its folder here.`).
- **One seam.** Every `ffmpeg` and `ffprobe` call goes through `VideoTool::run`. `SystemTool` starts the process
  with a list of arguments (no shell, a path is one argument, prefixed `file:` so a name can never be read as an
  option or a protocol, `-protocol_whitelist file`, `-nostdin`, the input closed), no console window on Windows,
  the standard error kept to its last 4 KiB for messages, and the child killed when the job is cancelled, when the
  consumer of its output stops and when the call unwinds. `NoVideoTools` refuses every call: a core starts with it,
  so a test or a headless session never starts a program, and the platform layer installs `SystemTool`.
  `FakeTool` answers with canned text and frames and records the calls; no test needs FFmpeg.
- **Compressed audio export.** The same seam also encodes the audio export to MP3, AAC or Opus
  (`encode_audio` in `crates/video/src/encode.rs`, see section 2.7a): a WAV file in, a tagged file out, with progress,
  cancel and no partial file.
- **Film export.** The same seam makes a new film from the song's mix and the picture of its film
  (`export_film` in `crates/video/src/film.rs`, see section 2.7a): the picture copied or re-encoded, the mix as AAC
  or PCM, optionally the film's own sound as a second track, with progress, cancel and no partial file.
- **Probe.** `ffprobe -v error -print_format json -show_format -show_streams` is parsed by `parse_probe`: length,
  size, rotation (display matrix or the old `rotate` tag), the nominal frame rate (the average rate, snapped to the
  standard rates within 0.02 %; the container's rate when there is no average; `variable_rate` when the two differ
  by more than 1 %), start time, timecode, and the audio tracks with channels, rate, language and title. Cover art
  is not a film.
- **Proxy.** A film is turned into one file of small independent JPEG frames (`proxy.rs`): every frame of the
  picture at the nominal rate (`fps` and `scale=-2:360` in the filter, so frame `n` is at `n / rate` whatever the
  film's timestamps and the picture is upright), cut from ffmpeg's piped `mjpeg` output by `FrameSplitter` (it walks
  the JPEG markers, so bytes in a table are never taken for an end), and written by `ProxyWriter`: a 40-byte header
  (version, size, rate, frame count, index offset), the frames, and an index of frame offsets, so frame `n` is one
  seek and one read (`ProxyReader::read_frame`). JPEG was chosen because it is intra-only, about 20 to 40 KiB at 360
  lines and decodable by a small pure-Rust decoder (`zune-jpeg`, see [Dependencies](DEPENDENCIES.md)).
- **Build.** `prepare` probes, finds the proxy in the cache or builds it: ffmpeg's output is split and written to
  `<key>.atproxy.part` with `done` and `total` frame counters for progress, and the file is renamed only when ffmpeg
  ended with success and every frame is in. A cancel, an error, a cut-short frame or a stream without frames removes
  the part file; a proxy under its final name is always whole.
- **Cache.** `ProxyCache` lives in the per-user cache folder (`LOCALAPPDATA/Atomik Rack/Cache/video` on Windows,
  `~/Library/Caches/Atomik Rack/video` on macOS, `$XDG_CACHE_HOME/atomik-rack/video` elsewhere). The key is a hash
  of the absolute path, the size, the modification time, a hash of the first and last mebibyte and
  `PROXY_VERSION`; an edited, moved or replaced film has another key, and a proxy of another format version is
  refused by the reader and rebuilt. After each build the cache is pruned to 2 GiB, least recently used first (a
  hit touches the file), never the proxy just built, and unfinished files are removed.
- **In the app.** `AppCore::check_ffmpeg` and `AppCore::start_prepare_video` run on the `atomik-video` worker
  (`crates/app/src/video_worker.rs`, one job at a time, a result message even when the thread panics) with the
  progress in the status line and `AppCore::cancel_video`; closing the app cancels the job. File > Video holds
  Prepare Video, Check FFmpeg and Cancel Video Job (`video.prepare`, `video.check_ffmpeg`, `video.cancel`). The
  films prepared in a session are kept in `AppCore::prepared_videos`. The engine and the audio thread are not
  touched; the film's place in the song is in the next list.
- **The film in the song.** `RackDoc::video` is `Option<VideoClip>` ([model/src/video.rs](../crates/model/src/video.rs)):
  one film per song, **linked by path and never copied** (films are huge; Collect Files leaves it where it is), with
  the facts the probe found (`VideoFacts`: length, frame rate as a fraction, picture size as it shows, start
  timecode, whether it has sound) so a project opens, and shows its clip, without FFmpeg. Every position is
  **integer microseconds**, not ticks: a film runs on the clock, so a tempo change must not move it (a tick is a
  fraction of a beat and moves in time with the tempo map). Integers compare exactly, round-trip through the file as
  plain numbers and need no frame arithmetic (30000/1001 fps has no exact microsecond frame; the viewer rounds, the
  model never does). `VideoClip::start_tick` / `end_tick` give the tick a position falls on under the song's tempo
  map for the ruler. The clip has a **start** (in song time, up to 24 hours before or after the song's start: a
  film may begin before bar 1) and a **trim** `in_us..out_us` of the source, at least a millisecond long and inside
  the film; trimming the in point moves the start with it, so the picture that stays keeps its place. It is saved as
  the root field `"video"` only when set (**format 44, `.ignorable()`: no bump**), read and written by
  `video::{load_video, write_video}`, and edited with `Command::Video(VideoCommand)`: `Set` (add or replace),
  `Remove`, `Move`, `Trim` and `Relink`, each one `Swap` that reports `ViewChanged`; `Move` and `Trim` carry a merge
  id so a drag is one undo step (`MergeKey::Video`). It is not accepted by `Command::Batch`. The model has no
  dependency on `atomik-video`: the app turns a `ProbeInfo` into `VideoFacts` (`facts_of`).
- **Import, relink, missing.** The video worker can report its probe before it builds the proxy
  (`Msg::VideoProbed`). Import Video (`video.import`, File > Video) picks a film, probes it and the song takes it at
  its start when the probe is in (one undo step, replacing a film the song had), while the proxy builds on in the
  background with progress and cancel; if that is cancelled the film stays in the song without its preview. The
  probe of a job is applied only to the project that started it. Without FFmpeg the toast is the install hint and
  nothing changes. Relink Video (`video.relink`) probes another file the same way and points the film at it with
  the new facts, keeping the start and trim (the out point is limited to the new length); when the length (more than
  half a frame apart) or the frame rate differs from what the project knew, the toast says so
  (`VideoFacts::differences`). A film whose file is gone opens as usual; `AppCore::video_missing` (cached per path,
  looked at again on open, import and relink) marks it, a toast says so at open, and the project inspect of the MCP
  tools lists the film with `missing`. Remove Video, Move Video to Playhead and Trim Video Start / End to Playhead
  (`video.remove`, `video.move_to_playhead`, `video.trim_start`, `video.trim_end`) read the playhead through the
  tempo map into the film's own time.
- **The Video window.** View > Video (`view.toggle_video`) is a floating window of its own, like the Spectrum EQ
  (`WindowKind::Video`, `Pane::Video`, `windows.video` in `ui-state.json`); its view is
  [video_view.rs](../crates/ui/src/video_view.rs) and its logic `app_core_video_view.rs`. The core works out the
  picture once per `AppCore::frame`: the song position (ticks, read through the tempo map into microseconds) goes
  through `atomik_video::timeline::locate` with the film's start, trim and the proxy's rate, in integer arithmetic
  (a frame changes exactly at its start for 24, 25 and 30000/1001 alike), giving the proxy frame, or black before
  and after the film with the frame number the film would have (for the timecode, drop frame included:
  `Timecode`). A `FrameDecoder` worker thread opens the proxy and decodes with `zune-jpeg`; the core asks it for
  the frame shown first, then `timeline::wanted` (ahead while playing, around while stopped, the first frames of
  the loop near its end), a new request replaces the old one so a slow decode drops frames, and decoded frames
  sit in a byte-limited `FrameCache` (48 MiB). Nothing is read or decoded on the UI or audio thread; closing the
  window frees the worker and the pictures. The song position is whole ticks, so the picture can change up to one
  tick (under a millisecond at usual tempos) late and at most one UI frame after the transport. The view paints
  an RGBA image brush, the timecode and frame number overlay (`video.toggle_overlay`) and a bar with Fit, 50%,
  100%, 200% (kept in `ui-state.json`). With no picture it shows a message (no film, file missing, FFmpeg
  missing) or the proxy's progress. The proxy of a song's film is built quietly (`Purpose::Open`, no toast) once
  per film when a project with a film opens (after the platform has set the tools and cache, in `video_frame`)
  and when the window opens. No key is bound (the menu has both commands).
- **The video row.** A song with a film shows it in a row of its own above the track lanes, under the tempo and
  energy lanes and over the whole width (it does not scroll with the tracks): `arrangement/video_row.rs`, 54 px
  high, or an 18 px bar when collapsed with the chevron of its header cell. View > Lanes and Markers > Video Row
  (`view.toggle_video_row`) hides it; both choices are in `ui-state.json` (`show_video_row`, `video_row_collapsed`).
  The row is its own band between the lanes and the energy lane, so no lane or clip lies under it and it takes no
  press a track would. The bar runs from the film's start to its end through the tempo map; thumbnail slots are as
  wide as a picture of the bar's height (24 to 170 px), slot `k` starting `k` slots after the film's left edge and
  showing the frame that plays at its start (`VideoSlot`), so the slots follow the zoom and hold still while
  scrolling. Only the slots in view are asked for: `ArrangementView::video_wanted` lists the frames without a
  picture, the middle of the view first, and the core (`app_core_video_thumbs.rs`) hands them to a second
  `FrameDecoder` (`spawn_file_scaled`: its own thread and file handle, each picture scaled down to 92 px high by
  `Picture::scaled_to_height` on that thread) and gives the finished pictures to `add_video_thumb`. The row keeps
  them by frame number in a 10 MiB budget (the least recently wanted leave first), so zooming back finds them, and
  a slot waits as a plain placeholder. A collapsed or hidden row, a missing film or no preview closes the worker.
  The core also feeds the row (`VideoFeed`): missing, the preview's frame count, a note for when there is none
  ("Preparing the preview 40%", no FFmpeg, not built) and the timecodes at the playhead, of the first and of the
  last frame of the trim, counted from the film's own start timecode (the header shows the playhead's, the bar
  the other two); a missing film is a red labelled bar and a film without a preview a labelled one, and both
  still move. A press selects the film (any other press or Escape lets go, Delete removes it:
  `VideoCommand::Remove`), a drag of the body moves it and a drag of either end trims it, every step a
  `Command::Video` with one merge id and `EndMerge` at the release (Escape sends the first values back under the
  same id): one undo step. The snapping is `atomik_model::video_snap`: a move takes the grid line nearest the
  start (`moved_start`; a start before the song's first tick has no grid) and then the nearest whole frame of the
  song's clock; a trim takes the grid line for the edge's song time, then the nearest whole frame of the source
  (`trimmed_in`, `trimmed_out`: at least one frame is left, and an out edge within half a frame of the file's end
  takes the end itself). The no-snap role switches both snaps off. The row is drawn live over the cached lanes,
  so a timecode that changes with every frame of playback does not re-record them.
- **The film's sound.** Import Video Audio (`video.import_audio`, `app_core_video_audio.rs`) is a third job of the
  video worker (`Job::Audio`): it finds FFmpeg, probes the film, chooses the track (`atomik_video::pick_stream`: the
  one asked for by its stream number, else the only one, else the job ends with the list and the shell opens a
  pop-up of one row per track whose choice runs `video.import_audio_pick <stream>`, which starts the job again for
  that track; no track is `VideoError::NoAudio`, and a film whose facts say it has none is refused before any
  program starts), writes it with `extract_audio` (`audio_args`: `-map 0:N -vn -c:a pcm_s24le -rf64 auto -f wav`,
  `-ac 2` only above two channels, no `-ar`, progress from `-progress pipe:1` read as `out_time_us` into the
  job's counters, the file removed on a cancel, a failure or an empty result) into a temporary folder, hands the
  WAV to the song's own `import_file` (decode, resample to the song's rate by the existing converter, store in the
  bundle by hash with peaks) and deletes the folder. A cancel set before the import leaves no asset; one that
  comes after the file is stored is too late and the sound is placed. The result reaches the UI thread as
  `Done::audio`; the track, the clip and the link are added there as one undo step (`join_next` folds the clip and
  the link into the step of the track), only for the project that started the job and the film that is still the
  song's. File > Video > Import Audio with Video (`ui-state.json`: `video_import_audio`) chains the job after
  Import Video.
- **Aligning the sound.** `atomik_model::video_audio::placement` works in frames of the song's rate and is pure.
  The probe gives each track its own `start_time_s`; the lead is `audio start - picture start` in microseconds
  (`atomik_video::lead_us`; ffmpeg writes no silence for a late start, so the lead is the whole offset: checked
  with a real file whose sound starts 0.5 s late). The track's first sample belongs at the song frame of
  `film start + lead - trim in`; what is shown is the part inside the track and inside the trim from the song's
  start; the clip starts on the first tick whose frame is at or after that and `source_offset` skips the
  frames between, so the sample under the first frame plays on exactly that frame at any tempo; the length is
  whole ticks that stop at the trim's out point (never past it) or cover the track to its end. A film before the
  song's start, a track shorter than the film and a trim with no sound in it are handled (the last gives no
  placement and the clip stays).
- **Keep with video.** The link is `VideoClip::audio_link: Option<AudioLink { clip, lead_us, keep }>`, saved as
  `"audio_link": {"clip", "lead_us", "keep"}` inside the `"video"` object (**still format 44**: the root field is
  ignorable, so an older reader drops the whole film with its link; nothing new at the root). `Move`, `Trim` and
  `Relink` (and `KeepAudio(true)`) plan the film's `Swap` and, when the link is kept and names a plain unwarped
  audio clip of an asset at the song's rate, the clip's own edit (`SetAudioClip` planned and validated like any
  other, fades and envelope clamped to the new length), as one `Edit::Compound`: one undo step, undone in reverse.
  A drag merges the pairs (`Edit::merge_video`: the film and the clip each keep their first old and take the last
  new value), so it stays one step. The clip is recomputed from the film's state, not moved by the delta, so a
  tick rounding never accumulates. A clip that was deleted, warped or turned to another rate is skipped and the
  film still moves; `VideoCommand::LinkAudio` sets or clears the link and `KeepAudio(bool)` turns the following off
  and on (on lines the clip up again). `Command::Video` now also reports `SongChanged` when it moved a clip.
- **Tests.** `crates/video` has unit tests for the version parser, the discovery order, the probe of an MP4/H.264,
  a ProRes MOV with a timecode, an MKV with two audio tracks, a silent film, a rotated variable-rate phone film
  and cover art, the argument lists, the frame splitter, the proxy index and random access, the cache key,
  the version check and the pruning, and the worker's cancel; all with the fake. One ignored test uses a real FFmpeg
  (see [TESTING.md](TESTING.md)). The model has `crates/model/tests/video.rs` (every command as an undo step, drags
  merging, the trim and start limits, relink, a tempo change keeping the film locked to time, the saved text and an
  old file) and a video op in `doc_sequences.rs`, and the frame snap arithmetic in `video_snap.rs`; the UI has
  `crates/ui/tests/video_row.rs` (slot layout per zoom, the frames asked for, the byte budget, the states painted,
  moves and trims with both snaps, Escape, Delete, no press taken from a track); the app has
  `app_core_video_clip_tests.rs`, `app_core_video_thumbs_tests.rs`, `app_core_video_audio_tests.rs` (the track
  chosen, the rate converted, the start offsets, no audio, no FFmpeg, a failing FFmpeg, a stopped job leaving no
  asset, keep with video through a move, a trim and the toggle, one undo step, the option of Import Video) and the
  File > Video menu tests, all with the fake; the model has `crates/model/tests/video_audio.rs` (the sound moving
  with the film to the sample at two tempos, one undo step, a drag merging, a trim, the lead, keep off and on, a
  missing clip, the saved link) and the arithmetic tests of `video_audio.rs`.

## 3. Inside `Engine::render`

[crates/engine/src/engine.rs](../crates/engine/src/engine.rs):

- **Batches.** Devices always process exactly `BATCH = 64` frames
  ([crates/core/src/lib.rs](../crates/core/src/lib.rs)). `render` accepts any host block length,
  including 0. It copies host input into a one-batch input FIFO and copies the previous batch's
  output out of a one-batch output FIFO. Each time the FIFO fills, it runs one batch.
- **Latency.** The FIFO adds a fixed latency of 64 frames (`Engine::latency_frames()`). It is part
  of the recording compensation. Device latency (`Device::latency_frames`) is compensated by the
  routing (section 5).
- **Batch order** is the one in 2.4. Control messages travel through one ordered queue
  (`CONTROL_QUEUE_CAPACITY` = 1024), so "insert A, then a routing that uses A" is always applied in
  that order.
- **Per device**, in routing order: zero the device's output scratch (so outputs arrive zeroed and
  unconnected inputs read zeros), gather cabled inputs, merge cable notes with scheduled notes, call
  `process_stage`, scatter the outputs to slots.
- **No routing.** The engine outputs silence. Notes that arrive while there is nothing to deliver
  them to are counted in `dropped_events()`.
- **Retiring.** Replaced plans, routings, songs, taps, replaced sample maps and removed devices are
  handed back through a retire queue and dropped by `EngineHandle::collect_garbage` on the caller's
  thread. If the queue is full the item is parked and further control messages wait. Nothing is ever
  dropped on the audio thread.

### Hot edits

A persistent **device arena** holds the prepared devices; a **routing plan** (order, levels, slots,
cable copies, delay lines) only references them by slot. `Topology` is the non-RT routing model,
validated like `Graph`; `Topology::compile` produces a `RoutingPlan`;
`EngineHandle::{insert_device, remove_device, submit_routing}` queue changes. Structural edits
therefore never reset a device's state. The `Graph` / `RenderPlan` / `submit_plan` API still works:
a plan swap exchanges the whole arena and routing in one control message.

### Multicore

[workers.rs](../crates/engine/src/workers.rs): a pool of `atomik-render-N` threads runs the devices
of one dependency level alongside the audio thread. Ownership is handed off, not shared: the audio
thread moves `Box<Node>`s into per-worker `rtrb` job queues and gets them back through done queues
(no `unsafe`, no locks). Gathering inputs and scattering outputs always happen on the audio thread,
in plan order, so the output is bit-identical for any worker count. The audio thread waits at a
barrier (a bounded spin, then yielding its core; counted as telemetry); idle workers spin, then
park. The app and `atomik-play` start at most `cores - 1` workers. A level with a single device is
never dispatched. The app's default is an automatic count (`auto_workers` in
[audio-io](../crates/audio-io/src/lib.rs)): one worker per physical core but the audio thread's own,
at most 8. Simultaneous multithreading is read from the kernel on Linux and guessed elsewhere
(`workers_auto` in `settings.json`; a file that sets `workers` and not `workers_auto` keeps its
number, the Audio settings tab has an Auto chip).

### Per-device processing time

[timing.rs](../crates/engine/src/timing.rs) feeds the CPU readout (View > Meters and Links > Show Device CPU). The
engine never reads a clock itself: the host installs a monotonic nanosecond counter once (`install_clock`;
the app passes `monotonic_ns` of audio-io) and switches the measurement on with `set_enabled`; the app core does that
when it is made, so the engine measures all the time and the View switch only decides whether the figures show.
`Node::run` reads the counter before and after each device call and adds the
difference to the node's own `busy_ns`, on whichever thread runs the node (a worker of the multicore engine measures its
nodes the same way, and a plugin in its own process is timed across the call that waits for it). After each batch the
audio thread publishes every device's total next to the audio time rendered so far into
[`Meters`](../crates/engine/src/console.rs) (`device_cpu`, a few atomics per device; the slots are
cleared with the device-value slots when a device leaves or the arena is swapped). A reader keeps
two samples and turns them into a load with `load_percent`: the share of one callback's budget the
device used. The numbers are never read back into the signal, nothing allocates and nothing locks
(the clock slot is a `OnceLock`, read as an atomic load), so a bounce is bit-identical with the
measurement on or off (`cpu_timing.rs` in [engine/tests](../crates/engine/tests), which also pins the cost at two
clock reads per device call with a fake clock; `cpu_slow_device.rs` runs a busy-wait device, serial and on workers,
under the allocation guard and checks that it has the biggest share). On, it costs about 70 ns per device per batch
(100 devices cost about 0.5 % of a 64-frame callback at 48 kHz; [perf/BASELINE.md](../perf/BASELINE.md); the
`render_batch` bench prices it with `ATOMIK_BENCH_TIMING=1`). With the switch off a node pays two relaxed loads per call.
The app side is [app_core_cpu.rs](../crates/app/src/app_core_cpu.rs): once per frame it reads the
samples and, once 200 ms of audio have passed, turns them into a load that goes into the device's `Gauge`
([cpu_gauge.rs](../crates/app/src/cpu_gauge.rs)): the load averaged over about a second (an exponential average with a
time constant of one second of audio) and a peak held for two seconds. The core sums each track's chain (the
instrument, its channel and the insert effects, found with `track_chain`) into `TrackCpu`. The loads go to the rack
view, which draws them on the name plates over the cached panels (a container shows the sum of the devices inside it),
and the track totals to the console, which draws them live on the strip name plates; `paint::cpu_text` and
`paint::cpu_color` give both the same text (one decimal below 10 %) and the warning colour above 25 %. The status line
tooltip ([shell_cpu.rs](../crates/app/src/shell_cpu.rs)) lists the five heaviest devices (`heaviest_devices`) under the
engine load of the audio status; the tooltip box takes one row per line. The freeze hint is the pure `HintWatch` of
`cpu_gauge.rs` (the engine load above 70 % for 5 s while playing, once per track per song, the heaviest freezable
track that was not suggested yet) and a card in the lower left corner ([cpu_hint.rs](../crates/app/src/cpu_hint.rs))
whose Freeze button calls `freeze_track_of`. The switch and the hints given are not saved; a new song forgets both
the figures and the hints.

### Master tune

The project's tune (`RackDoc::tune_cents`, -100 to +100 cents around A = 440 Hz, set by
`Command::SetMasterTune`, one undo step) reaches the devices as `TimeInfo::tune_cents`. The app sends it
through the ordered control queue (`EngineHandle::set_master_tune`): with a glide for an edit (about 20 ms,
one exponential step per batch, so held notes do not step) and without one for a fresh engine and for
`render_core`, which queues the document's value before the first batch of a bounce. It is never a live
atomic: every batch carries one value that changed at a batch boundary, so a bounce is the same for any host
block size and a project at 0 renders bit for bit as before (every device adds a literal 0 cents). The
synths add it as the cents offset of `note_to_hz`; the sampler, drum machine, loop slicer and grain sampler
add it to their playback ratio; the tuner in the app reads against `tuner_ref` shifted by it. Audio tracks
and plugins are not touched. The per-device audit is in [DEVICES.md](DEVICES.md#master-tune). The Audio
tab of Settings edits the open project's tune and, once changed there, keeps the value as the tune new
projects start with (`master_tune_cents` in the settings file).

### Modulation matrix

A device whose type declares modulation tables (`ModTables`, registered with `Registry::register_with_mods`) has
eight slots; a slot routes one of the device's own sources to one of its own targets with an amount and an optional
`via` source that scales it. It is the working default of decision E in
[DECISIONS.md](DECISIONS.md#e-modulation-framework) (device-level slots, no engine change), and the engine never
sees a slot. The document keeps the structure per device
(`RackDevice::mods`: source, target and `via` as ids into the device's tables, edited by
`WorkflowCommand::SetModSlot`, one undo step, saved from format 41 only when a slot is in use) and the amount of
slot 1 as the ordinary param `mod1_amount` (and so on), so it is automatable. `RackDoc::expand` turns the structure into the
device's `internal.mod1_source`, `_target` and `_via` params (id + 1, 0 for none; `SyncOp::ModSlotsChanged` makes the
app re-send them after an edit), and the device sums its slots with a `ModBus` at its control rate: `amount * source
* via` in slot order with `+ - * /` only, added to the target in the device's own unit. A device with no slot in use
runs none of that code, so a project without slots bounces bit for bit as before. The poly synth and the mono bass
are the first users (nine sources, three targets); the framework is written down for device authors in
[EXTENDING.md](EXTENDING.md#modulation-matrix) and the sources and targets in [DEVICES.md](DEVICES.md).
The matrix is edited in [`mod_matrix_panel.rs`](../crates/ui/src/mod_matrix_panel.rs) (`ModMatrixView`), a modal
view the device's context menu opens (`rack.mod_matrix`): the app feeds it `ModMatrixInfo` from the registry's tables
and the device's slots (`AppCore::mod_matrix_info`), the pickers are a `ContextMenu` of the table's names, and every
edit is an ordinary `UiAction::Command` (`SetModSlot`, `SetParam` on `modN_amount` with one merge id per drag).

## 4. Threading model

| Thread | Runs | May allocate or block? |
|---|---|---|
| Audio callback (cpal, or the caller of `render_offline`) | `Engine::render`, the song player, audio rendering, the record tap copy | No |
| Engine workers `atomik-render-N` | `Device::process_stage` of one level | No |
| UI thread (winit, vello) | `AppCore`, `Shell`, views, `EngineHandle` calls, `compile`, applying worker results | Yes, but never waits for the audio thread |
| `atomik-streamer` | `StreamSource::read` (disk), fills lane rings | Yes (disk), isolated from the audio thread by the ring |
| `atomik-import`, `atomik-load` | decode, resample, hash, write, peaks, read-back | Yes |
| `atomik-record` | drains the record tap, writes WAV, cuts laps, stores assets | Yes |
| `atomik-browse` | file browser: folder listings, the tree's sub-folders, the recursive walk, favourite and tag places, the drives | Yes (disk) |
| `atomik-probe` | file browser: audio header reads (`probe_file`), visible rows first | Yes (disk) |
| `atomik-preview` | file browser: decodes the previewed file and fills the preview ring | Yes (disk) |
| `atomik-detail` | file browser: the tempo a file states and its loudness picture for the info pane (newest request only) | Yes (disk) |
| `atomik-thumbs` | file browser: the 48-bar waveform thumbnail of each visible audio row (first thirty seconds, newest request first) | Yes (disk) |
| `atomik-library-save` | file browser: writes `library.json` from the newest snapshot | Yes (disk) |
| `atomik-video` | video: looks for FFmpeg, or probes a film and builds its proxy file from ffmpeg's piped frames (one job at a time, progress and cancel); `atomik-video-stdout` and `atomik-video-stderr` read the two pipes of the child process | Yes (disk, child process) |
| `atomik-update` | update check: one request for the list of releases, only after the user said Yes, at most once a day (one short-lived thread per request, a timeout of 8 s; the shell polls its answer on the frame) | Yes (network) |
| `atomik-library` | library: copies the audio of a saved item into its sibling bundle | Yes (disk) |
| `atomik-adopt` | library: copies the audio of a loaded item into the session bundle, then decodes and builds peaks like `atomik-load` | Yes (disk) |
| MIDI callback (midir) | maps event times to frames, pushes live notes; parses every message once more for the control path and queues it in the control hub (a short mutex, never held by the audio thread), consuming it when a mapping listens | Yes, then one wait-free push |
| `atomik-midi-out` | MIDI output: drains the taps of the MIDI Out devices and the engine's clock tap, converts engine frames to wall-clock times, sends each message to its port when due (polls every millisecond, spins for the last 0.3 ms); releases the notes of a source that goes away | Yes, but never waits for the audio thread |
| MIDI clock callback (midir) | measures the tempo of a followed clock and counts its position, and publishes both, with Start, Stop, Continue and the locates, in a lock-free cell the audio thread reads at the batch boundary | Yes, a few atomic stores |
| Control surface callback (midir) | queues the raw bytes of the surface (SysEx included); the UI thread polls, decodes and answers | Yes (a short mutex) |

The engine is created as two halves by `new_engine(cfg)` or `new_engine_with_options(..)`:

| Half | Type | Runs on |
|---|---|---|
| Real-time (RT) | `Engine` | the audio thread |
| Non-real-time | `EngineHandle`, `Graph`, `Topology`, the compile functions | any other thread |

They share only wait-free single-producer/single-consumer ring buffers (`rtrb`), allocated in
`new_engine`, and a few atomics:

| Queue | Direction | Carries | Capacity |
|---|---|---|---|
| control | handle → engine | inserts, removals, routings, plans, timed params, tempo, song snapshots, transport, the record tap, device sample maps, `PreviewStart` and `PreviewStop` | 1 024 |
| retire | engine → handle | replaced plans, routings, songs, taps, sample maps, preview streams (`Retired::Preview`) | 64 (+ 1 parked) |
| removed devices | engine → handle | devices taken out of the arena | the arena size |
| params | handle → engine | `(device, param, plain)` | 4 096 |
| notes | handle → engine | `ScheduledNote` | 4 096 |
| live notes | MIDI thread → engine | live note events | 4 096 |
| stream requests | engine → streamer | `StreamRequest` | 256 |
| record ring | engine → writer | records of `u32` words | 4 s of input |
| preview ring | decoder → engine | interleaved stereo `f32` frames of the file being previewed (one ring per preview) | `PREVIEW_RING_FRAMES` = 65 536 frames |
| atomics | engine → everyone | dropped events, stream underruns, meters, latencies, the transport state (a seqlock cell) | – |

A full queue makes the handle method return `EngineError::QueueFull`; the engine never waits. Live
audio adds `atomik-audio-io`: the callback publishes `(engine host frame, instant)` through a
`FrameClock` (so MIDI can be mapped to an exact frame) and counts xruns and DSP load. cpal has no
duplex stream, so captured samples reach the render callback through a wait-free ring.

**A stream that stops.** The callback only counts itself. `atomik_audio_io::Liveness` compares the
callback counter with the clock on the UI thread (the status poll, four times a second): no new
callback for more than `max(4 periods, 250 ms)` is `StreamState::Stalled` (a stream that never
called back gets two seconds), and clears by itself when the callbacks return. The stream error
callback sets `Telemetry::device_lost` when cpal reports the device gone or the stream invalid
(`StreamState::Lost`, final until the stream is rebuilt). The shell announces either once with a
toast that names Audio > Restart Audio, shows `AUDIO STOPPED` or `AUDIO DEVICE LOST` in the status
bar, and, when `device_loss.reset_to_default` is on in the settings, reopens the host's default
devices (a lost device at once, a stalled one after two seconds of silence) without saving them.
`LiveSession` keeps the devices that were chosen (`fall_back_to_defaults`, or the configuration that did
not open and was replaced by the last working one) and, while the fallback plays, reads the device list
every 3 s; once the chosen output device is listed again, after it had been missing from the list, the
settings name it again, a notice says so and `take_switch_back` asks the shell for the restart, which waits
while the song plays or records. It is one try (a failed one does not make the device wanted again), and
picking devices in the dialog forgets it.
Input that the capture ring could not hold, or that the render callback discarded to bound the
latency, is `Telemetry::input_overruns` (frames; the first discard, of what was captured before the
output started, is not counted). The Audio Settings dialog warns when the open input and output are
different devices, since their clocks drift apart; a device named like the output is the default
input, so one interface for both is what opens unless another input is chosen.

**Settings that do not open.** `LiveSession` remembers the device configuration that last opened a
stream (`device_loss.last_good` in `settings.json`). When the current one fails to open, at
start-up or in Apply, the session opens the remembered one instead and `Shell` says so;
`apply_settings` writes the file only with devices that opened, so a configuration that fails is
never saved. Without a remembered configuration audio stays off with the error and the file keeps
the old devices.

**Several MIDI inputs.** `settings.json` holds `midi_inputs`, a list of `{name, enabled, role, mpe}`
(`MidiInputSetting`; `mpe` is `{enabled, side, members}`, left out of the file while it is the default, see
"6p. Per-note expression"); every enabled port is open at once. `role` is `keyboard` (its notes play the
selected device and feed the mappings) or `mapping` (it feeds the mappings only, its notes play
nothing). A file with the old single `midi_input` and no `midi_inputs` loads as that input,
enabled, and is saved in the new form. The engine has one live note queue with one producer, so
every port, the audition path and the Piano Keys push through one `LiveFeed` behind a short mutex
(`MidiLink` clones share it): the MIDI threads and the UI thread take that lock, the audio thread
never does. Each port has its own `PortInput` (parser, running status, `NoteRouter`), so a
note-off goes where its port's note-on went. The feed never stamps a note before one already
queued and never later than the host frame plus two buffers (`LiveFeed::admit`), so a stamp from a
stale clock cannot hold back the notes behind it, since the engine only looks at the head of the
queue. `LiveSession::sync_midi` opens and closes ports to match the settings and leaves the
others alone; a port that is not there yet is reported in the status and opened by Audio > Rescan
MIDI Ports (`audio.rescan_midi`) or, while Audio Settings is open, by the one second re-listing
of the ports. The `session_midi_input` tool takes the name of the port a message comes from.

**MIDI output.** The MIDI Out device (`atomik.midi-out`, see [DEVICES.md](DEVICES.md)) sends notes and
controllers to a port, and the audio thread never touches a port. The device turns what it
receives into MIDI bytes and pushes each, stamped with the engine host frame it belongs to, on a
`MidiTap` (`Device::midi_tap`, device-sdk): a preallocated lock-free ring of 4 096 messages of at
most three bytes, written by the device and read by one thread, with 256 slots kept for messages
that release a note (a full ring counts what it loses and, when a release is lost, tells the
reader to silence the port). The engine publishes the tap of every device that has one through
its meter bank (`Meters::midi_taps`, kept like the scope taps and forgotten when the device is
removed or a plan replaces the arena). `LiveSession` hands the taps, and the port of each device
(the device's own, else `midi_out_port` of the settings), to the `atomik-midi-out` thread
(`atomik_midi_io::out`), which it starts with the first such device. The thread converts frames
to wall-clock times from the `FrameClock` the audio callback publishes plus the output latency
(`frame_due`), holds one time-ordered queue per port (`Scheduler`), polls the taps every
millisecond and spins for the last 0.3 ms before a message is due. It records which notes each
source has sounding, so a source that goes away (the device deleted, its port changed, the
session ended, the engine rebuilt) is silenced with a note off per note and All Notes Off on its
channels. A port that will not open or that refuses a send drops its messages and is tried again
once a second. `AppCore` tells the sink which port each device sends to (`set_midi_out_ports`,
by engine device id) after every edit and rebuild. The scheduler and the thread are not specific
to notes: a message is a source, a port, a due time and up to three bytes, which is all MIDI clock
output needs, and the clock is the second user (see below). The jitter of the path is about a poll
interval (1 ms) for a message produced less than that ahead and tens of microseconds otherwise,
plus the port driver's own latency; a bounce never sends MIDI (nothing reads the taps offline,
and the device has no audio output).

**MIDI clock out.** With Options > Sync > MIDI Clock Out the song player (`player.rs`) writes, in
every playing batch, a MIDI clock for each 640 ticks (24 per quarter note) into a second `MidiTap`
(`EngineHandle::sync_out`, `crates/engine/src/sync.rs`), each stamped with the engine host frame.
A clock sits at `F(640 k)`, the song frame of tick `640 k` through the tempo map and the same
rounding every note gets (`clocks_in`), so it is exact at a constant tempo and across ramps and
cannot drift; ranges that tile the timeline give every clock once. Transport changes are noticed
at the batch start and sent once: Start from tick 0, Continue from elsewhere, Stop, a Song Position
Pointer (sixteenth notes, 14 bits) for a locate and for a loop wrap, and a locate while playing as
Stop, position, Continue (or the position alone, by setting). Switching the clock on while playing
tells the receiver the position and continues it; switching it off sends one Stop. The clock can
run on while the transport is stopped, at the tempo of the position. Everything is a few atomic
stores per message (`SyncOut::configure` flags are read once per batch), so a batch that sends
nothing costs one atomic load, and a bounce is unchanged. The tap reaches the `atomik-midi-out`
thread as a clock route (`Core::set_sync`, source id `SYNC_SOURCE`): each message is queued for
every clock port at its time plus that port's offset in milliseconds. `LiveSession` starts the
thread for the clock alone, even with no MIDI Out device, and takes the route away half a second
after the clock is switched off, so the Stop is out and the thread idles again. The settings are
`settings.json` `"sync"` (the Sync tab of Settings and Options > Sync), app settings, not part of
the project.

**MIDI clock in.** Options > Sync > Follow MIDI Clock opens the port named in the Sync tab with
`atomik_midi_io::clock::open_clock_in`. Its midir callback feeds a `ClockFollower`, which keeps
the arrival times of the last 24 clocks (one beat) and takes the tempo from their mean spacing (a
gap of half a second starts the measurement afresh; the last tempo stays meanwhile), and counts
Start, Continue, Stop and Song Position Pointer into an `ExternalClock`, a seqlock cell like the
transport cell (one writer, no waiting). While the cell is followed (`ExternalClock::set_following`)
the player reads it at every batch boundary: the clock's tempo replaces every tempo map with one
constant segment (the position keeps its musical place when the tempo moves: the tick is kept and
the frame recomputed), Start locates to 0 and plays, Continue plays, Stop stops and releases the
held notes, a position locates (chase included). The transport commands and the count-in are
ignored meanwhile (the app also refuses the buttons and says why), and a clock count that does not move
for half a second stops the transport like a Stop, then continues it when the clocks return. Following
changes nothing when it is off, and a bounce never follows a clock; live following is outside the
determinism contract by nature.

The measured tempo lags a sender whose tempo wanders, so the player also locks its phase (`phase_lock.rs`).
The `ClockFollower` counts the sender's position in whole clocks (`ExternalState::phase`: the last Start or
Song Position Pointer plus 640 ticks for each clock that arrived while the sender ran; a count, so it does
not wander with the tempo). Once per batch the player compares the middle of the clock being counted with
its own tick (folded into the loop when one plays, the shortest way round) and closes the error over an
eighth of a beat, at most 10 % of the ticks of the batch, in whole frames with the remainder carried, so the
same clock always gives the same positions and no batch moves the song by more than a couple of frames. An
error beyond a beat is no drift (clocks lost on the cable, or a sender that moved without saying so) and
locates to the count; a Start or a Song Position Pointer is the hard anchor, a locate that restarts the
lock. A sender that has not said where it is (clocks and Continue only) gives no count and the song runs
on the tempo alone.

Audio clips play while following (`audio_follow.rs`). They are placed on the song's own tempo map, which the
followed tempo replaces, so a clip is played like a note: it starts at the player's frame where the song
reaches its first frame (through the followed tempo and back through the song's own map) and runs at its
own rate to its end. A locate, the transport starting, a loop wrap and a new song re-seek: the clips
underway start from the frame the position is inside. A warped clip is its finished, stretched buffer, so
it keeps the stretch of the song's own tempo; it is not stretched to the followed one. Up to four clips of
a track play at once; streamed assets are silent while following. The state is a small array in the
track's RT scratch: nothing allocates. Not done: stretching clips to the followed tempo, and a re-sync
inside a clip that runs for many bars (it drifts by the share the two tempos differ).

**Master keyboard.** Workflow, **Master keyboard**
 (`MasterKeyboard`, in `workflow.json`): in
Standard mode the live keyboard follows the selected track; in Separated mode it plays the track
chosen with the arrow on its arrangement header (`AppCore::master_keyboard_track`, session state)
until the arrow is clicked again. `notes_target` reads that track first. **Manual Rec**
(`sequencer.manual_rec`, the MAN REC button in the arrangement corner) stops selecting a track from
arming it, and a track created while it is on is not armed. The corner's M and S buttons
(`sequencer.clear_mutes`, `sequencer.clear_solos`) clear every mute or solo in one undo step. None
of this is saved in the project.

## 5. The engine's console features

[crates/engine/src/console.rs](../crates/engine/src/console.rs), with the devices `atomik.channel` and `atomik.master`.

- **Stages.** A device may declare stages (`DeviceDescriptor::stages`, the stage of each socket).
  Insert and FX-return loops such as `channel.insert_send → effect → channel.insert_return` are
  device-level cycles, which HI-3 would reject. With stages the cycle check, render order, worker
  levels and delay compensation use (device, stage) nodes, with an implicit edge from stage `s` to
  `s + 1`. `Device::process_stage(stage, ctx)` defaults to `process` for stage 0.
- **Feedback routing.** `atomik.feedback` (Feedback Route, [feedback.rs](../crates/devices/src/feedback.rs))
  puts its outputs in stage 0 and its inputs in stage 1, so a loop through it is a legal (device, stage)
  order: outputs first, everything downstream of them, the inputs last. It is the only way a loop is
  accepted, by `Command::Connect`, by the project reader and by the engine alike, inside a container as
  well (the check runs on the flattened rack). The delay is a `DelayLine` per side read `delay - n` frames
  back for frame `n` of the batch, with the delay at least `BATCH` frames, so the samples that come back
  were written by earlier batches whatever the host block size and however the batches are spread over
  workers: a bounce is bit-identical at every block size and worker count
  (`crates/render/tests/feedback.rs`). It reports no latency, so the delay compensation leaves a loop
  alone, and what enters the line is clamped to plus or minus 16 so a loop with gain stays finite.
- **Summing inputs.** A socket with `Direction::SummingInput` accepts many cables. Contributions are
  summed as a left fold in ascending source-endpoint order, fixed at routing compile, so the sum is
  bit-identical for any block size and worker count.
- **Plugin delay compensation (PDC).** At routing compile, in topological order, a device's output
  latency is the maximum over its inputs plus its own `latency_frames()`. Each cable into an Audio
  or Cv summing input is delayed by `max_in − lat_in`. Delay lines are allocated at compile, sized
  exactly, owned by the routing plan and dropped off the RT thread; a routing swap keeps the line of
  a cable whose delay did not change. If a delay would exceed 10 s, compensation is disabled for the
  plan and `PdcReport::limit_exceeded` says so. `set_compensation(false)` makes every delay 0.
  **Return alignment** (the console's `align_returns` project flag, `Graph::set_align_inputs`; off
  for a project saved before format 37, on in a new one) extends the rule to every audio cable of a
  stage: plain inputs too (FX returns, sidechain keys, the inputs of a device that two paths of
  different latency feed), so each lands with the slowest cable of its stage and the dry strips are
  delayed to meet a latent send's return. The delay lines are allocated at compile like the others.
  With the flag off the plan is the one it always was, and `PdcReport::misaligned` lists each audio
  cable that reaches its device earlier than a sibling and is not delayed to match, with the frames
  it is early by (the status line shows the count); `PdcReport::output_latency` is the latency of
  the slowest signal reaching the outputs, and with the flag on the metronome goes through a delay
  line of that length (`ConsoleRouting::click_delay`) so the click lands with the audio.
- **Meters.** `Meters` is a fixed bank of atomics. After each batch the engine publishes peak and
  RMS per metered endpoint; the UI reads them once per frame.
- **Device meter values.** A device that answers `Device::has_meter_values()` with true when it is
  inserted gets `Device::meter_values(&self, out)` called by the audio thread once per batch, after
  the routing ran (every node is home), in ascending device id order. It writes up to
  `MAX_METER_VALUES` (64) latest values (gain reduction, loudness, band energies; peak-hold is the
  device's business). The engine stores them as f32 bits in the arena slot's bank inside `Meters`
  with a count, an owner id and a publish sequence that only grows; any other device costs one bool
  test per batch. The UI reads with `EngineHandle::device_meter_values(id, out)` or
  `Meters::device_values` (`AppCore::device_meter_values(key, out)`), getting `(count, seq)`; an
  unchanged `seq` means no news. A removed device (or a swapped plan) clears its slot, and a reader
  only gets values from the slot whose owner is the id it asked for, so a reused slot never shows
  old values.
- **Model and app.** The model keeps the console (`Console`, channels bound to rack devices, master,
  8 returns, VCA groups); every instrument gets a channel automatically. Solo is additive and
  returns are solo-safe; the app sets `internal.solo_silence` and the `internal.vca_*` feed on the
  engine only.

### Output buses (model)

[crates/model/src/console_bus.rs](../crates/model/src/console_bus.rs), with the device type
`atomik.bus` (`console::BUS_TYPE`; the model never depends on the device crate). `is_console_type`
is true for channel, bus and master, so a bus is left out of the device browser, cannot be copied,
pasted, a channel source, a return or insert effect, or sit inside a container, and is excluded like
a channel wherever console devices are skipped (sidechain sources, clipboard).

- **A bus is a strip.** `Console.buses: Vec<ConsoleBus { bus, name }>` lists the bus devices in
  console order (between the channels and the master in the console view). A bus has the channel's
  params and sockets except `in_l/in_r`, which become the summing input `bus_l`/`bus_r`. Invariants
  (loader and commands): every entry is an existing `atomik.bus` device listed once, with a trimmed,
  non-empty name of at most 32 characters, unique ignoring case (the device carries the same name),
  and buses exist only while there is a master. Project format 12 writes
  `"console"."buses": [{"bus", "name"}]` only when there are buses.
- **Routing is cables.** A strip's `out_l/out_r` go to one destination (`OutputDest`): `Master`,
  `Bus(key)` (cables to that bus's `bus_l/bus_r`) or `AudioIo(n)` (`out_l` to `interface:out_{2n-1}`
  and `out_r` to `out_{2n}`; pair 1 is outputs 1+2). Interface outputs take one cable, so a pair has
  one source; the master already owns 1+2. The sends of channels and buses always go to the master's
  `send_bus{k}`. `link_kind` classifies channel to master (`ChannelOut`), channel to bus
  (`ChannelToBus`), bus to master (`BusOut`), bus to bus (`BusToBus`) and the sends (`ChannelSend`,
  `BusSend`) as console links, hidden in the rack; Audio I/O cables are ordinary cables.
  `output_dest` reads `Master`, `Bus`, `AudioIo`, or `Direct`/`Mixed`/`None` as before; a channel
  hand-cabled to a free interface pair now reads `AudioIo` instead of `Direct`.
- **Commands** (`ConsoleCommand`, each one undo step, planned on a scratch copy):
  `CreateBus { name, dest }` (the device goes after the last bus, else at the end of the rack; its
  key is `console::new_bus_key(doc)` computed before; sends cabled to the master; the master is
  created when missing), `RenameBus`, `DeleteBus`, `ReorderBus`, `SetOutput { strip, dest }`
  (replaces every cable leaving the strip's `out_l/out_r`; refuses a loop, a bus that is not a bus,
  an Audio I/O pair beyond `output_channels` or already fed, and `Direct`/`Mixed`/`None`) and
  `RouteToNewBus { strips, name }` (a new bus on the master and the strips routed to it). Deleting a
  bus moves each feeder cable to the bus's own destination: the master, a bus, or its Audio I/O pair
  when it has exactly one feeder (a pair takes one cable), the master otherwise.
  `Command::RemoveDevice` and the workflow removal of a bus device are `DeleteBus`; the generic
  `Command::Rename` of a bus device is `RenameBus`. A bus has the channel's insert chain
  (`SetInsert`, `MoveInsert`, `CreateEffect` on a selected bus). Console devices are not copied:
  duplicating or pasting an instrument gives the new channel the master as output, whatever the
  original channel did, and a bus loses a feeder cleanly when its channel or instrument is removed.
- **Solo.** `RackDoc::solo_silence` returns the channels in console order, then the buses. With no
  solo nothing is silenced; otherwise a strip stays audible when it is soloed, when it is on the
  path of a soloed strip to the master or an Audio I/O pair, or when it feeds a soloed bus, both
  transitively; the siblings feeding a bus that is only on a soloed channel's path stay silenced.
  The app pushes `internal.solo_silence` to each strip by its own device type.

### Mix channels and parallel channels

[crates/model/src/console_mix.rs](../crates/model/src/console_mix.rs). Every command is one undo step,
planned on a scratch copy.

- **Mix channel.** `AddMixChannel { name }` makes an `atomik.channel` with no `source` (the
  loader always allowed that), wired to the master and its sends like any channel, after the last
  channel in the rack and folded; an empty name gets the default. Its `in_l/in_r` are the Direct In:
  cable anything there. `RemoveChannel` removes it with its cables; the app's Delete Channel (key
  and strip menu) is `WorkflowCommand::RemoveDevices` on the device behind each strip, so an
  instrument's strip goes with the instrument and its track, which asks first when the track holds
  clips.
- **Strip clipboard.** Copy, Cut, Paste and Duplicate in the console are app-core work
  ([crates/app/src/app_core_strips.rs](../crates/app/src/app_core_strips.rs)), with no model command of
  their own. A `StripSnapshot` holds a `DeviceSnapshot` (taken with tracks) of the devices that feed the
  selected channels plus each channel's `ChannelSettingsClip` of every section, because a device
  snapshot leaves console devices out and a pasted instrument gets a fresh channel. A paste is
  `PasteDevices`, then `MoveChannel` to put the new channels after the selected strip and
  `PasteSettings` onto each, the later two joined into the paste's undo step. A bus and a channel with
  no source have nothing behind them: they are copied as a name and a settings clip (`LooseStrip`), and
  the paste makes a new mixer channel (`AddMixChannel`) or a bus on the master (`CreateBus`, a free
  name such as `Drums 2` when the name is taken) before pasting the settings. The master is not
  copied. The strip context menu has Cut, Copy, Paste and Duplicate rows for the same commands.
  Ctrl-drag of a name plate emits `UiAction::CopyStrips`, which runs the same paste at the drop
  position. A chain holds up to `INSERT_SLOTS` (16) effects; the
  console draws four rows and scrolls the rest.
- **Parallel channel.** Every channel and bus has `parallel_out_l/r`, the signal after the width,
  before the fader and the pan, with the mute. `CreateParallel { strip }` makes a source-less channel
  named `P<n>: <strip name>` fed from those sockets and routed where the strip is routed (the master,
  or the bus the strip plays into). `Console::parallels` holds `(parent, child)` pairs in creation
  order (format 39, written only when there are some). A rename of the strip renames the copies whose
  name still has that form; removing the strip (as a channel, a bus, or through its instrument)
  removes its copies, theirs too; a soloed strip leaves its copies audible (`solo_silence` treats the
  path of a copy like the path of a bus). A copy can have copies of its own. Latency is aligned
  by the engine at the master's summing input like any two paths.
- **Send follow.** The console flag `send_follow` (format 39, written only when on; on in a new
  project, off for a file saved before it) keeps a hidden console link `LinkKind::FollowLink`,
  `bus:follow_out -> strip:follow_in`, for every strip that plays into a bus; `SetOutput`,
  `CreateBus`, `RouteToNewBus` and `DeleteBus` keep the links and `SetSendFollow` makes or removes
  them all in one step. The link is a control cable (no delay compensation, no early-cable
  warning). The bus writes its gain, fader times mute times the gain of the bus above, in a stage of
  its own ahead of its audio stages, so the link closes no loop; the strip multiplies the post-fader
  part of its sends by it, sample by sample. Pre-fader sends and the main output are untouched, and a
  strip without the link is bit for bit the old strip, so the flag off reproduces the old bounce.
  See the bus section of the device reference for the timing.

### Output buses (console, rack and app)

- **Model helper.** `console::output_options(doc, strip)` lists the destinations a strip's selector
  offers (master, each bus except the strip itself, every Audio I/O pair up to the format maximum),
  each with an `OutputAvail`: `Available` holds exactly when `SetOutput` would accept it, else
  `Loop`, `InUse` (a pair already fed by another source; pair 1 is the master's) or `BeyondOutputs`.
  A generic `AddDevice` of `atomik.bus` is refused: a bus is made by `CreateBus` or `RouteToNewBus`
  only.
- **Console strips.** `ConsoleView` holds the channel strips first, then the bus strips
  (`console.buses` order), all drawn by the same layout and painter (the bus descriptor has the
  channel's params, so `strip_layout` gives the same controls); the "add channel" placeholder keeps
  its place after the last channel, which puts a one-slot gap between the two groups, and a bus
  strip sits on a faintly tinted background with the caption `BUS n` and its name on the plate.
  Reordering a name plate by drag stays inside its group (`MoveChannel` among channels, `ReorderBus`
  among buses). A click on a plate selects the strip (Ctrl/Cmd toggles, Shift selects the range);
  the selection is view state, outlined on the plate, and what Ctrl/Cmd+G uses when the console has
  the focus. Mute, solo and the meters work as on a channel: the view's silenced flag comes from
  `RackDoc::solo_silence`, so a bus on a soloed channel's path and the feeders of a soloed bus read
  as audible.
- **Output selector.** Every strip has a slim selector row (`console_layout::OUT_H`) above its name
  plate; the master's reads `MAIN 1+2` in the same place so the faders line up. A click opens a
  menu: Master, each bus by name (loops left out), the Audio I/O pairs (greyed with the reason:
  `(the master)`, `(in use by <strip>)`, and one row
  `Audio I/O 5+6 and up: raise Outputs in Audio settings` for the pairs beyond `output_channels`)
  and `New bus...`, which sends `RouteToNewBus` with `default_bus_name()`. The menu marks the
  current destination, and a long one scrolls with the wheel. The selector label reads `Master`,
  `Bus <name>`, `Audio I/O 3+4`, `Custom` (hand-cabled), `Mixed` or `none`.
- **Rename and menu.** A double-click on a bus name edits it in place (the console is a text owner
  then: `ConsoleView::is_text_owner`, the shell puts `Text` on the context stack); Enter or a click
  elsewhere sends `RenameBus`, Esc cancels, at most 32 characters. A refusal (empty or duplicate
  name) is shown in the toolbar (`ConsoleView::notify_rejected`, cleared by the next click) and in
  the rack, like any rejected command. The right-click menu of a bus plate is Rename and Delete bus;
  a channel plate still removes the channel on right-click.
- **Route to new output bus.** `mixer.route_to_new_bus` (Ctrl/Cmd+G, Mixer menu):
  `AppCore::strips_to_route` takes the console's selection when the console has the focus and it is
  not empty, else the rack's selection (a channel or bus device is itself, an instrument is its
  channel), and `route_selection_to_new_bus` applies one `RouteToNewBus` named `Bus n`, then selects
  the new bus in the console. With nothing selected it says so instead.
- **Rack.** A bus is a console device like a channel: it is always in the rack, its cables are
  console links hidden unless "Show Console Links" is on, and its panel is the mix channel panel
  (the channel's panel, with `bus_l/bus_r` as the summing input, see **Mix channel panel**). The
  output display on the plate reads `Output: Master`, `Output: Bus <name>` or
  `Output: Audio I/O 3+4`; only a hand-cabled output that is none of these (`Direct`, `Mixed`) keeps
  the warning colour and the "Leaves the master" tooltip.
- **Mix channel panel** ([ui/src/mix_panel.rs](../crates/ui/src/mix_panel.rs),
  [rack/mix_input.rs](../crates/ui/src/rack/mix_input.rs)). Channels, buses and the master are always in the
  rack (the switch "Show Console Devices in Rack" and its command are gone; an older `ui-state.json` that still holds
  the key loads and the key is ignored). A channel or a bus draws as a hand-made panel, one rack unit
  (`mix_layout`), instead of the generated layout of its descriptor. Front: MUTE and SOLO (the `mute` and `solo`
  params), a horizontal fader (the `fader` param: a drag along the track, a double-click resets it, the readout is
  the gain in dB), a PAN knob, an L and R peak meter, BYPASS INSERT FX, SHOW INSERT FX and the AUDIO OUTPUT display
  (`Master Section`, `Bus <name>` or `Audio I/O 3+4`, the words of the plate's output label from `displays_of`; a
  folded strip keeps the label on its plate). The controls are ordinary `Hit::Control`s, so a press sets the very
  `SetParam` the console strip sets and both views follow each other through the document. BYPASS INSERT FX is the
  console strip's B-all: one `Command::Batch` from `bypass_inserts_commands` (one undo step), or the strip's own
  `insert.bypass` while there is no chain. The plate shows the name of the source instrument, as the console strip
  does, and a rename changes that instrument (a bus or a source-less channel shows and renames its own name). The
  meters are fed once per frame by `AppCore::update_console_meters` (also while the console is hidden, as long as a
  strip panel is open: `RackView::shows_mix_panels`) through `RackView::set_mix_meters`, in steps of half a dB, so a
  quiet strip stops repainting. Back: only four pairs, INPUT (`in_l`/`in_r`, or `bus_l`/`bus_r` on a bus), INSERT FX
  TO DEVICE (`insert_send_l/r`), INSERT FX FROM DEVICE (`insert_return_l/r`) and OUTPUT (`out_l`/`out_r`), with short
  labels (`socket_label`). Every other socket (the eight sends, the parallel output, the key, the CV inputs, the gain
  reduction and follow sockets) has no jack in the rack: they stay in the model, keep their cables (the sends stay
  wired to the master) and stay editable in the console. SHOW INSERT FX is view state: `RackView::inserts_shown`
  and `set_inserts_shown` keep a set of the devices whose switch is off (every device starts shown; the set is not
  saved with the project and is pruned with the devices). A channel's panel goes right under its source
  instrument (`AddChannel`, `AddChannelFor` and the create chain insert it after the source); a channel with no
  source goes after the last channel, and its insert effects sit right under it.
- **Insert stack** ([rack/insert_area.rs](../crates/ui/src/rack/insert_area.rs)). The effects of a channel's or
  bus's insert chain (`insert_chain`, derived from cables) are drawn stacked right under its panel, in chain order, in
  its column, inside a frame in the strip's accent colour (an indented frame like the inside of an unfolded
  container). The frame ends with a drop row ("drop or create an effect here"); with SHOW INSERT FX off the stack
  is left out of the layout and the frame is one header line that names the effects. `RackView::shown` lists the
  stack after its strip whatever the document order is (`stacks`: the chain up to the first effect in another column),
  `rebuild` adds the room of the header line or the drop row (`area_gaps`) and `compute_areas` records the frames
  (`RackView::insert_areas`, `InsertAreaInfo`). `sync` lays the rack out again when a stack changes, also one that is
  hidden (`areas_current`). The model keeps the stack in the document as well: every insert command ends with
  `console_insert::stack`, one `Edit::Layout` that puts the chain right under the strip in chain order, and a
  move of a channel or bus (`rack_groups::expand_moves`) takes its inserts along in every sort mode and never lands
  between the members of another strip's stack (a routed single effect may, which is how it joins a chain there).
  New commands: `ConsoleCommand::AddInsert { channel, slot, effect }` inserts at a position and shifts the later
  effects down (an existing effect of another chain moves over in the same step), and
  `ConsoleCommand::ExtractInsert { channel, slot, to }` takes an effect out, closes the chain behind it and leaves
  the effect uncabled in the rack (at the rack slot `to` when given). `CreateEffect` with an insert selected, and
  `CreateEffectAt` at a slot under a strip or one of its inserts, put the new effect in the chain at that position
  (`stacked_position`); a plain create with the strip selected still appends. Gestures, each one undo step: a rack
  device dragged into the frame is `AddInsert` at the place under the pointer (`MoveInsert` when it already is an
  insert of that strip), a browser row dropped there is `CreateEffectAt`, an insert dragged out of the frame is
  `ExtractInsert`, Delete on an insert is `DeleteDevices` (its neighbours are bridged), a drag of the strip moves it
  with its stack, a click on the header line shows the stack and one on the drop row selects the strip (so that
  creating appends). The console's insert rows reorder by drag (`MoveInsert`) and carry an E that emits
  `UiAction::EditInsert`, which `AppCore` answers with `RackView::reveal_insert` (the area unfolds, the effect is
  selected and the rack scrolls to it). The fold state stays view state (see above): it is not saved with the
  project, so a project opens with every insert area shown.
- **Master section panel** ([ui/src/master_panel.rs](../crates/ui/src/master_panel.rs),
  [rack/master_input.rs](../crates/ui/src/rack/master_input.rs)). The master draws as a hand-made four-unit panel
  (`master_layout`) that shares the top row of the mix channel panel (`mix_panel::has_mix_top`: the horizontal
  `fader`, the meter fed by `set_mix_meters`, BYPASS INSERT FX, SHOW INSERT FX and the output display) and adds eight
  send rows (`return_row`): the number, the name of the effect on the return slot (`master_data`, from
  `Console::returns`), the `return{k}.level` param as a horizontal slider (`mix_travel` gives the track of the
  fader being dragged) and an EDIT button (`edit_rects`, `Hit::ReturnEdit`). The back draws `fx_send{k}_l/r`,
  `return{k}_l/r`, the insert loop, `main_l/r` and `monitor_l/r` (`socket_label`); the other sockets have no
  jack. The bus compressor, the control room source and the key input are console view only. The master has the
  insert area of a channel (`has_mix_top` in `insert_area.rs`), with the master as the owner of `SetInsert`,
  `AddInsert`, `MoveInsert` and `ExtractInsert`; `insert_owner` includes it, so the console's E button and
  `reveal_insert` work for it. The shared send effects are grouped by `console_insert::send_chain` (the effects
  cabled in series from `fx_send{k}`) and `master_stack` (the master's inserts, then the effects of sends 1 to 8 that
  sit in the master's column): `RackView::send_stacks` lists each send's effects after the master's stack,
  `area_gaps` adds a header line above each group and `compute_areas` records the groups
  (`RackView::send_groups`, `SendGroupInfo`), drawn as a frame labelled "Send N". The groups have no drop row. In the
  model `console_insert::stack` on the master gathers its inserts and then the send effects right after it, a
  move of the master takes `master_stack` along and a move never lands inside it (`rack_groups`), a new send goes
  after the stack (`send_effect_index`) and `AssignSend` moves the effect there. `CreateTarget::Selected(master)`
  keeps the send rule (a ninth effect is added unrouted with the "All 8 send and return slots are in use" note);
  `CreateTarget::MasterInserts` (what `RackView::effect_create_target` answers after a click on the master's drop
  row, cleared by any selection change), `CreateEffect` after an insert of the master and `CreateEffectAt` right
  under the master panel make a master insert. The fold state is view state, as for the channels.
- **Meters.** The topology meters the `out_l`, `out_r` and `gr_cv_out` of every bus like a channel's
  (`CHANNEL_METER_SOCKETS`), so the console draws bus meters, gain reduction and latency readouts.
- **Colours.** A bus strip takes the colour of its device when it has one (a click on the colour band
  of its name plate opens the picker, `ColorTarget::Devices`), else the default bus colour. The output
  selector of a strip that plays into a bus shows a swatch in that bus's colour, and so do the bus
  rows of the selector's menu.
- **Meter modes and peak reset.** Mixer > Meter Mode (`mixer.meter_mode`, kept in `ui-state.json` as
  `meter_mode`, peak by default) picks the ballistics the strip meters draw: peak (instant attack, 24 dB/s
  release), PPM (the peak bars and their hold fall 24 dB in 2.8 s) or VU (the RMS bars integrate over
  300 ms, 99 % of a steady level in that time, rising and falling alike); the maths is in
  [crates/ui/src/console_meter.rs](../crates/ui/src/console_meter.rs). The master's L and R peak
  readouts show the highest peak until it is reset: a click on either of them drops the readouts, the
  holds and the clip lamps of every meter.
- **Total latency.** The toolbar's `LATENCY` readout is the largest strip latency; when the master's
  inserts or look-ahead add more, `TOTAL` follows with the latency of the whole mix up to the
  interface outputs (`PdcReport::output_latency`).
- **Master release Auto.** `comp.auto` (appended after `internal.tap_live`, off by default, and
  an AUTO button beside the release knob) makes the bus compressor's release follow the program: a slow
  average of the gain reduction blends it between 60 ms (brief peaks) and 1.2 s (sustained reduction of
  6 dB or more); `comp.release` is not used while it is on (`Compressor::set_release_auto`).

### Mixer groups: VCA faders

[crates/model/src/console_vca.rs](../crates/model/src/console_vca.rs), the VCA strips of
[crates/ui/src/console_vca.rs](../crates/ui/src/console_vca.rs), the three `internal.vca_*` params
of the channel strip. A **VCA group** is one fader, one mute and one solo over several console
strips (channels and buses). It has no audio of its own and no cables: it is console state
(`Console::vcas`), so the routing, the plan and the bounce of a project without groups are
untouched. It is not a track folder (folders organise the arrangement and change nothing in the
mix). "Create VCA from Selection" with a folder selected makes a group of the channels of its
tracks, and a group can also **follow** a folder (below).

- **Model.** `VcaGroup { id, name, color, level_db, mute, solo, members, follows }`: ids count from 1
  (`max + 1`), a name is trimmed, 1 to 32 characters and unique ignoring case, at most 64 groups,
  `level_db` within `VCA_MIN_DB..=VCA_MAX_DB` (-60 to +12,
  [atomik-core](../crates/core/src/param.rs)); a new group takes the colour `VCA_PALETTE` holds for
  its id and it can be cleared. A strip is in **at most one group** (assigning moves it), members
  are kept in console order, and a strip that is removed (channel or bus, by any command) leaves its
  group in the same undo step (the planner takes it out first, so undo puts the strip back and then
  its membership). Commands (`ConsoleCommand`): `CreateVca`, `RenameVca`, `SetVcaColor`,
  `SetVcaLevel` (with a drag merge id), `SetVcaMute`, `SetVcaSolo`,
  `AssignToVca { vca: Some | None, strips }` (a command that changes nothing is refused) and
  `DeleteVca`; each is one undo step on one `Edit::ConsoleVcas` (the whole list swaps) that reports
  `ConsoleChanged`; `ClearSolos` and `ClearMutes` also clear the groups' flags in their one step.
  The level is not an automation target (a group is not a device; see the follow-ups in the
  roadmap).
- **Following a folder.** `follows: Option<u32>` is the id of a track folder: the members are then
  the console channels of the folder's tracks, sub-folders included (`RackDoc::folder_strips`), and
  `follow_sync` brings them in line after **every** command, inside the same undo step as the command
  that moved a track in or out, added or deleted one, or dissolved the folder (the edit joins the
  command's `Edit::Compound`, so one undo takes both back). A strip of the folder joins the group
  even from another group; where two followed folders are nested the later group takes the strips
  they share. `CreateVcaForFolder { folder, name }` makes the group and `SetVcaFollow { vca, folder }`
  turns following on or off (off keeps the members); one group follows a given folder. The members
  of a following group are not edited by hand: `AssignToVca` into it or out of it, and `CreateVca`
  over its members, are refused with a message saying to unfollow first. A group whose folder is
  dissolved or loses its last track (a folder with no track is not saved) stops following and keeps
  its members. The Mixer menu's Create VCA from Folder (and the folder row's context menu) names the
  group after the folder; the VCA strip's menu has Stop following folder.
- **Gain.** A member plays at **its own fader plus the group's offset**: the offset is a multiplier
  on the fader gain (`atomik_dsp::fader_gain_vca`), so with a member at -3 dB and the group at -6 dB
  the strip is at -9 dB, and the member's own fader keeps its position. The bottom of the range (-60
  dB) is off; exactly 0 dB is the plain fader bit for bit. `RackDoc::effective_fader_db` is that sum
  for the UI and the MCP inspection.
- **Mute and solo.** A member's *effective* flags are its own or its group's:
  `effective_mute = mute || group.mute`, `effective_solo = solo || group.solo`, and the strip's rule
  is unchanged: **solo beats mute** (a member of a soloed group plays even if it or the group is
  muted, as a soloed strip ignores its own mute), and with any effective solo
  `RackDoc::solo_silence` silences every strip that is neither soloed nor on the path of a soloed
  strip (the additive rule of the buses). Members stay individually controllable: their own M and S
  still work and combine by the rule above (an own mute under a group that is neither muted nor
  soloed mutes the strip; an own solo adds the strip to the audible set). A soloed group with no
  members soloes nothing. The channel strip applies it:
  `muted = (mute || vca_mute) && !(solo || vca_solo)`, silenced by `internal.solo_silence` as
  before.
- **Engine.** `RackDoc::vca_feeds` is the state of every strip (`VcaFeed { db, mute, solo }`, the
  default for a strip outside a group). The app sends it through `push_vca_feeds` to the strips'
  `internal.vca_db`, `internal.vca_mute` and `internal.vca_solo` (engine only, like
  `internal.solo_silence`: not stored, saved or undoable, hidden from the generated rack panels)
  whenever the console changes and after an engine rebuild, and only what changed since the last
  push; a strip that is added or removed forgets what it was sent. `internal.vca_db` moves the fader
  target through the same 5 ms smoother as the fader, so there is no zipper noise, and the fader CV
  rides on top of it. A bounce sets the non-default feeds in `console_options`, so it plays what the
  console plays. Nothing runs when no group exists: the three params stay at their defaults and the
  strip is bit-identical to one before the feature (`crates/render/tests/vca.rs`).
- **File.** Project format 26:
  `"console"."vcas": [{"id", "name", "color"?, "level_db"?, "mute"?, "solo"?, "members"}]`, omitted
  when empty (a project without groups differs from its version 25 file in the version line alone)
  and refused before version 26. The loader names the JSON path of a duplicate id or name, an
  out-of-range level, a member that is no console strip or is in two groups; members may come in any
  order and load in console order. `"follows"` (a folder id, written only when set) is an optional
  field of format 43; a reader that does not know it ignores it and keeps the members, and a file
  that names a folder the song does not have loads as an ordinary group.
- **Console view.** The VCA strips follow the buses, one slot each, with the strip height: a plate
  filled with the group's colour (name, `VCA n`, member count), M and S, the offset readout and a
  tall fader. A click on the plate selects the members, a double-click renames, a right-click opens
  Rename, Select members, a colour and Delete VCA; the fader drags with a merge id (a double-click
  resets to 0 dB, the wheel steps, Shift is fine). A right-click on a channel or bus plate offers
  New VCA..., Add to VCA `name` for each group that would take it and Remove from VCA, acting on the
  whole selection when the strip is in it; dragging a name plate onto a VCA strip assigns it (and
  the selection, when the strip is in it). A member's plate carries the group's colour as a chip,
  its fader keeps its own position with a marker where it plays, and its readout shows that level.
  The arrangement draws a slim bar in the group's colour at the right edge of the header of a track
  whose channel is in a group; a track header (or several selected ones) dragged onto the bar of
  another track in a group joins that group when released there (`AssignToVca`; the bars light up
  and the insertion line is hidden while it hovers). **Link Faders** (Mixer menu, kept in
  `ui-state.json`, not in the project): while on, a fader or send level of a strip in a group that is
  not part of a multiple selection moves the levels of every member of its group by the same dB
  (`console_gang`, one `Batch` and one undo step per drag, the offsets between the strips kept); a
  multiple selection still takes its own strips along.
- **Level lane.** A group's level can be automated. The lane is an ordinary automation lane on the track of the
  master (`vca_lane_key` names it `vca.<id>`, `VCA_LANE_PARAM` is its range in dB, and `lane_param` is the one lookup
  every lane check goes through, so a group lane is accepted wherever a param lane is). `AddAutomationLane` needs
  the master and an existing group, `DeleteVca` removes the lane in the same step (`remove_lane_edit`; the id of a
  deleted group is given to the next one), and the loader and `RackDoc::validate` refuse a lane of a missing group.
  The song input expands the lane into the `internal.vca_db` lane of every member (`RackDoc::vca_lane_targets`,
  the way a macro bank's lane plays on its targets), so a bounce and the live song play the same, and the app builds
  the song again when the members of a group with a lane change (`RackDoc::vca_lanes`). A group fader move is
  offered to the automation take as a move of the `vca.<id>` param (`AppCore::apply_command`), the engine hold of a
  written lane is the members' `internal.vca_db`, `hold_on_mute` batches `SetVcaLevel` and the console fader shows
  the lane's value through `view_with_shown`. The console's group menu has a Show and a Remove row for each lane,
  the fader carries the automation mark, and the arrangement labels the lane "Group N level".
- **Mute and solo lanes.** A group's mute and solo are lanes of the same track, `vca.<id>.mute` and `vca.<id>.solo`
  (`VcaLaneKind` names the three kinds, `vca_lane_parts` splits a key, `vca_lane_key_of` builds one; `vca_lane_id`
  still means the level lane only). Their values are 0 or 1 (a value from 0.5 up is on, and the points are clamped
  to 0..1). They expand into the `internal.vca_mute` and `internal.vca_solo` lane of every member
  (`RackDoc::vca_kind_targets`), `DeleteVca` removes every lane of the group in its step, and the loader and
  `RackDoc::validate` refuse a lane of a missing group. A move of the group's M or S button is offered to the
  automation take as a move of the lane's param, a muted lane holds the flag it gave (`SetVcaMute` and `SetVcaSolo`
  can be batched like `SetVcaLevel`), and `view_with_shown` shows the lane's flag. The other strips' solo silence
  (`internal.solo_silence`) is derived from the stored flags only, so a solo lane never silences a strip outside the
  group: the same limit as the solo lane of a single channel.
- **Rack chip.** A device of the rack that is in a group wears its name on its plate, in the group's colour
  (`group_chip::chips_of` in the rack view): a channel or a bus that is a member and the instrument of an automatic
  channel. It sits above the output display of an unfolded plate and after the CPU readout of a folded one.
- **Commands.** `mixer.create_vca` (Create VCA from Selection), `mixer.add_to_vca`,
  `mixer.remove_from_vca` and `mixer.delete_vca` are registry commands in the Mixer menu, without a
  default key. They act on the strips of the focused pane (the console's selection; the
  arrangement's tracks through their channels; the rack's selection as for Route to New Output Bus);
  `mixer.add_to_vca` and `mixer.delete_vca` take the id of a group as their argument (otherwise the
  last group, and the groups of the selected strips). `session_command` takes it as `arg`, and
  `session_status` lists the groups under `console.vcas`. `mixer.create_vca_for_folder` (Create VCA
  from Folder) and `mixer.link_faders` (a check item) are in the Mixer menu too; once the song has
  groups, Add to Group and Delete Group submenus follow Delete VCA with a row that names each group
  (`AddToVca(Some(id))`, `DeleteVca(Some(id))`). The track header and folder row context menus
  have Create VCA (and the folder row Create VCA that Follows).

### Groups routed to a bus

[crates/model/src/console_group_bus.rs](../crates/model/src/console_group_bus.rs). A VCA group can play its members
into an output bus, the way a folder can (see the next section): `RouteVcaToBus { vca, bus }` and
`RouteVcaToNewBus { vca, name }` set the output of the member strips (channels and buses) to the bus with the
ordinary `SetOutput` cables, and `UnrouteVca { vca }` sends the members that still play into it back to the master
(the bus stays). The group itself keeps no audio: it stays a master fader, mute and solo over the members. The route
lives on the group (`VcaGroup::bus`, and `VcaGroup::routed`, the members the route has put on the bus), so a
project without a group bus is exactly what it was.

- **Following.** `console_group_bus::sync` runs after every command, in the same undo step (it joins the command's
  `Edit::Compound`, after the VCA follow): a strip that joins the group gets its output on the bus (a strip that
  would make a loop, or the bus itself, stays where it is), one that leaves or is removed goes back to the master if
  it still plays into the bus, and a route whose bus is deleted is forgotten with the cables left alone. A strip
  the route owns is never moved again, so an output chosen by hand for one member stays.
- **Saved.** Optional `bus` and `routed` fields of a group (`console.vcas[]`), written only when the group is
  routed, at project format 44 and `.ignorable()`. The loader refuses a bus that is not a console bus and a routed
  strip that is not a member; `RackDoc::validate` checks the same.
- **Commands.** `mixer.route_vca` (Route Group to New Bus) and `mixer.unroute_vca` (Stop Routing Group) are in the
  Mixer menu without a default key, with the id of a group as their argument (otherwise the groups of the selected
  strips); Route Group to Bus and Stop Routing Group submenus with a row per group follow Delete Group
  (`RouteVca(Some(id))`, `UnrouteVca(Some(id))`). The group plate menu has Route to New Bus and Stop Routing to
  Bus.

### Folders routed to a bus

[crates/model/src/console_folder.rs](../crates/model/src/console_folder.rs), the level bar in
[crates/ui/src/arrangement/folder_fader.rs](../crates/ui/src/arrangement/folder_fader.rs). A track folder can
play into an output bus: `RouteFolderToBus { folder, bus }` and `RouteFolderToNewBus { folder, name }` set the
output of the console channels of the folder's tracks (sub-folders included) to the bus with the ordinary
`SetOutput` cables, and `UnrouteFolder { folder }` sends the channels that still play into it back to the master
(the bus stays). The console keeps a `FolderRoute { folder, bus, members }` for each routed folder
(`Console::folder_routes`, at most 64, one per folder; several folders may share a bus), and `members` is the
channels the route has put on the bus.

- **Following.** `console_folder::sync` runs after every command, in the same undo step (it joins the command's
  `Edit::Compound`, like the VCA follow): a track that joins the folder gets its output on the bus and is added to
  `members`, one that leaves goes back to the master if it still plays into the bus, a route whose folder is
  dissolved (or empty) or whose bus is deleted is forgotten, and the cables stay. A channel the route already owns
  is never moved again, so an output chosen by hand for one member stays. Where a routed folder holds another
  routed folder, the inner tracks belong to the inner route.
- **Saved.** An optional `folder_routes` field of the console (`{"folder", "bus", "members"}`), written only when
  a folder is routed, at project format 44 and `.ignorable()`: the cables carry the routing, so a reader that
  drops the field plays the same mix. A route to a folder the song lacks is dropped on load.
- **Level bar.** A folder header that plays into a bus shows the bus fader as a thin bar along its bottom edge; a
  folder that only a VCA group follows shows the group's level on it (a folder with neither has no bar, so the
  other headers are unchanged). A press sets the level, a drag is one undo step (`SetParam` of the bus `fader` or
  `SetVcaLevel` with a merge id), Esc restores it and a double-click resets it to unity.
- **Commands.** `mixer.route_folder` (Route Folder to New Bus; with a bus number from 1 as its argument it picks
  that bus) and `mixer.unroute_folder` (Stop Routing Folder) are in the Mixer menu and the folder row's context
  menu, without a default key; once the song has a bus, a Route Folder to Bus submenu lists them
  (`RouteFolder(Some(n))`). They act on the folder selected in the arrangement.

### Channel strip upgrades

The input trim, the polarity, a parallel compressor and the insert bypass of a channel strip
([channel.rs](../crates/devices/src/channel.rs),
[console_insert.rs](../crates/model/src/console_insert.rs),
[plugin-host/src/device.rs](../crates/plugin-host/src/device.rs)). None of it adds a persisted
field: the strip's params are stored sparsely and a missing one is its default, so the project
format stays at 26 and a project that does not touch the controls is bit-identical to one saved
before (the golden hashes and `crates/devices/src/channel/tests.rs` prove it).

- **Input trim and polarity.** `input.gain` (-24 to +24 dB, default 0) and `input.phase` (0 or 1)
  are the first two params of the strip and its first stage: the gain is a multiplier on the input,
  the polarity a sign, both through one 5 ms smoother (a flip is a ramp through zero, not a click).
  They are ordinary params, so they are automatable, undoable and visible in the rack's channel
  panel (a knob with its dB value, a toggle) and in the console's INPUT section (the trim knob
  with its value in dB under it, Ø and Ø R buttons). `mixer.toggle_polarity` flips the
  selected strips. A strip with the polarity inverted and its normal twin cancel exactly.
- **Parallel compressor.** `pcomp.on`, `pcomp.threshold`, `pcomp.ratio`, `pcomp.attack`,
  `pcomp.release`, `pcomp.makeup` and `pcomp.mix` are appended after the existing params (no id
  moves). It sits in stage 1, after the insert return, the dynamics and the EQ and before the width,
  the fader, the pan and the sends (so the sends carry it): the tap is **post-inserts**. It is a
  second `atomik_dsp::Compressor`, the one of the strip's dynamics section (RMS detector, soft
  knee), followed by the makeup, and is blended with the dry signal,
  `out = dry (1 - mix) + compressed makeup mix`: mix 1 is the compressor alone (the test compares it
  with a standalone compressor render). It has no look-ahead, so it adds no latency and the dry and
  compressed paths are sample aligned; there is nothing to match and the strip reports no latency
  for it. The blend is `pcomp.mix` while `pcomp.on`, else 0, through the 5 ms smoother; when it is 0
  and settled the stage is skipped (no CPU) and the signal is untouched bit for bit; switching it on
  starts the compressor from a cleared state. The console's PARALLEL section (after DYNAMICS) holds
  the enable button and the threshold, ratio and mix knobs; the attack, release and makeup are in
  the rack's channel panel only (`console_layout::RACK_ONLY` keeps them out of MORE).
- **Insert bypass keeps the delay compensation.** A bypassed insert effect plays its input delayed
  by its own latency, so the strip stays on the timing the routing computed (section 5, PDC): a
  built-in effect through `Bypassable` (see 6c), a plugin through `PluginDevice` (a dry ring of 100
  ms written every sample and read `latency` frames late; the wet and the dry signal of the 5 ms
  crossfade are aligned, and with no latency the dry signal is the input bit for bit). The effect
  keeps running, so a toggle changes no timing and, for the same reason as for any bypass, clicks
  only through the crossfade. The strip's own `insert.bypass` replaces the insert return by the send
  and cannot know the latency of the chain (a stage is not told its input latency), so the console
  no longer uses it: its button, **BYP ALL**, sets the `bypass` of every effect on the chain in one
  `Command::Batch` (`atomik_model::bypass_inserts_commands`; one undo step; lit when every
  bypassable effect is bypassed, `inserts_bypassed`), and a **B** on each filled slot sets one
  effect. Without a chain the button is the strip's own switch as before, and a project that has
  `insert.bypass` set keeps it (reads as bypassed, cleared by the next click). An effect without a
  bypass is skipped by BYP ALL and has no B; a container on the chain has the switch of 6e.
- **Look-ahead.** `comp.lookahead` (0 to 10 ms, appended after `internal.tap_live`) puts a delay line of
  that length in the audio path of the dynamics section (gate and compressor) while the side-chain still
  reads the early signal, so the gain is already down when the peak leaves the line. `latency_frames` reports
  it with the phantom latency of a negative `latency.adjust`, so the routing delays the other channels
  by the same amount (section 5, PDC). The line runs whenever the look-ahead is above 0, with the section on,
  off or bypassed, so the strip's timing never depends on a switch; changing the amount re-reads the line from
  another point, a click like `latency.adjust`. At 0 nothing runs and the strip is bit-identical.
- **Polarity of each side.** `input.phase_r` is appended too and inverts the right side on top of `input.phase`,
  through a second input smoother, so old projects (where `input.phase` flipped both sides) read the same.
- **Section bypass.** `dyn.bypass` and `eq.bypass` set the dynamics or EQ section's own on/off crossfade to
  off while keeping its settings. `bypass_inserts_commands` adds them to the batch of the BYP ALL switch for a
  section in use (`section_bypass_states`: `comp.on` or `gate.on`, `eq.on`), `inserts_bypassed` reads them with
  the effects, and the console keeps them out of MORE (`console_layout::RACK_ONLY`); the master has none.
- **Console readouts.** The trim knob (`input.gain`) writes its value in dB at rest where the label goes,
  and the gain-reduction bar writes the reduction in dB while it moves. The parallel compressor's detector
  is `pcomp.peak` (a PEAK button in the PARALLEL section).
- **Key and tap of the parallel compressor.** `pcomp.key` and `pcomp.pre` are appended after `eq.bypass`. With
  `pcomp.key` set and `key_in` cabled the parallel compressor detects on the key (the copy `stage0` takes, the same
  one the dynamics use, not delayed by the inserts) instead of the signal it compresses; the dynamics section keeps
  its own `key.on`. With `pcomp.pre` the compressed copy is the stage-0 signal at the insert send (`mid`), blended
  into the signal after the inserts, so it does not pass through the chain and is aligned with the dry path only while
  the chain and `latency.adjust` add no delay. The console has a KEY button in the PARALLEL section; the tap switch is
  rack-only (`console_layout::RACK_ONLY`). A container in the chain is an effect with a `bypass` param (6e), so its
  **B** and BYP ALL work like any slot's.
- **Auto make-up and E mode.** `comp.auto_makeup` and `eq.e_mode` are appended after `pcomp.pre` (74 params; the
  bus has them too). With `comp.auto_makeup` the compressor's output is raised by `-threshold * (1 - 1/ratio)` dB,
  the reduction a full-scale signal gets, capped at 24 dB, through the 5 ms smoother; `gr_cv_out` and the meters
  read the reduction alone. With `eq.e_mode` every bell (HMF, LMF, and HF or LF in bell mode) runs at the stored Q
  times `1 + |gain| / 18` (`atomik_dsp::strip_eq::e_mode_q`), so a large boost or cut is narrower; shelves and a
  bell at 0 dB are unchanged and the drawn curve (`StripCurve::e_mode`) follows. Off, both are bit-identical to
  before. The console has an M-UP button under the compressor knobs and an E button beside the EQ switch.
- **Key auto-on.** A cable into `key_in` of a channel or a bus sets `key.on` in the same undo step (the master's
  `comp.key` already did); removing the cable leaves the switch.
- **Master level CV and selection.** The master has `fader_cv_in` (stage 2) and `fader_cv_trim` (appended after
  `comp.auto`): `trim * cv * 24` dB on the fader like a channel's, bit-identical while the input is not cabled. A
  click on the master's name plate selects it (`ConsoleView::master_selected`); the settings copy, paste, reset and
  clear rows then act on it. A master copy (`ChannelSettingsClip::master`) pastes onto the master only, and a
  channel or bus copy never onto it.
- **Commands and tools.** `mixer.toggle_polarity` and `mixer.bypass_all_inserts` are registry
  commands in the Mixer menu without default keys; they act on the strips of the focused pane like
  the VCA commands (`AppCore::strips_for_vca`), flip all inverted strips back or invert all, and
  bypass all or restore all, each one undo step. `session_status` lists the strips that use any of
  it under `console.strips`, and `session_set_param` sets a param of a device by key (docs/MCP.md).

### Mixer snapshots

[crates/model/src/console_snapshot.rs](../crates/model/src/console_snapshot.rs), the snapshot bar of
[crates/ui/src/console_snapshots.rs](../crates/ui/src/console_snapshots.rs),
[crates/app/src/app_core_snapshots.rs](../crates/app/src/app_core_snapshots.rs). A snapshot is
console state, like a VCA group: no device, no cable, no audio, so the routing, the plan and the
bounce of a project that has none are untouched.

- **Scope.** `MixerState` holds, per captured device, the params that are not at their default: for
  a console strip (channel, bus, master) every descriptor param but `internal.*` (read generically,
  so a param added to a strip later is captured with no change here), for an effect on a strip's
  insert chain or on an FX return only its `bypass` (every param but `internal.*` when
  `MixerState::effects` is set, per snapshot, by `SetSnapshotEffects`); and the VCA groups as `SnapshotVca` (level,
  mute, solo, members; the name and colour stay the group's own). Sparse means a param missing from
  a snapshot is its default, as in a device's params. It also holds the **wiring**
  ([crates/model/src/console_snapshot_wiring.rs](../crates/model/src/console_snapshot_wiring.rs),
  `MixerState::wiring`): for each strip the output (`Master`, a bus or an Audio I/O pair; a strip cabled by hand
  elsewhere holds none) and the effects of its insert chain in order. It does **not** hold the song (clips,
  automation, tempo, markers), the other cables or the rack structure (devices added or removed, the order and
  names of the strips). A snapshot also has a `note` and a `color` for its chip (`SetSnapshotNote`,
  `SetSnapshotColor`), and a console holds `MAX_SNAPSHOTS` (64).
- **Recall** (`RackDoc::recall_changes`, pure) lists the params whose effective value differs and
  pushes ordinary `Edit::Param` steps, plus one `Edit::ConsoleVcas` when the groups differ, in one
  compound undo step. The app therefore sends ordinary `ParamChanged` ops to the engine: no routing
  rebuild, and the strip's gains, pan, sends, parallel mix and effect bypass cross-fade through
  their 5 ms smoothers, so a recall mid-playback has no zipper noise. A `SnapshotScope` (bits for
  levels, mutes and solos, sends, insert bypass, processing and group membership) restricts it by
  param key (`category`; a `Routing` scope covers the wiring); recalling what already is there is refused by
  the model (the UI and the core do nothing instead).
- **Wiring recall.** `RackDoc::wiring_changes` (pure) lists the strips whose output differs and those whose
  chain differs, and `console_snapshot_wiring::apply` re-cables them inside the recall's compound step: the old
  chains come off their loops, each chain is cabled from the snapshot's members, then the outputs are routed
  (`set_output`). Each piece runs under `Planner::attempt`, so one that does not fit is skipped and the rest
  recalls: a member that is gone, on a return, feeding a channel, on the chain of a strip the snapshot does not
  hold or cabled elsewhere; a bus that is gone; a route that would loop; an Audio I/O pair another cable has
  taken. A snapshot with no wiring (`None`: an older file) leaves routing and chains alone. Cables change, so the
  engine gets a routing rebuild, not param messages. A morph flips the wiring at the half way point, as it does a
  switch.
- **Structural drift.** A device or group that no longer exists is skipped
  (`RackDoc::snapshot_notes` lists them, shown in the bar and in `session_status`); a strip, effect
  or group the snapshot never knew is left alone. A VCA group is matched by id and is not brought
  back once deleted; a strip the snapshot knew that was not a member of the group leaves it, a
  member that is gone is left out. `RackDoc::snapshot_matches` is the same plan asking whether it
  changes anything, which is what lights a chip.
- **Morph** (`ConsoleCommand::MorphSnapshots { from, to, at }`, `RackDoc::morph_state`): the state at
  position `at` between two snapshots, built per param from the descriptor (`morph_value`: a discrete param
  switches at the half way point, an exponential taper moves in its normalised scale, every other one linearly;
  position 0 and 1 are exactly the two snapshots), then planned as a recall with `SnapshotScope::ALL`. It is a
  pure function of the two snapshots and the position, so the app plays it by applying successive positions:
  `AppCore::snapshot_morph_tick` runs from `transport_frame`, skips a step that changes nothing
  (`RackDoc::morph_matches`), folds every step after the first into the first one's undo step (`join_next`)
  and ends when the clock reaches the morph time or when any other command is applied (`apply_command` clears
  it). The event loop keeps its frame rate while `snapshot_morphing()`.
- **Compare** (`Console::compare`, `CompareState { snapshot, other, on_snapshot }`, not saved):
  beginning stores the working mix in `other` and recalls the snapshot; a flip swaps the live mix
  with `other`. An edit made on one side is part of that side's state when it is flipped away, so
  toggling is lossless and the stored snapshot only changes with Update. Choosing another snapshot
  while comparing replaces the snapshot side (or the slot, from the working side); recall, deleting
  the compared snapshot and END leave the compare and keep the live side. `Edit::SnapshotCompare`
  makes begin, flip and end ordinary undo steps that restore the slot with the params.
- **File.** Project format 27:
  `"console"."snapshots": [{"id", "name", "devices": {key: {param: value}}, "vcas"?}]`, at most 64,
  omitted when empty (a project without snapshots differs from its version 26 file in the version
  line alone). The loader checks the shape, ids, names and numbers but not the references, since a
  snapshot may name what was deleted. `"effects": true`, `"note"` and `"color"` are optional and ignorable
  additions under format 43, written only when set; `"wiring": {strip: {"output"?, "inserts": [...]}}`
  (`output` is `master`, `bus:<key>` or `io:<pair>`) is one under format 44.
- **Snapshot lane** ([crates/model/src/console_snapshot_lane.rs](../crates/model/src/console_snapshot_lane.rs)):
  an automation lane of the master with the key `snapshot` (`SNAPSHOT_LANE_KEY`; `lane_param` gives the master
  its descriptor, a whole number 1 to 64), made by `AddAutomationLane`, so it is drawn in the arrangement and
  moves, copies and splits with its clips like any lane. Its stepped points name snapshots by id;
  `RackDoc::snapshot_lane_events` flattens them to `(tick, id)`, one per change of snapshot, and
  `AppCore::snapshot_marker_frame` recalls the one the playhead crossed (the later of a marker and a point wins).
  A build without the lane refuses a file that has one. The engine never sees the lane (the master has no such
  param) and a bounce does not recall. The arrangement
  draws it like a pattern lane, as blocks named after the snapshots (`ArrangementView::set_snapshot_names`) with
  a picker of the snapshots; Add or Remove Snapshot Lane is in the snapshot chip menu.
- **Console view and app.** The bar sits under the toolbar and moves the strips down by
  `SNAPSHOT_BAR_H`; chips that do not fit scroll (`console_snapshots::layout` takes the first visible
  chip; the wheel and two arrows move it, `reveal` brings the current snapshot's chip into view when it
  changes). The view derives the current snapshot
  (compared, else exactly matching, else the host's hint, the last stored, updated, recalled or
  compared one) and shows a dot when the mix has moved from it. Renaming uses the console's
  text-owner pattern. The Mixer menu commands act on the current snapshot, or on the one named by
  the argument (`session_command` takes an id or a name, `<snapshot>:<scope>` for a scoped recall, a
  name for Store).

### MIDI mappings and control surfaces

[crates/model/src/midi_map.rs](../crates/model/src/midi_map.rs) and
[midi_map_io.rs](../crates/model/src/midi_map_io.rs) (the data, the maths and the file),
[crates/app/src/app_core_midimap.rs](../crates/app/src/app_core_midimap.rs) and
[midi_control.rs](../crates/app/src/midi_control.rs) (the runtime),
[crates/midi-io/src/surface](../crates/midi-io/src/surface) and
[crates/app/src/surface.rs](../crates/app/src/surface.rs) with
[app_core_surface.rs](../crates/app/src/app_core_surface.rs) (the control surface),
[crates/ui/src/midi_maps_panel.rs](../crates/ui/src/midi_maps_panel.rs) (the panel). Neither a
mapping nor a surface is sound: both only decide which commands the hardware issues, so the routing,
the plan and the bounce of a project are untouched (`crates/render/tests/midi_maps.rs` proves a
project with mappings of every kind bounces bit for bit like the same project without them).

```
 MIDI callback (midir thread)                          UI thread (once per frame)
   parse_message ─► ControlHub inbox ───────────────────► midi_control_frame(now)
   hub.consumes(port, msg)?  (consume list, learn flag)      learn?  ─► Learner ─► MidiMapCommand::Learn
        yes: no note                                         else    ─► MidiMapping::feed ─► SetParam / SetVcaLevel /
        no : NoteForwarder ─► live note queue ─► audio                   SetTempo / a transport key  (apply_command)
   SysEx (mapping-role port) ─► parse_mmc ─► hub MMC queue ──► machine_control ─► transport_action
```

- **Parameter numbers and machine control.** [crates/model/src/midi_param.rs](../crates/model/src/midi_param.rs)
  holds `ParamParser` (the state of one port and channel: the selected NRPN or RPN, the data halves,
  a 7 bit sender spread over 14 bits until a low half is seen, the null RPN) and `ParamBank` (one
  parser per port and channel, bounded). The app feeds every message to the bank before the
  mappings: a completed run is a `ParamEvent` (`Value` of 14 bits or `Step`) that the mappings with
  an `Nrpn(n)` or `Rpn(n)` source hear through `MidiSource::matches_param` and
  `MidiMapping::feed_param`; the controllers of the run stay plain controllers for the other
  mappings, and the `Learner` keeps its own bank, learns the number when a run completes and never
  learns the selecting controllers. The hub's consume rule treats the controllers of a run as part
  of a consuming parameter mapping. [crates/midi-io/src/mmc.rs](../crates/midi-io/src/mmc.rs) reads
  MIDI Machine Control (`parse_mmc`: stop, play and deferred play, fast forward, rewind, record
  strobe and exit, pause, locate with SMPTE time); a port in the mapping role keeps SysEx
  (`MidiConnection::open` ignores only time and active sensing there), `PortInput::on_message` queues
  the command in the hub, and `AppCore::machine_control` runs it as a `TransportAction` (a locate
  converts seconds through the song's tempo table; learning swallows it).

- **Sources and targets.** A `MidiSource` is a kind of message (`Cc(n)` 7 bit, `Cc14(msb)` a
  controller pair where the LSB is the MSB plus 32, `PitchBend`, `Note(key)`, `Nrpn(n)` and `Rpn(n)`
  parameter numbers of 14 bits), a channel or any, a port by name or any (the panel's port and
  channel cells edit these two). A `MidiTarget` is `Param { device, param }` (a path key inside a container is
  allowed), `Vca { id }` (the group's level), `Tempo`,
  `Transport(Play | Stop | TogglePlay | Record | Loop | ReturnToZero)` or `SelectedStrip { param }`
  (the selected console strip, resolved at each message through the console's selection). Parameters
  are driven as a **fraction of their own range** (`ParamDescriptor::to_normalized` and
  `from_normalized`, so the curve of a cutoff or a fader is the knob's); a VCA level is linear in dB
  over -60..+12, the tempo linear over 20..999 BPM rounded to 0.1.
- **Options and maths** (`MapOptions`, `MidiMapping::feed`, pure and unit tested). `min` and `max`
  (fractions of the range, `min` may be above `max`), `invert`, a mode (`Absolute`,
  `Relative(TwosComplement | BinaryOffset | SignMagnitude)`, `Toggle`, `Momentary`), `sensitivity`
  (a detent as a fraction of the range, 1/64 by default), `pickup` (soft takeover, on by default)
  and `consume` (on by default). An absolute 7 bit controller is `v/127`, a 14 bit pair
  `(msb << 7 | lsb)/16383` (a message of either half updates the value, so an MSB-only controller
  still works), pitch bend `v/16383`, a note's velocity `v/127`. **Soft takeover** holds an absolute
  control back until it is within 1.2 % of the target or has crossed it between two messages; once
  picked up it follows without jumps, and it drops out again when the target is moved by anything
  else (the mouse, automation, a recall) so the control must catch it again. A stepped (discrete)
  param never uses it. A **relative** encoder moves the current value by `detents x sensitivity`
  (inverted if asked), clamped to the range. A **toggle** flips on the press (a note on, a
  controller of 64 or more; a repeat while held is not a press) and follows the target's real state;
  a **momentary** button sits at one end while held. A transport target is a trigger on the press.
- **Applying.** Every effect is an ordinary command through `AppCore::apply_command`, so it is
  validated, undoable and sent to the engine as a param change that the device's own smoothers (5 ms
  on the strip's gains) ramp: there is no zipper noise and nothing special on the audio thread. A
  continuous move carries **one merge id per gesture** (`MIDI_MERGE_BASE` = 12 x 2^40), so a sweep
  is one undo step like a knob drag; a gesture ends when the control has been still for
  `GESTURE_TIMEOUT` (0.5 s, a knob sends no release), which ends the document's merge group and
  calls `release_automation`, so a **Touch** lane of automation recording lets go there. A value the
  target already has is not a command (a held knob does not flood the history). Two controls moved
  together each have their own gesture; the document merges only consecutive commands, so they
  alternate into separate undo steps.
- **Learning.** *MIDI Learn* mode (`AppCore::set_midi_learn`, the Mixer menu's check item, "MIDI
  LEARN" in the status bar) swallows the first change a view makes to a control (`learn_swallows`: a
  `SetParam`, a VCA fader move, a tempo edit, a transport button) and arms that target instead; the
  right-click menu of a knob, fader or switch (`UiAction::ControlMenu`, Learn MIDI and Forget MIDI)
  arms it directly. While a target is armed the first hardware control moved becomes its source
  through the `Learner` (a note on, a pitch bend and a controller from 64 up decide at once; a
  controller below 64 waits 0.1 s for its pair, in either order, so an MSB/LSB pair becomes one
  `Cc14` source; the channel-mode controllers 120 to 127 and note offs are never a control). A
  learned source takes its channel and **any port** (a port can be set in a file), and the options
  of `MapOptions::for_source` (a note is a toggle button, anything else absolute with takeover).
  `MidiMapCommand::Learn` on a target that already has a mapping moves it to the new control and
  keeps its options. While learning every message is consumed. A completed NRPN or RPN run decides at
  once and is learned as `Nrpn(n)` or `Rpn(n)`.
- **What is consumed.** The hub holds the sources of the mappings with `consume` and the learn flag,
  replaced by the UI thread whenever the mappings change and read by the MIDI thread per message. A
  consumed message is parsed and timestamped but not routed, so it plays no note into the armed
  track and is not recorded as one; running status stays right. `consume: false` lets a mapped
  message through (a mod wheel that both plays and maps).
- **Addressing: project and presets.** A **project** mapping addresses a concrete target (a device
  key and a param key, a group id): the loader checks the shape and the numbers, not the references,
  so a mapping outlives its device (the panel lists it as missing, it does nothing, and undo of the
  device removal brings it back to life). A **preset** (`MidiPreset`, written by
  `RackDoc::midi_preset`, read by `RackDoc::resolve_preset`) addresses by **role**: a device type
  and the position among the top-level devices of that type (`atomik.channel` number 3, the first
  `atomik.poly-synth`) with the param key, a VCA group id, the tempo, a transport key or the
  selected strip. It drops the input port. Applying a preset resolves each mapping against the
  project's own devices, skips (and names) those whose instance, param or group is missing, and adds
  the rest in one undo step. A device inside a container has no role and is left out of an export
  with a note. The preset of one device (`RackDoc::midi_preset_for_device`, the panel's DEVICE
  button) addresses its params as `PresetTarget::OfType` (a device type and the param key, no
  position, so a device inside a container can have one): `resolve_preset_on` points them at the
  selected device when it has that type, else at the first device of the type, which is how a
  plugin (whose type id names the plugin) keeps its mappings. Presets are files
  (`atomik-midi-preset`, JSON; the shell opens the dialogs in a `midi-presets` folder next to
  `settings.json`).
- **Commands, undo and the file.** `Command::MidiMap(MidiMapCommand)`: `Add`, `Learn`, `Set`,
  `Remove`, `Forget` (every mapping of a target), `AddMany` and `Clear`, each one undo step on one
  `Edit::MidiMaps` (the whole list swaps), reporting `SyncOp::MidiMapsChanged` (the app rebuilds the
  consume list and forgets the takeover state). Ids are `max + 1`, at most 512 mappings; a command
  that changes nothing is refused. Project format 28 saves `"midi_maps"` after `"console"`, omitted
  when empty (a project without mappings differs from its version 27 file in the version line alone,
  which `crates/model/tests/midi_maps.rs` and `crates/render/tests/midi_maps.rs` check for text and
  for the bounce).
- **Views.** The rack and the console take a `MidiMarks` set (`set_midi_marks`): a small dot on
  every mapped knob, fader, switch and VCA strip, an outline on each while MIDI Learn is on and in
  the warning colour on the armed one; no marks paint exactly what was painted before. The **MIDI
  Mappings panel** (Mixer menu) lists the mappings: a click on the mode, the invert, pickup or
  consume chips edits them, a click or the wheel moves a range, LEARN arms the target again, a cross
  deletes; each edit is one undo step.

**Control surfaces.** A driver is plain Rust behind `SurfaceDriver` (in `atomik-midi-io::surface`,
no I/O and no model): `connect`, `on_message(bytes, &SurfaceState, &mut actions, &mut replies)`,
`feedback(&SurfaceState, &mut out)` (only what differs from what was sent) and `disconnect`. The app
builds the `SurfaceState` from the console (`AppCore::surface_view`): the strips **in console
order** (the channels, then the output buses, then the VCA groups; the track folders of the
arrangement never reorder the console), each with its name (the instrument's), the position of the
console's own fader (so the surface and the mouse share one law), pan, mute, solo, record-ready and
selection, the master, the transport lamps and the position (bar, beat, sixteenth and ticks, and
seconds). `SurfaceAction`s are applied as ordinary edits: a fader is a `SetParam` of `fader` (a
VCA's is its level) with the gesture's merge id, a **touch** opens the gesture and holds it until
the release (one undo step; Touch automation writes exactly while the finger rests), a surface
without touch sensors times out like a mapped control; a V-pot turns the pan, mute and solo flip,
record-ready flips the track's arm, select selects the strip as a click on its name plate does (the
track follows, a VCA selects its members), the transport keys are the transport buttons, rewind and
fast forward move a bar and the jog wheel a sixteenth per detent.

- **The eight-fader driver** (`Mcu`, id `mcu`, "Control surface (MCU-compatible)"; the table at the
  top of [mcu.rs](../crates/midi-io/src/surface/mcu.rs) lists every message). Faders are 14 bit
  pitch bend on channels 0 to 7 and 8 for the master, both ways: the motor is driven with the
  message the fader sends, a touched fader is not driven, a move the surface made itself is not
  echoed, and a change of one wire step is the rounding of the round trip, not a move. V-pots are
  relative controllers (sign and magnitude, one detent is 1/32 of the pan range) with LED rings (a
  single dot, the centre LED at the middle). Mute, solo, record-ready and select keys and their
  LEDs, bank left and right (eight strips, the last page starts at the last multiple of eight) and
  channel left and right (one strip), the transport keys and lamps, the jog wheel, a time display of
  ten 7-segment digits (bars, beats, sixteenths and ticks, or timecode at 30 frames, switched by the
  display key) and the two assignment digits (the first strip's number), and the scribble strips as
  SysEx: 56 characters per row, seven per strip (six characters and a space; names are cut,
  non-ASCII becomes `?`), the name above and the pan below (the fader's value while it is touched).
  The host connection handshake answers a query with the documented response to its challenge; the
  device query goes out on connect and "go offline" on disconnect. Every bank move and a handshake
  make the next pass send the whole surface again. Level meters, flip and the function keys are not
  implemented.
- **The generic controller** (`Generic`, id `generic`, "Generic controller (mapped)"). It has no
  layout: its messages are not decoded by the driver (`feeds_mappings`) but parsed by the host and
  run through the project's MIDI mappings tagged with the surface. A mapping whose source names a
  surface (`"surface"` in the source, learnt automatically when the control moved is on a surface)
  hears that surface only; a source without one hears every surface and every port, so older
  projects map as they did. Two surfaces can drive two devices at the same time with the same
  controller numbers.
- **Controller profiles.** `ProfileDriver` ([controller_driver.rs](../crates/app/src/controller_driver.rs),
  surface driver id `profile:<name>`) is a surface driver whose layout is a `ControllerProfile`
  ([controller_profile.rs](../crates/app/src/controller_profile.rs)), so any controller set up with the
  wizard works the same way. A profile is `controllers/<name>.json` next to `settings.json`:
  `{ "format": 1, "name", "input", "output" (optional), "controls": [ ... ] }`, each control
  `{ "role", "index" (knob and fader, 1 to 8), "message", "number", "channel" (from 1), "mode",
  "feedback" }`. Roles are `knob`, `fader`, `master`, `play`, `stop`, `record`, `loop`, `bank_left`,
  `bank_right`, `page_down` and `page_up`; the optional root fields `pickup` (default true) and `keep_page`
  (default false) are written only when they differ, so an older file reads as the defaults; messages `cc`,
  `cc14` (the MSB controller 0 to 31, the LSB is 32 above), `nrpn`, `note` and `pitchbend`; modes `absolute`, `relative_twos`, `relative_offset`, `relative_sign` and `button`. A file is
  read strictly (an unknown role, message or mode, two controls on one message, a key that is not a button
  or a file of a newer format is an error, never a silent drop), capped at 64 controls and 64 KiB.
  The driver turns the messages into the usual `SurfaceAction`s plus `SurfaceAction::Knob`, which the app
  applies to a parameter of `AppCore::followed_device` (the locked device, else the selected
  one; hidden parameters, plugin CV rows and `bypass` are skipped) as a `SetParam` with a gesture merge id;
  a stepped parameter moves one step per click of an encoder. `SurfaceState::knobs` carries the value of every
  remote parameter of the followed device (at most 128) and `SurfaceState::remote` its identity, name and the
  parameter names, so the driver, which owns the knob page, shows eight at a time: `SurfaceAction::Knob` carries
  the index over all of them, the page keys and `SurfaceDriver::knob_page` (the commands
  `controller.knob_page_next` and `controller.knob_page_prev`, `AppCore::knob_page`) move the page with
  wrapping, and a change of the followed device resets it unless the profile keeps a page per device. The
  soft takeover of an absolute knob or fader is `MapState::take_over`
  ([midi_map.rs](../crates/model/src/midi_map.rs)), the rule MIDI learn uses, with a 2 percent window and one
  `MapState` per control, rearmed on a device, page or bank change; encoders, stepped parameters and faders with
  feedback never wait. `SurfaceAction::Notice` carries the status line text ("Knobs: Filter 9-16", "Turn knob 3
  up to pick up Cutoff"). After a page change the knobs are sent again at the next feedback pass. Faders are the
  strips of the driver's bank (bank keys move eight, the last page starts at the last multiple of eight).
  Feedback sends only what changed (CC, a 14 bit pair, NRPN, pitch bend or a note, by the control's own message)
  and leaves a control alone for about half a second after the hand moved it.
  The wizard is a state machine without I/O ([controller_wizard.rs](../crates/app/src/controller_wizard.rs))
  over a `Capture` ([controller_detect.rs](../crates/app/src/controller_detect.rs)) that tells a control from
  its messages: an encoder sends a handful of values clustered in one of three codings in both directions, a
  knob sweeps a range; it is tested from fake message streams. The screen is
  [controller_wizard_view.rs](../crates/app/src/controller_wizard_view.rs); while it is open the surfaces
  let go of their ports. A saved profile also adds a surface (name, driver, input, output) to
  `settings.json`; `Shell::surface_driver` builds the driver from the id.
- **Several surfaces and the focused device.** `settings.json` `"surfaces"` is a list (name, `use`,
  driver, ports; at most eight); each enabled one runs its own driver and link in
  `AppCore::surface_frame`, in settings order, and an older file with one `"surface"` loads as a
  list of one. A mapping target `focused` is a param of the focused device: the device the surface
  of the message is locked to (`AppCore::lock_surface`, Mixer > Control and Macros > Lock Surfaces to
  Selection; never saved), else the device selected in the rack. A locked surface never falls back to the selection
  for a device that lacks the param.
- **Commands as targets.** A `command` target (`RemoteCommand`: select the previous or next track,
  undo, redo, previous or next preset of the focused device) is a trigger run on the press. It ends
  an open mapped sweep first, so undo takes back the whole sweep. The preset step uses the plugin
  presets of a plugin device and the patch list of the library folder for the others. Mixer >
  Control and Macros > Map Command arms one for the next control moved.
- **Ports and threads.** `SurfaceLink` is the surface's two ports (`SystemLink`: a `midir` listener
  that keeps SysEx and only queues the bytes, and an output; `LoopbackSurface`: the same in memory,
  for tests and the MCP server). `AppCore::surface_frame(now)` runs on the UI thread, never the
  audio thread: it polls the link, hands every message to the driver, applies the actions, and sends
  the feedback at most every 1/30 s. The surface is an **app** setting, not part of the project:
  `settings.json` `"surfaces"`, the Surfaces tab of Settings; a port
  that cannot be opened is reported and the surface is not set up.

### Macro banks, the parameter search and automation that follows the selection

[crates/model/src/macros.rs](../crates/model/src/macros.rs) and
[macros_io.rs](../crates/model/src/macros_io.rs) (the data, the maths, the commands and the file),
[crates/model/src/param_search.rs](../crates/model/src/param_search.rs) (the index and the ranking),
[crates/app/src/app_core_macros.rs](../crates/app/src/app_core_macros.rs) (the runtime),
[crates/ui/src/macros_panel.rs](../crates/ui/src/macros_panel.rs),
[param_search_panel.rs](../crates/ui/src/param_search_panel.rs) and
[macro_marks.rs](../crates/ui/src/macro_marks.rs) (the views),
[crates/devices/src/macro_bank.rs](../crates/devices/src/macro_bank.rs) (the device),
[crates/mcp/src/macro_tools.rs](../crates/mcp/src/macro_tools.rs) (the tools). A project that uses
none of it has no `"macros"` field, no extra device and no expansion work, and renders bit for bit
as before (`crates/render/tests/macros.rs`).

- **A bank is a device.** `atomik.macro-bank` (`MACRO_BANK_TYPE`; the model never depends on the
  device crate) is an inert Utility with the params `macro1` to `macro8` (0..1) and one Notes input
  it ignores. Its knobs are ordinary params, so they are automatable, undoable, in the rack's
  generated panel and MIDI-learnable with no special case. The Notes input is what gives it a song
  track: **a track exists only for a device that takes notes**, so an effect or a channel strip has
  no automation lane of its own, and the way to automate one is to assign its param to a macro and
  automate the macro. Several banks may exist (a per-track bank is a bank you keep for that track's
  params); the **current bank** (view state: the one chosen in the Macros panel, else the one last
  made or used, else the first) is where "Assign to Macro" in the right-click menu and the search
  put a param.
- **Assignments.** `RackDoc::macros: MacroSet { assigns, names }`. A
  `MacroAssign { id, bank, index, device, param, min, max, invert, curve, smooth_ms }` binds macro
  `index` (0 to 7) of a bank to a param of a device of the rack or **inside a container** (a path key
  such as `layer/synth`); `min` and `max` are plain values
  of the param (at macro 0 and macro 1), `invert` runs the macro backwards, `curve` bends the travel
  and `smooth_ms` is a live glide time. Rules: a param follows **at most one
  macro** in the project (assigning it again moves it, in the same undo step); a macro may drive any
  number of params (512 in all); the target is not a bank and not a container (a container has its own
  macros, section 6e; a param one of those drives is refused, and the container macro that wants a
  param a bank drives is refused too), the param exists and is not `internal.*`, and `solo` of a
  console strip is refused (the mix derives the silenced strips from it). A device that holds driven
  params cannot move into or out of a container (`check_not_driven`): the assignment names it by its
  key, which the move would change. A target may have an automation lane (see "A lane on a driven param"). A direct
  `SetParam` on a driven param is refused with
  `MacroDriven` naming the macro, so the model never hides where a value came from. The rack's
  controls do not send that command for a driven knob: `AppCore::redirect_driven_edit` turns the
  edit into `MacroCommand::MoveRange`, which shifts the range of the assignment by the amount that
  makes the param show the new value at the macro's position (`RackDoc::moved_range`, kept inside
  the param's own range; a drag carries one merge id, so it is one undo step; an edit that would
  change nothing is dropped without a message). The console strips and the plugin parameter pane
  still lock a driven control; there the range is edited in the Macros panel.
- **The value.** `assign_value`: `v = min + (max - min) x curve(m)`, with `m` replaced by `1 - m`
  when inverted, `curve` the identity, `m * m` (`Exp`, slow start) or `m * (2 - m)` (`Log`, fast
  start; `MacroCurve::shape`, only `+ - *`), clamped to the descriptor range and rounded half up for a discrete param. It is
  `expand::macro_value`, the container macros' function: only IEEE `+ - *` and comparisons, so every
  platform agrees and a bounce is reproducible; the default mapping is linear in the plain value (a
  cutoff moves linearly in Hz across the macro), unlike MIDI mappings, which move a fraction of the
  param's own curve, and the curve is a choice of the assignment. `assign_position` is its inverse
  (`MacroCurve::unshape` takes the one square root): **the first assignment to a macro that drives nothing
  moves the macro to the position that gives the target its current value**, so nothing jumps (a
  value outside the range clamps to its end); a macro that already drives others pulls the new
  target to its own position. **Removing** an assignment (`Remove`, `Clear`) leaves the param at the
  value the macro gave it, in the same undo step (an `Edit::Param` beside the edit of the whole
  set). Commands (`MacroCommand`: `Assign` with an optional bank, `SetRange` (with an optional drag
  merge id), `SetShape` (curve and smoothing), `MoveRange`, `Remove`, `Clear`, `Rename`) are one undo
  step each on one `Edit::Macros` that reports `SyncOp::MacrosChanged` for
  every bank touched; an assignment **outlives its devices** like a MIDI mapping (it does nothing
  while the bank or the target is missing, and the undo of the removal brings it back).
- **Where it is applied.** `RackDoc::expand` sets the driven params of the expanded devices
  (`macros::apply`) and lists the targets with the container macros in
  `ExpandedRack::macro_targets`, so a live engine rebuild, a bounce and `macro_values` read the same
  values (an inside target is found by its path key in the expansion). A song lane on a macro param is expanded by
  `expand_bank_lane` into one lane per live
  target with every point mapped by `assign_value` (a discrete target rounds each point), and
  `atomik_render::project::song_input_with_warps` joins each lane to the target's own track (a
  target without a track gets an automation-only one, a frozen target is left out); the lane equals
  hand-made lanes on the targets bit for bit, except for a target that has a lane of its own (see "A lane on a driven
  param"). In the app, a `ParamChanged` of a bank's `macroN` and `MacrosChanged` run
  `send_macro_values` (one `set_param` per changed target; a freed param gets its stored value
  back), the song is rebuilt (and, when a target has a lane of its own, on every move of the macro:
  `bank_shapes_lanes`), and `engine_lanes` makes an automation take hold every target of a
  macro lane. **Smoothing** has two layers. The device's own: the engine sets the value at the
  control rate and each device ramps it as it ramps a knob (5 ms on the channel strip's gains), the
  same path as a MIDI-mapped move, and a lane is applied per 64-frame batch like any automation
  lane. And the assignment's `smooth_ms`: `send_macro_values` does not set a smoothed target but
  starts a `Glide` (app state, `macro_glide_tick` from `transport_frame`) from the value the engine
  has now to the new one, a line over that time at the frame rate, ending exactly on the value; a
  first value, a stepped param, a rebuilt engine and a removed assignment end it, and the event loop
  keeps its frame rate while one runs (`macro_gliding`). A bounce, a macro lane and a freshly
  opened project use the plain mapping.
  `crates/app/src/app_core_macros_tests.rs` plays a project with a static macro and with a lane on
  one live at several block sizes and compares it with the bounce bit for bit.
- **The views.** `RackDoc::view_with_driven_values` gives the rack a copy of the document whose
  devices carry the values the macros give (the song is empty, as in a container's scope document;
  `None`, and so no copy, when nothing differs), the console and the parameter pane read
  `device_with_driven_values`, so a driven control **shows the macro's value**. It is drawn locked
  with the `M<n>` tag the container panels use, says "driven by macro n" in its tooltip, in the
  rack takes a drag, a double click and the wheel as a move of the range (see above; the console
  and the parameter pane refuse them with a message), and **keeps its right-click menu**
  (`UiAction::ControlMenu`: Learn MIDI, Forget MIDI, Assign to Macro 1 to 8 or to a new bank, Remove
  from Macro). A right click on a row of the parameter pane opens the same menu. The **Macros
  panel** (Mixer menu) lists the current bank's eight macros (a bar, a name edited in place, LEARN,
  CLEAR) and the params each drives (MIN and MAX moved by a fiftieth of the param's range or one
  step on a click, or continuously by a drag sideways, one merged undo step; CURVE and SM for the
  shape and the smoothing; INV, remove, a click on the target selects it in the rack); a bank is
  chosen by its chip and NEW BANK makes one. A middle click on MIN or MAX, or Enter with the pointer
  on it, types the value in place (`EntryUnit` reads Hz, kHz, ms, s and percent; the value is held to
  the param's range and rounded for a stepped param; Enter is one `SetRange`). A rack knob dragged with
  `Role::AssignMacro` held
  (`Alt+Shift`; `crates/ui/src/rack/macro_drag.rs`) onto a macro knob of a bank panel ends in
  `MacrosAction::Assign`, which `AppCore::assign_to_bank_macro` applies.
- **File.** Project format 29 saves
  `"macros": {"assigns"?: [{"id", "bank", "macro" (1 to 8), "device", "param", "min", "max", "invert"?}], "names"?: [{"bank", "macro", "name"}]}`
  after `"midi_maps"`, omitted when empty (a project without macros differs from its version 28 file
  in the version line alone, which `crates/model/tests/macros.rs` and
  `crates/render/tests/macros.rs` check for text and for the bounce). The loader names the JSON path
  of a repeated id, a second assignment of one param, a macro outside 1 to 8 and a bad name; it does
  not check the references. Plugin param rows a macro drives are written with the layout like those
  a lane uses. An assignment may also carry `"curve"` (`"exp"` or `"log"`, written only when not linear)
  and `"smooth_ms"` (a live glide time up to 5000 ms, written only when set); both are ignorable fields
  under format 43, so no version bump was needed. The `"device"` of an assignment may be a path key
  (`"layer/synth"`); a loader that does not find the device keeps the assignment dormant.
- **A lane on a driven param (see "A lane on a driven param").** `crates/model/src/macro_lane.rs`: a driven param with
a lane of its
  own plays `clamp(lane(t) + (max - min) x share(macro(t)))`: the lane is the base and the macro adds
  its range (the macro at 0 leaves the lane as drawn; an inverted one adds nothing at 1). The macro is
  the bank's stored value or its own lane. `RackDoc::driven_lane_points` joins the two flattened lanes
  at the points of either (`compose`; the sum of two straight or held segments is straight, so that is
  exact; a bent segment is cut into `BEND_PARTS` straight pieces; a jump stays a jump, the segment
  before it ending one tick early), and `song_input_with_warps` plays the result in the target's own
  track while `expand_bank_lane` leaves out a target with such a lane, so a bounce and the live song
  agree (`crates/render/tests/macros.rs`, `app_core_macros_tests.rs`). Following the selection shows
  the param's own lane once it has one. The knob shows the macro's value, not the sum, while the song
  plays.
- **Automation follows the selection.** The workflow preference `follow_selection` (View menu, the
  Workflow tab; `workflow.json`, an app setting, off by default) makes every
  `UiAction::ParamTouched` (a press on a rack control, a console control or a row of the parameter
  pane, locked or not) show the touched param's lane: `AppCore::param_touched` records it as the
  device's last touched param and, with the switch on, `follow_param` resolves the lane host (the
  param's own lane when its device has a track, **the macro's lane on the bank's track** when a
  macro drives it unless the param has a lane of its own, nothing for an audio track, a device inside a
  container or an internal param; an
  effect or a channel strip gets an automation track, see 2.6b), adds the lane when it is missing
  (`AddAutomationLane`, one undo step; an **empty lane is a view, no automation data exists until
  something is written**; never while an automation take records), opens the folders that hide the
  track (`SetGroupCollapsed`, they are the project's) and tells the arrangement to `reveal_lane`.
  The arrangement outlines the followed lane, shows its track's lanes (a collapse is view state) and
  **shows a folded lane open without changing the project's fold** (its own chevron still folds it,
  until another parameter is followed), scrolls it to a third of the way down when it is out of
  sight, and follows the lane that appears later. Selecting a device in the
  rack or a track in the arrangement shows the lane of the last param touched on it **when that lane
  exists** (a selection never adds a lane). View > Show Automation Lane does the same for the
  selected device on demand and explains why it cannot for an effect. The **Edit Automation** item
  of a control's right-click menu (`automation.edit_control`, enabled when `AppCore::has_lane_for`
  finds a lane host) does it for that exact param: `AppCore::show_lane_of` adds the lane when
  missing (one undo step) and the shell then reveals the arrangement and focuses it, as Show
  Automation Lane does.
- **The parameter search.** `ParamIndex::build` lists every param of every device (a container's
  inside by path key, console strips, plugin params the model knows, macros) and the VCA levels,
  each with its **track** (the instrument, its channel strip and the effects on its insert chain are
  one track; a bus is its name, the master `Master`, a return effect `Returns`, a VCA group `Mixer`,
  the rest `Rack`), device name, param name, key and unit; internal params are left out. It holds
  names only, with a `signature` (a hash of the devices' keys, types, names and layouts, the cables,
  the strips and the groups, never a value) so the app keeps one index and rebuilds it only when
  `signature_of` changes. `search` splits the query on spaces and `>`, ignores case, needs **every**
  word to match one of the five fields, scores a word 8 at the start of a field, 6 at the start of a
  word, 4 as a substring and 1 to 2 as a fuzzy match of three or more letters at most three
  characters apart, weighs the fields (param 10, device 6, track 5, key 4, unit 3), orders by score,
  then the shorter param name, then rack order, and cuts at `limit` (never above 500; the panel
  lists 100). A search is a scan over the lowercased fields, so a project of thousands of params
  stays responsive, and the rows read their values only when shown. The panel (View > Find
  Parameter, `Ctrl/Cmd+Shift+P`) acts on a row: Enter reveals it (the device in the rack, the strips
  or the group's members in the console), Ctrl/Cmd+L shows its lane, M assigns it to the next free
  macro of the current bank, E arms it for MIDI learn (refused for a driven param: the controller
  belongs on the macro), Ctrl/Cmd+Left and Right adjust the value in place by a fiftieth of the
  range (a step for a stepped param, Shift a tenth; a VCA level moves a dB).
- **MCP.** `project_inspect` and `session_status` list `macros`; `session_macros`,
  `session_param_search` and the stateless `param_search` (docs/MCP.md).

## 6. Audio tracks and the sample pool

### Files and the bundle: `atomik-audio-file`

Nothing in this crate runs on the audio thread. Every function may allocate and block on disk.

- **Decode** (`decode_file`, `decode_bytes`): WAV, AIFF, FLAC, MP3 and OGG through symphonia. A
  decoder panic becomes an error. Lossless sources must decode completely. The in-memory decode
  holds at most `DEFAULT_MAX_SAMPLES` (2^30 samples, 4 GiB of `f32`) and checks the size before it
  grows a buffer, since an allocation failure aborts the process and no `catch_unwind` sees it;
  `decode_file_with` and `decode_bytes_with` take the limit as a parameter and also return the
  count of repaired samples (`Decoded`). `AudioData::new` turns every NaN and infinity into silence
  (`new_counted` says how many), so nothing non-finite reaches the mixer; `AssetInfo` carries the
  count of an import.
- **WAV** (`read_wav_f32`, `read_wav_f32_limited`, `WavStreamReader`): the header's sample count is a
  claim, so the allocation of a whole read is bounded by what the file's own size can fill. The
  streaming reader walks the chunks itself with 64-bit offsets (RIFF, and RF64 with its `ds64`
  size), so an asset above 4 GiB streams; a data size the file cannot hold is cut to the file.
- **Resample** (`resample`): a fixed-parameter rubato sinc resampler with an exact alignment
  correction, so the result never depends on the caller. Used on import when the file's rate differs
  from the session rate.
- **Hash and ids** (`content_hash`, `is_valid_hash`): a SHA-256 over a canonical encoding of the
  decoded audio. The id is 64 lowercase hex digits and is checked everywhere it is used as a file
  name.
- **Peaks** (`PeakPyramid`): min/max per bucket at 256, 1 024, 4 096 and 16 384 frames, with a compact cache format.
- **`Bundle`**: the project folder. `Audio/<hash>.wav` (32-bit float), `Audio/.peaks/<hash>.peaks`.
  `import`, `add_recording`, `load`, `stream_reader` (`WavStreamReader`, seekable), `copy_bundle`,
  `orphans` and `remove_orphans`. Every file is written to `<name>.tmp~`, fsynced and renamed.
  `save_text_atomic` does the same for the project file.

### The model

`RackDoc::assets` is the sample pool (`AssetMeta`: channels, rate, frames, original name), keyed by
id. `RackDoc::input_channels` is the number of host inputs (`interface:in_1..N`). An audio track is
a track on an `atomik.audio-track` device whose lane 0 is `LaneKind::Audio`; its clips are
`ClipContent::Audio(AudioClipContent)` with the asset id, source offset in frames, gain in dB, fades
in frames and a fade curve, a reverse flag, a gain envelope and a crossfade shape (see 2.17). Clip
start and length stay in ticks; audio is not stretched. `flatten_audio` turns a track into regions
(later clips win, except where a crossfade lets two overlap; fades are measured inside the original
clip).

An audio clip may loop (`Clip::loop_length`, never together with `reverse`; a take may loop too, see
the takes section):
`flatten_audio` emits one region per repetition, cut at the repetition's boundary ticks and so
frame-exact through the tempo map. Each repetition restarts the source at the clip's source offset
and carries its own fades, envelope and warp job (warp buffers of repetitions of one length share a
key, so one buffer serves a loop of a thousand repetitions), which makes a loop and the same loop
turned into separate clips (`WorkflowCommand::ConvertLoop`) play the same samples. A looped clip
crossfades with the clip before it only inside its first repetition. The engine's region lists are
already explicit, so nothing on the audio thread changes. A looped audio clip splits only where a
repetition starts. The file format holds an audio loop in the `loop_length` field that every clip
already had (format 37 reads it, older readers refuse it).

### The engine side

- **Assets and regions** ([audio.rs](../crates/engine/src/audio.rs)): `SongAsset::Memory` (planar
  `f32`, shared) or `SongAsset::Stream`. A `SongAudioRegion` is a song-frame start, a source offset,
  a length, a linear gain and the fade, reverse, envelope and crossfade data. For region frame `i`
  the output is `asset[offset + i] × gain(i)` (`offset - i` when reversed), cut at the region end
  and the asset end; see 2.17. Mono plays on both channels, and for more than two channels the first
  two are played. Equal-power fades use `sin_quarter`, a polynomial that calls no libm function, so
  every platform rounds the same (HI-2).
- **Rendering.** `Renderer::segment` renders each batch segment into a preallocated stereo pair per
  track, at its batch offset, so loop wraps and locates are sample-accurate. `publish_audio` hands
  the pair to the track's device node, and `ProcessContext::playback` gives it to `Device::process`.
- **Streaming** ([stream.rs](../crates/engine/src/stream.rs)): see 2.5. The ring carries the track's
  streamed regions' finished output (gains applied, overlapping regions mixed) in playback order,
  tagged by playback index and epoch, so loop wraps need no refill.
  `EngineHandle::{stream_underruns, stream_primed}` report the state; the app delays Play until the
  lanes are primed. Offline bounces use `Memory` assets only.
- **Record tap** ([record.rs](../crates/engine/src/record.rs)): see 2.6.
  `EngineHandle::record_tap(channels, capacity)` allocates the ring off the RT thread and queues the
  tap; `RecordTap::drain` returns only runs that are contiguous in engine frames, so a start tag is
  always exact; an overflow is counted in frames and never blocks.
  `EngineHandle::record_tap_gained(channels, capacity, controls)` is the same tap with a gain and a
  clip-safe limiter per strip (`TapChannel`, `TapControls`); a channel reads a host input or, for
  resampling, an output socket of a routed device (`TapChannel::source`, `Arena::output`), read at
  the end of the batch, after the routing, with no allocation; the plain tap copies
  bit for bit as before.
- **The device** `atomik.audio-track` ([audio_track.rs](../crates/devices/src/audio_track.rs)):
  `out = playback + input × monitor_gain`. `monitor` is off, auto or on; with auto the app's
  `internal.monitor_live` (the arm flag) decides. A change is a 5 ms crossfade. It has one stage and
  no latency.

### The Spectrum EQ window

View > Panels > Spectrum EQ (F2) opens a floating window of its own (an OS window, so it resizes and moves
freely; closing it ends it) that shows one console strip. The channel, bus and master devices
([channel.rs](../crates/devices/src/channel.rs), [master.rs](../crates/devices/src/master.rs)) each
own a `ScopeTap` allocated in `prepare`; with the app-set `internal.tap_live` param at 1 they push
the signal after the EQ and before the fader, and with it at 0 (the default) they write nothing, so
a strip with no window is allocation-free and bit-identical. The app
([app_core_spectrum_eq.rs](../crates/app/src/app_core_spectrum_eq.rs)) switches the param on for
the shown strip only, runs the FFT on its own thread (4096 frames, the spectrum device's
analysis) and draws the strip's filters and EQ from `StripCurve` in
[strip_eq.rs](../crates/dsp/src/strip_eq.rs), built on `magnitude_db`; a test measures the strip
with a sweep against it. The view ([spectrum_eq.rs](../crates/ui/src/spectrum_eq.rs)) emits
`SetParam` commands under one merge id (a band's frequency and gain as one batch), so a drag is one
undo step and records automation. Follow Selection tracks the mixer's primary strip. The master
shows the analyser only. The same tap switch is what the EQ device's own analyser can gate.

### The app: workers and rules

[crates/app](../crates/app): `AppCore` (no window, no GPU) owns the document, the registry, the
views and the engine sink; `Shell` is everything inside the window; `platform/` is winit, the GPU
and the native menu. Audio adds:

- [`app_core_audio.rs`](../crates/app/src/app_core_audio.rs): bundle rules, import, the asset cache, Save and Save As.
- [`audio_import.rs`](../crates/app/src/audio_import.rs): the import, load and folder-scan workers
  (message passing only).
- [`app_core_relink.rs`](../crates/app/src/app_core_relink.rs) and
  [`audio_relink.rs`](../crates/app/src/audio_relink.rs): relinking missing audio (the core logic,
  and the pure bounded folder scan); the panel is
  [`ui/src/relink_panel.rs`](../crates/ui/src/relink_panel.rs).
- [`app_core_rate.rs`](../crates/app/src/app_core_rate.rs): a new project at a rate, the sample
  rate conversion with its audio, and the channel counts.
- [`app_core_templates.rs`](../crates/app/src/app_core_templates.rs): the templates folder
  (`Templates` beside the library folder, listed by name and refreshed by the shell once a second),
  File > New from Template (`open_template`: the project opens with no file name, so Save cannot
  overwrite it, and its audio is copied into the session bundle by the adopt worker, so it plays at
  once and goes into the project folder on the first save), Save as Template and the default song
  (`write_copy` writes the saved form of the song with only the audio it uses and changes nothing of
  the open song; File > New opens the song named by the `default_song` workflow preference, a
  start with no project does too), the song information (`set_song_info`, one undo step; the title is
  in the window title) and the scratch folder preference (`scratch_folder` in `workflow.json`: the
  session scratch bundle is made there from the next import on, with a fallback to the temporary
  folder, and the orphan-recording scan looks there too). The Song Information panel is
  [`ui/src/song_info_panel.rs`](../crates/ui/src/song_info_panel.rs) (`SongInfoView`,
  `UiAction::SongInfo`): one focused text, Tab between them, the model's limits applied as you type. Each
  text is a [`TextField`](../crates/ui/src/text_field.rs): a caret and a selection (pointer, Shift+arrows,
  Ctrl+A), the clipboard (`atomik_ui::clipboard`, backed by the system's through
  [`text_clipboard.rs`](../crates/app/src/text_clipboard.rs) and the `arboard` crate, text only),
  input-method composition (`UiEvent::Ime`, sent by `platform/app.rs` from winit's `Ime` events while
  `Shell::wants_text_input` holds) and AltGr characters as text. `Canvas::text_width` measures a run so the caret
  sits on the shaped glyphs (the display-list recorder falls back to an estimate).
  The file browser's search and path fields are the same widget. The rack-style pick lists (the arrangement's
  input, parameter, bus and take lists, the console's bus and source lists, the plugin parameter and preset lists,
  the container macro and CV lists) share [`PickList`](../crates/ui/src/pick_list.rs) and `CompactList` for their
  geometry, hover row, scrolling and frame; the registry-driven `ContextMenu` stays the menu for commands. Labels
  that must fit a width are cut by what the canvas measures (`paint::fit_label_in`, `paint::tail_fit`) in the file
  browser and the rename fields, not by a character count. `paint::snap_hairline` places a one-pixel line in the
  middle of a physical pixel at fractional display scales (the arrangement and the audio editor use it). The rack
  forgets the per-device view state (selected pad, edited pattern, page, cache ids) of devices that leave the
  document (`prune_device_maps`).
- [`app_core_archive.rs`](../crates/app/src/app_core_archive.rs) and
  [`archive_worker.rs`](../crates/app/src/archive_worker.rs): File > Project Files > Export and
  Import Project Archive. The archive is one `.atomikz` file, a stored (uncompressed) ZIP with ZIP64 fields for
  entries and archives over 4 GiB, holding `<name>/<name>.atomik` and `<name>/Audio/<hash>.wav`
  ([`audio-file/src/archive.rs`](../crates/audio-file/src/archive.rs)). Writing streams through a part
  file that is renamed when complete; reading takes the central directory first and accepts only
  those two shapes (a path that leaves the folder, a duplicate, a second project, a compressed or
  encrypted entry, an entry or total over `ArchiveLimits` is refused before a byte is written),
  checks every CRC, and extracts into a new folder `<destination>.part` that is renamed when whole
  and removed on any failure or cancel. One job runs at a time on a worker thread
  (`Msg::Archived`), with a progress counter and a cancel flag (File > Project Files > Cancel Project Archive);
  an unpacked project is handed to the shell like a library project (`take_open_request`), which asks
  about unsaved changes before opening it.
- [`app_core_share.rs`](../crates/app/src/app_core_share.rs): File > Share Project, and opening a share. A share
  is the same `.atomikz` archive with three more kinds of entry, all optional and each checked by shape like the
  rest (`ArchiveExtras`: `<name>/credits.txt` of at most 1 MiB, `<name>/preview.mp3` or `preview.wav`, one at most,
  and `<name>/stems/<file>.wav`; a name that is anything else is refused, so a `../` entry never gets as far as a
  path). The worker (`Job::Share`) renders the preview and the stems with the export worker's own `run` into a
  temporary folder that is removed whatever the end, then packs everything; one progress counter and one cancel flag
  cover the three steps, and a cancel or a failure leaves no archive. The preview is an MP3 when the encoder state is
  ready and a 16-bit WAV file otherwise, at -14 LUFS. The panel's texts become the song information (one undo
  step) before the job starts, and the audio is found in the places Collect Files looks (`collect_sources`). Opening
  one is an import into a folder the user picked (`start_share_open`); `take_share_credits` tells the shell to say
  so after the project is open. The panel is [`share_panel.rs`](../crates/ui/src/share_panel.rs).
- [`stream_source.rs`](../crates/app/src/stream_source.rs): the 64 MiB streaming threshold and the
  engine's `StreamSource` over a WAV reader (a read error plays as silence and is counted).
- [`audio_record.rs`](../crates/app/src/audio_record.rs): the pure recording rules (compensation, laps, placement).
- [`audio_writer.rs`](../crates/app/src/audio_writer.rs) and
  [`app_core_rec_audio.rs`](../crates/app/src/app_core_rec_audio.rs): the writer thread and the glue
  that follows the transport.
- The bundle layout and Save rules are shared with the bounce in
  [crates/render/src/bundle.rs](../crates/render/src/bundle.rs).

## 6a. Sample devices

The sampler `atomik.sampler` ([DEVICES.md](DEVICES.md#sampler-atomiksampler-instrument)) is the
first device that plays shared sample data. The path from a dropped file to a sounding voice:

- **Sample maps** ([device-sdk/src/samples.rs](../crates/device-sdk/src/samples.rs)). A `SampleMap`
  is a list of `Arc<SampleBuffer>` (planar, immutable, at the session rate) and a list of `Zone`s.
  `SampleMap::new` validates every zone (ranges, frames inside the buffer, start < end, loop fields
  inside start..end when looping, crossfade fits), so a device never checks again. Types that take
  samples are registered with `Registry::register_with_samples` and answer
  `Registry::takes_samples`; the model and the app ask the registry, not a device list.
- **Swap and draining.** `Device::swap_samples(map) -> Option<SampleMapRef>` is RT-safe and
  allocation-free. The sampler holds at most two maps, `current` and `draining`, and never drops
  one: it hands one back. A voice whose zone index exists in the new map with a pointer-equal buffer
  moves to it and keeps its position; every other voice keeps playing from the old map (now
  draining) with a 5 ms fade-out. A second swap while voices still drain cuts them and returns the
  draining map; a stale draining map comes back at the next swap. Exactly one map (or none) is
  returned per call.
- **Engine.** `EngineHandle::set_device_samples` checks the id against the mirror (unknown ids are
  rejected on the handle side), then queues `ControlOp::Samples`, applied at a batch boundary in
  queue order, so insert then samples works. The audio thread calls `swap_samples` and queues the
  returned map as `Retired::Samples`; `collect_garbage` drops it off the audio thread. A removed
  device's maps leave with the device through the same retire path. Hot edits can carry samples
  (`HotEdit::Samples`); offline and patch builds call `Graph::set_device_samples` before `compile`,
  which drops the returned map on the non-RT thread.
- **Model** ([model/src/zones.rs](../crates/model/src/zones.rs)). `RackDevice::zones` is a list of
  `ZoneDoc` (the SDK `Zone` with a pool asset id instead of a buffer index; empty and not serialised
  for every other device). `Command::SetZones { device, zones, merge: Option<u64> }` replaces the
  whole list in one undo step and merges consecutive drags with the same id on the same device
  (`MergeKey::Zones`). It is validated with the SDK's own `SampleMap::new` over throwaway data-less
  buffers, so the model and the engine cannot disagree, and each asset must be in the pool with
  frames inside its length. `SyncOp::ZonesChanged` tells the app. Zone assets count in
  `referenced_assets`, including the undo and redo history. Project format v4 stores `zones` inside
  the device object; v1 to v3 still load, and zones need version 4. `auto_map` and `parse_note_name`
  turn dropped files into zones (C4 = 60; a note name in the stem sets the root and neighbouring
  roots split the keyboard halfway; otherwise one key each from C4 or the drop key, in name order).
- **App** ([app_core_samples.rs](../crates/app/src/app_core_samples.rs)). `AppCore` builds each
  sampler's map from the asset cache with the shared builder
  `atomik_render::samples::build_sample_map` and sends it through `EngineSink::set_device_samples`:
  on every `ZonesChanged`, right after a sampler is inserted (so after every engine restart, in
  order), and when an asset finishes loading. Sampler assets are always in RAM: a memory asset of an
  audio track shares its sample data and one `Arc<SampleBuffer>` per asset is kept (so a zone edit
  keeps voices on the same buffers); an asset that is only streamed is loaded fully by a worker.
  Zones whose audio is missing or not loaded yet are left out of the map and play silence, and stay
  in the document. `UiAction::ImportSamples` runs the import worker and applies the `RegisterAsset`s
  and one `SetZones` as a single undo step. `--stress-sampler` adds 4 samplers of 32 voices to the
  stress song.
- **Sampling.** The SAMPLE button beside EDIT on the sampler, granular, drum and slicer panels
  emits `UiAction::SampleInto`
  ([app_core_sampling.rs](../crates/app/src/app_core_sampling.rs)). The first press opens an engine
  record tap on the sampling input (a stereo pair or one channel; a right-click cycles it, so it
  is not a saved setting) with a ring of 30 s allocated off the audio thread; each frame moves the
  ring into a reserved buffer. The second press, or the 30 s limit, stops the tap, cuts the silence
  before the first frame at -60 dBFS (`leading_silence`, the detector of strip silence), stores one
  asset in the project bundle and lands it as one `SetZones` step through the same completion as a
  dropped file. Esc, an engine restart or a take that starts cancels without touching the document.
  The tap serves one user at a time, so sampling and recording audio tracks exclude each other.
- **Sample edits.** `UiAction::EditSample` renders crop, normalise, reverse and fades on the worker
  `atomik-sample-edit` with the pure functions of `atomik_audio_file::edit` into a new asset and
  swaps it into the zone (or every zone playing the same asset) as one undo step
  ([app_core_sample_edit.rs](../crates/app/src/app_core_sample_edit.rs)). `remap_zone` keeps each
  zone on the same audio (a crop shifts the markers, a reverse mirrors them). The old asset stays
  in the pool while the history needs it. `RackDoc::asset_usage` counts what uses every pool asset
  and `Document::remove_asset` drops one nothing needs.
- **Editor pane.** `Pane::SampleEditor` shows `SampleEditorView`
  ([ui/src/sample_editor.rs](../crates/ui/src/sample_editor.rs)) in place of the arrangement. Opened
  by `UiAction::OpenSampleEditor` (double-click on the panel or its Edit button), closed by
  `CloseSampleEditor` (button, Esc, or when the sampler is removed). Auditions send
  `UiAction::AuditionVelocity` (7-bit velocity from the click height; the core sends
  `velocity / 127` and a matching note-off); `LoadSamplesDialog` asks the shell for the file dialog.
- **Render.** The bounce builds the same maps from the bundle (always memory) with the shared
  builder and installs them with `Graph::set_device_samples` before compile
  ([render/src/project.rs](../crates/render/src/project.rs),
  [build.rs](../crates/render/src/build.rs)). A missing zone asset is a bounce error that names it;
  the live app plays silence instead. Live and bounce are bit-identical once the live transport's
  one-batch (64 frame) start delay is removed.

## 6b. Drum machine and loop slicer

Both devices sit on the 4b-2a sample path (section 6a): zones, `SetZones`, the sample map swap and
the shared map builder are unchanged. What they add:

- **Pads are zones** ([model/src/zones.rs](../crates/model/src/zones.rs)). `atomik.drum-machine` has
  16 pads on the fixed keys 36 to 51 (pad *n* = key 35 + *n*). A pad's sound is the zones with
  `key_lo == key_hi ==` its key, so velocity layers need no new data; zones on other keys are
  ignored. The helpers `pad_key`, `pad_of_key`, `pad_zones`, `with_pad_files` (one file at full
  velocity, several split evenly by velocity in name order) and `fill_empty_pads` build the zone
  lists; the app applies each as one `SetZones`. A pad plays at most 8 zones. Choke groups (a hit
  fades the other pads of its group over 2 ms, group 0 never chokes), per-pad monophonic retrigger
  (the previous hit fades over 2 ms), 32 voices with oldest-first stealing, and gate or one-shot per
  pad are inside the device ([drum_machine.rs](../crates/devices/src/drum_machine.rs)).
- **Individual outputs and several channels per source.** The drum machine has `pad1_out_l` to
  `pad16_out_r` besides `out_l`/`out_r` (35 audio sockets), global `level_cv_in` and `pitch_cv_in`,
  a `padN_gate_cv_in` and `padN_gate_cv_out` per pad and four shared send outputs (73 sockets in
  all). A pad also has a tone, low and high cut, drive and two send levels (`PadFx`,
  [pad_fx.rs](../crates/devices/src/drum_machine/pad_fx.rs)); a pad with all of them at their
  defaults is not touched, so existing projects render as before. A pad whose output is
  connected on either side plays only there; the soft limiter and master level apply to the main mix
  only. `ConsoleCommand::AddChannelFor { source, out_l, out_r, name }`
  ([model/src/console_cmd.rs](../crates/model/src/console_cmd.rs)) makes a console channel fed by
  chosen audio outputs of a source, in one undo step. It is validated: both sockets must be audio
  outputs of the source and not already cabled, and the source must not be a console device or sit
  on a return. `ConsoleChannel::source` lets one source feed several channels; solo, removal of the
  source (all its channels go with it, and undo brings them back) and save and load handle that. The
  rack's TO CH button on a pad uses it and names the channel "Drum machine Pad N".
- **Insert chains.** `ConsoleCommand::SetInsert { channel, slot, effect }` and
  `MoveInsert { channel, from, to }`
  ([model/src/console_insert.rs](../crates/model/src/console_insert.rs)) put up to four stereo
  effects on a channel's or the master's insert loop, wired in order
  `insert_send → fx1 → … → insert_return`. The chain is derived from cables (`insert_chain`), so
  nothing new is saved and the project format is unchanged; a hand-cabled loop of the same shape
  reads back as a chain. `InsertEffect::New { type_id }` adds a device right after the previous
  stage, `Existing(key)` takes a rack effect; replacing or removing a slot drops its cables but
  leaves the device in the rack; an empty chain leaves the insert unpatched. Each edit is one undo
  step, validated on a scratch copy (effect kind and stereo sockets, no cycle, not a return effect,
  a channel source or a member of another chain). Bypass is the `bypass` param of each effect (see
  Channel strip upgrades in section 5), which keeps the delay compensation; the strip's own
  `insert.bypass` is older and is not compensated. The console strip's INSERTS section (and the
  master's bus column) shows four slot rows: a filled slot (name, a B that bypasses the effect and
  an x) selects the effect in the rack or removes it, the first empty slot opens the add menu (new
  effect types, then free rack effects).
- **Step grid and swing in flatten.** A note clip may carry
  `StepGrid { resolution (1/8, 1/16, 1/32), steps 16..=64, swing 0..=75 % }` (`Clip::grid`,
  `SongCommand::SetClipGrid`, merged for swing drags). Swing is applied when the clip is flattened
  for the engine ([song/flatten.rs](../crates/model/src/song/flatten.rs)): a note in an odd step,
  counted from the clip's content start after the offset, moves later by
  `(swing x step + 100) / 200` ticks (integer arithmetic, so live and bounce agree to the bit); its
  length stays; every loop pass swings alike; a note pushed past the clip or loop end is dropped.
  The document's notes are never changed by swing.
- **`Command::Batch`** accepts `Connect`, `Disconnect`, `SetClipGrid`, `SetClipGeometry`,
  `AddTrack`, `AddClip`, `SetParam` and `SetZones`. The commands are validated in order against a
  scratch copy, the whole batch is rejected if one fails, it is one undo step, and each kind of
  `SyncOp` is reported once (one `ZonesChanged` per device). A merge id inside a batch is ignored.
  The step editor uses it for grid plus length, "New pattern" for track plus clip, the audio track
  selector for cables, and the loop load for `loop.beats` plus slices.
- **Reverse in the shared playhead.** `Zone::reverse` / `ZoneDoc::reverse` mark a reversed zone.
  `PlayRegion::reversed` has no loop; its `Playhead` starts at `end - 1`, steps down to `start` and
  fades out at `start`. The sampler, the drum machine and the slicer all use it, so there is one
  rule ([dsp/src/sample_read.rs](../crates/dsp/src/sample_read.rs)).
- **Onset detection** ([dsp/src/onset.rs](../crates/dsp/src/onset.rs)).
  `detect_onsets(channels, rate, sensitivity)` is pure, deterministic and allocating, so it runs
  only on a worker: a mono sum, RMS energy flux with a window of 1024 and a hop of 256 frames at 48
  kHz, an adaptive threshold from the sensitivity (only IEEE arithmetic and `sqrt`), localisation
  back to the foot of the attack (at most 5 ms) and a 30 ms minimum gap. More sensitivity never
  gives fewer onsets.
- **Slicing helpers** ([model/src/slices.rs](../crates/model/src/slices.rs)). `atomik.loop-slicer`
  plays slice *n* on key 23 + *n* (24..=87, at most 64 slices) as a zone over the loop asset.
  `slices_from_markers` normalises markers (sorted, first at 0, at least 1 ms apart, at most 64) and
  keeps the per-slice fields (gain, pan, transpose, tune, reverse) by slice index; a new slice
  copies the last previous slice's fields. `grid_markers` is exact integer arithmetic and reduces
  the divisions when beats times divisions exceed 64. `to_track_notes` writes one note per slice at
  `round_half_up(start x beats x PPQ / frames)` ticks (u128 arithmetic) with the length up to the
  next slice, and the clip is `loop.beats` long. `default_loop_beats` rounds the loop length at the
  song tempo to whole beats.
- **App** ([app_core_drums.rs](../crates/app/src/app_core_drums.rs),
  [app_core_slicer.rs](../crates/app/src/app_core_slicer.rs)). Importing on a drum machine uses the
  pad rules above; on a slicer only the first file is used, and a worker runs onset detection at the
  current sensitivity, so the UI thread never analyses audio. The result is applied as one `Batch`
  of `SetParam loop.beats` and `SetZones` (dropped if the document generation changed). `SliceLoop`
  (transients on a worker, or an even grid) ends in one `SetZones` that keeps the per-slice fields;
  `SlicesToTrack` writes the clip as one undo step (replacing the selected clip of the slicer, else
  adding one at the bar-snapped playhead, with `AddTrack` in a `Batch` if needed). The demos are
  `examples/projects/drum-demo` and `slicer-demo`; `--stress-drums` and `--stress-slicer` add 4
  devices with dense patterns to the stress song.
- **Panes.** Three panes replace the arrangement and exclude each other (section 7): the sample
  editor, the step editor (`Pane::StepEditor`) and the slice editor (`Pane::SliceEditor`); each
  closes when its device or clip goes away.
- **Project format v6.** v5 added the clip `grid` (only when present); v6 adds `reverse` to every
  zone (required in v6, rejected before). Saving always writes v6; v1 to v5 still load.

## 6c. Effects

Six original Effect devices in `atomik-devices` (and the guitar amp below), each stereo with the
sockets `in_l`, `in_r`, `out_l`, `out_r` (the compressor adds `sc_in_l`/`sc_in_r` and a
gain-reduction CV out). Their DSP building blocks live in `atomik-dsp` (`fdn`, `oversample`, the
existing `biquad`, `dynamics`, `lfo`). Every effect is allocation-free in `process`, deterministic
(bit-identical across host blocks and worker counts) and finite for any params; details and
parameter tables are in [DEVICES.md](DEVICES.md).

| Device | Notes |
|---|---|
| `atomik.reverb` | 8-line feedback delay network with room, hall and plate settings, seeded parabolic LFO modulation, freeze, latency 0. |
| `atomik.space-reverb` | The reverb engine with arena, cathedral, spring and echoverb algorithms, a wet high cut and a gated tail; latency 0. |
| `atomik.convolver` | Convolution reverb: an impulse response from the project pool, a 256-sample direct head plus uniform frequency-domain partitions (zero latency), built a few partitions per batch and crossfaded on a swap. |
| `atomik.vocoder` | 4 to 32 band channel vocoder with an internal noise carrier and a consonant hiss path; latency 0. |
| `atomik.compressor` | Peak or RMS detector, soft knee, auto release, key filters, sidechain input, look-ahead 0 to 5 ms reported as latency. |
| `atomik.eq` | Six bands (cut slopes 12/24/48 dB per octave), coarse band levels published for the analyser. |
| `atomik.distortion` | Ten curves (the last five with ADAA, see [shaper.rs](../crates/dsp/src/shaper.rs)), cut and body sections, off/2x/4x halfband oversampling; the oversampler latency is reported and the dry path is delayed to match. |
| `atomik.clipper` | Clipper: a hard-to-soft curve ([clipper.rs](../crates/dsp/src/clipper.rs)) with symmetry and a DC blocker, off/2x/4x/8x oversampling (latency 0/29/35/37 reported, dry and delta aligned), a 2 or 3 band mode on the mastering crossovers, a delta switch and published clip activity. |
| `atomik.echo` | Stereo echo on the dsp `Echo` line (glided time, wobble): colour, crush and diffusion in the loop, ducking, triggered and roll modes; latency 0. |
| `atomik.modulation` | Chorus, flanger, phaser with tempo sync from the transport. |
| `atomik.mastering` | 3-band multiband, mid/side width (optionally split by band), soft clip, true-peak limiter with a 0.5/1.5/4 ms look-ahead (4x detector), BS.1770 / R128 loudness and true-peak meter. |
| `atomik.guitar-amp` | Guitar amp: gate, tight high-pass and boost, a 4x oversampled three-stage preamp with a tone stack and a power amp, a synthesised cabinet with a mic, then chorus, ping-pong delay (tempo sync) and reverb; five factory setups; a constant 35 frames of latency. |

- **Guitar amp.** `atomik.guitar-amp` is an effect built from original blocks in
  [dsp/src/amp](../crates/dsp/src/amp) (input stage, preamp, tone stack, power amp, cabinet) and the
  existing chorus, delay line and room of `atomik-dsp`. The preamp and power amp run inside one 4x
  `Oversampler` round trip (35 frames, reported through `latency_frames`, so PDC and the bypass
  wrapper treat it like the oversampled distortion) and every clip is first-order antiderivative
  anti-aliased in `f64`; the cabinet is a cascade of designed biquads with gliding coefficients (no
  recorded impulse response). Its `setup` choice is a label the device ignores:
  `AppCore::apply_command` runs `expand_setup`
  ([app/src/app_core_setups.rs](../crates/app/src/app_core_setups.rs)) first, which turns a
  `SetParam` of `setup` on a guitar amp into one `Command::Batch` of the setup param and every param
  of the chosen row of `devices::guitar_amp::setups::SETUPS`, so choosing a setup is one undo step
  from any front end (rack panel, MIDI, MCP) and a project only holds plain params. The panel is
  `FxKind::GuitarAmp` (speakers of the selected cabinet, the mic, in/out and gate meters, a wide
  setup selector, a tooltip per control from `fx::amp_help`).
- **Latency.** A device's `latency_frames()` may follow a param (compressor look-ahead, distortion
  oversampling, mastering); the guitar amp's is a constant 35. The engine's PDC (section 2.4)
  recompiles when it changes and the changed cables' delay lines restart, which is audible once at
  the change (known limitation).
- **Bypass.** Every built-in `DeviceKind::Effect` (the six above, the guitar amp, the stereo delay,
  Character, and the spectrum and phase meter of section 6k) ends its params with `bypass` (a 0/1
  toggle, default 0, the key plugins use) and is registered as `Bypassable<D>`
  ([devices/src/bypass.rs](../crates/devices/src/bypass.rs)), a wrapper that forwards the whole
  `Device` trait to the effect, intercepts only that param and replaces `out_l`/`out_r` by the main
  inputs delayed by the current `latency_frames()` through a 5 ms per-sample crossfade (exact dry
  when bypassed, exact wet otherwise). The effect keeps running, so its tail, CV outs and meters are
  unaffected. Appending a param keeps every id and saved project valid (params are stored sparsely,
  a missing one is its default), so neither the project format nor the descriptor `version` changes.
  The UI places it as a BYPASS LED button in the left name strip of every such panel
  (`paint::bypass_button_rect`, hit as `Hit::Bypass` in [ui/src/rack.rs](../crates/ui/src/rack.rs),
  one `SetParam` per click, macro-driven refusal like any control) and veils the panel body while it
  is on (`paint::bypass_veil`); `autolayout::has_strip_bypass` is the one rule (a built-in effect,
  a note player or a container with a `bypass` param) and the auto-layout and the hand-made layouts
  leave the param out of the body. The console's bypass buttons set this param on the effects of a chain (Channel strip
  upgrades, section 5); the strip's own `insert.bypass` is a separate switch that is not delay
  compensated. See [DEVICES.md](DEVICES.md#effect-bypass).
- **Utility devices.** Gain Tool, Stereo Tool, Gate, Gate + Compressor, Sidechain Tool and Envelope
  Filter are `Bypassable` effects; Feedback Route (section 5), Audio Split/Merge and CV Split/Merge are
  plain utilities, and the merge inputs of the two splitters
  are `Direction::SummingInput` (the model's cable rules and the engine's summing and delay
  compensation treat them like a console bus input, in source order). They share the descriptor
  builders of `devices/src/tool_support.rs`, use the deterministic `log2_det`/`exp2_det`, and the
  gate is `atomik_dsp::Gate` in its deterministic mode (the channel strip's gate keeps its own maths
  and goldens). The Sidechain Tool's pump mode is a pure function of the transport position of the
  batch, so it needs no history and a locate gives the same gain; its curve mode plays up to eight points
  with bends (`atomik_dsp::duck_curve`) from every key hit or note, entering a retrigger where the curve
  reaches the dip that is playing. The points live in `hidden.*` params (`console::is_hidden_param`): stored
  and undoable like any param, offered to no list of targets, so the project format did not change. The
  Sidechain Tool has a panel of its own (`duck_panel.rs`, `rack/duck_input.rs`: the curve editor) and
  LEARN (`app_core_duck_learn.rs`: the key is rendered offline on the worker `atomik-duck-learn` by
  `render_device_key`, `atomik_dsp::duck_learn::learn` turns its hits into points, one undo step); the others
  have the automatic panel layout. The render test `utility_devices.rs` bounces each effect and checks the
  merge alignment. See [DEVICES.md](DEVICES.md#utility-devices).
- **Meters.** Reduction, levels, loudness and EQ band energies reach the UI through the device meter
  values of section 5; `RackView::set_device_meter_values` feeds the panels once per frame.
- **Sidechain helper.** The UI emits one `Command::Batch` (disconnect the old source, connect the
  new one, set `sc_enable`) so the helper is one undo step.
- **Insert chains** are described in section 6b (`ConsoleCommand::SetInsert`, `MoveInsert`); the
  example project is `examples/projects/fx-demo`, and `--stress-fx` is the app's stress project for
  them. The factory effect `atomik.character`) has the example `examples/projects/character-demo`
  and `--stress-character`; its two latency classes make the engine's latency epoch move in the
  middle of a song, so a bounce follows the epoch like the live app does
  (`render::plugin_pass::render_song_following_latency`, 64-frame batches, routing recompiled after
  the batch).

## 6d. Step sequencer and arpeggiator

Two original note devices (`DeviceKind::Utility`, listed together as "Players" in the browser) that
take notes in and send notes out over an ordinary Notes cable, plus gate, pitch, velocity and
trigger CV. Details, socket and param tables are in [DEVICES.md](DEVICES.md).

- **Transport position for devices.** `TimeInfo` carries `tick` (the transport position at the first
  frame of the batch, fractional), `ticks_per_frame` (constant inside a batch) and `loop_wrap`
  (offset and target tick of a wrap strictly inside the batch, at most one, only with a song).
  `Player::run` returns a `BatchTime` and `run_batch` copies it into the `TimeInfo` of every device.
  Without a song the tick is a free-running accumulator at the engine tempo. A wrap exactly on a
  batch boundary is reported as a tick jump only. A device never reads wall time.
- **Note clock** ([devices/src/note_clock.rs](../crates/devices/src/note_clock.rs)). Both players
  share one clock. Step `k` of resolution `s` ticks starts at `k x s` plus, for odd `k`, the integer
  swing shift `(swing x s + 100) / 200`; its On is due at the first frame
  `ceil((start - batch tick) / ticks_per_frame)` (a 1e-6 frame tolerance), so steps are derived from
  the tick every batch and never drift. A `JumpTracker` flags a locate, a start, a stop or a
  clock-source change (a tempo change inside a batch is tolerated in proportion to the change of
  `ticks_per_frame`); at a jump every sounding note is closed and the step counter is recomputed
  from the new tick. Offs that fall into a later batch wait in a fixed 64-slot `PendingOffs` table
  that stores absolute ticks. `NoteCore` writes notes and CV (gate, pitch `(key - 60) / 24` clamped,
  velocity, a 1 ms trigger) and keeps event offsets non-decreasing.
- **Pattern data hand-over.** Like sample maps: `Registry::register_with_player_data` marks the
  type, `Device::swap_player_data` is RT-safe and hands the old `Arc<PlayerData>` back, the engine
  queues `ControlOp::PlayerData` (applied at a batch boundary in queue order, so insert, then data,
  then routing) and retires the returned value through the garbage path (`Retired::PlayerData`,
  `HotEdit::PlayerData` offline). A swap never cuts a sounding note: its end is already scheduled.
- **Step sequencer** ([devices/src/step_sequencer.rs](../crates/devices/src/step_sequencer.rs)). Up
  to 8 patterns of 1 to 64 steps; per step on, key, velocity, gate length, probability and tie; per
  pattern a resolution (seven values) and a swing. Step index = global step number mod length, so
  locates and loops land in phase. Transport-synced only; with no data it plays nothing. Pattern
  switch `now` or at the pattern end, transpose, follow (a held key on `notes_in` transposes), mute,
  velocity and gate scale.
- **Arpeggiator** ([devices/src/arpeggiator.rs](../crates/devices/src/arpeggiator.rs)). Held keys in
  `notes_in`; modes up, down, up-down (no repeated ends), random, as-played and chord; octave range
  1 to 4; seven tempo-synced rates; gate, swing, hold, input or fixed velocity. With `run = always`
  and the transport stopped it uses an internal clock that starts at the playhead tick and advances
  at the current tempo, so a keyboard can be arpeggiated with the song stopped.
- **Determinism.** No running RNG: a probability test or an arpeggiator random pick is
  `hash64(device seed ^ random.seed, global step number, slot)`, so a bounce, a live run, a locate
  and a second loop pass give the same result from any start position and any block size.
- **Model** ([model/src/steps.rs](../crates/model/src/steps.rs),
  [steps_cmd.rs](../crates/model/src/steps_cmd.rs)). `RackDevice::steps` holds a `StepSeqDoc` (1 to
  8 `PatternDoc`s, each with always 64 steps so a longer length keeps old data). Commands
  `SetPattern` (drags merge), `AddPattern`, `RemovePattern` (lowers the `pattern` param in the same
  undo step), `DuplicatePattern`; all four are accepted by `Batch`. `SyncOp::PatternsChanged` covers
  edit, undo and redo. `to_player_data` and `player_data_for(device, registry)` build the engine
  value; the model refuses `steps` on devices that take no pattern data. Project format v7 writes
  `steps` only when present; v1 to v6 load.
- **UI** ([ui/src/seq_panel.rs](../crates/ui/src/seq_panel.rs),
  [arp_panel.rs](../crates/ui/src/arp_panel.rs), [seq_editor.rs](../crates/ui/src/seq_editor.rs)).
  Hand-made 2 U panels (8 pattern slots, preview and Edit for the sequencer; mode and rate buttons
  and a live held-keys display for the arpeggiator) and the grid editor pane `Pane::SeqEditor` (note
  lane plus velocity, gate and probability bar lanes, tie toggles, length, resolution and swing,
  playing-step cursor, audition). Both players publish meter values (sequencer: pattern, step,
  running; arpeggiator: held count and the next 8 keys) that the app feeds to the panels and the
  editor.
- **App and render** ([app/src/app_core_players.rs](../crates/app/src/app_core_players.rs),
  [render/src/project.rs](../crates/render/src/project.rs)). The app sends each sequencer's data
  right after its insert and on every `PatternsChanged` (one swap per edit, undo or redo); the
  renderer installs the same `player_data_for` maps before compile, so a bounce equals live
  playback. `examples/projects/player-demo` and `--stress-players` (4 chains of sequencer,
  arpeggiator and synth) are the demo and the stress load.

## 6e. The container device

`atomik.container` is a device whose inside is another rack: its own devices and cables, sixteen
macros (knobs or switches) that drive inner params, and note splits that route incoming notes by key
and velocity to inner instruments. It has stereo audio in and out, four CV inputs and a notes in, and it is saved
inside the project. The decisive design choice is **flattening**: the engine has no container concept. A pure
model function turns the tree into the flat device and cable lists the engine already understands,
so PDC, multicore levels, summing order, seeds and hot edits work on the exact flat graph unchanged.

- **Document** ([model/src/container.rs](../crates/model/src/container.rs)).
  `RackDevice::inner: Option<Box<ContainerDoc>>`, only on devices of `DeviceKind::Container`.
  `ContainerDoc { devices, cables, macros: [MacroDoc; 16], splits: Vec<NoteSplit>, open: bool }`
  (`open`: unfolded in the rack, view state only, stored as an optional `"open": true`); inner cables
  and splits use local keys, and the reserved key `boundary` names the inside edge (outputs `in_l`,
  `in_r` and the CV outputs `cv1` to `cv4`; inputs `out_l`, `out_r`). Limits: nesting depth 3, 32 devices per
  container, 256 inner devices per project, 8 splits, 32 targets per macro and 128 per container; no
  console devices, audio tracks, the interface or the split device inside.
- **Path keys.** An inner device is addressed everywhere by the keys from the top joined with `/`
  (`layer/synth`); `/` and `~` are not valid in a user key, so a path never collides.
  `RackDoc::resolve` and `resolve_mut` walk the tree; `RackDoc::device` stays top-level only so a
  site that forgot the expansion fails loudly. Song tracks, console sources, returns and the
  interface cannot name an inner device. Commands
  ([container_cmd.rs](../crates/model/src/container_cmd.rs)):
  `ContainerCommand::{AddInner, SetSplits, SetMacro, RemoveMacroTarget, RenameMacro, SetMacroKind, DuplicateContainer}`
  and the four moves below, plus path keys in the existing commands; each is one undo step, drags
  merge (ids from 7 x 2^40), and the new sync ops are `MacrosChanged` and `SplitsChanged`.
- **Expansion** ([model/src/expand.rs](../crates/model/src/expand.rs)).
  `RackDoc::expand(&Registry) -> ExpandedRack` gives the flat devices (path keys, macro-applied
  params, zones, steps), the spliced cables, the alias of a container to its `~split` device and the
  macro targets. Order: rack order, each container's slot taken by its `<container>/~split`, then
  its inner devices depth first; the engine id is the 1-based position in that list on load and
  bounce. Cables are spliced by tracing every flat input backward through boundaries and containers
  (a pass-through loop is caught by a hop limit and reported as a cycle); a boundary end with
  nothing behind it is a silent edge; a cable that would appear twice is rejected. A document
  without containers expands to itself. The cycle check of a cable runs on the expanded graph (Kahn
  over (device, stage)) only when the document has a container, so insert and return loops through
  containers are judged as the engine judges them.
- **Combine and uncombine** ([model/src/container_move.rs](../crates/model/src/container_move.rs)).
  `ContainerCommand::Combine { keys, name }` wraps top-level devices in a new container at the first
  device's slot, in one undo step; `Uncombine`, `MoveIntoContainer` and `MoveOutOfContainer` are the
  same splice the other way. A cable between two devices that move together stays a cable; a
  cable to a device outside goes through the container's own sockets: every distinct source that
  enters (and every distinct moving output that leaves) takes one of the two audio sockets, `_l`
  names first, and a notes cable becomes the notes input plus a full-range split to the device.
  The flat graph after the move is the one the rack had, plus the container's `~split` (bit for
  bit when no moved device draws from the project seed, which a device gets from its place in the
  flat list). A cable the container has no socket for (CV, a third audio input or output, a notes
  output, a second notes source) refuses the command and names each such cable, as do a frozen
  track, a macro bank that drives a moved device and a group over the device limits. A device no
  container can hold (a mixer strip, an audio track) is left in the rack with a note and the rest
  is combined; only a selection made of such devices is refused, naming the first. A container has
  one song track, one console channel and one FX return: at most one moved device may have them
  (its *owner*; two refuse the combine and are named), and the track, the channels and the return then
  name the container (the track keeps its place, folder and colour; its automation lanes are dropped,
  because they name params of the owner, and the track's note entry becomes the splits).
  `Uncombine` hands them to the inner device that stands out (the only instrument, else the end of
  the track's note chain, else the only device) and drops the container's macros and macro lanes.
  What a command dropped comes back as text from `RackDoc::container_move_notes`, which the app
  shows after the step; the Edit menu enables Combine from the same scratch-document plan the context
  menu uses (`combine_menu`, computed once per revision and selection). The moves work on top-level
  devices and top-level containers.
- **Bypass.** The container's `bypass` param (a toggle) is read by `RackDoc::expand`, so the engine
  needs no change: a bypassed container has no inner devices, cables or macros in the flat graph,
  and a signal into `in_l`, `in_r` goes straight to whatever `out_l`, `out_r` feed (its `~split`
  stays, with nothing behind it). The app inserts or removes the inner engine devices on the
  switch (`AppCore::sync_bypass`) and submits the routing between the two. The inside is cut at
  once: there is no crossfade and no tail, unlike the effect bypass of
  [decision J](DECISIONS.md#j-bypass-tail-policy). The param is a normal saved device param, so
  the project format is unchanged.
- **Split device** ([devices/src/note_split.rs](../crates/devices/src/note_split.rs)).
  `atomik.note-split` is internal (`INTERNAL_TYPE_IDS`, never in the browser): one Notes input,
  eight Notes outputs, six params per slot (`sK.on`, key and velocity ranges, transpose). A Note-On
  goes to every enabled slot whose ranges contain it; each slot keeps a per-key route (and an owner
  table per output key) so the matching Off follows the same route even after a param change, a
  retrigger closes the old note first and `AllOff` clears every table. Allocation-free, latency 0.
- **Boundary CV.** The container has the CV inputs `cv1` to `cv4`; inside, the boundary has the CV
  outputs of the same name. Nothing new is expanded: a cable from a CV source to `container:cv1` and
  an inner cable `boundary:cv1 -> synth:cutoff_cv_in` are spliced by the same backward trace as the
  audio sockets into the flat cable `source -> synth:cutoff_cv_in`, so the engine sees the patch
  one would have built without a container, bit for bit
  ([render/tests/container_modulation.rs](../crates/render/tests/container_modulation.rs) bounces
  both, and a property test in [model/tests/container_modulation.rs](../crates/model/tests/container_modulation.rs)
  compares the flat cables of random patches). One boundary CV fans out to any number of inner
  inputs; an uncabled one is a silent edge; a bypassed container passes none. CV is not
  summed or scaled here: use a device's modulation slots
  ([decision E](DECISIONS.md#e-modulation-framework)) for that.
- **Macro kinds and sixteen macros.** `MacroDoc::kind` is `Knob` or `Switch`; a switch gives its
  targets `min` for a value below one half and `max` from one half up (`MacroKind::effective`, one
  comparison, applied before `macro_value`, also to the points of a macro automation lane, so a
  switch lane should use stepped points). The container's `macro9` to `macro16` params follow `bypass`
  in the descriptor (ids 9 to 16) so older ids keep their place; one param is still driven by at most
  one of the sixteen. A macro bank keeps eight. The container panel shows the first eight knobs; the
  container editor lists the rest on its MACROS 9-16 tab and has the KNOB / SWITCH button.
- **Split pass flags.** `NoteSplit::pass` (`SplitPass { bend, cc, pressure }`, all on by default,
  which is what a split always did) says which channel messages the split sends to its device;
  notes always play. The flags travel to the split device as the params `sK.pass_bend`,
  `sK.pass_cc` and `sK.pass_pressure` (ids 48 to 71, default on; the expansion sets one only when it
  is off), and the editor's SPLITS list has the B, C and P chips.
- **Macros** (`macro_value` in expand.rs). `v = min + (max - min) x m`, clamped to the descriptor
  range, discrete and choice params rounded half up by comparison; only IEEE `+ - *`. A param is
  driven by one macro in the whole project; `SetParam` on a driven param is `MacroDriven`; an outer
  macro may drive an inner container's `macroN` (outermost first). Automation of `macroN` on a song
  lane expands (`expand_macro_lane`) into one lane per target with the points mapped by the same
  function, so it equals hand-made lanes on the targets bit for bit.
- **Format v8** ([model/src/container_io.rs](../crates/model/src/container_io.rs),
  [project/mod.rs](../crates/model/src/project/mod.rs)). `inner` is written after `steps` only when present,
  in the canonical form of the top level; v1 to v7 load; every violation is a load error naming its
  JSON path; a v1 to v7 device keyed `boundary` is renamed `boundary-N` with a warning. The
  container's macros, kinds and pass flags added later are optional fields of `inner` (the project
  format is 43): `macros` has 8 entries, or 16 when one of the last eight is used, a
  macro's `"kind": "switch"` is written only for a switch, and a split's `"pass_bend"`,
  `"pass_cc"`, `"pass_pressure"` only when `false`, so a project that uses none of it saves as
  before. A CV cable to a container is an ordinary cable to its socket `cv1` to `cv4`.
- **App and render** ([app/src/app_core_containers.rs](../crates/app/src/app_core_containers.rs),
  [render/src/flat.rs](../crates/render/src/flat.rs),
  [render/src/project.rs](../crates/render/src/project.rs)). Everything that feeds the engine reads
  one `RackDoc::expand` per edit: `rebuild_engine` inserts the expanded devices in order, then one
  routing; `DeviceAdded(container)` inserts its split and inside, `DeviceRemoved(container)` removes
  every id with the `container/` prefix. Macro values and `~split` params are sent right after the
  insert and again on `MacrosChanged` and `SplitsChanged`, as deltas only (`macro_sent`,
  `split_sent`). `target_of` resolves a container to its split, so the keyboard, audition, the
  editor's preview and a song track reach the inner instruments through the splits. The bounce
  builds the patch from the expansion (inner devices get generated patch ids; the engine id is the
  position), so live equals bounce; `expanded_ids` maps every device of the expansion to its id. A lint test pins the few raw
  `doc.devices` uses.
- **UI** ([ui/src/container_panel.rs](../crates/ui/src/container_panel.rs),
  [breadcrumb.rs](../crates/ui/src/breadcrumb.rs),
  [container_editor.rs](../crates/ui/src/container_editor.rs),
  [rack/container_input.rs](../crates/ui/src/rack/container_input.rs)). OPEN enters a container:
  the rack view works on a scope document (the container's devices and cables with local keys,
  boundary drawn as two pseudo-panels) and lifts what it emits to path keys and
  `ContainerCommand`s; a breadcrumb bar, Back, Esc and Backspace go up. A double-click on the
  panel or its fold arrow instead unfolds the container in place
  ([rack/inline.rs](../crates/ui/src/rack/inline.rs)): `ContainerDoc::open` (saved as an optional
  `"open"` flag, undoable with `ContainerCommand::SetOpen`, never seen by `expand`) makes the
  view work on a flat document, `flatten`, that holds each unfolded container's devices (path
  keys), its two strips (pseudo-devices `<path>/boundary` and `<path>/boundary~out`, half a unit
  tall on the front and whole on the back) and its inner cables with path keys, so a gesture
  emits path keys with nothing to lift. A `Frame` per unfolded container is drawn in the margin
  beside the column and is a drop target; `inline_drop` turns a drop into a reorder
  (`MoveDevice`), `MoveIntoContainer` or `MoveOutOfContainer` (planned on the project, kept
  in `host_doc` while an unfolded rack handles an event, so a refusal reads as the model's own
  message), and `can_cable` keeps a cable inside one container. The 2 U panel has the fold
  arrow, OPEN, EDIT, TO CH and eight named macro knobs; a right-click on an inner control maps it to a macro, a
  mapped control is locked, and Ctrl/Cmd+D duplicates the selected container. The container's name
  strip has the BYPASS switch of the built-in effects (one `SetParam` of `bypass`, the body veiled
  while it is on, locked while a macro drives it).
  [rack/container_move.rs](../crates/ui/src/rack/container_move.rs) holds the rack's side of
  Combine, Uncombine and the moves: the device menu and the Edit menu list Combine and Uncombine
  (Ctrl/Cmd+Shift+G and U), and Move Out of Container for a device of an open or unfolded container; the device menu
  enables each by planning the command on a scratch copy of the document
  (`combine_reason`, `uncombine_reason`, `move_out_reason`), so a refusal is the model's own message
  and the key or the Edit menu shows it as a toast. Dropping one device on the body of a
  container's panel (not its top and bottom 14 units, which still place a device next to it) sends
  `MoveIntoContainer`; dropping one on the project part of the breadcrumb of an open container sends
  `MoveOutOfContainer` to the slot after the container. A group is placed as any other move.
  `Pane::ContainerEditor` has a SPLITS tab (key by velocity map with draggable bars, audition) and a
  MACROS tab (name, value, targets with min and max).
- **Console.** A container has no channel at creation; TO CH adds one, and an effect container works
  in an insert chain or on a return (`console_insert` accepts `DeviceKind::Container`). Inner
  latency (a compressor look-ahead) is compensated by the existing PDC on the flat graph at summing
  inputs; parallel inner paths that meet in a plain input are compensated only with return
  alignment on, as at the rack level.
- **Tests as proof.** The expansion theorem (`render/tests/container_demo.rs`): the container bounce
  is bit-identical to a hand-built flat twin across host blocks and worker counts;
  `engine/tests/flat_container.rs` does the same for hand-built flat graphs with PDC and 264
  devices; `app` tests prove live equals bounce through edits, undo, redo and an engine restart.
  Example: `examples/projects/container-demo`; stress: `--stress-containers`.

## 6f. Plugin hosting

Third-party CLAP and VST3 plugins are rack devices: instruments and effects that are cabled,
automated, macro-mapped, saved and bounced like any built-in device. Both formats go through one
machinery (the host crate, the manager, the device, the scanner, the model and the UI); a format is
a backend behind `PluginBackend`. The user-facing behaviour is in
[DEVICES.md](DEVICES.md#third-party-plugin-devices-clapid-and-vst3id). By default every plugin
instance runs **in its own process** (the sandbox, below), so a plugin that crashes or hangs cannot
take the app down; Settings > Plugins can switch back to running plugins inside the app process
(applied at the next start). Scanning is always out of process.

- **Crates and the unsafe policy.** `atomik-plugin-host` (the generic `PluginDevice`, the
  `PluginManager`, the slot and hand-shake, scan, cache, catalog, journal, settings; `unsafe` only
  in `fpenv.rs`, the floating-point control register, and `ole.rs`, `OleInitialize` on Windows),
  `atomik-plugin-clap` (the CLAP backend over `clack-host` and `clack-extensions` 0.2.0, pinned),
  `atomik-plugin-vst3` (the VST3 backend over the `vst3` crate 0.3.0, pinned, bindings generated
  from VST 3.8; the host layer is ours) and `atomik-plugin-ipc` (the sandbox wire: the mapped block
  and the OS wake-up calls), the three crates that allow `unsafe` as a whole (every block with a
  `SAFETY:` comment), `atomik-plugin-scan` (the scanner child
  binary, both formats), and the in-repo test plugins under `crates/testplugins` (`core` holds the
  DSP shared by every test plugin, so a CLAP and a VST3 effect can be required to give the same
  bits; `clap-shell` is a raw `clap-sys` plugin shell and `vst3-shell` a plugin shell over the
  `vst3` crate's plugin-side helpers, so each host is tested against a plugin not built with its own
  library; `clap`, `vst3` and the `-bad`, `-hang`, `-crash`, `-leak` and `-variants` libraries of
  each format are cdylibs). `atomik-model` never names a plugin crate. `unsafe_code = "deny"`
  everywhere else;
  [plugin-host/tests/unsafe_policy.rs](../crates/plugin-host/tests/unsafe_policy.rs) pins the
  allowed places by name and requires a `SAFETY:` comment on every block.
- **One descriptor per plugin.** `descriptor_for(&PluginRef, &PluginLayout)` in
  `atomik-device-sdk::plugin` builds a `&'static DeviceDescriptor` once per distinct (plugin,
  layout) and **interns** it (a deliberate, bounded leak: at most 4 096 descriptors per process, 2
  048 params each, pinned by tests). Type ids are `clap.<id>` and
  `vst3.<32 hex digits of the class id>` (the same text on every OS); sockets follow the rule
  `in_l, in_r, sc_in_l, sc_in_r, notes_in, out_l, out_r, notes_out`; params are `bypass` then
  `p<plugin id>` keys with the plugin's own plain value (CLAP value; VST3 normalised, stepped as
  index). `Registry::register_plugin` keeps plugin types in their own map; `Registry::create` of a
  plugin type builds the stand-in device, the real one comes from the manager.
- **The device (RT side).** [plugin-host/src/device.rs](../crates/plugin-host/src/device.rs):
  `PluginDevice` implements `Device` for every backend through the object-safe `PluginProcessor`. It
  calls the plugin with exactly **64 frames** per batch (so live, bounce and every host block feed
  it identical blocks, drains a fixed param table into coalesced events at the batch start, turns
  `NoteKind`s (now including pitch bend, CC, pressure and note expressions) into note and MIDI
  events with host note ids, passes the batch `TimeInfo` as the transport, writes the output through
  a 5 ms bypass crossfade to the dry main input (an effect's dry input is delayed by the plugin's
  reported latency, so a bypassed latent plugin stays where the delay compensation put it), applies
  the sleep and skip rules (they depend only on batch content), sets flush-to-zero around `process`,
  and turns an error status, a panic or a non-finite output into FAILED (silence, or dry for
  effects). Nothing in this path allocates or locks (`assert_no_alloc` tests at host blocks 1, 64,
  333, 480, 512 with workers 1, 2, 4 and a restart in the middle).
- **Lifecycle (the manager).** The winit thread is the plugin main thread. `PluginManager` owns each
  instance's main-thread half, moves only the audio processor half into the `PluginDevice`, and
  talks to it through a `PluginSlot` (atomics) and wait-free `rtrb` rings. A **restart** (sample
  rate, latency or param-table change, RELOAD) is a hand-shake: the manager requests a stop, the
  device answers silence or dry and hands the processor back at the next batch, the manager
  deactivates and activates again and sends it back; the device then re-sends every param the rack
  ever set. Dropping a device never destroys the plugin on the dropping thread: `Drop` posts to the
  **reaper queue**, and `poll` tears down on the main thread. An engine restart keeps the living
  instances (`retain`, then `reattach` at the new rate: nothing is created again). Plugin callbacks
  into the host only post to a bounded message ring that `poll` drains; a full ring drops and
  counts.
- **Latency and PDC.** The reported latency (clamped to 1 s) is an atomic on the slot;
  `Device::latency_frames` reads it and the engine's existing PDC does the rest. A plugin announces
  a change (CLAP `latency.changed`, VST3 `restartComponent(kLatencyChanged)`), the manager restarts
  it, the backend reads the latency again after the re-activation (the VST3 backend also after every
  `setActive` and every state restore), and the app recompiles the routing once the engine publishes
  it (the known single discontinuity of effect latency changes). The same loop runs in a bounce (see
  Render).
- **Scanner, cache, consent.** Plugins are off by default. `atomik-plugin-scan clap <path>` loads
  one file in a child process, activates every plugin once, and prints a JSON `ScanResult`; the app
  runs up to two children with a hard timeout (default 30 s, 5 to 300), kills a hang, and blacklists
  files that crash, hang, print garbage or are not plugins until they change or the user retries.
  `plugins.json` (atomic write, versioned, incremental rescan by size and mtime) feeds
  `PluginCatalog`, which registers descriptors **without loading any plugin file**. A project never
  carries a path: a device resolves only by (format, id) against that cache. A project that uses
  missing or disabled plugins opens with **stand-ins** that keep cables, channel, song track,
  automation, macro targets, pins and the state blob byte for byte, and RELOAD or a rescan swaps the
  real plugin in.
- **Plugin manager.** What the user decides about a plugin (a star, tags, hidden by Disable, the quarantine after
  three failures in a row) is kept by plugin id in `plugins_user.json` beside the settings, apart from the scan cache
  and the catalog, which it never touches: the browser and the menus hide a disabled or quarantined plugin, while a
  project that uses it still resolves it and loads it. The scan cache keeps when each file was scanned.
- **State and undo.** The plugin state is an opaque byte blob (`state.save` into a buffer), captured
  before every save, when the editor closes, a quiet second after `state.mark_dirty`, before an
  engine restart and before the device is removed; it lives in the document as `PluginDoc::state`
  through `PluginCommand::SyncState`, which is not an undo step. Caps: 16 MiB per plugin, 128 MiB
  per project. On restore the state goes first and the rack's saved params after. Edits made in the
  plugin's own window become `SetParam`s through a gesture merger (merge ids from 8 x 2^40, one
  gesture is one undo step, flagged so they are not echoed back to the plugin); a patch change
  inside the plugin (a preset picked in its window, say) is one undo step too: `PluginCommand::SetState`
  (undoable, merge ids from `8 x 2^40 + 2^38`) at the next quiet capture, when the editor closes
  and before an undo, unless a param was turned meanwhile; undo gives the old blob back to the live
  plugin through `SyncOp::PluginStateChanged`. Presets: `PluginInstance::list_presets` /
  `load_preset` (VST3 `.vstpreset` files; CLAP preset discovery and preset-load in
  [plugin-clap/src/presets.rs](../crates/plugin-clap/src/presets.rs), with short stable ids so the
  plugin is only asked for locations it declared; the null backend has fixed ones), the sandbox
  messages `ListPresets`, `LoadPreset` and `Presets`, and `AppCore::load_plugin_preset` as one
  `SetState`. `PluginInstance::read_params` (sandbox: `ReadParams` and `ParamValues`) gives the host
  values the plugin holds, and `AppCore::refresh_knobs` writes the changed ones into the document as
  `SetParam`s folded into the same undo step, without echoing them back to the plugin.
- **Format v9.** `plugin` is written after `inner`: format, id, name, vendor, version, `path_hint`
  (shown, never used), a compact `layout` (sockets and **only the used param rows**), base64
  `state`, pins, run mode, powered. Params are saved sparsely. Canonical save, load, save is
  byte-identical; every violation names its JSON path; v1 to v8 load unchanged. A param key a newer
  plugin no longer has is a **dead key**: kept in params, lanes and macro targets, ignored at send
  time, never a load error.
- **Editor windows.** The core never touches a window: `PluginWindowRequest`s go to the platform
  layer, which creates one plain winit window per editor (no wgpu surface), hands the native handle
  (Win32, AppKit, Xcb, Xlib) to the backend for `gui.set_parent`, and forwards resize and close.
  Wayland has no embedding, except that a sandboxed plugin's process forces winit's X11 backend
  (XWayland) when `DISPLAY` is set. In process, or without `DISPLAY`, the PARAMS pane (a searchable,
  module-grouped list of every param) is the fallback and the editor for plugins without a GUI.
- **Crash handling.** Sandboxed (the default), a plugin crash ends only its own process: the device
  turns CRASHED and RELOAD creates a new instance (see the sandbox below). In process, a native
  plugin crash takes the app down and the app reduces the damage: the crash journal
  `plugins-in-use.json` (offers DISABLE at the next start; sandboxed instances are not journaled),
  recovery snapshots (below), and a panic boundary around `process`.
- **Autosave and crash recovery** ([recovery.rs](../crates/app/src/recovery.rs),
  `app_core_plugins.rs`). Every project, with or without plugins, is snapshotted:
  `AppCore::plugin_tick` queues a snapshot when the document differs from its saved state and from
  the last snapshot (the document revision, see 2.2) and the autosave period has passed (Settings,
  Workflow, **Autosave**: off, 1, 2, 5 (default), 10, 15 or 30 minutes; while a plugin is loaded
  every 60 s whatever changed, and after a plugin state capture). The frame only clones the
  document; one `RecoveryWriter` thread serialises, writes
  `<config>/recovery/<project>-<sequence>.recover.atomik` through a temporary file and a rename, and
  prunes to the newest three, so two snapshots never prune each other. A snapshot of a project with audio
  has a note `<snapshot>.bundle` (`RecoveryDir::bundle_note_path`) with the bundle folder, written and
  pruned with it; `open_recovery` hands that folder to the open like a template's, so the audio is copied
  into the session's own bundle and loaded (no note, or a folder that is gone: the clips show as missing).
  A session marker
  `session-<pid>-<n>.lock` in the same folder stays locked by the running process (the operating
  system drops the lock when the process ends in any way); a clean quit (`shutdown_plugins`) removes
  it, so a marker that can be locked at start belongs to a run that ended unexpectedly and shows the
  unclean-exit banner with OPEN RECOVERY when a snapshot newer than the project file exists.
  `settings.json` and `ui-state.json` are written with the same temporary-file-and-rename write
  (`recovery::atomic_write`); a file that does not parse is renamed `<name>.bad`, the defaults are
  used and the status line says so once.
- **Orphaned recordings.** An audio take streams to `Audio/rec-<pid>-<take>-<n>.wav` (2.6). At start
  and when a project opens, `recover_orphan_recordings` scans (bounded) the opened bundle's `Audio/`
  and the session scratch bundles in the temp folder for such files whose process has no live marker
  (the scratch scan also skips files touched in the last minute: that folder is shared by every
  Atomik of the user). Each file goes through the import path as a track `Recovered take N` with one
  clip at the start of the song, so the frames up to the last header refresh come back (at most
  about a second is lost); one undo removes track and clip, and the `rec-` file is deleted only after the
  project is saved.
- **The sandbox** ([plugin-sandbox](../crates/plugin-sandbox/src),
  [plugin-ipc](../crates/plugin-ipc/src), [plugin-hostd](../crates/plugin-hostd/src)).
  `SandboxBackend` is a `PluginBackend` like the format backends: `instantiate` starts one
  `atomik-plugin-host <format> <file> <id> --block <path> --doorbell <name> --parent-pid <pid>`
  child per instance (next to the app, or `atomik --plugin-host`; `atomik-render` finds it next to
  itself). The child hosts the instance with the real CLAP or VST3 backend; its main thread is the
  plugin main thread (control channel, timers, the editor window) and an audio thread serves the
  shared block. The parent never loads a plugin file.
  - *Control channel.* The child's stdin and stdout carry frames (a little-endian `u32` length, a
    JSON message, an optional binary payload such as a state blob). Every request has an id and
    exactly one reply; plugin callbacks come back as notifications and reach the instance's
    `HostMessenger` on a reader thread. The child sends every pending notification before each
    reply, and a bounce asks for a `Flush` after every batch, so a callback reaches the host before
    the next batch exactly as in process (a bounce counts in batches, never in time). Main-thread
    calls have timeouts. The ones that matter (10 s create, activate and state; 2 s deactivate and
    flush) kill the child when they are not answered. The cosmetic ones (param text, default pins,
    layout changes, closing the editor; 250 ms) only drop the request: the last text stays up, the
    instance counts as slow after three misses in a row and is then asked once a second (a
    warning says so), and the child is stopped after twelve. A reply that comes late is ignored,
    except a layout change, which the child says only once and the instance keeps. The child
    takes a private handle to its real stdout for frames and
    points its stdout at stderr (`take_frame_stdout`), so a plugin that prints cannot corrupt the
    stream.
  - *Audio.* One shared block per instance (a mapped file of 54 528 bytes: header, one request area,
    one response area with the main and the aux output channels, raw `f32` bits and fixed-capacity
    events), exchanged by sequence numbers.
    `RemoteProcessor::process` copies the block in, rings the doorbell (Windows named event, Linux
    shared futex, macOS shared ulock; one atomic swap and no system call while the child is awake)
    and spins until the answer, the death flag or the deadline. Live, the deadline is one batch
    period: a late answer gives `ProcessStatus::Late` (dry for an effect, silence otherwise, the
    `late` meter counts it), and the events of a batch that could not be sent are carried to the
    next one at offset 0, so a note-off is never lost. A bounce (`SandboxConfig::bounce`, set by
    `atomik-render`, also for `--plugin-mode realtime`) and an offline instance wait for every batch
    up to a 30 s watchdog, so a sandboxed bounce is byte-identical to an in-process one.
  - *Crash.* When the child ends, the reader thread marks the block dead (`DeathHandle`) and posts
    `ProcessExited` with the exit code or signal and the last stderr lines; the next `process`
    returns `Lost`, the device turns `Crashed` (dry or silent, never called again), and the manager
    emits `ManagerEvent::Crashed`. A lost instance fails every call at once; `restart` and `reload`
    refuse it, and the app's RELOAD creates a new instance from the document's last captured state
    and the rack's params.
  - *Supervision.* A child never outlives the app. On Windows it joins a job object that ends
    its processes when the app's last handle to the job closes, whatever ends the app; every
    child also runs a watchdog thread that ends it a second after its parent's process is gone,
    and three seconds after the control channel closes, even when its main thread is stuck inside
    a plugin call (a stuck child is how an orphan used to survive). Live, `RemoteProcessor` counts
    late batches in a row (plain integers on the audio thread, no lock): after three seconds' worth
    the block is marked dead and the main thread ends the process with a reason that begins `HUNG`;
    the app reloads such a plugin from its last captured state, twice in a row, then leaves it
    CRASHED for the user (a bounce never counts: it waits for the watchdog). When instances are
    dropped (closing a project, quitting) the manager asks every child to end first and waits for
    them on one shared deadline, so slow children cost one second together, not each. Everything a
    child sends is bounded before it is allocated: replies and notifications at 1 MiB of JSON,
    blobs only on the state, log lines at 4 KiB and editor sizes at 16 384 pixels (others are
    refused and counted). A burst of parameter edits reaches the app as the last value per
    parameter, in order with the gesture's begin and end. The block file has a random name, is
    created exclusively with mode `0600` on Unix, and files of apps that crashed are swept at the
    next start. The scanner's output is read to its end whatever its length, and over the cap the
    scan fails at once as too large. A hostile app process that shares the user account is out of
    scope: the block is validated by the child, not protected from its own parent.
  - *Editors.* A sandboxed instance has an `EditorKind::External` editor: the child opens a
    top-level window, owned by the app's main window on Windows, and reports `GuiClosed`. The app
    creates no window for it.
  - *Tests.* The wire ([plugin-ipc/tests](../crates/plugin-ipc/tests): layout, exchange,
    cross-process doorbell, no-alloc, stdout protection), the parent with a fake child
    ([plugin-sandbox/src/tests](../crates/plugin-sandbox/src/tests)), the child driven directly and
    end to end through `SandboxBackend`, the manager and the engine
    ([plugin-hostd/tests](../crates/plugin-hostd/tests): bit-identical to in process for the CLAP
    and VST3 test plugins across blocks and workers, crash, kill and reload, hangs, latency
    restarts, state, a plugin that prints), and
    [render/tests/plugin_sandbox_bounce.rs](../crates/render/tests/plugin_sandbox_bounce.rs) (both
    plugin demos bounced byte-identical with the sandbox on and off, offline and realtime). The
    child's built-in `null` test plugins are selected with `ATOMIK_PLUGIN_HOST_FORMAT=null`, a test
    hook.
- **The VST3 backend** ([plugin-vst3/src](../crates/plugin-vst3/src)). `module.rs` loads a `.vst3`
  file or bundle per OS (Windows DLL and `InitDll`, macOS bundle and `bundleEntry`, Linux
  `ModuleEntry`) and keeps the module alive while a plugin of it lives; `layout.rs` lists the audio
  classes, negotiates the buses (stereo or mono main, an optional stereo sidechain, everything else
  deactivated, surround refused with the reason) and reads params (hidden, read-only and
  program-change ones skipped, units as modules), `IMidiMapping` and note-expression support;
  `instance.rs` is the main-thread half (component and controller created and connected through two
  proxies so neither holds the other, the controller brought in step with the component's state, the
  host `IComponentHandler`, the editor `IPlugView` with an `IPlugFrame` that is also an `IRunLoop`);
  `processor.rs` is the audio half (fixed-capacity `IParameterChanges` and `IEventList`,
  `silenceFlags` per channel, `ProcessContext` from the batch transport, host note ids, pitch bend
  and CCs through `IMidiMapping`, per-note expressions, output notes back as `notes_out`). Params
  are the **normalised** value, a stepped one (`stepCount` > 0) its step index. In VST3 the
  controller talks to the processor only through the host, so an editor's `performEdit` is pushed to
  the processor at the next batch and every value the rack sets is mirrored to `setParamNormalized`
  on the main thread, so an open editor and the saved controller state follow the rack. The state
  blob is `AVS1` + the component section + the controller section (each a little-endian `u32` length
  and its bytes); restore is `setState`, `setComponentState`, then the controller's own `setState`.
  The editor is hosted in the same separate window as a CLAP one; on Linux the host's `IRunLoop`
  pumps both the timers and the file-descriptor handlers (`registerEventHandler`; each tick a
  non-blocking `poll(2)` on the main thread calls the ones that are ready), so an editor that needs
  either one works.
- **Render.** `atomik-render` builds plugin devices through the same manager
  (`render/src/plugins.rs`; both backends, a file routed to its backend by its format,
  `format_of_file`) after registering the installed plugins before the project loads.
  `--allow-plugins` is required (exit 2 otherwise), `--plugin-mode offline|realtime` (offline by
  default; realtime matches what was heard), `--missing-plugins error|stand-in`, `--plugin-cache`.
  The report line says `plugins: N (determinism not guaranteed)`. A project with real plugins is
  bounced by `render/src/plugin_pass.rs` instead of the plain offline loop: a silent **pre-roll**
  (transport stopped) until no plugin has restarted or changed its latency for 8 batches, then the
  song in **64-frame batches**, with a plugin-manager poll and, when the engine published a new
  device latency, a routing recompile (`Graph::topology`, `EngineHandle::fill_latencies`,
  `submit_routing`) after **every batch**: the offline form of `AppCore::frame`. Counted in batches,
  never in time, so it is deterministic and independent of workers (and `--block` has no effect on a
  plugin project); a bounce therefore has the delay compensation a project has live, from its first
  frame, and follows a latency that changes mid-song.
- **Determinism, defined.** The host feeds the plugin identical inputs (64-frame blocks, event
  offsets and order, param values, transport, note ids) for any host block and worker count, so the
  output is bit-identical for plugins that are deterministic functions of their inputs (all the test
  plugins). Real plugins that use time, random seeds or hardware state are not, bounce is not
  guaranteed equal to live for them, and golden hashes are never taken over real plugins.
- **Tests as proof.** `plugin-host/tests/engine_integration.rs`, `plugin-clap/tests/engine_clap.rs`
  and `plugin-vst3/tests/{process,lifecycle,state,engine,leak,cross_format,vst3_host}.rs` (the
  suites loop over both formats; the CLAP and the VST3 test effect give the same bits) (the chain
  equals the reference DSP and is bit-identical across blocks and workers under the allocation
  guard, PDC from a reported and a changed latency, thread-check per worker count, a restart in the
  middle of a render), `plugin-clap/tests/{process,lifecycle,state,gui,leak,crash,hang}.rs` against
  the real backend and the test plugins, `plugin-scan/tests/scanner.rs` (real child process),
  `render/tests/plugin_demo.rs`, `plugin_demo_vst3.rs` and the app's `app_core_plugins*_tests.rs`
  (live equals the realtime bounce, including a saved latency and a latency that moves mid-song in
  both formats, stand-ins, one instance per frame, engine restart keeps instances, journal and
  snapshots, `LayoutChanged` and relink rebuilds), `--stress-plugins` and the 200-add-and-remove and
  500-instance loops. Examples: `examples/projects/plugin-demo` (CLAP) and `plugin-demo-vst3` (both
  formats in one project).
- **Known limits.** No about box (the VST trademark line is in `--help`, the README and DEVICES.md);
  editor window positions are not remembered across sessions; no Wayland embedding for in-process
  editors or without `DISPLAY` (sandboxed editors use XWayland); macOS and Linux paths are
  compile-checked (type-checked and linted) but have not been run against real plugins; the
  sandbox's Linux and macOS wake-up and stdout redirection run against the real child in the CI
  job "sandbox end to end" and in the test matrix, and have not been checked by hand with a
  window; the editor window of the child is a utility window on X11 (a hint) and its macOS process
  is an accessory (no Dock icon), neither checked by hand; one process per
  instance costs memory with many instances; a sandboxed editor is a separate window, not embedded.

## 6g. Input, commands and the keymap

Every key the app understands goes through one registry, one data-driven keymap and one router. The
cheat sheet is [KEYBOARD.md](KEYBOARD.md). The standard default profile, the daily keys and the tool
palette are described in the **Daily keys** bullets below, the clipboard, rack multi-selection and
the context menus in the **Clipboard, selection and context menus** bullet, and the auto-routing
creation chain in the **Creation chain** bullet.

- **Registry and keymap data** ([ui/src/keymap](../crates/ui/src/keymap), pure, no window system).
  `Registry::bundled()` lists every command (553) by a stable dotted id with label, group, default
  context, an optional choice argument, and the flags `repeatable`, `menu` and `can_decline`.
  `default_keymap.json` (embedded) holds two profiles, `standard` and `legacy` (the keys of earlier
  versions), as `{ command, arg?, context, chords, os? }`. `Cmd` in a chord is the logical primary
  modifier (Control on Windows and Linux, Command on macOS); `os` overrides carry only the
  exceptions. `Chord::parse` and its canonical printer round-trip; `PhysChord` is a chord resolved
  for one OS and matched **strictly** (Control is never Command, Super never matches `Cmd` on
  Windows and Linux).
- **Contexts and the router.** `Context` is `Global`, `Rack`, `Sequencer`, `Editor`, `Mixer`,
  `Browser` or `Text`. The shell (`Shell::context_stack`) builds a `ContextStack` from the focused
  pane and from whether a view owns a text field (`is_text_owner()` on the rack, the sequencer and
  container editors, the parameter pane, the file browser and the console; the tempo field takes
  every key before the router). `Keymap::resolve` returns the bindings of the stack, most specific
  context first, then `Global`; the first command the shell can run wins, a command that cannot (a
  view key not migrated yet, or one that `can_decline`) falls through to the next and finally to the
  focused view. While `Text` is on the stack only chords with the primary modifier, function keys
  and Esc resolve; everything else goes to the text owner. The `Router` also tracks held keys: a
  press of a key already held is a repeat and reaches only `repeatable` commands (undo, redo, zoom,
  arrows of the editors); a lost window focus clears the held keys.
- **Input layer** (`UiEvent`, [app/src/translate.rs](../crates/app/src/translate.rs)).
  `Key::Numpad(NumpadKey)` and `Key::Insert` come from winit's physical key and location, so the
  numpad is distinct from the main row with NumLock on or off; a numpad key no command takes reaches
  the views as the character it types. `Key::Digit(n)` is the physical main-row digit, produced only
  on macOS with Command and Option held (the Mac secondaries). `UiEvent::ModifiersChanged` is
  emitted once per real change; releases are delivered for every key. Ctrl+click on macOS becomes a
  secondary click (and its release follows). Pointer gestures keep the lenient
  `Modifiers::command()`.
- **Menus and shortcut text** ([app/src/commands.rs](../crates/app/src/commands.rs)).
  `AppCommand::command_id()` and `from_command` are a bijection onto the registry's menu commands;
  `menus(state, keymap)` takes each item's shortcut text from the active keymap and OS (the first
  chord of the command), so a remap changes the menu. No shortcut string literal remains in the file
  (a test greps for it); native macOS accelerators derive from the same text.
- **The user keymap** ([ui/src/keymap/user.rs](../crates/ui/src/keymap/user.rs),
  [editor.rs](../crates/ui/src/keymap/editor.rs),
  [app/src/keymap_store.rs](../crates/app/src/keymap_store.rs)). `keymap.json` next to
  `settings.json` is a diff over the profile: an entry holds the full chord list of one (command,
  argument, context) in logical `Cmd` grammar for every OS and replaces the profile binding,
  including its OS overrides; `removed` takes a chord away; `roles` overrides the modifier roles;
  the chosen profile is stored too. Loading is lenient (unknown fields ignored, unknown command ids
  kept and not applied, wrong-shaped entries skipped with a note); a file that is not a JSON object
  is renamed `keymap.json.bad` and the defaults run with a status message. Saving is canonical
  (sorted, no timestamps) and atomic (temp file, rename). Building a keymap never fails: bad,
  reserved and unknown entries are dropped and listed in a `Report`; two commands on one chord in
  one context are a **conflict** (an error in the report), a context binding over a `Global` one is
  a **shadow** (a note). Reserved chords (Esc, Alt+F4, Alt+Tab and Super chords on Windows and
  Linux, Cmd+Q, Cmd+H, Cmd+M and Cmd+Tab on macOS, plain Space, Enter and Tab in `Text`) cannot be
  assigned, but a default may keep its own. The shell hot-reloads: applying the dialog swaps the
  router and the role table at once, and `poll_status` compares the file's modification time and
  size and reloads an edited or deleted file.
- **Menus that scroll and stay on screen**
  ([menu_layout.rs](../crates/ui/src/menu_layout.rs), [menubar.rs](../crates/app/src/menubar.rs)). The in-window
  bar lays a dropdown or a flyout out with `menu_layout::layout` for the window's size (`MenuBar::set_window`, set by
  the shell before every event): a list taller than the room below the bar is cut to it (`EDGE_MARGIN` left under
  it) and scrolls by the mouse wheel over it, by a pointer resting on the strip at its top or bottom
  (`MenuBar::tick`, from `Shell::frame_at`), and by Up, Down, Home and End, which keep the lit row whole in the view
  (`offset_to_show`). Hit boxes, hover and the submenu logic read the scrolled rows (`ListLayout::rows` are cut to the
  view; a row scrolled out has none). A flyout starts level with its row, moves up before it scrolls, and opens on
  the left of its parent when the right side leaves the window. Submenus nest to any depth (`MenuBar::subs`, one entry
  per open list, `deep_look` for the shell's repaint check); the macOS native menu builds the same nesting
  recursively (`native_menu::fill`) and rebuilds a submenu when a label, state or check in it changes. No top-level
  menu has more than `MAX_MENU_ROWS` rows: File, Edit, View, Mixer, Track and Audio keep their long runs in submenus by
  theme, and the analysis features (Sound Analyst, Reference Panel and Reference, Translation Lens, Check Mix, Masking
  Map, Kick and Bass Check, Will the Bass Read, Find Repetition, Delivery Targets) have the Analyze menu after Mixer.
  Only the menu path of a command changed: `crates/app/tests/golden/menu_commands.txt` pins every item and its
  command and `menu_tree.txt` the tree (`ATOMIK_UPDATE_GOLDEN=1` regenerates them).
- **The unsaved-changes question, Close and the recent songs**
  ([shell.rs](../crates/app/src/shell.rs), [recent.rs](../crates/app/src/recent.rs),
  [menubar.rs](../crates/app/src/menubar.rs)). New, Open, Close, Quit and a project to open (a
  library pick, a drop, Open Recent) with unsaved changes raise
  `ShellRequest::ConfirmSave { then: PendingAction }`; the platform shows Yes (save), No (discard)
  and Cancel (nothing) and calls `Shell::save_then`, `Shell::discard_then` or nothing. A song with
  no file yet answers `PickSaveFileThen`, and the platform calls `Shell::save_to_then` with the
  picked path; a cancelled picker or a failed save ends the pending action with the song still open.
  The window's close button and Cmd/Ctrl+Q are `AppCommand::Quit`, so they take the same path.
  `AppCore` keeps a `RecentFiles` (eight project paths, newest first, de-duplicated by `PathKey`;
  `recent.json` next to `settings.json`, written atomically) that a successful `open` or `save_as`
  updates; up to `MAX_PINNED` songs can be pinned (`pin_recent`, listed first, kept by Clear Menu,
  a `pinned` list in the file that older builds ignore, shared by the open songs through
  `Workspace::sync_shared`); a patch is not listed and a missing file leaves the list when chosen. The menu model has
  `MenuEntry::Submenu`; File > Open Recent is built by `menus_with_recent` from the core's list
  (`MenuItem.label` is a `Cow`, the recent songs being named after their files). The in-window bar
  draws a flyout beside the row (hover, click, Right and Left, Enter) and the macOS native menu
  rebuilds the submenu when its entries change (muda cannot relabel an item). The Workflow switch
  `open_last_project` (`workflow.json`) makes the platform call `Shell::restore_last_project` at
  start-up when the command line names no project (`Cli::starts_empty`). The platform's start-up
  path is `Shell::start_without_project`: the last song, else the default song and the start
  screen. The start screen ([app/src/start_screen.rs](../crates/app/src/start_screen.rs),
  `StartScreen`) is a shell modal like the About box; it lists New, Open, up to eight templates
  and the recent songs (name, folder, day of the file), and a pick returns the `AppCommand` of the
  File menu row (`New`, `Open`, `NewFromTemplate`, `OpenRecent`), so the question about unsaved
  changes and the missing-file messages are the menu's own. The `show_start_screen` flag
  (`workflow.json`, default on) is the box's check box and a row of Settings > Workflow; File >
  Start Screen opens it at any time. A template under the pointer or the keys has a card with its
  `TemplatePreview` (`AppCore::template_preview` loads the file without opening it, once per
  template while the box is open), and a pin button on a recent row returns `StartOutcome::TogglePin`.
  The genre templates of the factory `Templates` folder come first in `AppCore::templates` (then the
  user's own, sixteen in all) and open like any template.
  The headless and MCP sessions never call `start_without_project`, so they never show it. The shell
  gives the box a strip of cards (`StartScreen::with_cards`): the factory examples
  (`AppCore::examples`, the projects of the factory `Projects` folder with a line each, opened by
  `AppCommand::OpenExample` through `AppCore::open_template` as unsaved songs) and a guide card that runs
  `AppCommand::GuidedTour`; the `show_guide_card` flag (`workflow.json`) is the second check box of the box.
  The guided first song ([app/src/guide.rs](../crates/app/src/guide.rs), `Guide`) is a floating
  panel of the main window, not a modal: `Shell::guide_event` hands it only the pointer events inside its
  rectangle and never a key, and nothing of it is saved. `Progress::read` reads the song (tracks, note
  clips, the loop switch, a drum machine, a moved channel fader, an effect, the file and
  `AppCore::exports_finished`) each frame (`Shell::sync_guide`) and `Guide::update` moves on when the step on
  show turns from not done to done.
- **Settings tabs** ([app/src/settings_dialog.rs](../crates/app/src/settings_dialog.rs),
  `keyboard_tab.rs`, `workflow_tab.rs`). Audio (unchanged), Keyboard (a `KeymapEditor`: list by
  area, search, learn by pressing, add, clear, reset one or all, the conflict box with Swap,
  Unassign and Cancel, shadow notes, profile chips, modifier roles, export and import through file
  pickers), Workflow (auto-route, create keeps the browser open, follow the song, autosave, the
  record mode and the keep-events policy; saved in `workflow.json`, unknown fields kept) and
  Interface (next bullet). Nothing is saved until Apply; Cancel and Esc discard every draft.
- **Interface preferences** ([app/src/interface_tab.rs](../crates/app/src/interface_tab.rs),
  [app/src/about_box.rs](../crates/app/src/about_box.rs), [ui/src/theme.rs](../crates/ui/src/theme.rs)).
  `workflow.json` also holds `theme` (`dark`, `light`, `system`), `app_zoom` (75 to 200 percent in
  steps of 25; a hand-edited value snaps to the nearest step), `knob_range` (`normal`, `precise`,
  `very_precise`: the drag a full sweep takes is one, two or four times as long, for the rack, the
  mixer and the container editor) and `value_tooltips` (hovering any rack control shows its value,
  not only an automated or driven one); a file from before has the defaults, which are the look and
  the feel the app always had. The Interface tab edits them in a draft (`UiPrefs`) that
  `SettingsDialog::preferences` merges into the Workflow draft, and View > Theme and View >
  Application Zoom (`AppCommand::SetTheme`, `AppCommand::AppZoom`) apply and save at once. None of
  it touches the project or the engine.
  - **Theme.** `Theme::dark` and `Theme::light` fill the same tokens. What the sequencer views used
    to spell as colours (lanes, piano keys, the veil outside a clip, the mute colour) is a token
    too, faint overlay lines use `Theme::overlay` (white on dark, near-black on light) and the type
    over a lit control or an accent fill is `on_accent`, so no view keeps a colour of one theme.
    `ThemeChoice::theme(system_dark)` picks the theme in use; `Shell::set_system_dark` is told the
    operating system's setting at start and when it changes. `Shell::apply_theme` hands the theme to
    `AppCore::set_theme` and to every open panel; each view's `set_theme` derives its `SeqTheme`
    again and drops what was recorded with the old colours (the rack and the console bump the
    revision of every cached list, the arrangement syncs again). `contrast_ratio` tests in
    `theme.rs` hold the type, the muted type, the focus ring and the lamp colours of both themes
    to their WCAG ratios.
  - **Application zoom.** The shell draws and lays out in logical units at the scale each window
    was told. The platform tells every window `Shell::ui_scale(display scale)`, the display scale
    times `Shell::app_zoom`, and works the logical size and the pointer positions out with it, so
    layout, drawing and hit tests follow with no other change; `App::apply_zoom` tells the windows
    again when the zoom changed.
  - **Help, Window and the divider.** F1 (`help.keyboard_shortcuts`, both profiles) opens the
    settings dialog on the Keyboard tab, the searchable list of every command and its keys.
    Documentation and Report an Issue are `ShellRequest::OpenUrl` with the project's two public
    addresses (the platform opens the browser); the About box (`AboutBox`) shows the version of the
    build, the GPL-3.0-or-later notice, where the third-party notices come from (see
    [RELEASING.md](RELEASING.md)) and the repository, and nothing about a person. The Window menu
    brings the main window and a detached rack or mixer forward (`view.focus_window`). A
    double-click on the divider collapses the rack to its smallest height and the next one restores
    the split; dragging the divider forgets the remembered split.
- **Tooltips** ([ui/src/tooltip.rs](../crates/ui/src/tooltip.rs), `help_at` on every view with
  buttons, [app/src/shell_tooltip.rs](../crates/app/src/shell_tooltip.rs)). A view answers
  `help_at(pos) -> Option<Help>` from the same hit test its press uses: `Help` is the button's
  rectangle (stable per button, so it is the identity), a short text and, for a button that runs a
  command, the command id and argument. `TipTracker` is the whole behaviour: `pointer(help)` keeps
  the timer while the pointer stays on one `Help` and restarts it on another, `tick(now)` shows it
  after `HELP_DELAY` (0.6 s) on the clock `Shell::frame_at` is given (no wall-clock source here),
  `dismiss()` is a press, a wheel turn or a key (it stays hidden until the pointer is on another
  button), `clear()` is leaving the window; each returns whether what is on screen changed, and only
  then does the shell repaint (`needs_repaint` is not touched by a move on a button or a tick while
  waiting). `Shell::help_at` asks the modal panels first, then the plugin banner, then the pane
  under the pointer through `AppCore::help_in_pane` (the rack answers in its own coordinates and the
  shell shifts them); it answers `None` while a menu, a dialog, a drag or a file drag is up. The
  text gets its shortcut from the active keymap at paint time (`Help::text_with`,
  `Keymap::shortcut_text`), the box is placed under the button and kept inside the window
  (`tooltip::place`), and `Shell::tooltip_pending` keeps the platform loop ticking only while a
  tooltip waits. The preference `show_tooltips` (Workflow tab, `workflow.json`, default on) turns
  `help_at` off and, through `RackView::set_button_tips`, the rack's own immediate texts of BYPASS
  and a return's EDIT button; the value readout of a drag and the other information tooltips are not
  help and are not gated. `crates/ui/tests/tooltips.rs` walks the button enums of the views through
  their `*_center` helpers and fails when a button has no text or names a command that is not in the
  registry.
- **Help Mode** (`help.hover_help`, Help > Help Mode, Shift+F1; [app/src/help/](../crates/app/src/help),
  [app/src/shell_help.rs](../crates/app/src/shell_help.rs),
  [ui/src/help_panel.rs](../crates/ui/src/help_panel.rs)). It reuses the tooltip hit tests: a view gives
  its `Help` a stable id (`Help::id`), and the knobs, which have no tooltip, answer through `help_at_with` with an id
  `param:<device type>:<param key>`. `Shell::help_lookup` is `help_at` without the delay and the preference. The long
  text is two data files compiled in (`controls.json`, one entry per id, and `devices.json`, one line per device
  type), and `help::resolve` turns a `Help` into a `HelpCard`: a parameter card is built from its descriptor and the
  about line of its device, an id with an entry is that entry with the live shortcut, and anything else falls back to
  its tooltip text. A docked strip above the status line shows it (`Shell::content_bottom` moves the bottom of every
  pane up by `HELP_PANEL_HEIGHT`), and only a press on its "More" link is taken: it opens the built-in manual at
  that section (`Resolved::manual`; a section the manual does not hold falls back to a `ShellRequest::OpenUrl`).
  Tests check that every id a view names has an entry, that no entry is stale, that
  every device type has an about line, and list the controls of a window that still fall back to their tooltip.
- **The manual** (`help.manual`, Help > Manual, Ctrl+F1; [app/src/manual.rs](../crates/app/src/manual.rs),
  [app/src/shell_manual.rs](../crates/app/src/shell_manual.rs), [ui/src/manual/](../crates/ui/src/manual)). The app
  compiles `docs/README.md` and `docs/KEYBOARD.md` in with `include_str!`, so the manual and the documentation are one
  text. `atomik_ui::manual::markdown` reads the subset the documents use (decision V: written in the project, no
  crate); `Manual::new` cuts each document into pages at its headings of level one and two, builds the table of
  contents and the GitHub-style anchors (`Manual::resolve` reads `#anchor`, `FILE.md#anchor` and addresses), and
  `Manual::search` ranks one hit per section (a heading match above a body match) with a snippet and the matched
  ranges. `ManualView` lays a page out once per width on the canvas it paints on (`layout.rs`, measured text), keeps
  the history for Back and Forward, and takes every event like the command palette. A link that leaves the manual is
  `ManualAction::Open`; the shell opens the fixed address of the file (`manual::outside_url`) or says it is not part of
  the manual. The command palette lists a "Manual: <heading>" row per heading (`palette::manual_entries`). Tests parse
  the real documents: every `more` section of the help text, every anchor of `SECTIONS` and every link of the documents
  must resolve.
- **Revision, dirty flag and repaints** ([model/src/lib.rs](../crates/model/src/lib.rs)
  `Document::revision`, [app/src/app_core.rs](../crates/app/src/app_core.rs),
  [app/src/shell.rs](../crates/app/src/shell.rs)). `Document::revision()` takes a new process-wide
  number on every change of the document (`apply`, including the silent plugin data syncs and asset
  registrations, `undo`, `redo`, an asset prune that removed something) and a new `Document` starts
  with one nobody had, so equal revisions mean equal documents. `AppCore::is_dirty` keeps the
  structural compare against the saved baseline (so undoing back to the saved state is clean) but
  runs it once per `(revision, baseline)` and memoises the answer; a new baseline goes through
  `set_saved`. The window titles are built from one `title()` per event (`WindowKind::title`).
  `handle_pane_event` ends in `after_pane_event`: a pointer move, a pointer leave or a modifier
  change that sent no action, on a document the views have already read (`views_synced_at`), only
  forwards the shown automation values (and syncs the three views that show them when one changed);
  every other event, and the first after the document changed, takes `after_view_change`, which
  stays the forced variant (every view reads the document) for the call sites that change a view
  directly. `ArrangementView::sync_at` and `PianoRollView::sync_at` skip the document for a revision
  they already read, and the host hands them the revision around `handle` and `paint`
  (`set_doc_revision`, cleared after the call) so their own sync at the start of those calls does
  not compare the song or clone the open clip either. `Shell::forward` no longer raises the
  shell-wide repaint for every event: a forwarded event with actions marks every window (the
  document or another pane may have changed), one without marks the window of its pane, and a
  pointer move, leave or modifier change marks nothing, because each view raises its own
  `needs_repaint` when a hover or a drag changes what it shows; a move inside one menu-bar title
  repaints nothing and a change of the bar repaints the main window only.
  `crates/app/src/shell_repaint_tests.rs` pins that a move to where the pointer already is paints
  nothing in any pane and that a move in one window does not mark another;
  `app_core_revision_tests.rs` holds the dirty-flag property test and the gated-versus-forced view
  comparison. The per-step whole-`Song` clone of the arrangement sync stays until the song is shared
  (`Arc<Song>`).
- **Cheat sheet.**
  `atomik --print-keymap [--os win|mac|linux] [--profile standard|legacy] [--keymap FILE]` writes
  markdown from the registry and the keymap; [KEYBOARD.md](KEYBOARD.md) is its output and a test
  keeps them equal (`ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-app --lib cli::` regenerates it).
- **Daily keys: dispatch** ([app/src/shell.rs](../crates/app/src/shell.rs), `Shell::run_command`).
  The app default profile is `standard` (`APP_DEFAULT_PROFILE`); `legacy` stays selectable. Every
  command of a resolved key is offered to the focused view first (`ArrangementView::command`,
  `PianoRollView::command`, `RackView::command`, and the step, sample and slice editors ->
  `CommandOutcome::{Handled, Declined, NotApplicable}`, reached through `AppCore::pane_command`),
  then to the shell (`run_shell_command`: the transport steps, the pane toggles, the create keys,
  and every `AppCommand` through `from_command`). The first that takes it wins, so a user binding in
  a context shadows a `Global` binding of the same chord at run time and an unbound chord does
  nothing in a view that used to handle it itself. The views keep `key()` only for Escape (and Home
  in the arrangement) and text entry (the container and sequencer editors also ignore chords with
  the primary modifier there, so a command chord that no binding took is never typed into a name);
  the sample and slice editors answer `edit.delete` (the selected zone, or the selected slice merged
  into the previous one; declined with nothing to remove or mid-gesture, so the next binding runs),
  the step editor `editor.toggle_note_mute`, and the sequencer and container editors have no command
  of their own (every one is declined, so the global bindings run); the console answers the
  `mixer.*` view commands (`ConsoleView::command`: `mixer.value_up/down` and the page and `_fine`
  variants step the focused control, `mixer.scroll_left/right/home/end` move the strips,
  `mixer.select_left/right` and `mixer.extend_left/right` move or grow the strip selection,
  `edit.select_all` and `edit.delete` select and remove strips; a fader, send level, mute, solo or
  send switch of a selected strip is applied to every selected strip as one `Command::Batch`
  (`console_gang.rs`; a drag's batches share one merge id and merge into one undo step, see
  `MergeKey::Gang`); Up, Down, Page Up and Down, the arrows, Home and End with and without Shift in
  the `Mixer` context,
  repeatable, rebindable, conflict-checked, in the cheat sheet), so its `key()` keeps only Escape
  and the F5 shortcuts and a chord no binding took does nothing; **the console text owner**: one
  owner for every console name edit: `ConsoleView::is_text_owner()` is true while a strip or bus
  name is being typed (a double-click on a name plate, or `begin_rename(strip)`; a channel renames
  the device whose name it shows, the source instrument's else the channel's, as one `Rename`, a bus
  its own name as `RenameBus`), with one buffer; the shell then reports `Text` over `Mixer`, so only
  primary-modifier chords, function keys and Esc run commands, Space and Delete are text, Enter
  commits (an empty or unchanged name commits nothing), Esc cancels, a pointer press elsewhere
  commits like Enter, losing the focus cancels, and a command chord is never typed in; undo, redo,
  the transport keys, select all, duplicate, delete, split, quantize, transpose, nudge, zoom, snap,
  follow and the tools are commands.
- **Daily keys: tools, roles, zoom, follow** ([ui/src/viewport.rs](../crates/ui/src/viewport.rs),
  `ViewportState`, one per time view). `Tool` is Select, Pencil, Eraser, Razor or Speaker
  (`Tool::NOTE` leaves the speaker out: the piano roll has no audio to hear; `tool.*` commands,
  toolbar buttons in the arrangement corner and the piano roll top bar, the active tool lit). Select
  keeps the modeless gestures except drawing: in the piano roll a press on the empty grid starts a
  rubber band (a click clears the selection, Shift or Ctrl adds to it) and never creates a note, a
  press near a note's left or right edge resizes it (every selected note by the same delta, snapped,
  with a minimum length) and one on its body moves the whole selection; the pencil makes a plain
  click draw (a clip one snap step long, a note) and ignores clips and notes; the eraser deletes the
  clip, note or automation point under the pointer; the razor splits a clip or note at the snapped
  pointer position (one `SplitClips` command: one undo step however many clips it cuts) and a razor
  click in the arrangement ruler splits every clip crossing it; dragging the razor along a lane cuts
  the range out of the clips there (`CutOut`; with the copy role held the clips stay and a copy of
  the range is placed after it). **Playhead and locators:** a press in the arrangement ruler or
  piano roll ruler locates and the drag keeps locating the snapped tick under the pointer (one
  `Locate` per changed tick, clamped at zero, mapped through `content_to_song` in the roll); the
  playhead's flag in the arrangement ruler and its line in either grid are grabbed within three
  pixels (Select tool, no Shift or Ctrl, never over a clip's edge, fade, point or mute control or
  over a note), and the drag keeps the pointer's offset from the line. Each loop flag in the
  arrangement's ruler drags alone and `SetLoop` is sent on release with one snap step kept between
  the two (the loop's enabled flag is kept); `Ctrl+click` in the ruler sets the left locator and
  `Alt+click` the right one (on macOS `Opt+click` the left and `Cmd+click` the right,
  `locator_edge`), `Shift+click` sets the End marker, all snapped and ahead of the razor
  (`locator_range` in `ui/src/arrangement.rs`). The arrangement reports a `CursorHint` too
  (`ArrangementView::cursor`: resize over the playhead, a loop edge, a marker line and, with Select,
  a clip's edge and during a resize; the active tool's cursor `CursorHint::of_tool` over lanes and
  clips). The piano roll reports the mouse cursor as a `CursorHint` (`ui/src/cursor.rs`,
  `PianoRollView::cursor`: resize over a note edge and during an edge drag, move over a note body
  with Select and during a move, the tool's cursor (`Pen`, `Eraser`, `Razor`) over the grid, `Pen`
  while drawing and a crosshair while banding); `Shell::cursor` hands it on while the roll is the
  visible song pane and `platform/app.rs` sets the winit cursor when it changes: the OS icon
  (`translate::cursor_icon`), or for the tool hints a custom cursor drawn by `cursor_art`
  (`translate::cursor_art`; a bitmap per picture and scale, made once with `create_custom_cursor`
  and cached in `App.custom_cursors`). A drag that carries something has its own hints
  (`CursorHint::Dragging` the closed hand, `Copy`, `NotAllowed`), decided in `Shell::cursor_in`
  before the pane under the pointer: files dragged out of the file browser show `Copy` over a
  place that takes them and `NotAllowed` elsewhere (`drag_cursor::file_drop_hint`); a device, a
  clip, a track or a palette row held by one pane shows that pane's hint (`RackView::cursor`,
  `ArrangementView::cursor`), turned into `NotAllowed` by `CursorHint::outside_own_pane` while
  the pointer is over another pane; the plate of a device that can be dragged shows `Hand`. The
  **alternate-tool role** (a hold) switches the tool for
  one gesture (Select -> Pencil, any other tool -> Select) and shows on the toolbar from
  `ModifiersChanged` alone (`Shell::forward_modifiers`); **disable snap** is the Ctrl+Alt hold
  (`Roles`; the legacy roles keep Alt alone for it and have no alternate tool, `Roles::legacy`).
  `sequencer.snap_toggle` (S) switches snap off and back to the previous value; `view.zoom_in/out`
  (H, G, Ctrl+=, Ctrl+-) are time zoom in the sequencer and the editor and rack zoom in the rack
  (the views claim them when focused, the rack declines and the shell zooms it);
  `sequencer.zoom_selection` (Z, Shift+Z) fits the selected clips or notes and, again, the whole
  song or clip (`TimeAxis::fit_span`); `sequencer.follow` (F) pages the view to the playhead while
  the transport runs (`TimeAxis::page_to`), a manual scroll pauses it until the next toggle or stop,
  and the Workflow preference `follow_song` is its start state. **Nudge** in the piano roll:
  `editor.nudge_left/right` move the selected notes by the snap step,
  `editor.nudge_left_beat/right_beat` by one beat of the meter (`Meter::beat_ticks`, the Beat snap
  step) and `editor.nudge_left_tick/right_tick` by one tick, whatever the snap (Shift+arrow and
  Alt+arrow in the legacy profile, `Cmd+Shift+arrow` and `Cmd+Shift+Alt+arrow` in the standard one;
  the tick keys avoid Ctrl+Alt+arrow, which graphics drivers take on Windows). All
  are repeatable, clamp the group at tick 0 and are one undo step per press (`move_selection`). The
  arrangement nudges clips the same way: `sequencer.nudge_left/right` (and `_beat`, `_tick`;
  `Cmd+arrow`, `Cmd+Shift+arrow`, `Cmd+Shift+Alt+arrow` in both profiles) move the selected clips as one
  `Batch` of `SetClipGeometry` and stop the group at tick 0. **Clip gestures.** The left edge of any
  clip trims it (note and automation clips: `WorkflowCommand::TrimClips`, which moves the start,
  keeps the end and moves the content offset; a clip that grows left past content tick 0 shifts its
  notes and points right, a looped clip wraps its loop; audio keeps its own handle, and the other
  selected audio clips follow it by the same distance); the right edge resizes every selected clip
  by the same change; both are one merge group, so one undo step. `sequencer.delete_outside_events` (`DeleteOutsideEvents`) removes what
  lies outside the clips' content window (automation points keep one boundary point on each side).
  The **constrain** role (Shift) keeps a clip's start while it is dragged between lanes and a note
  on one axis, and the **copy** role (Ctrl, Option on macOS) drags copies of notes as well as clips;
  both read the modifiers on every move, so they can be pressed after the mouse-down.
- **Lane handle, lane range and extend.** `arrangement/static_handle.rs` draws the static value of an automation
  lane as a line with a handle (`Hit::StaticHandle`, `Drag::Static`); the drag sends `SetLaneStatic` with one merge id
  (`MergeKey::LaneStatic` folds the steps into one) and Esc sends the old value back. `arrangement/lane_range.rs`
  holds the time range of one or several lanes that a drag with `Role::RangeDrag` selects (view state, forgotten by
  any other press or Esc; the drag takes every lane row from the one it began on to the one under the pointer, and
  `ArrangementView::lane_ranges` reports them): `edit.delete` on it sends one `CutOut` over the clips of all the
  lanes, and the app captures it with `ClipSnapshot::capture_ranges` (the parts of the lanes' clips inside the
  range, starting at its start) for Copy and Cut. A paste puts the clips on the range's lane when it has one, and
  each part on a lane of its own kind of its own track when it has several; a range bounce renders each track once.
  `WorkflowCommand::ExtendClips` grows clips to the neighbouring clip of their lane (the start side reuses the
  left-trim planner; an audio clip uses the edge maths of `audio_trim` in the model, the same functions as the
  arrangement view's audio edge drags, so it plays on from its file and reaches back only as far as the file has
  audio). A drag of the start edge of an
  audio clip also trims the other selected audio clips by the same ticks (`Drag::Audio` carries them). The snap
  resolutions and the snap mode (`arrangement_snap`, `piano_roll_snap`, `snap_mode`) and the piano roll's note
  audition switch (`piano_roll_audition`, command `editor.audition_toggle`) and the unit of the clip
  inspector's fades (`fade_unit`, `ms` or `ticks`) are saved in `ui-state.json`.
- **Daily keys: transport, panes, create.** `AppCore` gained `locate_bars`, `go_locator`,
  `stop_or_rewind` (a second Stop returns to the start), `nudge_tempo`
  ([app/src/app_core_workflow.rs](../crates/app/src/app_core_workflow.rs)); the numpad transport
  keys, `C`/`L`/`Ctrl+Enter` and the locators are registry commands. F6 (`rack_visible`, saved in
  `workflow.json`, not in the project) hides the rack and moves the transport bar to the top; F7
  hides the rack and the console and remembers the layout it replaced; F8 and Shift+Tab close the
  open editor or open the editor of the selected clip; Enter in the arrangement opens the selected
  note clip and falls back to return to zero; K cycles the cable display; the macOS secondaries
  `Cmd+Opt+<digit>` are the same commands by physical digit. `create.instrument` and `create.effect`
  open the device browser in **create mode** (`RackView::open_browser_create(CreateKind)`: the list
  is filtered to instruments or effects, the filter is focused, Enter adds the highlighted device
  through the existing `AddDevice` and closes the browser unless the Workflow setting says
  otherwise, Esc cancels); `create.audio_track` is `AddAudioTrack` and `create.mixer_channel` opens
  the console's add-channel picker. The same Enter (and the double-click add) creates through the
  creation chain described below.
- **The one-time notice.** On the first start with the `standard` profile and a settings folder, the
  status bar lists the keys that moved, printed for the OS from the active keymap
  (`Shell::keys_notice`); `keys_notice_seen` in `workflow.json` keeps it from showing again.
- **Clipboard, selection and context menus.** *Rack selection*
  ([ui/src/rack/selection.rs](../crates/ui/src/rack/selection.rs)): `RackView` keeps a set of
  selected devices with a primary (the last clicked: `selected()`, what the editors and the live
  keyboard follow) and an anchor; the toggle and range **roles** (`Role::ToggleSelect`,
  `Role::RangeSelect`, handed to the rack by `AppCore::set_roles` like to the time views) decide
  Ctrl or Cmd click and Shift click, so the OS is a role-table parameter; a click on empty rack
  clears (so does Escape when no drag, menu or focused control is pending), a plain press on a
  member of a group keeps it for a group drag and collapses on release, a right click on a member
  keeps the group. `UiAction::Select` reports one device, `UiAction::SelectMany { primary, keys }`
  several (path keys inside a container). The selection commands are registry commands the rack
  answers (`edit.select_all`, `edit.delete` -> `RemoveDevices`, `edit.duplicate` ->
  `DuplicateDevices`, `rack.select_next/prev`, `rack.extend_next/prev`, `rack.disconnect` ->
  `DisconnectDevice`); a group drag ends in one `MoveDevicesToSlot` (`MoveDevices` inside a
  container; never a no-op). The arrangement gained a track selection (header click, Up and Down)
  and an automation-point selection; its clip selection takes Shift and Ctrl clicks across tracks
  and a rubber band (`Drag::Marquee`: a plain drag on a lane with the Select tool, or below the last
  track; the pencil, Alt and a double-click create clips instead; a plain click on an empty automation
  lane picks the lane and its track and adds nothing, while a double-click adds a clip with a point);
  dragging clips onto another track ends in `MoveClips` (one
  `ClipMove { id, track, start }` per clip: the grabbed clip's track change applies to all, each
  clip takes the lane of its own `LaneKind` on the target track, one without such a lane rejects the
  command and the view does not emit it), Ctrl at the press makes it a copy (fresh ids from
  `next_clip_id`, no live move meanwhile). The clip commands of a gesture share one merge group
  (`MergeKey::Clip(merge)` no longer names a clip, and `Edit::fold_clip_edit` keeps one edit per
  clip), so a drag of several clips, with the final `MoveClips` carrying the same `merge`, is one
  undo step; Join, Duplicate (`DuplicateClips`, `DuplicateTrack`, `DuplicatePoints`) and Delete
  (`RemoveClips`, `RemoveTrack`, the points) are workflow commands, so each is one undo step; the
  piano roll duplicates notes (`DuplicateNotes`). The restructuring commands are workflow
  commands too: `InsertTime` and `RemoveTime` (the `time_edit` module of the model) move clips,
  take groups, markers, ranges, tempo and meter events, the loop and the song End together (what
  starts on the point moves, but the song's first tempo and meter) and are refused while a track is
  frozen; `ReverseClips`, `ScaleTempo`, `ChopClips` and
  `CleanupAutomation` (`clip_transform`) work on a selection, and `JoinClips` leaves muted clips
  out and unrolls loops. They act at the loop locators and on the selection through the
  `sequencer.*` ids the Edit menu and the clip menu use. `StretchClips` is `ScaleTempo` about each
  clip's own fixed edge: the gesture is an Alt+drag on a clip edge (`Drag::Stretch`), whose steps
  reach the app as `UiAction::StretchStep` and run as a `Document::apply_trial` preview
  (`AppCore::stretch_step`: the trial is cancelled and re-planned from the original at every
  step, so no undo step exists until the release commits one). The end edge dragged with
  `Role::ScaleSelection` (Ctrl+Alt+Shift) is the same drag with `whole` set: it scales the whole selection about
  its earliest start, so its steps are `ScaleTempo` previews (`UiAction::ScaleStep`, `AppCore::scale_step`) on the
  ratio of the grabbed edge's new place to its old one, both counted from that start. Held on a start edge it is the
  same drag about the latest end of the selection (`ScaleTempo` with `from_end`, `scale_ratio_from_end` keeps every
  start at tick 0 or later). The same trial serves the Scale
  Tempo panel (`ScaleView`, opened by `sequencer.scale_tempo` with `dialog`: every usable value
  previews through `AppCore::scale_preview`, Enter commits with `AppCore::scale_apply`) and the
  crossfade drag (`Drag::Crossfade`, `UiAction::CrossfadeStep`): `MoveCrossfade` (`audio_join` in
  the model) moves the overlap of an audio clip with the clip before it, the later clip starting
  and the earlier one ending the same number of ticks later, so both play what they played at
  every tick outside it. The grip at the end of the zone drags the crossfade's length instead
  (`UiAction::CrossfadeLengthStep`, `ResizeCrossfade`: the later clip stays and the earlier clip's
  end follows, snapped, with the same trial and one undo step on release); it is offered only when
  the earlier clip ends where the zone does and neither clip plays backwards or loops. `JoinClips`
  on an audio lane (`join_audio`) merges consecutive pieces of one file that continue each other
  (`piece_from` gives the source frame the second piece must start at) and merges their gain
  envelopes (`join_envelopes`) and their warp markers (`joined_markers`: the markers of the first
  piece before the join, one at the join, the later piece's after it, checked to give the same
  source frame as each piece at every marker, start and end); anything that would change the sound
  is refused. Pieces of different files are a render: `join_render` plans it (one lane, no gap, no
  muted clip), the app queues a dry `Kind::Join` render on the freeze worker (the same as the loop
  render) and `RenderJoin` swaps the clips for one clip of the render in one undo step. *Clipboard*
  ([app/src/clipboard.rs](../crates/app/src/clipboard.rs),
  [app_core_clipboard.rs](../crates/app/src/app_core_clipboard.rs)): `Clipboard` holds one typed
  `ClipboardData` (`Clips`, `Notes`, `AutomationPoints`, `Devices`, `Tracks`, the snapshots of
  `atomik_model::workflow`) in `AppCore`, which New and Open keep, so a copy survives both. Copy and
  Cut of those five kinds also write an `atomik-clipboard/1 <kind>` envelope to the system clipboard
  (`AppCore::set_clipboard`, [clipboard_os.rs](../crates/app/src/clipboard_os.rs), through the
  `OsClipboard` trait so tests use an in-memory one): a second line of JSON naming the audio folder,
  then the model's text of the kind (`clips_to_text`, `notes_to_text`, `points_to_text`,
  `devices_to_text` in `project/snippet.rs`, which reuse the project's clip object, an `atomik-clip`
  file, the project's point tuples and an `atomik-project` text). A paste first calls
  `AppCore::adopt_os_clipboard`: an envelope that differs from the text the holder stands for
  (`Clipboard::known`) came from another instance and replaces the in-process data; anything else
  uses it as it is. Audio is fetched by hash from the named folder through the audio lenders. Over
  8 MB, a failed write and the kinds that stay local (strips, takes, settings, controller points)
  keep the copy in the instance. Copy, cut and paste of the focused pane run in `AppCore::clipboard_command` (cut
  is copy then the view's own delete): clips and points paste at the playhead (on the selected track
  when there is one), notes at the playhead inside the open clip, devices after the primary device
  (inside the open container), tracks after the selected track; a repeated paste advances the
  playhead by the span of what was pasted unless the transport runs. `AppCore::apply_command` wraps
  every `WorkflowCommand`: before it, a paste registers the pool entries its snapshot needs
  (`RegisterAsset`, not an undo step); after it, the pasted clips (from `next_clip_id`), devices,
  notes and points are selected. A refusal (empty or foreign clipboard, nothing selected) is a
  status message. *Context menus* ([ui/src/context_menu.rs](../crates/ui/src/context_menu.rs),
  [app/src/context_menus.rs](../crates/app/src/context_menus.rs)): a right click makes the view emit
  `UiAction::ContextMenu { target, at }`; the shell builds a `ContextMenu` from a list of registry
  command ids (labels from the registry, shortcut text from the active keymap and OS, enabling from
  the selection and the clipboard), shows it as a modal overlay (Escape, a click elsewhere or a
  resize closes it, keys are swallowed) and runs the chosen command through `Shell::run_command` in
  the pane the menu belongs to, the path a key press takes. Menus: device, empty rack, track header,
  clip, empty lane, piano-roll grid; a right click on a note still deletes it. The Edit menu has
  Cut, Copy, Paste, Duplicate, Delete and Select All with live enabling.
- **Patching gestures and the jack menu.** A cable drag ends in `RackView::cable_up`
  ([ui/src/rack/jacks.rs](../crates/ui/src/rack/jacks.rs)): the cable that was picked up from a
  connected input (`Drag::Cable::detached`) stays in the document, drawn as the held one, until the
  release, so Escape and a drop on its own input change nothing; the release emits one command, a
  plain `Connect`/`Disconnect` or a `Command::Batch` of the cuts and cables (a batch accepts only
  cable commands, so one undo step and no format change). The stereo partner (`stereo_partner`) is
  the `_l`/`_r` twin of a cable between audio sockets whose keys end in the same side letter, when
  both twin sockets exist and the input is free; Alt, or a picked-up cable, makes it one cable. A
  right click on a jack emits `UiAction::JackMenu`; the shell asks `RackView::jack_menu` for a
  `JackMenu` (devices in rack order with their compatible jacks, judged by `cables::compatible`
  plus, at the top level, the dry run `RackDoc::cable_fits`, which applies the rules of
  `Command::Connect` without changing anything; an occupied plain input is judged as if its cable
  were removed, since choosing it replaces it) and builds the rows itself
  (`context_menus::jack_entries`, a `ContextMenu::from_entries` panel, because the labels are the
  project's own names). Every row runs the registry command `rack.jack` with a `JackOp` text as its
  argument, which `AppCore::jack_op` turns into one command (`Connect`, `Replace` as a batch of
  `Disconnect` and `Connect`, `Disconnect` of every cable of the jack,
  `ConsoleCommand::AddChannelFor`) or a scroll (`RackView::reveal_key`, which scrolls across columns
  as well as down). Inside a container the model has the last word on a cable the menu offers.
  Proof: `ui/tests/jack_patching.rs`, `model/tests/cable_fits.rs`, `app` `app_core_jacks_tests` and
  `shell_jacks_tests`.
- **Creation chain.** Creating a device that wires itself is a model command, one undo step, planned
  on a scratch copy ([model/src/workflow_create.rs](../crates/model/src/workflow_create.rs)):
  `WorkflowCommand::CreateInstrument { index, source, key, options }` adds the device (its console
  channel and the master come from the `AddDevice` rule), its song track when it has a Notes input,
  the track armed, and disarms the tracks named in `options.disarm`;
  `CreateEffect { source, target, key, options }` routes by rule (the selected channel, or the
  channel made for the selected instrument, gets the effect in its first free insert slot; the
  selected FX return effect gets it in series on the return; another top-level device with free
  audio outputs gets it right after it, cabled stereo; anything else, a full chain, a non-stereo
  effect or the master only adds it); `AutoRoute { devices, options }` applies the same rules to
  existing devices (an instrument without channel or track, a loose effect); `DisconnectDevices`
  cuts every cable of several devices; `DeleteDevices` removes like `RemoveDevices` and bridges the
  audio path of a removed effect where it is unambiguous.
  `CreateOptions { route, skip_track, disarm }`: `route == false` is exactly the plain add. The
  model cannot know which tracks an earlier auto-route armed (the format has no flag), so `AppCore`
  keeps that list for the session (`auto_armed`, cleared by New and Open,
  [app/src/app_core_create.rs](../crates/app/src/app_core_create.rs)) and passes it as `disarm`;
  after the command it selects the new device so `notes_target` plays it, and names what was made in
  a status message. The rack view builds the commands: the browser Enter and the double-click add
  use them for instruments and effects (built-in and plugins) at the top level (dragging a browser
  row uses the slot variants `CreateInstrumentAt` and `CreateEffectAt`, and the app fills `disarm`
  for both), with `route` = the Workflow setting `auto_route` inverted by Shift and `skip_track` =
  Alt; inside a container the plain add stays. Delete bridges only with auto-routing on.
  `rack.auto_route` and `rack.disconnect` work on the whole selection. *Branch order of
  `CreateEffect`* (one scratch copy per rule, a rule that cannot be done falls back to the plain
  add, so creating never fails): route off, or an effect that is not a stereo effect: added only;
  the master: added only; a channel or the instrument it was made for: the first free insert slot
  (four slots), the effect right after the chain; the effect of an FX return: in series on that
  return; any other top-level device with free stereo outputs: right after it, cabled; otherwise
  added after the selection or at the end. *Bridging* (`DeleteDevices`): only a top-level effect,
  utility or container with audio sockets; each audio input socket with exactly one cable joins the
  output of its own side (a single connected input and output pair whatever the names) and its
  source is cabled to every destination of that output; nothing is bridged for an ambiguous input,
  console devices, instruments, mixers, the insert loop of a channel, an FX return effect or
  anything inside a container, and a bridge the cable rules refuse leaves the removal plain.
  `RemoveDevices` and `AddDevice` are unchanged. Proof: `model/tests/workflow_create{,_props}.rs`,
  `ui/tests/workflow_create.rs`, `app` `app_core_create_tests` (including live equals bounce for a
  created project) and `shell_create_tests` (run for three operating systems).
- **What is not here yet.** Bridging inside containers; and a gesture that starts a console STRIP
  name edit from the keyboard (a double-click on a name plate edits it, `begin_rename` is the
  programmatic start).
- **Tests as proof.**
  `keymap::{chord,command,context,conflict,roles,script,sheet,tests,user_tests}`,
  `ui/tests/keymap_goldens.rs` (the six listings
  `ui/tests/keymaps/{win,mac,linux}-{standard,legacy}.txt`), `app`
  `shell::{keys_tests,keymap_tests,qa_tests,workflow_tests}`,
  `ui/tests/{workflow_arrangement,workflow_piano_roll,workflow_rack,workflow_snapshot}.rs`,
  `settings_tabs_tests`, `keymap_store`, `commands::tests`, `translate::tests`, `cli::tests`,
  `model/tests/workflow_{devices,clipboard,props,create,create_props}.rs`,
  `ui/tests/{workflow_selection,workflow_arrangement_selection,workflow_create}.rs`, and `app`
  `app_core_clipboard_tests`, `shell_clipboard_tests`, `clipboard`, `context_menus`,
  `app_core_create_tests` (with the live-equals-bounce test of a created project) and
  `shell_create_tests`. Every test takes the OS as a parameter and builds its own temporary config
  directory. Two table tests keep the profiles honest:
  `keymap::tests::every_legacy_key_is_unchanged_in_standard_or_a_listed_move` (a legacy binding the
  standard map loses must be in a reviewed constant of moved keys) and
  `shell::workflow_tests::auto_route_does_nothing_without_a_selected_device_or_in_a_time_view`.
- **Moving, pasting and replacing.** `MoveDevices`, `MoveDevicesToSlot`, `PasteDevices(At)` and
  `DuplicateDevices` carry `route` (the same setting, inverted by Shift in `move_drop`). With it
  the plan runs on a scratch `Planner`: `workflow_create::unroute` bridges the old place (the
  delete bridging, the send/return slot, the entry of the tracks a player fed), the plain move or
  paste is pushed, then `route_effect_in_place` / `route_player_in_place` apply the creation
  rules at the new slot (the device above it for an effect, the device below for a player; an
  effect right below an insert joins that chain). `reroutable` limits it to stereo effects and
  note players whose cables sit on the sockets the rules wire, a route the document refuses
  leaves the plain move, and the whole plan is one undo step.
  [replace_device.rs](../crates/model/src/replace_device.rs) plans `ReplaceDevice`: remove and
  insert under the same key, so the track, channel, sends, note entries and chain place stay, the
  cables whose sockets the new descriptor has are reconnected, the values of the params both
  descriptors have (same key and unit) are carried over and the automation lanes of missing
  params go (`replace_lost_lanes`; `replace_reason` is what a drop target asks first). A row of the
  device browser dragged out of the rack over a track header goes through the shell (the rack
  keeps the pointer): `RackView::dragged_replacement` says what it would create,
  `ArrangementView::replace_hover` outlines the header, and the release is `ReplaceDevice`. The
  Create and Replace Device submenus list `atomik_ui::browser::device_groups` (rows built by the
  shell, `Spec::Rows`, running `create.device` and `rack.replace_device` with the type id). Undo steps
  carry a name from `Command::label` ([labels.rs](../crates/model/src/labels.rs)), kept per step
  in the `Document` and read by `undo_label` / `redo_label` for the Edit menu and `session_status`.
  Cables are coloured by `cables::role_of`, and `CableVisibility` has `SelectedOnly` and
  `HideAutoRouted` besides the earlier three.

## 6h. The master section and Audio I/O

The master section and the Audio I/O device are first-class citizens of the rack. No project-format
change (version 9), no change to the routing plan, summing order, PDC or stages.

- **Starter document.** `atomik_model::new_project_doc()` is `empty_doc()` plus the master (key
  `master`, name "Master", first in `devices`, unfolded) and the cables
  `master:main_l/r -> interface:out_1/2`; it is built with the same helpers as the master creation
  of `AddDevice`, is deterministic and saves byte-identically (golden
  `model/tests/golden/new_project.json`). The app uses it for File, New, for a start without a file
  (`Cli::start_document`; the stress options keep their generated racks) and nowhere else: Open,
  patch load and the render CLI never create a master, so old and foreign files load unchanged.
  `AppCore::new` and `with_doc` keep taking the document they are given (tests build rack-only
  documents with `empty_doc()`).
- **Master is fixed.** Command level only (never a load error): delete, cut, copy and moves of the
  master, and anything placed above it while it is the first device, are rejected with
  `ModelError::Fixed` (or clamped to index 1 for adds and pastes). The UI shows the message ("The
  Master Section cannot be deleted") like any rejection.
- **Console links are a display.** `console::link_kind` classifies a cable as a console link
  (channel or bus `out` to the master or a bus, `send{k}` to master `send_bus{k}`) and
  `console::output_dest` reads where a channel or bus goes; the rack hides the links unless "Show
  Console Links" is on and paints "Output: Master", "Output: Bus <name>" or "Output: Audio I/O 3+4"
  on the plate. The cables, and so the compile, are the same.
- **Auto-assign.** `CreateEffect` with the master selected puts the effect on the lowest free
  send/return (`SetReturn`, one undo step); all eight used adds it unrouted. `AssignSend` ("Use as
  Send Effect", `rack.assign_send`) does the same for an existing effect. The model decides
  (`effect_route_note`, `first_free_send_slot`); the app only passes the selection and shows the
  text: `AppCore::created` reports "Effect routed to send/return N" or "All 8 send and return slots
  are in use" ([app/src/app_core_create.rs](../crates/app/src/app_core_create.rs)).
- **Master additions.** `send1.level` to `send8.level` (appended ids, 0 dB bit-exact), an 18-entry
  `monitor.source` (taps kept in device state), and the key switch that turns on with a cable into
  `key_in` (one undo step). See [DEVICES.md](DEVICES.md).
- **One name.** "Audio I/O" everywhere a user reads it (type id and key `interface` unchanged); a
  grep test (`ui/tests/single_name.rs`) guards it.
- **Audio I/O data in the app** ([app/src/app_core_io.rs](../crates/app/src/app_core_io.rs)).
  `EngineSink::host_channels()` gives the channel counts of the running stream (`LiveSession` keeps
  them from `StreamInfo`; the offline sink and a stopped session say `None`, all channels
  available); `AppCore::frame` and every engine start pass them to `RackView::set_io_host_channels`,
  which derives the lamp colours (green connected, yellow free, red cabled but missing on the
  device). `AppCore::feed_io_meters` runs once per frame and gives `RackView::set_io_peaks` the peak
  per socket: **inputs** are metered at the interface's own `in_k` output endpoints, **outputs**
  `out_k` (audio inputs of the interface, which the engine cannot meter by endpoint) take the
  largest peak of the *source* endpoints of the cables that end there. The meters are the engine's
  existing read-only bank (`Topology::add_meter`, planned when the routing is submitted, reused
  buffers, no allocation per frame) and touch no sample (test: live equals bounce with the feed
  running every block). A meter read resets its peak, so an endpoint the console strips also meter
  (the master's `main`) is read once per frame and the reading is shared with
  `update_console_meters` of the same frame.
- **Big Meter.** View > Meters and Links > Big Meter floats a larger meter over the top right of the rack
  ([big_meter.rs](../crates/ui/src/big_meter.rs), glue in
  [big_meter_view.rs](../crates/ui/src/rack/big_meter_view.rs)). It shows one pair of sockets
  (inputs two by two, then outputs; the arrows pick the pair) from the same per-socket peaks as the
  small meters (`set_io_peaks`), with the ballistics on the UI thread, fed the frame clock that
  `RackView::tick` already gets: VU (a 300 ms rise to 99 % of a steady tone, 0 VU at a reference
  level of -18 dBFS that the buttons move between -24 and -9), PPM (rises at once, falls 24 dB in
  2.8 s) and PEAK (falls 20 dB per second), a hold marker that lasts 5 s or until reset (a click on
  a bar) and a clip lamp per channel that latches above full scale until it is clicked. It never
  touches the engine. Not saved: it starts hidden.
- **First start.** An older `ui-state.json` may still carry `console_notice_seen` and
  `show_console_devices`: both are ignored now (the console devices are always in the rack, there
  is no switch and no notice).
- **Example.** `examples/projects/master-demo` (starter, Lead and Bass, a reverb on send and return
  1, a master send level), generated by `render/tests/master_demo.rs`.
- **Tests.** `render/tests/old_projects_4m.rs` pins that every committed example project is still
  format 9 and saves back byte for byte, and that Open creates a master only in a file that had one.

## 6i. Factory devices: `atomik.character`

Original built-in devices are built with one repeatable pipeline; `atomik.character` is the most
elaborate. See its section in [DEVICES.md](DEVICES.md).

- **DSP stage blocks** (`atomik-dsp`): `shaper` (curves, level-dependent dynamic type, optional
  first-order antiderivative anti-aliasing), `leveller`, `crossover` (fourth-order, flat sum),
  `phaser`, `chorus`, `echo` (glide rule), `comb`, `room` (a parametrisation of `fdn`), `ringmod`,
  `bitcrush`, `noise` (white, pink, rumble, `Crackle`, `Wander` and the programme-presence gate),
  `spectral` (an STFT frame processor with the algorithms identity, gate with hold, slew, band-pass,
  band-stop, quantise and the combined `Lossy`), `modal`, `wobble` and `smoother2`. All are
  allocation-free after `new`, chunk-invariant, flush denormals and keep libm out of the per-sample
  path (`det_math`, `det_trig`); `crates/dsp/tests/stage_blocks.rs` pins a render hash per block
  configuration so a platform difference shows on the macOS run.
- **Device** (`atomik-devices`, `character.rs` and `character/`): the engine batch of 64 frames runs
  as four 16-frame control blocks; per block the CV smoothers (8 ms, second order), the Transform
  morph (`voicing::PresetVoicing::resolve`) and the active chain run. Gain and volume are `4 x^2` on
  a linear knob; the dry tap sits **before** the input gain and is delayed by the active preset's
  latency, so Dry/Wet 0 is the input delayed, bit for bit. Sixteen chains are built in `prepare`; a
  preset change is a 30 ms smoothstep crossfade of the outgoing and incoming chain (the incoming one
  starts reset, the outgoing one is reset when the fade ends, a request during a fade is queued and
  the last wins).
- **Rack and voicing as data.** `rack.rs` is a generic stage graph (`RackChain`): per-stage sends, a
  load-time acyclic and reachability check, lazy evaluation (a stage no active route reaches does
  not run and keeps its state), automatic delay alignment of parallel paths. The numbers live in
  `data/character-voicing.csv` and `data/character-routes.csv` (embedded with `include_str!`). Only
  `wide` is hand-wired (`row2.rs`).
- **Tiered latency.** Every preset except `lossy` is in the short class (`29 + wobble centre`: 45,
  46, 60 frames at 44.1, 48, 96 kHz); `lossy`, the only preset on the spectral stage, is in the long
  class (`+ the frame`: 1069, 1070, 2108 frames). A preset change inside a class never moves the
  latency; one across the classes publishes it once, when the fade ends (one engine `latency_epoch`
  bump).
- **The bounce follows the latency epoch.** The live app recompiles the routing when the epoch
  moves; the plain bounce does the same
  (`atomik_render::plugin_pass::render_song_following_latency`: 64-frame batches, routing recompiled
  after the batch), so a live run and a bounce of a project that crosses the classes agree. The
  plain bounce therefore no longer renders in `--block` sized chunks (output is identical for every
  block by construction).
- **UI and app.** `FxKind::Character` with four meter values; `character_panel.rs` (the 4 x 4 matrix
  as one focusable widget, knobs, screen, back panel with the CV trims), `character_screen.rs` (16
  procedural glyphs and scenes); the app feeds the meters through the generic `meter_count() > 0`
  rule (inside containers by path key) and adds `--stress-character` (8 containers cycling through
  all 16 presets under macro lanes). `atomik-ui` repeats the one-line gain law and the preset tables
  and pins them to the device in tests (the UI links no device code).

## 6j. The file browser

A docked panel on the left of the window (View, File Browser, `F9`) lists folders and audio files,
plays the selected file at once, and drags files onto the other panes. It adds the crate
`atomik-library`, the preview voice in the engine, and three workers and a saver in the app. It
changes no project format and no routing, and it never writes, moves or deletes a user's audio file:
the browser itself writes only the library file `library.json` (with its `.tmp` and `.bad`
companions). The library of devices, patches, containers, clips and projects ([section
6l](#6l-the-library)) amends that: the app also writes inside the library folder, on Save to Library
and on Rename, and nowhere else.

- **The panel** ([ui/src/file_browser.rs](../crates/ui/src/file_browser.rs) and `file_browser/`). A
  folder tree over a file list. The tree has Favourites, Tags (one child per tag with its count),
  Bookmarks, then the drives and the home folder; folders expand lazily. The list shows folders
  first, then audio files (wav, aif/aiff, flac, mp3, ogg), in a natural order ("kick 2" before "kick
  10"); the columns that fit are star, name, duration and, when wide, channels, rate and size. The
  search field filters as you type with the language of `atomik_library::Query`: words match the
  name (case-insensitive, all words), `#tag` (or `tag:`) matches a tag, `fav` keeps favourites,
  `kind:` picks audio, a format, folders or an item kind, `cat:` a category, `author:` an author
  and `in:` a folder the row lies in (all tokens must match; the matcher allocates nothing per
  row); a toggle searches the sub-folders too. The path bar has back and forward buttons that walk
  the last 64 places shown (a new place after stepping back drops the ones ahead), and a
  right-click on it, or a click on its shortened start, opens a pop-up of every folder above the
  one shown. The row menu has Show in Folder (the `Reveal` request: Explorer with the file
  selected, `open -R` on macOS, the folder in the file manager elsewhere), Edit Author and Edit
  Categories (an inline entry, empty clears), and the named favourite lists: Add to a list, New
  list and, inside a list, Remove from it. The view owns the query, sort, selection, scroll, expansion and the
  filtered index list, and paints only the visible rows, so painting does not grow with the folder
  size. With exactly one file or library item selected, an info pane takes the foot of the list (when
  the panel is tall enough): the name, the facts (format, channels, rate, length, size, the tempo the
  file states), tags, author and categories, and for audio a loudness picture; the view asks for the
  picture with `take_detail_request` (a poll, not an action) and the app answers with `set_detail`. A
  patch browser aimed at a plugin device adds the plugin's presets as virtual `Preset` rows. It does
  no disk access: the app pushes everything it shows (`set_places`, `set_children`,
  `set_location`, `set_rows`, `append_rows`, `patch_info`, `patch_marks`, `set_preview`) and answers
  the `UiAction::Browse(BrowseAction)` actions it emits (navigate, show a `Place`, preview,
  favourite, tags, bookmarks, `RequestInfo`, `DragFiles`). In the list, `Ctrl+A` (`Cmd+A`) selects
  all rows.
- **The core and its workers**
  ([app/src/app_core_browser.rs](../crates/app/src/app_core_browser.rs),
  [browser_workers.rs](../crates/app/src/browser_workers.rs)). The UI thread never touches the disk
  for the browser: it sends jobs and `AppCore::frame` drains the answers. `atomik-browse` lists a
  folder (`read_dir` and metadata), the sub-folders of a tree node, the drives, the files of a
  favourite or tag place, and walks the sub-folders for the recursive search (results stream in
  batches, capped at 100 000 entries). `atomik-probe` reads audio headers for the rows the view asks
  for, visible rows first, and the results patch the rows by path. `atomik-detail` streams the one
  file of the info pane into 480 loudness levels (`overview_levels`, at most ten minutes of it, no
  more than the newest request) and reads its tempo from the file's metadata. `atomik-thumbs` reads the
  same picture at 48 bars for each visible audio row (thirty seconds at most, newest request first) and the
  panel keeps it per file and size. Deleting a library item (`delete_library_item` in
  [app_core_library.rs](../crates/app/src/app_core_library.rs)) runs on the UI thread: it removes the
  item file and only the audio of its own sibling bundle that no neighbour of the same name lists, never
  recursively. Every listing, walk and lookup
  carries a generation, so a newer request makes older answers stale and stops a running walk.
  Folders are not watched: a folder is listed again when it is shown and on refresh.
- **The library file** ([app/src/library_store.rs](../crates/app/src/library_store.rs),
  [library/src/db.rs](../crates/library/src/db.rs)). `<config_dir>/library.json` holds favourites,
  tags, authors, categories, named favourite lists and bookmarked folders, keyed by `PathKey` (a
  lexical normal form of the path: `/` separators, case-folded on Windows):
  `{"version":2,"places":["/home/me/samples"],"lists":[{"name":"Drums","paths":["/home/me/samples/kick.wav"]}],"entries":{"/home/me/samples/kick.wav":{"favorite":true,"tags":["808"],"author":"Ada","categories":["Percussion"]}}}`.
  Version 1 files (no `lists`, `author` or `categories`) load unchanged and are written as version 2
  by the next save; a file from a later version is left alone like any newer one. Entries with
  nothing to remember are removed, and the output is deterministic (ordered maps). The text is
  encoded before anything is written, and a failure to encode (`LibraryError::Encode`) is reported
  and keeps the file on disk, so a bad snapshot can never replace the library with an empty one.
  The file is read once when the path is set. A file that is not valid is renamed
  `library.json.bad` (replacing an older one), and the app starts with an empty library and says so
  in the status bar. A file with a newer version, or one that cannot be read, is never overwritten:
  the library stays in memory only. A change marks the database dirty; the next frame hands a JSON
  snapshot to `atomik-library-save`, which writes a temporary file and renames it (one write in
  flight, the newest snapshot wins). `Shell::shutdown` flushes the library synchronously.
- **The preview path** ([engine/src/preview.rs](../crates/engine/src/preview.rs),
  [audio-file/src/stream_decode.rs](../crates/audio-file/src/stream_decode.rs)). On a click the UI
  thread creates the channel with `preview_channel()` and starts the voice at once with
  `EngineHandle::preview_start(stream, gain)` (`ControlOp::PreviewStart`); the feed goes to
  `atomik-preview`, which opens the file with `StreamDecoder`, calls `PreviewFeed::set_source_rate`,
  and pushes decoded chunks (a small first chunk, then larger ones) until the end of the file, an
  abandoned stream or a newer preview. The voice is *pending* until the decoder has set the rate and
  pushed data: it is silent, counts no underrun and its fade does not advance, and the previous
  stream fades out meanwhile. The decoder never runs more than the ring (65 536 frames) ahead, so a
  long file is never decoded whole. The voice resamples from the file's rate with cubic Hermite
  interpolation (bit-exact passthrough at equal rates), fades in over 5 ms, fades the old stream out
  over 5 ms on a switch or a stop (the crossfade sums to unity), and is mixed into interface outputs
  1 and 2 after the metronome, outside the routing and the master meters. An empty ring before the
  end of the stream is silence, counted, never a wait. Finished or replaced streams go back through
  the retire queue (`Retired::Preview`), so the audio thread never frees. The panel shows progress
  from a `PreviewMonitor` (frames played, done, underruns). A preview is live only (see HI-2).
- **Drag to other panes.** `BrowseAction::DragFiles` hands the gesture to the shell
  (`Capture::FileDrag`). Moves become `UiEvent::FileHover` over the pane under the pointer and the
  release becomes `UiEvent::FileDrop`, through the same path as an OS file drop, so the arrangement,
  the sample editor and sampler panels react as they do to a drop from the desktop. A release over
  the panel, over a pane that takes no files, or after Esc cancels.
- **Keys.** The panel is the key context `Files` (over `Global`): while it has the focus, its keys
  are the `files.*` commands, which the shell hands to the panel as the keys they stand for; they
  win over global chords of the same keys (Space previews instead of playing, Backspace goes up a
  folder). While the search or tag field owns the keyboard the context is `Text` over `Files`.
  `view.toggle_file_browser` (`F9`) is global. The full list is in [KEYBOARD.md](KEYBOARD.md).
- **Preview level.** A control in the header (`BrowseAction::SetPreviewLevel`, -60 to +6 dB in whole
  dB, a double-click returns to 0 dB) sets the gain every preview starts with: audio files, patches,
  containers and clips all go through `AppCore::preview_gain`. The level lives in `workflow.json`
  (`preview_db`), not in `ui-state.json`, and applies to the next preview.
- **Persistence.** `ui-state.json` keeps the panel's visibility, width, tree/list split, the
  auto-preview switch and the last folder. `atomik --file-browser [DIR]` shows the panel at start,
  on `DIR` when given.

## 6k. Analysers: the scope tap, spectrum and phase meter

`atomik.spectrum` and `atomik.phase-meter` are two ordinary `DeviceKind::Effect` devices (stereo
`in_l`, `in_r`, `out_l`, `out_r`, the shared `bypass`, no latency) that **pass the audio through bit
for bit** and let the UI see it. The problem they solve is the 32-number limit of
`Device::meter_values`: a spectrum or a goniometer needs the recent samples themselves, and the FFT
and the drawing must not cost the audio thread anything.

```
audio thread                                 UI thread (once per frame)
Device::process                              AppCore::feed_device_scopes
  out = in (bit-exact)                         sink.read_device_scope(id)  ---+
  tap.push(64 frames)  --- ScopeTap --->       Spectrum::update (FFT, average, peaks)
  meter_values (peaks, correlation, LUFS)      RackView::set_analyser -> panel repaint
```

- **`ScopeTap`** ([device-sdk/src/scope.rs](../crates/device-sdk/src/scope.rs)): a ring of the last
  32 768 stereo frames made of `AtomicU32`s (the bits of the `f32`s) and one `AtomicU64` write
  position. The device is the only writer: `push` does relaxed stores for the frames and one release
  store of the position per batch. A reader copies the newest frames oldest first with `read_latest`
  and then checks that the writer did not lap the part it copied (a lapped copy is retried, then
  reported as empty). No `unsafe`, no lock, no allocation after `ScopeTap::new`, which the device
  calls when it is built, so the tap outlives a `prepare`.
- **Opt-in.** `Device::scope_tap()` (default `None`, non-RT) returns the device's `Arc<ScopeTap>`;
  `Bypassable` forwards it. `EngineHandle::insert_device` and `submit_plan` read it and register it
  in `Meters` (`Meters::scope_tap(id)`, a short `Mutex<Vec>` that only the control side and the
  readers take, never the audio thread); `remove_device` and a new plan forget it. The app reads it
  through `EngineSink::read_device_scope` (the live session, the offline test sink and the recording
  fake each implement it).
- **Analysis on the UI side** ([dsp/src/spectrum.rs](../crates/dsp/src/spectrum.rs),
  [dsp/src/stereo_meter.rs](../crates/dsp/src/stereo_meter.rs)). The spectrum's FFT is the in-house
  radix-2 transform of the time-stretch, windows and dB come from the deterministic `det_trig` and
  `log2_det`, and the average and the peak hold run on the audio's own clock (the tap's write
  position), so the display does not depend on how often it is drawn.
  [app_core_analysers.rs](../crates/app/src/app_core_analysers.rs) keeps one analysis state per
  analyser device of the flat rack (inside containers by path key), reads the device's settings from
  the document (`size` up to 16384, `window`, `channel`, `mode`, `averaging`, `hold`, `decay`,
  `tilt`, `smoothing`, `weighting`, `reference`), skips a device whose tap did not move (a paused
  engine costs no view work) and a bypassed device (its panel keeps the last picture and says
  `BYPASSED`), and hands `RackView::set_analyser` an `AnalyserData`: the spectrum trace and peaks
  (240 log columns or 31 bands), the loudest bin, the spectrogram image, the reference curve, the
  loudness history and range, and the newest 2048 scope frames decimated by two. Three more
  pieces of pure display code sit in the dsp crate: `spectrogram.rs` (a scrolling 8-bit image
  advanced by whole rows of audio time), `loudness_history.rs` (the short-term graph and the
  Tech 3342 range over a 0.1 LU histogram) and `k_weighting.rs` (the BS.1770 filter and its
  response, shared with the loudness meters). A rising edge of the spectrum's `reference.capture`
  stores the 31 band levels on screen in `spectrum-reference.json` beside the library file
  ([spectrum_reference.rs](../crates/app/src/spectrum_reference.rs), read once at start-up, written
  atomically, never overwriting a newer version's file): the curve is a user setting, not part of
  the project, so the project format is unchanged. The phase meter's numbers (peaks with hold, RMS,
  correlation, balance, width, mid and side, sample and true peak, momentary, short-term and
  integrated LUFS from the mastering device's BS.1770 meter) are computed in `process`, because they
  are cheap and exact there, and published as 16 meter values.
- **Kick and bass check** ([app/src/kick_bass.rs](../crates/app/src/kick_bass.rs) the analysis and the
  fixes, [app_core_kick_bass.rs](../crates/app/src/app_core_kick_bass.rs) the worker and the picks,
  [kick_bass_panel.rs](../crates/ui/src/kick_bass_panel.rs) the panel, `Analyze > Kick and Bass Check`).
  `analyse` runs on the worker `atomik-kick-bass` on a copy of the song: it renders the two channel strips
  of the picked tracks in one `render_stems` pass (the first 20 seconds, plugins as stand-ins), mixes each
  to mono and hands them to `atomik_dsp::kick_bass::check`, which low-passes both at 120 Hz (the sum of the
  subs is the sub of the sum), takes the kick onsets of `detect_onsets` as the hits, and compares in the
  window of each hit the mean power of the sum with the power sum of the two (a delta below -1.5 dB cancels,
  above +1.5 dB builds up). The fundamentals are the median spacing of rising zero crossings in the
  sounding tail of each hit; two within two semitones and not the same pitch are said to beat. When the
  check finds a problem, every fix that fits (`Fix`) is built by the same `apply_fix` that the real apply
  uses, on a scratch `Document`, rendered again and checked again; a fix is offered when its score
  (overall delta plus a quarter of the worst hit) is 0.5 dB better. The rotation angle and the bass delay
  are searched on the signals first (`atomik_dsp::PhaseRotator`, `best_bass_delay`) and proven by the
  render. The fix-asymmetry row (`Headroom`) is `atomik_dsp::best_rotation` on the bass alone, also proven
  by a render, and is offered whether or not anything cancels. Applying runs the steps through
  `AppCore::apply_command` with `join_next`, so the whole fix is one undo step, and a result that
  belongs to an older revision of the song offers no fix. The strip and the fix rows are drawn from
  `KickBassInfo`, which the app rebuilds every frame while the panel is open.
- **Panels** ([ui/src/analyser.rs](../crates/ui/src/analyser.rs) the data and the maths,
  [analyser_paint.rs](../crates/ui/src/analyser_paint.rs) the drawing; `FxKind::Spectrum` and
  `FxKind::PhaseMeter`): the spectrum plot over 20 Hz to 20 kHz and the device's dB `range`, the
  phase meter's goniometer, correlation, balance and width bars, level bars with RMS and hold, and
  the loudness readouts. Colours come from the theme (`lcd`, `text`, `text_muted`) and the device
  accent.
- **Headless reading.** A QA session has no audio stream, so nothing fills a tap.
  `atomik_render::project::probe_devices` renders the document offline up to a frame with the
  bounce's own code and returns, for each device asked, its last meter values and the newest 8192
  frames of its tap; `AppCore::probe_meters` runs the live analysis on them, fills the panels and
  returns named numbers, and the MCP tool `session_meters` wraps it (see [MCP.md](MCP.md)).
- **Real-time safety and determinism.** The audio path is a copy, 128 relaxed stores and one release
  store (`process_is_allocation_free_for_every_registered_device` covers both devices; the engine's
  `scope_tap.rs` renders under the allocation guard); the output does not depend on any param, so a
  bounce with the devices equals a bounce without them to the bit (`render/tests/meter_probe.rs`),
  and every project saved before they existed loads and bounces unchanged: nothing new is persisted
  (the devices are ordinary devices with ordinary params, and the project format is unchanged).
- **Not covered.** Plugin devices have no tap; a console channel strip has no meter panel of this
  kind (the console's own peak and RMS meters stay); the reference curve is a stored one, not a
  second device (see the roadmap).

### Mix analysis: the masking map

Analyze > Masking Map answers "which tracks cover each other, where and when" without changing the song.

- **Band maps.** `atomik_render::masking::render_maps` renders every console channel as a stem
  (`render_stems`, one deterministic pass, the strips as they sound in the mix) over the song or the time
  selection and hands each stem to `atomik_dsp::masking::analyse` at once, so only a band map per track is kept:
  the level of 27 one-third-octave bands from 40 Hz to 18 kHz for every block of 4096 frames (windows of 8192,
  Hann, the channels summed to mono; 0 dB is a full-scale sine). The FFT, the window and the dB are the
  deterministic ones, so the maps are the same bits on every platform and at every host block size.
- **Score.** `rank_pairs` compares every pair. A cell (one band of one block) clashes when both tracks have
  content in it (above -60 dB and within 40 dB of the track's loudest cell) and one leads by 3 dB or more; it
  scores the lead over 18 dB (at most 1) times a weight that fades out under -20 dB. A band's score is the
  percentage of the analysed time it is covered; the pair takes the direction with the larger total, is ranked by
  its worst band and keeps the time ranges in which that band clashes and the mean spectra of both tracks there.
  The exact numbers are in the module documentation of `atomik_dsp::masking`.
- **Worker and state.** `AppCore::start_masking` runs `masking_worker` on a thread of its own (the engine keeps
  its cores); the result, `AppCore::masking`, is dropped with the project and never touches the song.
  `locate_masking` moves the playhead to the start of a clash.
- **De-mask.** `AppCore::demask` is one undo step: a new Dynamic EQ at the end of the covered channel's insert
  chain (or one already there with a free bell band and no other key), its key cabled from the covering channel's
  insert send, and a bell band on at the clash band, listening to the key, cutting half the lead (2 to 6 dB) from
  6 dB under the covering track's average level. The panel is `atomik_ui::MaskingView`; it draws and reports, the
  shell does the rest.

### Mix analysis: the mix check

Analyze > Check Mix lists what the mix needs, with a measured number for each finding, and never changes the song
by itself.

- **What is read.** `atomik_render::mixcheck::plan` takes every console channel (up to 48), the FX returns and
  the master as stems, plus the feed of every non-linear insert (compressor, dynamics, distortion, Character,
  guitar amp, mastering and the clipper's type id): the output of the stage before it on the chain, or the
  insert send for the first. `render_check` renders them in the stem passes and keeps, per stem, a
  `SignalStats` and a band map, so the audio is never held at once.
- **Measurements.** `atomik_dsp::mixcheck::measure` reads the level over the blocks that are not silent, a peak
  envelope per 1024 frames and the energy of the low end (a fourth-order crossover at 120 Hz) for the stereo
  pair and its mono sum. The thresholds and their reasons are constants of that module, in one place.
- **Findings.** `atomik_render::mixcheck::findings` is a pure function of the document and the measurements:
  bass root (the key at the middle of the held time of the bass track's notes), weak fundamental (bass and
  kick, against the 2nd to 4th harmonic in the mean band map), low-mid share with the tracks that fill it,
  peaks (the highest windows of the master and the tracks loudest there), mono loss and device feeds. A
  finding may carry a `Fix` and a `Tool` (the masking map, a Clipper on the master and the kick and bass check;
  `Tool::available` is true for all three). `mixcheck_clipper` adds the Clipper in one undo step and
  `mixcheck_kick_bass` picks the finding's track before the shell opens the check.
- **Worker, state and fixes.** `AppCore::start_mixcheck` runs `mixcheck_worker` on its own thread;
  `AppCore::mixcheck` is dropped with the project. `fix_mixcheck` is one undo step: a Gain Tool inserted before
  the device with the trim to -18 dBFS RMS, a static band of a Dynamic EQ (a free bell of one on the chain, or a
  new device) at the loudest low-mid band of the track, or a Stereo Tool with Mono on at the end of the chain. The
  panel is `atomik_ui::MixCheckView`.

### Mix analysis: the energy map and the repetition finder

View > Lanes and Markers > Energy Lane draws how much energy each section of the song has, and
Analyze > Find Repetition lists what a track repeats unchanged. Neither changes the song by itself.

- **Sections and roles.** `atomik_model::energy_map::sections` is a pure function of the song: the range markers,
  else each point marker to the next (or the End) with a leading "Start" section, else `DEFAULT_SECTION_BARS`
  bars through the meter map. `Role::guess` reads the first word of the name that means a role (a build or
  pre-chorus, a drop or chorus, a break or bridge, an intro, an outro, a verse); anything else is `Role::Other`.
- **Measures.** `atomik_render::energy::render_blocks` renders the master as a stem over the whole song and hands
  it to `atomik_dsp::energy::analyse` at once, which keeps a `Block` per 4096 frames: the summed band power of
  `atomik_dsp::masking::analyse`, its moment over the bands and the mean squares of mid and side. `Blocks::features`
  reads a range of frames as loudness (dB), brightness (the power-weighted band position as Hz) and width
  (`sqrt(S / (M + S))`). The blocks are the same bits at every host block size. The tracks that play in a section
  are counted from the model (`active_counts`: notes as they play, with tails, or audible audio clips, on tracks
  that are not muted away).
- **Energy.** `energy_map::energy` puts each measure on a 0 to 1 scale (loudness between -40 and -10 dB,
  brightness on a log scale from 250 Hz to 4 kHz, width up to 0.7, the tracks as a share of the tracks the song
  uses) and adds them with `WEIGHTS` (0.40, 0.25, 0.20, 0.15). `EnergyMap::new` also finds the `Flag`s with the
  constants `FLAG_MARGIN`, `NEAR_EQUAL` and `FLAT_BARS`; `Flag::message` gives the sentence.
- **Worker and lane.** `AppCore::start_energy` runs `energy_worker` on a thread of its own (the engine keeps its
  cores); the result is kept with the document revision it measured, so `AppCore::energy` says when it is out of
  date, and it is dropped with the project. `energy_frame` hands `ArrangementView::set_energy_map`, the progress
  and a note to the lane (`arrangement/energy_lane.rs`), a row of `ENERGY_LANE_HEIGHT` under the tempo lane that
  only paints. The option is `show_energy_lane` of the view state and is off for a file without it.
- **Repetition.** `atomik_model::repetition::find` reads the notes a track plays (`flatten_track_notes`) as one
  signature per bar (place in the bar, length, key, velocity) and looks for 1, 2 and 4 bar patterns that play
  `MIN_REPEATS` times in a row, the shortest period first and each bar claimed once; audio lanes are scanned for
  touching, identical, equally long clips and for a clip that loops its content that often. `variations` plans
  the three suggestions against the document: muting is one `SetClipContent` of one unlooped clip that nothing
  else plays (checked against the flattened notes), the filter is a `Command::Batch` of two automation lanes on
  the track's channel strip (`lpf.on` with a static value of 0 and a one-bar clip of 1, `lpf.freq` ramping down to
  `FILTER_TARGET_HZ`), planned only when the filter is off and neither param has a lane. Anything else is advice.
- **App and panel.** `AppCore::find_repetition`, `apply_repetition` (one undo step, then a new search with the
  made variations kept for the idea's track and start) and `locate_repetition`; the panel is
  `atomik_ui::RepetitionView`, which draws and reports through `UiAction::Repetition`.

### Mix analysis: the translation lens, "Will the bass read?" and Split Sub

Three tools for how the low end carries over to small speakers, club systems and mono.

- **The lens.** `atomik_dsp::lens` holds the filters: a `Voicing` (headphones, small speaker, club sub) and a mono
  switch make `LensSettings`, and `Lens::process` runs them on `f64` biquads. The small speaker is a second-order
  Butterworth high-pass at 140 Hz (`SMALL_HIGHPASS_HZ`: -3 dB there, about -7 dB at 100 Hz, -18 dB at 50 Hz) and a
  presence bell of +3 dB at 3 kHz (Q 0.7); the club sub is a second-order high-pass at 32 Hz; mono sums to
  `(L + R) / 2` on both sides before the filters. With everything off the lens is the identity, bit for bit.
  `response_db` reads the magnitude from the same coefficients.
- **On the live output only.** `atomik_engine::EngineHandle::lens_set` sends the settings in a control message; the
  engine's lens voice (`crates/engine/src/lens.rs`) runs on interface outputs 1 and 2 after the master chain and the
  reference and before the click and the preview, and a change ducks 5 ms down, swaps the filters at silence and
  opens 5 ms. An offline render, a bounce and an export never send it, so they never hear it, and a test scans the
  render and model sources for the names. The app keeps the setting like the loudness match (`AppCore::lens`,
  `set_lens_voicing`, `toggle_lens_mono`, `lens_off`): sent when it changes and again to a new engine, never in the
  project, not an undo step. A voicing that is on goes back to headphones when chosen again. The switches are the
  Analyze > Translation Lens submenu, five rows in the transport bar's more menu and `mixer.lens_*` commands, and the
  status line shows `LENS ...` while the lens is on.
- **The meter.** `atomik_dsp::bass_read::read` cuts the mid signal into Hann windows of 8192 frames (hop 4096) with
  the in-house transform. In each window with at least -70 dB between 25 and 150 Hz the note is the lowest strong
  peak of that range; the note's energy is its fundamental plus its 2nd to 5th harmonics, and what survives is the
  harmonics times the power gain of the small-speaker lens at their frequencies. The reading is survived over total
  energy over all windows, so loud notes weigh more: about 16 % for a saw bass at 55 Hz, under 0.1 % for a sine. The
  verdict is `Reads` from 8 %, `Partly` from 2 %, else `Lost`, and `NoBass` without a low note. The strip (the master
  or a track's channel) is rendered as a stem by `atomik_render::translation::render_reading` on the worker
  `bass_read_worker`; `AppCore::start_bass_read` keeps the result with the document revision it read, the panel
  is `atomik_ui::BassReadView` (a list of strips, a meter with two ticks at the thresholds and the verdict).
- **Split Sub.** `WorkflowCommand::SplitSub` (Edit > Split Sub, and a row on the patch name pop-up of a Mono Bass)
  plans, on a scratch document, one edit: the Mono Bass track goes into a container (the combine, so the track,
  the console channel and the note entry follow it), a Poly Synth sub layer joins it (one voice, two sines at the
  pitches of the bass's oscillators and the level of their fundamentals, phase reset, no detune, a 12 dB low-pass at
  1.25 times the split), an EQ puts a 24 dB low cut at the split on the bass, and a Line Mixer sums the two with
  3.0103 dB on each channel to give back the pan law. `split_sub::plan` has the details and the level notes. The
  split is 120 Hz; the container's devices are ordinary ones to adjust. The automation lanes of the track are
  dropped by the combine and the status line says so.

### Sound analysis: the Sound Analyst

"Recreate this sound": Analyze > Sound Analyst measures an audio file and proposes a starting patch on a built-in
synth. The analysis never changes the song; Make Patch is the one undo step.

- **Measurements.** `atomik_dsp::sound_analysis::analyse` mixes the input to mono (the first `MAX_SECONDS`) and
  works on a grid of `HOP_S` frames that starts where the sound starts. The level curve is the RMS of a `WINDOW_S`
  window over the loudest frame; the sound starts at `ONSET_LEVEL` and ends at `END_LEVEL`. `Envelope` has the
  attack (10 % to 90 % of the peak, crossings found between frames), the decay (to within `GAP_FRACTION` of the way
  to the sustain), the sustain (the mean of the longest plateau: a run of `MIN_PLATEAU_S` that stays within
  `PLATEAU_DB`, above `SUSTAIN_FLOOR`), the release (to a tenth of the sustain) and `hold_s`, the length of the
  note. Without a plateau the sustain is 0 and the note is held to the end. `Brightness` follows the spectral
  centroid of a Hann window of about 40 ms (start, end, level-weighted mean, the time to travel 90 % of the way).
  `PitchMotion` comes from `atomik_dsp::notes::track_pitch`, the YIN tracker of Audio to Notes without its
  segmentation; a sound is pitched when half of its frames above `PITCH_GATE` are voiced. The spectrum of
  `SPECTRUM_FFT` frames (three windows from the peak) gives the peaks (local maxima `PEAK_PROMINENCE` times the
  median around them), the noise share (the power not within two bins of a peak) and the `HarmonicKind` (pure, all,
  odd only, inharmonic, noise) with the thresholds `ALIGNED_MIN`, `PURE_BELOW`, `ODD_ONLY_DB` and `NOISE_FROM`.
  An inharmonic sound has no pitch motion and measures its partials against the lowest strong one. `compare`
  gives `Distances` between two profiles (decibels, octaves of centroid, semitones) and `Distances::scores`
  turns them into 0 to 1 scores with `HALF_MATCH`. The decision path uses only the deterministic maths of the
  crate, so the same samples give the same profile on every platform.
- **Proposal and fit.** `atomik_render::sound_analyst::propose` is a table of constants and rules: the poly synth
  on one oscillator for pure, all-harmonic and odd-only sounds (`TRIANGLE_BELOW_DB` picks a triangle over a
  square), the FM synth on the twin-stack routing for inharmonic ones, and the poly synth as a noise voice
  (noise at full level, the oscillator a sine four octaves under the note, a high-pass from
  `NOISE_HIGHPASS_FROM_HZ` of centroid, else a band-pass) for noise and unpitched sounds. The measured times are
  converted with the exact shape of the synth's envelope (`decay_share`, `release_share`, `attack_ms` take the
  blur of the measuring window off), the brightness movement becomes a filter envelope, the pitch offset a mod
  envelope on the pitch (the poly synth) or `pitch.env_amount` (the FM synth), the noise share the noise level.
  `fit` renders the proposal as an inline patch on its note (`patch_json`, 48 kHz, one voice) with the offline
  renderer, measures it with `analyse` and corrects the cutoff, and the opening of the filter envelope, by the
  octaves the centroid is off, divided by the slope the renders so far show (`REFINE_PASSES`), keeping the best.
- **App and panel.** `AppCore::analyse_sound_file` decodes, analyses and fits on the worker `sound_worker`
  (`Msg::Sound`; stop, progress and the generation check as in the other workers). `make_sound_patch` is a
  `CreateInstrument` with the proposal's synth, then one `Command::Batch` of `SetParam` and an `AddClip` with the
  one note, joined to the creation, so one undo takes it all. `start_sound_match` renders the made patch with the
  params the device has now; a match keeps those params, and `sound_data` marks it stale when they changed. The
  panel is `atomik_ui::SoundAnalystView`, which draws and reports through `UiAction::SoundAnalyst`; the shell
  routes audio files dropped on the open panel to the analysis (nothing is imported behind it) and
  `ShellRequest::PickSoundFile` asks the platform for one. The result is dropped with the project.

### Reverb sends: the wizard and the keyed reverbs

Track > Add Reverb Send builds a parallel reverb for a track, and the reverbs it builds can be ducked and gated
by the track.

- **The key.** Both reverbs share the engine in `reverb.rs`; `reverb/keyed.rs` (`Keyed`) is the duck and the gate
  of the wet path. The key is `sc_in_l/r` when one is cabled (a lone side feeds both), else the main input; its
  level is a peak follower (5 ms attack, `duck_release` fall). `engage` is linear between half the threshold and
  the threshold (no `log` on the audio path); `duck * engage` is the share taken out of the wet signal: all of it
  for the broadband mode, the part under `duck_freq` (a Linkwitz-Riley split, so with the duck on the mode adds
  an allpass to the wet signal) or a bell of Q 2 at `duck_freq` (designed every 32 samples, at most 40 dB). The key
  gate reuses the `Gate` of the space reverb on the key. Everything happens after the output gain and the space
  reverb's own high cut and gate, before the mix, so the dry signal is bit-identical.
- **The command.** `WorkflowCommand::AddReverbSend` (`reverb_send::plan`) plans one edit on a scratch document: the
  EQ, the reverb and, for a vocal, a de-esser join the rack after the last device on a return, the first of them
  goes to the lowest free slot with `SetReturn`, the others follow in series (`series_after`, shared with
  creating an effect on a return), the EQ and the reverb get their params, the channel's `out_l/r` are cabled to
  the reverb's key when the duck is on, and the channel's `send{k}.on`, `.level` and `.pre` are set. A track without
  a console channel, a device inside a container, a frozen track, no free slot and an option out of range are
  refused and change nothing. `suggested_decay` reads the played notes of the track (`flatten_track_notes`).
- **App and panel.** `AppCore::add_reverb_send` takes the track selected in the rack or the arrangement, applies
  the command, selects the new reverb and says what was built; `reverb_of_return` finds it. The wizard is
  `atomik_ui::ReverbSendView` (steppers, like the strip silence panel), reporting through `UiAction::ReverbSend`;
  the shell keeps its choices between openings and feeds it the track and the suggested decay.

### Rumble Maker and Reverse Reverb Print

Two Track-menu commands turn a render into new audio on a new track. Both reuse the freeze machinery of 2.19:
a job on the freeze queue renders the source on the `atomik-freeze` worker, and the worker makes something of the
render (`Post` in [freeze_worker.rs](../crates/app/src/freeze_worker.rs)) before it is stored in the project's
bundle, so the print is saved, collected and undone like any other audio, and nothing runs on the audio thread.

- **The DSP** ([dsp/src/rumble.rs](../crates/dsp/src/rumble.rs)). `rumble_layer` builds one piece per kick hit,
  from the hit to the next hit: the hit's sub band (a fourth order Butterworth run forward and backward, so it has
  no phase shift and the piece starts in phase with the kick), then either a chunk of the tail repeated with
  equal-power crossfades (the tail starts where the 5 ms envelope has fallen to half of its peak, the chunk is a
  third of what is left between 30 and 250 ms, cut at zero crossings, each repeat 0.85 times the one before) or
  the sub band through the large `Room` (damped, low-passed again, set to 0.7 of the sub band's peak and added to
  it). A piece fades in over 1.5 ms and out over 15 ms, and the whole layer is scaled down when it would be louder
  than the kick's sub band. `reverse_swell` reverses the source into the room, keeps the tail that rings after it
  and reverses that again: a swell that rises to its loudest where the source starts, set to 0.7 of the source's
  peak.
- **The command** ([model/src/rumble.rs](../crates/model/src/rumble.rs)). `WorkflowCommand::AddRumble` plans one
  edit on a scratch document: the audio track of the rendered clip (`plan_rendered_track`, right after the kick),
  a Sidechain Tool in trigger mode whose key sockets are cabled from the kick's channel output, an Auto-Filter
  with no sweep as a 24 dB low-pass (the EQ's low-pass starts at 1 kHz) and the EQ's low cut. `kick_key` says
  which notes are the kick (the first pad that plays a synthesised kick on a drum machine, else its first pad; every
  note elsewhere), `kick_hits` lists their start ticks, `suggested_release` (a third of the median gap) and
  `suggested_decay` (twice the gap) read the rhythm. A swell is a plain `AddRenderedTrack`.
- **App** ([app_core_rumble.rs](../crates/app/src/app_core_rumble.rs)). `rumble_maker` (`sequencer.rumble_maker`,
  argument `tail` or `reverb`) renders the selected clips of the track, or all of them, through the insert chain
  with a 2 s tail; the notes that are not on the kick pad are taken out of the copy of the document the worker
  renders. The hits are the frames of the kick notes (`rumble_of` finds them in an audio kick). `reverse_reverb_print`
  (`sequencer.reverse_reverb_print`, the length in seconds) renders the selected clips of each track, else the
  range of the selected lane or the locators, dry, and `swell_place` puts the swell on whole ticks that end at the
  frame of the source's first tick (the longest that fits before it, so a source near the start gets a shorter
  swell and one at tick 0 is refused). The result is checked against the document as for a freeze and applied as
  one undo step; the new clip is selected and the status line says what was made.

### Transition kit

Add Transition (`sequencer.add_transition`, the choice as its argument) puts a transition at a section boundary.
The edits are one model command, the prints reuse the freeze worker of the section above, and nothing runs on the
audio thread.

- **The boundary** ([model/src/transition.rs](../crates/model/src/transition.rs)). `boundaries` lists the starts of the
  sections after the first (markers, range starts, else the section lines of the energy map) and `nearest_boundary`
  picks the first at or after a tick. The view keeps the boundary a context menu opened on
  (`ArrangementView::take_transition_boundary`: a marker's tick, a range's start, or the bar line nearest to the
  pointer in the ruler); the app falls back to the selected marker or range, then the playhead.
- **The edits.** `WorkflowCommand::AddTransition { at, edit }` plans one edit on a scratch document with
  `TransitionEdit::FilterDown`, `Silence` or `Fill`. A filter down adds an Auto-Filter as the last insert of the
  master or a group bus (reusing the one a former filter down made) and a `cutoff` clip of nine linear points that
  follow an exponential curve from 20 kHz to 300 Hz over the last bars; the lane's static value holds the open
  cutoff outside the clip. A silence is a one-point step clip of the master's `fader` lane over one beat, its static
  value the fader as it was. A fill rewrites the plain, unmuted, unlooped note clips of a track that cover the beat
  before the boundary (`SetClipContent`, content ticks through the clip's offset). A clip that would overlap another
  on its lane is refused.
- **The prints.** `first_hit` names the track whose first note or audio clip start in the first beat after the
  boundary is earliest (a drum machine first, then track order) and `hit_span` its span. The app queues
  `Kind::Riser` (`Post::ReverseReverb`, ending at the boundary through `swell_place`, as in the section above) or
  `Kind::Impact` (`Post::Impact`, which pads the render with the reverb's seconds and runs
  `atomik_dsp::transition::impact`: a tanh saturator driven to four times the peak and scaled back to it, the large
  `Room` at three seconds added at 0.9 of the dry peak, a 30 ms fade out). Both land with `AddRenderedTrack`.
- **The count.** `overuse` says when a kind is on more than half of the boundaries and on at least three of them.
  The app keeps `(boundary, kind, undo depth)` for the session; an undo takes the entries above the new depth off,
  a new or opened project clears it, and the warning is added to the status line message.

### Delivery targets and the Peak Surgeon

Analyze > Delivery Targets (`file.delivery_targets`) and Edit > Peak Surgeon (`sequencer.peak_surgeon`) open one modal
panel, `DeliveryView` ([ui/src/delivery_panel.rs](../crates/ui/src/delivery_panel.rs)), on its Targets or its Peaks
page. The panel only draws a `DeliveryData` and emits `UiAction::Delivery`; the app core holds the state and nothing
runs on the audio thread.

- **The targets** ([model/src/delivery.rs](../crates/model/src/delivery.rs)). `TARGETS` is the table of eight
  generic targets (an integrated loudness and a true-peak ceiling each, no service named). `assess` applies one to a
  measured master: the gain is the target minus the loudness, and the verdict is `Unchanged`, `TurnedDown`,
  `TurnedUp` (the peak after the gain stays under the ceiling) or `IntoLimiter` (it does not: a quiet master with hot
  peaks that a normalising chain would turn up into its own limiter).
- **The measurement.** `atomik_render::delivery::measure_master` renders the master section as a stem over the song
  with no tail (as `energy::render_blocks` does) and measures it with `loudness::measure`, the report the export
  writes. The app runs it on the worker of [delivery_worker.rs](../crates/app/src/delivery_worker.rs) (`Msg::Delivery`,
  progress and cancel as atomics), keeps the last report with the document revision it measured and shows it faded
  once the song changed.
- **The project's target.** `RackDoc::delivery` is `Option<Delivery>` (a name, a loudness and a ceiling), saved as the
  root field `delivery` only when set (format 44, `.ignorable()`), edited with `Command::SetDelivery`, one `Swap`
  that reports `ViewChanged`, and accepted by `Command::Batch`. Set Ceiling is a batch of that command and a
  `SetParam` of `ceiling` on the Mastering device of the master's insert chain, else its Clipper
  (`AppCore::delivery_set_ceiling`), so it is one undo step; `Delivery::compare` words the export toast against it.
- **The scan** ([model/src/peak_surgeon.rs](../crates/model/src/peak_surgeon.rs)). `Scanner` takes the levels a clip
  plays (the highest absolute sample of any channel times `AudioClipContent::played_gain`: clip gain, fades and
  envelope) in chunks, so a clip of any length is read from the engine's assets in 64k-frame blocks, forwards or, for
  a reversed clip, block by block from the end. A peak is the first frame of a flat top at least as high as every frame
  within `width` frames, and the scanner keeps the `KEEP` highest.
- **The plan and the edit.** `plan` takes the peaks from the highest down and dips each by `step_db` (0.2 to 0.4)
  until it is under the threshold; a second step on the same frame deepens that dip, a dip never takes more than
  `MAX_DIP_DB`, and the number of dips is bound by the room left in the envelope (`dips_that_fit`: three points
  each). `apply_dips` writes each dip as three breakpoints of the clip's gain envelope (a triangle in linear gain,
  the value at a breakpoint being the existing envelope's times the dips'), replacing the existing points inside the
  dip's span, so the edit is one `SetAudioClip` and the file and the clip gain stay as they were. The app
  ([app_core_surgeon.rs](../crates/app/src/app_core_surgeon.rs)) reads the clip again after every edit and reports the
  drop of the highest peak from that second scan, not from the plan. A warped or looped clip, a missing file and an
  asset at another rate are refused.

## 6l. The library

The Library sections of the file browser (Devices, Patches, Containers, Clips, Projects) list what
can be put into a project and let the user save and load it. It adds no project format version:
items are ordinary project texts, plus one small new file type.

- **Sections and the folder.** `atomik_library::LibrarySection` names the tree entries. **Devices**
  is virtual: one row per built-in device type, the path `atomik-device:<type id>`
  ([`device_uri`](../crates/library/src/item.rs), never read from disk: the stat, probe and preview
  paths skip it). The other four are folders of the **library folder**, `Patches/`, `Containers/`,
  `Clips/` and `Projects/`, listed recursively by a walk job; a missing folder is an empty list. The
  folder is `--library <dir>` (`ShellOptions.library_root`), else the one chosen in Settings,
  Workflow (`WorkflowPrefs.library_folder`), else `Documents/Atomik Rack/Library`
  (`default_library_dir`). Favourites and tags of items stay in `library.json`, keyed by the item's
  path.
- **Item formats.** A **patch** (`.atomikpatch`) and a **container** (`.atomikcontainer`) are
  `atomik-project` texts at the current project version that hold only devices (all in column 0),
  the cables between them and the pool entries of their samples, with an empty song and console
  ([library_io.rs](../crates/model/src/library_io.rs): `save_device_item` writes canonical text,
  `load_device_item` reads any project as a `DeviceSnapshot`). A container item is one container
  device with its inside, macros and splits. Any project reader opens an item, and "Open as Project"
  does. A **clip** (`.atomikclip`) is `atomik-clip` version 1
  ([clip_io.rs](../crates/model/src/clip_io.rs)):
  `{"format": "atomik-clip", "version": 1, "ppq": 15360, "tempo_bpm": 120.0, "meter": {"num": 4, "den": 4}, "clip": {"length", "offset", "loop_length", "muted", "name"?, "color"?, "notes": [[tick, length, key, velocity(, true)]], "grid"?, "scale"?, "controllers"?}}`.
  `"clip"` is a project note clip without `id` and `start`, with its limits and note rules; the
  output is canonical; `tempo_bpm` and `meter` are informational (nothing is rescaled); another
  `ppq`, a newer `version` and an audio clip are refused, errors name the JSON path. A **project**
  is a `.atomik` file or bundle as before.
- **Bundle layout.** The audio of an item is in the sibling folder
  `<Library>/<Section>/<stem>/Audio/<hash>.wav`, the layout
  `atomik_render::bundle::existing_bundle_root` already reads for a project, without peaks (they are
  rebuilt on load). The names are content hashes, so copies are idempotent.
- **Save to Library** (`rack.save_to_library`, `sequencer.save_clip_to_library`, both unbound, in
  the device and clip context menus; [app_core_library.rs](../crates/app/src/app_core_library.rs))
  writes the selected devices or the one selected note clip with the atomic text save under a free
  name (`Poly Synth`, `Poly Synth 2`; a clip is `<track> bar <bar>`), makes the section folder on
  the first save and copies the audio on the `atomik-library` worker. It is not an edit: the
  document, the undo history and the dirty flag stay as they are. The browser lists the section,
  selects the new row and opens its rename entry.
- **Load** ([`AppCore::load_library_item`](../crates/app/src/app_core_library.rs) with a
  `LoadTarget`): a device row is added the way the rack's browser adds it (the creation chain at the
  top level, `AddInner` inside a container); a patch or container is pasted (`PasteDevices`,
  `PasteDevicesAt` for a slot), a clip pasted on a note lane (`PasteClips`), each one undo step with
  fresh keys; the item's pool entries are registered (`RegisterAsset` is not a step) and the
  `atomik-adopt` worker copies the audio the session bundle lacks and loads it for the engine. A
  file the item does not hold is listed missing and can be relinked, and the devices load anyway. A
  project is not loaded: the core raises an open request, and the shell asks about unsaved changes
  (`ShellRequest::ConfirmSave` with `PendingAction::OpenFile`) before it opens the file. Double
  click and Enter in the panel, the drops of the panel's own drag and of the operating system
  (`UiAction::InsertLibrary`, `InsertLibraryClip`) and `session_library load` all end in this
  function. Library paths never reach the audio import: the shell sends them to the rack or the
  arrangement, or refuses them with a message.
- **Rename** (`BrowseAction::RenameItem`) works only on item files strictly inside the library
  folder (never `..`, never the folder itself), keeps the extension, refuses an empty, invalid or
  taken name (a file or folder of that name counts), renames the sibling audio folder with the file
  (and puts the file back if that fails), moves the favourite flag and tags to the new path and
  lists the place again.
- **The factory library** ([factory.rs](../crates/app/src/factory.rs)). A second, read-only root of
  the browser: `Place::Factory(section)` and the Factory tree entries (`PlacesData::factory`, shown only when the
  folder has sections) list `<factory>/Patches`, `Containers`, `Clips` and `Projects` with the same walk job as the
  library sections. `factory::locate` finds the folder: `ATOMIK_FACTORY_DIR`, else `factory` beside the executable (zip,
  tarball, installer, `/opt/atomik-rack/factory` in the Debian package), else `../Resources/factory` (macOS bundle),
  else the `factory` folder of the checkout above a `target/<profile>` folder. `Shell::set_factory_folder` hands it
  to the core (`AppCore::set_factory_root`; the platform layer calls it for every song). The folder must be outside
  the user library folder. `factory.json` (`{"version": 1, "items": {"Patches/Name.atomikpatch": {"author",
  "categories", "tags"}}}`) is read once into a `Factory` and its marks are merged into the rows
  (`Factory::decorate`) and into the mark updates after a favourite or tag change, under the user's own marks in
  `library.json`, which still key favourites and tags by the item's path. `delete_item` and `rename_item` refuse a
  path inside the factory folder first, so a factory item is never changed whatever the library folder is, and
  `stepping_folder` follows a shown factory section for patch stepping. Packaging copies `factory/` into every
  artifact (`scripts/release/package-*.sh`, the `.iss` file) and the smoke test checks `factory.json`. Every item
  is written from the tables in `app_core_factory_build.rs` (test code, the sound design): 42 device patches
  (`Def`; the semi-modular ones fill modulation slots by key), 14 containers (`ContainerDef`: eleven effect chains wired
  in series, three modular instruments with explicit cables, macros whose position is set from the first target's
  value so the container sounds as designed), 12 note clips (`app_core_factory_clips.rs`, step and chord patterns
  written with `save_clip_file`), 4 drum kits (`app_core_factory_kits.rs`) and 3 drawn-table patches
  (`app_core_factory_tables.rs`: key frames as harmonic recipes, baked like the editor bakes them, the table audio in
  `<name>/Audio/<id>.wav` beside the item). A kit sample is rendered by
  `render_hit`: a hidden project with one drum machine, a pad on a synthesised module, offline through
  `render_doc_in`, mixed to mono, trimmed, faded and levelled; the kit item is the drum machine with a zone per pad
  and the audio in `<kit>/Audio/<id>.wav` beside it (the layout of any library item with samples). The item
  writers are the ones Save to Library uses (`save_device_item`), and `app_core_factory_tests.rs` loads, applies and
  renders each item; the exact audio is compared on Windows x86-64 only, where the library is written. A patch
  preview of a drum machine plays a pattern on its pad keys (`DRUM_PHRASE`).
  The `Projects` folder holds three example songs (`app_core_factory_examples.rs`), each a folder
  `<name>/<name>.atomik` in the layout Save As writes. `ExampleDef` is a table of `Voice`s (a factory patch, a modular
  container, a sampled kit, or the pads of a kit as the drum machine's own synthesised modules) whose clips are
  factory clips or short patterns of its own, placed by `Place` and looped to fill the bars, plus a `mix` function
  for the returns, buses, groups, master insert and automation. `build_example` runs it on `new_project_doc` with
  model commands, so every value is checked like an edit, and saves it with `save_project_checked`; a sampled kit's
  audio goes to `Projects/<name>/Audio/<id>.wav`. `AppCore::open` hands a project inside the factory folder to
  `open_template`, so it opens as an untitled song and Save asks for a place. The tests build each example from the
  tables and the shipped kits (so the text is the same on every platform), check its length, devices and samples,
  and bounce it.
- **What the library does not write.** Everything outside the library folder and the session bundle
  is read only. Items do not carry project-level state: macro assignments, song tracks (a patch has
  none) and console settings are not saved with a patch, and a pasted instrument gets its console
  channel by the usual paste rule. Templates, a start screen, collect and relocate, deleting items
  and watching the folder are out of scope.
- **Device patches** ([docs/DECISIONS.md](DECISIONS.md#d-device-patch-format);
  [device_patch.rs](../crates/model/src/device_patch.rs),
  [app_core_patches.rs](../crates/app/src/app_core_patches.rs)). A device patch is a `.atomikpatch`
  of exactly one device. `WorkflowCommand::ApplyPatch { device, snapshot }` makes an existing device
  take it in place, as one compound undo step: every param (a param the patch does not store goes
  back to its default; a param a macro drives is left alone), zones, step patterns, curves, drum
  patterns, a plugin's state blob (`Edit::PluginState`, reported as `SyncOp::PluginStateChanged`,
  which the app restores into the live instance) and the patch name (`RackDevice::patch_name`,
  `Edit::PatchName`); a container's inside is replaced by taking the container out and putting the
  new one back in its place (the engine rebuilds the whole inside either way, and the cables and the
  track stay). The device keeps its key, name, column, colour, cables, track and console channel, so
  it sounds and bounces exactly like a device created from the same patch
  (`crates/render/tests/device_patch_bounce.rs`). The type must match, console devices have no
  patches, a frozen device is refused by `freeze::guard`, every sample of the patch must be in the
  pool (the app registers them before the command, as for a paste, and adopts the audio from the
  item's bundle), and a patch that changes nothing is rejected. `ResetDevice` applies
  `DeviceSnapshot::defaults` (a new device's sound) and `SetPatchName` sets the name alone.
  `ItemSummary::device_type` and `ItemInfo::device_type` carry the type id of a one-device patch. In
  the app, `LoadTarget::Device(key)` (a drop on a device's patch strip, or anywhere on the panel of
  a device of the patch's own type, `UiAction::ApplyLibraryPatch`; a patch dropped on a panel of
  another type is inserted below it, unless the type is of the panel's kind: see
  `LoadTarget::Replace` below) applies a patch file and says why when it holds several
  devices or
  is for another type; `LoadTarget::Replace(key)` (`UiAction::ReplaceWithLibrary`: a device row or a
  one-device patch dropped on the middle of a top-level device of the same kind in the rack, or on a
  track header in the arrangement) is `WorkflowCommand::ReplaceDevice` after `replace_reason`, and a
  patch of another type then loads its sound with the next command joined into the same undo step
  (a failed load undoes the swap); a patch of the device's own type only applies;
  `save_device_patch` writes one device as a patch named after the file the
  platform's save dialog picked (`ShellRequest::PickSavePatch`, which confirms an overwrite) and
  keeps the name on the device; `step_patch` walks the patches of the device's type in the folder of
  the patch it last took (else the folder last browsed for its type, else the library's Patches
  folder), in natural name order and wrapping, each as its own undo step outside patch browsing;
  `ClipboardData::Settings` carries one device's snapshot for `edit.copy_settings` and
  `edit.paste_settings`, and `rack.reset_device` and `rack.save_patch` are in the device context
  menu. The patch strip itself is on the foot of the name plate of every unfolded device but the
  console devices (`paint::patch_strip`): previous, the patch name (a click starts patch browsing,
  below; a right-click lists the patches of its folder), next and save. Old project files and patch
  files are unchanged: the name is the only new field, and old and new bounce bit for bit alike
  (HI-1, HI-2).
- **Patch browsing** ([docs/DECISIONS.md](DECISIONS.md#d-device-patch-format);
  `Document::apply_trial` in [lib.rs](../crates/model/src/lib.rs),
  [app_core_patches.rs](../crates/app/src/app_core_patches.rs),
  `FileBrowserView::set_patch_target`). Browse on a patch strip (`rack.browse_patch`, a click on the
  patch name) aims the file browser at one device: an orange band names it, the list keeps folders
  and the patches whose `ItemInfo::device_type` is the device's type, and the plate's patch name is
  drawn orange (`RackView::set_patch_browse`). While aimed, a selected patch row
  (`BrowseAction::Audition`), the previous and next keys (`rack.patch_prev`, `rack.patch_next`,
  which walk the folder the browser shows) and a pick in the pop-up of the patch name (right-click;
  `rack.patch_pick`) apply the patch as a **trial**: `Document::apply_trial` performs the command
  like `apply` but keeps the edit off the undo stack (and the redo history aside), so the engine
  gets the usual `ParamChanged`/`ZonesChanged`/`PatternsChanged`/`PluginStateChanged` syncs, the
  device's node is never rebuilt and notes that sound keep sounding, and no undo entry exists.
  **Accept** (`commit_patch_browse`: Return, a double-click or Enter on a patch row, `Load` of a
  patch while aimed, another device's Browse, any other command through `apply_command`, an undo or
  redo) turns the trial into one compound undo step from the original device to the patch it has
  now; **cancel** (`cancel_patch_browse`: Escape, `BrowseAction::Cancel`) undoes every trial edit
  and gives back the redo history, so the document is the original byte for byte. While a trial is
  open `Document::undo` and `redo` do nothing (the app accepts first), a new document drops it, and
  `referenced_assets` counts its samples. The folder of each device type that was browsed last is
  kept in `ui-state.json` (`patch_folders`) and starts the next browse of that type; the patches of
  a folder are probed once (`FolderPatches`, kept by the folder's modification time, probed afresh
  at the start of a browse and after a save into it), not once per step. Return and Escape work from
  any pane (`Shell::patch_browse_key`), and the file browser decides them itself when it has the
  keys.
- **QA.** `library_list` and `session_library` ([docs/MCP.md](MCP.md#the-tools)) read and drive the
  same code; `validate` and `project_inspect` accept the item files.

## 6m. Windows: the platform side

The Piano Keys strip is a window of its own like the Spectrum EQ: `WindowKind::PianoKeys` holds the pane while
the window is open (`DetachState::detached` means open), the main layout gives it no room, and the
octave and velocity are `ui-state.json` fields applied at start and saved by `save_zoom_state`. The question
about unsaved changes (`ShellRequest::ConfirmSave`) is a message box with the buttons in `SAVE_BUTTONS` on every
platform (`save_answer` reads the result); on Windows rfd makes it a task dialog, which needs Common Controls 6, so
the build scripts of the app and of the MCP server embed `windows/atomik.manifest` in every program they link.

The rack and the console (the mixer) can leave the main window (View > Panels > Detach Rack, Detach Mixer, or
`--detach rack,console`). A `Shell` is one song: one document, one view per pane and its undo history; the
shell owns each open window's size and scale and says which windows should exist
([`Shell::windows_wanted`](../crates/app/src/shell_windows.rs)). Any number of songs can be open, each
with windows of its own; the layer above the songs is described under "Several songs" at the end of this
section. This section is what
[crates/app/src/platform](../crates/app/src/platform) does with that. The pure model (kinds, saved
geometry, placement, drag routing, reconcile) is
[crates/app/src/windows.rs](../crates/app/src/windows.rs); the decisions of the platform code that
need no window (monitor and geometry conversion, the pointer outside a window, the screenshot name)
are in [platform/window_logic.rs](../crates/app/src/platform/window_logic.rs) and unit-tested. The
winit and GPU code is not (it needs a window and a GPU).

- **One GPU context, a surface per window.** `GpuContext`
  ([gpu.rs](../crates/app/src/platform/gpu.rs)) holds the one `RenderContext` and a vello `Renderer`
  per device, built on first use; each window has a `WindowSurface` and draws through the context.
  Every window has its own `Scene` and `SceneCache`; the `TextEngine` is shared. `render_offscreen`
  (`--screenshot`) is separate and writes one PNG per open window (`shot.png`, `shot-rack.png`,
  `shot-console.png`).
- **The arrangement keeps two recorded layers.** `ArrangementView` records the lanes (backgrounds,
  grid, clips, waveforms, drags, take ghosts) and the track list into `DisplayList`s stamped with
  a process-unique revision ([paint_cache.rs](../crates/ui/src/arrangement/paint_cache.rs)), so a
  backend that caches lists by id and revision (`SceneCanvas` with a `SceneCache`) reuses their
  encoding. The view's `repaint` flag (raised by every state change) and a stamp of the view
  rectangles, scale and document revision re-record them; a playhead that crossed a pixel raises
  only `playhead_dirty`, a moving track meter only `headers_dirty`. The playhead line, ruler flag,
  marker strip, tempo lane, scrollbar, overlays and menus are drawn live in the same order as
  before, so the recorded op stream does not change (`ArrangementView::layer_records` counts the
  recordings; the test `arrangement_layers` compares the cached paint with an uncached one).
- **Windows are reconciled.** `App.windows` maps each window, a song (`DocId`) and a `WindowKind`, to an
  `AppWindow` (window, surface, scene, cache, pointer, file drag, cursor, title, zoom) and `ids` maps
  winit's `WindowId` back. After every call into the shell `sync_windows` goes through the songs, closes the
  windows of a song that is gone and compares the open windows of each song with its `windows_wanted()`
  (`windows::reconcile`): it closes the extra ones and creates the missing ones. A detached window
  is placed with `windows::place_detached` (the main window with `windows::place`): a usable saved
  geometry goes through `place`; with none, the window opens maximised on the first monitor the
  other open app windows (`desktop_rects`, the main window included) are not on, taking the monitors
  left to right from the one after the main window's and wrapping (so the rack takes the first free
  monitor and the console the next), its restored bounds the default size centred there; when every
  monitor is used, or there is one, it is cascaded 32 desktop units down and right from the main
  window of its own song at the default size, clamped onto the monitor. Either way it is placed from
  `Shell::saved_geom` and the monitors of the moment (`MonitorInfo`, converted at the edge by
  `window_logic::monitor_info`), opened with a position only when `place` gives one (no monitor, as
  on Wayland, leaves it to the OS), maximised when saved so, and then told `ScaleChanged` and
  `Resize` with `handle_in`. The main window is placed and restored the same way. A window that
  cannot be created is attached again (`window_closed`).
- **The menu bar in the detached windows.** `Shell::bar_height_in(window)` is the height of the in-window menu bar
  at the top of a window: the main window and the detached rack and console carry it (when the app draws its own bar,
  not on macOS), the small tool windows and the detached browser do not. Each window with a bar has its own `MenuBar`
  state (`window_bars` for the detached ones) over the one `Shell::menu_model`, so the menus, check marks and enabled
  states cannot drift. `menu_bar_event` gives an event to the bar of its window (a key goes to the window whose menu is
  open) before the panes see it; a press on a detached bar first gives that window's pane the keys, so the commands
  act on it through `live_pane` and `pane_command`. Opening a menu closes every other one (`close_other_bars`), as do
  `window_focused` for another window and `focus_lost`. `pane_rect` of a detached pane starts below the bar, so
  `hit_in`, the rack's view origin, the drops and the cross-window drag maths (`to_desktop`, `window_under`, which work
  on the whole inner rect of a window) agree; `paint_detached` translates the rack below the bar and paints the bar
  over it. The platform raises a window's minimum width to `Shell::menu_bar_width`. The saved geometry is the whole
  inner size, bar included.
- **Closing.** The close button of a detached window calls `window_closed(kind)` (the pane returns
  to the main window); closing the main window of the only song quits and keeps the detached flags, so
  the windows reopen at the next start; closing the main window of one of several songs runs
  Window > Close Song Window for that song (its own question about unsaved changes, then its windows
  and plugin instances go). `ShellRequest::FocusWindow` raises a window (and un-minimises it);
  `ToggleMaximise` maximises or restores it. File dialogs are parented to the window that last had
  the focus.
- **Geometry.** `Moved` and `Resized` of every window, the main one included, call
  `Shell::window_moved(kind, geom)` with the outer position in desktop units, the inner logical size
  and the maximised flag (`window_logic::geom_of`). A minimised window reports nothing. The shell
  writes `ui-state.json` itself.
- **Desktop units.** Windows and X11 report physical pixels in one desktop space; macOS reports
  points, where a window's physical size is its logical size times its own scale.
  `window_logic::desktop_space` picks `DesktopSpace::Physical` or `Points`, and a window's rect is
  recomputed from `inner_position`, `inner_size` and `scale_factor` when it is needed, so it is
  never stale. Each window's `ScaleFactorChanged` goes only to that window: monitors with different
  scales are independent.
- **Input routing.** Each event goes to the song its window belongs to
  (`Workspace::select`), with its window: `handle_in(kind, event)`. Keys,
  modifiers and focus are global: `Focused(true)` gives the song the focus (`Workspace::focus`), calls
  `window_focused(kind)` (the keys go to the
  pane that last had them in that window) and moves the window to the front of the `ZOrder` of its song;
  `Focused(false)` calls `focus_lost()`, which is idempotent, so the order of the two events when
  hopping between windows does not matter. The pointer and the OS file drag are per window.
- **A drag across windows.** The OS keeps delivering a drag's events to the window that got the
  press, with positions outside it. While `Shell::cross_window_drag()` is true (files from the file
  browser or items from the library) the platform works out the window under the pointer for every
  pointer event: the position becomes desktop units (`to_desktop`), `window_under` picks the
  frontmost window of the app holding it, and the shell gets `handle_routed(kind, event, under)`.
  Any other gesture stays in its window with `handle_in`. Where the OS gives no window positions
  (Wayland) `under` is the window itself while the pointer is inside it and `None` outside, and the
  status line says once per drag that "Drag between windows is not available on Wayland"; a drop
  outside cancels. A file dropped from the OS arrives per window and goes to that window with its
  own pointer.
- **Painting and pacing.** `paint_in(kind)`, `needs_repaint_in(kind)`, `cursor_in(kind)` and
  `title_in(kind)` are per window, and so is the redraw request: a window is redrawn only when the
  shell says it has something to paint, so the 60 Hz meter ticks redraw the console's window and not
  the rack's or the main one. Every tick the platform paints, in turn, each window in
  `Shell::windows_to_paint()` (main, rack, console, whichever has the focus) instead of waiting for
  the OS's redraw requests: Windows serves one pending paint per pass of its message loop, and the
  window it picks first (the focused one) could keep winning while it animated, starving the others.
  A frame that was painted but not presented (`FrameResult::Skipped`) is asked for again
  (`Shell::repaint_window`). With more than one window open the surfaces present without waiting for
  the vertical blank (`window_logic::presents_with_vsync`): the windows are painted one after the
  other on one thread, so a wait for one monitor's blank would stall the others, and monitors with
  different refresh rates would drag each other down; the 60 Hz tick sets the pace. A lone window
  keeps vsync.
- **Invariants.** Every song has a main window, and it is the owner of the editors of that song's sandboxed
  plugins (`set_plugin_editor_owner`), whichever pane is detached; plugin editor windows are
  separate top-level windows (kept per song) and are not touched by any of this. Nothing here reaches the engine,
  the render path or the sink, so HI-1 and HI-2 are unaffected. A surface is dropped before its
  window. The shell's state, not the platform's, decides which panes are detached.

### Several songs

[`Workspace`](../crates/app/src/workspace.rs) is the application layer above the shells. It holds the open
songs (`DocId`, ids are not reused in a run), the one calls are routed to (`Workspace::select`; everything that
is not a workspace method reaches that song's `Shell` through `Deref`, so the platform layer reads like the
single-song app) and the one with the **play focus**.

- **Audio is unique.** Only the song with the play focus has a running stream, MIDI inputs and control surfaces;
  every other song runs on a `LiveSession` that is switched off (the state of a song whose stream failed to
  open: sink calls are accepted and ignored, the status line says "another song has the audio"). `Workspace::focus`
  gives the audio to another song: `Shell::release_audio` shuts the old song's stream, inputs and surfaces and
  stops its transport, then `Shell::claim_audio` hands the new song the settings the old one ran with, enables its
  session and rebuilds its engine (`AppCore::rebuild_engine`), as a change of the devices does. The old stream is
  closed before the new one opens, so a device that cannot be shared works and no song leaks a note into the
  other. Plugin instances stay loaded in the song that owns them; closing a song shuts its plugin host down.
- **What a click does.** Giving a window the OS focus gives its song the play focus, so the keys, the MIDI
  keyboard and the audio follow the window being used. The input focus and the play focus are one for now.
- **Commands.** Window > New Song Window (`window.new_song`), Open Song in New Window (`window.open_song`), Close
  Song Window, Next Song, Previous Song and the list of open songs (`window.song`, at most nine) are shell
  requests (`ShellRequest::NewSong`, `PickOpenSong`, `CloseSong`, `ShowSong`) that only a workspace answers
  (`ShellRequest::FocusSong` tells the platform layer which window to raise). Every request-returning shell
  call (`execute`, `handle_in`, `confirmed`, `save_then`, `discard_then`...) has a workspace version that does
  this.
- **Questions that span the songs.** Close Song Window runs the closed song's own Save, Don't Save, Cancel
  question (`ShellRequest::ConfirmSave` names the song through the workspace, so the answer reaches it even
  when the platform routed elsewhere in between). Quit asks about every song with unsaved changes, one after the
  other, focusing each; a Don't Save is remembered until the quit ends or a new quit starts.
- **Shared and per song.** Each song has its own document, undo history, panes, panels, plugin host, autosave and
  recovery snapshots (`recovery/song-<n>` for a song opened next to the first, `plugin_setup::song_plugin_config`,
  with no crash journal: the first song's names what a crash lost). The keymap, the workflow preferences and the
  layout of the window are copied into a new song, and `Workspace::sync_preferences` carries a change made in one
  song to the others. Settings and the scan cache are files each song reads and writes itself; a change made in
  one song's Settings reaches another one when it takes the audio, which carries the audio settings along. The
  window state file (`ui-state.json`) belongs to the first song to hold it and passes to the next one when that
  song closes.
- **The library and the recent list are live.** Each song keeps its own copy in memory, and the file is written
  by whichever song changed it. `Workspace::sync_shared` (after every routed call and every frame) compares the
  recent list and a library revision (`AppCore::library_revision`, counted by changes made in the song, not by
  adoption) and hands a change to the other songs (`AppCore::adopt_recent`, `AppCore::adopt_library`, which
  repaints the browser's marks and places and writes nothing), so one song's write cannot drop another's change.
  A new song starts from the state the others hold.
- **Moving material between songs.** The clipboard follows the song the calls are routed to (see the takes
  clipboard above), and so does the audio it needs. When the clipboard crosses to a song,
  `AppCore::set_audio_lenders` gives that song the bundle folders of the song it was copied in (and those that
  song borrowed from). A paste of clips, takes, devices, tracks, strips or a patch registers the assets the song
  lacks (`workflow_pre`) and `AppCore::fetch_lent_audio` has a worker (`spawn_fetch`, thread `atomik-fetch`) copy
  the files from the lending bundles into the song's own (a scratch bundle when it has none) and load them; a file
  the lender no longer has is listed as missing like any lost audio. Assets are content hashes, so nothing in the
  pasted material is rewritten while both songs run at one rate.
- **Audio at another sample rate.** A paste whose pool entries are not at the song's rate
  (`atomik_model::assets_off_rate`) never reaches the model as it is: `AppCore::convert_pasted_audio`, called from
  `apply_command`, holds the command back and starts a worker (`spawn_convert_paste`, thread `atomik-paste-rate`;
  never the audio or the UI thread) that reads each original from the song's own bundle or else from a lending one
  and stores only the resampled copy in the song's bundle (`Bundle::convert_rate_from`: the lender is never
  written). `apply_paste_converted` then loads and registers the copies (not an undo step) and applies the command
  again with `DeviceSnapshot::at_rate`, `ClipSnapshot::at_rate` or `TakeSnapshot::at_rate`, which point clips, takes,
  frozen renders and zones at the copies and scale every frame position by the rate ratio, the same arithmetic as
  `Command::ConvertSampleRate` (they share `rate_convert.rs`). The paste is one undo step; a copy made once is
  remembered per asset and rate, so the next paste of the same audio needs no worker. A pending paste counts in
  `AudioState::pending`, which keeps the save's orphan tidy off the new files.
- **Dragging between song windows.** A drag of clips, a track header or devices that is let go over a window of
  another open song is Copy in the first song and Paste in the second, at the point of the drop, without touching
  either clipboard. The platform layer (`App::dispatch`) asks `App::other_song_under` which song's window holds
  the pointer when the release is over none of the dragging song's own windows, and calls
  `Workspace::drag_to_song`: `Shell::take_song_drag` ends the drag (`ArrangementView::cancel_carried` or
  `RackView::drop_carried`, which put back what the gesture had moved) and returns the snapshot Copy would take
  (`AppCore::carried_data`); the audio lenders are set as for a paste; `Shell::drop_song_data` converts the point
  to the pane's own coordinates and `AppCore::drop_carried` places the data: clips on the lane and snapped tick
  under the pointer (`ArrangementView::drop_spot_at`, a note or audio lane that fits, else each clip on its own
  lane kind), devices at the rack slot there (`RackView::drop_slot_at`), tracks after the track there. Files
  dragged out of the file browser follow the same hit test (`Workspace::files_to_song`,
  `Shell::foreign_file_drag`), so a patch, a container or a device row of the shared library drops on whichever
  song is under the pointer, with the hover outline there. Both go through the ordinary paste commands, so the
  audio fetch and the rate conversion above apply and the drop is one undo step.
- **Recovery of the other songs.** A song next to the first keeps its snapshots in `recovery/song-<n>` with its own
  session marker. At start, `crashed_song_snapshots` looks in those folders for a marker whose process is gone
  (and removes it, so the copies are offered once) and `Workspace::recover_songs` opens the newest snapshot of
  each as an unsaved song in a window of its own, up to eight, without moving the audio or the focus; a message
  in the first song says how many came back. The first song's own copy is offered by its banner as before.
- **Not done yet.** Separate Input Focus and Play Focus buttons on the interface panel (the song that has the
  stream also has the MIDI inputs, so the two are one today: they need a way to feed a song's live notes into the
  engine of another), a drop marker in the window under the pointer while clips or devices (not files) are dragged
  over another song, and a session of the MCP server with several songs (it stays one song).

## 6n. Logs, crash reports and diagnostics

Everything the app has to say goes through the `log` facade; nothing prints with `eprintln!` (a
test, `crates/app/tests/no_eprintln.rs`, keeps production code off it; the command-line programs
`atomik-mcp`, `atomik-play`, `atomik-plugin-scan` and `atomik-plugin-host` are the exception, their
stderr is their output).

- **The log.** [logging.rs](../crates/app/src/logging.rs) installs a logger in `main` before the
  settings load. It writes `<config folder>/logs/atomik.log`, rotated at 2 MiB into `atomik.1.log`
  and `atomik.2.log` (three files in all), and every record also goes to stderr. The app's own
  crates log from `info`, everything else from `warn`; `ATOMIK_LOG=debug` (or `error`, `warn`,
  `info`, `trace`) changes the level of the app's crates. If the file cannot be opened the log falls
  back to stderr alone and says why. With no config folder in the environment the logs, the plugin
  cache, the plugin journal and the recovery snapshots use `atomik-rack` in the OS temp folder and a
  toast says so (`settings::config_folder`).
- **The audio thread never logs.** HI-1 stands: the callback and the render workers only increment
  atomics. The UI thread reads them in `Shell::poll_status` and writes the records (a line when a
  counter grew, at most every 5 seconds).
- **The panic hook.** [crash.rs](../crates/app/src/crash.rs) installs a hook once. A panic writes
  `crash-<timestamp>.txt` next to the log: message, place, thread, a forced backtrace, the version,
  the OS and the audio and MIDI devices the session had open; then the previous hook runs. A panic
  on an audio or render thread (`is_realtime_thread`) does no file I/O: it raises a flag and the UI
  thread writes the report on its next status poll. The engine's fault containment is unchanged; the
  report says it happened.
- **Overload counters.** `status::OverloadStats` gathers what the real-time side dropped or could
  not do in time: note events dropped (`EngineHandle::dropped_events`), input overruns, frames the
  audio recording tap lost, plugin events dropped and sandbox batches that came back late (summed
  from the plugin devices' meter values), and control stalls (pushes to the engine that found its
  queue full, plus MIDI control messages the inbox dropped). The status bar shows each counter only
  when it is not zero, and `session_status` reports all of them under `overload`. A headless
  session has no audio stream, so its input overruns read 0.
- **Help > Export Diagnostics.** [diagnostics.rs](../crates/app/src/diagnostics.rs) writes one text
  file: version and platform, the status line and every counter, `settings.json`, a summary of the
  plugin cache and the plugin journal, the last crash report and the end of the log. Everything
  under the user's home folder is written as `~`.

### The update check

The program makes one kind of network request, and only when the user said yes: the check for a newer
version. [crates/update](../crates/update) holds everything except the buttons; the shell side is
[update_check.rs](../crates/app/src/update_check.rs) (the `Updater`: the rules, the worker, what is kept),
[shell_update.rs](../crates/app/src/shell_update.rs) (the words and the buttons) and
[update_notice.rs](../crates/app/src/update_notice.rs) (the card, `UpdateNotice`). The decision is
[DECISIONS.md](DECISIONS.md#s-the-update-check).

- **Consent.** The answer to "Check for new versions once a day?" is the workflow preference `update_check`
  (`None` until answered, in `workflow.json`; `Consent` in the crate). While it is `None` nothing is asked:
  at the first start the question shows as the notice in the lower left corner (over the start screen too),
  and Help > Check for Updates (`help.check_for_updates`) shows the same question instead of going to the
  network. Settings > Video & updates changes the answer (the section is
  [updates_tab.rs](../crates/app/src/updates_tab.rs)).
- **The request.** One GET of the fixed address `RELEASES_URL` (the list of releases of the project), HTTPS
  only, no query, no redirect, no cookie store, no compression, a timeout of 8 seconds, at most 1 MiB read.
  The only header the program sets is the User-Agent, `atomik-rack/<version>`; the client adds `Host` and
  `Accept`. Nothing about the user or the machine is sent. It runs on its own thread, `atomik-update`, never on the
  UI or the audio thread, and any failure is logged at debug level and told to nobody unless the user asked.
- **The answer.** `newest` takes the highest version of the list that is not a draft; a pre-release counts only
  when the running build is one itself. `Version` orders release candidates (`0.4.0-rc.2` < `0.4.0-rc.10` <
  `0.4.0`). A newer version shows the notice with What's new (the page is made from the checked tag, never taken
  from the answer), Skip this version (kept in `updates.json`; a later version shows again) and Later.
  Nothing is downloaded or installed.
- **The day.** After a Yes the check runs at the start when a day has passed since the last request, counted
  from the request whatever its outcome (`UpdateState`, in `updates.json` next to `settings.json`; it is read again
  before each write, so two song windows do not undo each other). Help > Check for Updates does not wait for the day.
- **The seam.** The request goes through the `HttpGet` trait. A `Shell` starts with `Offline`, which fails every
  request; only `platform/app.rs` installs `UreqGet`, and not for a screenshot or a stress run. Tests use
  `FakeHttp`, which records the requests, so no test reaches the network and the tests can assert what was sent.
- **Tests.** The version order, the list parser (drafts, pre-releases, odd entries, hostile tags) and the
  fake answers (newer, same, older, malformed, error status, no network) are in `crates/update`; the throttle, the
  consent states, the skipped version and "no request while off" in `update_check_tests.rs` and
  `shell_update_tests.rs`.

## 6o. Reference tracks

A song keeps up to 16 reference audio files to compare the mix with. The list is project data with no part in the
sound: `RackDoc::references` (`Reference`: a path and an optional cue in milliseconds), edited by
`Command::SetReferences` (one undo step, `ViewChanged`) and saved as the optional root `"references"` (written only
when there is an entry, ignorable, so the project format stays 44). Save As, a save to a new place and Collect Files
copy a reference that lies outside the project into its `Files` folder with the same helper as the plugin files and
point the list at the copy in one undo step; the files are never changed or removed.

**The voice** ([engine/src/reference.rs](../crates/engine/src/reference.rs)). `ReferenceVoice` lives in the engine
beside the preview voice and reuses its stream (`preview_channel`, the same 4-point Hermite resampling, the same
retire queue). It runs at the end of `Engine::run_batch`, after the routing and the master meters and before the click,
on interface outputs 1 and 2, and only live: nothing offline ever calls `EngineHandle::reference_start`, and a voice
that is told nothing leaves the outputs bit for bit as they were. `ReferenceSettings` (listening, linear gain, band,
stereo solo, measure) reach it through `ControlOp::ReferenceStart`, `ReferenceSet` and `ReferenceClear`.
Listening crossfades the mix and the reference linearly over 30 ms; the gain glides with a 150 ms time constant. A band
(`ReferenceBand`: Linkwitz-Riley filters at 80 Hz, 300 Hz and 4 kHz) and a mid or side solo (`ReferenceSolo`) go through
two identical paths, one for the mix and one for the reference, and a change of either ducks the output for 5 ms each
way around the swap. While `measure` is on, the voice measures the short-term loudness of the mix (the K-weighting of
`atomik_dsp::k_weighting`, 3 s windows every 100 ms) and publishes it in `ReferenceShared`. The engine therefore
depends on `atomik-dsp` now.

**The session** ([app/src/app_core_reference.rs](../crates/app/src/app_core_reference.rs)). Selecting a
reference decodes it on the worker `atomik-reference` and measures `measure_loudest`: the maximum of the 3 s window over the file. The
reference key (`AppCore::toggle_reference`) fills the ring from the song position in seconds (through the tempo map) or
from the cue, then sends `EngineSink::reference_start`; a feeder thread keeps the ring full. While any reference exists
the engine measures the mix; the matched gain is the mix reading minus the loudest section, moved at most 0.5 dB every
15 frames, the first reading at once, readings at or under -60 LUFS ignored. A new engine (`rebuild_engine`) returns the
song to the mix. Commands: `mixer.toggle_reference` (Ctrl+Shift+R), `mixer.add_reference`, `mixer.select_reference`,
`mixer.next_reference`, `mixer.remove_reference`, `mixer.reference_cue`, `mixer.reference_match`,
`mixer.reference_band` and `mixer.reference_solo`, all in the Analyze menu's Reference submenu.

**The decision zone** ([render/src/zone.rs](../crates/render/src/zone.rs),
[app/src/app_core_zone.rs](../crates/app/src/app_core_zone.rs),
[ui/src/zone_panel.rs](../crates/ui/src/zone_panel.rs)). `atomik_render::zone::measure` reads seven numbers from
the first two channels of a file: gated integrated loudness, the loudest 3 s loudness, punch (sample peak minus
integrated loudness), brightness (the least-squares slope of the octave-band energies at 125 Hz to 8 kHz, each
band two band-pass sections in `f64`), the low end (share of the energy under 120 Hz through a Linkwitz-Riley
low-pass), width (side over mid energy, held to -60..+20 dB) and correlation. It is a pure function of the samples,
so it reads the same on every platform within the resolution shown. Every attribute but the two loudness ones is a
ratio, so it does not move when a file is scaled (`Attribute::normalised`); a test scales a file and checks it.
`zone_row` sets a mix against a list of references: the range is the lowest and highest reading, and the `Verdict`
is `Inside` (within the attribute's resolution of the range), `Above(distance)` or `Below(distance)`.
`AppCore::refresh_zone` runs the analysis on the worker `atomik-zone`: each reference is decoded from its file (a
reading is kept by path while the file stays listed), and the mix is a render of the song with `render_doc_in`, the
renderer of a bounce, so it never contains the reference voice. A song with third-party plugins is refused. The
result never touches the document (no command, no undo step); `AppCore::zone_data` turns it into the `ZoneData` that
`ZoneView` draws, and says when the song changed since the render started (`Document::revision`). The same
measurement is the `zone` entry of the MCP audio report. The panel opens with `mixer.reference_panel`.

## 6p. Per-note expression (MPE)

A controller that gives every sounding note a MIDI channel of its own sends the bend, the pressure and CC 74 of
each note on that note's channel. Atomik keeps that as data of the note and plays it per voice.

- **Zones** ([mpe.rs](../crates/core/src/mpe.rs)). `MpeZones { lower, upper }` holds the member channel counts of the
  lower zone (master on channel 1, members 2, 3, ...) and the upper zone (master on channel 16, members 15, 14, ...);
  two zones share at most 14 members and the one set last wins a conflict. `MpeZones::config` takes an MPE
  Configuration Message (RPN 6 on a master channel; the data entry is the member count, 0 removes the zone).
- **Live input** ([mpe.rs](../crates/midi-io/src/mpe.rs)). A port with `mpe.enabled` is opened with `MidiLink::mpe`
  set to its zones (`MpeSetting::zones`); its `NoteForwarder` reads the bytes with an `MpeInput` instead of `parse`.
  On a member channel the note on and off are ordinary notes, and the pitch bend (times the channel's range, 48
  semitones until RPN 0 on that channel changes it), the channel pressure and CC 74 become `NoteKind::NoteExpr`
  events for the key sounding on that channel (tuning in semitones, pressure and slide `0..=1`). A channel keeps
  what it last held and sends it right after the next note on there. Master channels, channels outside the zones
  and every port without `mpe.enabled` read as before, and the registered-parameter messages of a member channel
  are the input's own. The events go through the usual `NoteRouter` (an expression follows the device holding its
  key), one message gives at most four events and nothing allocates.
- **Model** ([note_expression.rs](../crates/model/src/note_expression.rs)). `MidiData::expression` holds one
  `NoteExpression { tick, key, pitch, pressure, timbre }` per note that has curves, found by the note's start tick
  and key (a clip never holds two notes there); a curve is points `(ticks after the note start, value)`, linear
  between points, held at both ends, at most 1024 points per curve and 16 384 notes with curves per clip. It
  lives in the clip's `midi` data, so splits, joins, loop conversion, tempo scaling and clip copies carry it. A
  note moved or retuned in the editors no longer finds its entry (it is found by the note's start and key), and
  the saved file leaves out the entries that no note matches. The file field is `"expression"` of a note clip
  after `"sysex"`, written only while some note has curves and ignorable under format 44: `[tick, key, pitch, pressure, timbre]` with each curve a list
  of `[ticks, value]`. `flatten_expression` finds the notes like `flatten_track_notes` does (offset, loop, swing,
  mutes, hiding, live groove) and gives song-tick curves. `layered_clip` and `merge_laps` keep the entries of the
  notes they keep and give a replaced note the take's.
- **Recording** ([record.rs](../crates/app/src/record.rs)). The tap offers `NoteExpr` events like any other; a take
  collects those of the note held on their key in the same lap and writes them as the note's curves
  (`record_curve`: a burst closer than about 5 ms keeps its last value, a repeated value is dropped, 1024 points at
  most). Input quantize moves the note and its curves together.
- **Playback.** `render` turns the flattened curves into `SongTrack::expression` (`SongExpression`: the note's
  start tick and key, the kind and points in song ticks). `compile_track` matches each to its note, drops points
  at or after the note's end and expands them to `NoteExpr` events sorted after the note on of their tick; the RT
  player sends them only for a key it holds, and a seek or a loop wrap into a note sends each curve's value in
  force with the chased note. Nothing is allocated on the audio thread and a bounce does not depend on the block
  size. `ControllerState` keeps a fixed array per key (tuning, pressure, slide), cleared by that key's next note on;
  the poly synth, the wavetable synth and the FM synth read it per voice (destinations in
  [DEVICES.md](DEVICES.md#per-note-expression-mpe)).
- **MIDI files** ([midi_file.rs](../crates/app/src/midi_file.rs)). A track with expression is exported as MPE: RPN 6 on
  channel 1 with the number of member channels the notes need, RPN 0 (48 semitones) on each, every note on a member
  channel that is free at its start, and the note's curves as pitch bend, channel pressure and CC 74 on its channel
  (the value at the start just before the note on, then each point, then rest after the note off); the track's
  controllers and program changes go to channel 1. An MPE track of a song file uses channels 1 to 16 of its own
  MIDI track. The import reads RPN 6 on a master channel, merges the notes of the member channels into one track
  (with the master channel's controllers) and keeps each note's bend, pressure and CC 74 as its curves.
- **Editing** ([expression_edit.rs](../crates/model/src/expression_edit.rs)). Pure functions over one curve:
  `set_point` (adds, or takes the value of a point at the same tick; `None` at 1024 points), `move_points` (a group
  moves together, limited so no point leaves the note or the range, a moved point replaces an unselected one at its
  tick), `remove_points`, `draw_stroke` (a freehand stroke replaces what it covers, thinned with `thin_points` of
  Cleanup Automation, harder until it fits the cap), `scale_to_length`, `with_curve` and `apply_curve` (copies a
  curve to other notes, scaled to each length). Each returns the new curve or data, so a gesture derives every step
  from the data at its press.
- **Piano roll** ([expression.rs](../crates/ui/src/piano_roll/expression.rs)). The strip belongs to the edited note,
  the lowest selected note: shown while it has expression, or after `editor.edit_expression` (then for any note,
  and with several selected). It has tabs for the three curves, the pitch scale chip (48, 24, 12 or 2 semitones
  either way), APPLY (several notes selected) and the clear button, and a plot over the note's length. A click on
  empty space adds a point; a drag from empty space is a rubber band (the Pencil tool draws a freehand stroke
  instead); a drag on a point moves it and the selected ones (Shift: a tenth of the travel; Ctrl: whole semitones
  for a pitch); Alt-click, a double click, a right click and the Eraser delete; Delete and Select All work while
  the plot has the focus. A gesture sends `SetClipMidi` with one merge id and `EndMerge` on release, so it is one
  undo step; Escape sends the data from the press back. `editor.apply_expression_curve` and the APPLY button copy
  the curve of the chosen tab from the edited note to the other selected notes.
- **Output** ([mpe_out.rs](../crates/core/src/mpe_out.rs)). `MpeEncoder` is the sending side of a zone: `note_on`
  takes a member channel (never used first, then the longest released, then the oldest note's), resets or presets
  the channel's bend, pressure and CC 74 before the note on, `tuning`, `pressure` and `slide` send a bend, channel
  pressure and CC 74 only when the channel's value changes, `note_off` follows the key the note started on, and
  `mcm` is the MPE Configuration Message (RPN 6 on the master channel). `Initial::of` reads the expression events
  that share a note on's frame so they go first. The MIDI Out device (`mpe.zone`, `mpe.members`) sends everything
  as frames on its tap and announces the zone at its start, at each playback start and on a change. The plugin
  device (`mpe.send`, the last param) turns notes into `PluginEventKind::Midi` raw messages (wire tag 9) on member
  channels 2 to 16; the CLAP processor passes them to a plugin that takes MIDI and the VST3 processor ignores them
  (those plugins get note expression events). Both devices are allocation free on the audio thread.

## 7. The UI crate

`atomik-ui` has no window-system dependency and no time source. The app feeds `UiEvent`s and applies
the returned `UiAction`s; views draw through the `Canvas` trait into a display list (tested with
JSON goldens), and only `scene_canvas.rs` talks to the GPU (vello). Views: the rack, the arrangement
(with audio clips, waveforms, fade, gain and trim handles, the input selector and file-drop
mapping), the piano roll, the console, the transport bar, the device browser, the file browser panel
(6j) and the sample editor. The audio maths (tempo map, waveform columns, handle mapping, input
cable sets) is pure and headless in [ui/src/audio.rs](../crates/ui/src/audio.rs). The sample editor
([ui/src/sample_editor.rs](../crates/ui/src/sample_editor.rs)) replaces the arrangement like the
piano roll: a key × velocity map of the zones, a zone list, the selected zone's waveform with start,
end, loop and crossfade markers and numeric fields. Every edit emits `Command::SetZones` with a full
list that [zone_edit.rs](../crates/ui/src/zone_edit.rs) clamps into the SDK's rules, and file drops
on the editor or on a sampler panel become `UiAction::ImportSamples`; the sampler panel adds a
sample strip with an Edit button to the generic controls.

The step editor ([ui/src/step_editor.rs](../crates/ui/src/step_editor.rs)) also replaces the
arrangement and edits one note clip of an `atomik.drum-machine` as a grid of 16 pad rows (pad 1, key
36, at the bottom) by the clip's `StepGrid` steps: clicks and drags emit the piano roll's
`SetClipContent`, the toolbar changes resolution and length as one `Command::Batch` of `SetClipGrid`
and `SetClipGeometry` (the clip length follows the grid) and swing as merged `SetClipGrid` drags;
notes the grid cannot show are kept untouched. The drum machine has its own rack panel
(`autolayout::drum_layout`): a 4×4 pad grid (click selects and auditions, drops load the pad; a pad
lights when the engine plays it, from the hit counters the device publishes as meter values and fades
on the rack's tick), the
eighteen knobs of the selected pad (view state, remapped onto `padN.*`), master level and an Edit
button, and a back panel that groups the sockets per pad (outputs and gates) with the two global CV
jacks and trims in a CV section.

The slice editor ([ui/src/slice_editor.rs](../crates/ui/src/slice_editor.rs)) is the same kind of
pane for an `atomik.loop-slicer`: the loop waveform with a marker at every slice start (drag to
move, double-click to add, Delete to merge a slice into the previous one; the first marker is
fixed), per-slice level, pan, pitch and reverse, and a toolbar for the slicing mode (transients with
a sensitivity, or an even grid), `loop.beats`, Slice and To track. The UI holds no samples, so Slice
and To track only emit `UiAction::SliceLoop` and `UiAction::SlicesToTrack` for the app; every marker
or field edit is one `SetZones` built with the model's `slices_from_markers` (drags use merge ids
from 6·2⁴⁰). The slicer's rack panel (`autolayout::slicer_layout`) adds an overview strip, a
sensitivity knob (view state), the Slice, To track and Edit buttons and the device knobs.

The wavetable synth `atomik.wavetable-synth` keeps its tables out of the document and out of the
engine's data paths: [dsp/src/wavetable.rs](../crates/dsp/src/wavetable.rs) generates the twelve
factory tables (additive spectra, or a time-domain drawing read back with an FFT), builds each frame
once per octave of bandwidth with an inverse FFT of the truncated spectrum, and hands out a
`&'static WavetableBank` built once per process (`factory_bank`, on the thread that first creates
the device, never the audio thread). A device holds that reference, so a table change is a plain
param change and the audio thread only reads; a change fades from the old table to the new in each
voice. A sample dropped on the device becomes a thirteenth table (`imported`): the device takes
one sample like the convolver (`register_with_samples`), and where the sample map is made
(`atomik_render::samples::finish_map`, for the app and the bounce alike) `attach_table` builds the
stored table of the first zone with `wavetable::build_pyramid` and appends it to the map as one more
buffer, so the audio thread only reads it. The device publishes the modulated frame position of its
newest voice through `meter_values`. The panel
([ui/src/wavetable_panel.rs](../crates/ui/src/wavetable_panel.rs)) is the generated control layout,
with its sections put in an order that packs into seven rack units
(`autolayout::auto_layout_with_order`), and a strip at the left whose two displays draw one cycle of
each oscillator's current frame with `wavetable::display_frame` (the UI crate depends on the DSP
crate for this; it holds no table data) at the published position while a voice sounds.

The table editor stores a table the way a drop does, so nothing new is saved and the project format
is unchanged. [dsp/src/wavetable/edit.rs](../crates/dsp/src/wavetable/edit.rs) is pure maths on
frames of 2048 samples: the harmonic read and additive rebuild (`analyse`, `rebuild`,
`set_harmonic`), `paint_line` and the whole-frame processes, `align_fundamental` (a shift in time
through the harmonics, arithmetic only, so it is bit-identical on every platform), `bake` (key frames
to the 16 stored frames, the frames between keys by linear blend), `morph_warnings` and
`morph_dip_db`, and the import cleanup (`detect_cycle`, `cut_cycles`, `drop_near_duplicates`,
`crossfade_edges`, `source_frames`). The panel
([ui/src/wavetable_editor.rs](../crates/ui/src/wavetable_editor.rs), a modal wired in `shell.rs` like
the bass-read panel, opened by `UiAction::EditWavetable` from a click on a table display) keeps a
working copy of the key frames and emits one `WavetableAction::Commit` per finished gesture;
[app_core_wavetable.rs](../crates/app/src/app_core_wavetable.rs) bakes it and hands the mono asset to
`store_sample`, which registers it and replaces the zone in one `SetZones` step. The asset name
records the key frame count and whether the frames were aligned, which is how
`AppCore::wavetable_data` shows the key frames of a stored table again; a sample with any other name
is shown as it is cut, with its cycle length, and can be cut by cycle (`cut_wavetable_cycles`).

The FM synth `atomik.fm-synth` is plain arithmetic: [dsp/src/fm.rs](../crates/dsp/src/fm.rs) holds
the twelve operator routings (the panel draws its diagram from the same table), the ratio and
fixed-frequency rule and `limits`, which estimates the highest partial of every operator and returns
the scales that keep it under a bandwidth limit, so a bright patch plays duller on a high key
instead of aliasing (the voice stays allocation-free). A per-operator wave choice, poly glide, a chorus
and an `oversample` choice were appended after `bend.range`, so older patches load unchanged (the
defaults are the sine, off, mix 0 and 1x, which render the old sound bit for bit). With `oversample`
on, the voices run at twice the rate into a buffer that `atomik_dsp::oversample` decimates, the limiter
reaches 1.25 times as far, and the device reports the filter delay as variable latency. Its `sound`
choice works like the guitar amp's `setup`: `expand_setup` turns it into one `Command::Batch` of every
param of the chosen row of `devices::fm_synth::sounds`. A `sound` or `algorithm` choice of six entries
or more opens the rack's pick list (`master_panel::is_pick_list`) instead of stepping on a click.

The poly synth `atomik.poly-synth` is one voice engine that three devices share. Its params after
`bend.range` were appended (every default reproduces the earlier sound bit for bit, which
[synth_defaults.rs](../crates/devices/tests/synth_defaults.rs) pins with a hash of a busy render), so
projects and patches saved before them load unchanged and no format field was needed. `render_batch`
renders one batch from a `BatchIn` (notes and the CV inputs) into a `BatchOut`, with or without the
final soft limiter, and `PolySynth::with_descriptor` runs the same engine under another descriptor:
`atomik.mono-bass` ([mono_bass.rs](../crates/devices/src/mono_bass.rs)) is that engine with the
defaults of a bass (one voice, legato, glide, the ladder and its drive) built by a `const fn` over the
poly synth's param table. `atomik.dual-layer` ([dual_layer.rs](../crates/devices/src/dual_layer.rs))
holds two engines, feeds both the same batch without the limiter, mixes them with the gains of
`layer.mode` and `layer.morph` and limits the sum once, so each end of the morph is one layer bit for
bit. Its descriptor is built once on first use from the poly synth's table, with the layer prefixes
`a.` and `b.` on the keys (the strings are leaked once; a plugin descriptor is interned the same way).
The additive wave bank (`wave_bank` in `wavetable.rs`, 32 waves written as harmonics and built by
inverse FFT like the factory tables, read at the mip level its pitch allows) and the 24 dB ladder
(`ladder.rs`: four TPT stages solved without a delay, `f64` state, an anti-aliased clipper in front)
are in the DSP crate. New paths use the deterministic `tan` (`Svf::cutoff_to_g_det`) and `2^x`, so
they are bit-identical on every platform. The generic panel is used for all three devices; the poly
synth and the mono bass take seven rack units, the dual layer takes more.

The organ `atomik.organ` ([organ.rs](../crates/devices/src/organ.rs)) is a bank of 176 sine wheels, one per
semitone, each a rotating phasor advanced with additions and multiplications only and pulled back to the unit
circle once per batch, so the device is bit-identical on every platform and costs nothing while no key sounds.
A key reads nine wheels through the gains of the drawbars (three of them a pitch another key's drawbar may share);
the percussion envelope, the key click, the scanner (two delay lines swung by a sine) and the drive follow, and
the controller helper of the other instruments handles the pedal, volume and expression. Its `sound` param is
expanded by the app into one batch like the FM synth's (`organ::sounds`); the generic panel is used.

The grain sampler `atomik.granular` is registered with samples like the sampler, so the app builds
its `SampleMap` from its zone (a drop replaces the one zone) and swaps it in with
`set_device_samples`; a voice plays the first zone that matches the note (or, with the layered zone
mode, one voice for every matching zone), or a built-in vowel pad
(`dsp::grain::default_source`, cached per sample rate) while no map is installed.
[dsp/src/grain.rs](../crates/dsp/src/grain.rs) holds the windows, the fixed 64-grain pool (full: the
grain nearest its end is replaced), the grain clock and the read that low-passes a high
transposition; every voice owns its pool, so nothing allocates on the audio thread. The device
publishes 63 meter values (position, spray, grain counts and the last 29 grains), which the app
feeds the rack view each frame like an effect's meters, next to the peaks of the loaded assets for
the waveform.

The six effects (reverb, compressor, EQ, distortion, modulation, mastering) and the guitar amp have
hand-made rack panels with labelled sections and a small visual each
([ui/src/fx_layout.rs](../crates/ui/src/fx_layout.rs), [fx_paint.rs](../crates/ui/src/fx_paint.rs),
maths in [fx.rs](../crates/ui/src/fx.rs): decay sketch, transfer curve, EQ response with draggable
handles, distortion and clipper curves, LFO sketch, crossover and loudness readouts). The app calls
`RackView::set_device_meter_values(&DeviceKey, &[f32])` once per frame with
`AppCore::device_meter_values` for the effect devices; the view turns the values into dB display
values and repaints a panel only when one changed. The compressor panel's "SC from..." list
([ui/src/sidechain.rs](../crates/ui/src/sidechain.rs)) applies one `Command::Batch` (disconnect,
connect a channel's pre-fader insert send or a device's `out_l`/`out_r` to `sc_in_l`/`sc_in_r`, set
`sc_enable`), so it is one undo step.

The spectrum and the phase meter (section 6k) have panels of the same kind (`FxKind::Spectrum`,
`FxKind::PhaseMeter`; [ui/src/analyser.rs](../crates/ui/src/analyser.rs) and
[analyser_paint.rs](../crates/ui/src/analyser_paint.rs)) that also take the app's analysis through
`RackView::set_analyser`.

## 8. How the hard invariants are enforced

### HI-1: no allocation, lock or blocking on the render path

| Mechanism | Where |
|---|---|
| `Device::process` gets a `ProcessContext` of borrowed slices only; `prepare` is the only allocating method | [crates/device-sdk/src/context.rs](../crates/device-sdk/src/context.rs) |
| Every slot, scratch batch, note buffer, delay line, playback pair and ring is allocated in `compile`, `submit_song`, `record_tap` or `new_engine` | [plan.rs](../crates/engine/src/plan.rs), [engine.rs](../crates/engine/src/engine.rs), [stream.rs](../crates/engine/src/stream.rs), [record.rs](../crates/engine/src/record.rs) |
| Overflowing note events are dropped and counted; a full record ring drops the batch and counts it; an empty stream ring is silence plus a counter | `deliver_note`, `TapRt::capture`, `StreamLaneRt::take` |
| Retired plans, routings, songs, taps, sample maps (`ControlOp::Samples` hands the map a device returns from `swap_samples` to the queue) and devices go back through a queue and are dropped off the audio thread; a removed sampler takes its maps with it | `retire` in [engine.rs](../crates/engine/src/engine.rs), `collect_garbage` |
| A metering device's `ScopeTap` is a ring of atomics written with relaxed stores (one release store per batch); the engine only registers it, at insert or plan submit, never on the audio thread | `ScopeTap` in [device-sdk/src/scope.rs](../crates/device-sdk/src/scope.rs); [scope_tap.rs](../crates/engine/tests/scope_tap.rs) |
| Device meter values go from `Device::meter_values` into a preallocated atomic bank in `Meters` (stack scratch, atomics only; only opted-in devices are called) | `Arena::publish_device_values`, `Meters::publish_device_values`; [meter_values.rs](../crates/engine/tests/meter_values.rs) |
| The automation hold (the lanes a take is writing) is a fixed array of atomics the player loads once per batch; setting it is a few stores from the UI thread | `AutomationHold` in [player.rs](../crates/engine/src/player.rs); [song_automation_hold.rs](../crates/engine/tests/song_automation_hold.rs) runs it under the counting allocator |
| Disk, decoding, hashing and WAV writing exist only in non-RT crates and threads | `atomik-audio-file`, the app workers, the streamer |
| `unsafe` is denied workspace-wide; only the plugin backends and the sandbox wire (whole crates), two host modules, the test allocator and the test plugins allow it, each block with a `SAFETY:` comment | root `Cargo.toml`; the `unsafe` column of the crate table in section 1; [unsafe_policy.rs](../crates/plugin-host/tests/unsafe_policy.rs) |
| The plugin sandbox's engine side copies into the mapped block and spins on its own child for at most one batch period live (a bounce waits up to the watchdog): the one bounded wait on the render path, with no lock, no allocation and no blocking system call | `RemoteProcessor` in [plugin-sandbox/src/processor.rs](../crates/plugin-sandbox/src/processor.rs), `ParentPort` in [plugin-ipc/src/parent.rs](../crates/plugin-ipc/src/parent.rs); the no-alloc tests there and in [plugin-hostd/tests](../crates/plugin-hostd/tests) |

Tests (the counting allocator is described in [TESTING.md](TESTING.md#allocation-guard-hi-1)):

- `render_is_allocation_free`, `allocating_device_is_caught_by_guard`:
  [engine/tests/alloc.rs](../crates/engine/tests/alloc.rs); the real demo in
  [render/tests/alloc.rs](../crates/render/tests/alloc.rs); every device in
  `process_is_allocation_free_for_every_registered_device`
  ([devices/src/lib.rs](../crates/devices/src/lib.rs)).
- Workers: `multicore_alloc.rs` and `multicore_alloc_bites.rs` in
  [crates/engine/tests](../crates/engine/tests), and the worker guard in the testkit.
- `ac_a3_audio_bounce_is_bit_identical_and_allocation_free`
  ([audio_bounce_alloc.rs](../crates/engine/tests/audio_bounce_alloc.rs)); every render in
  [audio_stream.rs](../crates/engine/tests/audio_stream.rs) and
  [record_tap.rs](../crates/engine/tests/record_tap.rs) runs under `assert_no_alloc`;
  `streaming_and_recording_with_workers_are_allocation_free_and_bit_identical`
  ([audio_stream_workers.rs](../crates/engine/tests/audio_stream_workers.rs)) adds three workers
  with `forbid_on_this_thread`.
- the drum machine's and the slicer's `process_swap_and_params_are_allocation_free_...` tests (32
  and 16 voices, all outputs connected) and their legs of
  `process_is_allocation_free_for_every_registered_device`. Onset detection allocates by design and
  never runs on the audio thread.
- The preview voice: [preview_alloc.rs](../crates/engine/tests/preview_alloc.rs) renders under the
  counting allocator across start, a switch with both slots busy, underrun, finish, stop and
  retirement, with the streams created, fed and dropped on the handle side only.
- `swap_samples_never_allocates` and the sampler leg of
  `process_is_allocation_free_for_every_registered_device`;
  [sample_swap.rs](../crates/engine/tests/sample_swap.rs) checks that a replaced or removed map is
  freed by `collect_garbage`, never on the render thread.

### HI-2: determinism

| Mechanism | Where |
|---|---|
| `HashMap`, `HashSet`, `Instant::now` and `SystemTime::now` are banned by clippy; the exceptions are the CLI `--stats` timer, the audio-io telemetry, the clock of the CPU readout and the wall clock of the daily update check, which never influence audio | [clippy.toml](../clippy.toml); local `#[allow]`s |
| Graph, topology and compile state use `BTreeMap`/`BTreeSet`; ready sets are ordered by id; summing is a fixed-order fold | [graph.rs](../crates/engine/src/graph.rs), [plan.rs](../crates/engine/src/plan.rs), [console.rs](../crates/engine/src/console.rs) |
| Each device's seed is `mix_seed(patch seed, device id)` | `mix_seed` in [plan.rs](../crates/engine/src/plan.rs) |
| Fixed 64-frame batches behind a FIFO make the host block size irrelevant; workers only compute, the audio thread gathers and scatters | [engine.rs](../crates/engine/src/engine.rs), [workers.rs](../crates/engine/src/workers.rs) |
| Song positions are integer frames from integer tick spans; the audio fade polynomials (and the envelope and crossfade maths) use only IEEE `+ − × ÷` | [player.rs](../crates/engine/src/player.rs), `sin_quarter` in [audio.rs](../crates/engine/src/audio.rs) |
| Bounces load every asset fully; asset ids hash the decoded samples; the resampler's parameters are fixed | [render/src/bundle.rs](../crates/render/src/bundle.rs), [audio-file](../crates/audio-file/src) |

The file browser's preview is live only. `ControlOp::PreviewStart` is sent only by the app's file
browser through `EngineSink::preview_start`, which `LiveSession` forwards to the running engine.
Bounces and offline renders (`atomik-render`, `render_offline`) never go through `EngineSink`, so
offline output cannot contain a preview and is unchanged. `OfflineSink`, the hand-rendered engine of
the app's tests and stress runs, does forward previews to its engine so that tests can render one;
that is test support, not a render path.

Tests: `render_any_host_block_is_bit_identical`
([determinism.rs](../crates/engine/tests/determinism.rs)), the multicore proptest,
`cli_block_size_does_not_change_output` and the goldens
([render/tests/e2e.rs](../crates/render/tests/e2e.rs)), the project bounce tests,
`audio_bounces_are_bit_identical_across_blocks_and_workers` and
`a_bounce_of_several_audio_tracks_is_identical_across_blocks_and_workers`
([render/tests](../crates/render/tests)), and for the sampler
`the_bounce_is_bit_identical_across_host_blocks_and_worker_counts` and
`voice_stealing_and_layering_are_deterministic`
([sampler_demo.rs](../crates/render/tests/sampler_demo.rs)). Sampler voice stealing orders by a
per-device start counter, and the sample reader uses only IEEE arithmetic and `sin_quarter`.

### Fault containment (needs `panic = "unwind"`)

**Panic policy.** A panic is for a violated internal invariant only. None may be reachable from a
document, a file, a device or plugin, or an operating-system device event: those are errors or
contained faults. Every thread boundary turns a panic into a reported fault rather than a crash or
a silent stop: the audio callback and the engine workers (below), the app's command, undo and redo
(an "Internal error" toast that leaves the document intact, section 2.2), decoders and the plugin
boundaries. An `expect` in production code names the invariant it relies on.

A bug in a device must cost that device, not the session. The rules:

- `Node::run` ([plan.rs](../crates/engine/src/plan.rs)) wraps the device's `process` in
  `catch_unwind`, on the audio thread and on a worker alike. A panic marks the node faulted: its
  outputs are zeroed and it is skipped from then on, the engine stores the device's instance id in a
  sticky atomic ([fault.rs](../crates/engine/src/fault.rs)) and keeps rendering everything else.
  Nothing is re-raised. The calls a device gets outside `process` (a parameter, a sample map or
  pattern data hand-off, the reads of its meter values and latency) go through `Node::contain`, the
  same guard: a panic there faults only that node, and `Arena::settle_faults` records it once per
  batch (a bool test while nothing happened). `Engine::fault` and `EngineHandle::fault` report it; the audio callback
  publishes it in `Telemetry::faulted`/`faulted_device`, the status line shows
  `ENGINE FAULTED: <device>` and a toast points at Audio > Restart Audio, which rebuilds the engine.
  The callback also wraps the whole render in `catch_unwind` as a last line: a panic outside any
  device (the engine's own code) silences the stream and sets `Telemetry::faulted`.
- `catch_unwind` costs nothing while nothing panics, so HI-1 holds. The panic path allocates (the
  payload, the default hook's message); it runs once per faulted device and is a documented
  exception. The panic hook ([§6n](#6n-logs-crash-reports-and-diagnostics)) does no file I/O on the
  audio thread: it raises a flag and the UI thread writes the crash report.
- A worker thread that dies outside a job loses the nodes it owned; the barrier stops waiting for
  them, records their devices as faulted and the worker gets no more work. `Pool::new` returns a
  `PoolError` when no worker could be started, and the engine then renders single-threaded (the
  output does not depend on the worker count).
- `RunningStream::stop` returns an error instead of aborting when the backend does not release the stream within 5 s.
- Offline renders never abort: `try_render_offline*` ([offline.rs](../crates/engine/src/offline.rs))
  return an `OfflineError` for a rejected scripted edit, a full queue or a device that panicked (no
  audio is returned for a render that lost a device), `atomik_render::RenderError::Engine` carries
  it to the CLI (non-zero exit), the export and freeze workers and the QA server's `bounce`. The
  un-prefixed `render_offline*` functions panic on that error and exist for tests.
- **All of this depends on unwinding.** Under `panic = "abort"` every `catch_unwind` (devices,
  decoders, plugin boundaries, workers) does nothing and a contained fault becomes a process abort.
  `[profile.release]` sets `panic = "unwind"` with a comment, and `unwind_invariant.rs` in
  [crates/engine/tests](../crates/engine/tests) fails if any profile sets `abort`. Tests:
  `a_panicking_device_is_contained` ([multicore.rs](../crates/engine/tests/multicore.rs)),
  `a_panicking_decoder_returns_error` ([decode.rs](../crates/audio-file/src/decode.rs)).

### HI-3: graph validity

| Mechanism | Where |
|---|---|
| `Graph::connect` (and `Topology`) check ids, direction, kind, input uniqueness and cycles before changing anything; cycles are checked per (device, stage) | `connect`, `validate_cable`, `would_create_cycle` in [graph.rs](../crates/engine/src/graph.rs), [arena.rs](../crates/engine/src/arena.rs) |
| `compile` re-validates every cable and the toposort fails on a cycle (defence in depth) | [plan.rs](../crates/engine/src/plan.rs) |
| `RenderPlan` and `RoutingPlan` have no public mutators; `compile` is the only constructor | [plan.rs](../crates/engine/src/plan.rs) |
| The model rejects cables that the engine would reject, and a `Batch` of cable commands is validated in order and rejected as a whole | [crates/model/src/rules.rs](../crates/model/src/rules.rs) |
| Device descriptors are validated when registered | [registry.rs](../crates/device-sdk/src/registry.rs) |

Tests: the `GraphError` cases in [graph.rs](../crates/engine/src/graph.rs), the proptest
`random_connect_sequences_keep_graph_valid`, `compile_revalidates_corrupted_graph`, and the model's
`ac_s1` and `ac_u1` properties.

### HI-4: originality

This invariant is process, not code: contributions must be original work or compatibly licensed (see
[CONTRIBUTING.md](../CONTRIBUTING.md)). No build script or test refers to anything outside this
repository.

### Untrusted input

A project, a patch, a MIDI file, an audio file or a message from a plugin child can be corrupt or
made to be hostile. Each reader refuses or bounds it before the size it claims is allocated:

| Input | Bound | Where |
|---|---|---|
| Project, patch, clip and library item files | read only when the file is within `MAX_DOCUMENT_BYTES` (256 MiB; `ATOMIK_MAX_PROJECT_MIB` raises it), the refusal names the size | `read_document_text` in [read_limit.rs](../crates/model/src/read_limit.rs), used by the app, the renderer and the QA server |
| Audio files | at most `DEFAULT_MAX_SAMPLES` held in memory; a WAV header's sample count never sizes an allocation beyond the file; NaN and infinity become silence and are counted | [decode.rs](../crates/audio-file/src/decode.rs), [wav.rs](../crates/audio-file/src/wav.rs), [data.rs](../crates/audio-file/src/data.rs) |
| Standard MIDI Files | `MAX_TRACKS`, `MAX_EVENTS_PER_TRACK` and `MAX_EVENTS`; a chunk that claims more than the file has is read to its end | [smf.rs](../crates/midi-io/src/smf.rs) |
| Sandbox control channel | a frame and its blob over `MAX_FRAME_BYTES` are refused before anything is allocated; the blob grows as bytes arrive | `read_frame` in [frame.rs](../crates/plugin-ipc/src/frame.rs) |

Tests: the bounded-input tests (a header that claims gigabytes, under a counting allocator) in
[audio-file/tests/bounded.rs](../crates/audio-file/tests/bounded.rs), the robustness property tests
of [audio-file](../crates/audio-file/tests/robustness.rs),
[midi-io](../crates/midi-io/tests/robustness.rs) and
[model](../crates/model/tests/robustness.rs), and the fuzz targets of [fuzz/](../fuzz) that run on a
schedule (see [TESTING.md](TESTING.md#fuzzing)).

## 9. The audio interface pseudo-device

`Graph::new` creates device id 0, `Graph::INTERFACE`, with type id `atomik.audio-interface`.

- Its sockets `in_1..in_N` are Audio **outputs**: host input enters the graph through them. This is
  where an audio track's input cables start.
- Its sockets `out_1..out_M` are Audio **inputs**: host output leaves the graph through them.
- It is a pure source and a pure sink, so `in_k → … → out_j` is never a cycle.
- At most 32 channels each way (`MAX_INTERFACE_CHANNELS`).
- Its descriptor is built once per channel combination and cached. This happens off the render path.
  See [crates/engine/src/interface.rs](../crates/engine/src/interface.rs); the UI and the model name
  the same sockets.

## 10. Known limitations
- **Deep zoom of streamed audio.** An asset above the 64 MiB streaming threshold has no samples in
  the views, so below 256 frames per pixel its waveform stays at the finest peak level (blocky
  min/max) instead of reading the samples; the UI never reads the disk to paint.

- **Params are quantised to batch boundaries.** A param message takes effect at the next 64-frame
  boundary; devices smooth audible params themselves. Notes are sample-accurate. Song automation is
  applied once per batch.
- **Automation recording.** Only parameter changes that reach `AppCore::apply_command` are recorded;
  a mapped MIDI controller is such a change (see the MIDI mappings section), a controller can also
  be recorded into note clips as controller lanes (2.12). A Touch starts at the first move of a
  control, not at the press (a press without a move is not seen; a surface's touch-sensitive fader
  is the exception, it holds the gesture from the touch), and an un-dragged change (a typed value,
  the MCP server) touches for one tick. Arming a lane takes effect at the next take. A recorded
  lane is held in the engine from the touch; between touches the lane plays as it was, with a jump
  back at the release. With the tempo lane armed (the R button of its header) a take also records
  the tempo the map has at the playhead after each tempo edit, as an Overwrite pass: at the end the
  events inside the written stretch are replaced by the thinned points (`tempo_pass_map`) in one
  `SetTempoMap` that joins the last tempo edit's undo step. The take does not retime itself.
- **MIDI controller data.** Controller lanes belong to note clips or, outside clips, to a note track
  (Track > Edit Track Controllers, 2.12); a track's own lanes are shown only in the roll's track mode, not in the
  arrangement (its header has a CC badge). Per-note expression (MPE) is not a lane: it is kept by note (see
  "6p. Per-note expression"), is edited in the piano roll's strip and follows a note only while its start and key
  stay; a recorded lane is one `Step` point per burst of events, not a smooth curve. A ramp is played as steps
  of 40 ticks. Pressure is kept as a
  modulation source in five instruments but no parameter reads it yet, and the only registered
  parameter is RPN 0 (the bend range); there is no NRPN. Points are selected, copied and thinned
  one lane at a time: there is no selection across lanes and no duplicate of points.
- **MIDI files.** Type 2 files and SMPTE divisions are refused, RPN and NRPN data are not kept (the bend range
  of an MPE member channel and the zone message are read and written for MPE tracks only), and a tempo ramp
  is exported as a staircase and imported as steps. A song
  file does not carry audio tracks or automation lanes. The bytes of a SysEx message cannot be typed
  yet (the piano roll adds General MIDI System On, moves, deletes and clears messages). A
  file imported as one clip merges all its tracks and channels into that clip's controller lanes
  (the last value at a tick wins), and a channel that has controllers but no notes goes to the track's own
  controller lanes (a file imported as one clip folds those into the clip). The default export resolution is
  15 360 ticks per quarter note, which every reader should accept but a hardware sequencer may not; File > MIDI Export
  Resolution offers 960 and 480. The file browser does not list `.mid` files, so a file can only be
  dropped from the operating system.
- **Time-stretch.** A warped clip is rendered once into a buffer, so a change of tempo, setting or
  clip waits for a worker (the clip plays an interim resampled read meanwhile, at the right timing
  but the wrong pitch for a transposed clip; a streamed asset plays unwarped, and so does a
  reversed clip, backwards, until its buffer is ready); the stretch engines are offline only and
  the Tonal mode does not keep formants when transposing (2.18).
- **Tempo and meter.** Tempo ramps are linear in BPM only (no other curve shapes), meter changes
  only start on bar lines, and tempo-synced devices follow a ramp in steps of one 64-frame batch.
  The LFO of the modulation effects computes its synced phase from the absolute frame and the tempo
  of the batch (see DEVICES.md), so while the tempo ramps their phase moves with it; the delay, Mod
  Source, step sequencer and arpeggiator work from the tick and stay on the beat. The piano roll and
  the step editor count bars from the clip start under the meter in effect there.
- **Relinking missing audio.** A missing file is reported and its clips play silence until it is
  relinked (File > Project Files > Relink Missing Audio...). A relink keeps zone ranges and clip offsets as they
  are, so a replacement shorter than a zone or a clip offset needs is refused; the folder search
  matches by file name only (no content search), looks 8 levels deep and at 50 000 entries at most,
  and a bounce still stops with an error naming a missing asset.
- **Sampler scope.** One interpolation mode (cubic Hermite), no release samples, stereo output only, no
  zero-crossing snap, and zones are RAM-only. SFZ and SoundFont 2 files import the common subset into
  zones (the readers are in `atomik-audio-file`, the zones are made off the UI thread; see the sampler in
  DEVICES.md for what is read); release samples, keyswitches, modulators and a SoundFont's other presets
  are not. A bounce stops with an error on a missing zone asset, while the live app plays silence.
- **Drum machine and slicer scope.** No kit presets, per-pad envelopes or sample-start, time-stretch
  inside slices or REX import; the slicer is one loop with at most 64 slices.
- **Effects.** The limiter's true-peak detection is the BS.1770 4x one; a 16x oversampled reference
  reads full-band white noise up to about 1 dB over the ceiling. Changing the compressor look-ahead
  or the distortion oversampling changes the reported latency and makes one audible discontinuity at
  the change. No linear-phase EQ, no FFT analyser.
- **Players.** The step sequencer is monophonic (one note per step, no chords, ratchets or flams),
  has no pattern chains or song mode (the CV sequencer chains patterns) and plays only with the
  transport; the arpeggiator has no mod or bend CV outputs and no render of its own (Send Players
  to Track renders a whole stack); a rack-wide player header is not drawn: Direct Record, Send
  Players to Track, Fold All and Bypass All Players are Track menu items. The note tap is lost if
  the engine is rebuilt during a take.
  The drum generator (flams, ratchets) and the CV sequencer hold 32 patterns, but the
  CV sequencer's rack panel and grid editor still show eight slots: later patterns are reached with
  the `pattern` param, the pattern commands and the project file.
- **Containers.** No standalone container presets, no cut and paste across scopes, no container
  level or CPU meter, linear macros only, one split per target instrument, a combine keeps one
  song track, channel or FX return and drops the automation lanes of the device that had them,
  a bypassed container cuts its inside without a crossfade, no CV or sidechain sockets on the
  boundary, and `export_patch` writes a container without its inside (patches are flat; the project
  file keeps it). The container editor has no View-menu entry (EDIT on the panel opens it).
- **Recordings and streaming.** A recorded lap is read back into RAM before the app decides whether
  to stream it, and the reported device latency can miss buffering that a driver does not report;
  the user offset in the settings corrects that.
- **MIDI mappings and control surfaces.** A mapping has one source and one target (a source may have
  several mappings, a target may have several), a preset addresses a device by its position among
  the devices of its type (so reordering the rack changes what it addresses) and not at all inside
  a container (the preset of one device addresses by type alone), a project that uses an NRPN or RPN
  source is refused by an older build of format 43, machine control has no device number and no
  per-port switch (a mapping-role port always obeys it), a mapped tempo
  change is one undo step per message (the tempo has no merge group), two controls moved at once
  alternate into separate undo steps, a relative encoder cannot be a pitch bend or a note, a VCA
  level is mapped linearly in dB, the selected-strip role resolves at each message (so the soft
  takeover restarts in effect when the selection changes), and plugin windows cannot be learned from
  (use the parameter pane's controls or the right-click menu). The surface driver has no level
  meters, flip, assignment or function keys, no extender unit and no second protocol (see the
  roadmap); a surface that does not answer the device query still works, and the handshake and the
  display layout are hand-checked on hardware only.
- **Macros and the search.** A macro drives a param of a top-level device only (a container keeps
  its own macros, and a target inside one is refused), the mapping is linear in the plain value (a
  cutoff moves linearly in Hz), a macro does not drive another bank's macro, a driven param cannot
  also have its own lane, a direct edit of a driven param is refused (not folded into the range), so
  the Macros panel is the one place to change a range, and a bank is one rack device (no per-track
  bank is made for you). The search lists a container's inside by path key but cannot assign it (see
  the container macros), has no saved queries or history, and reads a plugin's params from the
  layout the project saved.
- **Feedback loops are rejected, except through a Feedback Route.** A cable that would close a loop
  returns `WouldCreateCycle` (the model says `BadCable`), unless the loop passes a stage boundary, and
  the only device with one that carries a signal back is `atomik.feedback`. Its delay is at least one
  engine batch (64 frames) and cannot be shorter, a loop's own latency is not compensated, and there is
  no feedback send and return pair, so two places in a rack are joined by cables only.
- **Platform-dependent last bits.** Output is bit-identical across host block sizes and worker
  counts on one platform. Across operating systems, math-library differences can change the last
  bits, so the SHA-256 goldens are checked on Windows x86-64 only (see
  [TESTING.md](TESTING.md#golden-files)). Only the pinned stage blocks and the deterministic helpers of
  the DSP crate claim bit identity on every platform; no cross-platform bit-identity goal is set
  for the older primitives, whose sound is kept by fingerprints (0.5 dB) on every platform, and
  the sound of every released saved-file format by the corpus (see
  [Bit-identity claims](TESTING.md#bit-identity-claims)).
- **Analysers.** A plugin device has no scope tap, so no spectrum or goniometer (its own meters
  stay); the analysers read the audio of the device they sit on, so they are meters of a chain, not
  of a console channel or bus (no console panel yet); the reference curve is a stored one (no
  second device to compare against); a headless session reads them with `session_meters` (an
  offline render), not live.
- **Plugins.** Sandboxed (the default), each instance is a process of its own and its editor a
  separate window; a live batch that the child answers late passes dry. In process, a native crash
  takes the app down (the crash journal and recovery snapshots limit the damage). The plugin is
  always called with 64-frame blocks, so a plugin that prefers large blocks costs more CPU.
  Bit-identical output is guaranteed only for plugins that are deterministic functions of their
  inputs (the test plugins); real plugins that use time or random seeds are not, and a bounce is not
  guaranteed to equal what was heard.  no Wayland embedding in process or without `DISPLAY` (the
  PARAMS pane is the fallback), a change made in the plugin's own window outside a gesture is not
  shown until the state is captured, pins default to the first eight params (not the plugin's
  remote-controls page), CLAP presets and VST3 program lists are not listed (only `.vstpreset`
  files), the Windows DPI choice per plugin is not offered, recovery snapshots hold the project
  file without its audio bundle, and surround and multi-output are not supported. A plugin gets per-note
  expression as note expression events (CLAP and VST3); MPE MIDI on member channels goes only to a CLAP plugin that
  takes MIDI, with the plugin device's `mpe.send` switched on.
- **File browser.** Tags and favourites are keyed by path, so moving or renaming a file outside the
  app loses them. A preview plays at 0 dB with no level control and no cue output, and the browser
  shows no waveform. The tree does not expand to reveal the folder shown in the list. Folders are
  not watched; they are listed again when shown or refreshed. A third preview start within 5 ms of a
  switch cuts the oldest stream (the voice has two slots). Dropping a browser drag onto the
  transport bar or the console cancels it (an OS drop there imports the files at the playhead). The
  browser lists audio files only: devices, patches and projects are not in it.

## 11. Notes for maintainers

- Notes inputs always report `is_connected() == true`, because the scheduler can feed any of them.
- The compiler has its own `BTreeMap`-based Kahn sort and cycle search; no graph library is used.
- After a change, re-check the dependency table in section 1 against the `Cargo.toml` files, and the
  test names in this document with `grep -rn "fn <name>(" crates`.
