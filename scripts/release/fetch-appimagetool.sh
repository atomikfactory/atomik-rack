#!/bin/sh
# Downloads the official appimagetool at a pinned version and checks its SHA-256 before it is
# made executable. CI only; nothing else (and no test) downloads anything.
# Usage: fetch-appimagetool.sh <dest-file>
# To move to a new version: change both values below together, from the checksum shown on the
# release page, and say so in docs/DEPENDENCIES.md.
set -eu
version="1.9.1"
sha256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
url="https://github.com/AppImage/appimagetool/releases/download/$version/appimagetool-x86_64.AppImage"
dest="$1"
mkdir -p "$(dirname "$dest")"
curl --fail --location --silent --show-error --retry 3 --output "$dest" "$url"
echo "$sha256  $dest" | sha256sum -c -
chmod +x "$dest"
echo "appimagetool $version at $dest"
