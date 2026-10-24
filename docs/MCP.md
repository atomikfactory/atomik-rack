# The MCP server (QA and testing)

`atomik-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio. It lets an MCP client (an AI coding assistant, or any program that speaks the protocol) do the checks a person does by hand: build or load a project, bounce it, measure and compare the audio, and drive the real app shell with menu commands, pointer and keys, then read the screen as text or as a PNG. There is no window, no GPU and no audio device involved, so it runs the same on a developer machine and in CI.

It is a development tool. Nothing in the app links it and releases do not ship it.

**What it is for**

- Checking a change to the sound: bounce before and after, compare, and read where and by how much the loudness moved.
- Checking a change to the UI or a workflow: open a project, run a command or a gesture, read the texts the UI draws, look at the screenshot, assert on the document.
- Reproducing a bug report from a project file, and keeping the evidence (a WAV, a PNG, a JSON report).
- Exploratory QA that does not deserve a permanent test yet.

**What it is not for.** It does not replace the Rust tests: a behaviour that matters gets a headless test in the repository ([TESTING.md](TESTING.md)). It does not play audio (the session has no audio device), does not load third-party plugins in a bounce, and does not show the GPU output: a screenshot is drawn by a CPU rasteriser from the same display lists, so it is close to the app but not pixel-equal to it. Listening to a render, plugin editors and hardware stay manual checks.

## Build and run

```sh
cargo build -p atomik-mcp          # target/debug/atomik-mcp (or .exe)
target/debug/atomik-mcp --help
```

```text
atomik-mcp [--root DIR]...
```

The server reads one JSON-RPC message per line on stdin and writes one per line on stdout. The working directory is always an allowed folder; each `--root` adds one. Relative paths in a tool call are taken from the working directory.

### Connecting a client

Any MCP client takes a command and its arguments. As JSON, in the usual `mcpServers` form:

```json
{
  "mcpServers": {
    "atomik": {
      "command": "target/debug/atomik-mcp",
      "args": []
    }
  }
}
```

Start the client in the repository root so that `examples/` and `target/` resolve. On Windows the command is `target/debug/atomik-mcp.exe`. Use a release build for long bounces (`cargo build --release -p atomik-mcp`, then `target/release/atomik-mcp`). To let the server build itself on start, use `"command": "cargo"` with `"args": ["run", "-q", "-p", "atomik-mcp", "--"]`; the first start then waits for the compile, which some clients time out on.

Do not commit a client configuration: per [CONTRIBUTING.md](../CONTRIBUTING.md) local tool files stay out of the tree (`/.mcp.json` is ignored).

### What the server may touch

Every path argument is resolved from the working directory and must end up inside the working directory or a `--root` folder, after symbolic links and `..` are resolved. A path outside is refused with a message that names the allowed folders, for reads and writes alike. Outputs default to `target/mcp/` under the working directory, which `.gitignore` already covers (`/target`, `*.wav`).

## The tools

A call that fails returns a result with `isError` set and a message; the model can read it and retry. Results are pretty-printed JSON text, except the screenshot, which also carries an image block. dB values are floored at -240 (silence). Times are seconds, positions are logical pixels (see [Pointer](#pointer-and-keys)).

### Stateless tools

| Tool | Arguments | Result |
|---|---|---|
| `list_devices` | `type_id`, `kind`, `query` | The built-in device types (id, name, kind, parameter and socket counts), filtered; with `type_id` the full descriptor: every parameter key with range, default, taper, unit, and every socket. Use it to write a patch or project that loads. |
| `project_inspect` | `path`, `include_params` | A project (`.atomik` or a bundle folder) or a patch, described: devices, cables, plugin and asset counts, the song (tracks, lanes, clips, tempo events, loop, markers, end in ticks and beats, `ticks_per_beat`) and the console. The first problem of a bad file is an error with its JSON path. |
| `validate` | `path` | `{valid, kind, warnings}` or `{valid: false, error}`. A file that does not load is data here, not a failed call. No rendering. |
| `bounce` | `path`, `output`, `format` (`f32`, `pcm24`), `block`, `workers`, `loop_count`, `window_s`, `reference`, `tolerance_db` | Renders a project (its song, end + 2 beats) or a patch to a WAV with the code of `atomik-render`, and returns the audio report plus `wall_s` and `realtime_factor`. With `reference`, also the comparison with that WAV. Projects that use third-party plugins are refused with the `atomik-render --allow-plugins` hint. |
| `analyze_audio` | `path`, `window_s` | The audio report of a WAV (below). |
| `compare_audio` | `a`, `b`, `tolerance_db`, `window_s` | Whether two WAVs match (below). |
| `list_plugins` | `cache` | The plugins in the app's scan cache (`plugins.json`): type id, name, vendor, format, file, and the blacklist. It never loads a plugin; scan with the app first. |

**The audio report** (`analyze_audio`, and `audio` in a bounce): sample rate, channels, frames, duration, peak and RMS in dBFS (overall and per channel), DC offset, clipped samples (at or above full scale), non-finite samples (NaN, infinity), `silent`, the silent stretches (below -80 dBFS for at least 50 ms), the RMS of consecutive windows (`window_rms_dbfs`, default 0.5 s, widened so that at most 400 are reported: this is the loudness fingerprint of the goldens) and a coarse spectrum: the share of the power in eight bands (sub, bass, low-mid, mid, upper-mid, presence, brilliance, air) and the dominant frequency.

**The comparison** (`compare_audio`, and `comparison` in a bounce): `same_shape` (rate, channels and length equal), `bit_identical`, `max_abs_diff`, `first_diff_frame`, the largest per-window loudness difference `max_window_diff_db` and where (`worst_window_s`). `matches` is true when the shapes agree and no window differs by more than `tolerance_db` (default 0.5, the tolerance of the render goldens). The comparison is made on the samples of the render, before any 24-bit quantisation.

### Session tools

One headless app at a time: the real `Shell` with the audio off and no window, the same code the smoke test and the shell tests drive. `session_open` replaces any open session; the other tools fail with "no open session" until one is open.

| Tool | Arguments | What it does |
|---|---|---|
| `session_open` | `path`, `width`, `height`, `scale`, `os` | Starts the app on the starter project, or on `path`. A bad file is refused with its JSON path and the previous session is kept. `os` (`windows`, `mac`, `linux`) picks the keymap and shortcut texts. Returns the status. |
| `session_status` | `include_params` | Title, unsaved changes, status line, focused and lower pane, undo and redo availability, window size, and the document (as `project_inspect`). |
| `session_commands` | `menu`, `query`, `enabled_only` | The menu commands: menu, label, `id`, shortcut, checked, enabled. |
| `session_command` | `id`, `confirm`, `wait` | Runs a menu command as the menu or its shortcut would. A command that needs a native dialog (`file.open`...) or the "discard unsaved changes" confirmation is reported in `requests` and does not run silently; `confirm: true` accepts the discard. A freeze or bounce renders on a worker and the session has no frame loop, so the call waits for the render to finish unless `wait` is `false` (the answer then has `waited` and the render's `message`, for example `Froze Lead`). |
| `session_pointer` | `action`, `x`, `y`, `to_x`, `to_y`, `steps`, `dx`, `dy`, `button`, `modifiers` | `move`, `down`, `up`, `click`, `double_click`, `drag`, `wheel`, `leave`. |
| `session_key` | `key` or `text`, `modifiers` | Presses and releases a key, or types characters. |
| `session_texts` | `contains`, `limit` | Every text the UI draws, with its position and size. |
| `session_screenshot` | `save_to` | A PNG of the window as the app paints it, returned as an image; `save_to` also writes the file. |
| `session_save` | `path` | Saves the document as a project. |
| `session_relink` | `file`, `asset`, `folder` | Relinks the missing audio of the open project, answering the dialogs of File > Relink Missing Audio... (`session_command` `file.relink_audio` opens the list; its LOCATE... and SEARCH FOLDER... buttons raise native dialogs, reported as `pick_relink_path`). With `file`, that audio file replaces the missing asset `asset` (its file name in any case, or the start of its id; optional when exactly one is missing); with `folder`, the folder is searched and the missing files matched by name. Waits for the import and returns `relinked`, `still_missing`, `document_changed`, `message` (the toast), `dirty` and `can_undo`. |
| `session_midi` | `action`, `file`, `scope`, `as_song` | Imports or exports a Standard MIDI File, answering the dialogs of File > Import MIDI... and File > Export MIDI... (`session_command` `file.import_midi`, `file.import_midi_song`, `file.export_midi_clip`, `file.export_midi_track` and `file.export_midi_song` raise native dialogs, reported as `pick_midi_file`). `action: import` reads `file`: one clip on the selected note track, or new tracks when none is selected; with `as_song`, new tracks from the start plus the file's tempo map, meter and markers. `action: export` writes `file` for `scope` (`clip` the selected note clip, `track` the selected note track, `song` every note track, the default). One undo step per import. Returns `message` (the toast), `document_changed`, `tracks`, `dirty`, `can_undo` and, for an export, `bytes`. |
| `session_bounce` | as `bounce` without `path` | Bounces the document as it is now, edits included, saved or not, and returns the report. Audio clips play from the session's bundle (the project's own folder, or the scratch bundle of this session's imports); a project with missing audio is refused until it is relinked. |
| `session_close` | | Closes the session without saving. |

#### Pointer and keys

Positions are **logical pixels**: the window size you asked for (default 1280 x 860), whatever the scale. A screenshot at scale 2 is twice as many pixels, so divide its coordinates by the scale; `session_texts` already reports logical positions, so use it to find what to aim at (the `y` of a text is its baseline: aim a few pixels above it). Modifiers are `shift`, `ctrl`, `alt`, `meta`, and `cmd` (meta on `mac`, ctrl elsewhere). Keys are a single character, `Escape`, `Enter`, `Space`, `Backspace`, `Delete`, `Tab`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Home`, `End`, `Insert`, `F1`..`F24`, `Digit0`..`Digit9` (the physical digit row, for Alt chords) and `Numpad0`..`Numpad9`. What a chord does is the keymap's business: `session_commands` lists the shortcuts, and a key test through `session_key` checks the binding as well as the command.

