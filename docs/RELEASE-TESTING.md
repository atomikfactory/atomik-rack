# Release testing protocol

What the continuous integration cannot reach: sound devices and drivers, MIDI hardware, real
third-party plugins, windows on real displays, and the installers. Every release candidate gets one
recorded run of the checks below, on Windows, macOS and Linux, and the report is attached to the
draft release.

- **Ids are stable.** An id (`HW-AUD-01`) never changes meaning and is never reused; a check that no longer
  applies is marked as retired in its title, not deleted, so an old report still reads.
- **Each item names the automated test it complements**, or `none`. A manual check is the part of
  the behaviour a test cannot see (sound that is heard, a window that is seen, a device that is
  plugged in).
- **Results** are `pass`, `fail` or `n/a`. `n/a` needs a note that says why (no such hardware, not
  the OS the item applies to). A `fail` blocks the release unless the release notes list it as a
  known issue.
- **Which parts a release needs.** A release candidate (a tag with a suffix such as `-rc.1`) needs
  the Windows items; a final release needs the items of all three operating systems. An item that
  says `Windows, macOS, Linux` is run on each of them and recorded once per run.
- **Run on the build that will ship**, the installer or archive of the draft release, not a build from
  source, unless the item says otherwise.

## Making the report

```sh
sh scripts/release/test-report.sh 0.3.0-rc.2          # writes release-test-report-v0.3.0-rc.2.md
sh scripts/release/test-report.sh --check             # validates this file (unique ids, every field)
sh scripts/release/test-report.sh --verify release-test-report-v0.3.0-rc.2.md --os windows
```

[scripts/release/test-report.sh](../scripts/release/test-report.sh) reads the ids from this file and
writes a report with one row per id, the environment captured (operating system, processor, commit,
version), and fields for the audio device and driver, the plugin versions and the tester. Fill in
the Result and Note columns by hand. `--verify` fails while an item of the chosen operating system
is still `pending` or `fail`, when an `n/a` has no note, or when an id of the protocol is missing
from the report; run it before attaching the report to the draft release. The script refuses a
protocol in which an id appears twice.

