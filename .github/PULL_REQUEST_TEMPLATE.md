## Summary

<!-- What does this change do, and why? Link the issue: "Closes #123". -->

## How it was tested

<!-- Name the new or changed tests. For bug fixes, confirm the regression test failed before the fix.
     For a change to the sound or the UI, also say which QA steps of docs/MCP.md you ran and what they
     showed (for example: bounce bit-identical before and after, blocks 64/333/512, or the screenshot and
     the texts of the new state). -->

## Checklist

- [ ] **Original work:** this contribution is my own work, or third-party material under a GPL-3.0-or-later-compatible licence that I have identified above. It contains nothing copied from proprietary products or their binaries.
- [ ] **DCO:** every commit is signed off (`git commit -s`).
- [ ] **Tests:** behaviour changes are covered by tests (written first where possible), and bug fixes include a regression test.
- [ ] **Gates pass locally:**
  - [ ] `cargo fmt --all --check`
  - [ ] `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] `cargo test --workspace`
  - [ ] `cargo deny check`
- [ ] **Real-time safety (HI-1):** no allocation, freeing, locking, blocking or I/O on the render path; new render-path code is exercised under `atomik_testkit::no_alloc::assert_no_alloc` or `DeviceHarness`. (Tick if not applicable.)
- [ ] **Determinism (HI-2):** no `HashMap`/`HashSet`, no time sources, only seeded RNGs; output does not depend on host block size. (Tick if not applicable.)
- [ ] **Architecture:** the crate dependency rules in `docs/ARCHITECTURE.md` are respected.
- [ ] **Docs:** public APIs are documented and user-facing docs (and the changelog, if user-visible) are updated.
