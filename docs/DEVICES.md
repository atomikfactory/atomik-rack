# Device reference

Atomik Rack ships twenty original devices: the poly synth, line mixer and stereo delay, the effects (reverb, compressor, EQ, distortion, modulation, mastering), the Character effect, the audio track, the sampler, the drum machine and the loop slicer, the note players (step sequencer and arpeggiator), the container and its internal note split, and the console devices `atomik.channel` and `atomik.master`. The tables below are taken from the static descriptors in [crates/devices/src](../crates/devices/src). Where this document and the code differ, the code wins.

Conventions used throughout:

- **Keys** are the stable names used in patch files (`"params": { "filter.cutoff": 1200 }`, `"from": "synth:out_l"`). Always refer to params and sockets by key. Ids follow the table order from 0, but that order is an implementation detail.
- **Plain values** are what you write in a patch. Out-of-range values are clamped with a warning. **Discrete** params (choices, toggles, counts) are rounded to the nearest integer.
- **Choice** params take the index of the choice, starting at 0. **Toggle** params are 0 (off) or 1 (on).
- **Taper** is the curve used to map a 0..1 control to the range (for a future user interface). *Exp* is exponential, and *dB* is linear in decibels with the minimum meaning silence.
- **Audio** is nominally ±1.0. **CV** is nominally −1..+1. Stereo is a pair of mono sockets `*_l` and `*_r`.

---

## Poly Synth: `atomik.poly-synth` (Instrument)

