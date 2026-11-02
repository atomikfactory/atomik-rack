#!/bin/sh
# Tests scripts/check-advisory-ignores.sh against throw-away files. Usage: sh scripts/check-advisory-ignores-test.sh
set -eu
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
here="$(dirname "$0")"
good="$dir/good.toml"
printf '%s\n' 'ignore = [' '  { id = "RUSTSEC-2099-0001", reason = "x; review-by 2030-01-01" },' ']' > "$good"
bare="$dir/bare.toml"
printf '%s\n' 'ignore = [' '  { id = "RUSTSEC-2099-0002", reason = "no date" },' ']' > "$bare"
TODAY=2029-12-31 sh "$here/check-advisory-ignores.sh" "$good" > /dev/null
if TODAY=2030-01-02 sh "$here/check-advisory-ignores.sh" "$good" > /dev/null 2>&1; then
  echo "FAIL: an ignore past its review date must fail" >&2
  exit 1
fi
if TODAY=2029-12-31 sh "$here/check-advisory-ignores.sh" "$bare" > /dev/null 2>&1; then
  echo "FAIL: an ignore without a review date must fail" >&2
  exit 1
fi
# The real file passes today.
sh "$here/check-advisory-ignores.sh" "$here/../deny.toml" > /dev/null
echo "advisory ignore check: ok"
