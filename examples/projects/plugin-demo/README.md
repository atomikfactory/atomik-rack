# plugin-demo

A project made of the in-repo CLAP test plugins (`crates/testplugins/clap`) and one built-in
synth, to try third-party plugin hosting without installing anything.

| Device | What it is |
|---|---|
| Lead | `atomik.test.synth` (CLAP instrument), console channel, song track |
| Echo | `atomik.test.gain` (CLAP effect, 128 frames of latency) on Lead's channel insert |
| Bass | built-in poly synth, feeds Echo's sidechain input |
| Arp | `atomik.test.noteshift` (CLAP note effect, +7 semitones), song track |
| Pad | a second `atomik.test.synth`, fed by Arp's notes out |
| Box | a container holding a third `atomik.test.synth`, console channel, song track |

A project never stores a plugin path: a plugin is found by its id in your scan cache. So build
the test plugins, then let the scanner see them.

## Build and find the test plugins

```text
cargo build -p atomik-test-clap
```

This writes the library to `target/debug/` (`atomik_test_clap.dll` on Windows,
`libatomik_test_clap.so` on Linux, `libatomik_test_clap.dylib` on macOS). Give it the `.clap`
extension in a folder of its own:

```text
mkdir clap-test
cp target/debug/atomik_test_clap.dll clap-test/atomik_test_clap.clap
```

Then either

* set `CLAP_PATH` to that folder before starting `atomik` (and the scanner), or
* open Settings, Plugins, enable plugins (read the warning), add the folder under Folders and
  press SCAN.

The four plugins appear in the browser's Plugins group, and the demo opens with real devices.
(The in-repo tests do the copy for you: `atomik_plugin_host::testing::clap_test_plugin`.)

## Open and bounce

```text
atomik examples/projects/plugin-demo/plugin-demo.atomik
atomik-render examples/projects/plugin-demo/plugin-demo.atomik -o demo.wav --allow-plugins
```

Without opening the app first, the cache of the built test plugins can be written for the bounce
(`ATOMIK_PLUGIN_CACHE_OUT=cache.json cargo test -p atomik-render --test plugin_demo regenerate`, then
`--plugin-cache cache.json`).

`atomik-render` refuses a project with plugins unless `--allow-plugins` is given, reads the same
`plugins.json` cache as the app (or `--plugin-cache <path>`), bounces with the plugins in offline
mode (`--plugin-mode realtime` for the live mode) and prints
`plugins: 5 (determinism not guaranteed)`. For these test plugins the WAV is bit-identical across
runs, `--block` sizes and `--workers`. With a plugin missing, the bounce fails naming it, or use
`--missing-plugins stand-in`.

## Regenerate

```text
ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test plugin_demo regenerate
```
