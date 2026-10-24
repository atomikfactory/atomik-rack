# Roadmap

This is a list of intentions, not promises. There are no dates. The phases are in rough priority order, but items inside a phase are not ordered. Ideas and help are welcome: open a discussion or an issue before starting something big.

## Done so far

The plugin sandbox (each plugin instance in its own process), release pipeline and binaries, the file browser for audio, per-clip and per-note mute, output buses, multi-column rack, the Mod Source device (two LFOs and an envelope as CV), CV inputs on the effects and instruments, gates on the drum machine, tempo ramps and meter changes along the song (a tempo lane and meter tags in the arrangement), loop recording into stacked takes with comping (note and audio), quantize with strength, swing, groove templates shared between tracks and humanize (baked into the notes), MIDI controller data (pitch bend, aftertouch and CC lanes in note clips, recorded and played back), MIDI file import and export (type 0 and 1 files for a clip, a track or the song, with tempo, meter, markers and controller lanes), named markers and range markers with a marker list, scales and chords in the piano roll (a scale per clip, fold, snap to scale, a chord stamp, chordify, arpeggiate and strum), deep zoom down to the sample with finer snap values and per-track heights, plugin editor fixes (Serum 2 and other VSTGUI editors on Windows, editors opening from the sandbox, VST3 file-descriptor handlers on Linux), the `atomik-mcp` QA server, relinking of missing audio (any file or a folder search, one undo step), note chase (notes already sounding at play, locate or a loop wrap start from the right place, switchable per track), automation recording (Overwrite, Touch and Latch per lane, an arm button and a write-mode button on each automation lane, thinning of the recorded points, a take is one undo step), and the keyboard workflow (remappable keymap, nudge by beat or tick, console text owner). See [CHANGELOG.md](CHANGELOG.md) for the details.

## Phase 1: Stability and platforms

- **Plugin sandbox follow-ups.** Run the sandbox on Linux and macOS for real (the wake-up and stdout redirection are compile-checked only), optional grouping of several instances in one process, and embedding sandboxed editors where the platform allows it.
- **macOS and Linux verification.** Linux has been tried on Ubuntu 26.04 only; other distributions, audio setups and real plugins still need checking. macOS compiles and is linted but has not been run on a Mac. Verifying and fixing both is a priority.
- **VST3 on Wayland.** Plugin editor embedding on native Wayland (sandboxed editors already run through XWayland; in-process editors and sessions without `DISPLAY` fall back to the parameter pane). Editors that need file-descriptor handlers now run on Linux; they still need checking with more real plugins.
- **Signed builds.** Code signing and notarisation for the release archives.

## Phase 2: The sequencer

