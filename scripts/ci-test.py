#!/usr/bin/env python3
"""Run the test programs of the workspace side by side, as CI does.

usage: ci-test.sh <debug|release> [cargo package arguments...]

`cargo test` runs the test programs one after the other, each with libtest's threads. Most of
the 19 000 tests are small, so one program rarely keeps all cores busy. This script builds the
programs once (`cargo test --no-run`), then runs several of them at the same time, each with
the environment and the working directory `cargo test` gives it, and prints one table at the end.
Nothing else changes: the tests run as libtest threads in one process per program, no test is
retried, every program runs even if another fails.

Package arguments: `--workspace`, `-p name`, `--exclude name`, or bare package names (each one
means `-p name`). SKIP_RENDERS=true leaves out the eight example-song programs of atomik-render.
CI_TEST_JOBS overrides the number of programs run at once (default: the CPU count, at most 3).
Doc tests are not run here: use `cargo test --doc` for them.
"""
import concurrent.futures
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

# The example-song programs: whole example projects rendered one or more times.
RENDER_PROGRAMS = {
    ("atomik-render", name)
    for name in (
        "fx_demo",
        "plugin_demo_vst3",
        "plugin_demo",
        "character_demo",
        "container_demo",
        "determinism_examples",
        "player_demo",
        "drum_demo",
    )
}

# Programs that start and kill real plugin child processes or read the process table: they run
# one at a time, after the parallel batch, with nothing else running. The leak loops compare the
# thread and handle counts of their own process at two points; on a machine busy with other
# programs a thread that is leaving can still be counted at the first point.
SERIAL_PROGRAMS = {
    ("atomik-plugin-hostd", "e2e_kill"),
    ("atomik-plugin-hostd", "e2e_watch"),
    ("atomik-plugin-hostd", "orphan"),
    ("atomik-plugin-hostd", "sweep"),
    ("atomik-app", "kill_recovery"),
    ("atomik-plugin-hostd", "leak_loops"),
    ("atomik-plugin-clap", "leak"),
    ("atomik-mcp", "leak_loop"),
}

VALUE_FLAGS = {"-p", "--package", "--exclude"}


def package_args(args):
    out = []
    i = 0
    while i < len(args):
        a = args[i]
        if a in VALUE_FLAGS and i + 1 < len(args):
            out += [a, args[i + 1]]
            i += 2
        elif a.startswith("-"):
            out.append(a)
            i += 1
        else:
            out += ["-p", a]
            i += 1
    return out


