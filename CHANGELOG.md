# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, any release
may break APIs and the patch and project formats.

## [Unreleased]

### Added
- **Device patches.** Every device panel (except the console devices) has a patch strip on the foot
  of its name plate: **previous** and **next** arrows, the **patch name** (click it to browse
  patches, below) and a **save** button. A patch of one device (`.atomikpatch`) now applies to a
  device that is already in the rack: drop it on the strip (the device takes the params, samples,
  patterns, curves, drum patterns, plugin state or container inside as one undo step, and keeps its
  name, place, cables and track; a patch for another type or of several devices changes nothing and
  says so), or step through the patches of the device's type with the arrows (natural name order,
  wrapping, each step one undo step). **Save** asks for a name with the system's save dialog (it
  confirms an overwrite), writes the device as a one-device patch with its samples, and shows the
  name on the strip. The device context menu has **Copy Settings**, **Paste Settings** (onto a
  device of the same type), **Reset Device** (the sound of a new device) and **Save Patch**; they
  are the `edit.copy_settings`, `edit.paste_settings`, `rack.reset_device` and `rack.save_patch`
  commands (no keys). A device that took a patch bounces bit for bit like one created from it.
  **Project format 36**: a device may carry `"patch"` after its `"name"` (the name of the patch it
  last took its sound from, display only), written only when set; a project without any differs from
  its version 35 file in the version line alone. See
  [docs/DECISIONS.md](docs/DECISIONS.md#d-device-patch-format).
- **Patch browsing.** **Browse** on a device's patch strip (click the patch name, the device context
  menu or **Ctrl+Shift+B**, `rack.browse_patch`) aims the file browser at that device: an orange
  band names it, the list shows only folders and the patches for its type, and the patch name on the
  plate turns orange. Selecting a patch (a click, or the arrow keys in the list), **Next Patch** and
  **Previous Patch** (**Ctrl+Right**, **Ctrl+Left** in the rack, also in the context menu) and the
  patches picked from the pop-up that a right-click on the patch name opens (the patches of its
  folder, natural order) are heard at once but make **no undo entry**, and the device's engine node
  is not rebuilt, so held notes go on sounding with the new patch. **Return** (or a double-click on
  a patch, or pressing another device's Browse, or any other edit) accepts: the whole browse is
  **one undo step**. **Escape** puts the device back exactly as it was. The folder last browsed for
  each device type is remembered in `ui-state.json` (`patch_folders`), and the patches of a folder
  are read once, not at every step. The commands are `rack.browse_patch`, `rack.patch_next`,
  `rack.patch_prev` and `rack.patch_pick`; `docs/MCP.md` has a recipe to check it with `session_key`
  and `session_pointer`. See [docs/DECISIONS.md](docs/DECISIONS.md#d-device-patch-format).
- **Audio clip editor.** Double-clicking an audio clip, **Enter** on a selected one or **F8** opens
  the clip in an editor that replaces the arrangement (Esc closes it; Up and Down open the previous
  or next audio clip and keep the mode; double-click the title to rename the audio file, one undo
  step). **Overview** shows the clip with its warp markers. **Slice** edits them: double-click or
  **I** adds a marker pinned where the audio already plays (no sound change), dragging a marker
  stretches the audio around it, the alternate-tool modifier (or the **Slide** toggle) slides it
  along the audio, the constrain modifier (**Shift**) carries the markers after it along, the nudge
  keys move the selected marker, **Delete** removes it, **Revert** removes every marker, **Split**
  cuts the clip at each marker (one undo step) and **To slicer** makes a Loop Slicer that slices the
  clip at its markers (one undo step). Entering Slice mode on a clip without warp switches warp on
  at the song tempo. Keys: **1** Overview, **2** Slice (the mode is remembered).
- **Transient markers and the speaker tool.** In the audio clip editor's Slice mode the transients
  of the clip (found on a worker thread, kept in memory per file and sensitivity) are drawn under
  the ruler; the **SENS** buttons change how many are found, a double-click or **I** near one puts
  the marker exactly on it (no sound change), and **Add all** puts a marker on every transient in
  one undo step. The **Speaker** tool (**T**, the SPK button next to Razor in the arrangement, and
  the SPEAKER toggle in the audio clip editor) plays the audio under the pointer exactly as it is in
  its file, unprocessed (no warp, fades or envelope; a reversed clip plays backwards) for as long as
  the button is held, at the clip gain. It uses the preview voice, so it is heard live and never
  reaches a bounce or export. No project format change.
- **Warp on import.** Settings, Workflow, **Warp on import** (Off, **Loops only** by default, All
  files; `"warp_imports"` in `workflow.json`): an imported audio file that carries a tempo, or whose
  length is a whole number of bars (1, 2, 4 or 8) at a round tempo between 70 and 180 BPM, becomes a
  warped clip at its own tempo, as long as the bars it holds, so a loop follows the song tempo from
  the start; a one-shot, a file flagged as a one-shot and a long recording are left as they are. All
  files warps every file (one nothing is known about plays unstretched at the song tempo). Off
  imports exactly as before. The warp joins the undo step that adds the clip. The tempo is read from
  the `acid` chunk of a WAV file (tempo, beats, one-shot flag), with the `smpl` chunk and a tempo
  written in the `bext` description as further hints, by a bounded chunk scanner that never reads
  more than a few hundred bytes of a chunk and does not fail on a damaged file. See
  [docs/DECISIONS.md](docs/DECISIONS.md#m-warp-on-import-default).
- **Warp panel: Scale and Reset.** **Scale** (25 to 400 percent) changes the playback speed of the
  warped clip: APPLY divides the source tempo by it and the panel reads 100 again. **RESET** (Reset
  to Unstretched Tempo) turns warp off and clears the markers of the selected clips, and resizes a
  warped clip so it plays the same stretch of audio at its own speed; one undo step.
- **The Atomik Rack logo is the application icon.** The main window and the detached Rack and Mixer
  windows show it in the title bar, the taskbar and Alt-Tab (Windows and X11; macOS shows the Dock
  icon of the bundle), `atomik.exe` carries it so Explorer, the Start menu and a pinned taskbar
  button show it, the Windows installer and uninstaller use it, `Atomik Rack.app` has an `.icns`
  icon, and the `.deb` installs it in the hicolor theme and names it in the desktop entry. The
  README shows the logo. The master and the derived `.png`, `.ico` and `.icns` files are in
  `assets/icon/`; the `make_icons` example
  (`cargo run --release -p atomik-app --example make_icons`) regenerates them. A Windows-only build
  dependency, `embed-resource`, embeds the icon in the executable. See
  [docs/RELEASING.md](docs/RELEASING.md#the-application-icon).
- **Save, Don't Save or Cancel.** Closing, quitting, opening or creating a song with unsaved changes
  now asks **Yes** (save, then carry on), **No** (discard) or **Cancel**, instead of "Discard them?"
  with no way to save from the question. A song with no file yet asks where to save it first; a
  cancelled Save As or a save that fails stops the action and the song stays open.
- **File > Close (`Ctrl+W`) and File > Open Recent.** Close ends the song and starts an empty one,
  after the same question. **Open Recent** lists the last eight songs you opened or saved, newest
  first, without duplicates; a song that is gone leaves the list with a message; **Clear Menu**
  empties it. The list is kept in `recent.json` next to `settings.json`. The commands are
  `file.close`, `file.open_recent` (the place in the list as the argument) and `file.clear_recent`.
- **Layer recording.** Settings, Workflow, **Recording over clips**: **Replace (takes)** is what
  recording always did (a new clip, or the passes of a loop stacked as takes); **Layer** merges the
  take into the note clip under its start (the old notes stay, a new note replaces one with the same
  start and key, a controller you moved replaces the old values over the take), grows the clip when
  the take runs past its end, and puts every pass of a loop recording into one clip. A Layer take
  that starts before an existing clip plays over it instead of being hidden by it (an older clip
  that starts inside the take is trimmed to start where the take ends, or removed when it ends
  inside it). One undo step takes the whole take away.
- **Audio recorded over a clip becomes a take.** Recording audio where a clip already is no longer
  stacks two plain clips (the later-starting one hid the other): the old clip and the new recording
  become the takes of one take group, the new take playing over the overlap and the old one
  everywhere else, in one undo step; a further recording over the group adds another take row.
- **Keep events in clip.** Settings, Workflow, **Notes outside a clip**: a note drawn, moved or
  pasted past the end of its clip is silent; **Keep** leaves it (as before), **Expand clip**
  lengthens the clip to the next bar line, **New clip** moves the notes to a new clip on the bar. It
  is part of the same undo step as the edit (a drag is completed when you let go).
- **Note tails.** A new project lets a note ring to its own end past the end of its clip, the loop
  end or the start of the next clip (the keys are released at the loop wrap and at stop). **Project
  format 35**: the song may carry `"tails": true`; a project saved before it has no flag and cuts
  every note at the end of its clip, loop pass or covering clip exactly as before, so it plays and
  bounces bit for bit as it did.
- **Open last project at start-up.** A Workflow switch (off by default): the app starts with the
  song that was open last time, unless a project is named on the command line.
- **Autosave for every project.** A changed project is copied to the recovery folder every few
  minutes (Settings, Workflow, **Autosave**: off, 1, 2, 5 by default, 10, 15 or 30 minutes), with or
  without plugins, by a background thread, so a frame never waits for it. After Atomik ends
  unexpectedly the next start shows a banner with OPEN RECOVERY. Unfinished audio recordings left by
  the crashed run come back as a `Recovered take` track in the project; the recording file is
  deleted only after you save.
- **A pre-count for recording.** Settings, Audio, **Pre-count (bars)** (Off, 1, 2, 3 or 4 bars;
  **Options > Pre-count**, `Ctrl+P` in the standard key set, switches it on or off and remembers the
  length): when a recording starts from a stopped transport the click counts that many bars of the
  meter at the playhead, at the tempo there, before anything plays. The first beat of each bar is
  accented, the click sounds even with the metronome off, the transport bar shows **COUNT** and the
  beat of the bar (and **PRE n** while it is on), and nothing played during the count is recorded:
  the take and its clip begin at the position the count started from, bar 1 when you start there. A
  loop is counted in once, not at every lap; **Stop** cancels the count. With the pre-count off (the
  default) recording behaves exactly as before. Saved in `settings.json` as `pre_count_bars`.
- **An Options menu.** The playback and recording settings in one place: **Click**, **Pre-count**,
  **Input Quantize**, **Chase Notes** (the selected track's note chase), **Delay Compensation** and
  **Settings...**. Every entry is the command it already was, so the Transport, Mixer and Audio
  menus keep theirs.
- **An Inspector for every kind of clip.** View > **Inspector** (off by default, remembered) shows a
  strip under the arrangement's lanes with the selected clips' **Position**, **Length**, loop
  **Offset** and **Loop length** as segmented `bar.beat.16th.tick` fields that follow the song's
  meter. Press a part and drag, scroll over it (Shift: the next finer unit), or double-click to
  type: digits replace the part, `+240` and `-240` add or subtract ticks, `3.2.1.0` sets the whole
  field, Up and Down step while typing. With several clips selected a field shows a dash where they
  differ and an edit is relative: every clip moves by the change of the first clip's value, each
  clamped to its own range. **Match** gives every selected clip the first clip's length, offset and
  loop. One typed value, wheel notch or Match is one undo step; a drag is one. Audio clips have no
  offset or loop and keep those fields dimmed. The same switch shows a **note strip** under the
  piano roll: **Note** (the pitch as a name like `C#3`: drag, scroll, or type a name, a number or
  `+n`/`-n` semitones), **Position**, **Length** and **Velocity** (1 to 127) of the selected notes,
  relative on several notes and clamped note by note. While automation points are selected the
  arrangement strip shows their **Position** and **Value** (in the parameter's unit) instead,
  relative and clamped to the lane's range; a move that would put two points on one tick is not
  sent.

- **Piano Keys.** View > **Piano Keys** (`F4`) shows a strip above the status bar with two octaves
  and a note, and plays with the mouse or the computer keyboard. A press plays the key with a
  velocity that follows the height of the click and a drag glides across the keys; **Hold** latches
  keys, **Sustain** keeps released keys sounding, **VAR** adds a small random change to each typed
  velocity, and the octave runs over ten octaves (`C-1` to `C8`). In **KEYS** mode (the default) the
  strip, while it has the focus, turns the keyboard into notes:
  `A W S E D F T G Y H U J K O L P ; '` are the keys from the strip's lowest C, `Z` and `X` move the
  octave, `1` to `0` set the velocity (1, 14, 28, 42, 56, 70, 84, 98, 112, 127; 98 on start) and
  `Shift` is the sustain pedal. The characters are the `keys.note`, `keys.octave_up`,
  `keys.octave_down` and `keys.velocity` commands of a new `keys` context, so Settings, Keyboard
  changes them; everything else keeps its command, and **MOUSE** mode leaves every letter a command.
  The notes reach the armed track (or the selected device) on the same live path as a MIDI keyboard,
  so a take records them, and no note stays on when the strip loses the focus, changes mode or is
  closed. Live input only: bounces and the project format are unchanged.
- **Stereo patching and a jack menu.** Dragging a cable from a left jack (`out_l`, `in_l`) to a left
  jack of an audio socket now cables the right pair too when it exists and is free, in one undo step
  (the same for right to right; **Alt** at the release makes it one cable; mono jacks, CV and notes
  sockets and mismatched sides still make one). A **right click on a jack** on the back of the rack
  opens a menu: **Connect to** lists, per device in rack order, the jacks the cable rules allow (an
  input that is taken is starred and choosing it replaces its cable, one undo step), **Disconnect**,
  **Scroll to Connected Device** (across columns too) and, on an audio output, **New Mixer
  Channel**; entries that cannot be chosen are greyed. Hovering a jack names the jack at the other
  end of its cable ("In L from Poly Synth > Out L"). The menu's entries run the new `rack.jack`
  command (Patch Jack, no key).
- **Performance controllers in the melodic instruments.** The poly synth, wavetable synth, FM synth,
  grain sampler and sampler now follow the **sustain pedal** (CC 64: a released note keeps sounding
  until the pedal goes up, then releases at that moment), **volume** and **expression** (CC 7 and CC
  11, squared and smoothed so they never click), **channel and key pressure** (kept as a modulation
  source for the matrix to come) and the **pitch-bend range**: a new `bend.range` param (0 to 24
  semitones, default 2) that the registered parameter 0 (CC 101, 100, 6, 38) also sets. The sampler
  now plays pitch bend as well. Everything else sounds exactly as before. `docs/DEVICES.md` gains a
  MIDI chart of what each instrument does with each message, and a test checks it by rendering.
- **A log file, crash reports and Help > Export Diagnostics.** Every session writes `atomik.log` in
  the `logs` folder of the config folder (rotated at 2 MiB, three files kept), with what the app and
  its libraries have to say; the messages that used to go to a terminal nobody sees are in it. A
  panic writes `crash-<time>.txt` next to it (message, thread, backtrace, version, OS and the audio
  and MIDI devices). The new **Help** menu has **Export Diagnostics...**: one text file with the
  settings, the plugin cache and journal, every counter, the last crash report and the end of the
  log, with your home folder written as `~`. `ATOMIK_LOG=debug` raises the log level. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6n-logs-crash-reports-and-diagnostics).
- **The status bar shows dropped work.** When they are not zero it adds **dropped events** (note
  events that found a full buffer), **record overflow** (frames the audio recorder lost), **plugin
  drops** and **late sandbox batches**, and `session_status` reports them under `overload`. The same
  counters are written to the log when they grow.
- **Delete, mute and hold an automation lane.** An automation lane header now has an **M** (mute)
  and an **X** (delete) button, and the track header menu has **Delete Automation Lane**, **Mute
  Automation Lane** (checked while muted), **Set Static Value** and **Clear Static Value** for the
  lane whose header you pressed last. Deleting removes the lane with its clips, write mode and fold
  in one undo step, and a track that held automation lanes alone goes with its last lane. A muted
  lane drives nothing, and the parameter keeps the value the lane gave at the playhead (the mute and
  that value are one undo step). A control's right-click menu has **Clear Automation** (the lane of
  that control) and **Set Static Automation Value**.
- **A static value for a lane.** A lane with a static value gives it before its first clip, between
  its clips and after the last one, and for the whole song when it has no clip, so a one-bar clip no
  longer fixes the parameter for the rest of the song. **Project format 36**: an automation lane may
  carry `"muted": true` and `"static": value` after its `"folded"`, written only when set; a project
  without them differs from its version 35 file in the version line alone and bounces bit for bit as
  before.
- **A lane picker with check marks, on automation tracks too.** The **+ AUTO** menu lists every
  parameter of the track's device, checks the ones that have a lane and removes the lane of a
  checked one when you click it; an automation track has the picker (a small **+**) next to its
  first lane's controls.
- **Auto-create lanes.** A switch next to Record in the transport bar (**Transport > Auto-create
  Lanes**, `transport.auto_create_lanes`, session state) makes a take also record the parameters
  that have no lane yet: move a knob while it runs and its lane is added when the take ends, with
  the recorded clip, in the same undo step. A parameter that already has a lane records only when
  that lane is armed; a parameter a macro drives, an internal one and an audio track's get no lane
  (the macro one says why).
- **Override.** Moving a control by hand while the song plays and no take runs used to be undone by
  the lane at the next audio batch. The lane now holds the value you set until you press the
  **OVERRIDE** lamp that appears in the transport bar (`transport.clear_override`), stop or locate;
  at most 32 lanes can be held at once, and the document and a bounce are untouched.
- **Alt-click a control to edit its automation.** A click on a knob, fader or switch with the **Edit
  automation** role held (Alt, Option on macOS, remappable under Settings, Keyboard) shows its
  automation lane, adding it when it is missing, like the control menu's **Edit Automation**.
- **Trim the start of any clip, and resize several clips at once.** Dragging the left edge of a note
  or automation clip moves its start and keeps its end (the content stays where it was in song time;
  growing a clip left past its first note shifts the notes, controller points or automation points
  right, a looped clip wraps its loop instead). With several clips selected the edge moves every one
  of them, and so does the right edge. A drag is one undo step. **Delete Events Outside Clips**
  (`sequencer.delete_outside_events`) removes the notes that start outside their clip's window and
  the automation and controller points outside it, keeping one point on each side so the curve at
  the edges is unchanged.
- **Split and cut several clips in one undo step.** **Split at Playhead**, the razor and a razor
  click in the ruler now split every clip they cross as a single step (a split over twelve clips was
  twelve undo steps). Dragging the razor along a lane cuts the range out of the clips there; with
  the copy modifier held the clips stay and a copy of the range is placed right after it.
- **Nudge clips.** `Ctrl+Left` and `Ctrl+Right` (`Cmd` on macOS) move the selected clips by one snap
  step, `Ctrl+Shift` by a beat and `Ctrl+Alt` by a tick, as one undo step; the group stops at
  tick 0.
- **Relative snap and more snap values.** `sequencer.set_snap_mode` (`absolute` or `relative`)
  chooses whether a moved clip or note lands on a grid line or moves by whole steps and keeps its
  distance from the grid (the arrangement corner reads **SNAP REL** while it is relative). The snap
  selector gains **1/2**, **1/128** and **1/2T** (`half`, `onetwentyeighth`, `half_triplet` for
  `sequencer.set_snap`).
- **Constrain and copy-drag work on notes and clips.** Holding **Shift** (the Constrain role, now
  listed in Settings, Keyboard) while dragging keeps a clip's start, so it only changes lanes, and
  moves a note along one axis; the modifier is read on every move, so it can be pressed after the
  mouse-down. **Ctrl** (the Copy role) drags a copy of the selected notes in the piano roll, as it
  does for clips, in one undo step.
- **More daily keys in the standard key set.** `X` toggles the crossfade of the selected audio clips
  (equal power, or off when it is on), `Alt+M` fits the selected tracks to the window, `Ctrl+E`
  toggles the clip editor in the sequencer, `Shift+Z` zooms to the selection like `Z`, `Ctrl+Alt+=`
  and `Ctrl+Alt+-` zoom the time axis like `Ctrl+=` and `Ctrl+-`, and `F` toggles Follow in every
  context. `Escape` in the arrangement closes the open clip or editor (when no menu or drag is open;
  otherwise it clears the selection as before). `Alt+Shift+Left` and `Alt+Shift+Right` move the loop
  by one bar, `Alt+Shift+Up` and `Alt+Shift+Down` change its length by a bar (`transport.loop_move`,
  one undo step each, never before bar 1 and never under one bar). `!` is **All Notes Off**
  (`transport.all_notes_off`): it releases every note on every device and on the Piano Keys. The
  legacy key set is unchanged.
- **Shift+click in the arrangement ruler sets the End marker** at the snapped position.
- **A newer project can open in an older build when it only adds optional fields.** A project file
  whose root says `"compat"` (the oldest version that can read it) and lists its `"ignorable"`
  fields opens with those fields skipped and a warning each; any other unknown field is still
  refused. The writer adds the two fields only when every change since `compat` is skippable, so
  files of format 36 are unchanged byte for byte.
- **"N items ignored while opening".** Opening a project reports what the file had that the
  document does not, in a toast and the log: the height of a device that has no track, a comp clip
  that is not on its lane, an instrument of a version 1 file that could not get a mixer channel and
  the skipped fields of a newer file.

### Fixed
- **A command that does not fit the document is refused, not fatal.** Every edit now checks what it
  names before it changes anything, and a step that does not fit leaves the document and its undo
  history as they were with an error, where it used to abort the program with unsaved work. The app
  also catches a panic inside a command, an undo or a redo: it says "Internal error, document
  restored" and writes a recovery snapshot.
- **Pasting or duplicating a track past the song End moves the End out.** Only a single clip did; a
  pasted or duplicated track left the End short and a bounce stopped early. A MIDI import follows
  the same rule instead of carrying its own copy.
- **Quantize at the last tick keeps every note inside the song.** Two notes pulled onto the same
  last tick used to push one past the end of the timeline, a note no edit would accept and no file
  could hold. The quantize now moves it to the nearest free tick before, and refuses a result a clip
  edit would refuse.
- **A project that holds a number that is not finite is refused before it is written.** Save checks
  the document first and says what is wrong, rather than writing a file that cannot be read back.
- **No config folder no longer means no recovery.** When the environment has no config folder (no
  `APPDATA` or `HOME`), the plugin cache, the plugin journal, recovery snapshots and logs now use a
  folder in the temp directory for the run, and a toast says settings are not saved.
- **A bug in one device no longer takes the app down.** A device that panics while rendering is
  silenced and skipped, everything else keeps sounding, the status line shows
  `ENGINE FAULTED: <device>` and a toast points at Audio > Restart Audio. Stopping the audio no
  longer aborts the app when a slow driver does not answer within five seconds (it reports an
  error), and a computer that refuses to start the extra render threads falls back to fewer. A
  bounce or export that hits a rejected edit or a failing device now fails with a message (and a
  non-zero exit code for `atomik-render`) instead of aborting, and never writes silence into the
  file. The release profile now says `panic = "unwind"` explicitly, with a test that no profile
  turns it off.
- **`settings.json` and `ui-state.json` can no longer be left truncated.** They are written to a
  temporary file and renamed. A file that cannot be read is kept as `.bad` and Atomik starts with
  the defaults and says so.

### Changed
- **A clip made by a click is one snap step long.** A double-click on an empty lane and a pencil
  click create a clip as long as the snap step (a bar with snap off or at Bar) where they made a
  bar. The Constrain role is wired (arrangement and piano roll) and listed in Settings, Keyboard.
- **Modifier roles that nothing read are no longer listed, and the ones that have a gesture now
  follow their remap.** Settings, Keyboard lists only the roles a gesture acts on (a remap of the
  others stays in `keymap.json`). **Fine adjust** now decides which modifier makes a knob or fader
  drag, wheel notch or arrow key a tenth as far in the rack and the mixer, **Reset parameter** which
  click resets a knob or fader there, and **Copy while dragging** which modifier makes an
  arrangement clip drag drop a copy (on macOS the default is now `Opt`, as the role table says,
  instead of `Cmd`). Constrain is not listed until a gesture uses it. The Modifier roles section of
  `docs/KEYBOARD.md` says which role does what and that AltGr on many Windows layouts is `Ctrl+Alt`,
  the default of Disable snap.
- **A cable picked up from an input is cut when you let go, not when you press.** Pressing a
  connected input used to disconnect it at once, so there was no way back; now the cable is drawn as
  the one you hold and the project changes only at the release: dropped on another input it moves
  (one undo step), dropped on nothing it is cut, dropped on its own input or cancelled with **Esc**
  it stays untouched.
- **The File Browser moves to `F9`** in the standard key set (View, File Browser), freeing `F4` for
  the Piano Keys; the Atomik legacy set keeps `F4` for the file browser and puts the Piano Keys on
  `F9`. On macOS the alternatives are `Cmd+Opt+4` (Piano Keys) and `Cmd+Opt+9` (File Browser). The
  one-time notice of the moved keys names both.
- **`session_command` and `session_status` follow the new question.** The unsaved-changes request of
  `session_command` is now `confirm_save` (was `confirm_discard`; `confirm_save_open` for a project
  picked in the library); `confirm: true` answers Don't Save. `session_status` lists `recent_files`.
- **Moving the pointer over the app costs next to nothing.** The document carries a revision number
  that changes with every edit, undo, redo and data sync; the unsaved-changes mark (` *` in the
  title) is compared once per change instead of once per input event, the sequencer and the piano
  roll read the document again only when it changed, and a pointer move that changes nothing no
  longer repaints any window. A hover that changes what is shown, or a drag, still repaints, and
  only the window it happens in. Large songs no longer slow the pointer down.
- **The ruler's locator clicks follow the platform.** On Windows and Linux `Ctrl+click` sets the
  left locator and `Alt+click` the right one; on macOS `Opt+click` still sets the left and
  `Cmd+click` the right.
- **Keys moved in the standard key set.** Group Tracks is `Ctrl+Shift+T` (was `Ctrl+Shift+G`), Move
  Track Up and Down are `Ctrl+Alt+Up` and `Ctrl+Alt+Down` (were `Alt+Up` and `Alt+Down`, which the
  file browser's parent folder keeps), and Reset Track Heights is `Alt+Z` (was `Shift+Z`). The
  one-time notice of moved keys names Group Tracks too. The legacy key set keeps its keys.
- **The project reader is driven by one field table**, and the module is split by area
  (`crates/model/src/project`): adding a field is one table row. A saved project of every released
  format version from 13 on is kept in `crates/model/tests/fixtures` and checked to load and resave
  canonically. Load, batch and missing-asset failures have their own error kinds (`Load`,
  `BadBatch`, `MissingAsset`), so a toast no longer calls a project error a patch file error. Saved
  files are byte for byte what they were.
- **Documents match the code again.** The architecture, testing, devices and extending guides and
  the security policy are corrected (the format version, the crate table with a new `unsafe`
  column, the VST3 editor and sandbox statements, the golden files, the allocation guard coverage,
  the plugin scope of the security policy). Tests check the crate table against the `Cargo.toml`
  files, that every function the guides cite exists, and the command count the architecture guide
  quotes. The long documents are hard-wrapped at 100 columns, and the continuous integration job
  checks the wrapping and the links of the Markdown files.

## [0.2.0-rc.2] - 2026-10-28

### Added
- **Automate a mixer channel, the master or an effect.** Any parameter that can have a lane now can:
  right-click a knob or fader of a console strip, the master, an insert or send effect or any effect
  in the rack, in the mixer or in the rack, choose **Edit Automation**, and the lane is added on an
  **automation track**, in one undo step, and shown in the arrangement (the old "automate it through
  a macro" workaround is not needed; the macro route still works). An automation track is the usual
  DAW track of a device that plays no notes: it belongs to the channel or effect, shows its name and
  colour, and holds automation lanes alone (no note lane, no audio lane). The first lane shares the
  track's header row, which carries the name, the expand chevron, a mute button and that lane's
  write-mode and arm buttons; the other lanes fold and collapse as on any track. It can be renamed,
  coloured, moved, put in a folder, copied with its device and deleted like any track. **Mute stops
  its automation** (the param keeps its own value); **solo** has no meaning for it: it has no solo
  or arm button, a folder's solo and arm leave it alone, and soloing another track does not silence
  it. It cannot be frozen. Automation recording (the `W` button, Touch, Latch, Overwrite) writes
  into these lanes like any lane, the strips, the master and the rack controls follow them while the
  song plays, and a bounce plays them exactly as it plays an instrument's lanes (bit-identical
  across block sizes). The master section now draws the automation mark on its controls, as the
  channel strips do. A device inside a container, an audio track and the internal parameters still
  have no lane. **Project format 34**: a track may sit on a device without a Notes input and then
  holds `"automation"` lanes only; every older file loads unchanged, and a project without such a
  track differs from its version 33 file in the version line alone.
- **The loop is between a left and a right locator flag.** The loop bar in the arrangement ruler is
  gone: an **L** flag marks where the loop starts and an **R** flag where it ends, drawn like the
  song's **E** flag, with a tint between them in the ruler (faint while the loop is off, clearly
  stronger while it is on; the lanes keep their loop shading). Drag a flag to move it (it snaps, the
  disable-snap keys free it, Escape cancels, and one drag is one undo step; the left flag cannot
  pass the right one), choose **Transport > Set Left Locator at Playhead** or **Set Right Locator at
  Playhead**, or right-click the ruler and choose **Set Left Locator Here** or **Set Right Locator
  Here**. The commands have no default key and can be mapped in Settings > Keyboard. Flags that meet
  stay readable: R hangs to the left of its line, L and E to the right, and a flag that would cover
  another moves down one row; where R and E share a tick, a press on the shared line takes R. A
  press in the lower band of the ruler now locates the playhead like the bar numbers (it no longer
  toggles the loop or draws a new loop; the loop button, **Loop to Selection**, a click on a range
  marker and the flags set it). `Alt+click` and `Ctrl+click` (`Cmd`) in the ruler still set the left
  and right locator.
- **Edit Automation on a control.** A right click on a knob, fader or switch of an instrument in the
  rack (or of the console or the parameter pane) now offers **Edit Automation** at the top of its
  menu: it adds the automation lane of that parameter to the device's track (the track too, when the
  device has none), unfolds the track's lanes and the folders that hide it, brings the arrangement
  into view and outlines the lane, ready to draw in. When the lane is already there it only shows
  it. Adding is one undo step, and an empty lane holds no automation until you draw. A parameter a
  macro drives opens the macro's lane on the bank's track. Channel strips, the master and effects
  get an automation track of their own (see the entry above). View > Show Automation Lane now also
  brings the arrangement into view. New command `automation.edit_control` (menu only, no default
  key).
- **A dragged note ends where you let go.** Drawing a note in the piano roll (the Pencil, or a
  double-click with Select) and dragging now ends the note at the pointer, snapped to the current
  snap (the disable-snap key frees it), and the note follows the pointer while you drag; before, the
  drag was added to the default length. Dragging left of the start keeps the minimum length (one
  snap step), and a click without a drag still draws a note of the default length. A chord stamp is
  stretched the same way.
- **Stepping through notes plays them.** In the piano roll, Left and Right (and Shift+Left and
  Shift+Right) now sound the note they select, on the track's instrument, for a quarter of a second;
  the next step, a click, closing the roll or opening another clip ends it at once, so nothing
  hangs. Shift+Right and Shift+Left now move the end of the selection like a text selection: with
  the selection extended to the right, Shift+Left gives the last note back (and once only the
  starting note is left, extends to the left past it), and the other way round. A selection made
  another way (a rubber band) starts from its earliest note, so Shift+Left first drops its latest
  one. A shrink sounds the note the selection now ends on.
- **Guitar Amp.** A new effect, `atomik.guitar-amp` ("Guitar Amp" in the browser): a guitar
  amplifier with a cabinet, chorus, delay and reverb, and five **setups**. The signal runs through a
  noise gate (`gate`), a tight high-pass and a mid boost (`tight`, `boost`), a three-stage preamp
  (`gain`) with a passive-style tone stack (`bass`, `mid`, `treble`, which affect each other) and a
  power amp (`master`, `sag`, `presence`, `depth`), then a cabinet synthesised from designed filters
  (1x12 open, 2x12, 4x12 closed or none, with a `mic` position from the centre of the cone to its
  edge; no recorded impulse response), a chorus, a ping-pong delay (in ms or as a note value at the
  project tempo) and a reverb, each with an on switch and a mix, and a `level`. The preamp and the
  power amp run at four times the sample rate and every clip is antiderivative anti-aliased, so a
  high note at full gain keeps everything that is not a harmonic below -53 dB; the 35 samples of
  latency are reported, so plugin delay compensation and the bypass switch keep the amp in time with
  the rest of a project. The amp plays the mono sum of its two inputs (the cabled one when only one
  is) and makes the stereo image with its effects. The **setup** selector loads **Clean**,
  **Crunch**, **Industrial Rhythm** (very high gain, tight and percussive for drop-tuned guitars: a
  pre-gain high-pass and mid boost, a scooped but present mid, strong presence, a closed 4x12 and a
  noise gate), **80s Lead** (hot, singing sustain, mid-forward and compressed, with a 380 ms
  ping-pong delay and a plate-like reverb) or **Synthwave** (clean to light crunch, bright, a wide
  chorus, a dotted-eighth delay and a big reverb): it sets every control in one undo step, and you
  tweak from there. The rack panel shows the speakers of the chosen cabinet with the microphone, in
  and out meters, a gate meter, and a one-line tooltip on every control. The five setups are
  level-matched. The amp is an original design and the setups are named for the sound, not for any
  product or player. See [docs/DEVICES.md](docs/DEVICES.md#guitar-amp-atomikguitar-amp-effect).
- **Export the song or the loop as an audio file.** **File > Export Song as Audio File...** renders
  from the start of the song to the End marker and keeps the tail that is still audible (reverb and
  delay ring out, up to 10 s, cut where the output stays below -90 dBFS; never shorter than the
  End); **File > Export Loop as Audio File...** writes exactly the audio between the left and the
  right locator with no tail, so the file loops on the grid. Both ask for a WAV file, then for the
  options: **16-bit** with dither (triangular, the default for 16-bit; none; or triangular with the
  noise shaped towards the high frequencies), **24-bit** or **32-bit float**, the **sample rate**
  (the project's, or 44.1, 48, 88.2 or 96 kHz: the render is converted before it is quantised), and
  whether a song keeps its **tail**. The render runs faster than real time on a worker thread
  through the same path as a bounce (mute and solo, the master section, automation, tempo ramps and
  plugins, the latter needing the plugin sandbox as a freeze does), on a copy of the project, so
  playback and editing go on; a progress bar with **Cancel** replaces the options, and a cancelled
  or failed export leaves no file (it is written under a temporary name and renamed when complete,
  and an existing file is replaced only by a complete one). The dither is seeded, so the same
  project exports the same bytes; 24-bit and float are not dithered. The choices are remembered in
  `export.json` next to `settings.json`. Both are commands (`file.export_song_audio`,
  `file.export_loop_audio`, no default keys) that can be remapped in Settings, Keyboard. The
  renderer gains 16-bit PCM output (`atomik-render --format pcm16`, dithered) and the QA server
  gains `session_export`. A bounce (`atomik-render`, `session_bounce`) still ends two beats after
  the End. No project format change. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#27a-export-as-an-audio-file).
- **A song End marker.** The arrangement ruler shows an **E** flag where the song ends: drag it (it
  snaps like the loop locators, and one drag is one undo step), choose **Transport > Set End Marker
  at Playhead**, or right-click the ruler and choose **Set End Marker Here**. The lanes past it are
  shaded and the scroll bar covers it. The End is the length of the song for a render; playback
  still runs past it. Drawing, moving or stretching a clip past the End now moves the End out to the
  next bar line in the same undo step (it never moves back by itself). Projects need no migration:
  the End was already saved.
- **A tuner for the audio track input.** The name plate of an audio track in the rack has a
  **TUNER** switch, on the left under the name, off by default: it opens a display with the note and
  octave of what is plugged into the track (a guitar or bass, 28 Hz to 1.6 kHz), a needle from -50
  to +50 cents that turns green within 3 cents and lights an IN TUNE lamp, the frequency in Hz and
  the deviation in cents. It reads the input after the track's input gain whether or not the track
  is armed, smooths the needle, and holds the last note for 0.8 s when the string decays; a signal
  below about -56 dBFS shows PLAY A NOTE (raise the input gain for a weak pickup). **Mute while
  tuning** (on by default, a param of the track) silences the track while the tuner shows; the **A4
  reference** (415 to 466 Hz, default 440) is a param too. Both are additive: older projects load
  with 440 Hz and mute on, and the tuner is hidden whenever a project opens. While it is hidden the
  panel has no display and no tuner settings and is one rack unit shorter. The analysis runs off the
  audio thread, so tuning costs the audio thread two small copies per batch.
- **A level meter in every audio track header in the sequencer.** Each audio track shows a small
  stereo meter with a peak hold, a dB readout of the held peak (`-6.2`, `+1.4`, `-inf`) and a clip
  lamp that lights when a peak reaches 0 dBFS and stays lit until you click the meter. An armed
  track reads its input after the input gain, per channel (the level the transport bar's recording
  meter shows, and the lamp is shared with it: clicking either clears both); an unarmed track reads
  its playback, the output of its console channel after the fader and the mute, so the number is
  what you hear from the track. At the Small track height the header keeps the bars and the lamp and
  drops the readout; at every other height the meter sits on the first line, after the track name
  (an audio track's name has less room). The meters update even with the mixer hidden, and the
  sequencer repaints only when a visible meter changes. Note tracks have no meter (use the mixer).
- **Left and Right select the previous and next note in the piano roll.** With the piano roll
  focused, `Left` and `Right` select the note before or after the selection in time order (notes
  that start together go from low to high pitch); with nothing selected, `Right` takes the first
  note at or after the playhead and `Left` the last one before it. `Shift+Left` and `Shift+Right`
  add the neighbour to the selection. The view scrolls to keep the note in sight. They are the
  commands Select Next Note, Select Previous Note, Extend Selection to Next Note and Extend
  Selection to Previous Note, remappable in Settings, Keyboard. In the standard keys the nudge
  already sits on `Ctrl/Cmd+Left` and `Right` (with Shift for a beat, Alt for a tick), so nothing
  moves; the legacy keys keep `Left` and `Right` as the nudge and leave the new commands unbound.
- **Notes are coloured by velocity.** In the piano roll a note's fill (and its bar in the velocity
  lane) runs from a dim steel blue for a soft note, through blue and green, to a bright warm yellow
  for a loud one. A selected note is lifted toward white with a light outline, and a muted note
  stays a faint hollow fill in its velocity colour. The note clips in the arrangement keep the
  clip's colour.
- **Draw notes with the Select tool.** In the piano roll, a double-click on the empty grid draws a
  note (at the snapped start, the length of the snap step or of the last drawn note, as the Pencil
  does) and selects it; keep the button down after the second press and drag to set where it ends.
  It is one undo step. A single click or drag on the empty grid still clears the selection and
  rubber-bands, and a double-click on a note does what it did.
- **A mouse cursor per edit tool, and resize arrows on a clip's edge.** Over the arrangement's lanes
  and clips and the piano roll's grid the pointer shows the tool in use: a pencil for Pencil (its
  tip is the pointer), an eraser for Eraser, a pair of scissors for Razor, the arrow for Select; the
  alternate-tool key swaps it while held. The cursors are drawn in code, outlined for dark and light
  content, and sharp at 1x and 2x. A clip's right edge in the arrangement (and the start edge of an
  audio clip) now shows the horizontal resize arrows with Select, and keeps them during the drag; a
  note's ends in the piano roll already did, and the other tools show their own cursor there since
  they do not resize. Headers, the ruler and the buttons keep the arrow. No project format change.
- **Rename a device in the rack.** Double-click the name on the plate of any rack device
  (instrument, effect, player, utility or container, inside a container too, in the docked and in
  the detached Rack window) and type a new name in place: Enter commits, Escape cancels, a click
  elsewhere commits, and an empty name gives the device back the default name of its type. It is the
  same name the track and the mixer channel show, so all three change together, as one undo step. No
  project format change.
- **Grain sampler.** A new instrument, `atomik.granular` ("Grain Sampler" in the browser): one
  sample played as a cloud of grains, up to 64 at once per voice. Density, size, three window shapes
  (Hann, Tukey, percussive), a position through the sample with a **scan** (0 freezes, negative runs
  back) and a random **spray**, a pitch from the key, transpose and a per-grain scatter, a share of
  reversed grains, a random stereo spread and a grain cap that replaces the grain nearest its end.
  Eight voices by default with an amp and a mod envelope, a filter, an LFO, the mod wheel (spray by
  default), pitch bend and pitch, position, density, cutoff and level CVs. Drop a sample on the
  panel or load it from the browser (one sample; a drop replaces it in one undo step, relink works
  as for the sampler); with none the device plays a built-in vowel pad made in code. A high
  transposition is low-passed against aliasing. The panel shows the waveform of the sample window
  with the position, the spray band and the live grains, a sample strip and a tooltip on every
  control. Five **sounds** (Init, Frozen Pad, Shimmer, Stutter, Texture) load through the same
  one-undo-step batch as the FM synth's. `examples/patches/granular-demo.json` is a demo.
- **FM synth.** A new instrument, `atomik.fm-synth` ("FM Synth" in the browser): four sine operators
  that phase-modulate each other through one of twelve routings (stacks, forks, pairs, additive),
  each with a frequency ratio (coarse step plus a fraction, or a fixed frequency in Hz), detune,
  level, velocity sensitivity, key level scaling and its own envelope with a delay; a damped
  feedback path on operator 4 that turns a sine saw-like without going to noise; a pitch envelope,
  an LFO (pitch, amplitude, index), the mod wheel and an index CV on the modulation index; poly with
  up to 16 voices and stealing, mono and legato with glide; stereo spread by key and the usual
  pitch, level and pan CVs. A bandwidth limiter eases the index and the feedback back on high keys
  so bright patches do not alias. The panel draws the algorithm as a diagram and each operator's
  envelope as a shape, with a row of controls per operator and a tooltip on every control. Six
  **sounds** (Init, E. Piano, Bell, Bass, Brass, Pad) load through the same one-undo-step batch as
  the guitar amp's setups. `examples/patches/fm-demo.json` is a demo.
- **Wavetable synth.** A new instrument, `atomik.wavetable-synth` ("Wavetable Synth" in the
  browser): two wavetable oscillators, each with a table (twelve factory tables generated in code:
  saw to square, pulse width, sine to saw, sine to hollow, triangle to saw, vowels, a harmonic
  sweep, a hard-sync sweep, two FM families, seeded digital spectra and a drawbar organ), a frame
  position (0 to 100 %, the two neighbouring frames blended, so a sweep is smooth), octave,
  semitone, cents and level; up to seven unison copies of each with detune and stereo spread; a sub
  sine and noise; a stereo pair of state-variable filters (LP, BP, HP); an amp and a mod envelope
  and an LFO, any of which can move the frame positions, and the mod envelope or the LFO the pitch
  and the cutoff; velocity, pitch bend and the mod wheel as in the poly synth; CV inputs for the
  cutoff, pitch, position, resonance, level and pan. The tables are band-limited by octave (the
  oscillator reads the richest level whose harmonics stay below Nyquist, detune and pitch modulation
  included), so high notes do not alias; they are built once per process and shared by every
  instance, and `process` allocates nothing. The panel shows each oscillator's current frame. No
  project format change (the device is new; `examples/patches/wavetable-demo.json` is a unison pad).
  See [docs/DEVICES.md](docs/DEVICES.md).
- **Input gain, level meter and clip-safe for audio recording.** Arm an audio track and the right
  end of the transport bar shows its input: **INPUT GAIN** (-60 dB, which is off, to +24 dB; drag or
  scroll, Shift for tenths of a dB, double-click for 0 dB; a drag is one undo step), **SAFE** and an
  input level meter in dBFS with a peak hold and a clip lamp that lights when the level after the
  gain reaches 0 dBFS and stays lit until you click the meter. It shows the selected armed track,
  else the first armed one, works before a take starts, and gives way to the clip inspector on a
  narrow bar. The gain is applied to the recording and to the input monitor alike, so what you hear
  is what is written, and a gain turned during a take glides without a zipper. **SAFE** puts a
  limiter on the take only (instant attack, about 43 ms release, the channels of a stereo track
  limited together, never above 0 dBFS, a take that stays below it untouched to the bit; the monitor
  is not limited); without it a hot input is kept as it was played in the 32-bit float file. Two
  tracks recording one input each keep their own gain. The settings are the track device's new
  `rec_gain` and `rec_clip_safe` params: stored, undoable and listed with the device, and a project
  written before them records at 0 dB with SAFE off, as it did. The engine gains
  `EngineHandle::record_tap_gained` (a gain and a limiter per recorded track, wait-free controls);
  the plain record tap is unchanged. See ARCHITECTURE.md 2.6.
- **Clip inspector.** Select audio clips in the arrangement and the transport bar shows their
  properties to the right of the tempo and signature: **FADE IN** and **FADE OUT** in milliseconds,
  **TRANSPOSE** in semitones (the two decimals are cents), **GAIN** in dB and a **REV** button, with
  the clip's file name (or "N audio clips") in front. Values take the same drag, wheel (Shift: fine)
  and double-click-to-type as the tempo. With several clips a field shows a dash when they differ,
  and an edit sets all of them in one undo step. A fade never grows past the clip less the other
  fade. A reversed clip cannot be transposed and the Repitch warp mode does not transpose: when no
  selected clip can, the TRANSPOSE field is dimmed and its tooltip says why; in a mixed selection
  those clips keep their settings. REV switches a warped clip's warp and transposition off, as
  Reverse Audio does. With only an audio track selected the bar shows its name and the fields
  dimmed, with a tooltip saying to select a clip; with nothing audio selected the inspector is not
  drawn. On a narrow bar the clip name goes first, then the fields from the right (gain, reverse,
  transpose, fade out). Every field has a help tooltip.
- **Tooltips on the buttons.** Resting the pointer on a button for 0.6 s shows a one-line tooltip
  under it that says what the button does and, when it runs a command that has a key, that key as
  the menus print it (the first chord of the active keymap, so a key you rebind shows at once, and
  an unbound command shows none). It hides on a press, a key, the wheel or when the pointer leaves
  the button, stays inside the window, and costs no repaint while it waits or when it does not
  change. Everything that is a button has one: the transport buttons and the tempo and signature
  readouts; the arrangement's tools, snap selector, track, lane, folder and take buttons and the
  marker list buttons; the piano roll's toolbar, scale bar and controller chips; the mixer's
  toolbar, snapshot bar, strip buttons and choices, inserts, output selectors and VCA strips; the
  rack's fold, edit, slice, TO CH, container, plugin, step sequencer, arpeggiator and drum generator
  buttons, its switches and choices and the breadcrumb; the sample, slice, step, sequencer and
  container editors and the parameter pane; the file browser's toggles, sort headers and path; the
  plugin banner; and the quantize, Strip Silence, Audio to Notes, Warp, MIDI Mappings, Macros, Find
  Parameter, Relink and Plugins panels. **Settings > Workflow > Show tooltips** (on by default,
  saved in `workflow.json`) switches them off, together with the rack's own hover texts of BYPASS
  and a return's EDIT button; the value readout shown while a knob or fader is dragged, the names of
  jacks and presets and the note mark of a track comment are information, not help, and stay. In the
  QA server `session_pointer` takes `wait` (seconds the pointer rests) and answers with the
  `tooltip` text; `session_texts` and `session_screenshot` show it. See
  [docs/KEYBOARD.md](docs/KEYBOARD.md).
- **Drag and wheel on values.** The tempo and the time signature of the transport bar, the snap
  selector of the arrangement and the piano roll, and the values of the quantize, Audio to Notes,
  Strip Silence and Warp panels take a vertical drag (up increases, down decreases, 5 pixels per
  step) and the mouse wheel (one notch, one step). The BPM readout acts on the part under the
  pointer: whole BPM (tenths with Shift) or the two decimals; the signature steps the beats per bar
  on the left of the slash and the beat unit (2, 4, 8, 16) on the right; the snap selector and the
  quantize grid step through their lists in order and stop at the ends. A tempo or signature drag
  applies live and is one undo step. A click without movement does what it did (the snap selector
  now cycles on release instead of on press), and double-click still types the tempo or the
  signature. The gestures are in [docs/KEYBOARD.md](docs/KEYBOARD.md).
- **Drum Generator.** A note player (`atomik.drum-generator`, "Players" in the browser) that plays 8
  patterns of 16 lanes (one key each, by default the drum machine's 16 pads) and varies them with a
  pure function of the position: `density` adds hits on empty steps or removes hits, weighted by
  what the lane is (an original table: kick on the beats, snare and clap on the backbeats, hats on
  every eighth, toms late in the bar), `variation` moves hits a step and adds ghost notes in some
  bars, `fill.every` and `fill.amount` turn the end of every 2nd, 4th or 8th bar of the time
  signature into a fill, and `accent` boosts each role's strong positions. A bounce is bit-identical
  at any block size and worker count. The 4-unit panel has the pattern slots, a grid of 16 lanes by
  16 steps with a page selector (a click toggles a step, a vertical drag sets its velocity, a square
  mutes a lane), COPY, PASTE and CLEAR, and the knobs; the lane labels show the pad names of the
  drum machine it is cabled to, directly or through other players. GENERATE writes the edited
  pattern from a style (four on the floor, broken beat, half time or straight), `density` and
  `random.seed`, RANDOMISE does the same with the next seed, each one undo step and the same pattern
  for the same seed. Project format 33 saves the patterns (`"drums"` on the device; older projects
  load unchanged and render the same). See
  [docs/DEVICES.md](docs/DEVICES.md#drum-generator-atomikdrum-generator-utility-players).
- **CV Sequencer.** A note player (`atomik.cv-sequencer`, "Players" in the browser): the step
  sequencer's patterns and note rules plus two rows of control voltage per pattern on
  `curve_a_cv_out` and `curve_b_cv_out`, held per step or sliding linearly into the next step,
  bipolar or unipolar with an amount, and a `chain` of 1 to 8 patterns that play in order, each in
  its own resolution and swing. The position is computed from the tick, so a locate, a loop wrap and
  a bounce land on the same step and curve value at any block size. The curves hold their last value
  on stop and while bypassed. The panel is the step sequencer's with a strip per curve; the grid
  editor shows two curve lanes for it (press or drag to set a value, Alt-click for a slide). Project
  format 32 saves the curves (`"curves"` on the device; older projects load unchanged and render the
  same). The `player-demo` project gains a Filter Sweep cabled to a synth's cutoff. See
  [docs/DEVICES.md](docs/DEVICES.md#cv-sequencer-atomikcv-sequencer-utility-players).
- **Player stacks.** A note player can sit above an instrument and play through it for the
  instrument's track. Select an instrument (or a player of its stack) and add a player from the
  device browser, or the library's device rows: the player goes above the top of the stack in the
  rack, its notes are cabled into it, the track's notes now enter the new player (project format 31,
  `"notes_to"` on the track), and the whole step is one undo. A second player goes above the first.
  Shift adds the player unrouted, and a player dropped at a rack slot or created inside a container
  is a plain add. Deleting a player cables its note source to what it fed, and the track's entry
  moves to the next device down. The live keyboard on an armed track, the audition, a bounce and a
  freeze all play through the stack, bit for bit the same at any block size; a take records the
  notes you played, before the players; a project whose entry names a device that is gone plays the
  instrument as before. `project_inspect` and `session_status` list a track's `notes_to`. See
  [docs/DEVICES.md](docs/DEVICES.md), "Stacking players".
- **Note Repeater and Note Delay.** Two note players (`atomik.note-repeater` and
  `atomik.note-delay`, "Players" in the browser). The repeater is an echo: every note keeps playing
  and adds repeats at a note value or a time in milliseconds, with a velocity decay per repeat, a
  pitch step in semitones or scale degrees, a gate and an optional dry note; repeats ring out after
  the key is released, across a loop wrap and while stopped. The delay moves every note event later
  (note value or milliseconds), with swing on the off-beats of a 1/8 or 1/16 grid while the
  transport plays and seeded humanize of time and velocity, keeping note lengths and never adding a
  note. Both keep their notes in a fixed queue of 512 scheduled events that drops notes, never note
  ends, when full, and have the BYPASS switch. See
  [docs/DEVICES.md](docs/DEVICES.md#note-repeater-atomiknote-repeater-utility-players).
- **Key & Chord.** A note player (`atomik.key-chord`, "Players" in the browser) that locks notes to
  a scale and plays chords from single notes: the scale (root and any of the piano roll's 16 kinds)
  snaps notes to the nearest, next higher or next lower scale note, or filters out the notes outside
  it; a chord (a diatonic triad, seventh or ninth on the scale, or any of the 21 chord kinds) with
  inversion, an open, drop-2 or bass voicing, an octave shift and a velocity for the added notes.
  Changing the scale or chord while keys are held releases the old chord exactly, so nothing sticks
  or doubles. A FROM CLIP button on its panel copies the scale of the clip open in the piano roll
  into the device in one undo step. It has the BYPASS switch of the other players. See
  [docs/DEVICES.md](docs/DEVICES.md#key--chord-atomikkey-chord-utility-players).
- **Note Filter.** A note player (`atomik.note-filter`, "Players" in the browser) that transposes
  and filters the notes cabled through it: transpose and octave, a key range (pass inside, pass
  outside or fold into the range), a velocity range, and velocity curve, scale, offset or a fixed
  value, with switches for pitch bend, controllers and pressure. A note shifted outside 0..127 is
  dropped, never clamped. Changing a param while keys are held releases the pitch each key actually
  sent, so nothing sticks or doubles. It has the BYPASS switch of the other players. See
  [docs/DEVICES.md](docs/DEVICES.md#note-filter-atomiknote-filter-utility-players).
- **Bypass on the step sequencer and the arpeggiator.** Both note players get a `bypass` param
  (appended last; older projects load with it off) and the BYPASS LED button of the effects in their
  name strip. Bypassed, a player passes its input notes straight through and plays nothing; the
  switch closes what it sent, and a key held at the switch is ignored until its own Off, so nothing
  hangs. See [docs/DEVICES.md](docs/DEVICES.md#player-bypass).
- **Tracks in a folder are indented.** The content of a track header (chevron, name, M, S and R, the
  other buttons, and the labels of its lanes) moves right by 10 px per level of folder, up to three
  levels, so nested folders read as a tree. The clips and the folder rows do not move.
- **Rename a track in the sequencer.** Double-click the name in a track header (an instrument, audio
  or bus track) to type a new one in place, or use **Track > Rename Track...** (also in the track
  header menu). Enter or a click elsewhere commits, Esc cancels. It is the mixer's rename
  (`Command::Rename`, a bus is renamed in the console too): one undo step, the name changes
  everywhere, and a blank name is ignored. A double-click on a note lane's label still renames the
  lane.
- **Detachable windows.** **View > Detach Rack** (`Ctrl/Cmd+Shift+F6`) and **Detach Mixer**
  (`Ctrl/Cmd+Shift+F5`) put the rack or the console in a window of their own, on a second monitor
  for instance; closing the window, or the same command, puts it back. One document and one set of
  shortcuts serve all the windows, the meters run at full rate in the mixer window, files and
  library items can be dragged from the file browser into the detached rack, and the windows come
  back where they were at the next start (the main window too, maximised or not; a window that was
  on a monitor you removed opens on a visible one). `--detach rack,console` opens them at start.
  Drag between windows is not available on Wayland (it cancels with a note). The platform side is in
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6m-windows-the-platform-side).
- **Library.** The file browser gets a Library section: **Devices** (the built-in devices),
  **Patches**, **Containers**, **Clips** and **Projects**, kept in a library folder
  (`Documents/Atomik Rack/Library` by default; **Settings > Workflow > Library folder**, or
  `--library <dir>`). **Save to Library** (the device and clip context menus; commands
  `rack.save_to_library` and `sequencer.save_clip_to_library`, unbound) writes the selected devices
  as a patch, one container as a container item, or the selected note clip as a clip, with the
  samples they use beside them; saving changes nothing in the project and the new row opens its
  rename entry. Double click or Enter loads an item (a device row is added like in the device
  browser, a patch or container is pasted, a clip goes on the selected note track at the playhead),
  and so does dragging it onto the rack (at the slot under the pointer, inside a container too) or a
  clip onto a note lane; each load is one undo step and brings its audio, and audio the item lacks
  stays missing and relinkable. A project opens after the question about unsaved changes, also when
  dropped on the window. **Rename** (F2, the context menu) renames an item with its audio folder and
  keeps its tags and favourite. New file types: `.atomikpatch` and `.atomikcontainer` (ordinary
  project texts with only devices, cables and sample entries) and `.atomikclip` (`atomik-clip`
  version 1: one note clip with its tempo and meter). Templates, a start screen and collect and
  relocate are not part of this. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6l-the-library).
- **The library in the MCP server.** `validate` and `project_inspect` read the item files,
  `library_list` lists a library folder, `session_open` takes a `library` folder and
  `session_library` saves the selection or a clip into it and loads items back (recipe in
  docs/MCP.md).
- **MIDI learn.** Map any parameter to a MIDI control by moving it. **Mixer > MIDI Learn** switches
  the mode on (the status bar says so, and every mapped control is outlined): change a control in
  the rack or the console and it is armed instead of moving, then move a knob, fader, pad or key on
  the hardware and the mapping exists. A right click on a knob, fader or switch offers **Learn
  MIDI** and **Forget MIDI** without the mode. The sources are 7 bit controllers, 14 bit controller
  pairs (MSB and LSB are recognised as one source), pitch bend and notes, on one channel or any,
  from one input or any; the targets are any device parameter (rack panels, console strips, the
  plugin parameter pane), a VCA group's level, the tempo, a transport key (play, stop, play/stop,
  record, loop, return to zero) and the **selected strip** (one fader that follows the console's
  selection). Each mapping has a range (a fraction of the target's own range), invert, a mode
  (absolute, three relative-encoder encodings, toggle and momentary buttons), **soft takeover** (an
  absolute control waits until it reaches the value, so nothing jumps; on by default) and whether it
  is consumed. A consumed control plays no notes into the armed track (on by default; a keyboard key
  you did not map still plays, and everything is consumed while learning). A move is an ordinary
  edit: the engine's smoothers apply it, a sweep is one undo step, and **automation recording
  captures it as a touch** that lets go half a second after the control stops. Controls with a
  mapping carry a small dot; **Mixer > MIDI Mappings...** lists them with their source, target,
  range, mode and switches to edit, re-learn or delete. Mappings are saved with the project
  (**project format 28**; older projects load unchanged and bounce bit for bit the same, and so does
  a project with mappings, which never change a render) and as **presets**: Export and Import MIDI
  Mapping Preset write and read a file that addresses devices by type and position (the first poly
  synth, channel strip 3) instead of by key, so a preset made for one project applies to the
  matching devices of another. Creating, editing and deleting a mapping is undoable.
- **Control surfaces.** The **Surfaces** tab of Settings picks a control surface driver and the MIDI
  input and output ports it talks through (saved in `settings.json`, not in the project). The first
  driver is the common eight-fader motor surface protocol (MCU-compatible): 14 bit motor faders both
  ways (a touched fader is left alone; the touch sensors start and end an automation touch and hold
  one undo step), the master fader, V-pots with LED rings for the pan, mute, solo, record-ready and
  select keys with their LEDs, bank and channel keys (the strips are in console order: channels,
  then buses, then VCA groups), the transport keys and lamps, the jog wheel, the beats and timecode
  display, and the scribble strips (strip names, the pan and, while touched, the fader level).
  Feedback is sent only for what changed, at most thirty times a second, from the UI thread. The
  driver is plain Rust behind a small trait and tested byte for byte without hardware. A second
  protocol, level meters and OSC are on the roadmap.
- **MIDI control in the MCP server.** `project_inspect` and `session_status` list the mappings;
  `session_midi_input` sends note, controller and pitch bend messages to the mappings as a live
  input would, `session_midi_learn` learns, edits and exports mappings, and `session_surface` plays
  a control surface on an in-memory port, so learn, mappings and the surface driver can be tested
  headlessly (recipe in `docs/MCP.md`).

- **Macro banks.** A **macro bank** is a new device (`atomik.macro-bank`, in the device browser with
  the utilities) with eight macro knobs. Right-click any knob, fader or switch of a rack panel, a
  console strip or a row of the parameter pane and choose **Assign to Macro > Macro 1 to 8** (or
  **Assign to New Macro Bank**): the param is driven by the macro between a minimum and a maximum,
  optionally inverted, and several params can follow one macro. A driven control shows the macro's
  value, wears an `M1` tag, refuses a drag and keeps its right-click menu (**Remove from Macro**
  leaves the param at the value it had). A first assignment moves the macro so nothing jumps.
  **Mixer > Macros...** is the panel of the banks: a bar for each macro, a name edited in place,
  LEARN for a controller, and every assignment's range, invert and removal. The macros are
  automatable (the bank has its own track in the arrangement, which is also how an effect or a
  channel strip is automated, since only a device that takes notes has a track) and a lane on a
  macro plays as lanes on its targets, live and in a bounce, bit for bit like hand-made lanes.
  Project format 29 saves them; older projects load unchanged and a project without macros renders
  as before.
- **Find Parameter.** **View > Find Parameter...** (`Ctrl/Cmd+Shift+P`) searches every parameter of
  the project, containers' insides, channel strips, buses, the master, plugin params, macros and VCA
  levels included, by track, device, param name and unit as you type (`lead > filter > cut` works, a
  prefix ranks above a substring above a fuzzy match). Each row reads `track > device > param` with
  its value, a macro tag and MIDI and lane marks; the arrows move, Enter reveals it in the rack or
  the console, **Ctrl/Cmd+L** shows its automation lane, **M** assigns it to a macro, **E** arms it
  for MIDI learn and **Ctrl/Cmd+Left and Right** adjust the value in place.
- **Automation follows the selection.** **View > Automation Follows Selection** (also in Settings,
  Workflow; off by default) shows the lane of the parameter you touch: pressing a knob in the rack,
  a fader in the console or a row of the parameter pane adds the empty lane to the track when it is
  missing (no automation data is written), opens the folder that hides the track, shows a folded
  lane open without changing the project, scrolls it into view and outlines it; a param a macro
  drives shows the macro's lane, and selecting a device shows the last lane touched on it. **View >
  Show Automation Lane** does it on demand. Nothing is added while an automation take records.
- **Macros and the parameter search in the MCP server.** `project_inspect` and `session_status` list
  the macro banks; `session_macros` makes banks and assignments, `session_param_search` and
  `param_search` search the session's project or a file (recipe in docs/MCP.md).

- **Note lanes.** A track can have several note lanes that feed its device together. **Add Note
  Lane**, **Rename Note Lane...**, **Delete Note Lane** and **Mute Lane** are in the Track menu and
  the track header's context menu; a lane from the second on has a header with its name and an M
  button, and the first lane gets a small chip when there are several. A click on a lane's header,
  on its empty part or on one of its clips picks it: recording, Paste and MIDI import go to the
  picked lane. Clips drag between lanes, a double-click on a lane's name renames it, and a muted
  lane plays nothing (in playback, bounce and freeze) and exports no MIDI. Unmuted lanes merge:
  where two lanes start the same key or set the same controller at the same tick, the later lane
  wins. `project_inspect` and `session_status` list each note lane's index, label, name and mute.

### Changed
- **The project format is version 31** (a track may have a `notes_to`, the top of its player stack).
  Version 30 files load unchanged.
- **The project format is version 30** (a notes lane may have a `name` and `muted`, and a track may
  have several notes lanes). Version 29 files load unchanged.
- **The project format is version 29** (an optional `macros` object after `midi_maps`). Version 28
  files load unchanged. A MIDI mapping cannot be armed for a param a macro drives (map the macro),
  and the device browser lists the macro bank among the utilities.

### Fixed
- **Controls follow their automation lane.** A knob, fader, switch or selector of a parameter that
  has an automation lane now moves with the lane while the song plays, in the rack, in the console
  strips, in the parameter list of a plugin and in the detached windows, and shows the lane's value
  at the playhead when the song is stopped (a locate moves the controls too). Before, the sound
  followed the lane but the controls kept the value stored in the project. The stored value is not
  changed by playback: no undo step, and the project does not become modified. A control whose
  parameter has a lane carries a thin cyan ring and a dot at its corner, and its tooltip says it is
  automated. While you record a lane, the control shows the value you are moving (in Touch it
  follows the lane again when you let go); with the song stopped, a value you set by hand stays on
  show until you move the playhead or play. A lane on a muted track, or on a track another one's
  solo silences, plays nothing, so its control shows the stored value.
- **A project with comped takes opens after one of its comp clips was removed.** A take group
  remembers the clips it compiled onto the lane; removing or moving one of them with an ordinary
  clip edit left its id in the group, the save wrote it, and opening the project failed with "clip N
  is not on this lane". The save now writes only the ids still on the lane, and opening drops an id
  that is not there, so a project saved that way opens as it was. No format change.
- **Every detached window animates.** With the rack, the mixer and the sequencer in separate
  windows, only the focused one kept moving: the others froze until they were focused, because the
  OS's redraw requests were served window by window and the window it served first starved the rest.
  The app now paints every window that has something new each tick, asks again for a frame the GPU
  did not show, and, with more than one window open, presents without waiting for each monitor's
  vertical blank, so windows on monitors with different refresh rates no longer slow each other down
  (a single window keeps vsync). An idle app still paints nothing.
- **A reversed audio clip shows its waveform.** The arrangement painted nothing for a clip with
  Reverse on, although it played: the waveform reads the source frames under each pixel column, and
  a reversed clip reads them downwards, so every column asked for a range that ended before it began
  and found no audio. Columns now take the range in either order, and the silent stretch at the left
  of a reversed clip that starts above the end of its file is skipped instead of ending the drawing.
  The waveform is the mirror of the forward one, right after toggling, when toggled back and after a
  save and reload (the picture depends only on the clip). No format change.
- **Context menus no longer repaint the window for every pointer move.** An open context menu (the
  piano roll Notes menu with a big clip behind it was the worst case) marked the whole window dirty
  on every event, so each pointer move re-encoded and re-rendered the roll, thousands of notes
  included, even inside one row. It now repaints only when the highlighted row or the open submenu
  changes.
- **Reverse Audio works on a warped clip.** Reversing a clip whose warp or transpose was on was
  refused by the model (a reversed clip cannot be warped), so the command did nothing and the clip
  kept playing forwards. Reverse now switches the warp and the transposition off and reverses in one
  undo step; the warp mode, tempo and markers are kept.
- **Tracks enter at the top of a player stack.** A track made for an instrument that already has
  note players cabled into it, and a player cabled by hand into the device a track enters at, now
  make the track play through the stack (clips and the live keyboard reach the top player); cutting
  a note cable moves the entry back down to what still feeds the instrument. Before, such a track
  played the instrument directly and the players did nothing.
- **Submenus no longer vanish on the way to them.** Moving the pointer from a menu row that opens a
  submenu (the arpeggio options of the piano roll notes menu, for instance) toward the submenu
  crossed other rows of the parent panel and closed it. Now a move inside the triangle between the
  pointer and the submenu's near edge keeps it open, and the pointer must rest on another row for
  about a quarter of a second to replace it, on the left side of a menu near the screen edge too.
  This is the one menu code of the context menus (piano roll, sequencer, rack, console); the macOS
  native menus are the system's.
- **Keys work in menus.** In an open menu, Up and Down move the highlight (it wraps around and skips
  separators and disabled items, and it is the hover highlight too), Home and End jump to the first
  and last item, Right or Enter opens a submenu on its first item, Left or Esc closes it and returns
  to its row (Esc on the first panel closes the menu), and Enter or Space chooses the item. In the
  in-window menu bar (Windows and Linux) Left and Right move to the neighbouring menu, and Right on
  an item does too. An open menu keeps every key, so Space no longer starts playback behind it.
- **Right click on a note in the piano roll.** With the Select tool it no longer deletes the note:
  it selects the note (a note already in a multi-selection keeps the selection) and opens the notes
  menu, so Delete, Quantize and the rest act on it. The pencil, eraser and razor still delete on a
  right click.

## [0.2.0-rc.1] - 2026-10-25

### Fixed
- **Bounce in Place and Unfreeze and Keep Audio make an audible track.** The new audio track was
  silent while playing: its devices reached the engine after the routing that connects them, so the
  track and its channel were never cabled to the master. The changes of a compound edit inside
  another one are now reported with every device first, then one routing. The new track also sits
  right after its source track, in the same folder and colour, instead of at the end of the list.
- **Piano roll tools.** Only the pencil creates notes now: with Select a press on the empty grid
  starts a rubber band (a click clears the selection, Shift or Ctrl adds to it) instead of drawing
  one. A note can be resized from its left edge as well as its right one, and with several notes
  selected every one of them changes by the same delta (snapped, with a minimum length), in one undo
  step like a group move. The mouse cursor shows what a press will do: a horizontal resize cursor
  over a note edge and during the drag, a move cursor over a note body, a crosshair for the pencil.
- **Sandboxed plugin editors on Wayland.** The plugin process now uses the X11 backend (XWayland)
  whenever `DISPLAY` is set, so a sandboxed CLAP or VST3 editor opens as an X11 window it can embed
  into instead of failing with "Wayland sessions cannot embed a plugin editor". The rack window
  stays native Wayland. In-process editors and sessions without `DISPLAY` still fall back to the
  parameter pane.

### Added
- **Installers, a nightly build and signing in the release pipeline.** A release now publishes a
  Windows installer (`atomik-rack-<version>-windows-x86_64-setup.exe`, built with Inno Setup:
  Program Files or per-user, Start menu shortcut, optional desktop icon, uninstaller, `.atomik` file
  association), a macOS disk image (`atomik-rack-<version>-macos-universal.dmg`) and a Debian
  package (`atomik-rack_<version>_amd64.deb`, with the programs in `/opt/atomik-rack` and links in
  `/usr/bin`), next to the zip and tar.gz archives, with a `.sha256` for each and one `SHA256SUMS`.
  The smoke test installs and uninstalls each of them. A daily **nightly** pre-release (tag
  `nightly`, replaced each night that `main` changes) is built by the same reusable workflow.
  Signing is wired and waits for certificates: Authenticode for the Windows programs and installer
  (a certificate file, or Azure Trusted Signing), Developer ID signing with the hardened runtime and
  notarisation for the macOS app and disk image, and a GPG signature of `SHA256SUMS`. Each step is
  skipped with a notice until its secrets exist in the `release-signing` environment, so builds are
  unsigned today. See `docs/RELEASING.md`.
- **Mixer snapshots and comparison.** A snapshot is a named copy of the console: every param of
  every channel, output bus and the master (fader, pan, width, mute, solo, input trim and polarity,
  the sends, the dynamics, EQ and parallel compressor, the insert switches, whatever a strip gets
  later), the bypass of the insert and return effects, and the VCA groups' level, mute, solo and
  membership. A **snapshot bar** under the console toolbar shows a chip per snapshot (up to 16,
  named `A`, `B`, `1`, `2`... or by you), a **+** that stores the mix, and an **A/B** toggle. A
  click on a chip recalls it, a double-click renames it in place, and a right-click offers Recall
  (all, or only the levels, the mutes and solos, the sends or the insert bypass), Compare with this,
  Update from current mix, Rename, Move and Delete. The chip of the current snapshot is lit, with a
  dot once the mix has moved. **Compare** keeps the working mix in a hidden slot, recalls the
  snapshot and flips between the two without losing an edit made on either side; END keeps whichever
  is live. A recall is ordinary param changes (the strips' own 5 ms smoothers apply it, no clicks,
  no rebuild) and one undo step, as are store, update, rename, move, delete and every compare step.
  A snapshot does not hold the arrangement, the cables, the rack's structure or the effects' own
  parameters; a recall skips channels, effects and groups that were deleted since (the bar says
  which) and leaves the ones it never knew. The Mixer menu has Store Snapshot, Update Snapshot,
  Recall Snapshot, Recall Next and Previous Snapshot, Compare A/B, End Compare and Delete Snapshot
  (no default keys). Project format 27 saves them (a project without snapshots differs from its
  version 26 file in the version line alone and bounces bit for bit as before); the MCP inspection
  lists them and `session_command` stores, recalls (with a scope) and compares by id or name.
- **Channel strip upgrades.** A **parallel compressor** on every channel and bus: a second
  compressor (the one of the strip's dynamics section) after the inserts, the dynamics and the EQ,
  blended with the dry signal by **mix** (1 is the compressor alone) and followed by a makeup; the
  console has a PARALLEL section after DYNAMICS with an enable button and threshold, ratio and mix
  knobs, and the rack's channel panel has the attack, release and makeup too. It has no lookahead,
  so the dry and compressed paths stay aligned and it adds no latency; off, or at mix 0, the strip
  is bit for bit what it was and costs nothing. The **input trim** (-24 to +24 dB) and **polarity**
  were already on the strip; **Mixer > Toggle Polarity** now flips the selected strips. **Insert
  bypass keeps the delay compensation**: a bypassed plugin effect plays its input delayed by the
  latency it reports (as the built-in effects already did), so the channel does not move against the
  others when you toggle it, with a phase-aligned 5 ms crossfade; each filled insert slot has a
  **B** that bypasses its effect, and the strip's **BYP ALL** button (and **Mixer > Bypass All
  Inserts**, on the selected strips) bypasses every effect of the chain in one undo step. No project
  format change: older projects load unchanged and bounce bit for bit. `session_status` lists the
  strips that use any of it and `session_set_param` sets a device's param in a headless session.
- **Mixer groups and VCA faders.** A VCA group is one fader, one mute and one solo over several
  channels and buses, with no audio and no routing of its own. **Mixer > Create VCA from Selection**
  (on the console's selection, the arrangement's selected tracks or a selected folder, or the rack's
  selection) makes a group named `VCA n`; its strip in the console, after the buses, has a plate in
  the group's colour with the name and the member count, M and S, an offset fader from -60 dB (off)
  to +12 dB and a readout. Members keep their own faders and play at **their level plus the group's
  offset** (the sum in dB, smoothed like a fader, no zipper noise); a member's strip shows a marker
  where it plays and its readout shows that level. The group's mute and solo count as the members'
  own: solo beats mute, a soloed group silences the other strips as a solo does, and the members
  stay individually controllable. A strip is in at most one group; drag its name plate onto a VCA
  strip, use the right-click menu of a channel or bus (New VCA..., Add to VCA, Remove from VCA) or
  **Mixer > Add to VCA**, **Remove from VCA** and **Delete VCA**; members show the group's colour as
  a chip on their plate and as a bar on their track header in the arrangement. Every change is one
  undo step, a removed channel leaves its group with it, and a drag of the fader is one step. A
  project without groups sounds and bounces bit for bit as before. Project format 26 saves the
  groups (omitted when there are none, so older projects load unchanged); `session_status` lists
  them and `session_command` takes the id of a group. The rack's generated panels no longer show the
  app-only `internal.*` params (the channel's solo-silence toggle). Not yet: automation of the group
  fader, linked faders, groups of groups and a folder the group follows (see the roadmap).
- **Spectrum and metering.** Two new effects that pass the audio through untouched (bit for bit, no
  latency, the bounce is identical with or without them) and show what they see; both are in the
  device browser with the effects and have the usual bypass. **Spectrum** (`atomik.spectrum`) draws
  an FFT of 1024 to 8192 points (Hann or Blackman-Harris) of the left, right, mid, side or sum, on a
  log axis from 20 Hz to 20 kHz over a dB range you choose, averaged over a time you choose, with
  peak hold and decay, an optional tilt of 3 or 4.5 dB per octave, a loudest-bin readout, or the 31
  one-third-octave bands instead of the fine trace. **Phase Meter** (`atomik.phase-meter`) shows a
  goniometer (mid/side or plain left/right) from the recent samples, the stereo correlation from -1
  to +1 over an averaging time, the balance and a width figure, peak, RMS and hold bars for each
  channel, the sample peak and the true peak, and the momentary, short-term and integrated loudness
  in LUFS (BS.1770-4, the meter of the mastering device) with a reset. The audio thread only copies
  each batch into a lock-free ring (`ScopeTap`, a new opt-in of the device SDK registered by the
  engine); the FFT, the averaging and the drawing run on the UI side, so the audio path costs 128
  atomic stores a batch. The headless QA server gets `session_meters`, which renders a project
  offline up to a position and returns the readings (and fills the panels for `session_texts` and
  screenshots), so a generated -23 dBFS 1 kHz tone can be checked for -23 LUFS, correlation +1 and a
  1 kHz line. Nothing new is saved in a project: the devices are ordinary devices, the project
  format is unchanged and older projects bounce exactly as before.
- **Audio to notes.** **Audio > Audio to Notes...** extracts the notes of the selected audio clip
  into a note clip that starts and ends with it, in one undo step. **Pitched** follows one voice or
  instrument (a YIN pitch tracker with a voicing decision, written in house) and cuts it into notes
  by pitch stability and by re-attacks, so vibrato and a glide inside the pitch drift stay one note;
  the key is the pitch rounded to a semitone, the length follows the voicing and the decay, the
  velocity the level. **Drums** finds the hits of a loop and sorts them into kick, snare, closed and
  open hat (and a kick with a hat on top) from the spectrum after each onset, on the General MIDI
  keys 36, 38, 42 and 46. The panel sets the sensitivity, the shortest note, the range, the pitch
  drift, open hats, **Beats timing** (starts rounded to 1/4 to 1/32, or free) and the target: a new
  poly synth track or the note track selected in the rack. The notes follow the tempo map under the
  clip and the warp time map of a warped clip (a transposed clip adds its transposition; a clip in
  Repitch mode is refused), a reversed clip is read as it plays, and streamed assets are read in
  chunks. The analysis runs on a worker thread (at most ten minutes of a clip) while the song plays.
  `session_audio_notes` runs it in a headless MCP session. No file format change.
- **Freeze and bounce in place.** **Track > Freeze Track** renders the selected tracks through their
  instrument and every effect of their channel's insert chain and plays the render instead, so none
  of those devices run (the point: CPU). The track's clips, instrument and effects stay untouched;
  **Unfreeze Track** brings them back exactly (one undo step), **Unfreeze Track and Keep Audio**
  also leaves the render as a clip on a new audio track. **Freeze Track up to Selected Device**
  stops at the instrument or the insert effect selected in the rack, and the effects after it stay
  live; the channel's fader, pan, sends and output are never frozen. **Freeze with Tail** (on by
  default) keeps the decay of reverbs and delays past the end of the clips, cut where it falls below
  -90 dBFS. The render runs through the offline bounce on a worker thread (progress in the status
  line, **Cancel Freeze**, several tracks queue) and is stored in the project's audio folder; a
  render of a track that changed meanwhile is thrown away. The song keeps its sound: the project
  bounced with a track frozen equals the unfrozen bounce bit for bit when the chain has no tail or
  latency, and a chain with latency stays aligned with the other tracks. A frozen track shows a
  snowflake in its header and its render in ice blue in place of its clips; editing it, its devices
  or the tempo is refused with a message that says to unfreeze. **Bounce in Place** (and **and Mute
  Originals**) renders the selected clips through the track's chain into one clip on a new audio
  track and freezes nothing. Plugins render through the plugin sandbox. Project format 25 saves a
  frozen track (version 24 files load unchanged). The QA server runs the commands (`session_command`
  waits for the render) and lists `frozen` in the inspection.
- **Time-stretch and pitch-shift (warp).** Audio clips can follow the song tempo and be transposed
  without changing their length. **Warp** (Audio > Warp Audio, or Warp Settings...) plays a clip
  through a time map in beats at the tempo it was recorded at, so the audio stays on the grid when
  the song tempo changes or ramps; three engines are written in house, with no new dependency:
  **Beats** keeps every transient of drums and percussive loops where the tempo puts it and its
  attack intact and stretches the audio between hits, **Tonal** is a phase vocoder with phase
  locking for sustained and polyphonic material, and **Repitch** resamples (speed and pitch
  together, the cheap one). **Transpose** (Audio > Transpose, up or down a semitone or an octave, or
  the panel with semitones and cents, up to two octaves either way) changes pitch without changing
  length in Beats and Tonal. The source tempo comes from **Warp Loop as N Bars** (the clip becomes N
  bars long at the tempo its length implies), **Warp to Song Tempo** (an estimate from the
  transients of the audio, snapped to the exact tempo of the clip when it holds whole beats) or the
  panel's source tempo, DETECT TEMPO, /2 and x2. **Quantize Audio** (Edit menu and clip context
  menu) finds the transients of the selected clips and moves them to the grid of the quantize
  options by its strength with warp markers, stretching locally between them, with the ends of the
  clip pinned; markers survive trims and splits. The clip shows a `W` (with the transposition,
  `W +3`) and its markers as thin lines, and its waveform follows the stretch. A warped clip is
  rendered off the audio thread into a cached buffer (at most two background workers, 512 MiB,
  replaced when a setting, the tempo or the clip changes) and plays unwarped until it is ready; a
  bounce renders every buffer first, so it is never dry and stays bit for bit repeatable. Reverse
  and warp exclude each other; Strip Silence needs the clip unwarped. Project format 24; every
  earlier project plays and bounces as before. The QA server's inspection lists a clip's warp.
- **Audio editing.** Audio clips get fades and crossfades, clip gain and gain envelopes, reverse,
  normalise and strip silence, all non-destructive. A clip has a fade in and out whose shape is
  Linear, Equal power, Exponential or S-curve (Audio > Fade Shape, or the clip context menu), the
  fade and gain handles on the clip as before. A clip that starts over the end of an earlier one on
  the same track now **crossfades** with it: over the overlap the earlier clip fades out and the
  later one fades in, equal power by default (Audio > Crossfade picks Linear, Exponential, S-curve
  or Off, a hard cut), and the zone is drawn on the later clip; a clip inside another still hides
  it. **Gain envelopes** are drawn on the clip over the waveform: click with the pencil to add a
  point and drag it, drag points with the select tool, right-click or use the eraser to remove one,
  Audio > Clear Gain Envelope; the envelope ramps smoothly between points, sits in clip time and
  follows the audio when the left edge is trimmed. Clip gain also has Audio > Clip Gain (up and down
  1 dB, reset) next to the gain handle, and Audio > Fades sets a 5 ms declick or no fades.
  **Reverse** (Edit > Reverse Audio) plays the clip's span backwards and works with trims, fades and
  envelopes (an `R` marks the clip). **Normalise** (Edit > Normalise Audio) sets the clip gain so
  the peak of what each selected clip plays reaches 0 dBFS. **Strip Silence** (Edit > Strip
  Silence...) opens a panel (threshold, minimum silence, pad before and after, fade) and cuts the
  selected clips into their sounds, one undo step. Project format 23; files from before load with
  every clip's crossfade off, so old overlaps stay hard cuts and old bounces keep their bytes. The
  QA server's inspection lists audio clips with their gain, fades, reverse, envelope and crossfade.
- **Track organisation.** Tracks can be put in folders (Track > Group Tracks, Ctrl+Shift+G, on the
  selected tracks or the tracks of the selected clips; shift-click or ctrl-click on headers selects
  several). A folder is a row above its first track with a chevron that collapses and expands it, a
  name typed in place (double-click, or Track > Rename Folder), M, S and R buttons that mute, solo
  or arm every track of the folder in one undo step, and a summary of the clips of its tracks;
  folders nest up to 8 deep, Track > Ungroup (or Delete on a selected folder) dissolves one and
  keeps its tracks, and Move Track Up and Down (Alt+Up and Alt+Down) move a track one place at a
  time, in and out of folders, or a folder with everything in it. A folder does not touch the audio
  or the routing. A track can have a colour (header and clips; Colour submenu of the track header
  menu) and a comment (Track > Track Comment, typed in the header; a note mark shows it and its
  first line is a tooltip). An automation lane folds to a thin strip with the chevron at the left of
  its header (Track > Fold Automation Lanes folds them all), and the take rows that are open stay
  open with the project. Track > Track Height sets the selected tracks, or all, to Small, Medium,
  Large or Fit (as tall as the view lets them be). Project format 22 saves it all (a project without
  any of it differs from its version 21 file only in the version line; version 21 files open
  unchanged); the MCP inspection lists folders, colours and comments, and the Track menu commands
  are session commands.
- **MIDI file import and export.** Standard MIDI Files (type 0 and 1) in and out of the app. File >
  Export MIDI Clip, Track and Song write the selected note clip or track (type 0) or every note
  track (type 1: a conductor track with the song name, tempo map, time signatures and markers, then
  one track per note track, named after its device) with pitch bend, aftertouch and CC lanes as
  controller events; they write what plays (loops, offsets and swing applied, muted notes and clips
  left out) at 15 360 ticks per quarter note, the app's own resolution, so a round trip keeps every
  tick. File > Import MIDI puts a file in one clip on the selected note track at the playhead, or
  makes one new track per MIDI track and channel when no note track is selected; File > Import MIDI
  as Song makes new tracks from the start and also takes the file's tempo map, time signatures and
  markers. A `.mid` file dropped on the window makes new tracks at the playhead. Every import is one
  undo step. The reader handles running status, SysEx and unknown meta events (skipped), a note-on
  with velocity 0 as a note-off, hanging notes, type 0 files split by channel and other divisions,
  and refuses type 2 files, SMPTE time and corrupt files with a message instead of changing
  anything. Audio and automation lanes, range markers and program changes are not exported; a tempo
  ramp is exported as a staircase. No project format change. The QA server gains `session_midi`.
- **Quantize, swing, grooves and humanize.** Edit > Quantize Options (also in the clip and note
  context menus) opens a panel for the selected clips of the arrangement, or the selected notes of
  the piano roll (every note of the open clip when none is selected): a grid from 1/4 to 1 bar with
  triplets, a strength that moves notes part of the way, swing from 50 % to 75 %, a groove, humanize
  timing and velocity with a seed, and an option to snap the note ends too. A groove is a named
  template of per-step timing offsets and velocity scales: six come built in (Straight, Swing 55, 60
  and 66 %, a shuffled 1/8 and an accented 1/16) and EXTRACT GROOVE FROM CLIP measures one from a
  clip's own feel and keeps it in the song, shared by every track, until DELETE. The grid is the
  song's, so a clip that starts off the grid still lands on the bar lines. Humanize is the same on
  every run (it comes from the seed, the clip and each note's own position), so a bounce is
  reproducible; another seed gives another take. Applying is baked into the note times, one undo
  step over all selected clips, and playback and bounces are unchanged; two notes of one key that
  land on the same tick both stay (the later moves one tick on). Audio clips, controller lane points
  and the takes of a take group are left alone. Ctrl+K (Cmd+K on macOS, as in Reason) quantizes the
  selected notes of the piano roll or the selected clips of the arrangement at once with the panel's
  last settings (Q does the same in the legacy profile), Edit > Quantize does it from the menu
  (enabled with a selection, like Cut and Copy), and the piano roll's toolbar has a QUANT button
  that does it too, with a `...` button (or a right click on QUANT) that opens the panel. Project
  format 21 (older files load without grooves); `project_inspect` lists the song's grooves.
- **Loop recording and takes.** Recording over an enabled loop now stacks every pass as a take of
  one take group on the track (note and audio), instead of laying overlapping clips: the last pass
  plays and the others are kept. In the arrangement a thin comp strip under the lane shows which
  take plays where (each take has its own colour) and its chevron opens one row per take. Click the
  name of a take or its row to play that take alone, drag across a take row to comp a range from it
  (snapped, Escape cancels), use the cross to delete a take and FLATTEN to keep what plays as plain
  clips and drop the takes. Every action, and a whole recording, is one undo step. The comp is
  compiled into ordinary clips, so playback and bounces are unchanged, and a cut inside an audio
  take gets a 5 ms fade at the splice so it does not click. Punch and non-loop recording are
  unchanged. Project format 20 (older files load without groups); `project_inspect` lists the
  groups.
- **MIDI controller data.** Pitch bend, aftertouch (channel and per key) and CC lanes in note clips,
  played to devices and plugins, recorded from live MIDI input and edited in the piano roll. A
  CONTROLLERS field in the scale bar opens an area under the velocity lane with chips for Bend,
  Press, Mod 1, Vol 7, Pan 10, Expr 11 and Sust 64, a stepper for any other CC and one for poly
  aftertouch; draw with the pencil or the select tool (one point per grid step), drag points, delete
  them with the eraser or a right click, Ctrl or Cmd click toggles ramp and step, and a whole stroke
  is one undo step. Recording keeps every pitch bend, aftertouch and CC that reaches an armed track,
  per loop pass, next to the notes. Playback sends the events sample-accurately, restores each
  controller's value when you play or locate in the middle of a clip, and zeroes pitch bend,
  aftertouch and the sustain pedal on stop. The poly synth now follows pitch bend (two semitones)
  and the mod wheel (vibrato). Project format 19 (older files load without lanes).
- **Automation recording.** Arm an automation lane with the W button on its header, pick how it
  writes with the OVR / TCH / LTC button next to it (Overwrite replaces the whole pass, Touch writes
  only while you hold the control, Latch writes from your first touch until you stop), arm a track
  or just the lane, and press Record: moves of the lane's parameter (a knob drag, a typed value, a
  plugin's own window, a container macro) become automation points. The recorded points are thinned
  (Ramer-Douglas-Peucker, 0.5 % of the parameter's range) so a drag does not leave hundreds of
  points. While a lane is being written the old automation does not fight the control; a loop
  recording writes one clip per pass, and a whole take, whatever the number of lanes and passes, is
  **one undo step**. The write mode is saved in the project (project format 18; older files load on
  Touch), the arm state is not. MIDI controller input is not recorded yet.
- **Note chase.** A note that spans the play position now sounds when you press Play, locate while
  playing, or the loop wraps, instead of staying silent until its next note. Each track has a
  **Chase Notes** item (checked by default) in its header's context menu; switch it off for a track
  that should play only from its note starts. Live edits never retrigger a note that is already
  sounding. The setting is one undo step and is saved with the project (format 17: a track that does
  not chase has `"no_chase": true`; versions 1 to 16 load with chase on). The QA server's project
  and session status report `chase` for each track.
- **Relink missing audio.** File > Relink Missing Audio... (enabled while a project's audio files
  are missing) lists them with a LOCATE... button each and SEARCH FOLDER.... LOCATE... takes any
  audio file: it is imported into the project bundle and every clip and sampler, slicer and drum pad
  zone that used the missing asset is switched to it in one undo step (the old, missing state comes
  back with undo); a file with the very same audio only restores the bundle's file, with no document
  change. SEARCH FOLDER... scans a folder (8 levels, 50 000 entries at most, off the UI thread) and
  relinks the missing files it finds by name, in any case, leaving the rest listed; a toast says how
  many were relinked and how many are still missing. A file that does not decode, or that is too
  short for the zones that used the old one, is reported by name and changes nothing. The QA server
  gains `session_relink`, and `session_bounce` now renders from the session's bundle, so audio
  projects bounce in a session.
- **QA server (`atomik-mcp`).** A development tool, not part of the release archives: a Model
  Context Protocol server over stdio that lets an MCP client bounce projects and patches, measure
  and compare WAV files (levels, silence, loudness windows, spectrum, bit-exact or within a
  tolerance), and drive a headless app (menu commands, pointer, keys, the texts the UI draws, PNG
  screenshots, undo, save, and a bounce of the open document), with every path confined to the
  working directory and `--root` folders. [docs/MCP.md](docs/MCP.md) has the tools and the QA
  recipes that [CONTRIBUTING.md](CONTRIBUTING.md) now asks for. `atomik-ui` gains an optional
  `cpu-canvas` feature (the `vello_cpu` canvas its preview tests used, now library code).
- **Scales and chords in the piano roll.** A clip has a scale (a root and one of fifteen: Major, the
  minors and the modes, the pentatonics, Blues, Whole Tone, Phrygian Dominant and Hungarian Minor;
  no scale is Chromatic), chosen from a new scale bar under the piano roll's toolbar or the notes
  context menu's Scale submenu, or found by **Detect** from the notes. In-scale rows are tinted,
  out-of-scale rows are dark, the root rows are marked and the keyboard names them. The scale is
  saved with the clip (project format 16; older files load without one). **Snap to Scale**
  (`Shift+S`) puts pencil-drawn notes and vertical drags on the nearest in-scale key (a tie goes
  down) and makes Transpose Up and Down move by scale degree, and **Conform Selection to Scale**
  moves the selected notes onto the scale in one step. **Fold** (`Shift+F`, or the Fold field) hides
  the keys outside the scale, or every key the clip does not use, and drawing, dragging, band
  select, the keyboard and the key zoom all work on the folded rows. The **chord stamp** (twenty-one
  chords, four inversions, and diatonic triads, sevenths and ninths built from the scale) makes the
  pencil drop a whole chord that you can drag to length and that sounds when you click; **Chordify**
  (`Shift+C`) turns each selected note into the chord, **Arpeggiate** turns each chord of the
  selection into an arpeggio over its length (up, down, up-down, down-up, random or as played, at
  the current snap, triplets included, with a gate and one to three octaves), and **Strum** delays
  the notes of each chord one after the other and keeps their ends. Every edit is one undo step. The
  context menus gain submenus for all of it, and the snap to scale and fold choices are kept between
  sessions.
- **Deep zoom and finer editing.** The time zoom goes down to the sample: a tick is up to 16 px wide
  (about 10 px per sample at 120 BPM and 48 kHz), the ruler grid follows with lines at 1/32, 1/64,
  1/128 notes and down to single ticks, and below the bars the ruler names its lines
  `bar.beat.sixteenth.tick`. Snap gains 1/32, 1/64, the triplets 1/4T, 1/8T, 1/16T and 1/32T and
  **Grid**, which snaps to the lines the ruler shows at the current zoom (the selector in the
  arrangement and the piano roll cycles through them; `sequencer.set_snap` takes `thirtysecond`,
  `sixtyfourth`, `quarter_triplet`, `eighth_triplet`, `sixteenth_triplet`, `thirtysecond_triplet`
  and `grid`, and the piano roll's Quantize uses the same step). Waveforms in the arrangement, the
  sample editor and the slice editor read the samples themselves when you zoom in below 256 frames
  per pixel, and below one frame per pixel draw a line through the sample values with a dot on every
  sample (audio streamed from disk keeps the finest peak level); the editors' wave zoom reaches 32
  px per frame whatever the length of the file. Vertical zoom: drag the bottom edge of a track
  header to set that track's height (24 to 400 px, a double-click resets it), Alt+wheel in the
  arrangement zooms every track (Shift+H and Shift+G, Shift+Z resets the zoom and the heights),
  headers shorter than 52 px keep one line, and the piano roll zooms its keys the same way
  (Alt+wheel, 6 to 48 px, Shift+H, Shift+G and Shift+Z; the white keys are named from 16 px). Track
  heights are saved in the project (format 15, versions 1 to 14 load unchanged: a song may carry
  `"track_heights"`, written only when non-empty); the vertical zooms are saved with the window
  state.
- **Markers and range markers.** The arrangement has a marker strip under its ruler (View, Marker
  Strip; on by default and kept between sessions) with named point markers and named ranges (Intro,
  Verse...). Add a marker at the playhead with Shift+M (Transport, Add Marker), by a double-click or
  the pencil on the strip, or from the marker list; drag it to move it (snapped by the current snap,
  the disable-snap keys free it), double-click its label to type a new name in place, and delete it
  with the eraser, Delete or the context menu, which also has a small colour palette. Click a marker
  to locate the playhead there; `,` and `.` (and the Transport menu) go to the previous and next
  marker. Ranges are bands in a row above the markers: Transport, Add Range Marker from Loop (or the
  strip's context menu, from the loop or the selected clips) creates one, a drag of the body moves
  it, the edges resize it, and a click sets the loop to its range (Loop This Range and Select Clips
  in Range are in its context menu). View, Marker List docks a list of every marker and range at the
  right edge of the arrangement with its position as bar.beat.tick through the meter map (and the
  end of a range); a click on a row locates (a range row also sets the loop), a double-click on a
  name renames it, `x` deletes it, and `+ Marker` and `+ Range` add. Every edit is one undo step and
  a drag is one step. Markers follow the bars when the tempo or the meter changes. Project format 14
  (versions 1 to 13 load unchanged): a song may carry `"markers"` and `"ranges"`, written only when
  there are some. The legacy key profile is unchanged (the commands are in the menus).
- **Tempo and meter changes.** The song can change tempo with steps and linear ramps (the tempo
  moves in BPM over ticks) and change time signature on any bar line. A tempo lane in the
  arrangement (View, Tempo Lane; off by default and kept between sessions) plots the tempo curve:
  add a point with the pencil or a double-click, drag it (snapped in time, whole BPMs up and down,
  Shift for tenths, Alt for no snap), delete it with the eraser or Delete, and right-click for Ramp
  to Next, Set Tempo... and Delete. Right-click the ruler to insert a meter change (2/4 to 12/8) at
  that bar; its tag sits next to the bar number and can be dragged to another bar, changed or
  deleted. The ruler, the grid, snapping, the bar keys, new clips, recording and the metronome
  follow the meter map, and notes, audio clips and loop points land on the exact frame of the ramp
  live and in the bounce (one tempo map in `atomik-core` replaces the four copies of the
  conversion). Tempo-synced devices and Mod Source follow the tempo and the meter at every 64-frame
  batch. The transport shows the tempo and the meter at the playhead, and typing a tempo changes the
  event in effect there. Project format 13 (versions 1 to 12 load unchanged): a tempo event may
  carry `"ramp": true`, and `"meter"` is a list of `{"bar", "num", "den"}` changes. The metronome
  now clicks on the beats of the meter, an eighth in 6/8 rather than a quarter.
- **Plugin sandbox.** Every CLAP and VST3 plugin instance runs in its own `atomik-plugin-host`
  process, so a plugin that crashes or hangs no longer takes the application down. A crashed
  plugin's device shows CRASHED with the reason (dry for an effect, silent otherwise) while audio
  goes on, and RELOAD starts it again from its last saved state. Audio is exchanged through shared
  memory in the same 64-frame blocks, with no added latency; live, a block the plugin answers late
  passes dry. Bounces wait for every block, so a sandboxed bounce is byte-identical to an in-process
  one. Editors open in a window of the plugin's process. Settings > Plugins has "Run plugins in a
  separate process (recommended)", on by default (applies after restarting); `atomik-render` has
  `--plugin-sandbox on|off`. Release archives and the macOS bundle include the new
  `atomik-plugin-host` program.
- **Output buses.** The console can hold named output buses (`atomik.bus` strips with the channel's
  controls): channels and buses route to the master, to another bus (loops are refused) or to a free
  Audio I/O stereo pair, with commands to create, rename, reorder, delete and route to a new bus,
  each one undo step, and a solo that keeps a soloed bus's feeders and a soloed strip's buses
  audible. Project format 12 saves the bus list (omitted when there are none; versions 1 to 11 load
  unchanged). In the console, bus strips sit between the channels and the master with the full
  channel strip, an output selector on every strip (Master, buses, Audio I/O pairs, New bus...;
  pairs in use or beyond the project's outputs are greyed with the reason), rename by double-click,
  a bus context menu (Rename, Delete bus), drag to reorder and meters. Route to New Output Bus
  (Ctrl/Cmd+G, Mixer menu) puts the selected channels on a new bus. In the rack the plate reads
  `Output: Bus <name>` or `Output: Audio I/O 3+4`; an Audio I/O pair no longer shows the warning
  colour.
- **Mod Source.** A new modulation device with two LFOs (seven shapes with morph, free or
  tempo-synced rate, key sync, rate and amount CV inputs) and an ADSR envelope triggered by notes or
  a gate CV, with CV outputs for the rest of the rack. Its panel has a shape display for each LFO
  and a live scrolling scope of both LFOs and the envelope, and the device browser lists it in a new
  Modulators group.
- **Multi-column rack.** The rack can have several columns side by side (saved in the project,
  project format 11; older files load as one column). Add Column and Remove Column are in the rack
  context menu; an empty column stays until it is removed, and removing a column moves its devices
  to the one on its left. Each column scrolls with the rack, which now also scrolls sideways to
  reach all of them.
- **Dragging between columns.** A device, or a selected group, can be dragged to any place in any
  column; the drop line shows where it lands, and dropping to the right of the last column makes a
  new column. The whole move is one undo step. The rack scrolls by itself when you drag near its
  edges.
- **Drag devices from the browser.** Drag a row of the device browser onto the rack to create the
  device exactly where you drop it, in any column; an effect is routed as if the device above the
  drop point were selected. Release over the browser, or press Esc, to cancel. A double-click still
  adds below the selection.
- **Rack navigator.** A strip at the right edge of the rack (View, Rack Navigator; on by default)
  shows every column as small coloured blocks and the part of the rack you are looking at. Click to
  centre the view there, drag to scroll, use the mouse wheel as over the rack; inside a container it
  shows the container's column. The choice is kept between sessions.
- **Paste and plugin drops in columns.** Paste from the rack's context menu lands where you
  right-clicked, and a keyboard paste lands below the selected device in its column. A plugin that
  is neither an instrument nor an effect can be dragged from the browser into a column or the
  new-column zone like any other device, and the new-column zone now stays on screen while you drag,
  whatever the window width.
- **CV inputs on the effects, console channels and instruments.** One convention: a CV socket
  `<target>_cv_in` with a bipolar `<target>_cv_trim` (−1..+1, default +1). The stereo delay (mix,
  feedback), reverb, modulation and distortion (mix, drive), the EQ (gain of the four shelf and peak
  bands) and every console channel (fader, pan, the eight sends) take one; so do the poly synth
  (pitch, resonance, level, pan), the sampler (resonance, level, pan), the drum machine (global
  level and pitch, see below) and the loop slicer (level, and cutoff through a new global low-pass,
  `filter.cutoff`, which is open by default). Unplugged, nothing changes, and older patches and
  projects load as before.
- **Gates and global CV on the drum machine.** Two global CV inputs, `level_cv_in` (±24 dB) and
  `pitch_cv_in` (±24 semitones, also bending voices that are already sounding, glided over each
  64-sample batch), each with a bipolar trim. Per pad, a gate in (`padN_gate_cv_in`: a rise above
  0.5 plays the pad, velocity from the level, the fall releases a gate-mode pad) and a gate out
  (`padN_gate_cv_out`: high from every hit, from notes, the sequencer or the gate in, until its
  note-off). The back panel shows the 16 gate pairs and the two CV jacks with their trims.
- **CV activity indicators.** On the back of the rack, a CV cable that carries signal glows and
  lights both of its jacks, brighter for a stronger signal and fading smoothly when it falls silent.
- **Nudge by beat or tick.** In the piano roll, the selected notes can move by one beat or by one
  tick, whatever the snap: `Ctrl+Shift+Left/Right` (beat) and `Ctrl+Alt+Left/Right` (tick) in the
  standard profile, `Shift+Left/Right` and `Alt+Left/Right` in the legacy one (`Cmd` for `Ctrl` on
  macOS).
- **Console keys are commands.** The console's arrow, Page Up/Down, Home and End keys are now
  `mixer.*` commands in the Mixer context (the same keys as before, Shift for fine steps), so they
  show in the cheat sheet and can be rebound in Settings, Keyboard. The console also owns the
  keyboard as a text field while a channel name is being typed, so Space, Delete and the letters
  never run commands then; Enter commits and Esc cancels.
- **Editor keys follow the keymap.** Delete and Backspace in the sample editor (remove the selected
  zone) and the slice editor (merge the selected slice into the previous one) are now the
  `edit.delete` command, so they follow Settings, Keyboard and the active profile. The sample,
  slice, step, sequencer and container editors no longer read Ctrl+Z and Ctrl+Y themselves: undo and
  redo are `edit.undo` and `edit.redo` everywhere, and rebinding them takes effect in the editors.
  Esc and typing a name are unchanged.
- **Time signature in the transport bar.** Double-click the meter readout and type a signature
  (`3/4`, or a bare `3` to keep the beat unit, 1 to 32 over 2, 4, 8 or 16): it changes the meter
  change in effect at the playhead, undoable like the tempo. Before, the first signature could not
  be changed at all (a 4/4 start has no ruler tag).
- **Ctrl+Shift+wheel zooms vertically.** In the arrangement (track heights) and the piano roll (key
  heights), next to Alt+wheel; Ctrl+wheel stays the time zoom.
- **Multi-select and rubber band in the arrangement.** Shift+click adds a clip to the selection,
  Ctrl+click toggles it, across tracks (Ctrl+click is also how a copy drag starts, see below). A
  rubber band selects the clips it touches: drag from empty space below the last track, or Shift or
  Ctrl+drag on an empty lane (a plain drag on a lane still draws a clip); Shift or Ctrl add to the
  selection, a plain band replaces it, and Esc cancels it. Delete, copy, paste, duplicate, split,
  mute and the drag act on every selected clip, as before, and every selected clip is outlined.
- **Drag clips onto other lanes.** Dragging clips vertically onto another track moves them there
  (the time follows the horizontal drag), and Ctrl+drag drops copies instead (the clips stay put and
  the copies are selected). Each clip lands on the lane of its own kind: a note clip on the note
  lane, an audio clip on an audio track's lane, an automation clip on the lane of the same
  parameter; a target track without that lane (a note clip onto an audio track) ignores the drop and
  changes nothing. A group keeps its relative tracks and offsets, and the whole gesture, including
  the horizontal moves, is one undo step (a drag of several clips used to leave one step per clip
  and per move). A ghost shows where the drop lands. New `WorkflowCommand::MoveClips`; no project
  format change, since a clip belongs to its lane in the file.

### Fixed
- **Track header click acts like a console strip.** Clicking a track header in the arrangement (or
  stepping with Up and Down) now selects its strip in the console, auto-arms the track as a channel
  selection does (disarming the tracks an earlier auto-arm armed), selects the device in the rack so
  the live keyboard plays it, and a plain click hands the keys to the rack.

- **Device search.** Typing in the device browser's search kept the scroll of the full list, so the
  few matches (Serum for "ser") sat above the visible panel; the list now returns to the top on
  every edit.

### Removed
- **The plugin warning.** Turning plugins on in Settings > Plugins, or LOAD on a project that uses
  plugins, no longer shows a warning to confirm: plugins now run in their own processes by default.

## [0.1.0] - 2026-10-21

First public version. Windows (x86-64) is the primary platform and is tested by hand; the test suite
runs in CI on Windows, macOS and Linux, Linux has been tried on Ubuntu 26.04, and macOS has not been
run on real hardware yet. Project files are format version 10 (versions 1 to 9 load).

### Added
- **Engine.** A real-time engine with fixed 64-frame batches that is allocation-free and lock-free
  on the render path, hot edits of the rack while audio plays, multicore rendering with
  bit-identical output for any worker count and host block size, plugin delay compensation across
  the whole graph, and cable validation (types, direction, cycles).
- **Rack and console.** A device rack with audio, CV and note cables and front and back views; a
  console with a channel strip per instrument (gain, filters, dynamics, EQ, insert chain, 8 sends,
  pan, fader), solo and mute, meters; a locked master section with 8 send buses and FX returns, a
  bus compressor with a key input, a control-room output and the Audio I/O device.
- **Devices.** Poly synth, line mixer, stereo delay, sampler, drum machine, loop slicer, audio
  track, reverb, compressor, 6-band EQ, distortion with oversampling, modulation (chorus, flanger,
  phaser), mastering suite, the Character effect (16 presets, Transform macro), step sequencer,
  arpeggiator and the container device with macros and note splits.
- **Plugin hosting.** CLAP and VST3 instruments and effects as rack devices, with an out-of-process
  scanner, a plugin cache, editor windows, state saving, crash journal and recovery snapshots. VST2
  is not supported.
- **Arrangement and audio.** Note, automation and audio clips, a piano roll, sample, step, slice and
  container editors, import of WAV, AIFF, FLAC, MP3 and OGG, audio recording with latency
  compensation, disk streaming for large files, and project bundle folders with content-addressed
  audio.
- **Workflow.** A data-driven, remappable keymap with a standard and a legacy profile, tools
  (select, pencil, eraser, razor), a clipboard for clips, notes, points, devices and tracks, context
  menus, rack multi-selection and an auto-routing creation chain.
- **Tools.** `atomik` (the application), `atomik-render` (offline bounce of patches and projects to
  WAV), `atomik-play` (a live patch player) and `atomik-plugin-scan` (the plugin scanner).
- **Audio and MIDI I/O.** Live audio through WASAPI, CoreAudio and ALSA (and ASIO as an opt-in build
  feature that needs the Steinberg SDK, which is not included), and MIDI input with exact event
  timing.
- **Project hygiene.** GPL-3.0-or-later licence, contribution guide with DCO sign-off and
  test-driven development rules, code of conduct, security policy, CI for Windows, macOS and Linux
  (fmt, clippy, tests, cargo-deny, coverage), architecture, device, testing and keyboard
  documentation, and example projects.
- Project format 10: per-note mute. A muted note is a fifth element `true` in the note's array;
  muted notes stay in the clip and are skipped by playback and bounce. Versions 1 to 9 still load.
  New `SetClipsMuted` workflow command mutes or unmutes several clips in one undo step.
- Mute in the editors: `M` mutes or unmutes the selected clips in the arrangement (any clip kind,
  one undo step) and the selected notes in the piano roll (in the step editor, the cell under the
  pointer); Mute / Unmute items in the clip and notes context menus; an `M` button in each clip
  header; muted notes and step cells are painted hollow.
- **File browser.** A docked panel (View, File Browser, `F9`) with a folder tree (Favourites, Tags,
  Bookmarks, drives) over a file list of folders and audio files, a search field (words, `#tag`,
  `fav`, optionally in sub-folders), instant preview on click with a 5 ms crossfade when switching
  (live only, outside the routing and the meters), favourites, tags and bookmarked folders kept in
  `library.json` in the configuration folder, and drag to the arrangement, sample editor and sampler
  panels like an OS file drop. New crate `atomik-library`; `atomik --file-browser [DIR]` shows the
  panel at start. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#6j-the-file-browser).
- Release archives for Windows (zip), macOS (universal `Atomik Rack.app`, ad-hoc signed, in a zip)
  and Linux (tar.gz), each smoke-tested before a draft release is created, with per-archive
  `.sha256` files and a `SHA256SUMS` file.

### Known limitations
- Feedback loops are rejected; insert and return loops use stages instead.
- Parameter changes take effect at 64-frame boundaries (devices smooth audible parameters); notes are sample-accurate.
- No time-stretch for audio clips and no relinking of missing audio files.
- Plugins run inside the application process, so a plugin crash takes the application down.
- No note chase on play or locate.
- Linux VST3 editors that need file-descriptor handlers do not work, and there is no editor
  embedding on Wayland (the parameter pane is the fallback).
- Release builds are not code-signed or notarized (the macOS app is signed ad hoc); see the Install
  section of the README for the first start.
