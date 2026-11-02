#!/bin/sh
# Fails when an advisory ignore in deny.toml has no review date or its date has passed, so no
# ignore outlives the reason it was made for. Every `{ id = "RUSTSEC-..." ... }` entry must carry
# `review-by YYYY-MM-DD` in its reason; renew the date only after looking at the advisory again.
# Usage: sh scripts/check-advisory-ignores.sh [deny.toml]
# TODAY=YYYY-MM-DD overrides the date (tests).
set -eu
file="${1:-deny.toml}"
today="${TODAY:-$(date -u +%Y-%m-%d)}"
status=0
count=0
# One entry per line: the ignore entries are single-line inline tables.
for entry in $(grep -n '^[[:space:]]*{[[:space:]]*id[[:space:]]*=' "$file" | cut -d: -f1); do
  line="$(sed -n "${entry}p" "$file")"
  id="$(printf '%s' "$line" | sed -n 's/.*id[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p')"
  date="$(printf '%s' "$line" | sed -n 's/.*review-by \([0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]\).*/\1/p')"
  count=$((count + 1))
  if [ -z "$date" ]; then
    echo "error: $file:$entry: the ignore of $id has no 'review-by YYYY-MM-DD' in its reason" >&2
    status=1
  elif [ "$today" \> "$date" ]; then
    echo "error: $file:$entry: the ignore of $id was due for review on $date (today is $today): check the advisory again, then remove the ignore or move the date" >&2
    status=1
  else
    echo "ok: $id is ignored until $date"
  fi
done
echo "$count advisory ignore(s) checked"
exit "$status"
