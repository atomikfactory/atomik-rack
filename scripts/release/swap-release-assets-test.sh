#!/bin/sh
# Checks that swap-release-assets.sh uploads before it deletes and keeps the new names, with a fake
# `gh` that logs its calls. Usage: sh scripts/release/swap-release-assets-test.sh
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
mkdir "$dir/bin"
cat > "$dir/bin/gh" <<'FAKE'
#!/bin/sh
echo "gh $*" >> "$FAKE_LOG"
case "$1 $2" in
  "release view") printf '%s\n' atomik-rack-nightly-20261101-aaaaaaa-linux.tar.gz SHA256SUMS ;;
esac
FAKE
chmod +x "$dir/bin/gh"
: > "$dir/SHA256SUMS"
: > "$dir/atomik-rack-nightly-20261102-bbbbbbb-linux.tar.gz"
export FAKE_LOG="$dir/log"
: > "$FAKE_LOG"
PATH="$dir/bin:$PATH" sh "$here/swap-release-assets.sh" nightly owner/repo \
  "$dir/atomik-rack-nightly-20261102-bbbbbbb-linux.tar.gz" "$dir/SHA256SUMS"
upload="$(grep -n 'release upload' "$FAKE_LOG" | head -1 | cut -d: -f1)"
delete="$(grep -n 'delete-asset' "$FAKE_LOG" | head -1 | cut -d: -f1)"
[ -n "$upload" ] && [ -n "$delete" ] && [ "$upload" -lt "$delete" ] || { echo "FAIL: upload must come before delete" >&2; cat "$FAKE_LOG" >&2; exit 1; }
[ "$(grep -c 'delete-asset' "$FAKE_LOG")" = 1 ] || { echo "FAIL: only the stale asset is deleted" >&2; cat "$FAKE_LOG" >&2; exit 1; }
grep -q 'delete-asset nightly atomik-rack-nightly-20261101-aaaaaaa-linux.tar.gz' "$FAKE_LOG" || { echo "FAIL: wrong asset deleted" >&2; exit 1; }
# A dry run changes nothing.
: > "$FAKE_LOG"
DRY_RUN=1 PATH="$dir/bin:$PATH" sh "$here/swap-release-assets.sh" nightly owner/repo "$dir/SHA256SUMS" > /dev/null
if grep -q 'upload\|delete-asset' "$FAKE_LOG"; then echo "FAIL: a dry run must not change the release" >&2; exit 1; fi
echo "swap-release-assets: ok"
