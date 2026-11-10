#!/bin/sh
# Writes the AppStream metainfo file of a build: the template packaging/linux/*.metainfo.xml.in
# with its releases list made from the release label.
# Usage: linux-metainfo.sh <label> <out-file>   (repository root)
#   label  vX.Y.Z[-pre], nightly-YYYYMMDD-<sha> or dev-<sha>
# The release version is the Debian-style one from version-info.sh (0.4.0~rc.1 sorts before 0.4.0, and
# AppStream reads "~" the same way). A label with a "-" or a nightly or dev label is a development
# release. The date is today (UTC), or the one of SOURCE_DATE_EPOCH, or RELEASE_DATE (YYYY-MM-DD).
set -eu
label="$1"
out="$2"
template="packaging/linux/io.github.atomikfactory.AtomikRack.metainfo.xml.in"
version="$(sh scripts/release/version-info.sh "$label" deb)"
type=stable
case "$version" in *[~]*) type=development ;; esac
date="${RELEASE_DATE:-}"
if [ -z "$date" ]; then
  if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    date="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%d 2> /dev/null || date -u -r "$SOURCE_DATE_EPOCH" +%Y-%m-%d)"
  else
    date="$(date -u +%Y-%m-%d)"
  fi
fi
releases="  <releases>
    <release version=\"$version\" date=\"$date\" type=\"$type\">
      <url type=\"details\">https://github.com/atomikfactory/atomik-rack/blob/main/CHANGELOG.md</url>
    </release>
  </releases>"
mkdir -p "$(dirname "$out")"
awk -v releases="$releases" '$0 == "@RELEASES@" { print releases; next } { print }' "$template" > "$out"
if grep -q '@RELEASES@' "$out"; then
  echo "error: the releases marker was not replaced" >&2
  exit 1
fi
echo "wrote $out ($version, $date, $type)"
