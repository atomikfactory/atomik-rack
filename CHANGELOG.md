# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, any release
may break APIs and the patch and project formats.

## [Unreleased]

### Added
- **A built-in manual.** Help > Manual (`help.manual`, Ctrl+F1, Cmd+Alt+F1 on macOS) opens the user documentation in a
  large panel that works offline: a table of contents built from the headings on the left, the page on the right with
  headings, paragraphs, bold and italic text, code, lists, tables and links between sections (a picture shows its
  description), a search field with ranked results (a heading match above a body match) and a line around each match
  with the matched words in bold, and Back and Forward buttons (Alt+Left and Alt+Right). The text is `docs/README.md`
  and `docs/KEYBOARD.md` compiled in, so the manual and the documentation are one source; a new Manual section and a
  Devices section in `README.md` describe it. The More link of Help Mode now opens the manual at its section instead of
  the browser, the command palette lists "Manual: <heading>" for every section, and a link to a page the manual does not
  hold (the device reference, the other documents) opens that page on the project site, one request to a fixed address.
  Help > Documentation still opens the project page. The Markdown reader is written in the project (decision V).
  The project format is unchanged.
- **Backup copies in a second folder.** Settings > Backup has a Backup folder (None by default, with Choose... and
  None), Keep (the copies of each project that stay, 1 to 50, default 10) and Include audio (default on). Every
  successful save also copies the project to `<backup folder>/<name>/<name> <YYYY-MM-DD HHMMSS>.atomik` (UTC) and, with
  audio, the audio files it uses to `<backup folder>/<name>/Audio/`, once each by content hash. The copy runs on its own
  worker after the save is confirmed, never delays or fails a save, and a failure shows `Backup failed: <reason>` once
  per session (later ones go to the log). Autosave snapshots make no backup. Only the newest Keep copies stay; an audio
  file goes only when no kept copy names it (every project file of the folder is read first, and an unreadable one keeps
  all the audio), and files that are not copies are never touched. File > Restore from Backup lists the copies of the
  open project with their date and size and opens the chosen one as a new unsaved song, after the question about
  unsaved changes. The command is `file.restore_backup` (no default key); the settings are `backup_folder`,
  `backup_keep` and `backup_audio` in `workflow.json`. The project format is unchanged.
- **Share a project as one file.** File > Share Project opens a small panel with the title, the author and a note (they
  go into the song information as one undo step when you changed them) and three check boxes: Include a preview (an
  MP3 of the whole song at -14 LUFS, or a 16-bit WAV file when FFmpeg is not set up), Include stems (one 24-bit WAV
  file per mixer strip) and Include plugin list. A line says what the share will weigh and turns to the warning
  colour over 500 MB. SHARE asks for a file name (`<title>.atomikz`, the project archive extension) and writes, on a
  worker with progress and cancel, one file with the project, every audio file it uses, `credits.txt` (title, author,
  note, the date, the version of the program and the plugins with vendor and format), the preview and the stems.
  A cancel or a failure leaves no file. File > Open and a drop on the window take an `.atomikz` file: a folder
  dialog asks where to unpack it (it starts in `Shared` beside the library folder), the project opens and the Song
  Information shows what the sender wrote. The archive reader accepts only the project, its audio, the credits, one
  preview and the stems, and still refuses entries that leave the folder, duplicates and anything over its limits
  before it writes a byte. The command is `file.share_project` (no default key). The project format is unchanged.
- **A plugin manager.** Options > Plugin Manager opens a table of every plugin the scan found: its name, vendor,
  format (VST3 or CLAP), category (instrument, effect or note effect), status (OK, Failed with the reason,
  Quarantined, Disabled, Unsupported), file and when it was last scanned, with the files that failed to scan as rows
  of their own. Click a column head to sort (again to reverse it); the search field matches the name, vendor, path and
  tags, and four buttons filter by format, status, category and tag. The star of a row marks a favourite: the device
  browser lists favourites first, in a Favourites group of their own and at the top of their kind. Tag the chosen
  plugins from the small field (the same tags as the file browser: no `#`, no comma, 32 characters at most) and find
  them in the browser by typing `#` and the tag in its search field. Disable hides the chosen plugins from the
  browser and the menus; a project that uses one still loads it. A plugin whose process ends, or that fails to
  load, three times in a row is quarantined (hidden like a disabled one, with a message once); a working instance, or
  a load that works after a failed one, starts the count again, and Try Again clears the quarantine. Rescan All and
  Rescan Selected run on the scan worker with a progress line and a Cancel button, and Show in Folder opens the file
  manager at the file. Favourites, tags, disabled plugins and the quarantine are kept by the plugin's id (so a plugin
  that moved keeps them) in `plugins_user.json` in the settings folder; a missing or damaged file reads as empty.
  The scan cache now also keeps when each file was scanned. The command is `options.plugin_manager` (no default key).
- **Soft takeover and knob pages for controllers.** An absolute knob or fader of a controller set up with the wizard
  no longer makes a parameter jump: when it is not where its parameter is (after another device is selected, a page
  or bank change, or an edit with the mouse), moving it does nothing until it comes within 2 percent of the value or
  sweeps through it, using the same rule as MIDI learn, and the status line says which way to turn it ("Turn knob 3
  up to pick up Cutoff"). Relative encoders, stepped parameters and faders that receive their value as feedback do
  not wait. The eight knobs now have pages: Page Down and Page Up (two optional keys the wizard asks for last), or
  the commands Controller Knobs: Next Page and Previous Page (`controller.knob_page_next` and
  `controller.knob_page_prev`, no default keys), move over the parameters of the followed device eight at a time and
  wrap round; the status line names the page ("Knobs: Filter 9-16") and motorised or LED controls receive the new
  values. The page goes back to 1 when another device is followed, unless the profile keeps the page for each device.
  Pick up values (on by default) and Keep the knob page for each device (off by default) are check boxes on the last
  page of the wizard and chips in Settings > Controllers; they are saved in the profile file, and older profiles
  read as the defaults.
- **Editing note expression by hand, and MPE out.** The strip beside the selected note in the piano roll is now an
  editor for its pitch, pressure and timbre curves. A tab picks the curve; a click on empty space adds a point, a
  drag moves it (Shift moves finely, Ctrl snaps a pitch to a semitone; time stays inside the note and the value
  inside the range), Alt-click or a double click deletes it, a drag from empty space is a rubber band that selects
  points to move or delete together, and with the Pencil tool a drag draws the curve freehand, thinned like Cleanup
  Automation. A scale chip steps the pitch plot through 48, 24, 12 and 2 semitones. Apply copies the curve to the
  other selected notes, stretched to each note's length. Every gesture is one undo step and a curve keeps its limit
  of 1024 points. The command Edit Note Expression (`editor.edit_expression`, found in the command palette) shows
  the strip for a note without curves and with several notes selected; Apply Curve to Selected Notes
  (`editor.apply_expression_curve`) copies a curve. The MIDI Out device
  has an MPE zone (off, lower or upper) and a member channel count: with it on, every note goes out on a member
  channel (taken in turn, the one released longest ago first), its curves as that channel's pitch bend, pressure and
  CC 74, the first values before the note on, and the MPE Configuration Message when the zone is set and at each
  playback start; with it off the output is as before. A plugin device has a Send MPE option (the MPE button on its
  panel) that sends a lower zone as raw MIDI on channels 2 to 16 to a plugin that takes plain MIDI; plugins that take
  note expressions still get them as note expression events without it.
- **Help Mode.** Help > Help Mode (Shift+F1; F1 is still the shortcut list) docks a Help panel above the status line
  that explains the control under the pointer: its title, one to four sentences, the shortcut of the command it runs
  (read from your keymap, so a rebound key shows) and a "More: <section>" link that opens the matching section of the
  documentation in the browser. Clicks still reach the controls, the ordinary tooltips stay quiet while the mode is on,
  and the panel keeps the last control it explained while the pointer is over empty space or over the panel, so the
  link can be reached. Help text is written for the transport bar, the arrangement tools and lane buttons, the mixer
  buttons and sections, the file browser, the piano roll tools, the launcher and the buttons of the rack panels. Every
  knob, switch and choice of every device explains itself from its descriptor (its name, its range and unit) plus a
  line about its device, and every device type has such a line. A control with no entry yet falls back to its tooltip
  text. The text lives in two data files (`crates/app/src/help/`), and a test lists the controls that still lack an
  entry.
- **Genre templates.** Six songs to start from ship with the program and come first in File > New from Template and on
  the start screen: Ambient, Hip-Hop Boom Bap, House, Lo-fi, Synthwave and Techno. Each has a tempo, a song key, eight
  sections over 64 bars as ranges, six named and coloured tracks (kick, drums, bass, chords, lead and a texture) of
  factory patches, kit pads and clips, a filled launcher, two sends (a space reverb and an echo), the strip EQ on the
  bass, a duck from the kick, a glue compressor and a -1 dBTP limiter on the master aimed at -14 LUFS, and song
  information. Your own templates follow them in the list.
- **Export presets.** One choice exports for a purpose. The export panel has a Preset row (a drop-down, Save Preset
  and Delete Preset) and File > Export for runs a preset at once: Streaming (a 24-bit WAV and an MP3 at 320 kbps at
  -14 LUFS, true peaks under -1 dBTP), Podcast (a mono MP3 at 128 kbps at -16 LUFS), Video (AAC at 256 kbps and
  48 kHz at -14 LUFS, or the film with that mix when the song has one), Stems for mixing (a 24-bit WAV per strip,
  no loudness change, no dither) and Club (a 24-bit WAV with -9 LUFS as a guide only, no limiter added, and a
  warning in the report if the peaks clip). Choosing a preset fills the panel's fields, which stay editable (the
  name shows "(changed)"); Save Preset keeps them under a name of your own in `export_presets.json`. The panel has
  new rows for the mono mix-down, the loudness (off, -16 or -14 LUFS, or a -9 guide) and an extra MP3 next to a
  lossless file. A loudness target measures the render, turns it up or down and, when the peaks would pass the
  ceiling, holds them with the true-peak limiter of the Mastering device; the toast says the level before, what
  was done and any limiting. The presets that write MP3, AAC or Opus are greyed with "Needs FFmpeg" without it.
  A mono MP3 is now written as one channel.
- **Quick sidechain: Duck from.** One step makes a track duck under another, such as a bass under a kick or a pad
  under a vocal. The right-click menu of a track header, the rack menu of a track's device and the mixer's strip menu
  have a Duck from submenu (and Track > Quick Sidechain has Duck from...) that lists the other tracks by name and in
  their colour, at most thirty and then More... for a pop-up with all of them. Choosing a track puts a Compressor
  named "Duck from <track>" at the end of the ducked track's insert chain, with its key cabled from the source's
  post-fader output (Duck from (pre-fader) takes it before the fader) and set to 4:1, 5 ms attack, 120 ms release
  and -24 dB threshold, in one undo step; the toast says "Bass ducks under Kick. Adjust: Threshold on the
  Compressor." Choosing the same pair again selects the device ("Already ducking from Kick"), a source that the
  track feeds is refused as a loop, Remove Ducking takes every ducker off in one step, and Gentle Pump puts a
  quarter-note pump with no key on the track. Nothing is added to the project file: the device name marks the ducker.
- **AppImage and Flatpak for Linux.** The release and nightly builds now also produce
  `atomik-rack-<version>-x86_64.AppImage` and `atomik-rack-<version>.flatpak` (a single-file bundle), each with a
  checksum in `SHA256SUMS`, beside the `.tar.gz` and the `.deb`. All four hold the same programs. One desktop entry,
  one AppStream file (with a releases list made from the version) and the icons are installed under the app id
  `io.github.atomikfactory.AtomikRack` by the `.deb`, the AppImage and the Flatpak, and the program uses that id as
  its window class so the desktop shows the right name and icon. The AppImage takes the sound, font and graphics
  libraries from the host; the Flatpak runs on `org.freedesktop.Platform` with Wayland and X11, GPU, sound, MIDI
  and read-only access to the home folder, with `~/.vst3` and `~/.clap` named (plugin folders under `/usr` are not
  visible in it). In a Flatpak the update notice says to update with your software centre. Pull requests check the
  scripts, the manifest, the desktop entry and the metainfo; the packages are built only by the release workflows.
- **CPU use per device.** The engine now times every device all the time (about half a percent of a 64-frame callback
  for a hundred devices, never part of the sound), and the figures are averaged over about a second with a peak held for
  two seconds. View > Show Device CPU (the old CPU Meters switch, off at the start) shows a figure such as `3.2%` on the
  name plate of each device, in the warning colour above 25 % of the audio time, and the total of its track (instrument,
  channel and insert effects) on each channel strip of the mixer. The tooltip of the status line gives the engine load
  and lists the five heaviest devices with their tracks. When the engine load stays above 70 % for five seconds while
  playing, a card says which track uses the most CPU and offers to freeze it with one click, once per track per song.
- **Chord track and song key.** The song can have a key and a chord lane that the rest of the app follows. A key is a
  root and a scale kind (every kind of the piano roll but chromatic); key changes along the song work like meter
  changes, and the key at a tick is the last change at or before it, else the song key. A chord has a root, a quality
  (the 21 qualities of the chord stamp), an optional bass note and a span; chords never overlap. View > Lanes and
  Markers > Key Lane and Chord Lane show two rows above the track lanes, under the tempo and energy lanes. A click on
  the empty lane adds a key change (the key already in effect) or the tonic chord of the key there, a bar long, and a
  drag from the press sets its length; a drag moves a tag or a chord, the right end of a chord resizes it and neither
  runs into the next chord; a double click opens a picker with the root, the quality (or the scale) and the bass note
  and a text field for a chord symbol such as `Am7`, `F#m7b5`, `C/G`, `Bbmaj7` or `Dsus4/A`, read by a small parser
  that takes the common spellings (`m`, `min`, `-`, `maj7`, `M7`, `dim`, `sus`, `add9`, flats and sharps in letters or
  signs) and says what it cannot read; the right button lists Delete and Copy, and Paste on the empty lane. Every
  gesture is one undo step and Escape puts a drag back. The transport bar has a Key field with the key at the playhead
  (a click sets the song key; it moves into the overflow list on a narrow bar, where Song Key... is a row) and
  Transport > Song Key... and Detect Song Key do the same from the menu. A clip without a scale of its own follows the
  song key at its start in the piano roll (highlighting, snap to scale and the fold), and a scale on the clip still
  wins. The chord stamp has a Song Chord entry that drops the chord of the lane at the place drawn (with its bass note
  below), and Fit Notes to Chords moves the selected notes that fall outside the chord at their position to the
  nearest chord tone, a tie going down and never onto a note that is there already, in one undo step. Detect Song Key
  reads a pitch-class histogram of every note clip (a note counts as much as it is long) against the standard major
  and minor key profiles, sets the best of the 24 keys as one undo step and says in the status line how clear it was;
  a toast holds no buttons, so Undo is the way back. In the browser, `key:song` in the search finds the loops tagged
  with the song key (`A minor`, `D dorian`; flats match too, and a song without a key filters nothing), and
  `key:minor` finds a part of a tag. Format 45, the song fields `key`, `key_changes` and `chords` are written only
  when set and are ignorable, so the version and the compat stay; every saved file lists them in its `ignorable`
  header, so the example projects, the factory files and the format fixtures were saved again. Command count 538.
- **Audio and MIDI setup.** The first time Atomik Rack starts (no `settings.json` yet) a short panel opens before the
  start screen and gets sound working in three steps. Sound output lists the audio hosts, the output devices and the
  buffer sizes (the same lists as Audio Settings, with the hint "smaller = less delay, more CPU"), a Test button plays
  a soft sine chord for one second at -18 dBFS through the engine, and the output meter beside it moves when the sound
  reaches the device; the output latency is shown in milliseconds, as the stream reports it or as an estimate from
  the buffer when the host does not say. If the device does not open, a plain message says why and points to another
  one. Input is optional: an input device list, a live input meter and a Turn On Input button for a project with no
  input channels. MIDI keyboard lists the MIDI inputs with a check box each, lights a "note received" lamp and plays a
  soft tone for every key, and Set Up Controller... opens the controller wizard. Next, Back, Skip and Done move
  through it; Skip and Done both write `settings.json`, so it never opens by itself again, and each choice is applied
  at once through the same path as Audio Settings, which writes the same file. The question about checking for new
  versions waits until the setup is closed. Options > Audio and MIDI Setup... (`options.audio_setup`) opens it again
  at any time. The engine now publishes the loudest output and input sample of each batch (the preview voice and
  the click included). Format unchanged (45), command count 533.
