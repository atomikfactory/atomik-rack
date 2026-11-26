# Decision log

Design questions that shape several pieces of work, with the options, what each one implies, and where the project stands. Roadmap items and pull requests cite an entry here instead of re-arguing it.

**Status values.**

- **Decided**: the maintainer chose; build to it.
- **Open**: the maintainer deferred it; nothing changes until it is decided.
- **Default (not yet confirmed)**: no review yet. The recommended default is the working assumption, so work can start, but it is not a commitment and may change. Say so in the pull request when you rely on one.

All entries below were recorded on 2026-10-30 and decided (or deferred, or defaulted) by the maintainer. To change an entry, add a dated note under it; do not rewrite history.

| Entry | Topic | Status |
|---|---|---|
| [A](#a-large-batch-plugin-mode) | Large-batch plugin mode | Default (not yet confirmed) |
| [B](#b-asio-sdk-licensing-for-official-builds) | ASIO SDK licensing for official builds | Open (deferred) |
| [C](#c-delay-compensation-for-fx-returns-and-sidechain-keys) | Delay compensation for effect returns and sidechain keys | Decided |
| [D](#d-device-patch-format) | Device patch format | Decided |
| [E](#e-modulation-framework) | Modulation framework | Default (not yet confirmed) |
| [F](#f-block-based-arrangement) | Block-based arrangement | Default (not yet confirmed) |
| [G](#g-middle-c-naming) | Middle C naming | Decided |
| [H](#h-stable-note-and-point-identity) | Stable note and point identity | Default (not yet confirmed) |
| [I](#i-cross-platform-bit-identity-of-the-older-dsp-primitives) | Cross-platform bit identity of the older DSP | Default (not yet confirmed) |
| [J](#j-bypass-tail-policy) | Bypass tail policy | Default (not yet confirmed) |
| [K](#k-undo-history-budget) | Undo history budget | Default (not yet confirmed) |
| [L](#l-format-version-cadence) | Project format version cadence | Decided (already applied) |
| [M](#m-warp-on-import-default) | Warp on import default | Decided |
| [N](#n-video-through-the-users-ffmpeg) | Video through the user's FFmpeg | Default (not yet confirmed) |
| [O](#o-the-video-of-a-song-and-its-time) | The video of a song and its time | Default (not yet confirmed) |
| [R](#r-controller-profiles) | Controller profiles | Default (not yet confirmed) |
| [Y](#y-the-pitch-keeping-preview-of-a-warped-clip-is-a-real-time-wsola) | Pitch-keeping preview of warped clips | Default (not yet confirmed) |

---

## A. Large-batch plugin mode

**Question.** Hosted plugins are called in fixed batches of 64 frames whatever the audio card's buffer size. Should a plugin be allowed to run once per several batches at the card's buffer size?

**Options.**

1. Keep the fixed 64-frame batch.
2. Opt in per plugin: call it once per N batches at the card's buffer size.

**Implications.** Option 1 is what keeps a live render and a bounce bit-identical (the determinism invariant in [ARCHITECTURE.md](ARCHITECTURE.md)); the cost is plugin CPU at large buffers. Option 2 saves CPU for heavy plugins, but the live output would then differ from the bounce for those plugins (already true for plugins that are not deterministic themselves, see [DEVICES.md](DEVICES.md)), and the sandbox protocol would need to change.

**Decision.** Working assumption: option 1. Revisit only if a measured sandbox CPU problem makes a case for it.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

## B. ASIO SDK licensing for official builds

**Question.** Should official release binaries support ASIO, and under what licence terms?

**Options.**

1. Keep ASIO an opt-in build feature that is never in releases (today; see [RELEASING.md](RELEASING.md#asio)).
2. Ship a separate ASIO build under the SDK's GPLv3 option.
3. Ask users to bring ASIO4ALL or FlexASIO and use WASAPI otherwise.

**Implications.**

- Since October 2025 the ASIO SDK is also offered under GPLv3, which is compatible with this project's GPL-3.0-or-later. An ASIO binary would then be GPLv3-only, and the SDK sources would have to be provided as corresponding source. The repository policy that the SDK is never bundled would have to change for that artifact. The current Steinberg terms must be read and confirmed before anything ships.
- ASIO4ALL and FlexASIO are drivers, not SDKs: a host still needs the SDK to speak ASIO to them.
- WASAPI exclusive mode is the licence-free low-latency alternative on Windows; shared mode keeps the larger latency.
- Option 1 leaves the Windows latency story as it is; option 3 is free but a poor first experience.

**Decision.** Open. The maintainer deferred it. Nothing changes for now: ASIO stays an opt-in feature and official builds are made without it. If the SDK terms are confirmed as GPL-compatible, the natural next step is a separate, signed ASIO artifact; otherwise document FlexASIO and WASAPI exclusive mode. Needs a maintainer legal judgement and a read of the current SDK terms, which should be attached here as evidence when it is reopened.

**Status.** Open (deferred by the maintainer, 2026-10-30).

## C. Delay compensation for FX returns and sidechain keys

**Question.** When a send feeds a return, or a key feeds a sidechain input, through a device that adds latency, which paths are aligned?

**Options.**

1. Keep returns and sidechain keys as plain inputs, not aligned (today).
2. Align them by summing inputs, the same way the master and the plan already align.
3. Stage-level delay lines.

**Implications.** Option 2 changes the output bits of projects that use a latent device on a send or a key, so it must not change old projects silently. Option 3 is more machinery for little gain. A return is modelled as the input of a single device, which any change here must keep true.

**Decision.** Option 2, behind a project flag. The flag is off for projects saved before the change, so they keep sounding exactly as before; new projects have it on. The behaviour needs a hand check with a latent device on a send and with a latent sidechain key, rendered live and bounced, before it ships; attach the result here.

**Status.** Decided. Decided by: maintainer, 2026-10-30.

**Implementation note, 2026-10-30.** Built as a console project flag, `align_returns` (the Mixer menu's *Align Returns*, one undo step). With it on, every audio cable of a stage, plain inputs included, is delayed to the slowest one: send returns, sidechain keys and split paths line up, and the dry strips wait for a latent send; the metronome waits for the output latency. Format 37 holds the field; a file without it (and every file before 37) loads with it off, so old bounces are unchanged bit for bit, and a new project writes it on. Still to do before this ships, as the decision requires: the hand check with a latent device on a send and with a latent sidechain key, live and bounced (result to be attached here).

## D. Device patch format

**Question.** How is a single device's sound saved and applied?

**Options.**

1. Keep project-embedded sound tables and library patch bundles, with no way to apply a patch to one device.
2. A documented single-device patch (parameters, zones, steps, curves, drums, plugin state, sample references) applied in place.

**Implications.** Today a patch that sets only `setup` silently does nothing (see [DEVICES.md](DEVICES.md)), and the MCP server and the renderer cannot use the app-side factory tables. Option 2 is additive: existing patch files and library bundles keep loading unchanged, with an optional patch name field.

**Decision.** Option 2. A single-device patch is applied in place as one undo step; existing library patch bundles keep working; a patch may carry an optional name.

**Status.** Decided. Decided by: maintainer, 2026-10-30.

## E. Modulation framework

**Question.** What shape does general modulation routing take?

**Options.**

1. None: the Mod Source device and CV cables stay the only routing.
2. A device-level modulation bus helper: each device declares modulation slots (source, amount, destination), computed inside the device, with no engine change.
3. An engine-level modulation graph.

**Implications.** Option 2 keeps real-time safety and determinism local to each device and adds no engine concept. Option 3 is a rewrite of the engine's routing.

**Decision.** Working assumption: option 2, with the operators `+ - * /` only. Not yet reviewed.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

*Note, 2026-11-01.* Built on the working default, so it stands or changes with the review: device-level slots (eight per device), the operators `+ - * /` only, no engine change. The shape of the framework (slot count, source and target tables by id, `via` as a 0..1 scale on the amount) is in [EXTENDING.md](EXTENDING.md#modulation-matrix); the poly synth and the mono bass are the first devices to use it, and project format 41 stores the slot structure. The matrix panel is a modal view opened from the device's context menu (Modulation Matrix), built from the tables.

## F. Block-based arrangement

**Question.** Should the arrangement support blocks, reusable sections that own a set of clips per lane?

**Options.**

1. Not implemented.
2. Block-owned clips per lane, with a song-level block list and a format change.

**Implications.** Option 2 is a very large change to the song model, the arrangement UI and the flatten step, and is not on the roadmap today.

**Decision.** Working assumption: defer. Decide after the next phase of work ships.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

## G. Middle C naming

**Question.** Is MIDI note 60 shown as C4 or C3?

**Options.**

1. Keep C4 = 60.
2. Switch globally to C3 = 60.
3. A preference.

**Implications.** A global switch changes every displayed note name and any text the MCP server returns. A preference is cheap and does not touch the project format.

**Decision.** A preference, "Middle C = C3 / C4", defaulting to C4 (today's naming). Format-free: it lives in the settings, not in the project.

**Status.** Decided. Decided by: maintainer, 2026-10-30.

## H. Stable note and point identity

**Question.** How does the editor keep a selection pointing at the same note or automation point after the data changes?

**Options.**

1. Accept index-based selection (it retargets after undo, quantize or an MCP edit).
2. Select by (tick, key) and re-resolve after each change.
3. Model-level ids (a format change that touches flatten, clip content edits and MIDI import).

**Implications.** Option 2 is cheap and removes most wrong-note edits. Option 3 is the clean fix and a prerequisite for per-note automation and editing several lanes at once.

**Decision.** Working assumption: option 2 now, option 3 when the undo and model work that needs it starts.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

## I. Cross-platform bit identity of the older DSP primitives

**Question.** Must bounces be bit-identical across Windows, macOS and Linux for the older primitives (sine oscillators and LFOs, filter cutoff mapping, dynamics, pan, biquad design), which call the system math library?

**Options.**

1. Bit-identical on every platform: replace those calls with the project's own deterministic functions and regenerate the golden files once.
2. Per-platform goldens only.

**Implications.** Option 1 makes a bounce portable and strengthens the determinism invariant, at the price of one deliberate golden regeneration that must be reviewed on Windows x86-64. Option 2 leaves the invariant weaker on those primitives.

**Decision.** Working assumption: option 1, in one regeneration batch.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

## J. Bypass tail policy

**Question.** What happens to the tail of an effect that is bypassed?

**Options.**

1. Keep the tail forever (today: a bypassed device costs full price).
2. Stop the inner effect after a decay window, or once it reports that it is quiet.

**Implications.** Option 2 changes the output only if the tail was still audible, and offline renders must use the same rule as live, so the determinism invariant holds.

**Decision.** Working assumption: option 2; skip the effect after its reported tail, with a flush threshold of 1e-30 in energy.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

## K. Undo history budget

**Question.** Is the undo history bounded?

**Options.**

1. Unbounded (today).
2. Capped by number of steps.
3. Capped by bytes, releasing assets held by evicted steps.

**Implications.** An unbounded history grows with every destructive audio edit. A byte budget bounds memory regardless of the kind of edit, and needs a visible note that older steps were dropped.

**Decision.** Working assumption: both caps, whichever is hit first. 1,000 steps and an estimated
512 MiB (`MAX_UNDO_STEPS`, `MAX_UNDO_BYTES`); the oldest steps go first, the latest step always
stays, and the assets only dropped steps held leave the pool (and their audio memory) when the history lets
go of the step, unless the clipboard still holds a copy, and at the latest at the next prune. There is no
"history trimmed" note yet.

**Status.** Default (not yet confirmed). Recorded 2026-10-30; implemented with these numbers on 2026-10-31; awaiting the maintainer's review.

## L. Format version cadence

**Question.** How often does the project format version change?

**Options.**

1. One bump per feature.
2. One batched bump per release, with every new optional field marked ignorable by older readers.

**Implications.** Batching keeps the number of versions small and the upgrade story simple; ignorable optional fields keep older readers working where they can.

**Decision.** Option 2. One batched bump per release; new optional fields are ignorable. Already applied: format 35 (note tails) followed this.

**Status.** Decided (already applied). Decided by: maintainer, 2026-10-30.

*Note, 2026-10-30.* The reader side is built. Every field is one row of the table in `crates/model/src/project/schema.rs`, and a row marked `ignorable` is the mechanism for "new optional fields are ignorable": a file newer than the reader opens when its root carries `"compat"` (the oldest version that can read it) and an `"ignorable"` list of the fields to skip, and the reader drops those with a warning each. The writer adds the two root fields when the newest changes are all ignorable; no format change has used it yet, so no file carries them. The first batched bump to use it should mark its new optional fields `ignorable` in the table. See [ARCHITECTURE.md](ARCHITECTURE.md#22-project-open-edit-and-live-sync).

*Note, 2026-10-30.* Format 37 is the first batched bump. Its one new field, a MIDI Out device's `"midi_out"` port, is on a device object, and a device may sit inside a container, so no path can name it and the table cannot mark it `ignorable` (the table test refuses that). It is therefore a field an older reader cannot skip: a version 37 file with one is not opened by a build that reads 36, which says so, and a file without one differs from its version 36 file in the version line alone.

## M. Warp on import default

**Question.** Are audio files warped to the song tempo when imported?

**Options.**

1. Off (today).
2. Loops only.
3. All files.

**Implications.** Stretching a one-shot or a long recording on import is surprising and costs time; leaving loops unwarped makes the first thing a user does a manual step.

**Decision.** Loops only: a file detected as a loop (tempo metadata, or a clean bar length) is warped on import; one-shots and long recordings are not.

**Status.** Decided. Decided by: maintainer, 2026-10-30.

Applied: Settings, Workflow, **Warp on import** (Off, Loops only by default, All files); see [ARCHITECTURE.md](ARCHITECTURE.md) section 2.18.

## N. Video through the user's FFmpeg

**Question.** How does Atomik Rack decode video for video tracks?

**Options.**

1. Link a decoder library (a codec crate or FFmpeg itself) into the program.
2. Ship an FFmpeg build beside the program.
3. Use the FFmpeg the user installed, through its command-line programs, and read only a small proxy of the film.

**Implications.** Linking or shipping a decoder makes the program (and its licence terms, patents and size) answer for
every codec, and an FFmpeg build with the common codecs is GPL or LGPL with its own obligations. The user's own
FFmpeg already decodes what the user has; the cost is that video needs an install, so the program must find it,
say clearly when it is missing and never fail without it. Reading frames of a long film in the player with an
external process per frame is too slow, so the picture is turned once into a low-resolution file of independent
frames that is one seek away at any frame.

**Decision.** Option 3. No codec and no FFmpeg is bundled; `atomik-video` finds `ffmpeg` and `ffprobe` (Settings
folder, then `PATH`, then the usual package folders on macOS), needs release 4 or later, and runs them only through
one trait with a fake for tests. The proxy is JPEG (every frame intra-coded; about 360 lines), packed in one indexed
file per film in a per-user cache that is pruned to 2 GiB. JPEG decoding is a small pure-Rust dependency that comes
with the viewer; the first item adds none (the viewer added it, see P).

**Status.** Default (not yet confirmed). Decided by: none yet.

## O. The video of a song and its time

**Question.** How does a song hold a film, and in which unit are its start and trim kept, given that the song is in
ticks and the tempo can change?

**Options.**

1. Ticks, like clips: the film's start and length are song ticks, converted with the tempo map.
2. Seconds as floating point.
3. Integer microseconds.

**Implications.** A film runs on the clock. In ticks, a tempo change would move the film in time and shrink or
stretch it against the picture, which is never what a user wants for sync (the tempo of a song is edited long after
the film is placed). Floating-point seconds do not compare or round-trip exactly, so an undo or a save and load
could move a start by a rounding error and the golden files would depend on the printer. Integers are exact; a
microsecond is far finer than a frame (a frame of 30000/1001 fps is 33 366.7 microseconds and has no exact integer
form in any unit, so no unit would be exact for frames: the viewer rounds to a frame, the model never counts them).
The tick of a position is still needed for the ruler; it is computed from the tempo map and moves with the tempo,
the stored start does not. The film is linked by path and never copied (a film is gigabytes); the cost is that a
moved film is missing until it is relinked, which the app marks and offers, comparing the length and frame rate with
the facts the project kept.

**Decision.** Option 3. `RackDoc::video` is one optional clip with a path, the probe facts, a start and a trim in
microseconds. It is saved as an optional ignorable root field of format 44 (no bump: a reader that does not know it
drops it with a warning and plays the song, which is all it can do), and edited by `Command::Video`.

**Status.** Default (not yet confirmed). Decided by: none yet.

## P. The JPEG decoder of the viewer

**Question.** Which decoder turns a frame of the video proxy into pixels for the viewer window?

**Options.**

1. `zune-jpeg`: a pure-Rust baseline and progressive decoder, `MIT OR Apache-2.0 OR Zlib`, one small dependency.
2. `jpeg-decoder`: pure Rust and older, but no longer developed.
3. `image`: a framework with a decoder per format, many dependencies.
4. FFmpeg for every frame (a process per frame, or one long-running process asked for frames).

**Implications.** The viewer decodes a frame per video frame while the song plays, so it must be fast and must not
need the user's FFmpeg at that moment (the proxy exists so that it does not). A process per frame is far too slow and
a long-running one adds a protocol and a second thing to kill. `image` brings formats nobody uses here. `zune-jpeg`
is maintained, reads what FFmpeg's motion JPEG encoder writes, and decodes a 360-line frame in a few milliseconds.
Its licence (any of MIT, Apache-2.0 or Zlib) is on the allow list of `deny.toml`. Its SIMD paths are `unsafe`
inside the dependency; the build turns the default features off and keeps the portable code, which is fast enough
at this size and keeps the amount of third-party `unsafe` in the program as small as it can be.

**Decision.** Option 1, `default-features = false` with `std`. Decoding runs on a worker thread of the app
(`FrameDecoder`), never on the UI thread or the audio thread, with a byte-limited cache of decoded frames.

**Status.** Default (not yet confirmed). Decided by: none yet.

## Q. The film's sound and "keep with video"

**Question.** How does the film's own sound get into the song, how is it lined up with the picture, and where does
the link that makes it follow the film live?

**Options.**

1. Extract the sound to a WAV file with FFmpeg and use the song's import (any sample rate, the bundle, the peaks),
   placing a plain audio clip.
2. Ask FFmpeg for the song's rate and channel count directly, and place the clip from the file.
3. Keep the sound inside the film clip (no audio track), played by the engine from the film.

For the link: a field of the film (the audio clip's id, how late the sound starts, a flag), or a field of the audio
clip, or a second command the user runs after each move.

**Implications.** The song already imports audio files, hashes and stores them, resamples to the song's rate and
draws peaks; option 1 reuses all of that and the sound is an ordinary clip the user can edit, mute or bounce.
Option 2 would add a second resampler path (FFmpeg's) beside the song's. Option 3 would put the film's decoder in
the audio path, which video must never touch. A clip starts on a tick, a fraction of a beat that is a few dozen
samples at usual tempos, so a tick alone cannot place the sound under the first frame; the skip into the asset
(`source_offset`) absorbs the rest, and the arithmetic has to be done in frames of the song's rate from the film's
microseconds. The link belongs to the film because the film is what moves and trims (the edit has to be one undo
step of both, planned from the film's new state), and because an audio clip has no idea of a film; as an optional
part of the already ignorable `video` field it needs no format bump.

**Decision.** Option 1, extracted at the track's own rate and resampled by the import, surround mixed down to
stereo by FFmpeg. The link is `VideoClip::audio_link` (clip id, lead in microseconds, a keep flag); `Move`, `Trim`
and `Relink` plan the film and the clip together as one compound edit and a drag merges both. The clip is recomputed
from the film's state each time. Importing the film's audio is one undo step with the new track.

**Status.** Default (not yet confirmed). Decided by: none yet.

## R. Controller profiles

**Question.** How does a hardware controller that is set up once keep working: as mappings saved in each project, or as a driver of its own?

**Options.** (1) A set of mappings written into the project's MIDI maps. (2) A profile file with the controller's own surface driver.

**Implications.** Mappings address a concrete device and live in the project, so a controller set up in one song would have to be mapped again in the next, and a layout that follows the selected device (knobs on whatever device is selected, faders on the strips of a bank) is not something a mapping can say. A driver reads a layout by position, follows the selection like the existing surfaces, owns its bank and its feedback, and the profile is plain data that can be shared. It adds one more kind of surface driver but no new plumbing: the actions are the ones the console surfaces already use, plus one for a remote knob.

**Decision.** Option 2. A profile is a JSON file in the settings folder, found by its MIDI port name, and a surface of the settings runs it (`profile:<name>`). The project format does not change.

**Status.** Default (not yet confirmed). Decided by: none yet.

## S. The update check

**Question.** How does the program learn that a newer version exists without sending anything about the user or the
machine, without a heavy network stack, and without ever installing anything by itself?

**Options.**

1. No check: the user looks at the project page.
2. A menu item that only opens the releases page in the browser: the program makes no request of its own.
3. An optional check with a small HTTPS client inside the program: one request for the list of releases, a notice
   when a newer version exists, nothing downloaded or installed.

For the client: `reqwest` (an asynchronous framework with its own runtime), `ureq` (blocking, small, rustls), or
`rustls` used by hand with a hand-written HTTP exchange. For the cryptography under rustls: `ring` (small, widely
used, a few C and assembly files) or `aws-lc-rs` (a C library and a heavier build). For the root certificates: the
bundled `webpki-roots` (the same on every platform, a data licence of its own) or the system store through a
platform verifier (one code path per platform, more crates).

**Implications.** Option 1 and 2 keep the promise that the program never touches the network, at the price that
few users ever learn about a release. Option 3 is the only network code in the program, so it has to be easy to audit:
one fixed address, a User-Agent that is only the program name and version, no cookie, no query, no redirect, a
timeout and a size limit, behind a trait with a fake so that no test can reach the network. Consent has to come
first: until the user has answered the question, nothing is sent, and Help > Check for Updates shows the question
instead of making a request. A runtime or an asynchronous framework would be a large dependency for one request a day
on a worker thread. `ureq` with rustls does exactly that with a handful of crates, all pure Rust except `ring`'s
small native part. The bundled
roots make the behaviour the same on the three platforms and need no system call; their licence (CDLA-Permissive-2.0,
a permissive data licence) is added to the allowed ones in `deny.toml`. The cost is that a certificate authority
removed from the bundle is trusted until the next release: acceptable for a request that carries nothing private.

**Decision.** Option 3: `ureq` 3 with rustls on `ring` and the bundled roots, HTTPS only, in `crates/update` behind
the `HttpGet` trait. The address is the list of releases of the project's own repository; a pre-release is offered
only to a pre-release build; a draft never; the release page opened by "What's new" is made from the checked tag,
not taken from the answer. At most one request a day after a Yes; a check by hand never waits. A failure is silent
(logged at debug level) unless the user asked. The answer is the workflow preference `update_check` and the time of
the last request and the skipped version are in `updates.json`.

**Status.** Default (not yet confirmed). Decided by: none yet.

## T. The clip launcher: the override model and the transport rule

**Question.** When a track plays a launcher slot, what happens to the clips the arrangement has for it, and what does
a launch do while the transport is stopped?

**Options.** For the arrangement: (1) a slot plays on top of it; (2) a slot replaces it for that track until the user
gives the track back; (3) a slot replaces it only while it plays. For a launch with the transport stopped: (a) the
launch is refused; (b) it waits for the user to press play; (c) it starts the transport.

**Implications.** Option 1 makes a launched loop and the track's own clips sound together, which is never what the
user wants from one instrument. Option 3 makes the arrangement jump in when a slot stops, so stopping a loop in a live
set changes the music in a way nobody asked for; option 2 keeps the track silent until a clear gesture, but a track
then needs a way back (Back to Arrangement, and stopping the slot). Launching is a performance gesture: a refusal
(a) is a dead button, (b) hides the effect of the click until a second action, and (c) is what a producer expects from
a play triangle. The launch still waits for its quantize boundary, so a position on a bar line launches at once and
any other waits for the next one.

**Decision.** Option 2 with a way back by two routes: Back to Arrangement gives every track back at the next tick, and
stopping a slot (a track's stop square, Stop All, or a scene launch with Scene Stops Empty Tracks on) gives that track
back at the next boundary of the launch quantize. A track has exactly two states, the arrangement and a slot, and the
engine holds them in the song snapshot, so a song edit keeps a playing slot playing. A launch with the transport stopped
starts the transport (c) from where it stands; the engine itself keeps a request until play, so the app's two messages
need no order. The launcher is saved as an optional, ignorable field of the song, so the format stays as it is.

**Status.** Default (not yet confirmed). Decided by: none yet.

*Note, 2026-11-09.* Format 45 is a batched bump made so that a reader of 44 can open what this build saves. The reader
of 44 only honours `"compat"` and `"ignorable"` in a file whose version is above its own, and refuses an unknown field
in a file of version 44, so the fields added after the 44 reader shipped (the clip launcher and the per-note expression)
could not be skipped under 44. Their rows are now `since` 45 and `ignorable`, the writer says `"compat": 44`, and a
reader of 44 opens a 45 file and drops those parts with a warning each. Every file saved now carries the two header
fields; a file before version 37 that carries them is read as the older file it says.

## U. Linux packages: AppImage and Flatpak from the release programs

**Question.** How does the program reach Linux users on any distribution without a second build of the program for
each package, and what may the sandboxed package see?

**Options.** For the programs in the Flatpak: (1) compile inside `flatpak-builder` with the Rust SDK extension, from
the source archive; (2) take the five programs of the release tarball as the source, with its checksum. For the
AppImage: (a) bundle every library the programs load; (b) bundle only what a base system may lack and take the
sound, font, display and graphics libraries from the host. For the Flatpak's reach: (x) a minimal sandbox that sees
only the document and music folders; (y) read-only access to the home folder, with the plugin folders named.

**Implications.** Option 1 makes the Flatpak a second, slower build (the whole workspace, with a vendored copy of the
crates because the build has no network) whose programs differ from the ones tested and signed everywhere else. Option 2
puts the same binaries in the `.tar.gz`, the `.deb`, the AppImage and the Flatpak, and the build is one `install` per
file; the cost is that the programs are linked on the CI runner, so the Flatpak needs the C library of its runtime to be
at least as new as the runner's (the smoke check runs `ldd` inside the sandbox, so a mismatch fails the build, not a
user). For that reason the Rust SDK extension is not part of the manifest: nothing is compiled. Bundling the sound
libraries (a) breaks on hosts whose ALSA plug-ins sit elsewhere, and the font and graphics libraries must match the
host's configuration and drivers, so (b) is the usual AppImage rule; with the programs' present needs it bundles nothing
today and the image still carries the factory library and the desktop files, so it needs ALSA, fontconfig and a graphics
driver exactly like the archive. A song, a sample folder and a plugin live anywhere under the home folder: (x) would
make the file dialog useless for most of the work, so (y), read-only, with `~/.vst3` and `~/.clap` listed so the
list of what the program reads stays visible. Flatpak reserves `/usr`, so `/usr/lib/vst3` and `/usr/lib/clap` cannot be
granted; plugins installed system-wide are not seen by the Flatpak (the AppImage and the packages see them). The
update check keeps `--share=network`, the only network use, because it is the program's own optional request; inside a
Flatpak its notice says to update with the software centre instead of pointing at a download page.

**Decision.** Option 2, (b) and (y). One desktop entry, one AppStream file (its releases list made from the version)
and the icons are installed under the app id `io.github.atomikfactory.AtomikRack` by all three Linux packages
(`packaging/linux`, `scripts/release/linux-share.sh`); the program sets the same id as its window class so the desktop
shows the right name and icon. The AppImage is built with the official `appimagetool` at a pinned version and checksum,
the Flatpak by `flatpak-builder` against `org.freedesktop.Platform` 25.08; both are built only by the release and nightly
workflows, and pull requests check the scripts, the manifest and the metadata without building.

**Status.** Default (not yet confirmed). Decided by: none yet.

---

## V. The Markdown reader of the built-in manual

**Question.** The built-in manual shows the user documents of `docs/` inside the app. Which Markdown reader draws
them, and which files does the manual hold?

**Options.** For the reader: (1) a pure-Rust Markdown crate with a permissive licence (pulldown-cmark is MIT and pure
Rust); (2) a small reader of the subset the documents use, written in the project. For the files: (a) the user
documents only (`docs/README.md` and the generated `docs/KEYBOARD.md`); (b) every document of `docs/`.

**Implications.** A crate reads all of CommonMark, and the manual would still need its own layout, because the app
draws everything itself: the events of the crate would be turned into the same blocks and styled runs either way, so
the crate saves only the parsing, a few hundred lines that a fixture test pins down. It adds a dependency to the UI
crate, a licence and advisory line to keep, and behaviour (HTML blocks, reference links, nesting rules) that the
documents do not use and the layout does not draw. The in-house reader covers headings with GitHub-style anchors,
paragraphs, lists, fenced code, tables, quotes, rules, bold, italic, code, links and images (shown as their alt text),
and reads anything else as plain text; it never fails. Its cost is that a document that starts to use a Markdown
feature it does not read shows it as text until the reader learns it; the coverage tests parse the real documents, so a
heading or a link that does not resolve fails the build. For the files, the manual of the architecture, the tests and
the decision log is for contributors and would triple the compiled-in text for little use inside the app, and the
device reference alone is longer than all the user documents together; the sections that explain the app are in
`README.md`, which says where the reference is and links to it on the project page.

**Decision.** Option 2 and (a). The reader is `atomik_ui::manual::markdown`, the manual and its search are in
`atomik_ui::manual`, and the app compiles the two files in with `include_str!` (`app/src/manual.rs`), so the manual
and the documentation are one text. A link to a file the manual does not hold opens the page of the repository it names,
one request to a fixed address like Help > Documentation, and nothing else is ever opened from the text.

**Status.** Default (not yet confirmed). Decided by: none yet.

## W. How the interface is translated

**Question.** The interface should be usable in French, and adding another language should be one file. Which way
do the texts reach the language: a call at every place a text is written (`t("Save")`), a key per text with a
catalogue of identifiers (`menu.file.save`), or one lookup at the place where text is drawn?

**Options.** (1) A call at each place, with the English text as the key: readable, but the program draws text in a few
thousand places (labels returned by functions, tables of choices, helpers that cut a label to its room), so every one
would need editing, and a text added later without the call would silently stay English. (2) Identifier keys with a
catalogue per language, as `fluent` or `gettext` do: the right answer for a program that starts with it, but it replaces
every text of the code by an identifier, which makes the code harder to read and the change enormous. (3) One lookup
where a [`TextRun`](../crates/ui/src/canvas.rs) is made, with the English text as the key, plus a lookup of the same
kind where text is not drawn by the program (the native menus, the tooltips, the width of a tab).

**Implications.** With (3) every label that is written as a text is translated where it is drawn, in every panel,
and a text with no entry shows in English, so the program is always complete and the catalogue can grow one entry at a
time. The cost is that the lookup cannot tell a word of the program from a word of the user: a track called `Bass`
would show as `Basse`. The places that draw the user's own words (track, clip, strip and device names, file and patch
names, plugin parameters) take the `verbatim` road (`TextRun::verbatim`, `fit_label_raw`), and the menus translate only
their fixed texts, never the song, template and track names they list; a place not yet on that road shows a name
translated only when it is exactly a word of the catalogue. Text built with `format!` is found by its shape: an entry
may name values in braces (`Added {name}`) and then matches any text of that shape and puts the values into the French.
A count has a singular and a plural entry, and French counts 0 and 1 as singular. The catalogue is one text file per
language, `English text => texte`, included with `include_str!` (no file is read at run time, nothing can be missing),
parsed once, searched by binary search, and never allocates for a text it knows.

**Decision.** Option 3. The catalogue is `crates/ui/locales/fr.txt` and the lookup is `atomik_ui::i18n` (`t`, `tr`,
`tf`, `tn`, `key_name`). The language is a setting (`language` of `workflow.json`: `system`, `en` or `fr`), shown in
Settings > Video under the update question, where there was room; System reads the language of the operating system
once at start (the `sys-locale` crate, see [DEPENDENCIES.md](DEPENDENCIES.md)) and falls back to English. A change
applies at once: the shell hands the views a fresh theme, which drops every cache of drawn text, and lays them out
again. The project format is unchanged, because the language belongs to the person and not to the song. The tests
never read the language of the machine: the process starts in English and a test that wants French scopes it to its
own thread. The manual (`docs/README.md` and `docs/KEYBOARD.md`), the texts of Help Mode (`help/*.json`) and the
names of devices stay in English for now. The French uses the glossary at the top of the catalogue (Piste, Clip,
Boucle, Calque, Table de mixage, Envois, Retours, Réglages, Annuler, Rétablir).

**Status.** Default (not yet confirmed). Decided by: none yet.

## X. Themes are JSON files

**Question.** The colours of the interface were two constructors in code, Light and Dark. Users want their own
themes, and a third look (a neon, 1980s one) should not need a change of the program. How are themes kept?

**Options.** (1) More constructors in code: nothing can be shared or edited, and each theme is a release. (2) A text
format of its own: one more parser to write and test. (3) JSON, which the program already reads and writes for its
settings, with a version field.

**Implications.** With (3) the built-in themes can be files too, compiled in with `include_str!`, so the built-in and the
user themes go through one loader and a built-in theme is an example of the format. The cost is that a file can be wrong,
so the loader checks the version, the name and each colour, skips a file that fails (one log line and one message
naming the files, never deleting anything) and lets a file leave colours out: they come from the built-in theme it is
based on, so a theme made for this version still opens when a later version adds a colour. The built-in Light and Dark
files give back the old palettes byte for byte (a test compares them with the former code), so no existing rendering
changes. A theme is a preference and not project data: it lives in `workflow.json` (`theme_id`, written only when a theme
other than Dark, Light or System is in use, so an older file with `"theme": "dark"` still gives Dark) and in the
`themes` folder next to `settings.json`, and the project format stays as it is.

**Decision.** Option 3. The format is described in [EXTENDING.md](EXTENDING.md#9-making-a-theme). A theme sets the colour
tokens of `Theme` (about sixty), a few numbers that were already in `Theme`, and nothing else: no new styling
dimension. A glow on the meters and the playhead was left out, since it would cost frame time for one theme.

**Status.** Default (not yet confirmed). Decided by: none yet.

## Y. The pitch-keeping preview of a warped clip is a real-time WSOLA

**Question.** A warped clip plays a buffer rendered by a worker. Until it is ready the engine plays an interim read
through the clip's time map, which was a resampling: the pitch moved with the stretch and a transposition was not
heard. What should play instead?

**Options.** (1) Keep the resampling. (2) A real-time phase vocoder like the Tonal mode: FFTs on the audio thread,
a heavier and more complex state per clip. (3) A streaming WSOLA like the body of the Beats mode: overlap-added
windowed pieces of the source, each moved a little to continue the previous one, and a resample for the transposition.

**Implications.** WSOLA needs no FFT and only a few small buffers, so it can be built once and run with no allocation;
its cost is bounded by limiting the similarity search (a quarter frame either way, on the sum of both channels, every
eighth sample over candidates four apart, then refined), which measured about 2.4 ms per second of stereo audio at
48 kHz per clip (16 tracks of transposed interim warps took 8.9 % of real time against 5.1 % for the resampled read, see
[perf/BASELINE.md](../perf/BASELINE.md)), about 8 % of one core for 32 clips. It is rougher than the Tonal buffer on
sustained chords and smears attacks more than the Beats buffer, which is acceptable for the second or two before the
buffer replaces it. At a ratio of 1 with no transposition it gives back the source bit for bit, so an unstretched clip
does not change. The stretchers are preallocated per audio track when the song is compiled: a fixed cap of four keeps
the memory bounded (each is sized for 192 kHz, about 150 KB), and regions take the slots in turn, so two that sound
together never share one. A region that finds its slot busy within one block falls back to the resampling and is
counted.

**Decision.** Option 3 for Beats, Tonal and transposed clips; a Repitch clip without transposition keeps the resampling,
which is its true sound. Reversed and streamed clips keep their behaviour (no interim warp). Offline renders never use
it.

**Status.** Default (not yet confirmed). Decided by: none yet.

## Z. Vocal pitch edits are pinned to source frames and rendered with TD-PSOLA

**Question.** The Vocal mode of the audio clip editor moves the notes of a monophonic clip in pitch, flattens their
drift and moves them in time. What is saved, and how is it rendered?

**Options.** Saving: (1) the notes and their edits in clip time (ticks), (2) per-note edits in the asset's own frames,
with the note analysis done again on open. Rendering: (a) the Tonal phase vocoder with a time-varying ratio, (b) a
resample per note, (c) TD-PSOLA driven by the monophonic tracker's pitch.

**Implications.** Edits in ticks move away from their audio whenever the tempo or a warp marker changes; edits in
source frames stay on the note whatever the time map does, survive a split and a join by their span, and keep the file
small (the analysis is not saved: it is quick and found again). A resample moves the formants with the pitch (a
chipmunk voice); the phase vocoder keeps the length but smears the voice and moves the formants too. PSOLA overlap-adds
one grain per period at the new spacing without resampling the grains, so the spectral envelope stays put; at a ratio
of 1 the grains add back to the source exactly. It needs a pitch mark per period, which the tracker gives, and only
suits one voice at a time. Applying it to the asset span before the time map lets the warp stretch run on the corrected
audio (one more pass, no new combined engine) and keeps the bounce bit-identical.

**Decision.** (2) and (c): `pitch_edits` on the audio clip, pinned to asset frames, an ignorable field of format 45;
`atomik_dsp::stretch::psola` applies them to the span a warp job reads, then the warp runs. Each note glides in and out
of its edit over 20 ms. A clip with edits is always rendered, through the 1:1 map when warp is off. Timing moves use
warp markers, not a new kind of edit.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AA. Chords from audio: a voice-folded chroma, triad-biased templates, Viterbi on the beats

**Question.** How does Detect Chords from Audio turn a recording into chord-lane chords, and how far can polyphonic
audio to notes go?

**Options.** Chroma: (1) fold the raw spectrum into twelve bins, (2) fold the pitches a multi-pitch search finds.
Recognition: (a) a trained model, (b) templates of each chord quality scored by cosine, frame by frame, (c) the same
templates over beat units with a Viterbi pass. Polyphonic notes: (i) a transcription model, (ii) the voices of the
multi-pitch search tracked into notes.

**Implications.** A raw chroma counts a note's third and fifth harmonics as the fifth and the major third of the
chord, so a lone C reads as C major and a minor chord leans major; folding the voices of a harmonic-sum search with
iterative cancellation (a smooth harmonic envelope taken out per voice, at most six voices) counts each note once. A
model needs training data and a dependency and cannot be checked offline against synthetic material; templates are a
few lines, deterministic and cover every lane quality. Frame-by-frame decisions flicker on passing notes; units on the
song's beat grid (a beat, half bar or bar) with a self-transition bonus make chords change only on beats, at most once
per unit, and a cached analysis lets the panel re-score instantly. Triads come first by a small penalty on sevenths and
sixths, so a seventh wins only when it is clearly there; a bass bonus tells Am7 from C6. A recording off A = 440 is
corrected from the circular mean of the voices' fundamentals. Tracking the same voices into notes gives usable
sketches of a piano or guitar part, but harmonics shared by notes an octave or a twelfth apart, dense mixes and
distortion still lose or add notes.

**Decision.** (2), (c) and (ii): `atomik_dsp::harmony` (whitened STFT peaks as fractional keys, multi-pitch with
cancellation, tuning, chroma and bass chroma), `chords` (templates of 13 qualities on 12 roots plus no chord, Viterbi
over units, slash basses), `poly` (voice tracks into notes snapped to onsets). The key from audio compares the summed
chroma with the key profiles of `key_detect`; on an exact tie the major key wins.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AB. The song harmony reaches devices: compiled with the song, looked up per batch

**Question.** How do note devices (Key & Chord, the Arpeggiator, the CV Quantizer) read the song key and the chord
lane on the audio thread, live and in bounces?

**Options.** (1) Devices get the model's harmony through a shared lock; (2) the app sends key and chord changes as
timed events into each device; (3) the harmony is compiled with the song into sorted shared slices that the engine
swaps with the song, and the player hands each batch a fixed-size view of what holds over the batch.

**Implications.** A lock on the audio thread is out. Timed events need a route per device and a chase on every locate
and loop wrap. A compiled harmony costs a binary search per batch, follows locates, loops, tempo maps and the clip
launcher for free, and is the same in a bounce. A view that borrows the compiled slices cannot live in the copyable
time info the workers receive; a small copy can: the key and chord at the first frame and at every frame where one
changes inside the batch (eight at most; a 64-frame batch never holds more in music).

**Decision.** (3). `atomik_core::harmony::SongHarmony` (in `SongInput::harmony`, built by `song_harmony` in the
renderer) and `HarmonyNow` in `TimeInfo::harmony`, read through `ProcessContext::harmony`. A change lands on the frame
where a note of the same tick is emitted. Stopped, the batch holds the harmony at the playhead. A harmony edit now
reports `SongChanged` so the engine gets it with the next song swap. The new device params are appended with defaults
that follow nothing, so older songs and patches play as before.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AC. The Learn strip of a plugin window is a docked window of its own

**Question.** Learn in the plugin window needs a thin app-drawn strip with the plugin's name and the LEARN AUTOMATION
and LEARN MIDI toggles at each plugin editor. Where does it go for each kind of editor: the embedded ones (our own
top-level window, with the plugin's view embedded in it) and the external ones (the window of a sandboxed plugin's own
process, the default)?

**Options.** (1) Inside the editor window: the window grows by the strip's height, the plugin's view goes into a native
child area below it and the app draws the strip above; (2) a borderless window of the app, docked to the editor
window's top edge and following its moves and resizes, for both kinds; (3) the strip only in the parameter pane.

**Implications.** (1) needs a native child window (a child `HWND`, an `NSView` subview, an X11 subwindow) between the
top-level window and the plugin's view, which the app can only make through `unsafe` platform calls (the workspace
denies `unsafe` outside the plugin backends), and it cannot reach into the window of a sandboxed plugin's process at
all. (2) never puts a surface over or beside the plugin's view inside one window, so it cannot cause the flicker and
focus bugs that made the editors top-level windows in the first place; it needs the editor's frame, which our own
windows give and which a plugin's process now reports (`GuiFrame`, on every move and resize and when the editor is
shown). (3) leaves the user's eyes on the rack instead of the plugin.

**Decision.** (2), with (3) as the fallback. The strip is an undecorated window with its own surface, shown without
being activated, owned on Windows by the editor window (embedded) or the app window (external) so it stays above them
and minimises with them, an X11 utility window on Linux; it docks right above the editor's frame, or below it when
the space above is off the screen. A Wayland session gives no window positions, so it has no strip; the parameter
pane's top bar has the same two toggles everywhere. The mode is view state per editor, never saved.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AD. Webcam hand tracking: the camera, the models, privacy and the gesture rules

**Question.** The webcam is to become a controller: pinch the thumb and index finger in front of the camera and
twist the wrist like a knob. How is the camera read on each system, how are the hand models run, where do the model
files come from, what happens to the pictures, and how is the gesture decided?

**Options.** Camera: (1) `nokhwa` on every system; (2) `nokhwa` on Windows and macOS and a pure Rust Video4Linux
client (`linuxvideo`) on Linux; (3) OpenCV; (4) the platform calls by hand. Inference: (1) `tract` reading the
official `.tflite` files; (2) `tract` on `.onnx` files converted from them; (3) ONNX Runtime or the TensorFlow Lite
C++ library through bindings. Model files: (1) bundled in the installer; (2) downloaded once after the user agrees;
(3) a bigger model that is downloaded every run. Gesture: a trained gesture network, or plain rules on the 21
landmarks.

**Implications.** `nokhwa` is Apache-2.0 and wraps Media Foundation and AVFoundation, but its Linux backend goes
through `v4l`, whose `v4l2-sys-mit` runs `bindgen` at build time (libclang and the kernel headers on every build
machine and in CI); its default JPEG decoder is a C library, and the program already has `zune-jpeg`, so the default
features are off and the frames are converted in `camera/convert.rs`. `linuxvideo` is 0BSD (added to the allowed
licences), has no `bindgen` and no system library, and reads Video4Linux with the plain kernel interface. OpenCV is a
huge C++ dependency. Platform calls by hand mean `unsafe` in our crates, which the workspace denies; the capture
crates use `unsafe` inside, ours does not. `tract` is pure Rust (MIT or Apache-2.0) and reads the official `.tflite`
files directly: both models load and run, so no conversion script, no extra hosting and no second copy of a file
whose licence is not ours to republish. Its build script only asks the C compiler of the machine which assembly it
can use (every macOS and Linux build machine has one). ONNX Runtime and TensorFlow Lite are C++ libraries with
downloads or a CMake build. The two models are 2.3 and 5.5 MB; bundling them makes every installer bigger for a
feature that not every user has a camera for, so they are downloaded once, after a dialog that says what, from where
and how big, from one fixed HTTPS address each (`ureq`, no redirect, nothing about the user or the machine), and are
checked against a SHA-256 written in the source at the download and again before each run. The camera picture is
the most private thing the program can touch: it is only used after a user action, only in memory (never written,
logged, sent or put in a project), only while the camera is on, and a lit indicator shows when it is (the
application side comes with the wiring). A twist read from the wrist angle needs no network: the gesture is the
angle of the wrist-to-knuckle line and the distance between two fingertips.

**Decision.** (2), (1), (2), plain rules. The crate is `atomik-vision`, with no dependency on the application. The
pinch is the thumb tip to index tip distance over the hand size (wrist to middle knuckle): it starts below 0.25 and
ends above 0.40, so a hand near one number does not flicker; the numbers come from the synthetic hands of the tests
and wait for hand checks. The twist is a ratchet: the angle of the wrist-to-knuckle line, unwrapped, smoothed with a
One Euro filter and measured from the angle at the moment the pinch began, so a user can open the hand, turn it back
and pinch again to keep turning; a change under 1.5 degrees is held back, so a still hand does not drift. *Revised
(Nov 14, from a hand check):* a wrist cannot turn half a turn, and a user turns a knob that faces the camera like a
bottle cap, fingers pointing at the lens, which leaves the wrist-to-knuckle line a few wandering pixels long. The
twist is now the turn of that line and of the knuckle line (index to little finger), each weighted by the square of
its length in the picture in this frame and the last; the hand size is never under the knuckle line over 0.85; and
the pinch distance counts the model's depth of the two fingertips, so loosening a grip towards the camera lets go. A hand that
is lost keeps its pinch for 150 ms, then the pinch ends. The palm detector runs only when no hand is followed (the
landmarks of one frame frame the next), so a followed hand costs one run of the landmark model. The capture is on its
own thread with a one-frame handoff (the newest frame wins, nothing queues).

**Status.** Default (not yet confirmed). Decided by: none yet.

## AE. Webcam knob: the camera target, the gesture and the switch

**Question.** A pinch and twist of the hand must turn "a knob". Which one, how far does a twist move it, how does it
join the undo history and the automation, and when is the camera on?

**Options.** Target: (1) the control under a pointer that follows the hand (needs the camera to see the screen);
(2) the control last pressed with the mouse; (3) the selected device's first param. Move: (1) a position of the hand
sets the value; (2) a ratchet of the twist adds to it. History: (1) every twist is a step; (2) a pinch is one gesture
through the path a mapped controller uses. Switch: (1) a setting that starts with the app; (2) a switch the user turns
on each time.

**Implications.** (1) of the target would need the camera to find the screen and the pointer. (3) hides what will
move. (2) is what the user already sees and does with the mouse, and every press on a control already reaches the app as
one event (`ParamTouched`), so there is one place to set it and no second hit test. A plugin's own window is not
the app's: its touch detection (a gesture begin, or the first change after half a second) is reused, whether or not a
Learn mode is armed. A position of the hand (1) jumps the value when the pinch starts; the ratchet adds the twist to
the value the control has, so it never jumps. The gesture reuses the merge id and the held gesture of the mapped
controllers (`GestureKey`), so one pinch is one undo step, Touch automation writes while the fingers are together and a
plugin parameter moves like any other. A switch that starts with the app would put a camera in use without a user
action.

**Decision.** (2), (2), (2), (2). The camera target is the last control pressed, a `(device or strip, param key)` in
view state only: never saved, cleared when its device goes (a delete or the undo of its creation), marked with a ring
and a small lens while the camera is on. A twist of `d` radians adds `d / full_turn` of the normalised range, where
`full_turn` is the setting "Twist for full range" (30 to 360 degrees, default 150); with "Fine control by distance" the
hand size at the pinch scales it from 1 (a hand of 0.12 frame heights or smaller, far away) to 0.5 (0.30 or larger,
near), linearly in between. A stepped param counts steps from where the pinch began, one for each step width of turn,
and takes a step back only after a whole step width the other way. The turn is kept inside the range, so there is no
wind-up. A pinch with no target changes nothing and says "Click a knob first". The camera is off by default, on only
after the user turns it on (the CAM button, `camera.toggle`, View > Panels > Camera), and off when turned off, when the
app quits and on any camera error; the hand models download only after the consent dialog's Download. The settings
(camera, hand, full turn, dead zone, reverse, fine control, pinch sensitivity, preview) are workflow preferences in a
`camera` object of `workflow.json`, never in a project. The format is not changed.

**Calibration (added after the first hand check).** A wrist turns about 150 degrees from one side to the other, so the
first defaults (270 degrees, a quarter near the camera) asked for far more twist than a hand gives, and "Pinch
sensitivity" was read as a twist setting. The default is now 150 degrees, the near scale 0.5, the setting is called
"Pinch detection", and `camera.calibrate` measures the user: a held still pinch (1.5 s after 0.3 s to settle) gives
the jitter `j` (peak to peak), then the right end and the left end (each held 0.8 s within max(2, 2j) degrees, the
right at least 10 degrees from the start, the left at least 20 from the right). `full_turn` is the span between the
ends, the dead zone (a tracker setting, 0.5 to 6 degrees, default 1.5) is `0.75 j + 0.3`, and `reverse` is set when
the right end reads below the left. The angle is the tracker's, before `reverse`, so a calibration never depends on
the last one. Letting go starts again from the still step; 30 s without finishing, the command again or the camera
going off change nothing. A pinch turns no knob while it runs, and the result is saved to `workflow.json` at once.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AF. The camera as a virtual MIDI controller: how a mapping names it, and why the format is unchanged

**Question.** The webcam is to be a controller that MIDI Learn can map to anything, next to the knob it already turns.
How does a saved mapping say that its source is the camera, which controls does a hand have, and which notes may reach
an instrument?

**Options.** Source: (1) a new field on a mapping and a project format bump; (2) the `surface` field, with a reserved
surface name; (3) the `port` field, with a reserved input name. Controls: (1) a set the user defines; (2) a fixed set
per hand. Notes: (1) the camera is a keyboard like any port; (2) a controller whose notes feed mappings only, with a
switch to play an instrument.

**Implications.** (1) needs a bump and a migration for something the existing fields already say. (2) would make a
camera mapping look like a generic surface, which has its own lock and device logic, and a user may name a surface
"Camera". (3) is the field a mapping already uses for "this input only": a mapping with a port hears that port only, and
the camera goes through the same list of ports as the others in the MIDI Mappings panel. A user-defined set of controls
needs a place to edit them; a fixed set is documented once, has fixed channels (the right hand 1, the left hand 2) and
lets mappings, presets and the documentation agree. A keyboard that plays by default would put a note on the selected
instrument every time a finger touched, with the camera in front of a person who is talking, gesturing or just working.

**Decision.** (3), (2), (2). A camera source is `port = "atomik:camera"` (the reserved name, shown as Camera), on channel
1 or 2, with the fixed controller or note of its control: twist is controller 20 (a relative encoder in two's
complement; a learned twist starts as an endless encoder), the pinch is note 60, the palm height controller 21, left
and right controller 22, hand closeness controller 23 and the open palm note 61. Camera messages are heard only by
mappings whose port is the camera, and the camera's mappings are heard by nothing else, even a real port that is named
the same. Nothing changes in a saved project: a mapping that names the camera is an ordinary mapping with a port, the
format stays 45 and an older build reads such a mapping as one that listens to an input it does not have. The camera's
mode (Turn clicked knob, MIDI controller, Both) is a workflow setting; with MIDI Learn armed the camera sends in any mode
so a control can be learned without switching. The camera's notes play an instrument only with the setting "Camera notes
play the selected instrument", off by default: a note track on all inputs never hears them. The note tracks have no
per-track input choice yet, so that setting stands for choosing Camera as an input.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AG. Meter panels on strips and plugin devices: where the tap sits, what it costs, how big the FFT may be

**Question.** A console strip or a plugin device should be able to show a spectrum or a phase meter without a device
being inserted in a chain. Where does the panel get its signal, what does that cost while nobody looks, and how large an
FFT may it run?

**Options.** Signal: (1) an analyser device that the app inserts and hides; (2) a second tap in each strip with a switch
param of its own; (3) the tap the strips already have, with the point after the fader and a flag in the tap that the UI
sets. Cost: the flag is read once per block, or the tap always writes. Size: raise the ring of every tap, or give
the large ring to the spectrum device only.

**Implications.** (1) changes the graph, the latency report and every bounce, and has to be undone when the panel
closes. (2) adds a param to every strip and plugin (the plugin layout is versioned and part of its saved state) for
something that is not part of the sound. (3) adds nothing to a descriptor or the format: the flag lives in the
`ScopeTap` the engine already hands to the UI, `Arc` shared, so the UI sets it without a message to the audio thread
and the device checks it with one relaxed load per batch. A plugin device has no tap yet and gets one the same way. A
ring of 131 072 frames is 1 MiB: every strip and plugin would pay that for a size almost nobody uses, so the strips and
plugins keep 32 768 frames, which serves analyses up to 16384 points (a read is capped at half a ring so the writer can
never lap it), and only the spectrum device has the large ring and the sizes 32768 and 65536. The analysis is the UI's
already (`atomik_dsp::spectrum`), so the size is not a real-time question; a 65536-point FFT is about a millisecond.

**Decision.** (3), the flag read once per batch, the large ring on the spectrum device only. The point is after the fader and the
pan of a channel or bus, after the fader of the master and after the bypass mix of a plugin device; the pre-fader point of
the Spectrum EQ window wins on a strip it also shows. With the flag off nothing is written and the output is
bit-identical, with it on the output is still bit-identical, because the tap only copies. The panels are view state, not saved.
The second curve of a spectrum panel is another strip (its own tap, switched on too) or the reference track, whose
long-term average spectrum is measured with the file on the worker that decodes it and moved by the matched gain and the
trim. A trim is one more optional field of a reference in the `references` list, written only when it is not zero, so
the project format stays 45 and a build without it drops the field with the list.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AH. Webcam gestures: a fixed vocabulary, a table that is empty by default, and a per-project override

**Question.** The webcam should do more than turn one knob: a second hand, a few gestures that run app commands, and
camera settings that follow a project. Which gestures, how are they mapped to commands, what happens by default, and
where does a per-project choice live without a format bump?

**Options.** Vocabulary: (1) a set the user records; (2) a small fixed set found by plain code from the landmarks.
Mapping: (1) only MIDI Learn on notes; (2) a table of gesture to command (a picker like the keyboard editor's), with
the notes as well. Default: (1) a useful set on; (2) the table empty, with a Suggested button. Per project: (1) a
format bump with a new required field; (2) an optional ignorable root field.

**Implications.** A recorded gesture needs a classifier and training data the project does not have, and a false
positive stops or records the song in front of an audience, so the vocabulary is fixed and tested on synthesized hands
like the pinch: fist, thumbs up, point and peace (each from which fingers are extended or curled, held 300 ms), the open
palm that already existed (a second for a command, so an ordinary pause of the hand never plays) and a swipe (half a
frame height in 400 ms, mostly sideways, with a lockout). A command runs through the shell exactly as its key would, so
it asks whatever that key asks and no confirmation dialog is added. A gesture fires once per hold, re-arms only after the
hand changes pose, waits 500 ms after any command, and never fires while a hand pinches (a pinch is a knob or the pad);
a hold that is ready inside the cooldown stays silent rather than firing late, which is the predictable choice. The same
gestures are Camera notes 62 to 67, so MIDI Learn can map them with everything else. The two-hand pad is both hands
pinched: the middle between the palms and their distance are X, Y and spread (controllers 24 to 26 on a third channel,
or the last two clicked knobs, one undo step as a batch with one merge id). The per-project override is the optional root
field `camera` (the mode and the table, ignorable, written only when set), so the format stays 45 and an older build
plays the project as before.

**Decision.** A fixed vocabulary of fist, open palm, thumbs up, point, peace, swipe left and swipe right; a table of
gesture, hand (either, left or right) and command in Settings > Camera that is **empty by default**, with a Suggested
button (fist = Stop, open palm held for a second = Play, thumbs up = Record); the gestures also as notes 62 to 67; the
pad on channel 3; and the `camera` project field. The defaults are off because a gesture is the one input that fires
without the user touching anything: a wave at a friend, a stretch or a stopped hand must never stop, play or record, so
the user opts in, and sees what they chose.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AI. A clip that opts out of the song key: editor only, and a chord in Sound Match on the poly synth

**Question.** A note clip follows the song key unless it has a scale of its own. Should a clip be able to opt out of the
key without having a scale, and should the devices that follow the song key (Key and Chord, the CV Quantizer) skip its
notes too? And when a sound held to be matched is a chord, which synth plays it and how is the match measured?

**Options.** Opt-out: (1) editor only (no highlight and no snap from the key); (2) editor and devices, by tagging each
note event with the clip it came from. Chord in Sound Match: (1) keep one pitch and ignore the other notes; (2) the poly
synth plays every note, with the chord named; (3) the FM synth.

**Implications.** The engine hands a device bare note events (a frame offset, a key, a velocity): a note carries no clip
of origin, and adding one touches every event the devices copy on the audio thread, for one flag. A device reads the song
key at the frame it sits at, not for a clip. A chord has no single pitch and its partials do not line up as one note's do,
so the single-pitch curve and the harmonic kind mean nothing for it; the level and brightness curves still do.

**Decision.** Opt-out (1): the toggle `Follow song key` (the Scale menu of the piano roll) is a clip field written only
when off (ignorable, so the format stays 45), and it is limited to the editor: the clip is not highlighted or snapped by
the song key, and the devices keep following the song key for its notes. The document says so. Chord (2): the sound is
read with the multi-pitch analysis of the chord detection, a key counts as a note when it is in most of the loud frames
and at least a third as strong as the strongest, and fewer than two pitch classes is one note, which takes the single-note
path exactly as before. The poly synth plays all the notes on as many voices; the waveform and the envelopes come from the
profile as for one note; the pitch curve is left out of the match; the numbers show the chord and its notes in place of
the pitch.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AJ. Plugins that share a process, editors in the sandbox, and a quarantine that holds at load

**Question.** (1) Should several sandboxed plugin instances be able to share one child process, and what is the crash
domain? (2) Can a sandboxed editor be embedded in a window of the app, so that the Learn strip sits inside it? (3) A
plugin that failed three times in a row is quarantined: should a project that uses it still load it, and should the
manager say why?

**Options.** Sharing: (1) always one child per instance; (2) a setting with three levels, and a choice per plugin;
(3) a shared child with one thread for all instances. Embedding: (1) keep the companion window; (2) re-parent the
child's window into a window of the app, per platform. Quarantine: (1) hide only (as a disabled plugin); (2) do not load
it in a project until the user says so.

**Implications.** One child per instance isolates fully and costs a process each; a shared child costs less but a crash
or a hang of any plugin ends or stalls every plugin in it, and the plugins share the child's main thread (editors, timers,
licence dialogs). The audio path is the part that must stay real-time safe: each instance has its own shared block and
doorbell, and one thread serving two instances would let a slow plugin delay another's batch. Embedding across processes
is a platform matter: a Windows child can make its window a child of a window handle the app sends (the handle is the
same kind the owner already is), with focus and resize to manage between two threads of two processes, and the winit
call that does it is `unsafe`; X11 can re-parent a window with XEmbed; macOS has no embedding of a view of another
process without remote views. A quarantine that only hides leaves a project that uses the plugin to crash again and
again; one that blocks the load turns a device into a stand-in the user can decide about.

**Decision.** (1) The setting `plugin_process` (Settings > Plugins) has three levels: one child per plugin instance (the
default, today's behaviour, and the only wire traffic when it is chosen), one per plugin file, and one shared child
(always per plugin format). A plugin can ask for its own child or for the shared one in the plugin manager's PROCESS
column, kept in `plugins_user.json` and not in the project. A group child (`atomik-plugin-host --group`) holds at most 16
instances, and the 17th starts another child of the same kind. Each instance of a group has its own slot on the control
channel, its own shared audio block, its own doorbell and **its own audio thread**: the audio path of an instance is
exactly that of an ungrouped child, and no queue or lock is shared between instances on it. The crash domain is the
process: when it ends or is stopped (a crash, a hang, a call that was not answered, a broken protocol) every instance in it
is lost together, each reason names the group and its plugins, and RELOAD creates new instances, which start a new group.
A hang timeout of one instance stops the whole child. An instance that is closed leaves its group and the child goes on for
the others; one that does not answer is abandoned, not killed, so a slow close never punishes the others. A setting change
applies to instances created after it. (2) Windows was investigated and is not built: it needs the child to create its
window as a child of the app's handle (an `unsafe` winit call in the child crate, a resize and focus protocol between the
two processes, and the strip drawn by the app in the same window), and none of that can be tested without a window; the
app also keeps editors out of its own GPU window on purpose. The companion Learn strip stays on every platform. On Linux
an XEmbed re-parent is not cheap with winit (it needs the X11 window of the app's editor window and the plugin's own
window ids across processes) and stays a companion window; on macOS it is not possible across processes. (3) A quarantined
plugin is not loaded when a project opens: its devices stay stand-ins that say QUARANTINED with the reason of the last
failure, the rest of the project opens normally, and LOAD ANYWAY (the RELOAD button) loads that device, for this session,
without lifting the quarantine; TRY AGAIN in the manager lifts it. The project file is unchanged: it is a decision at load.
The reason (Crashed, Hung or Could not load, three times in a row, and the plugin's own words) is kept with the quarantine
in `plugins_user.json` and shown in the manager's status column and its tooltip.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AK. Stem separation: signal processing, because no model is free of non-commercial terms

**Question.** How does the program split a song into vocals, drums, bass and other, under the rule that a model whose
weights or training data carry non-commercial terms is not used?

**The rule.** A model whose weights OR training data carry non-commercial terms is a no-go, however permissive the
licence written on the weights is.

**Rejected.** Open-Unmix `umxhq` (MIT on the weights, but trained on MUSDB18-HQ, whose terms are non-commercial: a
licence on the weights does not remove the terms of the data); Open-Unmix `umxl` (CC BY-NC-SA 4.0); Demucs (the author
wrote that the weights "are not covered by the MIT license, and are provided only for scientific purposes"). Other
published separators were looked at and fail the same rule: they are trained on multitrack sets released for
non-commercial research (MUSDB18 and others under CC BY-NC-SA) or say nothing about their weights.

**Decision.** `atomik-stems` is a signal-processing separator with no model and no download, so there is no network
use, no file to host and no licence to carry. It is a 2048-point STFT with a hop of 512 (46 ms, 21.5 Hz bins). The
drums are the percussive part of a harmonic/percussive split: a median of 17 frames across time (about 200 ms) is the
harmonic estimate, a median of 17 bins across frequency (about 370 Hz) the percussive one, and the soft mask is the
percussive power over the sum of the two. The bass is the harmonic part under a smooth crossover (a smoothstep from 100
to 200 Hz, centred on 150 Hz). The vocals are the harmonic part above it that is centred in the stereo image (a mask
from the left/right similarity of each cell) with a weight that keeps the voice's range (to 5 kHz) at full strength; a
mono input has no image, so its vocals are the 200 Hz to 5 kHz harmonic band at a weight of 0.6, and the dialog says so.
The other stem is the mix minus the three, in the time domain, so the four add up to the mix to float rounding. A song
is cut into chunks of 20 s with 1 s of context and a linear crossfade, so memory depends on the chunk; the median passes
run on a few threads over frame ranges, and every cell is the same function of the same inputs, so the same input
gives the same bits whatever the thread count. A 3-minute stereo song takes a few seconds in a release build.

**Limits, said in the dialog and the README.** It is a fast split with bleed between stems, not a learned model: a
kick leaks into the bass, a snare tail into the other stem, a centred guitar or synth into the vocals, and reverb is
shared. It is a starting point to mix with.

**Later.** The `StemBackend` trait stays, so a model can be plugged in when one exists whose weights and training data
both allow commercial use and redistribution in a GPL-3.0 program. None is known today.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AL. Meter taps: a point flag and a large ring on request, and the device curve beside the device

**Question.** How can a meter panel show a strip before its fader, and a window of 65536 points, without a megabyte
per strip, a lock on the audio thread, or a change to what a bounce renders?

**Decision.** The tap (`ScopeTap`) gains a **point flag** next to its live flag: a strip writes the signal after its
EQ, inserts and parallel compressor (the point the Spectrum EQ window already used) when the flag says PRE, and its
output otherwise. The flag is read only while the tap is live, so a tap nobody shows still costs one relaxed load. The
point belongs to the tap, and a tap is shared by every panel of a strip, so the app sends PRE for a strip while any
panel of it asks for it; a strip drawn only as a second curve stays POST. The tap also gains a **large ring** of 131 072
frames that the UI allocates when a panel picks 32768 or 65536 points and stores in a `OnceLock`: the writer does one
`OnceLock::get` per block (an atomic load) and, once the ring exists, writes into both rings; no lock and no allocation
on the audio thread, and no `unsafe`. The history of the large ring starts at the first block written after it
appeared, and a read takes the ring that holds more, so the first second of a large window is short rather than wrong.
Nothing of this is in an offline render, so a bounce is the same to the bit.

**The second curve of the Spectrum device** is project data beside the device, not a parameter: a source is a strip or
the reference track, and a parameter is a number, which would tie the curve to the position of a strip in the console.
It is the optional root field `spectrum_curves` of format 45 (ignorable), with one undo step per choice, like the fold
state of the insert areas (`inserts_folded`). A key whose device is gone is not written.

**Rejected.** A large ring on every strip from the start (about 1 MiB times every channel and bus for a feature few
use); growing the ring on the audio thread; a per-panel tap (a second copy of the signal per panel, and an engine
change for each).

**Status.** Default (not yet confirmed). Decided by: none yet.

## AM. Formant-preserving pitch shift in the Tonal engine, and where the comp is edited

**Question.** How does a transposed chord or a whole clip keep its formants, where does that run, and how does the
audio clip editor edit takes without a second comp?

**Decision.** The phase vocoder of the Tonal mode moves the envelope itself. A pitch shift is a stretch followed by a
resample, which moves every formant by the pitch ratio `r`. Before the resample, each synthesis frame's bins are
multiplied by `E(j * r / s) / E(j)`, where `E` is the smooth magnitude envelope of the analysis frame and `s` the
formant shift (1 for none); after the resample the envelope is `E(f / s)`. `E` comes from **cepstral liftering**: the
log magnitude spectrum is turned into a cepstrum, the quefrencies below about 1.25 ms are kept (a linear taper to
2.5 ms) and the result is transformed back. LPC was left out: it needs an order per sound and a stable filter, while
the lifter has one number that follows the sample rate. The gain is limited to a factor of 16. It uses only IEEE
arithmetic and the deterministic logarithm and exponential of the crate, so the render is bit-identical on every
platform, and a clip that does not use it takes the same path as before to the bit. It runs in the **rendered buffer
only**: the real-time stretcher (a streaming WSOLA) has no spectrum, so the preview of a clip that is still rendering
keeps the formants moving with the pitch until the buffer is ready. Adding an FFT stage to it would need its own
buffers and a latency; not done.

**Stereo link.** With the option on, the peaks and the phase rotations come from the mid (from the louder channel for
a frame whose mid is nearly empty) and both channels take the same rotation, so every bin keeps the phase relation of
the left and the right. It is a clip option, off for every existing clip, because turning it on changes the render;
no new-clip default was added (the Interface and Workflow tabs are full).

**Saved fields.** `keep_formants`, `formant` (cents) and `link_stereo` in the clip's `warp` object, written only when
set, ignorable at format 45: an older version drops them with a warning and plays the clip as before.

**Comp.** The comp stays the take group's `CompSegment` list, edited by commands on the group; the editor's Comp mode
is a larger view of it. `SetCompRange` is `SetCompSegment` with the crossfade at its two cuts in one step, so a drag
is one undo step; `FlattenComp` replaces the group and its compiled clips by one clip of a render the app made on the
freeze worker (the render path Join Clips of different files uses), also one step. Neither needed a saved field.

**Rejected.** A formant stage after the resample (it would need a second analysis of the shifted signal); LPC; a
second comp model for the editor; making the stereo link the default (it changes old renders).

**Status.** Default (not yet confirmed). Decided by: none yet.

## AN. Plugin state is collected in the background in the live app; explicit saves still wait

**Question.** A plugin answers a request for its state on its own main thread, which is busy exactly when the state
has just changed (a patch loading in its window). The app asked on the UI thread, a second after each change, before
every command and when an editor closed, so a busy plugin froze the whole interface until it answered or was stopped
for taking too long.

**Decision.** In the live app the state is asked for without waiting and stored when it arrives. The request has a
slot of its own on the control link, so it can be out while other calls go on, and nothing times it out: a plugin
that is busy answers when it is free, and one that is gone is noticed by the process watch as before. At most one
request is out per instance. A request that comes while one is out only asks again when that one lands, and an
answer that was overtaken by a restore of the state is dropped. A late, failed or dropped answer never replaces a
stored state. The step in the undo history appears when the state arrives, and changes inside one request fold into
it as bursts already did.

A wait stays only where the stored state is about to be used: save, archive, share, templates, versions, an audio
restart and a preset load need the state now, and undo, redo and any command that could remove, replace or copy a
plugin device capture first, so the removal's undo and the copy carry the latest state. Those take
the request that is out instead of sending another. A recovery snapshot never asks: it is written from
the stored states, and a periodic one asks every plugin first and goes without those that have not
answered after two seconds.

**Rejected.** A shorter timeout (a working plugin is still stopped, and the freeze is only shorter); a thread that
waits for the plugin (a second place that can stall, and the answer still has to reach the document on the UI
thread); capturing on every parameter change (a plugin that flags itself dirty after a capture would loop).

**Open.** What a save does when the plugin does not answer in time: see AO.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AO. A plugin that is alive is never stopped by a timeout; the user decides

**Question.** A request to a sandboxed plugin that is not answered in time (ten seconds for the state, a restore,
an activation or the presets; twelve missed cosmetic calls in a row) stopped the plugin's process and marked it
crashed. In a real log this happened to a plugin that was working: its main thread was busy, its audio thread was
still running. Stopping it lost the user's sound and every setting made in its window since the last capture.

**Decision.** A timeout stops a plugin's process only when the process shows no sign of life. The sign of life is a
counter in the shared block that the process's audio thread bumps on every pass of its loop (one relaxed store, a
pass at least every ten milliseconds whatever the plugin's main thread does); a timed-out wait that sees it move in
its last half second leaves the process alone. The call fails, and the plugin is NOT RESPONDING: the app stops
waiting for it (every later call fails at once, without being sent, and a cosmetic call returns its neutral result),
asks it for nothing more, and the state ends by itself when the late answer arrives. The plugin is not marked
crashed, does not count toward the quarantine and keeps the last state stored for it. The user decides: RELOAD
stops the process and starts a new one from the stored state. A save, archive, share, template or version while a
plugin does not answer completes with the stored state and tells the user once; an audio restart and the removal of
the device go on the same way and write a line in the plugin log. A process with no sign of life is stopped as
before, with the same text, and offline rendering (a bounce, an export, a freeze) keeps its watchdog and stops on a
timeout, so a render never depends on the clock and never enters the state.

**Rejected.** Waiting without a limit (a wedged plugin would block a save forever); a longer timeout (a working plugin
is still stopped, later); a heartbeat written by the plugin's main thread (it stops exactly when the plugin is
busy); a thread in the app that pings the plugin (a second place that can stall); failing a save while a plugin does
not answer (the user loses the rest of the project to one busy plugin).

**Open.** Whether a plugin that stays NOT RESPONDING for minutes should offer to be stopped without the RELOAD
button; whether the notice after a save should offer to save again once the plugin answers.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AP. Render workers spin only inside a callback, and the automatic count follows the physical cores

**Question.** With an audio stream open, every render worker kept a core busy even with the transport stopped and
nothing sounding: the app's worker linger (500 000 spins, chosen so that workers are already awake when a callback
needs them) is longer than the gap between two callbacks, so workers never parked. A stopped 100-device rack cost
829 % of one core with 8 workers and 216 % with 2 (about 20 % with none), and a machine of two cores and four
threads was counted as four cores and given three workers.

**Decision.** The pool has an atomic "a callback is active" flag. The audio thread raises it at the start of every
render call that can use the pool (a plan with a level of two or more devices, a pool that is not empty), unparks
every live worker so the wake-up overlaps the first serial work of the call, and lowers it at the end of the call.
A worker spins while it has spun fewer than the linger **and** the flag is up; otherwise it parks. Inside a callback
nothing changes; between callbacks no worker spins. The audio thread uses atomics and `unpark` only. The linger
setting keeps its choices and its default (500 000 spins): it now bounds the spinning inside one callback, which is
bounded by the callback itself, so the long value costs nothing between callbacks and keeps workers awake through
a long callback. Offline rendering is unchanged in output (bit-identical for any worker count). The automatic worker
count is one per physical core but the audio thread's own, at most 8, one worker on two cores and none on one core;
the physical count comes from the `num_cpus` crate, with the old thread-count guess as the fallback when it reports
nothing or more cores than threads. An explicit worker count is never overridden.

**Evidence.** The paced benchmark (`bench_paced_callbacks_per_linger` in `crates/engine/tests/worker_linger.rs`,
release, 4 workers, a callback every 2.7 ms at 128 frames, no audio device; the table is in `perf/BASELINE.md`)
shows the process CPU falling from 420 to 450 % of a core with the long linger (flag forced up, the old behaviour) to
about 60 % at 128 frames, of which about 50 points are the benchmark's own pacing loop, and from 390 % to 14 % at 480
frames. The price is the OS wake-up of a parked worker at the start of each callback: at 128 frames the median
callback rose from about 42 to about 66 microseconds and the p99 from about 61 to about 140 microseconds (the p99 is
5 % of the 2.7 ms budget), against a p99 of 130 for the old behaviour with the short engine linger; at 480 frames the
p99 is about 260 to 330 microseconds against 200 to 260. The after rows are alike whatever the linger, so the
default does not change.

**Rejected.** Keeping workers spinning for a short time after a callback (the gap is 2.7 ms at 128 frames, longer
than any useful grace, and it would bring the idle cost back in proportion); lowering the default linger (it would
only make workers park inside long callbacks and wait for an OS wake-up between levels); a count of workers read
only from the thread count (it cannot tell two cores of four threads from four cores).

**Status.** Default (not yet confirmed). Decided by: none yet.

## AQ. Cosmetic plugin answers never hold a frame: ask now, use later, and a slow plugin is judged by time

**Question.** The interface asks a sandboxed plugin for things that only polish it: the text of a parameter value
(one request per visible row of the parameter pane for every new value, all in one frame), the size of an editor
(one per resize event of a window or dock drag), the default pins, a layout change, closing the editor. Each waited up
to 250 ms on the interface thread for the plugin's main thread, which a plugin keeps busy exactly when it loads a
preset or builds a window. Three misses made the instance slow for a second, but one answer reset the count, so a
plugin that answered between its stalls cost three full waits again and again; twelve misses stopped the process
although it was alive (the rule of AO covered the important calls, not these).

**Decision.** A cosmetic request is sent at once and filed in a small table of the link; the reader thread files the
answer whenever it comes, and the app collects the late ones every frame (parameter texts and editor sizes) and puts
them to use: a text fills its row only if the row still shows the value it was asked for, and a parameter whose
text is not there shows its number, never the text of the previous value. The main thread waits for an answer only
for what is left of one allowance of 20 ms per frame shared by all instances, never longer than 250 ms for one
request, and not at all for an instance that was waited for in vain already in this frame. An instance is slow while
three of its answers within five seconds were late; one fast answer does not end that, time does. A slow instance is
waited for once a second at most (its requests still go out and their answers are still used), and the user gets one
warning per slow episode instead of one per recovery. Resizing keeps one request out per instance and only the
latest wanted size behind it, so a drag is two requests, not a backlog. Closing the editor keeps its single bounded
wait of 250 ms, since the window must not go while the plugin may still use it; the state is then asked for in the
background as before. Late cosmetic answers never stop or mark a plugin; a request that nothing answered for as
long as an activation takes (the `create` timeout, 10 s) is judged like any other unanswered call: NOT RESPONDING
when the process shows a sign of life, stopped when it does not. A bounce keeps a plain wait of 250 ms and no
allowance, so it does not depend on the frame. A plugin in this process still answers synchronously.

**Rejected.** Dropping the late answer (the pane stayed on the number although the plugin had answered); a thread
that asks the plugin on the app's behalf (another place that can stall, and the answer still has to reach the UI
thread); a shorter wait per call without a shared allowance (thirty rows still add up); keeping the last text up
while the plugin is busy (it is the text of another value); killing after a count of misses (a live plugin is
never stopped for being slow).

**Open.** The 20 ms allowance and the five-second window are not tuned against real plugins; the hand check is a
parameter pane open while a sampler plugin loads a large patch.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AR. The live interface never waits for a plugin to load, restart or end; offline rendering does

**Question.** After the state captures and the cosmetic calls, three more things still held the interface thread,
each for up to 10 s: creating a plugin (restoring its saved state and activating it ran in one frame, so a sampler
restoring a big library froze the app), a restart or an engine restart (every instance deactivated one after the
other, then activated one after the other, so K slow plugins cost K waits), and removing a device or closing a project
(the instance was deactivated and its process ended in a blocking drop of up to three and a half seconds).

**Decision.** In the live app the interface thread starts these calls and looks again on a later frame. A creation
goes through starting, restoring and activating as polled steps with the long load timeout and the sign-of-life
judgement; a restart re-activates the same way and the device plays dry meanwhile; an engine restart sends every
deactivation and then every activation without waiting, and a device is a stand-in until its plugin is active; an
instance that goes away is destroyed for the app at once and deactivated and ended by the next frames, with the
existing bounds (a process that does not end a second after it was asked is stopped). A plugin in this process
and everything offline keep the blocking calls: a bounce, an export and a freeze never depend on how long a plugin
takes, and its event stream never carries a time-dependent entry. The rule is "the live interface never waits for a
plugin; offline rendering does".

**Rejected.** Running the creation on a thread (a plugin needs the thread it was created on for some calls, and the
polled requests already exist); waiting for all deactivations together and then all activations together (the
interface still stands still for the slowest plugin); letting the old instance finish before an undo may create a new
one (the user would wait for a plugin that was removed on purpose); killing a plugin that is alive but slow to restore
(only the load timeout and the absence of a sign of life stop one).

**Open.** A sampler that restores for minutes is not yet tried against the real thing; the hand check is a project
whose sampler holds a large library, opened, restarted by a sample rate change and removed.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AS. A plugin's MIDI controller stand-ins are left out of its parameter list, unless the project uses them

**Question.** Many VST3 plugins publish one parameter for every MIDI channel and controller number, so that a host
without MIDI routing can still automate a controller. On a typical set of plugins most of all listed parameters
were such stand-ins, many plugins hit the 2 048-entry limit with them and had real parameters cut off behind them,
and each one was parsed at start, copied into every instance, written twice into the project's layout snapshot when
the plugin went live, saved and offered in every menu. A project must still load with every lane, learned control,
macro target and stored value intact, including the ones that name a stand-in, and a bounce must not change.

**Decision.** A VST3 parameter is a stand-in when `IMidiMapping` assigns it to exactly one channel and controller
pair, and that controller number goes to a different parameter on at least one other channel. The scan and every
instance (and every re-read after a restart) ask for all 16 channels and all 130 controller numbers once, with a
budget of half a second in total; a plugin that is slower, or has no mapping, keeps today's table. A parameter every
channel reaches through the same controller (a mod wheel, a volume), or only one channel reaches, is a real control
and stays. The stand-ins are left out of the table; the mapping itself is untouched, so controllers still reach the
plugin, and a stand-in's step count is kept so a value for it still converts. The scanner version is 4, so the next
scan rebuilds the plugin list file; an older file still loads and its entries are scanned again.

*Keep what the project uses.* The document is the authority on what must stay: a parameter is **used** when a stored
value, a track's automation lane, a note clip's parameter lane, a macro target or bank assignment, a panel pin or a
learned MIDI control names it (`RackDoc::plugin_param_usage`, which also decides which layout rows a save keeps; the
last two kinds were added to it, since the layout used to be recoverable from the scan and no longer is). The rows
of the used parameters, taken from the layout snapshot the document keeps (its rows are what a save writes), are
handed to the plugin host (`PluginHost::set_pinned_params`) before the descriptors are registered and before an
instance is made. The registry's descriptor of the plugin and the layout of every instance are then the plugin's
own table followed by those rows in id order (`merge_pinned`; the plugin's last parameters give way if the table
would pass the cap, a pinned row never does), so the two agree, the descriptor knows `p<id>` before and after the
plugin is live, and a rescan (which registers the descriptors again) and a reload (which pins what the loaded
project uses) give the same table. The layout snapshot stored in an existing project (it lists the rows the project
used when it was saved) loads unchanged; when the plugin goes live the live layout replaces it, which is the same
rows plus the plugin's own table, so a project saved afterwards writes the same used rows as before. Pins are
recomputed when a document is opened and before each instance is created, so a pasted device that uses a stand-in
is covered too. The bounce refuses a plugin whose table no longer matches the registry's (the time budget could
make the scan and a later instance differ), since a lane would then drive another parameter.

**Rejected.** Dropping every parameter `IMidiMapping` names (it would remove a real volume or mod wheel control that
a plugin maps to one controller); hiding the stand-ins in the menus only (the cost is the table, not the menu);
passing the used ids into the instance's own table read (the scan, the registry and the live layout would each
need the document, and a bounce needs the registry before the plugin is loaded); keeping a separate list of the
stand-ins in the scan cache (a second copy of what the document already holds for the parameters that matter).

**Open.** A referenced parameter the plugin no longer has at all (a newer plugin version dropped it) is pinned too
while the project holds a row for it, so it shows in the list and does nothing; before it was a dead key. A
stand-in's value is not read back from the plugin when a preset loads. The hand check is a project made with an
older version that automates a stand-in of a sampler or synth plugin, opened, saved and bounced.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AT. Plugins attach as they become ready, and a plugin that is loading can be skipped

**Question.** Up to eight plugins load at the same time, but the app followed and attached only the one at the front
of the queue: a plugin that waits for a licence dialog or reads a huge library for minutes left every later,
already loaded plugin a silent stand-in. A loading device offered no way out except waiting for the load timeout
or removing the device.

**Decision.** Every started creation is advanced each frame without being installed (`PluginManager::creation_ready`
stops at an active plugin, so nothing of it is listed or reported before the app takes it), and the oldest ready
one in queue order is attached, one per frame. Devices are keyed by their path, not by the order of attachment, and
an attach changes no audio of the others, so the project sounds the same whichever plugin finishes first. SKIP is
the RELOAD button while a device says LOADING, and the command Skip Loading Plugin for the selected device: it
cancels the creation (the process ends across frames), leaves the stand-in with the stored state untouched, and the
device says SKIPPED. A skipped device is not queued again by a rebuild, a rescan or a paste; RELOAD creates it. It
is not a crash and not a failed load, so neither the crashed settings nor the quarantine count change.

Making the child (the shared block, the doorbell and the process) took about 7 ms per plugin on the development
machine and 60 to 80 ms for the eight that start in one frame, which a virus scanner makes longer. The sandbox now
does it on a short-lived `plugin-spawn` thread per start, so the interface thread spends about 0.3 ms for eight;
the start says "not ready" until the child is handed over, and a start that is skipped before that ends its child as
soon as it is there. A child that cannot be made (the host program is missing or blocked, no memory, the shared block
or the doorbell could not be created) is the app's or the machine's fault, not the plugin's: it is its own error kind
(`BackendError::Spawn`, `is_spawn_failure`), the device says FAILED with the text and RELOAD retries, and nothing is
added to the crashed settings or counted towards the quarantine (a missing host program would otherwise quarantine
every plugin of a project). A creation that ends because it was skipped, cancelled by an engine rebuild, by removing
the device or by closing the project is never an error at all.

**Rejected.** Attaching every ready creation in one frame (an attach does real work and would lengthen the frame);
installing a creation in the manager as soon as it is ready and attaching it later (events of an instance the app
does not know yet would be dropped); a separate SKIP button (the panel has no room, and RELOAD is disabled while
loading anyway); treating a skip as a failure (a quarantine for a plugin the user chose not to wait for).

**Open.** Whether a plugin that stays LOADING for minutes should offer SKIP more prominently, for example in the
status line; the load timeout (ten minutes) still ends a load that nobody skips.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AU. A sandboxed plugin is waited for up to the callback's budget, and one that cannot keep up stops being spun for

**Question.** For every 64-frame batch the audio thread sent the request to the plugin process and spun until the
answer or one batch period (1.3 ms at 48 kHz). The real budget is the host buffer (10 ms at 480 frames), so a
plugin with bursty work (a block-based transform, look-ahead, a preset that has just loaded) missed the period
although it fitted the buffer easily: the batch was dropped (dry for an effect, silence for an instrument) and
heard. And since the parent spins while the child works, a sandboxed plugin cost about twice its own CPU.

**Decision.** The output callback publishes, at its start, the moment by which its plugins must have answered: its
start plus 70 % of the buffer duration (`CALLBACK_PLUGIN_BUDGET_PERCENT`; the rest is for the mix, the conversion
and the hand-over). It is one relaxed atomic store of a time in `CallbackDeadline` (`atomik-core`), a cell of the
process that the stream writes and every live processor reads, so the stream and the plugin layer need no wiring
between them (there is one live callback per process, and a restarted stream writes the same cell). A live batch
waits until the later of one period from now and the published moment, never longer than 100 ms from now and never
past the published moment: a callback with no slack, a stale value from an earlier callback and a missing value
all mean the plain period. Offline processors never read it, nor the clock for a decision. When a plugin has run
out of four full waits in a row, the next 24 batches (about three callbacks) are only polled once and are late at
once when the answer is not there, and then a full wait is tried again; one more miss starts the next cool-down,
and an answer ends the run. Only waits that were actually spent count (a batch that finds the previous request
still out is late at once without a wait and neither counts nor resets). The state is per processor; the render
workers that run plugin devices only read the published value, so several plugins may wait at once on different
threads.

A plugin that needs more than real time must not cost more than before, though: the first version spent 7 ms of every
10 ms callback on such a plugin (each callback holds a few answered batches, which reset the miss count, before the
wait that runs into the deadline), leaving the rest of the engine 30 % of the callback. So there is a second,
independent rule per processor (plain integers, audio thread only, no clock read of its own). A callback is
exhausted for a processor when one of its waits ran out at the published moment; a new callback is seen as the
published value changing. After three exhausted callbacks in a row (`EXHAUSTED_AFTER`) the processor falls back to
the old behaviour: its waits ignore the published value and last one batch period, for 100 callbacks
(`FALLBACK_CALLBACKS`, about a second; counted, not timed). Then the budget is tried again; if it is exhausted three
times again the next span is twice as long, up to 500 callbacks (`FALLBACK_MAX_CALLBACKS`, about five seconds). A
callback inside the budget resets the count and the span. The miss-based cool-down works in both modes. Three
callbacks are a short enough run to ignore one bad callback and a long one for a plugin that is simply too slow
(30 ms of extra spin before it stops); 100 callbacks keep the retry cost near 3 % of the fall-back, and the doubling
makes a plugin that never fits retry only a few times a minute. In a chain of plugins that fit alone but not
together, only the plugin whose wait runs into the deadline is exhausted: it falls back, the ones before it keep
the budget.

The hang guard is unchanged and still counts batches without progress, not time: the one wait that can last
several periods is that of the batch that submitted, and the other batches of the callback find the request still
out and are late at once, so a silent child is declared hung after the same number of batches, which the audio
callbacks deliver at the device's pace, as before (a test runs a silent child with and without a published budget
and compares the counts).

**Rejected.** Passing the deadline through the session, the engine and the manager to each instance (a dozen
signatures, and every stream restart would have to hand it over again); waiting for the whole buffer (the plugins
would leave nothing for the rest of the callback); a longer period for everything (a plugin that is truly late would
hold the callback longer, and the first batch of a callback could not use the slack of the others); pipelining by one
batch (it adds 64 frames of latency that the in-process path would have to copy to keep bounces identical); sharing
one wait between all plugins of a level (not in this change).

**Open.** The 70 %, the four misses and the 24 batches are measured on the test plugin only (`perf/BASELINE.md`),
not on real plugins; the hand check is a bursty third-party effect at a 480-frame buffer, counting the late
sandbox batches in the log before and after.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AV. A panic of the interface is contained and the app keeps running, with limits

**Decision.** The windowed event loop catches a panic in a window event, in the per-tick work and in a window's
redraw. It asks for a recovery snapshot of every open song, drops the gestures and menus in flight, repaints,
and shows a notice strip (Save, Dismiss) that stays until dismissed. The crash report is written as before. The
notice is raised once and not again within 30 s. After five panicking ticks or redraws in a row the content of
the window is paused (the notice alone is drawn, Save and Ctrl+S still work) rather than painting into a loop;
the pause lasts until the app is restarted. Exit is bounded the same way: the wait for the recovery writer and
the removal of the scratch folder give up after 5 s.

**Why.** The release profile unwinds so that faults can be contained, and the engine, the plugin calls and the
document's apply, undo and redo already are. The rest of the UI thread was the one place where a bug cost the
whole session, including everything since the last snapshot. Mainstream DAWs keep running after a contained
fault in a view and tell the user to save; the notice says the same and recommends restarting, because the
state after a panic may be inconsistent in ways the app cannot check.

**Limits.** The repair is the minimum that is safe: gestures, menus and the merge group are dropped; view state
beyond that is not rebuilt. The document is not touched (a command is planned before it changes anything). A
panic that keeps coming pauses the content instead of recovering. Only the windowed loop is guarded: the
headless shell, the QA server and the tests still see panics, so a test that expects one is unaffected. A panic
inside a plugin's own process or the engine keeps its own containment. A scripted run that contained a panic ends with a non-zero exit code. The autosave period is unchanged.

**Rejected.** Rebuilding the whole shell after a panic (loses more than it saves and could panic again);
catching inside every handler (hides the failing site and costs a guard per call); exiting after the first panic
with a recovery copy (what happened before: the work since the snapshot was lost).

**Open.** The five-in-a-row limit and the 30 s quiet period are judgement values; the hand check is a forced
panic in a long session (`ATOMIK_UI_PANIC`).

**Status.** Default (not yet confirmed). Decided by: none yet.


## AW. A plugin state that is too large is reported and never dropped silently, and never stops the plugin

**Decision.** A state the document refuses (over 16 MiB for a device, 128 MiB for the project) or that a
plugin in its own process cannot hand over (over the 64 MiB of one frame, or a reply over the 1 MiB JSON limit)
is not offered to the document again at every capture. The device carries a marker until a capture fits: its
panel shows `too large: <size> > <limit>` on the second line (the vendor line gives way), the plugin keeps its
status word, a save names the device once in its notice, and the log has one warning line per change of
situation. The project keeps the last state that fitted. The child answers `StateTooLarge` (with the size and the
limit) instead of ending, and the app never stops the plugin for it.

**Why.** The plugin is healthy and playing: calling that FAILED or CRASHED would be false and a reload would
restore the old state and repeat. Dropping the state silently makes a project that "saved fine" lose its
samples; every mainstream host that has a size limit tells the user at save time instead. A marker that stays
on the device is the part that is still true after the toast is gone.

**Limits.** Short term only: the project format is unchanged, so a state over the caps is still not stored.
Storing large states beside the audio assets and streaming them over the pipe in pieces is the real fix and is
not done. The panel line is short (the panel is narrow); the full words are in the notice and the log.

**Rejected.** A new status word (hides the real state); raising the caps (the document is one JSON text and every
capture copies the state); stopping the plugin at the limit (what happened before).

**Open.** A real sampler's state sizes against the two caps, by hand.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AX. A plugin fails after a run of bad blocks, not at the first one

**Decision.** A block in which a plugin's `process` returns an error, or writes a non-finite sample, is a bad
block: it is counted and replaced by what a late batch gets (the dry input for an effect, silence for the
rest), with a linear ramp over the block going into the fallback and over the first good block coming back, so
that the output does not click. The device fails, with the reason of the last bad block, only after
`bad_run_limit` (a quarter of a second of 64-frame blocks) in a row; a good block starts the count again. A
panic fails it at once; a late or refused batch is not a bad block and does not change the count; a plugin that
cannot start processing still fails at once.

**Why.** Plugins return an error or a NaN for a block while they load a preset, and on a denormal or a division
by zero, and are fine on the next one; hosts that stop the plugin for good make a glitch cost the user a RELOAD
and the take. The rule looks only at what the plugin returned, so a bounce, live playback, a plugin in the app
and one in its own process agree and the sandbox on and off render the same bytes; a project with no bad block is
untouched, because the ramp only exists around a bad block.

**Limits.** The ramp is one block (1.3 ms at 48 kHz), not the bypass crossfade's 5 ms: a ramp across blocks would
need state that outlives the block and the next block may be bad again. Aux outputs are cut. A late batch keeps its
hard cut (existing projects render the same). Finite garbage (1e20) is not detected.

**Rejected.** Failing after a fixed number of bad blocks whatever the rate (a quarter of a second is the same
audio at every rate); the 5 ms ramp (a bad block only reaches the fallback at its end); muting the plugin for a
while after a bad block (the plugin would miss the notes and parameters that follow).

**Open.** The quarter of a second is a judgement value; the hand check is a third-party plugin that glitches while
a preset loads.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AY. The free-space warning before a take is a floor of ten minutes of audio, twice over, and only a warning

**Decision.** When an audio take starts, the app asks the system how much room is left on the volume of the
project's `Audio` folder and shows a notice when it is less than ten minutes of the take's own audio, twice over
(`record_space::FREE_SPACE_MINUTES`; 460 MB for one stereo track at 48 kHz). The take starts anyway. The twice is
the stored copy: when a take ends, each lap is written again under its content hash before the in-progress file is
deleted. A take whose writer cannot write stops at once, keeps its files for the next start to recover, and says why;
the transport keeps running.

**Why.** Ten minutes is a song-length take and is a small part of any disk the app is installed on, so the warning
is rare and means something; a longer take on a nearly full disk is exactly the case the notice is for. A refusal
would be wrong because the number is an estimate (quotas, other programs writing, a volume that cannot say), and
because a short take on a small disk is a normal thing to do.

**Limits.** The check is made when the take starts, not when a track is armed, because that is where the folder is
chosen. A disk that fills up during a take is caught by the writer's error, not by the estimate. The system call
comes from the `fs4` crate (see [DEPENDENCIES.md](DEPENDENCIES.md)).

**Rejected.** Refusing to record below the floor; checking on every arm (the folder is not chosen yet, and the
notice would repeat); a floor in bytes whatever the sample rate and track count.

**Status.** Default (not yet confirmed). Decided by: none yet.

## AZ. A plugin's request for a restart is acted on only when it changes something, and a loop is cut off by counting polls

**Decision.** When a plugin asks the host to restart it, the host restarts it only if something differs. A latency
request restarts when the latency the plugin reports now is not the one its running activation was made with. A
param-table rescan restarts when what the host caches differs (the ids and their order, the ranges, the step
counts, the ports); when only titles or groups changed, nothing restarts and the app shows the new titles
(`ManagerEvent::ParamTitlesChanged`). An explicit restart request is honoured. What a plugin asks for while the host
is activating it is what that activation did and is dropped: the latency is read once after the activation. A
plugin that still asks again and again is honoured `RESTART_BURST` = 4 times in a row; the fifth request and the
ones after are ignored, one warning goes to the plugin's log, and the plugin keeps running in its last working
activation. The burst ends after `RESTART_QUIET_POLLS` = 200 manager polls in which the plugin was running and
did not ask (polls during a restart do not count, so a slow restart cannot end its own burst), and
the next request is honoured again. The comparison is made where the plugin lives: in the format backend's `poll`, which runs in the
plugin's own process when the plugin is sandboxed, so a plugin in this process and a sandboxed one agree and only a
request that matters crosses to the manager.

**Why.** Plugins often announce a latency change from inside their own activation, and a plugin that announces
unconditionally made every activation start the next one, for ever: dropouts, CPU, a stream of interface changes.
Mainstream hosts restart a plugin for the change it announced, not for the announcement. Four in a row is more than a
patch change or a sign-in needs (a latency change, then a layout change, then one more) and few enough that a loop
costs a fraction of a second. The window is counted in polls, never in time, because an offline render polls once
after every batch: a bounce does the same on every run, with the plugin sandbox on or off (200 polls are a few
seconds at the interface rate and about two seconds of audio in a bounce, far longer than a restart takes).

**Limits.** A plugin that asks for a restart once every few seconds for ever is honoured every time; that is a
plugin that restarts on purpose. A latency that changes by itself while the plugin is parked (deactivated) is read at
the next activation. A request ignored by the rule is lost, not queued: the plugin is not restarted later for it.

**Rejected.** Comparing in the manager only (a sandboxed plugin would need a call into its process for every
request, on the interface thread); a time window (a bounce would depend on the machine).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BA. The interface runs at full frame rate only while something moves, and a minimized window does almost nothing

**Decision.** The windowed event loop has three wake rates. At the full frame period (60 Hz) it runs while something
moves or is polled at that rate: the transport plays or records, a take, a deferred start or a momentary param
pulse is in flight, a live input analysis, a sampling or the camera runs, a control surface is open, MIDI was heard
in the last second, a worker or an import is busy, a menu or tooltip waits for the pointer, the rack scrolls, a
glide or a morph runs, a plugin loads or is captured, a plugin editor window is open, or a pointer button is held.
It also stays at the full rate for one second (`PAINT_TAIL`) after the last frame that painted something,
except a frame that only the status poll asked for and a frame painted again because it was not shown. With an
audio stream open (or a plugin loaded) and none of that, it wakes every 100 ms (`IDLE_STREAM_WAKE`). While every
window has no area and nothing animates it wakes every 250 ms (`MINIMIZED_WAKE`) and paints nothing; with something
animating it keeps the frame period and paints nothing, because the timed work of the frame (a snapshot recalled at
a marker, a glide, a momentary pulse, a preview note) runs there; a resize to no area is dropped
before the graphics target is reallocated and before the shell lays out. A project with no stream and no plugin
still sleeps until an event, and the tail applies only where a slow wake exists anyway. Live MIDI wakes the loop at
once (an event-loop proxy called from the MIDI thread when a message is queued), and the exact deadlines (quit time,
screenshot, submenu switch, graphics back-off) win over every rate.

The DSP figures of the status line are held steady for ten seconds while they move by less than two points
(`status::DspHold`): an idle engine's load jitters by a fraction of a point, which flipped the rounded figure and
repainted the whole window every couple of seconds for nothing. A real change shows at once.

**Why.** An open stream used to mean 60 wakes a second, each running the whole housekeeping frame, for a stopped
transport and an empty rack: a measurable share of a core, no deep CPU sleep, and the same while minimized. Mainstream
hosts drop their interface to a few updates a second when idle or hidden and go back to full rate on input or
playback. The second of tail is long enough for a meter's ballistics and a light to settle and for an animation that
repaints every few frames to keep itself going, so the tail covers what the list of causes does not name; the first
paint of anything that changes starts it. Painting nothing while minimized is the cheapest saving of all.

**Limits.** Work that is polled on the frame but is not in the list (engine garbage, the plugin event pumps of idle
plugins, the status poll, the recovery writer, the plugin cache writer, an idle plugin's notifications, the templates
folder, autosave timers) runs at most 100 ms late while idle, 250 ms while minimized and idle. A change that happens with
nothing awake is seen at the next slow wake, up to 100 ms later, and then runs at the full rate. A control surface
or an open plugin editor keeps the full rate for as long as it is open. A small move of the DSP figures in the status
line shows up to ten seconds late.

**Rejected.** Waking on every cause with a proxy (the causes live on many threads and in many places; the list plus
the paint tail is smaller and cannot miss a visible change for longer than the slow period); a longer idle period
(a mapped control or a worker's result would wait visibly); throttling by painting less often at the full rate (the
cost is the frame, not only the paint).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BB. Captured input waits for three buffers of the configured size, plus an allowance that grows on lumps and shrinks when the room is unused

**Decision.** The render callback keeps at most `3 * B + extra` frames of captured input in the ring, where `B` is the
buffer the stream was opened with or the largest callback of the last two windows, if larger (a window is 2000
callbacks), and `extra` starts at zero. `extra` grows by half a buffer (at least 16 frames) only when a counted trim
follows a lump: input that arrived in one callback exceeding a buffer by at least half a buffer and at least 32
frames. It lies between zero and two buffers, clamped to between 10 ms and 30 ms of audio. A trim after a steady
arrival is clock drift: it is counted as lost but changes nothing. At the end of a window `extra` shrinks by one step
only if no lump was trimmed in the window and the largest backlog the window saw (before its trim) would still have
fitted under the smaller threshold. The allowed backlog beyond one callback (`2 * B + extra`) is published as
`input_backlog_allowed_frames` (with `input_backlog_extra_frames`); Export Diagnostics and the debug trim line always
show it, the status line only while `extra` is above zero. Examples at 48 kHz, frames held including the read: a
buffer of 64 holds 192 (4 ms), at most 672 (14 ms); 256: 768 (16 ms), at most 1280 (27 ms); 512: 1536 (32 ms), at most
2560 (53 ms); 4096: 12288 (256 ms), at most 13728 (286 ms).

**Why.** The old rule measured three buffers of the callback that was running. Shared-mode drivers deliver callbacks
of uneven size, so after a short callback a burst of input looked like a backlog and was dropped: audio lost from takes
and monitoring with no clock drift at all. The configured buffer is what the interface promises on average; the extra
absorbs a driver that delivers packets (10 ms against a 64-frame buffer, say) and the cap keeps the latency bounded.
Growing only on lumps keeps drift between two devices from buying latency that cannot stop the loss; shrinking only
when the room was not used stops a steady packet driver from losing a packet every window; forgetting the largest
callback after a window keeps one freak callback from tripling the threshold for a session. Takes are placed with the
measured input latency, which already includes the frames the ring holds, and the allowance changes only what is
discarded, so a take does not move. All integers on the callback, counted in callbacks, so a bounce and the audio path
are untouched.

**Limits.** Two interfaces whose clocks drift still lose a frame now and then (counted); there is no resampling. A
packet driver loses the first packets while the allowance grows (a handful, one step each). A steady backlog left over
from a burst is room in use, so the allowance stays until the ring drains. An outlier callback is followed at once and
forgotten within two windows (about ten seconds at a 64-frame buffer).

**Rejected.** A fixed larger threshold (more latency for everyone); a threshold from the running average of callback
sizes (still tracks short callbacks); growing on every counted trim (drift pushes it to the ceiling for good); shrinking
by wall time (the audio thread has no clock it may read for this).

**Status.** Default (not yet confirmed). Decided by: none yet.


## BC. A plugin rests when its input was silent for its tail and its own output for a second

**Decision.** One rule for every plugin format and kind, decided per 64-frame block from batch content and from what the
plugin returned, never from a clock. A plugin is not called in a block when nothing wakes it (an event of any kind, a
param change included; a request of the plugin; a non-silent main or sidechain input; a change of the play state) and
all of this holds: its input has been silent, with no event, for at least the tail it reports; its own output (main,
aux, and the notes it emits) has been at or below -120 dB for the idle time of one second, counted in calls
(`IDLE_SECONDS`, 750 blocks at 48 kHz); no note is open; and either the format leaves the choice to the host and the
tail is not infinite (VST3), or the plugin last answered "continue if not quiet" (CLAP; what its tail extension says
does not matter then). A VST3 plugin that declares an infinite tail is never skipped by the host. A CLAP plugin that
answers "sleep" rests until woken, with no other condition; one that answers "tail" or "continue" is called every block.
A plugin that is new, restarted, reset or just un-parked is observed for the idle second before it can rest. A VST3
instrument that rests is also told to stop processing and is started before the block that wakes it, as before.

**Why.** The old rule skipped a VST3 plugin only for a tail of exactly zero, so a reverb that reports a tail ran for
ever on silence, and a plugin that reports none while it rings (a common framework default) was cut at the first silent
input block. The CLAP path rested only on "sleep", so plugins that answer the other quiet statuses never did. Requiring
the output to have been silent for a second is what makes a skipped call inaudible: a skipped plugin makes silence,
because it had been making silence. An infinite tail is the plugin saying it may sound with no input at any time (a
feedback delay set to three seconds is silent for three seconds and then echoes), and silence so far says nothing about
when; before this rule such a plugin was always called, and it still is. A CLAP plugin says for
itself, per block. Everything the rule looks at is in the batches or in the plugin's answers, so a live run, a bounce and a plugin in its own process agree, and
nothing on the audio path allocates, locks or reads a clock.

**Limits.** A plugin whose output never goes quiet (a constant offset, noise, a dither) never rests, which is the safe
side. A plugin that makes sound from the transport alone after a second of silence, with no event, is not woken
mid-play (a change of the play state wakes it), as already for instruments; a VST3 plugin that declares a finite tail
and then sounds on its own is cut, the declaration being its word. Any event, a param event of automation
included, restarts the idle second, so a plugin under continuous automation is always called. After a wake the plugin's
modulation phase and similar state are where the last call left them, not where silent calls would have taken them.
In a plugin's own process the tail it reports reaches the host one block late, which only matters if a plugin changes
its tail while it runs.

**Rejected.** Resting on the reported tail alone (cuts plugins that under-report); a fixed idle time with no output test
(cuts any ring longer than it); resting an infinite tail on output silence (loses a late echo); trusting "continue" and
"tail" to mean the plugin may rest (they mean the opposite in the CLAP specification); a wall-clock idle time (a bounce
would differ from playback).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BD. A bypassed effect is parked after ten seconds of bypass, and comes back cleared and pre-rolled

**Decision.** An effect (not an instrument or a note effect) is called as ever while its bypass is on, so comparing with
and without it (on, off, on, a few seconds apart) is seamless. When the bypass has been on without a break for
`PARK_AFTER_SECONDS` (10 s, 7500 blocks at 48 kHz, counted from the end of the 5 ms crossfade to the dry signal) the effect
is parked like a powered-off device: no `process` call, the dry ring and its delay compensation keep running, the end of
an open note is still delivered, new notes are held back. Pending param changes go to the plugin in one call, with silent
input and discarded output, at most every `PARK_FLUSH_SECONDS` (50 ms); the plugin is still parked afterwards and the call
counts for nothing else. The same call serves a powered-off device. The state shows as STOPPED. When the bypass is
switched off after a park the plugin is reset (`reset`), then called for the pre-roll with its output held at the dry
signal, then faded in over the usual 5 ms. The pre-roll is `PREROLL_SECONDS` (20 ms: 15 blocks at 48 kHz) or the plugin's
latency in frames if that is longer, so that a plugin with look-ahead has produced real output by the time its first wet
frame is used. The rule is the same live and in a bounce: it counts blocks.

**Why.** A user who bypasses a heavy plugin to save CPU saved nothing: the plugin was processed for ever after the fade.
But parking at once would clear the plugin on every return of a comparison (a compressor with no gain reduction, a reverb
with no tail, 20 ms late), so the plugin keeps running until the bypass has clearly become a decision. A parked plugin has
not heard the input for as long as it was bypassed, so without a reset a reverb would bring back the tail it had, and
without a pre-roll its filters and followers would start from stale state under the fade. A value set meanwhile must be in
the plugin when its state is saved; waiting for the next real block left it out of a project saved meanwhile, so it is
sent at once, rate-limited so that automation on a parked plugin costs one call in 38 blocks and not one per block.
A bounce and live playback must agree, so nothing depends on the run mode; a bounce whose effect is bypassed throughout,
or never, is what it was (the dry signal, or no change); only a bypass switched off after more than ten seconds
differs from the version before, and there it equals the live result.

**Limits.** After a park the plugin restarts from a cleared state (only audible for a plugin whose output depends on its
history). A plugin that dislikes `reset` gets one per return from a park. Instruments and note effects keep what bypass
meant for them (silence or no notes, still called). A value set in the plugin's own window reaches the plugin directly and
is not affected.

**Rejected.** Parking as soon as the fade ends (every comparison clears the plugin); pre-rolling for the plugin's whole
tail (unbounded cost on the audio thread); no reset (stale tails); parking live only (an export would differ from what was
heard); keeping parameter changes pending until the next real block (missing from a project saved meanwhile); running the
pre-roll with the output faded in (a clicking, half-formed wet signal).

**Status.** Default (not yet confirmed). Decided by: none yet.


## BE. A plugin file is scanned class by class, and the scan's time limit is per class

**Decision.** The scanner child prints one line when it knows the classes of a file (their ids and names, nothing created
yet) and one more as each class is finished, then the result line. The parent measures the time limit per class: the clock
starts with the child and starts again at every such line, so the limit of the plugin settings (30 s by default) is what one
class may take, not what a whole file may take. When the child hangs or crashes inside a class, the classes that answered
are kept, the first class without an answer is listed in the cache as unsupported with the reason, and the scanner is
started again for the classes after it with that class and the answered ones left out (`MAX_CLASS_RESTARTS`, 8, at most).
The file as a whole is blacklisted, as before, only when no class answered; two classes that fail before any answers also
end the attempt, because a file in which everything fails is the file's fault. Everything a scanner child starts lives in a
group that ends with it (a job object that kills on close on Windows, a process group elsewhere), so a timeout, a crash, a
cancel and a finished scan all leave no helper process; the scanner runs below normal priority and with the system error
dialogs off, so a crash is a crash at once. A machine with four logical CPUs or fewer runs one scanner at a time.
A group is signalled at most once, and on Unix only before its leader is reaped: a process group id is held only while the
group has a member or its leader is a zombie, so a later signal could kill an unrelated process that was given the number.
Every path therefore looks at the leader without reaping it (`waitid` with `WNOWAIT`), signals the group, then waits for the
child; a group dropped unsignalled is signalled in its drop, still before the child is waited for.

**Why.** A shell file with a hundred classes was judged by one limit for all of them and blacklisted whole when one was
slow, losing the ninety-nine that were fine. A limit per class is what a user sets the number for: how long one plugin may
take to wake up. Counting only the scanner's own lines, not whatever a plugin prints, keeps a plugin that talks while it is
stuck from keeping itself alive. The cache keeps no new field: a class that did not answer is an ordinary entry that is not
supported, with the reason, so an old cache reads as before and the scanner version did not change (the entries of plugins
that behave are the same bytes).

**Limits.** A class that times out costs a full limit, then a restart of the file's load; a file of many bad classes costs
up to nine limits before it is given up on. The retry with no time limit (NO TIME LIMIT in the plugin manager, NO LIMIT in the
plugin settings) runs one file alone with no limit, and is offered for a file that is blacklisted for a timeout, not for
one class listed as unsupported in a file that mostly worked; for that file a full rescan is the way to try again.

**Rejected.** One limit that grows with the number of classes (a file that is slow to load is not slow per class, and the
number is only known once the file is loaded); a limit per file with no restart (the old behaviour); keeping the answered
classes in a separate cache section (a format change for what a flag on an entry says).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BF. A plugin that crashed after a while is reloaded once by itself, and its death is noticed from its process

**Decision.** The app learns that a plugin process died from the process itself, not only from the end of its output pipe:
a watcher thread of the link waits on the child (a wait on the process handle on Windows, `waitid` with `WNOWAIT` on Unix,
which leaves the exit status to be collected as before; a group child has one watcher for all its members). A plugin that
started a helper process which inherited the pipes used to look alive until the hang guard or the helper ended; it is now
CRASHED when it dies. The pipe and the watcher lead to the same code, which does its work once, so there is one event and
one log line per death; a child the app asked to end is never reported. A bounce keeps its deterministic waits and the same
reporting as before.

A crashed plugin (not one the hang guard stopped, which has its own two reloads) is reloaded once, by itself, from the
document's last captured state, through the same path and load queue as RELOAD, when it had run for at least ten seconds
from the moment its instance was ready (`CRASH_RELOAD_MIN_ALIVE_SECS`): a plugin that dies while it loads would only die
again. The device says so in the plugin log and in a notice, once per crash. A second crash within five minutes of that
reload (`CRASH_RELOAD_WINDOW_SECS`) stays CRASHED until RELOAD, which gives the allowance back; so does a crash during an
export or a bounce, and a crash that quarantined the plugin. There is no setting for it: the hang reload has none either.

**Counting.** Every crash is counted toward the quarantine, as before, including the one that is reloaded; the reload counts
nothing and does not reset the count. A plugin that crashes after every reload is therefore quarantined by its third crash,
and a plugin that crashes at every load is not looped, since a crash before ten seconds is not reloaded.

**Why.** Ten seconds separates "died at load" from "died while working"; five minutes is long enough that a plugin that dies
on the same material each time shows the pattern within one attempt, and short enough that a one-off crash earlier in the
session does not use up the allowance of a plugin that has behaved since. Resting on the process rather than on traffic
matters because a parked or sleeping plugin sends nothing.

**Limits.** A helper process that outlives its plugin and holds the pipes keeps one blocked reader thread and the pipe
handles until the helper ends; dropping the dead instance does not wait for it (only threads that have ended are joined).
The app does not end a plugin's helper itself.

**Rejected.** Polling the process (a wake-up per child for nothing); one reload per session (a crash an hour ago would
cost a plugin its allowance); reloading again and again with a back-off (a plugin that crashes on its own state would
loop, and the user would learn of it late); a setting (nobody asked to switch it off, and the hang reload has none).

**Status.** Decided by the owner on 2026-11-21 (one automatic reload, then stay crashed).

## BG. The render workers are used only while the audio thread needs help

**Question.** The engine woke the render workers on every callback of a project that has a level of two or more
devices, and the workers spin through the whole callback. A stopped rack of 100 silent devices, which takes about a
seventh of the callback time on the audio thread, kept 76 to 86 % of a core busy with automatic workers and 30 % with two,
against 18 % with none. What a live engine needs to know is not whether spreading one level saves time but whether the
audio thread needs help to meet its deadline.

**Decision.** Each node keeps a smoothed cost in nanoseconds per batch (its stages added up; an integer written by the
thread that runs it and read by the audio thread between batches, like the CPU readout's total). Before each batch a
live engine (`EngineOptions::realtime`, set by the app's stream and `atomik-play`) sums the costs of the plan and compares
the sum with the batch budget. The plan is **busy** when the sum reaches 30 % of the budget and **calm** after a second
of audio under 20 %. Entering is immediate (the next batch after the smoothed cost crosses 30 %), leaving is slow, and
between the shares nothing changes: a wrong "busy" costs some CPU, a wrong "calm" on a heavy project is a missed deadline.
A calm plan runs every level inline and wakes no worker; a busy one hands every level over that has a device a worker can
take (a device pinned to the audio thread cannot be taken, so a level of pinned devices wakes nobody). Two loads are read:
the smoothed one (the sum of the smoothed per-node costs, set by batches that ran inline and only ever lowered by the
others) and the seen one (the sum of each node's raw time in the last timed batch, wherever it ran: an upper estimate of
the inline load, and the only figure that rises while the plan is busy). Entering uses the smoothed load; while busy,
a seen load at or over 50 % of the budget stops the inline probes and the calm count, because inflation cannot be told
from load without an inline batch and an inline batch at that load is not safe: staying busy is the safe side. The callback flag and
the wake-up at the start of a call follow the state, and a level that is handed over in the middle of a call raises the
flag itself. A plan whose nodes have no cost yet is busy, and so is the first second after the transport starts (the
stopped to playing edge): playback never begins inline and finds the load a batch late. The hold counts audio time.
Offline engines (a bounce, an export, the freeze and analysis workers) have no deadline and want throughput, so they never
use the rule and hand every level over, exactly as before; the choice is `EngineOptions::realtime`, false by default.

**Costs.** While the plan is calm every device is timed on every batch, so a rise is seen at once: the worst delay to
become busy is one batch plus the half-way smoothing (a jump to twice the enter share is seen in the first batch). That
costs two clock reads per device per batch (a few tens of nanoseconds a pair, an estimate that was not measured on its
own: the sampler's stopped-rack rows with the rule on show no difference from none). The app times every device call all the time already (its CPU readout), so the clock is the host's there;
`atomik-play` runs with the device timing off and uses the engine's own clock, on the same batches. A busy plan cannot be
timed where it runs: a node takes longer on a worker than at home (its data has moved to another core), which would read as a load over the leave share and keep a calm plan busy for
ever; the first version of the rule, timed where the nodes run, left the stopped stress rack with seven workers at 87 %
of a core in the sampler. So a
busy plan runs one batch in 64 inline and timed (never in the hold, never while a cost is unknown, and only if the batch
just before it, as timed where it ran, was under 50 % of the budget, so a probe batch is bounded by what was just
measured and not by an old figure; the first version of the rule bounded it by the last inline load, which is stale by
construction once the plan is busy, and ran one whole batch on the audio thread, a second after Play, for a project that
is light when stopped and takes 150 % of a batch when playing), and the batches it runs on workers can only lower a
smoothed cost, never raise it, so a plan that became light is seen without waiting for a probe. The sum counts each
device once, at its last stage, so a multi-stage device whose stages differ is read as their total.

**A plan that needed help while playing keeps the workers while it plays.** A song with a quiet section of more than a
second under 20 %, followed by a dense one, would go calm in the quiet part and take the dense onset inline: the first
batch of the onset on the audio thread alone, late at small buffers, at every such transition. So once the plan has
shown a need while the transport plays (the smoothed load at or over 30 %, or the seen load at or over 50 %, for 250 ms
of audio in a row: one noisy batch would otherwise make a light project heavy for good) the gate remembers it for the
life of the pool, and while the transport plays it stays busy: it never leaves and never probes. Stopped, it leaves as any
plan does (a second under 20 %); the next Play holds and then stays busy. A project that never showed a need while playing
plays inline. The memory is not cleared on a plan swap, only when the engine is rebuilt: a project made light by removing
devices keeps the workers while it plays until the engine is rebuilt. The workers' CPU in the quiet part of such a song is
small, since they spin only for the length of a callback, and the measured gain (the stopped rack, the light project) is
untouched.

**Paths that carry timing.** Playback start: the hold above, from the first playing batch. Live notes on a stopped
transport: no hold, but the plan is calm and timed on every batch, so voices that raise the cost are seen in the next batch
(at the enter share the audio thread alone carries 30 % of a batch before workers join; the paced benchmark's heaviest
patch carries 12 %). A preview clock (a note player previewing while the song stands still): its devices run and are
timed like any other. Audio streaming lanes and clips: they are part of the devices' cost; the streamer's own thread is not
the render workers. Sandboxed plugin devices: their measured cost includes the wait for their process, which is real time
the audio thread spends, so it counts towards busy (a plugin that waits long is a reason to have the other devices run
beside it). A device pinned to the audio thread counts in the load but is never handed over. A plan swap in the middle of
playback: the state lives in the pool, not in the plan; the devices that were in the arena keep their costs, a new device
has none and makes the plan busy for the batches until it is timed. MIDI out and the metronome do not go through the nodes.
Nothing the rule reads reaches the signal: a node's output does not depend on its thread, and a bounce is byte-identical
whichever rule decides (the tests render the same project under the live rule, the offline rule, forced inline and forced
dispatch, also while the busy state flips).

**Evidence.** `perf/BASELINE.md`, "Render workers used only while the audio thread needs them": the stopped stress rack falls
from 76 to 86 % to 16 to 19 % with automatic workers and from 30 to 34 % to 17 to 19 % with two, equal to the figure with no
workers; the empty project is unchanged; the heaviest patch of the paced benchmark, inline, has a p99 under a fifth of the
callback period. A rule that judged each level by the time spreading it would save (tried first, 7 us to dispatch) tied
with dispatching everything on a bounce (rack 0.73 against 0.71 of the workers 0 time, console-64 0.61 against 0.61,
lanes-200 1.04 against 0.96, lanes-2000 0.88 against 0.92) and could not lower the idle cost of the live rack, because as
long as one level goes to the workers they spin through the whole callback; it was removed.

**Rejected.** A per-level saved-time rule (above); timing every call on every batch in a bounce (nothing there needs the
costs); sampling one batch in 32 while calm (a rise could go unseen for 43 ms; the delay is the price of saving 5 us a
batch); a long dwell to enter (an inline plan that turns heavy would run on the audio thread alone for a long time);
measuring a busy plan where it runs (the feedback above); skipping silent devices (a later item that builds on the
per-node cost).

**Limits.** The shares are fixed numbers, not settings. A project that sits between 20 and 30 % of the budget keeps the
state it has. A plan whose seen load is at or over 50 % of the budget is never probed inline and leaves busy only when its
devices get lighter (their runs on workers lower the costs). A plan that has never been busy while playing and jumps from
under the enter share to over a whole batch within one batch runs that one batch inline, playing or stopped (a large chord
on a stopped transport, a dense onset in a song that has not needed help before): the rule bounds what an inline plan can
run up between two batches by the enter share and the growth of one batch, not that growth. The load is that of one engine: two engines in one process share the
device timing switch but each has its own rule state.

**Status.** Default (not yet confirmed). Decided by: none yet.

## BH. The windows that present without waiting share one frame slot and are painted at most once in it

**Decision.** With two or more windows open every surface presents without waiting for the vertical blank, and the
loop paints them in turn on one thread. Time is cut into slots of the frame period, 60 Hz (`App::frame`, a field set by the Performance
level, 30 Hz at Low power: see decision BT; `App::frame_started` is where the current slot began), and a
window is painted at most once per slot. The rule is a pure function (`window_logic::paint_gate`) of how the window
presents, the time since the slot began, whether the window was already painted in it, and the reason for the paint. A
window with vsync is never held back. A paint of a window not yet painted in the slot is made at once, in that slot
(the first paint of a window too). A window already painted in it waits for the slot to end, and the paint made after
the end opens the next slot. Resize or scale change, focus change or uncover, a frame painted but not shown (the
retry), a redraw the system asks for while nothing is marked (an exposure) and the screenshot paths are painted at
once and do not move the slot: they only mark the window as painted in it, so a resize does not push the animation
phase around. A tick judges every window at its one time, not at the moment after another window's render. A held-back
paint changes nothing in the shell: the window stays marked, the reason is kept for the counters, and the loop's wake
deadline becomes the earlier of its own and the time the slot ends (`window_logic::loop_deadline`), also when no
stream or plugin would wake it. A paint that arrives by a timer counts as due when it is up to an eighth of the period
(about 2 ms) early; input counts as due only at the full period.

**Why.** A sweep of the pointer, a drag or a scroll delivers an event batch every millisecond or two, each marks the
window and each cost a full scene build and render, many times what any display shows. The tick already sets the
pace of animations; this puts the same bound on events. The vsync window is paced by its present call, and a timer
that beats against the blank would show every other frame. Resizes, exposures and retries are things the system or the
last frame is waiting for: a stale or stretched window during a live resize, or a frame that stays unshown, is worse
than one more paint. The tolerance exists because the tick's timer rounds to milliseconds: a tick that arrives a hair
early would be held and the cadence would slip. It is an eighth of the period because the cadence stays within 8 % of
60 Hz; input gets none so that a sweep never paints more often than the period allows.

**Why one slot for all windows.** The first form gave each window its own phase (gated on its own last paint). A paint
off the tick (an exposure, a focus change, a resize) then left two animating windows half a period apart for good: one
window was due at the tick and the other half a period later, and each wake-up runs the whole of the loop's tick, so
the loop woke twice per frame (120 a second instead of 60, in a model of the loop on the pure functions) and the change
that was meant to save CPU doubled the tick rate in the most common multi-window case. With one slot the windows are
painted by the same wake-up, a window painted off the tick only marks itself as painted in the slot, and the loop is
back to one wake-up per period within two periods of any such paint.

**Why the period is the app's and not the monitor's.** The windows can be on monitors of different rates and the app
paints them from one loop, so there is no single rate to read; the vsync window already follows its monitor. A later
low-power mode will lengthen the period, which is why it is a value the app holds and not a constant.

**Limits.** The period is 60 Hz even on a faster monitor, so an unpaced window on a 144 Hz screen shows at most 60
updates a second (the tick it replaces never did better). A held-back paint is delayed by at most one slot. The
counter of held-back paints counts each time a paint is asked for too early, not each distinct paint.

**Rejected.** A phase per window (above); pacing the vsync window as well (the timer beats against the blank and halves the rate); holding back
resizes (stretched content during a live resize); reading the monitor's rate (several monitors, one loop); a
tolerance for input too (a sweep could paint more often than the period allows).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BI. A sandboxed plugin's start, stop and reset are never waited for on the audio thread

**Question.** For a plugin in its own process, `start_processing`, `stop_processing` and `reset` were requests of
their own, and the audio thread spun for the answer for up to 50 ms (at least `LIVE_OP_DEADLINE`). That is a dropout
by construction (50 ms is 15 to 40 callbacks), and a start that took longer than the wait was a permanent failure of
the device ("the plugin could not start processing" until RELOAD), where the same plugin in this process just starts,
however long it takes. Since the sleep rule an instrument that goes quiet is told to stop and is started again by the
batch that wakes it, so a start happens on the audio thread whenever a sleeping instrument gets a note, and a reset
after every locate.

**Decision.** Live, the audio thread spends on a start, a stop or a reset no more than it spends on any batch, and a
slow start is late, not failed. The start and the reset ride on the batch that needs them: the request carries "stop
first", "start first" and "reset first" (bits above the op code in the request's op word), the child performs them in
that order inside the one request and then processes the batch, and the answer says whether the start succeeded
(`ProcessStatus::StartFailed`, no audio). The audio thread waits for the batch with the batch deadline, the cool-down
and the hang guard it already has. A stop is sent at once and not waited for; when a request is in flight it is owed and
rides on the next batch, ahead of its start. A start that was owed and not sent is dropped by a stop (the child never
heard of it). A refusal that arrives after a late batch is read before the next request overwrites the answer. The
same rule holds offline, where the batch is waited for up to the watchdog: there is no second, synchronous path, and
the child sees the same calls in the same order, so a bounce is byte-identical with the sandbox on and off.

**Why.** The batch already has everything a start needs: a deadline, a way to be late, a counter, a recovery when the
answer arrives, and a hang detection. A separate request would need its own state machine for "start pending", "start
late" and "events that arrived meanwhile". With the start on the batch no event is lost (the notes of the batch travel
with the request and are processed after the start, at their offsets), and the cost of the start shows as what it is:
time in that batch. In-process plugins keep calling the trait methods as before; the remote processor only records
what is owed.

**What a slow start costs now.** A start of 100 ms is one full wait (one batch period, or the callback's published
budget, at most 100 ms) that runs out, then the batches behind it are polled and late at once for the rest of the 100 ms,
counted in the `late` meter, dry for an effect and silent for an instrument; the plugin then plays. One miss is far from
the four that start a cool-down and an answered batch forgives it, so many slow starts in a row never look like a
broken plugin. A start slower than the hang limit (three seconds of batches with nothing completed) is a hung child
and is ended and reloaded by the existing rule.

**Limits.** The device believes the plugin started as soon as it asked, so a refusal fails it one batch later than
before (the batch that carried the start, or the first one after it when that one was late). A reset that is owed when
the plugin is stopped is not sent on its own: a stop supersedes it. A stop that cannot be sent because a request is in
flight and is followed by no start is left to the child, which stops its plugin itself when it gives it back.

**Rejected.** Submitting the start alone, playing dry or silence meanwhile, polling once per batch and delivering later
every event that arrived (a second state machine, and a note-on and its note-off could be split by it); keeping the
synchronous wait with a longer deadline (a longer dropout); a separate offline path (it would let a bounce and live
play follow different rules).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BJ. Only a parameter change or a log line can be lost on the way from a plugin to the app

**Question.** Everything a plugin or its device told the app went through one ring of 256 messages per instance, and
`post` dropped a message when the ring was full or another thread was posting at that instant. The VST3 processor
posted a change for every output parameter queue in every block. A stalled consumer plus a metering plugin filled the
ring, and two threads posting at once lost one message even with an empty ring. What was lost included messages that
must arrive: a request to save, a new latency, the end of a gesture (an undo gesture stays open), a failure, the news
that the plugin's process died (the single automatic reload depends on it).

**Decision.** Three roads, one order. (1) The five messages that carry no data (`StateDirty`, `LatencyChanged`,
`RescanParams`, `RequestRestart`, `RequestCallback`) are bits in an atomic: posting sets the bit from any thread and is
always accepted; the receiver takes them all at once and hands them out first, in a fixed order. (2) A thread that is
not inside `process` posts to a queue under a mutex it waits for, so two such threads never lose a message to each
other; a change that follows a change of the same parameter, with nothing posted since, replaces it (a drag in a
window is one message and its end still carries the last value); from 1 024 waiting messages a change and a log line
are dropped and counted, every other message is queued, and 65 536 is a cap against a plugin that never stops (only
a runaway stream of window-move or callback messages with a consumer gone for good could reach it). (3) A thread
inside `process` keeps the wait-free ring and its `try_lock`, with one rule added: a change is pushed only while more
than 64 slots are free, so a flood of changes cannot take the room of a begin or an end, a log line or a failure. Each
message of the ring and the queue is stamped from one counter and the receiver hands out the older head, locking the
queue before it looks at the ring so that a message posted later on the other road is never handed out first. The
device keeps a failure the ring refused in a plain field and says it again on the next blocks.

**Why a change may still be lost.** A change outside a gesture only feeds MIDI learn and the camera touch, and inside
a gesture the merger keeps the last value, so a later value of the same parameter makes an earlier one worthless. What
is done so that the latest value survives: a queued change is replaced by the next of the same parameter, the VST3
processor keeps the latest value of every parameter it reports and offers it again, also with nothing new written,
until the ring takes it, and the reserve keeps the ring from being full of changes. A change that is refused is
counted in the dropped counter.

**Output parameters of a VST3 plugin.** The processor builds the sorted list of the ids of the layout the app shows
when the plugin is activated, and reports a value written during `process` only for those ids, only when it differs
from the last one reported, and at most once every 15 blocks (about 20 ms at 48 kHz; blocks are counted, a clock is
never read, so a bounce and a live run report at the same blocks). A value the plugin wrote before the user edited the
same parameter in the plugin's window is not reported over the edit. For a parameter the plugin automates out while
the user drags it, the app sees the begin, the edits and the plugin's own values in the order they were posted, and
the gesture's last value is the last one posted.

**Limits.** Flags no longer keep their place among the other messages: the app's restart request is one boolean and
`StateDirty` only stamps a time, and the plugin process forwards each flag as it takes it. Log lines a plugin writes
from its audio thread still use the small ring (they can be shed there).

**Rejected.** A bigger ring (the same loss, later); waiting for the lock on the audio thread; one queue for every
thread (the audio thread would lock); counting each flag (the app does not need it).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BK. A VST3 plugin is brought up and taken down by the book

**Question.** A VST3 plugin whose controller is a class of its own is joined to its component through two proxies that
each hold their destination strongly, and the host disconnected both halves with null before it terminated them. A
plugin written the usual way ignores a null, so the ring kept both halves alive for ever; a component that initialised
and was then found not to be an audio processor was never terminated; a controller that refused to initialise was used
anyway; and the factory never got the host context it may ask for.

**Decision.** `disconnect` is called on each half with the proxy that half was connected with, never with null, and
both proxies are then cut (their destinations cleared) whatever the plugin answered; the order of teardown is
deactivate, disconnect, terminate the controller (when it is a class of its own), terminate the component. A separate
controller whose `initialize` does not return ok fails the plugin with an honest error (the controller is released
without `terminate`, the component is terminated) instead of the plugin going on without a working controller. When the
factory answers the controller class with the component's own object (compared as the `FUnknown` of each) the plugin is
a single object: one `initialize`, no proxies, one `terminate`. A factory that implements `IPluginFactory3` gets
`setHostContext` once, before anything is created from it, and the module keeps the context for as long as it keeps the
factory (the factory first, then the context, then the module's exit function, then the library). Every way out of
`create` after the component initialised terminates it, by a guard that is dropped armed.

**Why `disconnect` gets the proxy.** A plugin lets go of its peer in `disconnect(other)` only when `other` is the object
it stored (the reference hosting code does the same and a plugin may answer `kResultFalse` otherwise). The proxies hold
their destinations strongly, so component -> proxy -> controller -> proxy -> component is a ring that keeps both halves
alive for ever when the plugin ignores a null. Cutting the proxies after the call breaks the ring on the host's side
whatever the plugin does (nothing at all, or an error): the plugin never has to cooperate to be freed. A proxy takes a
destination out under its lock and releases it after the lock, because a release can run plugin code that calls `notify`
on the same proxy.

**Why a refusing controller fails the plugin.** A plugin with no parameters, no controller state and no editor is worse
than an honest error, and the format's reference hosting code fails the plugin too. The error is
`BackendError::Unsupported`: an instantiate (opening a project, adding the plugin) returns it, and a scan lists the
class as unsupported with the same reason, so it is never offered. A controller class the factory cannot create at all
is unchanged: the plugin loads without a controller.

**Rejected.** Disconnecting with null and also with the proxy (a null is meaningless to a strict plugin and harmless to
the others, but it hides a bug); keeping the proxies alive and clearing them at the next instance (a plugin would
outlive its removal); a second proxy per half.

**Status.** Default (not yet confirmed). Decided by: none yet.

## BL. A bounce flushes every sandboxed plugin after every batch, and the flushes are waited for together

**Question.** A bounce polls the plugin manager after every 64-frame batch, and each sandboxed plugin answers a `Flush`
that carries every notification it queued, one plugin after the other. Could the round trip be skipped for a plugin that
has posted nothing, or at least shared?

**Decision.** Every sandboxed plugin is still flushed after every batch, as the sync point that makes a restart, a
latency change or a dirty state land on the same batch in every run. What changes is the waiting: `PluginManager::poll`
first sends the request to every instance (`PluginInstance::begin_poll`) and each instance takes its answer in its own
`poll`, so the plugin processes flush at the same time. A request that another call abandoned in between is simply sent
again by `poll`, as before.

**Why no "nothing to flush" flag.** What a plugin posts from its own main thread is not tied to a batch: it may do so
at any moment, so a flag read on the parent side would race with it and a bounce would depend on timing. The round trip
is the only thing that makes the notifications of a batch visible before the next one.

**Rejected.** Skipping the flush when the plugin reported nothing since the last one; flushing every few batches (a
restart would land on another batch than in a live run).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BM. A VST3 editor is told the size it asked for, whether or not the user may resize it

**Question.** A VST3 editor asks for a size with `resizeView` and the format expects the host to answer by telling the
view with `onSize`. The host only had `resize_gui`, which asks `canResize` first and refuses an editor of a fixed size,
so an editor that changes its own size (a zoom menu, a view that folds open) never heard that its window had followed.

**Decision.** `PluginInstance::plugin_resized_gui` (default: nothing) is called where the window has followed a size the
plugin asked for: in the app when it handles the plugin's resize message (for a plugin of the app's own process), and
in the plugin process right after its window followed (for a sandboxed one; the app's call is then a no-op). The VST3
instance calls `onSize` with that rectangle, without `canResize` and without `checkSizeConstraint` (the plugin chose
the size), only while the view is open, and not again for a size the view was told already (at the opening, by a user
resize or by this call). A resize by the user keeps its `canResize` check.

**Why no `canResize` check.** The question "may the user drag the frame" is not the question "did the window take the
size the plugin chose". An editor that can be resized is told by this call too, a moment before the platform's own
resize event reaches `resize_gui` with the same size: that second `onSize` carries the rectangle the view already
has, as the one it always got from that event did. `resize_gui` is left as it was, so a resize by the user reaches a
plugin exactly as before.

**Why from the poll and not from `resizeView`.** The plugin is inside its own call there, and answering a request with
a call back into the view from the same stack is the way a plugin loops or deadlocks. The message is handled at the next
poll, as before, and the call happens there.

**CLAP.** Unchanged: its host callback only posts the request, and the window's own size change reaches the plugin
through `set_size`, which is attempted only for an editor that reports it can be resized.

**Rejected.** Making `resize_gui` skip the `canResize` check (the user's drag of a fixed editor must stay refused);
calling `onSize` from inside `resizeView` (reentrancy).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BN. The MPE switch of a plugin device is ignored, and not offered, for a format without raw MIDI

**Question.** The `mpe.send` switch of a plugin device sends the notes as raw MIDI messages on member channels. VST3 has
no raw MIDI input, and the VST3 processor drops those messages on purpose, so a VST3 instrument on a device with the
switch on received no notes at all and stayed silent.

**Decision.** `PluginFormat::takes_raw_midi` says whether a format takes raw MIDI (CLAP: yes, VST3: no). The manager
tells each plugin device its format when it builds it (a device built without that call assumes a format that takes
raw MIDI), and a device of a format that does not take raw MIDI treats `mpe.send` as off whatever its value, so the
notes reach the plugin as note events, with the per-note expression the plain path already gives a VST3 plugin as note
expression events. The parameter stays in the descriptor and in saved projects (no descriptor change, no format change):
a project that has the value on keeps it, and its VST3 instrument now plays. The switch is hidden, not greyed out, on
the panel of such a plugin (the rack panel and its hit test share one visibility rule). A plugin of this format in its
own process takes the same path: the format comes from the request that names the plugin, not from where it runs.

**Rendered audio.** The one deliberate change of rendered audio of this work: a project with a VST3 instrument on a
device whose switch was on rendered silence from that device and now renders its notes. That case was broken.

**Rejected.** Translating the MPE messages into VST3 events (the plain path already carries the same per-note
information natively, with a note id); removing the parameter for such a plugin (a descriptor change, and a project
saved with the value would no longer match); greying the switch out (a control that cannot work is better absent).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BO. A delay line splits its delay without the library `floor` where the target has no rounding instruction

**Question.** A release build for baseline x86-64 has no SSE4.1, so `f32::floor` is a call into the C runtime there,
and every read of a delay line made one (a reverb reads about twenty lines per sample). On `aarch64`, or with `sse4.1`
enabled, `floor` is one instruction, and integer arithmetic in its place would be slower than what exists. The build
flags are not to change (the program must still start on any x86-64).

**Decision.** `DelayLine::read` and `ModDelay::read` split the delay into its whole part and the index of it with
`split_floor`, which gives the same bits as `floor` and a cast for every `f32`. It picks by `cfg(target_arch)` and
`cfg(target_feature)`, in one place and not at the call sites: with an instruction (`aarch64`, or `sse4.1`) it is the
expression itself, so the platform behaves as it did. Elsewhere a delay between 1 and 2^23 samples is cut out of its
bits by a shift (the integer is the floor, and converts back exactly) and every other value goes to the expression.
Both forms give the same bits, so a project renders identically on every platform, and the tests run the form for the
targets without the instruction on every platform (against the old bodies and against the expression, bit for bit;
once by hand against all 2^32 `f32` bit patterns).

**Rejected.** Building with `target-cpu` or `sse4.1` (excludes older machines); a truncating conversion or a rounding
add and compare (measured: the first costs a saturating cast, the second a branch taken at random for a random
fraction); the same code on every platform (slower on `aarch64`); the same replacement for the phase wraps of the
oscillators (`p - p.floor()`: measured on the FM synth, the wavetable synth, the organ, the rotary speaker, the semi
modular and the drum machine, none moved beyond the noise, see `perf/BASELINE.md`, so they keep the library call).

**Status.** Default (not yet confirmed). Decided by: none yet.


## BP. A device that has gone silent is not called: the `quiescent` and `idle_advance` contract

**Question.** Every routed device is called every batch, however silent its input, so an idle rack costs what a busy
one costs for the devices whose cost does not depend on the signal (the reverbs, the amp and character devices, the
delays: a few percent of a batch each). The engine cannot tell
from outside that a device has nothing left to do, and a rule that guesses changes rendered audio.

**Decision.** The `Device` trait gets two methods with defaults, so every existing device behaves as before.
`quiescent(&self) -> bool` (default `false`) is the device's own statement that, from its present state, a batch whose
audio and CV inputs are all `+0.0`, with no note, no parameter change and the transport merely advancing, would write
only `+0.0` to every output, emit no note, and leave it in exactly the state `idle_advance(BATCH)` leaves it in, in
which it would say so again. `idle_advance(&mut self, frames)` (default: nothing) advances what runs free (an LFO phase,
a counter) as `process` would have, step by step. A source never says `quiescent`. The engine asks after each `process`
of a single-stage device, and while the answer was true it does not call `process` for a batch in which: every audio and
CV input is bitwise `+0.0` (a `-0.0` or a NaN is sound); no note is in a note input; no playback batch is on; the
transport only advanced (the same tempo, playing state, time signature, loop, tune and harmony, the frame one batch on, the
position still or one batch of ticks on, no loop wrap); the node is not faulted and not on its own preview clock. It calls
`idle_advance(BATCH)` instead, on the thread that would have run the node; the gather and scatter run as always, so the
outputs are `+0.0` and the engine's own meters read zeros. Any call into the device that can change it (`set_param`, a
sample map or pattern data hand-off) and every routing swap (the device may see other `connected` flags) clears the answer,
which is asked again after the next `process`; a read of its meters or its latency does not. A multi-stage device is not
skipped. `Bypassable` is quiescent in the two states that only record the dry input (effect running at the wet end with
a silent dry ring and a quiescent effect, or stopped behind a settled bypass with a silent ring) and not while it
crosses, counts down to its stop or matches loudness; `DelayLine` and `ModDelay` know that they hold only zeros.
For the load rule (`crate::load_gate`): a skipped batch is not timed and feeds nothing to the smoothed cost, so a node keeps
the cost of its last real run and the gate stands where it would have stood. The hand-over of a level is decided after its
gather, counting only the nodes that really run: a level of idle devices wakes no worker, and the batch in which they wake
hands the same nodes over as without the skip. A test-only switch (`Engine::set_idle_skip`, never a setting, never saved)
turns the skip off; the default is on. The rule is a pure function of the node and of what the gather delivered, so live,
offline, any block size and any worker count skip the same batches of the same nodes.

**Rendered audio.** None changes: a render with the skip on is byte for byte the render with it off, checked per
condition, for a free-running device across silence (and a device with a wrong `idle_advance`, which the same check
catches), and for every worker count.

**Rejected.** Skipping on a measured level instead of exact zeros (a bounce would depend on a threshold); counting a
skipped node as free in the load rule (the batch in which it wakes would be decided as calm and run late, the trap
the rule exists to avoid); a skip decided by the engine for devices that never said so (the engine cannot know that a
device has no free-running state).

**Status.** The two methods: Decided by: the owner. The engine rule and its details: Default (not yet confirmed).
Decided by: none yet.


## BQ. The interface tells a device whether anything shows its display-only numbers: the observed flag

**Question.** The mastering device and the phase meter measure loudness and true peak (the phase meter also the
stereo image) on the audio thread every batch, for a panel that is often folded, scrolled out or in a minimized
window. The sound never reads those numbers. How does the interface say that nobody looks, without losing a value
that an offline report, a test or the QA server reads?

**Decision.** `ScopeTap` (`atomik-device-sdk`) gets a second flag next to `live`: `observed`, with `set_observed`
and `is_observed` (one relaxed load per batch), **on by default**. A device that carries it returns the tap from
`Device::scope_tap`; the mastering device, which has no samples to show, carries it on a tap that is never written
(`ScopeTap::flags_only`, the smallest ring). Only the windowed app clears it (`EngineSink::set_device_observed`,
sent when it changes and again for a device with a new engine id), for a device whose panel is not on screen, and
sets it again before the panel is, so the headless readers (an offline render, a bounce, the command-line tools,
the QA server's sessions, every test) never see it change. It is not a parameter: it is not saved, automated, undone
or routed through `set_param`.

What a device skips is exactly what only the display reads. The mastering device skips the loudness meter and the
two true-peak passes; the limiter, its detector and the gain-reduction values are never gated. The phase meter skips
the stereo image, the RMS levels, the loudness and the true peak; its recent and held peaks and the sample peak since
the last reset (a latched clip history, cheap) always run, and so do the audio and the scope tap. A skipped value
reads as after a reset (-120, or 0 for the image), and when the flag is set again the meters start from nothing at
that batch, so the integrated loudness and the true-peak maximum cover the time since the panel came back. The panel
shows what it shows after a reset; no new text, and no value is invented for the hidden time. The same rule, at the
view level, decides which analyser panels are analysed at all (`RackView::is_panel_visible`: unfolded, inside the
rack area, in an open container, front face up, the window neither minimized nor covered).

**Rejected.** A device parameter (it would be saved with the project and automated, and a project would sound the
same but read differently); a new engine-wide channel for flags (the tap already travels from the device to the
interface); keeping the meters running and only hiding the numbers (the cost is the point); gating the sample peak
(a clip that happened while the panel was folded would not show when it opened). Stopping the engine's own level
meters the same way was measured and left alone: the console strips, the track header meters and the record level
read them and keep a latched peak, clip lamps and a held maximum, so a hidden mixer would lose a clip.

**Status.** Default (not yet confirmed). Decided by: none yet.

## BR. The machine's memory is read in one file of the app, and three limits follow it

**Question.** On a machine with 4 GB, the undo history (512 MiB), an import (4 GiB decoded) and a bounce (every asset
decoded, the whole output held, twice) can each outgrow the memory, and an allocation that fails aborts the process.
The limits have to follow the machine, and the figure has to come from the system.

**Decision.** `crates/app/src/platform/memory.rs` is the one new file that may use `unsafe`: `GlobalMemoryStatusEx`
on Windows and `sysctlbyname("hw.memsize")` on macOS, each a few lines with a `SAFETY:` comment; Linux reads
`/proc/meminfo` as text and needs none. It gives the total (read once) and the memory that is available now. Every
limit is a pure function of those figures, so a test injects a machine (`AppCore::set_memory_info`): the undo budget
(`undo_bytes_for`: 128 MiB under 8 GiB), the import limit (`import_limits_for`: a quarter of the memory, never above
the old cap) and the estimate of a render (`atomik_render::estimate`). The model, audio-file and render crates stay
free of system calls: they take the limit or the figure as a parameter. Every limit leaves a machine with 16 GiB or
more, and a machine that does not tell its memory, exactly as before. The size compared is the reported memory
rounded up to a whole GiB, because a machine reports a little less than it is sold with. Below 16 GiB a file that has to be converted
to the song's rate counts twice, because the conversion holds a second copy; at 16 GiB and above the old cap is
kept as it was, so the doubling does not shrink what a large machine accepts.

A render that may not fit is a question, not a refusal: the estimate is rough and the system can page. The export
panel shows the two figures under the options and a second EXPORT goes on; a freeze or a preset export says it in a
message and goes on when asked again. The command-line tools and the QA server have no one to ask and never ask.

**Rejected.** Reading `/proc/meminfo` or the registry in the model crate (the model stays free of system calls);
refusing a render outright (a machine with a large page file can do it); a setting for each limit (no new
setting); a native confirmation dialog (a second EXPORT is the way the panel already confirms a replaced file).

**Status.** Default (not yet confirmed). Decided by: the owner for the one `unsafe` file; the rest none yet.

## BS. A session has a budget for the audio it keeps in memory, and an asset handed to the engine is never converted

**Question.** An audio asset up to 64 MiB decoded was loaded whole, and a larger one streamed from disk. The rule looks
at one asset, so forty five-minute mono stems (55 MiB each, 2.2 GiB) all stayed resident, muted and unused ones too,
on a machine with 4 GB.

**Decision.** The sum has a budget, a pure function of the machine's memory (`memory_limits::audio_budget_for`): a
quarter of the reported size (rounded up to a whole GiB, as the other limits), at least 256 MiB, and no budget when
the memory is not known. That is 1 GiB at 4 GiB, 2 GiB at 8, 4 GiB at 16, 8 GiB at 32 and 16 GiB at 64, so a machine
with 16 GiB or more only sees it for a session holding more than 4 GiB of audio. When the resident audio would
exceed it, the largest assets stream (`stream_source::plan`: size descending, ties by hash, so two opens agree),
except an asset of at most 4 MiB decoded (a streamed lane costs a ring of about 770 KiB and disk reads for too little
memory) and an asset a sampler zone uses (zones play from RAM; a streamed one would be read whole into a second copy).
The 64 MiB threshold stays as it was: the budget alone already does what a lower threshold under 8 GiB would, and
better, because it counts the sum and not the one file.

Every place that decides asks one `StreamRule` (`AppCore::stream_rule`: the threshold, the budget and the audio the
session holds at that moment, recounted from the assets it keeps). A project opening decides for the whole list
before it loads any, so a streamed asset is never decoded first, and writes one line in the log when the budget moved
files to the disk. An import, a recording, a paste, a freeze or a rate conversion decides for one asset: it streams
when keeping it would pass the budget. **An asset handed to the engine is never converted**, in either direction: a
deletion gives its memory back and the assets that stream keep streaming until the project is opened again, because
swapping an asset under a playing engine means a hand-over to the audio thread and a click risk for little gain.
The budget is hidden like the threshold: `stream_budget_bytes` in the settings, and the injected memory figure of a
test decides the default. A bounce, an export, a stem export and a freeze render never stream: the render crate loads
every file.

**Rejected.** A lower per-asset threshold under 8 GiB (the budget covers it); a visible setting (no new setting);
streaming the oldest or the quietest assets (the largest saves the most memory for the fewest rings and reads);
converting live assets when memory frees up; counting the reference track, a plugin's own sample memory and warp
renders (each is one object of its own, loaded on request).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BT. One Performance setting (Auto, Full, Low power) sets the frame rate, the render size and the automatic workers

**Decision.** The Interface tab has one setting, Performance, saved in `workflow.json` (`performance`: `auto`,
`full`, `low_power`; a file without it is Auto, the project format is untouched). Low power does three things and only
these. The shared frame slot is 30 Hz instead of 60 (the same `App::frame` the frame cap already used, now a value the
level sets, not a second pacing mechanism). A main window's graphics target is rendered at 0.75 of its physical size
and drawn to the surface by the existing blit, with linear filtering. And an automatic render worker count is cut to
one. **Auto** is Low power on a machine with 4 or fewer logical CPUs and Full otherwise, decided once at start and never
switched while the program runs; the button says which one it picked ("Auto (Low power)"). The frame rate and the render
size apply when the setting is applied; the worker count is read when an engine starts, so it applies from the next
start (no live change of the worker count exists, and a bounce never depends on it).

**Why.** A 4K panel costs four times the pixels of a 1080p one, and the UI is drawn whole at each paint, so rate and
render size are the two levers that cut the GPU and CPU cost without a rewrite; fewer workers cut the idle spin on a
small machine. 0.75 exists cleanly (1440 x 810 from 1920 x 1080, 2880 x 1620 from 4K) and keeps text readable under
a linear stretch; 0.5 would not. At Low power a lone window does not wait for the vertical blank either, because a
60 Hz monitor would let input paint it twice as often as the level says. Only a count that was automatic is made
conservative: a number the user fixed is theirs. The scene is scaled into the smaller target at render time, so layout,
hit testing, reported text positions and screenshots never see the scale.

**Rejected.** Switching Auto while the program runs (the frame rate and the picture would jump, and a laptop on
battery is not a smaller machine); 0.5 as the Low power scale; scaling the surface itself (the window would need a new
swap chain at every change); a lower worker count than one at Low power (a heavy rack would miss its deadline); a
separate setting per effect (three switches for one decision).

**Status.** Default (not yet confirmed). Decided by: none yet.

## BU. The audio restarts by itself after a contained fault, only while stopped, at most once a minute and three times

**Decision.** When the engine reports a contained fault (a device that panicked, or a panic that halted the stream),
the shell rebuilds the engine by itself, with these limits: only while the transport is stopped (a fault during
playback waits for the stop), at least 60 seconds after the previous automatic restart, and at most three automatic
restarts in a session (a plain count, not a streak: the simplest rule that cannot loop). Before the rebuild the device
the fault names is bypassed in the model, as a normal undoable edit, so the new engine does not meet the same crash; a
fault that names no device bypasses nothing, and a device without a bypass switch (an instrument) is named in the
message and left in place. After the limit the user gets the manual message (Audio > Restart Audio), which keeps
working. A plugin whose process crashed keeps its own crash handling: it is not an engine fault, and a device that the
plugin path holds as crashed is never restarted from here. A message says what happened.

**Why.** One contained bug in an unattended or live session was permanent silence until someone found the menu. The
bypass first keeps the rebuild from repeating the crash; the stop and the minute keep a restart from cutting a take or
looping. Undo brings the device back, and if it panics again the same limits hold.

**Rejected.** Restarting during playback (it cuts the music and a take); an unlimited or per-streak count (a device that
always panics would restart for ever, or the streak would need its own reset rule); removing the faulted device (a
bypass is reversible); acting on a crashed sandboxed plugin here (it already has its reload).

**Status.** Decided by the owner on 2026-11-24 (restart automatically, as planned).

## BV. No software rendering: a machine without a graphics driver gets a message

**Decision.** The interface is drawn on the graphics card and nothing else. The app never runs on a software (CPU)
adapter unless the user names one with `WGPU_ADAPTER_NAME`, and when no hardware adapter is left it shows a native
message (a graphics driver with DirectX 12, Vulkan or Metal is needed, what was found, the log path) and exits with an
error code. The README's install section states the requirement. There is no CPU rendering mode.

**Why.** The Windows software rasteriser faults inside a system library a few seconds after start, which Rust cannot
catch, so it is not a fallback for this renderer. A spike measured the CPU renderer already in the tree (used for the
screenshots) at 16 to 29 ms per main-window frame for light scenes and 47 ms for a dense rack on a fast desktop, one
thread and no display-list cache: too slow for a usable interface, and a slower machine is the one that would need it.

**Rejected.** A "software rendering" mode on the CPU renderer (frame times above); falling back to the software
adapter silently (the crash above).

**Status.** Decided by the owner on 2026-11-24.

## BW. The audio stream opens and stops on a worker thread; the calls made meanwhile wait for the engine

**Decision.** Opening a stream (with its output-only retry and its fall back to the last working device) and stopping
one run on a worker thread, one job at a time per session. `EngineSink::reset` returns at once in the app (a headless
run and the tests keep the blocking form), the session is "starting" until the answer is taken at the next status
poll, and the status line says "Starting audio...". While it is starting there is no engine (`EngineSink::has_engine`
is false, as with audio off), so the plugins wait behind their stand-ins as they do then; the control calls (devices,
routing, song, tempo, transport, previews) are kept in order, the newest routing and song replace older ones, the
newest value of each parameter is kept, and all of it is sent when the stream is up, as far as the engine's queue has
room and the rest as the engine takes it. A new request replaces the one waiting; a stream that arrives for an older
request is stopped, never used. A stream stopped by a reset is stopped by the same job, before the next one is opened,
and a job that opens waits for the stops other sessions are running, so two songs that hand the audio over never fight
for a device. The engine a stopped stream hands back is dropped on the UI thread. A quit waits at most ten seconds for
the worker, then leaves it behind.

**Why.** The window froze for as long as a driver took to answer, up to 5 s per call and 10 s when a fixed buffer was
refused and retried, and a hung driver froze it for good. The blocking calls are the only part that waits; everything
else stays on the UI thread as before.

**Rejected.** Filling an engine made before the stream and attaching it later (the output-only retry and the fall back
each need a fresh engine, so the calls made meanwhile would be lost and the app would have to send the whole document
again); a time limit on the wait inside `reset` (a slow driver would still freeze the window for that long, and the
outcome would depend on the machine's speed); stopping on the UI thread and only opening on the worker (a hung stop is
as long as a hung open).

**Status.** Decided by the owner on 2026-11-25.

## CB. Song scales are part of the harmony, quantizers name them by id, and the format stays

**Question.** Where do scales shared between quantizers live, and how does a quantizer pick one by name?

**Decision.** The named scales (up to 16: an id, a name, twelve pitch classes) are a field of `Harmony`, saved as the
optional ignorable song field `scales` of the current format, so an edit is one `SetHarmony` (one undo step) and
reaches the engine with the song swap. The engine carries the whole table in every batch's `HarmonyNow`. A CV
Quantizer saves the scale's id (`scale.source` named, `scale.id`), so a rename keeps every link; an id the song lacks
leaves it on its own scale. A new scale takes the id after the highest in use. The delay line of the Gate Processor is
one bit per sample and a minute long so a synced delay of four bars fits at any tempo.

**Why.** Reusing the harmony command and compile path needs no new command, history entry or engine message. Ids keep
links through renames and edits.

**Rejected.** Storing the name in the device (a rename would break links, and the audio thread would need strings); a
separate song command for scales (more plumbing for the same undo behaviour); a format bump (the field is ignorable).

**Status.** Chosen by the implementing agent for review.
