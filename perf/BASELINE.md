# Benchmark baseline

Wall-clock medians and 99th percentiles (21 samples) of the benches in
[docs/TESTING.md](../docs/TESTING.md#benchmarks-and-the-performance-job), taken once on the
reference machine below with the default settings (`cargo bench -p <crate> --bench <name>`).
Re-measure on your own machine before comparing: only numbers from one machine compare.
Instruction-count baselines (`perf/baseline-instructions.txt`) and binary-size baselines
(`perf/baseline-sizes.txt`) need the CI runner: how they are made is in
[docs/TESTING.md](../docs/TESTING.md#baselines-of-the-count-gates).

## Reference machine

- CPU: AMD Ryzen 9 3950X, 16 cores, 32 threads
- Memory: 64 GB
- OS: Windows 11 Pro
- Toolchain: rustc 1.99.0 (bench profile: release with debug info)
- Power plan and background load: desktop session, no other builds running during the runs

## Results

```text
render/rack/workers0/block256: median 741.458 ms  p99 854.694 ms
render/rack/workers2/block256: median 474.366 ms  p99 508.582 ms
render/song/workers0/block256: median 145.504 ms  p99 211.111 ms
render/song/workers2/block256: median 118.787 ms  p99 133.959 ms
render/console-64/workers0/block256: median 1.605 s  p99 1.784 s
render/console-64/workers2/block256: median 915.424 ms  p99 960.019 ms
render/lanes-200/workers0/block256: median 188.216 ms  p99 216.964 ms
render/lanes-200/workers2/block256: median 208.444 ms  p99 222.571 ms
render/lanes-2000/workers0/block256: median 1.895 s  p99 2.031 s
render/lanes-2000/workers2/block256: median 1.919 s  p99 1.980 s
render/large-song/workers0/block256: median 1.255 s  p99 1.444 s
render/large-song/workers2/block256: median 888.830 ms  p99 939.125 ms
document/rack/save: median 322.391 us  p99 336.014 us
document/rack/load: median 1.885 ms  p99 1.999 ms
document/rack/validate: median 11.981 us  p99 12.364 us
document/rack/clone: median 72.135 us  p99 103.861 us
document/rack/expand: median 182.758 us  p99 227.651 us
document/rack/edit+undo: median 1.107 us  p99 1.336 us
document/song/save: median 253.312 us  p99 342.057 us
document/song/load: median 1.272 ms  p99 1.780 ms
document/song/validate: median 18.006 us  p99 30.269 us
document/song/clone: median 46.635 us  p99 62.376 us
document/song/expand: median 96.263 us  p99 139.448 us
document/song/edit+undo: median 1.303 us  p99 1.314 us
document/large/save: median 2.962 ms  p99 4.232 ms
document/large/load: median 39.007 ms  p99 60.991 ms
document/large/validate: median 317.821 us  p99 328.058 us
document/large/clone: median 559.723 us  p99 671.617 us
document/large/expand: median 1.397 ms  p99 1.985 ms
document/large/edit+undo: median 854.0 ns  p99 952.8 ns
dsp/biquad-stereo/4096: median 20.137 us  p99 20.979 us
dsp/svf-lowpass/4096: median 21.846 us  p99 22.078 us
dsp/chorus-stereo/4096: median 295.573 us  p99 313.935 us
dsp/compressor-stereo/4096: median 137.688 us  p99 139.815 us
device/arpeggiator/256batches: median 57.627 us  p99 61.035 us
device/bus/256batches: median 306.620 us  p99 311.140 us
device/channel/256batches: median 294.482 us  p99 316.491 us
device/character/256batches: median 6.089 ms  p99 6.148 ms
device/compressor/256batches: median 1.306 ms  p99 1.318 ms
device/cv-sequencer/256batches: median 50.754 us  p99 73.895 us
device/distortion/256batches: median 2.575 ms  p99 3.888 ms
device/drum-generator/256batches: median 22.860 us  p99 30.201 us
device/drum-machine/256batches: median 635.944 us  p99 892.400 us
device/eq/256batches: median 925.706 us  p99 1.240 ms
device/fm-synth/256batches: median 11.184 ms  p99 11.499 ms
device/granular/256batches: median 5.986 ms  p99 6.735 ms
device/guitar-amp/256batches: median 11.041 ms  p99 11.959 ms
device/key-chord/256batches: median 5.905 us  p99 6.833 us
device/line-mixer/256batches: median 172.675 us  p99 192.473 us
device/loop-slicer/256batches: median 434.060 us  p99 480.367 us
device/macro-bank/256batches: median 4.801 us  p99 5.420 us
device/master/256batches: median 438.424 us  p99 691.482 us
device/mastering/256batches: median 6.441 ms  p99 8.165 ms
device/midi-out/256batches: median 20.764 us  p99 21.061 us
device/mod-source/256batches: median 1.601 ms  p99 1.640 ms
device/modulation/256batches: median 1.734 ms  p99 2.419 ms
device/note-delay/256batches: median 6.359 us  p99 9.787 us
device/note-filter/256batches: median 5.945 us  p99 6.245 us
device/note-repeater/256batches: median 6.285 us  p99 7.096 us
device/note-split/256batches: median 18.034 us  p99 20.061 us
device/phase-meter/256batches: median 2.246 ms  p99 2.491 ms
device/poly-synth/256batches: median 2.484 ms  p99 2.801 ms
device/reverb/256batches: median 3.937 ms  p99 5.208 ms
device/sampler/256batches: median 363.186 us  p99 443.280 us
device/spectrum/256batches: median 137.103 us  p99 139.704 us
device/step-sequencer/256batches: median 23.255 us  p99 34.728 us
device/stereo-delay/256batches: median 480.575 us  p99 775.217 us
device/wavetable-synth/256batches: median 3.882 ms  p99 5.511 ms
```

## Render workers used only while the audio thread needs them (2026-11-22)

Three measurements of the load rule (`crates/engine/src/load_gate.rs`; the rule and its reasons are in
[DECISIONS.md](../docs/DECISIONS.md#bg-the-render-workers-are-used-only-while-the-audio-thread-needs-help)).

**Bounce with two workers, what a bounce hands over.** `cargo bench -p atomik-render --bench render_batch -- "rack|console-64|lanes"`
with `ATOMIK_BENCH_GATE=dispatch` and `ATOMIK_BENCH_GATE=levels` (a switch of the bench binary that sets the mode engines
start in; the value `levels` existed only in the measured build of the first commit of this change, which had the
per-level rule, and the current tree accepts `dispatch`, `inline` and `load` only, so the per-level column cannot be
reproduced from it), five rounds, one binary, the two settings alternating round by round so that the drift of the machine
cannot favour one; medians of the five runs, and for workers 0 the median of all ten (the setting does not touch it).
"dispatch" is what the engine did before the rule and what a bounce does now (every level of two or more devices goes
to the workers). "per level" is a rule tried first and removed: it decided each level from the time spreading it would
save (at least 7 us to dispatch, under 3.5 us to run inline, 64 batches of the opposite verdict to change). The ratios
are to the workers 0 median of the same session.

```text
case                 workers 0    workers 2, dispatch    workers 2, per level
rack                 765.9 ms     544.9 ms (0.71)        561.2 ms (0.73)
console-64           1.350 s      827.1 ms (0.61)        817.6 ms (0.61)
lanes-200            168.1 ms     161.4 ms (0.96)        175.3 ms (1.04)
lanes-2000           1.745 s      1.599 s (0.92)         1.533 s (0.88)
```

Per level earned nothing (it ties on rack and console-64, loses on lanes-200 and gains 4 points on lanes-2000), so the
bounce keeps dispatching every level and the rule that remains is the load rule. The earlier claim that cheap levels
are slower with workers does not hold on this machine: lanes-200 is faster with two workers than with none.

**The paced callback benchmark** (`cargo test -p atomik-engine --release --test worker_linger -- --ignored --nocapture`:
a callback every 128 or 480 frames at 48 kHz, paced like an audio thread, 10 s per row, four workers, linger 500 000).
"inline" is no workers, "dispatch" hands every level over, "live" is the load rule of a live engine. "share" is the
median callback of the inline row as a percentage of the callback period: the load the audio thread alone carries. The
patches are one synth with one voice (light), four synths with six voices each (existing) and six synths with sixteen
voices each (heavy). Callback times in microseconds.

```text
patch     frames  mode      p50     p99     max   share %  busy   parks   cpu % of a core
light     128     inline    14.2    31.2    92.8   0.5     no         0   50.8
light     128     dispatch  14.0    29.4    93.3          yes        4   48.9
light     128     live      14.7    31.6    75.3          no         4   50.5
light     480     inline    45.7    84.5   125.5   0.5     no         0   13.3
light     480     dispatch  44.8    86.1   114.2          yes        4   10.0
light     480     live      46.5    89.9   146.6          no         4   12.5
existing  128     inline    87.0   144.3   208.3   3.3     no         0   51.6
existing  128     dispatch  67.1    85.1   164.6          yes   26 247   58.6
existing  128     live      84.0   143.5   184.3          no     5 253   53.0
existing  480     inline   328.7   522.7   612.3   3.3     no         0   13.9
existing  480     dispatch 197.8   279.3   397.3          yes    7 004   39.2
existing  480     live     318.6   516.6   622.7          no     1 411   12.5
heavy     128     inline   327.0   502.3   605.5  12.3     no         0   51.6
heavy     128     dispatch 159.5   259.8   377.2          yes   29 996   74.5
heavy     128     live     311.2   500.5   743.4          no     5 989   54.9
heavy     480     inline  1201.4  1911.0  2061.6  12.0     no         0   26.4
heavy     480     dispatch 574.1   738.8   866.7          yes    8 004   44.1
heavy     480     live    1190.8  1794.4  1983.4          no     1 609   30.6
```

The pacing loop itself costs about 50 points at 128 frames (the first row, with no workers and a nearly empty
callback). None of the three patches reaches the enter share (30 %), so a live engine runs all of them inline after
the first second (the parks of the live rows are those of the start, while the plan was busy): the inline p99 of the
heaviest, 500 us at 128 frames and 1.8 ms at 480, is 19 % and 18 % of the callback period. A plan over 30 % cannot be
built from this device set within the poly synth's voice limit; the tests of the rule (`crates/engine/tests/load_gate.rs`)
cover that side with devices of chosen cost.

**The running app**, `scripts/perf/sample-app.ps1` on the release build of each side (the base of this change and
this change), transport stopped, nothing sounding, no `--play`, 15 s settle, 20 s window, three rounds with the two
alternating; CPU is a percentage of one core:

```text
scenario           before (3 rounds)      after (3 rounds)
stress (auto)      76.4, 80.7, 85.7       16.8, 16.4, 19.0
stress-workers2    30.1, 33.8, 32.9       17.6, 18.5, 18.4
stress-workers0    17.6, 17.9, 17.0       18.1, 17.1, 17.9
empty              0.31, 1.09, 0.47       0.70, 0.55, 0.70
```

After the review fixes (the seen load, the probe guard, the rule while playing) the same rows were taken again on the
final build, three rounds of the stopped rack and one of the reference: stress (auto) 19.6, 17.5, 23.7; stress-workers2
17.2, 16.3, 17.0; stress-workers0 17.5. The stopped rack still calms down.

The stopped stress rack with automatic workers (7 here) and with two workers now costs what it costs with none, since
its devices take about a seventh of the callback time (about 220 us in a 1.33 ms batch) and the workers sleep. The empty
project is unchanged within the spread of the sampler (its idle figures read 0.3 to 1.1 on either side).

## Model commands on a growing song

`cargo bench -p atomik-model --bench commands`: a command and its undo on a song of 10 tracks and
100 clips with 10^3, 10^5 and 10^6 notes (medians of 21 samples, same machine). Before is the
document that copies the song and every note for each command; after is the shared song and note
lists with in-place undo.

```text
case             notes   before      after
adddevice        10^3    185.4 us    156.5 us
adddevice        10^5    257.5 us    173.5 us
adddevice        10^6    7.376 ms    183.5 us
addaudiotrack    10^3    267.5 us    216.2 us
addaudiotrack    10^5    418.9 us    223.1 us
addaudiotrack    10^6    15.485 ms   222.3 us
addchannel       10^3    51.2 us     41.0 us
addchannel       10^5    152.5 us    39.9 us
addchannel       10^6    4.465 ms    38.3 us
connect          10^3    79.8 us     56.2 us
connect          10^5    146.0 us    57.3 us
connect          10^6    4.285 ms    56.6 us
paste            10^3    288.6 us    230.3 us
paste            10^5    486.9 us    229.3 us
paste            10^6    11.665 ms   225.2 us
adddevice in a rack of 100 devices and a full container: 1.945 ms (not measured before)
```

## Drawing: text, waveform columns, playhead repaints

Same machine, release profile, medians. Text: 300 distinct labels shaped cold, then again warm
(`cargo test -p atomik-ui --release --test text_cache -- --ignored --nocapture`). Waveform:
`cargo test -p atomik-ui --release --test bench_wave -- --ignored --nocapture`, one channel of
1,920 columns over a peak level with a tempo-mapped source.

```text
case                                   before      after
text, 300 labels (cold / warm)         57.9 ms     33 us (warm; the first paint still shapes)
paint_channel, 1,920 columns           292.8 us    94.1 us
```

A playhead that moves inside one pixel no longer raises a repaint in the arrangement or the piano
roll, and `AppCore::frame` skips the shown-value pass while the revision, position and play state
are unchanged (tests `playhead_repaint`, `app_core::autoview_tests`).

## Arrangement playback frames

Same machine, release profile, a 64-track, 2,000-clip, 20,000-note song at 1920x1080, a
playhead moving a few pixels per frame, medians of 120 frames
(`cargo test -p atomik-ui --release --test bench_arrangement -- --ignored --nocapture`). "record"
paints into a display list (the cached layers are copied in); "record + encode" paints into a GPU
scene with the scene cache, as the app does.

```text
case                                   layers off    layers cached
record, per frame                      3.25 ms       1.37 ms (copying the 9,140 recorded ops)
record + encode, per frame             3.79 ms       0.135 ms
```

Lane and track list layers are recorded again only when something they draw changed; the
waveform columns of a clip are part of the lanes layer, so playback does not recompute them.

## Engine scheduling

Same machine, bench profile, medians (`cargo bench -p atomik-render --bench render_batch -- lanes`).
Lanes are applied only when their value changed; a bounce renders the same samples.

```text
case                                   before        after
render/lanes-200/workers0/block256     188.2 ms      158.6 ms
render/lanes-200/workers2/block256     208.4 ms      173.2 ms
render/lanes-2000/workers0/block256    1.895 s       1.720 s
```

The render workers are capped at cores - 1 and the level barrier yields its core after 2,000 empty
spins; both only matter on a live stream (see `atomik-play --stats` for the callback percentiles).

## CPU readout

Same machine, bench profile, medians of 21 runs (`cargo bench -p atomik-render --bench render_batch
-- "rack/workers0|console-64/workers0"`, with `ATOMIK_BENCH_TIMING=1` for the second column). The
per-device timing reads the clock twice around each device call. The app always has it on (the
engine measures every device all the time; View > Show Device CPU only shows the numbers), so the
second column is what the running app pays. A bounce renders the same samples either way.

```text
case                                   off           timing on
render/rack/workers0/block256          754.2 ms      777.0 ms
render/console-64/workers0/block256    1.486 s       1.565 s
```

The rack case is 100 cheap devices, so it is the worst case for the overhead: about 70 ns per
device per 64-frame batch, which is 0.5 % of the 1.33 ms callback at 48 kHz for 100 devices. With
the timing off (the first column) a device call pays two relaxed atomic loads.

## I/O speed

Same machine, release test profile (`cargo test -p atomik-model --release --test load_speed --
--ignored --nocapture`, best of 5; the import peak is `cargo test -p atomik-audio-file --test
import_peak -- --nocapture`, a 20 s stereo file; the WAV figures are `cargo test -p
atomik-audio-file --release --test wav_io -- --ignored --nocapture`, one minute of stereo).

```text
case                                           before        after
open 200 000 notes (100 tracks, 500 clips)     291 ms        78 ms
  allocations per note                         6.31          1.31
document/large/load (2 000 notes)              39.0 ms       7.7 ms
import peak, 44.1 and 48 kHz, in decoded sizes 3.23x         1.03x
write_wav_f32, one minute of stereo            31.6 ms       15.1 ms
read_wav_f32, one minute of stereo             70.4 ms       13.1 ms
reads per 256-frame chunk, sequential stream   1             1/16
```

The write figure before is hound's sample-at-a-time loop into memory, so it leaves the disk out of
the old side only.

## Device CPU

`cargo test -p atomik-devices --release --lib eq::tests::cost -- --ignored --nocapture`, one
64-frame batch of noise through the EQ at default settings.

```text
case                                  before       after
EQ, analyser on (always on before)    2065 ns      2065 ns
EQ, analyser off (panel not shown)    2065 ns      369 ns
```

The compressor and gate shortcuts (no `log10` under the knee, no `powf` for a steady gain) and the
masked delay line change no bit (tests compare them with the plain formulas); no timing is
recorded for them yet. A bypassed effect costs the wrapper's ring copy after two seconds.

## App shell

Same machine, release test profile. Save: `cargo test -p atomik-app --release --lib save_speed --
--ignored --nocapture` (best of 5; a 200 000 note project, 100 tracks, 500 clips, a 9 MB project
text, no plugins). "Blocking" is the whole save on the UI thread, which is what File > Save did
before; "UI thread" is what File > Save blocks now (the document clone and the bundle plan), the
rest runs on a worker. The plugin captures are one request per sandboxed plugin followed by the
answers, so they cost the slowest plugin instead of the sum; that needs real children and is a hand
check.

```text
case                                            before        after
Save, 200 000 notes, UI thread blocked          52.9 ms       0.7 ms
event-loop wakeups per second, a plugin loaded
  and nothing playing                           60            10
keymap.json stats per second                    4             0 (on focus and menu open)
ui-state.json writes per navigation click       1             at most one per 2 s
```

## A tuned release profile (measured, not adopted)

`atomik-render --stats` real-time factor, median of 3 runs on the Windows x86-64 development machine, the
`release` profile (`debug = 1`, default LTO and codegen units) against the same profile with `lto = "fat"` and
`codegen-units = 1` (`panic = "unwind"` kept in both). Every output bounced bit for bit the same.

```text
example                                  release    fat LTO, 1 unit    change
patches/fm-demo.json                     24.2x      24.7x              +2.1 %
patches/granular-demo.json               10.2x      10.7x              +4.9 %
patches/wavetable-demo.json              30.9x      32.1x              +3.9 %
projects/demo.atomik                     125.9x     131.3x             +4.3 %
projects/fx-demo                         15.2x      15.4x              +1.3 %
```

The gain is below the 5 % a separate `dist` profile needs to be worth its much longer link, so none was added and
the release builds keep the `release` profile.

## Real-time stretcher (interim warps)

Hand check, not a bench: `cargo test -p atomik-engine --test audio_interim_pitch --test audio_interim -- --ignored
--nocapture` (test profile, optimised) bounces 10 s of 16 stereo audio tracks at 48 kHz, each playing an interim warp
at 1.3x speed, on the reference machine:

```text
16 interim warps (resampled read), 10 s: about 510 ms (5.1 % of real time)
16 pitch-keeping interim warps (Tonal, +3 semitones), 10 s: about 890 ms (8.9 % of real time)
```

The real-time stretcher costs about 2.4 ms per second of stereo audio per clip, so 32 clips at once take about 8 % of
one core on top of the plain read.

## Idle baseline of the running app (2026-11-20)

The running app, not a bench: the rows were taken with an external sampler before the sampling script
(`scripts/perf/sample-app.ps1`, see [docs/TESTING.md](../docs/TESTING.md#measuring-the-running-app)) existed, at
commit `1940e40e`, on the reference machine above with the release build, the transport stopped and nothing
sounding. CPU is a percentage of one core, so 829 is about eight cores busy. The linger rows are the same
scenarios with `--linger` (the idle spins before a render worker parks). "Two cores / four threads" pins the
process to four logical processors with an affinity mask. The GPU column is the dedicated memory of the app's
process.

| Scenario | CPU % | Private MB | Working set MB | Threads | GPU dedicated MB |
|---|---|---|---|---|---|
| Empty project, no audio | 0.16 | 681 | 179 | 36 | 503 |
| Empty project, audio on | 1.6-2.3 | 686-697 | 188-191 | 47 | 503 |
| Empty project, audio on, minimized | 2.2-3.5 | 690-693 | 190 | 47 | 503 |
| Stress rack (100 devices), no audio | 0.23 | 727 | 219 | 36 | 503 |
| Stress rack, audio on, automatic workers (8) | 829 | 804 | 298 | 47 | 503 |
| Stress rack, audio on, 2 workers | 216-219 | 814-817 | 306-312 | 41 | 503 |
| Stress rack, audio on, no workers | 19.2-20.7 | 811-818 | 307-309 | 39 | 503 |
| Stress rack, 2 workers, worker linger 20 000 | 48-50 | 815-817 | 308 | 41 | 503 |
| Stress rack, automatic workers, worker linger engine default | 124 | 817 | 310 | 47 | 503 |
| Stress rack, no audio, rack and mixer detached | 0.16 | 801 | 282 | 42 | 757 |
| Two cores / four threads (affinity): stress rack, 3 workers | 310 | 799 | 309 | 42 | 503 |
| Two cores / four threads: stress rack, 1 worker | 116 | 784 | 295 | 39 | 503 |
| Two cores / four threads: stress rack, no workers | 20.4 | 823 | 307 | 39 | 503 |
| Two cores / four threads: stress rack, 3 workers, linger engine default | 73 | 811 | 307 | 42 | 503 |

## Idle wake-ups: full frame rate only while something moves (2026-11-21)

The same reference machine and release build, the transport stopped and nothing sounding (no `--play`). Before is
the branch this change started from, after is this change. The first two rows of CPU are the process CPU time over
40 s after a 20 s settle, read at the start and the end of the window with no other probe on the app, three runs
each (two for minimized, with the transport stopped: a minimized window with something
animating keeps the frame period): the numbers move by a few tenths of a percent from run to run, so the runs are listed.
The sampler rows are `scripts/perf/sample-app.ps1` (20 s window): its UI probe sends the window a message every
100 ms, which wakes the app about ten more times a second in both columns, and the settings file it writes keeps the
first-run audio setup panel (live meters, repaints all the time) from opening. "Wakes" is the app's own `ui activity`
line.

| Scenario | CPU % before | CPU % after | Wakes/s before | Wakes/s after |
|---|---|---|---|---|
| Empty project, audio on (40 s runs) | 0.86, 1.17, 1.29 | 0.43, 0.66, 0.86 | 60 | 10 |
| Empty project, audio on, minimized (40 s runs) | 1.29, 1.48 | 0.63, 0.74 | 60 | 4 |
| Empty project, no audio (sampler) | 0.31 | 0.31 | 6 | 6 |
| Empty project, audio on (sampler) | 0.93 | 1.25 | 62 | 14 |
| Empty project, audio on, minimized (sampler) | 1.63 | 0.78 | 62 | 6.5 |
| Stress rack, audio on, no workers (sampler) | 20.2 | 16.5 | 60 | 14 |

What is left with the stream open is the audio stream's own thread, not the interface: the empty project with audio
on is 0.3 to 0.4 points above the no-audio figure in the better runs. The status line's DSP figures are held steady
for ten seconds while they move by less than two points; before that, an idle engine's jitter in the rounded figure
repainted the whole window every couple of seconds (3 paints a second in the stress rack).

## Meter feeds with the engine running and nothing sounding (2026-11-21)

The same reference machine and release build, the transport stopped, no `--play`, `scripts/perf/sample-app.ps1`
(15 s settle, 20 s window, one run per row). Before is the branch this change started from (the idle wake item
already on it), after is this change. The stress rack's devices are silent, so their published values do not move;
the engine still publishes a new sequence with every batch.

| Scenario | CPU % before | CPU % after | GPU % before | GPU % after | Paints/s before | Paints/s after |
|---|---|---|---|---|---|---|
| Stress rack, audio on, no workers | 17.9 | 16.9 | 0.87 | 0.53 | 0.1 | 0.5 |
| Stress rack, audio on | 69 | 92 | 3.2 | 0.9 | 0.2 | 0 |
| Stress rack, no audio, rack and mixer detached | 0.78 | 0.47 | 0 | 0 | 0 | 0 |
| Empty project, audio on | 1.09 | 1.09 | 0.02 | 0.02 | 0 | 0 |

Wakes stay at 14 a second (6 detached without audio) in both columns. The stopped stress rack already paints about
nothing before this change (the earlier item's full-rate tail ends because the panels settle), so the paint count does
not fall further; the 17 points over the no-audio row are the engine's own render of 100 devices on the audio thread,
which the interface cannot reduce (the run with render workers varies with how the workers spin). The differences
between before and after are within the run-to-run spread of the sampler.

## Render workers parked between callbacks (2026-11-20)

`cargo test -p atomik-engine --release --test worker_linger -- --ignored --nocapture`, no audio device: the
four-device patch with six voices per device, four render workers, a callback every 128 or 480 frames at 48 kHz paced
like an audio thread (sleep, then spin to the deadline), 10 s per row. "before" forces the callback flag up for good
(the workers spin for the whole linger between callbacks, as they did before the flag existed); "after" is the
behaviour now. CPU is the whole process as a percentage of one core and includes the benchmark's own pacing loop
(the row with no workers shows it: about 51 % at 128 frames, 16 % at 480). Idle spins are counted when a worker parks.
The table is one run; a second run agreed within the usual spread (the p99 of the after rows at 128 frames was 128 to
143 microseconds in both; the maximum is dominated by the machine's scheduling).

```text
frames workers linger  flag   | callback us: p50   p99   max | barrier spins | idle spins | parks | cpu % of a core
   128       0       0  after  |             84.1 147.5  192.9 |             0 |          0 |     0 |   51.3
   128       4   20000  before |             60.3 130.7  572.6 |          7916 |  533557059 | 26248 |  150.4
   128       4   20000  after  |             67.1 128.0  299.4 |          3712 |   23038449 | 26246 |   60.0
   128       4  100000  before |             44.9  64.3  115.4 |          1338 | 2201031675 | 15002 |  419.6
   128       4  100000  after  |             65.0 141.7  315.7 |          5115 |   23379281 | 26246 |   58.1
   128       4  500000  before |             41.6  61.1  150.7 |          2393 | 2438722445 |  1235 |  452.5
   128       4  500000  after  |             66.2 143.4 1062.5 |         12259 |   23532273 | 26246 |   64.2
   480       0       0  after  |            324.5 529.2  563.0 |             0 |          0 |     0 |   15.9
   480       4   20000  before |            204.7 332.1  692.3 |         10400 |  149882766 |  7008 |   59.4
   480       4   20000  after  |            183.8 275.8  822.6 |         11574 |   20072498 |  7023 |   11.6
   480       4  100000  before |            195.6 263.0  850.2 |          8691 |  709431650 |  7003 |  122.8
   480       4  100000  after  |            183.5 262.6  719.7 |          7287 |   19969866 |  7018 |   14.2
   480       4  500000  before |            168.7 203.5  283.7 |          4500 |  2332296008 |  4006 |  392.8
   480       4  500000  after  |            207.4 327.6  807.6 |          8247 |   23419475 |  7018 |   14.1
```

Reading it: the idle burn of the long linger (420 to 450 % at 128 frames, 390 % at 480) falls to the callback's duty
cycle plus the pacing loop. The cost is the wake-up of a parked worker at the start of each callback: at 128 frames
the median callback is about 66 microseconds instead of 42 to 60 and the p99 about 140 instead of 61 with the long
linger (5 % of the 2.7 ms budget; the old behaviour with the short linger, which parked between callbacks too, had
about 130). The 128-frame p99 therefore misses the "within 10 % of the long linger" target; the decision and the
reasons are in [DECISIONS.md](../docs/DECISIONS.md#ap-render-workers-spin-only-inside-a-callback-and-the-automatic-count-follows-the-physical-cores).
The parks column counts about two per worker per callback: the unpark at the start of a callback and the ones per
dispatched level leave a token that makes one park return at once.

### The running app after the change

`scripts/perf/sample-app.ps1` with the release build of the commit that parks the workers between callbacks, on the
same machine and rack as the idle baseline above (transport stopped, nothing sounding, no `--play`; 15 s settle, 20 s
window). The "before" figures are the rows of that section.

| Scenario | CPU % before | CPU % after |
|---|---|---|
| Stress rack, audio on, automatic workers (7 here: the count follows the physical cores) | 829 (8 workers) | 94 |
| Stress rack, audio on, 2 workers | 216-219 | 34 |
| Stress rack, audio on, no workers | 19.2-20.7 | 20.5 |
| Two cores / four threads (affinity `0xF`): stress rack, 3 workers (`--workers 3`) | 310 | 55 |

The automatic count with an affinity mask is not a row: the app picks its count before the mask applies, so it started
seven workers on four logical processors, and that run did not answer the window probe (the workers run at the
callback's priority). Give the worker count explicitly in such a run, as the script's help says.

## Plugin processes: idle cost and thread priority (2026-11-21)

The release app with `--stress-plugins` (16 plugin processes: 8 synths and 8 effects, CLAP and VST3 test plugins,
sandbox on), 10 s settle, then CPU time and context switches of the 16 `atomik-plugin-host` processes over 20 s
(a performance-counter sample per second over their threads), three runs per row, same machine and build dir as
above, no `--play`, nothing sounding. "no audio" is `--no-audio`: no batch is ever sent, so the processes are
truly idle. "audio on, stopped" is the default audio stream with the transport stopped: the app still sends the
plugins a request per batch, so that row also pays for the batches (the saving there is the spin and the
ticker only). The built-in null host format cannot take the stress project (the test plugins' ids are not its
ids), so the real test plugins stand in for it.

| Scenario | CPU of all 16 processes, % of one core, before | after | Context switches/s before | after |
|---|---|---|---|---|
| Idle, no audio | 21.4, 23.5, 17.6 | 4.2, 5.2, 2.0 | 9 250 to 9 800 | 2 550 to 2 650 |
| Audio on, transport stopped | 45.6, 45.3, 52.2 | 35.2, 35.3, 34.8 | 24 800 to 25 300 | 19 900 to 20 800 |

`cargo test --release -p atomik-plugin-hostd --test overhead -- --ignored --nocapture` (one 64-frame batch of a
VST3 test synth through the child): before, round trip 0.5 us per batch (p99 18.0 us sandboxed against
16.8 us in process), paced child CPU 1.7 %; after (two runs), round trip 0.2 and 0.4 us (p99 17.4 and 17.7 us against
17.7 and 17.2 in process), paced child CPU 2.8 and 0.9 %, parent 1.6 and 0.3 %: the run to run spread of the paced
CPU figure (read through a PowerShell call) is larger than the change. No late buffer in either.

## The wait on a plugin that works in bursts (2026-11-21)

`cargo test --release -p atomik-plugin-hostd --test overhead -- --ignored --nocapture wait_on`: the built-in busy
test plugin (`null.busy`: a passthrough that spins for N microseconds per batch, or on every eighth batch) in a real
child, driven through the plugin manager and the device as a paced 480-frame callback at 48 kHz (400 callbacks,
4 s, one every 10 ms, 3000 batches), device-less: the probe publishes the callback's budget itself (7 ms, the
70 % of the buffer the stream publishes) in the styles that use it. Same machine and release profile as above,
nothing sounding. "late" is the device's late-batch count; "app" is the mean time per callback the audio side
spent inside the plugin calls (the spin: it follows the plugin's own work), and its 99th percentile; "child" is the
plugin process's CPU in % of one core (read through a PowerShell call, so a few points of noise). The four columns
separate the two parts of the change: **before** is the wait of one batch period without the cool-down (the
cool-down was switched off for this column by building it with a huge miss count); **budget** adds the published
deadline; **cool-down** is one batch period with the cool-down; **both** is the shipped behaviour.

| Plugin work | before: late, app us (p99), child | budget: late, app us (p99), child | cool-down: late, app us (p99), child | both: late, app us (p99), child |
|---|---|---|---|---|
| steady 0.3 ms per batch | 0, 2271 (2447), 22 % | 0, 2273 (2447), 25 % | 0, 2283 (2492), 25 % | 0, 2281 (2501), 22 % |
| burst: 3 ms every 8th batch | 1595, 688 (1358), 16 % | 0, 2835 (3045), 29 % | 1595, 687 (1356), 13 % | 0, 2840 (3118), 25 % |
| burst: 6 ms every 8th batch | 1595, 689 (1359), 32 % | 0, 5648 (6046), 59 % | 1595, 686 (1358), 32 % | 0, 5653 (6066), 59 % |
| steady 1.8 ms per batch (cannot keep up) | 3000, 1344 (1361), 19 % | 1800, 7003 (7005), 69 % | 3000, 344 (1355), 18 % | 1803, 6985 (7006), 75 % |
| steady 20 ms per batch (cannot keep up) | 3000, 458 (1355), 65 % | 3000, 2368 (7011), 67 % | 3000, 232 (1354), 66 % | 3000, 1197 (7011), 67 % |

What it shows. A plugin that fits the buffer but not one batch (the two burst rows) lost 1595 of 3000 batches (a late
batch leaves the request out, so the batches that follow it in the callback are late too); with the budget it loses
none, and the audio side spends the plugin's own work time waiting for it (3 ms and 6 ms of one callback's 10 ms),
which is the price of not dropping the batch and the same as the plugin costs in process. A plugin that cannot keep
up (the last two rows) still loses batches, and now holds the audio thread longer: the budget lets the first wait
run to the deadline, 7 ms of every 10 ms (70 % of a core) in the 1.8 ms row, where the plugin does answer most of
the waits. The cool-down is what bounds the 20 ms row (a plugin that answers nothing in time): the wait runs out
four times, then the next 24 batches are only polled, and the app time per callback falls from 2368 to 1197 us
(and from 458 to 232 us with one batch period). It does not cut the cost of a plugin that keeps answering within
its waits (the 1.8 ms row): those waits are real work, not a miss. The child's CPU is the plugin's work and does
not change. Both the burst and the heavy rows are the test plugin's, not a measurement of a real plugin.

### The same probe with the fall-back rule (final code)

The first version of the budget spent 7 ms of every 10 ms callback on a plugin that needs more than real time
(table above, row "steady 1.8 ms"). A second rule now sends such a processor back to one period per wait: three
callbacks in a row in which a wait ran out at the published moment, then 100 callbacks of one period, then the
budget is tried again (the span doubles at each renewed exhaustion, up to 500 callbacks; a callback inside the
budget resets it). Same probe, 800 callbacks (8 s, so the retries are in the average); "late" is summed over the
plugins and "batches" is per plugin. Two rows are new: a plugin at the edge (0.9 ms per batch, 6.75 ms of work in
a 7 ms budget) and two bursty plugins in series (4 ms each, which fit alone but not together).

| Plugin work | wait | late/batches | app us per callback (p99) | child CPU |
|---|---|---|---|---|
| steady 0.3 ms per batch | one batch | 0/6000 | 2278 (2471) | 25 % |
| | callback | 0/6000 | 2276 (2475) | 19 % |
| burst: 3 ms every 8th batch | one batch | 3195/6000 | 685 (1358) | 16 % |
| | callback | 0/6000 | 2835 (3059) | 29 % |
| burst: 6 ms every 8th batch | one batch | 3195/6000 | 685 (1361) | 27 % |
| | callback | 0/6000 | 5647 (6046) | 53 % |
| steady 1.8 ms per batch (cannot keep up) | one batch | 6000/6000 | 344 (1354) | 19 % |
| | callback | 5964/6000 | 454 (7002) | 20 % |
| steady 0.9 ms (at the budget) | one batch | 8/6000 | 6768 (7281) | 72 % |
| | callback | 0/6000 | 6775 (7255) | 66 % |
| 2 plugins in series, burst 4 ms each | one batch | 6394/12000 | 1372 (2703) | 38 % |
| | callback | 2795/12000 | 5531 (7033) | 62 % |
| steady 20 ms per batch (cannot keep up) | one batch | 6000/6000 | 232 (1354) | 66 % |
| | callback | 6000/6000 | 316 (7005) | 67 % |

The burst rows keep 0 late batches at the same cost as before the rule. The rows where the plugin cannot fit are
back to about the one-period cost (454 against 344 us, 316 against 232 us, the difference being the retry
callbacks; the 1.8 ms row was 6985 us without the rule). The 0.3 ms row is unchanged. At the edge the work fits the
budget, so no callback is exhausted and the rule never engages (the one-period column even drops 8 batches). For the two
bursts in series the first plugin takes 4 ms of the 7 and answers; the second has 3 ms left, so its waits run out
at the deadline and it falls back to one period, while the first keeps the budget: late batches fall from 6394 to
2795 (the second plugin's bursts are still lost), and the audio side pays 5.5 ms per callback, most of it the first
plugin's own work, against 1.4 ms. The two plugins do not fit together, which no wait length changes.

## A bounce with ten plugins in ten processes (2026-11-23)

The command-line bounce (`atomik-render --plugin-sandbox on --plugin-mode offline`, release build with debug info) of a
60 s project of ten test CLAP synths, each on its own console channel with a note pattern, every plugin in a process of
its own, 48 kHz (45 000 batches of 64 frames). Same machine as above, nothing else running; wall time of the whole
command, three runs each, and the time the plugin manager's poll after the batches took (a counter around the poll,
not kept in the code). **before** asks the processes for their notifications one after the other, **after** sends
every request first and then collects the answers.

| | wall time, runs | median | the polls |
|---|---|---|---|
| before | 58.1 s, 59.2 s, 59.5 s | 59.2 s | 50.7 to 52.3 s (88 %) |
| after | 42.9 s, 41.4 s, 42.5 s | 42.5 s | 34.2 to 35.3 s (83 %) |

The wall time falls by about 28 % and the three runs of each column lie within 1.5 s of each other. The WAV is the same
bytes before and after. The polls still take most of the bounce: each batch still waits for one round trip to the
slowest process, and the ten processes share one machine.

## Idle instruments and shortcuts in the FM synth and the organ (2026-11-23)

`cargo bench -p atomik-devices --bench process -- <device>` on the machine above, release profile with debug info:
median of 256 batches, two runs each. The `idle` rows are the same rig without notes (every input cabled with
noise); the other rows hold a three-note chord. **before** is the tree without the change of the row, **after** the
tree with it. A row that did not move says so. The machine was quiet for every row but the last, which was taken
later with other software running: compare the two sides of a row, not one row with another.

| row | before, runs | after, runs |
|---|---|---|
| poly-synth, idle | 1.349 ms, 1.384 ms | 1.232 ms, 1.260 ms |
| fm-synth, idle | 1.401 ms, 1.380 ms | 1.273 ms, 1.243 ms |
| wavetable-synth, idle | 1.420 ms, 1.385 ms | 1.245 ms, 1.246 ms |
| sampler, idle | 369.9 us, 368.1 us | 285.7 us, 288.0 us |
| granular, idle | 375.3 us, 400.2 us | 275.5 us, 277.7 us |
| organ, idle | 997.5 us, 1018 us | 234.0 us, 236.7 us |
| organ, chord | 2.569 ms, 2.555 ms | 2.596 ms, 2.535 ms (did not move) |
| fm-synth, chord (default patch), silent operators skipped | 11.660 ms, 11.685 ms | 7.217 ms, 7.209 ms |
| fm-synth, chord, key scale of zero without `exp2` (on top of the row above) | 8.215 ms, 8.367 ms | 8.269 ms, 8.511 ms (did not move) |

The LFO that an idle instrument no longer evaluates is 64 sine calls a batch, about 0.3 us on this machine: 85 to
100 us of the 256 batches, a quarter of an idle sampler or granular synth and a tenth of the other three. What
remains of an idle poly, FM and wavetable synth is not touched here (their output stage runs for every sample).
The organ's scanner was three quarters of an idle organ. In the default FM patch two of the four operators have
level 0, and the sine series of an operator is the largest part of a voice sample: skipping the silent ones takes
38 % off the chord. The key scale shortcut saves four `exp2` calls per voice every 16 samples, against four sine
series per sample: it is below what the bench resolves.

## The oversampler filters a block at a time (2026-11-23)

The halfband stages of `atomik_dsp::oversample::Oversampler` used to run sample by sample, the stages of a 4x or 8x
chain interleaved, each output reading its taps through an index table and a masked ring buffer. They now run a
stage at a time over a linear history, the taps as the outer loop and the samples as the inner one, so a tap is a
multiply-add over contiguous slices that the compiler vectorises. Every output receives the same additions in the
same order as before: no output bit changes (the tests compare against the old form bit for bit). Release profile with
debug info, same machine as above, two runs of each side; **before** is `main`, **after** the branch. The `dsp` rows
are `cargo bench -p atomik-dsp --bench blocks` (median of 21 runs of 4096 samples fed in 64-sample batches, a hard
clip between up and down; `up` is the up half alone at 4x, which is how the true-peak detectors use it), the `device`
rows `cargo bench -p atomik-devices --bench process` (median of 256 batches).

| row | before, runs | after, runs |
|---|---|---|
| dsp 2x round trip | 161.6 us, 175.9 us | 53.0 us, 55.6 us |
| dsp 4x round trip | 291.5 us, 296.5 us | 88.1 us, 89.1 us |
| dsp 8x round trip | 580.4 us, 501.0 us | 133.8 us, 169.7 us |
| dsp 4x up | 161.5 us, 166.4 us | 41.0 us, 42.5 us |
| device distortion | 3.032 ms, 2.919 ms | 1.606 ms, 1.425 ms |
| device saturation | 2.535 ms, 2.967 ms | 1.945 ms, 1.979 ms |
| device clipper | 1.878 ms, 1.834 ms | 1.302 ms, 1.332 ms |
| device mastering | 6.098 ms, 7.526 ms | 4.598 ms, 4.938 ms |
| device loudness-meter | 1.770 ms, 1.721 ms | 992 us, 874 us |
| device phase-meter | 1.690 ms, 1.679 ms | 1.048 ms, 1.008 ms |
| device spectrum | 1.637 ms, 1.645 ms | 962 us, 895 us |
| device guitar-amp | 11.972 ms, 11.286 ms | 11.498 ms, 11.508 ms (did not move) |
| device character | 6.074 ms, 6.475 ms | 6.251 ms, 6.218 ms (did not move) |
| device fm-synth, chord | 7.265 ms, 7.821 ms | 8.686 ms, 8.629 ms (did not move, see below) |

The primitive is 3.0 to 3.9 times faster (the 8x second run of the after column is an outlier: the other four runs
of that row lie between 133 and 142 us). The devices that run a plain curve between the stages gain 23 to 51 %
(saturation the least, distortion the most), the three meters that hold a true-peak detector 38 to 49 % and the
mastering device 25 to 34 % (its limiter is one of several stages). The guitar amp, where
the clip stages at four times the rate are most of the cost, and the character device did not move. The FM synth
bench patch has its oversampling off (the default), so the row cannot move; a third pair of runs taken straight
after the first two with the old code again gave 8.667 ms and 1.517 ms idle for it (the machine was busier than
for the first runs): compare the two sides of a row, not one row with another. The block form keeps about 10.6 KB of
buffers per oversampler built for 64-sample batches (2.0 KB before): passes of at most 64 base-rate samples keep the
working space the same for any `max_block`, and 32-sample passes measured about 15 % slower.

## No library call to round a number on the delay line reads (2026-11-23)

A release build for baseline x86-64 has no SSE4.1, so `f32::floor` and `f64::floor` are calls into the C runtime
there, and the per-sample paths made one or several each. The whole-part split of a delay line (`DelayLine::read`,
`ModDelay::read`) now answers a delay of 1 to 2^23 samples with a shift of its bits and sends every other value to the
library expression, so no output bit changes (the tests compare against the old bodies bit for bit, and the split
against all 2^32 `f32` bit patterns). On `aarch64`, or with `sse4.1`, it is the library expression itself. Release
profile with debug info, same machine as above; the two bench executables (`before` is the branch below this item,
`after` this item) were built once each and run in turn, three runs of each side, so the machine's drift hits both
sides alike. Rows are `cargo bench -p atomik-devices --bench process` (median of 21 runs of 256 batches) and
`cargo bench -p atomik-dsp --bench blocks`.

Delay lines (`DelayLine::read`, `ModDelay::read`):

| row | before, runs | after, runs |
|---|---|---|
| device reverb | 4.164 ms, 4.123 ms, 4.144 ms | 4.219 ms, 3.681 ms, 3.693 ms (-11 %) |
| device space-reverb | 4.199 ms, 4.187 ms, 4.220 ms | 3.774 ms, 3.767 ms, 4.159 ms (-10 %) |
| device granular | 4.890 ms, 4.913 ms, 4.917 ms | 4.475 ms, 4.421 ms, 4.440 ms (-10 %) |
| device rotary-speaker | 2.672 ms, 2.682 ms, 2.674 ms | 2.522 ms, 2.525 ms, 2.534 ms (-6 %) |
| device modulation | 1.856 ms, 1.854 ms, 1.848 ms | 1.810 ms, 1.800 ms, 1.800 ms (-3 %) |
| device stereo-delay | 480.2 us, 479.7 us, 480.7 us | 472.0 us, 485.9 us, 473.3 us (within noise) |
| device echo | 1.047 ms, 1.048 ms, 1.044 ms | 1.067 ms, 1.061 ms, 1.065 ms (within noise) |
| device feedback | 170.2 us, 171.8 us, 169.7 us | 176.0 us, 175.8 us, 175.6 us (+3 %, see below) |
| device character | 5.293 ms, 5.315 ms, 5.319 ms | 5.257 ms, 5.111 ms, 5.244 ms (within noise) |
| dsp chorus-stereo | 305.0 us, 302.7 us, 304.4 us | 294.6 us, 297.7 us, 301.5 us (within noise) |

The reverbs read about twenty delay lines per sample, each at its own delay, and gain a tenth; the granular device
and the rotary speaker gain through the lines they read. A first form of the split (a rounding add and a compare)
cost 7 to 9 % on the stereo delay and the rotary speaker: a branch on whether the sum had rounded up is taken at
random for a delay with a random fraction. The shipped form has no float comparison and no such branch. The feedback
device reads one fixed delay per batch, which the old code computed once ahead of the loop; the new form is a few
integer operations a read and costs it 3 % (about 6 us of 170 us). `Echo`, the stereo delay and the character
device read a delay line too and did not move beyond the noise.

The other per-sample sites that call `floor` were tried with the same kind of replacement for the phase wrap `p -
p.floor()` (a few comparisons for the three ranges a phase lives in, the library expression for the rest, no output
bit changed) and **left as they are**, because no row moved beyond its run-to-run noise: the sine of the stage blocks
(`sin_cycles`) and the phase wraps and operator waves of the FM synth, the wavetable reads and the oscillator phase
wraps of the wavetable synth, and the organ's scanner, the rotary speaker's angle, the semi modular's FM oscillator,
the drum machine's sine, the ring modulator and the modulation source's shapes. Before is again the delay line change,
five runs each for the FM synth and wavetable synth rows, three for the others:

| row | before, runs | after, runs |
|---|---|---|
| fm-synth, chord | 7.146 ms, 7.082 ms, 7.130 ms, 7.095 ms, 7.078 ms | 7.024 ms, 6.994 ms, 7.032 ms, 7.041 ms, 7.548 ms (did not move) |
| fm-synth, idle | 1.241 ms, 1.252 ms, 1.271 ms, 1.280 ms, 1.302 ms | 1.250 ms, 1.242 ms, 1.278 ms, 1.258 ms, 1.276 ms (did not move) |
| dsp chorus-stereo | 301.9 us, 297.4 us, 297.6 us | 288.2 us, 289.7 us, 291.6 us (-3 %, under the noise bar) |
| wavetable-synth, chord | 4.258 ms, 4.411 ms, 4.255 ms, 4.259 ms, 4.261 ms | 4.327 ms, 4.415 ms, 4.361 ms, 4.427 ms, 4.408 ms (+3 %, kept off) |
| wavetable-synth, idle | 1.247 ms, 1.280 ms, 1.248 ms, 1.270 ms, 1.264 ms | 1.299 ms, 1.264 ms, 1.246 ms, 1.235 ms, 1.238 ms (did not move) |
| organ, chord | 2.696 ms, 2.659 ms, 2.993 ms | 3.231 ms, 2.632 ms, 2.642 ms (did not move) |
| rotary-speaker | 2.543 ms, 2.527 ms, 2.520 ms | 2.533 ms, 2.525 ms, 2.553 ms (did not move) |
| semi-modular | 3.224 ms, 2.514 ms, 2.484 ms | 2.613 ms, 2.481 ms, 2.491 ms (did not move) |
| drum-machine | 647.0 us, 640.2 us, 640.6 us | 643.1 us, 636.3 us, 647.4 us (did not move) |
| mod-source | 1.136 ms, 1.119 ms, 1.127 ms | 1.186 ms, 1.184 ms, 1.667 ms (noise, the third run an outlier) |
| modulation | 1.810 ms, 1.801 ms, 1.808 ms | 1.811 ms, 1.829 ms, 1.983 ms (did not move) |
The library `floor` of this machine is cheap (a call into the C runtime that runs the rounding instruction), so what a
replacement saves is the call overhead and the registers a call spills: it shows only where a read is a short chain
repeated many times, as in a delay line, and not behind a sine series or a Hermite read. A form of the phase wrap that
used a truncating conversion or a rounding add (and no comparison on the range) measured slower than the call in a
loop of dependent wraps (about 11 and 7 ns against 5 ns). The disassembly of the bench executables carries no symbols
(the debug information is in a separate file the tool does not read), so the calls that remain were not counted: of
the one function compiled on its own for a look, the shift form has no call on the path of a delay in range.

## Values that have settled are not computed again (2026-11-24)

Four small changes of one rule: a value that has arrived is not recomputed, and no output bit changes. Rows are the
`process` bench (`cargo bench -p atomik-devices --bench process -- 'device/<name>/256'`), the median of 21 samples of
256 batches, two runs each, before and after taken close together on the same build folder.

**The one-pole smoother on its target.** `OnePoleSmoother::next_value` returns at once when the value holds the target
bit for bit and the target is finite (the step leaves `+0` added and the snap writes the target back, so state and
result are unchanged; a `-0` target is kept as it is, an infinite or NaN value takes the full path). With a time
constant of 10 ms the smoother is within the `1e-7` snap about 8,000 samples (170 ms) after a step of one, so a held
control sits on its target nearly all the time.

| row | before, runs | after, runs |
|---|---|---|
| guitar-amp | 10.971 ms, 10.935 ms | 9.321 ms, 9.298 ms (-15 %) |
| eq (the other user, steps per block) | 880.0 us, 887.3 us | 872.8 us, 872.9 us (did not move, slowed nothing) |

**The wavetable synth resolves its tables once per control step.** `table_data` was called for the current and the old
table of both oscillators for every sample of a voice. `table` and `old_table` are written only in `control` (every
control interval, when the selected table changes) and in `start`, so the slices are looked up again after those two
and held in between. They are the same slices, so the output is the same.

| row | before, runs | after, runs |
|---|---|---|
| wavetable-synth, chord | 4.637 ms, 4.540 ms | 4.274 ms, 4.371 ms (-6 %) |
| wavetable-synth, idle | 1.334 ms, 1.328 ms | 1.395 ms, 1.331 ms (did not move) |

**The character device parses its voicing once.** `Character::new` parsed the embedded voicing tables and built the
sixteen preset chains to take their specs and to check them, then built and prepared sixteen chains for the instance,
so about four times as many chains were built as the instance keeps. The parsed and checked tables are now held in a
`OnceLock` and shared through an `Arc` (nothing in them depends on an instance or a sample rate); the chains an
instance keeps are built as before, in `new` and again in `prepare`. The audio path does not read the cache. Measured
by a loop of 50 creations of `Character::new` and of 50 `prepare` calls in a unit test (release build, not kept in
the tree), with the allocation counter of the test kit on one creation:

| row | before, runs | after, runs |
|---|---|---|
| 50 x `Character::new` | 75.5 ms, 73.3 ms | 46.2 ms, 48.5 ms (-38 %) |
| 50 x `prepare` | 42.9 ms, 48.1 ms | 43.0 ms, 45.6 ms (unchanged) |
| allocations of one `new` | 2,961 | 601 (the first creation of a process still parses once) |
| peak heap of one `new` | 2,512,820 bytes | 2,476,036 bytes |

**The reverb reads a ramp that has arrived once per batch (tried, left out).** The reverb makes its thirty ramps once
per batch and evaluates each at every sample. A ramp whose increment is zero has the same value at every sample, so
the loop was changed to read those once and to evaluate only the ramps still moving (a unit test compared the two
for both reverbs, with and without a cable on the mix input, bit for bit). The saving did not move beyond the run to
run noise, because the feedback network and the delay reads cost far more than the evaluation of the ramps, so
nothing was committed:

| row | before, runs | after, runs |
|---|---|---|
| reverb | 3.781 ms, 3.949 ms | 3.661 ms, 3.775 ms (-3 %, under the noise bar) |
| space-reverb | 4.051 ms, 3.839 ms | 3.795 ms, 3.748 ms (-4 %, under the noise bar) |

## A device that has gone silent is not called (2026-11-24)

The rows are about the engine, not a device: the `idle` rows of the device bench call `process` directly and cannot see
what the engine leaves out. `cargo bench -p atomik-render --bench render_batch -- idle` renders 256 batches (a third
of a second at 48 kHz) of an engine holding twenty stereo delays with nothing cabled, so every one reads silence,
with the skip switched off and on (`Engine::set_idle_skip`); only the render loop is timed, the engine was built and
had rendered once before. The machine is the one above, release profile with debug information, median of 21 samples,
three runs each. The stereo delay is quiescent from its first batch (its lines are empty), so the rows are what an
idle rack of such devices costs. **skip off** is the engine as it was.

| row | skip off, runs | skip on, runs |
|---|---|---|
| 20 idle stereo delays, 256 batches | 7.004 ms, 6.808 ms, 6.758 ms | 859.9 us, 802.5 us, 801.0 us |

That is 27 us a batch for the twenty devices before and 3.2 us after (about 0.16 us a node: the gather that fills the
node's buffers, the test of its inputs and its outputs' copy to the slots, which a skipped node still takes). The
delay of 4 s needs 4 s of silence after its last repeat before it says it is quiescent: an idle rack saves nothing
while a tail rings, and a stereo delay left with a long feedback takes minutes to run dry (the repeats fall below
the flush threshold of the line, 1e-20, only after about 900 passes at a feedback of 0.95).

Does counting the zeros cost a busy line? `cargo bench -p atomik-devices --bench process -- reverb`, three runs each,
before is the tree without the counter, after the tree with it (the line counts only if asked, see the commit):

| row | before, runs | after, runs |
|---|---|---|
| reverb | 3.634 ms, 3.586 ms, 3.619 ms | 3.616 ms, 3.614 ms, 3.610 ms (did not move) |
| space reverb | 3.650 ms, 3.633 ms, 3.664 ms | 3.792 ms, 3.629 ms, 3.708 ms (noise, one slow run) |

A first form of the counter, kept for every line and branch free, cost the reverb 1.5 % and the space reverb 3.5 %
(3.667 and 3.778 ms means), which is why a line now counts only after `track_zeros`: a line nobody asked about pays one
predictable branch a write.

## More effects that go quiet (2026-11-24)

The same engine rows as above (`cargo bench -p atomik-render --bench render_batch -- idle`), one pair for each effect
that now says it is quiescent: twenty of them, nothing cabled, 256 batches, skip off and on, median of 21 samples,
three runs each. What the skip must not cost a busy effect is measured with the device bench
(`cargo bench -p atomik-devices --bench process -- <name>`, the row of the effect with noise on every input), before
(the tree without the change) and after, three runs each; the limit is a rise of 2 % of the busy row.

| effect | idle row, skip off | idle row, skip on | busy row before | busy row after |
|---|---|---|---|---|
| echo | 21.73 ms, 21.08 ms, 21.38 ms | 1.251 ms, 1.241 ms, 1.238 ms | 1.014 ms, 1.007 ms, 1.016 ms | 1.027 ms, 1.023 ms, 1.019 ms (+1.1 %) |
| reverb | 80.29 ms, 80.39 ms, 80.58 ms | 5.270 ms, 5.363 ms, 5.317 ms | 3.547 ms, 3.549 ms, 3.542 ms | 3.578 ms, 3.541 ms, 3.554 ms (+0.3 %) |
| space reverb | 82.23 ms, 83.52 ms, 83.11 ms | 5.269 ms, 5.243 ms, 5.260 ms | 3.588 ms, 3.596 ms, 3.613 ms | 3.652, 3.646, 3.709, 3.599, 3.614, 3.592, 3.629 ms (+1.0 %) |
| modulation | 34.39 ms, 34.98 ms, 35.79 ms | 1.289 ms, 1.309 ms, 1.294 ms | 1.759 ms, 1.741 ms, 1.742 ms | 1.727, 1.740, 1.714, 1.702 ms (-1.5 %, noise) |

Echo: the first form, which asked both of its delay lines to count the zeros they store, cost the busy row 2.5 %
(1.042 ms, 1.038 ms, 1.031 ms against the 1.012 ms above), so the echo ORs the bits of the two values it writes into one
accumulator per batch instead and counts whole batches of zero writes against the ring length (a batch that held one
value at its start does not count, so it can only claim less than it knows). That is the form that was kept. An idle
echo needs its whole ring, 2^18 samples (5.5 s at 48 kHz), of zero writes before it is quiescent.

Reverb: the delays do not each count their zeros (that is what cost 2.2 % when it was always on). A first form of
this device ORed the bits of the values it wrote to its twelve delays inside the sample loop; it measured +1.1 % on the
reverb, but the space reverb, which runs the same code in a copy of its own, moved by +4 % to +6 % (3.60 ms before, 3.74
to 3.83 ms after, with a change that does not touch the loop at all: a layout effect of the compiler, not of the
work), so that form was replaced by one that does nothing in the loop: at the end of a batch, if no input sample is
non-zero, the stretch the batch wrote to each delay is looked at (a vectorised OR of at most 64 samples a delay) and
whole zero batches are counted against the longest ring; a batch with a sound on an input costs one early-out test.
The controls (about 50 one-pole block smoothers) are compared with their targets once a batch; the filters and
allpasses are scanned only when the delays are already empty. An idle reverb needs its pre-delay ring (16,384 samples,
a third of a second) and then the tail to fall below the 1e-20 flush; in the idle row it is quiescent from its first
batch. The skip-on row is mostly the eight LFOs stepped sample by sample (512 additions a batch), which is what keeps
their phases bit-exact. Busy rows of this form, three runs each against the base of 3.546 ms (reverb) and 3.599 ms
(space reverb, below): 3.578, 3.541 and 3.554 ms; 3.622, 3.603 and 3.657 ms (+0.8 %).

Space reverb: the engine of the reverb, so the same form; it adds its high cut (compared with zero) and its gate, whose
resting state is found by stepping a copy of it one more silent sample and comparing every field bit for bit (the
gate closes to exact zero through the flush, or stalls just below 1 when it is open, so neither "is zero" nor "is one"
is the test). The first runs after the change read 3.65 to 3.71 ms and the later ones 3.59 to 3.63 ms with the same
binary: the spread of this machine between runs is about 2 %, which is why the seven runs are all listed.

Modulation: the lines of the chorus and the flanger are looked at once a batch as the reverb's are (nothing in the
sample loop); the phaser compares its allpass states and its feedback state with zero. The rows are the chorus at its
default (the device bench default mode and an unsynced LFO); the allpass states and the lines take 43 batches or a few
hundred samples to empty, so an idle modulation effect is quiescent from its first batch. An LFO locked to a playing
transport is never skipped, so the row of a project that plays a synced chorus is the busy row. Four more runs of the
busy row after the change read 1.727 to 1.702 ms against 1.759 to 1.741 ms before: no cost that the machine can see.

## Instruments that go quiet (2026-11-24)

The engine rows of the effects above, now for the instruments that say they are quiescent
(`cargo bench -p atomik-render --bench render_batch -- idle/<name>`): twenty of them, nothing cabled and no note, 256
batches, skip off and on, median of 21 samples, one run each. What the answer must not cost a playing instrument is
the device bench's chord row (`cargo bench -p atomik-devices --bench process -- device/<name>/256batches`: a held
three-note chord), before (the tree without the change) and after; the limit is a rise of 2 % of the busy row.

| instrument | idle row, skip off | idle row, skip on | busy row before | busy row after |
|---|---|---|---|---|
| organ | 5.530 ms | 1.082 ms | 2.526 ms | 2.493, 2.493 ms (-1.3 %, noise) |
| sampler | 2.452 ms | 0.951 ms | 0.282 ms | 0.284, 0.286 ms (+0.8 %, +1.6 %) |
| granular | 2.274 ms | 1.259 ms | 4.656 ms | 4.713, 4.667 ms (+1.2 %, +0.2 %) |
| fm synth | 2.418 ms | 0.931 ms | 7.076, 7.181, 7.320 ms | 7.259, 7.027, 7.077, 7.068 ms (-1.2 % on the means, noise) |
| wavetable synth | 2.315 ms | 1.115 ms | 3.980 ms | 4.042, 3.955 ms (+1.6 %, -0.6 %) |
| poly synth | 2.306 ms | 1.002 ms | 2.338 ms | 2.330, 2.327 ms (-0.3 %, noise) |

Organ: the answer is computed once at the end of a batch and only when no key sounds (a chord ends the check at the
first voice). What it reads then is a handful of flags and settled-smoother tests, the scan-line zero counter the
scanner shortcut already keeps, and (last) one test per tone wheel that the per-batch normalisation would change
nothing, a fixed point the phasors reach a batch or two after the last key. The percussion envelope is not a
condition: it multiplies a sum of nothing, so `idle_advance` lets it fall with the same multiplications. The default
patch is quiescent 28 batches after the last key is released (the key envelope's 4 ms release down to 1e-4) and 2
batches from the start. The skip-on row is the scanner phase stepped sample by sample (64 additions and wraps a
batch), which is what keeps it bit-exact.

Sampler: the answer is one pass over the voices at the end of a batch (the pass the batch starts with already asks the
same question), the two gains' settled flags and the master gain's sign. The device bench rig gives the sampler no map,
so its chord row measures a sampler whose notes find no zone, the cheapest busy batch there is, which makes a rise of
0.8 to 1.6 % an upper bound on what a playing sampler pays; a sampler that plays a map costs far more per batch and the
pass is the same. The skip-on row is the LFO stepped sample by sample (64 additions and compares a batch). A default
patch is quiescent 113 batches after the last key is released (the 150 ms release) and one batch from the start.

Granular: the same question as the sampler's, one pass over the voices at the end of a batch (the counts of grains
and voices the panel reads are made by a pass of their own that was there already). Its skip-on row is dearer than the
sampler's because the skipped batch also advances the frame counter and clears the panel's newest-voice view, and the
LFO is stepped sample by sample as before. The chord row is the built-in source, so it is a real busy row. A default
patch is quiescent 300 batches after the last key is released (the 400 ms release) and one batch from the start.

FM synth: one pass over the voices, then the two stages behind them, each only when everything before it passed: the
oversampler's decimator histories (a few hundred floats, only at 2x and 4x) and, with the chorus on, the two chorus
rings (about 1,450 floats each, a vectorised OR). The chorus is off in the bench rig, so the chord row does not run the
ring check (when the chorus is on and a voice sounds the pass stops at the first active voice all the same). The
decimators are FIR, so one silent block empties them; the chorus needs its 30 ms ring (22 batches more). The default
patch is quiescent 220 batches after the last key is released (the carriers' release), one batch from the start. The
three baseline runs of the busy row show this machine's spread (2 %); the four runs after the change lie inside it.

Wavetable synth: one pass over the voices at the end of a batch, the cable test on the LFO output and the three
smoother tests; nothing in the sample loop. The published position of the newest voice is computed at the end of a
batch already and reads -1 when no voice runs, which is the value a skipped batch leaves. The skip-on row is the LFO
stepped sample by sample. The default patch is quiescent 217 batches after the last key is released and one batch
from the start.

Poly synth (and the mono bass, which is the same engine under another descriptor): the same pass over the voices, the
smoother tests, and for each effect that is on the check of its lines: the chorus's two rings (a vectorised OR of about
1,450 floats each) and the echo's two zero counters (`track_zeros` on the lines, set when the lines are made at
`prepare`, so a write counts the zeros it stores: a compare and an add a sample, paid only while the echo is on; the
default patch has it off). The echo is the long wait: its ring is 65,536 samples and the repeats (feedback 0.35,
ping-pong) take about 43 passes of 0.3 s to flush below 1e-20, so a patch with the echo on rests about 13 s after the
last key and one with feedback 0 about 1.4 s after it. LFO 2 is stepped only while it has a destination, exactly as
the batch does. The default patch is quiescent 217 batches after the last key is released and one batch from the
start; the mono bass with its defaults 96.

## Memory held by 64 channel strips (2026-11-24)

Bytes asked from the allocator, with the testkit's counting allocator (`peak_heap_of`), and the working set and
private bytes (commit charge) of the process, read with the process list while it held the devices. 64 channel
strips prepared at 48 kHz, plus one convolution reverb prepared with no impulse in the process numbers.

| | before | after |
| --- | --- | --- |
| one scope tap (`ScopeTap::new`) | 256 KiB, written at once | unchanged: the analyser devices and an audio track keep it |
| one scope tap of a strip, master, EQ or plugin device | 256 KiB, written at once | a header of a few dozen bytes until a panel looks |
| 64 prepared channel strips, heap | 274.5 MiB | 258.5 MiB |
| process with them and a convolver, working set | 39.5 MiB | 22.6 MiB |
| process with them and a convolver, private bytes | 287.4 MiB | 271.2 MiB |

The 16 MiB the taps held was resident (every cell of both rings is written when the tap is made); what is left of the
255 MiB or so of the 64 strips is the two latency lines of 2 MiB each, which are zero filled by the allocator and not
touched, so they count as private bytes and not as working set.

## A convolution reverb with an impulse, and what it holds (2026-11-24)

The device bench had no row for a convolver that plays an impulse (its rig loads none, so it measures the direct head
and silence). `device/convolver-ir/256batches` loads a 3 s synthetic impulse (deterministic noise with an
exponential decay, mono, which serves both channels), lets the build finish (about 70 batches) and times 256 batches
like the other rows; `device/convolver-ir/batch-max` is the dearest single batch of 4096, five times over.

```text
device/convolver/256batches: median 2.590 ms (no impulse: 10.1 us a batch)
device/convolver-ir/256batches: median 8.941 ms  p99 9.118 ms  min 8.880 ms  max 9.118 ms  (n=21) (34.9 us a batch)
device/convolver-ir/batch-max: median 228.1 us  highest 603.2 us  (5 x 4096 batches)
```

The dearest batch is the one in four that sums the tail of every partition (562 for 3 s); it costs several times
the mean and is the burst an xrun on two cores would come from. A batch is 1.33 ms of audio at 48 kHz.

What a prepared convolver asks of the allocator (counting allocator, 48 kHz): 11.18 MiB with no impulse, 11.28 MiB
with a 0.5 s one. It is sized for the longest impulse (5 s) whatever is loaded: two convolvers of 937 partitions with
a spectrum of 257 bins each for the impulse, a second slot for a crossfaded swap and the input history. Only the input
history (about 3.7 MiB for the two convolvers) is written when the device is prepared; the rest is zero filled by the
allocator and touched as an impulse is built, so it is private bytes first and working set as impulses load.

Latency lines of a channel strip: 4 MiB each (two lines of 2 MiB, 480 000 samples of reserve), 256 MiB for 64 strips,
none of it touched until `latency.adjust` is raised. Private bytes of a process holding 64 strips: 271 MiB; working
set 22.6 MiB.

## A budget for the audio a session keeps in memory (2026-11-24)

Forty mono stems of five minutes at 48 kHz (54.9 MiB decoded each, 2.15 GiB together; a stereo stem is 110 MiB and
streams on its own already), decided by `stream_source::plan` (a pure function: no audio was built for these figures,
and the allocation peak was not measured at this size).

```text
memory   budget   resident stems   resident audio   streamed (rings of ~770 KiB)
none     none     40               2.15 GiB         0
4 GiB    1 GiB    18               0.97 GiB         22 (16.5 MiB)
8 GiB    2 GiB    37               1.98 GiB         3 (2.3 MiB)
16 GiB   4 GiB    40               2.15 GiB         0
```

At 4 GiB the resident audio is under 1 GiB (the rings add 16.5 MiB); the working set of the
process adds its own baseline to it. A project opens by reading the header of every file first (40 small reads) and
then decoding only the files that stay: the tests check that a streamed file is never decoded.

## Storing a long take (2026-11-25)

A ten minute stereo take at 48 kHz (28.8 million frames, a 220 MiB in-progress file) stored by the writer thread,
a one-off run of the app's `store_laps` (release profile, warm file cache, heap held at once on the writer's
thread, counting allocator; the test that printed it is gone). The ceiling is kept by
`crates/audio-file/tests/bounded.rs` (6 channels x 1 000 000 frames under 2 MB):

```text
path                          peak heap held   time
whole lap read back (before)  226 MiB          0.76 s
block by block (now)          6.7 MiB          0.93 s
```

The block path reads the file once per channel for the hash and the peak scan and once more to write the asset, so it
is about 23 % slower on a warm cache; a cold disk pays for the extra reads. Its memory is one block of every channel
plus the peaks, which is about 1 % of the audio held a few times over while it is written and loaded back (the
pyramid, its bytes, the loaded copy); a 60 minute stereo take needs about 40 MiB instead of 1.38 GB.

## Memory a plugin catalogue and a hosted plugin hold (2026-11-25)

Heap bytes held on one thread, counting allocator, synthetic plugins (names and module paths as a real plugin has them),
in `crates/plugin-host/tests/memory_figures.rs` and `crates/plugin-vst3/tests/memory_figures.rs` (debug test
profile; byte counts do not depend on it). 132 plugin files, one plugin each; "scan entries" is the scan cache the app
holds anyway, "catalog entries" the `PluginCatalog` built from it, "registered" what `register_pinned_into` adds (the
leaked descriptors and the table of interned layouts).

```text
params   scan entries   catalog entries   registered, before   registered, after   B per param (all three, after)
50       1.1 MiB        0.9 MiB           1.9 MiB              1.0 MiB             475
500      10.3 MiB       8.3 MiB           15.6 MiB             7.3 MiB             411
2000     40.8 MiB       33.2 MiB          61.5 MiB             28.4 MiB            407
0        0.06 MiB       0.09 MiB          0.36 MiB             0.31 MiB            -
```

One layout is 6.4 KiB at 50 params, 63.9 KiB at 500 and 256.9 KiB at 2000 (about 130 bytes a param). The browser's
rows (`PluginEntryInfo`, built by the app, not in the table) held one more deep copy of every layout before, 8.3 MiB at
500 params and 33.2 MiB at 2000; they now share the entry's `Arc`. At 500 params the catalogue held about 34 MiB in the
table plus 8 MiB in the rows (42 MiB) before, and about 26 MiB after.

One more hosted instance, in process, over the null backend (device, the instance's layout, the manager's listed
layout, the layout the handle returns), and the device alone (the parameter arrays, the event lists, the dry ring):

```text
params   one instance   device alone   of which event lists (reserved, never written)   one layout
50       227 KiB        194 KiB        127 KiB                                           6.4 KiB
500      432 KiB        226 KiB        138 KiB                                           63.9 KiB
2000     1120 KiB       335 KiB        173 KiB                                           256.9 KiB
```

The format layers, test plugins in process, 4 and 2 000 parameters (device included):

```text
format   4 params    2000 params   per param   fixed
CLAP     11 096 KiB  11 669 KiB    294 B       11 095 KiB
VST3     596 KiB     1 717 KiB     575 B       594 KiB
```

The CLAP fixed part is almost all one event buffer of the host library. It is created with a capacity of 6 656 events and
reserves 6 656 x about 104 bytes (the largest CLAP event) x 16, about 11 MiB, because the library multiplies the event
count by an event's size in bytes and then counts that in 16 byte headers. Nothing writes it beyond the events of a
block, so by the code it is committed address space and not working set (a working set was not measured). The VST3
fixed part is written when made: the input event list is 4 608 events of the VST3 event record, plus 150 and 256
parameter queues of about 240 bytes each (a queue holds 16 points).

None of the instance figures was changed. Sharing the manager's and the handle's copy of the layout saves two layouts
(128 KiB at 500 params), under the 200 KiB bar; the parameter queues are all needed when a batch touches every
parameter and the first touch arrives on the audio thread; the event lists could only be made smaller by proving a
bound under the device's `4 * MAX_EVENTS_PER_BATCH + 256` events.

## Performance setting: Full against Low power (2026-11-24)

The running app on the reference machine above (graphics card: a discrete desktop GPU), release build of the
branch that adds the Performance setting, the transport stopped and nothing sounding (no `--play`), the stress rack
(100 devices) with the audio stream open, one main window. Profile LP4: the process pinned to four logical
processors (two cores, four threads) right after launch, with `--workers 1` given explicitly so that the worker count
is the same in both rows (the setting only cuts an automatic count). A clean settings folder per run, with a
`workflow.json` that sets `performance` to `full` or `low_power`. The sampler is `scripts/perf/sample-app.ps1` (15 s
settle, 20 s window; its UI probe wakes the app about ten times a second in both rows). The "pointer sweep" rows
add a script that moves the pointer over the window every 10 ms during the window. GPU % is the summed GPU engine
utilisation of the app process; one run per row.

| Scenario | CPU % (one core) | GPU % | Wakes/s | Main paints/s | UI probe p95 ms |
|---|---|---|---|---|---|
| Stopped stress rack, Full | 26.0 | 3.6 | 14.1 | 0.5 | 0.2 |
| Stopped stress rack, Low power | 22.7 | 5.7 | 14.0 | 1.4 | 0.1 |
| Stopped stress rack, pointer sweep, Full | 36.9 | 14.3 | 38.8 | 7.0 | 0.9 |
| Stopped stress rack, pointer sweep, Low power | 35.0 | 16.3 | 40.2 | 5.5 | 3.4 |

Read these rows as "no difference": a stopped rack paints about once a second, and the sweep reached only 5 to 7
paints a second (a hover marks few repaints), far below either frame rate, so neither the 30 Hz slot nor the smaller
render target has anything to save; the gaps between the rows are run-to-run noise (the Low power idle row painted
more often, by chance, and shows more GPU). The frame time is not in the sampler's output. The effects show only while
something animates for real (a playing pattern, a drag, a 4K window); that needs the owner's hand check, since this
run may not play audio.

## Rack paint with a shown automation lane (2026-11-24)

`cargo test -p atomik-ui --release --test bench_shown_memo -- --ignored --nocapture`: a stopped rack of 100 devices
(no cables, a small document) at 1920x1080, one automation lane showing a value, a repaint forced on every paint,
200 paints after two warm-up paints; allocations counted on the thread. "Built each paint" is what every paint did
before (the revision is not declared); "kept" declares the document revision, as the app does. Three runs.

| Rack paint, one shown lane | ms/paint | Allocations/paint |
|---|---|---|
| Overlay built each paint | 0.44 / 0.52 / 0.48 | 892 |
| Overlay kept while nothing changed | 0.41 / 0.43 / 0.43 | 586 |

The document in this bench is small; a project with a console, assets, cables and macros copies more per build, so the
gain there is larger. The 586 allocations left are the other per-paint work of `prepare` and the paint itself.

## Start-up stages (2026-11-24)

Release build, a clean settings folder and library, `--no-midi --quit-after 5`, no playback. Five starts per profile
(normal; LP4 = affinity 0xF and `--workers 2`; WARP = `WGPU_BACKEND=dx12` with the Basic Render Driver). Times are
milliseconds after the process began, from the `startup` log lines: median / max of the five. "Before" is the base of
this change. "After" is the change with the adapter listing started at the very top of `run()`, before the audio
stream; the shipped placement starts it right after the stream runs (a few lines later) and has NOT been measured, see
the note below.

| Stage | Normal before | Normal after | LP4 before | LP4 after | WARP before | WARP after |
|---|---|---|---|---|---|---|
| Audio stream running | 46 / 51 | 254 / 265 | 49 / 50 | 260 / 266 | 48 / 50 | 65 / 68 |
| Event loop created | n/a | 388 / 398 | n/a | 396 / 412 | n/a | 207 / 217 |
| State built, fonts read | n/a | 392 / 402 | n/a | 400 / 418 | n/a | 211 / 221 |
| First window created | 452 / 496 | 402 / 414 | 476 / 591 | 416 / 433 | 250 / 307 | 226 / 232 |
| Adapters listed | n/a | 459 / 529 | n/a | 471 / 503 | n/a | 282 / 292 |
| Device created | n/a | 536 / 610 | n/a | 548 / 583 | n/a | 352 / 441 |
| Adapter chosen | 975 / 1275 | 781 / 862 | 1026 / 1161 | 799 / 842 | 584 / 642 | 360 / 450 |
| Device and renderer ready | 987 / 1911 | 794 / 873 | 1048 / 1180 | 814 / 858 | 3118 / 3179 | 2842 / 2968 |
| First frame presented | 1019 / 1941 | 825 / 909 | 1076 / 1217 | 848 / 894 | 3166 / 3225 | 2917 / 3040 |

What the table says. The window used to be visible and blank from its creation to the first present: about 570 ms
(normal), 600 ms (LP4) and 2.9 s (WARP). Listing the adapters is the slow step on a real GPU (about 500 ms in the
base, from window to adapter chosen); building the renderer is 12 to 20 ms there. On the software adapter the
renderer build (shader compilation) is about 2.5 s of the 3.1 s: that is the shader compiler's cost and a worker
cannot hide it, since the first frame needs it. Fonts take 2 to 4 ms, the event loop 130 to 180 ms.

A first, cold run of the base measured 1456 ms to the first frame (normal); the table uses the warm second set.

Note on the audio row: with the listing worker started before the audio stream, the stream came up at 254 ms instead
of 46 ms (the driver's first look at the machine slows the opening of the audio device). The shipped change therefore
starts the worker after the audio stream runs. That placement was not measured (the app was not started again); the
owner's check: start time with the change, and the audio row.

## Plugin load and overhead sweep (2026-11-24)

`cargo test --release -p atomik-plugin-hostd --test sweep -- --ignored --nocapture`
(see [docs/TESTING.md](../docs/TESTING.md#plugin-load-and-overhead-sweep) for the columns), the reference machine above,
the release test profile, nothing else running, one run. N instances of a gain plugin loaded one after the other
through the plugin manager, then four seconds of silence in 256-frame callbacks at 48 kHz. Load is the median of one
`instantiate` call; MB, threads and handles are per instance and cover this process and every process below it,
console hosts included (Windows starts a hidden console host for each child that has no window). CPU is a percentage of
one core for the whole set of N instances: "parent" is the test process, which also paces the batches and carries the
host side of every round trip, "child" is all children together. A shared child holds 16 instances, so 30 instances
make two. The first instance of a format loads its library, so the rows of N = 1 carry that cost (about 10 MB for the
test CLAP library, and about 1 MB for the VST3 one).

```text
shell  where             N  load ms/inst  children   MB/inst  thr/inst  hdl/inst  cpu parent  cpu child
null   child each         1      41.1          1       3.6      14.0     236.0       0.0%       1.0%
null   child each        10      40.3         10       3.3      14.0     235.0       7.9%      43.4%
null   child each        30      41.5         30       3.3      14.0     235.0      19.0%     112.8%
null   child per file     1      40.4          1       2.8      17.0     234.0       0.3%       0.7%
null   child per file    10       9.3          1       0.7       5.3      30.6       7.2%      36.6%
null   child per file    30       9.2          2       0.7       4.9      23.1      19.4%     102.9%
null   one shared child   1      37.1          1       3.7      17.0     234.0       1.7%       1.4%
null   one shared child  10       9.1          1       0.8       5.3      30.6       5.8%      31.5%
null   one shared child  30       9.2          2       0.7       4.9      23.1      21.5%     121.0%
clap   in process         1       0.1          0      10.8       0.0       0.0       0.3%       0.0%
clap   in process        10       0.0          0      11.1       0.0       0.0       1.7%       0.0%
clap   in process        30       0.0          0      11.1       0.0       0.0       3.4%       0.0%
clap   child each         1     105.0          1      13.2      14.0     235.0       2.0%       0.7%
clap   child each        10      44.0         10      14.0      14.0     235.0       7.4%      37.8%
clap   child each        30      42.5         30      14.1      14.0     235.0      23.3%     122.0%
clap   child per file     1      39.4          1      15.7      17.0     234.0       1.4%       2.4%
clap   child per file    10       9.6          1      11.7       5.3      30.6       7.5%      38.8%
clap   child per file    30       9.2          2      11.6       4.9      23.1      22.9%     114.6%
clap   one shared child   1      39.6          1      14.4      17.0     234.0       0.3%       0.7%
clap   one shared child  10       8.9          1      11.7       5.3      30.6       7.8%      29.3%
clap   one shared child  30       9.1          2      11.6       4.9      23.1      24.8%     118.7%
vst3   in process         1       0.4          0       1.0       0.0       0.0       1.7%       0.0%
vst3   in process        10       0.4          0       1.2       0.0       0.0       2.0%       0.0%
vst3   in process        30       0.4          0       1.2       0.0       0.0       1.4%       0.0%
vst3   child each         1      42.6          1       4.9      14.0     235.0       1.0%       0.0%
vst3   child each        10      43.7         10       4.3      14.0     235.0       2.7%       6.1%
vst3   child each        30      43.9         30       4.3      14.0     235.0       7.4%      32.7%
vst3   child per file     1      42.7          1       4.4      17.0     234.0       0.3%       0.7%
vst3   child per file    10       9.6          1       1.8       5.3      30.6       2.7%       8.9%
vst3   child per file    30       9.9          2       1.8       4.9      23.1       6.1%      23.2%
vst3   one shared child   1      41.8          1       4.3      17.0     234.0       0.7%       0.7%
vst3   one shared child  10      11.2          1       1.8       5.3      30.6       2.0%       4.3%
vst3   one shared child  30      10.3          2       1.8       4.9      23.1       6.8%      27.2%
```

What the rows say. A child costs about 40 ms to start and, with its console host, about 3.5 MB of private bytes, 14
threads (8 in the child, 6 more in this process and the console host together) and 235 handles, and the cost per
instance does not grow with N. A shared child starts an instance in about 9 ms and holds it for under 1 MB, 5 threads
and 23 to 31 handles. The "child" column is the plugin's work plus the child's own loop, so it differs between the
plugins: the test CLAP gain costs about four times what the VST3 one does. The in-process rows show the figures of the
plugin library alone (the first load), since adding an instance adds no thread or handle. The N = 1 row of "clap child
each" shows one slow first start (105 ms), a spread the other runs of the same row did not have (40 to 45 ms).
## Callback timing over a project (2026-11-24)

`cargo test --release -p atomik-render --test callback_timing -- --ignored --nocapture`
([docs/TESTING.md](../docs/TESTING.md#callback-timing-over-a-project-ignored-by-default)): the engine of a project as a
bounce builds it, live (`EngineOptions::realtime`), driven from a plain thread on an absolute-deadline schedule at
48 kHz, 30 s of audio per row after one second of warm-up, no audio device, no thread promotion. The time of each
`Engine::render` call goes into the callback histogram (p50 and p99 are the top of a bucket, exact to 25 %; max is
exact). `misses` counts callbacks that took longer than their period. `fx-demo` is the factory demo with the lowest
offline real-time factor (16.5x, against 18.9x for `character-demo` and 37x or more for the others); `stress-song`
is the generated 8-track song of the benches. The desktop session was in use during both runs.

**Normal (all 32 threads).**

| project | buffer | workers | callbacks | p50 us | p99 us | max us | misses | p99 / period |
|---|---|---|---|---|---|---|---|---|
| fx-demo | 64 | 0 | 22500 | 81.9 | 131.1 | 203.1 | 0 | 9.8 % |
| fx-demo | 64 | 2 | 22500 | 81.9 | 163.8 | 773.9 | 0 | 12.3 % |
| fx-demo | 128 | 0 | 11250 | 163.8 | 327.7 | 363.1 | 0 | 12.3 % |
| fx-demo | 128 | 2 | 11250 | 163.8 | 327.7 | 673.2 | 0 | 12.3 % |
| fx-demo | 256 | 0 | 5625 | 327.7 | 524.3 | 579.0 | 0 | 9.8 % |
| fx-demo | 256 | 2 | 5625 | 327.7 | 655.4 | 851.8 | 0 | 12.3 % |
| fx-demo | 512 | 0 | 2813 | 655.4 | 1048.6 | 1134.1 | 0 | 9.8 % |
| fx-demo | 512 | 2 | 2813 | 655.4 | 1164.6 | 1164.6 | 0 | 10.9 % |
| stress-song | 64 | 0 | 22500 | 41.0 | 81.9 | 148.5 | 0 | 6.1 % |
| stress-song | 64 | 2 | 22500 | 49.2 | 81.9 | 458.2 | 0 | 6.1 % |
| stress-song | 128 | 0 | 11250 | 81.9 | 163.8 | 325.3 | 0 | 6.1 % |
| stress-song | 128 | 2 | 11250 | 98.3 | 163.8 | 551.4 | 0 | 6.1 % |
| stress-song | 256 | 0 | 5625 | 163.8 | 327.7 | 362.7 | 0 | 6.1 % |
| stress-song | 256 | 2 | 5625 | 196.6 | 327.7 | 498.0 | 0 | 6.1 % |
| stress-song | 512 | 0 | 2813 | 327.7 | 603.2 | 603.2 | 0 | 5.7 % |
| stress-song | 512 | 2 | 2813 | 393.2 | 655.4 | 926.6 | 0 | 6.1 % |

**LP4: process affinity 0xF set right after launch (two cores, four threads); the workers are the rows' own.**

| project | buffer | workers | callbacks | p50 us | p99 us | max us | misses | p99 / period |
|---|---|---|---|---|---|---|---|---|
| fx-demo | 64 | 0 | 22500 | 81.9 | 131.1 | 187.6 | 0 | 9.8 % |
| fx-demo | 64 | 2 | 22500 | 81.9 | 163.8 | 295.3 | 0 | 12.3 % |
| fx-demo | 128 | 0 | 11250 | 163.8 | 262.1 | 405.8 | 0 | 9.8 % |
| fx-demo | 128 | 2 | 11250 | 163.8 | 327.7 | 2954.3 | 1 | 12.3 % |
| fx-demo | 256 | 0 | 5625 | 327.7 | 524.3 | 596.0 | 0 | 9.8 % |
| fx-demo | 256 | 2 | 5625 | 327.7 | 655.4 | 666.6 | 0 | 12.3 % |
| fx-demo | 512 | 0 | 2813 | 655.4 | 1048.6 | 1103.3 | 0 | 9.8 % |
| fx-demo | 512 | 2 | 2813 | 655.4 | 1146.4 | 1146.4 | 0 | 10.7 % |
| stress-song | 64 | 0 | 22500 | 41.0 | 81.9 | 152.5 | 0 | 6.1 % |
| stress-song | 64 | 2 | 22500 | 49.2 | 81.9 | 1804.1 | 3 | 6.1 % |
| stress-song | 128 | 0 | 11250 | 81.9 | 163.8 | 208.5 | 0 | 6.1 % |
| stress-song | 128 | 2 | 11250 | 98.3 | 196.6 | 1934.0 | 0 | 7.4 % |
| stress-song | 256 | 0 | 5625 | 163.8 | 327.7 | 676.8 | 0 | 6.1 % |
| stress-song | 256 | 2 | 5625 | 196.6 | 327.7 | 2526.9 | 0 | 6.1 % |
| stress-song | 512 | 0 | 2813 | 327.7 | 655.4 | 727.0 | 0 | 6.1 % |
| stress-song | 512 | 2 | 2813 | 393.2 | 655.4 | 1842.0 | 0 | 6.1 % |

What the tables say. Both projects stay far inside the budget: the p99 uses 6 to 12 % of the period at every buffer
size, and the only callbacks that ran late are four outliers of the LP4 run. The cost per frame is flat across 64 to
512 (a callback of 64 frames takes about 82 us at the median for `fx-demo`, one of 512 about 655 us), so a smaller
buffer does not cost more per second of audio here. Two workers do not help these callbacks: the load rule
(`load_gate.rs`) keeps the levels on the audio thread until it needs help, and with two workers the p99 and the max
are a little higher (`fx-demo` 164 us against 131 us at 64 frames). The slowest callbacks (max) are single outliers of
one to three milliseconds on a machine in use at the same time, mostly with workers 2; the four misses (one at 128
frames of `fx-demo`, three at 64 frames of `stress-song`, all with two workers on the pinned process) are such
outliers and do not repeat in the other rows. The harness thread spins between callbacks, so the numbers are lower
bounds of a driver's thread (caches and clock stay warm); only a comparison between builds on this machine means
something.

### Children with no console host (2026-11-25)

The same sweep, the same machine and build, after the sandbox and the scanner started their children with
`DETACHED_PROCESS` instead of `CREATE_NO_WINDOW`. The `consoles` column (console hosts below this process) is 0 in
every row; it was one per child. One run, per instance, null shell (the clap and vst3 rows differ by the plugin
library only):

| Row | children | MB (before, after) | threads (before, after) | handles (before, after) |
|---|---|---|---|---|
| child each, N = 10 | 10 | 3.3, 2.2 | 14.0, 12.0 | 235, 146 |
| child each, N = 30 | 30 | 3.3, 2.1 | 14.0, 12.0 | 235, 146 |
| child per file, N = 30 | 2 | 0.7, 0.7 | 4.9, 4.7 | 23.1, 17.1 |
| one shared child, N = 30 | 2 | 0.7, 0.6 | 4.9, 4.7 | 23.1, 17.1 |

Load per instance, CPU and the plugin libraries' own bytes did not change beyond the run-to-run spread (child each
40 ms before, 30 ms after on this run; one run of each, not a claimed gain).

## Thin LTO for the shipped programs: measured, not adopted (2026-11-24)

Release build vs the same build with `lto = "thin"` (codegen-units unchanged). atomik-render, `--loop-count 4`, median of 5, process CPU ms:

| Project | current | thin LTO | change |
|---|---|---|---|
| fx-demo | 4281 | 4141 | -3.3 % |
| character-demo | 3688 | 3672 | -0.4 % |
| audio-demo | 797 | 781 | -2 % |
| master-demo | 813 | 781 | -4 % |
| sampler-demo | 438 | 453 | +3 % |

Criterion (median): render song/workers0/block256 186.2 -> 177.5 ms; rack 719.9 -> 693.3 ms; large-song 1.462 -> 1.575 s (noisy); dsp biquad-stereo/4096 ~23 -> ~27 us (worse); chorus +1-2 %; compressor +1 %; oversample 4x +1-5 %.
Binaries +2.2 % (atomik) / +2.4 % (atomik-render); build time ~3x. Decision: not adopted (no DSP gain >= 5 %).

## CPU renderer, one main-window frame (spike, 2026-11-24)

Headless, the MCP screenshot path (`paint_in(Main, CpuCanvas)` + `finish()`), release, 3 warm-up + 20 frames, median / p95 ms. vello_cpu is single-threaded, so the 4-thread profile equals unrestricted.

| Scene | 1366x768 | 1920x1080 |
|---|---|---|
| empty project | 16.0 / 16.2 | 18.8 / 22.5 |
| master-demo | 19.5 / 20.0 | 27.9 / 35.4 |
| fx-demo | 27.6 / 28.1 | 32.6 / 39.0 |
| player-demo | 28.6 / 29.2 | 31.8 / 34.4 |
| stress rack (rack view) | 47.4 / 48.1 | 57.1 / 62.7 |

Recording (path-to-strip work while painting) dominates: 13-43 ms; rasterising 3-5 ms. The CPU canvas has no display-list cache (the GPU canvas caches by id and revision). Harness kept outside the repo: orchestration/spike56_frame_time.rs.

## Heap operations per audio block and per UI frame (2026-11-25)

Counted with the testkit's allocator (`forbid_on_this_thread`, `assert_no_alloc`, `allocations_of`), 48 kHz.

| What | Count |
|---|---|
| stress song, 10 s, blocks of 256, 0 and 2 workers, audio thread | 0 per block |
| stress song, same, worker threads | 0 |
| stress rack (100 devices, 300 cables), same | 0 per block, 0 on the workers |
| idle shell frame, empty, patch open, MIDI mappings, analysers shown or hidden | 0 |
| idle shell frame, fx-demo open (console meter feed) | 19 per frame (570 over 30 frames), now 0 |
| playing shell frame, fx-demo, 60 frames a second, playhead moving | 38 per frame (2263 over 60 frames), now 715 over 60 frames |

Where the fx-demo frames allocated: `console_readings_with` (the list of channel readings and a key clone per strip), `ConsoleView::set_meters`
and `RackView::set_mix_meters` / `set_device_meter_values` (a key clone each to look the device up), and, only while playing,
`tempo_lane::tempo_at`, which built a tempo table for the playhead position. The meter feed now keeps its readings in a buffer
(`AppCore::console_readings_into`) and hands the console the keys of `meter_points` by reference (`ConsoleView::set_meters_of`),
the rack looks a device up by the borrowed text of its key (`RackView::local_name`), and the arrangement compiles its tempo
once per song change (`tempo_lane::TempoCurve`). What remains while playing happens in five of the sixty frames, one per
window of the device CPU readout (`AppCore::feed_cpu_loads` rebuilding each track's chain summary, about 140 heap operations
a pass); `crates/app/tests/idle_frame_alloc.rs` bounds both counts (0 idle, 800 playing).
`tempo_lane::tempo_at`, which builds a tempo table for the playhead position. Not fixed here (UI code, not the audio path);
`crates/app/tests/idle_frame_alloc.rs` bounds both counts.
## A headless edit loop for ten minutes (2026-11-24)

`crates/mcp/tests/soak.rs` (see [docs/TESTING.md](../docs/TESTING.md#the-headless-soak-ignored-by-default)): the
headless session on `fx-demo` runs a random action of twelve per iteration (add or remove a device, turn a knob,
scroll and zoom, open and close a panel, select a track, undo and redo, save) with the undo history full from the
start, and paints the main window on the CPU canvas every iteration. Test profile of the workspace (optimised, debug
info), Windows, the machine above, no other load. About 12 to 20 iterations a second. Private memory is private bytes
from `Get-Process`; the slope is a least squares line through the samples after the first 20 % of the run.

One 600 second run (every seventh row of the table; the second 600 second run is within 15 % of it):

```text
   t(s)  iteration  resident(MiB)  private(MiB)  threads  undo-depth
    0.3          1           35.5          16.1        5        1000
   54.7        824           52.0          30.6        3        1000
  109.1       1466           50.3          27.9        2        1000
  164.0       2074           57.9          35.4        2         999
  218.5       2786           60.0          37.8        2        1000
  273.0       3483           61.8          41.8        2        1000
  327.5       4119           65.9          45.7        4        1000
  382.7       4740           68.6          46.6        4        1000
  437.4       5455           68.3          45.9        2        1000
  492.7       6055           71.5          49.8        2        1000
  548.1       6621           74.1          52.1        2         996
  595.5       7153           77.1          54.8        2        1000

warm-up 120 s, 61 samples after it
private memory slope after warm-up: +2.517 MiB/min
private memory finish / end of warm-up: 1.592
```

| Run | Slope after warm-up | Finish / end of warm-up |
|---|---|---|
| 600 s, first | +2.885 MiB/min | 1.689 |
| 600 s, second (the run above) | +2.517 MiB/min | 1.592 |
| 120 s, first | +4.154 MiB/min | 1.269 |
| 120 s, second | +5.383 MiB/min | 1.336 |
| 120 s, third | +4.275 MiB/min | 1.214 |

The runs above are from before the fix described below; the bounds of the test are now 2 MiB a minute and 2 times.

**A climb that looked like a real leak (explained further down: the loop's own doing).** The private memory does not
settle on the full loop: it goes from about 28 MiB at the end of the warm-up to about 53 MiB after ten minutes, 2.5 to
2.9 MiB a minute, about 3 KB an iteration; the resident memory follows, the thread count stays at 2 to 5. The loop was
run again with one group of actions at a time (100 or 150 seconds, same machine; slopes of this size are within the
noise of a run that short):

| Actions | Slope after warm-up |
|---|---|
| paint only (a track is selected) | +0.1 MiB/min (the table is flat within 0.3 MiB) |
| devices (add, remove) | -0.1 MiB/min |
| params | +0.3 MiB/min |
| zoom | +0.9 MiB/min (table flat within 0.8 MiB) |
| panels | +0.2 MiB/min |
| undo, redo (of the filled history) | +1.2 MiB/min (table flat within 0.4 MiB) |
| save | +0.4 MiB/min |
| devices and save | +0.2 MiB/min |
| devices and undo, redo | +2.0 MiB/min (150 s: from 22 to 27 MiB) |
| zoom, panels, select, params | -0.01 MiB/min |

The one combination that climbs on its own is adding and removing devices together with undo and redo; it also makes
up about two thirds of the climb of the full loop. The other groups are flat within the noise of the run. The soak
with `ATOMIK_SOAK_ACTIONS` found the combination; counting found the cause.

**Where the climb came from (2026-11-25): the loop, not the app.** The table above is explained by the soak's own
bookkeeping. The loop kept a list of the devices it had added and removed the oldest of them; an undo of a removal
brings a device back that the list had forgotten, and an undo of an add takes one away that the list still held, so
the rack grew without end: the document held 19 devices after 300 iterations of devices and undo and 106 after 12 000,
and every device brings its own memory (and its undo steps). The history itself was never the cause: it stays at its
limit of 1000 steps from the start, far below its byte budget. Counting, not guessing: a test of the shell without an
audio device ran the same script and measured the heap the shell held after each 300 iterations (on one thread, with
the allocator guard of the test kit). The net heap rose by 0.8 MiB over 40 blocks (12 000 iterations) with the rack
growing from 19 to 106 devices, and was flat (a few KiB either way, no trend) with the rack at 21 to 22 devices once
the loop read the added devices from the document. The counts showed no device, scope ring or cache outliving its
device. The loop now does that, and fails when the rack ever holds more than six added devices.

| Run (after the fix) | Slope after warm-up | Finish / end of warm-up |
|---|---|---|
| 600 s, all actions | +0.158 MiB/min | 1.001 |
| 120 s, first | +0.531 MiB/min | 1.117 |
| 120 s, second | +1.017 MiB/min | 1.039 |
| 150 s, devices and undo only | +0.387 MiB/min | 1.031 |

The private memory of the 600 second run stays between 28 and 29 MiB from the end of the warm-up to the finish, with
21 to 22 devices in the rack. The bounds are now 2 MiB a minute and 2 times: about twice the worst short run.

## Leak loops (2026-11-25)

`crates/plugin-hostd/tests/leak_loops.rs` and `crates/mcp/tests/leak_loop.rs` (see
[docs/TESTING.md](../docs/TESTING.md#the-leak-loops)): 200 cycles, read at cycle 10, 100 and 200 (test profile,
Windows, the machine above, no other load). Threads, handles and children are asserted EQUAL at cycle 10 and 200;
private bytes are reported only. Threads and handles count this process and everything below it; the thread count
differs between shells because the first child of a shell starts threads that end with its sandbox.

| Shell, 200 cycles | Cycle | Children | Threads | Handles | Private (MiB) |
|---|---|---|---|---|---|
| null plugin, one child per instance | 10 | 0 | 5 | 128 | 1.9 |
| | 100 | 0 | 5 | 128 | 2.7 |
| | 200 | 0 | 5 | 128 | 2.9 |
| test CLAP plugin, one child per instance | 10 | 0 | 2 | 128 | 2.5 |
| | 100 | 0 | 2 | 128 | 2.3 |
| | 200 | 0 | 2 | 128 | 2.5 |
| test CLAP plugin, in this process | 10 | 0 | 2 | 120 | 2.9 |
| | 100 | 0 | 2 | 120 | 3.8 |
| | 200 | 0 | 2 | 120 | 3.8 |
| `session_open` / `session_close` on `fx-demo` | 10 | - | 5 | 137 | 8.2 |
| | 100 | - | 5 | 137 | 19.4 |
| | 200 | - | 5 | 137 | 20.0 |

The session loop's private bytes rise by 11 MiB over the first 100 cycles and by 0.6 MiB over the next 100: the
allocator and the caches fill and then stay. The counts of the others do not move.