- **Clip launcher.** View > Launcher (F10) puts a grid where the rack was, the arrangement below it: one column per
  note track, one row per scene, a launch triangle in each slot, a scene column with a launch button and the scene's
  name, a stop square under each track and STOP ALL. A slot holds a note clip with its own loop length, name, colour and
  launch quantize; a double click opens it in the piano roll (an empty slot gets a one-bar clip), a drag moves it and
  Ctrl-drag copies it, and the right-click list renames, colours, sets the loop length and the quantize, duplicates and
  deletes. A launch waits for the next boundary of the quantize (None, 1/16, 1/8, 1/4, 1/2, 1, 2 or 4 bars, 1 bar by
  default, set for the song or per slot), exact through tempo ramps and meter changes, and then the clip loops from its
  own start while its track's arrangement stays silent, until the slot stops or Back to Arrangement (at once) gives the
  track back. Launching with the transport stopped plays; notes that are on when a slot stops or switches get their
  note-offs. A scene launches every slot of its row and, with Scene Stops Empty Tracks on (the default), stops the
  tracks that have nothing in it. Record Launches (a switch in the view and a command) makes the Record button write
  what was launched into the arrangement, at the song ticks it played, as looped clips, and the whole pass is one undo
  step; it needs no armed track. The Launcher menu under View has the commands, Clip to Launcher and Launcher Clip to
  Arrangement move a clip between the two by command, and Enter and Shift+Enter launch the slot and the scene under the
  cursor. Audio tracks have slots too: an audio clip (its file, gain and fades) plays unwarped, looped over the length
  of the slot, with a small waveform in the slot; an audio file dropped on an audio slot (a MIDI file on a note slot)
  makes its clip. A clip dragged from the arrangement into a slot copies it, and a slot dragged onto an arrangement lane
  writes a clip there at the snapped tick (each one undo step). Not done yet: warped audio in slots (it plays at the
  file's own tempo), launching from a controller, follow actions and scene tempos that change the song tempo. Command
  count 532.
- **Project format 45.** A reader of format 44 refuses a file of 44 that holds a field it does not know, so the
  launcher and the per-note expression, added after 0.4.0-rc.1, could not be skipped by it. Files now say version 45
  with `"compat": 44` and list those fields as ignorable: a 44 reader opens them, drops the launcher and the note
  expression with a warning each, and plays the rest. The version-45 corpus file and demo fixture are added.
- **Undo History panel.** Edit > Undo History (the command `edit.history`, no default key) docks a list of the edits
  made to the song on the right of the window, left of the Track Panel when both show. The first row is the state the
  song started in ("Opened", or "New song"), each row after it is one undo step with its name and how long ago it was
  made ("2 min ago"), oldest at the top. The row the song stands on is highlighted and the steps after it, the ones
  Redo would bring back, are dimmed. A click on a row undoes or redoes as many steps as it takes to reach the state
  right after that step, through the same Undo and Redo the keys run; a click on the first row undoes everything. With
  the list focused, Up and Down step one undo or redo, and Home and End go to the first and the last row. The list
  scrolls and keeps the current row in view, follows every edit, undo and redo (also the ones made with the keys while
  it is open) and starts again with a new or opened song; each song window has its own. Whether it shows is kept for
  the session. The times are not saved. Format unchanged (44), command count 521.
- **Command palette.** Help > Command Palette (Ctrl+Shift+K, Cmd+Shift+K on macOS) opens a floating panel with a search
  field and a list of every command: the menu items as the menus show them now, with their menu path, and the other
  commands the keys reach, under their group. The search takes several words and matches the name, the menu path and
  the id, by whole words first and by scattered letters after that, and the matched letters are bold. Each row shows
  the key the live keymap gives the command, so remapped keys show; a command that cannot run now is dimmed and still
  says why when run. Up, Down, Page Up and Page Down move, Enter or a click runs the command after the palette has
  closed (on the pane that had the focus), Escape or a click outside closes it, and typing never reaches a shortcut.
  The last eight commands run from the palette come first, and are kept in `palette.json` next to the settings.
  Format unchanged (44), command count 520.
- **More factory content.** The Factory root grows from 49 to 79 patches, from 12 to 36 note clips and gains 12
  audio loops (the addition is about 8 MB in git; a test keeps the whole folder under 25 MB). Four new drum kits
  (Timber Kit, acoustic-like; Circuit Kit, electronic; Cassette Kit, lo-fi; Bazaar Kit, hand percussion) have 14 or
  15 pads each on keys 36 to 50, rendered by the writer from the drum machine's pad modules. Thirty patches are
  new: more for the FM synth, dual layer, organ, semi-modular synth, grain sampler, mono bass and wavetable synth
  (at least five each now), and the first for two reverbs, an echo, modulation, distortion, the auto-filter and the
  rotary speaker. The 24 new clips are eight drum grooves (from 70 to 174 BPM, in straight and triplet time), six
  bass lines, five chord progressions and five arpeggios. The 12 loops are 24-bit stereo FLAC files rendered from
  clips by a factory kit or patch; each plays its clip twice and keeps the second pass so it repeats without a
  gap, and carries a tempo tag (`88 bpm`) and, when pitched, a key tag, so the browser's tag filter finds them.
  The Clips section of a library, and of the factory, now lists audio files next to note clips. Everything is
  original and rendered by the application's own devices; the writer is still test code
  (`ATOMIK_WRITE_FACTORY=1 cargo test -p atomik-app --lib write_the_factory_library -- --ignored`), and tests check
  that every file is what the tables describe, that each patch and kit makes sound and that each loop decodes at its
  tagged tempo. Format unchanged (44), command count unchanged.
- **MPE: per-note expression.** A MIDI input can be switched to MPE in Settings > Audio > MIDI inputs with MPE
  (a click steps through off and a lower or upper zone of 15, 7 or 4 member channels). The zone is the one set
  there until the controller sends an MPE Configuration Message, which then replaces it (a zone of 0 members
  removes it). On a member channel the note on and off are ordinary notes, and the pitch bend (48 semitones until
  RPN 0 says otherwise), the channel pressure and CC 74 of that channel are the expression of the note sounding
  there, not of every note: what a channel holds before the note on starts the next note. Recording keeps them
  with the note as three curves, pitch in semitones, pressure and timbre, a burst closer than about 5 ms folded
  into one point and at most 1024 points per curve; Layer mode, loop passes, input quantize, splits, joins and
  loop conversion carry them with the notes. The poly synth (and so the mono bass and the dual layer), the
  wavetable synth and the FM synth play them: the tuning moves that voice's pitch, the pressure raises its gain by
  up to 6 dB (it is the pressure source of the poly synth's modulation slots), and the timbre opens the cutoff of
  the poly synth by up to three octaves, moves the frame position of the wavetable synth and adds up to one to
  the modulation index of the FM synth (the other instruments take a note's pressure as their pressure source and
  ignore the rest). A note with no expression sounds exactly as before. Seeks and loop wraps inside a note send
  the value in force, and a bounce is the same at every block size. MIDI files keep it: a clip or track with
  expression is written as an MPE track (the zone announced on channel 1, the notes on member channels, each
  note's bend, pressure and CC 74 on its own channel) and a file that announces a zone is read back into notes
  with their expression. In the piano roll the selected note shows its curves in a small strip under it with a
  button that clears them (one undo step). **No project format change**: a note clip may carry an optional,
  ignorable `"expression"` field (under format 44), written only while a note has curves. Not done yet: editing
  the curves by hand, expression for the other instruments, MPE member channels to plugins that take MPE MIDI and
  to the MIDI Out device (CLAP and VST3 plugins already get note expression events), and an expression that
  follows a note when the note is moved or retuned (it is kept by the note's start and key).
- **Controllers that just work.** Settings > Controllers > Set Up Controller... walks through a controller:
  pick its MIDI input port, then move knob 1 to 8, fader 1 to 8, the master fader and press Play, Stop,
  Record, Loop and the two bank keys, with Skip, Skip Group, Back and Finish for what the keyboard lacks. It
  tells 7 bit and 14 bit controllers, NRPN, notes and pitch bend apart and whether a knob reports its position
  or is an encoder (three codings), asks to be turned both ways when it cannot tell, and ignores a control it
  already has. The result is a controller profile, a small JSON file in the `controllers` folder next to
  `settings.json` (the format is in the architecture notes), remembered by the MIDI port name, and set up as a
  surface of its own. The first eight knobs drive the first eight parameters of the selected device (or the
  device the surface is locked to) and follow the selection, the faders are the levels of the eight visible
  mixer strips and bank left and right move eight at a time, the keys are play, stop, record and loop. When the
  controller has an output port the wizard asks whether its knobs, faders and keys show values, and the
  profile then sends them back (LED rings, motorised faders, lit keys), only what changed and never against a
  hand on the control. Profiles are exported and imported from the same tab to share them, and several
  controllers work at once, each with its own profile, bank and lock. No project format change (profiles
  live with the settings, not the project).
- **An optional check for a newer version.** The first time the program starts it asks "Check for new versions once a
  day?" in a small notice in the lower left corner (over the start screen too); until you answer Yes nothing is sent.
  A Yes asks the project page once a day, at the start, for its list of releases; Help > Check for Updates asks at once
  (and shows the question first while it is open). The request is one HTTPS GET of a fixed address with the User-Agent
  `atomik-rack/<version>` and nothing else: no identifier, no cookie, no query, no redirect, a short timeout. It runs
  on a worker, and a failure is silent unless you asked. A newer version (release candidates in the right order; a
  pre-release only for a pre-release build; never a draft) shows a notice with What's new (its release page in the
  browser), Skip this version (a later version still shows) and Later. Nothing is downloaded or installed. The answer
  is in Settings > Video & updates (`update_check` in `workflow.json`), the time of the last request and the skipped
  version are in `updates.json`. The README and `SECURITY.md` say what is sent; the client is the new crate
  `atomik-update` (ureq on rustls with the bundled root certificates; `CDLA-Permissive-2.0` joins the allowed
  licences). The project format stays 44.
- **Film with Mix.** File > Export > Film with Mix... writes a new film made of the song's film and the song's mix,
  through the FFmpeg you installed. The film's span is exactly the part of the song under the film after its trim and
  start (a film that begins before the song gets silence in front of the mix); the whole song renders from the start
  to the End with black pictures before and after the film. The mix's first sample sits at the first exported frame.
  The picture is copied without re-encoding when the cut is at the film's start (AUTO), always (COPY, which cuts at
  keyframes: the dialog says so, and RE-ENCODE cuts exactly) or re-encoded to H.264 at a high, standard or small
  quality; black before or after the film needs a re-encode. The mix goes in as AAC at 128 to 320 kbit/s in an MP4
  file or as 24-bit PCM in a MOV file, and the film's own sound can stay as a second track. It runs on the export
  worker with progress and a cancel that leaves no partial file (an existing film is replaced only by a complete
  one), and the menu item is greyed with the reason in the toast when the song has no video, the film's file is
  missing or FFmpeg cannot be used. The choices are remembered in `export.json`. The project format stays 44.
- **MP3, AAC and Opus export.** The export panel has MP3, AAC (`.m4a`) and Opus buttons next to WAV, AIFF and FLAC
  and a QUALITY row: MP3 at 128, 192, 256 or 320 kbit/s or variable quality V0, V2 or V4, AAC at 128 to 320 and Opus
  at 64 to 192 kbit/s. They encode through the FFmpeg you installed, so no codec ships: the song, the loop and the
  selection are rendered to a temporary WAV file and FFmpeg encodes it on the export worker, with progress and a
  cancel that removes the temporary and partial files (an existing file is replaced only by a complete one). The
  song's title and author go into the file's tags as arguments, never through a shell. Without FFmpeg the three
  buttons are greyed with a line that says to set its folder in Settings > Video or put it on your `PATH`; stems stay
  WAV, AIFF or FLAC. The loudness in the toast is measured before encoding, so leave about 1 dB of true-peak
  headroom. The last format and quality are remembered in `export.json`. The project format stays 44.
- **A start screen that teaches.** A strip of cards sits above the lists of the start screen: the three factory
  example songs, each with a line about what it is for (a click opens one as a new unsaved song, like a template),
  and a Make your first song card. A check box at the foot of the box turns that card off; Help > Guided Tour starts
  the same guide whenever you like.
- **A guided first song.** Make your first song (the start screen card or Help > Guided Tour) opens a new song and
  shows a small floating panel with seven steps: add an instrument, play or draw some notes, loop them, add a drum
  pattern, mix (a level and an effect), save and export. Each step says what to do, watches the song and moves on
  when it is done (or waits for Next when you had done it already), and outlines the menu, button or area it talks
  about. Back, Skip, Move (to the other side of the window) and Close are always there. The panel never takes the
  keyboard, only the pointer inside its own rectangle, and what it shows is not saved in the song.
- **Drum pads light when they play.** A pad of the drum machine panel flashes when the engine plays it, whatever
  sent the note: a note generator, the sequencer, a MIDI input, a click on the pad or a gate on its jack. A harder
  hit lights it brighter and the light fades out in a quarter of a second. The drum machine counts the hits that
  sound and publishes the counters like any other device's display values; nothing on the audio thread waits for
  the panel, and a hit on a pad that makes no sound lights nothing.
- **A level for the browser's previews.** A small control in the browser's header sets the level audio files,
  patches, containers and clips play at when you preview them, from -60 dB to +6 dB (drag it, double-click to go
  back to 0 dB). It is saved with the Settings and applies to the next preview of every kind.

### Changed
- **The menu bar in every window.** The detached rack window and the detached mixer window now show the same menu bar
  as the main window (File, Edit, View and the rest), so every command is reachable from the window you work in. It
  is one menu model: the same items, check marks and enabled states in all three windows, and a toggle chosen in one
  shows its mark in the others. A command acts on the pane of the window whose bar you used: Edit > Select All, Copy
  and the others work on the rack devices in the rack window and on the strips in the mixer window. Only one menu is
  open at a time across the windows; it closes when another window takes the focus, when you press in another window
  or when the app loses the focus. The pane starts below the bar, a window is never narrower than the bar, and
  pointer positions, drags between windows and the saved window size are unchanged. The small windows (Spectrum EQ,
  Piano Keys, Video and the detached browser) keep no bar, and on macOS the system's menu bar stays the only one.
  Format unchanged, command count unchanged.
- **One text field everywhere.** Every place where you type text (names of markers, folders, tracks, clips, lanes,
  buses, groups, snapshots, devices, patterns, macros and assets, the search boxes, comments, the tempo, signature,
  position, time, locator and clip inspector entries, the note and point fields, the macro range values and the
  buffer size) is now the same field, with a caret, a selection by Shift and the arrows or the mouse, a double click
  on a word, Ctrl/Cmd+A, C, X and V, Home, End, word moves with Ctrl, input methods and an undo of its own:
  Ctrl/Cmd+Z undoes, Ctrl/Cmd+Shift+Z or Ctrl+Y redoes, a run of letters is one step and 50 steps are kept. While a
  field has the keyboard these chords edit the field and never run the song's Undo; with no field, Ctrl+Z is the
  song's Undo as before. Number fields refuse other characters. In the plugin parameter pane, whose search field
  always has the keys, Undo is the field's only while it has typing to take back. A right-click on a name, search,
  comment or macro field opens a small menu (Undo, Redo, Cut, Copy, Paste, Delete, Select All, dimmed when they have
  nothing to do) that edits that field only. Format unchanged (44), command count unchanged.
- **Bigger drum pads.** The pad grid of the drum machine panel uses the room between the pads and the knob
  columns, and the sample name and the pad number are drawn larger.
- **A click on an empty automation lane selects the lane.** With the Select tool a click on empty lane space picks
  the lane and its track, and a drag draws a selection band, instead of adding a clip and a point. A double-click
  on empty lane space still adds a point (a clip over the bar clicked, with a point at the pointer), and the Pencil
  tool still draws.

### Fixed
- **Select All in the search boxes.** Ctrl+A (Cmd+A on a Mac) in the device browser's search, the parameter
  search and the plugin parameter search now selects the text you typed, instead of the devices in the rack. These
  boxes now have a caret you can move or click into place, and Ctrl+C, X and V copy, cut and paste their text.
- **A dropped sample names its drum pad at once.** The panel kept saying "empty" for a pad that had just received
  a sample until something else rebuilt it; the pad names now follow the zones the moment they change.
- **Undo in the middle of a band selection.** Undoing while a rubber band was open in the arrangement left clips
  in the selection that the undo had removed; the band now keeps only the clips that still exist, on release and
  on cancel.
- **Undo of an effect created inside a channel's insert chain.** Creating an effect at a rack slot above the
  chain's end joins it to the chain and rearranges the rack in the same step; the step recorded the old order
  from before the new effect moved, so undo could fail and leave the effect behind. The recorded order now
  matches the rack as it really was.
- **Sandbox crash tests on slow machines.** The tests that wait for a crashed plugin child to turn its device
  to Crashed now wait on that state against a generous deadline instead of a fixed number of batches.
- **Cables that would close a loop through the console.** Dropping a cable from a console output onto a channel
  input (or its stereo pair) used to send a cable the project refuses; the drop is now ignored up front.

### Fixed
- **A compressed or renamed export no longer replaces a file unasked.** The save dialog picks a name before the
  format is chosen in the export panel, and the file gets the extension of the format, so an existing `.mp3`, `.m4a`,
  `.opus`, `.aiff` or `.flac` (or the `.mov` of a film) at that name was overwritten without a question. The dialog now
  proposes the extension of the format used last, and an export whose format changes the name to a file that exists
  says so under the options and starts only when EXPORT is pressed again.

## [0.4.0-rc.1] - 2026-11-08

### Added
- **The film's own sound.** File > Video > Import Video Audio takes an audio track out of the song's film through
  FFmpeg, on a worker with progress in the status line and a cancel that leaves no file and no asset, and imports
  it like any audio file (converted to the song's rate, stored in the project's audio folder by its hash, with its
  waveform). The sound goes on a new audio track named after the film, in one undo step, and its first sample sits
  exactly on the film's first frame: the song frame of the film's start plus the offset the file states between its
  sound and its picture, to the sample whatever the tempo (the clip starts on the first tick at or after it and
  skips into the sound by the rest), showing only the part inside the film's trim. A film with several audio tracks
  asks which one in a pop-up (number, channels, codec, language, title); a surround track is mixed down to stereo.
  A film with no sound, or a machine without FFmpeg, says so and changes nothing. Keep Audio with Video (on after an
  import, saved with the film) makes moving, trimming and relinking the film move and trim that clip in the same
  undo step, a drag included; turned off, the clip is an ordinary one, and turned on again it is lined up with the
  film again. Import Audio with Video makes Import Video do both. Project format stays 44 (the link is an optional
  part of the ignorable `video` field).
- **A video row in the arrangement.** A song with a film shows it in a row above the track lanes: a clip bar with a
  frame thumbnail every picture-width, the frame that plays at the start of each slot, so the pictures follow the
  zoom. Only the slots in view are decoded, on a worker, with a placeholder until they arrive, and they are kept
  while you scroll and zoom. The header cell shows the film's timecode at the playhead and the bar the timecode of
  its first and last frame. Drag the bar to move the film and either end to trim it: a move snaps to the grid and
  then to a whole frame, a trim snaps its edge to the grid and the cut to a whole frame of the film (the no-snap
  key switches both off); a drag is one undo step and Escape puts the film back. Click the bar and press Delete to
  remove the video. A missing file or a film with no preview yet is a labelled bar that still moves. The chevron
  collapses the row to a thin bar and View > Lanes and Markers > Video Row hides it (both are remembered).
- **The Video window.** View > Video opens a window with the picture of the song's film at the song position:
  frame-accurate while stopped (locate, scrub, ruler clicks), following the transport while it plays (frames are
  dropped, never lag), jumping back with the loop, and locked to time when the tempo changes. Before the film starts
  and after it ends it is black. The timecode (from the film's start timecode, drop frame included) and the frame
  number show over the picture (File > Video > Video Overlay, or the TC and # buttons); Fit, 50%, 100% and 200%
  set the size with the aspect kept. The window says why when it has no picture (no film, missing file, no FFmpeg)
  and shows the progress while the preview builds, which now starts by itself when a project with a film opens.
  Frames are decoded off the UI thread by a pure-Rust JPEG decoder (`zune-jpeg`), with a decode-ahead cache.
- **A video in the song.** A song can hold one film, linked by its path (never copied: films are huge) with what the
  probe found (length, frame rate, picture size, start timecode, whether it has sound) saved in the project, so the
  project opens and shows its film without FFmpeg. The film's start and trim are kept in microseconds, not in
  ticks, so it stays locked to time when the tempo changes. File > Video has Import Video... (probes the film, puts
  it at the start of the song and builds its preview in the background; without FFmpeg it says how to set it up),
  Remove Video, Relink Video..., Move Video to Playhead and Trim Video Start / End to Playhead, each one undo step.
  A song whose film file is gone opens with the video marked missing and a toast; Relink Video points it at the file
  again and warns when its length or frame rate differs from the original. The MCP project inspect lists the film.
  Project format 44 (an optional field that older readers skip); the viewer and the thumbnails come later.
- **Video foundation (FFmpeg).** Video tracks will be decoded by an FFmpeg you install: nothing is bundled. A new
  crate `atomik-video` finds FFmpeg (the folder set in Settings > Video, else your `PATH`, else the usual package
  folders on macOS), checks that it is release 4 or later and says clearly when it is missing, too old or broken:
  video is then off and nothing else changes. It reads a film with `ffprobe` (length, frame rate with variable rate
  handled, size, rotation, start time and timecode, and the audio tracks) and builds a small proxy of the picture:
  every frame as a low-resolution JPEG in one indexed file, so any frame is one seek. The proxy is built on a worker
  with progress in the status line and a cancel that leaves no partial file, and cached per user (`LOCALAPPDATA` on
  Windows, `Library/Caches` on macOS, `XDG_CACHE_HOME` elsewhere) keyed by the film's path, size, time and its first
  and last mebibyte, pruned to 2 GiB, oldest first. File > Video has Prepare Video..., Check FFmpeg and Cancel Video
  Job; Settings has a Video tab with the FFmpeg folder and what was found (the tab strip is a little tighter for the
  seventh tab). No project format change; no film appears in the arrangement yet.
- **Stretch cursor.** A clip edge that stretches the clip (Alt held over the edge, before and during the drag) shows a
  drawn cursor, a double arrow with a bar across its middle, in place of the plain column-resize icon. It is crisp at
  1x and 2x, and pressing or releasing Alt over an edge swaps it with the resize cursor at once, without a pointer move.
- **Mix channel panel.** A console channel or output bus is a compact one-unit panel in the rack, in place of the
  generated panel of its descriptor. Its front carries MUTE and SOLO, a horizontal fader (a drag along the track, a
  double-click resets it, a readout in dB), a PAN knob, an L and R peak meter, BYPASS INSERT FX, SHOW INSERT FX and the
  AUDIO OUTPUT display (Master Section, the bus name or the Audio I/O pair). The controls drive the same params as the
  console strip, so the rack and the mixer stay in step; BYPASS INSERT FX bypasses every insert effect (and the
  dynamics and EQ sections in use) in one undo step, as the B-all button of the strip does. The plate shows the name of
  the source instrument and a rename changes it. SHOW INSERT FX is view state for now (it starts on and is not saved
  with the project); it folds the insert stack under the panel (see below). The back carries four pairs of jacks and
  nothing else: INPUT, INSERT FX TO DEVICE, INSERT FX FROM DEVICE and OUTPUT. The sends, the parallel output, the key,
  the CV inputs and the gain reduction output have no jack in the rack; they keep working and their cables stay.
  The meters of the panels are fed while the rack shows them, with or without the console.
- **Insert stack in the rack.** The insert effects of a channel or an output bus are stacked right under its panel, in
  chain order, inside a frame in the strip's colour that ends with a drop row ("drop or create an effect here"). With
  SHOW INSERT FX off the frame shrinks to one header line that names the effects; a click on it shows them again.
  Creating an effect with an insert selected puts it after that insert, and with the strip selected at the end of the
  chain; dropping a device or a browser row in the frame adds it at the place under the pointer; dragging an insert up
  or down moves it in the chain; dragging it out to the plain rack takes it out of the chain, which closes behind it,
  and leaves the effect uncabled in the rack; deleting an insert reconnects its neighbours. Each gesture is one undo
  step, and a refusal (a device that is no stereo effect, a full chain) says why in the status line. Moving a
  channel or a bus moves its inserts along, and a move never lands between the effects of another strip. In the
  mixer the insert rows of a strip reorder by drag and each has an E that shows the effect in the rack, with the
  insert area unfolded and the effect selected. New commands `AddInsert` and `ExtractInsert`. The fold state is view
  state for now and is not saved with the project.
- **Master section panel.** The console master is a compact four-unit panel in the rack. The front carries the master
  fader (the same horizontal fader, readout and meter as a mix channel panel), BYPASS INSERT FX, SHOW INSERT FX, the
  AUDIO OUTPUT display and eight send rows: the number, the name of the effect on the send (or "free"), its return
  level as a slider (a drag, a double-click resets it) and an EDIT button that selects that effect and scrolls the rack
  to it. The back carries FX SEND 1-8 and FX RETURN 1-8 (left and right jacks), INSERT FX TO DEVICE and FROM
  DEVICE, MASTER OUT and CONTROL ROOM OUT; the summing inputs, the key and the CV sockets keep working but have no
  jack in the rack, and the bus compressor and the control room stay on the mixer. The master has the same insert
  area as a channel (still after its compressor): effects stacked under it, the same gestures, one undo step each, `SetInsert`,
  `AddInsert`, `MoveInsert` and `ExtractInsert` with the master as the strip, and in the mixer its insert rows
  reorder by drag and have an E. The effects of the 8 shared sends sit under the master's insert area, each send in
  a framed group labelled "Send N" (a send with several effects in series shows them all), and they move with the
  master; they stay ordinary devices shared by all channels. Creating an effect with the master selected still makes
  the next free send (with all 8 in use the effect is added unrouted and the status line says that all 8 send and
  return slots are in use), creating one after a click on the master's insert area, with one of its inserts selected
  or by dropping it right under the panel makes a master insert, and "Use as Send Effect" brings the effect under the
  master. SHOW INSERT FX stays view state (not saved with the project, no format change).
- **Delivery targets and Peak Surgeon.** File > Delivery Targets... (`file.delivery_targets`) opens a panel that
  renders the master on a worker, as the export would write it, and measures its integrated loudness, loudness range,
  true peak and sample peak. It lists eight common targets under generic names (Streaming loud -11 LUFS / -1 dBTP,
  Streaming -14 / -1, Streaming quiet -16 / -1, Podcast -16 / -1, Broadcast -23 / -1, Broadcast -24 / -2, Club -9 /
  -0.5 and Film mix -27 / -2) with the gain a normalising chain would apply to the measured master: turned down (no
  loss), left alone, turned up with its peaks still under the ceiling, or turned up into a limiter, which is the
  warning: a quiet master with peaks near 0 dBFS that the extra gain would push past the target's ceiling, so the
  chain's own limiter would act on it. Set Ceiling sets the ceiling of the Mastering device on the master's insert
  chain (else the master's Clipper) to the chosen target's, and the project's delivery target, in one undo step; with
  neither device it sets the target only. The target is the optional root field `delivery` of the project
  (`Command::SetDelivery`, ignorable at format 44, written only when set, also accepted in a `Command::Batch`), and
  the toast of every export, and of a stems export for the master, says how the export sits against it: the loudness
  over or under the target and the true peak against the ceiling. Edit > Peak Surgeon... (`sequencer.peak_surgeon`)
  opens the same panel on its Peaks page for the selected audio clip: the highest peaks of the clip as it plays (its
  gain, fades and envelope included, forwards or reversed) with their times and the headroom won by trimming down to
  each, a threshold in 0.1 dB steps, a dip step of 0.2, 0.3 or 0.4 dB and a half-width of 4 to 32 frames. TRIM dips
  the chosen peak by one step; APPLY ALL dips every peak above the threshold, each in steps at the peak's own frame,
  down to the threshold, never more than 3 dB deep. A dip is three breakpoints of the clip's gain envelope (a triangle
  of linear gain), so nothing is rendered, the file and the clip gain are untouched, and the whole trim is one
  `SetAudioClip` and one undo step; the clip is read again afterwards and the status line says how far the highest
  peak went down. A warped or looped clip, a missing file and an asset at another sample rate are refused with a
  reason. The format is unchanged (44, one new ignorable field) and the command count is 501.
- **Transition kit.** Add Transition puts a transition at a section boundary: the Track menu, and the right-click
  menus of a marker, a range and a bar line in the ruler (`sequencer.add_transition`, with the choice as its
  argument). A boundary is the start of a section: the marker or range the menu opened on, the bar line nearest to
  the pointer in the ruler, else the selected marker, else the first section line at or after the playhead. A
  riser (1, 2 or 4 bars) is the reverse reverb of the next section's first beat, from the track that plays the
  first hit (a drum machine first on a tie), printed on a new track that ends exactly where the section starts; it
  reuses the print of Reverse Reverb Print on the freeze worker. An impact is that hit through a saturator
  (`atomik_dsp::transition::impact`) and a three second reverb, printed from the downbeat. Filter Down (1, 2 or 4
  bars) puts an Auto-Filter low-pass as the last insert of the master, or with Group Filter Down of the group bus the
  selected track plays into, and closes its cutoff over the last bars in nine steps that fall by the same share per
  octave, open again at the boundary; a second one reuses the filter and its lane. Silence on Beat One takes the
  master fader to zero for one beat from the boundary. Fill plays the last beat of a note track (the selected one,
  else the first that plays through it, a drum machine first) twice at double rate, each note moved to half its
  distance from the beat and half as long. The filter, the silence and the fill are one `AddTransition` command
  each, so every transition is one undo step, and the status line says what it did. When one kind is on more than
  half of the boundaries, and on at least three, the status line says so. The project format is unchanged (44) and
  the command count is 499.
- **Rumble Maker and Reverse Reverb Print.** Track > Rumble Maker (`sequencer.rumble_maker`) turns the selected
  kick track into a rumble layer: the kick (the selected clips of the track, else all of them; the kick pad of a
  drum machine; an audio kick by its onsets) is rendered offline through its insert effects, and one piece per
  hit is built from its own sub band, which starts in phase with the kick, then goes on as a tail until the next
  hit, either by repeating a trimmed chunk of the kick's tail with crossfades (Tail Repeat) or through a long dark
  reverb low-passed to the sub band (Reverb Tail, its decay twice the gap between the hits). The layer is printed
  into the project's audio as one file on a new track after the kick's, with a Sidechain Tool keyed by the kick, a
  24 dB low-pass at 160 Hz and a low cut at 28 Hz, in one undo step. Track > Reverse Reverb Print
  (`sequencer.reverse_reverb_print`, 0.5 to 8 seconds) reverses the selected clips, or the range of a lane or the
  locators, into the reverb, keeps the tail and reverses it again: a swell that rises to its loudest where the source
  starts. It is printed on a new track on whole ticks and ends exactly at the first frame of the source; a source
  near the start of the song gets the swell that fits, one at the start is refused. Both render on the freeze
  worker, never on the audio thread, and the prints are saved, collected and undone like other audio. The project
  format is unchanged (44) and the command count is 498.
- **Reverb send wizard and ducking reverbs.** The Reverb and the Space Reverb take a key on `sc_in_l/r` (the main
  input when neither is cabled, as in the compressor) and seven new params: `duck` (how much of the wet
  signal the key takes out), `duck_release`, `duck_mode` (broadband, the part under `duck_freq`, or a band of Q 2
  around it) and `duck_threshold`, and `key_gate` with `key_gate_hold`, a gate that closes the tail (a 12 ms fade)
  that long after the key last exceeded the threshold. The duck and the gate act on the wet path only: the dry
  signal is never touched, bit for bit. The reverb panel has two new sections, DUCK and KEY GATE, and is three
  units tall. Track > Add Reverb Send... (`sequencer.add_reverb_send`) opens a small wizard (a de-esser for a
  vocal, the duck from the track, post- or pre-fader, the decay, the high-pass, the low-pass and the send level) and
  adds, in one undo step, a send of the track's channel to a new FX return that holds an optional de-esser, an EQ
  (a 24 dB high-pass at 300 Hz and a 24 dB low-pass at 8 kHz by default) and an all-wet Space Reverb, cables the
  channel's output to the reverb's key and turns the duck on. The decay is suggested from the track: twice the
  median gap between the onsets of its notes, between 0.4 and 4 s (3 s for a vocal), and 2 s for a track without a
  rhythm to read; the duck release follows the same gap. Three factory containers (Ducked Vocal Reverb, Gated
  Plate, Clean Low Hall) are the same chains for an FX return. The project format is unchanged (44) and the
  command count is 496.
- **Sound Analyst.** File > Sound Analyst opens a panel that takes an audio file (dropped on it or picked with
  OPEN FILE) and measures it on a worker thread: the amplitude envelope (the rise from a tenth to nine tenths of the
  peak, the decay to the sustain, the sustain level, the release and how long the note is held), the brightness over
  time (the spectral centroid), the pitch over time (the tracker of Audio to Notes; a pitch offset at the start and
  the time it takes to settle count as a pitch envelope), the harmonic profile (a pure tone, all harmonics, odd
  harmonics only, inharmonic, noise) and the share of the sound that is noise. The panel draws the level, brightness
  and pitch curves and prints the numbers. It also proposes a starting patch by rules that are constants in
  `atomik_render::sound_analyst`: the Poly Synth on one oscillator (sine, saw, square, or triangle when the third
  harmonic is under -14 dB) for a harmonic sound, the FM Synth for an inharmonic one (a modulator at the ratio of the
  strongest partial that is not a harmonic), and the Poly Synth as a noise voice for noise or an unpitched sound (a
  high-pass filter from 2.5 kHz of centroid, else a band-pass). The amp envelope is converted with the exact shape
  of the synth's envelope, the brightness movement becomes a filter envelope, the pitch movement a pitch envelope
  and the noise share the noise level. The proposal is rendered on the note of the sound (the key of its
  fundamental, held for the measured note length), measured again with the same analysis and corrected on the
  cutoff up to three times; the match meter shows how far each curve of the render is from the sound (decibels,
  octaves, semitones) with a score from 0 to 100 %. MAKE PATCH adds the patch on a new track with a one-note clip,
  in one undo step; MATCH renders the patch as it is now (a patch changed by hand), and a match of a patch that was
  changed afterwards is shown faded. Analysing never changes the song. The project format is unchanged (44) and the
  command count is 495. Not done: chords, patches for the sampler or the wavetable synth, several candidate synths
  side by side, effects recognised in the sound, and an audio clip of the song as the source.
- **Wavetable editor.** A click on a table display of the Wavetable Synth opens a panel for the device's table:
  a list of up to 16 key frames (add a blend, duplicate, delete, move), a box to draw the selected frame in, a
  bar and a dot for the level and phase of each of its first 64 harmonics (the frame is rebuilt by additive
  synthesis and every other harmonic stays as it was), and whole-frame processes (normalise, remove offset,
  invert, reverse, smooth; on one frame or on all). The key frames are baked into the 16 frames the synth plays,
  with the frames between two keys filled by the same linear blend, and stored as the device's one sample in the
  project's audio, so a table saves, reloads and undoes like a dropped sample, one undo step per gesture (a
  stroke, a bar drag, a button). PHASE-SAFE shifts every frame in time until its fundamental starts at phase 0
  (the waveform keeps its shape), so a morph no longer loses level where two frames' fundamentals were out of
  phase; the panel names the pairs whose fundamental would cancel and shows the level dip of the morph. For a
  table made from a dropped sample the panel shows the length of one cycle found in it and offers CUT (cut the
  sample again with one cycle per frame), DEDUPE (drop silent and near-identical frames) and EDGE (cross-fade
  the end of each cycle into its start); USE OSC 1 makes oscillator 1 play the table. The project format is
  unchanged (44). Three factory patches use drawn tables. Not done: brushes for a region of a frame, a
  mirrored-half drawing mode, and import from an image or from FFT chunks.
- **Drawn duck curve, learn from kick and band mode in the Sidechain Tool.** A new curve mode (`curve`) ducks
  along a hand-drawn gain curve of up to eight points with bends over one sixteenth, eighth, quarter or half
  note or one bar (in song time, so it follows tempo changes), started by every hit on the key, every note at
  the new `notes_in` socket, or either (`trigger`). A hit during a curve enters the new one where it first
  reaches the dip that is playing, so a double kick never lifts the gain. The panel now draws the curve across
  its top: drag a point, drag the line to bend a segment, click to add a point, double-click to remove one,
  Escape to put a drag back; the plot shows the running position. LEARN renders the range between the locators
  at the key (the sidechain cable, else the input) on a worker, averages the envelope of its hits over the
  curve length and puts a curve of up to eight points and the curve mode into the device in one undo step. The
  new `band` switch and `band_freq` duck only the part of the sound under the split (a 24 dB per octave
  Linkwitz-Riley pair added back, an allpass at rest), in every mode. The points are stored in the new
  `hidden.*` params, which are saved and undone with the device but offered to no macro, automation lane, MIDI
  mapping or search; the project format is unchanged (44). The `bypass` param of the device moved from id 8 to
  12 (projects store params by key). Four factory patches draw curves. Not done: a chosen track as the key
  without a cable, and more than eight points.
- **Translation lens, "Will the bass read?" and Split Sub.** Mixer > Translation Lens (and five rows in the
  transport bar's more menu) switches the live master between headphones (as it is), a small speaker (a 12 dB per
  octave high-pass at 140 Hz and a +3 dB bell at 3 kHz), a club sub system (a 12 dB per octave high-pass at 32 Hz)
  and mono (`(L + R) / 2` on both sides); choosing the voicing that is on goes back to headphones. It is
  monitoring only: it sits after the master chain and the reference, before the click, on the live output, is
  kept in the app like the loudness match, never saved, not an undo step, and never in a bounce or an export; the
  status line shows `LENS ...` while it is on. Mixer > Will the Bass Read? renders a track or the master on a
  worker and shows the share of the low note's energy that a small speaker still plays through the 2nd to 5th
  harmonics of the note (a saw bass at 55 Hz reads about 16 %, a pure sine under 0.1 %) on a meter with the verdict
  (reads from 8 %, partly from 2 %, lost below) and a line on what to do. Edit > Split Sub (also on the patch name
  pop-up of a Mono Bass) puts the bass track in a container, in one undo step, with a mono sine sub layer that
  follows the same notes, a 24 dB low cut at 120 Hz on the bass itself, and a mixer that sums them, so width and
  unison stay above the sub; with the bass's filter open the level stays within 1 dB. Not done: a choice of split
  frequency in the menu (the model takes 60 to 250 Hz), a lens that follows the reference only, and a headphone
  correction. The project format is unchanged (44) and the command count is 494.
- **Energy map and repetition finder.** View > Energy Lane (off by default, saved with the view) adds one row
  above the track lanes with a bar per section as tall as the section's energy, from 0 to 1, and its name and value.
  The sections are the range markers, else the point markers (each runs to the next, with a "Start" section before
  the first), else every 8 bars; a name is read as a role (intro, verse, build or pre-chorus, drop or chorus, break,
  outro). The master is rendered on a worker (`atomik-energy`, never the audio or UI thread) and each section gets
  its loudness, brightness (the centroid of the band levels) and stereo width from the same deterministic band
  analysis as the masking map, plus the share of the tracks that play in it; four documented weights (0.40, 0.25,
  0.20, 0.15) combine them. The lane flags a build with more energy than the drop after it, the strongest drop when
  another kind of section has more energy, and two neighbours of near-equal energy over 16 bars or more; the status
  line says it when the measurement ends. An out-of-date lane is drawn faded; View > Refresh Energy Lane measures
  again, and turning the lane on measures when there is no result. Edit > Find Repetition lists what a track plays
  three times or more in a row unchanged (a pattern of 1, 2 or 4 bars seen through loops, copies and grooves, or
  identical touching audio clips or one looping audio clip), where it is, and three suggestions each: drop the last
  hit of the last bar, a low-pass sweep over the last bar, cut the last beat. A suggestion that is a plain edit (mute
  the notes in one unlooped clip; add a sweep of the channel's low-pass on two new automation lanes when the filter
  is off and not automated) is one button and one undo step and says what it did; any other is advice in text.
  The project format is unchanged (44) and the command count is 488.
- **Dragging between song windows, and audio at another sample rate.** Clips, a track header or devices dragged
  out of one song's window and let go over a window of another open song are pasted there at the point of the
  drop: clips on the lane and snapped tick under the pointer (on a lane that fits them, else each on its own kind
  of lane), devices at the rack slot there, a track after the track there. It is Copy in the first song and Paste
  in the second, without touching either clipboard: the first song ends the drag with everything where it was, and
  the second takes one undo step, with the audio fetched from the first song's folder as for a paste. Files
  dragged out of the file browser (a patch, a container, a device row of the shared library) hover over and drop on
  another song's window the same way, with the same outline where they would load. Audio that is not at the sample
  rate of the song it lands in, whether pasted or dragged, is converted first: a worker (`atomik-paste-rate`, never
  the audio or UI thread) resamples it into the folder of the receiving song, reading the original from the song it
  came from and leaving it untouched, and the paste then points clips, takes, frozen renders and sampler zones at
  the copies with every position that counts source frames scaled, as one undo step; a second paste of the same
  audio reuses the copy, and a source that no longer has the file refuses the paste and says so. Clips pasted
  at the song's own rate behave as before. Not done: separate Input Focus and Play Focus buttons (the song with the
  stream still has the MIDI inputs), a drop marker while clips or devices are dragged over another song, and a
  session of the MCP server with several songs. The project format is unchanged (44) and the command count is 469.
- **Collect plugin files, and Move Project tidies a shared folder.** Collect Files, Save As and Move Project copy the
  files that the text state of a plugin names outside the project (an impulse response, a preset) into a `Files`
  folder of the project, under their own names (a second file with the same name and other bytes becomes `name 2`,
  and a copy that is already there is reused), and point the state at the copies. It is one undo step for all the
  plugins; a file that cannot be found, or is over 64 MiB, stays where it is and is listed in the toast, and never
  stops the save; Preview Collect Files counts them. A state that is not text is never read or changed. Where other
  projects share the old folder, Move Project now reads every project file in it and below (and the versions kept
  beside each) and removes the old audio that none of them uses, keeping the rest; a project, version list or folder
  that cannot be read (or a link, or a folder too deep to look into) keeps all of the old audio and the toast says
  so. Move Project and Collect Files refuse to work on a project inside the factory folder. The format is still 44
  and the command count 469.
- **Example projects.** The Factory root of the browser has a Projects section with three short songs made from the
  factory content and nothing else (built-in devices, the factory patches, containers, kit samples and clips, no
  plugin): Basalt Beat (16 bars at 96 BPM: a sampled drum kit with looped pattern clips, a walking sub, a clavier and
  a mallet hook, with effect containers on a channel and the master), Moonrise Modular (16 bars at 84 BPM: the three
  modular instruments, a drone and a sequenced pulse, a shimmer reverb and a ping-pong echo as send effects, and a
  container macro opened by automation) and Bus Room (24 bars at 118 BPM: drum pads on channels of their own into a
  Drums bus, a track folder that plays into a Music bus, two groups, two send effects and a master finish, with the
  beat ducked for a breakdown by an automated group level, a pad faded in and a lead filter opened). Picking one
  (double click, a drop, or the command line) opens it as a new unsaved song, as a template does: the factory file is
  never the song's file, so Save asks where to keep your copy. The folder grows to about 1.6 MB (the Basalt Beat
  carries the audio of its kit). The writer is the same test code as before (`ATOMIK_WRITE_FACTORY=1 cargo test -p
  atomik-app --lib write_the_factory_library -- --ignored`, or `write_the_example_projects` for the examples alone);
  tests check that each example is 16 to 32 bars of built-in devices and factory samples, that its file is what the
  tables build, that it bounces to audible sound under full scale with no silent stretch, and that opening it leaves
  the factory file untouched. The command count is unchanged and so is the project format.
- **More factory content.** The Factory root now holds 31 patches, 13 containers and 12 note clips (about 1.2 MB, most
  of it the audio of the drum kits). The containers are ten effect chains of built-in effects (tape saturation, a
  vocal chain, a ping-pong echo, a shimmer reverb, a lo-fi radio, drum punch, a slow phaser, a rotary speaker, a
  tempo-synced filter gate and a mix-bus finish), each with named macros that start where the sound was designed,
  and three modular instruments built from the CV and note devices (a sample-and-hold walk through a scale, a
  chord-and-arpeggio stack and an envelope-swept pad). Three new semi-modular patches use its sequencer and
  modulation slots. The four drum kits (nine pads each on keys 36 to 46) play short mono samples that the writer
  renders from the synthesised pad modules of the built-in drum machine, so no recording is involved; the audio sits
  beside each kit item, as it does for any library item with samples. The clips are four drum patterns on the pad
  keys of the kits, three bass lines, three chord progressions and two arpeggios. Space on a drum machine item now
  plays a drum pattern on its pad keys instead of the melodic phrase, which a kit leaves mostly silent. The tables
  and the renderer are test code in the same writer as before, and tests check that every item loads, that every
  patch, container and kit plays audible sound under full scale, that every macro of a chain changes the sound and
  that each kit sample is mono, short, levelled and named by its content. The command count is unchanged.
- **Factory library.** The file browser has a Factory root under Library with the sections the application ships
  items for (Patches today), next to the user's own library: items there list, search, filter by tag and category,
  take favourites and user tags, drag and load, preview with Space and step with the patch strip like library items,
  and they are read-only (delete and rename refuse them, and Save to Library always writes to the user library).
  The folder is `factory` beside the program (the zip, the tarball, the installer and the Debian package's
  `/opt/atomik-rack`), `Contents/Resources/factory` in the macOS app, `ATOMIK_FACTORY_DIR` when set, or the
  `factory` folder of the source checkout in a development build. It must lie outside the user library folder. A
  `factory.json` in it gives each item its categories, tags and author, which a patch file does not hold. The first
  content is 24 original patches (about 37 KB) for the poly synth, mono bass, dual layer, FM synth, grain sampler,
  wavetable synth and organ: pads, strings, plucks, keys, leads, bells, basses, textures and organs. The sound
  tables and the writer are test code (`ATOMIK_WRITE_FACTORY=1 cargo test -p atomik-app --lib
  write_the_factory_library -- --ignored`), and `ATOMIK_FACTORY_RENDER_DIR=<folder>` with
  `render_the_factory_library_to_wav` renders every patch to a WAV with a level report. Tests load every patch,
  apply it in place and render its preview phrase, which must be audible and stay under full scale. The command
  count is unchanged.
- **Project versions.** Every save keeps the project file it replaces as an automatic version in a versions folder
  next to the project (`Versions/` inside its bundle folder, else `<name> Versions/` beside the file), and the
  oldest automatic versions beyond a number are pruned (Settings > Interface > "Versions kept": 5 to 200, default
  20, `keep_versions` in `workflow.json`). A version is the plain text of a project file under a `.atomikver` name
  plus a small index, `versions.txt`, so the project format is unchanged (44). File > Save Version... keeps the song
  as it is, unsaved changes included, under a name; named versions and any version marked protected are never
  pruned. File > Versions... lists them newest first (date in UTC, name, NAMED or PROTECTED), with Protect and
  Restore. Restore asks about unsaved changes, keeps the current state as an automatic version first, then opens the
  chosen version as the song, unsaved, so Save writes it over the project and restoring the version kept first
  undoes the restore. A save that changes nothing keeps nothing, and a version identical to the newest one is not
  kept twice. Audio a kept version uses is no longer deleted as unused, so a restored version still plays. Pruning
  deletes only the files the index lists as automatic and unprotected inside the versions folder; any other file
  there, named versions, protected versions and everything outside the folder stay. Move Project carries the
  versions along (copied, compared byte for byte, then removed from the old place); Save As starts a history of its
  own; project archives and Collect Files leave the versions out. The command count is 469.
- **Compare versions.** The versions panel has a COMPARE button (and the C key) that lists what differs between the
  chosen version and the song as it is now, unsaved changes included. Shift-click on a second row (or Shift with
  Up and Down) marks it, and then the two versions are compared, the older one first. The result replaces the list
  with a scrollable text in four sections, each shown only when it has lines: Song (tempo, meter, length in bars and
  minutes, sample rate, master tune, title, author, markers and sections), Tracks (added, removed and renamed
  tracks; for the others the clip, note and automation lane counts, mute, solo, freeze and the insert effects that
  came or went), Devices (devices added and removed, with the track a device of an insert chain is on, renamed
  devices and how many parameters changed on each) and Mixer (strips, buses and groups added, removed or renamed;
  fader, pan, mute and solo of a strip; snapshots and FX returns). Back, Enter or Esc returns to the list. Nothing is
  kept or changed by a comparison, and audio is not compared. There is no new command (469) and the project format
  is unchanged (44).
- **Library browser tidy-up.** Audio rows of the file browser draw a small waveform at the end of the name column
  (48 bars of the first thirty seconds of the file, on a square-root scale so quiet files show). A worker reads it
  for the visible rows only, newest request first, from the loudness picture the info pane already uses; the answer
  is kept per file and size (up to 4000 files), a row without one, or whose file changed, draws nothing, and a narrow
  panel leaves the name its room. Patch, container and clip items below the library folder can now be deleted: the
  row menu has "Delete..." and the Delete key (command `files.delete`, Files context) asks in the path bar
  (`Delete 'name'?`), Enter confirms and Esc, any other key or a click cancels. There is no system trash helper, so
  the file is removed for good, together with the audio it alone uses in its sibling folder (`<name>/Audio`); audio
  that another item or project of the same name still lists, or that cannot be read to tell, is kept and the status
  line says so; a folder that is a link is never entered and folders are only removed once empty. Built-in devices,
  projects, factory content, anything outside the library folder and any other file are refused with a message. The
  item's favourite flag, tags, author, categories and list places are forgotten, a running preview stops and the
  section is listed again. The project format is unchanged (44); the command count is 467.
- **Library patch previews.** Space on a patch or container item of the file browser plays it without loading it:
  nothing is added to the song and the undo history is untouched. The item is built into a hidden scratch project
  and rendered with the bounce renderer on a worker thread, so no device is built or dropped on the audio thread.
  An item with an instrument (a synth patch, or a container with a synth inside, with its effects) plays a short
  built-in phrase: a low arpeggio and a held four-note chord. An item that holds effects only (an effect patch, a chain
  of effects, an effect container) is put in the insert chain of a built-in synth that plays short notes, so the
  delay or reverb tail is heard. The result, with its tail cut where the sound ends and scaled down when it would
  clip, goes into the browser's preview voice, so Space again, Esc or another preview stops it as for an audio file,
  and the row shows the position. Patches with plugins, CV utilities and items with nothing that takes sound are
  refused with a message in the status line; a failure of the render is reported there too. Previews never reach a
  bounce or an export. The project format is unchanged (44). Not done yet: playing the preview from the MIDI keyboard
  as well as the phrase, plugin patches, factory content and deleting items from the browser.
- **Library clip previews.** Space on a clip item of the file browser (the same key that plays an audio file) plays
  its notes without loading it, once, at the song tempo in effect at the playhead, while the transport stands still.
  With a note track selected the notes go through the instrument of that track (and the players above it) on a clock
  of their own; with none selected a small built-in voice renders them into the browser's preview voice, so Space
  again, Esc or another preview stops it as for an audio file. The row shows the position while it plays. Starting
  the transport, another preview or a stop ends it (a song that plays refuses the preview), a note held at the
  stop is released, and the document, the selection and the undo history are never touched. Previews never reach a
  bounce or an export. Selecting a clip does not play it, whatever the auto-preview switch says. The library holds
  note clips only (audio files play through the file browser already). The project format is unchanged (44). Not
  done yet: factory content and deleting items from the browser.
- **Synth follow-ups: FM unison, slope and 4x, and several zones on the grain sampler.** The FM synth has a **unison**
  stage: `unison.voices` (1 to 7) makes every key sound that many copies of the voice, spread from minus to plus
  `unison.detune` cents (the fixed-frequency operators keep their pitch), started from different phases and panned
  apart by `unison.spread`, sharing the voice's envelopes and adding in power; the voice allocation and the play
  modes are those of one voice. Each operator has a **slope** (`op1.slope` to `op4.slope`), a fifth envelope stage
  after the decay: while the key is held the level keeps falling, by 60 dB over that many ms (0 holds the sustain
  as before); the panel draws it in the envelope shape. The oversampling choice has a **4x** setting (two halfband
  stages, 35 frames of latency, reported): it helps where the richer waves and the feedback loop fold back, and
  costs about four times the voice CPU. All new params are appended and default to the old behaviour, so projects
  and patches sound as before, bit for bit (a test pins every factory sound on Windows). On the grain sampler, a
  sample dropped on the panel with the **copy modifier** held (Ctrl, or Opt on a Mac) keeps the zones it has and
  adds one per file, in one undo step, where a plain drop still replaces the sample; the sample editor sets the key
  and velocity ranges of the zones. The two edges of the **spray band** on the waveform are handles in the lower half
  of the display: drag one to set the spray (one undo step, Esc puts it back). The play mode and the zone mode open
  a **pick list** on a click. The project format is unchanged (44). Not done yet: six operators, a mod matrix
  beyond the fixed targets and a display of the modulated indices of a sounding voice for the FM synth; spectral or
  pitch-synchronous grains, a freeze of a live input, a legato note that crosses to another sample and more factory
  sounds for the grain sampler.
- **Start screen and collect follow-ups.** The start screen shows a **card** for the template under the pointer or
  the keys: its tempo, meter and tracks, read from the file when you reach it (a template made with a plugin this
  machine lacks says it has no preview). A recent song has a **pin** (click it, or press P on the row): a pinned
  song stays at the top of the recent list, in the order it was pinned, however many songs are opened after it, up
  to four; Clear Menu keeps the pins, the pins are shared by every open song and saved in `recent.json` (a build
  that does not know pins still reads the list). Settings, Workflow has a **Show the start screen at start-up** row
  beside the start screen's own check box. **Preview Collect Files** (File menu) says what Collect Files would copy
  and how big it is, and what is already there or cannot be found, without copying; Collect Files, Move Project
  and a Save As that copies audio show their count in the status line and can be stopped with **Cancel Collect or
  Move** (nothing is written then, the project stays as it was, and the files already copied stay). **Move
  Project** puts the project and the audio it uses in a folder of its own name inside the folder you pick, then
  removes the old project: only after the new project file reads back exactly as written and every audio file the
  old bundle holds is in the new one byte for byte, never audio that another project in the old folder (or below
  it) may use (the old project file alone goes then), never a place that is the old one, inside it or around it,
  and a folder that holds anything else stays with it. It refuses to move into a folder that already holds a
  project of that name. No project format change.
- **Macro follow-ups: the inside of a container, lanes of driven params, typed ranges.** A macro bank can now
  drive a param of a device inside a container (a path such as `layer/synth`, from the right-click menu inside the
  container, the Macros panel or the MCP `macros` tool): the engine, a bounce, the saved project and the container's
  own view (the macro tag and the macro's value) follow. A container's macro and a bank never drive the same
  param, and a device that holds driven params cannot move into or out of a container, since the assignment names it
  by its place. A param a macro drives may now have an **automation lane of its own**: the lane sets the base and
  the macro adds its range (nothing at macro 0, or at macro 1 when inverted), clamped to the param's range, and
  when the macro has a lane too the two are joined at the points of either; a bounce and the live song play the
  same, a move of the macro re-plays the lane, and following the selection shows the param's own lane once it has
  one. In the Macros panel a middle click on MIN or MAX, or Enter with the pointer on it, **types the value** in
  place (a number in the param's unit, with `k` for kilohertz, `s` for seconds and a percent as 0 to 100).
  No project format change.
- **Console editing follow-ups.** A click on the master's name plate in the console selects it, so its settings
  can be copied, pasted, reset and cleared like a strip's (a master copy pastes onto the master only, of this song or
  another). The strip compressor has **auto make-up** (the M-UP button: the output is raised by the reduction a
  full-scale signal would get, up to 24 dB, and the gain-reduction meter still reads the reduction alone) and the
  strip EQ an **E mode** (the E button: every bell's Q grows with its gain, so a large boost or cut is narrower; the
  drawn curve follows). A cable into the key input of a channel or a bus now switches its key on, in the same undo
  step. The master has a **level CV** input (`fader_cv_in`, amount `fader_cv_trim`, plus or minus 24 dB around the
  fader). The new switches and the amount are optional params that older builds ignore, and a strip or master that
  leaves them alone sounds as before.
- **Snapshot follow-ups: routing and insert chains, and a snapshot lane.** A snapshot now keeps where each strip
  plays (the master, a bus or an Audio I/O pair) and which effects sit on its insert chain, in order. Recall
  re-cables what differs in the same undo step as the params; what no longer fits (an effect that is gone or has
  moved to a return, a bus that is gone, a route that would make a loop, an Audio I/O pair another cable has
  taken) is skipped, named in the snapshot's status, and the rest still recalls. The chip menu has **Recall
  routing only** (a `routing` scope in the MCP `mixer.recall_snapshot` argument), a morph flips the wiring at
  the half way point, and a compare flips it with the rest of the mix. A snapshot made by an older build holds no
  wiring and leaves the routing alone, until Update from current mix takes it. A **snapshot lane** is an
  automation lane of the master whose points recall a snapshot while the song plays: it is drawn in the
  arrangement as blocks named after the snapshots, moves, copies and splits with its clips, has a picker of the
  snapshots on a right click, and every edit is one undo step; a marker and a point crossed in one frame recall the
  later one. **Add Snapshot Lane** and **Remove Snapshot Lane** are in the snapshot chip menu; a bounce does not
  recall (as for a marker). The wiring is an optional field that older builds ignore; a project that has a
  snapshot lane does not open in a build from before it (the lane is a lane of the master with a key that build
  does not know). The project format is unchanged (44). No new menu command ids.
- **Restructuring follow-ups: scale about the end, warp markers that join, and joining clips of different files.**
  With the scale-selection role held (Ctrl+Alt+Shift), a drag of the **start** edge of a selected clip scales every
  selected clip about the latest end of the selection, which stays where it is, as the end edge already did about the
  earliest start: the same preview, grid snap and one-step release, and a scale that would take a clip before the
  start of the song stops at the start. **Join Clips** now keeps warp markers: the joined clip carries the markers
  of every piece with one at the join, and the join is refused, naming the clips, when the markers would not play
  what the pieces did. **Join Clips on audio clips of different files** renders them: the clips of one lane that
  touch or overlap are bounced dry (each clip's gain, fades, envelope and warp, with the crossfades between them)
  on the same worker and queue as a freeze, with progress in the status line and Cancel Freeze, and the first clip
  then becomes one plain clip of the render spanning all of them, in one undo step. A gap, a muted clip and a
  frozen track refuse with a message; clips of one file still join without a render. No saved field is new (the
  project format is unchanged, 44) and no menu command id is new (the join and the gesture were already there).
- **Takes for automation passes, and a saved link between take groups.** A loop recording of automation now stacks
  its passes as the takes of one take group on the lane (the last pass plays), like a note or audio loop recording,
  instead of laying layered clips: the take rows open under the automation lane, a take draws as a line, and the
  takes can be picked and comped. A recording with one pass still lays plain clips. **Link With Selected Tracks** in
  a take menu saves a link between the take groups of the selected tracks (one undo step): a comp drag or razor cut
  in one then combs all of them, without the tracks having to stay selected, the strip reads "linked", and **Unlink
  Take Group** takes a group out. Saved as optional fields that older builds ignore (older builds drop automation
  takes and keep the clips they compiled); the project format is unchanged (44). A copied track is not linked to
  the original. No new menu command ids.
- **Mixer group follow-ups: a group bus, and automation of a group's mute and solo.** Mixer > **Route Group to New
  Bus** (and a **Route Group to Bus** list with a row per group, or Route to New Bus on the group's plate menu)
  plays the members of a VCA group into a bus named after the group, in one undo step; a strip that joins the group
  goes to the bus, one that leaves it or is removed goes back to the master, in the same step. **Stop Routing Group**
  sends the members back to the master. The group keeps its own level, mute and solo. A group can also have a
  **mute lane** and a **solo lane** next to its level lane on the master's track (the group plate menu shows each
  one separately): recorded from the group's M and S buttons, played on every member, held when the lane is muted,
  and shown on the plate while the song plays. A solo lane switches the solo of the group's own members; the other
  strips are silenced from the stored solo state, as for a strip's own solo lane. Saved as two optional fields of a
  group that older builds ignore; the project format is unchanged (44). New commands `mixer.route_vca` and
  `mixer.unroute_vca` (the group id is the argument). Groups of groups are still to come.
- **Track organisation follow-ups: a folder routed to an output bus, and a level bar on the folder header.** Mixer >
  **Route Folder to New Bus** (and the folder row's context menu, or a **Route Folder to Bus** submenu that lists
  the buses of the song) makes the channels of a folder's tracks play into a bus named after the folder, in one
  undo step; a track that moves into the folder follows, and one that moves out goes back to the master, in the
  same step as the move. **Stop Routing Folder** sends the channels back to the master. A routed folder shows the
  bus fader as a thin bar on its header, and a folder that only a VCA group follows shows the group's level on
  it: press or drag to set it (one undo step), Esc puts it back and a double-click resets it. Saved as an optional
  field that older builds ignore; the project format is unchanged (44). New commands `mixer.route_folder` (with an
  optional bus number) and `mixer.unroute_folder`.
- **MIDI controller follow-ups: a badge for a track's own lanes, a file channel of controllers on the track's
  lanes, and a meter map in the track lane grid.** A note track that holds controller lanes of its own shows a
  **CC** badge in its header (rows of 52 px or more); a click opens the lanes in the piano roll, as Track > Edit
  Track Controllers does. A MIDI file channel that has controllers and no notes is no longer left out on import:
  its lanes go to the own lanes of the track made from the same file track, or to a new track without a clip,
  in one undo step (a file imported as one clip puts them in the clip). The roll's track mode draws the song's
  whole meter map, so its bar lines and numbers follow the meter changes. The project format is unchanged (44).
  Still to come: lanes for per-note expression, and Bar and Beat snap that follows a changing meter in the track
  lanes.
- **CV utility follow-ups: a custom scale and hysteresis on the CV Quantizer, and a track mode on Sample and Hold.**
  The CV Quantizer has a **Custom scale** switch (`scale.custom`) that replaces the scale and root with twelve note
  switches (`scale.note.0` to `scale.note.11`, one per pitch class from C, starting as C major) saved with the
  device in the project; a custom scale with no note on restricts nothing. A **Hysteresis** knob (`hysteresis`, 0
  to 1 semitone) keeps the quantizer on its key until the input has moved that far past the middle between two scale
  keys, so a wobbling input does not chatter between them (at 0 it is the plain nearest key, as before). Sample and
  Hold has a **Mode** choice (`mode`): `track` follows the input while the clock is on and holds the last value while
  it is off, with `trig_in` high for the trigger clock and the first half of each cycle for the free and tempo
  clocks. The new params are appended and the project format is unchanged (44). Still to come: a delay and length in
  note values on the Gate Processor.
- **Channel strip follow-ups: a key and a tap for the parallel compressor.** The parallel compressor listens to the
  strip's key input while **KEY** is lit in the PARALLEL section (`pcomp.key`, with a key cabled; the dynamics
  section keeps its own KEY), and `pcomp.pre`, in the rack's channel panel, compresses the signal the insert chain
  receives instead of the one that comes back from it, blended with the signal after the inserts (the two are
  aligned only while the inserts add no latency). Both are appended params that default to off, so a project that
  does not touch them sounds as before. A container in the insert chain already had a bypass and a **B** on its
  slot (a bypassed container passes its input through, without the latency of its inside); this is now covered by tests.
- **Mixer group automation and the group chip in the rack.** The level of a VCA group is an automation lane: the
  group's right-click menu in the console has **Show Automation Lane** (and **Remove Automation Lane**), the lane
  sits on the master's automation track as "Group N level" over the group's range of -60 to +12 dB, and its
  fader carries the automation mark and follows the lane while the song plays or the playhead moves. The lane plays
  on every member of the group (in the live song and in bounces, so a ramp to the bottom silences the members from
  there on), the song follows a change of members, a move of the group fader is recorded into the lane when it is
  armed (Touch, Latch or Overwrite like any lane, with lane creation too) and the lane is held while it is written
  or overridden, and muting the lane keeps the group's level as it was. Deleting a group deletes its lane in the
  same undo step. The lane is an ordinary lane of the master with the param key `vca.<id>`, so the project format
  stays 44 and no field is added; a build without this change refuses a file that has one. A device of the rack
  that is in a mixer group (a channel or a bus that is a member, and the instrument of an automatic channel) wears a
  chip with the group's name in the group's colour on its header.
- **Freeze and export follow-ups: container devices, Freeze with Strip and Export Selection.** A container
  instrument or a container effect in the insert chain freezes like any device, with every device inside it left out
  of the engine (the render is taken at the inner output that feeds the container's output, and the latency cut
  from its start is the longest path through the chain, so a container with a latent effect lines up as the same
  effect outside it does). **Freeze with Strip** (Track menu, a check item, off by default,
  `sequencer.freeze_strip`) makes Freeze Track take the render at the output of the channel: the input stage, EQ,
  dynamics, inserts, fader and pan are baked in, the whole insert chain is included and the channel is not built
  while the track is frozen; what the channel fed (the master or a bus) is fed by the render. A strip with a send on
  into a return slot that holds an effect, or one that feeds a parallel channel, is refused with a message, since
  their signal would go silent; Re-render Frozen Track keeps the choice, and Freeze Track up to Selected Device
  never bakes the strip. The strip of a frozen track cannot be edited until it is unfrozen. The choice is a saved
  `strip` field of the frozen object, an ignorable one: the project format stays 44. **File > Export Selection as
  Audio File** (`file.export_selection_audio`) renders exactly the time range selected on the lanes of the
  arrangement, with no tail and whatever the locators say, and the stems panel has a third RANGE choice, SELECTION,
  next to the whole song and the locators (the choice is not remembered between sessions). Still to come: baking the
  sends into a freeze, a render that follows tempo changes, freezing with in-process plugins, a latency-compensated
  mix and the selection in the MCP export tool.
- **Tempo detection, Warp Loop as N Bars and Quantize Audio on a reversed clip.** Detect Tempo, Warp to Song Tempo,
  Warp Loop as Bars and Quantize Audio no longer refuse a clip that plays backwards. Quantize Audio moves each
  transient to the grid line nearest where the reversed clip plays it (the clip's two ends stay put, and the mirror
  is taken in frames, so it holds through tempo ramps), as one undo step. The tempo estimate is tested on click
  tracks at eight tempos from 75 to 170 BPM. The slice editor and Clip to Loop Slicer still refuse a reversed clip.
- **Recording and note lane follow-ups: dimmed loops, several lanes recorded at once and a remembered fade unit.**
  The piano roll dims the part of a looped clip's window that no repetition plays (a later clip of the lane hides
  it every time round), as it already did for clips without a loop. **Record to Lane** (`sequencer.toggle_lane_record`,
  in the track header menu, checked when the selected lane is armed) arms a note lane to record together with the
  other armed lanes of its track: a record dot shows on the header of each armed lane (the chip of lane 0 gets a
  record-coloured outline), and a take, a loop recording and a Layer or Engulf recording are written to every armed
  lane exactly as they would be to one, in a single undo step; the take's ghost notes show on each. With no lane armed
  a take goes to the selected lane as before, and an armed lane that is deleted, merged away or frozen drops out. The
  armed lanes are view state and are not saved. The unit of the clip inspector's fades (ms or ticks, the tag at the
  top right of a fade box) is kept in `ui-state.json` as `fade_unit`. The chip of lane 0 still has no S button: it
  shares the header's second line with the track's own buttons and a third control would leave no room for its
  label (use Solo Lane). The project format is unchanged (44).
