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
