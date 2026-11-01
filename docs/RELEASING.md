# Releasing

How a release of Atomik Rack is prepared, tagged and built.

## Versioning

The project follows [Semantic Versioning](https://semver.org/) with the usual pre-1.0 caveat: while the major version is 0, any release may change APIs and the patch and project file formats. Bump the minor version for new features or breaking changes and the patch version for fixes only. Project files carry their own format version; older versions must keep loading, and the changelog says when a format version changes.

The version lives in one place, `[workspace.package]` in the root `Cargo.toml`; every crate inherits it. No crate is published to crates.io (`publish = false`).

## Checklist

1. Make sure `main` is green: formatting, clippy, tests on all three operating systems, `cargo deny check`, the coverage gate and `sh scripts/check-public.sh` (see [TESTING.md](TESTING.md)).
2. Run the manual and hardware checks that apply to the release (see [TESTING.md](TESTING.md)).
3. Signing secrets present: open Settings, Environments, `release-signing` and check that the secrets of every platform you want signed are there (see [Signing](#signing)). With none the release is still built, unsigned, and the release notes say so. Check that the certificates have not expired.
4. Update [CHANGELOG.md](../CHANGELOG.md): move the entries under `[Unreleased]` into a new `[X.Y.Z]` section dated with the release date, keep the Keep a Changelog headings, and start a fresh empty `[Unreleased]`.
5. Bump `version` in `[workspace.package]` in `Cargo.toml` and refresh the lock file with `cargo update --workspace` (this only rewrites the workspace crate versions).
6. Open a pull request with those two changes, get it reviewed and merged.
7. Tag the merge commit on `main` and push the tag:

   ```sh
   git tag -s vX.Y.Z -m "Atomik Rack vX.Y.Z"   # use -a instead of -s for an unsigned annotated tag
   git push origin vX.Y.Z
   ```

   Signed tags are optional; annotated tags are the minimum. Tags are named `vX.Y.Z` (pre-releases `vX.Y.Z-rc.1`).
8. The `Release` workflow builds, signs (where the secrets exist) and smoke-tests the artifacts and creates a **draft** GitHub release (it fails early if the tag does not match the `Cargo.toml` version). Download the artifacts, check that they start on real machines (the smoke test cannot open a window or play sound), that the checksums match and, for a signed release, that the signatures verify (see [Verifying a download](#verifying-a-download)), paste the changelog section into the release notes, generate the [third-party notices](#third-party-notices) and attach them, then publish the draft.

## Binaries

The `Release` workflow (`.github/workflows/release.yml`) runs on a `vX.Y.Z` tag (pre-releases `vX.Y.Z-rc.1`) and can be started by hand (`gh workflow run release.yml --ref <branch>`). A manual run builds, signs and smoke-tests everything and keeps the artifacts as workflow artifacts; it never creates a release. The build, signing, packaging and smoke-test logic lives in one reusable workflow, `.github/workflows/build.yml`, which the `Release` and the `Nightly` workflows both call, and in `scripts/release/`, so it reads the same locally as in CI. The packaging scripts can be run by hand on the matching operating system (`sh scripts/release/package-deb.sh <label>` and so on; see the header of each script).

Jobs of `release.yml`:

1. `version` resolves the version label: the tag name on a tag, `dev-<shortsha>` otherwise. On a tag it fails unless the tag is `v` plus the `[workspace.package]` version in `Cargo.toml` (`scripts/release/version.sh`).
2. `build` calls `build.yml` (below).
3. `draft-release` (tags only, after `build`) checks `SHA256SUMS` and runs `gh release create --draft --verify-tag` with every artifact, its `.sha256`, `SHA256SUMS` and `SHA256SUMS.asc` when it exists. A tag with a `-` in it becomes a pre-release. The release notes say which platforms are signed. Only this job has `contents: write`, and it holds no signing secret.

Jobs of `build.yml`:

1. `build` (Linux, Windows, macOS) builds `atomik`, `atomik-render`, `atomik-play`, `atomik-plugin-scan` and `atomik-plugin-host` (the plugin sandbox child, package `atomik-plugin-hostd`) with `cargo build --release --locked`. The ASIO feature is never enabled. On macOS it builds `aarch64-apple-darwin` and `x86_64-apple-darwin`. The programs are handed to the next jobs as a short-lived workflow artifact (`scripts/release/stage-raw.sh`). This job has no secrets.
2. `package-linux`, `package-windows` and `package-macos` package each platform. The Windows and macOS jobs carry the `release-signing` environment, so they are the only places where the signing secrets exist, and they build nothing.
3. `smoke` installs or unpacks every artifact on its own platform and runs the checks below (`scripts/release/smoke.sh`). Where signing was configured it also insists on valid signatures.
4. `checksums` writes `SHA256SUMS` over every artifact, signs it with GPG when a key exists (`SHA256SUMS.asc`) and uploads both.

### Artifacts

| Platform | File | Contents |
| --- | --- | --- |
| Windows x86-64 | `atomik-rack-<version>-windows-x86_64-setup.exe` | Inno Setup installer: the five programs in `Program Files\Atomik Rack`, a Start menu shortcut, an optional desktop icon, an uninstaller, the `.atomik` file association (a task, on by default) and the licence page. Per machine by default, per user on request. |
| Windows x86-64 | `atomik-rack-<version>-windows-x86_64.zip` | folder with the five `.exe` files, `LICENSE`, `README.md`, `CHANGELOG.md` |
| macOS (Apple Silicon and Intel) | `atomik-rack-<version>-macos-universal.dmg` | disk image with `Atomik Rack.app` and an Applications link |
| macOS (Apple Silicon and Intel) | `atomik-rack-<version>-macos-universal.zip` | folder with `Atomik Rack.app`, `LICENSE`, `README.md`, `CHANGELOG.md` |
| Debian, Ubuntu (x86-64) | `atomik-rack_<version>_amd64.deb` | package `atomik-rack`: the programs in `/opt/atomik-rack`, links in `/usr/bin`, a desktop entry and the `.atomik` file type |
| Linux x86-64 | `atomik-rack-<version>-linux-x86_64.tar.gz` | folder with the five programs, `LICENSE`, `README.md`, `CHANGELOG.md` |

`<version>` is the tag (for example `v0.1.0`; the `.deb` name leaves out the `v`), `dev-<shortsha>` for a manual run, or `nightly-<YYYYMMDD>-<shortsha>` for a nightly. Every artifact has a `.sha256` file; the release also has one `SHA256SUMS` over all of them (and `SHA256SUMS.asc`, a GPG signature of it, when signed).

Inside the packages the versions are: the installer shows `0.1.0` (`0.1.0-rc.1` for a release candidate, `0.1.0-nightly.20261024+abc1234` for a nightly) and carries the numeric file version `0.1.0.0`; the Debian package version turns the pre-release marker into a tilde so that it sorts before the release (`0.1.0~rc.1`, `0.1.0~nightly20261024`). `scripts/release/version-info.sh` derives all of them from the label.

`Atomik Rack.app` holds all five programs in `Contents/MacOS/`, so the app finds `atomik-plugin-scan` and `atomik-plugin-host` beside its executable. `Contents/Info.plist` sets the identifier `com.atomikfactory.atomik-rack`, the name `Atomik Rack`, the executable `atomik`, the version from `Cargo.toml` (without any pre-release suffix), `NSHighResolutionCapable`, a microphone usage text for audio input, `CFBundleIconFile` (the icon, `Contents/Resources/atomik-rack.icns`), and `LSMinimumSystemVersion`, which `package-macos.sh` reads from the built binaries (the higher of the two architectures' deployment targets, Rust's default).

The `.deb` puts the real files in `/opt/atomik-rack` and symlinks `/usr/bin/atomik`, `atomik-render`, `atomik-play`, `atomik-plugin-scan` and `atomik-plugin-host` to them. The app looks for the scanner and the plugin host beside its own executable, resolved with `std::env::current_exe()`, which on Linux follows the symlink, so they are found in `/opt/atomik-rack` however the program is started. `Depends` is worked out from what the programs link (`scripts/release/deb-depends.sh`: the libraries they need mapped to Debian packages, and `libc6 (>= N)` from the newest glibc symbol they use), so the package installs on a system at least as new as the build machine's Ubuntu. The desktop entry says `Icon=atomik-rack` and the logo is installed in the hicolor theme (`/usr/share/icons/hicolor/{128x128,256x256,512x512}/apps/atomik-rack.png`).

### The application icon

The logo is the owner's own artwork, kept unchanged as `assets/icon/atomik-rack.png` (the master, 1254 x 1254 RGBA). Everything else in `assets/icon/` is derived from it by the `make_icons` example, which crops the master to the square around the artwork (4 % margin) and scales it with area averaging on premultiplied colour (the artwork is never redrawn or recoloured):

```sh
cargo run --release -p atomik-app --example make_icons -- assets/icon/atomik-rack.png assets/icon assets/logo
```

The icons are composited on an opaque black background, so they read on any taskbar or dock. The optional third argument writes `assets/logo/monogram.png` (transparent, the About box draws it) and `assets/logo/monogram-black.png`, both 512 pixels. It writes `atomik-rack-128.png`, `-256.png` and `-512.png`, `atomik-rack.ico` (16, 24, 32, 48, 64 and 128 pixel bitmap frames and a 256 pixel PNG frame) and `atomik-rack.icns` (PNG entries from 16 to 512 pixels, the 2x sizes included). Run it and commit the result when the master changes. Where each platform gets its icon:

| Where | How |
| --- | --- |
| Window, taskbar, Alt-Tab (Windows, X11) | `assets/icon/atomik-rack-128.png` is embedded with `include_bytes!` (`crates/app/src/app_icon.rs`) and set on the main window and the detached Rack and Mixer windows. macOS and some Wayland compositors ignore window icons. |
| `atomik.exe` in Explorer, the Start menu, the taskbar pin | `crates/app/build.rs` embeds `atomik-rack.ico` (resource `crates/app/windows/atomik.rc`) with the `embed-resource` crate, a Windows-only build dependency that needs the resource compiler of the Windows SDK (installed with the MSVC build tools). Other platforms skip it. |
| Windows installer | `SetupIconFile` in `scripts/release/windows/atomik-rack.iss` (`atomik-rack.ico`); the shortcuts and the Apps list use the icon of `atomik.exe`. |
| macOS | `package-macos.sh` copies `atomik-rack.icns` into `Contents/Resources` and `Info.plist` names it with `CFBundleIconFile`. |
| Debian, Ubuntu | `package-deb.sh` installs the 128, 256 and 512 pixel PNGs in `/usr/share/icons/hicolor/<size>x<size>/apps/atomik-rack.png`; the desktop entry has `Icon=atomik-rack`. |
| README | `assets/icon/atomik-rack.png` |

### Smoke checks

Run on every packaged artifact of each platform, with no window opened:

- `atomik --version`, `atomik --help`, `atomik-render --version`, `atomik-play --version` and `--help` succeed.
- `atomik-render examples/projects/demo.atomik -o smoke.wav` writes a WAV that has samples and a peak above 0.001 (not silent).
- Linux: `ldd atomik` is printed and must not report a missing library. The `.deb` is inspected (`dpkg-deb --info` and `--contents`), installed with `apt-get install ./file.deb` (which resolves its dependencies), the `/usr/bin` links must resolve into `/opt/atomik-rack`, the programs run from `/usr/bin`, and `apt-get remove` must leave no file behind.
- Windows: the installer runs silently for the current user into a temporary folder; the five programs, the licence files, the uninstaller, the Start menu shortcut and the `.atomik` association must exist, the programs run from there, and the silent uninstall must remove them.
- macOS: the programs are run from inside the `.app` (from the zip, and from the mounted `.dmg`, which must also hold the Applications link), `codesign --verify --deep --strict` passes, and the `Info.plist`, architectures (`lipo -archs`) and linked libraries are printed.
- When signing was configured for the platform the checks insist on it: Windows runs `signtool verify /pa` on the installer and the installed programs; macOS runs `spctl --assess` on the app and the disk image and `stapler validate` on both.

The checks do not start the graphical application, open an audio device or scan plugins: the runners have no display, GPU or sound card.

## Third-party notices

Help > About Atomik Rack shows the version, the GPL-3.0-or-later notice (the `LICENSE` file that every package carries) and says that the licences of the Rust libraries a build uses are collected for each release and attached to the release page. They are generated at release time and not kept in the repository, with the `cargo-about` subcommand, which reads the licence of every crate in `Cargo.lock`:

```sh
cargo install --locked cargo-about
cargo about init        # once, in a scratch copy: writes about.toml and about.hbs to adapt
cargo about generate --workspace --locked about.hbs > THIRD-PARTY-NOTICES.html
```

The licences it accepts must be the ones `deny.toml` allows, so a licence that passes `cargo deny check` is never refused here. Attach `THIRD-PARTY-NOTICES.html` to the draft release by hand (`gh release upload vX.Y.Z THIRD-PARTY-NOTICES.html`); the packaging scripts do not include it yet, so the About box points to the release page and not to a file next to the program. The About text is `AboutBox::lines` in `crates/app/src/about_box.rs`: it names the project and its public repository and nothing else, so a release needs no change to it.

## Signing

Signing is wired for every platform and each step is skipped, with a notice in the log, until its secrets exist; the build never fails because a secret is missing. As soon as the secrets are added, the next build signs. Once the secrets of a platform exist, anything that goes wrong in its signing chain fails the build.

### The `release-signing` environment

All secrets and the Azure variables live in one protected GitHub environment named `release-signing`, not in the repository secrets. You create it once (Settings, Environments, New environment):

1. Add the secrets and variables below to the environment. Only the jobs that name the environment can read them, and each secret is handed only to the step that needs it.
2. Under Deployment branches and tags, allow only `main` and the tag pattern `v*`, so no other branch or tag can use the secrets. (The manual runs from a feature branch then fail at the environment: run them from `main`, or leave this rule off while testing.)
3. Required reviewers are optional. With reviewers every run that reaches a signing job, nightlies included, waits for an approval; leave them off to keep the nightly unattended.

The workflows that read the environment (`release.yml`, `nightly.yml`) start only from a tag push, the schedule or a manual run. Pull requests and forks never run them, so the secrets are never exposed to code from a pull request. The CI workflow does not reference the environment. If the environment does not exist or is empty, GitHub creates it on first use and the signing steps are skipped.

Secrets are never printed: the scripts decode them into private temporary files or a temporary keychain that are deleted at the end of the step, and the Windows password is never put on a command line.

### macOS (Developer ID and notarisation)

Needs an Apple Developer Program membership.

| Name | Kind | Value |
| --- | --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | secret | the Developer ID Application certificate with its private key, exported as `.p12` and base64-encoded |
| `APPLE_CERTIFICATE_PASSWORD` | secret | the password you chose when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | secret | the identity name, for example `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_NOTARY_KEY_BASE64` | secret | the App Store Connect API key (`AuthKey_<id>.p8`), base64-encoded |
| `APPLE_NOTARY_KEY_ID` | secret | the key ID of that API key |
| `APPLE_NOTARY_ISSUER_ID` | secret | the issuer ID shown above the keys list in App Store Connect |

How to get them:

1. In the Apple Developer account, Certificates, create a **Developer ID Application** certificate from a certificate signing request made in Keychain Access, download it and double-click it to install it in your keychain.
2. In Keychain Access, find the certificate with its private key, export both as a `.p12` and set a password. Encode it: `base64 -i certificate.p12 | pbcopy`. The identity name is printed by `security find-identity -v -p codesigning`.
3. In App Store Connect, Users and Access, Integrations, App Store Connect API, create a key with the Developer role, download the `.p8` once (it cannot be downloaded again) and encode it: `base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy`. The key ID is in the list and the issuer ID above it.

What the build does when all three certificate secrets exist: imports the certificate into a temporary keychain (created and deleted in the job), signs every Mach-O file of `Atomik Rack.app` inside-out with the hardened runtime and a secure timestamp and the entitlements below, then, when the three notary secrets also exist, notarises the app with `xcrun notarytool submit --wait` and staples it, builds the disk image from the stapled app, signs it, notarises it and staples it, and checks the result with `codesign --verify --deep --strict`, `spctl --assess` and `stapler validate`. The zip contains the stapled app too. With the certificate but no notary key the app is signed and not notarised (Gatekeeper still warns). With neither, the app keeps the ad-hoc signature (`codesign -s -`) that Apple Silicon needs.

The entitlements are in `scripts/release/macos/entitlements.plist` and apply to the app and its programs. Atomik Rack loads third-party CLAP and VST3 plugins into its own process, into the plugin scanner and into the plugin host process. Plugins are signed by other developers or not at all, and many generate code at run time, so the hardened runtime needs `com.apple.security.cs.disable-library-validation` (load libraries that another developer signed), `com.apple.security.cs.allow-unsigned-executable-memory` and `com.apple.security.cs.allow-jit` (code a plugin generates at run time); without them macOS refuses to load most plugins in a notarised app. `com.apple.security.device.audio-input` lets the hardened app record from an audio interface (with the usage text already in `Info.plist`). The app does not use the App Sandbox.

### Windows (Authenticode)

Two ways, chosen automatically; Azure wins when both are configured.

**A certificate file** (any code-signing certificate you can export as a `.pfx`):

| Name | Kind | Value |
| --- | --- | --- |
| `WINDOWS_CERTIFICATE_PFX_BASE64` | secret | the `.pfx` file, base64-encoded |
| `WINDOWS_CERTIFICATE_PASSWORD` | secret | its password |

Encode it in PowerShell: `[Convert]::ToBase64String([IO.File]::ReadAllBytes("certificate.pfx")) | Set-Clipboard`. Note that certificate authorities now issue most code-signing certificates only on a hardware token or in a cloud HSM, from which no `.pfx` can be exported; for those use Azure.

The build imports the certificate into the runner user's store for the step only, signs with `signtool sign /fd SHA256` and an RFC 3161 timestamp from `http://timestamp.digicert.com`, verifies with `signtool verify /pa` and removes the certificate and the temporary file.

**Azure Trusted Signing** (now called Artifact Signing; no certificate to guard, Microsoft issues short-lived ones):

| Name | Kind | Value |
| --- | --- | --- |
| `AZURE_TRUSTED_SIGNING_ENDPOINT` | variable | the regional endpoint of the signing account, for example `https://eus.codesigning.azure.net` |
| `AZURE_TRUSTED_SIGNING_ACCOUNT` | variable | the signing account name |
| `AZURE_TRUSTED_SIGNING_PROFILE` | variable | the certificate profile name |
| `AZURE_TENANT_ID` | secret | the directory (tenant) ID |
| `AZURE_CLIENT_ID` | secret | the application (client) ID of an app registration |
| `AZURE_CLIENT_SECRET` | secret | a client secret of that app registration |

Create the signing account, complete its identity validation and create a public-trust certificate profile in the Azure portal, register an application, add a client secret and give the application the role "Trusted Signing Certificate Profile Signer" (shown as "Artifact Signing Certificate Profile Signer" in newer portals) on the account. The workflow uses `Azure/artifact-signing-action` (the new name of `azure/trusted-signing-action`) pinned to a commit like the other actions, with a SHA-256 digest and Microsoft's timestamp service; Dependabot keeps the pin current.

Both ways sign the five programs before they go into the installer and the zip, then the installer itself (the uninstaller that Inno Setup embeds is not signed separately), and verify every signature.

### Linux (GPG signature of `SHA256SUMS`)

| Name | Kind | Value |
| --- | --- | --- |
| `GPG_PRIVATE_KEY_BASE64` | secret | an ASCII-armoured secret key, base64-encoded |
| `GPG_PASSPHRASE` | secret | its passphrase |

Create and export a signing key:

```sh
gpg --quick-generate-key "Atomik Rack releases <address>" ed25519 sign 2y
gpg --armor --export-secret-keys <KEYID> | base64 -w0     # GPG_PRIVATE_KEY_BASE64 (macOS: base64 without -w0)
gpg --armor --export <KEYID> > atomik-rack-release.asc    # the public key: publish it so people can verify
```

The build writes a detached, armoured signature `SHA256SUMS.asc` next to `SHA256SUMS` (the `.deb` and the `.tar.gz` are covered by it). There is no apt repository.

### Signing status

A build is signed only where the secrets exist, and the release notes (and the nightly notes) say which platforms are signed. Until the secrets are added the binaries are not code-signed or notarised: the macOS app has only the ad-hoc signature described above, and macOS Gatekeeper and Windows SmartScreen warn on first launch; the README's Install section explains the first-run steps. There are no signing secrets in the repository, and no certificate exists yet.

### Verifying a download

```sh
# Checksums (every platform's files are in SHA256SUMS)
sha256sum -c SHA256SUMS --ignore-missing           # Linux; macOS: shasum -a 256 -c

# The signature of SHA256SUMS (Linux, or anywhere with GnuPG), with the published public key
gpg --import atomik-rack-release.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
```

```powershell
# Windows: the installer and the programs (signtool is part of the Windows SDK)
signtool verify /pa /v atomik-rack-<version>-windows-x86_64-setup.exe
Get-AuthenticodeSignature atomik-rack-<version>-windows-x86_64-setup.exe | Format-List
```

```sh
# macOS: the app (from the dmg or the zip) and the disk image
codesign --verify --deep --strict --verbose=2 "Atomik Rack.app"
spctl --assess --type execute --verbose=2 "Atomik Rack.app"
xcrun stapler validate "Atomik Rack.app"
spctl --assess --type open --context context:primary-signature --verbose=2 atomik-rack-<version>-macos-universal.dmg
xcrun stapler validate atomik-rack-<version>-macos-universal.dmg
```

## Nightly builds

The `Nightly` workflow (`.github/workflows/nightly.yml`) runs every day at 03:00 UTC and can be started by hand (`gh workflow run nightly.yml`, with `-f force=true` to build when nothing changed). It never runs in forks.

1. `changes` compares the head of `main` with the commit the `nightly` tag points to. With no new commit the run stops there (a notice says so), so a quiet repository costs almost nothing. A manual run with `force` builds anyway; a manual run from another branch builds and smoke-tests but does not publish.
2. `build` is the same `build.yml` as a release, with the label `nightly-<YYYYMMDD>-<shortsha>`, so the files are named `atomik-rack-nightly-<YYYYMMDD>-<shortsha>-<platform>.<ext>` (the `.deb` is `atomik-rack_nightly-<YYYYMMDD>-<shortsha>_amd64.deb`) and the installers show `0.1.0-nightly.<YYYYMMDD>+<shortsha>`. It signs exactly where the `release-signing` secrets exist. Its workflow artifacts are kept for three days.
3. `publish` (the only job with `contents: write`) updates **one rolling pre-release** tagged `nightly`: it deletes the old assets, uploads the new ones, moves the tag to the new commit and rewrites the notes (the commit range since the previous nightly, which platforms are signed and a warning when they are not). It is never the "latest" release. A newer run cancels one that is still going; the tag moves last, so an interrupted publish is simply redone by the next run.

Nightlies have no stable tag of their own: link to `https://github.com/atomikfactory/atomik-rack/releases/tag/nightly`. GitHub stops scheduled workflows in a repository that has had no activity for 60 days; re-enable the workflow in the Actions tab if that happens.

To build by hand, follow the build instructions in the [README](../README.md) and run `cargo build --release --locked --workspace`.

## ASIO

The ASIO backend is an opt-in build feature (`asio`) that needs the Steinberg ASIO SDK. The SDK is never bundled, committed or downloaded by CI, and official release binaries are built without it. Whether that should change is an open question in the [decision log](DECISIONS.md#b-asio-sdk-licensing-for-official-builds). Anyone who wants ASIO builds from source with their own copy of the SDK, under the SDK's own licence terms.

## Checksums

Every artifact is published with a `.sha256` file, and `SHA256SUMS` covers all of them (with `SHA256SUMS.asc` when signed). Users verify a download with `sha256sum -c SHA256SUMS --ignore-missing` or `sha256sum -c <file>.sha256` (Linux), `shasum -a 256 -c <file>.sha256` (macOS) or `Get-FileHash -Algorithm SHA256 <file>` (Windows, compare by eye). See [Verifying a download](#verifying-a-download) for the signatures.

## After the release

- Check that the published release shows the expected files.
- Open a pull request to keep `[Unreleased]` in the changelog current, if it was not already.
