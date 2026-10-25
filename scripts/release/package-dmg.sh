#!/bin/sh
# Builds a disk image from an app folder: Atomik Rack.app and an Applications symlink.
# Usage: package-dmg.sh <folder holding "Atomik Rack.app"> <output.dmg>   (macOS, hdiutil only)
# The image is a compressed read-only UDZO with the volume name "Atomik Rack". It is not signed
# here: sign-macos.sh does that (and notarize-macos.sh), because it needs the secrets.
set -eu
src="$1"
out="$2"
test -d "$src/Atomik Rack.app" || { echo "error: no Atomik Rack.app in $src" >&2; exit 1; }
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto "$src/Atomik Rack.app" "$stage/Atomik Rack.app"
ln -s /Applications "$stage/Applications"
rm -f "$out"
hdiutil create -volname "Atomik Rack" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$out"
hdiutil verify "$out"
