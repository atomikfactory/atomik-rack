# Dependencies

Which dependencies are held on purpose, why, and how they are kept current. The licence policy is in
`deny.toml`; the upgrade chores are scheduled by `.github/workflows/deps.yml`.

## Policy

- Every dependency is declared once in `[workspace.dependencies]` of the root `Cargo.toml` (a
  crate used by one crate only may name its version in that crate). `Cargo.lock` is committed and every
  build in CI and in the release workflows uses `--locked`.
- A new dependency needs a reason in the pull request: what it does that the standard library and the
  crates already here do not, its licence (it must pass `cargo deny check`, GPL-3.0-or-later compatible),
  and whether it is maintained. Prefer small crates over frameworks.
- Dependabot opens weekly pull requests for compatible updates of cargo and of the GitHub actions
  (grouped). Actions are pinned to a commit hash with the version in a comment.
- Pre-1.0 crates (versions `0.x`) can break their API in a minor release. Where that has hurt, the
  version is pinned exactly (below); the rest follow the quarterly pass.

## Deliberate pins

| Dependency | Pin | Where | Why |
| --- | --- | --- | --- |
| `clack-host`, `clack-extensions` | `=0.2.0` (exact) | `crates/plugin-clap` | The CLAP host bindings are 0.x and the extension traits change between minors; the backend is written against 0.2.0. Bump both together, by hand, with the plugin tests. |
| `vst3` | `=0.3.0` (exact) | `crates/plugin-vst3` | The bindings are generated from one version of the VST3 SDK headers (3.8.0, MIT-licensed). The licence of the SDK differs between its versions, so a bump is a licence decision as well as an API one: read the licence of the headers behind the new version before moving. |
| `rust-version` and `rust-toolchain.toml` | 1.99 | root | One toolchain for CI, the release builds and the docs. The two files move together; a dependency that needs a newer compiler moves both in a pull request of their own. |
| `serde_json` | feature `float_roundtrip` | root | Saved files and settings must read back the floats that were written. |
| `symphonia`, `arboard` | `default-features = false` | root | Only the formats and backends the program uses are built: fewer crates, fewer advisories, shorter builds. |

The clipboard dependency (`arboard`, MIT or Apache-2.0) was added for the text fields; the system
clipboard is its only use.

## Duplicate versions

`cargo deny check bans` warns when two versions of a crate are in the graph. The ones that cannot be
removed from this side are listed in `deny.toml` with the crate that causes them, so that only a new
duplicate warns:

- the `winit` 0.30 tree (the objc2 0.5 family, an older `core-graphics`, `bitflags` 1 and an older
  `rustix`): goes away with a winit and vello upgrade;
- the `midir` tree (an older `jni`, `coremidi`'s `core-foundation`): goes away with a midir release
  that moves to the current `jni`;
- the `rfd` tree (an older `pollster` next to ours);
- `hashbrown` 0.16 (under `gpu-allocator`, under wgpu, under vello) and `miniz_oxide` 0.8 (under `png`).

When a bump removes the cause, delete its entry in `deny.toml`. A duplicate that is not in the list is
a new one: look at who pulls it before adding a skip.

## Advisory ignores

An advisory is ignored in `deny.toml` only with a reason and a review date (`review-by YYYY-MM-DD` in
the reason). `scripts/check-advisory-ignores.sh` runs in the Security workflow (on every pull request
and every Monday) and fails once the date has passed; look at the advisory again, then remove the
ignore or move the date on purpose. The ignore of the unmaintained-crate notice for `ttf-parser`
(it arrives through winit's Wayland decorations) is the only one today.

## The quarterly pass

On the first of January, April, July and October the `Dependencies` workflow opens a review issue
(only one at a time). The pass is:

1. Bump the 0.x GUI and audio stack together where a release exists: vello, vello_cpu, parley, winit,
   muda, rfd, cpal and midir. They share platform crates, so one at a time makes duplicates. Run the
   UI tests and look at what `cargo deny check bans` says; drop the skip entries that no longer apply.
2. Review the advisory ignores.
3. Check the exact pins above against their latest releases.
4. `cargo update`, the whole test suite, commit `Cargo.lock`.

Every Monday the same workflow prints `cargo update --dry-run` in its job summary: what a refresh of
the lock file would change today. Nothing is committed by it.
