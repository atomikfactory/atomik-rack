#!/bin/sh
# Packages the Linux build: dist/atomik-rack-<version>-linux-x86_64.tar.gz (+ .sha256).
# Usage: package-tar.sh <version>   (run from the repository root after cargo build --release)
set -eu
version="$1"
name="atomik-rack-$version-linux-x86_64"
rm -rf dist
mkdir -p "dist/$name"
for bin in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
  cp "target/release/$bin" "dist/$name/"
done
cp LICENSE README.md CHANGELOG.md "dist/$name/"
(cd dist && tar -czf "$name.tar.gz" "$name" && rm -rf "$name")
sh scripts/release/checksum.sh "dist/$name.tar.gz"
ls -l dist
