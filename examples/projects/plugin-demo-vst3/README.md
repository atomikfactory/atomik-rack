# plugin-demo-vst3

A project that mixes CLAP and VST3 plugins, made of the in-repo test plugins
(`crates/testplugins/clap` and `crates/testplugins/vst3`, the same DSP in two formats) and one
built-in synth, to try both plugin formats without installing anything.

| Device | What it is |
|---|---|
| Lead | `atomik.test.synth` (CLAP instrument), console channel, song track |
| Echo | Atomik Test Gain (**VST3** effect, 128 frames of latency) on Lead's channel insert |
| Bass | built-in poly synth, feeds Echo's sidechain input |
| Arp | Atomik Test Note Shift (**VST3** note effect, +7 semitones), song track |
| Pad | `atomik.test.synth` (CLAP), fed by Arp's notes out: notes cross the format border |
| Vox | Atomik Test Synth (**VST3** instrument), console channel, song track |
| Wash | `atomik.test.gain` (CLAP effect, 64 frames of latency) on Vox's channel insert |
| Box | a container holding a **VST3** test synth, console channel, song track |

A project never stores a plugin path: a plugin is found by its (format, id) in your scan cache.
So build the test plugins, then let the scanner see them.

## Build and find the test plugins

```text
cargo build -p atomik-test-clap -p atomik-test-vst3
```

This writes the libraries to `target/debug/` (`atomik_test_clap.dll` and `atomik_test_vst3.dll`
on Windows, `libatomik_test_*.so` on Linux, `libatomik_test_*.dylib` on macOS). Give each its
extension and a folder of its own. CLAP is a plain library renamed to `.clap`; VST3 is a bundle:

```text
mkdir clap-test
cp target/debug/atomik_test_clap.dll clap-test/atomik_test_clap.clap

mkdir -p vst3-test/atomik_test_vst3.vst3/Contents/x86_64-win
cp target/debug/atomik_test_vst3.dll vst3-test/atomik_test_vst3.vst3/Contents/x86_64-win/atomik_test_vst3.vst3
```

(Linux: `Contents/x86_64-linux/atomik_test_vst3.so`. macOS: `Contents/MacOS/atomik_test_vst3`
plus a `Contents/Info.plist` naming that executable. A single `.vst3` library file also loads on
Windows.) Then either

* set `CLAP_PATH` and put the VST3 folder into `%LOCALAPPDATA%\Programs\Common\VST3` (or the
  folders of your OS) before starting `atomik`, or
* open Settings, Plugins, enable plugins (read the warning), add both folders under Folders and
  press SCAN.

Eight plugins appear in the browser's Plugins group (four per format, each with its badge), and
the demo opens with real devices. (The in-repo tests do all of this for you:
`atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin}`.)

## Open and bounce

```text
atomik examples/projects/plugin-demo-vst3/plugin-demo-vst3.atomik
atomik-render examples/projects/plugin-demo-vst3/plugin-demo-vst3.atomik -o demo.wav --allow-plugins
```

Without opening the app first, the cache of the built test plugins can be written for the bounce
(`ATOMIK_PLUGIN_CACHE_OUT=cache.json cargo test -p atomik-render --test plugin_demo_vst3 regenerate`,
then `--plugin-cache cache.json`).

`atomik-render` refuses a project with plugins unless `--allow-plugins` is given, reads the same
`plugins.json` cache as the app (or `--plugin-cache <path>`), bounces with the plugins in offline
mode (`--plugin-mode realtime` for the live mode) and prints
`plugins: 7 (determinism not guaranteed)`. For these test plugins the WAV is bit-identical across
runs, `--block` sizes and `--workers`, and equal to the same rack built from the CLAP test
plugins alone (the two formats run the same arithmetic). With a plugin missing (say the VST3
folder is not scanned) the bounce fails naming it, or use `--missing-plugins stand-in`.

## Stress

```text
atomik --stress-plugins --play
```

builds this project: 8 test synths and 8 test effects, half of each as VST3
when the VST3 test plugin is built (`cargo build -p atomik-test-vst3`).

## Trademark

VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other
countries.

## Regenerate

```text
ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test plugin_demo_vst3 regenerate
```
