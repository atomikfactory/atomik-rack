# Releasing

How a release of Atomik Rack is prepared, tagged and built.

## Versioning

The project follows [Semantic Versioning](https://semver.org/) with the usual pre-1.0 caveat: while the major version is 0, any release may change APIs and the patch and project file formats. Bump the minor version for new features or breaking changes and the patch version for fixes only. Project files carry their own format version; older versions must keep loading, and the changelog says when a format version changes.

The version lives in one place, `[workspace.package]` in the root `Cargo.toml`; every crate inherits it. No crate is published to crates.io (`publish = false`).

## Checklist

1. Make sure `main` is green: formatting, clippy, tests on all three operating systems, `cargo deny check`, the coverage gate and `sh scripts/check-public.sh` (see [TESTING.md](TESTING.md)).
2. Run the manual and hardware checks that apply to the release (see [TESTING.md](TESTING.md)).
3. Update [CHANGELOG.md](../CHANGELOG.md): move the entries under `[Unreleased]` into a new `[X.Y.Z]` section dated with the release date, keep the Keep a Changelog headings, and start a fresh empty `[Unreleased]`.
4. Bump `version` in `[workspace.package]` in `Cargo.toml` and refresh the lock file with `cargo update --workspace` (this only rewrites the workspace crate versions).
5. Open a pull request with those two changes, get it reviewed and merged.
6. Tag the merge commit on `main` and push the tag:

   ```sh
   git tag -s vX.Y.Z -m "Atomik Rack vX.Y.Z"   # use -a instead of -s for an unsigned annotated tag
   git push origin vX.Y.Z
   ```

   Signed tags are optional; annotated tags are the minimum. Tags are named `vX.Y.Z` (pre-releases `vX.Y.Z-rc.1`).
7. The `Release` workflow builds the binaries and creates a **draft** GitHub release. Download the artifacts, check that they start and that the checksums match, paste the changelog section into the release notes, then publish the draft.

## Binaries

The `Release` workflow (`.github/workflows/release.yml`) runs on a `vX.Y.Z` tag and can also be started by hand, in which case it only produces workflow artifacts. For each of Windows, macOS and Linux it builds with `cargo build --release --locked` the four programs `atomik`, `atomik-render`, `atomik-play` and `atomik-plugin-scan`, and packages them with the licence, README and changelog as a `.zip` (Windows) or `.tar.gz` (macOS, Linux), next to a `.sha256` file.

To build by hand, follow the build instructions in the [README](../README.md) and run `cargo build --release --locked --workspace`.

The binaries are not code-signed or notarised. macOS and Windows may warn on first launch; the release notes should say so.

## ASIO

The ASIO backend is an opt-in build feature (`asio`) that needs the Steinberg ASIO SDK. The SDK is never bundled, committed or downloaded by CI, and official release binaries are built without it. Anyone who wants ASIO builds from source with their own copy of the SDK, under the SDK's own licence terms.

## Checksums

Every archive is published with a SHA-256 file. Users verify a download with `sha256sum -c <archive>.sha256` (Linux), `shasum -a 256 -c <archive>.sha256` (macOS) or `Get-FileHash -Algorithm SHA256 <archive>` (Windows, compare by eye).

## After the release

- Check that the published release shows the expected files.
- Open a pull request to keep `[Unreleased]` in the changelog current, if it was not already.
