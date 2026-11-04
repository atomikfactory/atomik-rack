#!/bin/sh
# The manual release test report (docs/RELEASE-TESTING.md).
#
#   sh scripts/release/test-report.sh <version> [--out DIR]
#       Writes DIR/release-test-report-v<version>.md (DIR defaults to the current directory): one row
#       per protocol id, the environment captured, every result "pending".
#   sh scripts/release/test-report.sh --check
#       Validates the protocol: ids are unique and well formed, and each item has an
#       "Applies to" and an "Automated" field. Exits 1 with the problem otherwise.
#   sh scripts/release/test-report.sh --verify REPORT --os windows|macos|linux|all
#       Fails while an item that applies to the OS is "pending" or "fail", when an "n/a" has no note,
#       or when an id of the protocol is missing from the report.
#
# ATOMIK_PROTOCOL overrides the protocol file (the test of this script uses it). Run from the
# repository root. POSIX sh and awk only.
set -eu

protocol="${ATOMIK_PROTOCOL:-docs/RELEASE-TESTING.md}"

die() {
  echo "error: $*" >&2
  exit 1
}

# One line per item: id|title|applies to|automated. Fails (exit 1, message on stderr) on a
# duplicated or malformed id and on a missing field.
items() {
  [ -f "$protocol" ] || die "no protocol file at $protocol"
  awk '
    function flush() {
      if (id == "") return
      if (applies == "") { print "error: " id " has no Applies to field" > "/dev/stderr"; bad = 1 }
      if (auto == "") { print "error: " id " has no Automated field" > "/dev/stderr"; bad = 1 }
      print id "|" title "|" applies "|" auto
    }
    /^### / {
      flush()
      id = $2; title = $0; sub(/^### [^ ]+ /, "", title); applies = ""; auto = ""
      if (id !~ /^[A-Z][A-Z][A-Z]*-[A-Z][A-Z][A-Z]*-[0-9][0-9]$/) {
        print "error: malformed id " id > "/dev/stderr"; bad = 1
      }
      if (id in seen) { print "error: duplicate id " id > "/dev/stderr"; bad = 1 }
      seen[id] = 1
      count++
      next
    }
    /^- \*\*Applies to:\*\*/ { applies = $0; sub(/^- \*\*Applies to:\*\* */, "", applies); next }
    /^- \*\*Automated:\*\*/ { auto = $0; sub(/^- \*\*Automated:\*\* */, "", auto); gsub(/`/, "", auto); next }
    END {
      flush()
      if (count == 0) { print "error: the protocol has no items" > "/dev/stderr"; bad = 1 }
      exit bad
    }
  ' "$protocol"
}

capture() {
  case "$(uname -s)" in
    Linux)
      cpu="$(sed -n 's/^model name[^:]*: *//p' /proc/cpuinfo 2>/dev/null | head -n 1)"
      ;;
    Darwin)
      cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
      ;;
    *)
      cpu="${PROCESSOR_IDENTIFIER:-}"
      ;;
  esac
  [ -n "${cpu:-}" ] || cpu="unknown"
  os_name="$(uname -sr)"
  arch="$(uname -m)"
  commit="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
  date_utc="$(date -u +%Y-%m-%dT%H:%MZ)"
}

write_report() {
  version="$1"
  out="$2"
  capture
  list="$(items)" || exit 1
  {
    echo "# Release test report v$version"
    echo
    echo "## Environment"
    echo
    echo "| Field | Value |"
    echo "|---|---|"
    echo "| Version | $version |"
    echo "| Commit | $commit |"
    echo "| Date (UTC) | $date_utc |"
    echo "| Operating system | $os_name ($arch) |"
    echo "| Processor | $cpu |"
    echo "| Audio device and driver | (fill in) |"
    echo "| Plugin versions | (fill in) |"
    echo "| Tester | (fill in) |"
    echo
    echo "## Results"
    echo
    echo "Mark each item with \`pass\`, \`fail\` or \`n/a\` (an \`n/a\` needs a note). Steps and expected results"
    echo "are in docs/RELEASE-TESTING.md under the same id."
    echo
    echo "| ID | Item | Applies to | Automated | Result | Note |"
    echo "|---|---|---|---|---|---|"
    echo "$list" | while IFS='|' read -r id title applies auto; do
      echo "| $id | $title | $applies | $auto | pending | |"
    done
  } > "$out"
}

verify() {
  report="$1"
  os="$2"
  [ -f "$report" ] || die "no report at $report"
  list="$(items)" || exit 1
  problems=0
  for id in $(echo "$list" | cut -d'|' -f1); do
    if ! grep -q "^| $id |" "$report"; then
      echo "error: $id is missing from the report" >&2
      problems=$((problems + 1))
    fi
  done
  rows="$(awk -F'|' -v os="$os" '
    /^\| [A-Z][A-Z]*-[A-Z][A-Z]*-[0-9][0-9] \|/ {
      id = $2; applies = $4; result = $6; note = $7
      gsub(/^ +| +$/, "", id); gsub(/^ +| +$/, "", applies)
      gsub(/^ +| +$/, "", result); gsub(/^ +| +$/, "", note)
      if (os != "all" && tolower(applies) !~ os) next
      if (result == "pass") next
      if (result == "n/a" && note != "") next
      if (result == "n/a") { print id ": n/a needs a note"; next }
      print id ": " result
    }
  ' "$report")"
  if [ -n "$rows" ]; then
    echo "$rows" | while IFS= read -r line; do echo "error: $line" >&2; done
    problems=$((problems + $(echo "$rows" | wc -l)))
  fi
  if [ "$problems" -ne 0 ]; then
    echo "the report is not complete for $os ($problems problem(s))" >&2
    exit 1
  fi
  echo "the report is complete for $os"
}

case "${1:-}" in
  --check)
    items > /dev/null || exit 1
    echo "protocol ok: $(items | wc -l | tr -d ' ') items"
    ;;
  --verify)
    [ $# -eq 4 ] && [ "$3" = "--os" ] || die "usage: $0 --verify REPORT --os windows|macos|linux|all"
    case "$4" in windows | macos | linux | all) ;; *) die "unknown --os $4" ;; esac
    verify "$2" "$4"
    ;;
  "" | -h | --help)
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
    [ -n "${1:-}" ] || exit 1
    ;;
  -*)
    die "unknown option $1"
    ;;
  *)
    version="${1#v}"
    out_dir="."
    if [ $# -ge 3 ] && [ "$2" = "--out" ]; then
      out_dir="$3"
    elif [ $# -ne 1 ]; then
      die "usage: $0 <version> [--out DIR]"
    fi
    case "$version" in
      [0-9]*.[0-9]*.[0-9]*) ;;
      *) die "the version must look like 1.2.3 or 1.2.3-rc.1, not $1" ;;
    esac
    [ -d "$out_dir" ] || die "no directory $out_dir"
    file="$out_dir/release-test-report-v$version.md"
    write_report "$version" "$file"
    echo "wrote $file"
    ;;
esac
