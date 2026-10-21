#!/bin/sh
# Writes <file>.sha256 next to each given file (sha256sum format, bare file name).
set -eu
for f in "$@"; do
  dir="$(dirname "$f")"
  base="$(basename "$f")"
  if command -v sha256sum > /dev/null 2>&1; then
    (cd "$dir" && sha256sum "$base" > "$base.sha256")
  else
    (cd "$dir" && shasum -a 256 "$base" > "$base.sha256")
  fi
done
