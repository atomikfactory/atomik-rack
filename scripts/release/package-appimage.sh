#!/bin/sh
# Packages the Linux build as an AppImage: dist/atomik-rack-<version>-x86_64.AppImage (+ .sha256).
# Usage: package-appimage.sh [--dry-run] <label>   (repository root, after cargo build --release)
#   label      the release label: vX.Y.Z[-pre], nightly-YYYYMMDD-<sha> or dev-<sha>
#   --dry-run  prints the layout of the AppDir and the file it would write, and builds nothing
# Environment: APPIMAGETOOL is the path of the official appimagetool (CI fetches a pinned one with
# fetch-appimagetool.sh); without it the script stops after the AppDir is built.
#
# The AppDir keeps the layout the app expects: the five programs side by side in usr/bin (the app
# looks for atomik-plugin-scan and atomik-plugin-host beside its own executable, which is the
# real file even when started through AppRun), the factory library beside them, and the desktop
# files in usr/share, shared with the other Linux packages. Libraries come from the host except
# the few that appimage-libs.sh decides to bundle.
set -eu
dry=0
if [ "${1:-}" = "--dry-run" ]; then
  dry=1
  shift
fi
label="$1"
bins="atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host"
id="io.github.atomikfactory.AtomikRack"
file_version="${label#v}"
out="dist/atomik-rack-${file_version}-x86_64.AppImage"

if [ "$dry" = "1" ]; then
  # Fail on a bad label the same way a real run would.
  sh scripts/release/version-info.sh "$label" deb > /dev/null
  echo "AppDir (AtomikRack.AppDir):"
  echo "  AppRun"
  echo "  $id.desktop"
  echo "  $id.png"
  echo "  .DirIcon"
  for bin in $bins; do echo "  usr/bin/$bin"; done
  echo "  usr/bin/factory/"
  echo "  usr/lib/            (only libraries that appimage-libs.sh decides to bundle)"
  echo "  usr/share/applications/$id.desktop"
  echo "  usr/share/metainfo/$id.metainfo.xml"
  echo "  usr/share/mime/packages/$id.xml"
  for size in 128 256 512; do echo "  usr/share/icons/hicolor/${size}x${size}/apps/$id.png"; done
  echo "  usr/share/doc/atomik-rack/{LICENSE,README.md,CHANGELOG.md}"
  echo "output: $out"
  echo "checksum: $out.sha256"
  exit 0
fi

for bin in $bins; do
  test -f "target/release/$bin" || { echo "error: target/release/$bin was not built" >&2; exit 1; }
done
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
appdir="$work/AtomikRack.AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/lib" "$appdir/usr/share/doc/atomik-rack"

for bin in $bins; do
  install -m 0755 "target/release/$bin" "$appdir/usr/bin/$bin"
  # The release profile keeps line tables for backtraces (debug = 1): the image does not need them.
  strip --strip-unneeded "$appdir/usr/bin/$bin" 2> /dev/null || true
done
cp -R factory "$appdir/usr/bin/factory"
find "$appdir/usr/bin/factory" -type d -exec chmod 0755 {} +
find "$appdir/usr/bin/factory" -type f -exec chmod 0644 {} +
cp LICENSE README.md CHANGELOG.md "$appdir/usr/share/doc/atomik-rack/"

sh scripts/release/linux-share.sh "$appdir/usr/share" "$label"
cp "packaging/linux/$id.desktop" "$appdir/$id.desktop"
cp "assets/icon/atomik-rack-256.png" "$appdir/$id.png"
ln -s "$id.png" "$appdir/.DirIcon"

# Libraries the host may lack (usually none: see appimage-libs.sh).
# shellcheck disable=SC2046
bundle="$(sh scripts/release/appimage-libs.sh $(for bin in $bins; do echo "$appdir/usr/bin/$bin"; done))"
for lib in $bundle; do
  echo "bundling $lib"
  cp -L "$lib" "$appdir/usr/lib/"
done

cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
# Starts the program from inside the image. The programs find each other and the factory library
# beside their own executable, so nothing else has to be set.
here="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$here/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$here/usr/bin/atomik" "$@"
APPRUN
chmod 0755 "$appdir/AppRun"
find "$appdir" -type d -exec chmod 0755 {} +

echo "AppDir:"
(cd "$appdir" && find . | LC_ALL=C sort | grep -v '/factory/.' | head -n 80)

if [ -z "${APPIMAGETOOL:-}" ]; then
  echo "error: APPIMAGETOOL is not set (see scripts/release/fetch-appimagetool.sh)" >&2
  exit 1
fi
mkdir -p dist
# No FUSE on a CI runner: the tool unpacks itself and runs from there. Metainfo is validated by
# its own CI step (appstreamcli), so the tool's check is off.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$appdir" "$out"
chmod 0755 "$out"
sh scripts/release/checksum.sh "$out"
ls -l dist
