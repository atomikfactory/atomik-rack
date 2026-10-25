#!/bin/sh
# Derives the version strings the installers need from a release label.
# Usage: version-info.sh <label> <field>
#   label  vX.Y.Z[-pre]               a release tag
#          nightly-YYYYMMDD-<sha>      a nightly build
#          dev-<sha>                   a manual build
#   field  base      X.Y.Z (the numeric part)
#          display   the human version: X.Y.Z[-pre], X.Y.Z-nightly.YYYYMMDD+<sha>, X.Y.Z-dev.<sha>
#          deb       the Debian version: pre-releases sort before the release, so
#                    0.1.0-rc.1 -> 0.1.0~rc.1 and nightlies -> 0.1.0~nightlyYYYYMMDD
#          numeric   X.Y.Z.0 (four numeric parts, for Windows file version resources)
# Tags carry their own version; the other labels take the numeric part from Cargo.toml.
# Run from the repository root.
set -eu
label="$1"
field="$2"

cargo_base() {
  version="$(awk '
    /^\[workspace\.package\]/ { in_pkg = 1; next }
    /^\[/ { in_pkg = 0 }
    in_pkg && /^version[[:space:]]*=/ { gsub(/.*=[[:space:]]*"|".*/, ""); print; exit }
  ' Cargo.toml)"
  if [ -z "$version" ]; then
    echo "error: could not read [workspace.package] version from Cargo.toml" >&2
    exit 1
  fi
  printf '%s\n' "${version%%-*}"
}

pre=""
case "$label" in
  v[0-9]*)
    full="${label#v}"
    base="${full%%-*}"
    case "$full" in *-*) pre="${full#*-}" ;; esac
    display="$full"
    if [ -n "$pre" ]; then
      deb="$base~$(printf '%s' "$pre" | tr '-' '.')"
    else
      deb="$base"
    fi
    ;;
  nightly-[0-9]*-*)
    base="$(cargo_base)"
    rest="${label#nightly-}"
    date="${rest%%-*}"
    sha="${rest#*-}"
    display="$base-nightly.$date+$sha"
    deb="$base~nightly$date"
    ;;
  dev-*)
    base="$(cargo_base)"
    sha="${label#dev-}"
    display="$base-dev.$sha"
    deb="$base~dev$sha"
    ;;
  *)
    echo "error: unknown release label: $label" >&2
    exit 1
    ;;
esac

case "$field" in
  base) echo "$base" ;;
  display) echo "$display" ;;
  deb) echo "$deb" ;;
  numeric) echo "$base.0" ;;
  *) echo "error: unknown field: $field" >&2; exit 1 ;;
esac
