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
