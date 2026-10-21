#!/bin/sh
# Packages the macOS build: dist/atomik-rack-<version>-macos-universal.zip (+ .sha256).
# Needs target/aarch64-apple-darwin/release and target/x86_64-apple-darwin/release.
# Usage: package-macos.sh <version>   (run from the repository root on macOS)
set -eu
version="$1"
name="atomik-rack-$version-macos-universal"
bins="atomik atomik-render atomik-play atomik-plugin-scan"
arm="target/aarch64-apple-darwin/release"
x64="target/x86_64-apple-darwin/release"

# Bundle version: the numeric part of [workspace.package] version (CFBundle versions have no suffix).
cargo_version="$(awk '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p&&/^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/,"");print;exit}' Cargo.toml)"
bundle_version="${cargo_version%%-*}"

rm -rf dist
mkdir -p "dist/$name"
app="dist/$name/Atomik Rack.app"
macos="$app/Contents/MacOS"
mkdir -p "$macos"
for bin in $bins; do
  lipo -create "$arm/$bin" "$x64/$bin" -output "$macos/$bin"
  chmod +x "$macos/$bin"
done

# Lowest macOS the universal binaries accept: the higher minos of the two slices.
min_os="$(for t in "$arm" "$x64"; do
  otool -l "$t/atomik" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}'
done | sort -t. -k1,1n -k2,2n | tail -n 1)"
if [ -z "$min_os" ]; then
  echo "error: could not read minos from the binaries" >&2
  exit 1
fi
echo "LSMinimumSystemVersion: $min_os"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>atomik</string>
  <key>CFBundleIdentifier</key><string>com.atomikfactory.atomik-rack</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Atomik Rack</string>
  <key>CFBundleDisplayName</key><string>Atomik Rack</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$bundle_version</string>
  <key>CFBundleVersion</key><string>$bundle_version</string>
  <key>LSMinimumSystemVersion</key><string>$min_os</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Atomik Rack records audio from your input device.</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

# Ad-hoc signature: Apple Silicon refuses unsigned code. This is not a developer signature.
codesign --force --deep -s - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

cp LICENSE README.md CHANGELOG.md "dist/$name/"
(cd dist && ditto -c -k --keepParent "$name" "$name.zip" && rm -rf "$name")
sh scripts/release/checksum.sh "dist/$name.zip"
ls -l dist
