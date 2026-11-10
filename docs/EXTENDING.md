# Writing a new device

This guide explains how to add a device to Atomik Rack using `atomik-device-sdk`. It describes the contract in prose and points to the code. The built-in devices in [crates/devices/src](../crates/devices/src) are complete, working examples; the small ones (`stereo_delay.rs`, `line_mixer.rs`) are the best start. Read one of them next to this page.

A device must be an original design, or based on public textbooks, papers or standards, or on compatibly licensed code (see the original work policy in [CONTRIBUTING.md](../CONTRIBUTING.md)). It must not copy another product's code, data, presets or strings.

## 1. Where a device lives

Built-in devices are modules of `atomik-devices` (`crates/devices/src/<name>.rs`). Each module exposes three things:

- a `pub static DESCRIPTOR: DeviceDescriptor`;
- a `pub fn factory() -> DeviceFactory`;
- the device struct, which implements `atomik_device_sdk::Device`.

A device may depend on `atomik-core`, `atomik-device-sdk` and `atomik-dsp`, and on nothing else in the workspace. It must never depend on `atomik-engine` (see [ARCHITECTURE.md](ARCHITECTURE.md#1-crates-and-the-dependency-rule)).

DSP building blocks are already in `atomik-dsp` ([crates/dsp/src/lib.rs](../crates/dsp/src/lib.rs)):

- `PolyBlepOsc`, `Svf`, `Adsr`, `Lfo`;
- `Smoother` and `OnePoleSmoother`;
- `DelayLine`;
- `pan_constant_power`, `note_to_hz`;
- the seeded `Rng`.

Reuse them before you write new ones.

## 2. The descriptor

The descriptor is `'static` data. The engine, the patch loader and a future user interface all read it. The types are in [crates/device-sdk/src/descriptor.rs](../crates/device-sdk/src/descriptor.rs) and [crates/core/src/param.rs](../crates/core/src/param.rs).

`DeviceDescriptor` has these fields:

- `type_id`: globally unique, by convention `atomik.<kebab-name>`;
- `name`;
- `kind`: `Instrument`, `Effect`, `Mixer` or `Utility` (`Interface` is reserved for the engine);
- `version`;
- `params: &'static [ParamDescriptor]`;
- `sockets: &'static [SocketDescriptor]`.

Rules, checked by `DeviceDescriptor::validate()` and therefore by `Registry::register`:

- **Ids equal indices.** `params[i].id` must be `ParamId(i)` and `sockets[i].id` must be `SocketId(i)`. The engine indexes the context and the param table by id. Defining the ids as named constants next to the table (as the built-in devices do) keeps them in step.
- **Unique keys.** Param keys must be unique among params, and socket keys unique among sockets. Keys are public API: patch files use them. Once a device is released, do not rename a key.
- **Valid ranges.** Each param needs finite `min ≤ max` and `min ≤ default ≤ max`.
- **Tapers.**
  - `Taper::Exponential` requires `min > 0`. Use it for frequencies and times.
  - `Taper::Decibel` is linear in dB, and its `min` means silence (gain 0). Use `ParamDescriptor::db_param_gain` to turn a dB value into a gain.
  - `Taper::Linear` covers everything else.
- **Stages.** `stages` is empty (every socket is stage 0) or has one entry per socket. The stages used must be `0..=max` with every number present and `max` below 255: a table with a lone stage 7, a skipped stage or a 255 is refused.
- **Discrete.** Set `discrete: true` for choices (`Unit::Choice(&[..])`), toggles (`Unit::Toggle`) and counts. The engine then rounds the plain values for you.

Socket naming conventions:

- Stereo is two mono sockets, `*_l` and `*_r`.
- Inputs end in `_in` (or `_cv_in`), and outputs in `_out` or a plain name such as `out_l` or `master_l`.

## 3. The `Device` trait

The trait is in [crates/device-sdk/src/lib.rs](../crates/device-sdk/src/lib.rs). It must be `Send + 'static`.

| Method | Thread | May allocate? | Contract |
|---|---|---|---|
| `fn descriptor(&self) -> &'static DeviceDescriptor` | any | no | Returns your static descriptor |
| `fn prepare(&mut self, info: &PrepareInfo)` | non-RT | **yes** | Called once by `Graph::compile`, before the first `process`, and again whenever the sample rate changes. Size every buffer here, and leave the device in a reset state. |
| `fn reset(&mut self)` | RT | no | Clears audio state (voices, delay lines, smoothers) |
| `fn set_param(&mut self, id: ParamId, plain: f32)` | RT | no | `plain` is already clamped (and rounded if discrete) by the engine |
| `fn process(&mut self, ctx: &mut ProcessContext<'_>)` | RT | no | Processes exactly one batch of 64 frames |
| `fn latency_frames(&self) -> u32` | any | no | Default 0. The engine reads it after `prepare` and, while the project's delay compensation is on (the console's `compensation` flag), delays the faster paths into every summing input to match the slowest one. |
| `fn scope_tap(&self) -> Option<Arc<ScopeTap>>` | any | no | Default `None`. Opt-in to a ring of the recent stereo frames for the UI (see below). |

### `prepare` is the only allocating method

`reset`, `set_param` and `process` run on the render path. They must not:

- allocate, reallocate or free memory (no `Vec` growth, `Box::new`, `String`, `format!`, `collect`, or cloning heap data);
- lock or block;
- do I/O, including printing.

In particular:

- **Size buffers in `prepare`.** Size them for the worst case at `info.sample_rate`. The delay, for example, sizes 4 s of buffer there.
- **Keep fixed-size state inline.** Store voices, smoothers and similar state in fixed-size arrays in the struct; the poly-synth preallocates 16 voices.
- **Seed every pseudo-random source from `info.seed`.** The engine derives that seed from the patch seed and the device instance id. Never use OS entropy, time or addresses (HI-2).
- **No hash-ordered collections or time sources.** `HashMap`, `HashSet`, `Instant::now` and `SystemTime::now` are banned by [clippy.toml](../clippy.toml).

A device instance is constructed by its factory (`create: fn() -> Box<dyn Device>`) on the non-RT side. Construction may allocate, but `prepare` must still be correct when it is called again.

#### The audio-thread rule guard

Test builds check the "lock or block" rule as well as the allocation rule. The engine marks every
`process` call with `atomik_core::rt_guard::audio_thread()`; inside it:

- `atomik_core::rt_guard::RtMutex` panics when locked (use it for any mutex a device owns that
  other threads also take, so a device that locks in `process` fails its test);
- `atomik_core::rt_guard::forbid("what")` panics, and goes in front of every blocking host call
  (file or socket I/O, sleeping, joining a thread) that a helper may make.

The panic is a contained device fault, so a test asserts `EngineHandle::fault()`; see
[crates/engine/tests/rt_rules.rs](../crates/engine/tests/rt_rules.rs). The guard exists only with the
`rt-guard` feature, which only the test kit enables: release builds carry no detector code and
`RtMutex` is a plain `std::sync::Mutex` there. A guarded render test must call `Engine::render`
directly, because a violation's panic message allocates and `render_block` runs under the allocation
guard.

### Showing more than 32 numbers: the scope tap

`Device::meter_values` publishes up to 32 floats per batch, enough for a level, a gain reduction or a correlation. A device that wants the UI to draw something from the samples themselves (a spectrum, a goniometer) returns a [`ScopeTap`](../crates/device-sdk/src/scope.rs) from `scope_tap`: create it in the constructor (`Arc::new(ScopeTap::new())`, which allocates), keep a clone, and in `process` call `tap.push(&left, &right)` with the 64 frames of the batch. The push is relaxed atomic stores and one release store, so it is real-time safe; the engine registers the tap when the device is inserted, and the app reads the newest frames each UI frame and does the analysis (FFT, decimation, drawing) on its own thread. `atomik.spectrum` and `atomik.phase-meter` are the examples; see [ARCHITECTURE.md](ARCHITECTURE.md#6k-analysers-the-scope-tap-spectrum-and-phase-meter).

### Output zeroing contract

The engine zeroes every Audio/CV batch of the device before each `process` call (see `RenderPlan::process` in [crates/engine/src/plan.rs](../crates/engine/src/plan.rs)). It also clears note outputs. This gives you three guarantees:

- `ctx.output(socket)` starts at 0.0. You may accumulate into it with `+=`, or overwrite it.
- An output you do not write stays silent. It never holds stale data from a previous batch.
- An unconnected Audio/CV input reads 0.0. Do not rely on anything else.

`DeviceHarness::process` applies the same zeroing, so tests match the engine.

### Reading the context

[crates/device-sdk/src/context.rs](../crates/device-sdk/src/context.rs). All accessors take a `SocketId`:

- `input(id) -> &Batch` and `output(id) -> &mut Batch` give 64 samples (`Batch = [f32; 64]`).
- `notes_in(id) -> &[NoteEvent]` returns this batch's events, sorted by non-decreasing `offset` (0..63). Handle each event at its offset to stay sample-accurate; the poly-synth renders in segments between events.
- `notes_out(id) -> &mut NoteBuffer` has a fixed capacity. `push` returns `BufferFull` instead of growing.
- `is_connected(id)` is true for a cabled Audio/CV socket. Notes inputs always report true.
- `ctx.time` holds the batch start frame, the sample rate, the tempo and the playing flag.

### CV conventions

- CV is nominally **bipolar, −1..+1**. A CV consumer clamps its input to −1..+1 (or to 0..1 for unipolar uses) before using it.
- Built-in examples:
  - pitch-like or octave-like amounts scale as `amount × cv` octaves (the synth's cutoff CV);
  - time-like amounts scale as `2^cv` (the delay's time CV);
  - gain-like amounts use `cv.clamp(0, 1)` (the mixer's level CV).
- If a CV input should do nothing when unconnected, check `ctx.is_connected` rather than treating 0 as a value. The mixer does this, because an unconnected level CV must mean "gain × 1", not "gain × 0".
- A CV output should stay within −1..+1. The synth's `lfo_cv_out` does.
- Audio is nominally ±1.0. Keep outputs finite. The synth adds a soft limiter so its output is always bounded.

#### CV inputs with a trim (instruments, effects, console channel)

One convention for every dedicated CV input, with the shared helpers in [crates/devices/src/cv_in.rs](../crates/devices/src/cv_in.rs) (`clean`, `batch_mean`, `add_norm`, `db_gain`):

- Socket `<target>_cv_in` and a bipolar trim param `<target>_cv_trim` (−1..+1, default +1, negative inverts; keys of dotted params keep the dot: `band3.gain_cv_trim`, `master.level_cv_trim`). New params and sockets are appended or inserted by key, so existing keys never change and older patches and projects load as before.
- The CV is sanitised (non-finite becomes 0), clamped to −1..+1, multiplied by the trim and by a **fixed range per target**, and added to the target in its own unit; the sum is clamped to the parameter range. The ranges:

| Target | Range at trim +1, CV +1 | Used by |
|---|---|---|
| Normalised parameter (mix, feedback, drive, resonance) | +1 of 0..1 | delay, reverb, modulation, distortion mix/feedback/drive; poly synth and sampler resonance |
| Level or gain in dB | ±24 dB | poly synth, sampler, loop slicer output level; drum machine level; EQ band gain; channel fader |
| Send level | ±66 dB | console channel sends |
| Pan | ±1, added before the pan law | console channel pan; sampler pan (per voice); poly synth balance |
| Pitch | ±24 semitones | poly synth, drum machine |
| Cutoff | ±4 octaves | loop slicer global low-pass |
| Rate | ±4 octaves | Mod Source LFO rate (`lfoN_rate_cv_trim`) |

- Documented exceptions with no trim, both on the Mod Source: `lfoN_amount_cv_in` (a multiplier, clamped to 0..1) and `gate_cv_in` (a gate, high above 0.5).
- Unconnected sockets are skipped with `is_connected`, so the output is bit-identical to the device without the input (the render goldens must not change); each device has a test that sets odd trims with no cable and compares bits.
- Per-sample targets (level, pan) read the CV sample by sample with no extra smoothing, so a stepped CV gives a stepped gain; pitch, resonance and cutoff are read at the 16-sample control rate with the filter coefficient ramped in between. A device that smooths per batch uses the batch mean of the CV.
- Instrument-wide targets (synth pitch, resonance, level, pan; sampler level; slicer cutoff and level; drum machine level and pitch) act on all voices alike; per-voice-state targets stay per voice (sampler resonance and pan).

### Modulation matrix

A device can let the user route its own sources to its own targets without a cable. The framework is
device-level: the engine has no concept of it, the sums are computed inside the device, and the operators are
`+`, `-`, `*` and `/` only, so the result is bit-identical on every platform. It is the working default of
[decision E](DECISIONS.md#e-modulation-framework) (device-level slots; not yet reviewed by the maintainer).

- **Tables.** The device declares a `ModTables`: its **sources** (`ModSource`: an id, a key, a name and whether it
  is bipolar) and its **targets** (`ModTarget`: an id, a key, a name and the param it belongs to), and registers
  with `Registry::register_with_mods` (which validates them against the descriptor: ids equal indices, keys are
  unique, every target is a real param, at most 16 of each). Source and target **ids are stored in projects**, so
  a released table only grows: append, never reorder or reuse. A target is a *place* the sum goes, not a stored
  value: the device adds the sum to the target's own value while it renders and never writes it back.
- **Slots.** Every device with tables has `MOD_SLOTS` (8) slots. A slot is `source -> target` with an `amount`
  (-1 to 1) and optionally a `via` source that scales the amount: the contribution is
  `amount * source * via`, where `via` is read clamped to 0..1 (a mod wheel that fades an LFO in) and a slot
  without one is not scaled. Sources are sanitised (non-finite is 0) and clamped to -1..1 by the bus. A
  negative amount inverts; there is no other operator.
- **Params.** The slots reach the device as 32 params at the end of its table, made by `mod_slot_params` and
  joined to the device's own with `join_params`; `ModTables::first_param` says where the block starts. Slots are
  numbered 1 to 8 and each has four params, shown here for slot 1: `internal.mod1_source`, `internal.mod1_target`
  and `internal.mod1_via` (0 is none, `k` is id `k - 1`; set from the slot list, never stored as params) and
  the plain, automatable `mod1_amount`. The slot structure lives in the document (`RackDevice::mods`, edited by
  `SetModSlot`, one undo step, saved from project format 41), and the expansion turns it into the internal
  params. The amount is an ordinary param, so it is saved, undone, automated, mapped and in a patch like any
  other.
- **The bus.** The device keeps a `ModBus` (a small `Copy` value) next to its other shared state, hands it
  every param of the slot block from `set_param` (`ModBus::set_param` returns true for them), and at its control
  rate asks `ModBus::offset(target, &sources)` for the sum of every slot that reaches a target. `sources` holds
  the current value of each source by id. `ModBus::add_norm` is the `cv_in::add_norm` convention (base plus
  offset, clamped to 0..1) for a target in normalised space; a device that works in octaves or semitones scales
  the offset itself (the poly synth: 6 octaves of cutoff and 24 semitones of pitch at amount 1). Slots are
  summed in slot order, so the order is part of the contract. Guard everything with `ModBus::is_active`: a
  device with no slot in use runs none of the new code and must render exactly as before.
- **Rules.** No allocation and no lock on the audio thread (the bus is fixed-size); per-voice sources are
  computed by the device per voice; a dual or layered device that wraps another keeps the wrapped device's slot
  params out of its own table (the dual layer lists them as hidden). A registry that is built by copying
  another one must carry the tables over (`Registry::attach_mods`).
- **Tests.** Check the tables with `ModTables::validate` (the registry does it), that idle slots and a cleared
  slot render the same bits as no slots, that the output is the same bit for bit however the render is cut
  into calls and for every block size and worker count, and the allocation guard (the harness has it).
  The poly synth's tests are the model; its render test is in `crates/render/tests/mod_matrix.rs`.
- **Panel.** The generated front panel skips the slot params (`is_mod_slot_key`). The matrix itself is edited in
  the modal panel the device's right-click menu opens (Modulation Matrix, command `rack.mod_matrix`): it is built
  from the registered tables, so a device that registers `ModTables` gets it with no panel code.

## 4. Registering the device

Add one line to `register_all` in [crates/devices/src/lib.rs](../crates/devices/src/lib.rs), which registers each module's `factory()`. Registration validates the descriptor and rejects a duplicate `type_id`. The renderer's `default_registry()` ([crates/render/src/lib.rs](../crates/render/src/lib.rs)) calls `register_all`, so the new `type_id` becomes usable in patch files at once. `Registry::get` answers with the descriptor of any registered type (a plugin type too) and `Registry::create` builds an instance; only a built-in type has a constructor to hand out (`Registry::factory`).

Then update the tests that list every type id:

- `TYPE_IDS` in the `tests` module of `crates/devices/src/lib.rs`, in sorted order;
- the type list in [examples/patches/README.md](../examples/patches/README.md);
- [DEVICES.md](DEVICES.md).

## 5. Testing the device

Device tests live in a `#[cfg(test)] mod tests` inside the device module. `atomik-devices` already has `atomik-testkit` as a dev-dependency.

- **Allocation guard.** Install the counting allocator once per test binary with `install_alloc_guard!()`. For the unit tests of `atomik-devices` this is already done in `lib.rs`. If you add an integration test file under `crates/devices/tests/`, put `atomik_testkit::install_alloc_guard!();` at the top of it. A `//!` doc comment may come first.
- **`DeviceHarness`** ([crates/testkit/src/harness.rs](../crates/testkit/src/harness.rs)) drives one device without the engine:
  - `DeviceHarness::new(Box::new(MyDevice::new()), 48_000.0)` prepares the device at 120 BPM with seed 0. `with_options` lets you set the tempo and seed.
  - `set_param(key, plain)` clamps like the engine does.
  - `input_mut(key)` gives a writable input batch and marks the socket connected. `set_connected(key, bool)` changes the flag directly.
  - `push_note(key, event)` queues a note for the next batch.
  - `process()` runs one batch. **Every `process` call runs inside `assert_no_alloc`**, so any allocation fails the test with "HI-1 violated".
  - `render(batches, &[output keys])` returns the concatenated output samples.
- **Guard other RT methods.** Wrap `set_param` and `reset` in `atomik_testkit::no_alloc::assert_no_alloc(|| ..)` to prove they are real-time safe too. The helper `sweep_params_min_max` in [crates/devices/src/test_support.rs](../crates/devices/src/test_support.rs) does this for every param at min, max and default.
- **Signal checks.** Use `atomik_testkit::signal` (`rms`, `peak`, `db`, `dominant_frequency`, `max_step` for click detection) with explicit tolerances. Never compare floats with `==`, except for bit-exact determinism checks.

Tests every device should have (the built-in devices use these names):

- `descriptor_validates`: `validate()` passes, and `assert_keys` pins the key order;
- `set_param_every_param_min_max_does_not_panic`;
- `reset_silences_tails`;
- `process_is_allocation_free`;
- one test per behaviour in the device's spec, and a property test for "output stays finite for any params" where it applies;
- a determinism test if the device uses randomness (the same seed gives bit-identical output).

Also, `process_is_allocation_free_for_every_registered_device` in `crates/devices/src/lib.rs` runs every registered device at three sample rates with all inputs driven. It covers the new device automatically once it is in `register_all` and `TYPE_IDS`.

Run them:

```sh
cargo test -p atomik-devices
```

## 6. Checklist

- [ ] The design is original (or based on public literature or compatibly licensed code), and it uses no commercial names.
- [ ] The module exposes `DESCRIPTOR`, `factory()` and the device struct.
- [ ] `type_id` is unique and follows `atomik.<kebab-name>`.
- [ ] Param and socket ids equal their indices; keys are unique and stable; defaults are in range.
- [ ] Exponential params have `min > 0`. dB params use the Decibel taper. Choices, toggles and counts are `discrete`.
- [ ] All allocation happens in `prepare` (or the constructor). `process`, `reset` and `set_param` never allocate, lock, block or print.
- [ ] Randomness comes from `atomik_dsp::Rng` seeded from `PrepareInfo::seed`.
- [ ] Outputs are finite. CV inputs are clamped. Unconnected CV is handled with `is_connected` where 0 would be wrong.
- [ ] Notes are handled at their offsets.
- [ ] Audible param changes are smoothed, because the engine applies params only at 64-frame boundaries.
- [ ] Tests use `DeviceHarness`, with `set_param`/`reset` wrapped in `assert_no_alloc`.
- [ ] The device is registered in `register_all` and listed in `TYPE_IDS`, [DEVICES.md](DEVICES.md) and `examples/patches/README.md`.
- [ ] A device with a modulation matrix registers with `Registry::register_with_mods` and follows the rules of "Modulation matrix" above.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.

## 7. Factory content

The `factory` folder at the repository root ships with the application. Nothing in it is written by hand and nothing
is downloaded or recorded: the tables and the writer are test code in the app crate (`app_core_factory_build.rs`
and the files beside it), and every sound is rendered by the application's own devices.

- **Patches** (`Patches`): a row of the patch table per device patch (the device, its changed params by key and its
  modulation slots), with a few rows for every instrument and a number for the effects. A drum kit is a table of
  hits that the writer renders from the drum machine's pad modules into short mono samples, 12 to 16 pads for the
  newer kits, with the audio in `<kit>/Audio/` beside the item.
- **Clips** (`Clips`): note clips written from pattern tables (drum lanes as step strings, bass lines, chords,
  arpeggios), and audio loops (24-bit FLAC, stereo) rendered from those clips by a factory kit or a factory
  instrument patch. A loop plays its clip twice and keeps the second pass, so the loop repeats without a gap. The
  tags of a loop carry its tempo (`88 bpm`) and, when it is pitched, its key (`A minor`), which the browser's tag
  filter lists.
- **Templates** (`Templates`): six genre songs written from the tables in `app_core_factory_templates.rs` (a tempo,
  a key, the sections on an 8-bar grid, six tracks with their clips placed by bar, launcher scenes, the levels and the
  master). A track names a factory patch or the pads of a factory kit, and a clip of the factory table or one of the
  template's own; a drum clip can feed the kick track with its kick and the drums track with the rest. They hold no
  audio and are listed first by File > New from Template and the start screen. `write_the_template_projects` writes
  only them; `calibrate_the_template_levels` (ignored) tries the fader, trim and limiter input of each track and
  prints the values to put in the table. The tests check the files against the tables, the shape of each song, the
  level of each track (near -18 dBFS RMS) and the loudness of the master (near -14 LUFS).
- **Rebuilding**: `ATOMIK_WRITE_FACTORY=1 cargo test -p atomik-app --lib write_the_factory_library -- --ignored`
  writes every file and `factory.json`; the tests then check that the files are what the tables describe, that
  every patch makes sound (a non-silent, bounded bounce of a built-in phrase), that every loop decodes at its tagged
  tempo and that the folder stays under 25 MB.

## 8. Adding a language

The interface is translated by one text file per language, `crates/ui/locales/<code>.txt`, with the English text of the
program as the key ([DECISIONS.md](DECISIONS.md#w-how-the-interface-is-translated) explains the choice). To add a
language, say German (`de`):

1. Copy `crates/ui/locales/fr.txt` to `de.txt` and replace the French of each line. A line is `English text => Text`;
   a line that starts with `#` is a comment, a backslash and `n` make a line break and two backslashes make one. Keep
   the glossary at the top up to date: a word is translated the same way everywhere.
2. A text that names a value (`Added {name}`) keeps the same names in the translation; the order may change. Such a
   line also matches any text of that shape, so a message the program builds with `format!` is translated by naming
   its shape once. A count has two lines (`Copied {n} take` and `Copied {n} takes`); the rule that picks one is
   `tn` in `crates/ui/src/i18n.rs`.
3. Names of keys and modifiers (`Shift`, `Enter`) are the lines that start with `key:`. Device names, units and
   abbreviations that read the same stay as they are (or are left out: a text with no line shows in English).
4. In `crates/ui/src/i18n.rs` add the language to `Lang` (its `code`, its own `name`, the `include_str!` of the file)
   and to `LanguageChoice::ALL`; the settings file, the Settings > Video buttons and the System choice follow from the
   list. Add the plural rule to `tn`.
5. Run `cargo test -p atomik-ui --test i18n_catalog --test i18n_sources --test french_goldens` (copy the golden and
   catalogue tests for the new code): they check that no text has two lines, that every line names the values the
   English names, that no text is much longer than the English, that every command of the registry has a line, and
   that no tooltip passes 90 characters. The views are drawn in the language into goldens under `tests/golden/<code>/`
   (`ATOMIK_UPDATE_GOLDEN=1` writes them); look at what changed before you commit it.

A text with no line is not an error: it shows in English. The tests that draw the views list the texts that a view
drew without a line, which is the list of what is left to translate.

## 9. Making a theme

A theme is one JSON file. The three built-in themes (`crates/ui/themes/light.json`, `dark.json` and `synthwave.json`,
compiled in) and the themes of the user (the `themes` folder next to `settings.json`) go through the same loader
([DECISIONS.md](DECISIONS.md#x-themes-are-json-files) explains the choice). A theme that sets only a few colours is
fine: the others come from the theme it is `based_on`.

```json
{
  "version": 1,
  "name": "Night drive",
  "based_on": "dark",
  "dark": true,
  "accent_saturation": 0.85,
  "accent_lightness": 0.62,
  "corner": 3.0,
  "colors": {
    "background": "#140c28",
    "panel_bottom": "#22153f",
    "text": "#f3e9ff",
    "highlight": "#ff5db1",
    "grid_bar": "#ff2e9760"
  }
}
```

- `version` is 1. A file with another version is skipped and named in a message at start.
- `name` is up to 40 characters and is not the file name; two themes cannot share a name.
- `based_on` is `light` or `dark` (default `dark`). Every colour the file leaves out, and every number, comes from it.
- `dark`, `accent_saturation`, `accent_lightness` and `corner` are optional. `dark` says the type is light on dark
  surfaces; the two accent numbers (0 to 1) make the colour a device or track gets when nothing names one.
- Each entry of `colors` is `#rrggbb`, or `#rrggbbaa` for a colour that lets what is behind it show (a grid line, a
  shadow, a veil). A name the program does not know is ignored, so a newer file keeps working; a value that is not a
  colour makes the file unusable.

The names are the table `TOKENS` of `crates/ui/src/theme_doc.rs`, listed by group in the user manual. To add a token:
add the field to `Theme` (`crates/ui/src/theme.rs`), a row to `TOKENS` with its label, a value in each of the three
built-in files (a test refuses a built-in file that leaves one out), and a French line for its label in
`crates/ui/locales/fr.txt`. Paint code takes colours from the theme and never writes a colour of its own, so that a
theme can change it. The contrast test (`body_text_has_contrast_on_every_surface_of_every_builtin`) checks the body
text of every built-in theme against its panels, and the golden files `crates/ui/tests/golden/synthwave_*.json` and
`crates/app/tests/golden/*synthwave*.json` show what a theme paints.
