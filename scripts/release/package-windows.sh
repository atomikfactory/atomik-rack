#!/bin/sh
# Builds the Windows installer: dist/atomik-rack-<label>-windows-x86_64-setup.exe (no checksum:
# the installer is signed after this script, so the workflow writes the .sha256 last).
# Usage: package-windows.sh <label>   (Git Bash on Windows, repository root; dist/ is kept)
# Needs the five programs in target/release (already signed when signing is configured) and
# Inno Setup 6.3 or later: iscc is on the GitHub Windows runners; if it is missing it is
# installed with Chocolatey.
set -eu
label="$1"
display="$(sh scripts/release/version-info.sh "$label" display)"
numeric="$(sh scripts/release/version-info.sh "$label" numeric)"
name="atomik-rack-$label-windows-x86_64-setup"

find_iscc() {
  if command -v iscc > /dev/null 2>&1; then
    command -v iscc
    return 0
  fi
  for dir in "/c/Program Files (x86)" "/c/Program Files"; do
    for exe in "$dir/Inno Setup 6/ISCC.exe" "$dir/Inno Setup 7/ISCC.exe"; do
      if [ -f "$exe" ]; then
        printf '%s\n' "$exe"
        return 0
      fi
    done
  done
  return 1
}

iscc="$(find_iscc || true)"
if [ -z "$iscc" ]; then
  echo "Inno Setup not found; installing it with Chocolatey"
  choco install innosetup -y --no-progress
  iscc="$(find_iscc || true)"
fi
if [ -z "$iscc" ]; then
  echo "error: Inno Setup (iscc) is not available" >&2
  exit 1
fi
echo "iscc: $iscc"

for bin in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
  test -f "target/release/$bin.exe" || { echo "error: target/release/$bin.exe is missing" >&2; exit 1; }
done
mkdir -p dist
# Git Bash would rewrite the /D... and /O... switches as if they were paths: switch that off for iscc.
MSYS2_ARG_CONV_EXCL='*' "$iscc" /Qp \
  "/DAppVersion=$display" \
  "/DAppVersionNumeric=$numeric" \
  "/DSourceDir=$(cygpath -w "$PWD/target/release")" \
  "/DRepoDir=$(cygpath -w "$PWD")" \
  "/DOutputDir=$(cygpath -w "$PWD/dist")" \
  "/DOutputName=$name" \
  "$(cygpath -w "$PWD/scripts/release/windows/atomik-rack.iss")"
test -f "dist/$name.exe" || { echo "error: dist/$name.exe was not written" >&2; exit 1; }
ls -l dist
