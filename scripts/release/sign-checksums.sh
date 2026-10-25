#!/bin/sh
# Detached, ASCII-armoured GPG signature of SHA256SUMS: writes SHA256SUMS.asc next to it.
# Usage: sign-checksums.sh <path to SHA256SUMS>
# Needs GPG_PRIVATE_KEY_BASE64 (the base64 of an exported secret key) and GPG_PASSPHRASE in the
# environment; without both it prints a notice and signs nothing (exit 0). With both, any failure
# fails the script. The key lives in a throw-away GNUPGHOME that is deleted on exit, and no secret
# is ever printed.
set -eu
sums="$1"
if [ -z "${GPG_PRIVATE_KEY_BASE64:-}" ] || [ -z "${GPG_PASSPHRASE:-}" ]; then
  echo "::notice title=SHA256SUMS not signed::GPG_PRIVATE_KEY_BASE64 and GPG_PASSPHRASE are not set; publishing SHA256SUMS without a signature."
  exit 0
fi
GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
chmod 700 "$GNUPGHOME"
cleanup() {
  gpgconf --kill gpg-agent > /dev/null 2>&1 || true
  rm -rf "$GNUPGHOME"
}
trap cleanup EXIT

printf '%s' "$GPG_PRIVATE_KEY_BASE64" | base64 -d > "$GNUPGHOME/key.asc"
gpg --batch --quiet --import "$GNUPGHOME/key.asc"
rm -f "$GNUPGHOME/key.asc"
fingerprint="$(gpg --batch --list-secret-keys --with-colons | awk -F: '/^fpr:/{print $10; exit}')"
if [ -z "$fingerprint" ]; then
  echo "error: no secret key was imported from GPG_PRIVATE_KEY_BASE64" >&2
  exit 1
fi
rm -f "$sums.asc"
printf '%s' "$GPG_PASSPHRASE" | gpg --batch --yes --pinentry-mode loopback --passphrase-fd 0 \
  --local-user "$fingerprint" --armor --detach-sign --output "$sums.asc" "$sums"
gpg --batch --verify "$sums.asc" "$sums"
echo "SHA256SUMS signed with key $fingerprint"
