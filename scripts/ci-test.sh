#!/bin/sh
# Run the test programs of the workspace side by side, as CI does. See scripts/ci-test.py.
#   scripts/ci-test.sh <debug|release> [--workspace | -p name ... | name ...]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
for py in python3 python; do
  if command -v "$py" >/dev/null 2>&1 && "$py" -c 'import sys; sys.exit(sys.version_info[0] < 3)' >/dev/null 2>&1; then
    exec "$py" "$here/ci-test.py" "$@"
  fi
done
echo "ci-test.sh: python3 not found" >&2
exit 2
