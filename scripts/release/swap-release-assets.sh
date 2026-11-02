#!/bin/sh
# Replaces the assets of an existing GitHub release without ever leaving it empty: the new files
# are uploaded first (a file with the same name is replaced in place), and only then are the old
# assets that are not part of the new set deleted. A reader of the release always sees a complete
# set, the old one or the new one, apart from the instant a same-named file (SHA256SUMS) is swapped.
# Usage: swap-release-assets.sh <tag> <repo> <file>...
# DRY_RUN=1 prints what would be uploaded and deleted and changes nothing (`gh release view` is
# still read). Needs `gh` and GH_TOKEN, like the workflows.
set -eu
tag="$1"
repo="$2"
shift 2
[ "$#" -gt 0 ] || { echo "error: no files to upload" >&2; exit 1; }
for f in "$@"; do
  [ -f "$f" ] || { echo "error: $f is not a file" >&2; exit 1; }
done

old="$(gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')"

if [ "${DRY_RUN:-0}" = "1" ]; then
  echo "dry run: would upload $# file(s) to $tag (existing assets: $(printf '%s' "$old" | grep -c . || true))"
else
  gh release upload "$tag" "$@" --repo "$repo" --clobber
fi

# What stays: the names just uploaded. Everything else the release held before is stale.
keep="$(for f in "$@"; do basename "$f"; done)"
printf '%s\n' "$old" | while IFS= read -r asset; do
  [ -n "$asset" ] || continue
  if printf '%s\n' "$keep" | grep -qxF -- "$asset"; then
    continue
  fi
  if [ "${DRY_RUN:-0}" = "1" ]; then
    echo "dry run: would delete the old asset $asset"
  else
    gh release delete-asset "$tag" "$asset" --repo "$repo" --yes
  fi
done