def capture(cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout.strip()


def library_path_var():
    if sys.platform == "win32":
        return "PATH"
    if sys.platform == "darwin":
        return "DYLD_FALLBACK_LIBRARY_PATH"
    return "LD_LIBRARY_PATH"


def build(profile, pkg_args):
    cmd = ["cargo", "test", *pkg_args, "--no-run", "--message-format=json-render-diagnostics"]
    if profile == "release":
        cmd.insert(2, "--release")
    print("+ " + " ".join(cmd), flush=True)
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    tests, bins, link_dirs = [], {}, set()
    for line in proc.stdout:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        reason = msg.get("reason")
        if reason == "compiler-artifact" and msg.get("executable"):
            pid = msg["package_id"]
            target = msg["target"]
            if msg["profile"].get("test"):
                tests.append((pid, target, msg["executable"]))
            elif "bin" in target["kind"]:
                bins.setdefault(pid, {})[target["name"]] = msg["executable"]
        elif reason == "build-script-executed":
            for p in msg.get("linked_paths", []):
                link_dirs.add(p.split("=", 1)[-1])
    if proc.wait() != 0:
        print("error: the build failed", file=sys.stderr)
        sys.exit(1)
    return tests, bins, link_dirs


def program_env(pkg, target, pid, bins, exe, tmpdir, link_dirs, sysroot_lib, threads):
    root = os.path.dirname(pkg["manifest_path"])
    env = dict(os.environ)
    env["CARGO"] = os.environ.get("CARGO") or shutil.which("cargo") or "cargo"
    env["CARGO_MANIFEST_DIR"] = root
    env["CARGO_PKG_NAME"] = pkg["name"]
    env["CARGO_PKG_VERSION"] = pkg["version"]
    major, minor, rest = (pkg["version"].split(".", 2) + ["", ""])[:3]
    patch, _, pre = rest.partition("-")
    env["CARGO_PKG_VERSION_MAJOR"] = major
    env["CARGO_PKG_VERSION_MINOR"] = minor
    env["CARGO_PKG_VERSION_PATCH"] = patch
    env["CARGO_PKG_VERSION_PRE"] = pre
    env["CARGO_PKG_AUTHORS"] = ":".join(pkg.get("authors") or [])
    env["CARGO_PKG_DESCRIPTION"] = pkg.get("description") or ""
    env["CARGO_PKG_HOMEPAGE"] = pkg.get("homepage") or ""
    env["CARGO_PKG_REPOSITORY"] = pkg.get("repository") or ""
    env["CARGO_PKG_LICENSE"] = pkg.get("license") or ""
    env["CARGO_PKG_LICENSE_FILE"] = pkg.get("license_file") or ""
    env["CARGO_PKG_RUST_VERSION"] = pkg.get("rust_version") or ""
    env["CARGO_PKG_README"] = pkg.get("readme") or ""
    env["CARGO_CRATE_NAME"] = target["name"].replace("-", "_")
    env["CARGO_PRIMARY_PACKAGE"] = "1"
    if "test" in target["kind"]:
        env["CARGO_TARGET_TMPDIR"] = tmpdir
        for name, path in bins.get(pid, {}).items():
            env["CARGO_BIN_EXE_" + name] = path
    var = library_path_var()
    dirs = [os.path.dirname(exe), os.path.dirname(os.path.dirname(exe)), sysroot_lib]
    dirs += sorted(link_dirs)
    old = os.environ.get(var)
    env[var] = os.pathsep.join(dirs + ([old] if old else []))
    env.setdefault("RUST_TEST_THREADS", str(threads))
    return root, env


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ("debug", "release"):
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    profile = sys.argv[1]
    pkg_args = package_args(sys.argv[2:])
    skip_renders = os.environ.get("SKIP_RENDERS", "") == "true"

    tests, bins, link_dirs = build(profile, pkg_args)

    meta = json.loads(capture(["cargo", "metadata", "--format-version=1", "--no-deps"]))
    packages = {p["id"]: p for p in meta["packages"]}
    tmpdir = os.path.join(meta["target_directory"], "tmp")
    os.makedirs(tmpdir, exist_ok=True)
    sysroot_lib = capture(["rustc", "--print", "target-libdir"])

    cpus = os.cpu_count() or 2
    jobs = max(1, int(os.environ.get("CI_TEST_JOBS") or min(cpus, 3)))
    threads = max(2, cpus // jobs)

    programs = []  # (label, key, exe, cwd, env)
    skipped = []
    for pid, target, exe in tests:
        pkg = packages[pid]
        key = (pkg["name"], target["name"])
        label = "%s::%s" % key
        if skip_renders and key in RENDER_PROGRAMS:
            skipped.append(label)
            continue
        root, env = program_env(pkg, target, pid, bins, exe, tmpdir, link_dirs, sysroot_lib, threads)
        programs.append((label, key, exe, root, env))

    # Biggest executables first: the long programs start early and the run ends on small ones.
    programs.sort(key=lambda p: -os.path.getsize(p[2]))
    parallel = [p for p in programs if p[1] not in SERIAL_PROGRAMS]
    serial = [p for p in programs if p[1] in SERIAL_PROGRAMS]

    logs = tempfile.mkdtemp(prefix="ci-test-logs-")
    results = {}

    def run(p):
        label, _key, exe, cwd, env = p
        log = os.path.join(logs, label.replace(":", "_") + ".log")
        start = time.monotonic()
        with open(log, "wb") as out:
            code = subprocess.run(
                [exe], cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL
            ).returncode
        results[label] = (time.monotonic() - start, code, log)
        status = "ok" if code == 0 else "FAILED (%d)" % code
        print("  %-52s %7.1fs  %s" % (label, results[label][0], status), flush=True)

    print(
        "running %d test programs %d at a time (%s test threads each), then %d one at a time; skipped %d"
        % (len(parallel), jobs, os.environ.get("RUST_TEST_THREADS") or threads, len(serial), len(skipped)),
        flush=True,
    )
    wall = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        for f in [pool.submit(run, p) for p in parallel]:
            f.result()
    for p in serial:
        run(p)
    wall = time.monotonic() - wall

    failed = [label for label, r in results.items() if r[1] != 0]
    print("\n%-52s %8s  %s" % ("program", "seconds", "result"))
    for label, (secs, code, _log) in sorted(results.items(), key=lambda kv: -kv[1][0]):
        print("%-52s %8.1f  %s" % (label, secs, "ok" if code == 0 else "FAILED (%d)" % code))
    for label in skipped:
        print("%-52s %8s  skipped" % (label, "-"))
    print(
        "\n%d programs, %d failed, %.1f s wall (%.1f s summed)"
        % (len(results), len(failed), wall, sum(r[0] for r in results.values()))
    )
    for label in failed:
        print("\n===== output of %s =====" % label)
        with open(results[label][2], "r", encoding="utf-8", errors="replace") as f:
            sys.stdout.write(f.read())
    sys.stdout.flush()
    if failed:
        print("\nlogs kept in %s" % logs)
    else:
        shutil.rmtree(logs, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