The release workflow does **not** require the report yet: attaching it is a step of the
[release checklist](RELEASING.md#checklist) for now. Making `draft-release` fail without the report
asset is a later change, once a few reports have been made by hand.

The scheduled hardware run described in [TESTING.md](TESTING.md#a-scheduled-hardware-run-recipe)
covers the automatable subset (the items whose `Automated` field names a `hw_` test); until a
machine runs it, those tests are run by hand as part of these items.

## Changing the protocol

A change that adds behaviour depending on hardware, a driver, a real plugin or a window system adds
its items to this file in the same pull request, with the automated test it complements. Run
`sh scripts/release/test-report.sh --check` before pushing: the hygiene job of the continuous integration runs it too.

## Audio devices

### HW-AUD-01 Open the default output and play
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the release build installed; a built-in output and, where there is one, a USB interface.
  Windows uses WASAPI, macOS CoreAudio, Linux ALSA directly and through the PulseAudio or PipeWire bridge.
- **Steps:** start the app, open Settings > Audio, choose the backend and the default output, open
  `examples/projects/demo.atomik` and press Play.
- **Expected:** sound without clicks or dropouts; the status line shows the device and the rate; Stop
  silences at once.
- **Automated:** `crates/audio-io/tests/hw_loopback.rs`
- **Known bad:** a silent stream with Play running, or a device that is listed twice.

### HW-AUD-02 Buffer sizes 16, 64 and 256
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-AUD-01 passed.
- **Steps:** in Settings > Audio choose each of 16, 64 and 256 frames and play the demo project for a minute at each.
  A size the device refuses must fall back to the nearest one it accepts and say so.
- **Expected:** no xruns at 64 and 256; at 16 either clean playback or an honest overload readout
  (no silent stream); the chosen size or the fallback is shown.
- **Automated:** `crates/audio-io/tests/hw_loopback.rs`
- **Known bad:** the dialog shows 16 while the device runs at another size.

### HW-AUD-03 Ten minutes of the cumulative stress project
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the cumulative stress project saved from the app (every `--stress-*` option together)
  as a project file.
- **Steps:** `atomik-play --stats --duration 600 <project>` on the default output.
- **Expected:** the command exits with 0 after ten minutes; the summary shows `cb_p99_us` under the
  buffer period and no xruns.
- **Automated:** none
- **Known bad:** a non-zero exit, or a callback p99 above the buffer period.

### HW-AUD-04 Input monitoring
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** an input device with a signal (a microphone or a line source).
- **Steps:** arm an audio track, set Auto monitor, play a note or speak, then record and stop.
- **Expected:** the input is heard while stopped or recording and not doubled over the clips during plain
  playback; the meter follows the signal.
- **Automated:** `crates/play/tests/hw_cable.rs`
- **Known bad:** a doubled signal during playback, or a monitor that stays silent.

### HW-AUD-05 A five-minute recording loop
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-AUD-04 passed.
- **Steps:** record for five minutes in loop mode with takes, stop, and play the result back.
- **Expected:** every take has the length of its pass, no gap at a loop wrap, and the file plays
  without clicks.
- **Automated:** `crates/app/tests/hw_e5.rs`
- **Known bad:** a short take, or a click at a pass boundary.

### HW-AUD-06 Change the sample rate of the device
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a device that offers 44.1 and 48 kHz.
- **Steps:** play, then change the device rate in the system control panel (or in Settings > Audio).
- **Expected:** the app reopens the stream at the new rate or says why it cannot; the project keeps
  its own rate and sounds the same pitch.
- **Automated:** none
- **Known bad:** a pitch shift, or a stream that stays closed with no message.

### HW-AUD-07 Unplug and replug the interface during playback
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a USB interface selected as the output.
- **Steps:** play the stress song, unplug the interface, wait ten seconds, plug it in again.
- **Expected:** the status line reports the lost device within a moment, nothing hangs, the
  engine does not crash, and the app switches to the default device or reopens the interface once it is
  back, as Settings > Audio says it will.
- **Automated:** none
- **Known bad:** a frozen window, a crash report, or sound that does not return after the replug.

### HW-AUD-08 Devices that offer only integer formats
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a device or driver mode that offers only 16 bit or 32 bit integer samples.
- **Steps:** select it and play the demo project; record a few seconds.
- **Expected:** playback and recording work through the integer fallback with no added noise.
- **Automated:** none
- **Known bad:** distortion or a refusal to open a device that works in other software.

### HW-AUD-09 Duplex open and fallback
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** an interface whose input and output are separate devices (for example a USB
  microphone with a separate built-in output).
- **Steps:** choose the input and the output separately and play with input monitoring on.
- **Expected:** both open; if one cannot, the other still works and the status line says which.
- **Automated:** `crates/audio-io/tests/hw_loopback.rs`
- **Known bad:** both devices refused because one failed.

### HW-AUD-10 A stream that stops late
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a device that is slow to stop (some USB interfaces, a Bluetooth output).
- **Steps:** play and press Stop, then quit the app while playing.
- **Expected:** Stop returns promptly; quitting never hangs for more than a few seconds and leaves
  no process behind.
- **Automated:** none
- **Known bad:** a window that stays open after quit, or a process left in the task list.

## MIDI

### HW-MID-01 A MIDI keyboard
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a class-compliant keyboard.
- **Steps:** enable the input in Settings > MIDI, play on a synth track, then record and play back.
- **Expected:** notes, velocity, pitch bend, the mod wheel and the sustain pedal arrive; no stuck note
  after the keyboard is unplugged while a key is held.
- **Automated:** `crates/midi-io/tests/hw_midi.rs`
- **Known bad:** a stuck note after unplugging.

### HW-MID-02 Several MIDI inputs
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** two inputs, for example a keyboard and a pad controller.
- **Steps:** enable both, give one the keyboard role and the other the mapping role, play both at once.
- **Expected:** each does its own job; disabling one leaves the other working.
- **Automated:** `crates/midi-io/tests/hw_midi.rs`
- **Known bad:** the second input ignored, or notes from the mapping input played.

### HW-MID-03 MIDI Out to an external synthesiser
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a hardware synthesiser or a sound module on a MIDI output.
- **Steps:** add a MIDI Out device, choose the port and channel, play a note clip, then press Stop in the middle of a held note.
- **Expected:** the synthesiser plays the notes in time; Stop sends the note-offs and nothing hangs.
- **Automated:** none
- **Known bad:** a hanging note on Stop, or notes late by more than a few milliseconds.

### HW-MID-04 MIDI clock out
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a hardware sequencer or drum machine that follows external clock.
- **Steps:** turn on the clock output in Settings > Sync, press Play at 100, 120 and 150 BPM, and change the tempo
  during playback.
- **Expected:** the machine starts with Play, follows the tempo, and stops with Stop; no drift over five minutes.
- **Automated:** none
- **Known bad:** a machine that runs at half or double speed, or drifts.

### HW-MID-05 Follow an external MIDI clock
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a hardware clock source.
- **Steps:** choose the clock input, start the source, change its tempo, stop and restart it. Then let the
  source's tempo wander (a slow swing of 10 % either way, or a tempo knob turned by hand) for a minute with a
  song of notes and an audio clip playing, and send a Song Position Pointer in the middle.
- **Expected:** the transport follows start, stop and tempo; the readout in the status line says that
  the clock is external; losing the clock stops following without a crash. The song stays on the beat of the
  source however the tempo wanders (no slow drift), the audio clips play from where the song reaches them,
  and a position pointer moves the song at once.
- **Automated:** `crates/engine/tests/song_sync.rs`
- **Known bad:** a transport that keeps running after the clock stops; a song that creeps away from a
  source whose tempo wanders.

### HW-MID-06 A control surface
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a surface that speaks the eight-fader protocol, or another with generic controls.
- **Steps:** the hand checks of the control surface section of [TESTING.md](TESTING.md#midi-mappings-and-control-surfaces):
  faders both ways, touch, V-pots, buttons and lamps, banking, jog, display, unplug and replug.
- **Expected:** every control does what that section lists; the surface recovers after a replug.
- **Automated:** none
- **Known bad:** a motor fader that fights the finger.

## Plugins

The fixed list: two CLAP and two VST3 plugins from different makers per operating system (an instrument and
an effect of each format), recorded in the report's plugin field with their versions. Use the same
list for every release so that a result can be compared with the last one.

### HW-PLG-01 Scan the plugin folders
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the fixed list installed in the default folders.
- **Steps:** open Settings > Plugins and scan.
- **Expected:** every plugin of the list appears with its format and vendor; a bad file in the folder is
  skipped and named, and the scan does not hang.
- **Automated:** none
- **Known bad:** a scan that never ends, or a plugin missing that other hosts find.

### HW-PLG-02 Load in the sandbox
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-PLG-01 passed.
- **Steps:** add each plugin of the list to a rack with the sandbox on, play notes or audio through it.
- **Expected:** sound; Settings > Plugins says the plugin runs in the sandbox; the load does not freeze the window.
- **Automated:** `crates/plugin-vst3/tests/real_plugin.rs`
- **Known bad:** a freeze while loading, or a plugin reported as in-process with the sandbox on.

### HW-PLG-03 Open and close the editor
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-PLG-02 passed.
- **Steps:** open the plugin's editor, move a control, resize it where the plugin allows, close it, open it again.
- **Expected:** the editor shows and follows the mouse, the parameter moves in the rack panel, the
  position and size are remembered.
- **Automated:** none
- **Known bad:** an editor that stays blank, or one that opens off screen.

### HW-PLG-04 Save and reload plugin state
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-PLG-02 passed.
- **Steps:** change a plugin's settings, save the project, close and reopen it, compare the sound.
- **Expected:** the sound is the same; undo steps the state back.
- **Automated:** `crates/plugin-vst3/tests/real_plugin.rs`
- **Known bad:** a plugin back at its default settings.

### HW-PLG-05 Kill the plugin process and recover
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** HW-PLG-02 passed, a task manager or `kill`.
- **Steps:** while playing, end the sandbox child process of one plugin.
- **Expected:** the rest keeps playing, the plugin's panel shows that it crashed, and reloading it restores
  its state; the app does not crash.
- **Automated:** none
- **Known bad:** silence of the whole engine, or a plugin that cannot be reloaded.

### HW-PLG-06 A hung plugin
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a plugin that can be made to hang (suspend its sandbox process with a debugger or
  a signal).
- **Steps:** suspend the child while playing, then resume it.
- **Expected:** the app stays responsive, marks the plugin as slow and then as hung, and reloads it from the
  last state at most twice.
- **Automated:** none
- **Known bad:** a frozen window, or endless reloads.

### HW-PLG-07 The real-plugin tests
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the `ATOMIK_REAL_*` variables set as documented at the top of each file.
- **Steps:** run the ignored real-plugin tests (see the hardware section of [TESTING.md](TESTING.md#hardware-and-manual-tests)).
- **Expected:** all pass for the plugins of the list.
- **Automated:** `crates/plugin-vst3/tests/real_plugin.rs`
- **Known bad:** a test that passes only with the sandbox off.

### HW-PLG-08 The sandbox on macOS
- **Applies to:** macOS
- **Preconditions:** HW-PLG-02 passed on a Mac, a signed build.
- **Steps:** load a plugin, play, kill the child, quit the app.
- **Expected:** the wake-up between the audio thread and the child works (no dropouts at 64 frames), the
  child ends with the app, and Gatekeeper does not block the child.
- **Automated:** none
- **Known bad:** dropouts only with the sandbox on, or an orphaned child.

### HW-PLG-09 The sandbox on Linux
- **Applies to:** Linux
- **Preconditions:** HW-PLG-02 passed on Linux (X11 and, separately, Wayland).
- **Steps:** load a plugin, play, kill the child, quit the app.
- **Expected:** the same as HW-PLG-08.
- **Automated:** none
- **Known bad:** dropouts only with the sandbox on, or an orphaned child.

### HW-PLG-10 Editor placement under Wayland
- **Applies to:** Linux
- **Preconditions:** a Wayland session.
- **Steps:** open the editors of the list.
- **Expected:** each editor opens as a window of its own and is usable, or the app says that the plugin
  cannot show an editor on Wayland; none opens in the wrong place with no way to move it.
- **Automated:** none
- **Known bad:** an editor with no title bar that cannot be moved or closed.

## Windows and displays

### HW-WIN-01 The detached windows
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the detached windows checklist of [TESTING.md](TESTING.md#detached-windows).
- **Steps:** run that checklist: detach the rack and the mixer, move, resize and dock them, quit and restore.
- **Expected:** every line of the checklist passes.
- **Automated:** none
- **Known bad:** a window that restores off screen.

### HW-WIN-02 Two monitors at mixed scale
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** two monitors at different scale factors (for example 100 and 150 percent).
- **Steps:** drag the main window and a detached window across, draw, drag a knob and drop a file on each side.
- **Expected:** text stays sharp, the pointer lands on what it is over, and a window keeps its size
  relative to its content.
- **Automated:** none
- **Known bad:** a blurred window after the move, or a click that lands offset.

### HW-WIN-03 Text entry: clipboard and input methods
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** an input method installed (a Japanese or Chinese one).
- **Steps:** in a project name field and the browser search, type with the input method, select, copy and paste;
  paste text from another program.
- **Expected:** composition shows inline and commits; the clipboard round trip keeps non-Latin text.
- **Automated:** none
- **Known bad:** a composition that commits as raw letters.

### HW-WIN-04 Application zoom and theme
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** none.
- **Steps:** the hand checks of the theme, zoom and Help menu section of [TESTING.md](TESTING.md#theme-zoom-and-the-help-menu).
- **Expected:** every line of that list passes in both themes at 75, 100, 150 and 200 percent.
- **Automated:** none
- **Known bad:** a dark patch in the light theme.

### HW-WIN-05 Dragging between song windows
- **Applies to:** Windows, macOS (Linux on X11; Wayland gives no window positions)
- **Preconditions:** two songs open in windows of their own (Window > New Song Window), the second at another
  sample rate (File > New at 44.1 kHz) when the first is at 48 kHz, an audio clip in the first.
- **Steps:** drag the audio clip out of the first window and let go over the arrangement of the second; drag a
  synth by its plate out of the first window and let go over the rack of the second; drag a patch row of the
  file browser from one window over the other and let go on the rack there; undo in the second song.
- **Expected:** the first song keeps what it had; the second gets the clip (a moment later, converted to its rate,
  playing at the right pitch and length), the synth and the patch, each as one undo step; a patch hovering over the
  other window outlines the device it would load into; nothing moves in the song the drag started in.
- **Automated:** `crates/app/src/workspace_tests.rs` (the drops, without OS windows)
- **Known bad:** a clip that sounds an octave off after the drop, a drag that leaves the clip moved in the first
  song, or a drop that lands in the song the audio does not belong to.

## Listening

### HW-LIS-01 Listen to the demo renders
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** good headphones or monitors.
- **Steps:** render the slice demo and the demo projects (the listening check of [TESTING.md](TESTING.md#listening-check))
  and listen to each from start to end, then play the same live on the device.
- **Expected:** no clicks, zipper noise on moving controls, or aliasing; the live sound matches the render.
- **Automated:** none
- **Known bad:** a click at a loop or a clip boundary.

## Installers

### HW-INS-01 Install and uninstall on a clean machine
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** a clean virtual machine or a user account that never ran Atomik Rack.
- **Steps:** install the release artifact (installer, disk image or package), start the app, open
  a demo project, then uninstall.
- **Expected:** it starts first time with no missing libraries; the uninstall leaves no program files;
  user settings and projects stay.
- **Automated:** none
- **Known bad:** a missing runtime on a clean machine.

### HW-INS-04 The AppImage
- **Applies to:** Linux
- **Preconditions:** the `.AppImage` of the draft release on a desktop distribution that is not Ubuntu (and once on
  Ubuntu), with a sound card or a MIDI keyboard if one is at hand.
- **Steps:** `chmod +x` the file and start it from the file manager and from a terminal; open a demo project and play
  it; open Audio Settings and pick the device; check the title bar, the task bar icon and the name under Help >
  About; unpack it with `--appimage-extract` and run `squashfs-root/usr/bin/atomik-render` on a demo project.
- **Expected:** it starts first time with no missing libraries, sound plays, the icon and the name are the app's
  (not a generic one), and the render writes a WAV that is not silent. A distribution without FUSE 2 runs it with
  `APPIMAGE_EXTRACT_AND_RUN=1`.
- **Automated:** none
- **Known bad:** a base system without ALSA or fontconfig, a C library older than the one of the build machine.

### HW-INS-05 The Flatpak
- **Applies to:** Linux
- **Preconditions:** the `.flatpak` bundle of the draft release, Flatpak installed with Flathub added, a Wayland
  session and an X11 session if both are at hand, a MIDI keyboard if one is at hand, and a VST3 or CLAP plugin in
  `~/.vst3` or `~/.clap`.
- **Steps:** `flatpak install --user` the bundle and start it from the application menu and with `flatpak run
  io.github.atomikfactory.AtomikRack`; open a demo project from `~/Music` or `~/Documents` and from another folder
  of the home folder; save a song in Documents; play it and pick the audio device; open the MIDI input list; scan
  plugins and load the one in the user folder; read the update notice (Help > Check for Updates against a newer
  release list); uninstall.
- **Expected:** it starts with the right icon and name on Wayland and X11, sound plays, MIDI ports are listed, files
  open from the home folder and save to Music and Documents (and not elsewhere), the user plugin is found and runs,
  system plugin folders under `/usr` are not found (a known limit), the update notice says to update with the
  software centre, and the uninstall leaves the projects.
- **Automated:** none
- **Known bad:** a runtime older than the one the programs were linked against, a window without an icon.

### HW-INS-02 SmartScreen and Gatekeeper
- **Applies to:** Windows, macOS
- **Preconditions:** the downloaded artifact, with the quarantine flag it gets from a browser.
- **Steps:** open it the way a user does; record what the system says.
- **Expected:** the behaviour matches the signing state of the release notes (a signed and notarised build
  opens without a warning; an unsigned one shows the documented warning and can be opened).
- **Automated:** none
- **Known bad:** a signed build that is blocked, or a notes entry that says signed for an unsigned build.

### HW-INS-03 The version and the signing state
- **Applies to:** Windows, macOS, Linux
- **Preconditions:** the installed program.
- **Steps:** run each program with `--version`; verify the signatures and checksums as in [RELEASING.md](RELEASING.md#verifying-a-download).
- **Expected:** the version is the tag's; the checksums match; the signatures verify where the release
  notes say they exist.
- **Automated:** none
- **Known bad:** a version string that differs from the tag.
