#!/bin/sh
# Notarises a signed app or disk image with Apple's notary service and staples the ticket.
# Usage: notarize-macos.sh <path to .app or .dmg>
# Environment: APPLE_NOTARY_KEY_BASE64 (the App Store Connect API key, a .p8, base64),
# APPLE_NOTARY_KEY_ID, APPLE_NOTARY_ISSUER_ID. All three are required: this script is only called
# when they are set (the workflow decides), and it fails on anything that goes wrong - a rejected
# submission prints Apple's log first. The key is written to a private temporary file that is
# removed on exit; no secret is printed.
set -eu
target="$1"
for v in APPLE_NOTARY_KEY_BASE64 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID; do
  eval "value=\${$v:-}"
  [ -n "$value" ] || { echo "error: $v is not set" >&2; exit 1; }
done
umask 077
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
key="$work/AuthKey.p8"
printf '%s' "$APPLE_NOTARY_KEY_BASE64" | base64 --decode > "$key"

submit="$target"
case "$target" in
  *.app)
    # The service takes a zip, a dmg or a pkg, not a bare bundle.
    submit="$work/app.zip"
    ditto -c -k --keepParent "$target" "$submit"
    ;;
esac

echo "notarising $(basename "$target") (this waits for Apple, usually a few minutes)"
result="$(xcrun notarytool submit "$submit" --key "$key" --key-id "$APPLE_NOTARY_KEY_ID" \
  --issuer "$APPLE_NOTARY_ISSUER_ID" --wait --timeout 30m --output-format json)" || {
  printf '%s\n' "$result"
  echo "error: notarytool submit failed" >&2
  exit 1
}
status="$(printf '%s' "$result" | sed -n 's/.*"status" *: *"\([^"]*\)".*/\1/p' | head -n 1)"
id="$(printf '%s' "$result" | sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' | head -n 1)"
echo "notarisation $id: $status"
if [ "$status" != "Accepted" ]; then
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" --key "$key" --key-id "$APPLE_NOTARY_KEY_ID" \
      --issuer "$APPLE_NOTARY_ISSUER_ID" || true
  fi
  echo "error: the notary service did not accept $(basename "$target")" >&2
  exit 1
fi
xcrun stapler staple "$target"
xcrun stapler validate "$target"
