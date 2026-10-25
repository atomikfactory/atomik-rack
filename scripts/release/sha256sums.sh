#!/bin/sh
# Writes <dir>/SHA256SUMS over every release file in <dir> (not the .sha256 files, not SHA256SUMS
# or its signature) and checks it.
# Usage: sha256sums.sh <dir>
set -eu
dir="$1"
cd "$dir"
files="$(find . -maxdepth 1 -type f ! -name '*.sha256' ! -name 'SHA256SUMS*' | sed 's|^\./||' | LC_ALL=C sort)"
if [ -z "$files" ]; then
  echo "error: no release files in $dir" >&2
  exit 1
fi
if command -v sha256sum > /dev/null 2>&1; then
  printf '%s\n' "$files" | tr '\n' '\0' | xargs -0 sha256sum > SHA256SUMS
  sha256sum -c SHA256SUMS
else
  printf '%s\n' "$files" | tr '\n' '\0' | xargs -0 shasum -a 256 > SHA256SUMS
  shasum -a 256 -c SHA256SUMS
fi
