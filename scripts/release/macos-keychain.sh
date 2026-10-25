#!/bin/sh
# A throw-away keychain that holds the Developer ID certificate for one job (macOS runner).
# Usage: macos-keychain.sh create   imports the certificate and appends APPLE_KEYCHAIN_PATH to $GITHUB_ENV
#        macos-keychain.sh delete   deletes the keychain (run it in a step with `if: always()`)
# Environment for `create`: APPLE_CERTIFICATE_P12_BASE64, APPLE_CERTIFICATE_PASSWORD,
# APPLE_SIGNING_IDENTITY. The keychain password is random and never leaves this script; no secret
# is printed. `delete` needs only APPLE_KEYCHAIN_PATH and never fails.
set -eu
tmp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
keychain="$tmp/atomik-signing.keychain-db"
case "${1:-}" in
  create)
    for v in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY; do
      eval "value=\${$v:-}"
      [ -n "$value" ] || { echo "error: $v is not set" >&2; exit 1; }
    done
    pw="$(openssl rand -base64 24)"
    cert="$tmp/atomik-signing-cert.p12"
    trap 'rm -f "$cert"' EXIT
    printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$cert"
    security create-keychain -p "$pw" "$keychain"
    security set-keychain-settings -lut 21600 "$keychain"
    security unlock-keychain -p "$pw" "$keychain"
    security import "$cert" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -f pkcs12 \
      -T /usr/bin/codesign -T /usr/bin/security
    security set-key-partition-list -S apple-tool:,apple: -s -k "$pw" "$keychain" > /dev/null
    # Put it first in the user's search list, keeping the others.
    existing="$(security list-keychains -d user | sed 's/^[[:space:]]*"//; s/"$//')"
    # shellcheck disable=SC2086
    security list-keychains -d user -s "$keychain" $existing
    if ! security find-identity -v -p codesigning "$keychain" | grep -F -q "$APPLE_SIGNING_IDENTITY"; then
      echo "error: no valid code-signing identity matching APPLE_SIGNING_IDENTITY in the certificate" >&2
      security find-identity -v -p codesigning "$keychain" | sed 's/^/  /' >&2
      exit 1
    fi
    echo "APPLE_KEYCHAIN_PATH=$keychain" >> "$GITHUB_ENV"
    echo "keychain ready: $keychain"
    ;;
  delete)
    if [ -n "${APPLE_KEYCHAIN_PATH:-}" ]; then keychain="$APPLE_KEYCHAIN_PATH"; fi
    security delete-keychain "$keychain" > /dev/null 2>&1 || true
    rm -f "$tmp/atomik-signing-cert.p12"
    echo "keychain removed"
    ;;
  *)
    echo "usage: macos-keychain.sh create|delete" >&2
    exit 2
    ;;
esac
