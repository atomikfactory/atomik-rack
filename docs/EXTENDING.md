# Writing a new device

This guide explains how to add a device to Atomik Rack using `atomik-device-sdk`. It describes the contract in prose and points to the code. The three built-in devices in [crates/devices/src](../crates/devices/src) are complete, working examples. Read one of them next to this page.

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
| `fn latency_frames(&self) -> u32` | any | no | Default 0. The engine does not compensate it yet. |

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

- Unconnected sockets are skipped with `is_connected`, so the output is bit-identical to the device without the input (the render goldens must not change); each device has a test that sets odd trims with no cable and compares bits.
- Per-sample targets (level, pan) read the CV sample by sample with no extra smoothing, so a stepped CV gives a stepped gain; pitch, resonance and cutoff are read at the 16-sample control rate with the filter coefficient ramped in between. A device that smooths per batch uses the batch mean of the CV.
- Instrument-wide targets (synth pitch, resonance, level, pan; sampler level; slicer cutoff and level; drum machine level and pitch) act on all voices alike; per-voice-state targets stay per voice (sampler resonance and pan).

## 4. Registering the device

Add one line to `register_all` in [crates/devices/src/lib.rs](../crates/devices/src/lib.rs), which registers each module's `factory()`. Registration validates the descriptor and rejects a duplicate `type_id`. The renderer's `default_registry()` ([crates/render/src/lib.rs](../crates/render/src/lib.rs)) calls `register_all`, so the new `type_id` becomes usable in patch files at once.

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
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.