#### Command ids

An id is the registry command id, with `:argument` when the command has one (for example `view.toggle_browser`, `edit.undo`, `transport.add_marker`). `session_commands` is the source of truth: it lists each id with its label and shortcut ([KEYBOARD.md](KEYBOARD.md) lists the same commands by label).

## Recipes

These are the QA steps this project expects for a change, in the order to do them. Each names the tools; the CLI equivalents (`atomik-render`, the headless tests) work for anyone without an MCP client.

**A change to a device, the engine or the bounce (sound)**

1. Before the change: `bounce` the example projects and patches that exercise it into `target/mcp/before/`.
2. After the change: `bounce` again with `reference` set to the earlier WAV. Expect `bit_identical: true` for a refactor, or `matches: false` naming the window for an intended change, and read `window_rms_dbfs`, `peak_dbfs` and the spectrum bands to confirm the change is the one you meant (a darker filter lowers the upper bands, not the bass).
3. Determinism (HI-2): bounce the same file with `block` 64, 333 and 512 and `workers` 0 and 2; `compare_audio` must say `bit_identical`.
4. Sanity: `clipped_samples` and `non_finite_samples` are 0, `silent` is false where it should not be.
5. Listen to it yourself if the change is meant to sound different; the report cannot hear clicks or aliasing.