- **Automation recording follow-ups.** Record MIDI controller input (a CC to parameter path with MIDI learn), a preference for the thinning amount, and a touch that starts at the press instead of the first move (needs a press and release signal from every control, plugin windows included).
- **MIDI controller data follow-ups.** Controller lanes without a note clip, selecting several points with copy, paste and thinning, a bend range message (RPN) and pitch bend and mod wheel in the other instruments, a command and key for the controller area, and MPE (per-note expression) lanes.
- **Loop recording and takes follow-ups.** An overlapped constant-power crossfade at audio comp splices (it needs the engine to mix two regions; today a splice is a 5 ms fade out and in), adding a later recording pass to an existing group, opening a take in the piano roll or the sample editor, a waveform in the audio take rows, a command and key for the take rows, comping several tracks at once, and takes for automation passes (a loop recording of automation still lays layered clips).
- **Quantize and groove follow-ups.** A live, non-destructive groove per track or clip (a groove name saved on the track and applied when the song is flattened, so changing a template re-times every track that uses it without touching the notes; today a clip's step-grid swing is the only live swing and a groove is baked in), quantize of audio clips from their slice markers (see Audio editing), controller lane points that move with their notes, a text field to name and rename a groove, a groove from a MIDI file or an audio loop, per-note-length and legato options, humanize of lengths, a quantize button in the arrangement's toolbar (the piano roll has one), tooltips on the roll's toolbar buttons, and bar lines that follow meter changes for the swing parity (it counts steps from the start of the song).
- **MIDI file follow-ups.** A choice of export resolution (960 or 480 ticks per quarter note, for hardware that dislikes 15 360), the file browser listing `.mid` files for dragging from the panel, a drop onto a lane that imports into that track at the drop position, range markers exported, program changes and bank selects, per-note expression (MPE) and SysEx kept, and a groove taken from a MIDI file (see Quantize and groove follow-ups).
- **Track organisation.** Track groups and folders, colours, comments, lanes that can be folded, and a track-height preset.

## Phase 3: Audio

- **Time-stretch and pitch-shift.** Warp audio clips to the song tempo, with transient-aware modes for drums and a polyphonic mode for tonal material; transpose without changing length.
- **Audio editing.** Fades and crossfades on clips, clip gain and gain envelopes, reverse, normalise, strip silence, and audio quantize from the slice markers.
- **Freeze and bounce in place.** Render a track or a device chain to audio and swap it back in, with the original kept so it can be unfrozen.
- **Stem and selection export.** Export every track, bus or a selected range as separate files, in WAV, FLAC and MP3, with dither and loudness reporting.
- **Audio to notes.** Extract notes from monophonic and drum audio to a note clip.
- **Spectrum and metering.** A spectrum analyser and a phase and stereo meter as devices or console panels.

## Phase 4: Mixing and control

- **Mixer groups and VCA-style faders.** One fader that controls several channels, and linked mute and solo.
- **Channel strip upgrades.** Per-channel parallel compression, input trim and polarity, and an insert bypass that keeps the plugin delay compensation correct.
- **Snapshots and comparison.** Store several mixer states and switch between them.
- **MIDI learn and control surfaces.** Map any parameter to a MIDI controller by moving it, save the maps with the project and as presets, and support the common control-surface protocols for faders and transport.
- **Macro and parameter workflow.** Parameter lock to a macro from the rack, automation-follows-selection, and a search over every parameter of the project.
- **Feedback routing.** A supported way to route audio back on itself (with a mandatory delay stage) instead of the current rejection of loops.

## Phase 5: Devices

New original devices built from the existing DSP blocks, grouped by the gap they fill.

- **Utility and CV.** An LFO is done; still missing are a CV mixer and merger, a splitter, an attenuator and inverter, a CV math device, a quantizer with scales, a sample-and-hold, a slew limiter, a gate and trigger processor, and an audio-rate envelope follower.
- **Pattern and note tools.** A CV pattern sequencer, a scale and chord device, a note echo and a note delay, a note transposer and filter, and a drum pattern generator.
- **Instruments.** A wavetable synth, an FM synth, a granular sampler, a physical-modelling instrument, an organ, a monosynth with a mod matrix, and a semi-modular synth built from the CV devices.
- **Effects.** Pitch shifter, vocoder, tape and saturation, rotary speaker, auto-filter and envelope filter, gate and expander, transient shaper, de-esser, a standalone limiter, a multiband dynamics device, a convolution reverb with an impulse browser, a tuner, and a stereo imager.
- **Instrument patches.** A real patch format for every device: save, browse, tag and audition patches, with factory banks that ship with the application.

## Phase 6: Library and projects

- **Library of devices, patches and projects.** The browser handles audio files today; devices, patches, containers, clips and projects remain.
- **Project templates and a start screen.** Templates for common set-ups and a recent-projects list.
- **Collect and relocate.** Copy every file a project uses into its bundle, and relocate a project without breaking its links.
- **Song information.** Notes, credits and a version history inside the project.

## Phase 7: Ecosystem and device parity (last)

This phase comes last on purpose: it builds on everything above and on a stable public API.

- **A stable device SDK.** A versioned, documented interface and a package format so that third parties can write and distribute devices that run natively in the rack, with their own front and back panel, parameters, CV and note sockets, and presets. This extends [docs/EXTENDING.md](docs/EXTENDING.md).
- **Device packages and a store-less installer.** Install, update and remove device packages from a folder or an archive, with signature and version checks.
- **Factory device parity.** Fill the remaining gaps in the factory set so that each usual category of device (instruments, effects, players, utilities) has at least one original design, and add the deeper variants that earlier phases only started.
- **Matching other devices.** Equivalents of well-known devices and device formats from other platforms, written as original work: a parameter-compatible mode where it helps users move their patches, and importers for patch files that are openly documented. Anything that needs proprietary data stays out (see HI-4 in the README).

## Quality

- Keep the hard invariants (real-time safety, determinism, graph validity, originality) enforced by tests as the feature set grows.
- Grow the automated checks that run on macOS and Linux.
