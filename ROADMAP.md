# Roadmap

This is a list of intentions, not promises. There are no dates. The phases are in rough priority order, but items inside a phase are not ordered. Ideas and help are welcome: open a discussion or an issue before starting something big.

## Done so far

The plugin sandbox (each plugin instance in its own process), release pipeline and binaries, the file browser for audio, per-clip and per-note mute, output buses, multi-column rack, the Mod Source device (two LFOs and an envelope as CV), CV inputs on the effects and instruments, gates on the drum machine, tempo ramps and meter changes along the song (a tempo lane and meter tags in the arrangement), and the keyboard workflow (remappable keymap, nudge by beat or tick, console text owner). See [CHANGELOG.md](CHANGELOG.md) for the details.

## Phase 1: Stability and platforms

- **Plugin sandbox follow-ups.** Run the sandbox on Linux and macOS for real (the wake-up and stdout redirection are compile-checked only), optional grouping of several instances in one process, and embedding sandboxed editors where the platform allows it.
- **macOS and Linux verification.** Linux has been tried on Ubuntu 26.04 only; other distributions, audio setups and real plugins still need checking. macOS compiles and is linted but has not been run on a Mac. Verifying and fixing both is a priority.
- **Missing audio relink.** A way to relink audio files that went missing.
- **Note chase.** Notes that started before the play position or a locate sound from the right place.
- **VST3 on Linux and Wayland.** Run-loop support for editors that need file-descriptor handlers, and plugin editor embedding on Wayland (today the parameter pane is the fallback).
- **Signed builds.** Code signing and notarisation for the release archives.

## Phase 2: The sequencer

- **Markers and locators.** Named markers in the arrangement, jump to marker, loop to a marker range.
- **Automation recording.** Record knob moves and controller input into automation lanes, with overwrite, touch and latch behaviour, and thinning of the recorded points.
- **MIDI controller data.** Pitch bend, aftertouch and controller lanes in the piano roll and in note clips, recorded and played back to devices and plugins.
- **Loop recording and takes.** Record over a loop into stacked takes (note and audio), then pick or comp the best parts.
- **Quantize and groove.** Quantize with strength, swing and a groove template that can be applied to clips and shared between tracks; humanize.
- **MIDI file import and export.** Standard MIDI files in and out, for a clip, a track or the whole song.
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