**A change to the UI or a workflow**

1. `session_open` the project that shows it (or the starter project).
2. Reach the state: `session_command` for menu commands, `session_pointer` and `session_key` for gestures and chords.
3. Read the screen with `session_texts` (cheap, exact) and look at `session_screenshot` for layout, overlap and clipping. Save a screenshot with `save_to` when it belongs in the PR.
4. Assert on the document with `session_status`: the devices, cables, tracks (each with `muted`, `solo`, `armed` and `chase`, and a folder, colour or comment when it has one), markers, `dirty`, `can_undo`. Then `edit.undo` and check that one step undoes it.
5. If the edit should change the sound (or must not), `session_bounce` with `reference` set to the bounce of the file you opened.
6. Turn what you proved into a headless test (`crates/app/src/shell_*_tests.rs`, `crates/ui/tests/workflow_*.rs`) before the PR is done.

**Note chase**

Chase Notes (`sequencer.toggle_chase`) is an item of the track header's context menu, not of the menu bar, so `session_commands` does not list it. Reach it with `session_pointer`: right-click the track's header, then click Chase Notes. `session_status` shows the track's `chase` flag (on by default; a project file stores `"no_chase": true` for a track that does not chase).

**Track organisation**

The Track menu (`session_commands` with `menu: "Track"`) holds Group Tracks (`sequencer.group_tracks`), Ungroup, Collapse or Expand Folder (`sequencer.toggle_folder`), Rename Folder, Move Track Up and Down, Track Comment, Fold and Unfold Automation Lanes (`sequencer.fold_lanes:fold`, `:unfold`) and Track Height (`sequencer.track_height:small`, `:medium`, `:large`, `:fit`). They work on the selection, which is the pointer's: click a track header (shift-click adds one) or a folder row's name, then run the command; a disabled one says so. Track Colour (`sequencer.track_color`) is in the context menus only: right-click a header and choose from its Colour submenu. The typed edits (a folder name, a track comment) are text: `session_command` opens the field, then `session_key` with `text` types it and `Enter` commits. `session_status` and `project_inspect` list the song's `folders` (id, name, parent, collapsed, colour, the devices inside) and, per track, `folder`, `color` and `comment`, per automation lane `folded` and per take group `open`; each only when there is one. `crates/model/tests/golden/organised_project.json` is a small project that has all of them.

