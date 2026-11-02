#!/bin/sh
# Fails unless the saved-file corpus holds a file for the current project-format version:
# crates/model/tests/fixtures/corpus/atomik-v<PROJECT_VERSION>.atomik. Each format bump adds
# its file (scripts/format-corpus.sh), so a release cannot ship a format with no real saved file
# in the corpus. ATOMIK_CORPUS_VERSION overrides the version (used by the test of this script).
# Run from the repository root.
set -eu

version="${ATOMIK_CORPUS_VERSION:-$(sed -n 's/^pub const PROJECT_VERSION: u32 = \([0-9]*\);.*/\1/p' crates/model/src/project/mod.rs)}"
if [ -z "$version" ]; then
  echo "error: could not read PROJECT_VERSION from crates/model/src/project/mod.rs" >&2
  exit 1
fi
file="crates/model/tests/fixtures/corpus/atomik-v${version}.atomik"
if [ ! -f "$file" ]; then
  echo "error: project format $version has no corpus file ($file); run: sh scripts/format-corpus.sh $version $version" >&2
  exit 1
fi
if ! grep -q "^  \"version\": ${version},\$" "$file"; then
  echo "error: $file is not a version $version file" >&2
  exit 1
fi
echo "corpus file for project format $version: $file"
