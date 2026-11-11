#!/bin/sh
# Builds the Flatpak bundle: dist/atomik-rack-<version>.flatpak (+ .sha256), from the release tarball.
# Usage: package-flatpak.sh [--dry-run] <label>   (repository root; dist/ holds the tarball that
#        package-tar.sh wrote, dist/atomik-rack-<label>-linux-x86_64.tar.gz)
#   --dry-run  prints the staged files and the commands, and builds nothing
# Needs flatpak and flatpak-builder (apt-get install flatpak flatpak-builder). The runtime and the
# SDK come from Flathub (flatpak-builder installs them for the user).
# The tarball is the source of the programs (docs/DECISIONS.md, entry U): nothing is compiled here.
set -eu
dry=0
if [ "${1:-}" = "--dry-run" ]; then
  dry=1
  shift
fi
label="$1"
id="io.github.atomikfactory.AtomikRack"
file_version="${label#v}"
# package-tar.sh names the archive after the label itself (the tag keeps its "v").
tarball="dist/atomik-rack-${label}-linux-x86_64.tar.gz"
out="dist/atomik-rack-${file_version}.flatpak"
src="packaging/linux"

if [ "$dry" = "1" ]; then
  sh scripts/release/version-info.sh "$label" deb > /dev/null
  echo "staged in a temporary folder:"
  echo "  $id.yml           (from $src/$id.yml, @TARBALL_SHA256@ filled in)"
  echo "  atomik-rack-linux-x86_64.tar.gz  (from $tarball)"
  echo "  $id.desktop, $id.mime.xml"
  echo "  $id.metainfo.xml  (releases list from the label)"
  echo "  icon-128.png, icon-256.png, icon-512.png"
  echo "flatpak-builder --user --install-deps-from=flathub --force-clean --repo=<repo> <build> $id.yml"
  echo "flatpak build-bundle <repo> $out $id --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo"
  echo "output: $out"
  echo "checksum: $out.sha256"
  exit 0
fi

test -f "$tarball" || { echo "error: $tarball is missing (run package-tar.sh first)" >&2; exit 1; }
for tool in flatpak flatpak-builder; do
  command -v "$tool" > /dev/null 2>&1 || { echo "error: $tool is needed (apt-get install flatpak flatpak-builder)" >&2; exit 1; }
done
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage"
cp "$tarball" "$stage/atomik-rack-linux-x86_64.tar.gz"
sum="$(sha256sum "$stage/atomik-rack-linux-x86_64.tar.gz" | cut -d' ' -f1)"
sed "s/@TARBALL_SHA256@/$sum/" "$src/$id.yml" > "$stage/$id.yml"
if grep -q "@TARBALL_SHA256@" "$stage/$id.yml"; then
  echo "error: the checksum was not filled in" >&2
  exit 1
fi
cp "$src/$id.desktop" "$src/$id.mime.xml" "$stage/"
sh scripts/release/linux-metainfo.sh "$label" "$stage/$id.metainfo.xml"
for size in 128 256 512; do cp "assets/icon/atomik-rack-$size.png" "$stage/icon-$size.png"; done

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
(cd "$stage" && flatpak-builder --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
  --repo="$work/repo" "$work/build" "$id.yml")
mkdir -p dist
flatpak build-bundle "$work/repo" "$out" "$id" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
sh scripts/release/checksum.sh "$out"
ls -l dist
