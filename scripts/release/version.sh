#!/bin/sh
# Prints the release version to stdout and checks it against Cargo.toml.
#   Tag run    (GITHUB_REF=refs/tags/vX.Y.Z): prints the tag; fails unless the tag is
#              "v" + the [workspace.package] version.
#   Nightly    (ATOMIK_CHANNEL=nightly): prints nightly-<YYYYMMDD>-<shortsha>, with no tag
#              check (a nightly is a build of main, not a release). NIGHTLY_DATE overrides the
#              UTC date of today (used by tests).
#   Other runs: prints dev-<shortsha>.
# Run from the repository root.
set -eu

cargo_version="$(awk '
  /^\[workspace\.package\]/ { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version[[:space:]]*=/ { gsub(/.*=[[:space:]]*"|".*/, ""); print; exit }
' Cargo.toml)"
if [ -z "$cargo_version" ]; then
  echo "error: could not read [workspace.package] version from Cargo.toml" >&2
  exit 1
fi

if [ "${ATOMIK_CHANNEL:-}" = "nightly" ]; then
  echo "nightly-${NIGHTLY_DATE:-$(date -u +%Y%m%d)}-$(git rev-parse --short=7 HEAD)"
  exit 0
fi

case "${GITHUB_REF:-}" in
  refs/tags/*)
    tag="${GITHUB_REF#refs/tags/}"
    if [ "$tag" != "v$cargo_version" ]; then
      echo "error: tag $tag does not match Cargo.toml version $cargo_version (expected v$cargo_version)" >&2
      exit 1
    fi
    echo "$tag"
    ;;
  *)
    echo "dev-$(git rev-parse --short HEAD)"
    ;;
esac