- **Rack editing follow-ups: "+" handles in containers, every device type in Create, Replace Device and a row
  dropped on a track header.** An unfolded container has a "+" on the seam below its INPUT strip and below each of
  its devices (so also above its OUTPUT strip): pressing one opens the device browser and the row chosen is added
  inside that container at that place, as a drop there does. The Create submenu of the rack and device menus now
  lists, after Instrument, Effect and Player, a submenu per family (Instruments, Effects, Utilities, Modulators,
  Players, Containers) with every device type by name; a long family is split into parts named by their first and
  last device, and a choice creates the device as picking its row in the browser does (new command
  `create.device`, which takes the type as its argument). The device menu has a **Replace Device** submenu
  (`rack.replace_device`) that lists only the types the selected device can become and is greyed when there is
  none. A row of the device browser dragged out of the rack onto a track header outlines the header when the row
  can replace the track's device and replaces it on release, in one undo step. A replacement now keeps the value of
  every param the new device has by key with the same unit (a choice only when its list of choices is the same),
  clamped to the new range, besides the track, channel, cables and place it kept; hidden internal params and
  modulation slots are not carried, and a plugin never gives or takes values this way. The project format is
  unchanged (44).
- **Restructuring follow-ups: scale a selection by its edge, extend audio clips and select a range across lanes.**
  Dragging the end edge of a clip with the new **Scale selection** role (Ctrl+Alt+Shift, Cmd+Opt+Shift on macOS; a
  row in Settings, Keyboard) scales every selected clip about the earliest start of the selection, which stays
  where it is: the clips keep their places relative to it and everything in them follows, the grabbed edge goes
  where the pointer is (on the grid), the clips change live as a preview that adds no undo step, the release keeps it
  as one step and Esc puts everything back. It is Scale Tempo (`ScaleTempo`) on a ratio taken from the drag; Alt
  alone still scales each clip about its own fixed edge. **Extend to Next Clip** and **Extend to Previous Clip** now
  work on audio clips too, in the same step as note and automation clips: an audio clip plays on from its file (a
  reversed one moves its source offset with its end and grows no further than the start of its file), its fades and
  gain envelope follow, and reaching back moves the start only as far as the file has audio before it, so a clip
  that cannot reach its neighbour stops short and one with no room is left alone. The edge maths of audio clips
  moved from the arrangement view to the model for this and behaves as before under a drag. A range selected with
  Alt+Shift now follows the pointer onto other lanes: a drag that moves up or down takes every note, audio and
  automation lane between the lane it began on and the one under the pointer, across tracks as well. Copy and Cut
  take the parts of the clips of all of them, Delete cuts the range out of every lane in one step, Paste puts each
  part back on a lane of its own kind of its own track at the playhead, and Bounce Range in Place renders each track
  the lanes belong to once. A range on one lane works as before. The project format is unchanged (44).
- **Quantize follow-ups: controllers that follow, a grid per note and a groove from a MIDI file.** A quantize
  now carries the controller lane points of a note clip (pitch bend, aftertouch and CC) along with the notes it
  moves. The rule: a point between a note's old start (included) and its new start (excluded) moves by the same
  distance as the note, so it keeps its place relative to the note; when the spans of two notes cover a point the
  note whose old start is nearest takes it, and when two points end on one tick the later one is kept. Points
  outside every span, device-parameter lanes and a track's own controller lanes stay where they are, and notes that
  are not selected move nothing. **Grid per note** gives each note its own grid: the note value nearest to the
  note's length, from a 1/32 note up to the grid of the panel, so a long note lands on the big grid and a short one
  on a fine grid in one quantize (it does nothing while a groove names the grid). The quantize panel has both as
  switches (**Grid per note**, off, and **Move controllers**, on), and **GROOVE FROM MIDI FILE** next to EXTRACT
  GROOVE FROM CLIP: it asks for a `.mid` file, measures the onsets and velocities of all its notes against the
  panel's step and slots, and adds the result to the song's grooves under the file's name (numbered when taken); a
  MIDI file dropped on the open panel does the same and imports nothing behind it. The groove list in the panel
  shows three rows (it scrolls) so that the taller panel fits a small window. Not done yet: a groove from an audio
  loop, and a quantize of note lengths to a length grid.
- **Controller lanes without a note clip.** A note track can hold pitch bend, aftertouch and CC lanes of its own,
  outside its clips, in song time. **Track > Edit Track Controllers** opens the piano roll on them: the roll shows the
  ruler and the controller area over the whole song (no notes, so no grid, keys or velocity lane), with the same lane
  chips, drawing, dragging, Shift-click and rubber-band selection, delete, copy, cut, paste and thin as the lanes of a
  clip; a drag is one undo step. The lanes play with the controllers of the clips (at an equal tick the track's own
  point wins), in the engine, in a bounce and in the MIDI export, even on a track that has no clip. A take of
  controller events and no notes no longer makes a clip of only controllers: the pass is laid over the track's lanes,
  replacing what they held in the time the take covered, in one undo step (a take with notes keeps its controllers in
  its clip, as before). Duplicating a track copies the lanes; Merge Note Lanes leaves them where they are; a frozen
  track refuses edits to them. **Project format 44**: a note track's `"controllers"`, the same list as a note clip's in
  song ticks, written before the track's `"lanes"` only when set. A reader of 43 would play the track without them,
  so the field is not ignorable. Not done yet: MPE (per-note expression) lanes, a mark in the arrangement that shows a
  track holds lanes of its own, writing the controllers of an imported MIDI channel that has no notes to the track's
  lanes, and a track lane grid that follows a meter map (it counts bars from the first meter).
- **Several songs: audio that travels, a shared library and recent list, and recovery of every song.** Copy and
  paste already crossed from one open song to another for clips, takes, devices, tracks, strips and patches, but the
  audio they use stayed behind in the first song's bundle. A paste now copies the files the song lacks from the
  bundle of the song they were copied in (or one that song borrowed from) into the song's own bundle, on a worker,
  and loads them; a file the first song no longer has is listed as missing, like any lost audio. The sample rate of
  the other song is not converted. The favourites, tags, lists and places of the library and the recent list are now
  carried between the songs at once: a favourite marked or a song saved in one window shows in the others, the file
  is no longer rewritten from a stale copy by another song, and a song opened later starts from the same state. A
  start after a crash brings back the songs that were open next to the first one, one window each, as unsaved
  songs (up to eight, from their own recovery folders), without moving the audio, and a message says how many came
  back. The first song's copy is offered by its banner as before. Project format unchanged (43). Not done yet:
  separate Input Focus and Play Focus buttons (the song that has the stream also has the MIDI inputs), converting
  audio of another sample rate, dragging between song windows, and a session of the MCP server with more than one
  song.
- **Collect Files and relocation.** **File > Collect Files** saves the song in place and gathers the audio it uses
  into the song's own bundle folder (`<name>/` beside the project file, or the folder the file sits in when it is
  named like the file). A song that shared an `Audio` folder with other projects gets audio of its own, and audio
  that was missing from the bundle but sits in a nearby folder (the folder of the file, the folder above it, a
  folder named like the file) is copied in and loaded. The copy runs on the save worker, nothing outside the
  bundle is written, the folders it copied from are left as they were, and a message counts the files and bytes
  copied and names the audio no folder had (those stay missing and can be relinked). It needs a saved song. **Save
  As** now copies only the audio the song uses, not the whole `Audio` folder, and looks in the same nearby folders
  for audio the old bundle lacks, so a song saved into another folder keeps working. Links are content hashes, so
  no project text is rewritten. The project format is unchanged. Not done yet: a preview of what will be copied,
  a cancel for a very large collection, and collecting files that live outside the project, such as plugin presets.
