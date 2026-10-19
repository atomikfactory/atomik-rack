# Security policy

## Supported versions

Atomik Rack is pre-1.0. Only the latest release (once there is one) and the `main` branch receive fixes.

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

Third-party plugins run inside the application process and are outside the threat model: a malicious plugin can do anything the application can. Crashes caused by a buggy plugin are normal bugs, not vulnerabilities.

## What to expect

- We aim to acknowledge reports within 7 days.
- We will keep you informed while we investigate and fix the problem.
- We will credit you in the advisory unless you prefer otherwise.
- Please give us reasonable time to release a fix before disclosing publicly.

## Other private reports

Use the same private channel for Code of Conduct matters (see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)) and for concerns about the origin of material in the repository (see the original work policy in [CONTRIBUTING.md](CONTRIBUTING.md)); describe where the material is and do not attach or repost it.
