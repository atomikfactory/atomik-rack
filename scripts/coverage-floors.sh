#!/bin/sh
# Per-crate line coverage floors.
#
#   sh scripts/coverage-floors.sh <lcov.info> [floors-file]
#
# Sums the line counts (LF, LH) of every source file of `crates/<name>/` in an lcov report, prints
# a Markdown table (and appends it to the job summary when GITHUB_STEP_SUMMARY is set) and exits
# 1 when a crate is below its floor. The floors file (default scripts/coverage-floors.txt) has
# one `<crate> <percent>` per line; `default <percent>` covers a crate that has no line of its
# own; `#` starts a comment. Raise a floor when a crate's coverage has risen and stays there;
# never lower one to make a run pass without saying why in the commit.
#
#   sh scripts/coverage-floors.sh --self-test
#
# checks the script itself on a small synthetic report (passes, then fails a crate on purpose).
set -eu

here="$(cd "$(dirname "$0")" && pwd)"

check() {
  awk -v floors="$2" '
    function floor_of(name) {
      if (name in floor) return floor[name]
      return floor["default"]
    }
    BEGIN {
      while ((getline line < floors) > 0) {
        sub(/#.*/, "", line)
        n = split(line, f, " ")
        if (n >= 2) floor[f[1]] = f[2] + 0
      }
      if (!("default" in floor)) floor["default"] = 0
    }
    /^SF:/ {
      path = substr($0, 4)
      gsub(/\\/, "/", path)
      name = ""
      if (match(path, /crates\/[^\/]+\//)) {
        name = substr(path, RSTART + 7, RLENGTH - 8)
      }
    }
    /^LF:/ { if (name != "") found[name] += substr($0, 4) }
    /^LH:/ { if (name != "") hit[name] += substr($0, 4) }
    END {
      print "| crate | lines | covered | floor | status |"
      print "|---|---:|---:|---:|---|"
      bad = 0
      for (c in found) names[++count] = c
      # Sorted for a stable table.
      for (i = 2; i <= count; i++) {
        v = names[i]; j = i - 1
        while (j >= 1 && names[j] > v) { names[j + 1] = names[j]; j-- }
        names[j + 1] = v
      }
      for (i = 1; i <= count; i++) {
        c = names[i]
        pct = found[c] > 0 ? 100.0 * hit[c] / found[c] : 100.0
        fl = floor_of(c)
        status = "ok"
        if (pct + 1e-9 < fl) { status = "BELOW FLOOR"; bad++ }
        printf "| %s | %d | %.2f%% | %.0f%% | %s |\n", c, found[c], pct, fl, status
      }
      for (c in floor) {
        if (c != "default" && !(c in found)) {
          printf "| %s | 0 | - | %.0f%% | no data (excluded or renamed) |\n", c, floor[c]
        }
      }
      if (bad > 0) {
        printf "\n%d crate(s) below their coverage floor.\n", bad
        exit 1
      }
    }
  ' "$1"
}

if [ "${1:-}" = "--self-test" ]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  cat > "$tmp/lcov.info" << 'EOF'
SF:/work/atomik-rack/crates/alpha/src/a.rs
LF:10
LH:9
end_of_record
SF:/work/atomik-rack/crates/alpha/src/b.rs
LF:10
LH:9
end_of_record
SF:/work/atomik-rack/crates/beta/src/lib.rs
LF:4
LH:2
end_of_record
EOF
  printf 'default 80\nalpha 90\nbeta 50\n' > "$tmp/floors.txt"
  check "$tmp/lcov.info" "$tmp/floors.txt" > /dev/null || { echo "self-test: a passing report failed" >&2; exit 1; }
  printf 'default 80\nalpha 90\nbeta 51\n' > "$tmp/floors.txt"
  if check "$tmp/lcov.info" "$tmp/floors.txt" > /dev/null; then
    echo "self-test: a crate below its floor passed" >&2
    exit 1
  fi
  echo "coverage-floors self-test: ok"
  exit 0
fi

report="${1:?usage: coverage-floors.sh <lcov.info> [floors-file]}"
floors="${2:-$here/coverage-floors.txt}"
status=0
table="$(check "$report" "$floors")" || status=$?
printf '%s\n' "$table"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Coverage by crate"
    echo
    printf '%s\n' "$table"
  } >> "$GITHUB_STEP_SUMMARY"
fi
exit "$status"