- **Start screen.** A box with the choices for a song to work on: **New Song** (the default song when one is chosen),
  **Open**, the templates of the templates folder (the first eight) and the recent songs with the day each file was
  last changed and its folder. It is shown when the app starts with no song to open, and any time from **File > Start
  Screen**. The arrow keys move inside a column and between the two columns, Tab visits every row, Enter or Space
  picks, and Esc closes it; a click works as well. A pick does what the File menu row does, so unsaved changes are
  asked about first and a song that is gone leaves the list with a message. The option to reopen the last song at
  start-up still comes first, and a project named on the command line skips the box. The check box **Show this
  screen when Atomik Rack starts** is kept in `workflow.json` as `show_start_screen` (on by default). The headless
  and MCP sessions never show it. The project format is unchanged. Not done yet: previews of the templates, pinned
  songs, and a row for it in Settings.
- **Wavetable synth follow-ups.** A sample dropped on the wavetable synth (from the file browser or the desktop) is
  a table: choose `imported` in a table pick list and the sample plays as a table, cut into frames of 2048 samples
  (a short sample is one cycle, a long one contributes 16 evenly spaced frames, and the part between the zone's
  start and end is used, so the sample editor trims it). It is built into the same band-limited form as the factory
  tables, so it does not alias, and it is saved with the project's audio. The table choice opens a pick list on a
  click (a right-click still steps back). Oscillator 2 can have its own unison count, detune and spread (the Osc 2
  choice in the Unison section); the default shares the settings, so old projects sound the same. A change of
  table fades from the old table to the new over 12 ms instead of clicking. While a voice sounds, the displays follow
  its modulated frame position and keep the knob position as a faint curve. The panel is one rack unit taller and the
  project format stays 43. Not done yet: a table editor, a mod matrix beyond the fixed targets, morphing between two
  tables, more than 16 frames per imported table, and a curve of the imported table on the panel.
- **FM synth follow-ups.** Each operator picks a wave: the sine (the old sound, bit for bit), a half sine, the
  rectified sine, a squeezed sine, a triangle or a soft square. Poly glide makes a new note in poly mode slide in from
  the key played before it (the Glide time sets how fast, a stolen voice included). A stereo chorus (mix, rate, depth)
  follows the master level and is skipped at mix 0. Oversampling runs the voices at twice the sample rate behind a
  halfband filter: a bright patch keeps more of its index on a high key without aliasing, at about twice the CPU and
  29 frames of latency that the device reports. Four factory sounds are new (Marimba, Wide Keys, Glide Lead, Square
  Keys), the panel has a WAVE and a CHORUS section, and the sound and the algorithm open a pick list on a click (a
  right-click still steps back; the organ and grain sampler sounds do the same). The new params default to the old
  behaviour, so existing projects sound the same and the project format stays 43. Not done yet: six operators, an
  envelope with more than four stages, a unison stage, a mod matrix, and a display of the modulated indices of a
  sounding voice.
- **Grain sampler follow-ups.** A click or a drag on the waveform of the grain sampler sets the position to the place
  under the pointer (one undo step per drag, Escape puts the start value back). The Mode choice adds mono and legato:
  one voice plays the last key and returns to a key still held when another is let go, and legato keeps the grains
  and the envelopes running under a held key and changes the pitch only (a key on another sample window starts a new
  note). Glide (0 to 2 s) slides the pitch to the new key in both, also from the pitch of a note that has ended. The
  LFO Scope choice gives each voice its own LFO, restarted at the note and seeded by it, instead of one LFO free
  running for all. The Zones choice adds a layered mode: a note plays every sample whose key and velocity range fits
  (up to eight, one voice each) with the round-robin slots and velocity crossfades the sampler has, where the first
  mode keeps playing the first match. The panel draws the last 29 grains instead of 13, which raises the number of
  display values a device may publish from 32 to 64, and the Legato Lead and Wobble Pad sounds are new. The new
  params default to the old behaviour, so existing projects sound the same and the project format stays 43. Not done
  yet: spectral or pitch-synchronous grains, a freeze of a live input, a way to build several zones on the panel (a
  drop still replaces its one zone), and pick lists for the new choices.
- **Organ instrument.** A polyphonic drawbar organ in the browser: a tone generator with one sine wheel for each
  semitone feeds nine drawbars (16', 5 1/3', 8', 4', 2 2/3', 2', 1 3/5', 1 1/3' and 1') in equal temperament, each
  with nine positions 3 dB apart (0 is off), so two keys that land on one pitch share a wheel and add in phase.
  Sixteen keys sound, with a 1 ms attack and a 4 ms release and no velocity. Percussion adds the 2nd or 3rd
  harmonic with a Fast or Slow decay (60 dB in 0.2 s or 0.9 s) at a Soft or Normal level; it restarts only when a
  key is struck while none is held, and the 1' drawbar is silent while it is on. Key click adds a short burst of
  noise at a key press and a smaller one at a release. The scanner gives the vibrato V1 to V3 and the chorus C1 to
  C3 at 4 to 9 Hz (Off is exact, a change of type glides), and Drive (off is exact) and Level finish the signal
  below a soft limit of 2. Sustain, volume and expression work as on the other instruments and the master tune
  moves every wheel. The Sound choice loads Init, Jazz, Full, Rock, Mellow or Church in one undo step. A bounce is
  bit-identical on every platform and at every host block size, and the stereo output goes straight into the
  Rotary Speaker. The project format stays 43: the device is a new type id and no saved field changed. Not done
  yet: a panel with real drawbar sliders, tonewheel leakage and foldback, a second manual with a pedal bass, and a
  mod wheel that sets the scanner depth or the rotary speed.
- **Rotary Speaker effect.** A rotating-speaker cabinet in the browser: the two inputs are summed, driven through
  a soft clipper (Drive, off is exact), split at a Crossover (200 to 1600 Hz) and sent to a horn rotor (high band)
  and a drum rotor (low band). Each rotor has a slow and a fast speed (horn 0.3 to 2 and 3 to 10 Hz, drum 0.2 to
  1.5 and 3 to 8 Hz) and its own run-up time (50 ms to 3 s for the horn, 200 ms to 8 s for the drum), so a switch
  from Slow to Fast speeds up the way a heavy rotor does; Speed picks Stop, Slow or Fast and a gate on the Speed
  CV input forces Fast. Two microphones hear each rotor through a Doppler delay and a level that follows the rotor
  angle (Doppler and Tremolo amounts), and Mic spread moves them from the same point to opposite sides, which
  gives the stereo swirl. A CV output carries the horn angle. Mix 0 passes the input unchanged, the bounce is
  bit-identical on every platform, and the output stays finite for any setting. The project format stays 43.
  Not done yet: a cabinet resonance model, a microphone distance control and a dry path aligned to the wet one.
- **Copy and paste of CV sequencer curves with their pattern.** COPY on a CV sequencer panel or in the grid editor
  keeps the edited pattern's curve rows A and B with its steps, and PASTE writes both into the target pattern as one
  undo step (the two views share the clipboard, curves included). A copy from a step sequencer pastes its steps and
  leaves the target's curves alone, and a step sequencer never takes curves. Not done yet: copying one curve row
  alone, and a CLEAR that also clears the curves.
- **Feedback routing: the Feedback Route device makes a loop legal.** A cable that would close a loop is still
  refused ("a loop must pass through a Feedback Route"), but a loop that runs through the new Feedback Route
  utility is accepted when you cable it, when a project is read and inside containers. The device delays a stereo
  signal by at least one engine batch (64 frames, about 1.3 ms at 48 kHz), or by Time (0 to 1000 ms, whole
  frames), and scales what comes back by Gain (-60 to 0 dB); a change of either glides over about 50 ms. The first
  delay's worth of output is silence and the n-th frame out is exactly the (n - delay)-th frame in, so a bounce
  of a loop is bit-identical at every host buffer size and with any number of workers. The delay is not reported as
  latency (the loop is not compensated), what enters is limited to plus or minus 16 so a loop with too much gain
  stays finite, and the device has no bypass. Cable the end of the chain into In L/R and Out L/R to where the
  signal should re-enter; one loop may pass through several of them, and a Feedback Route may feed its own
  inputs for a plain repeating delay. The project format stays 43: the device is a new type id, no saved field
  changed. Not done yet: a delay shorter than one batch, a latency-aligned loop, a feedback send and return pair
  for two places far apart in a rack, and a distinct look for a cable that closes a loop.
- **Macro follow-ups: a curve and a smoothing time per assignment, a driven knob that moves its range, assigning by
  dragging a knob onto a macro and a range edit by dragging.** Each assignment in the Macros panel has a CURVE button
  (linear, exp for a slow start, log for a fast start: the travel between MIN and MAX bends, the two ends stay
  where they are, so a cutoff can move in a perceptual way while MIN and MAX stay plain values) and an SM button
  (the smoothing time of a live move: none, 10, 25, 50, 100, 250, 500 ms, 1, 2 or 5 s): when the macro moves, the
  target glides to its new value over that time instead of jumping. The glide is a playing aid; a bounce, an
  automation lane on the macro and a freshly opened project use the plain mapping, so a render stays reproducible,
  and a stepped param always jumps. Turning the knob of a param a macro bank drives in the rack (a drag, the wheel,
  a double click that resets) no longer shows a message and does nothing: it shifts the range of the assignment so
  the param shows the value you set at the macro's position, as far as the param's own range allows (a drag is one
  undo step); the console strips and the plugin parameter pane still lock a driven control. A knob or fader of the
  rack dragged with Alt+Shift (Opt+Shift on macOS, the new role Assign to macro in Settings, Keyboard) onto a macro
  knob of a macro bank assigns it to that macro; a line follows the pointer and the knob under it is outlined.
  MIN and MAX in the Macros panel can be dragged sideways (the whole param range over 300 px, Shift a tenth as fast)
  and one drag is one undo step; a click that does not travel still steps. The project format stays 43: an
  assignment's `"curve"` (written only when not linear) and `"smooth_ms"` (only when set) are ignorable fields, and
  the MCP `session_macros` tool has a `shape` action for both. Not done yet: a numeric entry for a range, a direct
  edit of a driven control in the console and the plugin pane, per-track banks made for you, macros that drive a
  container's inside or another macro, and the other items of the roadmap bullet.
- **Snapshot follow-ups: a morph from the mix as it is, a morph time that is kept, recall at a marker and A/B of one
  strip.** Mixer > Morph to Next Snapshot and Morph to Previous Snapshot (and Morph to this in the chip menu) now start
  from the mix as it is when you press them, not from the current snapshot, so a morph works with no snapshot
  current, from a mix that no snapshot holds and from one you have just moved; with no current snapshot, next is the
  first and previous the last, and a morph is refused only when the mix already is the snapshot. The morph time of
  Mixer > Morph Time is kept in the interface state and comes back at the next start. A snapshot can be tied to a
  marker: the chip menu lists the first eight markers (Recall at marker, and Stop recalling at it) and, while the song
  plays, the playhead crossing a marker of that name (ignoring case) recalls the snapshot, as does starting or
  looping back to just after one; a compare or a morph in progress is left alone. The strip name plate menu has
  Compare strip with the current snapshot: only that strip and the effects on its insert chain go to the snapshot's
  values and flip with Flip strip A/B, the rest of the mix and the groups stay as they are (the status area says
  one strip). The project format stays 43: a snapshot's `"marker"` is written only when set and an older build
  ignores it. Not done yet: snapshots that capture the output routing and the insert chains' members, and recall as
  an automation lane.
- **Console follow-ups: buses and mixer channels in the strip clipboard, meter modes, a peak reset, bus colours and a
  release Auto.** Copy, Cut, Paste and Duplicate in the console now take output buses and channels with no source
  device: they are copied as a name and their settings, and a paste makes a new mixer channel, or a bus on the master
  with a free name (`Drums 2`), in the same undo step as the strips that carry an instrument or a track. The strip
  context menu has Cut, Copy, Paste and Duplicate rows. Mixer > Meter Mode picks the ballistics of the strip meters:
  peak (the default), PPM (the peak bars fall 24 dB in 2.8 s) or VU (the RMS bars integrate over 300 ms); it is kept
  in `ui-state.json`. The master's L and R peak readouts keep the highest peak, and a click on either one resets
  them with the holds and the clip lamps. A bus takes the colour of its device (click the band of its name plate to
  pick one), and the output selector and its menu show a swatch in the colour of the bus a strip plays into. The
  toolbar's latency readout adds `TOTAL` with the latency of the whole mix when the master's chain adds some. The
  master bus compressor has an Auto release (`comp.auto`, an AUTO button beside the release knob, off by default and
  not saved while off): a slow average of the gain reduction blends the release between 60 ms and 1.2 s. Project
  format unchanged (43). The master's settings cannot be copied yet (the master cannot be selected as a strip), and
  the remote-control base for the strip surface, auto make-up, the E band mode, key-input auto-on and a level CV on
  the master are not built.
- **Track organisation follow-ups: stacked bars in small tracks, comments of several lines and folding the
  controller lane.** In a track under 32 px a note clip drops its header band and draws its notes as stacked bars
  over the whole clip (the name is small text on them, and the mute button is left out). A track comment is typed in
  a box of five lines over the header, with the caret, selection, clipboard and input method of the other text
  fields: Shift+Enter breaks the line, Enter or a press outside commits, Escape cancels. The controller
  area of the piano roll has a chevron at the left of its chip row that folds the lane away and keeps the chips,
  like a folded automation lane; picking a lane chip unfolds it. Project format unchanged (43). Routing the tracks of
  a folder to an output bus and a folder-level fader are not built yet.
- **Takes: a razor and comping several tracks at once.** With the Razor tool, a click in a take row cuts the comp
  at the pointer: from there to the end of the comp segment under it (or to the next segment when the comp is silent)
  plays from that take; a drag with the razor comps the dragged range like a plain drag, and a click that would change
  nothing sends nothing. When several tracks are selected and the track of the group is one of them, a comp drag or a
  razor cut in a take row applies the same take number and range to the groups on the same lane of every selected
  track, as one undo step (`SetLinkedComp`; a group without that take is left alone, and the rows of the linked
  groups preview the range). Project format unchanged (43): nothing new is saved. The link is the track selection, not
  a saved property of the groups, and takes for automation passes are not built yet.
- **Quantize and groove follow-ups: a live groove per clip, a groove name field, length options, a QUANT button in
  the arrangement and swing that follows the bar lines.** A note clip can play a groove of its own instead of its
  track's (`LIVE ON CLIPS` in the quantize panel at the panel's strength, `CLEAR LIVE` to hand it back; the notes are
  not edited, and renaming or deleting the groove follows on the clips). The panel has a text field that shows the
  chosen groove of the song and renames it (the `RENAME` button or Enter; the tracks and clips that use it follow),
  and two new rows, humanize length (a bounded random change to each note length, reproducible per seed) and legato
  (each quantized note reaches the start of the next selected one). The arrangement's corner has a QUANT button between
  S and MAN REC: it quantizes the selected clips with the panel's last settings, and a right click opens the panel.
  Swing now counts steps from the start of each bar of the meter map, so a meter change or a bar with an odd number
  of steps no longer turns the swing around (a 4/4 song quantizes as before; Quantize Audio follows the bars too).
  Project format unchanged (43): a clip's `"groove"` and `"groove_strength"` are written only when set and are
  ignorable (an older reader plays the track's groove). Controller lane points still do not move with their notes, and a
  groove from a MIDI file or an audio loop is not built yet.
- **Player preview: a play button on every note player.** The name strip of the step sequencer, the arpeggiator,
  the drum generator, the CV sequencer, the note repeater, the note delay, Key & Chord and the note filter has a
  small play button beside BYPASS. With the song stopped, it runs that player and the players after it in the
  stack on a clock of their own at the song tempo, from the start of the pattern, and the notes play through the
  instrument and the mixer channel of the track, so you can hear a player without playing the song; the song
  position does not move and nothing is saved. A second press stops it, as do Play on the transport, a MIDI clock
  that starts the transport, opening another song, restarting the audio engine and deleting the player; stopping
  closes the notes the players hold, and only one preview runs at a time (pressing another player's button
  switches to it). The note-processing players (arpeggiator, note repeater, note delay, Key & Chord, note filter)
  work on the notes played on the Piano Keys or a MIDI keyboard while they preview: the button selects the player,
  which is where live notes go. The device context menu has Preview Player (`rack.preview_player`, no default
  key). The engine takes one preview flag per device on the control path and keeps the clock on the device's node,
  so nothing allocates or waits on the audio thread. Project format unchanged (43): the preview is not saved.
  Players inside a container preview alone (the players after them in the container are not started).
- **MIDI learn follow-ups: NRPN and RPN sources, MMC as a transport, port and channel cells and the preset of one
  device.** The controllers that make up a registered or non-registered parameter number (99 and 98 or 101 and 100
  to select it, 6 and 38 for the data entry, 96 and 97 for increment and decrement) are followed per input port and
  channel, so two controllers or two channels never mix their selections. A completed run is a source of its own: MIDI
  Learn learns an NRPN or RPN as that number (a knob that sends one is no longer learned as CC 99, 98 or 6), the
  mapping panel and the project file show it as `NRPN 300` or `RPN 2` (project format unchanged, 43: the `"kind"` of
  a source may now be `"nrpn"` or `"rpn"` with a `"number"` up to 16383, and an older build refuses a project that
  uses one). The data entry is a 14 bit position (a sender that never sends the low half still reaches both ends of
  the range), data increment and decrement move the target by detents of the mapping's step in the absolute and the
  relative modes, a relative encoder may read the data entry like a 7 bit controller, buttons are down from the
  middle of the range and a transport target presses on the data entry or an increment. A mapping on a plain
  controller that is part of a run (CC 6, say) still hears it, and a mapping that consumes keeps the whole run away
  from the notes of the armed instrument. A port set to the mapping role in Settings > MIDI now also reads MIDI
  Machine Control: play, stop, pause, record strobe and exit, and locate (a time in hours, minutes, seconds, frames
  and subframes at the four SMPTE rates, turned into a song position through the tempo map) run as the transport
  buttons do, and rewind and fast forward step a bar. While MIDI Learn waits for a control, or while the transport
  follows an external clock, they are held back with the usual message. The rows of the MIDI Mappings panel have
  a PORT cell and a CH cell: a click steps the source on to the next input port (Any port, then the ports set up in
  Settings, then the ports the mappings already name) or channel (Any, 1 to 16), a right click back and the wheel
  both ways, each one undo step; the source of a control surface keeps its surface, and the panel is wider. The
  DEVICE button of the panel saves the mappings of the selected device (the focused device of a surface, else the
  selected one) as the preset of its type: its params are addressed as `device_type` targets, so the preset applies
  to the selected device of that type in any project, or to the first one when the selected device is of another
  type, wherever it sits in the rack, which is how a plugin keeps its mappings (a plugin's type names the plugin);
  IMPORT applies it. The MCP tools send NRPN runs and machine control SysEx through `session_midi_input` (the
  answer has `playing` and `position`) and `session_midi_learn` has `export_device`. Not done yet: HUI and other
  surface protocols, OSC, level meters, flip and the function keys on the eight-fader driver, an extender unit,
  sends and plugin parameters on the V-pots, learning the parameters of a plugin whose editor window is open, a
  merge group for the tempo and for several controls moved at once, an MMC device number and a per-port switch.
- **Snapshot follow-ups: effect parameters, a morph, a note and a colour, 64 snapshots and keys.** A mixer snapshot
  can hold every parameter of the effect devices on the insert chains and the FX returns, not only their bypass: the
  chip menu has Include the effect parameters (it takes the effects as they are now) and Keep only the effect bypass,
  Update keeps the choice, and A/B compare and recall follow it. Mixer > Morph to Next Snapshot and Morph to Previous
  Snapshot move the mix from the current snapshot to its neighbour (or, from the chip menu, to any snapshot) over the
  morph time of Mixer > Morph Time (1, 2, 4 or 8 seconds). Each frame sets the mix to the point the clock gives
  through ordinary parameter edits, so the strips' own ramps smooth it: a level or a frequency moves along its own
  scale, a switch or a choice flips at the half way point, a group's level moves in dB, and the whole morph is one
  undo step. Any other edit takes the mix over where it is, and a morph does not start while a compare is on or
  another morph plays. A snapshot has a note (Edit note in the chip menu; it shows in the status area while the
  snapshot is current and in the chip's tooltip) and a chip colour (Next colour steps through the palette, No colour
  clears it; the colour is a bar along the foot of the chip). A console now holds 64 snapshots (it held 16), and the
  chips that do not fit the bar scroll: the mouse wheel over the bar or the arrows that show at either end move the
  first chip, and the bar scrolls by itself to the chip of the snapshot that becomes current. Recall Next Snapshot,
  Recall Previous Snapshot and Compare A/B have default keys: `Ctrl+Alt+PageDown`, `Ctrl+Alt+PageUp` and
  `Ctrl+Alt+B` (`Cmd+Alt` on macOS) in the standard profile. The project format stays 43: a snapshot's
  `"effects"`, `"note"` and `"color"` are written only when set and an older build ignores them. Not done yet:
  snapshots that capture the output routing and the insert chains' members, recall as an automation lane or a marker
  action, a morph that starts from the mix as it is, a morph time kept between sessions, and A/B of one strip.
- **Channel strip follow-ups: compressor look-ahead, a polarity for each side, bypass-all over the dynamics and EQ,
  a parallel detector choice, a trim readout and a gain-reduction number.** The strip compressor gets a
  look-ahead of 0 to 10 ms (`comp.lookahead`, the LOOK knob in the console's DYNAMICS section): the strip delays the
  audio of its dynamics section while the gains are computed from the signal as it arrives, so the compressor and
  the gate act before a peak gets through. The delay is reported as the strip's latency, so the other channels are
  lined up with it by the same compensation as any latent effect, and it stays when the compressor is off or
  bypassed, so switching the section does not move the strip in time; at 0 the strip is bit-identical to before.
  `input.phase_r` (the Ø R button next to Ø) inverts the right side on top of what Ø does, so Ø with Ø R inverts the
  left side alone, each side with its own 5 ms ramp. `dyn.bypass` and `eq.bypass` take the dynamics (gate and
  compressor) or the EQ out of the signal through the section's own crossfade without losing its settings, and
  Mixer > Bypass All Inserts now also sets them for a section that is in use, in the same undo step as the effects,
  and restores them with the effects (a strip with only a dynamics or EQ section in use can be bypassed too). The
  parallel compressor gets a PEAK button for its detector (`pcomp.peak`; RMS stays the default). The console's trim
  knob reads its value in dB at rest in the place of its label (it showed it only while dragging), and the
  gain-reduction bar of the dynamics section writes the reduction in dB while it moves. The new params are appended
  after the existing ones, so projects, patches and automation keep their ids (project format unchanged, 43). Not
  done yet: a delay-compensated `insert.bypass` on the strip itself, bypass and a B for container effects, a parallel
  compressor with a key input, an EQ or saturation on the parallel path, a pre-insert tap and a choice of tap.
- **Mixer group follow-ups: a VCA that follows a folder, Link Faders, Create VCA in the menus, named group items and
  dragging a track header onto a group.** A VCA group can follow a track folder: Mixer > Create VCA from Folder (and
  the folder row's context menu) makes a group named after the folder whose members are the console channels of the
  folder's tracks, sub-folders included, and the group stays in line by itself as tracks are moved into or out of
  the folder, added, deleted or the folder is dissolved, in the same undo step as the edit that changed them (a
  strip of the folder leaves another group to join it). The members of such a group are not edited by hand
  (assigning to it or out of it is refused with a message), a group whose folder is dissolved or loses its last
  track stops following and keeps its members, and the VCA strip's menu has Stop following folder. The project
  file gets an optional `follows` on a group, written only when set (still project format 43). Mixer > Link Faders
  (kept with the window state, not in the project) makes a fader or send level of a strip in a group move the
  levels of every member of its group by the same dB, selected or not, where before strips moved together only
  while selected. The track header context menu has Create VCA, the folder row menu Create VCA and Create VCA that
  Follows, and once the song has groups the Mixer menu gets Add to Group and Delete Group submenus with a row that
  names each group, where Add to VCA and Delete VCA acted on the last group or on the selection's groups. A track
  header, or several selected ones, dragged onto the group bar at the right edge of another track's header joins that
  group when released (the bars of the group light up while it hovers). Not done yet: automation of the group fader,
  groups of groups, a VCA that routes its members to a bus and a group chip on the rack's device headers.
- **Metering follow-ups: spectrogram, reference curve, loudness history, K-weighting, 16384 points and
  smoothing.** The Spectrum analyser gets a third display, Spectrogram: the fine trace as a scrolling image with
  the frequency across and the newest row on top, one row for every 75 ms of audio over 6 seconds, brighter for
  louder, which scrolls with the audio (a paused engine stops it). FFT size goes up to 16384 points, a new
  Smoothing choice (off, 1/12, 1/6 or 1/3 octave) averages the fine trace across neighbouring bins with a window
  that grows with the frequency (the bands and the loudest-bin readout stay unsmoothed), and Weighting K applies
  the BS.1770 K-weighting to the whole display so it shows what the loudness meter hears. A Reference curve can
  be stored: switching Capture on stores the one-third-octave band levels on screen, kept in
  `spectrum-reference.json` beside the library file (so any project's spectrum can use it), and the Curve choice
  draws it over the trace or the bands, as stored or with its level matched to the signal so that shapes can be
  compared. In the Phase Meter, the new View choice (Readouts or History) turns the loudness box into a graph of
  the short-term loudness of the last five minutes with a line at the new Target (-36 to -6 LUFS, default -14),
  the integrated loudness and the loudness range (LRA, as EBU Tech 3342 defines it) of the programme since the
  last reset. All the new settings are ordinary params, so the device's patch strip saves them as user presets.
  The spectrum panel is one rack unit taller. The new params sit before Bypass in the two devices; projects store
  params by key, so older projects open as they were. Not done yet: the meters as console panels, a second
  device as the reference, and meters for plugin devices. No project format change.
- **Audio to notes follow-ups: pitch bend kept as a lane, several clips at once, progress and cancel, whole long
  clips, and drum kits with key maps.** With Keep pitch bend on (the default; a row of the Audio to Notes panel) a
  pitched note keeps how the voice moves inside it: the new note clip gets a pitch-bend lane that follows the
  notes (scaled for the default bend range of 2 semitones, each note's curve starting with the note and holding
  until the next one, the bend recentred between notes), where before the pitch was rounded to the semitone and the
  rest lost. Every selected audio clip is extracted now, one after the other, each into a note clip of its own on
  its own new track (or on the selected note track) and each one undo step. The analysis reads the whole of a clip,
  whatever its length, a minute of audio at a time (it was cut at ten minutes and read the clip into memory at
  once), so memory no longer grows with the clip; velocities are judged against the loudest note of the whole
  clip. The status line counts the percent while it runs (with the clip number when there are several), and the
  Audio to Notes panel opened meanwhile shows a bar and a CANCEL button that stops the work within a minute of
  audio and adds nothing for the clip it was on. In Drums mode the new Drum kit row can tell three toms and a
  crash from the kick and the hats (a tom holds its pitch after the hit where a kick sweeps down; a hat that rings
  for 600 ms is a crash), and the new Drum keys row writes the drums on General MIDI keys or on the pads in a row
  from the first pad of the drum machine (kick, snare, closed hat, open hat, low, mid and high tom, crash). The
  MCP tool `session_audio_notes` takes `keep_bend`, `kit` and `drum_keys` and returns `bend_points`. No project
  format change.
- **Freeze and bounce follow-ups: bounce a time range, and render a frozen track again.** Track > Bounce Range in
  Place renders a time range instead of whole clips: the range selected on a lane of a track, or else the range
  between the locators on each selected track that plays a note or audio clip in it. Everything the track plays in
  the range goes through its instrument and insert chain (with the tail, like Bounce in Place) into one clip on a
  new audio track that starts where the range does; a clip that crosses an edge of the range contributes the part
  inside, and the originals stay. Track > Re-render Frozen Track (also in the track header menu) renders a frozen
  track again through the same effects without unfreezing it: the track keeps playing its old render while the new
  one is made on the freeze worker, and one undo step swaps them (undo brings the old render back). It is for what
  the render depends on outside the track: a source that keys the chain, the Freeze with Tail setting, a plugin
  that was updated. The channel strip and the sends were never part of a freeze, so they already stay live on a
  frozen track. New commands `sequencer.bounce_range` and `sequencer.refreeze_track`, without default keys.
- **Audio editing follow-ups: a crossfade length handle, a curve for each fade, envelope points that snap and
  select together, and a strip silence that reads in blocks.** The crossfade where an audio clip starts over an
  earlier one has a grip at the line where it ends: dragging it makes the crossfade longer or shorter (the later
  clip stays, the earlier clip's end follows the pointer, snapped to the grid, previewed while you drag and one undo
  step when you let go), where the earlier clip ends with the crossfade and neither clip plays backwards or loops.
  A right click on a fade handle chooses the shape of that fade alone, so the fade-in and the fade-out can differ
  (Fade Shape in the menus still sets both); the shapes play, bounce and save that way (a project written by this
  version stores a separate fade-out shape only when there is one, and an older reader plays both fades with the
  fade-in shape), and the MCP clip listing names it. Gain envelope points snap to the grid when one is on, as a new
  point does with the pencil; Shift-click or Ctrl-click (Cmd on macOS) adds a point to the selection or takes it
  out, dragging one of the selected points moves them all by the same time and gain (the group stops at the points
  it does not hold), and Delete removes them together. Strip Silence scans a clip a block at a time instead of
  copying its whole span into memory, with the same result as before for any clip.
- **Time-stretch follow-ups: a reversed clip can be warped, a warp button and a tempo field in the clip inspector,
  and the tempo of AIFF files.** A clip that plays backwards can now follow the song tempo and be transposed: the
  stretched audio plays backwards (the buffer is rendered forward from the clip's source offset and turned around,
  so a bounce is the exact mirror of the forward clip and stays bit-identical from run to run). Reverse no longer
  switches a warped clip's warp and transposition off, Warp Audio, Transpose Audio, the warp panel, Scale Tempo and
  stretching a clip's edge accept reversed clips, and a project that holds such a clip saves and loads (a version
  before this one refuses the file). Until the buffer is ready a reversed warped clip plays its file backwards,
  unwarped. Trimming the start of a warped reversed clip cuts the end of its stretched span and leaves its markers
  alone; Detect Tempo, Warp to Song Tempo, Warp Loop as N Bars, Quantize Audio and the slice editor still refuse a
  reversed clip. The clip inspector in the transport bar has a WARP button and a TEMPO field after Reverse
  (they only appear when the window leaves room for them): WARP switches tempo following on and off for every
  selected clip as one undo step, TEMPO is the tempo the audio was recorded at, in BPM with two decimals, dragged,
  scrolled (one BPM a notch, Shift for a hundredth) or typed like the other fields, one merge group per drag, and
  dimmed with a reason until a selected clip is warped. AIFF and AIFF-C files now give the importer what they
  state: the beat count of an Apple loop (`basc`, the tempo follows from the length), a tempo written as
  `120 BPM` in a name, annotation or comment, and an instrument loop flag, so a loop in such a file warps on
  import as a WAV with a tempo does.
- **MIDI file follow-ups: bank selects with their program change, editable program changes and SysEx, SysEx-only
  tracks and ranges read back.** A program change now carries the bank select that goes with it (`CC 0` and
  `CC 32`, either or both): a file's bank select just before a program change at the same tick joins it and
  leaves the controller lanes, the export writes the two controllers just before the program change, and the
  clip's data keeps them through moves, splits, joins and loops (a project from before keeps its bank selects in
  the lanes, which are still written). The piano roll can edit a clip's program changes and SysEx: the MSG
  chip in the controller area shows them as markers (program changes above, SysEx below); a click on the
  program row adds a program change, a drag moves a marker on the grid, the secondary button or Delete removes
  it, the PRG, MSB and LSB steppers change the selected program change, + SYSEX adds General MIDI System On at
  the playhead and CLEAR empties the clip, each edit one undo step. A file track with SysEx and no notes (a
  conductor track with a reset message, or a file with nothing else) is imported as a clip without notes that
  carries the messages instead of being dropped. Range markers the export writes (a marker named after the
  range and one named `<name> (end)`) are read back as ranges by Import MIDI as Song. The project format stays
  43: a program change with a bank is saved as `[tick, program, bank MSB, bank LSB]`, the usual one as before,
  and a clip's data is changed by the new `SetClipMidi` command. Not done yet: per-note expression (MPE), a
  groove taken from a MIDI file, and typing the bytes of a SysEx message.
- **Oscillator start phase and Clean Up Bass.** The Poly Synth, Mono Bass and Wavetable Synth have two new
  params, `osc.phase` (reset or free) and `osc.phase_start` (0 to 360 degrees). Reset starts the oscillators at
  the angle at every note start, so every kick or bass note begins with the same first cycle; free starts each
  note at a seeded random angle. The default, reset at 0 degrees, is what these synths always did, so old
  projects sound the same bit for bit and no format change was needed (the params are appended, after the
  modulation slots on the poly synth and Mono Bass). Right-clicking the patch name of one of these synths now
  opens a pop-up that starts with Clean Up Bass: it sets the phase to reset at 0 degrees, one voice, a 12 ms
  amp release when the release is outside 5 to 20 ms and the filter key tracking on, only where they differ,
  in one undo step, and the status line names what changed. The command is `rack.clean_up_bass`
  The Semi-Modular and FM synth already restarted at phase 0; polarity stays in the Stereo Tool.
- **Kick and Bass Check, and phase rotation.** Mixer > Kick and Bass Check opens a panel where you pick a kick
  track and a bass track. The check renders the two channel strips in one pass on a worker (the first 20
  seconds, deterministic), compares the sub range below 120 Hz of their sum with the power sum of the two for
  every kick hit and draws a strip with one bar per hit (down: they cancel, up: they build up). It also reads
  the two fundamentals and warns when they are within two semitones, where they beat. When something cancels or
  beats it tries the fixes that fit, each one on a copy of the song that is rendered and checked again, so the
  strip next to a fix is what it really gives: start the oscillator of a built-in bass synth at a fixed angle,
  invert the polarity of the bass, rotate its phase, delay it by a few samples, or nudge the fine tune of the
  kick pad of a drum machine. A last row, "fix asymmetry", looks at the bass alone: when it swings further one
  way than the other, it finds the rotation that evens the positive and negative peaks and shows the headroom
  gained (and, since a rotation that is silent alone can change how the bass adds to the kick, the strip the
  song then gives). Apply is one undo step and the status line says what changed; the check never changes the
  song itself. The Stereo Tool has two new params, `rotate` (0 to 180 degrees) and `rotate_hz`,
  an all-pass that turns the phase at unit gain (0 is the input bit for bit, 180 is a polarity inversion); the
  fix for an uneven waveform in `atomik_dsp::best_rotation` finds the angle that evens the positive and negative
  peaks and reports the headroom gained. The command is `mixer.kick_bass_check`; the project
  format is unchanged.
- **Clipper.** A new effect, Clipper (`atomik.clipper`): an input gain drives the signal into a ceiling through a
  curve that runs from a hard clip to a soft knee (under the knee nothing changes; above it the curve leaves with
  slope 1 and settles on the ceiling), with a symmetry control that lowers one half of the wave (it adds even
  harmonics, and an 8 Hz high-pass takes the offset out), off, 2x, 4x or 8x oversampling with the filter latency
  reported (0, 29, 35 or 37 samples) and the dry path and the delta lined up with it, an output gain and a mix. A
  multiband mode splits the sound in two or three bands with the flat-sum crossovers of the mastering device and
  gives each band its own drive in front of its own curve. A delta switch plays what the clipper removes. The panel
  draws the transfer curve with the level of the moment, and reads the clip activity: the share of clipped samples
  and the longest clipped run in milliseconds, which turns warm past one millisecond, the number to judge
  audibility by. With a hard curve, no oversampling, no symmetry, one band, a full mix and 0 dB gains the clipper
  is bit-transparent below the ceiling. A guard (on by default) holds the single-band output to the ceiling after
  the oversampling filters. The oversampler gains an 8x stage (37 samples of latency; images at least 84 dB
  down). The project format stays 44 and no command is added. Not done: a lookahead limiter stage after the
  clipper, a user-drawn curve, and a per-band ceiling.
- **Dynamic EQ (`atomik.dynamic-eq`).** A new effect next to the EQ (no EQ param moves): five bands, a low shelf,
  three bells and a high shelf, each with a static gain, frequency and Q and its own dynamics: a threshold, a signed
  range (below 0 cuts the band while it is loud, above 0 lifts it, 0 keeps it static), a ratio, an attack and a
  release. A band listens to its own band of the input, or to the key input (`sc_in_l`/`sc_in_r`, a lone side
  feeding both, band-passed at the band's frequency; with no cable it listens to its input), and works on the
  stereo signal (the louder side decides), the mid or the side. Idle bands are skipped, so with every band off
  the signal passes bit for bit; there is no latency. The panel draws the static curve, the live curve with the
  gain every band has now, the reach of each band's dynamics and the spectrum of the input and, while the key is
  cabled, of the key; every band has a handle (drag for frequency and static gain, wheel for Q, double click to
  switch it off or on) and every control has a tooltip. The analysers run only while a panel is on screen. The
  format is still 44; not done: the low and high cut, a hold time and a look-ahead.
- **Resonance finder and delta on the Dynamic EQ.** The panel has FIND and CUT buttons in the corner of its plot.
  FIND renders the range between the locators (up to 30 seconds) offline on a worker, with one tap on what feeds the
  device's input, and marks the narrow peaks that stand 6 dB or more above the third-octave trend of the average
  spectrum, strongest first, each with a frequency and a Q estimated from its width (a line, a bar and a rank on the
  plot, and the frequencies in the status line). CUT sets the bells that are off, one per resonance, as dynamic
  cuts that act only while the sound at that frequency is above the trend, in one undo step; a bell that is on is
  never touched. A search changes nothing in the song. New param `delta` (id 56, the output is 55, bypass moves to
  57 and the never-stored analyser switch to 58; the device is new in this release): the device plays the input
  minus the output of its bands, so a cut is heard as the part that was taken out, and a neutral device is silent.
  The format is still 44 and no command is added.
- **Masking map.** Mixer > Masking Map renders every channel of the song (or the selected time range) as a stem on
  a worker, measures each in 27 one-third-octave bands from 40 Hz to 18 kHz and ranks the pairs of tracks by how
  much the louder one covers the other in a band at the same time (the score is the percentage of the time, so a
  clash that lasts is worth more than a blip). The panel lists the worst pairs, draws the two spectra of the chosen
  pair with the clash bands shaded, and a time range moves the playhead to where it starts. DE-MASK on a band adds
  a Dynamic EQ to the covered channel (or uses one that has a free bell band and no other key), cables its key from
  the covering channel's insert send and switches a bell band on at the clash band, cutting half the lead (2 to
  6 dB) while the covering track is loud there; it is one undo step and the status line says what it did. The
  analysis never changes the song. The format is still 44 and the command is `mixer.masking_map`.

- **Mix check.** Mixer > Check Mix renders the channels, the FX returns and the master of the song (or the
  selected time range) on a worker and lists what the mix needs. Each finding names the track, the measured number
  and a line on why: a bass root outside about 40 to 60 Hz (from the bass track's notes), a bass or kick whose
  fundamental is more than 6 dB under its harmonics, a low-mid build-up (the share of the master's power between
  150 and 500 Hz over 40%) broken down by track with the reverb returns, the master's highest peaks with the
  tracks that drive them, a low end under 120 Hz that loses 3 dB or more when the master is summed to mono, and a
  compressor, distortion, Character, amp or mastering device fed more than 9 dB from -18 dBFS RMS. A finding can
  carry a one-click fix, one undo step with a status-line message: a Gain Tool before the device that trims the
  feed to the reference level, a static cut on a Dynamic EQ band of the track that fills the low mids most, or a
  Stereo Tool with Mono on. A peak finding moves the playhead to it and a low-mid finding opens the masking map.
  The thresholds are constants with their reasons in one place. The check never changes the song, the format is
  still 44 and the command is `mixer.check_mix`.
- **Loudness match and a blind A/B test for every built-in effect.** Right-click an effect and choose Match Loudness:
  the effect's bypass wrapper then measures the K-weighted short-term loudness (the Mastering meter's filters, 100 ms
  steps over 3 s) of what goes in and of what the effect makes, and applies a gain, smoothed over about a second, to
  the effect's output so the two read the same. Switching Bypass then compares sound, not level. The gain (never more
  than 24 dB either way, held over silence) shows in the name strip as `MATCH +x.x dB` while the match is on.
  Switched off, the effect is bit-identical to before. The match is a listening aid: it is a hidden `internal.match`
  param that the app sets, so it is not saved in the project, not an undo step, and never part of a bounce or an
  export. Blind A/B Test (same menu) opens a panel for the selected effect: ten trials, each with two sounds X and Y,
  one with the effect on and one bypassed in an order drawn anew for every trial and never shown; play either as
  often as you like, then pick the one you prefer (a pick needs both heard; the panel has its own Match Level
  switch). The result lines say how often the effect won and the exact two-sided chance of a score that lopsided by
  guessing, and whether you reliably prefer it. The test sets bypass on the engine only, hands the effect back as
  the project has it when it ends, and changes nothing in the project. New commands `rack.toggle_match` and
  `rack.blind_ab` (no default keys); the command count is 471 and the format is still 44.
- **Reference tracks.** A song keeps up to 16 reference audio files (Mixer > Reference > Add Reference Track), saved
  in the project as paths in an optional root field (written only when there are any; the project format stays 44)
  and copied into the project's `Files` folder by Collect Files and by a save to a new place, as one undo step.
  One key (Ctrl+Shift+R, Cmd+Shift+R on macOS; Mixer > Reference > Listen to Reference) switches between the mix and
  the selected reference with a 30 ms crossfade, also on a stopped song. The reference plays after the master chain
  on outputs 1 and 2, so none of the master processing touches it, and only live: a bounce, an export, a freeze or
  an offline render never gets one. It starts at the file position equal to the song position, or at a start of its
  own (Start at Playhead Position; Follow Playhead takes it away). The level is matched automatically: the engine
  measures the short-term loudness of the mix (K-weighted, 3 s) while any reference exists, the file is measured
  once for its loudest 3 s section, and the reference plays at the difference, updated slowly (at most 0.5 dB a
  quarter of a second, the first reading at once, silence never moves it); Match Reference Level switches it off.
  Reference Band (sub below 80 Hz, low to 300 Hz, mid to 4 kHz, high above) and Reference Stereo Solo (mid or side
  on both sides) limit the mix and the reference alike, with a 5 ms duck around a change. Not done yet: a reference
  that follows a seek while it plays, a manual trim of the matched gain, and a loudness match over a chosen
  section. The command count is 478.
- **Reference panel and decision zone.** Mixer > Reference > Reference Panel opens a panel that lists the song's
  reference files by name, with the loudest section of each and the matched gain of the selected one (click or Up and
  Down selects, LISTEN is the reference key, Esc closes). Below the list, the decision zone sets the mix against the
  references on seven readings: loudness, the loudest part, punch (sample peak over loudness), brightness (the slope
  of the octave-band energy from 125 Hz to 8 kHz in dB per octave), low end (share of the energy under 120 Hz),
  width (side against mid, dB) and correlation. Each row draws the range the references span as a band and the mix
  as a marker, and says inside, above or below with the distance. All but the two loudness rows read the same at
  every level, so they are compared without matching levels first; the panel marks them. REFRESH renders the mix
  again; the references are read once and kept while they stay listed. The analysis runs on a worker, uses the
  renderer a bounce uses (a song with third-party plugins is refused), never changes the song, and the panel says
  when the song changed since the mix was measured. The same seven readings are in the `zone` entry of the MCP
  `analyze_audio` and bounce reports. The command count is 479.

- **Check Mix opens the clipper and the kick and bass check.** A finding that names a tool now has its button. The
  peaks finding adds a Clipper to the end of the master's inserts in one undo step, with a hard curve, 4x
  oversampling and the ceiling a decibel under the loudest peak the check found (never above -0.1 dB); a master
  that has a Clipper already is left as it is and the status line says so. A bass root or weak fundamental
  finding opens the Kick and Bass Check with the track of the finding picked as the bass (or as the kick when
  its name says kick) and the other one guessed from the names. The check itself still changes nothing.
- **More factory patches.** Four Dynamic EQ patches (Vocal Resonance Tamer, De-boxer, Air Lift, Drum Bus
  De-harsh), four Clipper patches (Master Peak Trim, Drum Bus Soft Clip, Bass Warm Clip, Kick Click Tamer), three
  synths that start every note at the same angle (Clean Kick Sub on the Poly Synth, Mono Sub Bass, Reset Pluck
  Bass on the Wavetable Synth) and a Double Clip container (a soft clipper then a hard one, with Drive and
  Softness macros). The Factory root now lists 42 patches and 14 containers.

### Changed
- **Listen to Reference key.** The default key of Listen to Reference is now Ctrl+Shift+R (Cmd+Shift+R on macOS), in
  place of Ctrl+Alt+R (Cmd+Alt+R), a chord other programs on the computer often claim system-wide. A binding the user
  changed keeps their choice.
- **Menus scroll, stay inside the window and are grouped.** A dropdown or a submenu taller than the window is cut to it
  and scrolls: with the mouse wheel over it, by resting the pointer on the small arrow strip at its top or bottom, or
  with Up, Down, Home and End (the lit row always stays in view). A submenu starts level with its row, moves up before
  it scrolls, and opens on the left when the right side would leave the window; submenus can now hold submenus of their
  own in the in-window menu bar too (Reference > Select Reference Track was out of reach there). The long menus are
  split into submenus by theme, so no menu has more than 30 rows: File (Import, Export, Project Files, Stress Test),
  Edit (Audio Edits, Time and Clips), View (Cables, Panels, Editors, Lanes and Markers, Meters and Links), Mixer (Groups
  and VCAs, Channel Strip, Snapshots, Control and Macros), Track (Note Lanes, Freeze, Bounce, Players, Lane Height and
  Folding) and Audio (Clip Gain, Fades, Crossfade, Warp, Transpose). A new **Analyze** menu after Mixer gathers the
  analysis tools: Sound Analyst, Reference Panel (the decision zone) and the Reference submenu, Translation Lens, Check
  Mix, Masking Map, Kick and Bass Check, Will the Bass Read, Find Repetition and Delivery Targets. Only the menu path
  of a command changed: ids, keys and the command count (the registry still lists 500) are the same. The macOS menus
  nest the same way and rebuild a submenu when a check or a label in it changes.
- **Channels, buses and the master are always in the rack.** View > Show Console Devices in Rack and its command
  (`view.toggle_console_devices`) are gone, with the one-time notice about it; the registry lists 500 commands. An
  older `ui-state.json` that still has `show_console_devices` or `console_notice_seen` loads as before and both keys
  are ignored.

### Fixed
- **Copy and paste between two running instances.** Copying clips, notes, automation points, devices or tracks in
  one Atomik Rack window and pasting in another used to do nothing, because the clipboard lived inside one process.
  Copy and Cut now also put a small versioned text on the system clipboard (the first line reads
  `atomik-clipboard/1` and the kind), and Paste in any instance that finds a copy of another instance there uses it.
  Audio clips and sampler devices bring their files: the paste fetches them by content hash from the audio folder of
  the song they were copied in, and a file that cannot be reached is pasted as missing media with a status line
  that says so, ready for the usual relink. Devices keep the cables between them and plugin devices carry their
  state, with the usual stand-in where the plugin is not installed. A copy over 8 MB, or a system without a
  clipboard, stays inside the instance (the status line says so for a big one); text from other programs is ignored
  by these pastes, a newer or damaged copy is refused with a message, and the Paste menu items enable for a copy
  waiting from another instance. Copies of strips, takes, device settings and controller points still stay in the
  instance they were made in.
- **Shift+drag on an audio clip's end edge loops it**, as it does for a MIDI clip. The clip grows and repeats the
  audio it plays (the same as dragging the loop handle), dragging back trims the repeats, and one drag is one undo
  step. A plain edge drag still resizes, and a reversed clip, which cannot loop, keeps the plain resize.
  A looped audio clip now shows its waveform in every repetition, with each repetition's fades, as it plays; before,
  only the first pass was drawn and the repeats were blank.
- **The browser's sample waveform** is drawn at full quality. The info pane read at most 480 coarse blocks of 4096
  frames, so a short one-shot showed a handful of wide bars. The picture is now read per column (the lowest and highest
  sample of each part, every frame read for a short file), up to 1024 columns, and drawn as a filled shape with two
  columns per logical pixel. Selecting a file again answers from a cache keyed by file, size and modification time.
  Clip waveforms in the arrangement and the saved peak cache are unchanged.
- **Quit > Save** no longer asks again and again. A project whose undo history still held an audio file that no clip
  used any more (a deleted clip, say) was saved without that file in its pool but compared against a pool that kept it,
  so it looked changed right after the save. The saved state now matches the live state, and once a Save in the quit or
  window-close question has worked the song is not asked again; a save that fails or a cancelled Save As leaves the
  window open.
- An **instrument patch** dropped on the rack, double-clicked or loaded from the browser now makes its song track, as
  creating the instrument does: named after the device, with its mixer channel, armed like a created instrument (the
  tracks an earlier auto-arm armed are disarmed; Manual Rec leaves arming alone), all in the load's one undo step. A
  patch of several instruments gets one track each. Effect patches, patches dropped into a container or its slot, and
  Replace Device or a patch dropped on an existing device (the device keeps its track) make no track.
- **Add Channel** in the mixer always has something to add. The picker lists **Empty Mix Channel** first: a channel
  with no source, routed to the master, named "Channel N" (the next free number), added in one undo step, selected
  in the mixer and the rack, with the status line saying what was added. Its input jacks can be cabled by hand from
  the rack back panel. The instruments without a channel follow it; the disabled "Every instrument has a channel"
  row is gone, and Create Mixer Channel names its channel the same way.
- Undoing while a clip start is being trimmed no longer makes the next mouse move send a trim before
  tick 0 that the document refuses; the drag measures from what the document holds.
- A plain **Save** no longer deletes audio from an `Audio` folder that several project files share. The unused
  audio of a bundle folder is tidied only when no other project file sits in the folder (or directly in one of
  its subfolders); where one does, the folder is left as it is. Collect Files gives a song audio of its own.
- Undoing while a clip is being drawn on a lane no longer lets the release add a clip to a track the undo removed.
- **Right-click menus open their submenus without lag.** A submenu opens the moment the pointer is on its row (it used
  to wait about a third of a second plus a frame), and a click on its row opens it at once. Only a switch from one
  open submenu to another waits, for a tenth of a second, so a diagonal move toward the open submenu still keeps it;
  the app wakes exactly when that wait ends instead of on the next event. A context menu taller than the window is cut
  to it and scrolls like the menu bar (wheel, the arrow strips at its top and bottom, Up, Down, Home and End), and a
  submenu near the window edge moves up or opens on the left to stay on screen. The menu bar flyouts already opened
  at once.
- **Tweaking a device inside a container no longer moves the live keyboard to another track.** The rack named the
  selected device by its view-local key inside a container (`synth`, not `container/synth`), so the keyboard did not
  find the armed track the device belongs to and fell back to the first armed track of the song, which then showed
  as the track playing (arrow in the track header, status line). A device inside a container now belongs to its
  container's track, and a selection that is no armed track (an effect, an empty container view) leaves the keyboard
  on the track selected in the arrangement while that one is armed. Tweaking never changes the selected track, the
  selected mix channel or which tracks are armed. Follow Selection and Show Automation Lane read the same document
  key now, so an inner device named like a top-level device no longer shows or adds that device's lane.

## [0.3.0-rc.2] - 2026-11-04

### Added
- **Recording and editing follow-ups: dimmed masked notes and tails, a Piano Keys window, a Save question in its
  own words, fades in ticks and sparse edits of large clips.** The piano roll dims what the open clip does not
  play: the notes (or parts of notes) under a later clip of the same lane, and the tail of a note that runs past
  the end of the clip, which is still drawn so it can be seen and trimmed; a clip with a loop, a muted clip and a
  take are not dimmed for the lane's overlap rule. View > Piano Keys (`F4`) now opens a floating window of its own
  (it moves, resizes and can sit on another monitor), the window's geometry and whether it was open are remembered
  with the other windows, and so are the strip's octave and velocity (`ui-state.json` `piano_keys_octave`,
  `piano_keys_velocity`); the main window's lower panes no longer give up room to a strip. The window is named
  `piano-keys` for the MCP session tools and `--detach`-style options. The question about unsaved changes has
  the buttons Save, Don't Save and Cancel on every platform: a task dialog with those words on Windows (which
  needs a manifest asking for Common Controls version 6, now embedded in the app and the MCP server), the native
  alert on macOS and the zenity question on Linux. The clip inspector's FADE IN and FADE OUT show a small unit
  tag, MS or TICKS: a click on it switches both fades between milliseconds and ticks of the clip's own span (a
  wheel notch or a typed value is then a 16th note or a number of ticks; the file keeps frames). A change of
  the note list of a large clip that keeps its length and touches at most an eighth of the notes (an inspector
  step on a selection, a transpose, a velocity or length change) is stored in the undo history as the changed
  notes only, so a long run of inspector steps on a clip of tens of thousands of notes no longer fills the history
  with copies of the whole list. No saved field changes (the project format stays 43). Deferred: the dimming of
  the masked parts of a looped clip, and remembering the fade unit between sessions.
