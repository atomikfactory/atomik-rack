#!/bin/sh
# Tests scripts/release/test-report.sh against the real protocol and throw-away ones.
# Usage (from the repository root): sh scripts/release/test-report-test.sh
set -eu
here="$(dirname "$0")"
script="$here/test-report.sh"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT

fails() {
  # fails "what" command...: the command must exit non-zero.
  what="$1"
  shift
  if "$@" > /dev/null 2>&1; then
    echo "FAIL: $what" >&2
    exit 1
  fi
}

# The real protocol is valid and makes a complete skeleton.
sh "$script" --check > /dev/null
sh "$script" 9.9.9-rc.1 --out "$dir" > /dev/null
report="$dir/release-test-report-v9.9.9-rc.1.md"
[ -f "$report" ] || { echo "FAIL: no report written" >&2; exit 1; }
ids="$(grep -c '^### [A-Z]*-[A-Z]*-[0-9]* ' docs/RELEASE-TESTING.md)"
rows="$(grep -c '^| [A-Z]*-[A-Z]*-[0-9]* |' "$report")"
[ "$ids" = "$rows" ] || { echo "FAIL: $rows rows for $ids ids" >&2; exit 1; }
grep -q '^| Version | 9.9.9-rc.1 |' "$report" || { echo "FAIL: no version" >&2; exit 1; }
grep -q '^| Commit | ' "$report" || { echo "FAIL: no commit" >&2; exit 1; }
# A leading v is accepted and gives the same file name.
sh "$script" v9.9.9-rc.1 --out "$dir" > /dev/null

# A fresh report is not complete; one with every result "pass" is.
fails "a pending report must not verify" sh "$script" --verify "$report" --os windows
sed 's/| pending |/| pass |/' "$report" > "$dir/pass.md"
sh "$script" --verify "$dir/pass.md" --os all > /dev/null
sh "$script" --verify "$dir/pass.md" --os linux > /dev/null

# A failed item blocks; an n/a needs its note.
sed '0,/| pass |/s//| fail |/' "$dir/pass.md" > "$dir/fail.md"
fails "a failed item must not verify" sh "$script" --verify "$dir/fail.md" --os all
sed '0,/| pass |/s//| n\/a |/' "$dir/pass.md" > "$dir/na.md"
fails "an n/a without a note must not verify" sh "$script" --verify "$dir/na.md" --os all
sed '0,/| pass | |/s//| n\/a | no such hardware |/' "$dir/pass.md" > "$dir/na-note.md"
sh "$script" --verify "$dir/na-note.md" --os all > /dev/null

# A row that is missing from the report is found.
grep -v '^| HW-AUD-01 |' "$dir/pass.md" > "$dir/missing.md"
fails "a missing id must not verify" sh "$script" --verify "$dir/missing.md" --os all

# Only the items of the chosen OS count: leave a Linux-only item pending.
sed 's/^\(| HW-PLG-09 .*\)| pass |/\1| pending |/' "$dir/pass.md" > "$dir/linux-pending.md"
fails "a pending Linux item blocks the Linux part" sh "$script" --verify "$dir/linux-pending.md" --os linux
sh "$script" --verify "$dir/linux-pending.md" --os windows > /dev/null

# A protocol with a repeated id, a bad id or a missing field is refused.
bad() {
  printf '%s\n' "$@" > "$dir/protocol.md"
  ATOMIK_PROTOCOL="$dir/protocol.md" sh "$script" --check
}
good_item='### HW-AUD-01 One
- **Applies to:** Windows
- **Automated:** none'
fails "a duplicate id must fail" bad "$good_item" "$good_item"
fails "a malformed id must fail" bad '### hw-aud-1 One' '- **Applies to:** Windows' '- **Automated:** none'
fails "a missing Automated field must fail" bad '### HW-AUD-01 One' '- **Applies to:** Windows'
fails "a missing Applies to field must fail" bad '### HW-AUD-01 One' '- **Automated:** none'
fails "an empty protocol must fail" bad '# nothing'
bad "$good_item" > /dev/null
fails "a bad version must fail" sh "$script" one-two --out "$dir"
echo "release test report: ok"
