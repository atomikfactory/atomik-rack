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
stays, and the assets only dropped steps held leave the pool at the next prune. There is no
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