- **Automation lane and gesture follow-ups: a static value handle, a time range on a lane, Extend to Next and Previous
  Clip, no note audition, trimming several audio clips and remembered snap.** An automation lane that holds a static
  value draws it as a line across the lane with a square handle at the left edge of the timeline; dragging the handle
  sets the value from the pointer height (on the scale of the lane's points) and one drag is one undo step, Esc puts
  the value back. Dragging on a lane with the new Lane range select role (Alt+Shift, Opt+Shift on macOS; remappable in
  Settings, Keyboard) shades a time range, snapped to the grid: Copy takes the parts of the lane's clips inside it, Cut
  and Delete remove them (the clips are cut at the range edges, one undo step) and Paste puts the copy at the
  playhead on the range's lane; any other press or Esc forgets the range. Extend to Next Clip and Extend to Previous
  Clip (`sequencer.extend_to_next_clip`, `sequencer.extend_to_previous_clip`, in the clip menu) grow the selected note
  and automation clips to the start of the next clip on their lane or back to the end of the previous one, the content
  staying where it is in song time, in one undo step; audio clips are left alone. Audition Notes in the notes menu of
  the piano roll (`editor.audition_toggle`) stops notes from sounding while they are drawn, moved or stepped to (the key
  strip still plays) and is remembered. Dragging the start edge of a selected audio clip trims every selected audio
  clip by the same distance, each as far as its own file allows. The snap resolution of the arrangement and of the
  piano roll and the snap mode (grid lines or whole steps) are saved in the window state file and restored at start.
  `SetLaneStatic` gained an optional merge id. The project format stays 43.
- **Looped clip and note lane follow-ups: looped audio takes, an audio loop converted into one clip, and a solo and a
  colour for each note lane.** A take of a group can loop now, audio included: the take menu offers Loop Take x2, x4
  and x8 and Play Take Once (`SetTakeLoop`), the take runs that many repetitions of its extent and the comp segments
  that ended at its end run on with it. A looped note take stays a looped clip; a looped audio take compiles to one
  clip per repetition, each restarting the source with its own fades and warp, so the lane plays what the looped
  take played, and a comp cut inside a repetition fades like a cut in any audio take. Convert Loop > To a Single Clip
  now works on audio: the looped clip is rendered on the freeze worker as the track plays it (gain, fades, envelope
  and warp, before the insert effects and the channel strip) and the clip becomes one plain clip of that render with
  the same place, name and colour, in one undo step (the status line shows the render's progress; looped note and
  automation clips among the selected ones convert at once as before). A note lane has an S button beside M on its
  header (the first lane's chip has no room: use Solo Lane in the track header menu, `sequencer.toggle_lane_solo`):
  while a lane is soloed only the soloed lanes of the track play, in the engine, the bounce, a freeze and the lane
  merge alike, a mute still silences a soloed lane, and the clips of the other lanes are drawn like a muted lane's.
  A Lane Colour submenu (`sequencer.lane_color`, a palette name or `track`) colours a lane's clips and header bar; a
  clip's own colour still wins. Both are saved as the optional `"solo"` and `"color"` fields of a notes lane, written
  only when set and ignorable by an older reader, and `project_inspect` lists them. Bouncing chosen clips in place
  ignores a solo on another lane. Recording to several lanes at once is not done yet (a take still goes to the one
  selected lane). The project format stays 43.
- **Rack editing follow-ups: "+" handles, a Create submenu, unfolding under a cable and replacing a device by
  dropping a library item.** Resting the pointer on the seam between two devices of a rack column, or at the top or
  foot of a column, shows a small "+": pressing it opens the device browser (titled INSERT DEVICE) and the row
  chosen, with Enter or a double click, is added at that slot, wired as a device dropped there is; Esc cancels. The
  rack and device context menus keep Create Instrument, Create Effect and Create Player in one Create submenu.
  A cable dragged over a folded device unfolds it after 0.7 seconds of rest, so a jack on its back can be reached
  (an ordinary fold edit, and the cable drag goes on over the new layout). A device row or a one-device patch from
  the library dropped on the middle of a device of the same kind, or on a track header, replaces that device in
  place as one undo step, routed like the browser's replace: the track, clips, channel and the cables that fit
  stay, the device keeps its key. A patch of another device type loads its sound inside the same step; a patch of
  the device's own type only changes its sound, and what the model refuses (another kind, a console device, a
  frozen track) is told in the status line. The project format stays 43.
- **Restructuring follow-ups: a percent panel for Scale Tempo, a Reverse Clips preference, joining audio clips and
  draggable crossfade zones.** Edit > Scale Tempo > Other opens a panel with one text field for any percentage
  from 10 to 1000 (a comma or a percent sign is understood): the selected clips show the result while you type,
  Enter or APPLY makes it one undo step, and Esc or CLOSE puts the clips back. The panel opens on the last
  percentage. Reverse Clips follows a new "Reverse clips" row of Settings > Interface (notes mirror as bars, the
  default, or only their starts, saved in `workflow.json`); the explicit Reverse Note Starts item and the `bars` and
  `starts` arguments of `sequencer.reverse_clips` do not follow it. Join Clips now works on audio: consecutive
  pieces of one file that touch or overlap, continue each other in the file and agree on gain, warp and mute become
  one clip that sounds exactly as the pieces did (the gain envelope is merged); pieces of different files, a gap, a
  jump in the file, a fade at the join, a reversed or looped clip and warp markers are refused with a message that
  names the clips and the files. Pressing in the crossfade zone at the start of an audio clip and dragging moves
  the cut and the crossfade together: the later clip starts later or earlier and the earlier one ends later or
  earlier by the same distance, with a live preview, snapping to the grid, one undo step, and Esc to cancel
  (`MoveCrossfade` in the model). Dragging a clip edge with Alt, which scales each clip about its own fixed edge,
  already covered edge-drag scaling. The project format stays 43.
- **Six CV utilities: CV math, quantizer, sample and hold, slew limiter, gate processor and envelope follower.**
  CV Math adds, subtracts, multiplies, takes the minimum or maximum, or the absolute value of two CVs (the second
  is a knob until a cable is attached), then scales and offsets the result. The CV Quantizer snaps a pitch CV
  (the one of the sequencers: 24 semitones per unit, 0 at middle C) to the nearest note of any of the piano
  roll's 16 scales on a root, with a transpose and a trigger at every change of note. Sample and Hold keeps the
  CV (or seeded noise, so a bounce repeats) it saw at each trigger, free-running tick or note value of the
  tempo, the last locked to the song position while playing. The Slew Limiter has its own rise and fall time,
  linear or exponential, from 0.1 ms to 10 s. The Gate Processor combines two gates with and, or, xor, nand or
  nor, divides, delays by up to two seconds and sets the length, with a trigger output at every rise. The
  Envelope Follower turns the level of an audio signal (peak or RMS, attack and release, linear or dB curve)
  into a CV and a gate while passing the audio through. All six are in the device browser, bounded under any
  parameters and signals, and documented in the device reference. The project format stays 43.
- **Five new effects: transient shaper, de-esser, auto-filter, tuner and pitch shifter.** The Transient Shaper
  raises or lowers the attack and the sustain of a signal whatever its level (two followers of the stereo power,
  their difference in dB decides), with `attack` and `sustain` CV inputs and a transient CV output. The De-esser
  turns sibilance down in its own band (the band is subtracted by the share that is turned down, so nothing
  else moves) or across the whole signal, with a listen switch and a CV input for the centre frequency. The
  Auto-Filter sweeps a 12 or 24 dB low-pass, band-pass, high-pass or notch filter with an LFO (five shapes,
  free or at a note value of the tempo and locked to the song position while playing, a stereo spread) and with
  the level of the input, with CV inputs for cutoff, depth and rate and an LFO CV output. The Tuner passes the
  audio through, optionally muted, and shows the note, the deviation in cents and the frequency on a needle
  scale like the audio track's tuner, against its own A4 shifted by the project's master tune. The Pitch Shifter
  moves a signal up to an octave either way with a correlation-aligned cross-fade, keeps the stereo image, takes
  a pitch CV input and reports its latency exactly (the delay at a shift of zero, where the output is the
  input delayed bit for bit; 7.6, 15.1 or 25.1 ms at 48 kHz for the three quality settings) so the delay
  compensation lines it up. All five are in the device browser, bypassable, bounded under any parameters, and
  documented in the device reference. The rotary speaker is still to come. The project format stays 43.
- **FLAC export, stems between the locators and a loudness report.** The export panel has a FLAC choice next to
  WAV and AIFF, for Export Song, Export Loop and Export Stems, at 16 or 24 bits (FLAC has no 32-bit float, so
  that button is inert while FLAC is chosen). The encoder is our own, with no new dependency: fixed predictors,
  Rice coding and stereo decorrelation, silence stored in a few bytes, and the MD5 of the samples in the header
  so `flac -t` and other tools can verify the file. It quantises exactly as the WAV writer does, so a FLAC file
  decodes to the same samples as the WAV of the same export, and the same project gives the same bytes. MP3
  stays out until its encoder licence is settled. Export Stems has a RANGE row: the whole song, or only what lies
  between the locators with no tail, lined up and numbered as before. Every finished export now says how loud it
  is, in the toast and in the log: the integrated loudness in LUFS, the true peak in dBTP and the loudness range
  in LU of the audio that was written (after any sample rate conversion); for stems the toast shows the master's
  and the log every file's. The MCP `session_export` takes `container` `flac` and returns a `loudness` object for
  the file and for each stem. FLAC and the range are remembered in `export.json`. The project format stays 43.
- **Console strip clipboard, longer insert chains and strip header buttons.** In the console, Cut, Copy, Paste and
  Duplicate (the Edit menu and the usual keys) work on the selected strips. A copied strip is its channel with the
  instrument or track device that feeds it, the song track and every setting of the channel (levels, sends,
  processing and the insert chain with the settings of its effects; plugins are not copied). Paste puts the new
  strips after the selected strip, Duplicate does the same without touching the clipboard, and each is one undo
  step; Cut copies and then deletes as Delete Channel does. A strip copied in the console can also be pasted in the
  rack or the arrangement, which take its devices and tracks. Dragging a channel name with the copy modifier held
  (Ctrl, or Cmd on macOS) copies the channel to the drop position instead of moving it. An insert chain now holds
  up to sixteen effects (an edit used to stop at four); the INSERTS section still shows four rows, scrolls a longer
  chain with the wheel and shows a thin bar at its right edge, and the track panel lists as many insert rows as the
  chain has. Each channel name plate has RACK and SEQ buttons that show the channel's device in the rack and its
  track in the arrangement, and the coloured band along its top picks the colour of the track when the device has
  one; a track with no colour of its own shows its folder's colour. Buses, the master and channels with no source
  device are not copied. The project format stays 43.
- **MIDI controller data and MIDI file follow-ups.** In the piano roll's controller area you can select several
  points: Shift-click adds or removes a point, a Shift press on empty space drags a rubber band, and Select All
  takes the whole lane. A selected point drags the whole selection (time on the grid, value at the MIDI
  resolution, the shape kept); Delete removes it, Copy and Cut put the points on the clipboard and Paste lays
  them into the lane shown from the playhead (the values keep their place in the range, so a bend can become a
  CC; points past the clip's end are dropped), each one undo step. The new THIN chip (and
  `editor.thin_controllers` with `normal`, `heavy` or `maximum`) simplifies the selection, or the whole lane when
  nothing is selected, dropping ramp points the line reproduces and held values that repeat. `Shift+L`
  (`editor.toggle_controllers`) shows and hides the controller area. The sampler answers the mod wheel (CC 1):
  `wheel.to_pitch` deepens the LFO vibrato (default 1 semitone) and `wheel.to_cutoff` opens the filter (default
  1 octave); with the wheel at rest nothing changes. File > MIDI Export Resolution writes MIDI files at the native
  15 360 ticks per quarter note, 960 or 480 (kept in the preferences; ticks are rounded, a note keeps at least a
  tick). Exported song and track files now carry range markers (a marker named after the range at its start and
  one named `<name> (end)` at its end). Program changes and complete SysEx messages in an imported file are kept
  in the clip and written back on export (bank selects already travelled as `CC 0` and `CC 32` lanes). The
  project format stays 43: a note clip's `"programs"` and `"sysex"` are optional, written only when set, and
  ignorable. Deferred: controller lanes without a note clip, MPE (per-note expression) lanes, a groove taken from
  a MIDI file, editing or clearing a clip's program changes and SysEx, and reading range markers back as ranges.
- **Track organisation follow-ups.** A folder mutes and solos by itself: the M and S buttons of a folder row set
  the folder's own flags, every track in it (and in its sub-folders) plays as muted or soloed with it, the
  tracks keep their own flags, and a track's button shows dimmed while a folder holds it. A folder is dragged by
  its header into and out of other folders, with its tracks and sub-folders, in one undo step. A track with no
  colour of its own shows the colour of the folder it is in. Colour > Custom... in the track and folder menus
  opens a colour picker (a saturation and value square, a hue strip, a hex text and the six palette colours as
  quick picks). Interface > Colour new tracks (off by default) gives each new track the palette colour the
  fewest tracks use. A rubber band over the track headers (drag from the empty part of the header column below
  the last track, or Alt and drag from a header; Shift or Ctrl keeps the selection) selects the tracks it
  touches. The project format stays 43: a folder's `"muted"` and `"solo"` are written only when set, and an
  older reader refuses them. Deferred: stacked bars for clips in tracks under 32 px, routing the tracks of a
  folder to an output bus with a folder fader, a comment of several lines, and folding the controller lanes in
  the piano roll.
- **Takes follow-ups.** Takes copy between take groups and between songs: Copy Take and Copy All Takes in a take
  row's menu (or Edit > Copy on a take row) put the takes on the clipboard, and Paste Takes (or Edit > Paste)
  adds them to a group as new takes on top, each playing over its extent like a new recording, or starts a
  group on the selected lane at the playhead. The clipboard survives New and Open and follows you to another
  open song. Import File as Take adds an audio file (an audio group) or a MIDI file (a note group) to a group
  as a new take that starts where the group starts. Alt+Down and Alt+Up step between the take rows that are
  shown, Alt+P plays the focused take alone and Enter opens it. A take opens in the piano roll or the audio
  editor (Open in Piano Roll and Open in Audio Editor in its menu), and what you change there edits the take,
  one undo step each, with the comp compiled again. New command ids `sequencer.take_next`,
  `sequencer.take_prev` and `sequencer.take_pick`; the project format is unchanged. Deferred: a razor tool on
  takes, comping several tracks at once and takes for automation passes.
- **Live groove per track.** A track can name a groove (one of the song's or a built-in one) and a strength,
  saved on the track and applied when the song is flattened for the engine, the offline render and the MIDI
  export, so the stored notes never change: editing a template re-times every track that uses it, and removing
  the groove gives the original sound back bit for bit (the track's notes move by the offset of the nearest
  grid slot, their velocity follows the slot's scale, both by the strength). The Track Panel has a Groove
  section for a note track: arrows step through none, the built-in grooves and the song's, and a bar sets the
  strength. New commands `SetTrackGroove` and `RenameGroove`; `DeleteGroove` takes the groove off the tracks
  that used it, in the same undo step. The project inspector lists a track's groove. The project format is
  now 43: a reader of 42 would play the notes ungrooved, so it refuses a file whose tracks have a groove
  instead of ignoring it; a project without a live groove differs from its version 42 file in the version
  line alone. Deferred: a groove per clip, controller points that move with their notes, a text field to
  name a groove (the rename command exists), grooves from a MIDI file or an audio loop, and bar lines that
  follow meter changes for the swing parity.
- **Recovery and fault follow-ups.** A recovery snapshot now opens with its audio: the writer leaves a note
  next to each snapshot that has audio, naming the project folder it was taken in, and opening the snapshot
  copies that audio into the new unsaved song (an older snapshot without the note still shows its clips as
  missing). A recovered recording is one undo step, track and clip together. When the chosen audio device is
  lost, or does not open and the last working configuration plays instead, the app reads the device list
  every few seconds and, once the device is listed again, switches back to it by itself (not while the song
  plays or records), with a log line and a toast; that is one try, so a device that is listed but still does
  not open is not retried in a loop, and choosing devices in the settings forgets the one to go back to. A
  device that panics outside `process` (taking a parameter, a sample or pattern hand-off, a meter or latency
  read) is now caught on the audio thread like one that panics in `process`: only that device is silenced
  and named, the others keep playing. No saved field changes (the project format is unchanged).
- **Phase lock for a followed MIDI clock.** The clock input now counts the sender's position (the last Start
  or Song Position Pointer plus one clock's worth of ticks for each clock that arrives while it runs), and
  the player holds the song to it: once per batch it moves the position a few frames towards the sender's,
  closing the difference over an eighth of a beat and never changing the pace by more than 10 %, so a
  sender whose tempo wanders no longer drifts from the song. A difference of more than a beat (clocks lost
  on the cable) relocates; a Start or a Song Position Pointer still jumps to its position. A sender that has
  not said where it is (clocks and Continue only) is followed by tempo alone, as before.
- **Audio clips while following a clock.** Audio clips used to be silent while the song followed an external
  clock; they now play. Each clip starts where the followed song reaches its first frame and plays at its
  own rate, and a locate, the transport starting, a loop wrap or a new song restarts the clips underway from
  the right place. Clips are not stretched to the followed tempo (a long take drifts by the share the two
  tempos differ) and streamed audio stays silent while following. No saved field changes (the project
  format is unchanged).
- **Several open songs.** Window > New Song Window and Open Song in New Window open a song in a window of its
  own (Ctrl+Shift+N and Ctrl+Shift+O, Cmd on macOS); each song has its own undo history, panes, plugins,
  autosave and recovery snapshots, and the Window menu lists the open songs (Next Song and Previous Song,
  Ctrl+Shift+PageDown and PageUp, switch between them). Closing a song's window (Window > Close Song Window,
  Ctrl+Shift+W, or the window's close button) runs that song's own question about unsaved changes and ends its
  plugin instances; quitting asks about every song with changes. The audio device stays unique: only the song
  you are working in has the audio stream, the MIDI keyboard and the control surfaces; clicking another song's
  window moves them to it, and the song that lost them is silent ("another song has the audio") with its
  transport stopped until it is focused again. A song opened next to the first keeps its recovery copies in
  `recovery/song-<n>` and writes no plugin crash journal. With one song open nothing changes, and the MCP
  server still hosts one song. Not done yet: moving clips, devices, channels and patches between open songs,
  separate Input Focus and Play Focus, and recovery of a second song after a crash. No saved field changes (the
  project format is still 42).
- **Manual release test protocol.** `docs/RELEASE-TESTING.md` lists the checks that need a device, a real plugin, a
  display or an installer (audio devices and unplugging, MIDI in, out, clock and control surfaces, plugins and the
  sandbox on each OS, detached windows and mixed-scale monitors, the listening check, installers), each with a
  stable id, its steps, the expected result and the automated test it complements. `scripts/release/test-report.sh
  <version>` writes the report skeleton with the environment captured, refuses a protocol with a repeated id, and
  verifies a filled report per operating system. The release workflow does not require the report yet.
- **Pixel goldens, view properties and a wider allocation guard.** The transport bar, arrangement, piano roll,
  console, rack, a context menu, cut labels and a CJK string have PNG goldens at 1x and 2x (the transport bar and
  the piano roll also at 1.25x), compared by a perceptual tolerance with a per-OS pin and a diff image that CI
  uploads when one fails. Random gesture scripts on the arrangement and the piano roll check that no emitted command
  is refused, that selections stay valid, that a view told the document revision draws what an always-syncing view
  draws and that undoing everything restores the document. Twenty-seven more render test files bounce under the
  allocation guard through one helper, and a test lists every render test that still does not, with its reason.
  Fixed: a double click on an audio lane no longer tries to draw a note clip, dragging a gain envelope point can no
  longer leave its clip, and a note moved onto another note of the same key and tick now replaces it instead of
  producing a note list the model refuses. No saved field changes (the project format is still 42).
- **UI widgets.** The arrangement, console, plugin parameter and container lists that offer rows to pick from
  (input and parameter lists, bus and source lists, take commands, plugin presets, macro and CV lists) now share one
  pick-list widget for geometry, scrolling, hover and frame, so a new list costs its rows only. The file browser's
  search and path fields use the same text field as the Song Information panel: a caret, a selection, the clipboard
  and input-method composition. Names in the file browser and the rename fields are cut and positioned by measured
  text, so Cyrillic and CJK names are no longer clipped or shortened early. The arrangement and the audio editor
  place their hairlines in the middle of a physical pixel, so lines stay one pixel wide at 125, 150 and 175 percent
  display scaling. The rack forgets the view state of devices that were removed. No saved field changes (the project
  format is still 42). Still to do: the remaining menus and label fits that estimate their width, the split of the
  large view files, a view table indexed by pane, screen-reader support, and the dependency of the UI crate on the
  DSP crate (the tuner, FM, strip EQ, shaper, grain and wavetable displays still read it).
- **Refinements.** Edit, Quantize Audio now honours the Quantize panel's swing, groove and humanize time: the
  groove shapes the grid lines, and the timing amount randomises each moved transient by an offset that depends
  only on the seed, the clip and the transient's number, so the same seed gives the same take and the markers stay
  strictly in order. A clip that already has warp markers of its own quantizes those markers instead of detecting
  transients. Edit, Normalise To brings the selected audio clips to a peak of -1 dBFS, an RMS of -18 dBFS or an
  integrated loudness of -23 or -14 LUFS through the clip gain, as one undo step; Normalise Audio still aims at
  0 dBFS. Help, Export Device Parameter List writes a text table (`device | key | name | unit | min | max |
  default | discrete`) of every built-in device parameter in a stable order. View, Detach Browser puts the file
  browser in a window of its own; its position and size are remembered, closing it docks the browser again, and
  the command `view.detach_browser` and the `--detach browser` option do the same. Shift+Up and Shift+Down in the
  arrangement extend the track selection. An armed audio track is monitored live only while the transport is
  stopped or recording, so plain playback no longer doubles the input over the clips. No saved field changes
  (the project format is still 42) and no bounce changes. The tuner button in the track header, the device icon
  double-click, creating a track from a console strip, reversed warped clips, the Recording Meter window and the
  per-track default stretch type are not part of this change.
- **Rack sorting, Track Panel and folder clips.** View, Rack Sorting keeps the rack arranged by track: Rack per
  Track gives every track (its instrument, players, channel, inserts and effects) a column of its own and never
  leaves one empty, Auto-Group keeps the devices of a track together, and No Grouping leaves the columns manual as
  before. The arranging happens in the same undo step as the edit that needed it, and moving one device moves its
  whole track. Right-click, Select All in Device Group selects a track's devices, and View, Single Column lays
  the columns out as one without changing the project. A project saves the mode as the optional `rack_sort` field
  only when it is not No Grouping, so older projects load as before and the project format is still 42; no sound
  changes. View, Track Panel docks a column on the right that shows the selected track's players, instrument,
  four insert slots, sends and fader, with bypass, move up and down and remove on the players and inserts, send
  switches and a fader drag, each one undo step. Folder rows now draw folder clips: a bar for each run of
  overlapping clips in the folder, which selects those clips when pressed, so moving (in time only, never to
  another track), muting, deleting, duplicating, splitting and the razor, eraser and mute tools act on all of
  them. Adding or replacing a device from the Track Panel is not part of this change.
- **Drum designer pads.** Every pad of the drum machine can play a synthesised voice instead of a sample: a
  kick, snare, hat, tom, tone or noise module, chosen with the new pad Module knob and shaped by Body, Snap and
  Sweep (what each does depends on the module). A module pad needs no samples and behaves like any other pad:
  choke groups, retrigger, the 32 shared voices, level, pan, velocity, tune and the pitch CV, the individual
  outputs and the pad's tone, cuts, drive and two sends all apply, and the pad label shows the module name. The
  drum machine panel is one rack unit taller for the four new knobs. Pads left on Sample sound exactly as before
  and the project format is still 42. The creative sampler with live stretch is not part of this change.
- **Semi-Modular synthesizer.** A new instrument, `atomik.semi-modular`, built from slots: three oscillator
  slots (saw, pulse, triangle, sine, one of the 32 additive waves, one of the 12 wavetables with a morph position,
  a band-limited two-operator FM and noise), three filter slots (12 dB low-, band-, high-pass and notch, the 24 dB
  ladder and a comb; series, parallel or split routing), a shaper (soft, asymmetric, hard and fold), three
  envelopes and two LFOs. The eight-slot modulation matrix joins eleven sources (envelopes, LFOs, velocity, key,
  mod wheel, pressure, the sequencer step and a random value per note) to fifteen targets (pitch, each oscillator's
  level and shape, each filter's cutoff and resonance, the drive and the level). A sixteen-step sequencer on the
  device's own tempo-synced clock plays the voices from the keys: the last key pressed is the root the pattern is
  transposed to, with step length (1/4 to 1/32 and triplets), length, swing, gate and ties, and the gate and pitch
  of the sounding step go out on CV jacks. Poly, mono and legato play with glide and at most eight voices; a
  render is the same bit for bit at every host block size. The panel is the generated one (13 U) and the
  matrix opens with the same command as the poly synth's. No existing sound changes and the project format is
  still 42.
- **Real-time stretch preview.** A warped clip is heard stretched at once: while its high-quality buffer is still
  rendering, the clip plays through an interim voice that reads the audio through the clip's time map (tempo
  changes and ramps, warp markers, trims) with Hermite resampling, so a tempo change or a marker drag is never
  silent and never plays the clip unwarped. The interim voice resamples, so the pitch follows the stretch and a
  transposition is only heard once the finished buffer arrives; streamed (very long) files keep playing unwarped
  until then. A CALC chip on the clip header shows which clips are still rendering, and the finished buffer takes
  over with a 5 ms crossfade, so the swap does not click. Bounces and exports never use the interim voice: they
  render every buffer first, so their bytes are unchanged.
- **Plugin CV programmer and aux outputs.** Every plugin device has eight CV inputs (`cv_in_1` to `cv_in_8` on its
  back panel), so a Mod Source LFO, an envelope or a sequencer curve can move a plugin's parameters. Each input has
  a destination (any parameter of the plugin) and a bipolar amount; the device panel is now 3 U with a CV
  PROGRAMMER row under the pinned knobs (a chip naming each destination and an amount knob), and the CV button of a
  row in the PARAMS pane assigns that parameter to a CV input through the same search. The device reads each CV
  once per 64-frame batch, so the result is the same in process, in the sandbox and for every block size. A plugin
  with several stereo output buses gets a pair of `out_aux_<k>_l/r` sockets for each extra bus (up to seven), so
  each part of a multi-output instrument can go to its own mixer channel; CLAP and VST3, in process and in the
  sandbox (the shared audio block grew and its protocol version is 2). The new sockets come after the old ones, the
  assignments are ordinary device params saved only when set, and a project that uses none of it saves and loads as
  before (the project format is still 42). The plugin scan cache is rebuilt once to learn the extra buses.
- **Container CV, sixteen macros, switches and split pass flags.** A container has four CV inputs (`cv1` to `cv4`
  on its back panel); inside, the INPUT strip has the matching CV jacks, so a modulation source cabled to the
  container drives any inner CV input (one jack fans out to several), and the bounce is bit for bit the one of
  the same patch built without a container. A container has sixteen macros instead of eight: macros 9 to 16 are
  on a new MACROS 9-16 tab of the container editor, and "Map to macro" in a control's menu lists all sixteen. Any
  macro can be made a switch with the KNOB / SWITCH button (its targets get their minimum below one half and their
  maximum from one half up); a switch shows an ON / OFF button in its row. Each note split has B, C and P chips
  that stop it passing pitch bend, controller changes or pressure to its device (it always plays its notes). A
  project that uses none of it saves as before apart from the version line: the new fields are written only when set,
  and the project format is now 42, so a reader of 41 refuses a file that uses them instead of failing on an unknown
  field. Deferred: macro sources (wheel, bend, pressure, velocity, key) driving inner parameters at
  engine rate and a custom panel designer.

## [0.3.0-rc.1] - 2026-11-02

### Added
- **Release and dependency hygiene.** A freeze or an unfreeze that finishes while the transport runs no longer
  rebuilds the engine under it: the sound goes on, the message says the freeze plays when playback stops, and the
  engine is rebuilt at the stop. Where plugins run is shown, not assumed: Settings > Plugins says "Running now:
  separate process" or "in-process", the status line shows `plugins: sandbox` or `plugins: in-process` while plugins
  are on, and a sandbox that is switched on but cannot start says so once instead of falling back silently. Waiting
  for plugins to load is bounded by time. `docs/DEPENDENCIES.md` lists the deliberate pins and why, the duplicate
  versions that `deny.toml` skips by root cause (so only a new duplicate warns), and the quarterly bump of the
  0.x GUI stack, which a scheduled workflow opens as an issue; the same workflow prints `cargo update --dry-run` every
  Monday. Every advisory ignore has a review date and the Security workflow fails once it has passed. The nightly
  release uploads its new files before it deletes the old ones, so it is never empty, and a manual run can be a dry
  run (`dry_run`); every build writes a Sigstore provenance attestation for the files in `SHA256SUMS`. The MCP server
  skips a message over 16 MiB with a protocol error and stays usable, and `docs/MCP.md` states the headless, no audio,
  no plugins contract. `SECURITY.md` says how far the plugin cache is trusted and why the macOS app has the
  library-validation entitlement. The sandbox child is an accessory process on macOS (no Dock icon) and a utility
  window on X11; the sandbox end-to-end tests have a CI job on Linux and macOS.
- **Modulation matrix framework.** A device can declare modulation sources and targets and sum up to eight
  slots (a source routed to a target with an amount, and optionally a second source that scales it) at its
  control rate, with no allocation and only `+ - * /`, so the result is bit-identical for every block size and
  worker count. The document stores the slots per device with one undoable command, the amount of a slot is an
  ordinary automatable param, and a project without slots saves, loads and bounces as before (project format
  41). The poly synth and the mono bass use it first: the three envelopes, both LFOs, velocity, key, mod wheel
  and pressure (channel and poly pressure finally have a destination) can move cutoff, pitch and level. A device's
  right-click menu opens its matrix (see below).
- **Modulation matrix panel.** Right-click a device that has a matrix (the poly synth, the mono bass) and choose
  Modulation Matrix: eight rows of source, target and via pickers (a menu of the device's own entries, the current
  one checked) and an amount bar, with the amount from -100 to +100 %. A slot needs a source and a target; until
  both are chosen the row stays a draft, and clearing either side of a slot in use clears it. Picks and the
  amount bar are ordinary undoable edits (a drag is one undo step) and the engine hears them like any other
  parameter change; editing a slot while a note sounds does not click. Esc closes the menu, then the panel.
- **Text fields with a caret, a selection, the clipboard and input methods.** One text field now serves the Song
  Information panel: click to place the caret, drag or Shift+arrows to select, double-click a word, Ctrl+A,
  Ctrl+arrows by word, copy, cut and paste with the system clipboard (the `arboard` library, MIT or Apache-2.0,
  text only), and input-method composition (Japanese, Chinese, Korean, dead keys) with the pre-edit text
  underlined in the field. A character typed with AltGr is text and never runs a shortcut in a text field. The
  in-place name edits of the rack, mixer and arrangement keep their end-of-text editing for now.
- **The piano roll keeps the same notes selected through edits that reorder them.** A quantize that moves a note
  past another, an undo, or a note added before the selection used to leave the selection on whatever sat at the
  old positions; it now follows the notes (by key and order within the key, else by start and key) and goes back
  to the original notes when an edit it could not follow, such as a transpose, is undone.
- **Saved-file corpus and sound pins.** `crates/model/tests/fixtures/corpus/` now holds one real saved project for
  every released format version with a surviving writer (1, 6 to 41), each checked to load without a warning, resave
  canonically and hold the same tracks, clips and notes as pinned; every file that has no audio bundle is also
  bounced and its per-second level (all platforms) and exact hash (Windows x86-64) pinned. The release workflow fails
  when the current format has no corpus file. A list of which sound is claimed bit-identical on every platform
  (`claims.toml`) is checked against the pinned stage blocks, every example project is checked to bounce to the same
  bits at several block sizes and worker counts, and the CI runs the release profile on Windows too.
- **CLAP presets, knobs that follow a preset, and a browser info pane.** A CLAP plugin's presets (those inside the
  plugin and the files in the folders it declares) are listed and loaded in PARAMS like VST3 presets, also from a
  sandboxed plugin. After a preset is loaded, or a patch is changed inside the plugin's window, the rack's knobs show
  the values the plugin now holds, and undo puts them back together with the sound. With Browse aimed at a plugin
  device the file browser lists the plugin's presets as rows (Enter or a double-click loads one). Selecting one audio
  file in the browser shows an info pane under the list: format, channels, rate, length, size, the tempo the file
  states, tags, author and categories, and a loudness picture read on a worker thread. VST3 program lists and a
  background index of the browser are not built yet. No project format change.
- **SFZ and SoundFont import into the sampler.** Drop an `.sfz` or `.sf2` file on a sampler (or load it from the
  sample editor) and it becomes the sampler's zones in one undo step. An SFZ keeps its regions, key and velocity
  ranges, root keys, loops, tuning, volume and pan, amplitude envelope, filter, round-robin, velocity crossfades
  and `<group>` headers; a SoundFont brings its own samples, and its first preset is read (ranges, root, tuning,
  attenuation, pan, loops, volume envelope, low-pass filter). Samples are converted to the project rate and
  stored in the project. Whatever is not read (unknown opcodes, release-triggered regions, modulators, other
  presets) is named once in a message and never stops the import. The readers cap file, region and sample
  counts, never panic on damaged files and have a fuzz target. The loop slicer's eight slots and the sampler's
  eight outputs are not built yet. No project format change.
- **Space reverb, convolution reverb and vocoder.** Three new effects. The Space Reverb adds arena, cathedral,
  spring and echoverb algorithms to the reverb engine, with a wet high cut and a gated tail (hold time and level).
  The Convolution Reverb plays an impulse response from an audio file of the project (drop a `.wav` on the device;
  no impulse files ship): a mono or stereo response of up to 5 seconds, level-normalised, with pre-delay, low and
  high cut, width and mix. It adds no latency, equals direct convolution to float rounding, allocates only when
  the device is prepared, and swapping the response crossfades instead of clicking. The Vocoder is a 4 to 32 band
  channel vocoder with a carrier input (an internal noise carrier when none is cabled) and a hiss path for
  consonants. A gated three-band filter, a bass amp voicing, echo loop jacks and a spectral vocoder mode are not
  built yet. No project format change.
- **Pattern lanes.** The `pattern` lane of a step sequencer, CV sequencer or drum generator is now a pattern
  lane: it reads "Pattern", draws its held numbers as labelled blocks, and a right click on a clip lists the
  device's patterns to pick from (a clip drawn on it starts with one stepped point). A take now catches pattern
  changes without AUTO LANES: the first one adds the lane and each change is written on the next downbeat as a
  held number, so the clip switches on a bar line. Track > Add Pattern Lane adds the lane, and Track > Pattern
  Lane to Notes renders what the patterns play, in the order the lane chose them, into a clip on a new note
  lane. No project format change: a pattern clip is an ordinary automation clip. A pattern offset is not built.
- **Bent automation segments.** Shift-drag on an automation segment bends it so the curve passes through the
  pointer at the middle of the segment (one undo step); a click without a drag, or a Shift-click on a point,
  straightens it again. The bend is a power curve evaluated with plain arithmetic only, so a bounce is
  bit-identical for every block size, worker count and platform, and the value a control shows is the value the
  engine applies. Points added on a choice, a switch or another whole-valued parameter now hold their value
  instead of ramping.
- **Parameter lanes in note clips.** A note clip can carry device-parameter lanes (a cutoff sweep, a send
  level) that move, duplicate, split and loop with the clip; a track's own automation lane of the same
  parameter overrides them. Edit > Move Automation into Note Clip turns a selected automation clip into a
  lane of the selected note clip, and Edit > Clear Clip Parameter Lanes removes them. Recording into a note
  clip and drawing a lane inside the piano roll are not there yet. Both additions are written to the project
  only when used (format 41), and the MCP `project_inspect` lists the lanes.
- **Containers fold and unfold in the rack.** A container no longer has to be opened in a view of
  its own to be worked on: the arrow on the title strip of its panel, or a double-click on the
  panel, draws its devices straight under it in the same column, framed in the container's colour
  between a thin INPUT strip and a thin OUTPUT strip that closes the frame. A container nested in
  an unfolded one unfolds the same way, one frame inside the other. The devices are the ones of
  the rack for every gesture: knobs and buttons, patch strips, selection (it stays inside one
  container), delete, duplicate, rename, the macro menu of a control (it maps to the container
  the device is drawn in), the add key after the selected device, and a drag inside the frame
  reorders. Drag a device of the rack into a frame and it moves inside; drag an inner device out
  of its frame and it moves back to the rack, in the column of its container (the model says why
  when it refuses); drop a browser row or a library item in a frame and it is added inside. On
  the back (Tab) the INPUT and OUTPUT strips show the container's own jacks, so cables between
  the container's sockets and its inner devices are patched as in the container's own view, and
  a cable never joins two levels. A Combine leaves the new container unfolded, so what you
  combined stays in view. Whether a container is unfolded is saved with the project (an optional
  `"open": true` in the container, written only when it is set; the project format is unchanged)
  and is one undo step. The OPEN button still shows the inside in a view of its own.
- **Master tune.** Audio > Settings has a Master tune of -100 to +100 cents around A = 440 Hz. It
  shifts every pitched device (the synths, the sampler, the drum machine, the loop slicer and the grain
  sampler; a fixed-frequency FM operator keeps its Hz) and the reference of the audio track tuner, is
  saved with the song and is part of a bounce, so the file sounds as it was heard. Changing it while
  notes ring glides instead of stepping, and it is one undo step. A value changed in Settings is also
  the tune new projects start with. Audio tracks and plugins are not affected. It is written only when
  not 0, so older projects load and bounce unchanged.
- **Several control surfaces, generic controllers and command mappings.** Settings > Surfaces now
  lists any number of surfaces (up to eight), each with its own driver, ports and an on/off
  switch; an older settings file with one surface loads as a list of one. A new Generic
  controller driver has no fixed layout: its controls are mapped with MIDI Learn, and a mapping
  learnt from a surface hears that surface only, so two controllers can send the same controller
  numbers to two devices at once. A mapping can address the focused device (the device selected
  in the rack) and Mixer > Lock Surfaces to Selection pins each surface to that device while the
  selection moves on. A control can also run a command: undo, redo, previous or next track,
  previous or next preset of the focused device (Mixer > Map Command arms one for the next
  control moved). Undo from a control takes back a whole mapped sweep as one step. The new
  mapping fields are written only when used, so older projects load and bounce unchanged.
- **Alt+drag a clip edge to stretch it.** With the Select tool, hold Alt (the alternate-tool role)
  over the start or end edge of a clip and drag: the clip gets longer or shorter by a ratio, the
  opposite edge stays where it is, and everything in the clip follows. Note starts and lengths
  and automation points scale proportionally, and an audio clip is warped to the new length with
  its pitch kept (it keeps its warp mode, Repitch included; a clip that is not warped yet turns
  warping on with the default engine, and one that plays backwards is refused). Every selected
  clip stretches by the same ratio about its own fixed edge, so each keeps its own start (or end).
  The edge snaps to the grid like a resize does (the disable-snap role works), the clips change
  live while you drag, the release is one undo step and Esc puts everything back; over an edge
  with Alt held the pointer is a split-bar cursor. Alt on the body of a clip keeps the alternate
  tool. The same ratio code as Scale Tempo does the work (the model command is `StretchClips`;
  the project format is unchanged).
- **Insert and Remove Bars at Locators in the ruler menu.** Right-click the ruler: the two
  commands sit under the locator items, next to the Edit menu entries that already did the same.
- **The logo in the About box and in `assets/logo/`.** Help > About now draws the AR monogram at
  the top. `assets/logo/monogram.png` (transparent) and `assets/logo/monogram-black.png` (the same
  on opaque black), both 512 pixels, are derived from the master by the `make_icons` example. The
  headless screenshots draw images too now.
- **MIDI files in the browser.** The file browser now lists `.mid` and `.midi` files (with a MIDI
  label, and `kind:midi` in the search field). Double-click or Enter, or Import in the row menu,
  imports one as File > Import MIDI does. Drag one from the browser or the system onto the
  arrangement: on a note lane it becomes one clip on that lane at the snapped drop position (a
  file with several tracks is merged into the one clip), below the tracks or on another kind of
  lane it makes new tracks from the drop position. Dropped anywhere else it still makes new
  tracks at the playhead.
- **Spectrum EQ window.** View > Spectrum EQ (F2) opens a floating, resizable window with a
  real-time analyser of the selected mixer strip, measured after its EQ and before its fader, and
  the strip's filters and EQ drawn as a response curve. Drag the HF, HMF, LMF and LF points (Shift
  keeps one axis, Alt-drag or the wheel changes a bell's Q, Ctrl-click resets the gain) and the
  high-pass and low-pass corners; a drag is one undo step and records automation. Checkboxes
  switch the EQ, the filters and the shelf-or-bell shape of the outer bands, the arrows pick a
  strip, and Follow Selection keeps the window on the selected one. The master shows the analyser
  only. The strip writes its analyser tap only while the window shows it, so a closed window costs
  nothing and renders stay bit-identical.
- **Sampling into devices.** A SAMPLE button beside EDIT on the sampler, granular, drum machine and
- **Direct Record, Send Players to Track, Fold All and Bypass All Players.** New Track menu items
  act on the selected tracks that have players above their instrument. Direct Record makes the
  take record the notes the players generate (what the instrument receives) instead of the keys
  you play; the flag is saved with the track only when it is on. Send Players to Track renders
  the song offline and writes what the stack generated, from the start to the song end, as a clip
  on a new "Players" lane in one undo step. Fold All and Bypass All fold or bypass every player
  of the stack, and undo as one step.
- **Arpeggiator: step pattern, insert, free rate, shift, repeat and input range.** A row of sixteen
  step cells (rest, on, tie, accent; a click cycles them) shapes the rhythm: a rest is silent, a
  tie lengthens the note before it, an accent plays at full velocity. Insert plays the lowest or
  highest key between the notes, shift transposes the figure, repeat plays each note up to four
  times, and a free rate sets the step in steps per second. Keys outside an input range are passed
  through as played. With all of it off the arpeggiator plays as before.
- **Note filter: fixed note length, polyphony limit and hold.** A fixed length ends each note by
  itself whatever the key does, a limit of 1 to 16 voices either steals the oldest note or ignores
  the new one, and hold keeps notes until it is switched off. No note hangs on an all-off or a
  reset.
- **Key & Chord: custom scale, chord alter and chord size.** Twelve toggles define a scale of your
  own (an empty set restricts nothing), alter moves the third or fifth of a chord (major or minor
  3rd, sus4, flat or sharp 5), and a size limit keeps the lowest one to five notes.
- **Note repeater: repeat mask and chord mode.** A 16-bit mask picks which repeats play (the
  others keep their place and decay), and chord mode starts every repeat with the note.
- **Drum generator: map mode.** The first four patterns become the corners of an XY map; position,
  density and mirror blend them with seeded randomness, so the corners play their own pattern
  exactly and the same hits come back after a locate. A lock per drum role keeps that role on the
  playing pattern. The panel is one rack unit taller for the map knobs.
- **Line Mixer: aux send and return.** Every channel gets an aux send (post-fader, or pre-fader
  with one switch) into two new aux outputs, and two aux inputs come back into the master at their
  own level. A mixer without a return sums exactly as before.
  loop slicer panels records from an audio input pair into a 30 s ring, cuts the silence before the
  sound and lands the sample in the device as one undo step (a drum machine puts it on the selected
  pad). A second press stops, Esc cancels, a right-click picks the next input pair or single
  channel. Sampling and recording audio tracks exclude each other.
- **Sample edits as render operations.** Crop, normalise, reverse and fade in or out render a new
  sample on a worker and swap it into one zone, or every zone that plays the same sample, in one
  undo step; the zone markers stay on the same audio. The pool can count what uses each sample.
- **Drum generator: 32 patterns, flams, ratchets and a resolution and shuffle per pattern.** A step
  can carry a flam (a soft grace hit and the full hit up to half a step later) or a ratchet (two to
  eight even hits across the step). The panel shows 32 patterns as four banks of eight, has RES and
  SHUF steppers for the edited pattern's resolution and swing, and a mode button that makes the
  cells edit velocity, flam or ratchet. The later hits of a flam or ratchet always end, even across
  a jump, a mute or a bypass. Flams and ratchets are saved as optional lane rows, only when a step
  uses them; the CV sequencer also holds 32 patterns (its panel shows the first eight).
- **Drum machine: tone, low and high cut, drive and two sends per pad.** Each pad gets a tilt
  tone, a low cut, a high cut, a drive and two send levels that feed four new shared send outputs
  (`send1_l`, `send1_r`, `send2_l`, `send2_r`). A pad with every effect at its default renders
  exactly as before; the drum panel is one rack unit taller for the extra knobs.
- **Step editor: shift, randomise, alter and invert.** Toolbar buttons and the commands
  `editor.step_shift`, `editor.step_randomise`, `editor.step_alter` and `editor.step_invert` move
  every hit a step, replace the pads in use with random hits, flip a few cells or swap hits and
  gaps. Each is one undo step; the random tools are seeded, so a repeat press gives another take
  and the same press always the same one.
- **Mod Source: LFO cross-modulation, lag, key follow and a 1 kHz rate.** LFO 2 can bend the rate
  and dip the level of LFO 1, a lag smooths all three outputs, key follow scales the free LFO rate
  with the latest key, and a free LFO runs up to 1000 Hz. All new params default to no change.
- **Sampler: zone groups, velocity crossfades, round-robin and per-zone envelope and filter.** A
  zone can belong to a group, fade in and out over a velocity range with an equal-power curve,
  take a slot in a round-robin cycle (counted per group and key, so a bounce and a locate repeat
  it) and carry its own amplitude envelope and filter. The sample editor has a SYNTH page for them;
  envelope and filter edits follow the zone's group. New zone fields are written only when set, so
  existing projects load and render as before. Not in this version: SoundFont and SFZ import,
  eight sampler outputs and the eight-slot loop slicer.
- **Effects: more distortion, an echo, a soft clip, split-band width and a choice of look-ahead.**
  The distortion gains five curves (`tape`, `fuzz`, `modulate`, `warp`, `scream`, each with
  first-order anti-aliasing), a `cut.low` and `cut.high` pair and a `body` bank (resonant or
  cabinet-like, with `body.amount` and `body.size`); its panel is three rack units tall now. The
  new Echo (`atomik.echo`) is a stereo delay whose repeats take a saturator, a low and high cut, a
  bit crusher, diffusion and wobble in the feedback loop, with ducking by the input level, ping-pong
  and three modes: normal, triggered (the repeats are heard while a trigger cable is high) and roll
  (the input is cut and the loop holds). Mastering gains a `softclip` before the limiter, a
  `lookahead` of 0.5, 1.5 or 4 ms (it changes the reported latency, and the ceiling holds at every
  one), and a split-band width: `width.split` with `width.freq`, `width.lo`, `width.hi` and a
  `width.solo` to hear one band; its panel is five rack units tall. Every new param defaults to the
  old sound bit for bit, so existing projects render as before. Not in this version: the
  Pulveriser, the echo loop breakout jacks, the extra reverb algorithms with gate and EQ,
  convolution with impulse responses, the vocoder, the gated filter, the bass amp voicing and the
  modulation and ensemble additions. The project format is unchanged.
- **Synth expansion: wave tables, oscillator modes, a second filter, a mono bass and a dual
  layer.** The poly synth gains params, all defaulting to the old sound bit for bit (existing
  patches and projects render as before): `osc1.table` and `osc2.table` pick one of 32 additive
  waves (band-limited by mip level), `osc2.mode` makes oscillator 2 ring, FM or hard-synced to
  oscillator 1 (`osc2.amount`), `filter.type` swaps the 12 dB filter for a 24 dB ladder with an
  anti-aliased `filter.drive`, `filter2.*` adds a second filter in series or parallel whose
  cutoff follows the first one's modulation by `filter.link`, `lfo2.*` is a second LFO (pitch,
  cutoff, level), `env.mod.*` is a mod envelope with a destination (pitch, osc 2 pitch, osc mix,
  osc 2 amount, filter 2 cutoff), `glide` and `legato` play like a mono synth, `spread` pans the
  voices by key into a real stereo pair, `drift` detunes each voice by a slow seeded random
  amount, and `chorus.*` and `delay.*` are an in-device chorus and ping-pong echo. Two new
  instruments share the voice: the Mono Bass (one voice, legato, glide, the ladder with drive,
  a light chorus) and the Dual Layer (two complete poly synth layers with `layer.mode` stack or
  morph, `layer.morph` and `layer.age`; each end of the morph is one layer exactly). All three
  use the generic panel for now (the poly synth is seven rack units tall); a hand-drawn panel,
  a modulation matrix and per-voice layer splits are not in this version. The project format is
  unchanged.
- **Plugin presets, patch steps and plugin window geometry.** The PARAMS pane of a plugin that
  offers presets has a preset bar: `<` and `>` load the previous or next preset (round the list),
  the name opens the list. A preset is loaded into the live plugin as one undo step, and undo and
  redo hand the state back to the plugin. VST3 presets are the `.vstpreset` files in the platform's
  preset folders (`ATOMIK_VST3_PRESET_DIRS` replaces them); a sandboxed plugin lists and loads
  them through new `ListPresets` and `LoadPreset` messages. A patch changed inside the plugin's
  own window is now an undo step too (stored at the next quiet capture, when the window closes and
  before an undo; a param gesture stays its own step). A plugin's window opens where the last
  window of the same plugin was, position and size, kept in `ui-state.json` and only on a screen
  that still exists. The plugin list of Settings shows ENABLED, DISABLED, UNSUPPORTED, COULD NOT
  LOAD or CRASHED for each plugin; a plugin whose process ended on its own stays CRASHED, also
  after a restart, until RETRY in its row clears it. Not in this version: CLAP presets, VST3
  program lists, presets in the browser, the mini toolbar of the plugin window (Automate, Remote,
  Screenshot), the per-plugin Windows DPI choice, and the panel screenshot.
- **Browser organisation: filters, history, info and favourite lists.** The search field takes
  `kind:` (`audio`, `wav`, `flac`, `folder`, `patch`, `clip`, `item`, ...), `cat:`, `author:`,
  `in:` (a folder the file lies in) and `tag:` next to words, `#tag` and `fav`; all tokens must
  match. The path bar of the file browser has back and forward buttons over the last 64 places
  shown, and a right-click on it lists every folder above the one shown. The right-click menu of
  a row adds Show in Folder (the file selected in the system file manager), Edit Author, Edit
  Categories, and favourite lists: Add to a list, New list and, inside a list, Remove from it.
  The tree lists the named lists under Favourites and has Rename and Delete for each. Authors,
  categories and lists are saved in `library.json`, now version 2; a version 1 file opens as it
  was and is written as version 2 by the next save, and a file that cannot be encoded is reported
  instead of replacing the library with an empty one. A rename of a library item keeps its place
  in its lists. Not in this version: the persistent index and All Locations place, the preview
  level and waveform, `.mid` rows, device palettes with categories and tags, and import-time
  warp and placement preferences.
- **Comp rows: crossfades, silence, take level and offset, and a take menu.** A cut between two
  audio takes in a comp can have a crossfade: the take before the cut runs on past it and the
  take after it fades in over it (equal power, two overlapping regions in the engine, no dip), up
  to the length of the segment after the cut and what is left of the take before it. A cut with
  no crossfade is the 5 ms dip as before, and such projects bounce bit for bit as they did. In
  the comp strip a drag across it silences the range, a drag of a cut moves it, and a crossfade
  has a handle on its tail; the strip's right-click menu sets the crossfade of the nearest cut
  (None, 1/32 to 1/4 note), silences a segment and deletes the takes the comp does not use. An
  audio take row now draws its waveform, and the right-click menu of a take row duplicates the
  take, moves it up or down (what plays does not change), deletes it, and for audio steps its
  level by 3 dB and its audio by 10 ms inside the file. New commands: `DuplicateTake`,
  `MoveTake`, `DeleteUnusedTakes`, `SetCompSilence`, `SetCompXfade` and `SetTake`, each one undo
  step. A comp segment saved with a crossfade has a fourth element (ticks); one without it is
  written as before, so the project format is unchanged (still version 39) and a file without
  crossfades opens in older builds. Copying takes between groups and songs, importing a file as
  a take, a razor tool and keys to step between take rows are not in this version.
- **Resampling.** An audio track can record the output of a mixer strip instead of its inputs: the
  input menu in the track header lists `Channel: name`, `Bus: name` and `Master: name` after the
  inputs, and arming and recording then prints that strip. The engine copies the strip's output at
  the end of each batch, so the clip is sample-identical to what the mixer rendered, and it is
  placed by the strip's own delay (a strip behind a latent effect lands where you hear it on the
  timeline). Inputs and strips can be recorded in one take, each track with its own input gain and
  clip-safe. A track cannot record a strip it feeds (its own channel, or a bus or the master it
  plays into): that take records nothing and the message says to send the track's channel to an
  Audio I/O pair or pick another source. The choice is a new `rec_source` setting of the audio
  track (not a project format change; a project without it records the inputs as before).
  Recording a player's generated notes, Send to Track and the player stack header are not in this
  version.
