#!/bin/sh
# Packages the Windows build: dist/atomik-rack-<version>-windows-x86_64.zip (+ .sha256).
# Usage: package-zip.sh <version>   (run from the repository root, in Git Bash)
set -eu
version="$1"
name="atomik-rack-$version-windows-x86_64"
rm -rf dist
mkdir -p "dist/$name"
for bin in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
  cp "target/release/$bin.exe" "dist/$name/"
done
cp LICENSE README.md CHANGELOG.md "dist/$name/"
(cd dist && 7z a -tzip "$name.zip" "$name" > /dev/null && rm -rf "$name")
sh scripts/release/checksum.sh "dist/$name.zip"
ls -l dist
