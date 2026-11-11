#!/bin/sh
# Checks the Flatpak packaging without Linux tools: the dry run names the bundle, and
# check-linux-packaging.py checks the manifest, the metainfo and the desktop entry.
# Usage: sh scripts/release/package-flatpak-test.sh
set -eu
out="$(sh scripts/release/package-flatpak.sh --dry-run v0.4.0)"
printf '%s\n' "$out" | grep -qF "output: dist/atomik-rack-0.4.0.flatpak" || { echo "error: the dry run lacks the bundle name" >&2; exit 1; }
# The archive must be the one package-tar.sh writes for the same label (the tag keeps its "v").
printf '%s\n' "$out" | grep -qF "from dist/atomik-rack-v0.4.0-linux-x86_64.tar.gz" || { echo "error: the dry run names another archive than package-tar.sh writes" >&2; exit 1; }
if sh scripts/release/package-flatpak.sh --dry-run not-a-label > /dev/null 2>&1; then
  echo "error: a bad label was accepted" >&2
  exit 1
fi
py=python3
python3 -c pass > /dev/null 2>&1 || py=python
"$py" -I scripts/release/check-linux-packaging.py
echo "package-flatpak: ok"
