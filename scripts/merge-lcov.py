#!/usr/bin/env python3
"""Merge the lcov reports of the coverage shards into one and apply the workspace line gate.

    python3 scripts/merge-lcov.py --output lcov.info [--fail-under-lines 90] shard-1.info ...
    python3 scripts/merge-lcov.py --self-test

Each shard measures the whole code it links, so a source file can be in several reports (the
model crate in every one of them, for instance), each with its own hit counts. The merge adds
the counts of a line over the reports and a line is covered when any shard ran it, which is what
one report over all the test programs would say. Only the line records are kept (`SF`, `DA`,
`LF`, `LH`): the gate and the per-crate floors (scripts/coverage-floors.sh) read lines only.
"""

from __future__ import annotations

import sys
from pathlib import Path


def read(text: str, into: dict[str, dict[int, int]]) -> None:
    current: dict[int, int] | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("SF:"):
            current = into.setdefault(line[3:], {})
        elif line.startswith("DA:") and current is not None:
            fields = line[3:].split(",")
            number, count = int(fields[0]), int(fields[1])
            current[number] = current.get(number, 0) + max(count, 0)
        elif line == "end_of_record":
            current = None


def write(files: dict[str, dict[int, int]]) -> str:
    out: list[str] = []
    for path in sorted(files):
        lines = files[path]
        out.append("TN:")
        out.append(f"SF:{path}")
        out.extend(f"DA:{n},{lines[n]}" for n in sorted(lines))
        out.append(f"LF:{len(lines)}")
        out.append(f"LH:{sum(1 for c in lines.values() if c > 0)}")
        out.append("end_of_record")
    return "\n".join(out) + "\n"


def totals(files: dict[str, dict[int, int]]) -> tuple[int, int]:
    found = sum(len(lines) for lines in files.values())
    hit = sum(1 for lines in files.values() for c in lines.values() if c > 0)
    return found, hit


def self_test() -> None:
    files: dict[str, dict[int, int]] = {}
    read("SF:/w/crates/a/src/x.rs\nFN:1,f\nDA:1,0\nDA:2,3\nLF:2\nLH:1\nend_of_record\n", files)
    read("SF:/w/crates/a/src/x.rs\nDA:1,2\nDA:2,0\nDA:3,0\nend_of_record\n"
         "SF:/w/crates/b/src/y.rs\nDA:7,1\nend_of_record\n", files)
    assert files["/w/crates/a/src/x.rs"] == {1: 2, 2: 3, 3: 0}
    assert totals(files) == (4, 3)
    again: dict[str, dict[int, int]] = {}
    read(write(files), again)
    assert again == files
    assert "LF:3\nLH:2\n" in write(files)
    print("merge-lcov self-test: ok")


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        self_test()
        return 0
    output = None
    gate = None
    inputs: list[str] = []
    args = iter(argv[1:])
    for arg in args:
        if arg == "--output":
            output = next(args, None)
        elif arg == "--fail-under-lines":
            gate = next(args, None)
        else:
            inputs.append(arg)
    if output is None or not inputs:
        print(__doc__, file=sys.stderr)
        return 2
    files: dict[str, dict[int, int]] = {}
    for name in inputs:
        read(Path(name).read_text(encoding="utf-8", errors="replace"), files)
    Path(output).write_text(write(files), encoding="utf-8")
    found, hit = totals(files)
    percent = 100.0 * hit / found if found else 0.0
    print(f"merged {len(inputs)} reports: {len(files)} files, {hit} of {found} lines, {percent:.2f}%")
    if gate is not None and percent + 1e-9 < float(gate):
        print(f"error: line coverage {percent:.2f}% is below {gate}%", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
