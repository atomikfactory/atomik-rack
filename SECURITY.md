# Security policy

## Supported versions

Atomik Rack is pre-1.0. Only the latest release (a release candidate counts) and the `main` branch receive fixes.

| Version | Supported |
|---|---|
| `main` | Yes |
| Latest release | Yes |
| Anything older | No |

## Reporting a vulnerability

**Do not open a public issue for security problems.**

Report privately through GitHub security advisories:

<https://github.com/atomikfactory/atomik-rack/security/advisories/new>

(On the repository page: **Security** > **Advisories** > **Report a vulnerability**.)

Please include:

- a description of the problem and its impact;
- steps to reproduce, ideally a minimal patch or project file or a test case;
- the commit hash or version and the platform you tested on;
- any suggested fix.

Examples of relevant issues: memory-safety bugs, crashes or hangs triggered by a crafted patch, project or audio file, path-handling problems (for example in project bundles), and vulnerable dependencies not yet caught by `cargo deny`.

The readers of these files are bounded and fuzzed: a file may claim any size, but a reader allocates
no more than the file can account for and refuses what exceeds a documented limit (project files
256 MiB, an audio file 2^30 samples in memory, a MIDI file two million events per track). The fuzz
targets are in `fuzz/` and run on a schedule (see `docs/TESTING.md`). A crafted file that gets past
a limit, aborts the program or makes it run out of memory is a vulnerability worth reporting.

## Third-party plugins

Plugin hosting is off until the user enables it. When it is on, each plugin instance runs in its own
child process by default (the sandbox, Settings > Plugins; `atomik-plugin-host` is the child), and a
scan runs out of process too. With the sandbox option off, plugins run inside the application
process.

- A plugin is trusted with the user's privileges. The sandbox is a robustness boundary, not a
  privilege boundary: a malicious plugin can read and write the user's files, and plugin bugs are
  not vulnerabilities in Atomik Rack.
- The host side of the sandbox is in scope: the crates `plugin-ipc`, `plugin-sandbox`,
  `plugin-hostd` and `plugin-host`, including the shared-memory block and the control channel they
  share with the child. Report a plugin that, through that channel, crashes or hangs the
  application, corrupts its memory, or escapes the sandbox's guarantees (a hostile child must not be
  able to take the application down).
- The plugin cache (`plugins.json` in the settings folder) is trusted as the user's own file. It is
  written by Atomik after a scan and lists the plugin files found, their ids and how to load them; the
  app does not sign it, and a scan compares each file's stamp (size and modification time) with it
  to decide whether the file is read again. Whoever can write to the
  settings folder can already run code as the user, so the cache is not a boundary: a plugin that the
  cache names is loaded the way a scanned one is (in the sandbox child when the sandbox is on).
  A cache of another schema version is rebuilt.
- The macOS app is built with the hardened runtime and the entitlement
  `com.apple.security.cs.disable-library-validation` (with the two that allow generated code), because
  it must load plugin libraries that other developers signed, or did not sign. That entitlement is
  what lets a library signed by anyone be loaded into the application or the plugin host process:
  the sandbox child limits the damage of a crash, not of a hostile plugin. See
  [docs/RELEASING.md](docs/RELEASING.md) for the full list of entitlements.
- Where plugins run is shown, not assumed: Settings > Plugins and the status line say `sandbox` or
  `in-process`, and when the sandbox is switched on but cannot start the app says so once and runs
  the plugins in-process instead of failing silently.
- A crash or hang of the plugin's own process, or of the application when the sandbox is off, caused
  by a buggy plugin is a normal bug, not a vulnerability.

## What to expect

- We aim to acknowledge reports within 7 days.
- We will keep you informed while we investigate and fix the problem.
- We will credit you in the advisory unless you prefer otherwise.
- Please give us reasonable time to release a fix before disclosing publicly.

## Other private reports

Use the same private channel for Code of Conduct matters (see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)) and for concerns about the origin of material in the repository (see the original work policy in [CONTRIBUTING.md](CONTRIBUTING.md)); describe where the material is and do not attach or repost it.
