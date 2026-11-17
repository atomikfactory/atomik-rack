#!/bin/sh
# Fills fuzz/corpus with the seeds of every fuzz target from the files the repository already
# keeps: the committed projects, patches, MIDI and audio fixtures, plus the regression inputs
# of fuzz/regressions (inputs that once made a target fail; each is kept for good).
#
#   sh scripts/fuzz-seed.sh
#
# The corpus folders are not tracked (they grow as the fuzzer finds paths); this script makes
# them again from tracked files. Run it before `cargo fuzz run <target>`.
set -eu

cd "$(git rev-parse --show-toplevel)"
corpus=fuzz/corpus

# byte <0..255>: writes that one byte.
byte() {
  printf "\\$(printf '%03o' "$1")"
}

# prefixed <selector 0..255> <file> <out-dir>: copies the file behind one selector byte.
prefixed() {
  { byte "$1"; cat "$2"; } > "$3/$(basename "$2")"
}

mkdir -p "$corpus/load_project" "$corpus/load_patch" "$corpus/smf_read" \
  "$corpus/audio_decode" "$corpus/ipc_frame" "$corpus/instrument_read"

# Projects.
for f in crates/model/tests/fixtures/*.atomik examples/projects/*.atomik; do
  [ -f "$f" ] && cp "$f" "$corpus/load_project/$(basename "$f")"
done

# Patches (selector 0), and the projects and clips as the other document readers: a project is
# also a library item (selector 2).
for f in examples/patches/*.json; do
  [ -f "$f" ] && prefixed 0 "$f" "$corpus/load_patch"
done
for f in crates/model/tests/fixtures/demo-v36.atomik crates/model/tests/fixtures/container-v8.atomik; do
  [ -f "$f" ] && prefixed 2 "$f" "$corpus/load_patch"
done

# MIDI files.
for f in crates/midi-io/tests/fixtures/*.mid; do
  [ -f "$f" ] && cp "$f" "$corpus/smf_read/$(basename "$f")"
done

# Audio: each fixture through the whole-file decoder (selector 0x?0, the hint is the high
# nibble: 1 wav, 2 flac, 3 mp3, 4 ogg, 5 aif), the WAV readers (0x08) and the preview decoder
# (0x?B).
for f in crates/audio-file/tests/fixtures/*; do
  [ -f "$f" ] || continue
  case "$f" in
    *.wav) hint=1 ;;
    *.flac) hint=2 ;;
    *.mp3) hint=3 ;;
    *.ogg) hint=4 ;;
    *.aif) hint=5 ;;
    *) continue ;;
  esac
  base="$(basename "$f")"
  { byte $((hint * 16)); cat "$f"; } > "$corpus/audio_decode/whole-$base"
  { byte $((hint * 16 + 8)); cat "$f"; } > "$corpus/audio_decode/wav-$base"
  { byte $((hint * 16 + 11)); cat "$f"; } > "$corpus/audio_decode/preview-$base"
done

# Instruments: the small SoundFont (odd selector) and an SFZ text (even).
prefixed 1 crates/audio-file/tests/fixtures/small.sf2 "$corpus/instrument_read"
{ byte 0; printf "%s
" "<group> lovel=1 hivel=63" "<region> sample=a.wav key=c4 loop_mode=loop_continuous"; } > "$corpus/instrument_read/small.sfz"

# The sandbox wire: a few frames (a 4-byte little-endian length, then the JSON).
frame() {
  json="$1"
  len=${#json}
  printf "\\$(printf '%03o' $((len & 255)))\\$(printf '%03o' $(((len >> 8) & 255)))\\000\\000"
  printf '%s' "$json"
}
frame '{"t":"Hello","protocol":1}' > "$corpus/ipc_frame/hello"
frame '{"t":"Instantiate","id":7}' > "$corpus/ipc_frame/instantiate"
frame '{"t":"Activate","id":8,"sample_rate":48000.0,"frames":64,"offline":false}' > "$corpus/ipc_frame/activate"
frame '{"t":"Ok","id":9}' > "$corpus/ipc_frame/ok"
{ frame '{"t":"Hello","protocol":1}'; frame '{"t":"Ok","id":1}'; } > "$corpus/ipc_frame/two"

# Guitar Pro and MusicXML files, tab and chord sheets: the file seeds are made by the test encoders of
# crates/tab (nothing is downloaded); the text seeds are written here.
mkdir -p "$corpus/tab_gp" "$corpus/tab_musicxml" "$corpus/tab_ascii" "$corpus/chord_sheet"
ATOMIK_FUZZ_SEED_DIR="$corpus/tab_gp" cargo test -q -p atomik-tab --test gp_robust write_fuzz_seeds -- --ignored
ATOMIK_FUZZ_SEED_DIR="$corpus/tab_musicxml" cargo test -q -p atomik-tab --test musicxml_robust write_fuzz_seeds -- --ignored
printf '%s\n' 'e|---0---3---|---5---|' 'B|---1-------|---5---|' 'G|-----0-----|-------|' \
  'D|---2-------|---7---|' 'A|---3-------|-------|' 'E|-----------|-------|' > "$corpus/tab_ascii/guitar.txt"
printf '%s\n' 'G|--0h2p0--|' 'D|----------|' 'A|----------|' 'E|--3b5r3~--|' > "$corpus/tab_ascii/marks.txt"
printf '%s\n' '[Verse]' 'Am       F' 'Words made up here' 'C        G' 'More made up words' > "$corpus/chord_sheet/over.txt"
printf '%s\n' '{title: Made Up}' '{capo: 2}' '[Am]Words [F]made up [C]here' > "$corpus/chord_sheet/inline.txt"

# Regression inputs: every file under fuzz/regressions/<target>/ joins that target's corpus.
for dir in fuzz/regressions/*/; do
  [ -d "$dir" ] || continue
  target="$(basename "$dir")"
  mkdir -p "$corpus/$target"
  for f in "$dir"*; do
    [ -f "$f" ] && cp "$f" "$corpus/$target/regression-$(basename "$f")"
  done
done

echo "fuzz corpus ready in $corpus"
