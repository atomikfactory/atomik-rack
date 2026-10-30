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
  "$corpus/audio_decode" "$corpus/ipc_frame"

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
