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

**Decision.** Working assumption: a byte budget (for example 256 MiB) with a "history trimmed" note.

**Status.** Default (not yet confirmed). Recorded 2026-10-30 as the working assumption; awaiting the maintainer's review.

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
