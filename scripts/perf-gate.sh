#!/bin/sh
# Instruction-count gate for the benches (used by .github/workflows/perf.yml, never per pull request).
#
#   scripts/perf-gate.sh measure OUT      run each bench binary once under valgrind (callgrind) and
#                                         write one `<bench> <instructions>` line per binary to OUT
#   scripts/perf-gate.sh compare BASELINE CURRENT [PERCENT]
#                                         exit 1 when a bench in CURRENT executes more than PERCENT
#                                         (default 10) instructions above BASELINE; a bench missing
#                                         from either file is reported and ignored
#   scripts/perf-gate.sh inflate IN OUT FACTOR
#                                         scale every count of IN by FACTOR (1.2 = 20 % more): the
#                                         injected regression the gate must catch
#
# `measure` needs valgrind (Linux) and the bench binaries built with `cargo bench --no-run`. A
# bench binary runs one pass of every scenario when ATOMIK_BENCH_ONCE is set, which is what
# makes its instruction count repeatable. The count is of the whole pass, build of the
# scenario included.
set -eu

BENCHES="render_batch process blocks document"

find_bench() {
  # The newest executable of the bench named $1 in target/release/deps (not its .d file).
  ls -t target/release/deps/"$1"-* 2>/dev/null | grep -v '\.d$' | grep -v '\.pdb$' | head -n 1
}

measure() {
  out=$1
  : >"$out"
  for bench in $BENCHES; do
    exe=$(find_bench "$bench")
    if [ -z "$exe" ]; then
      echo "perf-gate: no executable for the bench $bench (run cargo bench --no-run first)" >&2
      exit 2
    fi
    log=$(mktemp)
    ATOMIK_BENCH_ONCE=1 valgrind --tool=callgrind --callgrind-out-file=/dev/null "$exe" >/dev/null 2>"$log"
    count=$(sed -n 's/^==[0-9]*== Collected : \([0-9]*\).*/\1/p' "$log" | tail -n 1)
    rm -f "$log"
    if [ -z "$count" ]; then
      echo "perf-gate: valgrind gave no instruction count for $bench" >&2
      exit 2
    fi
    echo "$bench $count" >>"$out"
  done
  cat "$out"
}

compare() {
  baseline=$1
  current=$2
  percent=${3:-10}
  awk -v pct="$percent" '
    NR == FNR { base[$1] = $2; next }
    {
      if (!($1 in base)) { printf "%-14s %14s  (no baseline)\n", $1, $2; next }
      seen[$1] = 1
      change = ($2 - base[$1]) * 100.0 / base[$1]
      verdict = (change > pct) ? "REGRESSION" : "ok"
      if (change > pct) bad = 1
      printf "%-14s %14s  baseline %14s  %+7.2f %%  %s\n", $1, $2, base[$1], change, verdict
    }
    END {
      for (b in base) if (!(b in seen)) printf "%-14s (in the baseline only)\n", b
      exit bad
    }' "$baseline" "$current"
}

inflate() {
  awk -v f="$3" '{ printf "%s %.0f\n", $1, $2 * f }' "$1" >"$2"
}

case "${1:-}" in
  measure) [ $# -eq 2 ] || { echo "usage: $0 measure OUT" >&2; exit 2; }; measure "$2" ;;
  compare) [ $# -ge 3 ] && [ $# -le 4 ] || { echo "usage: $0 compare BASELINE CURRENT [PERCENT]" >&2; exit 2; }; compare "$2" "$3" "${4:-10}" ;;
  inflate) [ $# -eq 4 ] || { echo "usage: $0 inflate IN OUT FACTOR" >&2; exit 2; }; inflate "$2" "$3" "$4" ;;
  *) echo "usage: $0 measure OUT | compare BASELINE CURRENT [PERCENT] | inflate IN OUT FACTOR" >&2; exit 2 ;;
esac
