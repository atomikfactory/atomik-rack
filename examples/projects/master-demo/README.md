# master-demo

A project made from the starter project, to try the visible master section and the
send and return workflow without building anything first. No audio files, so no bundle folder.

| Device | What it is |
|---|---|
| Audio I/O | on top of the rack; the master's `main` is cabled to outputs 1 and 2 |
| Master | the locked master section (it cannot be deleted or moved); send 1 level is -4 dB |
| Lead | poly synth (saw), made by the creation chain: console channel (folded) and song track; sends to bus 1 at -6 dB |
| Bass | poly synth (low cutoff), same chain, no send |
| Hall | a reverb made with the master selected: it sits on send and return 1 |

```text
cargo run --release -p atomik-app -- examples/projects/master-demo/master-demo.atomik
```

Press Space to play four bars (Am F C G). Things to try:

- Flip the rack (Tab): two cables run from the master to Audio I/O outputs 1 and 2, nothing else.
  The channels' links to the master are not drawn; each channel reads "Output: Master". View,
  Show Console Links draws them.
- Select the master and press Ctrl+E, choose the delay, Enter: it takes send and return 2.
- Move the master's send 1 level; the amount reaching Hall changes. On the master panel, EDIT
  next to return 1 selects Hall.

Regenerate the file after an intentional change with
`ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test master_demo regenerate`.
