# Roadmap

This is a list of intentions, not promises. There are no dates, and the order is not fixed. Ideas and help are welcome: open a discussion or an issue before starting something big.

## Platforms and distribution

- **macOS and Linux verification.** Linux has been tried on Ubuntu 26.04 only; other distributions, audio setups and real plugins still need checking. macOS compiles and is linted but has not been run on a Mac. Verifying and fixing both is a priority.
- **Release pipeline and binaries.** A tag-triggered build that produces Windows, macOS and Linux artifacts with checksums (without the ASIO SDK), and a short release process document.
- **VST3 on Linux.** Run-loop support for editors that need file-descriptor handlers.
- **Wayland plugin windows.** Embedding plugin editors on Wayland (today the parameter pane is the fallback).

## Features

- **Browser and library.** A browsable library of devices, patches, samples and projects with search and tags.
- **LFO and CV source device.** A general modulation source that can drive any CV input.
- **More factory devices.** More original instruments and effects built from the existing DSP blocks.
- **Keyboard workflow improvements.** Nudge by beat or tick, a keymap for the remaining editors, and a text-owner model for the console.
- **Clip and note mute.** A per-clip and per-note mute flag (this needs a project format version bump).
- **Output buses.** More than one stereo main output and routing of channels to named buses.

## Robustness

- **Plugin sandboxing.** Running plugins in a separate process so that a plugin crash cannot take the application down.
- **Missing audio relink.** A way to relink audio files that went missing.

## Quality

- Keep the hard invariants (real-time safety, determinism, graph validity, originality) enforced by tests as the feature set grows.
- Grow the automated checks that run on macOS and Linux.
