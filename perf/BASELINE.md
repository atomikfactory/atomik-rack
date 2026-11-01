# Benchmark baseline

Wall-clock medians and 99th percentiles (21 samples) of the benches in
[docs/TESTING.md](../docs/TESTING.md#benchmarks-and-the-performance-job), taken once on the
reference machine below with the default settings (`cargo bench -p <crate> --bench <name>`).
Re-measure on your own machine before comparing: only numbers from one machine compare.
Instruction-count baselines (`perf/baseline-instructions.txt`) need valgrind on Linux and are
committed from the first run of the scheduled workflow.

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
per-device timing reads the clock twice around each device call; it is on only while View > CPU
Meters is. A bounce renders the same samples either way.

```text
case                                   off           timing on
render/rack/workers0/block256          754.2 ms      777.0 ms
render/console-64/workers0/block256    1.486 s       1.565 s
```

The rack case is 100 cheap devices, so it is the worst case for the overhead: about 70 ns per
device per 64-frame batch, which is 0.5 % of the 1.33 ms callback at 48 kHz for 100 devices. With
the readout off a device call pays two relaxed atomic loads.

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