A subtractive synthesizer with up to 16 voices. Each voice has two band-limited oscillators plus noise, a 12 dB/oct state-variable filter, an amp envelope and a filter envelope. One global LFO modulates pitch and cutoff. Source: [crates/devices/src/poly_synth.rs](../crates/devices/src/poly_synth.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `notes_in` | Notes | in | Note on, note off and all-notes-off |
| `cutoff_cv_in` | CV | in | Clamped to −1..+1. +1 raises the cutoff by `cv.cutoff_amount` octaves. Reads 0 when unconnected. |
| `pitch_cv_in` | CV | in | Clamped to −1..+1. +1 raises the pitch of every voice by `cv.pitch_amount` semitones. |
| `resonance_cv_in` | CV | in | Clamped to −1..+1. Adds `cv.resonance_amount` × CV to the normalised resonance (result clamped to 0..1). |
| `level_cv_in` | CV | in | Clamped to −1..+1. Scales the output by `cv.level_amount` × CV decibels. |
| `pan_cv_in` | CV | in | Clamped to −1..+1. Balances the output by `cv.pan_amount` × CV (−1 = left, +1 = right). |
| `out_l` | Audio | out | Left output |
| `out_r` | Audio | out | Right output: identical to `out_l` unless `pan_cv_in` is cabled (see below) |
| `lfo_cv_out` | CV | out | The global LFO, bipolar −1..+1, written every sample |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `osc1.wave` | 0–3: saw, square, triangle, sine | 0 (saw) | choice | Linear | yes |
| `osc2.wave` | 0–3: saw, square, triangle, sine | 1 (square) | choice | Linear | yes |
| `osc1.octave` | −3 to 3 | 0 | octaves | Linear | yes |
| `osc2.octave` | −3 to 3 | 0 | octaves | Linear | yes |
| `osc1.semi` | −12 to 12 | 0 | semitones | Linear | yes |
| `osc2.semi` | −12 to 12 | 0 | semitones | Linear | yes |
| `osc1.fine` | −50 to 50 | 0 | cents | Linear | no |
| `osc2.fine` | −50 to 50 | 7 | cents | Linear | no |
| `osc.mix` | 0 to 1 (0 = osc 1 only) | 0.5 | percent | Linear | no |
| `noise.level` | 0 to 1 | 0 | percent | Linear | no |
| `filter.mode` | 0–2: LP12, BP12, HP12 | 0 (LP12) | choice | Linear | yes |
| `filter.cutoff` | 20 to 20 000 | 4000 | Hz | Exp | no |
| `filter.resonance` | 0 to 1 | 0.2 | percent | Linear | no |
| `filter.env_amount` | −1 to 1 (±6 octaves) | 0.3 | none | Linear | no |
| `filter.key_track` | 0 to 1 | 0.5 | percent | Linear | no |
| `env.amp.attack` | 0.5 to 10 000 | 2 | ms | Exp | no |
| `env.amp.decay` | 0.5 to 10 000 | 200 | ms | Exp | no |
| `env.amp.release` | 0.5 to 10 000 | 300 | ms | Exp | no |
| `env.amp.sustain` | 0 to 1 | 0.7 | percent | Linear | no |
| `env.filter.attack` | 0.5 to 10 000 | 2 | ms | Exp | no |
| `env.filter.decay` | 0.5 to 10 000 | 400 | ms | Exp | no |
| `env.filter.release` | 0.5 to 10 000 | 300 | ms | Exp | no |
| `env.filter.sustain` | 0 to 1 | 0 | percent | Linear | no |
| `lfo.wave` | 0–4: sine, triangle, saw, square, sample&hold | 0 (sine) | choice | Linear | yes |
| `lfo.rate` | 0.05 to 30 | 2 | Hz | Exp | no |
| `lfo.to_pitch` | 0 to 12 | 0 | semitones | Linear | no |
| `lfo.to_cutoff` | 0 to 4 | 0 | octaves | Linear | no |
| `velocity.amp` | 0 to 1 | 0.5 | percent | Linear | no |
| `cv.cutoff_amount` | 0 to 4 | 2 | octaves | Linear | no |
| `voices` | 1 to 16 | 16 | none | Linear | yes |
| `master.level` | −60 to 6 (−60 = silent) | −6 | dB | dB | no |
| `cv.pitch_amount` | −24 to 24 | 12 | semitones | Linear | no |
| `cv.resonance_amount` | −1 to 1 | 0.5 | percent | Linear | no |
| `cv.level_amount` | −24 to 24 | 12 | dB | Linear | no |
| `cv.pan_amount` | −1 to 1 | 1 | none | Linear | no |

### Behaviour (as built)

- **Voices.** A voice goes Idle → Attack → Decay → Sustain → Release → Idle. A note-on for a key that is already sounding retriggers the same voice from its current level.
- **Voice stealing.** When every allowed voice is busy, the oldest started voice is stolen. It fades out linearly over 2 ms, and the new note starts after the fade. `AllOff` releases every voice.
- **Velocity.** Amp gain = envelope × (1 − `velocity.amp` × (1 − velocity)).
- **Filter.**
  - Cutoff = `filter.cutoff` × 2^(`env_amount`·6·filter env + `key_track`·(key − 60)/12 + `lfo.to_cutoff`·LFO + `cv.cutoff_amount`·CV). It is clamped to 20 Hz … 0.45 × sample rate.
  - Cutoff and pitch are updated every 16 samples per voice, with the filter coefficient interpolated in between.
  - Resonance maps to Q = 0.5·40^r (Q = 20 at r = 1). The filter stays stable at full resonance and does not self-oscillate into blow-up.
- **Output level.**
  - The output is the sum of voices × 0.5 × the master gain (smoothed over 5 ms).
  - **Soft limiter:** the signal then passes through a limiter that is linear up to ±2 and saturates smoothly towards ±4. Output is always finite and |x| < 4, whatever the params.
- **Mono output.** `out_l` **equals** `out_r` while `pan_cv_in` is not cabled. There is no stereo spread in this version.
- **CV inputs (pitch, resonance, level, pan).** Each is applied only while its socket is cabled; with all of them open the output is bit for bit what it was before they existed. Every CV sample is clamped to −1..+1 and multiplied by the signed amount param (a negative amount inverts the CV). All four act on the whole instrument, not per voice.
  - *Pitch:* semitones, added to every oscillator of every voice (key + offsets + LFO·`lfo.to_pitch` + `cv.pitch_amount`·CV), read at the 16-sample control rate like the cutoff.
  - *Resonance:* the normalised resonance becomes clamp(`filter.resonance` + `cv.resonance_amount`·CV, 0, 1); the filter damping ramps over each 16-sample control step.
  - *Level:* the master gain is multiplied per sample by 10^(`cv.level_amount`·CV/20), before the soft limiter. There is no extra smoothing, so a stepped CV gives a stepped gain.
  - *Pan:* per sample, the mono sum is balanced by `cv.pan_amount`·CV after the limiter: unity on both sides at the centre, the far side falls as cos(|pan|·π/2), the near side stays at 1.
- **Randomness.** The noise and the sample & hold LFO are seeded from the patch seed and the device instance, so renders are reproducible.

---

## Line Mixer: `atomik.line-mixer` (Mixer)

Six stereo channels into a stereo master. Source: [crates/devices/src/line_mixer.rs](../crates/devices/src/line_mixer.rs).

### Sockets

For each channel *n* = 1 to 6, in this order:

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `ch{n}_in_l` | Audio | in | Left input |
| `ch{n}_in_r` | Audio | in | Right input |
| `ch{n}_level_cv_in` | CV | in | When connected, multiplies the channel gain by the CV clamped to 0..1. Unconnected means gain × 1. |

Then:

| Key | Kind | Direction |
|---|---|---|
| `master_l` | Audio | out |
| `master_r` | Audio | out |

### Params

For each channel *n* = 1 to 6, in this order:

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `ch{n}.level` | −60 to 6 (−60 = silent) | 0 | dB | dB | no |
| `ch{n}.pan` | −1 (left) to 1 (right) | 0 | none | Linear | no |
| `ch{n}.mute` | 0 or 1 | 0 | toggle | Linear | yes |
| `ch{n}.solo` | 0 or 1 | 0 | toggle | Linear | yes |

Then:

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `master.level` | −60 to 6 (−60 = silent) | 0 | dB | dB | no |

### Behaviour (as built)

- **Pan law.** Pan is constant power with −3 dB at the centre: gain L = cos θ and gain R = sin θ, with θ = (pan + 1)·π/4. Stereo channels use the same law: left × cos θ, right × sin θ.
- **Mono channels.** If only one input of a channel is connected, the channel is mono and that input feeds both sides before panning. This applies to `_l` only (as specified) and also to `_r` only (an as-built extension).
- **Solo and mute.** If any channel is soloed, only soloed, un-muted channels are heard. Mute always silences the channel.
- **Smoothing.** Channel and master gain changes are smoothed over 5 ms.

---

## Stereo Delay: `atomik.stereo-delay` (Effect)

A stereo delay with free or tempo-synced time, feedback, dry/wet mix and ping-pong. Source: [crates/devices/src/stereo_delay.rs](../crates/devices/src/stereo_delay.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `in_l` | Audio | in | |
| `in_r` | Audio | in | |
| `out_l` | Audio | out | |
| `out_r` | Audio | out | |
| `time_cv_in` | CV | in | When connected: clamped to −1..+1, and the delay time is multiplied by 2^CV (then clamped to 1–4000 ms) |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `time.mode` | 0–1: ms, sync | 1 (sync) | choice | Linear | yes |
| `time.ms` | 1 to 4000 | 375 | ms | Exp | no |
| `time.sync` | 0–11: 1/32, 1/16T, 1/16, 1/16D, 1/8T, 1/8, 1/8D, 1/4T, 1/4, 1/4D, 1/2, 1/1 | 6 (1/8D) | choice | Linear | yes |
| `feedback` | 0 to 0.95 | 0.4 | percent | Linear | no |
| `mix` | 0 (dry) to 1 (wet) | 0.3 | percent | Linear | no |
| `ping_pong` | 0 or 1 | 0 | toggle | Linear | yes |

### Behaviour (as built)

- **Delay time.** The time is `time.ms`, or the `time.sync` note value at the patch tempo (1/4 at 120 BPM = 0.5 s). It is then scaled by the time CV and clamped to 1–4000 ms.
- **Time changes.** A change glides the read position with interpolated reads. Small changes take 50 ms. The glide speed is capped (the read head stays between standstill and 2× speed), so large jumps take longer, about 1 s per second of change. They are heard as a bounded pitch sweep and never click.
- **Feedback and mix.** Both are smoothed over 5 ms. Output = dry × (1 − mix) + wet × mix.
- **First batch.** The first batch after `prepare` or `reset` starts at the target values with no glide.
- **Ping-pong.** The input is summed to mono, **(L + R) / 2, into the left line**, and the feedback crosses L → R → L. The echoes therefore alternate sides and the stereo image of the input is not kept. When ping-pong is off, the two lines are independent.


---

## Reverb: `atomik.reverb` (Effect)

A stereo algorithmic reverb with three algorithms. Source: [crates/devices/src/reverb.rs](../crates/devices/src/reverb.rs), building blocks in [crates/dsp/src](../crates/dsp/src) (`fdn`, `mod_delay`, `allpass`, `parabolic_lfo`, `block_smoother`, `det_math`).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `in_l` | Audio | in | |
| `in_r` | Audio | in | |
| `out_l` | Audio | out | |
| `out_r` | Audio | out | |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `algorithm` | 0–2: room, hall, plate | 1 (hall) | choice | Linear | yes |
| `predelay` | 0 to 250 | 20 | ms | Linear | no |
| `size` | 0 to 1 | 0.5 | percent | Linear | no |
| `decay` | 0.2 to 20 (seconds) | 2 | none | Exp | no |
| `damping` | 0 to 1 | 0.4 | percent | Linear | no |
| `lowcut` | 20 to 1000 | 80 | Hz | Exp | no |
| `early` | 0 to 1 (0 = late only, 0.5 = balanced, 1 = early only) | 0.5 | percent | Linear | no |
| `width` | 0 (mono) to 1 | 1 | percent | Linear | no |
| `modulation` | 0 to 1 | 0.5 | percent | Linear | no |
| `mix` | 0 (dry) to 1 (wet) | 0.3 | percent | Linear | no |
| `freeze` | 0 or 1 | 0 | toggle | Linear | yes |
| `output` | −24 to 12 | 0 | dB | Linear | no |

### Behaviour (as built)

- **Structure.** Pre-delay, a one-pole low cut, six early-reflection taps per side from a tapped delay, and a late tail from an 8-line feedback delay network (Hermite-interpolated delay lines, a damping lowpass, a feedback gain and an in-loop allpass per line, mixed by an orthonormal 8×8 Hadamard matrix). Two allpass diffusers per side feed the network. The left and right tails are taken with two orthogonal Hadamard sign rows, so they are uncorrelated at `width` 1; `width` 0 collapses the wet signal to mono.
- **Algorithms.** *Room*: lines 17–44 ms, dense early taps (3–30 ms), brighter damping range (14 kHz at `damping` 0), little modulation. *Hall*: lines 33–94 ms, late and widely spaced early taps (7–62 ms), darker (10 kHz), full modulation. *Plate*: lines 11–44 ms, very dense, weak early taps (2–17 ms), brightest (16 kHz), most modulation. All delay times are multiplied by `0.25 + 1.5 × size`.
- **Decay.** Line *i* has loop time T_i = d_i + a_i samples (its delay plus its allpass delay) and gain g_i = 10^(−3 · 0.93 · T_i / (sr · `decay`)). Every line therefore loses 60 dB in about `decay` seconds, and so does the whole tail; 0.93 is an empirical factor found by measuring the impulse response. The damping lowpass has unity gain at DC: `decay` is the low and mid frequency RT60, and `damping` shortens the high end. A level compensation of (2 / `decay`)^¼ on the late signal keeps the wet level within a few dB over the decay range.
- **Damping.** Cutoff = `fc_top` × 2^(−6 × `damping`); at `damping` 0 the filter is bypassed.
- **Modulation.** Each line has its own parabolic LFO (0.2–1.3 Hz) with a start phase and rate taken from the device seed; the delay excursion is at most about 0.5 ms. It breaks up metallic ringing and is fully deterministic (no libm in the per-sample path).
- **Smoothing and clicks.** All controls are smoothed per batch and ramped across it. Line lengths, early tap times and tap gains use 250 ms / 100 ms smoothing, so changing `size` or `algorithm` bends the delays slowly (a gentle pitch glide of the tail) and never clicks. Decay, damping, low cut, width, mix, output and balance use 20–60 ms. The first batch after `prepare` or `reset` starts at the target values.
- **Freeze.** Fades over 25 ms: feedback gains go to 1, damping is bypassed and the input is muted, so the tail's energy circulates unchanged (about 3 dB lost over 8 s, from interpolation). Unfreezing lets it decay again.
- **Mix and latency.** Output = dry × (1 − `mix`) + wet × `mix` × output gain. The dry signal is never delayed and `output` applies to the wet signal only; at `mix` 0 the input is copied bit for bit. Latency is 0. `predelay` delays only the wet signal.
- **Safety.** Feedback state flushes values below 1e-20 to exactly 0 (no denormals; a decayed tail is exactly 0). Non-finite input samples are treated as 0 on the wet path. The wet signal is clamped to ±32.
- **Meter values.** `[output peak L, output peak R, tail RMS]`, linear, of the last batch.
- **Buffers** are allocated in `prepare` for the largest size at the sample rate (about 0.17 s per FDN line, 0.25 s for the pre-delay).

---

## Compressor: `atomik.compressor` (Effect)

A stereo feed-forward compressor with a sidechain input. Source: [crates/devices/src/compressor.rs](../crates/devices/src/compressor.rs); the static curve is `atomik_dsp::static_gain_reduction_knee_db`, the level maths use `log2_det` / `exp2_det` (no libm per sample).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `in_l`, `in_r` | Audio | in | |
| `sc_in_l`, `sc_in_r` | Audio | in | Sidechain key; used when `sc_enable` is on and at least one is cabled (a lone side feeds both channels) |
| `out_l`, `out_r` | Audio | out | |
| `gr_cv_out` | Cv | out | `1 - g`, `g` the smaller channel gain before makeup and mix (0 at rest; the console's convention) |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `threshold` | -60 to 0 | -18 | dB | Linear | no |
| `ratio` | 1 to 20 | 4 | none | Exp | no |
| `limit` | 0 or 1 (infinite ratio) | 0 | toggle | Linear | yes |
| `knee` | 0 to 24 (soft knee width, centred on the threshold) | 6 | dB | Linear | no |
| `attack` | 0.1 to 200 | 10 | ms | Exp | no |
| `release` | 5 to 2000 | 150 | ms | Exp | no |
| `auto_release` | 0 or 1 | 0 | toggle | Linear | yes |
| `lookahead` | 0 to 5 | 0 | ms | Linear | no |
| `makeup` | 0 to 24 | 0 | dB | Linear | no |
| `auto_makeup` | 0 or 1 | 0 | toggle | Linear | yes |
| `mix` | 0 (dry) to 1 (compressed) | 1 | percent | Linear | no |
| `sc_enable` | 0 or 1 | 0 | toggle | Linear | yes |
| `sc_hp` | 20 to 1000 (20 = off) | 20 | Hz | Exp | no |
| `sc_lp` | 1000 to 20000 (20000 = off) | 20000 | Hz | Exp | no |
| `stereo_link` | 0 (independent) to 1 (linked) | 1 | percent | Linear | no |
| `detector` | 0 peak, 1 RMS | 0 | choice | Linear | yes |

### Behaviour (as built)

- **Detector.** *Peak*: `|x|` per channel. *RMS*: one-pole mean square (10 ms), sine-calibrated so a sine of peak A reads A in both modes. The key is first filtered by a 2nd-order Butterworth high-pass (`sc_hp`) and low-pass (`sc_lp`), whatever the key source.
- **Stereo link.** Channel level in dB `l_c`; the detector level is `l_c + link × (max(l_L, l_R) − l_c)`. With `stereo_link` 1 both channels get the same reduction; with 0 they are independent.
- **Gain computer.** Soft knee of width `knee`: no reduction below `T − W/2`, a quadratic `(1 − 1/R)(x − T + W/2)² / (2W)` inside, `(1 − 1/R)(x − T)` above. `limit` makes the slope 1 (an infinite ratio). Measured against this analytic curve on 1 kHz sine steps (peak and RMS detector, 2 thresholds × 3 ratios incl. limit × 3 knees × 9 levels): worst error 0.03 dB.
- **Ballistics, in dB.** Release with peak hold `y1 = max(c, a_r·y1 + (1 − a_r)·c)`, then attack `gr = a_a·gr + (1 − a_a)·y1` while `y1 > gr`, else `gr = y1`, with `a = exp(−1/(τ·sr))`. **The attack and release times are one-pole time constants**: attack is the time for the gain reduction to reach 63.2 % of a step, release the time to fall to 36.8 % of its starting value. Measured at 5/100, 20/300 and 50/800 ms: within 0.1 % of the setting (the RMS detector adds its own 10 ms window to the attack).
- **Auto release.** `release` becomes the slow constant; the fast one is `max(release / 8, 2 ms)`. A fast peak-hold stage and a slow stage (fed by an integrated GR that follows the fast stage with a `release / 2` constant on the way up and at once on the way down) run in parallel and are combined with `max`: short peaks release fast, sustained compression slowly (measured with release 800 ms: the GR halves in 69 ms after a 30 ms burst, in 657 ms after 2 s of compression).
- **Lookahead and latency.** The audio path and the dry path are delayed by `round(lookahead × sr / 1000)` samples (at most 5 ms; the buffer is allocated in `prepare`) while the detector sees the undelayed key. `latency_frames()` is that **current** value (dynamic latency): the engine publishes device latencies after every batch boundary's parameter changes and bumps `latency_epoch`, so a host that recompiles on the epoch re-aligns PDC after the knob moves. Moving `lookahead` changes the delay at once (a click on programme material, as with any delay time). The tests check the impulse delay at 44.1, 48 and 96 kHz.
- **Makeup.** `makeup` plus, with `auto_makeup`, the reduction a 0 dBFS input would get with the current threshold, ratio and knee; the total is capped at 36 dB and smoothed per batch.
- **Mix.** `out = dry × (1 − mix) + wet × mix` with the dry path delayed by the lookahead: `mix` 0 is the (delayed) input bit for bit.
- **Meter values** (3): input peak (linear, L/R max), output peak (linear), gain reduction in dB (largest of either channel in the batch).
- **Safety.** Non-finite input is treated as 0; `f64` state with denormal flushes.

---

## EQ: `atomik.eq` (Effect)

A six-band stereo parametric EQ with an analyser feed. Source: [crates/devices/src/eq.rs](../crates/devices/src/eq.rs), filters from `atomik_dsp::SmoothedBiquad`.

### Sockets

`in_l`, `in_r` (Audio in), `out_l`, `out_r` (Audio out).

### Params

| Key | Range | Default | Unit | Discrete |
|---|---|---|---|---|
| `band1.on` / `.freq` / `.q` (low cut) | 0 or 1 / 20 to 2000 Hz / 0.3 to 4 | 0 / 80 / 0.707 | toggle / Hz / none | on: yes |
| `lowcut.slope` | 0 to 2: 12, 24, 48 dB/oct | 0 | choice | yes |
| `band2.on` / `.freq` / `.gain` / `.q` (low shelf) | 0 or 1 / 20 to 5000 Hz / −24 to 24 dB / 0.3 to 2 | 1 / 120 / 0 / 0.707 | toggle / Hz / dB / none | on: yes |
| `band3.on` / `.freq` / `.gain` / `.q` (peak 1) | 0 or 1 / 20 to 20000 Hz / −24 to 24 dB / 0.1 to 18 | 1 / 500 / 0 / 1 | toggle / Hz / dB / none | on: yes |
| `band4.on` / `.freq` / `.gain` / `.q` (peak 2) | 0 or 1 / 20 to 20000 Hz / −24 to 24 dB / 0.1 to 18 | 1 / 2500 / 0 / 1 | toggle / Hz / dB / none | on: yes |
| `band5.on` / `.freq` / `.gain` / `.q` (high shelf) | 0 or 1 / 500 to 20000 Hz / −24 to 24 dB / 0.3 to 2 | 1 / 8000 / 0 / 0.707 | toggle / Hz / dB / none | on: yes |
| `band6.on` / `.freq` / `.q` (high cut) | 0 or 1 / 1000 to 20000 Hz / 0.3 to 4 | 0 / 16000 / 0.707 | toggle / Hz / none | on: yes |
| `highcut.slope` | 0 to 2: 12, 24, 48 dB/oct | 0 | choice | yes |
| `output` | −24 to 24 | 0 | dB | no |

Frequencies and Q use the exponential taper, gains and `output` the linear one. The bands run in the order 1 to 6; the param ids are in that order too (`band1.on` is 0, `output` is 24).

### Behaviour (as built)

- **Filters.** RBJ cookbook sections with `f64` coefficients and state. Frequency (log₂ Hz), Q (ln Q) and gain (dB) are smoothed with a 10 ms one-pole and the coefficients are recomputed every 16 samples while a parameter moves (the parameters are interpolated, so sweeps do not zipper). Measured against the RBJ response (7 shelf and peak settings × 10 frequencies) the sine-sweep error is below 0.001 dB; the cut filters match the closed-form Butterworth response (bilinear-warped) to the same accuracy.
- **Cut slopes.** 12 dB/oct is one 2nd-order section (Q = `q`), 24 dB/oct a 4th-order Butterworth (Q 0.5412 and 1.3066), 48 dB/oct an 8th-order one (Q 0.5098, 0.6013, 0.8999, 2.5629). `q` of a cut scales the highest-Q section by `q / 0.7071`, so the default is the exact Butterworth (−3.01 dB at the corner) and a larger `q` adds resonance (at 12 dB/oct the gain at the corner is `q`). Measured slopes 12.0 / 24.1 / 48.2 dB/oct (low cut). A slope change fades the band out (3 ms), rebuilds it and fades it in (10 ms) so it does not click.
- **Off bands are bit-transparent.** Every band has a bypass weight (10 ms fade, snapping to exactly 0 after about 100 ms). A band that is off, and a shelf or peak with exactly 0 dB gain, is skipped, so with every band off and `output` 0 dB the output equals the input bit for bit (also the default state).
- **Analyser.** The mono mix of the output feeds a fixed bank of 16 RBJ band-pass biquads (0 dB peak gain, Q 2.485) with centres log-spaced from 40 Hz to 16 kHz (ratio 1.4915), designed in `prepare`. It runs at the full sample rate on the audio thread (16 `f64` biquads per sample; no FFT, no decimation) and the mean square of every band is taken per batch and smoothed with a 50 ms one-pole. `meter_values` (18): `[0..16)` band energies (linear power; a sine of peak A at a band centre reads about A²/2, use 10·log10 for dB), `[16]` output peak L, `[17]` output peak R. `atomik_devices::eq::analyser_centres(sr)` gives the centre frequencies.
- **Latency** 0. Non-finite input is treated as 0.

---

## Distortion: `atomik.distortion` (Effect)

Oversampled saturation, waveshaping and bitcrushing. Source: [crates/devices/src/distortion.rs](../crates/devices/src/distortion.rs), oversampler in [crates/dsp/src/oversample.rs](../crates/dsp/src/oversample.rs).

### Sockets

`in_l`, `in_r` (Audio in), `out_l`, `out_r` (Audio out).

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `curve` | 0–4: soft clip, hard clip, tube, foldback, bitcrush | 0 | choice | Linear | yes |
| `drive` | 0 to 48 | 12 | dB | Linear | no |
| `bias` | −1 to 1 | 0 | none | Linear | no |
| `tone` | −1 (dark) to 1 (bright), tilt of ±6 dB around 1 kHz | 0 | none | Linear | no |
| `output` | −24 to 12 (wet signal only) | 0 | dB | Linear | no |
| `mix` | 0 (dry) to 1 (wet) | 1 | percent | Linear | no |
| `crush.bits` | 1 to 16 | 8 | none | Linear | yes |
| `crush.rate` | 500 to 96000 (hold rate; at or above the sample rate no hold) | 96000 | Hz | Exp | no |
| `oversample` | 0–2: off, 2x, 4x | 2 | choice | Linear | yes |

### Behaviour (as built)

- **Curves.** Soft clip `u/√(1+u²)`; hard clip `clamp(u, ±1)`; tube: `u/√(1+u²)` for `u ≥ 0` and `u/√(1+(1.25u)²)` below (the negative ceiling is −0.8, so it makes even harmonics: 2nd harmonic −30 dBc at +18 dB drive against −197 dBc for the soft clip); foldback `|((u+3) mod 4) − 2| − 1` (identity inside ±1, reflects at ±1); bitcrush `round(clamp(u) × 2^(bits−1)) / 2^(bits−1)` followed by a sample-and-hold at `crush.rate`. `u = x × drive + bias`; the curve value at zero input is subtracted so a bias is not a step, and a DC blocker (8 Hz one-pole high-pass) always follows.
- **Oversampling.** The three nonlinear curves run between the halfband stages (see below). **Bitcrush and its sample-and-hold run at the base rate, without oversampling** (its aliasing is the effect); the signal still goes through the oversampler with an identity curve so the timing is identical for every curve.
- **Alias reduction** (5 kHz sine at 48 kHz, +24 dB drive on a 0.5 amplitude, alias power measured from 20 Hz to 19.2 kHz excluding the tone's harmonics): hard clip −14.2 dBc off, −29.2 dBc at 2x (15.0 dB lower), **−39.0 dBc at 4x (24.7 dB lower)**; soft clip 38.7 dB lower and tube 35.3 dB lower at 4x; foldback 22.5 dB lower; at +12 dB drive soft clip is 74 dB lower and tube 67 dB lower. A hard clip keeps a 1/k² harmonic tail, which limits what oversampling can do.
- **Tone** is a tilt: low shelf `−6 × tone` dB and high shelf `+6 × tone` dB at 1 kHz (RBJ, Q 0.707, smoothed); skipped at exactly 0.
- **Mix and latency (dynamic).** `latency_frames()` is the oversampler's latency for the current `oversample`: 0 (off), 29 (2x) or 35 samples at every sample rate, independent of the curve; the engine republishes it after parameter changes and bumps `latency_epoch`. Switching the setting clears the oversampler filters (a short transient). The dry path is delayed by the same latency, so `mix` 0 is the **input delayed by the latency, bit for bit** (the input itself with oversampling off). Drive, bias, output and mix are smoothed per batch (20 ms).
- **Meter values** (2): input peak, output peak (linear, last batch). Non-finite input is treated as 0; the curves are bounded by 1, so the output stays bounded at any drive.

### Oversampler (`atomik_dsp::oversample`)

Mono 1x / 2x / 4x polyphase halfband oversampler, shared with the planned limiter. Linear-phase FIR halfband stages (centre tap 0.5, Kaiser-windowed sinc with the coefficients computed in `new` from IEEE `+ − × ÷ sqrt` only, so they are identical on every platform): stage 1 (base ⇄ 2x) 59 taps, stage 2 (2x ⇄ 4x) 23 taps padded to 25 so the delay is a whole number of base samples. Allocation-free after `Oversampler::new(max_block)`. Latency (up and down, base samples): 2x **29**, 4x **35**, 1x 0 (measured as the impulse delay; the response is symmetric). Measured: passband deviation below 0.0005 dB (2x) and 0.0011 dB up to 0.4 fs; image and alias rejection at least 90 dB for the 2x chain and **84.7 dB for the 4x chain** (the transition band is 0.4–0.6 fs).

---

## Modulation: `atomik.modulation` (Effect)

Chorus, flanger and phaser with tempo sync. Source: [crates/devices/src/modulation.rs](../crates/devices/src/modulation.rs).

### Sockets

`in_l`, `in_r` (Audio in), `out_l`, `out_r` (Audio out).

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `mode` | 0–2: chorus, flanger, phaser | 0 | choice | Linear | yes |
| `rate` | 0.02 to 20 (free-running LFO) | 0.5 | Hz | Exp | no |
| `sync` | 0–7: off, 1/16, 1/8, 1/4, 1/2, 1 bar, 2 bars, 4 bars (4/4) | 0 | choice | Linear | yes |
| `depth` | 0 to 1 | 0.5 | percent | Linear | no |
| `feedback` | −0.95 to 0.95 | 0 | none | Linear | no |
| `delay` | 0.1 to 30 (chorus and flanger base delay; unused by the phaser) | 7 | ms | Exp | no |
| `stages` | 2 to 12, even (phaser; odd values round to the next even number) | 4 | none | Linear | yes |
| `spread` | 0 to 180 (right LFO phase offset, degrees) | 90 | none | Linear | no |
| `mix` | 0 (dry) to 1 (wet) | 0.5 | percent | Linear | no |
| `output` | −24 to 12 | 0 | dB | Linear | no |

### Behaviour (as built)

- **Chorus.** Three Hermite-interpolated modulated delays per side (`atomik_dsp::ModDelay`), base delays `delay × [1.0, 1.27, 0.79]`, LFO phases offset by a third of a cycle, instantaneous delay `base × (1 + 0.5 × depth × lfo)`; the voices are averaged and `feedback` feeds the average back.
- **Flanger.** One delay per side, `delay × (1 + 0.9 × depth × lfo)`, positive or negative feedback. The line reads before it writes, so the delay is exact (a 2 ms flanger at depth 0 returns an impulse at exactly 96 samples at 48 kHz).
- **Phaser.** `stages` first-order allpasses `(a + z⁻¹)/(1 + a z⁻¹)` in `f64`; the corner sweeps `1 kHz × 2^(3 × depth × lfo)` (125 Hz to 8 kHz at full depth) with `tan` from a [5/4] Padé approximation (error below 1e-5 up to 0.7 rad, so no libm in the per-sample path). Feedback takes the last stage's previous output (one sample of delay in the loop). With depth 0 and no feedback the magnitude is flat within 0.01 dB at any frequency.
- **LFO.** The parabolic sine-like wave of `atomik_dsp::ParabolicLfo` (`4u(1 − |u|)`), no libm. The right channel runs `spread/360` cycles ahead.
- **Tempo sync (documented choice).** With `sync` not off and the transport **playing**, the LFO phase is `frac(frame / (beats × sr × 60 / bpm))` computed in `f64` from the absolute frame of each sample, so it depends only on the frame index and the tempo, never on the batch layout; the period is exact (the phase matches the closed form to 1e-9 for all seven divisions at 44.1, 48 and 96 kHz, and a device started at a later frame matches one that ran from 0 bit for bit). A tempo change moves the phase. With the transport **stopped** the LFO free-runs at the synced rate from its last phase (the frame is ignored). With `sync` off it free-runs at `rate`.
- **Safety and latency.** Latency 0. `mix` 0 is the input bit for bit. `mix`, `output`, `depth`, `feedback` and the delay time are smoothed per batch (20 ms); a `mode` change clears the lines. The feedback signal is clamped to ±8 and state is flushed below 1e-20: at feedback ±0.95, depth 1, 12 stages and 30 s of full-scale noise the output stays finite and below 25 (all three modes), and the feedback tails decay to silence after the input stops. Delay times never leave the allocated line (at most 1.9 × `delay` for the flanger, 1.905 × `delay` for the chorus; the line holds 57.2 ms plus margin).

---

## Mastering: `atomik.mastering` (Effect)

Multiband compressor, stereo width, true-peak limiter and a BS.1770 loudness meter in one device. Source: [crates/devices/src/mastering.rs](../crates/devices/src/mastering.rs), with `mastering/limiter.rs` and `mastering/loudness.rs`.

### Sockets

`in_l`, `in_r` (Audio in), `out_l`, `out_r` (Audio out), `gr_cv_out` (CV out: `1 − g`, `g` the limiter gain times the smallest band compression gain).

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `input` | −24 to 24 | 0 | dB | Linear | no |
| `multiband.on` | 0 or 1 (0 bypasses crossovers and band compressors) | 1 | toggle | Linear | yes |
| `xover.low` | 40 to 2000 | 200 | Hz | Exp | no |
| `xover.high` | 500 to 12000 (effective value `max(xover.high, 1.25 × xover.low)`) | 3000 | Hz | Exp | no |
| `band1.threshold`, `band2.threshold`, `band3.threshold` | −60 to 0 | −18 | dB | Linear | no |
| `bandN.ratio` | 1 to 20 | 2 | none | Exp | no |
| `bandN.attack` | 0.5 to 200 | 30 / 15 / 5 (low / mid / high) | ms | Exp | no |
| `bandN.release` | 10 to 2000 | 300 / 200 / 100 | ms | Exp | no |
| `bandN.gain` | −12 to 12 (static band gain, after the compressor) | 0 | dB | Linear | no |
| `bandN.on` | 0 or 1 (0 bypasses the compression of the band; its gain still applies) | 1 | toggle | Linear | yes |
| `width` | 0 to 2 (0 to 200 %) | 1 | percent | Linear | no |
| `mono_below` | 0 (off) to 500: high-pass on the side signal | 0 | Hz | Linear | no |
| `ceiling` | −12 to 0 (true-peak ceiling of the limiter) | −1 | dB (dBTP) | Linear | no |
| `release` | 10 to 2000 (limiter) | 150 | ms | Exp | no |
| `output` | −24 to 12 (after the limiter) | 0 | dB | Linear | no |
| `meter.reset` | momentary toggle: a rising edge clears the loudness and true-peak meters | 0 | toggle | Linear | yes |

The param ids run in this order (`input` 0 … `meter.reset` 27); the band params are grouped per band (threshold, ratio, attack, release, gain, on).

### Behaviour (as built)

- **Chain.** input gain → multiband → width → limiter → output gain; the loudness and true-peak meter read the output. The limiter lookahead is a fixed 1.5 ms (no parameter; the latency is therefore constant).
- **Crossovers.** Two Linkwitz-Riley 4th-order crossovers (two cascaded Butterworth 2nd-order sections per branch). The low crossover splits the input into `L1`/`H1`, the high crossover splits `H1` into mid and high, and `L1` is passed through the high crossover's own low-pass plus high-pass (an allpass), so the bands sum to `AP_high(AP_low(x))`: **flat magnitude** (the measured deviation of the sum, ratio 1, over 16 frequencies from 30 Hz to 17 kHz and four crossover settings, including a swapped high < low setting, is below 1e-6 dB) with the **phase of two cascaded LR4 allpasses** (an impulse is smeared). The low crossover has priority: the effective high frequency is `max(xover.high, 1.25 × xover.low)`, capped at 0.45 sr. Crossover changes are not smoothed (the biquad state is continuous, but a sweep during playback can click).
- **Band compressors.** Per band the peak `max(|L|, |R|)` of the band signal (stereo-linked) drives the gain computer `atomik_dsp::static_gain_reduction_knee_db` (6 dB soft knee) and the same ballistics as `atomik.compressor` (release with peak hold, then attack; one-pole time constants of the GR in dB; the formulas are repeated in the device, the compressor itself is unchanged). `bandN.gain` is smoothed per batch.
- **Width.** `m = (l + r)/2`, `s = (l − r)/2 × width`, `l = m + s`, `r = m − s`. Width 0 gives bit-identical channels; **width 100 % with `mono_below` off skips the matrix, so the signal is bit-exact** (with the multiband off, the whole device is then a bit-exact delay as long as the limiter does not act). `mono_below` above 0 puts an LR4 high-pass (−6 dB at the corner, −24 dB one octave below, −48 dB two octaves below) on the side signal only: mono content is untouched, side content below the frequency disappears.
- **Limiter.** 4x oversampled true-peak detection per channel (`atomik_dsp::oversample` halfband stages, up path only, plus a parabolic refinement of local maxima capped at +10 %), channels linked. Required gain `min(1, ceiling / peak)`; a sliding minimum over `LA + 2` samples (`LA` = 1.5 ms) averaged over `LA` samples gives a linear attack ramp that reaches the required gain when the peak leaves the delay line; the release is an exponential with the `release` time constant (`env ≤ g`, so the guarantee holds), instant when the required gain falls. The output is finally clamped to ±ceiling (a last resort that only acts if the detector missed a peak). At rest the gain is exactly 1 and the audio is untouched. `output` is applied **after** the limiter, so the final ceiling is `ceiling + output`; keep it at 0 dB to guarantee the ceiling.
- **Loudness meter** (BS.1770-4 / EBU R128): K-weighting (high-shelf pre-filter and RLB high-pass) with coefficients computed for the actual sample rate (at 48 kHz they equal the published values to 1e-9); 100 ms sub-blocks; **momentary** = last 400 ms, **short-term** = last 3 s, both updated every 100 ms and starting from a zero-filled window; **integrated** with the −70 LUFS absolute gate and the −10 LU relative gate over 400 ms blocks at 75 % overlap. Gating storage is a fixed **histogram of 801 bins of 0.1 LU (−70 to +10 LUFS, the last bin catches louder blocks)** holding per bin a block count (`u64`) and the exact sum of mean squares (`f64`), so any programme length runs without allocation; the relative-gate membership is quantised to half a bin (0.05 LU), which moves a result on programme material by well under 0.01 LU. Silence and "no data" read −120. **True peak** is the 4x oversampled maximum (unrefined, as in BS.1770 Annex 2) of the output, held until reset. **`meter.reset`** (a rising edge from 0 to 1) clears the integrated loudness, both windows and the true-peak maximum, and `reset()` does too; the transport does not. Note that the output still carries the last `latency` samples of the audio from before the reset.
- **Latency** (constant per sample rate, set in `prepare`): `17 + round(1.5 ms × sr)` samples = **89 at 48 kHz, 83 at 44.1 kHz, 161 at 96 kHz**: 17 is the group delay of the oversampler's up path (a sample `n` lands in detector group `n + 17`), the rest is the lookahead; the audio never passes through the oversampler. The tests measure the impulse delay (multiband off).
- **Meter values** (8): `[momentary LUFS, short-term LUFS, integrated LUFS, true peak dBTP, GR band 1, GR band 2, GR band 3 (dB, largest in the batch), limiter GR (dB)]`.
- **Measured** (48 kHz unless stated; all in `crates/devices/src/mastering/tests.rs`):
  - Integrated loudness of stereo 1 kHz sines: −23 dBFS reads −22.993 LUFS, −33 dBFS −32.993, −20 dBFS −19.993 (a stereo sine reads its peak level in LUFS: both channels at −20 dBFS give −20 LUFS, not −23); 44.1 kHz −22.991, 96 kHz −23.011. One channel only, 997 Hz at 0 dBFS: −3.010 LUFS (−3.01 expected). EBU Tech 3341 cases 1 and 2 (stereo −23 and −33 LUFS, 20 s), case 3 (10 s at −36, 60 s at −23, 10 s at −36: −23.014) and case 4 (the same with 10 s of −72 LUFS at both ends: −23.014) are synthesised with a 1 kHz sine.
  - Gating: 20 s of tone followed by 20 s of silence reads −23.026 against −22.993 for the tone alone (difference 0.033 LU, the transition blocks); alternating 5 s tone / 10 s silence reads 0.19 LU lower (every edge adds three partly filled blocks, which the standard keeps).
  - Momentary/short-term on a 1 s burst at −23 dBFS: window half full (0.2 s in) −26 ± 0.4, full −23.0 ± 0.1, back to −120 0.4 s (momentary) and 3 s (short-term) after the end; a long tone reads −26 at 1.5 s and −23 at 3.1 s in the short-term window.
  - True peak meter on the fs/4, 45° sine with a −6 dBFS sample peak: −2.91 dBTP (−3.01 expected, error 0.10 dB).
  - Limiter (ceiling −1 dBTP unless stated), true peak measured on the output with an independent 16x windowed-sinc reference (steady state, the first 3000 samples and the last 64 excluded; the device's own BS.1770 meter, which includes the onset, agrees to 0.01 dB): the fs/4, 45° sine pushed +3 dBTP reads −1.003 / −3.004 / −6.003 dBTP for ceilings −1 / −3 / −6; 997 Hz −1.000; 11.5 kHz −1.003; 19 kHz −1.004; band-limited (0.35 fs) noise at 0 dBFS and +12 dB gain −0.996 and −0.989 (the largest overshoot, **+0.011 dB**), and through the whole default chain with width 150 % −0.991. **Limitation:** white noise has energy between 0.4 and 0.5 fs, which the 4x halfband stage attenuates, so the 16x reference of full-band white noise overshoots the ceiling by **0.90 to 1.03 dB** (the 4x BS.1770 reading stays at −0.976 / −0.980 dBTP, within 0.03 dB of the ceiling); programme material with little above 18 kHz is unaffected.
  - Sustained tones: with ceiling −1 and +6 dB over, the residual after removing the fitted tone is −52.7 dB at 100 Hz (the gain follows the peaks; 150 ms release) and −146.6 dB at 1 kHz and 5 kHz. Release: the gain reaches 63.2 % of its way back after 101.3 ms for a 100 ms setting.
  - Band compressors: a 100 Hz tone only reduces band 1, 6 kHz only band 3; a −10 dBFS 1 kHz tone with threshold −30 dB and ratio 4 gets 15 ± 0.3 dB GR in band 2.

---

## Character: `atomik.character` (Effect)

A stereo insert with 16 character presets in a 4 x 4 matrix, an input gain, a macro **Transform** morph, dry/wet and volume, and CV inputs with trims for Transform and Dry/Wet. An original design with its own preset names. Source: [crates/devices/src/character.rs](../crates/devices/src/character.rs) and its folder (`chain`, `rack`, `stages`, `voicing`, `row1` to `row4`); tables in [crates/devices/data](../crates/devices/data). All 16 presets are voiced; the voicing was set by ear and checked with analytic tests.

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `in_l` | Audio | in | |
| `in_r` | Audio | in | Unconnected: `in_l` feeds both channels |
| `transform_cv_in` | CV | in | Bipolar, added to `transform` through its trim; reads 0 when unconnected |
| `mix_cv_in` | CV | in | Bipolar, added to `mix` through its trim; reads 0 when unconnected |
| `out_l`, `out_r` | Audio | out | Stereo, mono sum or dual mono, per preset |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `gain` | 0 to 1 (knob position; gain = `4 x^2`, 0 dB at 0.5, +12 dB at 1, silence at 0) | 0.5 | none | Linear | no |
| `preset` | 0–15: reel, loudness, sparkle, low-end, wide, pocket-radio, camcorder, dusty-groove, lossy, swirl, torn-cone, tin-robot, bit-bender, spin-cycle, ion-sweep, ghost-pipes | 0 | choice | Linear | yes |
| `transform` | 0 to 1 | 0.5 | percent | Linear | no |
| `mix` | 0 (dry) to 1 (wet) | 1 | percent | Linear | no |
| `volume` | 0 to 1 (same law as `gain`) | 0.5 | none | Linear | no |
| `transform_cv_trim` | 0 to 1 | 1 | percent | Linear | no |
| `mix_cv_trim` | 0 to 1 | 1 | percent | Linear | no |

`gain` and `volume` are linear knob positions; the dB display is the UI's job (`atomik_devices::character::knob_gain_db`), because the shared param model has no square taper.

**Panel** ([crates/ui/src/character_panel.rs](../crates/ui/src/character_panel.rs), [character_screen.rs](../crates/ui/src/character_screen.rs)): a 3 U panel with the 4 x 4 preset matrix (one glyph per preset, the active button lit, one `SetParam` per click, arrow keys, Home and End when the matrix is focused), a screen with the active preset's animated scene and a latency class badge (`LAT SHORT`, `LAT LONG` for `lossy`), a meter strip (input peak, output peak, activity from `meter_values`), the knobs Input Gain, Transform, Dry/Wet and Volume with their values (gain and volume in dB through the `4 x^2` law, `-inf dB` at the bottom) and, while a CV input is cabled, a ring on the Transform and Dry/Wet knobs showing the range the CV sweeps (knob plus or minus its trim). The back has the six sockets and the two CV trim knobs under their jacks.

### Behaviour (as built)

- **Signal path.** `in` is tapped for the **dry path before the input gain** and delayed by the active preset's latency; the wet path is input gain (second-order smoother, 5 ms) → chain → linear dry/wet crossfade → volume (50 ms smoother) → a transparent safety that acts beyond ±4.0 only and bounds the output at 8. Input gain therefore drives the wet path only: `mix` 0 at volume 0 dB is the **input delayed by the latency, bit for bit**, for any input gain. Non-finite input is treated as 0.
- **Control rate.** The engine batch of 64 frames runs as four blocks of 16: per block the device smooths the CV values (8 ms 90 % rise, second order), resolves the **Transform morph** of the active preset (up to 8 values; `lin`, `log`, `square`, `exp` or `step` between the table points at Transform 0, 0.5 and 1) and runs the chain. `T = clamp(transform + trim × cv, 0, 1)` and `mix = clamp(mix + trim × cv, 0, 1)`.
- **Presets** are original chains built from `atomik-dsp` blocks: a generic **rack** (stages with sends, gains per send morphable, lazy evaluation of stages that no active route reaches, load-time acyclic and reachability checks, automatic alignment of parallel paths) for every preset except `wide`, which is hand-wired (mid/side with a decorrelator); rows 3 and 4 use the stage kinds `Spectral`, `Phaser`, `AutoPan`, `Echo`, `RingMod`, `Comb`, `BandPass`, `Crush`, `Tremolo`, `SweepLp`, `Hum`, `Bursts`, `Rattle`, `Modal` and `Room` (see the table below). Every number is data: `data/character-voicing.csv` (one row per stage parameter, with the three Transform points and the interpolation), `data/character-routes.csv` (the sends). Each preset has a `target_gain_db` row that level-matches it (about 0 dB at Transform 0.5 on −20 dBFS pink noise, within ±1.5 dB; at the ends within ±6 dB).
- **Generated noise** (hiss, rumble, crackle, pops, head-switch clicks) is synthesised and deterministic from the device seed (output depends on the seed and the sample index only), lives in the wet path, does **not** follow the input gain unless the table row `noise_follows_gain` says so, and is gated by a **programme-presence detector on the dry input**: an idle device is silent, the noise fades out after the programme stops (hold and fade are table rows, default 3 s and 2 s).
- **Oversampling and latency (tiered).** The saturating stages run 2x oversampled (the 29-frame halfband round trip; bypassed at 88.2 kHz and above behind a 29-frame pad). Every preset without the spectral stage reports the **short class latency** `29 + wobble centre` frames (the wobble centre is `ceil(0.3 ms × fs) + 2`: **45, 46 and 60 frames** at 44.1, 48 and 96 kHz); chains without an oversampled region or a wobble are padded up to it, parallel branches are aligned, so all short presets report the same value and a switch between them never changes it. `lossy`, the one preset with the spectral stage, is in the **long class**: the short class latency plus the spectral frame (1024 frames at 44.1 and 48 kHz, 2048 at 96 kHz, so the frame spans the same time), that is **1069, 1070 and 2108 frames** at 44.1, 48 and 96 kHz; its parallel and dry paths are aligned to it. `latency_frames` changes **only when a preset change across the two classes finishes its fade** (one bump of the engine's latency epoch, one PDC re-alignment).
- **Preset switch.** A change of `preset` starts a 30 ms crossfade (smoothstep weight, rounded up to whole 16-frame blocks) from the outgoing chain to the incoming one, which starts from reset state; the outgoing chain is reset when the fade ends. A request during a fade is queued and the last wins (at most 60 ms to the final preset). When the two chains have different latencies the dry path crossfades between the two delays with the same weight.
- **Meter values** (4): `in_peak` (after the input gain, larger of the two channels, last batch), `out_peak`, `transform_eff` (0..1) and `activity` (0..1, a 150 ms peak follower of the wet signal).
- **Aliasing.** The saturation curves (soft, asymmetric, hard, hard asymmetric) run at 2x with **first-order antiderivative anti-aliasing** (`Shaper::set_adaa`) and a band-limit ahead of every saturator that sees wideband input (a low-pass at 7 kHz ahead of `sparkle`'s exciter and one at 12 kHz after it, a low-pass at 12 kHz ahead of `pocket-radio`'s saturation). With Transform 1, input gain +12 dB and a −3 dBFS sine at 5 kHz and 18 kHz, all non-harmonic components of `reel`, `sparkle`, `low-end`, `pocket-radio` and `torn-cone` are below **−71 dBFS** at 44.1 and 48 kHz (worst: `torn-cone` at 18 kHz and 44.1 kHz, −71.0; `reel` −73.4; the others −78 to −130) and below −87 dBFS at 96 kHz, where the oversampler is bypassed. (Without the ADAA, with the same band-limits, the worst cases were −51.9 dBFS for `torn-cone` and −54.3 for `reel` at 18 kHz and 44.1 kHz and −59.0 at 96 kHz.) The test sets Volume to −24 dB: the output **safety beyond ±4.0 is itself a nonlinearity**, and at the default Volume the chain's +12 dB of gain with `sparkle`'s +9 dB shelf drives the output past that knee, where the safety aliases (a test measures it: more than 25 dB above the chain's own level); Volume is the user's compensation for a high input gain by design. **Cost of the ADAA:** it acts as a two-tap average at the 2x rate, so the small-signal response of the five saturating presets droops by `cos(pi f / (2 fs))`: 0.3 dB at 8 kHz, 0.7 dB at 12 kHz and 1.6 dB at 18 kHz at 48 kHz (the analytic tests include the term), and it adds a quarter of a frame to the latency of the saturating stages (not counted in the declared latency; the dry path is off by that fraction of a sample).

### In the app, the bounce and the examples

- **Meter feed.** The app feeds the Character panel like the other six effects: the four `meter_values` (input peak, output peak, `transform_eff`, `activity`) are read once per frame by `AppCore::feed_device_meters` for every device whose `FxKind` has meters, inside containers by path key; a recording-sink test covers all seven `FxKind`s and a real offline engine feeds the four values while music plays.
- **Latency live and in a bounce.** The engine publishes the class latency (45, 46 or 60 frames short, 1069, 1070 or 2108 long, at 44.1, 48 and 96 kHz) and bumps its latency epoch only when a preset change across the classes finishes its fade; the app recompiles the routing on the next frame (`AppCore::frame`), and the bounce does the same after every 64-frame batch (`atomik_render::plugin_pass::render_song_following_latency`). With compensation on, a click through Character at `mix` 0 starts with the dry click in both classes, and with compensation off it starts later by exactly the latency (`render/tests/character_pdc.rs`). A song lane reaches an effect only through a container macro, so a preset or Transform lane is a container with the Character inside and a macro on `preset` (min 0, max 15; a discrete target rounds each lane point) or `transform`.
- **Live equals bounce.** For `examples/projects/character-demo` (below) the live path, played at host blocks 64, 480 and 512, equals the bounce bit for bit (after the engine's one-batch output delay), also after a preset change across the classes made through the app, after undo and redo, and after an engine restart (`app_core_character_tests.rs`). A live session that has already run a while is not a cold start: the devices' free-running state has advanced (measured: a live engine that rendered only 256 idle frames before Play already differs from the bounce of the demo from frame 311), so only a cold engine matches a bounce bit for bit; the tests therefore edit before the first render and step the engine one batch per frame like the bounce loop does.
- **Example.** `examples/projects/character-demo/character-demo.atomik` (README beside it): a synth through a Character inside a container with preset and Transform lanes, a bass line with a Character on `ion-sweep` whose Transform is moved by the sequencer's note CV, and a pad with a Character on `lossy`; generated by `render/tests/character_demo.rs`, bit-identical across host blocks 64, 333, 480 and 512 and workers 0, 2 and 3.
- **Stress.** `--stress-character` adds 8 containers, each a poly synth into a Character whose preset steps every bar through all 16 presets (container `n` starts on preset `2 n`, so eight different presets sound at any time and `lossy` is crossed all the time) with a Transform lane, on top of the busy stress song; it combines with the other stress flags (`stress::add_stress_character`).

### Rows 3 and 4 (original voicings)

Stage kinds in signal order; the Transform column lists the morph values (at most 8 per preset); `chain` rows (level trim, presence gate times) are not listed. All numbers are in `data/character-voicing.csv`.

| Idx | Preset | Stages | Transform morph | Latency class |
|---|---|---|---|---|
| 8 | `lossy` | top bell lift, **spectral stage** (gate with hold, coarse level quantiser, seeded dropouts of four-bin bands per frame), top bell dip, resonant low-pass edge | gate threshold rises, hold shortens, quantiser step and dropout grow, low-pass edge falls | **long** |
| 9 | `swirl` | six-pole phaser, wow and flutter wobble, rotating constant-power auto-pan, feedback echo (120 ms) | phaser rate, depth and feedback; wobble depth; pan rate (follows the phaser) and depth; echo feedback | short |
| 10 | `torn-cone` | high-pass, 2x hard asymmetric clip, cone peak (+8 dB, Q 3), low-pass, rattle keyed by the input envelope, output compressor; mono | drive (12 to 42 dB) and its make-up, low-pass edge, rattle amount | short |
| 11 | `tin-robot` | normalised comb (6 to 1.5 ms, right channel detuned, slow seeded wander), band from a high-pass and a low-pass, two formant-like peaks, ring modulation (55 Hz) | comb delay and feedback, ring amount | short |
| 12 | `bit-bender` | bit and hold-rate reduction at the **base rate** (floor quantiser, no dither, no oversampling) with seeded stepped random modulation per channel, DC blocker; gated pops | bit depth (12 to 2), hold rate (24 kHz to 1.5 kHz), random rate and depth, make-up, pop rate | short |
| 13 | `spin-cycle` | drum tremolo (phase-offset channels, level compensated), sloshing swept low-pass, a resonant box (small room) and a long echo wash in parallel; motor hum and water bursts added | tremolo rate and depth, slosh depth, hum level, burst rate, echo send gain | short |
| 14 | `ion-sweep` | comb flanger (9 to 0.3 ms, 7 % channel offset) and resonant band-pass (400 Hz to 8 kHz), faint ring tone | **Transform is the sweep position**: comb delay and band-pass centre | short |
| 15 | `ghost-pipes` | high-pass on the excitation, twelve-mode inharmonic resonator bank (mono-sum excitation, even modes left and odd modes right, seeded detune, per-mode decay, level limiter after the bank) | root pitch (110 to 880 Hz), decay (0.2 to 4 s), detune spread, active mode count (4 to 12), level | short |

- **Level.** Each preset is level-matched with its `target_gain_db` row: within ±1.5 dB at Transform 0.5 and ±6 dB at the ends on −20 dBFS pink noise (`ghost-pipes`' resonators capture little of a broadband signal, so its bank gain is high and a limiter after the bank keeps loud tones bounded).
- **Reset.** A `reset` restores the start state of every preset, the wobble presets included (`Wobble::restart` re-seeds the modulation); a render after a reset equals the first render.
- **Determinism.** Every random stage (dropouts, wander, stepped random, bursts, rattle noise, mode detune) draws from the device seed and the sample index only.
- **Phaser sign (fixed).** The phaser block's all-pass coefficient had the wrong sign, which mirrored its sweep around fs/2 (the notches sat near 23 kHz instead of the centre frequency); the coefficient and its test (a two-pole sum notches at its centre) were corrected, and `swirl` is voiced on the corrected block.

---

## Audio Track: `atomik.audio-track` (Instrument)

The player device of an audio track. It has no sound of its own: the engine renders the track's audio clips (start, offset, trim, gain and fades) and hands the result to the device, which adds the monitored input and writes the outputs. Because it is an Instrument, it gets a console channel like any other source. You do not add it by hand: the arrangement's add-track row, File ▸ Import Audio and dropping files create it with its track and its input cables. Source: [crates/devices/src/audio_track.rs](../crates/devices/src/audio_track.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `in_l` | Audio | in | Left input, cabled from `interface:in_k` (the track's input selector makes these cables). Reads 0 when unconnected. |
| `in_r` | Audio | in | Right input. A mono track has the same channel on both, or only one of the two cabled. |
| `out_l` | Audio | out | Left output |
| `out_r` | Audio | out | Right output |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `monitor` | 0 off, 1 auto, 2 on | 1 | choice | Linear | yes |
| `internal.monitor_live` | 0 or 1 | 0 | toggle | Linear | yes |

`internal.monitor_live` is set by the app (1 while the track is armed). The UI hides it and it is neither saved nor undoable, like `internal.solo_silence`.

### Behaviour (as built)

- **Output.** `out = playback + input × monitor_gain`, per channel. Without playback (transport stopped, or no song) only the monitored input is heard.
- **Monitoring.** `monitor_gain` is 1 when `monitor` is on, or when it is auto and `internal.monitor_live` is 1; otherwise 0. With off the input never passes, even while armed.
- **Smoothing.** A change of the monitor gain is a 5 ms linear crossfade. The first batch after the device is prepared or reset snaps without a ramp.
- **Clips.** Clip audio is not stretched when the tempo changes; a clip's start follows the tempo map and its audio plays in real time. Fades are linear or equal power; mono assets play on both channels, and for more than two channels the first two are played.
- **Latency.** None (a single stage). The 64-frame engine FIFO and the interface latencies are accounted for by recording, not by the device.

---

## Sampler: `atomik.sampler` (Instrument)

A multi-zone sampler. It plays a **sample map** (shared sample buffers plus zones) that the app installs from the song's asset pool; the map is not a param and is not part of the descriptor. Every zone that matches a note sounds, so zones layer. Each voice has a cubic Hermite sample reader, an amp envelope, an optional 12 dB state-variable filter with its own envelope, and a shared LFO. Source: [crates/devices/src/sampler.rs](../crates/devices/src/sampler.rs), playback maths in [crates/dsp/src/sample_read.rs](../crates/dsp/src/sample_read.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `notes_in` | Notes | in | Note on, note off and all-notes-off |
| `pitch_cv_in` | CV | in | Clamped to −1..+1. +1 raises the pitch by `cv.pitch_amount` semitones. Reads 0 when unconnected. |
| `cutoff_cv_in` | CV | in | Clamped to −1..+1. +1 raises the cutoff by `cv.cutoff_amount` octaves (only with the filter on). |
| `resonance_cv_in` | CV | in | Clamped to −1..+1. Adds `cv.resonance_amount` × CV to the normalised resonance (only with the filter on). |
| `level_cv_in` | CV | in | Clamped to −1..+1. Scales the output by `cv.level_amount` × CV decibels. |
| `pan_cv_in` | CV | in | Clamped to −1..+1. Adds `cv.pan_amount` × CV to the pan of every voice. |
| `out_l` | Audio | out | Left output |
| `out_r` | Audio | out | Right output |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `voices` | 1 to 64 | 32 | none | Linear | yes |
| `master.level` | −60 to 6 (−60 = silent) | −6 | dB | dB | no |
| `tune.semi` | −24 to 24 | 0 | semitones | Linear | yes |
| `tune.fine` | −100 to 100 | 0 | cents | Linear | no |
| `env.amp.attack` | 0.5 to 10 000 | 1 | ms | Exp | no |
| `env.amp.decay` | 0.5 to 10 000 | 200 | ms | Exp | no |
| `env.amp.sustain` | 0 to 1 | 1 | percent | Linear | no |
| `env.amp.release` | 0.5 to 10 000 | 150 | ms | Exp | no |
| `velocity.amp` | 0 to 1 | 0.5 | percent | Linear | no |
| `filter.enabled` | 0 or 1 | 0 (off) | toggle | Linear | yes |
| `filter.mode` | 0–2: LP12, BP12, HP12 | 0 (LP12) | choice | Linear | yes |
| `filter.cutoff` | 20 to 20 000 | 8000 | Hz | Exp | no |
| `filter.resonance` | 0 to 1 | 0.2 | percent | Linear | no |
| `filter.env_amount` | −1 to 1 (±6 octaves) | 0 | none | Linear | no |
| `filter.key_track` | 0 to 1 | 0 | percent | Linear | no |
| `filter.velocity` | 0 to 1 (0 to 4 octaves down at velocity 0) | 0 | percent | Linear | no |
| `env.filter.attack` | 0.5 to 10 000 | 2 | ms | Exp | no |
| `env.filter.decay` | 0.5 to 10 000 | 400 | ms | Exp | no |
| `env.filter.sustain` | 0 to 1 | 0 | percent | Linear | no |
| `env.filter.release` | 0.5 to 10 000 | 300 | ms | Exp | no |
| `lfo.wave` | 0–4: sine, triangle, saw, square, sample&hold | 0 (sine) | choice | Linear | yes |
| `lfo.rate` | 0.05 to 30 | 2 | Hz | Exp | no |
| `lfo.to_pitch` | 0 to 12 | 0 | semitones | Linear | no |
| `lfo.to_cutoff` | 0 to 4 | 0 | octaves | Linear | no |
| `lfo.to_amp` | 0 to 1 | 0 | percent | Linear | no |
| `cv.pitch_amount` | 0 to 24 | 12 | semitones | Linear | no |
| `cv.cutoff_amount` | 0 to 4 | 2 | octaves | Linear | no |
| `cv.resonance_amount` | −1 to 1 | 0.5 | percent | Linear | no |
| `cv.level_amount` | −24 to 24 | 12 | dB | Linear | no |
| `cv.pan_amount` | −1 to 1 | 1 | none | Linear | no |

### Zones (the sample map)

A zone has a key range, a velocity range (1–127; a note velocity `v` maps to round(`v`·127), at least 1), a root key, transpose (−48 to 48 semitones), fine tune (±100 cents), gain (−60 to 12 dB), pan (−1 to 1), sample start and end, a loop (off, forward or ping-pong, with start and end), a crossfade length, a trigger mode (gate or one-shot) and a **reverse** flag. The map is validated when it is built, off the audio thread. A reversed zone plays from `end − 1` down to `start` through the same playhead, with the end fade at `start`; its loop and crossfade fields are ignored and not validated (the loop rules apply only when the loop is on and the zone is not reversed).

### Behaviour (as built)

- **Pitch.** The playback ratio is 2^((key − root + transpose + `tune.semi`)/12 + (zone cents + `tune.fine`)/1200 + (LFO·`lfo.to_pitch` + CV·`cv.pitch_amount`)/12). The ratio is updated every 16 samples. A root-key note without modulation reads every frame of the asset bit for bit; an octave up reads every second frame. Assets are already at the session rate, so there is no rate conversion.
- **Interpolation.** 4-point, third-order Hermite. The four frames are clamped to the buffer, so a read never leaves it.
- **Position.** The playhead is a whole frame plus an `f64` fraction. It advances by the ratio with integer carries, so a loop wraps on the exact frame and nothing drifts.
- **Loops.** A forward loop plays `loop_start`..`loop_end` and wraps. A ping-pong loop plays `loop_start`..`loop_end − 1` forwards and backwards, each end frame once per turn. A loop only starts once the playhead reaches `loop_start`, so a run-in before it plays once. A loop of one frame holds that frame.
- **Crossfade.** For a forward loop with a crossfade of N frames, the last N frames before `loop_end` blend the tail with the audio one loop length earlier (just before `loop_start`) with equal-power gains, so the wrap has no step. Ping-pong loops ignore the crossfade.
- **Voices.** A note-on starts one voice per matching zone. Gate zones release on note-off. One-shot zones ignore note-off and play to `end` (with a loop they play until stolen or `AllOff`). `AllOff` releases every voice. With the loop off, the last frames before `end` fade out linearly to zero (the fade is the smaller of 64 source frames and 1.5 ms at the session rate, and never longer than the zone), so the voice is silent at `end` and then goes idle; this avoids a click when the sample does not end at zero. Looped zones have no end fade. Notes start at the sample offset of their event.
- **Voice stealing.** When every allowed voice is busy, the oldest started voice is stolen (a start counter, one count per zone started, orders the voices, so stealing is deterministic). It fades out linearly over 2 ms, then the new note starts. A note-off for a note still waiting for its voice cancels it (gate zones).
- **Level.** Voice gain = zone gain × amp envelope × (1 − `velocity.amp` × (1 − velocity)) × tremolo, where tremolo is 1 − `lfo.to_amp` × (1 − LFO)/2. A mono sample goes through a constant-power pan (−3 dB at the centre). A stereo sample is balanced by the pan: unity at the centre, the far side falls as cos(|pan|·π/2). The sum times `master.level` (5 ms smoothing) passes through the soft limiter of the poly-synth (linear to ±2, below ±4 always).
- **Filter.** With `filter.enabled` at 0 (the default) the signal is not filtered at all. When on, each voice filters its left and right channels with 12 dB state-variable filters. Cutoff = `filter.cutoff` × 2^(`env_amount`·6·filter env + `key_track`·(key − 60)/12 + `lfo.to_cutoff`·LFO + `cv.cutoff_amount`·CV + 4·`filter.velocity`·(velocity − 1)), clamped to 20 Hz … 0.45 × sample rate, updated every 16 samples with the coefficient interpolated in between.
- **CV inputs (resonance, level, pan).** Each is applied only while its socket is cabled; with all of them open the output is bit for bit what it was before they existed. Every CV sample is clamped to −1..+1 and multiplied by the signed amount param.
  - *Resonance:* per voice, the normalised resonance becomes clamp(`filter.resonance` + `cv.resonance_amount`·CV, 0, 1) at the 16-sample control rate, the damping ramped in between. Only audible with `filter.enabled` on.
  - *Level:* the master gain is multiplied per sample by 10^(`cv.level_amount`·CV/20), before the soft limiter, for all voices. No extra smoothing.
  - *Pan:* per voice and per sample, the pan is the zone pan + `cv.pan_amount`·CV (clamped to ±1 by the pan law), through the usual law (constant power for a mono sample, balance for a stereo one).
- **No map or no matching zone:** silence. `reset` silences the voices and keeps the maps. There is no latency.
- **Map swaps.** The device holds the current map and at most one *draining* map, and never frees a map itself: `swap_samples` hands one back to the caller, which drops it off the audio thread.
  - A voice whose zone index exists in the new map, with a buffer that is the same `Arc`, moves to the new zone (it takes the zone's parameters and keeps its position). Every other voice keeps playing from the old map with a 5 ms fade-out; the old map becomes the draining map.
  - A second swap while voices still drain cuts those voices (no fade) and hands the draining map back. When no voice needs the old map and nothing is held as draining, the old map itself is handed back. When no voice needs it but a stale draining map is still held (all its voices ended), the stale one is handed back and the replaced map is kept as the draining map until the next swap. At most one map is returned per swap.
  - A note that waits for a steal fade survives a swap only if the new map has a zone at the same index that still matches it.
- **Voice count.** `voices` is the polyphony limit, changeable at any time; voices above a lowered limit fade out over 2 ms. All 64 voices are allocated when the device is created and prepared.

---

## Drum machine: `atomik.drum-machine` (Instrument)

A 16-pad sample player. Like the sampler it plays a **sample map** installed by the app (not a param). Pad *n* (1 to 16) sits on key 35 + *n* (36 to 51, GM). A pad's sound is the zones whose key range is exactly its key (`key_lo == key_hi == pad key`); their velocity ranges make velocity layers (overlapping ranges layer). Zones on other keys and notes on other keys are ignored. A hit with no zone for its velocity does nothing (no choke, no retrigger). Source: [crates/devices/src/drum_machine.rs](../crates/devices/src/drum_machine.rs), playback maths in [crates/dsp/src/sample_read.rs](../crates/dsp/src/sample_read.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `notes_in` | Notes | in | Note on, note off and all-notes-off |
| `out_l`, `out_r` | Audio | out | Main mix of every pad whose individual output is not cabled |
| `pad1_out_l`, `pad1_out_r` … `pad16_out_l`, `pad16_out_r` | Audio | out | Individual stereo output of pad *n* (sockets 3 to 34 in this order) |
| `pad1_level_cv_in`, `pad1_pan_cv_in` … `pad16_level_cv_in`, `pad16_pan_cv_in` | CV | in | Level and pan CV of pad *n* (sockets 35 to 66 in this order), clamped to −1..+1. Level: `padN.cv_level_amount` × CV decibels. Pan: `padN.cv_pan_amount` × CV added to the pan. |

### Params

`master.level` first, then eight params per pad *N* = 1 to 16 in this order, then the two CV amounts of each pad (161 params; ids follow the order):

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `master.level` | −60 to 6 (−60 = silent) | −6 | dB | dB | no |
| `padN.level` | −60 to 6 (−60 = silent) | 0 | dB | dB | no |
| `padN.pan` | −1 to 1 | 0 | none | Linear | no |
| `padN.tune` | −24 to 24 | 0 | semitones | Linear | yes |
| `padN.fine` | −100 to 100 | 0 | cents | Linear | no |
| `padN.decay` | 5 to 10 000 | 500 | ms | Exp | no |
| `padN.mode` | 0–1: gate, one-shot | 1 (one-shot) | choice | Linear | yes |
| `padN.choke` | 0 to 4 (0 = none) | 0 | none | Linear | yes |
| `padN.velocity` | 0 to 1 | 0.7 | percent | Linear | no |
| `padN.cv_level_amount` | −24 to 24 | 12 | dB | Linear | no |
| `padN.cv_pan_amount` | −1 to 1 | 1 | none | Linear | no |

### Behaviour (as built)

- **Pad zones.** `swap_samples` scans the map once and keeps, per pad, the indices of its first 8 zones in map order in fixed arrays (no allocation). Zones past the eighth of a pad are never played.
- **Voices.** 32 voices, all preallocated, not a param. A hit starts one voice per matching zone. When none is free, the oldest started voice is stolen (a voice that is already fading out for a choke or retrigger goes first): it fades out linearly over 2 ms, then the new hit starts. A start counter orders the voices, so stealing is deterministic.
- **Envelope.** Every voice has a 0.5 ms linear attack (click protection). *Gate* mode holds at 1 while the key is down; note-off starts the decay. *One-shot* mode starts the decay right after the attack and ignores note-off. The decay is exponential, 10^(−3·t/`decay`), so it is −60 dB after `padN.decay` ms; the voice goes idle at −80 dB. `padN.mode` decides: the zone's own trigger mode is ignored. A looped zone loops while held (gate) or until the decay ends. Without a loop the sample plays to `end` with the sampler's end fade (64 frames or 1.5 ms) and the voice goes idle. The mode is read at the hit; level, pan, tune, fine, decay and choke are read live (no smoothing, they change at the next event or batch boundary).
- **Reverse.** A zone with `reverse` plays backwards (`end − 1` down to `start`, no loop, the end fade is at `start`), exactly as in the sampler.
- **Pitch.** Ratio 2^((pad key − root + zone transpose + `padN.tune` + (`padN.fine` + zone cents)/100)/12): unity when the zone root is the pad key. Cubic Hermite read with the shared playhead (zone start/end and loops as in the sampler).
- **Level and pan.** Voice gain = envelope × 10^(`padN.level`/20) × zone gain × (1 − `padN.velocity` × (1 − velocity)). Pan is `padN.pan` plus the zone pan, clamped: constant power for a mono sample (−3 dB at the centre), balance for a stereo sample (unity at the centre).
- **CV inputs (level and pan, per pad).** Each is applied only while its socket is cabled; with all of them open the output is bit for bit what it was before they existed. The CV is clamped to −1..+1 and multiplied by the pad's signed amount. It acts on every voice of that pad (and so on its individual output too) and on no other pad.
  - *Level:* the voice gain is multiplied per sample by 10^(`padN.cv_level_amount`·CV/20). No extra smoothing, so a stepped CV gives a stepped gain.
  - *Pan:* per voice and per sample, the pan is `padN.pan` + zone pan + `padN.cv_pan_amount`·CV, through the pad's law (constant power for a mono sample, balance for a stereo one).
  - The panel has no front knobs for the two amounts (the front is full); they are params of the device, set at their defaults or from the project.
- **Choke and retrigger.** A hit on a pad in choke group *g* (1 to 4) fades every sounding voice of the other pads in group *g* to zero over 2 ms and cancels their pending hits. A hit also retriggers its own pad: its previous voices fade over 2 ms while the new ones start at once. Group 0 chokes nothing and is never choked. All-notes-off fades every voice over 2 ms.
- **Outputs.** A pad whose `padN_out_l` or `padN_out_r` is connected writes both channels only to its individual pair and leaves the main mix; the other pads are in `out_l`/`out_r`. `master.level` and the soft limiter of the poly-synth apply to the main mix only. Individual outputs are neither scaled nor limited (non-finite values become 0).
- **Map swaps.** Exactly as the sampler (see above): the device holds the current and at most one draining map and hands one back per swap. A voice whose zone index exists in the new map with a pointer-equal buffer moves to that zone (it keeps its pad and position and adopts the zone's start/end/loop/gain/root, so it keeps playing the same buffer even if the pad's zone list changed); other voices drain with a 5 ms fade; a second swap cuts voices that still drain. A pending hit survives only if the new map has a zone of its pad at the same index that still matches its velocity.
- **No map or no matching zone:** silence. `reset` silences the voices and keeps the maps.

---

## Loop slicer: `atomik.loop-slicer` (Instrument)

Plays the slices of one loop from keys. Like the sampler it plays a **sample map** installed by the app (not a param). Slice *n* (1 to 64) sits on key 23 + *n* (C1 = 24 up to 87). A slice is a zone with `key_lo == key_hi == its key`; its `start..end` is the slice, and its gain, pan, transpose, tune and **reverse** are the per-slice settings. Zones on other keys and notes on other keys are ignored; with several zones on one key the first in map order is the slice. A note on a key without a slice does nothing. Source: [crates/devices/src/loop_slicer.rs](../crates/devices/src/loop_slicer.rs), playback maths in [crates/dsp/src/sample_read.rs](../crates/dsp/src/sample_read.rs). The transient detector that proposes slice points is [crates/dsp/src/onset.rs](../crates/dsp/src/onset.rs) (`atomik_dsp::onset::detect_onsets`).

### Sockets

| Key | Name | Kind | Direction |
|---|---|---|---|
| `notes_in` | Notes in | Notes | in |
| `pitch_cv_in` | Pitch CV | CV | in |
| `cutoff_cv_in` | Cutoff CV | CV | in |
| `level_cv_in` | Level CV | CV | in |
| `out_l` | Out L | Audio | out |
| `out_r` | Out R | Audio | out |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `master.level` | −60 to 6 | −6 | dB | dB | no |
| `voices` | 1 to 16 | 16 | none | Linear | yes |
| `tune.semi` | −24 to 24 | 0 | semitones | Linear | yes |
| `env.attack` | 0.5 to 50 | 0.5 | ms | Exp | no |
| `env.decay` | 5 to 10000 | 2000 | ms | Exp | no |
| `env.mode` | 0 to 1 | 1 | gate / one-shot | Linear | yes |
| `velocity.amp` | 0 to 1 | 0.5 | percent | Linear | no |
| `loop.beats` | 1 to 64 | 4 | none | Linear | yes |
| `cv.pitch_amount` | 0 to 24 | 12 | semitones | Linear | no |
| `filter.cutoff` | 20 to 20 000 | 20 000 | Hz | Exp | no |
| `cv.cutoff_amount` | −4 to 4 | 2 | octaves | Linear | no |
| `cv.level_amount` | −24 to 24 | 12 | dB | Linear | no |

### Behaviour (as built)

- **Voices.** 16 voices, all preallocated; `voices` limits how many may sound (voices above a lowered limit fade over 2 ms). One hit starts one voice. When none is free, the oldest started voice is stolen (one already fading out goes first): it fades out linearly over 2 ms, then the new hit starts. A start counter orders the voices, so stealing is deterministic.
- **Retrigger.** A new hit of a slice fades its previous voices over 2 ms (and cancels their pending hits) while the new voice starts at once. Different slices overlap. All-notes-off fades every voice over 2 ms.
- **Envelope.** A linear attack of `env.attack` ms, then in *gate* mode the amp holds at 1 while the key is down and note-off starts the exponential decay; in *one-shot* mode (the default) the decay starts right after the attack and note-off is ignored. The decay is 10^(−3·t/`env.decay`), −60 dB after `env.decay` ms; the voice goes idle at −80 dB. The zone's trigger and loop fields are ignored: a slice plays once. The mode is read at the hit; `env.decay` is read live.
- **End of slice.** The sampler's end fade (64 frames or 1.5 ms) is at `end`, or at `start` for a reversed slice, then the voice goes idle.
- **Reverse.** A reversed slice plays from `end − 1` down to `start` (see the sampler's Zones section).
- **Pitch.** Ratio 2^((key − root + transpose + `tune.semi`)/12 + zone cents/1200 + CV·`cv.pitch_amount`/12), CV clamped to ±1 and updated every 16 samples. Slices normally have their key as root: unity ratio, so a slice is reproduced bit for bit once the attack is over.
- **Level and pan.** Voice gain = envelope × zone gain × (1 − `velocity.amp` × (1 − velocity)). A mono loop goes through a constant-power pan by the zone pan (−3 dB at the centre), a stereo loop is balanced (unity at the centre). The sum times `master.level` (5 ms smoothing) passes through the soft limiter of the poly-synth.
- **Filter and CV inputs (cutoff, level).** The slicer had no filter; it now has one global 12 dB low-pass (Butterworth, Q = 1/√2) over the summed voices, placed before `master.level`. It is engaged only while `cutoff_cv_in` is cabled or `filter.cutoff` is below its maximum of 20 kHz; otherwise the signal is not touched, so a patch that does not use either is bit for bit what it was before. Each CV is clamped to −1..+1 and multiplied by the signed amount param.
  - *Cutoff:* `filter.cutoff` × 2^(`cv.cutoff_amount`·CV), clamped to 20 Hz … 0.45 × sample rate, updated every 16 samples with the coefficient ramped in between. It acts on all voices together, not per voice.
  - *Level:* the master gain is multiplied per sample by 10^(`cv.level_amount`·CV/20), before the soft limiter. No extra smoothing.
- **`loop.beats`** is stored but unused by the audio; the app and the editor use it for grid slicing and for writing the loop as a note clip.
- **Map swaps.** Exactly as the sampler and the drum machine; a pending hit survives only if the new map has a slice zone of its key at the same index that still matches its velocity.
- **No map or no matching zone:** silence. `reset` silences the voices and keeps the maps.

---

## Step sequencer: `atomik.step-sequencer` (Utility, "Players")

A transport-synced, monophonic pattern player. Like the sampler it plays data installed by the app (not params): the **pattern data** (`StepSeqData`, up to 8 patterns of 1 to 64 steps, each with a resolution 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32 or 1/32T, a swing of 0 to 75 % and, per step, on/off, key, velocity 1 to 127, gate 1 to 100 % of the step, probability 0 to 100 % and tie). It is registered with `register_with_player_data`. Sources: [crates/devices/src/step_sequencer.rs](../crates/devices/src/step_sequencer.rs), shared clock in [crates/devices/src/note_clock.rs](../crates/devices/src/note_clock.rs).

### Sockets

| Key | Kind | Direction | Notes |
|---|---|---|---|
| `notes_in` | Notes | in | Held keys transpose the pattern when `follow` is on; `AllOff` closes the sounding note |
| `notes_out` | Notes | out | On and Off at sample-accurate offsets |
| `gate_cv_out` | CV | out | 1 while a note sounds (a tie keeps it high), else 0 |
| `pitch_cv_out` | CV | out | (key − 60) / 24 clamped to ±1, held after the note; with `cv.pitch_amount` 24 it is exact semitones |
| `velocity_cv_out` | CV | out | velocity / 127, held |
| `trig_cv_out` | CV | out | A 1 ms pulse at every played step start (ties retrigger it too) |

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `pattern` | 1 to 8 | 1 | none | Linear | yes |
| `pattern.switch` | 0–1: now, at-end | 1 (at-end) | choice | Linear | yes |
| `transpose` | −24 to 24 | 0 | semitones | Linear | yes |
| `follow` | 0–1: off, transpose | 0 (off) | choice | Linear | yes |
| `probability.amount` | 0 to 1 | 1 | percent | Linear | no |
| `random.seed` | 0 to 999 | 0 | none | Linear | yes |
| `mute` | 0 to 1 | 0 | toggle | Linear | yes |
| `velocity.scale` | 0 to 2 | 1 | none | Linear | no |
| `gate.scale` | 0.25 to 2 | 1 | none | Linear | no |

### Behaviour (as built)

- **Position.** The device reads the musical position from the engine (`TimeInfo.tick`, `ticks_per_frame`, `loop_wrap`) and derives the step every batch: step *k* = floor(tick / *s*) with *s* the step length in ticks (1/4 = 15 360, 1/8 = 7 680, 1/8T = 5 120, 1/16 = 3 840, 1/16T = 2 560, 1/32 = 1 920, 1/32T = 1 280). Step *k* starts at *k*·*s* plus, for odd *k*, the swing shift (swing·*s* + 100) / 200 ticks (integer division). The pattern step is *k* mod length, so a loop whose length is a multiple of the pattern repeats it in phase and a locate lands on the step a continuous run would have reached. Without a song the transport always plays.
- **Frames.** A step is due at the first frame ceil((start − batch tick) / ticks per frame); a value within 1e-6 frame above an integer counts as that integer (the f64 tick accumulates noise). Arithmetic is f64 `+ − × ÷`, `floor`, `ceil` only: bit-identical on every platform and independent of the host block size.
- **Notes.** One note per step: On at the step start, Off at start + max(1, floor(*s* × gate% / 100)), never past the next step's start; 100 % ends exactly at the next step (legato, the Off goes before the next On at the same frame). `gate.scale` scales the step's gate (result clamped to the step); `velocity.scale` scales the velocity (rounded, clamped to 1…127); `transpose` and the `follow` offset (held key on `notes_in` − 60, the most recent key wins, none = 0) shift the key (clamped to 0…127).
- **Tie.** A tied step holds its note into the next step: the same key continues without On or Off (the trig still fires), a different key emits the new On before the old Off at the same frame (legato), and a next step that does not play (off, probability, mute, no data) ends the note at that step's start. A tied step ignores `gate.scale`.
- **Probability.** A step plays when hash64(seed ⊕ `random.seed`, global step number, slot 0) maps below prob × `probability.amount`: 0 % never, 100 % × 1 always. No running RNG, so a bounce, a live run, a locate and a second loop pass agree.
- **Patterns.** `pattern` selects the playing pattern. `at-end` adopts a change at the start of the next pass (global step a multiple of the playing pattern's length) and, when stopped or after a jump, at once; `now` adopts it at the next batch. A pattern with another resolution or swing re-grids the clock at that point.
- **Note safety.** The sounding note is closed (Off at the first frame, or at the wrap offset) on stop, locate, loop wrap, `mute` and `AllOff`; a `reset` makes the next batch start with an `AllOff` if a note was sounding. A pattern-data swap never touches a sounding note (its end is already scheduled). A tempo change inside a batch is not a jump: the jump test accepts a tick that differs from the prediction by up to 64 × |ticks-per-frame change|.
- **No data:** nothing plays (the clock still runs). **Meter values:** pattern index (0-based), step index of the last started step, 1 while the transport runs.

---

## Arpeggiator: `atomik.arpeggiator` (Utility, "Players")

Turns the held keys on `notes_in` into a tempo-synced figure on `notes_out`. No pattern data. Source: [crates/devices/src/arpeggiator.rs](../crates/devices/src/arpeggiator.rs).

### Sockets

The same six as the step sequencer, in the same order: `notes_in`, `notes_out`, `gate_cv_out`, `pitch_cv_out`, `velocity_cv_out`, `trig_cv_out`.

### Params

| Key | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|
| `mode` | 0–5: up, down, up-down, random, as-played, chord | 0 (up) | choice | Linear | yes |
| `rate` | 0–6: 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32, 1/32T | 3 (1/16) | choice | Linear | yes |
| `octaves` | 1 to 4 | 1 | none | Linear | yes |
| `gate` | 5 to 100 (percent of the step, 100 = legato) | 80 | none | Linear | no |
| `swing` | 0 to 75 (percent) | 0 | none | Linear | no |
| `hold` | 0 to 1 | 0 | toggle | Linear | yes |
| `velocity.mode` | 0–1: input, fixed | 0 (input) | choice | Linear | yes |
| `velocity.fixed` | 1 to 127 | 100 | none | Linear | yes |
| `run` | 0–1: transport, always | 1 (always) | choice | Linear | yes |
| `random.seed` | 0 to 999 | 0 | none | Linear | yes |

### Behaviour (as built)

- **Order.** The held set is sorted by key for up, down, up-down and random and kept in press order for as-played. `octaves` stacks transposed copies (key + 12 × octave; a key above 127 is folded down by octaves). Up-down does not repeat its end points (set of 3 keys: 60 64 67 64 60 64 …). Random draws a uniform index of the pooled notes with hash64(seed ⊕ `random.seed`, global step number, slot 1). Chord plays the whole held set at every step, shifted by one octave per step cycling through `octaves`.
- **Time.** Same grid, integer tick rule and swing as the step sequencer. A key pressed mid-step waits for the next step boundary (a key at the boundary frame counts). The cycle restarts from its first note when the set goes from empty to non-empty (or is replaced by a fresh hold set).
- **Gate.** Off at start + max(1, floor(*s* × `gate`% / 100)), never past the next step; 100 % ends exactly at the next step. Releasing a key does not cut a sounding note: its scheduled end closes it.
- **Hold.** Released keys stay in the set; the next press after every physical key is up starts a fresh set; turning `hold` off drops the latched keys that are not held.
- **Run.** With `always` and the transport stopped the arpeggiator uses an internal clock that advances at the current tempo from the last playhead tick; `transport` is silent while stopped (sounding notes are closed). Starting or stopping the transport changes the clock source, which counts as a jump (notes closed, no hang).
- **Velocity.** The key's input velocity, or `velocity.fixed` / 127. **CV outs** as the sequencer (pitch CV of the last note of a step).
- **Note safety.** Notes are closed on a jump (locate, loop wrap, start or stop), `AllOff` (which also clears the set); the pending-off table (64 notes) closes the oldest note if a chord overflows it; a `reset` leaves an `AllOff` for the next batch.
- **Meter values:** the held key count, then the next 8 keys it will play (0 = none; in chord mode the lowest key of the chord).

## Container: `atomik.container` (Container, "Containers")

A device whose inside is another rack. Source: [crates/devices/src/container.rs](../crates/devices/src/container.rs). It has **no DSP**: the model flattens it into its inner devices before the engine sees it, so the engine never instantiates one. The registered factory builds a pass-through (`in_l` to `out_l`, `in_r` to `out_r`, notes ignored) only for descriptor-only users such as patch files and registry tests; the render patch builder rejects the type with a message.

### Sockets

| Key | Kind | Direction |
|---|---|---|
| `in_l`, `in_r` | Audio | In |
| `notes_in` | Notes | In |
| `out_l`, `out_r` | Audio | Out |

Inside, the pseudo-device `boundary` mirrors them: its outputs `in_l`, `in_r` carry what arrives at the container, its inputs `out_l`, `out_r` leave through it (it has no notes socket: notes enter through the note splits).

### Params

| Key | Id | Range | Default | Unit | Taper | Discrete |
|---|---|---|---|---|---|---|
| `macro1` to `macro8` | 0 to 7 | 0 to 1 | 0 | none | Linear | no |

The macro values are applied to inner params outside the engine (one `set_param` per mapped target; linear `min + (max - min) * m`, then clamp and rounding of discrete and choice params).

### Behaviour (as built, device half)

- The container can be an instrument or an effect; it has no console channel at creation (TO CH adds one). The latency of the flattened inside is the sum along the paths of the inner devices, compensated at summing inputs by the ordinary delay compensation (tested with a hand-built flat graph in `crates/engine/tests/flat_container.rs`).
- **Latency limit (as at the rack level):** only cables into **summing** inputs are delay compensated. Two inner paths with different latencies that meet in a plain input (for example the inputs of a line mixer inside) stay apart by the difference; align them yourself or end them in a console channel. The engine latency itself is the fixed batch FIFO; device latency is not trimmed from the output.

## Note split: `atomik.note-split` (Utility, internal)

Routes one notes input to up to eight outputs by key and velocity range. **Internal**: listed in `atomik_devices::INTERNAL_TYPE_IDS`, never shown in the browser or offered by an add menu, and created only by the model's container expansion (one per container, keyed `<container path>/~split`, its params set from the container's splits). Source: [crates/devices/src/note_split.rs](../crates/devices/src/note_split.rs).

### Sockets

`notes_in` (Notes in), then `out_1` to `out_8` (Notes out).

### Params

Six per slot *k* = 1 to 8, ids `6(k-1)` to `6(k-1)+5`, all discrete, in this order:

| Key | Range | Default | Unit |
|---|---|---|---|
| `sK.on` | 0 to 1 | 0 | toggle |
| `sK.key_lo` | 0 to 127 | 0 | none |
| `sK.key_hi` | 0 to 127 | 127 | none |
| `sK.vel_lo` | 1 to 127 | 1 | none |
| `sK.vel_hi` | 1 to 127 | 127 | none |
| `sK.transpose` | -48 to 48 | 0 | semitones |

### Behaviour (as built)

- A Note-On goes to **every** slot that is on and whose key range and velocity range (velocity as `round(v * 127)`, at least 1) contain it, both ends inclusive. Overlapping slots layer. The On is sent with the key transposed and clamped to 0..=127 and the velocity unchanged.
- **Route table.** Each slot keeps, per input key, the output key it sent, and which input key owns the open note on each output key. The matching Off (and any later Off of that key) goes to the same output with the same output key, whatever the zone params are by then (the same rule as the sampler's voices); an Off without an open On is dropped. Zone params affect new Ons only.
- **Balance.** Every output stays balanced: a repeated On of a key still open on that slot is preceded by an Off of the old output key, and when two input keys land on one output key (transpose clamping) the newer one takes the note over, closing the old one first. A proptest over random event and param-change scripts leaves no hung note.
- `AllOff` is forwarded to every output and clears the tables. `reset` clears them and, if any note was open, leaves an `AllOff` on every output for the next batch.
- Offsets are preserved and non-decreasing per output. An output that is not connected simply drops its events. Latency 0. `process`, `set_param` and `reset` never allocate (fixed 8 x 128 tables).

---

## Third-party plugin devices: `clap.<id>` and `vst3.<id>`

A CLAP or VST3 plugin the scanner found is a device type of its own: `clap.<the plugin's CLAP id>` (for example `clap.atomik.test.synth`) or `vst3.<the 32 hex digits of its class id>`. The two formats work the same way in the rack (cables, console, containers, automation, state, bounce) and each device shows its format badge, CLAP or VST3; the browser filter also matches the format name. The prefixes `clap.` and `vst3.` are reserved; the device is added from the browser's **Plugins** group (`PluginCommand::Add`), never by the generic add command. Plugins are **off until you enable them** in Audio > Plugins (a warning explains that a plugin is native code with the same access as the app); a project never stores a path, it finds its plugins by id in your scan cache (`plugins.json` in the config folder, filled by the out-of-process scanner `atomik-plugin-scan`).

### Sockets

Built from the plugin's ports, in this order: `in_l`, `in_r` (when it has a main input), `sc_in_l`, `sc_in_r` (a sidechain input), `notes_in`, `out_l`, `out_r` (a main output), `notes_out`. The kind is Instrument (notes in and audio out, and the plugin says so), Utility (a note effect: notes out, no audio) or Effect. A mono main bus is mapped (input = (L + R) / 2, the output copied to both sides). Further buses are disabled.

### Params

`bypass` (index 0, discrete 0 or 1, host-owned) and `p<id>` for every visible, writable plugin param (the plugin's own 32-bit id in decimal, stable across sessions), in the plugin's order, at most 2 048. A value is the plugin's own plain value (CLAP: the plugin's value; VST3: the normalised value 0 to 1, or the step index of a stepped param); a stepped param is rounded. Params are saved sparsely (only the ones you set) next to the plugin's opaque **state** blob, and edits reach the plugin as events at the start of each 64-frame batch. The panel pins up to 16 params as knobs; PARAMS opens the searchable list with every param (also the editor for a plugin without a window).

### Behaviour (as built)

- **64 frames.** The plugin is always called with exactly 64 frames, so the live path, a bounce and every host block size feed it the same blocks.
- **Instances.** Opening a project (or adding a device) inserts a silent stand-in at once and creates the real instance one per frame, with "Loading plugins 3/8" in the status bar. An audio restart keeps the instances (they are only re-activated at the new rate). A removed device's plugin is destroyed on the UI thread after the engine let go of it.
- **State.** The plugin's state is captured before every save, when its editor closes, a quiet second after the plugin says it changed, before an engine restart or the removal of its device, and stored in the project (never an undo step). A state that failed to restore is kept as it was and shown as FAILED until RELOAD.
- **Missing plugins.** A plugin that is not installed, disabled, or on a machine without plugins enabled opens as a stand-in (an instrument is silent, an effect passes its dry input) with MISSING and the reason; cables, console channel, track, automation, macro targets, pins and state are kept and the project saves unchanged. After a rescan finds the plugin, or with RELOAD, it is created with its saved state.
- **Editor.** OPEN GUI opens the plugin's own window (a separate native window per plugin, never inside the rack window). Parameter changes made there are one undo step per gesture; a patch change inside the plugin is not undoable. VST3 editors are hosted the same way (`HWND`, `NSView`, an X11 window id) and follow the window's size both ways. Wayland has no embedding for either format: use PARAMS there (or run Atomik under XWayland). On Linux a VST3 editor that needs file-descriptor handlers from the host's run loop stays dead (timers work); use PARAMS for it.
- **Latency and PDC.** The latency the plugin reports is compensated like any device latency. A plugin announces a latency change (CLAP `latency.changed` and a restart request, VST3 `restartComponent(kLatencyChanged)`); the host restarts it, reads the latency again after the re-activation (VST3 also after every `setActive` and every state restore) and the other channels are re-aligned, with the one-click discontinuity of the new delay lines. A latency that comes from a saved param (or from a container macro the song automates) is therefore followed live once the plugin has applied the value, and by a bounce from its first frame (see below).
- **Crashes.** A plugin runs inside the app, so a native crash takes the app with it. The app lists the loaded plugins in a journal (`plugins-in-use.json`) and, while a plugin is loaded, writes a recovery snapshot of the project every 60 s to `recovery/` (the last three are kept, never over your file). After an unclean exit it offers to disable those plugins and to open the newest snapshot.
- **Bounce and determinism.** `atomik-render` refuses a project with plugins unless `--allow-plugins` is given, reads the same `plugins.json` (or `--plugin-cache`), builds the plugins through the same code as the app (state, then params, then 64-frame blocks) and prints `plugins: N (determinism not guaranteed)`. The host gives a plugin the same inputs for every `--block` and `--workers`, so the output is bit-identical across blocks, workers and runs **only for plugins that are deterministic functions of those inputs** (the in-repo test plugins are). Many real synths, choruses and analog-modelled reverbs use time, random seeds or threads and are not; a bounce is also not bit-identical to live playback. Compare real plugins with a null test. `--plugin-mode realtime` runs the plugins in their live mode (the default is offline mode), `--missing-plugins stand-in` bounces with stand-ins instead of failing.
- **How a bounce treats restarts and latency.** A plugin can ask the host to restart it, and a restart is how a changed latency reaches delay compensation, so a bounce that only rendered would lose the compensation a project has live. A bounce of a project with plugins therefore does what the app does once per frame, once per **64-frame batch** instead of once per frame: before the song starts it renders silent batches with the transport stopped until no plugin has restarted or changed its latency for 8 batches in a row (at most 256), polling the plugin manager and recompiling the routing with the published latencies after every batch; then it renders the song in 64-frame batches with the same poll and recompile after each one. The cadence is counted in batches, never in wall-clock time, so the output does not depend on the worker count and a restart lands on the same batch in every run; because the engine is driven in batches, `--block` has no effect on a plugin project. A latency that changes mid-song (a container macro automated by the song moving a plugin's latency param) is followed the way live follows it. A change of a plugin's param table during a bounce is not followed (the bounce keeps its devices) and a plugin that fails or cannot restart is reported as a warning. Projects without real plugins use the plain offline loop, unchanged.
- **Patches.** A patch file cannot hold a plugin: exporting a rack with plugin devices is refused.

Try it with the test plugins: [../examples/projects/plugin-demo](../examples/projects/plugin-demo/README.md) (CLAP) and [../examples/projects/plugin-demo-vst3](../examples/projects/plugin-demo-vst3/README.md) (both formats in one project).

VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries. Atomik Rack uses no VST logo; the name VST3 only says which plugin format is hosted.

## Console master: `atomik.master` (Mixer)

The master section of the console. Source: [crates/devices/src/master.rs](../crates/devices/src/master.rs). The console creates it; you do not add it from the browser. Eight send buses and eight FX returns, a bus compressor with a key input, an insert loop, the fader, the main output and the control-room (`monitor_*`) output.

- **Send level.** `send1.level` to `send8.level` (ids appended after `monitor.source`, so every older id and key is unchanged): -60 to 6 dB (-60 = off), default 0 dB, decibel taper, the same as `return{k}.level`; automatable. `fx_send{k}_*` = `send_bus{k}_*` times the level, at stage 0. At 0 dB the signal is copied untouched (bit-exact, the same bits as before the param existed). A level at its default is not written to the project file, so a project that never touched it saves the same bytes and an older project opens at 0 dB.
- **Control-room source.** `monitor.source` is a pick list of 18 entries: `Main` (0, the post-fader main mix), `FX returns` (1, the returns sum alone), then appended `Send 1` to `Send 8` (2 to 9, the `fx_send{k}` signal after the send level) and `Return 1` to `Return 8` (10 to 17, the return after its fader and pan, before it joins the main sum). Stored values keep their meaning; an out-of-range value clamps to the last entry. Changing from one tap to another crossfades over 5 ms and settles bit-exactly on the new tap. Dim (-20 dB) and the control-room level apply to every source and never touch `main_*`.
- **Taps.** The send and return taps are copies the master keeps per block in its own state (written in stages 0 and 1, read in stage 2); the stages, sockets and cables of the device did not change, and the audio thread still does not allocate.
- **Key.** `comp.key` selects the `key_in` side-chain; adding a cable into `key_in` through a command also switches it on in the same undo step . Removing the cable leaves the switch on; loading a project never changes a param; a channel's `key_in` is unchanged.
- **In a new project.** File, New starts with Audio I/O on top and this device, named "Master", right under it, locked: it cannot be deleted, cut, copied or moved, and nothing can be dragged above it (older projects open as they are). `main_l/r` are cabled to Audio I/O outputs 1 and 2. The channels' links to it are shown as "Output: Master" on the channel instead of as cables ("Show Console Links" in the View menu draws them).
- **Panel.** Per send bus a level fader and the name of the effect on the slot; per return the effect's name (or `free`) and an EDIT button that selects the effect and scrolls the rack to it; the source is a pick list of the 18 entries; the key switch has a lamp that is lit while a cable ends at `key_in`.
- **Use as Send Effect.** Creating an effect with the master selected (Ctrl+E) patches it into the first free send and return in one undo step (`fx_send{k}` to the effect to `return{k}`); the right-click entry "Use as Send Effect" does the same for an existing, unrouted stereo effect and is disabled for one that is already routed. With all eight slots used the effect is added unrouted and the status line says "All 8 send and return slots are in use".

## Audio I/O: the `interface` pseudo-device

Called **Audio I/O** everywhere you read it (panel title, rack caption, menus, tooltips, status text); the reserved device key `interface` and the type id are unchanged. Its sockets are the host inputs `in_1..in_N` (outputs of the device, N = `input_channels`) and the host outputs `out_1..out_M` (inputs, M = `output_channels`), up to 32 each.

- **Lamp per socket.** Green: a cable ends or starts there and the host audio device has that channel. Yellow: no cable. Red: a cable exists but the selected audio device has fewer channels than the socket number (the audio still routes and is silent on the device). With no stream (audio off, the offline sink) every channel counts as present.
- **Meter and clip lamp per socket.** A peak meter on every socket and a clip lamp that lights when the peak went above 0 dBFS since the last reset; a click on the lamp resets it, a click on the panel's plate resets all. Inputs show the host signal; an output shows the peak of whatever is cabled to it (the master's `main`, a channel).
- **Audio tracks.** The input choice in an audio track's header makes real `interface:in_k -> track in_l/in_r` cables (visible on the back of the rack), as before.

---

## How to verify these tables

The descriptors are plain `static` tables, so you can read them directly:

```sh
grep -n -A7 'param($' crates/devices/src/poly_synth.rs     # synth params: id, key, name, (min, max, default), taper, unit, discrete
grep -n -A9 'ParamDescriptor {' crates/devices/src/stereo_delay.rs
grep -n -A9 'ParamDescriptor {' crates/devices/src/reverb.rs
grep -n -A9 'ParamDescriptor {' crates/devices/src/compressor.rs crates/devices/src/distortion.rs
grep -n 'static PARAMS' -A27 crates/devices/src/eq.rs   # EQ params use small const constructors (on, freq, gain, q, slope)
grep -n -A10 'const fn level\|const fn pan\|const fn toggle' crates/devices/src/line_mixer.rs
```

These tests check the keys, their order and descriptor validity (ids equal indices, unique keys, defaults in range):

```sh
cargo test -p atomik-devices descriptor_validates
cargo test -p atomik-devices register_all_registers_nine_devices_in_sorted_order
```

They do not check ranges or defaults against this page. If you change a descriptor, update this page by hand.
