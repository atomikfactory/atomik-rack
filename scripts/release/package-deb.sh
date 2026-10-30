#!/bin/sh
# Packages the Linux build as a Debian package: dist/atomik-rack_<version>_amd64.deb (+ .sha256).
# Usage: package-deb.sh <label>   (repository root, after cargo build --release; dist/ is kept)
#   label  the release label: vX.Y.Z[-pre], nightly-YYYYMMDD-<sha> or dev-<sha>
# The file name uses the label without its "v" (a "~" is not kept in file names: GitHub rewrites
# it); the package inside carries the Debian version from version-info.sh (0.1.0-rc.1 becomes
# 0.1.0~rc.1, so a release candidate sorts before the release).
#
# Layout: the five programs in /opt/atomik-rack, with /usr/bin/<program> symlinks to them. The app
# looks for atomik-plugin-scan and atomik-plugin-host beside its own executable, resolved with
# std::env::current_exe(), which on Linux is the target of /proc/self/exe: a symlink is followed,
# so the siblings are found in /opt/atomik-rack however the program was started.
set -eu
label="$1"
bins="atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host"
debver="$(sh scripts/release/version-info.sh "$label" deb)"
file_version="${label#v}"
out="dist/atomik-rack_${file_version}_amd64.deb"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

opt="$root/opt/atomik-rack"
mkdir -p "$opt" "$root/usr/bin" "$root/usr/share/applications" "$root/usr/share/mime/packages" \
  "$root/usr/share/doc/atomik-rack" "$root/DEBIAN"
for size in 128 256 512; do
  mkdir -p "$root/usr/share/icons/hicolor/${size}x${size}/apps"
  install -m 0644 "assets/icon/atomik-rack-$size.png" \
    "$root/usr/share/icons/hicolor/${size}x${size}/apps/atomik-rack.png"
done
for bin in $bins; do
  install -m 0755 "target/release/$bin" "$opt/$bin"
  # The release profile keeps line tables for backtraces (debug = 1): the package does not need them.
  strip --strip-unneeded "$opt/$bin" 2> /dev/null || true
  ln -s "../../opt/atomik-rack/$bin" "$root/usr/bin/$bin"
done

cat > "$root/usr/share/applications/atomik-rack.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Atomik Rack
Comment=A rack of devices you patch and play
Icon=atomik-rack
Exec=atomik %f
Terminal=false
Categories=AudioVideo;Audio;Midi;Music;
MimeType=application/x-atomik-project;
DESKTOP

cat > "$root/usr/share/mime/packages/atomik-rack.xml" <<'MIME'
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-atomik-project">
    <comment>Atomik Rack project</comment>
    <glob pattern="*.atomik"/>
  </mime-type>
</mime-info>
MIME

cat > "$root/usr/share/doc/atomik-rack/copyright" <<'COPYRIGHT'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Atomik Rack
Source: https://github.com/atomikfactory/atomik-rack

Files: *
Copyright: Atomik Rack contributors
License: GPL-3+

License: GPL-3+
 This program is free software: you can redistribute it and/or modify it under
 the terms of the GNU General Public License as published by the Free Software
 Foundation, either version 3 of the License, or (at your option) any later
 version.
 .
 On Debian systems, the complete text of the GNU General Public License
 version 3 can be found in /usr/share/common-licenses/GPL-3. The same text is
 in the LICENSE file of the source repository.
COPYRIGHT
gzip -9n -c CHANGELOG.md > "$root/usr/share/doc/atomik-rack/changelog.gz"
gzip -9n -c README.md > "$root/usr/share/doc/atomik-rack/README.md.gz"

depends="$(sh scripts/release/deb-depends.sh "$opt"/*)"
echo "Depends: $depends"
# Installed-Size is in KiB; the control file is not part of it.
size="$(du -sk "$root" | cut -f1)"
cat > "$root/DEBIAN/control" <<CONTROL
Package: atomik-rack
Version: $debver
Section: sound
Priority: optional
Architecture: amd64
Maintainer: Atomik Factory <28955350+atomikfactory@users.noreply.github.com>
Depends: $depends
Installed-Size: $size
Homepage: https://github.com/atomikfactory/atomik-rack
Description: Atomik Rack, a rack of devices you patch and play
 A modular digital audio workstation with a rack of devices, a sequencer,
 a mixer and CLAP and VST3 plugin hosting. Also installs atomik-render
 (offline bounce), atomik-play, atomik-plugin-scan and atomik-plugin-host
 (the plugin sandbox process).
CONTROL

find "$root" -type d -exec chmod 0755 {} +
find "$root/usr/share" -type f -exec chmod 0644 {} +
mkdir -p dist
dpkg-deb --root-owner-group --build "$root" "$out"
dpkg-deb --info "$out"
sh scripts/release/checksum.sh "$out"
ls -l dist
