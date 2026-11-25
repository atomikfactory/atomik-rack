#!/usr/bin/env python3
"""The test shards of the Coverage workflow.

The coverage run is split into shards that run in parallel jobs, because one job over the whole
workspace no longer fits in its time limit. The shards are listed in `coverage-shards.txt`.

    python3 scripts/coverage-shards.py matrix        the shard names as a JSON list (the job matrix)
    python3 scripts/coverage-shards.py check         fail unless every workspace package is in
                                                     exactly one shard
    python3 scripts/coverage-shards.py args <name>   the cargo package arguments of one shard
    python3 scripts/coverage-shards.py --self-test   test the parsing and the partition rule

`rest` lists `*`: every package that no other shard names. `args rest` is therefore
`--workspace --exclude a --exclude b ...`, and a new crate lands in it by itself.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CATCH_ALL = "*"


def parse(text: str) -> dict[str, list[str]]:
    """Shard name to its packages, in file order. A bad line is an error, never skipped."""
    shards: dict[str, list[str]] = {}
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        name, colon, rest = line.partition(":")
        name = name.strip()
        if not colon or not name or " " in name:
            raise ValueError(f"line {number}: expected `<shard>: <package> ...`")
        if name in shards:
            raise ValueError(f"line {number}: shard {name} is listed twice")
        packages = rest.split()
        if not packages:
            raise ValueError(f"line {number}: shard {name} has no package")
        if CATCH_ALL in packages and packages != [CATCH_ALL]:
            raise ValueError(f"line {number}: `*` must be the only entry of its shard")
        shards[name] = packages
    catch_all = [n for n, p in shards.items() if p == [CATCH_ALL]]
    if len(catch_all) != 1:
        raise ValueError("exactly one shard must list `*` (the one for every other package)")
    return shards


def partition(shards: dict[str, list[str]], members: list[str]) -> dict[str, list[str]]:
    """The packages each shard tests. Raises when a name is unknown or listed in two shards."""
    known = set(members)
    owner: dict[str, str] = {}
    for name, packages in shards.items():
        for package in packages:
            if package == CATCH_ALL:
                continue
            if package not in known:
                raise ValueError(f"shard {name} lists {package}, which is not a workspace package")
            if package in owner:
                raise ValueError(f"{package} is in two shards: {owner[package]} and {name}")
            owner[package] = name
    result = {n: sorted(p) for n, p in shards.items() if p != [CATCH_ALL]}
    for name, packages in shards.items():
        if packages == [CATCH_ALL]:
            result[name] = sorted(known - set(owner))
    placed = [p for packages in result.values() for p in packages]
    if sorted(placed) != sorted(known) or len(placed) != len(set(placed)):
        raise ValueError("the shards do not cover every workspace package exactly once")
    return result


def cargo_args(shards: dict[str, list[str]], name: str, members: list[str]) -> list[str]:
    if name not in shards:
        raise ValueError(f"no shard named {name}")
    packages = partition(shards, members)[name]
    if shards[name] == [CATCH_ALL]:
        listed = sorted(set(members) - set(packages))
        return ["--workspace"] + [x for p in listed for x in ("--exclude", p)]
    return [x for p in packages for x in ("-p", p)]


def workspace_members() -> list[str]:
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(out)
    ids = set(meta["workspace_members"])
    return sorted(p["name"] for p in meta["packages"] if p["id"] in ids)


def self_test() -> None:
    members = ["a", "b", "c", "d"]
    shards = parse("# c\nx: a b\n\nrest: *  # all else\n")
    assert partition(shards, members) == {"x": ["a", "b"], "rest": ["c", "d"]}
    assert cargo_args(shards, "x", members) == ["-p", "a", "-p", "b"]
    assert cargo_args(shards, "rest", members) == ["--workspace", "--exclude", "a", "--exclude", "b"]
    # A package that a later edit adds to the workspace falls into the catch-all shard.
    assert partition(shards, members + ["e"])["rest"] == ["c", "d", "e"]
    for bad, why in [
        ("x: a\ny: a\nrest: *\n", "in two shards"),
        ("x: zz\nrest: *\n", "not a workspace package"),
        ("x: a\n", "exactly one shard"),
        ("x: a *\nrest: *\n", "only entry"),
        ("x: a\nx: b\nrest: *\n", "twice"),
        ("x a\nrest: *\n", "expected"),
        ("x:\nrest: *\n", "no package"),
    ]:
        try:
            partition(parse(bad), members)
        except ValueError as error:
            assert why in str(error), (bad, error)
        else:
            raise AssertionError(f"accepted: {bad!r}")
    print("coverage-shards self-test: ok")


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        self_test()
        return 0
    try:
        shards = parse((HERE / "coverage-shards.txt").read_text(encoding="utf-8"))
        command = argv[1] if len(argv) > 1 else ""
        if command == "matrix" and len(argv) == 2:
            print(json.dumps(list(shards)))
        elif command == "check" and len(argv) == 2:
            result = partition(shards, workspace_members())
            for name, packages in result.items():
                print(f"{name}: {len(packages)} packages")
        elif command == "args" and len(argv) == 3:
            print(" ".join(cargo_args(shards, argv[2], workspace_members())))
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"coverage-shards: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