- **Mix channels, parallel channels and sends that follow the bus.** Create > Create Mixer
  Channel makes a channel with no instrument behind it, wired to the master, whose inputs are
  its Direct In: cable a track or an effect there. Mixer > Create Parallel Channel adds, for each
  selected strip, a channel fed from the strip's new parallel output (taken before the fader and the
  pan), named `P1: name`, played into the same bus as the strip. The name follows the strip's renames,
  removing the strip removes its copies, and soloing the strip leaves its copy audible. A copy can
  have copies; a compressed copy blended in matches the strip's own parallel compressor. Mixer >
  Delete Channel (and Delete in the mixer) removes the selected strips: a strip with no instrument
  goes with its copies, a strip an instrument feeds goes with the instrument and its track, asking
  first (choose it again to confirm) when the track holds clips. Mixer > Sends Follow Bus makes the
  post-fader sends of a strip that plays into a bus follow the bus fader and mute, through nested
  buses too; pre-fader sends are unchanged. It is on in a new project and off in a project saved
  before, which bounces exactly as before. The project format is 39: `send_follow` and `parallels`
  in the console, each written only when set.
- **Utility devices.** The palette gains eight small devices. Gain Tool (gain, balance, width and
  mute, with an aux input to add or cross-fade in and CV on gain and balance), Stereo Tool
  (polarity, width with a bass bypass, balance, mono and mid/side encode and decode), Gate (a
  gate or 2:1 expander with hold, sidechain key filters and a gain-reduction CV out), Gate +
  Compressor (a gate then a compressor with input gain, makeup and mix), Sidechain Tool (pump
  from the song position, trigger from the hits of a key, or classic ducking from its level, with
  depth, attack, recovery time and curve, and a CV out for ducking other devices), Envelope Filter
  (a state-variable filter swept by an ADSR started by a gate CV, notes or the input level, with
  an envelope CV out), Spider Audio (four summing stereo merge inputs and a four-way split) and
  Spider CV (four trimmed summing CV inputs and two split-and-invert inputs). The six effects have
  the BYPASS switch. Their level, gain and time maths is platform-independent, so a bounce is
  bit-identical everywhere, and a pumping Sidechain Tool gives the same gain after a locate as
  playing through. The Sidechain Tool's curve is set by attack, time and curve knobs; there is no
  hand-drawn curve editor yet.
