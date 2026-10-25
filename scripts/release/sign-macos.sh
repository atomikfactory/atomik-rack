#!/bin/sh
# Code-signs a macOS app bundle or a disk image.
# Usage: sign-macos.sh app <path to Atomik Rack.app>
#        sign-macos.sh dmg <path to .dmg>
# Environment:
#   MACOS_SIGN_IDENTITY  the Developer ID Application identity. Empty or unset: the app gets the
#                        ad-hoc signature that Apple Silicon needs to run it (not a developer
#                        signature) and a dmg is left unsigned.
#   APPLE_KEYCHAIN_PATH  the keychain that holds the identity (optional).
# With an identity every Mach-O file of the app is signed inside-out (deepest first, the main
# executable and the bundle last) with the hardened runtime, a secure timestamp and the
# entitlements in scripts/release/macos/entitlements.plist (why: see that file). Any failure fails.
set -eu
kind="$1"
target="$2"
identity="${MACOS_SIGN_IDENTITY:-}"
entitlements="$(cd "$(dirname "$0")" && pwd)/macos/entitlements.plist"

keychain_args=""
if [ -n "${APPLE_KEYCHAIN_PATH:-}" ]; then
  keychain_args="--keychain $APPLE_KEYCHAIN_PATH"
fi

case "$kind" in
  app)
    if [ -z "$identity" ]; then
      echo "signing: ad-hoc only (no Developer ID identity configured)"
      codesign --force --deep -s - "$target"
      codesign --verify --deep --strict --verbose=2 "$target"
      exit 0
    fi
    echo "signing: Developer ID, hardened runtime, timestamp"
    main="$target/Contents/MacOS/$(plutil -extract CFBundleExecutable raw "$target/Contents/Info.plist")"
    list="$(mktemp)"
    trap 'rm -f "$list"' EXIT
    # Every Mach-O file, deepest paths first.
    find "$target" -type f | awk -F/ '{ print NF, $0 }' | sort -rn | cut -d' ' -f2- > "$list"
    while IFS= read -r f; do
      [ "$f" = "$main" ] && continue
      if file -b "$f" | grep -q 'Mach-O'; then
        echo "  sign $f"
        # shellcheck disable=SC2086
        codesign --force --options runtime --timestamp --entitlements "$entitlements" \
          --sign "$identity" $keychain_args "$f"
      fi
    done < "$list"
    echo "  sign $target"
    # shellcheck disable=SC2086
    codesign --force --options runtime --timestamp --entitlements "$entitlements" \
      --sign "$identity" $keychain_args "$target"
    codesign --verify --deep --strict --verbose=2 "$target"
    codesign --display --verbose=2 "$target" 2>&1 | grep -E '^(Authority|TeamIdentifier|Timestamp|CodeDirectory)' | sed 's/^/  /'
    ;;
  dmg)
    if [ -z "$identity" ]; then
      echo "signing: the disk image stays unsigned (no Developer ID identity configured)"
      exit 0
    fi
    # shellcheck disable=SC2086
    codesign --force --timestamp --sign "$identity" $keychain_args "$target"
    codesign --verify --strict --verbose=2 "$target"
    ;;
  *)
    echo "usage: sign-macos.sh app|dmg <path>" >&2
    exit 2
    ;;
esac
