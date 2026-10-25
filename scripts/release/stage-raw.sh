#!/bin/sh
# Bundles the built programs of one platform into raw/raw-<platform>.tgz, so the packaging jobs
# (which hold the signing secrets) never build anything themselves.
# Usage: stage-raw.sh <linux-x86_64|windows-x86_64|macos-universal>   (repository root, after the build)
# The archive keeps the paths below target/ and the file modes; extract it at the repository root.
set -eu
platform="$1"
bins="atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host"
list="$(mktemp)"
trap 'rm -f "$list"' EXIT
case "$platform" in
  linux-x86_64)
    for b in $bins; do echo "target/release/$b"; done > "$list"
    ;;
  windows-x86_64)
    for b in $bins; do echo "target/release/$b.exe"; done > "$list"
    ;;
  macos-universal)
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
      for b in $bins; do echo "target/$t/release/$b"; done
    done > "$list"
    ;;
  *)
    echo "error: unknown platform: $platform" >&2
    exit 1
    ;;
esac
while IFS= read -r f; do
  test -f "$f" || { echo "error: $f was not built" >&2; exit 1; }
done < "$list"
mkdir -p raw
tar -czf "raw/raw-$platform.tgz" -T "$list"
ls -l raw
