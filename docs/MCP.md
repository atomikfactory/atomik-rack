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
| `project_inspect` | `path`, `include_params` | A project (`.atomik` or a bundle folder), a patch or a library item, described (`kind`: `project`, `patch`, `patch_item`, `container_item` or `clip`; an item has a one line `summary`, a clip is described by its tempo, meter and `clip` length, offset, loop and note count): devices, cables, plugin and asset counts, the song (tracks, lanes, clips, tempo events, loop, markers, end in ticks and beats, `ticks_per_beat`) and the console. The first problem of a bad file is an error with its JSON path. |
| `validate` | `path` | `{valid, kind, warnings}` or `{valid: false, error}`. A file that does not load is data here, not a failed call. No rendering. Library items are checked by extension: `.atomikpatch` (`kind` `patch_item`), `.atomikcontainer` (`container_item`, which must hold exactly one container) and `.atomikclip` (`clip`). |
| `library_list` | `folder`, `section`, `query` | The items of a library folder, as the Library sections of the file browser list them: the `.atomikpatch`, `.atomikcontainer`, `.atomikclip` and `.atomik` files under `Patches/`, `Containers/`, `Clips/` and `Projects/` (sub-folders included; a missing section folder is empty). Each item has `section`, `kind`, `name`, `path` (relative to `folder`), `size` and, when it reads, a one line `detail` (`3 devices: Poly Synth, Delay, ...`, `Container, 5 inside`, `4 bars, 32 notes`, `120 BPM, 8 tracks`). `section` is `patches`, `containers`, `clips` or `projects`; `query` words must all be in the name. Sorted by section and path, links not followed, at most 1000 items. `folder` must lie inside the allowed folders. |
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
| `session_open` | `path`, `width`, `height`, `scale`, `os`, `library` | Starts the app on the starter project, or on `path`. A bad file is refused with its JSON path and the previous session is kept. `os` (`windows`, `mac`, `linux`) picks the keymap and shortcut texts. `library` is the session's library folder (inside the allowed folders, made when missing; a session without it has none, and Save to Library says so). Returns the status, with `library`. |
| `session_status` | `include_params` | Title, unsaved changes, `recent_files` (the songs opened or saved this session, newest first), status line, focused and lower pane, undo and redo availability, window size, `windows` (per open window: `kind`, `detached`, `logical`, `scale`, `physical`, `focused`), and the document (as `project_inspect`). |
| `session_commands` | `menu`, `query`, `enabled_only` | The menu commands: menu, label, `id`, shortcut, checked, enabled. |
| `session_command` | `id`, `arg`, `confirm`, `wait` | Runs a menu command as the menu or its shortcut would; `arg` is the argument of the commands that take one the menu cannot spell, the id of a VCA group for `mixer.add_to_vca` and `mixer.delete_vca` (ids are in `session_status`, `document.console.vcas`; the Mixer menu commands act on the selection of the focused pane), or the id or the name of a snapshot for `mixer.recall_snapshot`, `mixer.update_snapshot`, `mixer.compare_snapshot` and `mixer.delete_snapshot` (`<snapshot>:<scope>` for a scoped recall; `mixer.store_snapshot` takes the new snapshot's name). A command that needs a native dialog (`file.open`...) or the unsaved-changes question (Save, Don't Save, Cancel: the request is `confirm_save`, and `session_save` then repeating the command is the Save answer) is reported in `requests` and does not run silently; `confirm: true` answers Don't Save. `file.close` is such a command; `file.open_recent:<n>` (the place in the recent list, 0 the newest) opens a recent song. A freeze or bounce renders on a worker and the session has no frame loop, so the call waits for the render to finish unless `wait` is `false` (the answer then has `waited` and the render's `message`, for example `Froze Lead`). |
| `session_pointer` | `window`, `action`, `x`, `y`, `to_x`, `to_y`, `steps`, `dx`, `dy`, `button`, `modifiers`, `wait` | `move`, `down`, `up`, `click`, `double_click`, `drag`, `wheel`, `leave`. `wait` (seconds) lets the pointer rest where it is, which is how a tooltip appears (after 0.6 s on a button); the answer has `tooltip` (its text, with the shortcut) or null. |
| `session_key` | `window`, `key` or `text`, `modifiers` | Presses and releases a key, or types characters. |
| `session_texts` | `window`, `contains`, `limit` | Every text the UI draws, with its position and size. |
| `session_screenshot` | `window`, `save_to` | A PNG of the window as the app paints it, returned as an image; `save_to` also writes the file. With `window` it is that window, at its own physical size. |
| `session_window` | `action`, `window`, `width`, `height` | Plays the OS for the windows: `focus`, `close` (a detached window; attaches its pane again, the main window is refused: use `session_close`) or `resize` (`width` x `height` logical, 200..8192). Returns the open windows. |
| `session_save` | `path` | Saves the document as a project. |
| `session_relink` | `file`, `asset`, `folder` | Relinks the missing audio of the open project, answering the dialogs of File > Relink Missing Audio... (`session_command` `file.relink_audio` opens the list; its LOCATE... and SEARCH FOLDER... buttons raise native dialogs, reported as `pick_relink_path`). With `file`, that audio file replaces the missing asset `asset` (its file name in any case, or the start of its id; optional when exactly one is missing); with `folder`, the folder is searched and the missing files matched by name. Waits for the import and returns `relinked`, `still_missing`, `document_changed`, `message` (the toast), `dirty` and `can_undo`. |
| `session_midi` | `action`, `file`, `scope`, `as_song` | Imports or exports a Standard MIDI File, answering the dialogs of File > Import MIDI... and File > Export MIDI... (`session_command` `file.import_midi`, `file.import_midi_song`, `file.export_midi_clip`, `file.export_midi_track` and `file.export_midi_song` raise native dialogs, reported as `pick_midi_file`). `action: import` reads `file`: one clip on the selected note track, or new tracks when none is selected; with `as_song`, new tracks from the start plus the file's tempo map, meter and markers. `action: export` writes `file` for `scope` (`clip` the selected note clip, `track` the selected note track, `song` every note track, the default). One undo step per import. Returns `message` (the toast), `document_changed`, `tracks`, `dirty`, `can_undo` and, for an export, `bytes`. |
| `session_audio_notes` | `mode`, `sensitivity`, `min_note_ms`, `key_lo`, `key_hi`, `drift_cents`, `open_hats`, `grid`, `new_track` | Runs Audio > Audio to Notes... on the first selected audio clip (select one with `session_pointer`; the command `sequencer.audio_to_notes` only opens the panel) with these settings (the panel's defaults for what is left out), waits for the worker and returns `started`, `message` (the toast), `document_changed`, `tracks`, `dirty`, `can_undo` and, for a result, `count` and the new clip's `notes` (`tick`, `length`, `key`, `velocity`, at most 500). One undo step; a result with no notes is a message and no change. |
| `session_set_param` | `device`, `param`, `value` | Sets a param of a device as turning its knob does (one undo step, clamped to the range) and returns the value it has now. It is how a headless session sets a channel strip's input trim (`input.gain`, dB), polarity (`input.phase`), parallel compressor (`pcomp.on`, `pcomp.threshold`, `pcomp.ratio`, `pcomp.attack`, `pcomp.release`, `pcomp.makeup`, `pcomp.mix`) or the `bypass` of an insert effect. `list_devices` with a `type_id` gives the keys and ranges. |
| `session_meters` | `at_s`, `device` | Reads what the spectrum and phase meter devices show at `at_s` seconds (default 1) without an audio stream: the document is rendered offline up to there with the code of `session_bounce`, the analyser panels are filled (so `session_texts` and `session_screenshot` show the readings) and the numbers come back per device. Phase meter `values`: `peak_l`, `peak_r`, `hold_l`, `hold_r`, `rms_l`, `rms_r` (linear), `sample_peak_dbfs`, `true_peak_dbtp`, `correlation`, `balance`, `width`, `mid_rms`, `side_rms`, `momentary_lufs`, `short_term_lufs`, `integrated_lufs`. Spectrum `values`: `peak_hz`, `peak_db` of the loudest bin, and `bands`, the 31 one-third-octave levels as `[centre Hz, dB]`. `device` limits it to one key. dB values floor at -240; a project with third-party plugins is refused. |
| `session_bounce` | as `bounce` without `path` | Bounces the document as it is now, edits included, saved or not, and returns the report. Audio clips play from the session's bundle (the project's own folder, or the scratch bundle of this session's imports); a project with missing audio is refused until it is relinked. |
| `session_export` | `file`, `span`, `format`, `dither`, `sample_rate`, `tail`, `window_s` | Exports the document as an audio file, answering the dialogs of File > Export Song / Loop as Audio File... (`session_command` `file.export_song_audio` and `file.export_loop_audio` raise a native dialog, reported as `pick_export_audio`). The app's export, run synchronously: `span` `song` (default) is the start to the End marker plus the audible tail (at most 10 s, cut below -90 dBFS; `tail: false` ends at the End), `loop` is exactly the frames between the locators and has no tail. `format` is `pcm16` (dithered: `dither` `none`, `tpdf` (default) or `shaped`, for `pcm16` only), `pcm24` (no dither, the default) or `f32`; `sample_rate` (8000 to 192000) converts from the project's rate. A project with plugins is refused (export it from the app with the sandbox on). Returns `path`, `bytes`, `format`, `dither`, `sample_rate`, `frames`, `seconds`, `span_frames`, `tail_frames`, `warnings` and the `audio` report of the file read back. |
| `session_midi_input` | `messages`, `port`, `interval`, `wait` | Sends MIDI messages to the control path as the live input would (a headless session has no port): `messages` is hex, one message per `;` (`B0 07 7F; 90 3C 64`), a note, a controller or a pitch bend. Each runs one frame, `interval` seconds after the last (default 0.02); `wait` seconds then pass, which lets a held mapped move go (half a second without a message). Returns the learn state (`learn_mode`, `armed`), `last`, `received`, `mappings`, `document_changed`, `message` and `can_undo`. |
| `session_midi_learn` | `action`, `target`, `on`, `id`, `min`, `max`, `invert`, `mode`, `step`, `pickup`, `consume`, `file` | MIDI learn and mappings. `action`: `mode` (`on`: MIDI Learn mode, as the Mixer menu item), `arm` (a `target`: the next control moved with `session_midi_input` becomes its source), `cancel`, `forget` (every mapping of a `target`), `edit` (mapping `id`: any of the options), `remove`, `clear`, `export` and `import` (a preset `file`). A `target` is `<device key> <param key>` (`channel fader`), `vca <id>`, `tempo`, `selected <param key>` or `transport <play\|stop\|toggle_play\|record\|loop\|return_to_zero>`. Returns the learn state and `midi_maps`. |
| `session_macros` | `action`, `bank`, `macro`, `device`, `param`, `id`, `min`, `max`, `invert`, `new_bank`, `name`, `value`, `on` | Macro banks. `action`: `new_bank`, `bank` (the current bank), `assign` (`device`, `param`, `macro` 1 to 8, with `bank` or `new_bank`, `min`, `max`, `invert`; a param follows one macro and assigning it again moves it), `range` (assignment `id`; what is left out stays), `remove` (`id`, or `device` and `param`: the param keeps its value), `clear`, `rename`, `set` (the macro's value), `follow` (`on`: Automation Follows Selection) and `lane` (show the lane of a param, adding the empty lane; a param a macro drives shows the macro's lane, an effect has none and the message says to use a macro). A refusal is `done: false` with the app's `message`. Returns the banks (macros, names, values, assignments), `current_bank`, `follow_selection` and `can_undo`. |
| `param_search` | `path`, `query`, `limit` | Searches every parameter of a project file by track, device, param name, key and unit (every word must match; `>` separates words: `lead > filter > cut`; prefixes beat substrings beat fuzzy matches). Each result is `track`, `device`, `param`, a `target` (`device` and `param` keys, or a `vca` id), the `unit`, the `value` the project stores (the macro's when one drives it, with the `macro`). Best first, at most `limit` (default 25, at most 500); `indexed` counts the params. |
| `session_param_search` | `query`, `limit`, `action`, `row`, `steps`, `fine` | The same search in the session, with the rows of the Find Parameter panel (the value as it reads there, `macro`, `midi`, `lane`). `action` acts on result `row` (default 0): `reveal` (select it in the rack or the console), `lane` (show its lane), `assign` (the next free macro of the current bank), `learn` (arm it for MIDI learn; refused for a driven param) or `adjust` (`steps` of a fiftieth of the range, `fine` a tenth). Returns `done`, the rows, the last `message` and `can_undo`. |
| `session_library` | `action`, `path`, `devices`, `clip`, `slot`, `container`, `track`, `lane`, `tick` | The library of the open session (it needs `session_open` with `library`). `save_selection` does what **Save to Library** does for the selected rack devices (or the keys in `devices`, which it selects first): a patch in `Patches/`, or a container in `Containers/` when exactly one container is selected, with its audio in a sibling folder; it is not an edit, so `dirty` and the undo history stay as they were. `save_clip` saves the selected note clip (or clip number `clip`) into `Clips/` as `<track> bar <bar>`. Both return `saved` (relative to the library folder, with `/`), `section`, `message`, `status_text`, `dirty`, `can_undo` and `missing_audio`. `load` does what a double click on the item does: `path` is an item file (relative to the library folder, else to the working directory; inside the allowed folders) or a device row `atomik-device:<type id>`. Devices go after the selected device, or into rack slot `slot` (`column:index`; a column past the last makes a new one; `container` names the container the slot is in). A clip goes on note lane `lane` (default 0) of the track `track` at `tick`, or on the selected note track at the snapped playhead. One undo step; the item's audio is copied into the session and loaded, and what the item lacks is listed in `missing_audio`. A project is refused (open it with `session_open`). A refusal, such as a clip with no instrument track selected, is an error carrying the app's status line. |
| `session_surface` | `action`, `driver`, `messages` | Plays a control surface on an in-memory port. `attach` (the driver `mcu`: returns what the host sends first), `send` (`messages`: the surface's messages as hex, a fader is pitch bend `E0 <lsb> <msb>`, a key a note on `90 <note> 7F`, a SysEx `F0 ... F7`), `frame` (the feedback for a change made elsewhere), `detach`. Returns the messages the host sent to the surface as hex (`sent`) and the counters. |
| `session_close` | | Closes the session without saving. |

#### Pointer and keys

Positions are **logical pixels**: the window size you asked for (default 1280 x 860), whatever the scale. A screenshot at scale 2 is twice as many pixels, so divide its coordinates by the scale; `session_texts` already reports logical positions, so use it to find what to aim at (the `y` of a text is its baseline: aim a few pixels above it). Modifiers are `shift`, `ctrl`, `alt`, `meta`, and `cmd` (meta on `mac`, ctrl elsewhere). Keys are a single character, `Escape`, `Enter`, `Space`, `Backspace`, `Delete`, `Tab`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Home`, `End`, `Insert`, `F1`..`F24`, `Digit0`..`Digit9` (the physical digit row, for Alt chords) and `Numpad0`..`Numpad9`. What a chord does is the keymap's business: `session_commands` lists the shortcuts, and a key test through `session_key` checks the binding as well as the command.

#### Clip inspector

With audio clips selected (`session_pointer` `click` on a clip, `shift` to add) the transport bar draws the inspector right of the signature: `session_texts` `contains: FADE` finds it, and a `drag` or `wheel` on a field edits every selected clip; `edit.undo` takes the whole edit back in one step. Resize the session (`session_window` `resize`) to see it drop its fields on a narrow bar.

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
4. Assert on the document with `session_status`: the devices, cables, tracks (each with `muted`, `solo`, `armed` and `chase`, and a folder, colour, comment, `notes_to` (the device its notes enter, the top of its player stack) or `rec_input` (`gain_db` and `clip_safe`, the recording input of an audio track, listed when not at 0 dB with clip-safe off; not the channel strip's `input_gain_db`) when it has one), markers, `dirty`, `can_undo`. Then `edit.undo` and check that one step undoes it.
5. If the edit should change the sound (or must not), `session_bounce` with `reference` set to the bounce of the file you opened.
6. Turn what you proved into a headless test (`crates/app/src/shell_*_tests.rs`, `crates/ui/tests/workflow_*.rs`) before the PR is done.


**Tooltips**

Move the pointer onto a button with `session_pointer` `action: "move"` and `wait: 1`: the answer's `tooltip` is the text a user would see (the shortcut comes from the keymap, so rebind a key first with `keymap.json` and `session_open` to see it change), and `session_texts` and `session_screenshot` show the box. `leave`, a click, a key or the wheel hides it. `Show tooltips` is in Settings, Workflow (`workflow.json`: `"show_tooltips": false`), and with it off `tooltip` stays null.

**Detached windows**

The rack and the mixer can live in windows of their own, and a headless session has them too, with no platform.

1. `session_open`, then `session_command` `view.detach_rack` and `view.detach_console` (they toggle). `session_status` lists `windows`: the main one, then each detached one with its `logical` and `physical` size (a detached window starts at 1280 x 720 for the rack and 1280 x 520 for the mixer, at the main scale), its `scale` and whether it is `focused`.
2. Aim at a window with `window` (`rack` or `console`; `main` is the default) on `session_pointer`, `session_key`, `session_texts` and `session_screenshot`. Positions are logical pixels of that window, which a detached pane fills from `0,0`. A window that is not open is refused with `the rack window is not open: run view.detach_rack`.
3. A key sent to a window that does not have the focus gives it the focus first, as clicking into the window would, so `session_status` `focus` follows (`Rack`, `Console`) and the keymap context with it. Transport, undo and save keys work from every window.
4. `session_window` plays the operating system: `focus`, `resize` (`width`, `height`) and `close`. Closing a detached window attaches its pane to the main window again, as the close button does; the main window is refused (`session_close` ends the session).
5. Screenshots are CPU renders at each window's own physical size: a screenshot of the rack window is the rack alone, with no menu bar and no status bar.

The cross-window drag of a file from the file browser needs the desktop position of the pointer, which only the real platform has; it is covered by `crates/app/src/shell_windows_tests.rs`, and the multi-monitor and GPU behaviour by the hand checks in `docs/TESTING.md`.

**Note chase**

Chase Notes (`sequencer.toggle_chase`) is an item of the track header's context menu, not of the menu bar, so `session_commands` does not list it. Reach it with `session_pointer`: right-click the track's header, then click Chase Notes. `session_status` shows the track's `chase` flag (on by default; a project file stores `"no_chase": true` for a track that does not chase).

**Note lanes**

A track can have several note lanes. The commands are items of the Track menu (and of the track header's context menu), so `session_commands` lists them and `session_command` runs them on the selected track and lane: click a track header (or a lane's label) with `session_pointer`, then `sequencer.add_note_lane`, `sequencer.rename_note_lane` (the name is typed: `session_key` with `text`, `Enter` commits), `sequencer.delete_note_lane` and `sequencer.toggle_lane_mute`. `session_status` and `project_inspect` give every note lane `index`, `label` (what the header shows) and, when set, `name` and `"muted": true`. `session_texts` shows the labels ("Notes 2").

**Track organisation**

The Track menu (`session_commands` with `menu: "Track"`) holds Group Tracks (`sequencer.group_tracks`), Ungroup, Collapse or Expand Folder (`sequencer.toggle_folder`), Rename Folder, Move Track Up and Down, Rename Track (`sequencer.rename_track`), Track Comment, Fold and Unfold Automation Lanes (`sequencer.fold_lanes:fold`, `:unfold`) and Track Height (`sequencer.track_height:small`, `:medium`, `:large`, `:fit`). They work on the selection, which is the pointer's: click a track header (shift-click adds one) or a folder row's name, then run the command (or double-click the name in the header with `session_pointer`, `double_click`); a disabled one says so. Track Colour (`sequencer.track_color`) is in the context menus only: right-click a header and choose from its Colour submenu. The typed edits (a folder name, a track name, a track comment) are text: `session_command` opens the field, then `session_key` with `text` types it and `Enter` commits. `session_status` and `project_inspect` list the song's `folders` (id, name, parent, collapsed, colour, the devices inside) and, per track, `folder`, `color` and `comment`, per automation lane `folded` and per take group `open`; each only when there is one. `crates/model/tests/golden/organised_project.json` is a small project that has all of them.

**Freeze and bounce in place**

Freeze Track and its siblings are Track menu commands (`session_commands` with `menu: "Track"`): `sequencer.freeze_track`, `sequencer.freeze_track_to_device` (up to the rack's selected device), `sequencer.freeze_tail` (a check item, on by default), `sequencer.freeze_cancel`, `sequencer.unfreeze_track`, `sequencer.unfreeze_track_keep`, `sequencer.bounce_in_place` and `sequencer.bounce_in_place_mute`. They work on the selected tracks or the tracks of the selected clips, so click a track header (or a clip) with `session_pointer` first; a command that cannot run now is disabled and `session_command` says which. To check that a freeze is transparent:

1. `session_open` a copy of the project in a temporary folder (a freeze stores its render in the project's bundle, so do not open a file inside `examples/`; `session_save` the example to `target/mcp/` first and open that).
2. Select the track, then `session_bounce` into `target/mcp/before.wav`.
3. `session_command` `sequencer.freeze_track`: the call waits for the render and answers with `message` `Froze <track>`. `session_status` shows `frozen` on the track (`start`, `length`, `asset`, the frozen `effects`), its own `lanes` still listed.
4. `session_bounce` into `after.wav` and `compare_audio` the two: a chain without latency or tail is `bit_identical`; with a reverb the difference is below -90 dBFS (the tail is cut there), and a chain with latency is the same audio earlier by that latency.
5. `edit.undo` brings the track back in one step; `session_texts` lists `(frozen)` in the lane while it is frozen.

**Spectrum and metering**

The spectrum and phase meter devices draw from the audio they see, which a session without a stream never plays, so `session_meters` renders the project offline up to a position and reads them. To check them on a generated tone:

1. Write a stereo test tone as a WAV (for the loudness reference, a 1 kHz sine at -23 dBFS peak in both channels, a few seconds long), import it to a new audio track, and put a phase meter and a spectrum on the track channel's insert chain (the Create Effect browser lists them as Phase Meter and Spectrum); save, and `session_open` the project with a tall `height` (3600) so that every panel is on screen.
2. `session_meters` with `at_s` after the tone has run for a while (3 s or more for the short-term window): the phase meter should read `momentary_lufs` and `integrated_lufs` of -23 (within 0.2), `correlation` +1, `balance` and `width` 0, `sample_peak_dbfs` -23; the spectrum's `peak_hz` within one bin of 1000 and its 1 kHz band at -23 dB with the neighbours tens of dB lower. Invert or attenuate one channel of the file and see `correlation` -1, `balance` and `width` follow.
3. `session_texts` with `contains: LUFS`, `CORRELATION` or `PEAK` finds the readings as the panels draw them (`session_screenshot` shows the goniometer and the plot); the same call twice returns the same numbers.

**Audio to notes**

`sequencer.audio_to_notes` (Audio menu, enabled with an audio clip selected) only opens the panel; `session_audio_notes` is the headless way to run it with settings. To check it on a generated file:

1. Write a test signal (a sine melody, or a click and noise pattern for drums) as a WAV file, `session_open` a project whose audio track plays it (build one by importing the file in an app test, or open a saved project), and select the clip with `session_pointer` (`session_texts` finds the clip label; click just under it).
2. `session_audio_notes` with `mode: pitched` (or `drums`) and, if wanted, `grid`, `key_lo` and `key_hi`: the call waits for the worker and answers with the new clip's `notes`; compare their `key` and `tick` with what you generated (at 120 BPM a second is 30 720 ticks). A project's `session_status` lists the new track's note lane.
3. `edit.undo` removes the track and the clip in one step. A file with no notes in it answers `No notes found in the clip` and changes nothing; a clip warped in Repitch mode is refused with a message.

**Automation recording**

An automation lane's header has a mode button (`OVR`, `TCH` or `LTC`: click to cycle, one undo step each) and a `W` arm button, so `session_texts` finds them by their text and `session_pointer` clicks them. `session_status` shows `armed_lanes` (session state, never saved, not an undo step), `recording`, and each automation lane's `write` mode in `document.song.tracks[].lanes[]` (`project_inspect` shows the same). With a lane armed, `session_command` `transport.record` starts a take without an armed track; a parameter change made while it runs (a knob drag with `session_pointer`) is written, and `transport.record` again (or stop) lands the take as automation clips, one undo step. The session has no audio device, so the transport does not advance: use the headless tests (`crates/app/src/app_core_autorec_tests.rs`) for the written ticks. A console channel, the master or an effect is automated the same way: its lanes are on an automation track (`document.song.tracks[]` with `"automation_only": true`, no note lane, the first lane in the header row, so its `W` and mode buttons are in the track header); right-click a control (`session_pointer` with the secondary button) and run `automation.edit_control` (Edit Automation) to make the track and the lane, or use `session_macros` `lane`.

**Controller lanes**

`project_inspect` (and `session_status`, under `document.song.tracks[].lanes[]`) lists the controller kinds a note lane uses, `"controllers": [{"kind": "pitch_bend", "points": 12}, {"kind": "cc", "number": 1, "points": 4}]`, summed over the lane's clips and left out when there are none. In the piano roll, `session_pointer` clicks the CONTROLLERS field of the scale bar (find it with `session_texts`), then a chip (BEND, PRESS, MOD 1, VOL 7, PAN 10, EXPR 11, SUST 64, `< CC n >`, `< POLY n >`, RAMP or STEP) and presses, drags and releases in the lane under it; `edit.undo` takes a whole stroke back. The session has no audio device and no MIDI input, so playback and recording are covered by the headless tests (`crates/engine/tests/song_controllers.rs`, `crates/app/src/record_tests.rs`); to hear a lane, `bounce` a project that has one with a device that responds (the poly synth follows pitch bend and CC 1) and `compare_audio` it with the same project without the lane.

`project_inspect` (and `session_status`) lists the audio clips of an audio lane under `"audio_clips"`: `[{"id": 10, "start": 61440, "length": 122880, "muted": false, "asset": "d5745abd", "source_offset": 48000, "gain_db": -3.0, "fade_in": 24000, "fade_out": 48000, "fade_curve": "linear", "crossfade": "equal_power", "reverse": true, "envelope": 3, "crossfade_ticks": 15360}]` (`reverse`, `envelope`, the number of gain points, and `crossfade_ticks`, the length of the crossfade zone at the clip's start, appear only when in use; `crossfade` is the curve or `off`). The audio editing commands are menu commands, so `session_command` runs them by id (`sequencer.audio_reverse`, `sequencer.audio_normalise`, `sequencer.audio_gain:up`, `sequencer.audio_fade_shape:s_curve`, `sequencer.audio_crossfade:off`, `sequencer.audio_fades:declick`, `sequencer.audio_envelope_clear`; `sequencer.audio_strip_silence` opens the panel, whose `-`, `+` and `APPLY` buttons `session_pointer` clicks, found with `session_texts`) on the clips selected by clicking them; draw envelope points with the pencil tool (`tool.pencil`) by clicking in a clip. To QA a crossfade, `bounce` the project with the clips overlapping and again with `sequencer.audio_crossfade:off`, and `compare_audio` the two: they differ only inside the overlap, and `analyze_audio` shows no jump at the seam of the crossfaded bounce.

A warped or transposed audio clip adds `"warp": {"enabled": true, "mode": "tonal", "bpm": 96.5, "transpose_cents": -300, "markers": 4}` to its `"audio_clips"` entry (left out at the default). The warp commands are menu commands too: `sequencer.audio_warp`, `sequencer.audio_warp_mode:beats|tonal|repitch`, `sequencer.audio_transpose:up|down|octave_up|octave_down|reset`, `sequencer.audio_warp_tempo`, `sequencer.audio_warp_bars:1|2|4|8|16`, `sequencer.audio_quantize` (all need a selected audio clip) and `sequencer.audio_warp_panel` (its `-`, `+`, `APPLY` and `DETECT TEMPO` buttons are found with `session_texts`). To QA a warp, `session_bounce` the project warped (the bounce renders every buffer first, so it is never dry), `analyze_audio` it (the dominant frequency of a transposed tone, the silent stretches of a squeezed loop), and `compare_audio` two bounces at other `block` and `workers` values: they must be `bit_identical`.

`project_inspect` (and `session_status`) also lists a lane's take groups, `"take_groups": [{"id": 1, "takes": 4, "start": 0, "end": 122880, "comp": [{"take": 3, "start": 0, "end": 122880}], "picked": 3, "clips": [5]}]` (takes are 0-based; `picked` is the take that plays the whole range alone, `null` for a mix; `clips` are the plain lane clips the comp compiles to), left out for a lane without groups. To QA comping: `session_open` a project with a group (a loop recording makes one; the session has no audio device, so use a saved file or build one with a headless test), find the header of the strip with `session_texts` (`Takes (4)` and `FLATTEN`), `session_pointer` click its chevron to open the take rows (`Take 1` to `Take 4`), click the name of a take to pick it, drag across a take row to comp a range, and read the result with `session_status`; `edit.undo` takes each step back. `bounce` the project before and after and `compare_audio` the two: a comp differs from a pick only inside the comped range, and a splice inside an audio take dips to silence for a few milliseconds (`analyze_audio` shows no peak at the splice).

`project_inspect` (and `session_status`) also lists the song's groove library, `"song": {"grooves": [{"name": "Groove 1", "step": "1/16", "slots": [[0.0, 1.0], [0.22, 0.7]]}]}` (slots are `[offset in steps, velocity scale]`; the built-in grooves are not listed), left out for a song without grooves. To QA quantize: `session_open` a project, select a clip with `session_pointer`, run `session_command` `edit.quantize_options` (Edit > Quantize Options), read the panel with `session_texts` (`QUANTIZE`, `Applies to: 1 selected clip`, `APPLY`), click its buttons by their text positions with `session_pointer` (`-` and `+` step a value; `APPLY`, `EXTRACT GROOVE FROM CLIP` and `DELETE` act), and look at the notes with `session_screenshot` or `session_status`. Escape closes the panel.

**Missing audio and relink**

`session_open` a bundle project whose `Audio/<hash>.wav` was deleted, `session_commands` with `query: relink` (the command is enabled only while audio is missing), `session_command` `file.relink_audio` and read the panel with `session_texts` or `session_screenshot`. Then `session_relink` with a `file` (different audio: one undo step, `document_changed` true) or the same audio (file moved or renamed: the file is restored, `document_changed` false) or a `folder`. `session_bounce` must now be audible and the same on every run; `edit.undo` brings the missing state back and enables the command again.

**Channel strip: trim, polarity, parallel compression and insert bypass**

`session_status` lists the strips that use them under `document.console.strips`: `strip`, `input_gain_db`, `polarity_inverted`, `parallel` (when it is on or any of its params is set: `on`, `threshold_db`, `ratio`, `attack_ms`, `release_ms`, `makeup_db`, `mix`) and, for a strip with inserts, `inserts` (each effect with `bypassed`, `null` when it has no bypass) and `inserts_bypassed`. A strip with nothing to report is left out, so a project that uses none of it reads as before. To check one:

1. `session_open` the project, `session_bounce` into `target/mcp/before.wav`.
2. `session_set_param` with `device` the strip's key (the `channel` of the demo, or the keys under `console` in the status), `param` `input.gain` or `input.phase`, then `session_bounce` with `reference` the first WAV: `bit_identical` is false, and setting the values back makes it true again. A strip with `pcomp.on` 1 and `pcomp.mix` 0 (or `pcomp.on` 0) bounces bit for bit as before; at `pcomp.mix` 1 `analyze_audio` shows the compressed level (a lower peak with `pcomp.makeup` 0).
3. For the insert bypass, put an effect on the strip's insert chain (`session_command` `create.effect`, or open a project that has one), `session_set_param` its `bypass` to 1, or select the strip (click its name plate) and run `mixer.bypass_all_inserts`; `inserts_bypassed` turns true and one `edit.undo` undoes the whole command. `mixer.toggle_polarity` flips the selected strips the same way.
4. Plugins are not loaded by a bounce (see Limits); the alignment of a bypassed latent plugin is a test (`app_core_plugins_latency_tests.rs`) and a hand check.

**Mixer snapshots**

`session_status` lists the snapshots under `document.console.snapshots` (`id`, `name`, `devices`, `vcas`, `matches_mix` and `skips` for what a recall would leave out because it was deleted) and a compare in progress under `document.console.compare` (`snapshot`, `side`: `B` while the snapshot is live, `A` for the working mix). To check them:

1. `session_open` the project, `session_bounce` into `target/mcp/reference.wav`, then `session_command` `mixer.store_snapshot` (the snapshot is named `A`; an `arg` names it).
2. `session_set_param` a fader (`device` `channel`, `param` `fader`), a mute or a send, and `session_bounce` with `reference`: `bit_identical` is false.
3. `session_command` `mixer.recall_snapshot` with `arg` `A` (a name or an id): the bounce is bit for bit the reference again, and one `edit.undo` brings the edited mix back. `arg` `A:levels` (or `mutes`, `sends`, `inserts`) recalls only that category.
4. `mixer.compare_snapshot` starts an A/B (`side` `B` is the snapshot), the same command flips it, `mixer.end_compare` keeps the live side; bounce after each to hear both. `mixer.update_snapshot`, `mixer.delete_snapshot` and `mixer.recall_next_snapshot` act on the current snapshot, or on the `arg`.

**MIDI learn, mappings and control surfaces**

`project_inspect` and `session_status` list the mappings under `midi_maps` (`id`, `source`, `target`, `mode`, `min`, `max` and, when they differ from the defaults, `invert`, `pickup`, `consume`), and `session_status` has a `midi` block (`learn_mode`, `armed`, the last message, the number of mappings, the surface). The Mixer menu's items are `session_command` ids: `midi.learn` (a toggle), `midi.mappings` (opens the panel), `midi.export_preset`, `midi.import_preset` (their dialogs are answered by `session_midi_learn`) and `midi.clear_maps`.

1. `session_open` the project and `session_bounce` into `target/mcp/reference.wav`.
2. `session_midi_learn` with `action: arm` and `target: channel fader`, then `session_midi_input` with `messages: B0 4A 40`: the first control moved becomes the source (a controller below 64 waits a tenth of a second for a possible 14 bit pair, so follow it with `wait: 0.5`). `session_status` shows the mapping, and the bounce with `reference` is `bit_identical: true`: a mapping is not sound.
3. Move it: `session_midi_input` with `B0 4A 00; B0 4A 7F`. Soft takeover is on, so the fader waits until the control reaches it (`document_changed` false until then); `session_midi_learn` `edit` with `pickup: false` makes it follow at once, `min`, `max` and `invert` shape the range, `mode: relative_twos` (or `relative_offset`, `relative_sign`, `toggle`, `momentary`) changes how the control drives it, and every edit and a whole sweep is one `edit.undo`.
4. `export` writes a preset by device type and position, `clear` removes everything, `import` applies the file to this project's own devices (the answer names what could not be placed).
5. A control surface: `session_surface` `attach`, then `send` with `E0 7F 7F` (the first fader to the top: the console's fader follows and nothing is echoed), `90 10 7F` (the mute key: the LED comes back), `90 2F 7F` (bank right: the scribble strips and rings are sent again) or a SysEx; `frame` after a `session_set_param` shows the motor following. The bytes of every message are documented in `crates/midi-io/src/surface/mcu.rs`.

**Macros and the parameter search**

`project_inspect` and `session_status` list the macro banks under `macros` (`banks`: `bank`, `name`, and `macros`: `macro`, `name`, `value` and `assigns` with `id`, `device`, `param`, `min`, `max` and, when they apply, `invert` and `live: false`; `orphans` counts assignments whose bank is gone).

1. `session_open` the project and `session_bounce` into `target/mcp/reference.wav`.
2. `session_macros` with `action: assign`, `device: lead`, `param: filter.cutoff`, `macro: 1`, `min: 300`, `max: 3000` makes a bank and puts the cutoff on its first macro (the first assignment moves the macro so the cutoff keeps its value: the bounce is still the reference). `session_macros` `set` with `macro: 1`, `value: 0.2` changes the sound; `session_set_param` on `lead` `filter.cutoff` is refused, saying which macro drives it.
3. `session_param_search` with `query: lead cutoff` shows the row (`macro: M1`); `action: adjust` on a free param, `assign` for the next free macro, `learn` to arm a controller (then `session_midi_input`), `lane` and `reveal`.
4. `session_macros` `follow` with `on: true`, then `lane` on a param of an instrument adds the empty lane (a track for a device that has none) and writes no data; `lane` on a delay's `mix` or a channel's `fader` adds the empty lane on an automation track of that device (`document.song.tracks[].automation_only`), and once `mix` is assigned to a macro it shows the macro's lane; `lane` on an internal param says it has no lane.
5. `session_save`, then `param_search` on the file finds the same rows without a session, and `project_inspect` lists the banks; a project of the previous format loads with none.

**The library**

A library is a folder with `Patches/`, `Containers/`, `Clips/` and `Projects/` ([ARCHITECTURE.md](ARCHITECTURE.md#6l-the-library)). `library_list` reads one without a session. In a session:

1. `session_open` the project with `library: target/mcp/library`.
2. `session_library` with `action: save_selection` and `devices: ["lead", "echo"]` writes `Patches/<name>.atomikpatch` and returns its relative path; `library_list` on the folder shows it with its one line `detail`, `session_status` shows `dirty` unchanged and `can_undo` as before. `project_inspect` and `validate` read the item file like a project (`kind` `patch_item`).
3. `session_library` with `action: load` and that `path` adds the devices again after the selected one (or at `slot: "1:0"`): `session_status` lists them with fresh keys, `can_undo` is true and one `edit.undo` removes all of them. A built-in device loads with `path: atomik-device:atomik.arpeggiator`.
4. A sampler: its audio is in `Patches/<name>/Audio/`; delete that folder and load again: the device loads, `missing_audio` names the sample and `session_relink` can fix it, as for a project.
5. `save_clip` with `clip: <number>` writes `Clips/<track> bar <bar>.atomikclip`; `load` it with `track`, `lane` and `tick` and read the clip count in `session_status`. With no `track` and no instrument track selected the answer is the error `Select an instrument track to load a clip`.

**Reproducing a bug report**

`validate` and `project_inspect` the file first (does it load, what is in it), then `session_open` it and follow the report. A bug that needs the file to be edited is reproduced by a `session_save` of the state that triggers it; attach that project to the issue.

**Checking an example or a new example project**

`validate`, then `bounce` and read the report: the duration is the song end plus two beats, the peak is in range, nothing is silent that should play, and the same file bounced twice is identical.

**A pull request**

Say in "How it was tested" which of these you ran and what they showed (for example "bounce of `demo.atomik` bit-identical before and after, 4 blocks and 2 worker counts"). The checklist items for tests and gates are unchanged.

### Recipe: export as an audio file

1. `session_open` the project and `session_bounce` into `target/mcp/reference.wav`.
2. `session_export` with `file` `target/mcp/song.wav`, `format` `pcm16`: `frames` is the End marker's frame (plus `tail_frames` of audible tail), and `compare_audio` of the two files (the export is the bounce for its first `span_frames`) differs by the quantisation only (at most 1.5 LSB, 2^-15 each, with the dither); `format` `f32` with `tail: false` is `bit_identical` over those frames.
3. For a loop, set the locators first (drag them in the ruler with `session_pointer`), then `session_export` with `span` `loop`: `frames` is exactly the frames between the locators and `tail_frames` is 0. With `sample_rate` `44100` the length is `round(frames * 44100 / 48000)`.
4. `session_command` `file.export_song_audio` only raises the native file dialog, reported as `pick_export_audio`: use `session_export` to write the file.

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