- **CPU meters, an automatic worker count, a free buffer size and a Big Meter.** View > CPU
  Meters shows on every device's name plate the share of the audio time that device takes (a
  container shows the sum of what is inside it), so the heavy device is easy to find; the engine
  times its devices only while the switch is on, costs about 70 ns per device per 64-frame batch
  (half a percent of a callback for a hundred devices), and a bounce is bit-identical either way.
  Audio settings now start the number of render workers from the machine (one per physical core
  but the audio thread's own, at most 8; the Auto chip, the default for new settings, a file that
  chose a number keeps it), the Buffer size section has a Type a size field for any value from 16
  to 8192 frames, and its note keeps showing the buffer and the input and output latency of the
  running stream. View > Big Meter floats a larger meter for one pair of the Audio I/O sockets
  over the rack: VU (reaches 99 % of a steady tone in 300 ms, 0 VU at a reference level you can
  move), PPM (falls 24 dB in 2.8 s) or PEAK, a hold marker that stays 5 s or until you click the
  bar, and a clip lamp per channel that stays lit until you click it. Neither switch is saved.
- **Stems, AIFF export and a dry clip bounce.** File > Export Stems as Audio Files writes one
  file per mixer strip (every channel, bus, FX return and the master) into a `Bounced <song>`
  folder, numbered in mixer order, with the same range and tail as Export Song, the same depth,
  dither and rate choices, and a progress bar with Cancel; a stopped export removes what it wrote.
  The stems are rendered in one deterministic pass of the same engine (a few strips per pass, so a
  long song with many strips does not need the memory of all of them), each strip is delayed by
  what the slowest path to the outputs adds on top of its own latency, so the files line up with
  each other and with the mix (and the channel stems add up to the master when nothing is sent
  to an effect), and CHANNELS can take the channels before their strips (the source's own sound,
  no fader, pan, insert or send). Every export has a FILE FORMAT choice, WAV or AIFF (AIFF-C for
  32-bit float), kept in `export.json`; the file takes the extension of the container. Track >
  Bounce Clips to New Track (Dry) renders the selected clips as the instrument plays them, before
  its effects and channel strip, into a new audio track. The MCP `session_export` has `container`,
  `stems` and `stems_dry`.
- **Templates, song information, project archives and a scratch folder.** File > New from
  Template lists the projects of the new Templates folder (next to the library folder, shown with
  Show Template Folder; a project put there appears in the menu within a second). A template opens
  as an untitled song, so Save asks for a name and the template is never written; its audio plays
  at once and is copied into the project folder on the first save, and a plugin that is not
  installed keeps its stand-in as in any project. Save as Template adds a copy of the song under
  its title or file name (a free name, the song itself untouched), and Use Song as Default Song
  saves one as the Default Song that File > New and a start with no project open instead of the
  starter project (Reset Default Song goes back; Close still starts the starter project). File >
  Song Information has the title, author, e-mail, web address and notes of the song (the notes up
  to 10 000 characters): the title shows in the window title, it is one undo step, nothing is ever
  filled in from the system, and the MCP `project_inspect` and `session_status` list what is set.
  File > Export Project Archive writes the project and the audio it uses as one `.atomikz` file (a
  stored ZIP, so any ZIP tool opens it) on a worker with a cancel; Import Project Archive unpacks one
  into a new folder beside it and opens the project. Unpacking refuses an archive with a path that
  leaves its folder, a duplicate, an unknown file, a compressed or encrypted entry or an entry over
  the limits, before it writes anything, and a failure or cancel leaves no partial folder. The
  relink panel has a PROJECT FOLDER button for a project moved without its folder: the files come
  back by their content. Settings > Workflow has a Scratch folder for the audio of a song that has
  no folder yet (the temporary folder of the system by default). The Workflow tab's rows are a
  little closer together to make room. The project format is now 38: it adds the optional
  `"info"` root field; older projects load and bounce as before.
