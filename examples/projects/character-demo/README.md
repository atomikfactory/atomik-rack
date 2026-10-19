# character-demo

A project for trying the factory effect Character (`atomik.character`) in the three ways it is
used: automated through a container, patched with a note CV, and in the long latency class. Eight
bars at 120 BPM in A minor (Am F C G, twice). No audio files, so no bundle folder.

| Device | What it is |
|---|---|
| Lead | a container: a poly synth (saw, 4 voices) cabled into a Character; macro 1 **Preset** drives the preset, macro 2 **Transform** drives Transform; the song track plays an arpeggio and automates both macros |
| Bass Line | a step sequencer (16 sixteenths, a little swing) playing Bass |
| Bass | a poly synth (one voice, low cutoff); its console insert holds **Bass Character** on `ion-sweep`, and the sequencer's `pitch_cv_out` is cabled into the Character's `transform_cv_in` (trim 50 %): the note sets how far the comb and the filter sweep |
| Pad | a poly synth with long chords; its console insert holds **Pad Character** on `lossy`, the preset with the spectral stage (the long latency class, 1070 frames at 48 kHz): the console's delay compensation delays Lead and Bass by the difference so the three stay aligned |

The Lead's preset lane steps every two bars through `reel`, `swirl`, `dusty-groove` and
`pocket-radio` (all short class, 46 frames at 48 kHz); the Transform lane rises from 20 % to 90 %
over four bars and falls back to 40 %. A song lane reaches an effect only through a container's
macro, which is why the Lead is a container.

```text
cargo run --release -p atomik-app -- examples/projects/character-demo/character-demo.atomik
```

Press Space to play. Things to try:

- Open the Lead container and the Character panel: the matrix lights the preset the lane has
  chosen, the screen and the meter strip move with the signal, and the Transform knob follows the
  lane. Click another preset while the song plays: one click, one crossfade, no click in the sound.
- Select Pad Character and click a short-class preset instead of `lossy` (for example `reel`): the
  panel's badge changes from `LAT LONG` to `LAT SHORT`, and for a moment the compensation realigns
  the rest of the mix. Click `lossy` again to hear the long class back.
- On Bass Character, change the trim of the Transform CV input (back of the panel, Tab flips the
  rack) from 0 to 100 %: at 0 the sweep stands still, at 100 % the notes move it.

Regenerate the file after an intentional change with
`ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test character_demo regenerate`.
