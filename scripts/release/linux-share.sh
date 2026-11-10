#!/bin/sh
# Installs the desktop integration files every Linux package shares into a share folder: the
# desktop entry, the file type, the AppStream metainfo and the icons, all named after the app id.
# Usage: linux-share.sh <share-dir> <label>   (repository root)
#   share-dir  the usr/share (or /app/share) folder of the package being built
#   label      the release label, for the releases list of the metainfo (linux-metainfo.sh)
set -eu
share="$1"
label="$2"
id="io.github.atomikfactory.AtomikRack"
src="packaging/linux"
mkdir -p "$share/applications" "$share/mime/packages" "$share/metainfo"
install -m 0644 "$src/$id.desktop" "$share/applications/$id.desktop"
install -m 0644 "$src/$id.mime.xml" "$share/mime/packages/$id.xml"
sh scripts/release/linux-metainfo.sh "$label" "$share/metainfo/$id.metainfo.xml"
chmod 0644 "$share/metainfo/$id.metainfo.xml"
for size in 128 256 512; do
  mkdir -p "$share/icons/hicolor/${size}x${size}/apps"
  install -m 0644 "assets/icon/atomik-rack-$size.png" "$share/icons/hicolor/${size}x${size}/apps/$id.png"
done
