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
and pinch again to keep turning; a change under 1.5 degrees is held back, so a still hand does not drift. A hand that
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
`full_turn` is the setting "Twist for full range" (90 to 360 degrees, default 270); with "Fine control by distance" the
hand size at the pinch scales it from 1 (a hand of 0.12 frame heights or smaller, far away) to 0.25 (0.30 or larger,
near), linearly in between. A stepped param counts steps from where the pinch began, one for each step width of turn,
and takes a step back only after a whole step width the other way. The turn is kept inside the range, so there is no
wind-up. A pinch with no target changes nothing and says "Click a knob first". The camera is off by default, on only
after the user turns it on (the CAM button, `camera.toggle`, View > Panels > Camera), and off when turned off, when the
app quits and on any camera error; the hand models download only after the consent dialog's Download. The settings
(camera, hand, full turn, fine control, pinch sensitivity, preview) are workflow preferences in a `camera` object of
`workflow.json`, never in a project. The format is not changed.

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
