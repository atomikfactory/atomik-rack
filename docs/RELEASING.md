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
7. The `Release` workflow builds and smoke-tests the archives and creates a **draft** GitHub release (it fails early if the tag does not match the `Cargo.toml` version). Download the artifacts, check that they start on real machines (the smoke test cannot open a window or play sound) and that the checksums match, paste the changelog section into the release notes, then publish the draft.

## Binaries

The `Release` workflow (`.github/workflows/release.yml`) runs on a `vX.Y.Z` tag (pre-releases `vX.Y.Z-rc.1`) and can be started by hand (`gh workflow run release.yml --ref <branch>`). A manual run builds and smoke-tests everything and keeps the archives as workflow artifacts; it never creates a release. The packaging logic lives in `scripts/release/`, so it reads the same locally as in CI.

Jobs:

1. `version` resolves the version: the tag name on a tag, `dev-<shortsha>` otherwise. On a tag it fails unless the tag is `v` plus the `[workspace.package]` version in `Cargo.toml` (`scripts/release/version.sh`).
2. `build` runs on Linux, Windows and macOS and builds `atomik`, `atomik-render`, `atomik-play` and `atomik-plugin-scan` with `cargo build --release --locked`. The ASIO feature is never enabled. On macOS it builds `aarch64-apple-darwin` and `x86_64-apple-darwin` and joins each program with `lipo`.
3. `smoke` downloads each archive, verifies its `.sha256`, unpacks it the way a user would and runs the checks below (`scripts/release/smoke.sh`).
4. `draft-release` (tags only, after `build` and `smoke`) writes `SHA256SUMS`, checks it and runs `gh release create --draft --verify-tag` with every archive, `.sha256` file and `SHA256SUMS`. A tag with a `-` in it becomes a pre-release. Only this job has `contents: write`.

### Artifacts

| Platform | File | Contents |
| --- | --- | --- |
| Windows x86-64 | `atomik-rack-<version>-windows-x86_64.zip` | folder with the four `.exe` files, `LICENSE`, `README.md`, `CHANGELOG.md` |
| macOS (Apple Silicon and Intel) | `atomik-rack-<version>-macos-universal.zip` | folder with `Atomik Rack.app`, `LICENSE`, `README.md`, `CHANGELOG.md` |
| Linux x86-64 | `atomik-rack-<version>-linux-x86_64.tar.gz` | folder with the four programs, `LICENSE`, `README.md`, `CHANGELOG.md` |

`<version>` is the tag (for example `v0.1.0`) or `dev-<shortsha>`. Each archive has a `.sha256` file; the release also has one `SHA256SUMS` over all archives.

`Atomik Rack.app` holds all four programs in `Contents/MacOS/`, so the app finds `atomik-plugin-scan` beside its executable. `Contents/Info.plist` sets the identifier `com.atomikfactory.atomik-rack`, the name `Atomik Rack`, the executable `atomik`, the version from `Cargo.toml` (without any pre-release suffix), `NSHighResolutionCapable`, a microphone usage text for audio input, and `LSMinimumSystemVersion`, which `package-macos.sh` reads from the built binaries (the higher of the two architectures' deployment targets, Rust's default). The bundle is signed ad hoc (`codesign -s -`), which is enough for Apple Silicon to run it but is not a developer signature. There is no icon yet.

### Smoke checks

Run on the packaged archive of each platform, with no window opened:

- `atomik --version`, `atomik --help`, `atomik-render --version`, `atomik-play --version` and `--help` succeed.
- `atomik-render examples/projects/demo.atomik -o smoke.wav` writes a WAV that has samples and a peak above 0.001 (not silent).
- Linux: `ldd atomik` is printed and must not report a missing library.
- macOS: the programs are run from inside the `.app`, `codesign --verify --deep --strict` passes, and the `Info.plist`, architectures (`lipo -archs`) and linked libraries are printed.

The checks do not start the graphical application, open an audio device or scan plugins: the runners have no display, GPU or sound card.

### Signing status

The binaries are not code-signed or notarised. The macOS app has only the ad-hoc signature described above, and macOS Gatekeeper and Windows SmartScreen warn on first launch; the README's Install section explains the first-run steps and the release notes link to it. There are no signing secrets in the repository.

To build by hand, follow the build instructions in the [README](../README.md) and run `cargo build --release --locked --workspace`.

## ASIO

The ASIO backend is an opt-in build feature (`asio`) that needs the Steinberg ASIO SDK. The SDK is never bundled, committed or downloaded by CI, and official release binaries are built without it. Anyone who wants ASIO builds from source with their own copy of the SDK, under the SDK's own licence terms.

## Checksums

Every archive is published with a `.sha256` file, and `SHA256SUMS` covers all of them. Users verify a download with `sha256sum -c SHA256SUMS --ignore-missing` or `sha256sum -c <archive>.sha256` (Linux), `shasum -a 256 -c <archive>.sha256` (macOS) or `Get-FileHash -Algorithm SHA256 <archive>` (Windows, compare by eye).

## After the release

- Check that the published release shows the expected files.
- Open a pull request to keep `[Unreleased]` in the changelog current, if it was not already.