**Freeze and bounce in place**

Freeze Track and its siblings are Track menu commands (`session_commands` with `menu: "Track"`): `sequencer.freeze_track`, `sequencer.freeze_track_to_device` (up to the rack's selected device), `sequencer.freeze_tail` (a check item, on by default), `sequencer.freeze_cancel`, `sequencer.unfreeze_track`, `sequencer.unfreeze_track_keep`, `sequencer.bounce_in_place` and `sequencer.bounce_in_place_mute`. They work on the selected tracks or the tracks of the selected clips, so click a track header (or a clip) with `session_pointer` first; a command that cannot run now is disabled and `session_command` says which. To check that a freeze is transparent:

1. `session_open` a copy of the project in a temporary folder (a freeze stores its render in the project's bundle, so do not open a file inside `examples/`; `session_save` the example to `target/mcp/` first and open that).
2. Select the track, then `session_bounce` into `target/mcp/before.wav`.
3. `session_command` `sequencer.freeze_track`: the call waits for the render and answers with `message` `Froze <track>`. `session_status` shows `frozen` on the track (`start`, `length`, `asset`, the frozen `effects`), its own `lanes` still listed.
4. `session_bounce` into `after.wav` and `compare_audio` the two: a chain without latency or tail is `bit_identical`; with a reverb the difference is below -90 dBFS (the tail is cut there), and a chain with latency is the same audio earlier by that latency.
5. `edit.undo` brings the track back in one step; `session_texts` lists `(frozen)` in the lane while it is frozen.

**Automation recording**

An automation lane's header has a mode button (`OVR`, `TCH` or `LTC`: click to cycle, one undo step each) and a `W` arm button, so `session_texts` finds them by their text and `session_pointer` clicks them. `session_status` shows `armed_lanes` (session state, never saved, not an undo step), `recording`, and each automation lane's `write` mode in `document.song.tracks[].lanes[]` (`project_inspect` shows the same). With a lane armed, `session_command` `transport.record` starts a take without an armed track; a parameter change made while it runs (a knob drag with `session_pointer`) is written, and `transport.record` again (or stop) lands the take as automation clips, one undo step. The session has no audio device, so the transport does not advance: use the headless tests (`crates/app/src/app_core_autorec_tests.rs`) for the written ticks.

**Controller lanes**

`project_inspect` (and `session_status`, under `document.song.tracks[].lanes[]`) lists the controller kinds a note lane uses, `"controllers": [{"kind": "pitch_bend", "points": 12}, {"kind": "cc", "number": 1, "points": 4}]`, summed over the lane's clips and left out when there are none. In the piano roll, `session_pointer` clicks the CONTROLLERS field of the scale bar (find it with `session_texts`), then a chip (BEND, PRESS, MOD 1, VOL 7, PAN 10, EXPR 11, SUST 64, `< CC n >`, `< POLY n >`, RAMP or STEP) and presses, drags and releases in the lane under it; `edit.undo` takes a whole stroke back. The session has no audio device and no MIDI input, so playback and recording are covered by the headless tests (`crates/engine/tests/song_controllers.rs`, `crates/app/src/record_tests.rs`); to hear a lane, `bounce` a project that has one with a device that responds (the poly synth follows pitch bend and CC 1) and `compare_audio` it with the same project without the lane.

`project_inspect` (and `session_status`) lists the audio clips of an audio lane under `"audio_clips"`: `[{"id": 10, "start": 61440, "length": 122880, "muted": false, "asset": "d5745abd", "source_offset": 48000, "gain_db": -3.0, "fade_in": 24000, "fade_out": 48000, "fade_curve": "linear", "crossfade": "equal_power", "reverse": true, "envelope": 3, "crossfade_ticks": 15360}]` (`reverse`, `envelope`, the number of gain points, and `crossfade_ticks`, the length of the crossfade zone at the clip's start, appear only when in use; `crossfade` is the curve or `off`). The audio editing commands are menu commands, so `session_command` runs them by id (`sequencer.audio_reverse`, `sequencer.audio_normalise`, `sequencer.audio_gain:up`, `sequencer.audio_fade_shape:s_curve`, `sequencer.audio_crossfade:off`, `sequencer.audio_fades:declick`, `sequencer.audio_envelope_clear`; `sequencer.audio_strip_silence` opens the panel, whose `-`, `+` and `APPLY` buttons `session_pointer` clicks, found with `session_texts`) on the clips selected by clicking them; draw envelope points with the pencil tool (`tool.pencil`) by clicking in a clip. To QA a crossfade, `bounce` the project with the clips overlapping and again with `sequencer.audio_crossfade:off`, and `compare_audio` the two: they differ only inside the overlap, and `analyze_audio` shows no jump at the seam of the crossfaded bounce.

A warped or transposed audio clip adds `"warp": {"enabled": true, "mode": "tonal", "bpm": 96.5, "transpose_cents": -300, "markers": 4}` to its `"audio_clips"` entry (left out at the default). The warp commands are menu commands too: `sequencer.audio_warp`, `sequencer.audio_warp_mode:beats|tonal|repitch`, `sequencer.audio_transpose:up|down|octave_up|octave_down|reset`, `sequencer.audio_warp_tempo`, `sequencer.audio_warp_bars:1|2|4|8|16`, `sequencer.audio_quantize` (all need a selected audio clip) and `sequencer.audio_warp_panel` (its `-`, `+`, `APPLY` and `DETECT TEMPO` buttons are found with `session_texts`). To QA a warp, `session_bounce` the project warped (the bounce renders every buffer first, so it is never dry), `analyze_audio` it (the dominant frequency of a transposed tone, the silent stretches of a squeezed loop), and `compare_audio` two bounces at other `block` and `workers` values: they must be `bit_identical`.

`project_inspect` (and `session_status`) also lists a lane's take groups, `"take_groups": [{"id": 1, "takes": 4, "start": 0, "end": 122880, "comp": [{"take": 3, "start": 0, "end": 122880}], "picked": 3, "clips": [5]}]` (takes are 0-based; `picked` is the take that plays the whole range alone, `null` for a mix; `clips` are the plain lane clips the comp compiles to), left out for a lane without groups. To QA comping: `session_open` a project with a group (a loop recording makes one; the session has no audio device, so use a saved file or build one with a headless test), find the header of the strip with `session_texts` (`Takes (4)` and `FLATTEN`), `session_pointer` click its chevron to open the take rows (`Take 1` to `Take 4`), click the name of a take to pick it, drag across a take row to comp a range, and read the result with `session_status`; `edit.undo` takes each step back. `bounce` the project before and after and `compare_audio` the two: a comp differs from a pick only inside the comped range, and a splice inside an audio take dips to silence for a few milliseconds (`analyze_audio` shows no peak at the splice).

`project_inspect` (and `session_status`) also lists the song's groove library, `"song": {"grooves": [{"name": "Groove 1", "step": "1/16", "slots": [[0.0, 1.0], [0.22, 0.7]]}]}` (slots are `[offset in steps, velocity scale]`; the built-in grooves are not listed), left out for a song without grooves. To QA quantize: `session_open` a project, select a clip with `session_pointer`, run `session_command` `edit.quantize_options` (Edit > Quantize Options), read the panel with `session_texts` (`QUANTIZE`, `Applies to: 1 selected clip`, `APPLY`), click its buttons by their text positions with `session_pointer` (`-` and `+` step a value; `APPLY`, `EXTRACT GROOVE FROM CLIP` and `DELETE` act), and look at the notes with `session_screenshot` or `session_status`. Escape closes the panel.

**Missing audio and relink**

`session_open` a bundle project whose `Audio/<hash>.wav` was deleted, `session_commands` with `query: relink` (the command is enabled only while audio is missing), `session_command` `file.relink_audio` and read the panel with `session_texts` or `session_screenshot`. Then `session_relink` with a `file` (different audio: one undo step, `document_changed` true) or the same audio (file moved or renamed: the file is restored, `document_changed` false) or a `folder`. `session_bounce` must now be audible and the same on every run; `edit.undo` brings the missing state back and enables the command again.

**Reproducing a bug report**

`validate` and `project_inspect` the file first (does it load, what is in it), then `session_open` it and follow the report. A bug that needs the file to be edited is reproduced by a `session_save` of the state that triggers it; attach that project to the issue.

**Checking an example or a new example project**

`validate`, then `bounce` and read the report: the duration is the song end plus two beats, the peak is in range, nothing is silent that should play, and the same file bounced twice is identical.

**A pull request**

Say in "How it was tested" which of these you ran and what they showed (for example "bounce of `demo.atomik` bit-identical before and after, 4 blocks and 2 worker counts"). The checklist items for tests and gates are unchanged.

## Limits and notes

- **No live audio.** The session opens no stream, so Play moves nothing audible and the engine telemetry of a live run (xruns, meters under load) is not available. The bounce is the sound check; the live twins are covered by the `app_core_*_tests`.
- **No plugins in a bounce.** `bounce` and `session_bounce` refuse a project that uses a third-party plugin. Use `atomik-render --allow-plugins` (sandbox and plugin cache apply), then `analyze_audio` and `compare_audio` on the WAV.
- **A screenshot is a CPU render.** The GPU path (`vello`) draws the same display lists, but anti-aliasing and text rasterisation differ slightly. Do not diff screenshots pixel by pixel across machines; the display-list goldens are the stable check.
- **One session.** `session_open` replaces the previous one.
- **Determinism.** Bounces are deterministic and so is the shell's painting; use that: an unexpected difference between two runs is itself a finding.
- **Wall time** in a bounce result is real time and varies; everything else does not.

## Extending the server

A tool is an entry of `TOOLS` in [crates/mcp/src/tier1.rs](../crates/mcp/src/tier1.rs) (stateless) or [session.rs](../crates/mcp/src/session.rs) (stateful): a name, a function that builds its definition, and a function `fn(&mut Server, &Args) -> Result<ToolOutput, String>`. A tool:

1. validates every argument with `Args` (a message that names the argument), and every path with `server.paths()` (never `std::fs` on a raw string);
2. returns JSON through `ToolOutput::json`, with stable key names, a result a model can use without a second call, and an error message that says what to do next;
3. gets a test in `crates/mcp/tests/` that goes through `Server::call_tool` against the example projects, including the failure and the path-policy cases, and the protocol test in `protocol.rs` that lists the expected tool names is updated;
4. is added to the tables above and to the instructions string in [crates/mcp/src/rpc.rs](../crates/mcp/src/rpc.rs).

Keep the server a thin layer over code that already has its own tests: `atomik-render` for bounces, `atomik-model` for documents, `Shell` for the app. It must stay deterministic, take no wall-clock input into a result (other than the labelled `wall_s`), open no audio device and no network connection, and write only where `Paths` allows.

The tests are `cargo test -p atomik-mcp`: the protocol over the real binary, every stateless tool against the examples (including bit-identical bounces across blocks and workers, a changed patch caught against its reference, and the path policy), and every session tool against a real headless shell (commands, undo, the confirmation, pointer and key input, texts and screenshots, save and a session bounce that equals the bounce of the same file).
