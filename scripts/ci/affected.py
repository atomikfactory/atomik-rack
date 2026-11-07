#!/usr/bin/env python3
"""Which workspace packages a change affects, for the CI test and clippy jobs.

Reads the changed file paths (one per line) on stdin and prints one line:

* ``all``: test the whole workspace. A file outside the crates (the root manifest or lock file,
  the toolchain, CI, scripts, factory content, example projects...) can change any crate's
  build or tests, so it always means everything.
* ``none``: nothing to build or test (documents only, already decided by the caller, or an
  empty list).
* otherwise the affected packages as cargo arguments, ``-p name -p name ...``: every package
  that owns a changed file, and every package that depends on one of those, directly or not,
  through normal, dev or build dependencies (a change to a crate can break the crates that use
  it and their tests).

Only the standard library is used (``tomllib`` needs Python 3.11), so the job needs no Rust
toolchain to decide. Run it locally with
``git diff --name-only origin/main... | python3 scripts/ci/affected.py``.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# Files under crates/ that never change what a crate builds or tests.
DOC_SUFFIXES = (".md",)

# Data folders at the root that only the tests of some crates read: a change there tests the
# crates whose code names the folder in a string literal (`"../../examples/projects"`,
# `.join("factory")`), not everything.
DATA_DIRS = ("factory", "examples")


def packages() -> dict[str, tuple[Path, set[str]]]:
    """Every package under crates/: name -> (directory relative to the root, dependency names)."""
    found: dict[str, tuple[Path, set[str]]] = {}
    for manifest in sorted((ROOT / "crates").rglob("Cargo.toml")):
        if "target" in manifest.relative_to(ROOT).parts:
            continue
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        package = data.get("package")
        if not package:
            continue
        deps: set[str] = set()
        tables = [data]
        tables += list(data.get("target", {}).values())
        for table in tables:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for key, spec in table.get(kind, {}).items():
                    name = spec.get("package", key) if isinstance(spec, dict) else key
                    deps.add(name)
        found[package["name"]] = (manifest.parent.relative_to(ROOT), deps)
    return found


def owner(path: str, dirs: list[tuple[Path, str]]) -> str | None:
    """The package whose directory holds `path`, the deepest one when packages nest."""
    parts = Path(path).parts
    for directory, name in dirs:
        if parts[: len(directory.parts)] == directory.parts:
            return name
    return None


def readers(pattern: str, dirs: list[tuple[Path, str]]) -> set[str]:
    """The packages whose Rust sources hold a string literal matching `pattern`."""
    literal = re.compile(r'"[^"\n]*' + pattern + r'[^"\n]*"')
    found: set[str] = set()
    for source in (ROOT / "crates").rglob("*.rs"):
        rel = source.relative_to(ROOT)
        if "target" in rel.parts:
            continue
        if literal.search(source.read_text(encoding="utf-8", errors="ignore")):
            name = owner(rel.as_posix(), dirs)
            if name:
                found.add(name)
    return found


def affected(changed: list[str]) -> str:
    changed = [c.strip() for c in changed if c.strip()]
    if not changed:
        return "none"
    pkgs = packages()
    # Deepest directories first, so a nested package wins over its parent.
    dirs = sorted(((d, n) for n, (d, _) in pkgs.items()), key=lambda x: -len(x[0].parts))
    roots: set[str] = set()
    # Crates whose tests read a changed document or data file: a file is not part of any build,
    # so the crates that depend on a reader are not affected through it.
    readers_only: set[str] = set()
    for path in changed:
        if path.endswith(DOC_SUFFIXES) or path.startswith("docs/"):
            # A document a crate's code reads by path (the keymap guide, the devices guide) is
            # checked by that crate's tests: test that crate. A mention in a comment does not count.
            readers_only |= readers(re.escape(Path(path).name), dirs)
            continue
        top = path.split("/", 1)[0]
        if top in DATA_DIRS and "/" in path:
            # `"factory"`, `"../../factory/..."` or `".../examples/..."`, not prose such as
            # "the factory list".
            readers_only |= readers(rf'(?:(?<="){top}(?=")|{top}/|/{top}\b)', dirs)
            continue
        if not path.startswith("crates/"):
            return "all"
        name = owner(path, dirs)
        if name is None:
            return "all"
        roots.add(name)
    if not roots and not readers_only:
        return "none"
    dependents: dict[str, set[str]] = {n: set() for n in pkgs}
    for name, (_, deps) in pkgs.items():
        for dep in deps:
            if dep in dependents:
                dependents[dep].add(name)
    result = set(roots)
    todo = list(roots)
    while todo:
        for user in dependents[todo.pop()]:
            if user not in result:
                result.add(user)
                todo.append(user)
    result |= readers_only
    return " ".join(f"-p {name}" for name in sorted(result))


if __name__ == "__main__":
    print(affected(sys.stdin.read().splitlines()))