- **Note lane workflows.** Drag a note lane by its label to another place among a track's lanes or
  onto another track's lanes (a line shows where it will land; hold Ctrl, or Alt on macOS, to
  copy it instead). Merge Note Lanes (Ctrl+R, also in the Track menu and the track header menu)
  puts everything a track plays on its top lane, one clip per stretch of sound, and removes the
  other lanes; the track sounds exactly as before, and it is one undo step. Explode Notes to Lanes
  (Track menu, clip menu) gives each pitch of the selected note clip a lane of its own, named after
  the pitch, and Extract Notes to a New Lane (the piano roll's Notes menu) does the same for the
  selected notes; the source clip keeps its controllers. Dub (Numpad3) adds a new lane to every
  armed note track and records the next pass on it, even while a take is running, from that tick
  on; Alt (Numpad6) does the same and mutes the lane of the last pass. The project format is
  unchanged.
- **Drawing and cleaning up automation, recording tempo.** With the pencil, a drag across an
  automation clip writes a point per snap step that follows the pointer, thinned as you draw so a
  long stroke does not leave hundreds of points; a press without a drag still places one point.
  Shift puts the points exactly on the straight line from where you pressed, and Ctrl+Shift (Cmd+Shift
  on macOS) holds the pressed value up to the pointer as a flat range. One stroke is one undo step,
  and Escape puts the clip back. Edit, the clip menu and the track menu have Cleanup Automation with
  Normal, Heavy and Maximum (0.5 %, 2 % and 5 % of the parameter's range): it thins the selected
  automation clips, or every automation clip of the selected tracks, in one undo step. Interface
  settings has a new Automation thinning choice for the same three levels; it also sets how many
  points a recorded take keeps, so that amount is now a preference. The tempo lane header has an
  R button: with it lit, a take also records the tempo you set while it runs (dragging a tempo
  point, typing a tempo, tap tempo), and when it stops the tempo events inside the recorded stretch
  are replaced by a thinned, ramp-like series in one undo step. The take does not retime itself, and
  warped audio follows the tempo map as before. The project format is unchanged.
- **Note editing tools.** The Notes menu of the piano roll (right click on the grid) has Legato
  (the selected notes are stretched to the start of the next one, with an overlap or a gap if you
  like), Join Notes (Ctrl+J: touching or overlapping notes of one pitch become one), Chop Notes
  (cut at the snap step, quarters, eighths, sixteenths or triplets), a Velocity submenu (set a
  value, scale by a percentage, ramp from the first to the last note or fade in and out, randomize),
  Alter Notes (each note, by chance, takes the pitch of another note of the selection), Randomize
  Notes (move pitches by up to some semitones) and Set Pitch, and Select Notes of the Same Pitch
  (a double click on the key column does the same). With nothing selected they work on the whole
  clip, and each is one undo step. The same velocity, alter and pitch tools are in the Note Tools
  submenu of the clip menu and then work on every note clip selected in the arrangement; audio
  clips are skipped. The random tools are repeatable: the result is a function of the settings, the
  clip and the notes, and each use is a new take. The velocity lane is now a pencil: drag over the
  empty part of it and every bar you pass takes the height of the pointer, Shift draws a straight
  ramp from where you pressed, and Alt only changes the selected notes; one stroke is one undo step.
  On a drum machine track the piano roll keys carry the names of the pad samples. Ctrl+J is also
  new in the roll; in the arrangement it still joins clips. The project format is unchanged.
- **Mute, Magnifier and Hand tools.** The arrangement toolbar has three more tools in a second
  row of its corner: Mute (Y) toggles the clip you click, or all the selected clips when it is one
  of them; Magnifier (U) zooms in around the pointer on a click, out with Shift, and to a
  rectangle on a drag, across and down; Hand (I) scrolls the view in both directions when you
  drag and edits nothing. Holding the alternate-tool modifier pairs Magnifier with Hand and Hand
  with Magnifier (Select and Pencil still pair, and every other tool gives way to Select). The
  eraser also deletes by a rubber band: drag from an empty spot over clips and they all go in one
  undo step. The corner's M, S and MAN REC buttons were painted once per tool; they are painted
  once now. The project format is unchanged.
- **Console editing.** The strips of the console can be selected and edited together. Left and
  Right select the next strip (Shift grows the range, the strip scrolls into view), Edit > Select
  All takes every channel and bus, and Delete removes the selected ones. A fader, a send level, a
  mute, a solo or a send switch of a selected strip moves on every selected strip: levels by the
  same number of dB (the offsets between the strips stay, and the whole group stops where the
  highest one reaches the top), switches to the state you clicked, a double-click resets them all.
  A drag of several faders is one undo step, and Escape puts them all back. Mixer > Copy Channel
  Settings takes the settings of the first selected strip (the strip menu also copies only the
  levels, the sends or the inserts) and Paste Channel Settings gives them to every selected strip
  in one undo step: the params of the copied sections, and an insert chain of new effects with the
  same settings (plugins are not copied). Reset Channel puts the selected strips back to fresh
  ones, keeping their output, and Clear Inserts empties their insert chains (the effects stay in
  the rack). The right-click menu of a name plate lists them. Left and Right used to scroll the
  strips: that is now Ctrl+Left and Ctrl+Right (Cmd on macOS). The project format is unchanged.
  Not yet: cut, copy, paste and duplicate of strips from the console, more than four inserts in
  the console, the RACK and SEQ jump buttons, and the strip and master additions in the roadmap.
- **Looped clips, and clip names and colours.** Every clip has a loop handle in the upper right
  corner of its body (it shows while the clip loops, is selected or is under the pointer):
  dragging it to the right repeats what the clip shows, starting a loop of its current content
  when it had none, and dragging back shortens it. The repetitions are marked by bevelled seams.
  Audio clips loop too: each repetition plays the audio from the clip's source offset again, with
  its own fades, gain envelope and warp, cut at the repetition's boundary ticks so it is
  frame-exact whatever the tempo map does; a reversed clip and a take cannot loop, a looped
  clip crossfades with the clip before it only inside its first repetition, and it splits only
  where a repetition starts. The inspector's Loop Length works on audio clips. The clip menu
  gains Convert Loop > To a Single Clip / To Separate Clips (the lane plays exactly what it
  played), Make Loop Instance Independent (the repetition under the playhead becomes a clip of
  its own), Rename Clip... (also a double-click on the clip's header strip) and Clip Colour with
  Use Track Colour. A clip's name and colour replace the generated label and the track's colour,
  follow duplicate, copy and paste, the clip library, a split and a join, and are saved with the
  project and the clip file; the one-clip MIDI export is named after the clip, and
  `project_inspect` lists them and the loops under `named_clips`. Project format 37 gains the
  clip fields `"name"` and `"color"` (cosmetic, an older reader may drop them) and reads audio
  loops, which older readers refuse; no version bump (37 is unreleased). Not yet: looping an
  audio take, and converting an audio loop into one clip.
- **Track management.** A track header can be dragged: the selected tracks (a press on one of
  several keeps the selection) move together to a place of the track list, into or out of a
  folder, in one undo step, with an accent line showing the place (indented by the folder depth)
  and no line where the place would cut another folder in two, where a track is frozen or where
  nothing would change; Escape cancels. With the copy modifier held the drop copies the tracks
  there instead. Edit > Duplicate copies every selected track (devices, console channels and
  clips, not armed) in one undo step and keeps the copies together after the last original.
  Dragging the bottom edge of a selected track's header resizes all the selected tracks by the
  same factor (Alt gives them one height), and Track Height > Minimise / Restore (Alt+Shift+M)
  shortens them to the least height and, pressed again, brings back the heights they had. Track >
  Delete Track and Device removes the selected tracks with their devices (the console channel and
  cables go with them, and one undo brings everything back). A track colour now also colours its
  device, so the rack and the console strip show it, in the same undo step. The project format
  is unchanged. Alt on a folder chevron collapses or expands every folder in one undo step. Not
  yet: dragging a folder row, a folder that mutes and solos its tracks by itself, an automatic
  colour for new tracks, a colour picker, and stacked bars for clips in tracks shorter than 32 px.
- **Rack editing.** Moving, duplicating and pasting an effect or a note player re-routes it as if
  it had been created at its new place, in the same undo step: where it was, an insert chain
  closes up, a send/return slot is freed and the tracks that entered at a player enter at its
  successor; where it lands, the creation rules run (an effect below an insert joins that chain,
  below an instrument it goes to its channel, below the master it takes a send, a player feeds the
  device below it). Shift while dropping, or the Workflow setting Auto-route off, keeps the cables
  as they are; a device whose cables are not the ones the rules make is only moved. Dropping a
  device from the browser on the middle of a rack device of the same kind (instrument, effect or
  note player) replaces it: the new device keeps the key, so the track with its clips, lanes and
  armed state, the console channel, the sends, the cables that fit its sockets, the note entry and
  the place in an insert chain all stay; its params start at their defaults and automation lanes
  for params it lacks are dropped. Console devices, containers and devices of a frozen track are
  refused. Create > Create Player opens the browser for note players (also in both rack context
  menus), and Alt on the fold button folds or unfolds the whole column in one undo step. Cables
  are coloured by what they do (inserts, sends and returns, sidechain keys, audio, notes and
  control), and the cable display has two more modes, selected devices only and everything but the
  console's own routing, with a dot on the jacks of a hidden cable; K walks through all five.
  Edit shows what Undo and Redo will do ("Undo Move Device"), and `session_status` reports it as
  `undo_label`. The project format is unchanged. Not yet: the "+" handles between devices, the
  Create submenu, unfolding after hovering a cable drag, and replacing from the track header or a
  library item.
- **Restructuring the arrangement.** Edit > Insert Bars at Locators opens the length of the loop
  range at the left locator and Remove Bars at Locators cuts the range out and closes the gap, each
  in one undo step: a clip that straddles the point is split, and the clips, take groups, markers,
  ranges, tempo and meter changes, the loop and the song End all move together (the tempo that
  follows a removed range carries on from its start). They are refused while a track is frozen, and
  a take group that straddles an edge of the range is refused too. Edit > Reverse Clips mirrors the
  notes of the selected clips as bars, Reverse Note Starts mirrors only where they start (the first
  note stays on the downbeat), and automation, controller points and audio clips reverse with them;
  two reverses give the clips back. Edit > Scale Tempo (also in the clip menu) plays the selected
  clips at 200, 50, 150, 125 or 75 percent of their tempo: clip starts scale from the earliest one,
  and lengths, loops, notes and automation scale with them. Audio clips are warped to follow, and
  one that plays backwards is refused. Edit > Chop Clips To cuts the selected clips into pieces of a
  bar, half a bar, a beat or 1/8 or 1/16 note from each clip's own start, playing the same.
  Pasting audio clips into a project that lacks their track makes an audio track for them in the
  same undo step. Not yet: the percent dialog and edge-drag scaling of Scale Tempo, a Workflow
  preference for the reverse mode, joining audio clips and draggable crossfade zones.
- **Light theme, application zoom and interface settings.** Settings has a new Interface tab: the
  theme (dark, light, or whichever the operating system uses; a switch repaints every view at
  once and View > Theme does the same), the application zoom (75 to 200 percent for every window,
  also in View > Application Zoom, kept for the next start), the knob range (Normal, Precise or
  Very precise: how far a drag has to travel to sweep a knob or fader) and value tooltips (resting
  on any rack control shows its value). The defaults are how the app looked and felt before, and
  an older `workflow.json` loads with them. The theme and the zoom touch neither the project nor
  the sound.
- **Help and Window menus.** Help > Keyboard Shortcuts (F1) opens the list of every command and
  its keys, Documentation and Report an Issue open the project's public pages, and About shows the
  version, the GPL-3.0-or-later notice and where the third-party licences come from. The Window
  menu brings the main window and a detached Rack or Mixer forward. A double-click on the divider
  between the rack and the song collapses the rack; the next one puts the split back.
- **Project sample rate and channel counts.** File > New at Sample Rate starts a project at 44.1,
  48 or 96 kHz, and File > Convert Sample Rate moves the open project to another rate in one undo
  step: the audio it uses is resampled into the project folder (the originals stay, so undo brings
  them back), clips keep their place in the song, and fades, gain envelopes, warp markers and
  sampler zones follow the new frame counts. Audio > Input Channels and Output Channels set the
  interface channel counts (the settings note now points at the real menu). A change of rate or
  channel count puts the playhead back where it was and keeps playing, instead of going to the
  start. The 88.2 and 192 kHz rates are not offered yet.
- **Performance measurement.** Every speed claim can now be re-measured with one command per
  crate: `cargo bench -p atomik-render --bench render_batch` (bounces of a generated rack, song,
  64-strip console, 200 and 2,000 automation lanes and a 100-track song), `-p atomik-devices
  --bench process` (every built-in device), `-p atomik-dsp --bench blocks` and `-p atomik-model
  --bench document` (save, load, validate, expand and an edit with its undo). Each case prints the
  median and the 99th percentile of 21 samples. The generated documents of the stress options moved
  to `atomik_model::stress`, with new ones for a song of any size and a console of gated,
  compressed strips. The live callback keeps an allocation-free histogram of its own time and of
  the level-barrier wait, and the end of an `atomik-play --stats` run prints its percentiles
  (`cb_p50_us`, `cb_p99_us`, `cb_p999_us`, `cb_max_us`, `spins_p99`). Opening a project logs where
  the time went (parse, build, devices, compile, submit, sync). A scheduled workflow, never run for
  a pull request, keeps the bench output, fails when the instructions of a bench grow by more than
  10 % over the committed baseline, and runs the release real-time-factor check. See
  [docs/TESTING.md](docs/TESTING.md#benchmarks-and-the-performance-job).
- **Combine and Uncombine: wrap devices in a container and unwrap them again.** The new commands
  **Combine** and **Uncombine** (Edit menu and device context menu, `Ctrl+Shift+G` and
  `Ctrl+Shift+U`; `rack.combine` and `rack.uncombine`) put the selected devices into a new
  container, or dissolve a container, in one undo step. The cables between the devices stay
  inside, the cables to the rest of the rack go through the container's two audio inputs, two
  audio outputs and notes input (a notes cable becomes a full-range split), and the project sounds
  exactly as it did, bit for bit, for devices that use no random numbers. A device with a song
  track, a console channel or an FX return passes them to the container (only one such device per
  container; its automation lanes are dropped and a message says so). When the container has no
  socket for a cable (CV, a third audio input or output, a notes output, a second notes source),
  the command is refused and names each cable; the context menu dims the item and the key or the
  Edit menu shows the refusal. A device moves into a container by dropping it on the container's
  panel, and out of an open container by dropping it on **Project** in the breadcrumb or with
  **Move Out of Container** in the context menu. Every container gains a **BYPASS** switch in its
  name strip, like the effects: while it is on the inside is left out and the input goes straight
  to the output. Bypass cuts at once, with no crossfade. The project format is unchanged (bypass
  is an ordinary saved param). `session_status` and `project_inspect` list what is inside a
  container and whether it is bypassed.
- **Align Returns: latent effects on sends, sidechain keys and split paths line up.** A new project
  setting, **Mixer > Align Returns** (undoable, saved in the project), delays every audio cable of
  a device so it lands with the slowest one. A send through an effect that reports latency now
  returns together with the dry channels (no comb filtering), a sidechain key from a latent source
  arrives with the signal it controls, and two paths of different latency that meet in one device
  are put back in step. The click waits for the latency of the audio, so it is heard where the
  music is. New projects have it on; projects saved before keep it off and bounce exactly as they
  did. When it is off, the status line says how many cables are out of step. A channel's
  **latency adjust** now reaches 480,000 samples (10 s at 48 kHz). Project format 37 gains the
  optional console field `align_returns`.
- **Transport bar: tap tempo, Rewind and Forward, locators, click level and Q Rec.** The bar has
  a second cluster right of the readouts. **Rewind** and **Forward** move by bars and repeat while
  held, faster the longer they are held. **TAP** sets the tempo at the playhead from the average of
  your last 2 to 16 taps (a pause of 2 seconds starts over; one undo step per sequence; also the
  command Tap Tempo). **POS**, **TIME** and the new **L** and **R** locator readouts can be typed
  into with a double-click (`5.1.0`, `00:12.500`; a locator is one undo step), and a go-to button
  sits by each locator. A **click level** slider sets the metronome volume and is remembered.
  **Q Rec** snaps recorded notes to any grid of the quantize panel (1/4 to 1/32 and triplets; old
  `off`, `1/8` and `1/16` settings still load), and **QUANT** applies the quantize panel's last
  settings to the selection. When the bar is too narrow for all of it (the recording-input strip
  and the clip inspector keep their room) the controls that do not fit move into a `...` menu.
  The new Workflow preference **Return to start on stop** makes Stop go back to where playback
  started and a second Stop go to the start. The tempo nudge keys now edit the tempo in effect at
  the playhead, like the tempo readout. While recording, a translucent red clip grows with the
  playhead on each armed track, showing the notes played so far, and **Delete** throws away what
  was captured so far while recording goes on. The project format is unchanged.
- **MIDI Out device: play hardware synths from the sequencer.** A new utility device, **MIDI Out**
  (`atomik.midi-out`), sends the notes of a track (or of a note player) and eight CV inputs, mapped to
  controllers with a number, scale and offset, to a MIDI output port. It has a channel, transpose,
  program and bank, and a latency offset in milliseconds (negative sends earlier) that lines a
  synth's audio up with the song. Right-click it and Choose MIDI Out Port, or leave the default, the
  new MIDI Out device port in Audio Settings > Surfaces. Messages are sent when their frame is heard,
  by a dedicated thread that polls every millisecond and spins for the last 0.3 ms; stop, locate and
  a loop wrap release every note the device started, and deleting the device, changing its port or
  closing the session releases them too, so no note hangs. A bounce never sends MIDI, and bounces
  with or without the device are identical. **Project format 37**: a MIDI Out device may carry
  `"midi_out": { "port": name }`, written only once a port is chosen; files without it differ from
  their version 36 file in the version line alone. The output scheduler and thread are shared with
  the MIDI clock output (below).
- **MIDI clock out and in, Song Position Pointer and an Options > Sync menu.** Atomik now sends
  MIDI clock to drum machines and synths and can follow one. **Clock out** (Options > Sync > MIDI
  Clock Out, ports in Settings > Sync) sends 24 clocks per quarter note, exact across tempo ramps,
  with Start, Stop and Continue as the transport changes and a Song Position Pointer for a locate
  or a loop wrap; an offset in milliseconds shifts what leaves a port (one value for every port in
  the dialog, one per port in `settings.json`), the clock can keep running while
  the transport is stopped, and a locate while playing can send Stop, the position and Continue or
  the position alone. It runs on the MIDI output thread, so it works with no MIDI Out device.
  **Clock in** (Options > Sync > Follow MIDI Clock) takes the tempo (the mean of the last beat's
  clocks), Start, Stop, Continue and Song Position Pointer from a chosen input port: the song
  plays at the clock's tempo, the transport buttons say they are following, and a clock that
  stops coming stops the transport and releases every note. The status line shows the clock out
  and the measured tempo. Settings are in `settings.json` under `"sync"`. Bounces never follow a
  clock and are unchanged. Not yet: the phase of the incoming clock is not tracked, so a sender
  whose tempo wanders can drift from the song, and audio clips are silent while following.
- **Several MIDI inputs at once, master keyboard modes and Manual Rec.** Audio Settings > MIDI
  inputs lists every port with a switch: all the switched-on ports are open together (a keyboard and
  a pad controller), and a port can be set to feed mappings only so its notes play nothing. The
  list refreshes every second while the dialog is open, and Audio > Rescan MIDI Ports opens a
  switched-on controller that was plugged in later. `settings.json` gets `midi_inputs`; a file with
  the old single `midi_input` loads as that input, enabled. Notes from the ports are kept in order
  and never stamped more than two buffers ahead, so one port cannot hold back another. Workflow >
  **Master keyboard** is Standard (the keyboard follows the selected track) or Separated (it plays
  the track picked with the arrow on its arrangement header). The arrangement corner has **MAN REC**
  (selecting a track, or creating one, no longer arms it) and M and S buttons that clear every mute
  or solo in one undo step; they are also the commands Clear All Mutes, Clear All Solos and Manual
  Record Arming. `session_midi_input` takes the port a message comes from. No project format change.
- **An unplugged or stalled audio device is noticed, and a device that fails never becomes the
  setting.** When no audio callback arrives for a quarter of a second (or four buffers, if longer)
  the status bar shows `AUDIO STOPPED`, and a driver that reports the device gone shows
  `AUDIO DEVICE LOST`; a toast says so once and points at Audio > Restart Audio. Audio Settings has a
  new choice, **If the audio device is lost**: offer the restart (default) or switch to the default
  devices by itself, keeping the chosen device in `settings.json` for when it is back. Stream
  errors are counted in the status bar. When the input and the output are different devices the
  dialog warns that their clocks drift apart, and shows the buffer and the input and output
  latency; the **input overruns** counter now counts the captured frames that were lost to that
  drift, and **control stalled** counts control messages that found a full queue. Audio settings
  that fail to open, at start-up or in Apply, fall back to the last configuration that opened (with
  a message) and are never saved. `settings.json` gets a `device_loss` object; no project format
  change.
- **Plugin processes that never outlive the app, and hung ones are replaced.** A sandboxed plugin's
  process now ends with the app however the app ends (a job object on Windows, a watchdog thread
  in every child that also covers a child stuck inside a plugin call and a closed control
  channel). A plugin that answers no audio batch for three seconds while playing is stopped and
  reloaded from its last state by itself, twice in a row, then left CRASHED for you to decide;
  bounces are not affected. A plugin that is too busy to answer the interface (the parameter text
  of the PARAMS pane, pins, editor close) is no longer stopped: the last text stays up, a warning
  says it is slow, and only a plugin that stays silent for twelve requests in a row is stopped.
- **Fuzzing of the readers of untrusted files.** `fuzz/` holds `cargo-fuzz` targets for project
  files, patches, clip files, library items and the library database, Standard MIDI Files, audio
  files (the whole-file decoder, the WAV readers and the preview decoder) and the control channel
  of the plugin sandbox, with seed corpora made from the committed fixtures
  (`scripts/fuzz-seed.sh`). A scheduled workflow runs each target for five minutes every week and
  uploads a failing input; it never runs on a pull request, and the fuzz crate is outside the
  workspace, so the normal build and test do not need a nightly toolchain. New tests that run on
  every push cover the same ground on stable: truncated, corrupted and oversized audio, MIDI and
  project files (1 000 cases for MIDI) and an allocation bound for a lying WAV header.
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
- **A bypassed container can no longer close a loop.** Bypassing a container whose output is cabled back to its own
  input, with something else fed from that output, made a saved file the loader refused ("would create a cycle").
  The bypass is now refused when it would do that, so a project that was saved always loads.
- **Device robustness.** A NaN or infinite sample on an audio input of the channel strip, the master,
  the line mixer, the audio track or the stereo delay now reads as silence, so one bad source can no
  longer silence the mix (finite audio is unchanged bit for bit). Two sample map swaps of one device
  are spaced by 5 ms, so a quick third map no longer cuts the voices that are fading out. The built-in
  grain source is kept for the standard sample rates only. A device that cannot be wrapped in a bypass
  is an error instead of a panic, and Character reports a rejected voicing. Every registered device
  passes the parameter min/max sweep, and the instruments, reverb, mastering and Character have
  bounded-output property tests. DEVICES.md lists the overflow and restart policies.
- **A fade or a gain envelope point can no longer run past its audio clip.** Adding or changing an
  audio clip, or stripping silence, refuses a fade or an envelope point longer than the clip. A
  project saved by an earlier version that has one loads with it clamped to the clip and a warning
  naming the clip; the sound is the same, since the player already clamped it.
- **Combine says what it cannot do, and no longer stops at a mixer strip.** Choosing Combine on a
  selection that includes a mixer strip (a channel, a bus or the master) or an audio track used to
  be refused as a whole. Those devices can never go inside a container, so they now stay in the
  rack with a note and the rest of the selection is combined; a selection made only of such
  devices is still refused, naming the first. A container has one song track, one mixer channel
  and one FX return, so two or more selected devices that each have one are refused with a
  message naming them ("Combine keeps one track and one mixer channel: ... each have ...;
  combine them separately"). The Edit menu now enables Combine only when the model would take the
  selection, as the context menu does, so a refused multi-selection is dimmed and a shortcut
  shows the reason in the status line. The project format is unchanged.
- **Insert and Remove Bars at Locators moved some things wrongly.** A tempo or meter change that
  starts exactly on the left locator stayed behind while the music after it moved, so the opened
  bars took the new tempo or the new bar length and the following bars no longer sat on bar
  lines; now it moves with the music (the song's first tempo and meter stay). A loop that starts
  on the locator but is not the locator range moves too. Remove Bars shortens a note that would
  ring on into the removed bars (with note tails on) so it ends where the cut starts. An
  automation curve stays continuous across an insert and joins with the value from the end of the
  range on a remove, and automation-only tracks and device lanes move like the rest (tests now
  cover all of it).
- **The Stereo Tool's MONO switch is the last stage.** It used to come before the Mid/side mode,
  so with Mid/side set to encode the output was the mid on the left and silence on the right.
  Now the two outputs are the same `(L + R) / 2` of whatever the mode produced, in every mode
  (once the 10 ms ramp has settled they are bit-identical); a render test checks left equals right
  with MONO on, and a panel test that MONO is a front toggle on the `mono` param.
- **The drum generator's Generate always wrote the same pattern and Randomise did nothing.** The
  seed only jittered velocities, and only when the density was not zero, so at the default
  density the steps never changed. Now every press of GENERATE picks a new seed (written to the
  Random seed param in the same undo step as the pattern), and the seed varies the rhythm at any
  density inside the chosen style: ghost notes, extra kicks on weak sixteenths, hat, open hat and
  percussion variations, dropped weak hits, accents and sometimes a short fill at the end of the
  pattern. The kick on the one, the strong kicks and the backbeats stay, and the same seed with
  the same params still writes the same pattern. RANDOMISE now draws the style, density,
  variation, accent and the pattern's swing as well as the seed, then writes the pattern, all in
  one undo step. Saved projects store the written patterns and load as they were saved.
- **A patch dropped on a device loads into it instead of adding a new device.** The drop used to
  load only on the small patch strip of the name plate; anywhere else on the panel it inserted a
  new device. Now a patch for the device's own type loads into the device wherever on its panel
  it lands (the panel is outlined while the file hovers, instead of the insertion line), from the
  file browser or the desktop. A patch for another type is inserted right below the device under
  the pointer, as before.
- **Clicking a drum machine pad plays the pad when the notes are cabled in.** A pad, sample key or
  slice pad click sent its note to the top of the track's player stack, so a drum generator in
  front of the drum machine swallowed it and nothing sounded. The click now goes to the device
  that owns the pad.
- **Nudge and track move keys no longer use Ctrl+Alt.** Graphics drivers can take Ctrl+Alt with the
  arrow keys on Windows (they rotate the screen), so the app may never see them. The
  tick nudge of clips and notes is now Ctrl+Shift+Alt+Left and Right (Cmd+Shift+Opt on macOS), and
  Move Track Up and Down are Ctrl+Shift+Alt+Up and Down. Ctrl+Left and Right (the snap step) and
  Ctrl+Shift+Left and Right (one beat) are unchanged and now have tests that drive the real key
  path.
- **A track moved up or down never vanishes into a collapsed folder.** The move keys used to put
  the track inside a collapsed folder, where it was hidden; it now hops over the whole folder. A
  track at the edge of an open folder leaves it in one press, above the folder or below its last
  track, and a track above an open folder moves into it.
- **Join Notes joins across a gap.** Ctrl+J in the note editor (and the Join Notes menu item) merges the
  selected notes of one key into one note from the first start to the last end, where it used to
  do nothing unless the notes touched. Make Legato, which stretches each note to the next one,
  has a key of its own, Ctrl+Shift+J.
- **The Select tool never creates a clip by dragging.** A plain drag on an empty lane is the rubber
  band, across lanes and tracks, like the piano roll's (Shift or Ctrl adds to the selection, a
  plain band replaces it). A click on empty lane area clears the clip selection (an empty
  automation lane still takes the point). Clips come from the Pencil tool, from holding the
  alternate-tool key (Alt) over the Select tool, or from a double-click on the lane.
- **Delete on a lane deletes the lane.** With an automation lane header picked, Edit > Delete (and the
  Delete key) removes that lane alone, as one undo step; an extra note lane goes the same way, and
  a track is only removed when its own header is picked.
- **A click on an empty automation lane draws.** With the Select tool, a click without a drag on
  empty lane area adds a clip over the bar clicked with a point at the pointer, as one undo step;
  a click inside a clip still adds a point there.
- **The followed automation lane can be folded.** The lane that follows the selected parameter is
  kept open, but its fold chevron now folds it; it stays folded until another parameter is followed
  or the lane is unfolded.
- **Joining clips no longer makes muted notes audible.** The notes of a muted clip used to be
  folded into the audible clip a Join made. A join now leaves muted clips where they are (muted
  clips on their own still join into a muted clip), and a looped clip is unrolled instead of being
  refused, so the joined clip plays what the clips played.
- **Closing a project with many sandboxed plugins is quick.** The plugin processes are asked to
  end together and waited for on one shared deadline, so slow ones cost about a second in total
  instead of up to three and a half seconds each.
- **A misbehaving plugin process cannot exhaust the app's memory.** Replies and notifications from
  a plugin process are bounded before they are read (1 MiB of JSON, state blobs only on the
  state, 4 KiB log lines, editor sizes up to 16 384 pixels), a burst of parameter edits reaches
  the app as the last value per parameter, and the scanner's output is read to its end, so a
  file that makes the scanner print more than 16 MiB fails at once as "too large" instead of
  after the scan timeout.
- **The plugin audio block is private.** Its file has a random name, is created exclusively with
  mode `0600` on Unix, and files left by an app that crashed are removed at the next start.
- **A hostile or corrupt file can no longer make the program run out of memory.** A WAV header that
  claims billions of samples no longer sizes an allocation (a 44-byte file that said so used to be
  asked for gigabytes, and running out of memory aborts the program with unsaved work); an audio
  file holds at most 2^30 samples in memory (4 GiB, about 3 hours of 96 kHz stereo) and says "audio
  too long" past that; a project, patch, clip or library item file over 256 MiB is refused before
  it is read, with a message that names its size (set `ATOMIK_MAX_PROJECT_MIB` to open a larger
  one); and a Standard MIDI File keeps at most about two million events per track and eight million
  in all (a longer one is refused with a message).
- **NaN and infinite samples in an audio file become silence.** An import stores them as zero and
  says so in the toast ("2 invalid samples (NaN or infinite) replaced with silence"); a bounce of a
  project whose bundle holds such a file is finite and lists the count in its warnings; the
  streaming reader does the same. A file without such samples reads bit for bit as before.
- **Audio files above 4 GiB stream.** The seekable reader of long recordings and imports reads RIFF
  and RF64 files with 64-bit offsets (it used to stop at the 32-bit limit of its WAV library), and a
  data size that the file cannot hold is cut to the file instead of failing later.
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
- **Play and Stop no longer wait on a parked retirement.** Transport commands travel on a lane of
  their own that applies even while the engine holds its control queue for the UI to collect
  retired plans, the retirement queue is four times larger, and the status line counts the batches
  the control path stood still.
- **A note off is never the note that is dropped.** The last slots of a device's note input and of
  the live MIDI queue are kept for note offs and All Off, so a dense controller lane or a pile of
  chords that overflows a batch cannot leave a note hanging. Nothing changes below the overflow line,
  so existing bounces are bit-identical.
- **A record tap whose stop was refused is stopped on the next frame** instead of recording on
  behind the finished take.

### Changed
- **Faster and steadier continuous integration.** Coverage no longer runs on every pull request: it runs on pushes
  to `main`, nightly, by hand and on a pull request that carries the `coverage` label, and it now also checks a
  floor for each crate (`scripts/coverage-floors.txt`) and prints a table in the job summary. A change to documents
  only skips the build and test jobs. Only `main` saves the build caches. The test plugin libraries are built once
  by the workspace build instead of by a nested `cargo build` in each test binary (the nested build stays as the
  local fallback). The live deadline test is strict off CI, retries only on CI and reports its attempts. The export
  cancel test and the child shutdown tests no longer depend on how fast the machine is, and the locator test draws
  with the Pencil tool. A weekly job keeps the `cargo build --timings` report.
- **Stricter device descriptors and a safer model API.** A device descriptor whose stages skip a
  number, start above 0 or use 255 is refused when the type is registered. A plugin whose layout is
  refused leaves nothing behind, `Registry::get` reports the descriptor of a plugin type instead of
  a placeholder constructor, and a tempo or meter map is always built non-empty. `Document::undo`
  and `redo` no longer take a registry, and `Document::begin_gesture` ends a drag's merge group by
  itself when it goes out of scope.
- **The rack navigator is a miniature of the real rack.** The strip at the right edge of the rack
  no longer shows colour blocks: every device is drawn as a scaled-down picture of its own panel
  (plate, name, knobs, faders and displays as they stand, a folded panel folded), so you can tell
  the devices apart and find the one you want at a glance. Text too small to read becomes a bar
  and very small details are left out; when the rack is so tall that a panel would be only a few
  pixels high the strip falls back to a flat block of the device colour. Clicking and dragging
  move the view exactly as before, and the viewport rectangle and the selection outlines stay on
  top. The miniature is recorded once per panel change, not per frame, and the live meters and
  scopes do not make it redraw during playback (the strip keeps its 136 px width).
- **Spider Audio and Spider CV are now Audio Split/Merge and CV Split/Merge.** Only the names
  shown in the browser, the panels and the documentation change; the device ids are the same, so
  saved projects and patches load as before.
- **The application icon sits on a black background.** The window, taskbar, Explorer, installer,
  Dock and Linux icons are regenerated from the logo on opaque black so they read on any
  taskbar.
- **Saving no longer freezes the window.** File > Save and the file picked for Save As write the
  project beside the window: the plugins' states are collected first (all at once, so a project with
  many plugins waits for the slowest one), then the project text is built, the audio is copied and
  the file is written on a worker while "Saving..." shows in the status line. You can keep editing;
  a change made meanwhile stays unsaved afterwards, and undoing back to the saved state is clean. A
  save that fails is reported and leaves the earlier file untouched. The saved file is
  byte for byte what it was.
- **Plugins load together and an idle project stays quiet.** Opening a project starts the processes
  of up to eight sandboxed plugins at the same time instead of one after the other (they still
  attach in document order). With a plugin loaded and nothing playing, the app looks at its
  plugins ten times a second instead of sixty and sleeps in between. `ui-state.json` is written
  atomically and at most every two seconds instead of at every click, `keymap.json` is checked when a
  window gets the focus or a menu opens instead of four times a second, and the plugin scan uses as
  many scanner processes as half the cores (two to eight).
- **Idle effects cost less.** The EQ's 16-band analyser runs only for panels on screen (its output
  is unchanged, the default-settings EQ costs about 5 times less), the compressor and the gate skip
  their `log10`/`powf` while at rest, delay lines wrap with a mask, and a bypassed effect stops two
  seconds after the bypass is fully engaged (see Effect bypass in DEVICES). The two-second rule can
  change the tail heard when an effect is switched back on.
- **A bigger drum machine panel that shows what is loaded.** The pad grid now takes the width and
  height of the panel instead of a small corner of it, with the 14 pad knobs and the master level
  in two columns at the right and the readout and the EDIT, SAMPLE and TO CH buttons under the
  grid. A pad with a sample is lit in the accent colour and shows the sample's file name (without
  the extension) on up to two lines, with the pad number small in a corner; an empty pad is dark
  and says "empty". A file dragged over a pad lights that pad.
- **The cursor shows what a drag does.** While files are dragged out of the file browser the cursor
  is the copy cursor over a place that takes them and the not-allowed cursor over every other
  place. A device moved in the rack, a clip, a track or a note lane moved in the sequencer show
  the closed hand (the copy cursor while the drop copies), and the not-allowed cursor while the
  pointer is over a pane that takes nothing. A device palette row dragged out shows the copy
  cursor over the rack. The plate of a device that can be dragged shows the open hand.
- **Importing audio holds about one copy of it, and big projects open about four times faster.** An
  import used to copy the audio five times (every check copied the buffer, and a file already at
  the project rate was cloned by the resampler); it now peaks at about 1.03 times the decoded size
  at 44.1 and 48 kHz instead of 3.2 times, and a file's declared length sizes the buffers once.
  WAV files are written and read in 64 KiB blocks (a minute of stereo reads about five times
  faster), and a streamed asset reads ahead 4,096 frames, so playing it costs one read per block
  instead of one per chunk. Opening a project no longer builds a heap path for every note and
  point and judges a rack's thousands of cables as one set: a 200 000-note project opens in 78 ms
  instead of 291 ms, with 1.3 allocations per note instead of 6.3, and the 2 000-note bench
  project in 7.7 ms instead of 39 ms. The project writer streams each note and point into the
  output. File bytes, content hashes, asset ids and load error messages do not change. Comparing
  tags no longer allocates, and the planners of Strip Silence, split, comp rows and clip growth
  compile the tempo map once instead of once per cut.
- **The arrangement draws playback from cached layers.** The lanes with their clips, notes and
  waveforms, and the track list, are recorded into display lists that stay between frames, so a
  frame in which only the playhead moved draws the playhead line, the ruler flag, the marker
  strip and the tempo lane over them instead of walking every clip again, and the GPU keeps the
  encoding of the same lists. A moving track meter re-records the track list alone. Any edit,
  scroll, zoom, selection, hover, tool, drag, theme, scale or size change records both layers
  again, and what is drawn is exactly what it was.
- **Live rendering leaves a core to the audio thread.** The app and `atomik-play` start at most as
  many render workers as the machine has cores minus one, and the audio thread, when it waits for
  a worker, yields its core after a short spin instead of spinning on without end (a worker that
  was preempted, or shares the core, now gets to run). Bounces are unaffected and stay
  bit-identical at every worker count and block size.
- **Idle automation costs nothing per batch.** A lane is sent to its device only when its value
  changed (plateaus, the stretch before the first point and after the last), and after a locate,
  a loop wrap, a transport change or any edit. A bounce with 200 lanes renders 16 percent faster
  and one with 2,000 lanes 9 percent faster. A repeated Mono or Legato mode on the FM synth no
  longer drops the keys still held, and a repeated voice limit no longer stretches a steal fade.
  Sockets that a cable overwrites are no longer cleared first, and a device whose latency cannot
  change (`Device::variable_latency`) is not asked for it every batch.
- **Rack, console and workflow commands no longer slow down with the size of the song.** A copy
  of the document, which every command makes to plan its undo step, shares the song and the notes
  of its clips instead of copying them: adding a device, an audio track or a channel, connecting a
  cable or pasting devices took 7 to 15 ms on a song of a million notes and now takes 40 to 230
  microseconds whatever the song holds (`cargo bench -p atomik-model --bench commands`). Undo
  works on the step itself instead of a copy of it, and names for new devices are found without
  scanning the rack once per candidate. Quantizing a pile of notes on one tick no longer walks the
  pile for each note.
- **Drawing does less when nothing moved.** Text is shaped once and kept in a bounded cache keyed
  by the string, size, weight and letter spacing, so a label drawn every frame costs a lookup
  (warm shaping of 300 labels is over 1,000 times cheaper than cold); a waveform column is read
  once per paint instead of about six times (`paint_channel` takes a third of the time); the
  arrangement and the piano roll repaint for a moving playhead only when its line crosses a pixel
  (or a take ghost or tempo readout under it changes); and the controls that follow automation
  flatten the lanes once per document change and skip the pass while the document, the playhead
  and the play state are unchanged. The pixels drawn are identical.
- **The undo history is bounded.** A project keeps the latest 1,000 undo steps and about 512 MiB of
  what they hold; older steps are forgotten, the latest is always kept, and the audio only
  forgotten steps used leaves the project folder at the next save. Saved files, undo results and
  bounces are unchanged.
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
- Test builds now catch a device that takes a lock or makes a blocking call inside `process`
  (`docs/EXTENDING.md`); release builds contain none of it. `docs/TESTING.md` has a recipe for a
  scheduled hardware run.

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
  and the takes of a take group are left alone. Ctrl+K (Cmd+K on macOS) quantizes the
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
