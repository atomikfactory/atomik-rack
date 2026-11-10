#!/bin/sh
# Checks what can be checked without Linux tools: the dry run of the AppImage script lists the
# layout the app needs, and a bad label fails. Usage: sh scripts/release/package-appimage-test.sh
set -eu
out="$(sh scripts/release/package-appimage.sh --dry-run v0.4.0)"
for want in "usr/bin/atomik" "usr/bin/atomik-plugin-scan" "usr/bin/atomik-plugin-host" "usr/bin/factory/" \
  "AppRun" "io.github.atomikfactory.AtomikRack.desktop" ".DirIcon" \
  "usr/share/metainfo/io.github.atomikfactory.AtomikRack.metainfo.xml" \
  "output: dist/atomik-rack-0.4.0-x86_64.AppImage"; do
  printf '%s\n' "$out" | grep -qF -- "$want" || { echo "error: dry run lacks: $want" >&2; exit 1; }
done
if sh scripts/release/package-appimage.sh --dry-run not-a-label > /dev/null 2>&1; then
  echo "error: a bad label was accepted" >&2
  exit 1
fi
nightly="$(sh scripts/release/package-appimage.sh --dry-run nightly-20261110-abc1234 | grep '^output:')"
[ "$nightly" = "output: dist/atomik-rack-nightly-20261110-abc1234-x86_64.AppImage" ] || { echo "error: $nightly" >&2; exit 1; }
echo "package-appimage: ok"
